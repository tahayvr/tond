// handlers/integrations.rs — Notification integration CRUD + test
//
// Routes:
//   GET    /api/integrations              — list all integrations
//   GET    /api/integrations/:id          — get single integration
//   POST   /api/integrations              — create integration
//   PATCH  /api/integrations/:id          — update integration (partial)
//   DELETE /api/integrations/:id          — delete integration (cascades links)
//   POST   /api/integrations/:id/test     — send a test notification
//
// Secret config fields (bot tokens, webhook URLs, SMTP passwords) are stored
// encrypted and never returned: responses list them in `saved_secrets`, and a
// PATCH that omits one keeps the saved value (see `secrets::seal_integration_config`).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;
use uuid::Uuid;

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    notifications, secrets,
    state::AppState,
};

const VALID_KINDS: &[&str] = &["telegram", "discord", "slack", "email"];

fn validate_kind(kind: &str) -> Result<(), AppError> {
    if !VALID_KINDS.contains(&kind) {
        return Err(AppError(TondError::InvalidInput(format!(
            "kind must be one of: {}",
            VALID_KINDS.join(", ")
        ))));
    }
    Ok(())
}

/// Validate the config for `kind` (merging in saved secrets from `existing`)
/// and return the stored form, secrets encrypted.
fn validate_and_seal_config(
    kind: &str,
    config: &serde_json::Value,
    existing: Option<&str>,
) -> Result<String, AppError> {
    secrets::seal_integration_config(kind, config, existing).map_err(|e| {
        AppError(TondError::InvalidInput(format!(
            "invalid config for {kind}: {e:#}"
        )))
    })
}

// ── List ──────────────────────────────────────────────────────────────────────

pub async fn list_integrations(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let rows =
        sqlx::query_as::<_, Integration>("SELECT * FROM integrations ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;

    let resp: Vec<IntegrationResponse> = rows.into_iter().map(IntegrationResponse::from).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(resp))))
}

// ── Get single ────────────────────────────────────────────────────────────────

pub async fn get_integration(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_integration(&state.db, &id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(IntegrationResponse::from(row))),
    ))
}

// ── Create ────────────────────────────────────────────────────────────────────

pub async fn create_integration(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Json(req): Json<CreateIntegrationRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.name.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name is required".to_string(),
        )));
    }
    validate_kind(&req.kind)?;
    let config_str = validate_and_seal_config(&req.kind, &req.config, None)?;

    let id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO integrations (id, name, kind, config, enabled)
         VALUES (?, ?, ?, ?, 1)",
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(&req.kind)
    .bind(&config_str)
    .execute(&state.db)
    .await?;

    let row = fetch_integration(&state.db, &id).await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(IntegrationResponse::from(row))),
    ))
}

// ── Update (PATCH — partial) ──────────────────────────────────────────────────

pub async fn update_integration(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateIntegrationRequest>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_integration(&state.db, &id).await?;

    let name = req.name.map(|n| n.trim().to_string()).unwrap_or(row.name);
    if name.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name cannot be empty".to_string(),
        )));
    }

    let config_str = match req.config {
        Some(ref cfg) => validate_and_seal_config(&row.kind, cfg, Some(&row.config))?,
        None => row.config,
    };

    let enabled: i64 = req
        .enabled
        .map(|b| if b { 1 } else { 0 })
        .unwrap_or(row.enabled);

    sqlx::query(
        "UPDATE integrations
         SET name = ?, config = ?, enabled = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&name)
    .bind(&config_str)
    .bind(enabled)
    .bind(&id)
    .execute(&state.db)
    .await?;

    let updated = fetch_integration(&state.db, &id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(IntegrationResponse::from(updated))),
    ))
}

// ── Delete ────────────────────────────────────────────────────────────────────

pub async fn delete_integration(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Remove linked alert_rule_notifications first (no FK cascade in SQLite default)
    sqlx::query("DELETE FROM alert_rule_notifications WHERE integration_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;

    let deleted = sqlx::query("DELETE FROM integrations WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected();

    if deleted == 0 {
        return Err(AppError(TondError::NotFound(format!(
            "Integration {id} not found"
        ))));
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": id }))),
    ))
}

// ── Test ──────────────────────────────────────────────────────────────────────

pub async fn test_integration(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_integration(&state.db, &id).await?;
    let config = secrets::open_integration_config(&row.kind, &row.config)
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?;

    let payload = notifications::NotificationPayload {
        rule_name: "Test".to_string(),
        event_or_metric: "test".to_string(),
        app_name: None,
        detail: Some("This is a test notification from Tond.".to_string()),
        fired_at: chrono::Utc::now().to_rfc3339(),
    };

    notifications::dispatch(&config, &payload)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("Test failed: {}", e))))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "message": "Test notification sent" }),
        )),
    ))
}

// ── Shared helper ─────────────────────────────────────────────────────────────

pub async fn fetch_integration(db: &sqlx::SqlitePool, id: &str) -> Result<Integration, AppError> {
    sqlx::query_as::<_, Integration>("SELECT * FROM integrations WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Integration {id} not found"))))
}
