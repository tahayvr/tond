// backup.rs — Snapshot engine
//
// Public API consumed by handlers and the pre-deploy hook:
//
//   create_snapshot(state, trigger, progress_tx) -> anyhow::Result<String>
//     Runs the local part of a snapshot and returns its ID. A failure marks
//     the row `failed` with the error. When a storage provider is configured
//     (full and database snapshots), the encrypted upload runs in the
//     background and reports through `remote_status`.
//
//   restore_full(state, snapshot_id) -> anyhow::Result<String>
//     Creates a pre_restore safety-net snapshot then launches a background task
//     that replaces all data and calls std::process::exit(0).
//     Returns the pre_restore snapshot ID.
//
//   restore_app(state, snapshot_id, app_name) -> anyhow::Result<()>
//     App-scope rollback: stop container → swap volume dir → start container.
//
//   restore_database(state, snapshot_id, db_name) -> anyhow::Result<String>
//     Database-scope restore: safety-net backup of the current data, then
//     stop → swap volume → start → import dump. Returns the safety net's ID.
//
//   backup_scheduler(state) -> !
//     Long-running background task. Re-reads config each iteration.
//
// Local snapshots are directories under <data>/backups/snapshots/<id>
// whose files are each encrypted at rest (snapshot_files.rs) and hard-linked
// against the previous snapshot when unchanged. Only regular files and
// directories are copied: symlinks in app volumes are skipped, so a container
// can't make a snapshot read host files. The SQLite database is captured
// with `VACUUM INTO` (a consistent copy while tond-server keeps running) and
// encrypted at once; database dumps likewise. Compose services that declare
// a `tond.backup.dump` hook (compose_backup.rs) dump into the snapshot before
// their volumes are copied; app restores feed the dump to their
// `tond.backup.restore` hook. Full snapshots also hold the TLS certificates.
//
// Scheduled backups remember their last slot (backup_schedule_state): one
// that came due while the server was down runs once at the next start, as a
// catch-up. A full restore leaves a marker so the next start puts the
// snapshot list right and starts or redeploys everything (restore_resume.rs).
//
// Remote copies are streamed end to end with constant memory: the snapshot
// directory is tarred, gzipped and encrypted (`snapshot_archive`, key derived
// from TOND_SECRET_KEY) on a blocking thread and handed to an S3 multipart
// upload one `storage::PART_SIZE` part at a time, with a manifest next to it
// so another server can list and import it (snapshot_remote.rs). Restoring
// downloads the archive to a file, checks its SHA-256 against the one
// recorded at upload, authenticates every record, and only then extracts it
// (re-encrypted for local storage) — so a server restoring from remote
// storage needs the same TOND_SECRET_KEY.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::handlers::storage_providers::decrypt_provider_secret;
use crate::state::AppState;
use crate::{
    compose_backup, db, db_exec, docker, secret_key, secrets, snapshot_archive, snapshot_files,
    snapshot_remote, storage,
};
use tond_common::{BackupConfig, ManagedDatabase, Snapshot};

// ── Progress event ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProgressEvent {
    pub step: String,
    pub message: String,
    pub done: bool,
}

pub type ProgressTx = mpsc::Sender<ProgressEvent>;

// ── Trigger enum ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum SnapshotTrigger {
    Manual {
        name: Option<String>,
    },
    /// "Back up now" for one managed database
    ManualDatabase {
        db_name: String,
    },
    Scheduled,
    PreDeploy {
        app_name: String,
        deployment_id: String,
    },
    PreRestore {
        preceding_snapshot_id: String,
    },
    /// Safety net taken before a database restore overwrites the data
    PreRestoreDatabase {
        db_name: String,
    },
    /// Taken before Tond updates (or rolls back) itself (updates/run.rs)
    PreUpdate {
        to_version: String,
    },
    /// One database's own schedule (`databases.backup_schedule`)
    ScheduledDatabase {
        db_name: String,
    },
    /// Taken before an app moves to a newer template version
    PreTemplateUpgrade {
        app_name: String,
        to_version: String,
    },
    /// Taken before a major-version upgrade touches the database
    PreUpgradeDatabase {
        db_name: String,
    },
}

impl SnapshotTrigger {
    fn trigger_str(&self) -> &'static str {
        match self {
            Self::Manual { .. } | Self::ManualDatabase { .. } => "manual",
            Self::Scheduled | Self::ScheduledDatabase { .. } => "scheduled",
            Self::PreDeploy { .. } => "pre_deploy",
            Self::PreRestore { .. } | Self::PreRestoreDatabase { .. } => "pre_restore",
            Self::PreUpdate { .. } => "pre_update",
            Self::PreUpgradeDatabase { .. } | Self::PreTemplateUpgrade { .. } => "pre_upgrade",
        }
    }

    fn scope(&self) -> String {
        match self {
            Self::Manual { .. }
            | Self::Scheduled
            | Self::PreRestore { .. }
            | Self::PreUpdate { .. } => "full".to_string(),
            Self::PreDeploy { app_name, .. } | Self::PreTemplateUpgrade { app_name, .. } => {
                format!("app:{}", app_name)
            }
            Self::ManualDatabase { db_name }
            | Self::PreRestoreDatabase { db_name }
            | Self::ScheduledDatabase { db_name }
            | Self::PreUpgradeDatabase { db_name } => format!("database:{db_name}"),
        }
    }

    fn trigger_app(&self) -> Option<&str> {
        match self {
            Self::PreDeploy { app_name, .. } | Self::PreTemplateUpgrade { app_name, .. } => {
                Some(app_name.as_str())
            }
            _ => None,
        }
    }

    fn deployment_id(&self) -> Option<&str> {
        match self {
            Self::PreDeploy { deployment_id, .. } => Some(deployment_id.as_str()),
            _ => None,
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Manual { name: Some(n) } => n.clone(),
            Self::Manual { name: None } => {
                format!("Manual {}", Utc::now().format("%Y-%m-%d %H:%M"))
            }
            Self::ManualDatabase { db_name } => {
                format!("{db_name} {}", Utc::now().format("%Y-%m-%d %H:%M"))
            }
            Self::Scheduled => format!("Scheduled {}", Utc::now().format("%Y-%m-%d %H:%M")),
            Self::PreDeploy { app_name, .. } => {
                format!(
                    "pre-deploy:{} {}",
                    app_name,
                    Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
                )
            }
            Self::PreRestore { .. } => {
                format!("pre-restore {}", Utc::now().format("%Y-%m-%d %H:%M"))
            }
            Self::PreRestoreDatabase { db_name } => {
                format!(
                    "pre-restore {db_name} {}",
                    Utc::now().format("%Y-%m-%d %H:%M")
                )
            }
            Self::PreUpdate { to_version } => {
                format!(
                    "pre-update to {to_version} {}",
                    Utc::now().format("%Y-%m-%d %H:%M")
                )
            }
            Self::ScheduledDatabase { db_name } => {
                format!(
                    "{db_name} scheduled {}",
                    Utc::now().format("%Y-%m-%d %H:%M")
                )
            }
            Self::PreTemplateUpgrade {
                app_name,
                to_version,
            } => {
                format!(
                    "pre-upgrade {app_name} to {to_version} {}",
                    Utc::now().format("%Y-%m-%d %H:%M")
                )
            }
            Self::PreUpgradeDatabase { db_name } => {
                format!(
                    "pre-upgrade {db_name} {}",
                    Utc::now().format("%Y-%m-%d %H:%M")
                )
            }
        }
    }
}

// ── Data-dir helpers ──────────────────────────────────────────────────────────

fn data_dir() -> String {
    std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string())
}

fn db_path() -> String {
    std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string())
}

pub(crate) fn snapshots_dir() -> PathBuf {
    PathBuf::from(data_dir()).join("backups").join("snapshots")
}

pub(crate) fn snapshot_dir(id: &str) -> PathBuf {
    snapshots_dir().join(id)
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Create a snapshot. Returns the snapshot ID once the local copy is complete.
/// `progress_tx` receives SSE-ready progress events while the snapshot runs.
pub async fn create_snapshot(
    state: &AppState,
    trigger: SnapshotTrigger,
    progress_tx: Option<ProgressTx>,
) -> Result<String> {
    create_snapshot_noted(state, trigger, progress_tx, None).await
}

/// `create_snapshot` with a note on the snapshot (e.g. a catch-up).
pub async fn create_snapshot_noted(
    state: &AppState,
    trigger: SnapshotTrigger,
    progress_tx: Option<ProgressTx>,
    notes: Option<String>,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let scope = trigger.scope();

    // Find previous completed local snapshot for hard-link base
    let parent_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM snapshots WHERE status = 'completed' AND local_path IS NOT NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;

    sqlx::query(
        "INSERT INTO snapshots (id, name, status, trigger, scope, trigger_app, deployment_id, parent_snapshot_id, notes)
         VALUES (?, ?, 'running', ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(trigger.name())
    .bind(trigger.trigger_str())
    .bind(&scope)
    .bind(trigger.trigger_app())
    .bind(trigger.deployment_id())
    .bind(&parent_id)
    .bind(&notes)
    .execute(&state.db)
    .await?;

    emit(&progress_tx, "start", "Snapshot started…", false).await;

    match build_snapshot(state, &id, &trigger, parent_id.as_deref(), &progress_tx).await {
        Ok(()) => Ok(id),
        Err(e) => {
            tracing::error!("Snapshot {id} failed: {e:#}");
            let _ = tokio::fs::remove_dir_all(snapshot_dir(&id)).await;
            let _ = sqlx::query(
                "UPDATE snapshots SET status = 'failed', error_message = ?, local_path = NULL,
                 completed_at = datetime('now') WHERE id = ?",
            )
            .bind(format!("{e:#}"))
            .bind(&id)
            .execute(&state.db)
            .await;
            emit(
                &progress_tx,
                "done",
                &format!("Snapshot failed: {e:#}"),
                true,
            )
            .await;
            Err(e)
        }
    }
}

async fn build_snapshot(
    state: &AppState,
    id: &str,
    trigger: &SnapshotTrigger,
    parent_id: Option<&str>,
    progress_tx: &Option<ProgressTx>,
) -> Result<()> {
    let trigger_str = trigger.trigger_str();
    let scope = trigger.scope();
    let trigger_app = trigger.trigger_app().map(|s| s.to_string());
    let deployment_id = trigger.deployment_id().map(|s| s.to_string());

    let staging = snapshot_dir(id);
    tokio::fs::create_dir_all(&staging).await?;

    let mut delta_bytes: i64 = 0;
    let mut total_bytes: i64 = 0;

    // Determine previous snapshot directory for hard-link dedup
    let prev_dir: Option<PathBuf> = if let Some(pid) = parent_id {
        let prev_path: Option<String> =
            sqlx::query_scalar("SELECT local_path FROM snapshots WHERE id = ?")
                .bind(pid)
                .fetch_optional(&state.db)
                .await?
                .flatten();
        prev_path.map(PathBuf::from)
    } else {
        None
    };

    // ── Determine what to back up based on scope ──────────────────────────────

    let is_full = scope == "full";
    let app_scope: Option<String> = scope.strip_prefix("app:").map(str::to_string);
    let db_scope: Option<String> = scope.strip_prefix("database:").map(str::to_string);

    // ── Step: SQLite (full scope only) ────────────────────────────────────────
    let mut apps_included: Vec<String> = Vec::new();
    let mut databases_included: Vec<String> = Vec::new();

    if is_full {
        emit(
            progress_tx,
            "sqlite",
            "Snapshotting SQLite database…",
            false,
        )
        .await;
        let dest = staging.join("tond.db");
        // VACUUM INTO writes a consistent copy without blocking readers or
        // writers (tond-proxy keeps reading the live file).
        sqlx::query("VACUUM INTO ?")
            .bind(dest.to_string_lossy().into_owned())
            .execute(&state.db)
            .await
            .context("failed to copy the SQLite database")?;
        let size = tokio::fs::metadata(&dest)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        seal(dest)
            .await
            .context("failed to encrypt the SQLite copy")?;
        delta_bytes += size as i64;
        total_bytes += size as i64;
        tracing::debug!("SQLite snapshot: {} bytes", size);
    }

    // ── Step: App volumes ─────────────────────────────────────────────────────
    let vol_base = PathBuf::from(data_dir()).join("volumes");

    let vol_apps: Vec<String> = if let Some(ref app_name) = app_scope {
        vec![app_name.clone()]
    } else if is_full {
        list_subdirs(&vol_base).await
    } else {
        Vec::new()
    };

    for (i, app_name) in vol_apps.iter().enumerate() {
        let src = vol_base.join(app_name);
        if !src.exists() {
            continue;
        }
        emit(
            progress_tx,
            "volumes",
            &format!("Copying app volumes ({}/{})…", i + 1, vol_apps.len()),
            false,
        )
        .await;

        let dest_vol = staging.join("volumes").join(app_name);
        let prev_vol = prev_dir.as_ref().map(|p| p.join("volumes").join(app_name));
        let (d, t) = copy_tree(src, dest_vol, prev_vol).await?;
        delta_bytes += d;
        total_bytes += t;
        apps_included.push(app_name.clone());
    }

    // ── Step: Compose named volumes ──────────────────────────────────────────
    // Bind mounts of compose apps live under volumes/<app> (copied above);
    // named volumes are Docker volumes <app>_<volume>. They are copied from
    // their mount point on this host, like the volume directories (a live,
    // crash-consistent copy; symlinks are skipped).
    let compose_apps: Vec<String> = if app_scope.is_some() || is_full {
        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM apps WHERE source_type = 'compose'")
                .fetch_all(&state.db)
                .await?;
        names
            .into_iter()
            .filter(|n| app_scope.as_ref().is_none_or(|a| a == n))
            .collect()
    } else {
        Vec::new()
    };
    let mut compose_volumes_included: Vec<String> = Vec::new();
    let mut compose_dumps: Vec<serde_json::Value> = Vec::new();
    for app_name in &compose_apps {
        // Services that declare a dump hook (`tond.backup.dump`) dump first,
        // so the snapshot holds a consistent copy next to the live files.
        compose_dumps.extend(dump_compose_services(state, &staging, app_name, progress_tx).await);
        let project = crate::deploy::compose_project_name(app_name);
        let volumes = match docker::compose_volumes(&state.docker, &project).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Could not list the volumes of {app_name}: {e:#}");
                continue;
            }
        };
        for (volume, mountpoint) in volumes {
            let src = PathBuf::from(&mountpoint);
            if !src.is_absolute() || !src.is_dir() {
                continue;
            }
            emit(
                progress_tx,
                "volumes",
                &format!("Copying volume {volume}…"),
                false,
            )
            .await;
            let dest = compose_volume_dir(&staging, app_name, &volume);
            let prev = prev_dir
                .as_ref()
                .map(|p| compose_volume_dir(p, app_name, &volume));
            let (d, t) = copy_tree(src, dest, prev).await?;
            delta_bytes += d;
            total_bytes += t;
            compose_volumes_included.push(volume);
        }
        if !apps_included.contains(app_name)
            && (compose_volume_root(&staging, app_name).exists()
                || compose_dump_root(&staging, app_name).exists())
        {
            apps_included.push(app_name.clone());
        }
    }

    // ── Step: Managed DB dumps (full scope or db scope) ───────────────────────
    if is_full || db_scope.is_some() {
        let db_vol_base = PathBuf::from(data_dir()).join("databases");
        let db_names: Vec<String> = if let Some(ref db_name) = db_scope {
            vec![db_name.clone()]
        } else {
            list_subdirs(&db_vol_base).await
        };

        for db_name in &db_names {
            emit(
                progress_tx,
                "database",
                &format!("Dumping database: {}", db_name),
                false,
            )
            .await;

            let dest_db = staging.join("databases").join(db_name);
            tokio::fs::create_dir_all(&dest_db).await?;

            // Logical dump through the engine (credentials decrypted here only)
            let db_row = sqlx::query_as::<_, ManagedDatabase>(
                "SELECT * FROM databases WHERE name = ? LIMIT 1",
            )
            .bind(db_name)
            .fetch_optional(&state.db)
            .await?;

            match db_row {
                Some(db) => {
                    let db = secrets::open_database(db)?;
                    // What restoring into a new database needs to know, even
                    // after this database is gone.
                    write_sealed(
                        dest_db.join(DB_META_FILE),
                        serde_json::to_vec_pretty(&DatabaseMeta::of(&db))?,
                    )
                    .await?;
                    let dumped = async {
                        dump_database(&state.docker, &db, &dest_db).await?;
                        // The engine wrote a plain dump: encrypt it at once.
                        for file in DUMP_FILES {
                            let path = dest_db.join(file);
                            if path.exists() {
                                seal(path).await?;
                            }
                        }
                        anyhow::Ok(())
                    }
                    .await;
                    if let Err(e) = dumped {
                        // A copy of a running engine's files isn't consistent:
                        // a backup of just this database needs the dump.
                        if db_scope.is_some() && db.status == "running" {
                            return Err(e.context(format!("couldn't dump {db_name}")));
                        }
                        tracing::warn!("Logical dump of {db_name} failed: {e:#}");
                    }
                }
                None if db_scope.is_some() => {
                    anyhow::bail!("database '{db_name}' not found");
                }
                None => {}
            }

            // Raw volume copy (always, as fallback)
            let src_db_vol = db_vol_base.join(db_name);
            if src_db_vol.exists() {
                let dest_raw = dest_db.join("volume");
                let prev_db_vol = prev_dir
                    .as_ref()
                    .map(|p| p.join("databases").join(db_name).join("volume"));
                let (d, t) = copy_tree(src_db_vol, dest_raw, prev_db_vol).await?;
                delta_bytes += d;
                total_bytes += t;
            }

            databases_included.push(db_name.clone());
        }
    }

    // ── Step: SSH keys (full scope only) ─────────────────────────────────────
    if is_full {
        let keys_src = PathBuf::from(data_dir()).join("keys");
        if keys_src.exists() {
            let prev_keys = prev_dir.as_ref().map(|p| p.join("keys"));
            let (d, t) = copy_tree(keys_src, staging.join("keys"), prev_keys).await?;
            delta_bytes += d;
            total_bytes += t;
        }
    }

    // ── Step: TLS certificates (full scope only) ─────────────────────────────
    // So a server move keeps serving HTTPS at once; a restore puts back only
    // the ones still valid (see `restore_certificates`).
    let mut certificates: Vec<serde_json::Value> = Vec::new();
    if is_full {
        let certs_src = PathBuf::from(data_dir()).join("certs");
        if certs_src.is_dir() {
            let prev_certs = prev_dir.as_ref().map(|p| p.join("certs"));
            let (d, t) = copy_tree(certs_src, staging.join("certs"), prev_certs).await?;
            delta_bytes += d;
            total_bytes += t;
        }
        let account = PathBuf::from(data_dir()).join("acme_account.json");
        if account.is_file() {
            let dest = staging.join("acme_account.json");
            tokio::task::spawn_blocking(move || snapshot_files::encrypt_file(&account, &dest))
                .await
                .context("snapshot copy task failed")??;
        }
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT domain, cert_type, expires_at, cert_path FROM certificates
             WHERE status = 'active'",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        certificates = rows
            .into_iter()
            .filter_map(|(domain, cert_type, expires_at, cert_path)| {
                // The directory under certs/ that holds this certificate.
                let dir = Path::new(&cert_path)
                    .parent()?
                    .file_name()?
                    .to_string_lossy()
                    .into_owned();
                Some(json!({
                    "domain": domain, "cert_type": cert_type,
                    "expires_at": expires_at, "dir": dir,
                }))
            })
            .collect();
    }

    // ── Write manifest.json ───────────────────────────────────────────────────
    let name: String = sqlx::query_scalar("SELECT name FROM snapshots WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let manifest = json!({
        "tond_version": env!("CARGO_PKG_VERSION"),
        "id": id,
        "name": name,
        "created_at": Utc::now().to_rfc3339(),
        "trigger": trigger_str,
        "trigger_app": trigger_app,
        "deployment_id": deployment_id,
        "scope": scope,
        "delta_bytes": delta_bytes,
        "total_bytes": total_bytes,
        "apps_included": apps_included,
        "compose_volumes_included": compose_volumes_included,
        "compose_dumps": compose_dumps,
        "databases_included": databases_included,
        "certificates": certificates,
    });
    write_sealed(
        staging.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;

    // ── Persist completion ────────────────────────────────────────────────────
    let cfg = get_backup_config(state).await?;
    let upload_to = cfg
        .storage_provider_id
        .clone()
        .filter(|_| is_full || db_scope.is_some());
    let local_path = staging.to_string_lossy().to_string();
    sqlx::query(
        "UPDATE snapshots
         SET status = 'completed', local_path = ?, delta_bytes = ?, total_bytes = ?,
             remote_status = ?, completed_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&local_path)
    .bind(delta_bytes)
    .bind(total_bytes)
    .bind(upload_to.as_ref().map(|_| "uploading"))
    .bind(id)
    .execute(&state.db)
    .await?;

    if is_full {
        // Update `latest` symlink
        let latest = snapshots_dir().join("latest");
        let _ = std::fs::remove_file(&latest);
        let _ = std::os::unix::fs::symlink(&staging, &latest);
    }

    // ── Remote upload (background, streamed and encrypted) ───────────────────
    if let Some(sp_id) = upload_to {
        emit(
            progress_tx,
            "s3",
            "Uploading an encrypted copy to remote storage…",
            false,
        )
        .await;
        let state = state.clone();
        let id = id.to_string();
        let keep_local = cfg.keep_local != 0;
        tokio::spawn(async move {
            upload_snapshot(&state, &id, &staging, &sp_id, keep_local).await;
        });
    }

    // ── Retention enforcement ─────────────────────────────────────────────────
    enforce_retention(state, &cfg, trigger_app.as_deref(), db_scope.as_deref()).await;

    let delta_mb = delta_bytes as f64 / 1_048_576.0;
    emit(
        progress_tx,
        "done",
        &format!("Snapshot completed ({:.1} MB new)", delta_mb),
        true,
    )
    .await;

    Ok(())
}

/// `<snapshot>/compose-volumes/<app>`: an app's named volumes in a snapshot.
fn compose_volume_root(snapshot: &Path, app_name: &str) -> PathBuf {
    snapshot.join("compose-volumes").join(app_name)
}

/// `<snapshot>/compose-volumes/<app>/<docker volume name>`
fn compose_volume_dir(snapshot: &Path, app_name: &str, volume: &str) -> PathBuf {
    compose_volume_root(snapshot, app_name).join(volume)
}

/// Ids of every container of a compose app.
async fn compose_containers(state: &AppState, app_name: &str) -> Vec<String> {
    let project = crate::deploy::compose_project_name(app_name);
    crate::compose::discover(&state.docker, &project)
        .await
        .map(|v| v.into_iter().map(|s| s.container_id).collect())
        .unwrap_or_default()
}

/// Put a snapshot's named volumes of a compose app back: each Docker volume
/// (created with compose's labels if missing) is emptied and refilled.
async fn restore_compose_volumes(state: &AppState, snapshot: &Path, app_name: &str) -> Result<()> {
    let root = compose_volume_root(snapshot, app_name);
    if !root.is_dir() {
        return Ok(());
    }
    let project = crate::deploy::compose_project_name(app_name);
    for volume in list_subdirs(&root).await {
        // Only this app's volumes: compose names them <project>_<volume>.
        let Some(short) = volume.strip_prefix(&format!("{project}_")) else {
            tracing::warn!("Skipping volume {volume}: not a volume of {app_name}");
            continue;
        };
        let mountpoint = match state.docker.inspect_volume(&volume).await {
            Ok(v) => v.mountpoint,
            Err(_) => {
                let labels = std::collections::HashMap::from([
                    ("com.docker.compose.project".to_string(), project.clone()),
                    ("com.docker.compose.volume".to_string(), short.to_string()),
                ]);
                state
                    .docker
                    .create_volume(bollard::models::VolumeCreateRequest {
                        name: Some(volume.clone()),
                        driver: Some("local".to_string()),
                        labels: Some(labels),
                        ..Default::default()
                    })
                    .await
                    .with_context(|| format!("could not create volume {volume}"))?
                    .mountpoint
            }
        };
        let dest = PathBuf::from(&mountpoint);
        if !dest.is_absolute() {
            anyhow::bail!("volume {volume} has no local mount point");
        }
        tokio::fs::create_dir_all(&dest).await?;
        let mut entries = tokio::fs::read_dir(&dest).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                tokio::fs::remove_dir_all(&path).await?;
            } else {
                tokio::fs::remove_file(&path).await?;
            }
        }
        snapshot_files::decrypt_tree_async(root.join(&volume), dest).await?;
    }
    Ok(())
}

/// `<snapshot>/compose-dumps/<app>`: the dumps of an app's services that
/// declare a `tond.backup.dump` hook, one `<service>.dump` each.
fn compose_dump_root(snapshot: &Path, app_name: &str) -> PathBuf {
    snapshot.join("compose-dumps").join(app_name)
}

/// Run the dump hook of every service of `app_name` that declares one,
/// storing each dump (encrypted) in the snapshot. A failed dump is logged
/// and recorded; the volume copy that follows still happens. Returns one
/// manifest entry per service.
async fn dump_compose_services(
    state: &AppState,
    staging: &Path,
    app_name: &str,
    progress_tx: &Option<ProgressTx>,
) -> Vec<serde_json::Value> {
    let hooks = match compose_backup::discover_hooks(&state.docker, app_name).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("Could not read the backup hooks of {app_name}: {e:#}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for hook in hooks {
        emit(
            progress_tx,
            "dump",
            &format!("Dumping {app_name}/{}…", hook.service),
            false,
        )
        .await;
        let root = compose_dump_root(staging, app_name);
        let dest = root.join(format!("{}.dump", hook.service));
        let result = async {
            tokio::fs::create_dir_all(&root).await?;
            let bytes = compose_backup::run_dump(&state.docker, &hook, &dest).await?;
            seal(dest.clone()).await?;
            anyhow::Ok(bytes)
        }
        .await;
        match result {
            Ok(bytes) => out.push(json!({
                "app": app_name, "service": hook.service, "ok": true, "bytes": bytes,
            })),
            Err(e) => {
                let _ = tokio::fs::remove_file(&dest).await;
                tracing::warn!(
                    "The backup dump of {app_name}/{} failed (its volumes are still copied): {e:#}",
                    hook.service
                );
                out.push(json!({
                    "app": app_name, "service": hook.service, "ok": false,
                    "error": format!("{e:#}"),
                }));
            }
        }
    }
    out
}

/// Feed each service of `app_name` that declares a `tond.backup.restore`
/// hook its dump from the snapshot (after its volumes were restored and its
/// containers started). Fails with every service whose restore failed.
pub(crate) async fn restore_compose_dumps(
    state: &AppState,
    snapshot: &Path,
    app_name: &str,
) -> Result<()> {
    let root = compose_dump_root(snapshot, app_name);
    if !root.is_dir() {
        return Ok(());
    }
    let hooks = compose_backup::discover_hooks(&state.docker, app_name).await?;
    let mut failures = Vec::new();
    for hook in hooks.iter().filter(|h| h.restore.is_some()) {
        let file = format!("{}.dump", hook.service);
        if !root.join(&file).is_file() {
            continue;
        }
        let result = async {
            let plain =
                snapshot_files::plain_subset_async(root.clone(), vec![file.clone()]).await?;
            compose_backup::run_restore(&state.docker, hook, &plain.path().join(&file)).await
        }
        .await;
        match result {
            Ok(()) => tracing::info!("Loaded the backup dump of {app_name}/{}", hook.service),
            Err(e) => failures.push(format!("{}: {e:#}", hook.service)),
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "the volumes were restored, but loading the dump failed for {}",
        failures.join("; ")
    );
    Ok(())
}

/// Names of the directories directly inside `base` (empty when missing).
async fn list_subdirs(base: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(base).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
                && let Some(n) = entry.file_name().to_str().map(|s| s.to_string())
                // Hidden entries are Tond's own (TLS copies, work dirs).
                && !n.starts_with('.')
            {
                names.push(n);
            }
        }
    }
    names
}

/// Copy `src` into the snapshot at `dest`, encrypted and hard-linked
/// against `prev` (`snapshot_files::encrypt_tree`), on a blocking thread.
async fn copy_tree(src: PathBuf, dest: PathBuf, prev: Option<PathBuf>) -> Result<(i64, i64)> {
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dest)?;
        snapshot_files::encrypt_tree(&src, &dest, prev.as_deref())
    })
    .await
    .context("snapshot copy task failed")?
}

/// Encrypt a plain file just written into a snapshot (in place).
async fn seal(path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || snapshot_files::seal_in_place(&path))
        .await
        .context("snapshot copy task failed")?
}

/// Write `bytes` into a snapshot as an encrypted file.
async fn write_sealed(path: PathBuf, bytes: Vec<u8>) -> Result<()> {
    tokio::task::spawn_blocking(move || snapshot_files::write_encrypted(&path, &bytes))
        .await
        .context("snapshot copy task failed")?
}

/// A snapshot's manifest file.
pub(crate) const MANIFEST_FILE: &str = "manifest.json";

/// Files a database dump may leave in `databases/<name>/`.
const DUMP_FILES: [&str; 3] = ["dump.archive", "dump.sql", "dump.rdb"];

/// The dump and metadata of database `db_name` in the local snapshot
/// `snapshot`, decrypted into a temporary directory (removed when dropped).
pub(crate) async fn plain_database_files(
    snapshot: &Path,
    db_name: &str,
) -> Result<snapshot_files::PlainDir> {
    snapshot_files::plain_subset_async(
        snapshot.join("databases").join(db_name),
        database_plain_files(),
    )
    .await
}

/// What restoring a database reads from `databases/<name>/` as plain files.
fn database_plain_files() -> Vec<String> {
    let mut files: Vec<String> = DUMP_FILES.iter().map(|f| f.to_string()).collect();
    files.push(DB_META_FILE.to_string());
    files.push("volume/dump.rdb".to_string());
    files
}

// ── Remote upload ─────────────────────────────────────────────────────────────

/// Upload an encrypted archive of the snapshot and record the outcome.
pub(crate) async fn upload_snapshot(
    state: &AppState,
    id: &str,
    staging: &Path,
    sp_id: &str,
    keep_local: bool,
) {
    match upload_archive(state, id, staging, sp_id).await {
        Ok((key, summary)) => {
            let _ = sqlx::query(
                "UPDATE snapshots SET remote_key = ?, storage_provider_id = ?, checksum_sha256 = ?,
                 archive_bytes = ?, remote_status = 'uploaded', remote_error = NULL WHERE id = ?",
            )
            .bind(&key)
            .bind(sp_id)
            .bind(&summary.sha256)
            .bind(summary.bytes as i64)
            .bind(id)
            .execute(&state.db)
            .await;
            tracing::info!(
                "Snapshot {id} uploaded ({:.1} MB encrypted)",
                summary.bytes as f64 / 1_048_576.0
            );
            if !keep_local {
                let _ = tokio::fs::remove_dir_all(staging).await;
                let _ = sqlx::query("UPDATE snapshots SET local_path = NULL WHERE id = ?")
                    .bind(id)
                    .execute(&state.db)
                    .await;
            }
        }
        Err(e) => {
            tracing::warn!("Remote upload of snapshot {id} failed: {e:#}");
            let _ = sqlx::query(
                "UPDATE snapshots SET remote_status = 'failed', remote_error = ? WHERE id = ?",
            )
            .bind(format!("{e:#}"))
            .bind(id)
            .execute(&state.db)
            .await;
        }
    }
}

/// Stream `staging` → tar → gzip → encryption → S3 multipart upload. Memory
/// use is bounded by a few `storage::PART_SIZE` buffers.
async fn upload_archive(
    state: &AppState,
    id: &str,
    staging: &Path,
    sp_id: &str,
) -> Result<(String, snapshot_archive::ArchiveSummary)> {
    let provider = sqlx::query_as::<_, tond_common::StorageProvider>(
        "SELECT * FROM storage_providers WHERE id = ?",
    )
    .bind(sp_id)
    .fetch_optional(&state.db)
    .await?
    .context("the configured storage provider no longer exists")?;
    let provider =
        decrypt_provider_secret(provider).context("cannot read the storage provider secret")?;
    let key = upload_key_for_snapshot(id);

    let (tx, rx) = mpsc::channel::<storage::Part>(1);
    let src = staging.to_path_buf();
    let snapshot_id = id.to_string();
    let writer = tokio::task::spawn_blocking(move || {
        let sink = PartWriter::new(tx, storage::PART_SIZE);
        let (summary, sink) = archive_snapshot_dir(&src, sink, &snapshot_id)?;
        sink.finish()?;
        Ok::<_, anyhow::Error>(summary)
    });

    let uploaded = storage::upload_stream(&provider, &key, rx).await;
    let summary = writer.await.context("archive task failed")??;
    uploaded?;
    // The manifest next to the archive lets another server list and import
    // this snapshot without this server's database.
    let manifest = snapshot_remote::remote_manifest(staging, id, &key, &summary).await?;
    storage::put_object(
        &provider,
        &snapshot_remote::manifest_key(id),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await
    .context("the archive uploaded but its manifest didn't")?;
    Ok((key, summary))
}

/// Stream the local (encrypted) snapshot directory `src` into `out` as one
/// snapshot archive bound to `snapshot_id`. Blocking.
pub(crate) fn archive_snapshot_dir<W: Write>(
    src: &Path,
    out: W,
    snapshot_id: &str,
) -> Result<(snapshot_archive::ArchiveSummary, W)> {
    let secrets = snapshot_files::secrets();
    let keys: Vec<&str> = secrets.iter().map(String::as_str).collect();
    snapshot_archive::write_archive(
        src,
        out,
        &secret_key::current_secret(),
        snapshot_files::archive_kind(),
        snapshot_id,
        snapshot_archive::Source::Encrypted {
            kind: snapshot_files::local_kind(),
            secrets: &keys,
        },
    )
}

/// A `Write` sink that cuts the stream into `part_size` parts and sends them
/// to an async consumer (blocking the writer thread while the consumer is
/// busy, which bounds memory). `finish` sends the tail and the end marker; a
/// writer dropped without `finish` makes the consumer fail instead of
/// completing a truncated upload.
struct PartWriter {
    tx: mpsc::Sender<storage::Part>,
    buf: Vec<u8>,
    part_size: usize,
}

impl PartWriter {
    fn new(tx: mpsc::Sender<storage::Part>, part_size: usize) -> Self {
        Self {
            tx,
            buf: Vec::with_capacity(part_size),
            part_size,
        }
    }

    fn send(&self, part: storage::Part) -> io::Result<()> {
        self.tx
            .blocking_send(part)
            .map_err(|_| io::Error::other("the upload stopped"))
    }

    fn finish(mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let tail = std::mem::take(&mut self.buf);
            self.send(storage::Part::Data(tail))?;
        }
        self.send(storage::Part::End)
    }
}

impl Write for PartWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = self.part_size - self.buf.len();
        let n = room.min(data.len());
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == self.part_size {
            let part = std::mem::replace(&mut self.buf, Vec::with_capacity(self.part_size));
            self.send(storage::Part::Data(part))?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Download a snapshot's archive, check it, and extract it into its local
/// snapshot directory. Nothing is extracted unless the whole archive
/// matches its recorded checksum and authenticates.
async fn fetch_remote_snapshot(state: &AppState, snap: &Snapshot) -> Result<PathBuf> {
    let (Some(key), Some(sp_id)) = (&snap.remote_key, &snap.storage_provider_id) else {
        anyhow::bail!("Snapshot has no local copy and no remote copy");
    };
    let provider = sqlx::query_as::<_, tond_common::StorageProvider>(
        "SELECT * FROM storage_providers WHERE id = ?",
    )
    .bind(sp_id)
    .fetch_optional(&state.db)
    .await?
    .context("Storage provider not found")?;
    let provider =
        decrypt_provider_secret(provider).context("cannot read the storage provider secret")?;

    tokio::fs::create_dir_all(snapshots_dir()).await?;
    let download = snapshots_dir().join(format!(".{}.download", snap.id));
    let staging = snapshot_dir(&snap.id);

    let result = async {
        let (_, sha256) = storage::download_to_file(&provider, key, &download).await?;
        if let Some(expected) = &snap.checksum_sha256 {
            anyhow::ensure!(
                &sha256 == expected,
                "the downloaded archive does not match the checksum recorded at upload \
                 (corrupted or replaced); nothing was restored"
            );
        }
        let archive = download.clone();
        let dest = staging.clone();
        let id = snap.id.clone();
        tokio::task::spawn_blocking(move || {
            verify_archive_file(&archive, Some(&id))?;
            extract_archive_file(&archive, &dest, &id)
        })
        .await
        .context("restore task failed")?
    }
    .await;
    let _ = tokio::fs::remove_file(&download).await;
    result?;

    sqlx::query("UPDATE snapshots SET local_path = ? WHERE id = ?")
        .bind(staging.to_string_lossy().as_ref())
        .bind(&snap.id)
        .execute(&state.db)
        .await?;
    Ok(staging)
}

/// Authenticate every record of the archive file (blocking). Returns the
/// snapshot id it holds; `expected_id`, when given, must match it.
pub(crate) fn verify_archive_file(archive: &Path, expected_id: Option<&str>) -> Result<String> {
    let secrets = snapshot_files::secrets();
    let keys: Vec<&str> = secrets.iter().map(String::as_str).collect();
    snapshot_archive::verify_archive(
        std::fs::File::open(archive)?,
        &keys,
        snapshot_files::archive_kind(),
        expected_id,
    )
}

/// Extract a verified archive into the local snapshot directory `dest`
/// (each file encrypted again for local storage), next to it first and then
/// moved into place, so a failed extraction never leaves a half-restored
/// directory. Blocking.
pub(crate) fn extract_archive_file(archive: &Path, dest: &Path, snapshot_id: &str) -> Result<()> {
    let secrets = snapshot_files::secrets();
    let keys: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let secret = secret_key::current_secret();
    let partial = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    if let Err(e) = snapshot_archive::extract_archive(
        std::fs::File::open(archive)?,
        &keys,
        snapshot_files::archive_kind(),
        Some(snapshot_id),
        &partial,
        snapshot_archive::Target::Encrypted {
            kind: snapshot_files::local_kind(),
            secret: &secret,
        },
    ) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::rename(&partial, dest)?;
    Ok(())
}

// ── Full restore (with pre-restore safety net) ────────────────────────────────

/// Launch a full restore in a background task. Returns the pre_restore snapshot ID.
///
/// The snapshot is fetched (a remote copy downloaded) and verified before
/// anything else happens, so a snapshot this server can't read — another
/// TOND_SECRET_KEY, a corrupted archive — fails here with nothing changed.
pub async fn restore_full(state: &AppState, snapshot_id: String) -> Result<String> {
    // Verify snapshot is completed
    let snap: Option<Snapshot> = sqlx::query_as::<_, Snapshot>(
        "SELECT * FROM snapshots WHERE id = ? AND status = 'completed'",
    )
    .bind(&snapshot_id)
    .fetch_optional(&state.db)
    .await?;

    let snap = snap.ok_or_else(|| anyhow::anyhow!("Snapshot not found or not completed"))?;
    anyhow::ensure!(
        snap.scope == "full",
        "This snapshot only holds {}; restore it from its app or database page",
        snap.scope
    );
    let staging = ensure_local(state, &snap).await?;
    let manifest = read_manifest(&staging).await?;
    let snap = fetch_snapshot(state, &snapshot_id).await?;

    // Auto-create a pre-restore safety net
    let pre_restore_id = create_snapshot(
        state,
        SnapshotTrigger::PreRestore {
            preceding_snapshot_id: snapshot_id.clone(),
        },
        None,
    )
    .await?;
    let pre_restore = fetch_snapshot(state, &pre_restore_id).await.ok();

    let state_clone = state.clone();
    tokio::spawn(async move {
        match do_full_restore(&state_clone, &snap, &staging, &manifest).await {
            Err(e) => tracing::error!("Full restore failed: {:#}", e),
            Ok(()) => {
                // What the restored database doesn't know: the snapshot it
                // came from, the safety net, and that apps and databases
                // must be started (or redeployed) at the next start.
                if let Err(e) =
                    crate::restore_resume::write_marker(&snap, pre_restore.as_ref(), &manifest)
                {
                    tracing::error!("Could not record the restore for the next start: {e:#}");
                }
                tracing::info!(
                    "Full restore of {} completed — exiting for systemd restart",
                    snapshot_id
                );
                std::process::exit(0);
            }
        }
    });

    Ok(pre_restore_id)
}

/// The snapshot's manifest (`manifest.json`, encrypted like every file).
pub(crate) async fn read_manifest(snapshot: &Path) -> Result<serde_json::Value> {
    let path = snapshot.join(MANIFEST_FILE);
    let bytes = tokio::task::spawn_blocking(move || snapshot_files::read_encrypted(&path))
        .await
        .context("restore task failed")??;
    serde_json::from_slice(&bytes).context("the snapshot's manifest is unreadable")
}

async fn do_full_restore(
    state: &AppState,
    snap: &Snapshot,
    staging: &Path,
    manifest: &serde_json::Value,
) -> Result<()> {
    tracing::info!("Full restore of {}: replacing all data", snap.id);

    // Stop all app containers
    let apps = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps")
        .fetch_all(&state.db)
        .await?;
    for app in &apps {
        if app.source_type == "compose" {
            for cid in compose_containers(state, &app.name).await {
                let _ = docker::stop_container(&state.docker, &cid).await;
            }
        } else if let Some(ref cid) = app.container_id {
            let _ = docker::stop_container(&state.docker, cid).await;
        }
    }

    // Stop all managed DB containers
    let dbs = sqlx::query_as::<_, ManagedDatabase>("SELECT * FROM databases")
        .fetch_all(&state.db)
        .await?;
    for db in &dbs {
        if let Some(ref cid) = db.container_id {
            let _ = docker::stop_container(&state.docker, cid).await;
        }
    }

    // Replace SQLite: decrypted next to the live file, then renamed over it.
    // The live WAL is checkpointed and removed first, so none of its frames
    // can be replayed onto the restored database at the next start.
    let db_dest = PathBuf::from(db_path());
    let db_src = staging.join("tond.db");
    if db_src.exists() {
        if let Some(parent) = db_dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&state.db)
            .await;
        let tmp = db_dest.with_extension("db.restoring");
        let (src, out) = (db_src.clone(), tmp.clone());
        tokio::task::spawn_blocking(move || snapshot_files::decrypt_file(&src, &out))
            .await
            .context("restore task failed")??;
        tokio::fs::rename(&tmp, &db_dest).await?;
        for suffix in ["-wal", "-shm"] {
            let _ = tokio::fs::remove_file(format!("{}{suffix}", db_dest.display())).await;
        }
        // Restore tond-proxy's read access (and the snapshot's mode with it).
        db::share_with_proxy(&db_dest);
    }

    // Replace app volumes
    let vol_base = PathBuf::from(data_dir()).join("volumes");
    for app_name in list_subdirs(&staging.join("volumes")).await {
        let dest = vol_base.join(&app_name);
        if dest.exists() {
            tokio::fs::remove_dir_all(&dest).await?;
        }
        snapshot_files::decrypt_tree_async(staging.join("volumes").join(&app_name), dest).await?;
    }

    // Replace the named volumes of compose apps
    let staging_named = staging.join("compose-volumes");
    if staging_named.exists() {
        for app_name in list_subdirs(&staging_named).await {
            if let Err(e) = restore_compose_volumes(state, staging, &app_name).await {
                tracing::warn!("Could not restore the volumes of {app_name}: {e:#}");
            }
        }
    }

    // Replace database volumes
    let db_vol_base = PathBuf::from(data_dir()).join("databases");
    for db_name in list_subdirs(&staging.join("databases")).await {
        let raw = staging.join("databases").join(&db_name).join("volume");
        if raw.exists() {
            let dest_vol = db_vol_base.join(&db_name);
            if dest_vol.exists() {
                tokio::fs::remove_dir_all(&dest_vol).await?;
            }
            snapshot_files::decrypt_tree_async(raw, dest_vol).await?;
        }
    }

    // Replace SSH keys
    let keys_src = staging.join("keys");
    let keys_dest = PathBuf::from(data_dir()).join("keys");
    if keys_src.exists() {
        if keys_dest.exists() {
            tokio::fs::remove_dir_all(&keys_dest).await?;
        }
        snapshot_files::decrypt_tree_async(keys_src, keys_dest).await?;
    }

    // TLS certificates that are still valid
    if let Err(e) = restore_certificates(staging, manifest).await {
        tracing::warn!("Could not restore the TLS certificates: {e:#}");
    }

    Ok(())
}

/// Put back the snapshot's TLS certificates that are valid for at least
/// another day (the proxy orders the others again), owned by whoever owns
/// `<data>/certs` on this server (tond-proxy, whose uid may differ from the
/// server the snapshot came from). The ACME account comes back too when this
/// server has none yet.
async fn restore_certificates(staging: &Path, manifest: &serde_json::Value) -> Result<()> {
    let src_root = staging.join("certs");
    let dest_root = PathBuf::from(data_dir()).join("certs");
    let valid_dirs = valid_certificate_dirs(manifest, Utc::now());
    if !src_root.is_dir() || valid_dirs.is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&dest_root).await?;
    let owner = tokio::fs::metadata(&dest_root).await?;
    for dir in list_subdirs(&src_root).await {
        if !valid_dirs.contains(&dir) {
            continue;
        }
        let dest = dest_root.join(&dir);
        if dest.exists() {
            tokio::fs::remove_dir_all(&dest).await?;
        }
        snapshot_files::decrypt_tree_async(src_root.join(&dir), dest.clone()).await?;
        hand_over(&dest, &owner);
    }
    let account_src = staging.join("acme_account.json");
    let account_dest = PathBuf::from(data_dir()).join("acme_account.json");
    if account_src.is_file() && !account_dest.exists() {
        let out = account_dest.clone();
        tokio::task::spawn_blocking(move || snapshot_files::decrypt_file(&account_src, &out))
            .await
            .context("restore task failed")??;
        hand_over(&account_dest, &owner);
    }
    Ok(())
}

/// The certs/ directories of the manifest's certificates that stay valid
/// for at least a day after `now`.
pub(crate) fn valid_certificate_dirs(
    manifest: &serde_json::Value,
    now: chrono::DateTime<Utc>,
) -> Vec<String> {
    let horizon = now + chrono::Duration::days(1);
    manifest
        .get("certificates")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter_map(|cert| {
            let dir = cert.get("dir")?.as_str()?;
            let expires = cert.get("expires_at")?.as_str()?;
            let expires = chrono::DateTime::parse_from_rfc3339(expires)
                .map(|t| t.with_timezone(&Utc))
                .or_else(|_| {
                    chrono::NaiveDateTime::parse_from_str(expires, "%Y-%m-%d %H:%M:%S")
                        .map(|t| t.and_utc())
                })
                .ok()?;
            // Never a path: one directory name directly under certs/.
            let plain_name = !dir.is_empty() && !dir.contains('/') && !dir.starts_with('.');
            (expires > horizon && plain_name).then(|| dir.to_string())
        })
        .collect()
}

/// Give `path` (recursively) the owner of `owner`, directories 0700 and
/// files 0600: certificates and keys belong to tond-proxy alone.
fn hand_over(path: &Path, owner: &std::fs::Metadata) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        let p = entry.path();
        let _ = std::os::unix::fs::lchown(p, Some(owner.uid()), Some(owner.gid()));
        let mode = if entry.file_type().is_dir() {
            0o700
        } else {
            0o600
        };
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
    }
}

// ── App-scope restore ─────────────────────────────────────────────────────────

pub async fn restore_app(state: &AppState, snapshot_id: &str, app_name: &str) -> Result<()> {
    let snap = fetch_snapshot(state, snapshot_id).await?;
    let local_path = ensure_local(state, &snap).await?;
    let vol_src = local_path.join("volumes").join(app_name);
    let named_src = compose_volume_root(&local_path, app_name);
    let dumps_src = compose_dump_root(&local_path, app_name);

    if !vol_src.exists() && !named_src.exists() && !dumps_src.exists() {
        anyhow::bail!("Snapshot does not contain volumes for app '{}'", app_name);
    }

    // Find the app's container
    let app = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps WHERE name = ? LIMIT 1")
        .bind(app_name)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("App '{}' not found", app_name))?;

    // Stop the container (every service of a compose app)
    let containers: Vec<String> = if app.source_type == "compose" {
        compose_containers(state, app_name).await
    } else {
        app.container_id.iter().cloned().collect()
    };
    for cid in &containers {
        let _ = docker::stop_container(&state.docker, cid).await;
    }

    // Replace the volume directory (bind mounts)
    if vol_src.exists() {
        let vol_dest = PathBuf::from(data_dir()).join("volumes").join(app_name);
        if vol_dest.exists() {
            tokio::fs::remove_dir_all(&vol_dest).await?;
        }
        snapshot_files::decrypt_tree_async(vol_src, vol_dest).await?;
    }
    // ...and a compose app's named volumes
    if app.source_type == "compose" {
        restore_compose_volumes(state, &local_path, app_name).await?;
    }

    // Start the containers again
    for cid in &containers {
        let _ = docker::start_container(&state.docker, cid).await;
    }

    // Load each service's dump (services with a `tond.backup.restore` hook)
    if app.source_type == "compose" {
        restore_compose_dumps(state, &local_path, app_name).await?;
    }

    Ok(())
}

// ── Database-scope restore ────────────────────────────────────────────────────

/// Restore one database from a snapshot that holds it (a full snapshot or a
/// backup of that database). A backup of the current data is taken first;
/// its ID is returned so the restore can be undone.
pub async fn restore_database(
    state: &AppState,
    snapshot_id: &str,
    db_name: &str,
) -> Result<String> {
    let snap = fetch_snapshot(state, snapshot_id).await?;
    anyhow::ensure!(
        snap.scope == "full" || snap.scope == format!("database:{db_name}"),
        "This snapshot doesn't include the database '{db_name}'"
    );

    let db_row =
        sqlx::query_as::<_, ManagedDatabase>("SELECT * FROM databases WHERE name = ? LIMIT 1")
            .bind(db_name)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Database '{}' not found", db_name))?;
    let db_row = secrets::open_database(db_row)?;

    // Fetch and verify the snapshot before touching the live data.
    let local_path = ensure_local(state, &snap).await?;
    let db_staging = local_path.join("databases").join(db_name);
    if !db_staging.exists() {
        anyhow::bail!("This snapshot doesn't include the database '{}'", db_name);
    }

    let safety_net = create_snapshot(
        state,
        SnapshotTrigger::PreRestoreDatabase {
            db_name: db_name.to_string(),
        },
        None,
    )
    .await
    .context("couldn't back up the current data first, so nothing was restored")?;

    // Stop container
    if let Some(ref cid) = db_row.container_id {
        let _ = docker::stop_container(&state.docker, cid).await;
    }

    // Replace raw volume
    let dest_vol = PathBuf::from(data_dir()).join("databases").join(db_name);
    let raw_src = db_staging.join("volume");
    if raw_src.exists() {
        if dest_vol.exists() {
            tokio::fs::remove_dir_all(&dest_vol).await?;
        }
        snapshot_files::decrypt_tree_async(raw_src, dest_vol).await?;
    }

    // Start container
    if let Some(ref cid) = db_row.container_id {
        let _ = docker::start_container(&state.docker, cid).await;
    }

    // Import the logical dump over the raw copy (a raw copy of a running
    // engine isn't consistent on its own) once the engine accepts connections.
    let has_dump = ["dump.archive", "dump.sql"]
        .iter()
        .any(|f| db_staging.join(f).exists());
    if has_dump && let Some(ref cid) = db_row.container_id {
        let imported = async {
            let plain =
                snapshot_files::plain_subset_async(db_staging.clone(), database_plain_files())
                    .await?;
            db_exec::wait_ready(
                &state.docker,
                cid,
                &db_row,
                std::time::Duration::from_secs(180),
            )
            .await?;
            import_database_dump(&state.docker, &db_row, plain.path()).await
        }
        .await;
        if let Err(e) = imported {
            tracing::warn!("Importing the dump into {db_name} failed: {e:#}");
        }
    }

    Ok(safety_net)
}

// ── Restore into a new database ───────────────────────────────────────────────

/// What a snapshot records about each database it holds
/// (`databases/<name>/meta.json`), so it can be restored into a new database
/// of the same engine, version and variant even after the original is gone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DatabaseMeta {
    pub db_type: String,
    pub version: Option<String>,
    #[serde(default)]
    pub variant: String,
    /// The engine database name (MongoDB restores rename its namespace)
    pub db_name: Option<String>,
    pub memory_limit: Option<i64>,
}

pub(crate) const DB_META_FILE: &str = "meta.json";

impl DatabaseMeta {
    pub fn of(db: &ManagedDatabase) -> Self {
        Self {
            db_type: db.db_type.clone(),
            version: Some(db.effective_version()),
            variant: db.variant.clone(),
            db_name: db.db_name.clone(),
            memory_limit: db.memory_limit,
        }
    }
}

/// Restore the database `source` held by a snapshot into a NEW managed
/// database `name` — same engine, version and variant, fresh credentials,
/// its own port. The original is not touched. If anything fails the new
/// database is removed again. Returns the new database's row.
pub async fn restore_to_new_database(
    state: &AppState,
    snapshot_id: &str,
    source: &str,
    name: &str,
    project_id: Option<String>,
) -> Result<ManagedDatabase> {
    use crate::handlers::databases as dbh;

    let snap = fetch_snapshot(state, snapshot_id).await?;
    anyhow::ensure!(
        snap.scope == "full" || snap.scope == format!("database:{source}"),
        "This snapshot doesn't include the database '{source}'"
    );
    dbh::validate_database_name(name).map_err(|e| anyhow::anyhow!(e.0.to_string()))?;
    let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM databases WHERE name = ?)")
        .bind(name)
        .fetch_one(&state.db)
        .await?;
    anyhow::ensure!(
        !taken,
        "A database named '{name}' already exists. Choose another name."
    );

    let local_path = ensure_local(state, &snap).await?;
    let encrypted_dir = local_path.join("databases").join(source);
    anyhow::ensure!(
        encrypted_dir.exists(),
        "This snapshot doesn't include the database '{source}'"
    );
    // The dump and metadata as plain files, removed when this returns.
    let plain = snapshot_files::plain_subset_async(encrypted_dir, database_plain_files()).await?;
    let dump_dir = plain.path().to_path_buf();

    // Engine, version and variant: from the snapshot, or (older snapshots)
    // from the source database if it still exists.
    let source_row: Option<ManagedDatabase> =
        sqlx::query_as("SELECT * FROM databases WHERE name = ? LIMIT 1")
            .bind(source)
            .fetch_optional(&state.db)
            .await?;
    let meta = match tokio::fs::read(dump_dir.join(DB_META_FILE)).await {
        Ok(bytes) => serde_json::from_slice::<DatabaseMeta>(&bytes)
            .context("the snapshot's database description is unreadable")?,
        Err(_) => source_row.as_ref().map(DatabaseMeta::of).with_context(|| {
            format!(
                "This snapshot predates restoring into a new database and '{source}' no longer exists"
            )
        })?,
    };
    anyhow::ensure!(
        tond_common::image_for(&meta.db_type, &meta.variant, meta.version.as_deref()).is_some(),
        "This server can't run {} {} {} any more",
        meta.db_type,
        meta.variant,
        meta.version.as_deref().unwrap_or_default()
    );

    let project_id = project_id.or_else(|| source_row.as_ref().and_then(|r| r.project_id.clone()));
    let mut db = ManagedDatabase::new(name.to_string(), meta.db_type.clone(), project_id);
    db.version = meta.version.clone();
    db.variant = meta.variant.clone();
    db.memory_limit = meta.memory_limit;

    let redis_like = matches!(db.db_type.as_str(), "redis" | "keydb");
    let volume = PathBuf::from(data_dir()).join("databases").join(name);
    if volume.exists() {
        // Leftovers of a database with this name that was deleted by hand.
        let mut entries = tokio::fs::read_dir(&volume).await?;
        anyhow::ensure!(
            entries.next_entry().await?.is_none(),
            "{} already holds data; remove it or choose another name",
            volume.display()
        );
    }
    if redis_like {
        // Redis loads dump.rdb when it starts: put it in place first.
        place_redis_dump(&dump_dir, &volume).await?;
    }

    if let Err(e) = dbh::provision_database(state, &mut db).await {
        if redis_like {
            let _ = tokio::fs::remove_dir_all(&volume).await;
        }
        anyhow::bail!("{}", e.0);
    }

    let loaded = async {
        let cid = db.container_id.clone().context("no container")?;
        db_exec::wait_ready(
            &state.docker,
            &cid,
            &db,
            std::time::Duration::from_secs(240),
        )
        .await?;
        load_dump_into(&state.docker, &db, &cid, &dump_dir, meta.db_name.as_deref()).await
    }
    .await;
    if let Err(e) = loaded {
        let _ = dbh::destroy_database(state, &db).await;
        return Err(e.context(format!(
            "restoring into '{name}' failed, so '{name}' was removed again; '{source}' is untouched"
        )));
    }
    Ok(db)
}

// ── Backup scheduler ──────────────────────────────────────────────────────────

/// A scheduled backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScheduledJob {
    /// A full snapshot (backup_config)
    Full,
    /// One database's own schedule
    Database(String),
}

/// Wake the scheduler to re-plan (a database schedule changed).
pub fn schedules_changed() {
    crate::runtime::changes::notify(crate::runtime::changes::Table::BackupConfig);
}

/// The next run of one cron expression (for the API).
pub fn next_run(expr: &str) -> Option<chrono::DateTime<Utc>> {
    crate::cron::parse_schedule(expr)
        .ok()
        .and_then(|s| s.after(&Utc::now()).next())
}

/// The key of a job in `backup_schedule_state`.
fn job_key(job: &ScheduledJob) -> String {
    match job {
        ScheduledJob::Full => "full".to_string(),
        ScheduledJob::Database(name) => format!("database:{name}"),
    }
}

/// A scheduled backup to run now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DueJob {
    pub job: ScheduledJob,
    /// The latest slot it runs for (recorded as `last_slot`)
    pub slot: chrono::DateTime<Utc>,
    /// The slot came due before this process started (the server was down)
    pub catch_up: bool,
    /// How many slots passed since the last run (one backup covers them all)
    pub missed: usize,
}

/// What the scheduler does next.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SchedulePlan {
    /// Jobs to run now, in order
    pub due: Vec<DueJob>,
    /// Jobs seen for the first time (or with a changed schedule): their
    /// state starts at `now`, so they never catch up on slots before that
    pub init: Vec<(String, String)>,
    /// When the next slot comes due
    pub next: Option<chrono::DateTime<Utc>>,
}

/// Decide what runs: a job whose first slot after its `last_slot` has
/// passed is due — once, however many slots passed. `states` maps job keys
/// to (schedule, last_slot); `started_at` is when this process started, to
/// tell a catch-up (a slot missed while the server was down) from a slot
/// that just came due.
pub(crate) fn plan_due(
    jobs: &[(ScheduledJob, String)],
    states: &std::collections::HashMap<String, (String, chrono::DateTime<Utc>)>,
    now: chrono::DateTime<Utc>,
    started_at: chrono::DateTime<Utc>,
) -> SchedulePlan {
    let mut plan = SchedulePlan::default();
    for (job, expr) in jobs {
        let Ok(schedule) = crate::cron::parse_schedule(expr) else {
            continue;
        };
        let key = job_key(job);
        let after_now = schedule.after(&now).next();
        let last = match states.get(&key) {
            Some((stored, last)) if stored == expr => *last,
            _ => {
                plan.init.push((key, expr.clone()));
                plan.next = min_time(plan.next, after_now);
                continue;
            }
        };
        let Some(first) = schedule.after(&last).next() else {
            continue;
        };
        if first > now {
            plan.next = min_time(plan.next, Some(first));
            continue;
        }
        // The latest slot that has passed (bounded: a minutely schedule
        // missed for a year is still few enough).
        let mut slot = first;
        let mut missed = 1;
        for t in schedule.after(&first).take(600_000) {
            if t > now {
                break;
            }
            slot = t;
            missed += 1;
        }
        plan.due.push(DueJob {
            job: job.clone(),
            slot,
            catch_up: first < started_at,
            missed,
        });
        plan.next = min_time(plan.next, after_now);
    }
    plan
}

fn min_time(
    a: Option<chrono::DateTime<Utc>>,
    b: Option<chrono::DateTime<Utc>>,
) -> Option<chrono::DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The schedules in force: the full snapshot (when enabled) and each
/// database with a schedule.
async fn scheduled_jobs(state: &AppState) -> Result<Vec<(ScheduledJob, String)>> {
    let cfg = get_backup_config(state).await?;
    let dbs: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, backup_schedule FROM databases WHERE backup_schedule IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;
    let mut jobs = Vec::new();
    if cfg.enabled != 0 {
        jobs.push((ScheduledJob::Full, cfg.full_schedule_cron));
    }
    jobs.extend(
        dbs.into_iter()
            .map(|(name, expr)| (ScheduledJob::Database(name), expr)),
    );
    Ok(jobs)
}

/// Read the schedule state, forgetting jobs that are no longer scheduled
/// (so turning a schedule back on starts over instead of catching up).
async fn schedule_states(
    state: &AppState,
    jobs: &[(ScheduledJob, String)],
) -> Result<std::collections::HashMap<String, (String, chrono::DateTime<Utc>)>> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT job, schedule, last_slot FROM backup_schedule_state")
            .fetch_all(&state.db)
            .await?;
    let live: Vec<String> = jobs.iter().map(|(j, _)| job_key(j)).collect();
    let mut out = std::collections::HashMap::new();
    for (job, schedule, last_slot) in rows {
        let parsed =
            chrono::DateTime::parse_from_rfc3339(&last_slot).map(|t| t.with_timezone(&Utc));
        match parsed {
            Ok(last) if live.contains(&job) => {
                out.insert(job, (schedule, last));
            }
            _ => {
                sqlx::query("DELETE FROM backup_schedule_state WHERE job = ?")
                    .bind(&job)
                    .execute(&state.db)
                    .await?;
            }
        }
    }
    Ok(out)
}

async fn set_last_slot(
    state: &AppState,
    job: &str,
    schedule: &str,
    slot: chrono::DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO backup_schedule_state (job, schedule, last_slot) VALUES (?, ?, ?)
         ON CONFLICT(job) DO UPDATE SET schedule = excluded.schedule, last_slot = excluded.last_slot",
    )
    .bind(job)
    .bind(schedule)
    .bind(slot.to_rfc3339())
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Sleeps until the next scheduled backup — the full snapshot or any
/// database's own schedule, whichever is first. A change to backup_config or
/// to a database schedule (`schedules_changed`) wakes it to re-plan. With
/// nothing scheduled it waits for a change. Jobs due together run one after
/// another (they may dump the same database).
///
/// Each job's last slot is kept in `backup_schedule_state`, so a backup that
/// came due while the server was down runs once when it starts again — not
/// once per missed slot — logged, audited (`backup_catch_up`) and noted on
/// the snapshot as a catch-up.
pub async fn backup_scheduler(state: AppState) {
    use crate::runtime::changes::{Table, Watch};
    use crate::runtime::loops::{self, Wake};

    const IDLE: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
    let lp = loops::register("backup-scheduler");
    let mut watch = Watch::new(&[Table::BackupConfig]);
    let started_at = crate::runtime::process_started_at();

    loop {
        let planned = async {
            let jobs = scheduled_jobs(&state).await?;
            let states = schedule_states(&state, &jobs).await?;
            let now = Utc::now();
            let plan = plan_due(&jobs, &states, now, started_at);
            for (key, expr) in &plan.init {
                set_last_slot(&state, key, expr, now).await?;
            }
            // Claim the due slots before running, so a re-plan (or a
            // restart mid-backup) never runs them twice.
            for due in &plan.due {
                let expr = jobs
                    .iter()
                    .find(|(j, _)| *j == due.job)
                    .map(|(_, e)| e.clone())
                    .unwrap_or_default();
                set_last_slot(&state, &job_key(&due.job), &expr, due.slot).await?;
            }
            anyhow::Ok(plan)
        }
        .await;
        let plan = match planned {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("backup_scheduler: failed to read the schedules: {e:#}");
                lp.wait(Some(std::time::Duration::from_secs(60)), &mut watch)
                    .await;
                continue;
            }
        };

        if !plan.due.is_empty() {
            if crate::runtime::shutdown::is_shutting_down() {
                return;
            }
            drop(lp.run());
            let state_clone = state.clone();
            crate::runtime::shutdown::spawn_tracked(async move {
                for due in plan.due {
                    run_scheduled(&state_clone, due).await;
                }
            });
            continue;
        }

        let Some(next) = plan.next else {
            lp.wait(Some(IDLE), &mut watch).await;
            continue;
        };
        let wait = (next - Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(1));
        tracing::debug!("backup_scheduler: next run in {:?}", wait);
        if lp.wait(Some(wait), &mut watch).await == Wake::Event {
            continue; // a schedule changed: re-plan
        }
        if crate::runtime::shutdown::is_shutting_down() {
            return;
        }
    }
}

/// Run one due scheduled backup. A failure marks the snapshot failed, which
/// the alert engine's "backup failed" rule picks up.
async fn run_scheduled(state: &AppState, due: DueJob) {
    let trigger = match &due.job {
        ScheduledJob::Full => SnapshotTrigger::Scheduled,
        ScheduledJob::Database(db_name) => SnapshotTrigger::ScheduledDatabase {
            db_name: db_name.clone(),
        },
    };
    let notes = due.catch_up.then(|| {
        format!(
            "Catch-up: the backup due at {} was missed while Tond was down{}",
            due.slot.format("%Y-%m-%d %H:%M UTC"),
            if due.missed > 1 {
                format!(" ({} slots missed, one backup taken)", due.missed)
            } else {
                String::new()
            }
        )
    });
    if let Some(note) = &notes {
        tracing::info!("Scheduled backup {}: {note}", job_key(&due.job));
        crate::audit_log::record(
            &state.db,
            &crate::audit_log::AuditActor::system("backup-scheduler"),
            crate::audit_log::AuditEntry::new("backup_catch_up")
                .kind("backup")
                .detail(format!("job={} {note}", job_key(&due.job))),
        )
        .await;
    }
    let _ = create_snapshot_noted(state, trigger, None, notes).await;
}

// ── Dump lock ─────────────────────────────────────────────────────────────────

/// One dump at a time per database: dumps write to a fixed path inside the
/// container, so a manual backup racing a scheduled one would corrupt both.
fn dump_lock(db_name: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};
    static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut map = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    // Forget locks nobody holds (databases come and go).
    map.retain(|_, l| Arc::strong_count(l) > 1);
    map.entry(db_name.to_string()).or_default().clone()
}

// ── SSE progress helper ───────────────────────────────────────────────────────

async fn emit(tx: &Option<ProgressTx>, step: &str, message: &str, done: bool) {
    if let Some(tx) = tx {
        let _ = tx
            .send(ProgressEvent {
                step: step.to_string(),
                message: message.to_string(),
                done,
            })
            .await;
    }
}

// ── Remote upload key ─────────────────────────────────────────────────────────

fn upload_key_for_snapshot(id: &str) -> String {
    snapshot_remote::archive_key(id)
}

// ── Config helper ─────────────────────────────────────────────────────────────

pub async fn get_backup_config(state: &AppState) -> Result<BackupConfig> {
    sqlx::query_as::<_, BackupConfig>("SELECT * FROM backup_config WHERE id = 1")
        .fetch_one(&state.db)
        .await
        .map_err(Into::into)
}

// ── Retention enforcement ─────────────────────────────────────────────────────

async fn enforce_retention(
    state: &AppState,
    cfg: &BackupConfig,
    trigger_app: Option<&str>,
    db_scope: Option<&str>,
) {
    // Full snapshots: keep last N (never fewer than one: a retention of zero
    // would delete the snapshot just taken)
    if let Ok(old_full) = sqlx::query_as::<_, Snapshot>(
        "SELECT * FROM snapshots WHERE scope = 'full' AND status = 'completed'
         ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        for snap in beyond_retention(&old_full, cfg.full_retention_count) {
            delete_snapshot_files_and_row(state, snap).await;
        }
    }

    // Database backups, per database: its scheduled backups keep its own
    // retention count; the others (manual, before a restore or an upgrade)
    // keep the full-snapshot count.
    if let Some(db_name) = db_scope
        && let Ok(old_db) = sqlx::query_as::<_, Snapshot>(
            "SELECT * FROM snapshots WHERE scope = ? AND status = 'completed'
             ORDER BY created_at DESC",
        )
        .bind(format!("database:{db_name}"))
        .fetch_all(&state.db)
        .await
    {
        let own_retention: i64 =
            sqlx::query_scalar("SELECT backup_retention FROM databases WHERE name = ?")
                .bind(db_name)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .unwrap_or(tond_common::DEFAULT_DB_BACKUP_RETENTION);
        for snap in retention_victims(&old_db, own_retention, cfg.full_retention_count) {
            delete_snapshot_files_and_row(state, snap).await;
        }
    }
    // Pre-deploy snapshots per app
    if let Some(app) = trigger_app {
        let scope = format!("app:{}", app);
        if let Ok(old_pre) = sqlx::query_as::<_, Snapshot>(
            "SELECT * FROM snapshots WHERE scope = ? AND trigger = 'pre_deploy' AND status = 'completed'
             ORDER BY created_at DESC",
        )
        .bind(&scope)
        .fetch_all(&state.db)
        .await
        {
            // (never fewer than one either: the newest is the one just taken)
            for snap in beyond_retention(&old_pre, cfg.pre_deploy_retention_per_app) {
                delete_snapshot_files_and_row(state, snap).await;
            }
        }
    }
}

/// The snapshots (newest first) beyond a retention of `keep`. A `keep` below
/// one (a row written before the API refused it) still keeps the newest, so
/// retention can never delete the snapshot it runs after.
fn beyond_retention(snapshots: &[Snapshot], keep: i64) -> impl Iterator<Item = &Snapshot> {
    snapshots.iter().skip(keep.max(1) as usize)
}

/// Which of one database's completed backups (newest first) retention
/// removes: scheduled ones beyond `scheduled_keep`, the others beyond
/// `other_keep` (each at least one).
fn retention_victims(backups: &[Snapshot], scheduled_keep: i64, other_keep: i64) -> Vec<&Snapshot> {
    let (scheduled, other): (Vec<&Snapshot>, Vec<&Snapshot>) =
        backups.iter().partition(|s| s.trigger == "scheduled");
    scheduled
        .into_iter()
        .skip(scheduled_keep.max(1) as usize)
        .chain(other.into_iter().skip(other_keep.max(1) as usize))
        .collect()
}

/// Delete a snapshot's local directory, its remote archive (best effort) and
/// its row.
pub async fn delete_snapshot_files_and_row(state: &AppState, snap: &Snapshot) {
    if let Some(ref path) = snap.local_path {
        let _ = tokio::fs::remove_dir_all(path).await;
    }
    if let (Some(key), Some(sp_id)) = (&snap.remote_key, &snap.storage_provider_id)
        && let Ok(Some(provider)) = sqlx::query_as::<_, tond_common::StorageProvider>(
            "SELECT * FROM storage_providers WHERE id = ?",
        )
        .bind(sp_id)
        .fetch_optional(&state.db)
        .await
        && let Ok(provider) = decrypt_provider_secret(provider)
    {
        // The manifest first: an archive without one is no longer listed.
        let _ = storage::delete(&provider, &snapshot_remote::manifest_key(&snap.id)).await;
        if let Err(e) = storage::delete(&provider, key).await {
            tracing::warn!(
                "Couldn't delete the remote copy of snapshot {}: {e:#}",
                snap.id
            );
        }
    }
    let _ = sqlx::query("DELETE FROM snapshots WHERE id = ?")
        .bind(&snap.id)
        .execute(&state.db)
        .await;
}

// ── Fetch + ensure local helpers ──────────────────────────────────────────────

pub(crate) async fn fetch_snapshot(state: &AppState, id: &str) -> Result<Snapshot> {
    sqlx::query_as::<_, Snapshot>("SELECT * FROM snapshots WHERE id = ? AND status = 'completed'")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Snapshot '{}' not found or not completed", id))
}

/// Returns the local path of a snapshot, downloading (and verifying) the
/// remote copy if necessary.
pub(crate) async fn ensure_local(state: &AppState, snap: &Snapshot) -> Result<PathBuf> {
    if let Some(ref local) = snap.local_path {
        let p = PathBuf::from(local);
        if p.exists() {
            return Ok(p);
        }
    }
    fetch_remote_snapshot(state, snap).await
}

// ── DB logical dumps (Step 6) ─────────────────────────────────────────────────

// MySQL/MariaDB dump and import scripts. User, database and password reach
// the client through the exec env ("$VAR" references and MYSQL_PWD), so no
// credential is ever interpolated into the shell string. MariaDB 11 images
// ship only the mariadb-* client names.
const MYSQL_DUMP_SCRIPT: &str = r#"if command -v mariadb-dump >/dev/null 2>&1; then d=mariadb-dump; else d=mysqldump; fi; "$d" --single-transaction -u"$TOND_DB_USER" "$TOND_DB_NAME" > /tmp/dump.sql"#;
const MYSQL_IMPORT_SCRIPT: &str = r#"if command -v mariadb >/dev/null 2>&1; then c=mariadb; else c=mysql; fi; "$c" -u"$TOND_DB_USER" "$TOND_DB_NAME" < /tmp/dump.sql"#;

fn mysql_exec_env(user: &str, password: &str, dbname: &str) -> Vec<String> {
    vec![
        format!("MYSQL_PWD={password}"),
        format!("TOND_DB_USER={user}"),
        format!("TOND_DB_NAME={dbname}"),
    ]
}

/// How long a dump or an import may run.
const DUMP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// The dump command of an engine and the file it writes inside the
/// container. Credentials go in the exec env, never into a shell string.
fn dump_command(db: &ManagedDatabase) -> Option<(db_exec::Cmd, &'static str)> {
    let kind = db.db_type.as_str();
    Some(match kind {
        "redis" | "keydb" => {
            // Managed Redis requires ACL auth (see handlers/databases.rs).
            // SAVE writes a consistent dump.rdb, which is then copied out.
            // REDISCLI_AUTH passes the password without exposing it in `ps`.
            let cli = if kind == "keydb" {
                "keydb-cli"
            } else {
                "redis-cli"
            };
            let user = db.db_user.as_deref().unwrap_or("default");
            (
                db_exec::Cmd::new(&[cli, "--user", user, "--no-auth-warning", "SAVE"])
                    .env("REDISCLI_AUTH", db.db_password.as_deref().unwrap_or("")),
                "/data/dump.rdb",
            )
        }
        "postgres" => {
            let user = db.db_user.as_deref().unwrap_or("postgres");
            let dbname = db.db_name.as_deref().unwrap_or("postgres");
            (
                db_exec::Cmd::new(&[
                    "pg_dump",
                    "-U",
                    user,
                    "-F",
                    "c",
                    "-f",
                    "/tmp/dump.archive",
                    dbname,
                ]),
                "/tmp/dump.archive",
            )
        }
        "mysql" | "mariadb" => {
            let user = db.db_user.as_deref().unwrap_or("root");
            let pass = db.db_password.as_deref().unwrap_or("");
            let dbname = db.db_name.as_deref().unwrap_or("mysql");
            let mut cmd = db_exec::Cmd::sh(MYSQL_DUMP_SCRIPT);
            cmd.env = mysql_exec_env(user, pass, dbname);
            (cmd, "/tmp/dump.sql")
        }
        "mongodb" => {
            let user = db.db_user.as_deref().unwrap_or("");
            let pass = db.db_password.as_deref().unwrap_or("");
            (
                db_exec::Cmd::new(&[
                    "mongodump",
                    "--archive=/tmp/dump.archive",
                    &format!("--username={user}"),
                    &format!("--password={pass}"),
                    "--authenticationDatabase=admin",
                ]),
                "/tmp/dump.archive",
            )
        }
        _ => return None,
    })
}

/// The file name a dump is stored under in a snapshot.
fn dump_file_name(out_file: &str) -> &'static str {
    if out_file.ends_with(".archive") {
        "dump.archive"
    } else if out_file.ends_with(".rdb") {
        "dump.rdb"
    } else {
        "dump.sql"
    }
}

/// Write a logical dump of `db` into the directory `dest` (dump.archive,
/// dump.sql or dump.rdb). Fails when the engine's dump command fails.
pub(crate) async fn dump_database(
    docker: &bollard::Docker,
    db: &ManagedDatabase,
    dest: &Path,
) -> Result<()> {
    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let lock = dump_lock(&db.name);
    let _guard = lock.lock().await;
    let Some((cmd, out_file)) = dump_command(db) else {
        tracing::debug!(
            "DB kind '{}': raw volume copy only, no logical dump",
            db.db_type
        );
        return Ok(());
    };
    db_exec::run(
        docker,
        container_id,
        &cmd,
        DUMP_TIMEOUT,
        &db_exec::secrets_of(db),
    )
    .await
    .context("the dump failed")?;

    let dest_file = dest.join(dump_file_name(out_file));
    // Stream the dump out of the container (Docker sends a tar holding the
    // one file) to disk, then unpack that file: constant memory for any size.
    let tar_path = dest.join(".dump.tar");
    let result = async {
        download_from_container_to_file(docker, container_id, out_file, &tar_path).await?;
        let tar_file = tar_path.clone();
        tokio::task::spawn_blocking(move || extract_single_file(&tar_file, &dest_file))
            .await
            .context("dump extraction task failed")?
    }
    .await;
    let _ = tokio::fs::remove_file(&tar_path).await;
    if !out_file.starts_with("/data/") {
        // Don't leave a copy of the data in the container's /tmp.
        let _ = db_exec::run(
            docker,
            container_id,
            &db_exec::Cmd::new(&["rm", "-f", out_file]),
            std::time::Duration::from_secs(30),
            &[],
        )
        .await;
    }
    result
}
/// Stream `path` from a container (a tar archive) into the file `dest`.
async fn download_from_container_to_file(
    docker: &bollard::Docker,
    container_id: &str,
    path: &str,
    dest: &Path,
) -> Result<()> {
    use futures::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;

    let mut stream = docker.download_from_container(
        container_id,
        Some(
            bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
                .path(path)
                .build(),
        ),
    );
    let mut file = tokio::fs::File::create(dest).await?;
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk.context("failed to read the dump from the container")?)
            .await?;
    }
    file.flush().await?;
    Ok(())
}

/// Copy the first regular file of the tar archive at `tar_path` to `dest`.
fn extract_single_file(tar_path: &Path, dest: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(std::fs::File::open(tar_path)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file() {
            let mut out = std::fs::File::create(dest)?;
            io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    anyhow::bail!("the container returned no dump file")
}

/// Upload the local file `src` into a container as `<dir>/<name>`, streaming
/// the tar Docker expects instead of building it in memory.
async fn upload_file_to_container(
    docker: &bollard::Docker,
    container_id: &str,
    dir: &str,
    name: &str,
    src: &Path,
) -> Result<()> {
    use axum::body::Bytes;

    let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, io::Error>>(4);
    let src = src.to_path_buf();
    let name = name.to_string();
    let producer = tokio::task::spawn_blocking(move || {
        let errors = tx.clone();
        let result = (|| -> io::Result<()> {
            let file = std::fs::File::open(&src)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(file.metadata()?.len());
            header.set_mode(0o644);
            header.set_cksum();
            let mut ar = tar::Builder::new(ChunkWriter::new(tx));
            ar.append_data(&mut header, &name, file)?;
            ar.into_inner()?.finish()
        })();
        if let Err(e) = &result {
            // Fail the request body instead of sending a truncated tar.
            let _ = errors.blocking_send(Err(io::Error::other(e.to_string())));
        }
        result
    });

    let body = bollard::body_try_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    let uploaded = docker
        .upload_to_container(
            container_id,
            Some(
                bollard::query_parameters::UploadToContainerOptionsBuilder::default()
                    .path(dir)
                    .build(),
            ),
            body,
        )
        .await;
    producer.await.context("dump upload task failed")??;
    uploaded?;
    Ok(())
}

/// A `Write` sink that forwards 256 KiB chunks to an async request body.
pub(crate) struct ChunkWriter {
    tx: mpsc::Sender<std::result::Result<axum::body::Bytes, io::Error>>,
    buf: Vec<u8>,
}

impl ChunkWriter {
    const CHUNK: usize = 256 * 1024;

    pub(crate) fn new(tx: mpsc::Sender<std::result::Result<axum::body::Bytes, io::Error>>) -> Self {
        Self {
            tx,
            buf: Vec::with_capacity(Self::CHUNK),
        }
    }

    fn send_buf(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(Self::CHUNK));
        self.tx
            .blocking_send(Ok(chunk.into()))
            .map_err(|_| io::Error::other("the container upload stopped"))
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.send_buf()
    }
}

impl Write for ChunkWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = (Self::CHUNK - self.buf.len()).min(data.len());
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == Self::CHUNK {
            self.send_buf()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Replace the data of `db` with the dump in `dump_dir` (in place: the
/// database keeps its name and credentials). Postgres drops what the dump
/// recreates, MongoDB drops the collections it restores.
async fn import_database_dump(
    docker: &bollard::Docker,
    db: &ManagedDatabase,
    dump_dir: &Path,
) -> Result<()> {
    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;
    let user = db.db_user.as_deref().unwrap_or("postgres");
    let dbname = db.db_name.as_deref().unwrap_or("postgres");
    let (file, cmd) = match db.db_type.as_str() {
        "postgres" => (
            "dump.archive",
            db_exec::Cmd::new(&[
                "pg_restore",
                "-U",
                user,
                "-d",
                dbname,
                "--clean",
                "--if-exists",
                "/tmp/dump.archive",
            ]),
        ),
        "mongodb" => (
            "dump.archive",
            db_exec::Cmd::new(&[
                "mongorestore",
                "--archive=/tmp/dump.archive",
                &format!("--username={user}"),
                &format!("--password={}", db.db_password.as_deref().unwrap_or("")),
                "--authenticationDatabase=admin",
                "--drop",
            ]),
        ),
        "mysql" | "mariadb" => {
            let mut cmd = db_exec::Cmd::sh(MYSQL_IMPORT_SCRIPT);
            cmd.env = mysql_exec_env(
                db.db_user.as_deref().unwrap_or("root"),
                db.db_password.as_deref().unwrap_or(""),
                db.db_name.as_deref().unwrap_or("mysql"),
            );
            ("dump.sql", cmd)
        }
        _ => return Ok(()),
    };
    run_import(docker, container_id, db, dump_dir, file, &cmd).await
}

/// Load the dump in `dump_dir` into `target`, a freshly created database
/// (a restore into a new database, or the new version during an upgrade).
/// `source_db_name` is the engine database name the dump came from (MongoDB
/// renames its namespace to the target's). Redis-like engines don't load
/// here: their dump.rdb is placed in the data directory before the start
/// (`place_redis_dump`).
pub(crate) async fn load_dump_into(
    docker: &bollard::Docker,
    target: &ManagedDatabase,
    container_id: &str,
    dump_dir: &Path,
    source_db_name: Option<&str>,
) -> Result<()> {
    let user = target.db_user.as_deref().unwrap_or("postgres");
    let dbname = target.db_name.as_deref().unwrap_or("postgres");
    let (file, cmd) = match target.db_type.as_str() {
        // No owner or grants from the dump: everything belongs to the new
        // database's own user. Stops at the first error.
        "postgres" => (
            "dump.archive",
            db_exec::Cmd::new(&[
                "pg_restore",
                "-U",
                user,
                "-d",
                dbname,
                "--no-owner",
                "--no-acl",
                "--exit-on-error",
                "/tmp/dump.archive",
            ]),
        ),
        // As root: views and routines carry DEFINER clauses naming the
        // source's user.
        "mysql" | "mariadb" => {
            let root_pw = target
                .root_password
                .as_deref()
                .or(target.db_password.as_deref())
                .unwrap_or("");
            let mut cmd = db_exec::Cmd::sh(MYSQL_IMPORT_SCRIPT);
            cmd.env = mysql_exec_env("root", root_pw, dbname);
            ("dump.sql", cmd)
        }
        "mongodb" => {
            let mut argv = vec![
                "mongorestore".to_string(),
                "--archive=/tmp/dump.archive".to_string(),
                format!("--username={user}"),
                format!("--password={}", target.db_password.as_deref().unwrap_or("")),
                "--authenticationDatabase=admin".to_string(),
                "--drop".to_string(),
            ];
            if let Some(src) = source_db_name.filter(|s| *s != dbname) {
                argv.push(format!("--nsFrom={src}.*"));
                argv.push(format!("--nsTo={dbname}.*"));
            }
            (
                "dump.archive",
                db_exec::Cmd {
                    argv,
                    env: Vec::new(),
                    user: None,
                },
            )
        }
        "redis" | "keydb" => return Ok(()),
        other => anyhow::bail!("Tond can't load {other} dumps"),
    };
    run_import(docker, container_id, target, dump_dir, file, &cmd).await
}

/// Upload `dump_dir/file` into the container's /tmp and run `cmd`.
async fn run_import(
    docker: &bollard::Docker,
    container_id: &str,
    db: &ManagedDatabase,
    dump_dir: &Path,
    file: &str,
    cmd: &db_exec::Cmd,
) -> Result<()> {
    let dump = dump_dir.join(file);
    anyhow::ensure!(
        dump.exists(),
        "the backup has no {file} for this database (it was taken while the database was stopped); \
         restore it in place instead"
    );
    upload_file_to_container(docker, container_id, "/tmp", file, &dump).await?;
    let result = db_exec::run(
        docker,
        container_id,
        cmd,
        DUMP_TIMEOUT,
        &db_exec::secrets_of(db),
    )
    .await
    .context("loading the dump failed");
    let _ = db_exec::run(
        docker,
        container_id,
        &db_exec::Cmd::new(&["rm", "-f", &format!("/tmp/{file}")]),
        std::time::Duration::from_secs(30),
        &[],
    )
    .await;
    result.map(|_| ())
}

/// Copy a Redis-like dump (dump.rdb from the logical dump, else the one in
/// the raw volume copy) into a data directory that isn't in use yet.
pub(crate) async fn place_redis_dump(dump_dir: &Path, data_dir: &Path) -> Result<()> {
    let candidates = [
        dump_dir.join("dump.rdb"),
        dump_dir.join("volume").join("dump.rdb"),
    ];
    let src = candidates
        .iter()
        .find(|p| p.exists())
        .context("the backup holds no dump.rdb for this database")?;
    tokio::fs::create_dir_all(data_dir).await?;
    tokio::fs::copy(src, data_dir.join("dump.rdb")).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::MetadataExt};

    use filetime::{FileTime, set_file_mtime};
    use tempfile::TempDir;

    use std::collections::HashMap;

    use super::*;

    fn temp_dir() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"))
    }

    #[test]
    fn mysql_dump_and_import_keep_credentials_out_of_the_shell_string() {
        let password = "p'w\"$(touch /tmp/pwned)`id`;";
        let env = mysql_exec_env("app", password, "shop");

        assert_eq!(env[0], format!("MYSQL_PWD={password}"));
        for script in [MYSQL_DUMP_SCRIPT, MYSQL_IMPORT_SCRIPT] {
            assert!(
                !script.contains("-p"),
                "the password must not be passed on the command line: {script}"
            );
            assert!(script.contains("\"$TOND_DB_USER\"") && script.contains("\"$TOND_DB_NAME\""));
        }
    }

    #[test]
    fn snapshot_trigger_helpers_report_expected_metadata() {
        let manual_named = SnapshotTrigger::Manual {
            name: Some("Release backup".to_string()),
        };
        assert_eq!(
            manual_named.trigger_str(),
            "manual",
            "manual trigger should report its trigger string"
        );
        assert_eq!(
            manual_named.scope(),
            "full",
            "manual snapshots should default to full scope"
        );
        assert_eq!(
            manual_named.name(),
            "Release backup",
            "manual named snapshots should preserve the provided label"
        );

        let predeploy = SnapshotTrigger::PreDeploy {
            app_name: "api".to_string(),
            deployment_id: "deploy-1".to_string(),
        };
        assert_eq!(
            predeploy.trigger_str(),
            "pre_deploy",
            "pre-deploy trigger should report its trigger string"
        );
        assert_eq!(
            predeploy.scope(),
            "app:api",
            "pre-deploy snapshots should scope to the target app"
        );
        assert_eq!(
            predeploy.trigger_app(),
            Some("api"),
            "pre-deploy snapshots should expose their trigger app"
        );
        assert_eq!(
            predeploy.deployment_id(),
            Some("deploy-1"),
            "pre-deploy snapshots should expose their deployment ID"
        );

        let scheduled = SnapshotTrigger::Scheduled;
        assert_eq!(
            scheduled.trigger_str(),
            "scheduled",
            "scheduled trigger should report its trigger string"
        );
        assert_eq!(
            scheduled.scope(),
            "full",
            "scheduled snapshots should be full scope"
        );
    }

    #[test]
    fn path_helpers_use_environment_overrides() {
        let old_data = std::env::var("TOND_DATA_DIR").ok();
        let old_db = std::env::var("TOND_DB_PATH").ok();
        let temp = temp_dir();
        let data_root = temp.path().join("data-root");
        let db_file = data_root.join("tond.db");
        unsafe {
            std::env::set_var("TOND_DATA_DIR", &data_root);
            std::env::set_var("TOND_DB_PATH", &db_file);
        }

        assert_eq!(
            data_dir(),
            data_root.to_string_lossy(),
            "data_dir should honor TOND_DATA_DIR overrides"
        );
        assert_eq!(
            db_path(),
            db_file.to_string_lossy(),
            "db_path should honor TOND_DB_PATH overrides"
        );
        assert_eq!(
            snapshots_dir(),
            data_root.join("backups").join("snapshots"),
            "snapshots_dir should derive from the data dir"
        );
        assert_eq!(
            snapshot_dir("snap-1"),
            data_root.join("backups").join("snapshots").join("snap-1"),
            "snapshot_dir should join the snapshot id under snapshots_dir"
        );

        if let Some(value) = old_data {
            unsafe {
                std::env::set_var("TOND_DATA_DIR", value);
            }
        } else {
            unsafe {
                std::env::remove_var("TOND_DATA_DIR");
            }
        }
        if let Some(value) = old_db {
            unsafe {
                std::env::set_var("TOND_DB_PATH", value);
            }
        } else {
            unsafe {
                std::env::remove_var("TOND_DB_PATH");
            }
        }
    }

    #[test]
    fn snapshot_copies_skip_symlinks_out_of_the_volume() {
        let temp = temp_dir();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        let secret = temp.path().join("host-secret.env");
        fs::create_dir_all(&src).unwrap_or_else(|err| panic!("mkdir: {err}"));
        fs::write(&secret, b"TOND_SECRET_KEY=hunter2").unwrap_or_else(|err| panic!("write: {err}"));
        fs::write(src.join("ok.txt"), b"ok").unwrap_or_else(|err| panic!("write: {err}"));
        std::os::unix::fs::symlink(&secret, src.join("leak"))
            .unwrap_or_else(|err| panic!("symlink: {err}"));

        snapshot_files::encrypt_tree(&src, &dest, None)
            .unwrap_or_else(|err| panic!("copy should succeed: {err}"));

        assert_eq!(
            snapshot_files::read_encrypted(&dest.join("ok.txt")).ok(),
            Some(b"ok".to_vec())
        );
        assert!(
            fs::symlink_metadata(dest.join("leak")).is_err(),
            "a symlink in a volume must not copy the host file it points at"
        );
    }

    #[test]
    fn snapshot_copies_keep_mtime_so_the_next_snapshot_dedupes() {
        let temp = temp_dir();
        let src = temp.path().join("src");
        fs::create_dir_all(&src).unwrap_or_else(|err| panic!("mkdir: {err}"));
        fs::write(src.join("big.bin"), vec![1u8; 4096]).unwrap_or_else(|err| panic!("{err}"));
        set_file_mtime(
            src.join("big.bin"),
            FileTime::from_unix_time(1_700_000_000, 0),
        )
        .unwrap_or_else(|err| panic!("mtime: {err}"));

        let first = temp.path().join("first");
        let (d1, _) = snapshot_files::encrypt_tree(&src, &first, None)
            .unwrap_or_else(|err| panic!("first copy: {err}"));
        let second = temp.path().join("second");
        let (d2, t2) = snapshot_files::encrypt_tree(&src, &second, Some(&first))
            .unwrap_or_else(|err| panic!("second copy: {err}"));

        assert_eq!(d1, 4096);
        assert_eq!((d2, t2), (0, 4096), "an unchanged file must be hard-linked");
        let ino = |p: &Path| fs::metadata(p).map(|m| m.ino()).unwrap_or(0);
        assert_eq!(ino(&first.join("big.bin")), ino(&second.join("big.bin")));
    }

    #[test]

    fn part_writer_cuts_parts_and_ends_only_on_finish() {
        let (tx, mut rx) = mpsc::channel::<storage::Part>(16);
        let mut writer = PartWriter::new(tx, 10);
        writer
            .write_all(&[7u8; 25])
            .unwrap_or_else(|err| panic!("write: {err}"));
        writer
            .finish()
            .unwrap_or_else(|err| panic!("finish: {err}"));

        let mut sizes = Vec::new();
        let mut ended = false;
        while let Ok(part) = rx.try_recv() {
            match part {
                storage::Part::Data(d) => sizes.push(d.len()),
                storage::Part::End => ended = true,
            }
        }
        assert_eq!(sizes, vec![10, 10, 5]);
        assert!(ended, "finish must send the end marker");

        // Dropping a writer without finish sends no end marker.
        let (tx, mut rx) = mpsc::channel::<storage::Part>(16);
        drop(PartWriter::new(tx, 10));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn compose_volume_restore_only_touches_the_apps_own_volumes() {
        let snap = temp_dir();
        assert_eq!(
            compose_volume_dir(snap.path(), "shop", "shop_data"),
            snap.path().join("compose-volumes/shop/shop_data")
        );
        // A snapshot naming another project's volume is ignored before any
        // Docker call (nothing listens on this Docker address).
        fs::create_dir_all(snap.path().join("compose-volumes/shop/billing_data/x"))
            .unwrap_or_else(|e| panic!("{e}"));
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let state = AppState {
            db,
            docker: bollard::Docker::connect_with_http(
                "http://127.0.0.1:9",
                1,
                bollard::API_DEFAULT_VERSION,
            )
            .unwrap_or_else(|e| panic!("{e}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(
                tond_common::ServerSettings::default(),
            )),
        };
        restore_compose_volumes(&state, snap.path(), "shop")
            .await
            .unwrap_or_else(|e| panic!("foreign volumes are skipped: {e:#}"));
        assert!(
            restore_compose_volumes(&state, snap.path(), "nothing-here")
                .await
                .is_ok(),
            "no named volumes in the snapshot: nothing to do"
        );
    }

    #[test]
    fn upload_key_for_snapshot_uses_snapshots_prefix() {
        assert_eq!(
            upload_key_for_snapshot("snap-123"),
            "snapshots/snap-123.tondsnap",
            "upload_key_for_snapshot should store archives under the snapshots/ prefix"
        );
    }

    /// A migrated in-memory database; Docker is unreachable on purpose.
    async fn test_state() -> AppState {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        AppState {
            db,
            docker: bollard::Docker::connect_with_http(
                "http://127.0.0.1:9",
                1,
                bollard::API_DEFAULT_VERSION,
            )
            .unwrap_or_else(|e| panic!("{e}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(
                tond_common::ServerSettings::default(),
            )),
        }
    }

    fn at(s: &str) -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap_or_else(|e| panic!("{e}"))
            .with_timezone(&Utc)
    }

    #[test]
    fn the_scheduler_plans_full_and_per_database_runs_in_order() {
        let now = at("2026-03-01T10:30:00Z");
        let jobs = vec![
            (ScheduledJob::Full, "0 3 * * *".to_string()),
            (
                ScheduledJob::Database("shop".into()),
                "0 11 * * *".to_string(),
            ),
            (
                ScheduledJob::Database("cache".into()),
                "*/15 * * * *".to_string(),
            ),
            (
                ScheduledJob::Database("broken".into()),
                "not a cron".to_string(),
            ),
        ];
        // First sight: nothing runs, every job starts from now.
        let plan = plan_due(&jobs, &HashMap::new(), now, now);
        assert!(plan.due.is_empty());
        assert_eq!(plan.init.len(), 3, "an invalid schedule is skipped");
        assert_eq!(plan.next, Some(at("2026-03-01T10:45:00Z")), "soonest first");

        let states: HashMap<String, (String, chrono::DateTime<Utc>)> = jobs[..3]
            .iter()
            .map(|(j, e)| (job_key(j), (e.clone(), now)))
            .collect();
        let plan = plan_due(&jobs, &states, now, now);
        assert!(plan.due.is_empty() && plan.init.is_empty());
        assert_eq!(plan.next, Some(at("2026-03-01T10:45:00Z")));

        // At 10:45 (server up all along) the cache backup is due, not a catch-up.
        let later = at("2026-03-01T10:45:00Z");
        let plan = plan_due(&jobs, &states, later, now);
        assert_eq!(
            plan.due,
            vec![DueJob {
                job: ScheduledJob::Database("cache".into()),
                slot: later,
                catch_up: false,
                missed: 1,
            }]
        );
    }

    #[test]
    fn backups_missed_while_down_run_once_as_a_catch_up() {
        let jobs = vec![
            (ScheduledJob::Full, "0 3 * * *".to_string()),
            (
                ScheduledJob::Database("shop".into()),
                "0 * * * *".to_string(),
            ),
        ];
        // Last ran at 03:00 on the 1st; the server was down until the 4th 09:20.
        let last = at("2026-03-01T03:00:00Z");
        let states: HashMap<String, (String, chrono::DateTime<Utc>)> = jobs
            .iter()
            .map(|(j, e)| (job_key(j), (e.clone(), last)))
            .collect();
        let started = at("2026-03-04T09:20:00Z");
        let plan = plan_due(&jobs, &states, started, started);
        assert_eq!(
            plan.due.len(),
            2,
            "one run per job, not one per missed slot"
        );
        assert_eq!(plan.due[0].job, ScheduledJob::Full);
        assert_eq!(plan.due[0].slot, at("2026-03-04T03:00:00Z"));
        assert!(plan.due[0].catch_up);
        assert_eq!(plan.due[0].missed, 3);
        assert_eq!(plan.due[1].slot, at("2026-03-04T09:00:00Z"));
        assert_eq!(plan.due[1].missed, 78);

        // A changed schedule starts over instead of catching up.
        let mut changed = states.clone();
        changed.insert("full".into(), ("0 4 * * *".into(), last));
        let plan = plan_due(&jobs, &changed, started, started);
        assert_eq!(
            plan.init,
            vec![("full".to_string(), "0 3 * * *".to_string())]
        );
        assert!(plan.due.iter().all(|d| d.job != ScheduledJob::Full));
    }

    #[tokio::test]
    async fn the_scheduler_state_forgets_schedules_that_were_turned_off() {
        let state = test_state().await;
        let now = at("2026-03-01T10:00:00Z");
        set_last_slot(&state, "full", "0 3 * * *", now)
            .await
            .unwrap_or_else(|e| panic!("set: {e:#}"));
        set_last_slot(&state, "database:gone", "0 * * * *", now)
            .await
            .unwrap_or_else(|e| panic!("set: {e:#}"));
        let jobs = vec![(ScheduledJob::Full, "0 3 * * *".to_string())];
        let states = schedule_states(&state, &jobs)
            .await
            .unwrap_or_else(|e| panic!("states: {e:#}"));
        assert_eq!(states.len(), 1);
        assert_eq!(states.get("full").map(|(_, t)| *t), Some(now));
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_schedule_state")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
        assert_eq!(left, 1);
    }

    fn snap(id: &str, trigger: &str) -> Snapshot {
        Snapshot {
            id: id.to_string(),
            name: id.to_string(),
            status: "completed".to_string(),
            trigger: trigger.to_string(),
            scope: "database:shop".to_string(),
            trigger_app: None,
            deployment_id: None,
            parent_snapshot_id: None,
            created_at: String::new(),
            completed_at: None,
            delta_bytes: None,
            total_bytes: None,
            local_path: None,
            remote_key: None,
            storage_provider_id: None,
            checksum_sha256: None,
            error_message: None,
            notes: None,
            remote_status: None,
            remote_error: None,
            archive_bytes: None,
        }
    }

    #[test]
    fn database_retention_counts_scheduled_backups_separately() {
        // Newest first.
        let backups = vec![
            snap("s5", "scheduled"),
            snap("m2", "manual"),
            snap("s4", "scheduled"),
            snap("s3", "scheduled"),
            snap("m1", "manual"),
            snap("u1", "pre_upgrade"),
        ];
        let ids = |v: Vec<&Snapshot>| v.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(retention_victims(&backups, 2, 2)), vec!["s3", "u1"]);
        assert_eq!(ids(retention_victims(&backups, 3, 1)), vec!["m1", "u1"]);
        // A retention of zero still keeps the newest.
        assert_eq!(ids(retention_victims(&backups, 0, 10)), vec!["s4", "s3"]);
    }

    #[test]
    fn full_and_pre_deploy_retention_never_delete_the_newest_snapshot() {
        let snaps = vec![
            snap("n3", "manual"),
            snap("n2", "manual"),
            snap("n1", "manual"),
        ];
        let ids = |keep: i64| {
            beyond_retention(&snaps, keep)
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(2), vec!["n1"]);
        assert_eq!(ids(5), Vec::<String>::new());
        // Zero or negative (stored before validation) behaves like one.
        assert_eq!(ids(0), vec!["n2", "n1"]);
        assert_eq!(ids(-3), vec!["n2", "n1"]);
    }

    #[test]
    fn database_scoped_triggers_report_their_scope_and_kind() {
        let scheduled = SnapshotTrigger::ScheduledDatabase {
            db_name: "shop".to_string(),
        };
        assert_eq!(scheduled.trigger_str(), "scheduled");
        assert_eq!(scheduled.scope(), "database:shop");
        let pre = SnapshotTrigger::PreUpgradeDatabase {
            db_name: "shop".to_string(),
        };
        assert_eq!(pre.trigger_str(), "pre_upgrade");
        assert_eq!(pre.scope(), "database:shop");
        assert!(pre.name().starts_with("pre-upgrade shop "));
    }

    #[test]
    fn database_meta_round_trips_what_a_new_database_needs() {
        let mut db = ManagedDatabase::new("shop".to_string(), "postgres".to_string(), None);
        db.variant = "pgvector".to_string();
        db.memory_limit = Some(256);
        let meta = DatabaseMeta::of(&db);
        assert_eq!(
            meta.version.as_deref(),
            Some("16"),
            "the effective version is recorded"
        );
        let json = serde_json::to_string(&meta).unwrap_or_default();
        assert!(!json.contains("password"), "{json}");
        let back: DatabaseMeta = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, meta);
    }

    #[tokio::test]
    async fn redis_dumps_are_placed_from_the_dump_or_the_raw_copy() {
        let dir = temp_dir();
        let dump_dir = dir.path().join("backup");
        fs::create_dir_all(dump_dir.join("volume")).unwrap_or_else(|e| panic!("{e}"));
        fs::write(dump_dir.join("volume").join("dump.rdb"), b"raw")
            .unwrap_or_else(|e| panic!("{e}"));
        let data = dir.path().join("data");
        place_redis_dump(&dump_dir, &data)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(fs::read(data.join("dump.rdb")).unwrap_or_default(), b"raw");

        fs::write(dump_dir.join("dump.rdb"), b"logical").unwrap_or_else(|e| panic!("{e}"));
        place_redis_dump(&dump_dir, &data)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            fs::read(data.join("dump.rdb")).unwrap_or_default(),
            b"logical"
        );

        let empty = dir.path().join("empty");
        assert!(place_redis_dump(&empty, &data).await.is_err());
    }

    // ── Streaming upload / restore against a fake S3 ─────────────────────────

    mod fake_s3 {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        use axum::{
            Router,
            body::Bytes,
            extract::{Path, Query, State},
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::any,
        };

        #[derive(Clone, Default)]
        pub struct Store {
            pub objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
            pub parts: Arc<Mutex<Vec<Vec<u8>>>>,
            pub max_part: Arc<Mutex<usize>>,
            pub aborted: Arc<Mutex<bool>>,
        }

        async fn handle(
            State(store): State<Store>,
            method: axum::http::Method,
            Path((_bucket, key)): Path<(String, String)>,
            Query(q): Query<HashMap<String, String>>,
            body: Bytes,
        ) -> impl IntoResponse {
            let mut headers = HeaderMap::new();
            match method.as_str() {
                "POST" if q.contains_key("uploads") => (
                    StatusCode::OK,
                    headers,
                    b"<InitiateMultipartUploadResult><UploadId>up-1</UploadId></InitiateMultipartUploadResult>"
                        .to_vec(),
                ),
                "PUT" if q.contains_key("partNumber") => {
                    let mut max = store.max_part.lock().unwrap_or_else(|e| e.into_inner());
                    *max = (*max).max(body.len());
                    store
                        .parts
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(body.to_vec());
                    headers.insert(
                        "etag",
                        format!("\"etag-{}\"", q["partNumber"])
                            .parse()
                            .unwrap_or_else(|_| panic!("etag header")),
                    );
                    (StatusCode::OK, headers, Vec::new())
                }
                "POST" if q.contains_key("uploadId") => {
                    let parts = std::mem::take(
                        &mut *store.parts.lock().unwrap_or_else(|e| e.into_inner()),
                    );
                    store
                        .objects
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, parts.concat());
                    (
                        StatusCode::OK,
                        headers,
                        b"<CompleteMultipartUploadResult/>".to_vec(),
                    )
                }
                "PUT" => {
                    store
                        .objects
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, body.to_vec());
                    (StatusCode::OK, headers, Vec::new())
                }
                "DELETE" if !q.contains_key("uploadId") => {
                    store
                        .objects
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&key);
                    (StatusCode::NO_CONTENT, headers, Vec::new())
                }
                "DELETE" if q.contains_key("uploadId") => {
                    *store.aborted.lock().unwrap_or_else(|e| e.into_inner()) = true;
                    (StatusCode::NO_CONTENT, headers, Vec::new())
                }
                "GET" => match store
                    .objects
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&key)
                {
                    Some(bytes) => (StatusCode::OK, headers, bytes.clone()),
                    None => (StatusCode::NOT_FOUND, headers, b"<Error/>".to_vec()),
                },
                _ => (StatusCode::BAD_REQUEST, headers, Vec::new()),
            }
        }

        /// ListObjectsV2 on one line, like AWS answers.
        async fn list(
            State(store): State<Store>,
            Query(q): Query<HashMap<String, String>>,
        ) -> impl IntoResponse {
            let prefix = q.get("prefix").cloned().unwrap_or_default();
            let objects = store.objects.lock().unwrap_or_else(|e| e.into_inner());
            let mut keys: Vec<&String> =
                objects.keys().filter(|k| k.starts_with(&prefix)).collect();
            keys.sort();
            let contents: String = keys
                .iter()
                .map(|k| {
                    format!(
                        "<Contents><Key>{k}</Key><Size>{}</Size></Contents>",
                        objects[*k].len()
                    )
                })
                .collect();
            format!(
                "<ListBucketResult><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
            )
        }

        /// Serve the fake bucket; returns its base URL.
        pub async fn serve(store: Store) -> String {
            let app = Router::new()
                .route("/{bucket}", axum::routing::get(list))
                .route("/{bucket}/{*key}", any(handle))
                .layer(axum::extract::DefaultBodyLimit::disable())
                .with_state(store);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("bind: {e}"));
            let addr = listener
                .local_addr()
                .unwrap_or_else(|e| panic!("addr: {e}"));
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            format!("http://{addr}")
        }
    }

    fn fake_provider(endpoint: &str) -> tond_common::StorageProvider {
        tond_common::StorageProvider {
            id: "sp-1".to_string(),
            name: "fake".to_string(),
            kind: "s3".to_string(),
            config: serde_json::json!({
                "endpoint": endpoint,
                "region": "auto",
                "bucket": "backups",
                "prefix": "tond/",
                "access_key_id": "AKIAFAKE",
                "secret_access_key": "fake-secret",
            })
            .to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn archives_stream_to_multipart_upload_and_restore_only_when_authentic() {
        let store = fake_s3::Store::default();
        let endpoint = fake_s3::serve(store.clone()).await;
        let provider = fake_provider(&endpoint);

        // A snapshot directory larger than one part, incompressible.
        let temp = temp_dir();
        let plain = temp.path().join("plain");
        let staging = temp.path().join("snap");
        fs::create_dir_all(plain.join("volumes/web")).unwrap_or_else(|e| panic!("{e}"));
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let data: Vec<u8> = (0..(storage::PART_SIZE + 3 * 1024 * 1024))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        fs::write(plain.join("volumes/web/blob.bin"), &data).unwrap_or_else(|e| panic!("{e}"));
        fs::write(plain.join("manifest.json"), b"{}").unwrap_or_else(|e| panic!("{e}"));
        // A local snapshot: every file encrypted at rest.
        snapshot_files::encrypt_tree(&plain, &staging, None)
            .unwrap_or_else(|e| panic!("local copy: {e:#}"));
        assert!(
            fs::read(staging.join("volumes/web/blob.bin"))
                .unwrap_or_default()
                .starts_with(snapshot_archive::LOCAL_MAGIC)
        );

        let (tx, rx) = mpsc::channel::<storage::Part>(1);
        let src = staging.clone();
        let writer = tokio::task::spawn_blocking(move || {
            let sink = PartWriter::new(tx, storage::PART_SIZE);
            let (summary, sink) = archive_snapshot_dir(&src, sink, "snap-1")?;
            sink.finish()?;
            Ok::<_, anyhow::Error>(summary)
        });
        let key = upload_key_for_snapshot("snap-1");
        let uploaded = storage::upload_stream(&provider, &key, rx).await;
        let summary = writer
            .await
            .unwrap_or_else(|e| panic!("join: {e}"))
            .unwrap_or_else(|e| panic!("archive: {e:#}"));
        uploaded.unwrap_or_else(|e| panic!("upload: {e:#}"));

        assert!(
            *store.max_part.lock().unwrap_or_else(|e| e.into_inner()) <= storage::PART_SIZE,
            "no part may exceed PART_SIZE (constant memory)"
        );
        let stored = store
            .objects
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("tond/snapshots/snap-1.tondsnap")
            .cloned()
            .unwrap_or_else(|| panic!("object stored under the provider prefix"));
        assert_eq!(stored.len() as u64, summary.bytes);
        assert!(
            !stored.windows(4096).any(|w| w == &data[..4096]),
            "remote copy must be encrypted"
        );

        // Restore path: stream the object back and check it before extracting.
        let download = temp.path().join("dl");
        let (bytes, sha256) = storage::download_to_file(&provider, &key, &download)
            .await
            .unwrap_or_else(|e| panic!("download: {e:#}"));
        assert_eq!(
            (bytes, sha256.as_str()),
            (summary.bytes, summary.sha256.as_str())
        );
        let restored = temp.path().join("restored");
        unpack_verified(&download, &restored, "snap-1")
            .unwrap_or_else(|e| panic!("restore: {e:#}"));
        assert_eq!(
            snapshot_files::read_encrypted(&restored.join("volumes/web/blob.bin"))
                .ok()
                .as_deref(),
            Some(data.as_slice()),
            "the fetched copy is a local snapshot again, encrypted at rest"
        );

        // Tampered or wrong-snapshot archives restore nothing.
        let mut tampered = stored.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x80;
        fs::write(&download, &tampered).unwrap_or_else(|e| panic!("{e}"));
        let target = temp.path().join("tampered");
        assert!(unpack_verified(&download, &target, "snap-1").is_err());
        assert!(
            !target.exists(),
            "nothing may be extracted from a tampered archive"
        );
        fs::write(&download, &stored).unwrap_or_else(|e| panic!("{e}"));
        assert!(unpack_verified(&download, &target, "snap-2").is_err());
        assert!(!target.exists());
    }

    /// A server that never took the snapshot finds it in the bucket, sees
    /// what it holds, imports it and restores from it; a snapshot made with
    /// another TOND_SECRET_KEY is shown as such and refused with a clear
    /// error before anything is downloaded.
    #[tokio::test]
    async fn a_fresh_server_lists_imports_and_fetches_remote_snapshots() {
        let temp = temp_dir();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("TOND_DATA_DIR", temp.path().join("data"));
        }
        let store = fake_s3::Store::default();
        let endpoint = fake_s3::serve(store.clone()).await;
        let provider = fake_provider(&endpoint);
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO storage_providers (id, name, kind, config) VALUES (?, ?, 's3', ?)",
        )
        .bind(&provider.id)
        .bind(&provider.name)
        .bind(&provider.config)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("provider: {e}"));

        // The old server's snapshot: a local copy, archived and uploaded.
        let id = "0b7c1f2e-0000-4000-8000-000000000001";
        let plain = temp.path().join("plain");
        fs::create_dir_all(plain.join("volumes/web")).unwrap_or_else(|e| panic!("{e}"));
        fs::write(plain.join("volumes/web/index.html"), b"hello").unwrap_or_else(|e| panic!("{e}"));
        let local = temp.path().join("old-server-snapshot");
        snapshot_files::encrypt_tree(&plain, &local, None).unwrap_or_else(|e| panic!("{e:#}"));
        let manifest = json!({
            "id": id, "name": "Nightly", "trigger": "scheduled", "scope": "full",
            "created_at": "2026-10-01T03:00:00+00:00", "tond_version": "0.2.0",
            "apps_included": ["web"], "databases_included": [], "total_bytes": 5,
        });
        snapshot_files::write_encrypted(
            &local.join(MANIFEST_FILE),
            &serde_json::to_vec(&manifest).unwrap_or_default(),
        )
        .unwrap_or_else(|e| panic!("{e:#}"));
        let (summary, archive) = archive_snapshot_dir(&local, Vec::new(), id)
            .unwrap_or_else(|e| panic!("archive: {e:#}"));
        let remote = snapshot_remote::remote_manifest(
            &local,
            id,
            &snapshot_remote::archive_key(id),
            &summary,
        )
        .await
        .unwrap_or_else(|e| panic!("manifest: {e:#}"));
        // Another server's snapshot, sealed with its own key.
        let foreign_id = "0b7c1f2e-0000-4000-8000-000000000002";
        let mut foreign = remote.clone();
        foreign.id = foreign_id.to_string();
        foreign.archive_key = snapshot_remote::archive_key(foreign_id);
        foreign.sealed = secret_key::encrypt_with(
            &secret_key::Key::derive("the-other-servers-key"),
            secret_key::Purpose::SnapshotManifest,
            "{}",
        )
        .unwrap_or_default();
        {
            let mut objects = store.objects.lock().unwrap_or_else(|e| e.into_inner());
            objects.insert(
                format!("tond/{}", snapshot_remote::archive_key(id)),
                archive,
            );
            objects.insert(
                format!("tond/{}", snapshot_remote::manifest_key(id)),
                serde_json::to_vec(&remote).unwrap_or_default(),
            );
            objects.insert(
                format!("tond/{}", snapshot_remote::manifest_key(foreign_id)),
                serde_json::to_vec(&foreign).unwrap_or_default(),
            );
            objects.insert("tond/snapshots/stray.txt".to_string(), b"x".to_vec());
        }

        let listed = snapshot_remote::list_remote(&state, Some(provider.id.clone()))
            .await
            .unwrap_or_else(|e| panic!("list: {}", e.0));
        assert_eq!(listed.len(), 2, "{listed:?}");
        let mine = listed
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("listed"));
        assert!(mine.readable && !mine.imported);
        assert_eq!(mine.apps, vec!["web".to_string()]);
        assert_eq!(mine.scope.as_deref(), Some("full"));
        let other = listed
            .iter()
            .find(|s| s.id == foreign_id)
            .unwrap_or_else(|| panic!("listed"));
        assert!(!other.readable && other.name.is_none());

        let refused = snapshot_remote::import_remote(&state, Some(provider.id.clone()), foreign_id)
            .await
            .err()
            .map(|e| e.0.code())
            .unwrap_or_default();
        assert_eq!(refused, tond_common::code::SNAPSHOT_KEY_MISMATCH);

        let row = snapshot_remote::import_remote(&state, Some(provider.id.clone()), id)
            .await
            .unwrap_or_else(|e| panic!("import: {}", e.0));
        assert_eq!(
            (row.scope.as_str(), row.status.as_str()),
            ("full", "completed")
        );
        assert!(row.local_path.is_none() && row.remote_key.is_some());
        let again = snapshot_remote::list_remote(&state, Some(provider.id.clone()))
            .await
            .unwrap_or_else(|e| panic!("list: {}", e.0));
        assert!(again.iter().any(|s| s.id == id && s.imported));

        // A restore fetches it: downloaded, checked, encrypted at rest again.
        let dir = ensure_local(&state, &row)
            .await
            .unwrap_or_else(|e| panic!("fetch: {e:#}"));
        assert_eq!(dir, snapshot_dir(id));
        assert_eq!(
            snapshot_files::read_encrypted(&dir.join("volumes/web/index.html")).ok(),
            Some(b"hello".to_vec())
        );
        assert_eq!(
            read_manifest(&dir).await.ok().map(|m| m["name"].clone()),
            Some(json!("Nightly"))
        );
    }

    /// An exported archive imports into a local snapshot (and twice is once);
    /// another server's archive is refused as a key mismatch.
    #[tokio::test]
    async fn exported_archives_import_as_local_snapshots() {
        let temp = temp_dir();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("TOND_DATA_DIR", temp.path().join("data"));
        }
        let state = test_state().await;
        let id = "0b7c1f2e-0000-4000-8000-000000000003";
        let local = temp.path().join("snap");
        fs::create_dir_all(&local).unwrap_or_else(|e| panic!("{e}"));
        snapshot_files::write_encrypted(
            &local.join(MANIFEST_FILE),
            &serde_json::to_vec(&json!({
                "id": id, "name": "Export", "trigger": "manual", "scope": "full",
                "created_at": "2026-10-01T03:00:00+00:00", "total_bytes": 0,
            }))
            .unwrap_or_default(),
        )
        .unwrap_or_else(|e| panic!("{e:#}"));
        let file = temp.path().join("export.tondsnap");
        let out = fs::File::create(&file).unwrap_or_else(|e| panic!("{e}"));
        archive_snapshot_dir(&local, out, id).unwrap_or_else(|e| panic!("{e:#}"));

        let row = snapshot_remote::import_archive_file(&state, &file)
            .await
            .unwrap_or_else(|e| panic!("import: {}", e.0));
        assert_eq!((row.id.as_str(), row.name.as_str()), (id, "Export"));
        assert_eq!(
            row.local_path.as_deref().map(PathBuf::from),
            Some(snapshot_dir(id))
        );
        let twice = snapshot_remote::import_archive_file(&state, &file)
            .await
            .unwrap_or_else(|e| panic!("again: {}", e.0));
        assert_eq!(twice.id, id);

        let foreign = temp.path().join("foreign.tondsnap");
        let out = fs::File::create(&foreign).unwrap_or_else(|e| panic!("{e}"));
        snapshot_archive::write_archive(
            &temp.path().join("data"),
            out,
            "the-other-servers-key",
            snapshot_files::archive_kind(),
            "0b7c1f2e-0000-4000-8000-000000000004",
            snapshot_archive::Source::Plain,
        )
        .unwrap_or_else(|e| panic!("{e:#}"));
        let refused = snapshot_remote::import_archive_file(&state, &foreign)
            .await
            .err()
            .map(|e| e.0.code())
            .unwrap_or_default();
        assert_eq!(refused, tond_common::code::SNAPSHOT_KEY_MISMATCH);
    }

    #[test]
    fn only_certificates_valid_for_another_day_come_back() {
        let manifest = json!({ "certificates": [
            { "dir": "example.com", "expires_at": "2026-12-01T00:00:00Z" },
            { "dir": "_wildcard.apps.example.com", "expires_at": "2026-12-01 00:00:00" },
            { "dir": "soon.example.com", "expires_at": "2026-10-01T12:00:00Z" },
            { "dir": "../escape", "expires_at": "2027-01-01T00:00:00Z" },
            { "dir": "broken.example.com", "expires_at": "not a date" },
        ]});
        assert_eq!(
            valid_certificate_dirs(&manifest, at("2026-10-01T00:00:00Z")),
            vec!["example.com", "_wildcard.apps.example.com"]
        );
        assert!(valid_certificate_dirs(&json!({}), at("2026-10-01T00:00:00Z")).is_empty());
    }

    /// Verify, then extract an archive file into a local snapshot directory.
    fn unpack_verified(archive: &Path, dest: &Path, id: &str) -> Result<()> {
        verify_archive_file(archive, Some(id))?;
        extract_archive_file(archive, dest, id)
    }

    #[tokio::test]
    async fn a_failed_archive_aborts_the_multipart_upload() {
        let store = fake_s3::Store::default();
        let endpoint = fake_s3::serve(store.clone()).await;
        let provider = fake_provider(&endpoint);

        let (tx, rx) = mpsc::channel::<storage::Part>(1);
        let producer = tokio::task::spawn_blocking(move || {
            let mut sink = PartWriter::new(tx, storage::PART_SIZE);
            sink.write_all(&vec![0u8; storage::PART_SIZE + 10])?;
            // The producer fails here: dropped without finish().
            Ok::<_, io::Error>(())
        });
        let result = storage::upload_stream(&provider, "snapshots/x.tondsnap", rx).await;
        let _ = producer.await;

        assert!(result.is_err(), "an unfinished stream must not complete");
        assert!(*store.aborted.lock().unwrap_or_else(|e| e.into_inner()));
        assert!(
            store
                .objects
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "no truncated object may be created"
        );
    }
}
