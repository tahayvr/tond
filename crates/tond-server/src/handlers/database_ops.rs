// handlers/database_ops.rs — managed database essentials
//
//   GET  /api/databases/catalog                  — engines, versions and variants
//   GET  /api/databases/{id}/backup-schedule     — schedule + backup health
//   PUT  /api/databases/{id}/backup-schedule     — set/clear the schedule, retention
//   GET  /api/databases/{id}/extensions          — postgres extensions (offered/installed)
//   POST /api/databases/{id}/extensions          — CREATE EXTENSION (offered ones only)
//   POST /api/databases/{id}/upgrade             — start a major-version upgrade (admin)
//   POST /api/databases/{id}/upgrade/confirm     — keep the new version, drop the old data
//   POST /api/databases/{id}/upgrade/rollback    — go back to the old version and data
//   POST /api/databases/{id}/upgrade/dismiss     — clear a failed upgrade's banner
//
// Restoring a backup into a new database is POST
// /api/snapshots/{id}/restore-new-database (handlers/backups.rs).

use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::{
    auth::{Authz, RateLimit, scope},
    backup, db_exec, db_upgrade,
    error::AppError,
    handlers::databases::{fetch_db, normalize_schedule},
    state::AppState,
};

// ── Catalog ──────────────────────────────────────────────────────────────────

/// GET /api/databases/catalog
pub async fn get_catalog(
    _rate: RateLimit,
    _auth: Authz<scope::DbRead>,
) -> Result<impl IntoResponse, AppError> {
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(catalog(std::env::consts::ARCH))),
    ))
}

// ── Backup schedule ──────────────────────────────────────────────────────────

/// Why a database has no automatic protection, if it hasn't.
pub fn backup_warning(
    name: &str,
    schedule: Option<&str>,
    full_snapshots_enabled: bool,
) -> Option<String> {
    (schedule.is_none() && !full_snapshots_enabled).then(|| {
        format!(
            "Nothing backs up {name} automatically. Add a schedule, or turn on scheduled \
             snapshots on the Snapshots page."
        )
    })
}

async fn backup_status(
    state: &AppState,
    db: &ManagedDatabase,
) -> Result<DatabaseBackupStatus, AppError> {
    let cfg = backup::get_backup_config(state).await?;
    let scope_name = format!("database:{}", db.name);
    let last_backup_at: Option<String> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM snapshots
         WHERE status = 'completed' AND (scope = ? OR scope = 'full')",
    )
    .bind(&scope_name)
    .fetch_one(&state.db)
    .await?;
    let last: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT status, error_message FROM snapshots
         WHERE scope = ? AND status IN ('completed', 'failed')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&scope_name)
    .fetch_optional(&state.db)
    .await?;
    let full_enabled = cfg.enabled != 0;
    Ok(DatabaseBackupStatus {
        next_run_at: db
            .backup_schedule
            .as_deref()
            .and_then(backup::next_run)
            .map(|t| t.to_rfc3339()),
        warning: backup_warning(&db.name, db.backup_schedule.as_deref(), full_enabled),
        schedule: db.backup_schedule.clone(),
        retention: db.backup_retention,
        full_snapshots_enabled: full_enabled,
        full_schedule: full_enabled.then_some(cfg.full_schedule_cron),
        last_backup_at,
        last_status: last.as_ref().map(|(s, _)| s.clone()),
        last_error: last.and_then(|(_, e)| e),
    })
}

/// GET /api/databases/{id}/backup-schedule
pub async fn get_backup_schedule(
    _rate: RateLimit,
    _auth: Authz<scope::SnapshotsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(backup_status(&state, &db).await?)),
    ))
}

/// PUT /api/databases/{id}/backup-schedule
pub async fn update_backup_schedule(
    _rate: RateLimit,
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDatabaseBackupSchedule>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    let schedule = normalize_schedule(req.schedule.as_deref())?;
    let retention = req.retention.unwrap_or(db.backup_retention);
    if !(1..=MAX_DB_BACKUP_RETENTION).contains(&retention) {
        return Err(AppError(TondError::InvalidInput(format!(
            "Keep between 1 and {MAX_DB_BACKUP_RETENTION} backups. A schedule that keeps none \
             would delete each backup right after taking it."
        ))));
    }
    sqlx::query(
        "UPDATE databases SET backup_schedule = ?, backup_retention = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&schedule)
    .bind(retention)
    .bind(&db.id)
    .execute(&state.db)
    .await?;
    backup::schedules_changed();
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("db_backup_schedule")
            .resource("database", &db.id, &db.name)
            .detail(format!(
                "schedule={} retention={retention}",
                schedule.as_deref().unwrap_or("off")
            )),
    )
    .await;
    let db = fetch_db(&state.db, &db.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(backup_status(&state, &db).await?)),
    ))
}

// ── Extensions (postgres) ────────────────────────────────────────────────────

fn require_postgres_running(db: &ManagedDatabase) -> Result<String, AppError> {
    if db.db_type != "postgres" {
        return Err(AppError(TondError::InvalidInput(
            "Extensions are a PostgreSQL feature.".to_string(),
        )));
    }
    match (&db.container_id, db.status.as_str()) {
        (Some(cid), "running") => Ok(cid.clone()),
        _ => Err(AppError(TondError::InvalidInput(format!(
            "Start '{}' first: extensions are read from the running database.",
            db.name
        )))),
    }
}

/// Parse `name|default|installed|comment` lines from psql.
pub fn parse_extension_rows(output: &str, offered: &[&str]) -> Vec<DatabaseExtension> {
    let mut found: Vec<DatabaseExtension> = output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            let name = parts.next()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let opt = |s: Option<&str>| {
                s.map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            Some(DatabaseExtension {
                default_version: opt(parts.next()),
                installed_version: opt(parts.next()),
                comment: opt(parts.next()),
                available: true,
                name,
            })
        })
        .collect();
    // Offered but missing from this image: listed as unavailable.
    for name in offered {
        if !found.iter().any(|e| e.name == *name) {
            found.push(DatabaseExtension {
                name: name.to_string(),
                available: false,
                installed_version: None,
                default_version: None,
                comment: None,
            });
        }
    }
    found.sort_by_key(|e| {
        offered
            .iter()
            .position(|o| *o == e.name)
            .unwrap_or(usize::MAX)
    });
    found
}

/// GET /api/databases/{id}/extensions
pub async fn list_extensions(
    _rate: RateLimit,
    _auth: Authz<scope::DbRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    let cid = require_postgres_running(&db)?;
    let offered = offered_extensions(&db.variant);
    // The names are catalog constants (validated), safe to quote.
    let list = offered
        .iter()
        .filter(|n| valid_extension_name(n))
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT name, coalesce(default_version, ''), coalesce(installed_version, ''), \
         coalesce(replace(comment, '|', '/'), '') FROM pg_available_extensions \
         WHERE name IN ({list}) ORDER BY name"
    );
    let cmd = db_exec::Cmd::new(&[
        "psql",
        "-X",
        "-A",
        "-t",
        "-F",
        "|",
        "-U",
        db.db_user.as_deref().unwrap_or("postgres"),
        "-d",
        db.db_name.as_deref().unwrap_or("postgres"),
        "-c",
        &sql,
    ]);
    let output = db_exec::run(&state.docker, &cid, &cmd, Duration::from_secs(30), &[])
        .await
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "Couldn't read the extensions: {e:#}"
            )))
        })?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(parse_extension_rows(&output, &offered))),
    ))
}

/// POST /api/databases/{id}/extensions
pub async fn create_extension(
    _rate: RateLimit,
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateExtensionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    let cid = require_postgres_running(&db)?;
    let offered = offered_extensions(&db.variant);
    let name = req.name.trim();
    if !valid_extension_name(name) || !offered.contains(&name) {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{name}' isn't one of the extensions offered for this database: {}",
            offered.join(", ")
        ))));
    }
    let sql = format!("CREATE EXTENSION IF NOT EXISTS \"{name}\"");
    let cmd = db_exec::Cmd::new(&[
        "psql",
        "-v",
        "ON_ERROR_STOP=1",
        "-X",
        "-q",
        "-U",
        db.db_user.as_deref().unwrap_or("postgres"),
        "-d",
        db.db_name.as_deref().unwrap_or("postgres"),
        "-c",
        &sql,
    ]);
    db_exec::run(&state.docker, &cid, &cmd, Duration::from_secs(120), &[])
        .await
        .map_err(|e| {
            AppError(TondError::InvalidInput(format!(
                "PostgreSQL couldn't enable {name}: {e:#}"
            )))
        })?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("db_extension_create")
            .resource("database", &db.id, &db.name)
            .detail(format!("extension={name}")),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "message": format!("{name} is enabled in {}.", db.name)
        }))),
    ))
}

// ── Major-version upgrades ───────────────────────────────────────────────────

/// POST /api/databases/{id}/upgrade — admins only: a guarded but
/// long-running change of the engine version.
pub async fn start_upgrade(
    _rate: RateLimit,
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpgradeDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_to(Scope::SettingsWrite, "upgrade a database to a new version")?;
    let db = fetch_db(&state.db, &id).await?;
    let to = req.version.trim().to_string();
    let upgrade = db_upgrade::start(&state, db.clone(), &to)
        .await
        .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("db_upgrade_start")
            .resource("database", &db.id, &db.name)
            .detail(format!("from={} to={to}", upgrade.from_version)),
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::ok(upgrade))))
}

/// POST /api/databases/{id}/upgrade/confirm
pub async fn confirm_upgrade(
    _rate: RateLimit,
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_to(Scope::SettingsWrite, "confirm a database upgrade")?;
    let db = fetch_db(&state.db, &id).await?;
    db_upgrade::confirm(&state, &db)
        .await
        .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("db_upgrade_confirm")
            .resource("database", &db.id, &db.name)
            .detail(format!("version={}", db.effective_version())),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "message": format!("{} stays on version {}. The old data was deleted.", db.name, db.effective_version())
        }))),
    ))
}

/// POST /api/databases/{id}/upgrade/rollback
pub async fn rollback_upgrade(
    _rate: RateLimit,
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_to(Scope::SettingsWrite, "roll back a database upgrade")?;
    let db = fetch_db(&state.db, &id).await?;
    let from = db
        .upgrade_state()
        .map(|u| u.from_version)
        .unwrap_or_default();
    db_upgrade::rollback(&state, &db)
        .await
        .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("db_upgrade_rollback")
            .resource("database", &db.id, &db.name)
            .detail(format!("back_to={from}")),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "message": format!("{} runs version {from} again, with its data from before the upgrade.", db.name)
        }))),
    ))
}

/// POST /api/databases/{id}/upgrade/dismiss
pub async fn dismiss_upgrade(
    _rate: RateLimit,
    _auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    db_upgrade::dismiss(&state, &db)
        .await
        .map_err(|msg| AppError(TondError::Internal(msg)))?;
    Ok((StatusCode::OK, Json(ApiResponse::<()>::ok(()))))
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

    async fn put_schedule(
        state: &AppState,
        schedule: Option<&str>,
        retention: Option<i64>,
    ) -> Result<serde_json::Value, AppError> {
        let response = update_backup_schedule(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path("shop".to_string()),
            Json(UpdateDatabaseBackupSchedule {
                schedule: schedule.map(str::to_string),
                retention,
            }),
        )
        .await?
        .into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        Ok(serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{e}")))
    }

    #[tokio::test]
    async fn backup_schedules_are_validated_stored_and_reported() {
        let state = test_state().await;
        sqlx::query("INSERT INTO databases (id, name, db_type) VALUES ('d1', 'shop', 'postgres')")
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        let err = put_schedule(&state, Some("every night"), None).await.err();
        assert!(err.is_some_and(|e| e.0.status_code() == 400));
        let err = put_schedule(&state, Some("0 3 * * *"), Some(0)).await.err();
        assert!(
            err.is_some_and(|e| e.0.to_string().contains("keeps none")),
            "a schedule that keeps zero backups is refused"
        );

        let json = put_schedule(&state, Some(" 0 3 * * * "), Some(14))
            .await
            .unwrap_or_else(|_| panic!("a valid schedule is stored"));
        assert_eq!(json["data"]["schedule"], "0 3 * * *");
        assert_eq!(json["data"]["retention"], 14);
        assert!(json["data"]["next_run_at"].is_string());
        assert!(json["data"]["warning"].is_null());

        // Off again, with full snapshots disabled: a warning.
        let json = put_schedule(&state, Some(""), None)
            .await
            .unwrap_or_else(|_| panic!("clearing works"));
        assert!(json["data"]["schedule"].is_null());
        assert_eq!(json["data"]["retention"], 14, "retention is kept");
        assert!(
            json["data"]["warning"]
                .as_str()
                .is_some_and(|w| w.contains("shop"))
        );

        let entry: Option<String> = sqlx::query_scalar(
            "SELECT action FROM audit_log WHERE action = 'db_backup_schedule' LIMIT 1",
        )
        .fetch_optional(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(entry.is_some(), "schedule changes are audit-logged");
    }

    #[tokio::test]
    async fn upgrades_need_an_admin_and_refuse_downgrades() {
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO databases (id, name, db_type, status, container_id, version)
             VALUES ('d1', 'shop', 'postgres', 'running', 'c1', '17')",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let call = |role: Role, version: &str| {
            start_upgrade(
                RateLimit,
                Authz::unchecked(Principal::test_user(role)),
                State(state.clone()),
                Path("d1".to_string()),
                Json(UpgradeDatabaseRequest {
                    version: version.to_string(),
                }),
            )
        };
        let err = call(Role::Deployer, "18").await.err();
        assert!(err.is_some_and(|e| e.0.status_code() == 403));
        let err = call(Role::Admin, "16").await.err();
        assert!(err.is_some_and(|e| e.0.to_string().contains("Downgrades")));
        let stored: Option<String> =
            sqlx::query_scalar("SELECT upgrade FROM databases WHERE id = 'd1'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(stored, None, "a refused upgrade leaves no state behind");
    }

    #[test]
    fn warns_only_when_nothing_backs_the_database_up() {
        assert!(backup_warning("shop", None, false).is_some_and(|w| w.contains("shop")));
        assert_eq!(backup_warning("shop", Some("0 3 * * *"), false), None);
        assert_eq!(backup_warning("shop", None, true), None);
    }

    #[test]
    fn extension_rows_mark_missing_offers_unavailable_and_keep_catalog_order() {
        let offered = ["vector", "pg_trgm", "citext"];
        let rows = parse_extension_rows(
            "pg_trgm|1.6||text similarity\nvector|0.8.1|0.8.1|vector type\n",
            &offered,
        );
        let names: Vec<&str> = rows.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["vector", "pg_trgm", "citext"]);
        assert_eq!(rows[0].installed_version.as_deref(), Some("0.8.1"));
        assert_eq!(rows[1].installed_version, None);
        assert_eq!(rows[1].default_version.as_deref(), Some("1.6"));
        assert!(!rows[2].available);
    }
}
