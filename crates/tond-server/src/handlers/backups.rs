// handlers/backups.rs — Snapshot API handlers + SSE progress stream
//
// Routes:
//   POST   /api/snapshots                       — trigger manual Full snapshot
//   GET    /api/snapshots                        — list snapshots (filters: scope, trigger, app)
//   DELETE /api/snapshots/{id}                   — delete record + local dir + remote object
//   GET    /api/snapshots/config                 — get backup configuration
//   PUT    /api/snapshots/config                 — update backup configuration
//   GET    /api/snapshots/{id}/logs/stream       — SSE progress stream
//   POST   /api/snapshots/{id}/restore           — Full restore (server restart)
//   POST   /api/snapshots/{id}/restore-app       — App-scope rollback (no restart)
//   POST   /api/snapshots/{id}/restore-database  — Database-scope restore (no restart)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::Event,
    response::{IntoResponse, Sse},
};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::json;
use sqlx::AssertSqlSafe;
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tond_common::*;

use crate::{
    auth::{RateLimit, RequireAdmin, RequireAuth},
    backup::{self, SnapshotTrigger},
    error::AppError,
    handlers::storage_providers::decrypt_provider_secret,
    state::AppState,
    storage,
};

// ── Create snapshot ───────────────────────────────────────────────────────────

pub async fn create_snapshot(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<impl IntoResponse, AppError> {
    let trigger = SnapshotTrigger::Manual { name: req.name };

    // Spawn in background; return the snapshot ID immediately
    let state_clone = state.clone();
    let (tx, _rx) = mpsc::channel(32);
    let id = backup::create_snapshot(&state_clone, trigger.clone(), Some(tx))
        .await
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "Failed to start snapshot: {}",
                e
            )))
        })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(SnapshotStarted {
            id,
            message: "Snapshot started".to_string(),
        })),
    ))
}

// ── List snapshots ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListSnapshotsQuery {
    pub scope: Option<String>,
    pub trigger: Option<String>,
    pub app: Option<String>,
}

pub async fn list_snapshots(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Query(query): Query<ListSnapshotsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let mut sql = "SELECT * FROM snapshots WHERE 1=1".to_string();
    let mut binds: Vec<String> = Vec::new();

    if let Some(ref scope) = query.scope {
        sql.push_str(" AND scope = ?");
        binds.push(scope.clone());
    }
    if let Some(ref trigger) = query.trigger {
        sql.push_str(" AND trigger = ?");
        binds.push(trigger.clone());
    }
    if let Some(ref app) = query.app {
        let scope_val = format!("app:{}", app);
        sql.push_str(" AND (trigger_app = ? OR scope = ?)");
        binds.push(app.clone());
        binds.push(scope_val);
    }

    sql.push_str(" ORDER BY created_at DESC");

    // `sql` is assembled only from the fixed fragments above; user input is bound via `?`.
    let mut q = sqlx::query_as::<_, Snapshot>(AssertSqlSafe(sql));
    for b in &binds {
        q = q.bind(b);
    }

    let rows = q.fetch_all(&state.db).await?;
    let resp: Vec<SnapshotResponse> = rows.into_iter().map(SnapshotResponse::from).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(resp))))
}

// ── Delete snapshot ───────────────────────────────────────────────────────────

pub async fn delete_snapshot(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let snap = fetch_snapshot_row(&state, &id).await?;

    // Delete local directory
    if let Some(ref path) = snap.local_path {
        let _ = tokio::fs::remove_dir_all(path).await;
    }

    // Delete remote object
    if let (Some(key), Some(sp_id)) = (&snap.remote_key, &snap.storage_provider_id) {
        let provider =
            sqlx::query_as::<_, StorageProvider>("SELECT * FROM storage_providers WHERE id = ?")
                .bind(sp_id)
                .fetch_optional(&state.db)
                .await?;

        if let Some(raw) = provider
            && let Ok(dec) = decrypt_provider_secret(raw)
        {
            let _ = storage::delete(&dec, key).await;
        }
    }

    sqlx::query("DELETE FROM snapshots WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "deleted": id }))),
    ))
}

// ── Get backup config ─────────────────────────────────────────────────────────

pub async fn get_backup_config(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let cfg = backup::get_backup_config(&state)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(BackupConfigResponse::from(cfg))),
    ))
}

// ── Update backup config ──────────────────────────────────────────────────────

pub async fn update_backup_config(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(req): Json<UpdateBackupConfigRequest>,
) -> Result<impl IntoResponse, AppError> {
    let current = backup::get_backup_config(&state)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    let enabled = req
        .enabled
        .map(|b| if b { 1i64 } else { 0 })
        .unwrap_or(current.enabled);
    let full_schedule_cron = req.full_schedule_cron.unwrap_or(current.full_schedule_cron);
    let full_retention_count = req
        .full_retention_count
        .unwrap_or(current.full_retention_count);
    let pre_deploy_enabled = req
        .pre_deploy_enabled
        .map(|b| if b { 1i64 } else { 0 })
        .unwrap_or(current.pre_deploy_enabled);
    let pre_deploy_retention_per_app = req
        .pre_deploy_retention_per_app
        .unwrap_or(current.pre_deploy_retention_per_app);
    let keep_local = req
        .keep_local
        .map(|b| if b { 1i64 } else { 0 })
        .unwrap_or(current.keep_local);
    // storage_provider_id: Some(id) = set/update, None = keep existing
    let storage_provider_id: Option<String> = match req.storage_provider_id {
        Some(v) => Some(v),
        None => current.storage_provider_id,
    };

    // Validate cron expression
    {
        use ::cron::Schedule;
        use std::str::FromStr;
        Schedule::from_str(&full_schedule_cron).map_err(|e| {
            AppError(TondError::InvalidInput(format!(
                "Invalid cron expression: {}",
                e
            )))
        })?;
    }

    sqlx::query(
        "UPDATE backup_config
         SET enabled = ?, full_schedule_cron = ?, full_retention_count = ?,
             pre_deploy_enabled = ?, pre_deploy_retention_per_app = ?,
             keep_local = ?, storage_provider_id = ?
         WHERE id = 1",
    )
    .bind(enabled)
    .bind(&full_schedule_cron)
    .bind(full_retention_count)
    .bind(pre_deploy_enabled)
    .bind(pre_deploy_retention_per_app)
    .bind(keep_local)
    .bind(&storage_provider_id)
    .execute(&state.db)
    .await?;

    let updated = backup::get_backup_config(&state)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(BackupConfigResponse::from(updated))),
    ))
}

// ── SSE progress stream ───────────────────────────────────────────────────────

pub async fn stream_snapshot_logs(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    type SseStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>>;

    // If snapshot is already completed or failed, return a single terminal event
    let snap = fetch_snapshot_row(&state, &id).await?;

    if snap.status == "completed" || snap.status == "failed" {
        let event = Event::default()
            .json_data(json!({
                "step": "done",
                "message": format!("Snapshot {}", snap.status),
                "done": true
            }))
            .unwrap_or_default();

        let s: SseStream = Box::pin(stream::once(async move { Ok::<Event, Infallible>(event) }));
        return Ok(Sse::new(s).keep_alive(Default::default()).into_response());
    }

    // For running snapshots, poll the DB until done
    let (tx, rx) = mpsc::channel::<Event>(32);
    let state_clone = state.clone();
    let snap_id = id.clone();

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let row: Option<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
                "SELECT status, delta_bytes, total_bytes FROM snapshots WHERE id = ?",
            )
            .bind(&snap_id)
            .fetch_optional(&state_clone.db)
            .await
            .unwrap_or(None);

            match row {
                None => break,
                Some((status, delta, total)) => {
                    if status == "completed" || status == "failed" {
                        let delta_mb = delta.unwrap_or(0) as f64 / 1_048_576.0;
                        let _ = tx
                            .send(
                                Event::default()
                                    .json_data(json!({
                                        "step": "done",
                                        "message": format!(
                                            "Snapshot {} ({:.1} MB new)",
                                            status, delta_mb
                                        ),
                                        "done": true,
                                        "delta_bytes": delta,
                                        "total_bytes": total,
                                    }))
                                    .unwrap_or_default(),
                            )
                            .await;
                        break;
                    }
                    // Still running — emit a heartbeat
                    let _ = tx
                        .send(
                            Event::default()
                                .json_data(json!({
                                    "step": "running",
                                    "message": "Snapshot in progress…",
                                    "done": false
                                }))
                                .unwrap_or_default(),
                        )
                        .await;
                }
            }
        }
    });

    let s: SseStream = Box::pin(ReceiverStream::new(rx).map(Ok::<Event, Infallible>));
    Ok(Sse::new(s).keep_alive(Default::default()).into_response())
}

// ── Full restore ──────────────────────────────────────────────────────────────

pub async fn restore_snapshot(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let pre_restore_id = backup::restore_full(&state, id)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("Restore failed: {}", e))))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(json!({
            "pre_restore_snapshot_id": pre_restore_id,
            "message": "Full restore started. The server will restart when complete. You can undo this restore using the pre_restore_snapshot_id."
        }))),
    ))
}

// ── App-scope rollback ────────────────────────────────────────────────────────

pub async fn restore_app(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreAppRequest>,
) -> Result<impl IntoResponse, AppError> {
    backup::restore_app(&state, &id, &req.app_name)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("App restore failed: {}", e))))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "message": format!(
                "{} rolled back to its state at snapshot {}. The app has been restarted.",
                req.app_name, id
            )
        }))),
    ))
}

// ── Database-scope restore ────────────────────────────────────────────────────

pub async fn restore_database(
    _rate: RateLimit,
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    backup::restore_database(&state, &id, &req.db_name)
        .await
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "Database restore failed: {}",
                e
            )))
        })?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "message": format!(
                "Database '{}' restored from snapshot {}.",
                req.db_name, id
            )
        }))),
    ))
}

// ── Shared helper ─────────────────────────────────────────────────────────────

async fn fetch_snapshot_row(state: &AppState, id: &str) -> Result<Snapshot, AppError> {
    sqlx::query_as::<_, Snapshot>("SELECT * FROM snapshots WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Snapshot {id} not found"))))
}
