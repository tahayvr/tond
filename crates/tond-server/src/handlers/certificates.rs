// handlers/certificates.rs — TLS certificate status
//
//   list_certs()  GET  /api/v1/certs                 — every certificate, with what it serves
//   retry_cert()  POST /api/v1/certs/{domain}/retry  — try issuance now instead of after backoff
//
// tond-proxy obtains and renews certificates; tond-server stores the results
// it reports over the control socket (proxy_link.rs). The list joins each row
// with what currently uses the domain (dashboard, an app, a preview, or
// nothing), and adds served domains the proxy has not reported on yet, so the
// Certificates page shows the whole picture.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use sqlx::SqlitePool;
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::auth::{Authz, RateLimit, scope};
use crate::error::AppError;
use crate::proxy_link;
use crate::state::AppState;

/// A domain tond currently serves, and whether tond-proxy routes it now (and
/// so should be obtaining a certificate for it).
struct Served {
    target: CertificateTarget,
    routed: bool,
}

/// Every domain tond serves, keyed by domain. Mirrors tond-proxy's routing
/// rules (crates/tond-proxy/src/main.rs `reload_locked`) for `routed`.
async fn served_domains(
    db: &SqlitePool,
    dashboard_domain: &str,
) -> Result<HashMap<String, Served>, sqlx::Error> {
    let mut served = HashMap::new();

    let apps: Vec<(String, String, String, String, bool)> = sqlx::query_as(
        "SELECT id, name, domain, readiness,
                status = 'running' AND host_port IS NOT NULL
         FROM apps WHERE domain IS NOT NULL AND domain != ''",
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

/// Certificate rows joined with what they serve, plus routed domains that
/// have no row yet. Problems first is the dashboard's job; this is sorted by
/// domain.
pub async fn certificate_list(
    db: &SqlitePool,
    dashboard_domain: &str,
) -> Result<Vec<CertificateResponse>, sqlx::Error> {
    let rows = sqlx::query_as::<_, Certificate>("SELECT * FROM certificates")
        .fetch_all(db)
        .await?;
    let mut served = served_domains(db, dashboard_domain).await?;

    let mut list: Vec<CertificateResponse> = rows
        .into_iter()
        .map(|row| {
            let target = served
                .remove(&row.domain.to_lowercase())
                .map_or(CertificateTarget::Unused, |s| s.target);
            CertificateResponse::from_row(row, target)
        })
        .collect();
    list.extend(
        served
            .into_iter()
            .filter(|(_, s)| s.routed)
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
    let dashboard_domain = state.settings.read().await.dashboard_domain.clone();
    let list = certificate_list(&state.db, &dashboard_domain).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(list))))
}

// ── Retry a certificate now ──

/// POST /api/v1/certs/{domain}/retry — ask tond-proxy to attempt issuance now
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
        if !served.get(&domain).is_some_and(|s| s.routed) {
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

        let list = certificate_list(&db, "Dash.Example.com")
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
