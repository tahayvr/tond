// handlers/certificates.rs — TLS certificate status and uploaded certificates
//
//   list_certs()   GET    /api/certs                  — every certificate, with what it serves
//   retry_cert()   POST   /api/certs/{domain}/retry   — try issuance now instead of after backoff
//   upload_cert()  POST   /api/certs/custom           — serve an uploaded certificate for a domain
//   delete_custom_cert() DELETE /api/certs/custom/{domain} — stop serving it (ACME takes over)
//
// tond-proxy obtains and renews certificates; tond-server stores the results
// it reports over the control socket (proxy_link.rs). The list joins each row
// with what currently uses the domain (dashboard, an app, a preview, a
// wildcard, or nothing), and adds served domains the proxy has not reported
// on yet — including the apps / preview wildcards once a DNS provider covers
// them — so the Certificates page shows the whole picture. Hosts covered by
// an active wildcard (or uploaded) certificate need no certificate of their
// own and are not listed as waiting.
//
// Uploaded certificates: tond-server doesn't parse X.509 (no OpenSSL here);
// it hands the PEMs to tond-proxy, which validates them (the key matches the
// leaf, the chain is in order, not expired, covers the domain), writes them
// to the certs directory it owns (key 0600) and serves them. The row has
// cert_type 'custom': never renewed or overwritten by ACME, expiry alerts
// still apply.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use sqlx::SqlitePool;
use tond_common::dns::wildcard_covers;
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::auth::{Authz, RateLimit, scope};
use crate::domains::{is_valid_hostname, normalize_domain};
use crate::error::AppError;
use crate::proxy_link;
use crate::state::AppState;

/// Largest PEM accepted for an upload (a long chain is a few KiB).
const MAX_PEM_BYTES: usize = 64 * 1024;

/// A domain tond currently serves, and whether tond-proxy routes it now (and
/// so should be obtaining a certificate for it).
struct Served {
    target: CertificateTarget,
    routed: bool,
}

/// The domain settings the list depends on.
#[derive(Debug, Clone, Default)]
pub struct CertContext {
    pub dashboard: String,
    pub apps: String,
    pub preview: String,
}

impl CertContext {
    pub async fn load(state: &AppState) -> Self {
        let s = state.settings.read().await;
        Self {
            dashboard: s.dashboard_domain.clone(),
            apps: s.apps_domain.clone(),
            preview: s.preview_domain.clone(),
        }
    }

    fn wildcard_purpose(&self, pattern: &str) -> &'static str {
        let base = pattern.strip_prefix("*.").unwrap_or(pattern);
        if !self.apps.is_empty() && base == self.apps {
            "apps"
        } else if !self.preview.is_empty() && base == self.preview {
            "previews"
        } else {
            "custom"
        }
    }
}

/// Every domain tond serves, keyed by domain. Mirrors tond-proxy's routing
/// rules (crates/tond-proxy/src/main.rs `reload_locked`) for `routed`.
async fn served_domains(
    db: &SqlitePool,
    dashboard_domain: &str,
) -> Result<HashMap<String, Served>, sqlx::Error> {
    let mut served = HashMap::new();

    let apps: Vec<(String, String, String, String, bool)> = sqlx::query_as(
        "SELECT a.id, a.name, d.domain, a.readiness,
                a.status = 'running' AND a.host_port IS NOT NULL
         FROM app_domains d JOIN apps a ON a.id = d.app_id",
    )
    .fetch_all(db)
    .await?;
    for (app_id, app_name, domain, readiness, running) in apps {
        served.insert(
            domain.to_lowercase(),
            Served {
                target: CertificateTarget::App { app_id, app_name },
                routed: running && readiness != "none",
            },
        );
    }

    let previews: Vec<(String, String, String, String, Option<i64>, bool)> = sqlx::query_as(
        "SELECT p.id, p.app_id, a.name, p.subdomain, p.pr_number,
                p.status = 'running' AND p.host_port IS NOT NULL AND p.expires_at > datetime('now')
         FROM preview_deployments p JOIN apps a ON a.id = p.app_id
         WHERE p.subdomain IS NOT NULL AND p.subdomain != '' AND p.status != 'expired'",
    )
    .fetch_all(db)
    .await?;
    for (preview_id, app_id, app_name, domain, pr_number, running) in previews {
        served.insert(
            domain.to_lowercase(),
            Served {
                target: CertificateTarget::Preview {
                    app_id,
                    app_name,
                    preview_id,
                    pr_number,
                },
                routed: running,
            },
        );
    }

    let dashboard = dashboard_domain.trim().to_lowercase();
    if !dashboard.is_empty() {
        served.insert(
            dashboard,
            Served {
                target: CertificateTarget::Dashboard,
                routed: true,
            },
        );
    }
    Ok(served)
}

/// Wildcards tond-proxy orders: `*.<apps domain>` and `*.<preview domain>`
/// when a DNS provider covers them (mirrors tond-proxy's issuance.rs).
async fn ordered_wildcards(db: &SqlitePool, ctx: &CertContext) -> Result<Vec<String>, sqlx::Error> {
    let providers = crate::dns_providers::list(db).await?;
    Ok([&ctx.apps, &ctx.preview]
        .into_iter()
        .filter(|base| !base.is_empty())
        .map(|base| format!("*.{base}"))
        .filter(|w| crate::dns_providers::provider_for(&providers, w).is_some())
        .collect())
}

/// Certificate rows joined with what they serve, plus routed domains that
/// have no row yet. Problems first is the dashboard's job; this is sorted by
/// domain.
pub async fn certificate_list(
    db: &SqlitePool,
    ctx: &CertContext,
) -> Result<Vec<CertificateResponse>, sqlx::Error> {
    let rows = sqlx::query_as::<_, Certificate>("SELECT * FROM certificates")
        .fetch_all(db)
        .await?;
    let mut served = served_domains(db, &ctx.dashboard).await?;
    let routed_hosts: Vec<String> = served
        .iter()
        .filter(|(_, s)| s.routed)
        .map(|(d, _)| d.clone())
        .collect();
    let hosts_covered = |pattern: &str| {
        let n = routed_hosts
            .iter()
            .filter(|h| wildcard_covers(pattern, h))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    };
    // Active wildcard / uploaded certificates cover hosts on their own.
    let covering: Vec<String> = rows
        .iter()
        .filter(|r| r.status == "active" && (r.domain.starts_with("*.") || r.cert_type == "custom"))
        .map(|r| r.domain.to_lowercase())
        .collect();

    let mut list: Vec<CertificateResponse> = rows
        .into_iter()
        .map(|row| {
            let domain = row.domain.to_lowercase();
            let target = if domain.starts_with("*.") {
                CertificateTarget::Wildcard {
                    purpose: ctx.wildcard_purpose(&domain).into(),
                    hosts: hosts_covered(&domain),
                }
            } else {
                served
                    .remove(&domain)
                    .map_or(CertificateTarget::Unused, |s| s.target)
            };
            CertificateResponse::from_row(row, target)
        })
        .collect();
    for wildcard in ordered_wildcards(db, ctx).await? {
        if !list.iter().any(|c| c.domain == wildcard) {
            let target = CertificateTarget::Wildcard {
                purpose: ctx.wildcard_purpose(&wildcard).into(),
                hosts: hosts_covered(&wildcard),
            };
            list.push(CertificateResponse::waiting(wildcard, target));
        }
    }
    list.extend(
        served
            .into_iter()
            .filter(|(_, s)| s.routed)
            .filter(|(d, _)| !covering.iter().any(|c| c == d || wildcard_covers(c, d)))
            .map(|(domain, s)| CertificateResponse::waiting(domain, s.target)),
    );
    list.sort_by(|a, b| a.domain.cmp(&b.domain));
    Ok(list)
}

// ── List Certificates ──

pub async fn list_certs(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let ctx = CertContext::load(&state).await;
    let list = certificate_list(&state.db, &ctx).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(list))))
}

// ── Retry a certificate now ──

/// POST /api/certs/{domain}/retry — ask tond-proxy to attempt issuance now
/// instead of waiting for the backoff. The failure count is kept, so repeated
/// retries of a broken domain still back off. A served domain without a row
/// yet just gets a certificate pass.
pub async fn retry_cert(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(domain): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let domain = domain.trim().to_lowercase();
    let updated = sqlx::query(
        "UPDATE certificates SET next_retry_at = NULL, updated_at = ? WHERE domain = ?",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(&domain)
    .execute(&state.db)
    .await?
    .rows_affected();

    if updated == 0 {
        let dashboard_domain = state.settings.read().await.dashboard_domain.clone();
        let served = served_domains(&state.db, &dashboard_domain).await?;
        if !served.get(&domain).is_some_and(|s| s.routed) && !domain.starts_with("*.") {
            return Err(TondError::NotFound(format!("No certificate for {domain}")).into());
        }
    }

    // Ask tond-proxy to try right away instead of at its next scheduled pass.
    tokio::spawn(proxy_link::request_certs(Some(domain.clone())));

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("cert_retry").resource("certificate", &domain, &domain),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Certificate retry queued", "domain": domain }),
        )),
    ))
}

// ── Uploaded certificates ──

/// A domain an uploaded certificate may serve: a hostname or `*.hostname`.
fn check_cert_domain(raw: &str) -> Result<String, AppError> {
    let domain = normalize_domain(raw);
    let host = domain.strip_prefix("*.").unwrap_or(&domain);
    if !is_valid_hostname(host) {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{}' isn't a valid domain (use app.example.com or *.example.com)",
            raw.trim()
        ))));
    }
    Ok(domain)
}

/// POST /api/certs/custom — validate (in tond-proxy) and serve an uploaded
/// certificate. Replaces an ACME certificate for the same domain; ACME stops
/// renewing it until the upload is removed.
pub async fn upload_cert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<UploadCertificateRequest>,
) -> Result<impl IntoResponse, AppError> {
    let domain = check_cert_domain(&req.domain)?;
    if req.cert_pem.len() > MAX_PEM_BYTES || req.key_pem.len() > MAX_PEM_BYTES {
        return Err(AppError(TondError::PayloadTooLarge(
            "Certificate and key must each be under 64 KiB of PEM".into(),
        )));
    }
    if !req.cert_pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err(AppError(TondError::InvalidInput(
            "The certificate must be PEM (-----BEGIN CERTIFICATE-----), leaf first, then the \
             intermediates"
                .into(),
        )));
    }
    if !req.key_pem.contains("PRIVATE KEY-----") {
        return Err(AppError(TondError::InvalidInput(
            "The private key must be PEM (-----BEGIN PRIVATE KEY----- or an RSA/EC key)".into(),
        )));
    }
    let installed = proxy_link::install_cert(&domain, req.cert_pem, req.key_pem)
        .await
        .map_err(|message| AppError(TondError::InvalidInput(message)))?;

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at,
                                   status, created_at, updated_at, last_error, last_attempt_at,
                                   next_retry_at, failure_count, issuer, last_renewed_at, challenge)
         VALUES (?1, ?2, 'custom', ?3, ?4, ?5, ?6, 'active', ?7, ?7, NULL, ?7, NULL, 0, ?8, ?7, NULL)
         ON CONFLICT(domain) DO UPDATE SET
            cert_type = 'custom', cert_path = excluded.cert_path, key_path = excluded.key_path,
            issued_at = excluded.issued_at, expires_at = excluded.expires_at, status = 'active',
            updated_at = excluded.updated_at, last_error = NULL, last_attempt_at = excluded.last_attempt_at,
            next_retry_at = NULL, failure_count = 0, issuer = excluded.issuer,
            last_renewed_at = excluded.last_renewed_at, challenge = NULL",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&domain)
    .bind(&installed.cert_path)
    .bind(&installed.key_path)
    .bind(&installed.issued_at)
    .bind(&installed.expires_at)
    .bind(&now)
    .bind(&installed.issuer)
    .execute(&state.db)
    .await?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("cert_upload")
            .resource("certificate", &domain, &domain)
            .detail(format!(
                "expires_at={} issuer={} names={}",
                installed.expires_at,
                installed.issuer.as_deref().unwrap_or("-"),
                installed.names.join(",")
            )),
    )
    .await;
    let ctx = CertContext::load(&state).await;
    let entry = certificate_list(&state.db, &ctx)
        .await?
        .into_iter()
        .find(|c| c.domain == domain);
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(entry))))
}

/// DELETE /api/certs/custom/{domain} — stop serving an uploaded certificate.
/// A domain Tond still serves gets an ACME certificate on the next pass.
pub async fn delete_custom_cert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(domain): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let domain = check_cert_domain(&domain)?;
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM certificates WHERE domain = ? AND cert_type = 'custom'")
            .bind(&domain)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_none() {
        return Err(TondError::NotFound(format!("No uploaded certificate for {domain}")).into());
    }
    proxy_link::remove_cert(&domain)
        .await
        .map_err(|m| AppError(TondError::Internal(m)))?;
    sqlx::query("DELETE FROM certificates WHERE domain = ?")
        .bind(&domain)
        .execute(&state.db)
        .await?;
    tokio::spawn(proxy_link::request_certs(None));
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("cert_upload_remove").resource("certificate", &domain, &domain),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "deleted": domain }))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Principal;
    use tond_common::Role;

    async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    async fn exec(db: &SqlitePool, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    #[tokio::test]
    async fn list_joins_targets_and_adds_routed_domains_without_a_row() {
        let db = db().await;
        let now = "2026-05-01T00:00:00Z";
        exec(
            &db,
            &format!(
                "INSERT INTO apps (id, name, domain, status, host_port, created_at, updated_at)
                 VALUES ('a1', 'web', 'web.example.com', 'running', 4000, '{now}', '{now}'),
                        ('a2', 'api', 'api.example.com', 'running', 4001, '{now}', '{now}'),
                        ('a3', 'worker', 'worker.example.com', 'running', 4002, '{now}', '{now}')"
            ),
        )
        .await;
        exec(&db, "UPDATE apps SET readiness = 'none' WHERE id = 'a3'").await;
        exec(
            &db,
            "INSERT INTO preview_deployments (id, app_id, slug, subdomain, host_port, status, expires_at, pr_number)
             VALUES ('p1', 'a1', 'abc', 'abc-web.preview.test', 5000, 'running', datetime('now', '+1 day'), 7)",
        )
        .await;
        exec(
            &db,
            &format!(
                "INSERT INTO certificates (id, domain, cert_path, key_path, issued_at, expires_at, status, issuer)
                 VALUES ('c1', 'web.example.com', '/c', '/k', '{now}', '2026-08-01T00:00:00Z', 'active', 'Let''s Encrypt R11'),
                        ('c2', 'old.example.com', '/c', '/k', '{now}', '2026-08-01T00:00:00Z', 'active', NULL)"
            ),
        )
        .await;

        let ctx = CertContext {
            dashboard: "Dash.Example.com".into(),
            ..Default::default()
        };
        let list = certificate_list(&db, &ctx)
            .await
            .unwrap_or_else(|e| panic!("list: {e}"));
        let by_domain = |d: &str| {
            list.iter()
                .find(|c| c.domain == d)
                .unwrap_or_else(|| panic!("{d} missing from {list:?}"))
        };

        let web = by_domain("web.example.com");
        assert_eq!(web.issuer_kind, IssuerKind::Letsencrypt);
        assert_eq!(
            web.serves,
            CertificateTarget::App {
                app_id: "a1".into(),
                app_name: "web".into()
            }
        );
        assert_eq!(
            by_domain("old.example.com").serves,
            CertificateTarget::Unused
        );

        // Routed, no row yet: listed as waiting.
        let api = by_domain("api.example.com");
        assert!(api.id.is_empty());
        assert_eq!(api.status, "pending");
        assert_eq!(
            by_domain("dash.example.com").serves,
            CertificateTarget::Dashboard
        );
        assert!(matches!(
            by_domain("abc-web.preview.test").serves,
            CertificateTarget::Preview {
                pr_number: Some(7),
                ..
            }
        ));
        // Background workers are never routed, so they need no certificate.
        assert!(!list.iter().any(|c| c.domain == "worker.example.com"));
        // Sorted by domain.
        let domains: Vec<&str> = list.iter().map(|c| c.domain.as_str()).collect();
        let mut sorted = domains.clone();
        sorted.sort();
        assert_eq!(domains, sorted);
    }
    #[tokio::test]
    async fn retry_cert_clears_next_retry_but_keeps_the_failure_count() {
        let state = AppState::for_tests().await;
        sqlx::query(
            "INSERT INTO certificates (id, domain, cert_path, key_path, issued_at, expires_at, status, next_retry_at, failure_count, last_error)
             VALUES ('c1', 'x.example.com', 'c', 'k', 'now', 'later', 'failed', '2999-01-01T00:00:00Z', 3, 'boom')",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("seed cert: {err}"));

        let response = retry_cert(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path("x.example.com".to_string()),
        )
        .await
        .unwrap_or_else(|_| panic!("retry_cert should succeed"))
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let (next, count): (Option<String>, i64) = sqlx::query_as(
            "SELECT next_retry_at, failure_count FROM certificates WHERE domain = 'x.example.com'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("read cert: {err}"));
        assert_eq!(next, None, "the proxy retries on its next pass");
        assert_eq!(count, 3, "backoff history is kept");

        let missing = retry_cert(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Path("nope.example.com".to_string()),
        )
        .await;
        assert!(missing.is_err(), "unknown domains are a 404");
    }
}
