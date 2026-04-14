// handlers/deployments.rs — deployment history and rollback
//
// GET  /api/apps/{id}/deployments  — list last 20 deployments for an app
// POST /api/apps/{id}/rollback     — roll back to the previous succeeded deployment
//
// Rollback logic:
//   1. Fetch the two most recent 'succeeded' deployments with a non-null image_tag.
//   2. Use the second entry (index 1) as the rollback target — the one before the
//      current succeeded deployment. If there is only one succeeded deployment,
//      return 400 (nothing to roll back to).
//   3. Spawn a background task calling deploy::deploy_image() with that image_tag.
//   4. Return 202 Accepted immediately.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use tond_common::*;

use crate::{
    alerts,
    audit_log::{AuthActor, write_audit_log},
    auth::{RateLimit, RequireAuth},
    deploy,
    error::AppError,
    state::AppState,
};

use super::fetch_app;

fn rollback_target_image_tag(candidates: &[Deployment]) -> Result<String, AppError> {
    if candidates.len() < 2 {
        return Err(AppError(TondError::InvalidInput(
            "No previous deployment to roll back to".to_string(),
        )));
    }

    candidates[1].image_tag.clone().ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "Rollback target is missing an image tag".to_string(),
        ))
    })
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

    let responses: Vec<DeploymentResponse> = deployments
        .into_iter()
        .map(DeploymentResponse::from)
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── Rollback App ──

pub async fn rollback_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let mut app = fetch_app(&state.db, &id).await?;

    // Find the two most recent succeeded deployments with a known image tag.
    let candidates = sqlx::query_as::<_, Deployment>(
        "SELECT * FROM deployments WHERE app_id = ? AND status = 'succeeded' AND image_tag IS NOT NULL ORDER BY created_at DESC LIMIT 2",
    )
    .bind(&app.id)
    .fetch_all(&state.db)
    .await?;

    let image_tag = rollback_target_image_tag(&candidates)?;

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
        Some(&format!("image_tag={}", image_tag)),
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(
            json!({ "message": "Rollback started", "image_tag": image_tag }),
        )),
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
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, volumes, drain_timeout, build_method, auto_deploy, preview_enabled, preview_excluded_keys, preview_env_vars, static_build, created_at, updated_at)
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
        }
    }

    #[test]
    fn rollback_target_image_tag_returns_previous_succeeded_image() {
        let candidates = vec![
            deployment("dep-current", Some("tond-frontend:current")),
            deployment("dep-previous", Some("tond-frontend:previous")),
        ];

        assert_eq!(
            rollback_target_image_tag(&candidates)
                .unwrap_or_else(|_| panic!("rollback target selection should succeed")),
            "tond-frontend:previous",
            "rollback_target_image_tag should choose the second-most-recent succeeded deployment image"
        );
    }

    #[test]
    fn rollback_target_image_tag_rejects_missing_history_or_missing_image_tag() {
        let err = rollback_target_image_tag(&[deployment(
            "dep-current",
            Some("tond-frontend:current"),
        )])
        .err()
        .unwrap_or_else(|| {
            panic!("expected rollback target selection to fail with only one succeeded deployment")
        });
        assert!(
            err.0
                .to_string()
                .contains("No previous deployment to roll back to"),
            "rollback_target_image_tag should report when there is no prior successful deployment"
        );

        let err = rollback_target_image_tag(&[
            deployment("dep-current", Some("tond-frontend:current")),
            deployment("dep-previous", None),
        ])
        .err()
        .unwrap_or_else(|| panic!("expected rollback target selection to fail when the previous deployment has no image tag"));
        assert!(
            err.0
                .to_string()
                .contains("Rollback target is missing an image tag"),
            "rollback_target_image_tag should report when the chosen rollback deployment lacks an image tag"
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
