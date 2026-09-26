// handlers/settings.rs — GET and PUT /api/settings
//
// Settings are held in memory in AppState.settings (Arc<RwLock<ServerSettings>>)
// and persisted to the `settings` table so that tond-proxy (a separate process)
// can read them from the DB on restart.
//
//   get_settings()    GET  /api/settings — return a snapshot of the in-memory settings
//   update_settings() PUT  /api/settings — patch one or more keys; write-through to DB
//
// upload_max_bytes caps uploaded source archives (see upload.rs); it must be
// between 1 MiB and 64 GiB.
//
// dashboard_domain, apps_domain and preview_domain are normalized (trimmed,
// lowercased) and validated together whichever of them changes: the preview
// and apps domains must each be a different registrable domain from the
// dashboard domain and from each other (see domains.rs).

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use chrono::{Duration, Utc};
use tond_common::*;
use uuid::Uuid;

use crate::{
    audit_log::{AuditEntry, record},
    auth::{Authz, Principal, RateLimit, scope},
    domains::{is_valid_hostname, normalize_domain, validate_apps_domain, validate_preview_domain},
    state::AppState,
    upload::{MAX_UPLOAD_MAX_BYTES, MIN_UPLOAD_MAX_BYTES, format_size},
};

async fn queue_dashboard_certificate(
    db: &sqlx::SqlitePool,
    dashboard_domain: &str,
    data_dir: &str,
) {
    let certs_dir = format!("{}/certs", data_dir);
    let cert_path = format!("{}/{}/cert.pem", certs_dir, dashboard_domain);
    let key_path = format!("{}/{}/key.pem", certs_dir, dashboard_domain);
    let now = Utc::now().to_rfc3339();
    let expires_at = (Utc::now() + Duration::days(90)).to_rfc3339();

    let _ = sqlx::query(
        "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at, status, created_at, updated_at)
         VALUES (?, ?, 'acme', ?, ?, ?, ?, 'pending', ?, ?)
         ON CONFLICT(domain) DO UPDATE SET status = 'pending', updated_at = excluded.updated_at,
            last_error = NULL, next_retry_at = NULL, failure_count = 0",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(dashboard_domain)
    .bind(cert_path)
    .bind(key_path)
    .bind(&now)
    .bind(&expires_at)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await;
}

fn invalid(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::err(msg))).into_response()
}

// ── Get Settings ──

pub async fn get_settings(
    _rate: RateLimit,
    _auth: Principal,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let settings = state.settings.read().await.clone();
    (StatusCode::OK, Json(ApiResponse::ok(settings))).into_response()
}

// ── Update Settings ──

pub async fn update_settings(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    // Collect the (key, value) pairs that were actually provided.
    let mut pairs: Vec<(&str, String)> = Vec::new();

    if let Some(v) = req.server_name {
        pairs.push(("server_name", v));
    }

    let current = state.settings.read().await.clone();
    let dashboard_domain = req.dashboard_domain.as_deref().map(normalize_domain);
    let preview_domain = req.preview_domain.as_deref().map(normalize_domain);
    let apps_domain = req.apps_domain.as_deref().map(normalize_domain);

    if let Some(d) = dashboard_domain.as_deref()
        && !d.is_empty()
        && !is_valid_hostname(d)
    {
        return invalid(format!("'{d}' is not a valid domain name"));
    }

    // Validate the pair as it will be after this update.
    let effective_dashboard = dashboard_domain
        .clone()
        .unwrap_or_else(|| current.dashboard_domain.clone());
    let effective_preview = preview_domain
        .clone()
        .unwrap_or_else(|| current.preview_domain.clone());
    if !effective_preview.is_empty()
        && (dashboard_domain.is_some() || preview_domain.is_some())
        && let Err(msg) = validate_preview_domain(&effective_preview, &effective_dashboard)
    {
        return invalid(msg);
    }

    let effective_apps = apps_domain
        .clone()
        .unwrap_or_else(|| current.apps_domain.clone());
    if !effective_apps.is_empty()
        && (dashboard_domain.is_some() || preview_domain.is_some() || apps_domain.is_some())
        && let Err(msg) =
            validate_apps_domain(&effective_apps, &effective_dashboard, &effective_preview)
    {
        return invalid(msg);
    }

    if let Some(v) = dashboard_domain {
        pairs.push(("dashboard_domain", v));
    }

    if let Some(v) = apps_domain {
        pairs.push(("apps_domain", v));
    }

    if let Some(v) = preview_domain {
        pairs.push(("preview_domain", v));
    }

    if let Some(max) = req.upload_max_bytes {
        if !(MIN_UPLOAD_MAX_BYTES..=MAX_UPLOAD_MAX_BYTES).contains(&max) {
            return invalid(format!(
                "The upload limit must be between {} and {}",
                format_size(MIN_UPLOAD_MAX_BYTES),
                format_size(MAX_UPLOAD_MAX_BYTES)
            ));
        }
        pairs.push(("upload_max_bytes", max.to_string()));
    }
    if let Some(hours) = req.session_idle_timeout_hours {
        if !SESSION_IDLE_HOURS_RANGE.contains(&hours) {
            return invalid(
                "The idle timeout must be between 1 hour and 720 hours (30 days).".to_string(),
            );
        }
        pairs.push(("session_idle_timeout_hours", hours.to_string()));
    }
    if let Some(days) = req.session_max_age_days {
        if !SESSION_MAX_AGE_DAYS_RANGE.contains(&days) {
            return invalid("The session lifetime must be between 1 and 365 days.".to_string());
        }
        pairs.push(("session_max_age_days", days.to_string()));
    }

    // Write-through: persist each changed key to the DB first so that
    // tond-proxy (separate process) picks it up on its next 5-second poll.
    for (key, value) in &pairs {
        let result = sqlx::query(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?, ?, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .execute(&state.db)
        .await;

        if let Err(e) = result {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    }

    if let Some((_, dashboard_domain)) = pairs
        .iter()
        .find(|(key, value)| *key == "dashboard_domain" && !value.trim().is_empty())
    {
        let data_dir =
            std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
        queue_dashboard_certificate(&state.db, dashboard_domain, &data_dir).await;
    }

    if !pairs.is_empty() {
        let changed: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new("settings_update")
                .kind("settings")
                .detail(changed.join(" ")),
        )
        .await;
    }

    // Update the in-memory cache.
    {
        let mut s = state.settings.write().await;
        for (key, value) in pairs {
            match key {
                "server_name" => s.server_name = value,
                "dashboard_domain" => s.dashboard_domain = value,
                "preview_domain" => s.preview_domain = value,
                "apps_domain" => s.apps_domain = value,
                "upload_max_bytes" => {
                    s.upload_max_bytes = value.parse().unwrap_or(s.upload_max_bytes)
                }
                "session_idle_timeout_hours" => {
                    s.session_idle_timeout_hours =
                        value.parse().unwrap_or(s.session_idle_timeout_hours)
                }
                "session_max_age_days" => {
                    s.session_max_age_days = value.parse().unwrap_or(s.session_max_age_days)
                }
                _ => {}
            }
        }
    }

    let settings = state.settings.read().await.clone();
    (StatusCode::OK, Json(ApiResponse::ok(settings))).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{Json, response::IntoResponse};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    use super::*;
    use crate::auth::RateLimiter;

    async fn test_state() -> (AppState, TempDir) {
        let data_dir = TempDir::new().unwrap_or_else(|err| {
            panic!("failed to create temp data dir for settings test: {err}")
        });

        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for settings test: {err}"));

        let state = AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        };

        (state, data_dir)
    }

    #[tokio::test]
    async fn update_settings_queues_pending_certificate_for_dashboard_domain() {
        let (state, _data_dir) = test_state().await;

        let response = update_settings(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(UpdateSettingsRequest {
                dashboard_domain: Some("dash.example.com".to_string()),
                ..Default::default()
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "settings update should succeed when queuing a dashboard certificate"
        );

        let row: (String, String, String) =
            sqlx::query_as("SELECT status, cert_path, key_path FROM certificates WHERE domain = ?")
                .bind("dash.example.com")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to load queued dashboard certificate row: {err}")
                });

        assert_eq!(
            row.0, "pending",
            "saving a dashboard domain should queue certificate provisioning as pending"
        );
        assert!(
            row.1.ends_with("/certs/dash.example.com/cert.pem"),
            "queued dashboard certificate should use the standard cert path"
        );
        assert!(
            row.2.ends_with("/certs/dash.example.com/key.pem"),
            "queued dashboard certificate should use the standard key path"
        );
    }

    async fn put(
        state: &AppState,
        dashboard_domain: Option<&str>,
        preview_domain: Option<&str>,
    ) -> axum::response::Response {
        update_settings(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(UpdateSettingsRequest {
                dashboard_domain: dashboard_domain.map(str::to_string),
                preview_domain: preview_domain.map(str::to_string),
                ..Default::default()
            }),
        )
        .await
        .into_response()
    }

    async fn stored(state: &AppState, key: &str) -> Option<String> {
        sqlx::query_as::<_, (String,)>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to read setting {key}: {err}"))
            .map(|(v,)| v)
    }

    #[tokio::test]
    async fn preview_domain_is_normalized_and_stored_when_cross_site() {
        let (state, _data_dir) = test_state().await;

        let response = put(
            &state,
            Some("dash.example.com"),
            Some(" Example-Previews.COM. "),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            stored(&state, "preview_domain").await.as_deref(),
            Some("example-previews.com"),
            "the preview domain should be persisted normalized for tond-proxy"
        );
        assert_eq!(
            state.settings.read().await.preview_domain,
            "example-previews.com",
            "the in-memory settings cache should hold the preview domain"
        );
    }

    #[tokio::test]
    async fn preview_domain_on_the_dashboard_site_is_rejected() {
        let (state, _data_dir) = test_state().await;
        assert_eq!(
            put(&state, Some("dash.example.com"), None).await.status(),
            StatusCode::OK
        );

        for preview in [
            "dash.example.com",
            "pr.dash.example.com",
            "previews.example.com",
            "example.com",
            "co.uk",
        ] {
            assert_eq!(
                put(&state, None, Some(preview)).await.status(),
                StatusCode::BAD_REQUEST,
                "preview domain {preview} must be rejected next to dash.example.com"
            );
        }
        assert_eq!(
            stored(&state, "preview_domain").await,
            None,
            "rejected preview domains must not be persisted"
        );
    }

    #[tokio::test]
    async fn dashboard_domain_change_is_checked_against_the_preview_domain() {
        let (state, _data_dir) = test_state().await;
        assert_eq!(
            put(
                &state,
                Some("dash.example.com"),
                Some("example-previews.com")
            )
            .await
            .status(),
            StatusCode::OK
        );

        assert_eq!(
            put(&state, Some("dash.example-previews.com"), None)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "moving the dashboard onto the preview domain's site must be rejected"
        );
        assert_eq!(
            stored(&state, "dashboard_domain").await.as_deref(),
            Some("dash.example.com")
        );

        assert_eq!(
            put(&state, None, Some("")).await.status(),
            StatusCode::OK,
            "clearing the preview domain is always allowed"
        );
        assert_eq!(state.settings.read().await.preview_domain, "");
    }

    #[tokio::test]
    async fn queue_dashboard_certificate_uses_supplied_data_dir() {
        let (state, data_dir) = test_state().await;

        queue_dashboard_certificate(
            &state.db,
            "dash.example.com",
            data_dir.path().to_string_lossy().as_ref(),
        )
        .await;

        let row: (String, String) =
            sqlx::query_as("SELECT cert_path, key_path FROM certificates WHERE domain = ?")
                .bind("dash.example.com")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to load queued certificate paths: {err}"));

        assert!(
            row.0.contains(data_dir.path().to_string_lossy().as_ref()),
            "helper should respect the supplied data dir for cert_path"
        );
        assert!(
            row.1.contains(data_dir.path().to_string_lossy().as_ref()),
            "helper should respect the supplied data dir for key_path"
        );
    }

    #[tokio::test]
    async fn upload_limit_is_validated_and_cached() {
        let (state, _data_dir) = test_state().await;
        let set = |max: u64| {
            let state = state.clone();
            async move {
                update_settings(
                    RateLimit,
                    Authz::unchecked(Principal::test_user(Role::Admin)),
                    State(state),
                    Json(UpdateSettingsRequest {
                        upload_max_bytes: Some(max),
                        ..Default::default()
                    }),
                )
                .await
                .into_response()
                .status()
            }
        };
        assert_eq!(
            set(1024).await,
            StatusCode::BAD_REQUEST,
            "below 1 MiB is refused"
        );
        assert_eq!(
            state.settings.read().await.upload_max_bytes,
            DEFAULT_UPLOAD_MAX_BYTES,
            "a refused limit leaves the default"
        );
        assert_eq!(set(200 * 1024 * 1024).await, StatusCode::OK);
        assert_eq!(
            state.settings.read().await.upload_max_bytes,
            200 * 1024 * 1024
        );
        assert_eq!(
            stored(&state, "upload_max_bytes").await.as_deref(),
            Some("209715200"),
            "the limit is persisted for restarts"
        );
    }

    async fn put_apps(state: &AppState, apps_domain: &str) -> axum::response::Response {
        update_settings(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(UpdateSettingsRequest {
                apps_domain: Some(apps_domain.to_string()),
                ..Default::default()
            }),
        )
        .await
        .into_response()
    }

    #[tokio::test]
    async fn apps_domain_must_be_its_own_site() {
        let (state, _data_dir) = test_state().await;
        assert_eq!(
            put(
                &state,
                Some("dash.example.com"),
                Some("example-previews.com")
            )
            .await
            .status(),
            StatusCode::OK
        );
        for bad in [
            "apps.example.com",
            "apps.dash.example.com",
            "example-previews.com",
            "x.example-previews.com",
        ] {
            assert_eq!(
                put_apps(&state, bad).await.status(),
                StatusCode::BAD_REQUEST,
                "{bad} must be rejected as the apps domain"
            );
        }
        assert_eq!(
            put_apps(&state, " Example-Apps.com ").await.status(),
            StatusCode::OK
        );
        assert_eq!(state.settings.read().await.apps_domain, "example-apps.com");
        assert_eq!(
            stored(&state, "apps_domain").await.as_deref(),
            Some("example-apps.com")
        );
        assert_eq!(
            put(&state, None, Some("previews.example-apps.com"))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "moving the preview domain onto the apps site is rejected too"
        );
        assert_eq!(
            put_apps(&state, "").await.status(),
            StatusCode::OK,
            "clearing is allowed"
        );
    }
}
