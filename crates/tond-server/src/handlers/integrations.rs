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
// Secret config fields (bot tokens, webhook URLs, SMTP passwords, API tokens)
// are stored encrypted and never returned: responses list them in
// `saved_secrets`, a PATCH that omits one keeps the saved value and `null`
// clears an optional one (see `secrets::seal_integration_config`).
//
// Each channel can follow the global quiet hours, have none, or its own
// (`quiet_hours`). Responses carry the channel's delivery health
// (`last_sent_at`, `last_error`).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;
use uuid::Uuid;

use crate::audit_log::{AuditEntry, record};
use crate::{
    alerts::{self, quiet},
    auth::{Authz, RateLimit, scope},
    error::AppError,
    secrets,
    state::AppState,
};

fn validate_kind(kind: &str) -> Result<(), AppError> {
    if !INTEGRATION_KINDS.contains(&kind) {
        return Err(AppError(TondError::InvalidInput(format!(
            "kind must be one of: {}",
            INTEGRATION_KINDS.join(", ")
        ))));
    }
    Ok(())
}

/// Stored form of a channel's quiet hours (None = follow the global setting).
fn quiet_hours_json(q: &ChannelQuietHours) -> Result<Option<String>, AppError> {
    match q {
        ChannelQuietHours::Global => Ok(None),
        ChannelQuietHours::Custom(window) => {
            quiet::validate(window).map_err(|e| AppError(TondError::InvalidInput(e)))?;
            Ok(Some(serde_json::to_string(q).unwrap_or_default()))
        }
        ChannelQuietHours::Off => Ok(Some(serde_json::to_string(q).unwrap_or_default())),
    }
}

/// URLs a channel posts to must be http(s).
fn validate_urls(config: &serde_json::Value) -> Result<(), AppError> {
    for field in ["webhook_url", "server_url"] {
        if let Some(url) = config.get(field).and_then(|v| v.as_str())
            && !url.trim().is_empty()
        {
            let ok = reqwest::Url::parse(url.trim())
                .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
            if !ok {
                return Err(AppError(TondError::InvalidInput(format!(
                    "{field} must be an http:// or https:// URL"
                ))));
            }
        }
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
    _auth: Authz<scope::SettingsWrite>,
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
    _auth: Authz<scope::SettingsWrite>,
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
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateIntegrationRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.name.trim().is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "name is required".to_string(),
        )));
    }
    validate_kind(&req.kind)?;
    validate_urls(&req.config)?;
    let config_str = validate_and_seal_config(&req.kind, &req.config, None)?;
    let quiet = quiet_hours_json(&req.quiet_hours.clone().unwrap_or_default())?;

    let id = Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO integrations (id, name, kind, config, enabled, quiet_hours)
         VALUES (?, ?, ?, ?, 1, ?)",
    )
    .bind(&id)
    .bind(req.name.trim())
    .bind(&req.kind)
    .bind(&config_str)
    .bind(&quiet)
    .execute(&state.db)
    .await?;

    let row = fetch_integration(&state.db, &id).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("integration_create")
            .resource("integration", &row.id, &row.name)
            .detail(format!("kind={}", row.kind)),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(IntegrationResponse::from(row))),
    ))
}

// ── Update (PATCH — partial) ──────────────────────────────────────────────────

pub async fn update_integration(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
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
        Some(ref cfg) => {
            validate_urls(cfg)?;
            validate_and_seal_config(&row.kind, cfg, Some(&row.config))?
        }
        None => row.config,
    };
    let quiet = match &req.quiet_hours {
        Some(q) => quiet_hours_json(q)?,
        None => row.quiet_hours,
    };

    let enabled: i64 = req
        .enabled
        .map(|b| if b { 1 } else { 0 })
        .unwrap_or(row.enabled);

    sqlx::query(
        "UPDATE integrations
         SET name = ?, config = ?, enabled = ?, quiet_hours = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&name)
    .bind(&config_str)
    .bind(enabled)
    .bind(&quiet)
    .bind(&id)
    .execute(&state.db)
    .await?;

    let updated = fetch_integration(&state.db, &id).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("integration_update").resource("integration", &updated.id, &updated.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(IntegrationResponse::from(updated))),
    ))
}

// ── Delete ────────────────────────────────────────────────────────────────────

pub async fn delete_integration(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Rule links and queued deliveries go with it (ON DELETE CASCADE).
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

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("integration_delete").resource("integration", &id, &id),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": id }))),
    ))
}

// ── Test ──────────────────────────────────────────────────────────────────────

pub async fn test_integration(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch_integration(&state.db, &id).await?;
    alerts::test_channel(&state, &row.id, &row.name, &row.kind, &row.config, None)
        .await
        .map_err(|e| {
            AppError(TondError::InvalidInput(format!(
                "The test didn't arrive: {e:#}"
            )))
        })?;
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
