// handlers/databases.rs — Managed Database handlers (Phase 6.3)
//
// Creates and manages Docker containers for postgres / mysql / redis databases.
// Each database gets a persistent volume, a randomly-assigned host port, and
// optional connection to a project Docker network for internal service-to-service
// communication.
//
// Routes (all require RequireAuth + RateLimit):
//   GET    /api/databases                         — list all managed databases
//   POST   /api/databases                         — create and start a new database container
//   GET    /api/databases/{id}                    — get database details + connection strings
//   PATCH  /api/databases/{id}                    — update custom URL / image overrides
//   DELETE /api/databases/{id}                    — stop+remove container, delete row
//   POST   /api/databases/{id}/stop               — stop container
//   POST   /api/databases/{id}/start              — start container (existing container)
//   POST   /api/databases/{id}/redeploy           — recreate container (picks up custom_image)
//   PATCH  /api/databases/{id}/credentials        — update stored credentials (takes effect on redeploy)

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use crate::{
    audit_log::{AuthActor, write_audit_log},
    auth::{RateLimit, RequireAuth},
    docker,
    error::AppError,
    state::AppState,
};

// ── Shared lookup helper ──

async fn fetch_db(pool: &sqlx::SqlitePool, id_or_name: &str) -> Result<ManagedDatabase, AppError> {
    let db: Option<ManagedDatabase> =
        sqlx::query_as("SELECT * FROM databases WHERE id = ? OR name = ? LIMIT 1")
            .bind(id_or_name)
            .bind(id_or_name)
            .fetch_optional(pool)
            .await?;
    db.ok_or_else(|| fetch_db_error(id_or_name))
}

// ── List databases ──

pub async fn list_databases(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let dbs: Vec<ManagedDatabase> =
        sqlx::query_as("SELECT * FROM databases ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;
    let responses: Vec<ManagedDatabaseResponse> =
        dbs.into_iter().map(ManagedDatabaseResponse::from).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── Get database (with connection strings) ──

pub async fn get_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    let detail = build_detail(db);
    Ok((StatusCode::OK, Json(ApiResponse::ok(detail))))
}

/// Build a `ManagedDatabaseDetail` from a `ManagedDatabase` row,
/// computing both external and internal connection strings.
/// If the database row has custom URL overrides set, those take precedence
/// over the auto-computed strings.
fn build_detail(db: ManagedDatabase) -> ManagedDatabaseDetail {
    // Use the user-supplied override if present; otherwise compute from credentials.
    let connection_string = if let Some(ref custom) = db.custom_external_url {
        Some(custom.clone())
    } else {
        db.host_port.and_then(|port| {
            engine_for(&db.db_type).map(|eng| {
                eng.external_connection_string(
                    port,
                    db.db_name.as_deref(),
                    db.db_user.as_deref(),
                    db.db_password.as_deref(),
                )
            })
        })
    };

    // Internal string: use override if set, otherwise compute (only when db has a project).
    let internal_connection_string = if let Some(ref custom) = db.custom_internal_url {
        Some(custom.clone())
    } else if db.project_id.is_some() {
        engine_for(&db.db_type).map(|eng| {
            eng.internal_connection_string(
                &db.name,
                db.db_name.as_deref(),
                db.db_user.as_deref(),
                db.db_password.as_deref(),
            )
        })
    } else {
        None
    };

    ManagedDatabaseDetail {
        database: ManagedDatabaseResponse::from(db),
        connection_string,
        internal_connection_string,
    }
}

fn fetch_db_error(id_or_name: &str) -> AppError {
    AppError(TondError::NotFound(format!("database '{}'", id_or_name)))
}

fn normalized_optional_override(value: Option<String>) -> Option<Option<String>> {
    value.map(|v| {
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn next_memory_limit(current: Option<i64>, requested: Option<i64>) -> Option<i64> {
    match requested {
        Some(v) if v <= 0 => None,
        Some(v) => Some(v),
        None => current,
    }
}

fn next_credential(current: Option<String>, requested: Option<String>) -> Option<String> {
    match requested {
        Some(v) => {
            let trimmed = v.trim().to_string();
            if trimmed.is_empty() {
                current
            } else {
                Some(trimmed)
            }
        }
        None => current,
    }
}

// ── Create database ──

pub async fn create_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Json(req): Json<CreateDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate name (alphanumeric + hyphens)
    if !req.name.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(AppError(TondError::InvalidInput(
            "Database name must be alphanumeric (hyphens allowed)".to_string(),
        )));
    }

    // Validate db_type against the engine registry — this is the only place
    // where the allowed set needs to be checked. Adding a new engine to
    // ALL_ENGINES in tond-common automatically unlocks it here.
    let engine = engine_for(&req.db_type).ok_or_else(|| {
        let supported = tond_common::ALL_ENGINES
            .iter()
            .map(|e| e.db_type)
            .collect::<Vec<_>>()
            .join(", ");
        AppError(TondError::InvalidInput(format!(
            "db_type must be one of: {}",
            supported
        )))
    })?;

    // Verify project exists if provided
    if let Some(ref pid) = req.project_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?)")
            .bind(pid)
            .fetch_one(&state.db)
            .await?;
        if !exists {
            return Err(AppError(TondError::NotFound(format!("project '{}'", pid))));
        }
    }

    let mut db = ManagedDatabase::new(
        req.name.clone(),
        req.db_type.clone(),
        req.project_id.clone(),
    );
    if let Some(mem) = req.memory_limit {
        db.memory_limit = Some(mem);
    }

    // Set up volume path
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let volume_path = format!("{}/databases/{}", data_dir, db.name);
    db.volume_path = Some(volume_path.clone());

    // Find a free port starting from 5400
    let host_port = docker::find_free_port(&state.docker, 5400)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    // Build Docker image, env vars, internal port, and volume bind from the engine registry.
    let env_vars = engine.build_env(
        db.db_name.as_deref(),
        db.db_user.as_deref(),
        db.db_password.as_deref(),
        db.root_password.as_deref(),
    );
    let bind_path = format!("{}:{}", volume_path, engine.data_dir);
    let container_port = engine.container_port;
    let image = engine.image;

    // Create volume directory on host (best-effort; Docker will also create it)
    let _ = tokio::fs::create_dir_all(&volume_path).await;

    // Connect to project network if applicable
    let network_name_owned;
    let network_name: Option<&str> = if let Some(ref pid) = req.project_id {
        network_name_owned = format!("tond-{}", pid);
        docker::ensure_network(&state.docker, &network_name_owned)
            .await
            .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
        Some(&network_name_owned)
    } else {
        None
    };

    let container_name = format!("tond-db-{}", db.name);
    let db_name_alias = db.name.clone();

    // Start the container using run_container_named (verbatim name)
    let container_id = docker::run_container_named(
        &state.docker,
        image,
        &container_name,
        container_port,
        host_port,
        env_vars,
        db.memory_limit,
        None, // no cpu limit for databases
        vec![bind_path],
        network_name,
        Some(&db_name_alias),
    )
    .await
    .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    db.container_id = Some(container_id);
    db.host_port = Some(host_port as i64);
    db.status = "running".to_string();
    db.image_used = Some(image.to_string());

    // Insert into DB
    sqlx::query(
        "INSERT INTO databases
            (id, name, db_type, project_id, container_id, host_port, status,
             db_name, db_user, db_password, root_password, volume_path, memory_limit,
             image_used, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&db.id)
    .bind(&db.name)
    .bind(&db.db_type)
    .bind(&db.project_id)
    .bind(&db.container_id)
    .bind(db.host_port)
    .bind(&db.status)
    .bind(&db.db_name)
    .bind(&db.db_user)
    .bind(&db.db_password)
    .bind(&db.root_password)
    .bind(&db.volume_path)
    .bind(db.memory_limit)
    .bind(&db.image_used)
    .bind(&db.created_at)
    .bind(&db.updated_at)
    .execute(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            AppError(TondError::AppAlreadyExists(req.name.clone()))
        } else {
            AppError(TondError::DatabaseError(e))
        }
    })?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_create",
        &actor.0,
        Some(&format!(
            "name={} type={} image={}",
            db.name, db.db_type, image
        )),
    )
    .await;

    let detail = build_detail(db);
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(detail))))
}

// ── Delete database ──

pub async fn delete_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    // Stop and remove the container (best-effort)
    if let Some(ref cid) = db.container_id {
        let _ = docker::remove_container(&state.docker, cid).await;
    }

    // Remove the volume directory from disk (best-effort)
    if let Some(ref vol) = db.volume_path {
        let _ = tokio::fs::remove_dir_all(vol).await;
    }

    sqlx::query("DELETE FROM databases WHERE id = ?")
        .bind(&db.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_delete",
        &actor.0,
        Some(&format!("name={}", db.name)),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": db.id }))),
    ))
}

// ── Stop database ──

pub async fn stop_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    let container_id = db.container_id.as_deref().ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "Database has no container".to_string(),
        ))
    })?;

    docker::stop_container(&state.docker, container_id)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    sqlx::query(
        "UPDATE databases SET status = 'stopped', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_stop",
        &actor.0,
        Some(&format!("name={}", db.name)),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "message": "Database stopped" }),
        )),
    ))
}

// ── Start database ──

pub async fn start_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    let container_id = db.container_id.as_deref().ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "Database has no container".to_string(),
        ))
    })?;

    docker::start_container(&state.docker, container_id)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    sqlx::query(
        "UPDATE databases SET status = 'running', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_start",
        &actor.0,
        Some(&format!("name={}", db.name)),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "message": "Database started" }),
        )),
    ))
}

// ── Update database (custom URL / image overrides / memory limit) ──

pub async fn update_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    // Normalise: treat an empty string as "clear the override".
    let new_external = normalized_optional_override(req.custom_external_url);
    let new_internal = normalized_optional_override(req.custom_internal_url);
    let new_image = normalized_optional_override(req.custom_image);

    // Apply only the fields that were present in the request body.
    let ext_url = match new_external {
        Some(val) => val,
        None => db.custom_external_url.clone(),
    };
    let int_url = match new_internal {
        Some(val) => val,
        None => db.custom_internal_url.clone(),
    };
    let cust_image = match new_image {
        Some(val) => val,
        None => db.custom_image.clone(),
    };
    // memory_limit: None means "field not sent — leave unchanged".
    // Some(0) clears the limit; Some(n) sets n bytes.
    let mem_limit = next_memory_limit(db.memory_limit, req.memory_limit);

    sqlx::query(
        "UPDATE databases
         SET custom_external_url = ?,
             custom_internal_url  = ?,
             custom_image         = ?,
             memory_limit         = ?,
             updated_at           = datetime('now')
         WHERE id = ?",
    )
    .bind(&ext_url)
    .bind(&int_url)
    .bind(&cust_image)
    .bind(mem_limit)
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_update",
        &actor.0,
        Some(&format!("name={}", db.name)),
    )
    .await;

    // Re-fetch and return the updated detail.
    let updated_db = fetch_db(&state.db, &db.id).await?;
    let detail = build_detail(updated_db);
    Ok((StatusCode::OK, Json(ApiResponse::ok(detail))))
}

// ── Redeploy database ──

/// Recreate the database container, optionally using a custom image.
///
/// This is distinct from `start_database` which simply calls `docker start` on
/// the existing container. `redeploy_database`:
///   1. Stops the old container (best-effort).
///   2. Removes the old container (best-effort), freeing the named slot.
///   3. Reuses the existing host port (or finds a free one on first deploy)
///      so that connection strings baked into apps remain valid.
///   4. Creates a fresh container with the same volume, credentials, and
///      network settings — using `custom_image` if set, otherwise the engine
///      default.
///   5. Updates `container_id`, `host_port`, `status`, and `image_used` in the DB.
pub async fn redeploy_database(
    _rate: RateLimit,
    _auth: RequireAuth,
    _actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    let engine = engine_for(&db.db_type).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "Unknown db_type '{}'",
            db.db_type
        )))
    })?;

    // 1 + 2. Stop and remove the old container (best-effort — don't fail if
    //         the container is already gone).
    if let Some(ref cid) = db.container_id {
        let _ = docker::stop_container(&state.docker, cid).await;
        let _ = docker::remove_container(&state.docker, cid).await;
    }

    // 3. Reuse the existing host port so that any connection strings baked into
    //    apps (env vars, etc.) remain valid across redeployments.  Only pick a
    //    fresh port when there is no existing one (first-time creation, which
    //    should not normally go through this path, but handle it defensively).
    let host_port: u16 = match db.host_port {
        Some(p) => p as u16,
        None => docker::find_free_port(&state.docker, 5400)
            .await
            .map_err(|e| AppError(TondError::DockerError(e.to_string())))?,
    };

    // 4. Choose the image: custom override takes precedence over the engine default.
    let image: &str = db.custom_image.as_deref().unwrap_or(engine.image);

    let env_vars = engine.build_env(
        db.db_name.as_deref(),
        db.db_user.as_deref(),
        db.db_password.as_deref(),
        db.root_password.as_deref(),
    );

    let volume_path = db.volume_path.clone().unwrap_or_else(|| {
        let data_dir =
            std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
        format!("{}/databases/{}", data_dir, db.name)
    });
    let bind_path = format!("{}:{}", volume_path, engine.data_dir);

    // Reconnect to project network if applicable.
    let network_name_owned;
    let network_name: Option<&str> = if let Some(ref pid) = db.project_id {
        network_name_owned = format!("tond-{}", pid);
        docker::ensure_network(&state.docker, &network_name_owned)
            .await
            .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
        Some(&network_name_owned)
    } else {
        None
    };

    let container_name = format!("tond-db-{}", db.name);
    let db_name_alias = db.name.clone();

    let new_container_id = docker::run_container_named(
        &state.docker,
        image,
        &container_name,
        engine.container_port,
        host_port,
        env_vars,
        db.memory_limit,
        None,
        vec![bind_path],
        network_name,
        Some(&db_name_alias),
    )
    .await
    .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    // 5. Persist the new container details.
    sqlx::query(
        "UPDATE databases
         SET container_id = ?,
             host_port    = ?,
             status       = 'running',
             image_used   = ?,
             updated_at   = datetime('now')
         WHERE id = ?",
    )
    .bind(&new_container_id)
    .bind(host_port as i64)
    .bind(image)
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    let updated_db = fetch_db(&state.db, &db.id).await?;
    let detail = build_detail(updated_db);
    Ok((StatusCode::OK, Json(ApiResponse::ok(detail))))
}

// ── Update credentials ──

/// Update the stored username / password (and, for SQL engines, database name).
///
/// Changes are persisted immediately but only take effect in the container on
/// the next `redeploy` — the running container is not restarted automatically.
pub async fn update_database_credentials(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDatabaseCredentials>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    // Apply only the fields that were supplied.
    let new_db_name = next_credential(db.db_name.clone(), req.db_name);
    let new_db_user = next_credential(db.db_user.clone(), req.db_user);
    let new_db_password = next_credential(db.db_password.clone(), req.db_password);
    let new_root_password = next_credential(db.root_password.clone(), req.root_password);

    sqlx::query(
        "UPDATE databases
         SET db_name       = ?,
             db_user       = ?,
             db_password   = ?,
             root_password = ?,
             updated_at    = datetime('now')
         WHERE id = ?",
    )
    .bind(&new_db_name)
    .bind(&new_db_user)
    .bind(&new_db_password)
    .bind(&new_root_password)
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_credentials_update",
        &actor.0,
        Some(&format!("name={}", db.name)),
    )
    .await;

    let updated_db = fetch_db(&state.db, &db.id).await?;
    let detail = build_detail(updated_db);
    // Credentials only take effect after a redeploy — signal this explicitly so
    // the dashboard can surface a "redeploy required" banner.
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "database": detail,
            "requires_redeploy": true,
        }))),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::to_bytes,
        extract::{Path, State},
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{ServerSettings, UpdateDatabaseCredentials, UpdateDatabaseRequest};

    use tond_common::ManagedDatabase;

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
            panic!("failed to run migrations for database handler tests: {err}")
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

    async fn seed_database_row(state: &AppState, db: &ManagedDatabase) {
        sqlx::query(
            "INSERT INTO databases (id, name, db_type, project_id, container_id, host_port, status, db_name, db_user, db_password, root_password, volume_path, memory_limit, custom_external_url, custom_internal_url, image_used, custom_image, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&db.id)
        .bind(&db.name)
        .bind(&db.db_type)
        .bind(&db.project_id)
        .bind(&db.container_id)
        .bind(db.host_port)
        .bind(&db.status)
        .bind(&db.db_name)
        .bind(&db.db_user)
        .bind(&db.db_password)
        .bind(&db.root_password)
        .bind(&db.volume_path)
        .bind(db.memory_limit)
        .bind(&db.custom_external_url)
        .bind(&db.custom_internal_url)
        .bind(&db.image_used)
        .bind(&db.custom_image)
        .bind(&db.created_at)
        .bind(&db.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed managed database row: {err}"));
    }

    fn sample_db() -> ManagedDatabase {
        ManagedDatabase {
            id: "db-1".to_string(),
            name: "postgres-main".to_string(),
            db_type: "postgres".to_string(),
            project_id: Some("project-1".to_string()),
            container_id: Some("container-1".to_string()),
            host_port: Some(5433),
            status: "running".to_string(),
            db_name: Some("appdb".to_string()),
            db_user: Some("appuser".to_string()),
            db_password: Some("secret".to_string()),
            root_password: None,
            volume_path: Some("/data/postgres-main".to_string()),
            memory_limit: Some(512 * 1024 * 1024),
            custom_external_url: None,
            custom_internal_url: None,
            image_used: Some("postgres:16-alpine".to_string()),
            custom_image: None,
            created_at: "2026-04-08T12:00:00Z".to_string(),
            updated_at: "2026-04-08T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn build_detail_prefers_custom_urls_and_computes_engine_urls_when_missing() {
        let detail = build_detail(sample_db());
        assert_eq!(
            detail.connection_string.as_deref(),
            Some("postgres://appuser:secret@localhost:5433/appdb"),
            "build_detail should compute an external connection string from the engine config when no override is set"
        );
        assert_eq!(
            detail.internal_connection_string.as_deref(),
            Some("postgres://appuser:secret@postgres-main:5432/appdb"),
            "build_detail should compute an internal project-network connection string when the database belongs to a project"
        );

        let mut db = sample_db();
        db.custom_external_url = Some("postgres://external.example.com/db".to_string());
        db.custom_internal_url = Some("postgres://internal.example.com/db".to_string());
        let detail = build_detail(db);
        assert_eq!(
            detail.connection_string.as_deref(),
            Some("postgres://external.example.com/db"),
            "build_detail should prefer a custom external URL override over the computed connection string"
        );
        assert_eq!(
            detail.internal_connection_string.as_deref(),
            Some("postgres://internal.example.com/db"),
            "build_detail should prefer a custom internal URL override over the computed project-network connection string"
        );
    }

    #[test]
    fn normalized_override_and_memory_helpers_apply_patch_semantics() {
        assert_eq!(
            normalized_optional_override(Some("  postgres://db  ".to_string())),
            Some(Some("postgres://db".to_string())),
            "normalized_optional_override should trim provided override values"
        );
        assert_eq!(
            normalized_optional_override(Some("   ".to_string())),
            Some(None),
            "normalized_optional_override should treat an empty string as a request to clear the override"
        );
        assert_eq!(
            normalized_optional_override(None),
            None,
            "normalized_optional_override should preserve omitted fields as 'leave unchanged'"
        );

        assert_eq!(
            next_memory_limit(Some(512), None),
            Some(512),
            "next_memory_limit should leave the current value unchanged when memory_limit is omitted"
        );
        assert_eq!(
            next_memory_limit(Some(512), Some(1024)),
            Some(1024),
            "next_memory_limit should apply a positive replacement value"
        );
        assert_eq!(
            next_memory_limit(Some(512), Some(0)),
            None,
            "next_memory_limit should clear the memory limit when the request sends zero"
        );
    }

    #[test]
    fn next_credential_keeps_existing_value_on_empty_input_and_updates_non_empty_values() {
        assert_eq!(
            next_credential(Some("current".to_string()), None),
            Some("current".to_string()),
            "next_credential should preserve the current value when the field is omitted"
        );
        assert_eq!(
            next_credential(Some("current".to_string()), Some("   ".to_string())),
            Some("current".to_string()),
            "next_credential should preserve the current value when the request sends an empty string"
        );
        assert_eq!(
            next_credential(Some("current".to_string()), Some(" next ".to_string())),
            Some("next".to_string()),
            "next_credential should trim and apply non-empty replacement values"
        );
    }

    #[test]
    fn fetch_db_error_mentions_missing_identifier() {
        let err = fetch_db_error("missing-db");
        assert!(
            err.0.to_string().contains("database 'missing-db'"),
            "missing database lookups should report the missing identifier in the error message"
        );
    }

    #[tokio::test]
    async fn update_database_persists_patch_fields_and_clears_empty_overrides() {
        let state = test_state().await;
        let mut db = sample_db();
        db.project_id = None;
        seed_database_row(&state, &db).await;

        let response = update_database(
            RateLimit,
            RequireAuth,
            AuthActor("admin@example.com".to_string()),
            State(state.clone()),
            Path(db.id.clone()),
            Json(UpdateDatabaseRequest {
                custom_external_url: Some("postgres://db.example.com/appdb".to_string()),
                custom_internal_url: Some("   ".to_string()),
                custom_image: Some("postgres:17-alpine".to_string()),
                memory_limit: Some(0),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("update_database should succeed for a seeded row"))
        .into_response();

        let body = response_json(response).await;
        assert_eq!(
            body["data"]["connection_string"],
            serde_json::Value::String("postgres://db.example.com/appdb".to_string()),
            "update_database should return the updated custom external connection string"
        );
        assert_eq!(
            body["data"]["custom_internal_url"],
            serde_json::Value::Null,
            "update_database should clear custom overrides when the request sends an empty string"
        );
        assert_eq!(
            body["data"]["custom_image"],
            serde_json::Value::String("postgres:17-alpine".to_string()),
            "update_database should persist a custom image override"
        );
        assert_eq!(
            body["data"]["memory_limit"],
            serde_json::Value::Null,
            "update_database should clear the memory limit when the request sends zero"
        );
    }

    #[tokio::test]
    async fn update_database_credentials_preserves_empty_inputs_and_flags_redeploy_required() {
        let state = test_state().await;
        let mut db = sample_db();
        db.project_id = None;
        db.root_password = Some("root-secret".to_string());
        seed_database_row(&state, &db).await;

        let response = update_database_credentials(
            RateLimit,
            RequireAuth,
            AuthActor("admin@example.com".to_string()),
            State(state.clone()),
            Path(db.id.clone()),
            Json(UpdateDatabaseCredentials {
                db_name: Some(" next_db ".to_string()),
                db_user: Some("   ".to_string()),
                db_password: Some("new-secret".to_string()),
                root_password: Some(" ".to_string()),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("update_database_credentials should succeed for a seeded row"))
        .into_response();

        let body = response_json(response).await;
        assert_eq!(
            body["data"]["requires_redeploy"],
            serde_json::Value::Bool(true),
            "update_database_credentials should explicitly flag that a redeploy is required"
        );
        assert_eq!(
            body["data"]["database"]["db_name"],
            serde_json::Value::String("next_db".to_string()),
            "update_database_credentials should trim and persist a supplied database name"
        );
        assert_eq!(
            body["data"]["database"]["db_user"],
            serde_json::Value::String("appuser".to_string()),
            "update_database_credentials should preserve the existing username when given an empty string"
        );
        assert_eq!(
            body["data"]["database"]["db_password"],
            serde_json::Value::String("new-secret".to_string()),
            "update_database_credentials should update the stored application password"
        );
        assert_eq!(
            body["data"]["database"]["root_password"],
            serde_json::Value::String("root-secret".to_string()),
            "update_database_credentials should preserve the root password when given an empty string"
        );
    }
}
