// backup.rs — Snapshot engine (Steps 4, 6, 7, 8, 9, 11)
//
// Public API consumed by handlers and the pre-deploy hook:
//
//   create_snapshot(state, trigger, progress_tx) -> anyhow::Result<String>
//     Runs the full create flow. Returns the snapshot ID.
//     `progress_tx` is an optional channel for SSE events; None = silent.
//
//   restore_full(state, snapshot_id) -> anyhow::Result<String>
//     Creates a pre_restore safety-net snapshot then launches a background task
//     that replaces all data and calls std::process::exit(0).
//     Returns the pre_restore snapshot ID.
//
//   restore_app(state, snapshot_id, app_name) -> anyhow::Result<()>
//     App-scope rollback: stop container → swap volume dir → start container.
//
//   restore_database(state, snapshot_id, db_name) -> anyhow::Result<()>
//     Database-scope restore: stop → swap volume → start → import dump.
//
//   backup_scheduler(state) -> !
//     Long-running background task. Re-reads config each iteration.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::handlers::storage_providers::decrypt_provider_secret;
use crate::state::AppState;
use crate::{db, docker, storage};
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
    Scheduled,
    PreDeploy {
        app_name: String,
        deployment_id: String,
    },
    PreRestore {
        preceding_snapshot_id: String,
    },
}

impl SnapshotTrigger {
    fn trigger_str(&self) -> &'static str {
        match self {
            Self::Manual { .. } => "manual",
            Self::Scheduled => "scheduled",
            Self::PreDeploy { .. } => "pre_deploy",
            Self::PreRestore { .. } => "pre_restore",
        }
    }

    fn scope(&self) -> String {
        match self {
            Self::Manual { .. } | Self::Scheduled | Self::PreRestore { .. } => "full".to_string(),
            Self::PreDeploy { app_name, .. } => format!("app:{}", app_name),
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

// ── Public entry point ────────────────────────────────────────────────────────

/// Create a snapshot. Returns the snapshot ID.
/// `progress_tx` receives SSE-ready progress events while the snapshot runs.
pub async fn create_snapshot(
    state: &AppState,
    trigger: SnapshotTrigger,
    progress_tx: Option<ProgressTx>,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let name = trigger.name();
    let trigger_str = trigger.trigger_str();
    let scope = trigger.scope();
    let trigger_app = trigger.trigger_app().map(|s| s.to_string());
    let deployment_id = trigger.deployment_id().map(|s| s.to_string());

    // Find previous completed local snapshot for hard-link base
    let parent_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM snapshots WHERE status = 'completed' AND local_path IS NOT NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;

    // Insert row with status = "running"
    sqlx::query(
        "INSERT INTO snapshots (id, name, status, trigger, scope, trigger_app, deployment_id, parent_snapshot_id)
         VALUES (?, ?, 'running', ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(trigger_str)
    .bind(&scope)
    .bind(&trigger_app)
    .bind(&deployment_id)
    .bind(&parent_id)
    .execute(&state.db)
    .await?;

    emit(&progress_tx, "start", "Snapshot started…", false).await;

    let staging = snapshot_dir(&id);
    tokio::fs::create_dir_all(&staging).await?;

    let mut delta_bytes: i64 = 0;
    let mut total_bytes: i64 = 0;

    // Determine previous snapshot directory for hard-link dedup
    let prev_dir: Option<PathBuf> = if let Some(ref pid) = parent_id {
        let prev_path: Option<String> =
            sqlx::query_scalar("SELECT local_path FROM snapshots WHERE id = ?")
                .bind(pid)
                .fetch_optional(&state.db)
                .await?;
        prev_path.map(PathBuf::from)
    } else {
        None
    };

    // ── Determine what to back up based on scope ──────────────────────────────

    let is_full = scope == "full";
    let app_scope: Option<String> = if scope.starts_with("app:") {
        Some(scope.trim_start_matches("app:").to_string())
    } else {
        None
    };
    let db_scope: Option<String> = if scope.starts_with("database:") {
        Some(scope.trim_start_matches("database:").to_string())
    } else {
        None
    };

    // ── Step: SQLite (full scope only) ────────────────────────────────────────
    let mut apps_included: Vec<String> = Vec::new();
    let mut databases_included: Vec<String> = Vec::new();

    if is_full {
        emit(
            &progress_tx,
            "sqlite",
            "Snapshotting SQLite database…",
            false,
        )
        .await;
        let dest = staging.join("tond.db");
        let db_src = db_path();
        // VACUUM INTO creates a consistent copy without blocking reads/writes
        sqlx::query("VACUUM INTO ?")
            .bind(dest.to_string_lossy().into_owned())
            .execute(&state.db)
            .await?;
        let size = tokio::fs::metadata(&dest)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        delta_bytes += size as i64;
        total_bytes += size as i64;
        tracing::debug!("SQLite snapshot: {} bytes", size);
        let _ = db_src; // suppress unused warning
    }

    // ── Step: App volumes ─────────────────────────────────────────────────────
    let vol_base = PathBuf::from(data_dir()).join("volumes");

    let vol_apps: Vec<String> = if let Some(ref app_name) = app_scope {
        vec![app_name.clone()]
    } else if is_full {
        // All apps that have a volume directory
        let mut names = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&vol_base).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
                    && let Some(n) = entry.file_name().to_str().map(|s| s.to_string())
                {
                    names.push(n);
                }
            }
        }
        names
    } else {
        Vec::new()
    };

    for (i, app_name) in vol_apps.iter().enumerate() {
        let src = vol_base.join(app_name);
        if !src.exists() {
            continue;
        }
        emit(
            &progress_tx,
            "volumes",
            &format!("Copying app volumes ({}/{})…", i + 1, vol_apps.len()),
            false,
        )
        .await;

        let dest_vol = staging.join("volumes").join(app_name);
        tokio::fs::create_dir_all(&dest_vol).await?;

        let prev_vol = prev_dir.as_ref().map(|p| p.join("volumes").join(app_name));

        let (d, t) = copy_dir_with_hardlinks(&src, &dest_vol, prev_vol.as_deref())?;
        delta_bytes += d;
        total_bytes += t;
        apps_included.push(app_name.clone());
    }

    // ── Step: Managed DB dumps (full scope or db scope) ───────────────────────
    if is_full || db_scope.is_some() {
        let db_names: Vec<String> = if let Some(ref db_name) = db_scope {
            vec![db_name.clone()]
        } else {
            // Discover all managed-db volume directories
            let db_vol_base = PathBuf::from(data_dir()).join("databases");
            let mut names = Vec::new();
            if let Ok(mut entries) = tokio::fs::read_dir(&db_vol_base).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
                        && let Some(n) = entry.file_name().to_str().map(|s| s.to_string())
                    {
                        names.push(n);
                    }
                }
            }
            names
        };

        for db_name in &db_names {
            emit(
                &progress_tx,
                "database",
                &format!("Dumping database: {}", db_name),
                false,
            )
            .await;

            let dest_db = staging.join("databases").join(db_name);
            tokio::fs::create_dir_all(&dest_db).await?;

            // Try logical dump via bollard
            let db_row = sqlx::query_as::<_, ManagedDatabase>(
                "SELECT * FROM databases WHERE name = ? LIMIT 1",
            )
            .bind(db_name)
            .fetch_optional(&state.db)
            .await?;

            if let Some(db) = db_row {
                let _ = dump_database(&state.docker, &db, &dest_db).await;
            }

            // Raw volume copy (always, as fallback)
            let src_db_vol = PathBuf::from(data_dir()).join("databases").join(db_name);
            if src_db_vol.exists() {
                let dest_raw = dest_db.join("volume");
                tokio::fs::create_dir_all(&dest_raw).await?;
                let prev_db_vol = prev_dir
                    .as_ref()
                    .map(|p| p.join("databases").join(db_name).join("volume"));
                let (d, t) =
                    copy_dir_with_hardlinks(&src_db_vol, &dest_raw, prev_db_vol.as_deref())?;
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
            let keys_dest = staging.join("keys");
            tokio::fs::create_dir_all(&keys_dest).await?;
            let prev_keys = prev_dir.as_ref().map(|p| p.join("keys"));
            let (d, t) = copy_dir_with_hardlinks(&keys_src, &keys_dest, prev_keys.as_deref())?;
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
        "remote_key": null,
        "storage_provider_id": null,
        "scope": scope,
        "delta_bytes": delta_bytes,
        "total_bytes": total_bytes,
        "apps_included": apps_included,
        "databases_included": databases_included,
        "checksums": {}
    });
    tokio::fs::write(
        staging.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )
    .await?;

    // ── Persist completion ────────────────────────────────────────────────────
    let local_path = staging.to_string_lossy().to_string();
    sqlx::query(
        "UPDATE snapshots
         SET status = 'completed', local_path = ?, delta_bytes = ?, total_bytes = ?,
             completed_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&local_path)
    .bind(delta_bytes)
    .bind(total_bytes)
    .bind(&id)
    .execute(&state.db)
    .await?;

    // Update `latest` symlink
    let latest = snapshots_dir().join("latest");
    let _ = std::fs::remove_file(&latest);
    let _ = std::os::unix::fs::symlink(&staging, &latest);

    // ── Remote upload (if configured) ────────────────────────────────────────
    let cfg = get_backup_config(state).await?;
    if is_full && let Some(ref sp_id) = cfg.storage_provider_id {
        emit(
            &progress_tx,
            "archive",
            "Creating archive for upload…",
            false,
        )
        .await;
        if let Ok((tar_path, checksum)) = archive_snapshot(&staging) {
            emit(&progress_tx, "s3", "Uploading to remote storage…", false).await;
            let provider = sqlx::query_as::<_, tond_common::StorageProvider>(
                "SELECT * FROM storage_providers WHERE id = ?",
            )
            .bind(sp_id)
            .fetch_optional(&state.db)
            .await?;

            if let Some(raw_provider) = provider {
                let decrypted = decrypt_provider_secret(raw_provider);
                if let Ok(dec) = decrypted {
                    let key = upload_key_for_snapshot(&id);
                    let upload_result = storage::upload(&dec, &key, &tar_path).await;
                    if let Ok(()) = upload_result {
                        sqlx::query(
                            "UPDATE snapshots SET remote_key = ?, storage_provider_id = ?,
                             checksum_sha256 = ? WHERE id = ?",
                        )
                        .bind(&key)
                        .bind(sp_id)
                        .bind(&checksum)
                        .bind(&id)
                        .execute(&state.db)
                        .await?;

                        if cfg.keep_local == 0 {
                            let _ = tokio::fs::remove_dir_all(&staging).await;
                            sqlx::query("UPDATE snapshots SET local_path = NULL WHERE id = ?")
                                .bind(&id)
                                .execute(&state.db)
                                .await?;
                        }
                    } else {
                        tracing::warn!(
                            "Remote upload failed for snapshot {}: {:?}",
                            id,
                            upload_result
                        );
                    }
                }
            }
            // Clean up the local tar.gz regardless
            let _ = tokio::fs::remove_file(&tar_path).await;
        }
    }

    // ── Retention enforcement ─────────────────────────────────────────────────
    enforce_retention(state, &cfg, trigger_app.as_deref()).await;

    let delta_mb = delta_bytes as f64 / 1_048_576.0;
    emit(
        &progress_tx,
        "done",
        &format!("Snapshot completed ({:.1} MB new)", delta_mb),
        true,
    )
    .await;

    Ok(id)
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
            tracing::error!("Full restore failed: {}", e);
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
    let local_path = snap
        .local_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No local path for snapshot"))?;

    let staging = PathBuf::from(local_path);
    if !staging.exists() {
        // Try to download from remote if available
        if let (Some(key), Some(sp_id)) = (&snap.remote_key, &snap.storage_provider_id) {
            let provider = sqlx::query_as::<_, tond_common::StorageProvider>(
                "SELECT * FROM storage_providers WHERE id = ?",
            )
            .bind(sp_id)
            .fetch_optional(&state.db)
            .await?;

            if let Some(raw_provider) = provider {
                let dec = decrypt_provider_secret(raw_provider)?;
                let tar_path = PathBuf::from("/tmp").join(format!("{}.tar.gz", snap.id));
                storage::download(&dec, key, &tar_path).await?;
                extract_tar_gz(&tar_path, &staging)?;
                let _ = tokio::fs::remove_file(&tar_path).await;
            } else {
                anyhow::bail!("Remote provider not found and no local path");
            }
        } else {
            anyhow::bail!("Snapshot local path does not exist and no remote");
        }
    }

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

pub async fn restore_database(state: &AppState, snapshot_id: &str, db_name: &str) -> Result<()> {
    let snap = fetch_snapshot(state, snapshot_id).await?;
    let local_path = ensure_local(state, &snap).await?;
    let db_staging = local_path.join("databases").join(db_name);

    if !db_staging.exists() {
        anyhow::bail!("Snapshot does not contain data for database '{}'", db_name);
    }

    let db_row =
        sqlx::query_as::<_, ManagedDatabase>("SELECT * FROM databases WHERE name = ? LIMIT 1")
            .bind(db_name)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Database '{}' not found", db_name))?;

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

    Ok(())
}

// ── Backup scheduler ──────────────────────────────────────────────────────────

#[allow(dead_code)]
pub async fn backup_scheduler(state: AppState) {
    use cron::Schedule;
    use std::str::FromStr;

    loop {
        let cfg = match get_backup_config(&state).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("backup_scheduler: failed to read config: {}", e);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }
        };

        if cfg.enabled == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
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
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }
        };

        if let Some(next) = schedule.upcoming(chrono::Utc).next() {
            let now = chrono::Utc::now();
            let wait = (next - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(60));
            tracing::info!("backup_scheduler: next run in {:?}", wait);
            tokio::time::sleep(wait).await;

            let state_clone = state.clone();
            tokio::spawn(async move {
                let _ = create_snapshot(&state_clone, SnapshotTrigger::Scheduled, None).await;
            });
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
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
/// Returns (delta_bytes, total_bytes).
fn copy_dir_with_hardlinks(src: &Path, dest: &Path, prev: Option<&Path>) -> Result<(i64, i64)> {
    let mut delta: i64 = 0;
    let mut total: i64 = 0;

    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let dest_path = dest.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            continue;
        }

        let src_meta = entry.metadata()?;
        let size = src_meta.len() as i64;
        total += size;

        // Try hard-link from previous snapshot if mtime+size match
        let hard_linked = if let Some(prev_dir) = prev {
            let prev_path = prev_dir.join(rel);
            if prev_path.exists() {
                let prev_meta = std::fs::metadata(&prev_path)?;
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
            } else {
                false
            }
        } else {
            false
        };

        if !hard_linked {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &dest_path)?;
            delta += size;
        }
    }

    Ok((delta, total))
}

/// Simple recursive directory copy (no deduplication).
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
            } else {
                tokio::fs::copy(entry.path(), dest_entry).await?;
            }
        }
    }
    Ok(())
}

// ── Archiving ─────────────────────────────────────────────────────────────────

/// Create a .tar.gz of the snapshot directory and return (tar_path, sha256_hex).
fn archive_snapshot(staging: &Path) -> Result<(PathBuf, String)> {
    let tar_path = staging.with_extension("tar.gz");
    let file = std::fs::File::create(&tar_path)?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut ar = tar::Builder::new(enc);
    ar.append_dir_all(".", staging)?;
    let enc = ar.into_inner()?;
    let _file = enc.finish()?;

    let contents = std::fs::read(&tar_path)?;
    let checksum = hex::encode(Sha256::digest(&contents));

    Ok((tar_path, checksum))
}

fn extract_tar_gz(tar_path: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(tar_path)?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(dec);
    ar.unpack(dest)?;
    Ok(())
}

// ── Remote upload key ─────────────────────────────────────────────────────────

fn upload_key_for_snapshot(id: &str) -> String {
    format!("snapshots/{}.tar.gz", id)
}

// ── Config helper ─────────────────────────────────────────────────────────────

pub async fn get_backup_config(state: &AppState) -> Result<BackupConfig> {
    sqlx::query_as::<_, BackupConfig>("SELECT * FROM backup_config WHERE id = 1")
        .fetch_one(&state.db)
        .await
        .map_err(Into::into)
}

// ── Retention enforcement ─────────────────────────────────────────────────────

async fn enforce_retention(state: &AppState, cfg: &BackupConfig, trigger_app: Option<&str>) {
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

async fn delete_snapshot_files_and_row(state: &AppState, snap: &Snapshot) {
    if let Some(ref path) = snap.local_path {
        let _ = tokio::fs::remove_dir_all(path).await;
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

/// Returns the local path of a snapshot, downloading from remote if necessary.
async fn ensure_local(state: &AppState, snap: &Snapshot) -> Result<PathBuf> {
    if let Some(ref local) = snap.local_path {
        let p = PathBuf::from(local);
        if p.exists() {
            return Ok(p);
        }
    }

    // Download from remote
    if let (Some(key), Some(sp_id)) = (&snap.remote_key, &snap.storage_provider_id) {
        let provider = sqlx::query_as::<_, tond_common::StorageProvider>(
            "SELECT * FROM storage_providers WHERE id = ?",
        )
        .bind(sp_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Storage provider not found"))?;

        let dec = decrypt_provider_secret(provider)?;
        let tar_path = PathBuf::from("/tmp").join(format!("{}.tar.gz", snap.id));
        storage::download(&dec, key, &tar_path).await?;

        let staging = snapshot_dir(&snap.id);
        extract_tar_gz(&tar_path, &staging)?;
        let _ = tokio::fs::remove_file(&tar_path).await;

        sqlx::query("UPDATE snapshots SET local_path = ? WHERE id = ?")
            .bind(staging.to_string_lossy().as_ref())
            .bind(&snap.id)
            .execute(&state.db)
            .await?;

        return Ok(staging);
    }

    anyhow::bail!("Snapshot has no local path and no remote key")
}

// ── DB logical dumps (Step 6) ─────────────────────────────────────────────────

async fn dump_database(docker: &bollard::Docker, db: &ManagedDatabase, dest: &Path) -> Result<()> {
    use bollard::exec::CreateExecOptions;

    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let kind = db.db_type.as_str();

    let (cmd, out_file) = match kind {
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
            (
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!(
                        "mysqldump --single-transaction -u{} -p{} {} > /tmp/dump.sql",
                        user, pass, dbname
                    ),
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
    } else {
        "dump.sql"
    };
    let dest_file = dest.join(dump_filename);

    // Use bollard to download the file from the container
    use futures::StreamExt as _;
    let mut stream = docker.download_from_container(
        container_id,
        Some(
            bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
                .path(out_file)
                .build(),
        ),
    );
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => buf.extend_from_slice(&bytes),
            Err(e) => {
                tracing::warn!("dump download error: {}", e);
                break;
            }
        }
    }

    if !buf.is_empty() {
        // The response is a tar archive; extract the single file
        let cursor = std::io::Cursor::new(&buf);
        let mut tar = tar::Archive::new(cursor);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if path.extension().is_some() || path.to_string_lossy().contains("dump") {
                entry.unpack(&dest_file)?;
                break;
            }
        }
    }

    Ok(())
}

async fn import_database_dump(
    docker: &bollard::Docker,
    db: &ManagedDatabase,
    dump_path: &Path,
) -> Result<()> {
    use bollard::exec::CreateExecOptions;
    use bollard::query_parameters::UploadToContainerOptionsBuilder;

    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let kind = db.db_type.as_str();
    let user = db.db_user.as_deref().unwrap_or("postgres");
    let dbname = db.db_name.as_deref().unwrap_or("postgres");

    // Create a tar containing the dump to upload
    let dump_bytes = tokio::fs::read(dump_path).await?;
    let mut tar_buf = Vec::new();
    {
        let mut ar = tar::Builder::new(&mut tar_buf);
        let mut header = tar::Header::new_gnu();
        header.set_size(dump_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        ar.append_data(&mut header, "dump.archive", dump_bytes.as_slice())?;
        ar.finish()?;
    }

    // Upload to container
    docker
        .upload_to_container(
            container_id,
            Some(
                UploadToContainerOptionsBuilder::default()
                    .path("/tmp")
                    .build(),
            ),
            bollard::body_full(tar_buf.into()),
        )
        .await?;

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
    use bollard::query_parameters::UploadToContainerOptionsBuilder;

    let container_id = db
        .container_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DB container not running"))?;

    let kind = db.db_type.as_str();
    let user = db.db_user.as_deref().unwrap_or("root");
    let pass = db.db_password.as_deref().unwrap_or("");
    let dbname = db.db_name.as_deref().unwrap_or("mysql");

    let dump_bytes = tokio::fs::read(dump_path).await?;
    let mut tar_buf = Vec::new();
    {
        let mut ar = tar::Builder::new(&mut tar_buf);
        let mut header = tar::Header::new_gnu();
        header.set_size(dump_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        ar.append_data(&mut header, "dump.sql", dump_bytes.as_slice())?;
        ar.finish()?;
    }

    docker
        .upload_to_container(
            container_id,
            Some(
                UploadToContainerOptionsBuilder::default()
                    .path("/tmp")
                    .build(),
            ),
            bollard::body_full(tar_buf.into()),
        )
        .await?;

    let cmd = match kind {
        "mysql" | "mariadb" => vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("mysql -u{} -p{} {} < /tmp/dump.sql", user, pass, dbname),
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
    fn archive_and_extract_snapshot_round_trip_contents_and_checksum() {
        let temp = temp_dir();
        let staging = temp.path().join("snapshot-dir");
        let extracted = temp.path().join("extracted");
        fs::create_dir_all(staging.join("nested"))
            .unwrap_or_else(|err| panic!("failed to create staging directories: {err}"));
        fs::write(staging.join("manifest.json"), b"{}")
            .unwrap_or_else(|err| panic!("failed to write manifest.json: {err}"));
        fs::write(staging.join("nested").join("data.txt"), b"payload")
            .unwrap_or_else(|err| panic!("failed to write data.txt: {err}"));

        let (archive, checksum) = archive_snapshot(&staging)
            .unwrap_or_else(|err| panic!("expected archive_snapshot to succeed: {err}"));
        let contents = fs::read(&archive)
            .unwrap_or_else(|err| panic!("failed to read created archive: {err}"));
        assert_eq!(
            checksum,
            hex::encode(Sha256::digest(&contents)),
            "archive_snapshot should return the SHA-256 checksum of the generated tarball"
        );

        extract_tar_gz(&archive, &extracted)
            .unwrap_or_else(|err| panic!("expected extract_tar_gz to succeed: {err}"));
        assert_eq!(
            fs::read(extracted.join("nested").join("data.txt"))
                .unwrap_or_else(|err| panic!("failed to read extracted data.txt: {err}")),
            b"payload",
            "extract_tar_gz should restore archived files"
        );
    }

    #[test]
    fn upload_key_for_snapshot_uses_snapshots_prefix() {
        assert_eq!(
            upload_key_for_snapshot("snap-123"),
            "snapshots/snap-123.tar.gz",
            "upload_key_for_snapshot should store archives under the snapshots/ prefix"
        );
    }
}
