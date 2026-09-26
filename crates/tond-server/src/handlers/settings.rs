// handlers/settings.rs — GET and PUT /api/settings
//
// Settings are held in memory in AppState.settings (Arc<RwLock<ServerSettings>>)
// and persisted to the `settings` table so that tond-proxy (a separate process)
// can read them from the DB on restart.
//
//   get_settings()    GET  /api/settings — return a snapshot of the in-memory settings
//   update_settings() PUT  /api/settings — patch one or more keys; write-through to DB

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use chrono::{Duration, Utc};
use tond_common::*;
use uuid::Uuid;

use crate::{
    auth::{RateLimit, RequireAdmin, RequireAuth},
    state::AppState,
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

// ── Get Settings ──

pub async fn get_settings(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let settings = state.settings.read().await.clone();
    (StatusCode::OK, Json(ApiResponse::ok(settings))).into_response()
}

// ── Update Settings ──

pub async fn update_settings(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(req): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    // Collect the (key, value) pairs that were actually provided.
    let mut pairs: Vec<(&str, String)> = Vec::new();

    if let Some(v) = req.server_name {
        pairs.push(("server_name", v));
    }

    if let Some(v) = req.dashboard_domain {
        pairs.push(("dashboard_domain", v));
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

    // Update the in-memory cache.
    {
        let mut s = state.settings.write().await;
        for (key, value) in pairs {
            match key {
                "server_name" => s.server_name = value,
                "dashboard_domain" => s.dashboard_domain = value,
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
            RequireAdmin,
            State(state.clone()),
            Json(UpdateSettingsRequest {
                server_name: None,
                dashboard_domain: Some("dash.example.com".to_string()),
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
}
