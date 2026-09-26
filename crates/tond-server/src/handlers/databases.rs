// handlers/databases.rs — Managed Database handlers (Phase 6.3)
//
// Creates and manages Docker containers for postgres / mysql / redis databases.
// Each database gets a persistent volume, a randomly-assigned host port, and
// optional connection to a project Docker network for internal service-to-service
// communication.
//
// Routes (all require RateLimit + an Authz scope, see each handler):
//   GET    /api/databases                         — list all managed databases
//   POST   /api/databases                         — create and start a new database container
//   GET    /api/databases/{id}                    — get database details + connection strings
//   PATCH  /api/databases/{id}                    — update custom URL / image overrides
//   DELETE /api/databases/{id}                    — stop+remove container, delete row
//   POST   /api/databases/{id}/stop               — stop container
//   POST   /api/databases/{id}/start              — start container (existing container)
//   POST   /api/databases/{id}/redeploy           — recreate container (picks up custom_image)
//   PATCH  /api/databases/{id}/credentials        — change passwords inside the running database
//   GET    /api/databases/{id}/credentials        — reveal passwords + connection strings (admin, audited)
//
// Secrets at rest: passwords and connection-string overrides are stored
// encrypted (`secrets::seal_database`); `fetch_db` returns them decrypted for
// use inside this module. Responses never carry them: passwords become
// `has_password` and connection strings are masked, except in the audited
// credentials reveal.
//
// Network exposure (H6): database ports are published on 127.0.0.1 unless an
// admin turns on `public_access`, which publishes on 0.0.0.0. Docker's iptables
// rules bypass UFW, so a 0.0.0.0 port is reachable from the internet even with
// the firewall "closed" — hence off by default and admin-only.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use crate::{
    audit_log::write_audit_log,
    auth::{Authz, RateLimit, scope},
    db_credentials::{CredentialField, container_running, password_change_step, run_step},
    docker,
    error::AppError,
    networks, secrets,
    state::AppState,
};

// ── Shared lookup helper ──

/// Load a database with its secret columns decrypted.
pub(crate) async fn fetch_db(
    pool: &sqlx::SqlitePool,
    id_or_name: &str,
) -> Result<ManagedDatabase, AppError> {
    let db: Option<ManagedDatabase> =
        sqlx::query_as("SELECT * FROM databases WHERE id = ? OR name = ? LIMIT 1")
            .bind(id_or_name)
            .bind(id_or_name)
            .fetch_optional(pool)
            .await?;
    let db = db.ok_or_else(|| fetch_db_error(id_or_name))?;
    Ok(secrets::open_database(db)?)
}

// ── List databases ──

pub async fn list_databases(
    _rate: RateLimit,
    _auth: Authz<scope::DbRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let dbs: Vec<ManagedDatabase> =
        sqlx::query_as("SELECT * FROM databases ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;
    let mut responses: Vec<ManagedDatabaseResponse> = Vec::with_capacity(dbs.len());
    for db in dbs {
        responses.push(ManagedDatabaseResponse::from(secrets::open_database(db)?));
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── Get database (with connection strings) ──

pub async fn get_database(
    _rate: RateLimit,
    _auth: Authz<scope::DbRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    let full_snapshots: bool =
        sqlx::query_scalar("SELECT enabled != 0 FROM backup_config WHERE id = 1")
            .fetch_optional(&state.db)
            .await?
            .unwrap_or(false);
    let warning = crate::handlers::database_ops::backup_warning(
        &db.name,
        db.backup_schedule.as_deref(),
        full_snapshots,
    );
    let mut detail = build_detail(db, public_host(&state).await.as_deref());
    detail.backup_warning = warning;
    Ok((StatusCode::OK, Json(ApiResponse::ok(detail))))
}

// Responses never carry passwords, whatever the caller's scopes: they are
// write-only (`build_detail` masks connection strings) and readable only
// through the audited `reveal_database_credentials` (db:credentials).

/// GET /api/databases/{id}/credentials — the plaintext passwords and
/// connection strings, for copying into an app or a client. Needs
/// db:credentials; every reveal is audit-logged.
pub async fn reveal_database_credentials(
    _rate: RateLimit,
    auth: Authz<scope::DbCredentials>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    let credentials = credentials_response(&db, public_host(&state).await.as_deref());
    write_audit_log(
        &state.db,
        None,
        None,
        "db_credentials_reveal",
        &auth.audit_actor(),
        Some(&format!("name={}", db.name)),
    )
    .await;
    Ok((StatusCode::OK, Json(ApiResponse::ok(credentials))))
}

/// Placeholder host shown in public connection strings when the server has no
/// dashboard domain configured yet.
const SERVER_ADDRESS_PLACEHOLDER: &str = "<server-ip>";

/// The address clients on the internet use to reach this server: the
/// dashboard domain when configured.
async fn public_host(state: &AppState) -> Option<String> {
    let domain = state.settings.read().await.dashboard_domain.clone();
    let domain = domain.trim();
    (!domain.is_empty()).then(|| domain.to_string())
}

// ── Redis / KeyDB authentication ──
//
// The redis and keydb images ignore REDIS_PASSWORD, so without extra config a
// managed Redis would accept anyone on the bridge network (or the internet with
// public access on). Tond writes an ACL config next to the data volume —
// `<data>/databases/.<name>.redis.conf`, outside the directory mounted as
// /data — and starts the server with it, mounted read-only:
//
//   user default off                        (unauthenticated clients get NOAUTH)
//   user <db_user> on ><password> ~* &* +@all
//
// The password lives in that file rather than on the command line, so it
// doesn't show up in `ps` on the host. Connection strings already carry
// `<db_user>:<password>`, which is exactly what Redis 6+ ACL clients send.

/// Mount point of the generated config inside the container.
const REDIS_CONFIG_MOUNT: &str = "/etc/tond/redis.conf";

fn is_redis_like(db_type: &str) -> bool {
    matches!(db_type, "redis" | "keydb")
}

/// ACL usernames/passwords go into a whitespace-separated config line, so they
/// must be plain tokens.
fn validate_redis_credential(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("Redis {what} cannot be empty"));
    }
    if value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\' || c == '#')
    {
        return Err(format!(
            "Redis {what} may not contain spaces, quotes, backslashes or '#'"
        ));
    }
    Ok(())
}

/// The generated ACL config for a Redis-like database.
fn redis_config(db_user: &str, db_password: &str) -> Result<String, String> {
    validate_redis_credential("username", db_user)?;
    validate_redis_credential("password", db_password)?;
    Ok(format!(
        "# Generated by Tond — edits are overwritten on redeploy.\n\
         dir /data\n\
         user default off\n\
         user {db_user} on >{db_password} ~* &* +@all\n"
    ))
}

/// What a database container runs.
pub(crate) struct ContainerSpec {
    pub image: String,
    pub env: std::collections::HashMap<String, String>,
    pub binds: Vec<String>,
    pub cmd: Option<Vec<String>>,
    pub port: u16,
}

/// The data directory of a database on the host.
pub(crate) fn volume_path_for(db: &ManagedDatabase) -> String {
    db.volume_path.clone().unwrap_or_else(|| {
        let data_dir =
            std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
        format!("{}/databases/{}", data_dir, db.name)
    })
}

/// The image a database runs: an admin's override, else the catalog image of
/// its engine, variant and version.
pub(crate) fn image_of(db: &ManagedDatabase) -> Result<String, AppError> {
    if let Some(custom) = &db.custom_image {
        return Ok(custom.clone());
    }
    db.catalog_image().map(str::to_string).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "No image for {} {} {}",
            db.db_type,
            db.variant,
            db.effective_version()
        )))
    })
}

/// Image, env, binds and command for a database whose data lives in
/// `volume_path`, running `image`.
pub(crate) async fn container_spec(
    db: &ManagedDatabase,
    volume_path: &str,
    image: &str,
) -> Result<ContainerSpec, AppError> {
    let engine = engine_for(&db.db_type).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "Unknown db_type '{}'",
            db.db_type
        )))
    })?;
    let mut env = engine.build_env(
        db.db_name.as_deref(),
        db.db_user.as_deref(),
        db.db_password.as_deref(),
        db.root_password.as_deref(),
    );
    if db.db_type == "postgres" {
        // PostgreSQL 18 images keep data in a versioned directory by default;
        // pin the layout Tond mounts for every version.
        env.insert("PGDATA".to_string(), engine.data_dir.to_string());
    }
    let mut binds = vec![format!("{}:{}", volume_path, engine.data_dir)];
    let (extra_binds, mut cmd) = engine_runtime(db, volume_path).await?;
    binds.extend(extra_binds);
    if db.tls && supports_tls(&db.db_type) {
        let dir = crate::db_access::tls_dir(volume_path, &db.name);
        binds.push(format!("{dir}:{}:ro", crate::db_access::TLS_MOUNT));
        cmd = Some(crate::db_access::tls_command(&db.db_type));
    }
    Ok(ContainerSpec {
        image: image.to_string(),
        env,
        binds,
        cmd,
        port: engine.container_port,
    })
}

/// Create and start the container of `db` (its name, home network, alias,
/// memory cap) publishing `host_port`. Returns the container id.
pub(crate) async fn run_database_container(
    state: &AppState,
    db: &ManagedDatabase,
    host_port: u16,
    image: &str,
) -> Result<String, AppError> {
    let volume_path = volume_path_for(db);
    let _ = tokio::fs::create_dir_all(&volume_path).await;
    if db.tls && supports_tls(&db.db_type) {
        crate::db_access::write_tls_files(state, &volume_path, &db.name)
            .await
            .map_err(|e| {
                AppError(TondError::Internal(format!(
                    "Couldn't prepare the TLS certificate: {e:#}"
                )))
            })?;
    }
    let spec = container_spec(db, &volume_path, image).await?;
    // Home network: the project network, or the database's own network.
    // Apps outside the project get access through explicit links.
    let network = networks::ensure_database_home(&state.docker, db)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
    docker::run_container_named(
        &state.docker,
        &spec.image,
        &format!("tond-db-{}", db.name),
        spec.port,
        host_port,
        exposure_for(db),
        spec.env,
        db.memory_limit,
        None, // no cpu limit for databases
        spec.binds,
        &network,
        Some(&db.name),
        spec.cmd,
    )
    .await
    .map_err(|e| AppError(TondError::DockerError(e.to_string())))
}

/// Extra binds and command for an engine. For Redis/KeyDB this writes the ACL
/// config (0644 so the container's redis user can read it; its parent
/// `databases/` is root-only on the host) and returns the server command.
async fn engine_runtime(
    db: &ManagedDatabase,
    volume_path: &str,
) -> Result<(Vec<String>, Option<Vec<String>>), AppError> {
    if !is_redis_like(&db.db_type) {
        return Ok((Vec::new(), None));
    }
    let config = redis_config(
        db.db_user.as_deref().unwrap_or_default(),
        db.db_password.as_deref().unwrap_or_default(),
    )
    .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    let config_path = redis_config_path(volume_path, &db.name);
    tokio::fs::write(&config_path, config)
        .await
        .map_err(|e| AppError(TondError::Internal(format!("writing Redis config: {e}"))))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            tokio::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).await;
    }
    let server = if db.db_type == "keydb" {
        "keydb-server"
    } else {
        "redis-server"
    };
    Ok((
        vec![format!("{config_path}:{REDIS_CONFIG_MOUNT}:ro")],
        Some(vec![server.to_string(), REDIS_CONFIG_MOUNT.to_string()]),
    ))
}

/// `<data>/databases/.<name>.redis.conf` — a sibling of the volume directory.
pub(crate) fn redis_config_path(volume_path: &str, name: &str) -> String {
    let parent = std::path::Path::new(volume_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    format!("{parent}/.{name}.redis.conf")
}

/// Which interface the database port is published on.
fn exposure_for(db: &ManagedDatabase) -> docker::Exposure {
    if db.public_access {
        docker::Exposure::Public
    } else {
        docker::Exposure::Loopback
    }
}

/// Plaintext connection strings of a (decrypted) database.
struct ConnectionStrings {
    external: Option<String>,
    public: Option<String>,
    internal: Option<String>,
}

/// Build a `ManagedDatabaseDetail` from a decrypted `ManagedDatabase` row,
/// with masked connection strings (see `connection_strings`).
fn build_detail(db: ManagedDatabase, public_host: Option<&str>) -> ManagedDatabaseDetail {
    let strings = connection_strings(&db, public_host);
    let mask = |s: Option<String>| s.as_deref().map(mask_url_password);
    let upgrade_targets = if db.custom_image.is_some() || db.upgrade.is_some() {
        Vec::new()
    } else {
        tond_common::upgrade_targets(&db.db_type, &db.variant, &db.effective_version())
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let firewall = crate::db_access::firewall_status();
    let access_warning = crate::db_access::access_warning(&db, &firewall);
    let allowlist_enforced = db.public_access
        && !db.allowlist_entries().is_empty()
        && firewall.available
        && firewall.applied_ports.contains(&db.host_port.unwrap_or(-1));
    let supports_tls = supports_tls(&db.db_type);
    ManagedDatabaseDetail {
        database: ManagedDatabaseResponse::from(db),
        connection_string: mask(strings.external),
        public_connection_string: mask(strings.public),
        internal_connection_string: mask(strings.internal),
        upgrade_targets,
        access_warning,
        allowlist_enforced,
        supports_tls,
        backup_warning: None,
    }
}

/// Compute the external, public and internal connection strings.
/// If the database row has custom URL overrides set, those take precedence
/// over the auto-computed strings. `public_host` is the server's public
/// address, used for the public connection string when public access is on.
fn connection_strings(db: &ManagedDatabase, public_host: Option<&str>) -> ConnectionStrings {
    let public_connection_string = if db.public_access {
        db.host_port.and_then(|port| {
            engine_for(&db.db_type).map(|eng| {
                let url = eng.connection_string_on(
                    public_host.unwrap_or(SERVER_ADDRESS_PLACEHOLDER),
                    port,
                    db.db_name.as_deref(),
                    db.db_user.as_deref(),
                    db.db_password.as_deref(),
                );
                with_tls_param(&db.db_type, db.tls, url)
            })
        })
    } else {
        None
    };

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

    // Internal string: use override if set, otherwise compute. Apps in the
    // database's project and linked apps reach it by its name (networks.rs).
    let internal_connection_string = if let Some(ref custom) = db.custom_internal_url {
        Some(custom.clone())
    } else {
        engine_for(&db.db_type).map(|eng| {
            eng.internal_connection_string(
                &db.name,
                db.db_name.as_deref(),
                db.db_user.as_deref(),
                db.db_password.as_deref(),
            )
        })
    };

    ConnectionStrings {
        external: connection_string,
        public: public_connection_string,
        internal: internal_connection_string,
    }
}

/// Ask clients to require TLS when the database serves it: `sslmode=require`
/// (libpq and most PostgreSQL drivers), `ssl-mode=REQUIRED` (MySQL clients).
fn with_tls_param(db_type: &str, tls: bool, url: String) -> String {
    if !tls {
        return url;
    }
    let param = match db_type {
        "postgres" => "sslmode=require",
        "mysql" | "mariadb" => "ssl-mode=REQUIRED",
        _ => return url,
    };
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{param}")
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
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.public_access {
        auth.require_to(Scope::SettingsWrite, "expose a database to the internet")?;
    }

    validate_database_name(&req.name)?;

    // Validate db_type against the engine registry — this is the only place
    // where the allowed set needs to be checked. Adding a new engine to
    // ALL_ENGINES in tond-common automatically unlocks it here.
    engine_for(&req.db_type).ok_or_else(|| {
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
    let variant = req.variant.clone().unwrap_or_default();
    let version = req.version.clone().filter(|v| !v.trim().is_empty());
    check_image_choice(&req.db_type, &variant, version.as_deref())?;
    let backup_schedule = normalize_schedule(req.backup_schedule.as_deref())?;

    // Verify project exists if provided
    if let Some(ref pid) = req.project_id {
        ensure_project_exists(&state, pid).await?;
    }

    let mut db = ManagedDatabase::new(
        req.name.clone(),
        req.db_type.clone(),
        req.project_id.clone(),
    );
    if let Some(mem) = req.memory_limit {
        db.memory_limit = Some(mem);
    }
    db.public_access = req.public_access;
    db.variant = variant;
    db.version = version.or_else(|| default_version(&db.db_type).map(str::to_string));
    db.backup_schedule = backup_schedule;

    provision_database(&state, &mut db).await?;
    if db.backup_schedule.is_some() {
        crate::backup::schedules_changed();
    }

    write_audit_log(
        &state.db,
        None,
        None,
        "db_create",
        &auth.audit_actor(),
        Some(&format!(
            "name={} type={} image={} public_access={}",
            db.name,
            db.db_type,
            db.image_used.as_deref().unwrap_or_default(),
            db.public_access
        )),
    )
    .await;

    // The creator gets the generated credentials once (when allowed to read
    // credentials at all); later reads are masked.
    let host = public_host(&state).await;
    let credentials = auth
        .has(Scope::DbCredentials)
        .then(|| credentials_response(&db, host.as_deref()));
    let detail = build_detail(db, host.as_deref());
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(CreatedDatabaseResponse {
            detail,
            credentials,
        })),
    ))
}

/// Database names become container names, network aliases and directory
/// names: alphanumeric and hyphens only.
pub(crate) fn validate_database_name(name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || name.len() > 63
        || name.starts_with('-')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(AppError(TondError::InvalidInput(
            "Database name must be alphanumeric (hyphens allowed)".to_string(),
        )));
    }
    Ok(())
}

/// Refuse a version or variant the catalog doesn't have, and x86_64-only
/// images on other servers.
fn check_image_choice(db_type: &str, variant: &str, version: Option<&str>) -> Result<(), AppError> {
    if !variant.is_empty() {
        let v = variant_for(db_type, variant).ok_or_else(|| {
            AppError(TondError::InvalidInput(format!(
                "Unknown variant '{variant}' for {db_type}"
            )))
        })?;
        if v.amd64_only && std::env::consts::ARCH != "x86_64" {
            return Err(AppError(TondError::InvalidInput(format!(
                "{} is published for x86_64 servers only; this server is {}.",
                v.label,
                std::env::consts::ARCH
            ))));
        }
    }
    if image_for(db_type, variant, version).is_none() {
        let list = versions_for(db_type, variant).join(", ");
        return Err(AppError(TondError::InvalidInput(format!(
            "Version {} isn't available. Choose one of: {list}.",
            version.unwrap_or("?")
        ))));
    }
    Ok(())
}

/// A trimmed cron expression that the scheduler accepts, or None for "off".
pub(crate) fn normalize_schedule(schedule: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(s) = schedule.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    crate::cron::parse_schedule(s).map_err(|e| {
        AppError(TondError::InvalidInput(format!(
            "Invalid backup schedule: {e}"
        )))
    })?;
    Ok(Some(s.to_string()))
}

async fn ensure_project_exists(state: &AppState, project_id: &str) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?)")
        .bind(project_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(AppError(TondError::NotFound(format!(
            "project '{project_id}'"
        ))));
    }
    Ok(())
}

/// Start a new database's container and store its row (secrets encrypted).
/// Sets the volume path, port, container, status and image on `db`. The name
/// is claimed first, so a duplicate fails before anything is started.
pub(crate) async fn provision_database(
    state: &AppState,
    db: &mut ManagedDatabase,
) -> Result<(), AppError> {
    let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM databases WHERE name = ?)")
        .bind(&db.name)
        .fetch_one(&state.db)
        .await?;
    if taken {
        return Err(AppError(TondError::AppAlreadyExists(db.name.clone())));
    }

    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    db.volume_path = Some(format!("{}/databases/{}", data_dir, db.name));

    // Find a free port starting from 5400
    let host_port = docker::find_free_port(&state.docker, 5400)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
    let image = image_of(db)?;
    let container_id = run_database_container(state, db, host_port, &image).await?;

    db.container_id = Some(container_id);
    db.host_port = Some(host_port as i64);
    db.status = "running".to_string();
    db.image_used = Some(image);

    let sealed = secrets::seal_database(db)?;
    let inserted = sqlx::query(
        "INSERT INTO databases
            (id, name, db_type, project_id, container_id, host_port, status,
             db_name, db_user, db_password, root_password, volume_path, memory_limit,
             custom_external_url, custom_internal_url,
             image_used, created_at, updated_at, public_access,
             backup_schedule, backup_retention, version, variant, allowlist, tls)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(&sealed.db_password)
    .bind(&sealed.root_password)
    .bind(&db.volume_path)
    .bind(db.memory_limit)
    .bind(&sealed.custom_external_url)
    .bind(&sealed.custom_internal_url)
    .bind(&db.image_used)
    .bind(&db.created_at)
    .bind(&db.updated_at)
    .bind(db.public_access)
    .bind(&db.backup_schedule)
    .bind(db.backup_retention)
    .bind(&db.version)
    .bind(&db.variant)
    .bind(&db.allowlist)
    .bind(db.tls)
    .execute(&state.db)
    .await;
    if let Err(e) = inserted {
        // Don't leave an orphaned container behind.
        if let Some(cid) = &db.container_id {
            let _ = docker::remove_container(&state.docker, cid).await;
        }
        return Err(if e.to_string().contains("UNIQUE") {
            AppError(TondError::AppAlreadyExists(db.name.clone()))
        } else {
            AppError(TondError::DatabaseError(e))
        });
    }
    Ok(())
}

/// Remove a database's container, own network, data directory, generated
/// config and row (best effort for everything but the row).
pub(crate) async fn destroy_database(
    state: &AppState,
    db: &ManagedDatabase,
) -> Result<(), AppError> {
    if let Some(ref cid) = db.container_id {
        let _ = docker::remove_container(&state.docker, cid).await;
    }
    // Its own network (a project network belongs to the project); links go
    // with the row (ON DELETE CASCADE).
    if db.project_id.is_none() {
        docker::remove_network(&state.docker, &networks::database_network(&db.name)).await;
    }
    if let Some(ref vol) = db.volume_path {
        let _ = tokio::fs::remove_dir_all(vol).await;
        let _ = tokio::fs::remove_file(redis_config_path(vol, &db.name)).await;
        let _ = tokio::fs::remove_dir_all(crate::db_access::tls_dir(vol, &db.name)).await;
    }
    sqlx::query("DELETE FROM databases WHERE id = ?")
        .bind(&db.id)
        .execute(&state.db)
        .await?;
    Ok(())
}
fn credentials_response(
    db: &ManagedDatabase,
    public_host: Option<&str>,
) -> DatabaseCredentialsResponse {
    let strings = connection_strings(db, public_host);
    DatabaseCredentialsResponse {
        db_user: db.db_user.clone(),
        db_password: db.db_password.clone(),
        root_password: db.root_password.clone(),
        connection_string: strings.external,
        public_connection_string: strings.public,
        internal_connection_string: strings.internal,
    }
}

// ── Delete database ──

pub async fn delete_database(
    _rate: RateLimit,
    auth: Authz<scope::DbDelete>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    if db.upgrade_state().is_some_and(|u| u.status == "running") {
        return Err(AppError(TondError::Conflict(format!(
            "'{}' is being upgraded. Wait for the upgrade to finish, then delete it.",
            db.name
        ))));
    }

    destroy_database(&state, &db).await?;
    crate::db_upgrade::remove_upgrade_dirs(&db.name).await;
    if db.backup_schedule.is_some() {
        crate::backup::schedules_changed();
    }

    write_audit_log(
        &state.db,
        None,
        None,
        "db_delete",
        &auth.audit_actor(),
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
    auth: Authz<scope::DbWrite>,
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
        &auth.audit_actor(),
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
    auth: Authz<scope::DbWrite>,
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
        &auth.audit_actor(),
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
    auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDatabaseRequest>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    refuse_while_upgrading(&db)?;

    // A custom image runs arbitrary code with the database volume mounted, and
    // public access opens the port to the internet: both are admin decisions,
    // like who may connect (the allowlist) and how (TLS).
    if req.custom_image.is_some() {
        auth.require_to(Scope::SettingsWrite, "change a database's Docker image")?;
    }
    let exposure_changed = req.public_access.is_some_and(|p| p != db.public_access);
    if exposure_changed {
        auth.require_to(Scope::SettingsWrite, "change a database's public access")?;
    }
    let public_access = req.public_access.unwrap_or(db.public_access);

    let allowlist = match req.allowlist {
        Some(entries) => {
            auth.require_to(Scope::SettingsWrite, "change who may connect to a database")?;
            let normalized = crate::db_access::normalize_allowlist(&entries)
                .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
            if !normalized.is_empty() {
                crate::db_access::require_firewall()
                    .await
                    .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
            }
            serde_json::to_string(&normalized).unwrap_or_else(|_| "[]".to_string())
        }
        None => db.allowlist.clone(),
    };
    let tls_changed = req.tls.is_some_and(|t| t != db.tls);
    if tls_changed {
        auth.require_to(Scope::SettingsWrite, "change a database's TLS setting")?;
        if !supports_tls(&db.db_type) {
            return Err(AppError(TondError::InvalidInput(format!(
                "{} doesn't support TLS in Tond; use an SSH tunnel to encrypt connections.",
                db.db_type
            ))));
        }
    }
    let tls = req.tls.unwrap_or(db.tls);

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
             public_access        = ?,
             allowlist            = ?,
             tls                  = ?,
             updated_at           = datetime('now')
         WHERE id = ?",
    )
    .bind(secrets::seal_db_external_url(ext_url.as_deref())?)
    .bind(secrets::seal_db_internal_url(int_url.as_deref())?)
    .bind(&cust_image)
    .bind(mem_limit)
    .bind(public_access)
    .bind(&allowlist)
    .bind(tls)
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "db_update",
        &auth.audit_actor(),
        Some(&format!(
            "name={} public_access={} allowlist={} tls={}",
            db.name, public_access, allowlist, tls
        )),
    )
    .await;

    let mut updated_db = fetch_db(&state.db, &db.id).await?;

    // A port binding (or the TLS mount) can only change by recreating the
    // container. Do it now (same image, volume and port) so the change takes
    // effect immediately.
    if (exposure_changed || tls_changed) && updated_db.container_id.is_some() {
        updated_db = recreate_container(&state, updated_db).await?;
    }
    // Apply the allowlist now rather than on the access loop's next wake-up.
    crate::db_access::reconcile_now(&state).await;

    let detail = build_detail(updated_db, public_host(&state).await.as_deref());
    Ok((StatusCode::OK, Json(ApiResponse::ok(detail))))
}

/// Refuse changes that recreate the container while an upgrade is running.
fn refuse_while_upgrading(db: &ManagedDatabase) -> Result<(), AppError> {
    if db.upgrade_state().is_some_and(|u| u.status == "running") {
        return Err(AppError(TondError::Conflict(format!(
            "'{}' is being upgraded. Wait for the upgrade to finish.",
            db.name
        ))));
    }
    Ok(())
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
    _auth: Authz<scope::DbWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;
    refuse_while_upgrading(&db)?;
    let updated_db = recreate_container(&state, db).await?;
    let detail = build_detail(updated_db, public_host(&state).await.as_deref());
    Ok((StatusCode::OK, Json(ApiResponse::ok(detail))))
}

/// Stop, remove and recreate a database container (steps 1–5 above) and
/// return the refreshed row.
pub(crate) async fn recreate_container(
    state: &AppState,
    db: ManagedDatabase,
) -> Result<ManagedDatabase, AppError> {
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

    // 4. Choose the image: custom override, else the catalog image of the
    //    engine, variant and version.
    let image = image_of(&db)?;
    let new_container_id = run_database_container(state, &db, host_port, &image).await?;

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
    .bind(&image)
    .bind(&db.id)
    .execute(&state.db)
    .await?;

    let updated = fetch_db(&state.db, &db.id).await?;
    networks::reconcile_database(state, &updated)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
    Ok(updated)
}

// ── Update credentials ──

/// Refuse a rename of the database or user: engines only apply those names when
/// they initialise an empty data directory, so the stored and real names would
/// drift apart.
fn refuse_rename(
    label: &str,
    current: Option<&str>,
    requested: Option<&str>,
) -> Result<(), AppError> {
    let requested = requested.map(str::trim).filter(|v| !v.is_empty());
    match requested {
        Some(next) if Some(next) != current => Err(AppError(TondError::InvalidInput(format!(
            "The {label} of an existing database can't be changed; only passwords can. \
             Create a new user or database from the database shell instead."
        )))),
        _ => Ok(()),
    }
}

/// Change the application and/or root password inside the running database,
/// then persist it.
///
/// The change runs in the engine itself (ALTER ROLE / ALTER USER /
/// changeUserPassword) through `docker exec`, authenticated with the current
/// credentials, so the stored password always matches the real one. Each
/// password is persisted only after the engine accepted it. Refused when the
/// database isn't running, for renames, and for engines that can't do it.
pub async fn update_database_credentials(
    _rate: RateLimit,
    auth: Authz<scope::DbCredentials>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDatabaseCredentials>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_db(&state.db, &id).await?;

    refuse_rename(
        "database name",
        db.db_name.as_deref(),
        req.db_name.as_deref(),
    )?;
    refuse_rename("username", db.db_user.as_deref(), req.db_user.as_deref())?;

    let mut changes: Vec<(CredentialField, String)> = Vec::new();
    if let Some(next) = next_credential(None, req.db_password)
        && Some(&next) != db.db_password.as_ref()
    {
        changes.push((CredentialField::AppPassword, next));
    }
    if let Some(next) = next_credential(None, req.root_password)
        && Some(&next) != db.root_password.as_ref()
    {
        changes.push((CredentialField::RootPassword, next));
    }
    if changes.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "Nothing to change: enter a new password.".to_string(),
        )));
    }

    // Validate every change before touching the database.
    for (field, password) in &changes {
        password_change_step(&db, *field, password)
            .map_err(|reason| AppError(TondError::InvalidInput(reason)))?;
    }

    let container_id = match db.container_id.as_deref() {
        Some(cid) if db.status == "running" && container_running(&state.docker, cid).await => {
            cid.to_string()
        }
        _ => {
            return Err(AppError(TondError::InvalidInput(format!(
                "Start '{}' first: the password is changed inside the running database. \
                 Nothing was changed.",
                db.name
            ))));
        }
    };

    let mut applied: Vec<&'static str> = Vec::new();
    for (field, password) in &changes {
        // Re-read so each step authenticates with the credentials now in effect.
        let current = fetch_db(&state.db, &db.id).await?;
        let step = password_change_step(&current, *field, password)
            .map_err(|reason| AppError(TondError::InvalidInput(reason)))?;
        let secrets = [
            password.as_str(),
            current.db_password.as_deref().unwrap_or_default(),
            current.root_password.as_deref().unwrap_or_default(),
        ];

        if let Err(e) = run_step(&state.docker, &container_id, &step, &secrets).await {
            let done = if applied.is_empty() {
                "Nothing was changed.".to_string()
            } else {
                "The application password was changed and saved; the root password was not."
                    .to_string()
            };
            return Err(AppError(TondError::Internal(format!(
                "Couldn't change the {} inside '{}': {e}. {done}",
                field.label(),
                db.name
            ))));
        }

        match field {
            CredentialField::AppPassword => {
                // A MySQL/MariaDB row without a root password mirrors the app
                // password; keep root's real (old) password explicit.
                let root = if matches!(current.db_type.as_str(), "mysql" | "mariadb")
                    && current.root_password.is_none()
                {
                    current.db_password.clone()
                } else {
                    current.root_password.clone()
                };
                sqlx::query(
                    "UPDATE databases SET db_password = ?, root_password = ?, updated_at = datetime('now') WHERE id = ?",
                )
                .bind(secrets::seal_db_password(Some(password))?)
                .bind(secrets::seal_db_root_password(root.as_deref())?)
                .bind(&db.id)
                .execute(&state.db)
                .await?;
                applied.push("db_password");
            }
            CredentialField::RootPassword => {
                sqlx::query(
                    "UPDATE databases SET root_password = ?, updated_at = datetime('now') WHERE id = ?",
                )
                .bind(secrets::seal_db_root_password(Some(password))?)
                .bind(&db.id)
                .execute(&state.db)
                .await?;
                applied.push("root_password");
            }
        }
    }

    write_audit_log(
        &state.db,
        None,
        None,
        "db_credentials_update",
        &auth.audit_actor(),
        Some(&format!("name={} changed={}", db.name, applied.join(","))),
    )
    .await;

    let updated_db = fetch_db(&state.db, &db.id).await?;
    let detail = build_detail(updated_db, public_host(&state).await.as_deref());
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "database": detail,
            "changed": applied,
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
        auth::{Authz, Principal, RateLimiter, rate_limit::RateLimit},
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
        let sealed = secrets::seal_database(db).unwrap_or_else(|err| panic!("seal: {err}"));
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
        .bind(&sealed.db_password)
        .bind(&sealed.root_password)
        .bind(&db.volume_path)
        .bind(db.memory_limit)
        .bind(&sealed.custom_external_url)
        .bind(&sealed.custom_internal_url)
        .bind(&db.image_used)
        .bind(&db.custom_image)
        .bind(&db.created_at)
        .bind(&db.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed managed database row: {err}"));
    }

    /// A database without a project (so seeding needs no projects row).
    fn standalone_db() -> ManagedDatabase {
        ManagedDatabase {
            project_id: None,
            ..sample_db()
        }
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
            public_access: false,
            backup_schedule: None,
            backup_retention: 7,
            version: None,
            variant: String::new(),
            allowlist: "[]".to_string(),
            tls: false,
            upgrade: None,
        }
    }

    #[test]
    fn build_detail_prefers_custom_urls_and_computes_engine_urls_when_missing() {
        let strings = connection_strings(&sample_db(), None);
        assert_eq!(
            strings.external.as_deref(),
            Some("postgres://appuser:secret@localhost:5433/appdb"),
            "the plaintext string (reveal) carries the password"
        );
        let detail = build_detail(sample_db(), None);
        assert_eq!(
            detail.connection_string.as_deref(),
            Some("postgres://appuser:••••••••@localhost:5433/appdb"),
            "build_detail should compute an external connection string from the engine config when no override is set"
        );
        assert_eq!(
            detail.internal_connection_string.as_deref(),
            Some("postgres://appuser:••••••••@postgres-main:5432/appdb"),
            "build_detail should compute an internal project-network connection string when the database belongs to a project"
        );

        let mut db = sample_db();
        db.custom_external_url = Some("postgres://external.example.com/db".to_string());
        db.custom_internal_url = Some("postgres://internal.example.com/db".to_string());
        let detail = build_detail(db, None);
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
    fn redis_config_disables_the_default_user_and_requires_the_acl_user() {
        let config = redis_config("tond_cache", "s3cretHex")
            .unwrap_or_else(|e| panic!("valid credentials rejected: {e}"));
        assert!(config.contains("user default off\n"));
        assert!(config.contains("user tond_cache on >s3cretHex ~* &* +@all\n"));
        assert!(config.contains("dir /data\n"));
        for bad in [
            "with space",
            "new\nline",
            "quo\"te",
            "back\\slash",
            "#comment",
            "",
        ] {
            assert!(
                redis_config("tond_cache", bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(redis_config("bad user", "pw").is_err());
    }

    #[test]
    fn redis_config_lives_next_to_the_volume_not_inside_it() {
        assert_eq!(
            redis_config_path("/opt/tond/data/databases/cache", "cache"),
            "/opt/tond/data/databases/.cache.redis.conf"
        );
    }

    #[tokio::test]
    async fn engine_runtime_mounts_the_config_read_only_for_redis_like_engines() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let volume = tmp.path().join("cache");
        let mut db = sample_db();
        db.name = "cache".to_string();
        db.db_type = "keydb".to_string();
        db.db_user = Some("tond_cache".to_string());
        db.db_password = Some("pw123".to_string());
        let (binds, cmd) = engine_runtime(&db, &volume.to_string_lossy())
            .await
            .unwrap_or_else(|_| panic!("engine_runtime failed"));
        assert_eq!(
            cmd,
            Some(vec![
                "keydb-server".to_string(),
                REDIS_CONFIG_MOUNT.to_string()
            ])
        );
        assert!(
            binds[0].ends_with(":/etc/tond/redis.conf:ro"),
            "{}",
            binds[0]
        );
        let written = std::fs::read_to_string(tmp.path().join(".cache.redis.conf"))
            .unwrap_or_else(|e| panic!("config not written: {e}"));
        assert!(written.contains("user tond_cache on >pw123"));

        db.db_type = "postgres".to_string();
        let (binds, cmd) = engine_runtime(&db, &volume.to_string_lossy())
            .await
            .unwrap_or_else(|_| panic!("engine_runtime failed"));
        assert!(binds.is_empty() && cmd.is_none());
    }

    #[test]
    fn public_connection_string_only_exists_when_public_access_is_on() {
        let detail = build_detail(sample_db(), Some("tond.example.com"));
        assert_eq!(detail.public_connection_string, None);
        assert_eq!(exposure_for(&sample_db()), docker::Exposure::Loopback);

        let mut db = sample_db();
        db.public_access = true;
        assert_eq!(exposure_for(&db), docker::Exposure::Public);
        let detail = build_detail(db.clone(), Some("tond.example.com"));
        assert_eq!(
            detail.public_connection_string.as_deref(),
            Some("postgres://appuser:••••••••@tond.example.com:5433/appdb")
        );
        let detail = build_detail(db, None);
        assert_eq!(
            detail.public_connection_string.as_deref(),
            Some("postgres://appuser:••••••••@<server-ip>:5433/appdb"),
            "without a dashboard domain the host is an obvious placeholder"
        );
    }

    #[test]
    fn tls_databases_ask_clients_to_require_tls() {
        let mut db = sample_db();
        db.public_access = true;
        db.tls = true;
        let detail = build_detail(db.clone(), Some("tond.example.com"));
        assert_eq!(
            detail.public_connection_string.as_deref(),
            Some("postgres://appuser:••••••••@tond.example.com:5433/appdb?sslmode=require")
        );
        assert!(detail.supports_tls);
        assert!(
            detail
                .access_warning
                .is_some_and(|w| w.contains("every address")),
            "public without an allowlist warns"
        );
        assert_eq!(
            with_tls_param("mysql", true, "mysql://u:p@h:1/d?x=1".to_string()),
            "mysql://u:p@h:1/d?x=1&ssl-mode=REQUIRED"
        );
        assert_eq!(
            with_tls_param("redis", true, "redis://h".to_string()),
            "redis://h"
        );
        assert_eq!(
            with_tls_param("postgres", false, "p://h".to_string()),
            "p://h"
        );
    }

    #[test]
    fn upgrade_targets_follow_the_catalog_and_stop_for_custom_images() {
        let mut db = sample_db();
        db.version = Some("16".to_string());
        assert_eq!(
            build_detail(db.clone(), None).upgrade_targets,
            vec!["17", "18"]
        );
        db.custom_image = Some("me/pg".to_string());
        assert!(build_detail(db, None).upgrade_targets.is_empty());
    }

    #[tokio::test]
    async fn creating_refuses_unknown_versions_variants_and_schedules_before_starting_anything() {
        let state = test_state().await;
        for (version, variant, schedule, needle) in [
            (Some("9"), None, None, "Version 9"),
            (None, Some("timescale"), None, "Unknown variant"),
            (None, None, Some("every day"), "Invalid backup schedule"),
        ] {
            let err = create_database(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Json(CreateDatabaseRequest {
                    name: "shop".to_string(),
                    db_type: "postgres".to_string(),
                    version: version.map(str::to_string),
                    variant: variant.map(str::to_string),
                    backup_schedule: schedule.map(str::to_string),
                    ..Default::default()
                }),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("{needle} must be refused"));
            assert_eq!(err.0.status_code(), 400);
            assert!(err.0.to_string().contains(needle), "{}", err.0);
        }
        for bad in ["-x", "a b", "", "ü"] {
            assert!(validate_database_name(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn allowlists_are_validated_and_refused_where_the_firewall_cannot_enforce_them() {
        let state = test_state().await;
        let mut db = standalone_db();
        db.public_access = true;
        seed_database_row(&state, &db).await;
        sqlx::query("UPDATE databases SET public_access = 1 WHERE id = ?")
            .bind(&db.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let patch = |allow: &[&str]| UpdateDatabaseRequest {
            allowlist: Some(allow.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };

        let err = update_database(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(db.id.clone()),
            Json(patch(&["not-an-ip"])),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("invalid entries must be refused"));
        assert_eq!(err.0.status_code(), 400);

        let err = update_database(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Deployer)),
            State(state.clone()),
            Path(db.id.clone()),
            Json(patch(&["203.0.113.7"])),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("members must be refused"));
        assert_eq!(err.0.status_code(), 403);

        crate::db_access::set_test_firewall(true);
        let response = update_database(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(db.id.clone()),
            Json(patch(&["203.0.113.7/32", "198.51.100.9/24"])),
        )
        .await
        .unwrap_or_else(|_| panic!("a valid allowlist is stored"))
        .into_response();
        crate::db_access::set_test_firewall(false);
        let json = response_json(response).await;
        assert_eq!(
            json["data"]["allowlist"],
            serde_json::json!(["203.0.113.7", "198.51.100.0/24"])
        );
    }

    #[tokio::test]
    async fn tls_is_refused_for_engines_that_cannot_serve_it() {
        let state = test_state().await;
        let mut db = standalone_db();
        db.db_type = "redis".to_string();
        seed_database_row(&state, &db).await;
        let err = update_database(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(db.id.clone()),
            Json(UpdateDatabaseRequest {
                tls: Some(true),
                ..Default::default()
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("redis TLS must be refused"));
        assert_eq!(err.0.status_code(), 400);
        assert!(err.0.to_string().contains("SSH tunnel"));
    }

    #[tokio::test]
    async fn members_cannot_enable_public_access_or_change_the_image() {
        let state = test_state().await;
        let mut db = sample_db();
        db.project_id = None;
        seed_database_row(&state, &db).await;

        for req in [
            UpdateDatabaseRequest {
                custom_external_url: None,
                custom_internal_url: None,
                custom_image: None,
                memory_limit: None,
                public_access: Some(true),
                ..Default::default()
            },
            UpdateDatabaseRequest {
                custom_external_url: None,
                custom_internal_url: None,
                custom_image: Some("evil/image".to_string()),
                memory_limit: None,
                public_access: None,
                ..Default::default()
            },
        ] {
            let err = update_database(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Deployer)),
                State(state.clone()),
                Path(db.id.clone()),
                Json(req),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("members must be refused"));
            assert_eq!(err.0.status_code(), 403);
        }

        let err = create_database(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Deployer)),
            State(state.clone()),
            Json(CreateDatabaseRequest {
                name: "public-db".to_string(),
                db_type: "postgres".to_string(),
                project_id: None,
                memory_limit: None,
                public_access: true,
                ..Default::default()
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("members must not create public databases"));
        assert_eq!(err.0.status_code(), 403);
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
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(db.id.clone()),
            Json(UpdateDatabaseRequest {
                custom_external_url: Some("postgres://db.example.com/appdb".to_string()),
                custom_internal_url: Some("   ".to_string()),
                custom_image: Some("postgres:17-alpine".to_string()),
                memory_limit: Some(0),
                public_access: None,
                ..Default::default()
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

    async fn call_update_credentials(
        state: &AppState,
        id: &str,
        req: UpdateDatabaseCredentials,
    ) -> Result<axum::response::Response, String> {
        update_database_credentials(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(id.to_string()),
            Json(req),
        )
        .await
        .map(IntoResponse::into_response)
        .map_err(|e| e.0.to_string())
    }

    fn password_only(password: &str) -> UpdateDatabaseCredentials {
        UpdateDatabaseCredentials {
            db_name: None,
            db_user: None,
            db_password: Some(password.to_string()),
            root_password: None,
        }
    }

    async fn stored_password(state: &AppState, id: &str) -> Option<String> {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT db_password FROM databases WHERE id = ?")
                .bind(id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read stored password: {err}"));
        crate::secret_key::decrypt_opt(
            crate::secret_key::Purpose::DatabasePassword,
            stored.as_deref(),
        )
        .unwrap_or_else(|err| panic!("stored password does not decrypt: {err}"))
    }

    #[tokio::test]
    async fn database_secrets_are_encrypted_at_rest_and_revealed_only_on_request() {
        let state = test_state().await;
        let mut db = standalone_db();
        db.custom_external_url = Some("postgres://appuser:secret@db.example.com/appdb".to_string());
        seed_database_row(&state, &db).await;

        let raw: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT db_password, custom_external_url FROM databases WHERE id = ?")
                .bind(&db.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("read raw row: {err}"));
        assert!(raw.0.as_deref().is_some_and(|v| v.starts_with("v1.")));
        assert!(raw.1.as_deref().is_some_and(|v| v.starts_with("v1.")));

        let shown = get_database(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Viewer)),
            State(state.clone()),
            Path(db.id.clone()),
        )
        .await
        .unwrap_or_else(|_| panic!("get_database should succeed"))
        .into_response();
        let shown = response_json(shown).await.to_string();
        assert!(!shown.contains("secret"), "detail must be masked: {shown}");

        let revealed = reveal_database_credentials(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(db.id.clone()),
        )
        .await
        .unwrap_or_else(|_| panic!("reveal should succeed"))
        .into_response();
        let revealed = response_json(revealed).await;
        assert_eq!(revealed["data"]["db_password"], "secret");
        assert_eq!(
            revealed["data"]["connection_string"],
            "postgres://appuser:secret@db.example.com/appdb"
        );
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'db_credentials_reveal'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("count audits: {err}"));
        assert_eq!(audits, 1, "every reveal is audit-logged");
    }

    #[tokio::test]
    async fn update_database_credentials_refuses_renames() {
        let state = test_state().await;
        let db = standalone_db();
        seed_database_row(&state, &db).await;

        let err = call_update_credentials(
            &state,
            &db.id,
            UpdateDatabaseCredentials {
                db_name: Some("other_db".to_string()),
                db_user: None,
                db_password: None,
                root_password: None,
            },
        )
        .await
        .expect_err("renaming the database should be refused");
        assert!(
            err.contains("only passwords can"),
            "unexpected message: {err}"
        );

        // Sending the current values back (as the dashboard may) is not a rename.
        let err = call_update_credentials(
            &state,
            &db.id,
            UpdateDatabaseCredentials {
                db_name: Some("appdb".to_string()),
                db_user: Some(" appuser ".to_string()),
                db_password: Some("   ".to_string()),
                root_password: None,
            },
        )
        .await
        .expect_err("a request with no new password has nothing to change");
        assert!(
            err.contains("Nothing to change"),
            "unexpected message: {err}"
        );
    }

    #[tokio::test]
    async fn update_database_credentials_refuses_when_the_database_is_not_running() {
        let state = test_state().await;
        let mut db = standalone_db();
        db.status = "stopped".to_string();
        seed_database_row(&state, &db).await;

        let err = call_update_credentials(&state, &db.id, password_only("new-secret"))
            .await
            .expect_err("a stopped database can't change its password");

        assert!(
            err.contains("Start 'postgres-main' first"),
            "unexpected message: {err}"
        );
        assert_eq!(
            stored_password(&state, &db.id).await.as_deref(),
            Some("secret"),
            "the stored password must stay in sync with the real one"
        );
    }

    #[tokio::test]
    async fn update_database_credentials_refuses_when_the_container_is_unreachable() {
        // The test Docker client points at a closed port, so the container
        // can't be confirmed running even though the row says it is.
        let state = test_state().await;
        let db = standalone_db();
        seed_database_row(&state, &db).await;

        let err = call_update_credentials(&state, &db.id, password_only("new-secret"))
            .await
            .expect_err("an unreachable container can't change its password");

        assert!(
            err.contains("Nothing was changed"),
            "unexpected message: {err}"
        );
        assert_eq!(
            stored_password(&state, &db.id).await.as_deref(),
            Some("secret")
        );
    }

    #[tokio::test]
    async fn update_database_credentials_refuses_engines_that_cannot_change_in_place() {
        let state = test_state().await;
        let mut db = standalone_db();
        db.db_type = "redis".to_string();
        db.db_name = None;
        seed_database_row(&state, &db).await;

        let err = call_update_credentials(&state, &db.id, password_only("new-secret"))
            .await
            .expect_err("redis passwords can't be changed in place");

        assert!(err.contains("Redis"), "unexpected message: {err}");
        assert_eq!(
            stored_password(&state, &db.id).await.as_deref(),
            Some("secret")
        );
    }
}
