// handlers/project_previews.rs — Project-level preview environment endpoints
//
// A project preview spins up one ephemeral preview deployment per member app
// under a shared slug. All child subdomains use the same prefix so it is
// immediately obvious they belong together:
//   abc123-frontend.<preview_domain>
//   abc123-api.<preview_domain>
// The preview domain is required (see domains.rs); without it creating a
// project preview is refused with a 400.
//
// Routes (all require RequireAuth + RateLimit):
//   POST   /api/projects/{id}/previews
//     Create a project preview: insert one project_previews row, then for
//     every member app insert a preview_deployments row linked via
//     project_preview_id and spawn a background build. Returns 202 with the
//     ProjectPreviewResponse (status will be "building" initially).
//
//   GET    /api/projects/{id}/previews
//     List all non-expired project previews with their child deployments and
//     the aggregated status.
//
//   DELETE /api/projects/{id}/previews/{preview_id}
//     Stop all child containers, mark child rows as 'expired', delete the
//     project_previews row.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use tond_common::{
    ApiResponse, App, CreateProjectPreviewRequest, PreviewDeployment, PreviewDeploymentResponse,
    ProjectPreview, ProjectPreviewResponse, TondError, aggregate_preview_status,
};

use crate::{
    auth::{RateLimit, RequireAuth},
    deploy, docker,
    domains::{preview_host, require_preview_domain},
    error::AppError,
    state::AppState,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn fetch_project(
    db: &sqlx::SqlitePool,
    id_or_name: &str,
) -> Result<tond_common::Project, AppError> {
    let project = sqlx::query_as::<_, tond_common::Project>(
        "SELECT * FROM projects WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(id_or_name)
    .bind(id_or_name)
    .fetch_optional(db)
    .await?;

    project.ok_or_else(|| AppError(TondError::NotFound(format!("project '{}'", id_or_name))))
}

/// Load child preview_deployments for a project preview and build the response.
async fn build_project_preview_response(
    db: &sqlx::SqlitePool,
    pp: ProjectPreview,
) -> ProjectPreviewResponse {
    let children: Vec<PreviewDeployment> = sqlx::query_as(
        "SELECT * FROM preview_deployments WHERE project_preview_id = ? ORDER BY created_at ASC",
    )
    .bind(&pp.id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let statuses: Vec<String> = children.iter().map(|c| c.status.clone()).collect();
    let status = aggregate_preview_status(&statuses);

    let previews: Vec<PreviewDeploymentResponse> = children
        .into_iter()
        .map(PreviewDeploymentResponse::from_preview)
        .collect();

    ProjectPreviewResponse {
        id: pp.id,
        project_id: pp.project_id,
        slug: pp.slug,
        ttl_seconds: pp.ttl_seconds,
        status,
        expires_at: pp.expires_at,
        created_at: pp.created_at,
        previews,
    }
}

/// Generate a short random hex slug (6 bytes = 12 hex chars).
fn random_slug() -> String {
    use tond_common::generate_raw_key;
    generate_raw_key()[..12].to_string()
}

// ── POST /api/projects/{id}/previews ─────────────────────────────────────────

/// Create a project-level preview environment.
/// Inserts one `project_previews` row and one `preview_deployments` row per
/// member app (all sharing the same slug). Builds run sequentially in the
/// background (same pattern as deploy_project) to avoid port conflicts.
/// Returns 202 immediately.
pub async fn create_project_preview(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    body: Option<Json<CreateProjectPreviewRequest>>,
) -> Response {
    let project = match fetch_project(&state.db, &project_id).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let preview_domain = match require_preview_domain(&state).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };

    let ttl_hours = body
        .as_ref()
        .and_then(|b| b.ttl_hours)
        .unwrap_or(24)
        .clamp(1, 168);
    let ttl_seconds = ttl_hours * 3600;

    // Load member apps
    let apps: Vec<App> =
        match sqlx::query_as("SELECT * FROM apps WHERE project_id = ? ORDER BY created_at ASC")
            .bind(&project.id)
            .fetch_all(&state.db)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
                )
                    .into_response();
            }
        };

    if apps.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Project has no member apps")),
        )
            .into_response();
    }

    let slug = random_slug();
    let now = chrono::Utc::now();
    let expires_at = (now + chrono::Duration::seconds(ttl_seconds)).to_rfc3339();
    let created_at = now.to_rfc3339();
    let pp_id = uuid::Uuid::new_v4().to_string();

    // Insert project_previews row
    if let Err(e) = sqlx::query(
        "INSERT INTO project_previews (id, project_id, slug, ttl_seconds, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&pp_id)
    .bind(&project.id)
    .bind(&slug)
    .bind(ttl_seconds)
    .bind(&expires_at)
    .bind(&created_at)
    .execute(&state.db)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
        )
            .into_response();
    }

    // Insert one preview_deployments row per app (all linked to pp_id)
    let mut inserted_previews: Vec<PreviewDeployment> = Vec::with_capacity(apps.len());
    for app in &apps {
        let preview_id = uuid::Uuid::new_v4().to_string();
        let subdomain = preview_host(&slug, &app.name, &preview_domain);

        if let Err(e) = sqlx::query(
            "INSERT INTO preview_deployments
             (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at, project_preview_id)
             VALUES (?, ?, ?, ?, ?, 'building', ?, ?, ?)",
        )
        .bind(&preview_id)
        .bind(&app.id)
        .bind(&slug)
        .bind(&subdomain)
        .bind(ttl_seconds)
        .bind(&expires_at)
        .bind(&created_at)
        .bind(&pp_id)
        .execute(&state.db)
        .await
        {
            tracing::error!("Failed to insert preview for app {}: {}", app.name, e);
            continue;
        }

        if let Ok(p) =
            sqlx::query_as::<_, PreviewDeployment>("SELECT * FROM preview_deployments WHERE id = ?")
                .bind(&preview_id)
                .fetch_one(&state.db)
                .await
        {
            inserted_previews.push(p);
        }
    }

    // Spawn sequential background builds
    let state_clone = state.clone();
    let previews_for_task = inserted_previews.clone();
    let apps_for_task = apps.clone();
    tokio::spawn(async move {
        for (app, mut preview) in apps_for_task.into_iter().zip(previews_for_task) {
            if let Err(e) = deploy::deploy_preview(&state_clone, &app, &mut preview).await {
                tracing::error!("Project preview build failed for app {}: {}", app.name, e);
                let _ =
                    sqlx::query("UPDATE preview_deployments SET status = 'failed' WHERE id = ?")
                        .bind(&preview.id)
                        .execute(&state_clone.db)
                        .await;
            }
        }
    });

    // Fetch the project_previews row and respond
    let pp =
        match sqlx::query_as::<_, ProjectPreview>("SELECT * FROM project_previews WHERE id = ?")
            .bind(&pp_id)
            .fetch_one(&state.db)
            .await
        {
            Ok(r) => r,
            Err(_) => ProjectPreview {
                id: pp_id,
                project_id: project.id,
                slug,
                ttl_seconds,
                expires_at,
                created_at,
            },
        };

    let resp = build_project_preview_response(&state.db, pp).await;
    (StatusCode::ACCEPTED, Json(ApiResponse::ok(resp))).into_response()
}

// ── GET /api/projects/{id}/previews ──────────────────────────────────────────

/// List all non-expired project previews (with child deployment rows and
/// aggregated status) for a project.
pub async fn list_project_previews(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Response {
    let project = match fetch_project(&state.db, &project_id).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    // A project preview is "expired" only when ALL its child deployments have
    // expired. We filter by checking that at least one child is not expired.
    // Simplest approach: load all pp rows then filter in Rust.
    let rows: Vec<ProjectPreview> = match sqlx::query_as(
        "SELECT * FROM project_previews WHERE project_id = ? ORDER BY created_at DESC",
    )
    .bind(&project.id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
            )
                .into_response();
        }
    };

    let mut responses: Vec<ProjectPreviewResponse> = Vec::with_capacity(rows.len());
    for pp in rows {
        let r = build_project_preview_response(&state.db, pp).await;
        // Skip if fully expired
        if r.status != "expired" {
            responses.push(r);
        }
    }

    (StatusCode::OK, Json(ApiResponse::ok(responses))).into_response()
}

// ── DELETE /api/projects/{id}/previews/{preview_id} ──────────────────────────

/// Stop all child containers, mark child preview_deployments as 'expired',
/// and delete the project_previews row.
pub async fn delete_project_preview(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path((project_id, pp_id)): Path<(String, String)>,
) -> Response {
    let project = match fetch_project(&state.db, &project_id).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    // Verify the project preview belongs to this project
    let pp: Option<ProjectPreview> =
        match sqlx::query_as("SELECT * FROM project_previews WHERE id = ? AND project_id = ?")
            .bind(&pp_id)
            .bind(&project.id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::err(format!("DB error: {}", e))),
                )
                    .into_response();
            }
        };

    let pp = match pp {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::err("Project preview not found")),
            )
                .into_response();
        }
    };

    // Load child deployments and stop their containers (best-effort)
    let children: Vec<PreviewDeployment> =
        sqlx::query_as("SELECT * FROM preview_deployments WHERE project_preview_id = ?")
            .bind(&pp.id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    for child in &children {
        if let Some(ref container_id) = child.container_id {
            let _ = docker::stop_container(&state.docker, container_id).await;
            let _ = docker::remove_container(&state.docker, container_id).await;
        }
    }

    // Mark all child preview_deployments as expired
    let _ = sqlx::query(
        "UPDATE preview_deployments SET status = 'expired' WHERE project_preview_id = ?",
    )
    .bind(&pp.id)
    .execute(&state.db)
    .await;

    // Delete the project_previews row (children stay for audit trail with status=expired)
    let _ = sqlx::query("DELETE FROM project_previews WHERE id = ?")
        .bind(&pp.id)
        .execute(&state.db)
        .await;

    tracing::info!(
        "Project preview {} (slug {}) deleted for project {}",
        pp.id,
        pp.slug,
        project.name
    );

    (
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "deleted": pp.id,
            "slug": pp.slug,
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
    use tond_common::{CreateProjectPreviewRequest, ServerSettings};

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
            panic!("failed to run migrations for project preview tests: {err}")
        });

        let settings = ServerSettings {
            server_name: "tond".to_string(),
            dashboard_domain: "dash.example.com".to_string(),
            preview_domain: "example-previews.com".to_string(),
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

    async fn seed_project(state: &AppState, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO projects (id, name, description, env_vars, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(Option::<String>::None)
        .bind("{}")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed project: {err}"));
    }

    async fn seed_project_app(
        state: &AppState,
        id: &str,
        name: &str,
        project_id: &str,
        source_type: &str,
    ) {
        let mut app = App::new(
            name.to_string(),
            source_type.to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        app.id = id.to_string();
        app.project_id = Some(project_id.to_string());

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
        .unwrap_or_else(|err| panic!("failed to seed project app: {err}"));
    }

    #[tokio::test]
    async fn create_project_preview_rejects_projects_without_member_apps() {
        let state = test_state().await;
        seed_project(&state, "project-1", "Platform").await;

        let response = create_project_preview(
            RateLimit,
            RequireAuth,
            State(state),
            Path("project-1".to_string()),
            None,
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "create_project_preview should reject empty projects"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("Project has no member apps"),
            "create_project_preview should explain why empty projects are rejected"
        );
    }

    #[tokio::test]
    async fn create_list_and_delete_project_preview_manage_child_rows() {
        let state = test_state().await;
        seed_project(&state, "project-1", "Platform").await;
        seed_project_app(&state, "app-1", "frontend", "project-1", "compose").await;
        seed_project_app(&state, "app-2", "api", "project-1", "compose").await;

        let response = create_project_preview(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path("project-1".to_string()),
            Some(Json(CreateProjectPreviewRequest { ttl_hours: Some(2) })),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::ACCEPTED,
            "create_project_preview should accept valid project preview requests"
        );
        let body = response_json(response).await;
        let project_preview_id = body["data"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("expected project preview response to contain an id"))
            .to_string();
        let previews = body["data"]["previews"].as_array().unwrap_or_else(|| {
            panic!("expected project preview response to include child previews")
        });
        assert_eq!(
            previews.len(),
            2,
            "create_project_preview should create one child preview per member app"
        );
        let slugs: std::collections::HashSet<_> = previews
            .iter()
            .map(|p| p["slug"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            slugs.len(),
            1,
            "all child previews in a project preview should share the same slug"
        );

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let failed_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM preview_deployments WHERE project_preview_id = ? AND status = 'failed'",
        )
        .bind(&project_preview_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to count failed project child previews: {err}"));
        assert_eq!(
            failed_count, 2,
            "compose project previews should fail in the background deploy path without Docker"
        );

        let list_response = list_project_previews(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path("project-1".to_string()),
        )
        .await;
        assert_eq!(
            list_response.status(),
            StatusCode::OK,
            "list_project_previews should succeed for existing projects"
        );
        let list_body = response_json(list_response).await;
        let listed = list_body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("expected list_project_previews data to be an array"));
        assert_eq!(
            listed.len(),
            1,
            "list_project_previews should return the created project preview"
        );
        assert_eq!(
            listed[0]["status"],
            serde_json::json!("failed"),
            "aggregate project preview status should reflect failed child previews"
        );

        let delete_response = delete_project_preview(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path(("project-1".to_string(), project_preview_id.clone())),
        )
        .await;
        assert_eq!(
            delete_response.status(),
            StatusCode::OK,
            "delete_project_preview should succeed for existing project previews"
        );

        let child_statuses: Vec<String> = sqlx::query_scalar(
            "SELECT status FROM preview_deployments WHERE project_preview_id = ? ORDER BY id ASC",
        )
        .bind(&project_preview_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to load child preview statuses after delete: {err}"));
        assert!(
            child_statuses.iter().all(|status| status == "expired"),
            "delete_project_preview should mark all child previews as expired"
        );

        let remaining_parent: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM project_previews WHERE id = ?")
                .bind(&project_preview_id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to count remaining project preview rows: {err}")
                });
        assert_eq!(
            remaining_parent, 0,
            "delete_project_preview should remove the parent project_previews row"
        );
    }
}
