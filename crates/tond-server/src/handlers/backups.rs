// handlers/backups.rs — Snapshot API handlers + SSE progress stream
//
// Routes:
//   POST   /api/v1/snapshots                       — trigger manual Full snapshot
//   GET    /api/v1/snapshots                        — list snapshots (filters: scope, trigger, app)
//   DELETE /api/v1/snapshots/{id}                   — delete record + local dir + remote object
//   GET    /api/v1/snapshots/config                 — get backup configuration
//   PUT    /api/v1/snapshots/config                 — update backup configuration
//   GET    /api/v1/snapshots/{id}/logs/stream       — SSE progress stream
//   POST   /api/v1/snapshots/{id}/restore           — Full restore (server restart)
//   POST   /api/v1/snapshots/{id}/restore-app       — App-scope rollback (no restart)
//   POST   /api/v1/snapshots/{id}/restore-database  — Database-scope restore (no restart)
//   GET    /api/v1/databases/{id}/backups           — snapshots that hold one database
//   POST   /api/v1/databases/{id}/backups           — back up one database now
//
// Remote copies of snapshots are encrypted with a key derived from
// TOND_SECRET_KEY: restoring them on another server needs the same key.

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

use crate::audit_log::{AuditEntry, record};
use crate::{
    auth::{Authz, RateLimit, scope},
    backup::{self, SnapshotTrigger},
    snapshot_remote,
    error::AppError,
    state::AppState,
};

// ── Create snapshot ───────────────────────────────────────────────────────────

pub async fn create_snapshot(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsCreate>,
    State(state): State<AppState>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<impl IntoResponse, AppError> {
    let trigger = SnapshotTrigger::Manual { name: req.name };
    let snapshot = run_snapshot(&state, trigger).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_create").resource("snapshot", &snapshot.id, &snapshot.name),
    )
    .await;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(snapshot))))
}

/// Take a snapshot (the local copy; a remote upload continues in the
/// background) and return its row.
async fn run_snapshot(
    state: &AppState,
    trigger: SnapshotTrigger,
) -> Result<SnapshotResponse, AppError> {
    let (tx, _rx) = mpsc::channel(32);
    // Tracked work: a graceful shutdown lets the snapshot finish even if this
    // request's connection is closed first.
    let worker = state.clone();
    let id = crate::runtime::shutdown::spawn_tracked(async move {
        backup::create_snapshot(&worker, trigger, Some(tx)).await
    })
    .await
    .map_err(|e| AppError(TondError::Internal(format!("Snapshot task failed: {e}"))))?
    .map_err(|e| AppError(TondError::Internal(format!("Snapshot failed: {e:#}"))))?;
    Ok(SnapshotResponse::from(
        fetch_snapshot_row(state, &id).await?,
    ))
}

// ── Per-database backups ──────────────────────────────────────────────────────

async fn database_name(state: &AppState, id_or_name: &str) -> Result<String, AppError> {
    sqlx::query_scalar("SELECT name FROM databases WHERE id = ? OR name = ? LIMIT 1")
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("database '{id_or_name}'"))))
}

/// GET /api/v1/databases/{id}/backups — this database's own backups and the
/// full snapshots (which hold every database), newest first.
pub async fn list_database_backups(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let name = database_name(&state, &id).await?;
    let rows = sqlx::query_as::<_, Snapshot>(
        "SELECT * FROM snapshots WHERE scope = ? OR scope = 'full' ORDER BY created_at DESC",
    )
    .bind(format!("database:{name}"))
    .fetch_all(&state.db)
    .await?;
    let resp: Vec<SnapshotResponse> = rows.into_iter().map(SnapshotResponse::from).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(resp))))
}

/// POST /api/v1/databases/{id}/backups — back up one database now.
pub async fn create_database_backup(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsCreate>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let name = database_name(&state, &id).await?;
    let snapshot = run_snapshot(
        &state,
        SnapshotTrigger::ManualDatabase {
            db_name: name.clone(),
        },
    )
    .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("db_backup")
            .resource("snapshot", &snapshot.id, &snapshot.name)
            .detail(format!("database={name}")),
    )
    .await;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(snapshot))))
}

// ── List snapshots ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ListSnapshotsQuery {
    pub scope: Option<String>,
    pub trigger: Option<String>,
    pub app: Option<String>,
}

pub async fn list_snapshots(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
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
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let snap = fetch_snapshot_row(&state, &id).await?;
    // Local directory, remote archive and row.
    backup::delete_snapshot_files_and_row(&state, &snap).await;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_delete").resource("snapshot", &snap.id, &snap.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "deleted": id }))),
    ))
}

// ── Get backup config ─────────────────────────────────────────────────────────

pub async fn get_backup_config(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
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
    auth: Authz<scope::SettingsWrite>,
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

    // A retention of zero would delete every snapshot right after taking it.
    if full_retention_count < 1 || pre_deploy_retention_per_app < 1 {
        return Err(AppError(TondError::InvalidInput(
            "Keep at least one snapshot: a retention of 0 deletes each snapshot right after \
             taking it. Turn the schedule off instead."
                .to_string(),
        )));
    }

    // Validate with the same parser the scheduler uses (standard 5-field cron,
    // 0/7 = Sunday, @daily-style macros).
    crate::cron::parse_schedule(&full_schedule_cron).map_err(|e| {
        AppError(TondError::InvalidInput(format!(
            "Invalid cron expression: {}",
            e
        )))
    })?;

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

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("backup_config_update").kind("backup_config"),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(BackupConfigResponse::from(updated))),
    ))
}

// ── SSE progress stream ───────────────────────────────────────────────────────

pub async fn stream_snapshot_logs(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
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
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Recorded before starting: a full restore replaces the database (and
    // this log with it) and restarts the server.
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_restore_full").resource("snapshot", &id, &id),
    )
    .await;
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
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreAppRequest>,
) -> Result<impl IntoResponse, AppError> {
    backup::restore_app(&state, &id, &req.app_name)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("App restore failed: {}", e))))?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_restore_app")
            .resource("snapshot", &id, &id)
            .detail(format!("app={}", req.app_name)),
    )
    .await;
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
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    let pre_restore_snapshot_id = backup::restore_database(&state, &id, &req.db_name)
        .await
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "Database restore failed: {e:#}"
            )))
        })?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_restore_database")
            .resource("snapshot", &id, &id)
            .detail(format!("database={}", req.db_name)),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(RestoreDatabaseResponse {
            message: format!(
                "Database '{}' restored. The data it held just before is saved as a backup.",
                req.db_name
            ),
            pre_restore_snapshot_id,
        })),
    ))
}

// ── Restore into a new database ───────────────────────────────────────────────

/// POST /api/snapshots/{id}/restore-new-database — restore one database of
/// a snapshot into a new managed database (same engine and version, fresh
/// credentials), leaving the original untouched: for recovery drills, or to
/// pull old rows out of a backup.
pub async fn restore_to_new_database(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreToNewDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_to(Scope::DbWrite, "create a database from a backup")?;
    let name = req.name.trim().to_string();
    let source = req.source.trim().to_string();
    let worker = state.clone();
    let (snap, src, target, project) = (id.clone(), source.clone(), name.clone(), req.project_id);
    // Tracked: a graceful shutdown lets the restore finish (or clean up).
    let db = crate::runtime::shutdown::spawn_tracked(async move {
        backup::restore_to_new_database(&worker, &snap, &src, &target, project).await
    })
    .await
    .map_err(|e| AppError(TondError::Internal(format!("Restore task failed: {e}"))))?
    .map_err(|e| AppError(TondError::InvalidInput(format!("{e:#}"))))?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_restore_new_database")
            .resource("snapshot", &id, &id)
            .detail(format!("source={source} new={name}")),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(json!({
            "message": format!("Restored {source} into the new database {name}. {source} was not changed."),
            "database": ManagedDatabaseResponse::from(db),
        }))),
    ))
}

// ── Remote storage, exports and imports ───────────────────────────────────────

#[derive(Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct RemoteListQuery {
    /// Storage provider to list; default: the one in the backup settings
    pub provider_id: Option<String>,
}

/// GET /api/v1/snapshots/remote — the snapshots in a storage provider's
/// bucket (from their manifests), including ones this server never took.
pub async fn list_remote_snapshots(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
    State(state): State<AppState>,
    Query(query): Query<RemoteListQuery>,
) -> Result<impl IntoResponse, AppError> {
    let list = snapshot_remote::list_remote(&state, query.provider_id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(list))))
}

/// POST /api/v1/snapshots/remote/{id}/import — add a snapshot from the
/// bucket to this server's list, to restore it from there.
pub async fn import_remote_snapshot(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RemoteProviderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let snap = snapshot_remote::import_remote(&state, req.provider_id, &id).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_import")
            .resource("snapshot", &snap.id, &snap.name)
            .detail("from=storage"),
    )
    .await;
    Ok((StatusCode::OK, Json(ApiResponse::ok(SnapshotResponse::from(snap)))))
}

/// POST /api/v1/snapshots/{id}/upload — upload a snapshot's encrypted
/// archive (and manifest) to a storage provider now (`tond export --remote`).
pub async fn upload_snapshot(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsCreate>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RemoteProviderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let provider = snapshot_remote::provider(&state, req.provider_id).await?;
    let snap = backup::fetch_snapshot(&state, &id)
        .await
        .map_err(|e| AppError(TondError::NotFound(format!("{e:#}"))))?;
    let dir = backup::ensure_local(&state, &snap)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?;
    sqlx::query("UPDATE snapshots SET remote_status = 'uploading', remote_error = NULL WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    let worker = state.clone();
    let (sid, sp) = (id.clone(), provider.id.clone());
    crate::runtime::shutdown::spawn_tracked(async move {
        backup::upload_snapshot(&worker, &sid, &dir, &sp, true).await;
    });
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_export")
            .resource("snapshot", &snap.id, &snap.name)
            .detail(format!("to=storage provider={}", provider.name)),
    )
    .await;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(SnapshotResponse::from(
            fetch_snapshot_row(&state, &id).await?,
        ))),
    ))
}

/// GET /api/v1/snapshots/{id}/manifest — what the snapshot holds (its
/// manifest.json).
pub async fn get_snapshot_manifest(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let snap = backup::fetch_snapshot(&state, &id)
        .await
        .map_err(|e| AppError(TondError::NotFound(format!("{e:#}"))))?;
    let dir = backup::ensure_local(&state, &snap)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?;
    let manifest = backup::read_manifest(&dir)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(manifest))))
}

/// GET /api/v1/snapshots/{id}/archive — the snapshot as one encrypted
/// archive, streamed (`tond export`). Readable only with this server's
/// TOND_SECRET_KEY.
pub async fn export_snapshot_archive(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let snap = backup::fetch_snapshot(&state, &id)
        .await
        .map_err(|e| AppError(TondError::NotFound(format!("{e:#}"))))?;
    let dir = backup::ensure_local(&state, &snap)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_export")
            .resource("snapshot", &snap.id, &snap.name)
            .detail("to=download"),
    )
    .await;

    let (tx, rx) = mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(4);
    let sid = id.clone();
    tokio::task::spawn_blocking(move || {
        let errors = tx.clone();
        let result = backup::archive_snapshot_dir(&dir, backup::ChunkWriter::new(tx), &sid)
            .and_then(|(_, writer)| writer.finish().map_err(Into::into));
        if let Err(e) = result {
            // Fail the body instead of ending it: a truncated export must
            // not look complete.
            tracing::warn!("Export of snapshot {sid} failed: {e:#}");
            let _ = errors.blocking_send(Err(std::io::Error::other(format!("{e:#}"))));
        }
    });
    let body = axum::body::Body::from_stream(ReceiverStream::new(rx));
    Ok((
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"tond-{id}.tondsnap\""),
            ),
        ],
        body,
    )
        .into_response())
}

/// POST /api/v1/snapshots/import — import an exported archive (the request
/// body, streamed to disk): verified with this server's TOND_SECRET_KEY,
/// extracted into a local snapshot and listed (`tond import <file>`).
pub async fn import_snapshot_archive(
    _rate: RateLimit,
    auth: Authz<scope::SnapshotsRestore>,
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Result<impl IntoResponse, AppError> {
    use tokio::io::AsyncWriteExt as _;

    let dir = backup::snapshots_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    let upload = dir.join(format!(".import-{}.download", uuid::Uuid::new_v4()));
    let received = async {
        let mut file = tokio::fs::File::create(&upload).await?;
        let mut stream = request.into_body().into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        std::io::Result::Ok(())
    }
    .await;
    let result = match received {
        Ok(()) => snapshot_remote::import_archive_file(&state, &upload).await,
        Err(e) => Err(AppError(TondError::InvalidInput(format!(
            "The upload broke off: {e}"
        )))),
    };
    let _ = tokio::fs::remove_file(&upload).await;
    let snap = result?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("snapshot_import")
            .resource("snapshot", &snap.id, &snap.name)
            .detail("from=archive"),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(SnapshotResponse::from(snap))),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::to_bytes;
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;

    use super::*;
    use crate::auth::{Principal, RateLimiter};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("sqlite: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    #[tokio::test]
    async fn default_backup_schedule_is_accepted_by_the_scheduler_parser() {
        // The default is standard 5-field cron; the raw `cron` crate rejects
        // it, which silently disabled scheduled backups.
        let state = test_state().await;
        let (schedule,): (String,) =
            sqlx::query_as("SELECT full_schedule_cron FROM backup_config WHERE id = 1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("read default: {err}"));
        assert_eq!(schedule, "0 3 * * *");
        assert!(crate::cron::parse_schedule(&schedule).is_ok());
    }

    #[tokio::test]
    async fn backup_config_refuses_a_retention_below_one() {
        let state = test_state().await;
        for body in [
            r#"{"full_retention_count":0}"#,
            r#"{"pre_deploy_retention_per_app":0}"#,
            r#"{"full_retention_count":-1}"#,
        ] {
            let req: UpdateBackupConfigRequest =
                serde_json::from_str(body).unwrap_or_else(|err| panic!("{body}: {err}"));
            let result = update_backup_config(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Json(req),
            )
            .await;
            assert!(
                matches!(result, Err(AppError(TondError::InvalidInput(_)))),
                "{body} must be refused"
            );
        }
        let (full, pre): (i64, i64) = sqlx::query_as(
            "SELECT full_retention_count, pre_deploy_retention_per_app FROM backup_config WHERE id = 1",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("read: {err}"));
        assert!(full >= 1 && pre >= 1, "nothing was stored");
    }

    #[tokio::test]
    async fn database_backups_list_its_own_backups_and_full_snapshots() {
        let state = test_state().await;
        sqlx::query("INSERT INTO databases (id, name, db_type) VALUES ('d1', 'shop', 'postgres')")
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("seed db: {err}"));
        for (id, scope, created) in [
            ("s1", "database:shop", "2026-01-03"),
            ("s2", "full", "2026-01-02"),
            ("s3", "database:other", "2026-01-04"),
            ("s4", "app:web", "2026-01-05"),
        ] {
            sqlx::query(
                "INSERT INTO snapshots (id, name, status, scope, created_at) VALUES (?, ?, 'completed', ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(scope)
            .bind(created)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("seed snapshot: {err}"));
        }

        let response = list_database_backups(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Viewer)),
            State(state.clone()),
            Path("d1".to_string()),
        )
        .await
        .unwrap_or_else(|_| panic!("list should succeed"))
        .into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("body: {err}"));
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("json: {err}"));
        let ids: Vec<&str> = json["data"]
            .as_array()
            .unwrap_or_else(|| panic!("array"))
            .iter()
            .filter_map(|s| s["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["s1", "s2"],
            "newest first, own backups + full snapshots"
        );

        let missing = list_database_backups(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Viewer)),
            State(state.clone()),
            Path("nope".to_string()),
        )
        .await;
        assert!(missing.is_err());
    }
}
