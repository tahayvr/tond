// handlers/deployments.rs — deployment history and rollback
//
// GET  /api/apps/{id}/deployments  — list last 20 deployments for an app
// POST /api/apps/{id}/rollback     — redeploy the exact image of an earlier deployment
//
// Every deployment runs its own image tag (`tond-<app>:<id>`, see
// deploy/images.rs), so rollback restores precisely what ran back then — code
// and dependencies — with the app's *current* env vars, volumes and limits.
//
// Rollback target:
//   - body `{"deployment_id": "..."}` picks one explicitly;
//   - without a body, the most recent succeeded deployment whose image differs
//     from the one running now.
// The target must have succeeded, carry a per-deployment image tag and not
// have been pruned. The deploy runs in the background; the handler returns 202.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use tond_common::*;

use crate::{
    alerts,
    audit_log::{AuthActor, write_audit_log},
    auth::{RateLimit, RequireAuth},
    deploy::{self, images},
    error::AppError,
    state::AppState,
};

use super::fetch_app;

/// POST /api/apps/{id}/rollback body (optional).
#[derive(Debug, Default, Deserialize)]
pub struct RollbackRequest {
    /// Deployment whose image to restore; default: the previous distinct image.
    pub deployment_id: Option<String>,
}

/// The body is optional: empty (or whitespace) means "previous image". Parsed
/// by hand because clients send `Content-Type: application/json` either way.
fn parse_rollback_request(body: &[u8]) -> Result<RollbackRequest, AppError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(RollbackRequest::default());
    }
    serde_json::from_slice(body).map_err(|e| {
        AppError(TondError::InvalidInput(format!(
            "Invalid rollback request body: {e}"
        )))
    })
}

/// Why a deployment can't be rolled back to, or `None` if it can.
fn rollback_blocker(app: &App, d: &Deployment, current_tag: Option<&str>) -> Option<String> {
    if app.source_type == "compose" {
        return Some("Rollback isn't available for Docker Compose apps".to_string());
    }
    if d.status != "succeeded" {
        return Some("Only successful deployments can be restored".to_string());
    }
    let Some(tag) = d.image_tag.as_deref() else {
        return Some("This deployment has no recorded image".to_string());
    };
    if !images::is_deployment_image(&app.name, tag) {
        return Some("This deployment predates per-deployment images".to_string());
    }
    if d.image_pruned {
        return Some("This deployment's image was pruned to save disk space".to_string());
    }
    if current_tag == Some(tag) {
        return Some("This image is already running".to_string());
    }
    None
}

/// Image tag of the most recent succeeded deployment — what the app runs now.
fn current_image_tag(deployments: &[Deployment]) -> Option<&str> {
    deployments
        .iter()
        .find(|d| d.status == "succeeded")
        .and_then(|d| d.image_tag.as_deref())
}

/// Pick the deployment to restore (see module docs). `deployments` is newest first.
fn pick_rollback_target<'a>(
    app: &App,
    deployments: &'a [Deployment],
    requested: Option<&str>,
) -> Result<&'a Deployment, AppError> {
    let current = current_image_tag(deployments);
    let invalid = |msg: String| AppError(TondError::InvalidInput(msg));

    if let Some(id) = requested {
        let d = deployments
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| AppError(TondError::NotFound(format!("deployment '{id}'"))))?;
        return match rollback_blocker(app, d, current) {
            None => Ok(d),
            Some(reason) => Err(invalid(format!(
                "Can't roll back to this deployment: {reason}"
            ))),
        };
    }

    if app.source_type == "compose" {
        return Err(invalid(
            "Rollback isn't available for Docker Compose apps; update the compose file and redeploy instead"
                .to_string(),
        ));
    }
    deployments
        .iter()
        .find(|d| rollback_blocker(app, d, current).is_none())
        .ok_or_else(|| {
            invalid(
                "No previous deployment to roll back to: there is no earlier successful deployment whose image is still available"
                    .to_string(),
            )
        })
}

/// Deployment rows plus rollback metadata for the API.
fn deployment_responses(app: &App, deployments: Vec<Deployment>) -> Vec<DeploymentResponse> {
    let current = current_image_tag(&deployments).map(str::to_string);
    let mut seen_current = false;
    deployments
        .into_iter()
        .map(|d| {
            let blocker = rollback_blocker(app, &d, current.as_deref());
            // Only the newest deployment running the current image is "current".
            let is_current = !seen_current
                && d.status == "succeeded"
                && d.image_tag.is_some()
                && d.image_tag == current;
            if is_current {
                seen_current = true;
            }
            let mut resp = DeploymentResponse::from(d);
            resp.is_current = is_current;
            resp.rollback_available = blocker.is_none();
            resp.rollback_unavailable_reason = blocker;
            resp
        })
        .collect()
}

// ── List Deployments ──

pub async fn list_deployments(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let deployments = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM deployments WHERE app_id = ? ORDER BY created_at DESC LIMIT 20",
    )
    .bind(&app.id)
    .fetch_all(&state.db)
    .await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(deployment_responses(&app, deployments))),
    ))
}

// ── Rollback App ──

pub async fn rollback_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let mut app = fetch_app(&state.db, &id).await?;
    let req = parse_rollback_request(&body)?;

    let deployments = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM deployments WHERE app_id = ? ORDER BY created_at DESC",
    )
    .bind(&app.id)
    .fetch_all(&state.db)
    .await?;

    let target = pick_rollback_target(&app, &deployments, req.deployment_id.as_deref())?;
    let target_id = target.id.clone();
    let image_tag = target.image_tag.clone().unwrap_or_default();

    // The image must still exist; if it vanished, record that and say so now.
    if state.docker.inspect_image(&image_tag).await.is_err() {
        let _ = sqlx::query(
            "UPDATE deployments SET image_pruned = 1 WHERE app_id = ? AND image_tag = ?",
        )
        .bind(&app.id)
        .bind(&image_tag)
        .execute(&state.db)
        .await;
        return Err(AppError(TondError::InvalidInput(format!(
            "Can't roll back: image {image_tag} is no longer on this server"
        ))));
    }

    let app_name = app.name.clone();
    let app_name_for_spawn = app_name.clone();
    let app_id_clone = app.id.clone();
    let image_tag_for_spawn = image_tag.clone();
    let actor_str = actor.0.clone();
    let state_clone = state.clone();

    tokio::spawn(async move {
        if let Err(e) = deploy::deploy_image(&state_clone, &mut app, &image_tag_for_spawn).await {
            tracing::error!("Rollback failed for {}: {}", app_name_for_spawn, e);
            let _ = sqlx::query(
                "UPDATE apps SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
            )
            .bind(&app.id)
            .execute(&state_clone.db)
            .await;
            alerts::fire_event_alerts(
                &state_clone,
                "deployment_failure",
                Some(app.id.as_str()),
                Some(app_name_for_spawn.as_str()),
                Some(&e.to_string()),
            )
            .await;
        } else {
            alerts::fire_event_alerts(
                &state_clone,
                "deployment_success",
                Some(app.id.as_str()),
                Some(app_name_for_spawn.as_str()),
                None,
            )
            .await;
        }
    });

    write_audit_log(
        &state.db,
        Some(&app_id_clone),
        Some(&app_name),
        "rollback",
        &actor_str,
        Some(&format!("deployment={} image_tag={}", target_id, image_tag)),
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(json!({
            "message": "Rollback started",
            "image_tag": image_tag,
            "deployment_id": target_id,
        }))),
    ))
}

pub async fn get_generated_dockerfile(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path((id, deployment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let dockerfile = sqlx::query_scalar::<_, Option<String>>(
        "SELECT generated_dockerfile FROM deployments WHERE id = ? AND app_id = ? LIMIT 1",
    )
    .bind(&deployment_id)
    .bind(&app.id)
    .fetch_optional(&state.db)
    .await?
    .flatten()
    .ok_or_else(|| {
        AppError(TondError::NotFound(
            "Generated Dockerfile not found".to_string(),
        ))
    })?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "deployment_id": deployment_id,
            "generated_dockerfile": dockerfile,
        }))),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{body::to_bytes, extract::State, response::IntoResponse};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db).await.unwrap_or_else(|err| {
            panic!("failed to run migrations for deployment handler tests: {err}")
        });

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn seed_app(state: &AppState, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, volumes, drain_timeout, build_method, auto_deploy, preview_enabled, preview_shared_keys, preview_env_vars, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind(id)
        .bind(name)
        .bind("git")
        .bind(3000_i64)
        .bind("running")
        .bind("{}")
        .bind("[]")
        .bind(30_i64)
        .bind("tbuild")
        .bind(0_i64)
        .bind(0_i64)
        .bind("[]")
        .bind("{}")
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app: {err}"));
    }

    fn deployment(id: &str, image_tag: Option<&str>) -> Deployment {
        Deployment {
            id: id.to_string(),
            app_id: "app-1".to_string(),
            image_tag: image_tag.map(ToString::to_string),
            status: "succeeded".to_string(),
            trigger_source: "manual".to_string(),
            generated_dockerfile: None,
            started_at: "2026-04-08T12:00:00Z".to_string(),
            finished_at: Some("2026-04-08T12:01:00Z".to_string()),
            created_at: "2026-04-08T12:00:00Z".to_string(),
            image_pruned: false,
        }
    }

    fn app(source_type: &str) -> App {
        App::new(
            "frontend".to_string(),
            source_type.to_string(),
            None,
            None,
            3000,
        )
    }

    #[test]
    fn default_rollback_target_is_the_previous_distinct_image() {
        let deployments = vec![
            deployment("dep-3", Some("tond-frontend:ccc")),
            deployment("dep-2", Some("tond-frontend:ccc")), // redeploy of same image
            deployment("dep-1", Some("tond-frontend:aaa")),
        ];
        let target = pick_rollback_target(&app("git"), &deployments, None)
            .unwrap_or_else(|e| panic!("target expected: {}", e.0));
        assert_eq!(target.id, "dep-1");
    }

    #[test]
    fn rollback_skips_failed_pruned_and_legacy_deployments() {
        let mut failed = deployment("dep-4", Some("tond-frontend:ddd"));
        failed.status = "failed".to_string();
        let mut pruned = deployment("dep-2", Some("tond-frontend:bbb"));
        pruned.image_pruned = true;
        let deployments = vec![
            failed,
            deployment("dep-3", Some("tond-frontend:ccc")),
            pruned,
            deployment("dep-legacy", Some("tond-frontend")),
            deployment("dep-1", Some("tond-frontend:aaa")),
        ];
        let target = pick_rollback_target(&app("git"), &deployments, None)
            .unwrap_or_else(|e| panic!("target expected: {}", e.0));
        assert_eq!(target.id, "dep-1");

        let err = pick_rollback_target(&app("git"), &deployments, Some("dep-2"))
            .err()
            .unwrap_or_else(|| panic!("pruned deployments can't be restored"));
        assert!(err.0.to_string().contains("pruned"), "{}", err.0);

        let err = pick_rollback_target(&app("git"), &deployments, Some("dep-3"))
            .err()
            .unwrap_or_else(|| panic!("the running image can't be 'restored'"));
        assert!(err.0.to_string().contains("already running"), "{}", err.0);

        let err = pick_rollback_target(&app("git"), &deployments, Some("missing"))
            .err()
            .unwrap_or_else(|| panic!("unknown deployments are 404"));
        assert_eq!(err.0.status_code(), 404);
    }

    #[test]
    fn rollback_reports_when_nothing_is_available() {
        let err = pick_rollback_target(
            &app("git"),
            &[deployment("dep-1", Some("tond-frontend:aaa"))],
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("a single deployment has nothing to roll back to"));
        assert!(
            err.0.to_string().contains("No previous deployment"),
            "{}",
            err.0
        );

        let err = pick_rollback_target(
            &app("compose"),
            &[
                deployment("dep-2", Some("tond-compose-frontend")),
                deployment("dep-1", Some("tond-compose-frontend")),
            ],
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("compose apps can't roll back"));
        assert!(err.0.to_string().contains("Docker Compose"), "{}", err.0);
    }

    #[test]
    fn rollback_body_is_optional() {
        assert!(parse_rollback_request(b"").is_ok_and(|r| r.deployment_id.is_none()));
        assert!(parse_rollback_request(b"  \n").is_ok_and(|r| r.deployment_id.is_none()));
        assert!(parse_rollback_request(b"{}").is_ok_and(|r| r.deployment_id.is_none()));
        assert!(
            parse_rollback_request(br#"{"deployment_id":"dep-1"}"#)
                .is_ok_and(|r| r.deployment_id.as_deref() == Some("dep-1"))
        );
        assert!(parse_rollback_request(b"{nope").is_err());
    }

    #[test]
    fn responses_mark_current_and_rollback_availability() {
        let mut pruned = deployment("dep-1", Some("tond-frontend:aaa"));
        pruned.image_pruned = true;
        let responses = deployment_responses(
            &app("git"),
            vec![
                deployment("dep-3", Some("tond-frontend:ccc")),
                deployment("dep-2", Some("tond-frontend:bbb")),
                pruned,
            ],
        );
        assert!(responses[0].is_current && !responses[0].rollback_available);
        assert!(!responses[1].is_current && responses[1].rollback_available);
        assert_eq!(responses[1].rollback_unavailable_reason, None);
        assert!(!responses[2].rollback_available && responses[2].image_pruned);
        assert!(
            responses[2]
                .rollback_unavailable_reason
                .as_deref()
                .is_some_and(|r| r.contains("pruned"))
        );
    }

    #[tokio::test]
    async fn get_generated_dockerfile_returns_persisted_content() {
        let state = test_state().await;
        seed_app(&state, "app-1", "frontend").await;
        sqlx::query(
            "INSERT INTO deployments (id, app_id, image_tag, status, trigger_source, generated_dockerfile, started_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("dep-1")
        .bind("app-1")
        .bind("tond-frontend")
        .bind("succeeded")
        .bind("manual")
        .bind("FROM node:20-alpine")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed deployment: {err}"));

        let response = get_generated_dockerfile(
            RateLimit,
            RequireAuth,
            State(state),
            Path(("app-1".to_string(), "dep-1".to_string())),
        )
        .await
        .unwrap_or_else(|_| panic!("expected generated Dockerfile endpoint to succeed"))
        .into_response();
        let body = response_json(response).await;

        assert_eq!(
            body["data"]["generated_dockerfile"],
            serde_json::Value::String("FROM node:20-alpine".to_string()),
            "generated Dockerfile endpoint should return the persisted deployment artifact"
        );
    }
}
