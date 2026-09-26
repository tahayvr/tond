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
// (hard-linked against the previous one). Only regular files and
// directories are copied: symlinks in app volumes are skipped, so a container
// can't make a snapshot read host files. The SQLite database is captured
// with `VACUUM INTO` (a consistent copy while tond-server keeps running).
//
// Remote copies are streamed end to end with constant memory: the snapshot
// directory is tarred, gzipped and encrypted (`snapshot_archive`, key derived
// from TOND_SECRET_KEY) on a blocking thread and handed to an S3 multipart
// upload one `storage::PART_SIZE` part at a time. Restoring downloads the
// archive to a file, checks its SHA-256 against the one recorded at upload,
// authenticates every record, and only then extracts it — so a server
// restoring from remote storage needs the same TOND_SECRET_KEY.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::handlers::storage_providers::decrypt_provider_secret;
use crate::state::AppState;
use crate::{db, docker, secret_key, secrets, snapshot_archive, storage};
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
}

impl SnapshotTrigger {
    fn trigger_str(&self) -> &'static str {
        match self {
            Self::Manual { .. } | Self::ManualDatabase { .. } => "manual",
            Self::Scheduled => "scheduled",
            Self::PreDeploy { .. } => "pre_deploy",
            Self::PreRestore { .. } | Self::PreRestoreDatabase { .. } => "pre_restore",
        }
    }

    fn scope(&self) -> String {
        match self {
            Self::Manual { .. } | Self::Scheduled | Self::PreRestore { .. } => "full".to_string(),
            Self::PreDeploy { app_name, .. } => format!("app:{}", app_name),
            Self::ManualDatabase { db_name } | Self::PreRestoreDatabase { db_name } => {
                format!("database:{db_name}")
            }
        }
    }

    fn trigger_app(&self) -> Option<&str> {
        match self {
            Self::PreDeploy { app_name, .. } => Some(app_name.as_str()),
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

fn snapshots_dir() -> PathBuf {
    PathBuf::from(data_dir()).join("backups").join("snapshots")
}

fn snapshot_dir(id: &str) -> PathBuf {
    snapshots_dir().join(id)
}

/// Keys that may decrypt a snapshot archive: the current TOND_SECRET_KEY and,
/// during a key rotation, the previous one.
fn archive_secrets() -> Vec<String> {
    let mut secrets = vec![secret_key::current_secret()];
    secrets.extend(secret_key::previous_secret());
    secrets
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Create a snapshot. Returns the snapshot ID once the local copy is complete.
/// `progress_tx` receives SSE-ready progress events while the snapshot runs.
pub async fn create_snapshot(
    state: &AppState,
    trigger: SnapshotTrigger,
    progress_tx: Option<ProgressTx>,
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
        "INSERT INTO snapshots (id, name, status, trigger, scope, trigger_app, deployment_id, parent_snapshot_id)
         VALUES (?, ?, 'running', ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(trigger.name())
    .bind(trigger.trigger_str())
    .bind(&scope)
    .bind(trigger.trigger_app())
    .bind(trigger.deployment_id())
    .bind(&parent_id)
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
                    if let Err(e) = dump_database(&state.docker, &db, &dest_db).await {
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

    // ── Write manifest.json ───────────────────────────────────────────────────
    let manifest = json!({
        "tond_version": env!("CARGO_PKG_VERSION"),
        "id": id,
        "created_at": Utc::now().to_rfc3339(),
        "trigger": trigger_str,
        "trigger_app": trigger_app,
        "deployment_id": deployment_id,
        "scope": scope,
        "delta_bytes": delta_bytes,
        "total_bytes": total_bytes,
        "apps_included": apps_included,
        "databases_included": databases_included,
    });
    tokio::fs::write(
        staging.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
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

/// Names of the directories directly inside `base` (empty when missing).
async fn list_subdirs(base: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(base).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
                && let Some(n) = entry.file_name().to_str().map(|s| s.to_string())
            {
                names.push(n);
            }
        }
    }
    names
}

/// `copy_dir_with_hardlinks` on a blocking thread.
async fn copy_tree(src: PathBuf, dest: PathBuf, prev: Option<PathBuf>) -> Result<(i64, i64)> {
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dest)?;
        copy_dir_with_hardlinks(&src, &dest, prev.as_deref())
    })
    .await
    .context("snapshot copy task failed")?
}

// ── Remote upload ─────────────────────────────────────────────────────────────

/// Upload an encrypted archive of the snapshot and record the outcome.
async fn upload_snapshot(
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
    let secret = secret_key::current_secret();
    let writer = tokio::task::spawn_blocking(move || {
        let sink = PartWriter::new(tx, storage::PART_SIZE);
        let (summary, sink) = snapshot_archive::write_archive(&src, sink, &secret, &snapshot_id)?;
        sink.finish()?;
        Ok::<_, anyhow::Error>(summary)
    });

    let uploaded = storage::upload_stream(&provider, &key, rx).await;
    let summary = writer.await.context("archive task failed")??;
    uploaded?;
    Ok((key, summary))
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
        tokio::task::spawn_blocking(move || unpack_verified(&archive, &dest, &id))
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

/// Verify the whole archive, then extract it next to `dest` and move it into
/// place, so a failed extraction never leaves a half-restored directory.
fn unpack_verified(archive: &Path, dest: &Path, snapshot_id: &str) -> Result<()> {
    let secrets = archive_secrets();
    let keys: Vec<&str> = secrets.iter().map(String::as_str).collect();
    snapshot_archive::verify_archive(std::fs::File::open(archive)?, &keys, Some(snapshot_id))?;

    let partial = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    if let Err(e) = snapshot_archive::extract_archive(
        std::fs::File::open(archive)?,
        &keys,
        Some(snapshot_id),
        &partial,
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

    // Auto-create a pre-restore safety net
    let pre_restore_id = create_snapshot(
        state,
        SnapshotTrigger::PreRestore {
            preceding_snapshot_id: snapshot_id.clone(),
        },
        None,
    )
    .await?;

    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = do_full_restore(&state_clone, &snap).await {
            tracing::error!("Full restore failed: {:#}", e);
        } else {
            tracing::info!(
                "Full restore of {} completed — exiting for systemd restart",
                snapshot_id
            );
            std::process::exit(0);
        }
    });

    Ok(pre_restore_id)
}

async fn do_full_restore(state: &AppState, snap: &Snapshot) -> Result<()> {
    // Fetch (and verify) the archive first: nothing is stopped or replaced
    // until the snapshot is known to be complete and authentic.
    let staging = ensure_local(state, snap).await?;

    // Stop all app containers
    let apps = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps")
        .fetch_all(&state.db)
        .await?;
    for app in &apps {
        if let Some(ref cid) = app.container_id {
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

    // Replace SQLite
    let db_dest = PathBuf::from(db_path());
    let db_src = staging.join("tond.db");
    if db_src.exists() {
        if let Some(parent) = db_dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&db_src, &db_dest).await?;
        // fs::copy also copies the snapshot's mode; restore tond-proxy's access.
        db::share_with_proxy(&db_dest);
    }

    // Replace app volumes
    let vol_base = PathBuf::from(data_dir()).join("volumes");
    let staging_vol = staging.join("volumes");
    if staging_vol.exists()
        && let Ok(mut entries) = tokio::fs::read_dir(&staging_vol).await
    {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let app_name = entry.file_name();
            let src = staging_vol.join(&app_name);
            let dest = vol_base.join(&app_name);
            if dest.exists() {
                tokio::fs::remove_dir_all(&dest).await?;
            }
            copy_dir_recursive(&src, &dest).await?;
        }
    }

    // Replace database volumes
    let db_vol_base = PathBuf::from(data_dir()).join("databases");
    let staging_dbs = staging.join("databases");
    if staging_dbs.exists()
        && let Ok(mut entries) = tokio::fs::read_dir(&staging_dbs).await
    {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let db_name = entry.file_name();
            let staging_db = staging_dbs.join(&db_name);
            let dest_vol = db_vol_base.join(&db_name);

            // Restore raw volume
            let raw = staging_db.join("volume");
            if raw.exists() {
                if dest_vol.exists() {
                    tokio::fs::remove_dir_all(&dest_vol).await?;
                }
                copy_dir_recursive(&raw, &dest_vol).await?;
            }
        }
    }

    // Replace SSH keys
    let keys_src = staging.join("keys");
    let keys_dest = PathBuf::from(data_dir()).join("keys");
    if keys_src.exists() {
        if keys_dest.exists() {
            tokio::fs::remove_dir_all(&keys_dest).await?;
        }
        copy_dir_recursive(&keys_src, &keys_dest).await?;
    }

    Ok(())
}

// ── App-scope restore ─────────────────────────────────────────────────────────

pub async fn restore_app(state: &AppState, snapshot_id: &str, app_name: &str) -> Result<()> {
    let snap = fetch_snapshot(state, snapshot_id).await?;
    let local_path = ensure_local(state, &snap).await?;
    let vol_src = local_path.join("volumes").join(app_name);

    if !vol_src.exists() {
        anyhow::bail!("Snapshot does not contain volumes for app '{}'", app_name);
    }

    // Find the app's container
    let app = sqlx::query_as::<_, tond_common::App>("SELECT * FROM apps WHERE name = ? LIMIT 1")
        .bind(app_name)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("App '{}' not found", app_name))?;

    // Stop container
    if let Some(ref cid) = app.container_id {
        let _ = docker::stop_container(&state.docker, cid).await;
    }

    // Atomically replace volume
    let vol_dest = PathBuf::from(data_dir()).join("volumes").join(app_name);
    if vol_dest.exists() {
        tokio::fs::remove_dir_all(&vol_dest).await?;
    }
    copy_dir_recursive(&vol_src, &vol_dest).await?;

    // Start container
    if let Some(ref cid) = app.container_id {
        let _ = docker::start_container(&state.docker, cid).await;
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
        copy_dir_recursive(&raw_src, &dest_vol).await?;
    }

    // Start container
    if let Some(ref cid) = db_row.container_id {
        let _ = docker::start_container(&state.docker, cid).await;
    }

    // Import logical dump
    let dump_path = db_staging.join("dump.archive");
    if dump_path.exists() {
        let _ = import_database_dump(&state.docker, &db_row, &dump_path).await;
    }
    let dump_sql = db_staging.join("dump.sql");
    if dump_sql.exists() {
        let _ = import_database_dump_sql(&state.docker, &db_row, &dump_sql).await;
    }

    Ok(safety_net)
}

// ── Backup scheduler ──────────────────────────────────────────────────────────

/// Sleeps until the next scheduled snapshot; a change to backup_config wakes
/// it to re-plan. Disabled or invalid schedules wait for a config change.
#[allow(dead_code)]
pub async fn backup_scheduler(state: AppState) {
    use crate::runtime::changes::{Table, Watch};
    use crate::runtime::loops::{self, Wake};
    use cron::Schedule;
    use std::str::FromStr;

    const IDLE: std::time::Duration = std::time::Duration::from_secs(3600);
    let lp = loops::register("backup-scheduler");
    let mut watch = Watch::new(&[Table::BackupConfig]);

    loop {
        let cfg = match get_backup_config(&state).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("backup_scheduler: failed to read config: {}", e);
                lp.wait(Some(std::time::Duration::from_secs(60)), &mut watch)
                    .await;
                continue;
            }
        };

        if cfg.enabled == 0 {
            lp.wait(Some(IDLE), &mut watch).await;
            continue;
        }

        let schedule = match Schedule::from_str(&cfg.full_schedule_cron) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "backup_scheduler: invalid cron '{}': {}",
                    cfg.full_schedule_cron,
                    e
                );
                lp.wait(Some(IDLE), &mut watch).await;
                continue;
            }
        };

        if let Some(next) = schedule.upcoming(chrono::Utc).next() {
            let now = chrono::Utc::now();
            let wait = (next - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(60));
            tracing::debug!("backup_scheduler: next run in {:?}", wait);
            if lp.wait(Some(wait), &mut watch).await == Wake::Event {
                continue; // config changed: re-plan
            }
            if crate::runtime::shutdown::is_shutting_down() {
                return;
            }
            drop(lp.run());

            let state_clone = state.clone();
            crate::runtime::shutdown::spawn_tracked(async move {
                let _ = create_snapshot(&state_clone, SnapshotTrigger::Scheduled, None).await;
            });
        } else {
            lp.wait(Some(IDLE), &mut watch).await;
        }
    }
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

// ── Hard-link deduplication copy ─────────────────────────────────────────────

/// Walk `src` and copy into `dest`, hard-linking from `prev` when mtime+size match.
/// Only regular files and directories are copied: symlinks (which a container
/// could point at host files), sockets and devices are skipped.
/// Returns (delta_bytes, total_bytes).
fn copy_dir_with_hardlinks(src: &Path, dest: &Path, prev: Option<&Path>) -> Result<(i64, i64)> {
    let mut delta: i64 = 0;
    let mut total: i64 = 0;

    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let dest_path = dest.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        let src_meta = entry.metadata()?;
        let size = src_meta.len() as i64;
        total += size;

        // Try hard-link from previous snapshot if mtime+size match
        let hard_linked = if let Some(prev_dir) = prev {
            let prev_path = prev_dir.join(rel);
            match std::fs::symlink_metadata(&prev_path) {
                Ok(prev_meta) if prev_meta.is_file() => {
                    let mtime_match = src_meta.modified().ok() == prev_meta.modified().ok();
                    let size_match = src_meta.len() == prev_meta.len();
                    if mtime_match && size_match {
                        if let Some(parent) = dest_path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        std::fs::hard_link(&prev_path, &dest_path).is_ok()
                    } else {
                        false
                    }
                }
                _ => false,
            }
        } else {
            false
        };

        if !hard_linked {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &dest_path)?;
            // Keep the mtime so the next snapshot can hard-link this file.
            if let Ok(mtime) = src_meta.modified() {
                let _ = std::fs::File::options()
                    .write(true)
                    .open(&dest_path)
                    .and_then(|f| f.set_modified(mtime));
            }
            delta += size;
        }
    }

    Ok((delta, total))
}

/// Simple recursive directory copy (no deduplication). Only regular files
/// and directories are copied.
async fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dest).await?;
    let mut stack = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((s, d)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&s).await?;
        while let Some(entry) = entries.next_entry().await? {
            let ft = entry.file_type().await?;
            let dest_entry = d.join(entry.file_name());
            if ft.is_dir() {
                tokio::fs::create_dir_all(&dest_entry).await?;
                stack.push((entry.path(), dest_entry));
            } else if ft.is_file() {
                tokio::fs::copy(entry.path(), dest_entry).await?;
            }
        }
    }
    Ok(())
}

// ── Remote upload key ─────────────────────────────────────────────────────────

fn upload_key_for_snapshot(id: &str) -> String {
    format!("snapshots/{}.tondsnap", id)
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
    // Full snapshots: keep last N
    if let Ok(old_full) = sqlx::query_as::<_, Snapshot>(
        "SELECT * FROM snapshots WHERE scope = 'full' AND status = 'completed'
         ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        for snap in old_full.iter().skip(cfg.full_retention_count as usize) {
            delete_snapshot_files_and_row(state, snap).await;
        }
    }

    // Database backups: keep last N per database
    if let Some(db_name) = db_scope
        && let Ok(old_db) = sqlx::query_as::<_, Snapshot>(
            "SELECT * FROM snapshots WHERE scope = ? AND status = 'completed'
             ORDER BY created_at DESC",
        )
        .bind(format!("database:{db_name}"))
        .fetch_all(&state.db)
        .await
    {
        for snap in old_db.iter().skip(cfg.full_retention_count as usize) {
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
            for snap in old_pre
                .iter()
                .skip(cfg.pre_deploy_retention_per_app as usize)
            {
                delete_snapshot_files_and_row(state, snap).await;
            }
        }
    }
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
        && let Err(e) = storage::delete(&provider, key).await
    {
        tracing::warn!(
            "Couldn't delete the remote copy of snapshot {}: {e:#}",
            snap.id
        );
    }
    let _ = sqlx::query("DELETE FROM snapshots WHERE id = ?")
        .bind(&snap.id)
        .execute(&state.db)
        .await;
}

// ── Fetch + ensure local helpers ──────────────────────────────────────────────

async fn fetch_snapshot(state: &AppState, id: &str) -> Result<Snapshot> {
    sqlx::query_as::<_, Snapshot>("SELECT * FROM snapshots WHERE id = ? AND status = 'completed'")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Snapshot '{}' not found or not completed", id))
}

/// Returns the local path of a snapshot, downloading (and verifying) the
/// remote copy if necessary.
async fn ensure_local(state: &AppState, snap: &Snapshot) -> Result<PathBuf> {
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

async fn dump_database(docker: &bollard::Docker, db: &ManagedDatabase, dest: &Path) -> Result<()> {
    use bollard::exec::CreateExecOptions;

    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let kind = db.db_type.as_str();
    // Credentials go in the exec env, never into a shell string (see mysql below).
    let mut env: Vec<String> = Vec::new();
    let (cmd, out_file) = match kind {
        "redis" | "keydb" => {
            // Managed Redis requires ACL auth (see handlers/databases.rs).
            // SAVE writes a consistent dump.rdb, which is then copied out.
            // REDISCLI_AUTH passes the password without exposing it in `ps`.
            let cli = if kind == "keydb" {
                "keydb-cli"
            } else {
                "redis-cli"
            };
            env.push(format!(
                "REDISCLI_AUTH={}",
                db.db_password.as_deref().unwrap_or("")
            ));
            (
                vec![
                    cli.to_string(),
                    "--user".to_string(),
                    db.db_user.as_deref().unwrap_or("default").to_string(),
                    "--no-auth-warning".to_string(),
                    "SAVE".to_string(),
                ],
                "/data/dump.rdb",
            )
        }
        "postgres" => {
            let user = db.db_user.as_deref().unwrap_or("postgres");
            let dbname = db.db_name.as_deref().unwrap_or("postgres");
            (
                vec![
                    "pg_dump".to_string(),
                    "-U".to_string(),
                    user.to_string(),
                    "-F".to_string(),
                    "c".to_string(),
                    "-f".to_string(),
                    "/tmp/dump.archive".to_string(),
                    dbname.to_string(),
                ],
                "/tmp/dump.archive",
            )
        }
        "mysql" | "mariadb" => {
            let user = db.db_user.as_deref().unwrap_or("root");
            let pass = db.db_password.as_deref().unwrap_or("");
            let dbname = db.db_name.as_deref().unwrap_or("mysql");
            env = mysql_exec_env(user, pass, dbname);
            (
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    MYSQL_DUMP_SCRIPT.to_string(),
                ],
                "/tmp/dump.sql",
            )
        }
        "mongodb" => {
            let user = db.db_user.as_deref().unwrap_or("");
            let pass = db.db_password.as_deref().unwrap_or("");
            (
                vec![
                    "mongodump".to_string(),
                    "--archive=/tmp/dump.archive".to_string(),
                    format!("--username={}", user),
                    format!("--password={}", pass),
                    "--authenticationDatabase=admin".to_string(),
                ],
                "/tmp/dump.archive",
            )
        }
        _ => {
            tracing::debug!("DB kind '{}': raw volume copy only, no logical dump", kind);
            return Ok(());
        }
    };

    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                cmd: Some(cmd),
                env: Some(env),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;

    let start_result = docker.start_exec(&exec.id, None).await?;
    // Wait for exec to finish by inspecting
    loop {
        let info = docker.inspect_exec(&exec.id).await?;
        if !info.running.unwrap_or(true) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    drop(start_result);

    // Copy dump from container to host using `docker cp`-style read
    let dump_filename = if out_file.ends_with(".archive") {
        "dump.archive"
    } else if out_file.ends_with(".rdb") {
        "dump.rdb"
    } else {
        "dump.sql"
    };
    let dest_file = dest.join(dump_filename);

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
struct ChunkWriter {
    tx: mpsc::Sender<std::result::Result<axum::body::Bytes, io::Error>>,
    buf: Vec<u8>,
}

impl ChunkWriter {
    const CHUNK: usize = 256 * 1024;

    fn new(tx: mpsc::Sender<std::result::Result<axum::body::Bytes, io::Error>>) -> Self {
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

    fn finish(mut self) -> io::Result<()> {
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

async fn import_database_dump(
    docker: &bollard::Docker,
    db: &ManagedDatabase,
    dump_path: &Path,
) -> Result<()> {
    use bollard::exec::CreateExecOptions;

    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let kind = db.db_type.as_str();
    let user = db.db_user.as_deref().unwrap_or("postgres");
    let dbname = db.db_name.as_deref().unwrap_or("postgres");

    // Stream the dump into the container
    upload_file_to_container(docker, container_id, "/tmp", "dump.archive", dump_path).await?;

    // Run the restore command
    let cmd = match kind {
        "postgres" => vec![
            "pg_restore".to_string(),
            "-U".to_string(),
            user.to_string(),
            "-d".to_string(),
            dbname.to_string(),
            "--clean".to_string(),
            "/tmp/dump.archive".to_string(),
        ],
        "mongodb" => vec![
            "mongorestore".to_string(),
            "--archive=/tmp/dump.archive".to_string(),
            format!("--username={}", user),
            format!("--password={}", db.db_password.as_deref().unwrap_or("")),
            "--authenticationDatabase=admin".to_string(),
            "--drop".to_string(),
        ],
        _ => return Ok(()),
    };

    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                cmd: Some(cmd),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;

    let _start = docker.start_exec(&exec.id, None).await?;
    loop {
        let info = docker.inspect_exec(&exec.id).await?;
        if !info.running.unwrap_or(true) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    Ok(())
}

async fn import_database_dump_sql(
    docker: &bollard::Docker,
    db: &ManagedDatabase,
    dump_path: &Path,
) -> Result<()> {
    use bollard::exec::CreateExecOptions;

    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let kind = db.db_type.as_str();
    let user = db.db_user.as_deref().unwrap_or("root");
    let pass = db.db_password.as_deref().unwrap_or("");
    let dbname = db.db_name.as_deref().unwrap_or("mysql");

    upload_file_to_container(docker, container_id, "/tmp", "dump.sql", dump_path).await?;

    let cmd = match kind {
        "mysql" | "mariadb" => vec![
            "sh".to_string(),
            "-c".to_string(),
            MYSQL_IMPORT_SCRIPT.to_string(),
        ],
        _ => return Ok(()),
    };

    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                cmd: Some(cmd),
                env: Some(mysql_exec_env(user, pass, dbname)),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;

    let _start = docker.start_exec(&exec.id, None).await?;
    loop {
        let info = docker.inspect_exec(&exec.id).await?;
        if !info.running.unwrap_or(true) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::MetadataExt};

    use filetime::{FileTime, set_file_mtime};
    use tempfile::TempDir;

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
    fn copy_dir_with_hardlinks_copies_new_files_and_hard_links_unchanged_files() {
        let temp = temp_dir();
        let src = temp.path().join("src");
        let prev = temp.path().join("prev");
        let dest = temp.path().join("dest");
        fs::create_dir_all(src.join("nested"))
            .unwrap_or_else(|err| panic!("failed to create src directories: {err}"));
        fs::create_dir_all(prev.join("nested"))
            .unwrap_or_else(|err| panic!("failed to create prev directories: {err}"));

        fs::write(src.join("same.txt"), b"same")
            .unwrap_or_else(|err| panic!("failed to write src same.txt: {err}"));
        fs::write(src.join("nested").join("changed.txt"), b"new contents")
            .unwrap_or_else(|err| panic!("failed to write src changed.txt: {err}"));
        fs::write(prev.join("same.txt"), b"same")
            .unwrap_or_else(|err| panic!("failed to write prev same.txt: {err}"));
        fs::write(prev.join("nested").join("changed.txt"), b"old")
            .unwrap_or_else(|err| panic!("failed to write prev changed.txt: {err}"));
        let mtime = FileTime::from_unix_time(1_700_000_000, 0);
        set_file_mtime(src.join("same.txt"), mtime)
            .unwrap_or_else(|err| panic!("failed to set src same.txt mtime: {err}"));
        set_file_mtime(prev.join("same.txt"), mtime)
            .unwrap_or_else(|err| panic!("failed to set prev same.txt mtime: {err}"));

        let (delta, total) = copy_dir_with_hardlinks(&src, &dest, Some(&prev))
            .unwrap_or_else(|err| panic!("expected copy_dir_with_hardlinks to succeed: {err}"));

        assert_eq!(
            fs::read(dest.join("same.txt"))
                .unwrap_or_else(|err| panic!("failed to read dest same.txt: {err}")),
            b"same",
            "copy_dir_with_hardlinks should copy file contents into the destination tree"
        );
        assert_eq!(
            fs::read(dest.join("nested").join("changed.txt"))
                .unwrap_or_else(|err| panic!("failed to read dest changed.txt: {err}")),
            b"new contents",
            "copy_dir_with_hardlinks should copy changed files from the source tree"
        );
        assert!(
            delta >= 12,
            "delta bytes should include newly copied file bytes, got {delta}"
        );
        assert!(
            total >= delta,
            "total bytes should be at least as large as delta bytes"
        );

        let prev_meta = fs::metadata(prev.join("same.txt"))
            .unwrap_or_else(|err| panic!("failed to stat prev same.txt: {err}"));
        let dest_meta = fs::metadata(dest.join("same.txt"))
            .unwrap_or_else(|err| panic!("failed to stat dest same.txt: {err}"));
        assert_eq!(
            prev_meta.ino(),
            dest_meta.ino(),
            "unchanged files should be hard-linked from the previous snapshot when metadata matches"
        );
    }

    #[tokio::test]
    async fn copy_dir_recursive_copies_nested_directory_structure() {
        let temp = temp_dir();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        fs::create_dir_all(src.join("nested"))
            .unwrap_or_else(|err| panic!("failed to create src directories: {err}"));
        fs::write(src.join("root.txt"), b"root")
            .unwrap_or_else(|err| panic!("failed to write root file: {err}"));
        fs::write(src.join("nested").join("child.txt"), b"child")
            .unwrap_or_else(|err| panic!("failed to write nested file: {err}"));

        copy_dir_recursive(&src, &dest)
            .await
            .unwrap_or_else(|err| panic!("expected copy_dir_recursive to succeed: {err}"));

        assert_eq!(
            tokio::fs::read(dest.join("root.txt"))
                .await
                .unwrap_or_else(|err| panic!("failed to read copied root file: {err}")),
            b"root",
            "copy_dir_recursive should copy files at the root"
        );
        assert_eq!(
            tokio::fs::read(dest.join("nested").join("child.txt"))
                .await
                .unwrap_or_else(|err| panic!("failed to read copied nested file: {err}")),
            b"child",
            "copy_dir_recursive should copy nested files recursively"
        );
    }

    #[test]
    fn copy_dir_with_hardlinks_skips_symlinks_out_of_the_volume() {
        let temp = temp_dir();
        let src = temp.path().join("src");
        let dest = temp.path().join("dest");
        let secret = temp.path().join("host-secret.env");
        fs::create_dir_all(&src).unwrap_or_else(|err| panic!("mkdir: {err}"));
        fs::write(&secret, b"TOND_SECRET_KEY=hunter2").unwrap_or_else(|err| panic!("write: {err}"));
        fs::write(src.join("ok.txt"), b"ok").unwrap_or_else(|err| panic!("write: {err}"));
        std::os::unix::fs::symlink(&secret, src.join("leak"))
            .unwrap_or_else(|err| panic!("symlink: {err}"));

        copy_dir_with_hardlinks(&src, &dest, None)
            .unwrap_or_else(|err| panic!("copy should succeed: {err}"));

        assert!(dest.join("ok.txt").is_file());
        assert!(
            fs::symlink_metadata(dest.join("leak")).is_err(),
            "a symlink in a volume must not copy the host file it points at"
        );
    }

    #[test]
    fn copy_dir_with_hardlinks_keeps_mtime_so_the_next_snapshot_dedupes() {
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
        let (d1, _) = copy_dir_with_hardlinks(&src, &first, None)
            .unwrap_or_else(|err| panic!("first copy: {err}"));
        let second = temp.path().join("second");
        let (d2, t2) = copy_dir_with_hardlinks(&src, &second, Some(&first))
            .unwrap_or_else(|err| panic!("second copy: {err}"));

        assert_eq!(d1, 4096);
        assert_eq!((d2, t2), (0, 4096), "an unchanged file must be hard-linked");
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

    #[test]
    fn upload_key_for_snapshot_uses_snapshots_prefix() {
        assert_eq!(
            upload_key_for_snapshot("snap-123"),
            "snapshots/snap-123.tondsnap",
            "upload_key_for_snapshot should store archives under the snapshots/ prefix"
        );
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

        /// Serve the fake bucket; returns its base URL.
        pub async fn serve(store: Store) -> String {
            let app = Router::new()
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
        let staging = temp.path().join("snap");
        fs::create_dir_all(staging.join("volumes/web")).unwrap_or_else(|e| panic!("{e}"));
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let data: Vec<u8> = (0..(storage::PART_SIZE + 3 * 1024 * 1024))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        fs::write(staging.join("volumes/web/blob.bin"), &data).unwrap_or_else(|e| panic!("{e}"));
        fs::write(staging.join("manifest.json"), b"{}").unwrap_or_else(|e| panic!("{e}"));

        let (tx, rx) = mpsc::channel::<storage::Part>(1);
        let src = staging.clone();
        let writer = tokio::task::spawn_blocking(move || {
            let sink = PartWriter::new(tx, storage::PART_SIZE);
            let (summary, sink) = snapshot_archive::write_archive(
                &src,
                sink,
                &secret_key::current_secret(),
                "snap-1",
            )?;
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
            fs::read(restored.join("volumes/web/blob.bin"))
                .ok()
                .as_deref(),
            Some(data.as_slice())
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
