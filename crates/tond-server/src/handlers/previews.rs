// handlers/previews.rs — Preview deployment endpoints
//
// Ephemeral preview deployments give each PR / branch its own live URL with
// an auto-expiry TTL. The proxy reads the `preview_deployments` table every
// 5 s and routes the assigned subdomain to the container's host port.
//
//   POST   /api/apps/{id}/preview
//     Build the app's image (same source as a regular deploy) and start a
//     container named `tond-preview-{slug}`. Returns a PreviewDeploymentResponse
//     with the preview URL. Runs in background; returns 202 immediately.
//     Body (optional): { "ttl_hours": 24 }
//
//   GET    /api/apps/{id}/previews
//     List all non-expired preview deployments for an app.
//
//   DELETE /api/apps/{id}/previews/{preview_id}
//     Stop and remove the preview container, mark the row as 'expired'.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use tond_common::{
    ApiResponse, CreatePreviewRequest, PreviewDeployment, PreviewDeploymentResponse, TondError,
    UpdatePreviewConfigRequest,
};

use crate::{
    auth::{RateLimit, RequireAuth},
    deploy, docker,
    error::AppError,
    state::AppState,
};

/// Fetch app helper
async fn fetch_app(db: &sqlx::SqlitePool, id_or_name: &str) -> Result<tond_common::App, AppError> {
    let app = sqlx::query_as::<_, tond_common::App>(
        "SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(id_or_name)
    .bind(id_or_name)
    .fetch_optional(db)
    .await?;

    app.ok_or_else(|| AppError(TondError::AppNotFound(id_or_name.to_string())))
}

/// Generate a short random hex slug (6 bytes = 12 hex chars).
fn random_slug() -> String {
    use tond_common::generate_raw_key;
    // generate_raw_key() returns 64 hex chars; take the first 12.
    generate_raw_key()[..12].to_string()
}

// ── POST /api/apps/{id}/preview ─────────────────────────────────────────────

/// Create a preview deployment for an app.
/// Spawns a background task that builds the image and starts the container.
/// Returns 202 immediately with the preview row (status = "building").
pub async fn create_preview(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    body: Option<Json<CreatePreviewRequest>>,
) -> Response {
    let app = match fetch_app(&state.db, &app_id).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    let body_ref = body.as_ref();
    let ttl_hours = body_ref
        .and_then(|b| b.ttl_hours)
        .unwrap_or(24)
        .clamp(1, 168); // max 7 days

    let pr_number = body_ref.and_then(|b| b.pr_number);
    let head_sha = body_ref.and_then(|b| b.head_sha.clone());
    let head_ref = body_ref.and_then(|b| b.head_ref.clone());
    let pr_url = body_ref.and_then(|b| b.pr_url.clone());

    let ttl_seconds = ttl_hours * 3600;
    let slug = random_slug();
    let now = chrono::Utc::now();
    let expires_at = (now + chrono::Duration::seconds(ttl_seconds)).to_rfc3339();
    let created_at = now.to_rfc3339();

    // Determine the subdomain using the cached dashboard_domain setting
    let dashboard_domain: Option<String> = {
        let s = state.settings.read().await;
        if s.dashboard_domain.is_empty() {
            None
        } else {
            Some(s.dashboard_domain.clone())
        }
    };

    let subdomain = dashboard_domain
        .as_ref()
        .map(|d| format!("{}-{}.{}", slug, app.name, d));

    let preview_id = uuid::Uuid::new_v4().to_string();

    // Insert 'building' row
    let insert_result = sqlx::query(
        "INSERT INTO preview_deployments
         (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at,
          pr_number, head_sha, head_ref, pr_url)
         VALUES (?, ?, ?, ?, ?, 'building', ?, ?, ?, ?, ?, ?)",
    )
    .bind(&preview_id)
    .bind(&app.id)
    .bind(&slug)
    .bind(&subdomain)
    .bind(ttl_seconds)
    .bind(&expires_at)
    .bind(&created_at)
    .bind(pr_number)
    .bind(&head_sha)
    .bind(&head_ref)
    .bind(&pr_url)
    .execute(&state.db)
    .await;

    if let Err(e) = insert_result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
        )
            .into_response();
    }

    // Fetch the just-inserted row to return it
    let mut preview = match sqlx::query_as::<_, PreviewDeployment>(
        "SELECT * FROM preview_deployments WHERE id = ?",
    )
    .bind(&preview_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(format!(
                    "Failed to fetch preview: {}",
                    e
                ))),
            )
                .into_response();
        }
    };

    let response_preview = preview.clone();
    let state_clone = state.clone();
    let app_clone = app.clone();
    tokio::spawn(async move {
        if let Err(e) = deploy::deploy_preview(&state_clone, &app_clone, &mut preview).await {
            tracing::error!("Preview deploy failed for {}: {}", app_clone.name, e);
            let _ = sqlx::query("UPDATE preview_deployments SET status = 'failed' WHERE id = ?")
                .bind(&preview.id)
                .execute(&state_clone.db)
                .await;
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(PreviewDeploymentResponse::from_preview(
            response_preview,
        ))),
    )
        .into_response()
}

// ── GET /api/apps/{id}/previews ─────────────────────────────────────────────

/// List active (non-expired) preview deployments for an app.
pub async fn list_previews(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Response {
    let app = match fetch_app(&state.db, &app_id).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    let previews = match sqlx::query_as::<_, PreviewDeployment>(
        "SELECT * FROM preview_deployments
         WHERE app_id = ? AND status != 'expired'
         ORDER BY created_at DESC",
    )
    .bind(&app.id)
    .fetch_all(&state.db)
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
            )
                .into_response();
        }
    };

    let responses: Vec<PreviewDeploymentResponse> = previews
        .into_iter()
        .map(PreviewDeploymentResponse::from_preview)
        .collect();

    (StatusCode::OK, Json(ApiResponse::ok(responses))).into_response()
}

// ── DELETE /api/apps/{id}/previews/{preview_id} ─────────────────────────────

/// Stop and remove a specific preview deployment.
pub async fn delete_preview(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path((app_id, preview_id)): Path<(String, String)>,
) -> Response {
    let app = match fetch_app(&state.db, &app_id).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    let preview = match sqlx::query_as::<_, PreviewDeployment>(
        "SELECT * FROM preview_deployments WHERE id = ? AND app_id = ?",
    )
    .bind(&preview_id)
    .bind(&app.id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::err("Preview not found")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
            )
                .into_response();
        }
    };

    // Stop and remove the container (best-effort)
    if let Some(ref container_id) = preview.container_id {
        let _ = docker::stop_container(&state.docker, container_id).await;
        let _ = docker::remove_container(&state.docker, container_id).await;
    }

    // Mark the row as expired
    let _ = sqlx::query("UPDATE preview_deployments SET status = 'expired' WHERE id = ?")
        .bind(&preview.id)
        .execute(&state.db)
        .await;

    tracing::info!("Preview {} deleted for app {}", preview.slug, app.name);

    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "deleted": preview.id,
            "slug": preview.slug,
        }))),
    )
        .into_response()
}

// ── PUT /api/apps/{id}/preview-config ────────────────────────────────────────

/// Enable or disable PR preview deployments for an app.
/// Body: `{ "preview_enabled": true }`
pub async fn update_preview_config(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(body): Json<UpdatePreviewConfigRequest>,
) -> Response {
    let app = match fetch_app(&state.db, &app_id).await {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };

    let enabled: i64 = if body.preview_enabled { 1 } else { 0 };
    let now = chrono::Utc::now().to_rfc3339();

    // Update preview_enabled
    if let Err(e) = sqlx::query("UPDATE apps SET preview_enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled)
        .bind(&now)
        .bind(&app.id)
        .execute(&state.db)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
        )
            .into_response();
    }

    // Update scoped secret fields if provided
    if let Some(ref excluded_keys) = body.preview_excluded_keys {
        let json = serde_json::to_string(excluded_keys).unwrap_or_else(|_| "[]".to_string());
        if let Err(e) =
            sqlx::query("UPDATE apps SET preview_excluded_keys = ?, updated_at = ? WHERE id = ?")
                .bind(&json)
                .bind(&now)
                .bind(&app.id)
                .execute(&state.db)
                .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
            )
                .into_response();
        }
    }

    if let Some(ref preview_env_vars) = body.preview_env_vars {
        let json = serde_json::to_string(preview_env_vars).unwrap_or_else(|_| "{}".to_string());
        if let Err(e) =
            sqlx::query("UPDATE apps SET preview_env_vars = ?, updated_at = ? WHERE id = ?")
                .bind(&json)
                .bind(&now)
                .bind(&app.id)
                .execute(&state.db)
                .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
            )
                .into_response();
        }
    }

    tracing::info!(
        "Preview deployments {} for app {}",
        if body.preview_enabled {
            "enabled"
        } else {
            "disabled"
        },
        app.name
    );

    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "preview_enabled": body.preview_enabled,
        }))),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Json,
        body::to_bytes,
        extract::{Path, State},
        http::StatusCode,
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{CreatePreviewRequest, ServerSettings, UpdatePreviewConfigRequest};

    use super::*;
    use crate::{
        auth::{RateLimiter, RequireAuth, rate_limit::RateLimit},
        state::AppState,
    };

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db).await.unwrap_or_else(|err| {
            panic!("failed to run migrations for preview handler tests: {err}")
        });

        let settings = ServerSettings {
            server_name: "tond".to_string(),
            dashboard_domain: "preview.example.com".to_string(),
        };

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(settings)),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn seed_preview_app(state: &AppState, name: &str, source_type: &str) -> tond_common::App {
        let mut app = tond_common::App::new(
            name.to_string(),
            source_type.to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        app.preview_enabled = 1;

        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_excluded_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.domain)
        .bind(&app.source_type)
        .bind(&app.source_url)
        .bind(&app.branch)
        .bind(app.container_port)
        .bind(&app.container_id)
        .bind(app.host_port)
        .bind(app.memory_limit)
        .bind(app.cpu_limit)
        .bind(&app.status)
        .bind(&app.env_vars)
        .bind(&app.health_check_path)
        .bind(app.health_check_interval)
        .bind(app.health_failure_threshold)
        .bind(app.health_consecutive_failures)
        .bind(&app.volumes)
        .bind(app.drain_timeout)
        .bind(&app.build_method)
        .bind(&app.webhook_secret)
        .bind(app.auto_deploy)
        .bind(&app.watch_branch)
        .bind(&app.deploy_key_pub)
        .bind(&app.project_id)
        .bind(&app.registry_credential_id)
        .bind(&app.github_app_id)
        .bind(&app.gitlab_token_id)
        .bind(app.preview_enabled)
        .bind(&app.preview_excluded_keys)
        .bind(&app.preview_env_vars)
        .bind(&app.dockerfile_content)
        .bind(&app.compose_content)
        .bind(&app.base_dir)
        .bind(app.static_build)
        .bind(&app.created_at)
        .bind(&app.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed preview app: {err}"));

        app
    }

    #[tokio::test]
    async fn create_preview_inserts_building_row_and_background_failure_marks_it_failed() {
        let state = test_state().await;
        let app = seed_preview_app(&state, "frontend", "compose").await;

        let response = create_preview(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path(app.id.clone()),
            Some(Json(CreatePreviewRequest {
                ttl_hours: Some(500),
                pr_number: Some(42),
                head_sha: Some("abcdef".to_string()),
                head_ref: Some("feature/login".to_string()),
                pr_url: Some("https://github.com/org/repo/pull/42".to_string()),
            })),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::ACCEPTED,
            "create_preview should accept valid preview requests"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["status"],
            serde_json::json!("building"),
            "create_preview should return the preview row in building state immediately"
        );
        assert_eq!(
            body["data"]["ttl_seconds"],
            serde_json::json!(168 * 3600),
            "create_preview should clamp TTL to the documented maximum of 168 hours"
        );
        let preview_id = body["data"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("expected preview response to contain an id"))
            .to_string();
        let subdomain = body["data"]["subdomain"]
            .as_str()
            .unwrap_or_else(|| panic!("expected preview response to contain a subdomain"));
        assert!(
            subdomain.ends_with(".preview.example.com") && subdomain.contains("-frontend."),
            "create_preview should derive a preview subdomain from slug, app name, and dashboard domain, got: {subdomain}"
        );

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let row: (String, i64, Option<i64>, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status, ttl_seconds, pr_number, head_sha, head_ref, pr_url FROM preview_deployments WHERE id = ?",
        )
        .bind(&preview_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to load preview row after background task: {err}"));
        assert_eq!(
            row.0, "failed",
            "compose previews should fail in the background deploy path without requiring Docker"
        );
        assert_eq!(
            row.1,
            168 * 3600,
            "persisted preview TTL should use the clamped maximum"
        );
        assert_eq!(row.2, Some(42), "create_preview should persist PR metadata");
        assert_eq!(
            row.3.as_deref(),
            Some("abcdef"),
            "create_preview should persist head_sha"
        );
        assert_eq!(
            row.4.as_deref(),
            Some("feature/login"),
            "create_preview should persist head_ref"
        );
        assert_eq!(
            row.5.as_deref(),
            Some("https://github.com/org/repo/pull/42"),
            "create_preview should persist pr_url"
        );
    }

    #[tokio::test]
    async fn list_previews_returns_non_expired_rows_for_app() {
        let state = test_state().await;
        let app = seed_preview_app(&state, "frontend", "git").await;
        sqlx::query(
            "INSERT INTO preview_deployments (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at, project_preview_id, pr_number, head_sha, head_ref, pr_url, comment_id, gitlab_project_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("preview-running")
        .bind(&app.id)
        .bind("slug-a")
        .bind(Some("slug-a-frontend.preview.example.com"))
        .bind(3600)
        .bind("running")
        .bind("2999-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed active preview: {err}"));
        sqlx::query(
            "INSERT INTO preview_deployments (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at, project_preview_id, pr_number, head_sha, head_ref, pr_url, comment_id, gitlab_project_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("preview-expired")
        .bind(&app.id)
        .bind("slug-b")
        .bind(Some("slug-b-frontend.preview.example.com"))
        .bind(3600)
        .bind("expired")
        .bind("2025-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed expired preview: {err}"));

        let response = list_previews(RateLimit, RequireAuth, State(state), Path(app.id)).await;

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "list_previews should succeed for existing apps"
        );
        let body = response_json(response).await;
        let previews = body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("expected list_previews data to be an array"));
        assert_eq!(
            previews.len(),
            1,
            "list_previews should exclude expired preview rows"
        );
        assert_eq!(
            previews[0]["id"],
            serde_json::json!("preview-running"),
            "list_previews should return the active preview row"
        );
    }

    #[tokio::test]
    async fn update_preview_config_persists_enable_flag_and_scoped_settings() {
        let state = test_state().await;
        let app = seed_preview_app(&state, "frontend", "git").await;

        let response = update_preview_config(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path(app.id.clone()),
            Json(UpdatePreviewConfigRequest {
                preview_enabled: false,
                preview_excluded_keys: Some(vec!["SECRET_KEY".to_string(), "TOKEN".to_string()]),
                preview_env_vars: Some(serde_json::json!({ "MODE": "preview" })),
            }),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "update_preview_config should succeed for existing apps"
        );
        let row: (i64, String, String) = sqlx::query_as(
            "SELECT preview_enabled, preview_excluded_keys, preview_env_vars FROM apps WHERE id = ?",
        )
        .bind(&app.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to read updated preview config: {err}"));
        assert_eq!(
            row.0, 0,
            "update_preview_config should persist preview_enabled as an integer flag"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&row.1)
                .unwrap_or_else(|err| panic!("failed to parse preview_excluded_keys JSON: {err}")),
            serde_json::json!(["SECRET_KEY", "TOKEN"]),
            "update_preview_config should persist preview_excluded_keys as JSON"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&row.2)
                .unwrap_or_else(|err| panic!("failed to parse preview_env_vars JSON: {err}")),
            serde_json::json!({ "MODE": "preview" }),
            "update_preview_config should persist preview_env_vars as JSON"
        );
    }

    #[tokio::test]
    async fn delete_preview_marks_preview_as_expired() {
        let state = test_state().await;
        let app = seed_preview_app(&state, "frontend", "git").await;
        sqlx::query(
            "INSERT INTO preview_deployments (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at, project_preview_id, pr_number, head_sha, head_ref, pr_url, comment_id, gitlab_project_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("preview-1")
        .bind(&app.id)
        .bind("slug-a")
        .bind(Some("slug-a-frontend.preview.example.com"))
        .bind(3600)
        .bind("running")
        .bind("2999-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed preview for deletion: {err}"));

        let response = delete_preview(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path((app.id.clone(), "preview-1".to_string())),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "delete_preview should succeed for matching preview rows"
        );
        let status: String =
            sqlx::query_scalar("SELECT status FROM preview_deployments WHERE id = ?")
                .bind("preview-1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read preview status after delete: {err}"));
        assert_eq!(
            status, "expired",
            "delete_preview should mark the preview row as expired"
        );
    }
}
