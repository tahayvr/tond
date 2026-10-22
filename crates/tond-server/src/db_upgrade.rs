// db_upgrade.rs — guarded major-version upgrades of managed databases
//
// Engines can't read data files written by an older major version (or
// need a slow, engine-specific in-place conversion), so Tond upgrades by
// moving the data through a logical dump into a fresh data directory, and
// never touches the old one until the user says so:
//
//   1. Apps lose access: the container moves to a private network
//      (tond-upgrade-<name>), so nothing is written after the dump.
//   2. Backup: a normal database backup (trigger `pre_upgrade`). If it
//      fails, nothing else happens.
//   3. A temporary container of the new version starts on an empty data
//      directory (<data>/db-upgrades/<name>/new) with the same credentials;
//      the dump from step 2 is loaded into it (Redis: its dump.rdb is put in
//      place before the start).
//   4. Switch: both containers stop, the live data directory moves to
//      <data>/db-upgrades/<name>/previous and the new one takes its place;
//      the database is recreated on the new image (same name, port,
//      networks, credentials) and must pass a health check — otherwise the
//      old directory is moved back and the old version restarted.
//   5. The upgrade waits for confirmation. Confirm deletes the previous
//      directory; roll back swaps it back (losing writes made since).
//
// Any failure before the switch leaves the old version running on its
// untouched data (and reconnects it to its networks). State lives in
// `databases.upgrade` (tond_common::DbUpgradeState) so the dashboard can
// follow along; a restart in the middle marks the upgrade failed and puts
// the old version back (`recover_interrupted`).
//
// Downgrades are refused (tond_common::check_upgrade).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use tond_common::{DbUpgradeState, ManagedDatabase, check_upgrade, image_for};

use crate::backup::{self, SnapshotTrigger};
use crate::handlers::databases as dbh;
use crate::state::AppState;
use crate::{db_exec, docker, networks, secrets};

/// How long a database gets to become ready after a start.
const READY_TIMEOUT: Duration = Duration::from_secs(300);

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string()))
}

/// `<data>/db-upgrades/<name>` — working space of one database's upgrade.
pub fn upgrade_root(name: &str) -> PathBuf {
    data_dir().join("db-upgrades").join(name)
}

fn upgrade_network(name: &str) -> String {
    format!("tond-upgrade-{name}")
}

fn temp_container_name(name: &str) -> String {
    format!("tond-db-{name}-upgrade")
}

/// Remove a database's upgrade working space (the database was deleted).
pub async fn remove_upgrade_dirs(name: &str) {
    let _ = tokio::fs::remove_dir_all(upgrade_root(name)).await;
}

async fn load(state: &AppState, id: &str) -> Result<ManagedDatabase> {
    let db: ManagedDatabase = sqlx::query_as("SELECT * FROM databases WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .context("the database no longer exists")?;
    secrets::open_database(db)
}

async fn save_state(state: &AppState, id: &str, upgrade: Option<&DbUpgradeState>) -> Result<()> {
    let json = upgrade.map(serde_json::to_string).transpose()?;
    sqlx::query("UPDATE databases SET upgrade = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(json)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Why `db` can't be upgraded to `to` right now, if it can't.
pub fn preflight(db: &ManagedDatabase, to: &str) -> Result<(), String> {
    if let Some(u) = db.upgrade_state() {
        match u.status.as_str() {
            "running" => return Err("An upgrade is already running.".to_string()),
            "awaiting_confirmation" => {
                return Err(format!(
                    "Confirm or roll back the upgrade to {} first.",
                    u.to_version
                ));
            }
            _ => {}
        }
    }
    if db.custom_image.is_some() {
        return Err(
            "This database runs a custom image. Clear the custom image first, or upgrade it by \
             changing the image yourself."
                .to_string(),
        );
    }
    if db.status != "running" || db.container_id.is_none() {
        return Err(
            "Start the database first: the upgrade copies its data while it runs.".to_string(),
        );
    }
    check_upgrade(&db.db_type, &db.variant, &db.effective_version(), to)
}

/// Start an upgrade of `db` to version `to` in the background. Returns the
/// initial state (status `running`).
pub async fn start(
    state: &AppState,
    db: ManagedDatabase,
    to: &str,
) -> Result<DbUpgradeState, String> {
    preflight(&db, to)?;
    let from = db.effective_version();
    let upgrade = DbUpgradeState {
        status: "running".to_string(),
        from_image: dbh::image_of(&db).map_err(|e| e.0.to_string())?,
        to_image: image_for(&db.db_type, &db.variant, Some(to))
            .unwrap_or_default()
            .to_string(),
        from_version: from,
        to_version: to.to_string(),
        step: "Starting".to_string(),
        backup_id: None,
        started_at: Utc::now().to_rfc3339(),
        finished_at: None,
        error: None,
        rollback_dir: None,
    };
    // Claim the upgrade atomically: two clicks can't start two upgrades.
    let claimed = sqlx::query(
        "UPDATE databases SET upgrade = ?, updated_at = datetime('now')
         WHERE id = ? AND (upgrade IS NULL OR json_extract(upgrade, '$.status') = 'failed')",
    )
    .bind(serde_json::to_string(&upgrade).map_err(|e| e.to_string())?)
    .bind(&db.id)
    .execute(&state.db)
    .await
    .map_err(|e| e.to_string())?
    .rows_affected();
    if claimed == 0 {
        return Err("An upgrade is already running.".to_string());
    }

    let worker = state.clone();
    let id = db.id.clone();
    let mut progress = upgrade.clone();
    crate::runtime::shutdown::spawn_tracked(async move {
        if let Err(e) = run(&worker, &id, &mut progress).await {
            tracing::warn!("Upgrade of database {id} failed: {e:#}");
            progress.status = "failed".to_string();
            progress.error = Some(format!("{e:#}"));
            progress.finished_at = Some(Utc::now().to_rfc3339());
            let _ = save_state(&worker, &id, Some(&progress)).await;
        }
    });
    Ok(upgrade)
}

async fn step(state: &AppState, id: &str, progress: &mut DbUpgradeState, text: &str) {
    tracing::info!("Upgrade of database {id}: {text}");
    progress.step = text.to_string();
    let _ = save_state(state, id, Some(progress)).await;
}

/// The whole guarded flow. An error before the switch leaves the old
/// version running; the caller records the failure.
async fn run(state: &AppState, id: &str, progress: &mut DbUpgradeState) -> Result<()> {
    let db = load(state, id).await?;
    let old_cid = db
        .container_id
        .clone()
        .context("the database has no container")?;
    let root = upgrade_root(&db.name);
    let staging = root.join("new");
    let net = upgrade_network(&db.name);

    // 1. Nothing new gets written: apps lose their network path to it.
    step(state, id, progress, "Disconnecting apps").await;
    docker::ensure_network(&state.docker, &net, false).await?;
    let isolate = async {
        docker::connect_network(&state.docker, &net, &old_cid, &db.name).await?;
        for n in docker::container_networks(&state.docker, &old_cid).await? {
            if n != net {
                docker::disconnect_network(&state.docker, &n, &old_cid).await?;
            }
        }
        anyhow::Ok(())
    }
    .await;

    let prepared = match isolate {
        Ok(()) => prepare_new_version(state, &db, progress, &staging, &net).await,
        Err(e) => Err(e.context("couldn't disconnect the apps")),
    };
    let temp = temp_container_name(&db.name);
    if let Err(e) = prepared {
        let _ = docker::remove_container(&state.docker, &temp).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        reconnect(state, &db).await;
        docker::remove_network(&state.docker, &net).await;
        return Err(e.context(format!(
            "nothing was changed; {} keeps running version {}",
            db.name, progress.from_version
        )));
    }

    // 4. Switch data directories and recreate on the new image.
    step(state, id, progress, "Switching to the new version").await;
    let _ = docker::stop_container(&state.docker, &temp).await;
    let _ = docker::remove_container(&state.docker, &temp).await;
    let _ = docker::stop_container(&state.docker, &old_cid).await;
    let live = PathBuf::from(dbh::volume_path_for(&db));
    let previous = root.join("previous");
    let _ = tokio::fs::remove_dir_all(&previous).await;
    tokio::fs::rename(&live, &previous)
        .await
        .context("couldn't move the old data directory aside")?;
    if let Err(e) = tokio::fs::rename(&staging, &live).await {
        let _ = tokio::fs::rename(&previous, &live).await;
        let _ = docker::start_container(&state.docker, &old_cid).await;
        reconnect(state, &db).await;
        return Err(
            anyhow::Error::from(e).context("couldn't move the new data directory into place")
        );
    }
    set_version(state, id, &progress.to_version).await?;

    let switched = async {
        let upgraded = dbh::recreate_container(state, load(state, id).await?)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.0))?;
        let cid = upgraded.container_id.clone().context("no container")?;
        db_exec::wait_ready(&state.docker, &cid, &upgraded, READY_TIMEOUT).await
    }
    .await;
    docker::remove_network(&state.docker, &net).await;

    if let Err(e) = switched {
        // Health check failed: put the old version back.
        step(
            state,
            id,
            progress,
            "The new version failed its health check; restoring the old one",
        )
        .await;
        let failed_dir = root.join("failed");
        let _ = tokio::fs::remove_dir_all(&failed_dir).await;
        let _ = tokio::fs::rename(&live, &failed_dir).await;
        tokio::fs::rename(&previous, &live)
            .await
            .context("couldn't move the old data directory back")?;
        set_version(state, id, &progress.from_version).await?;
        dbh::recreate_container(state, load(state, id).await?)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.0))?;
        let _ = tokio::fs::remove_dir_all(&failed_dir).await;
        return Err(e.context(format!(
            "version {} didn't start with the copied data, so {} runs version {} again",
            progress.to_version, db.name, progress.from_version
        )));
    }

    // 5. Keep the old directory until the user confirms.
    progress.status = "awaiting_confirmation".to_string();
    progress.step = format!(
        "Running version {}. Check your apps, then confirm or roll back.",
        progress.to_version
    );
    progress.rollback_dir = Some(previous.to_string_lossy().into_owned());
    progress.finished_at = Some(Utc::now().to_rfc3339());
    save_state(state, id, Some(progress)).await?;
    Ok(())
}

/// Steps 2 and 3: the backup, then the new version loaded from it.
async fn prepare_new_version(
    state: &AppState,
    db: &ManagedDatabase,
    progress: &mut DbUpgradeState,
    staging: &Path,
    net: &str,
) -> Result<()> {
    step(state, &db.id, progress, "Backing up the current data").await;
    let backup_id = backup::create_snapshot(
        state,
        SnapshotTrigger::PreUpgradeDatabase {
            db_name: db.name.clone(),
        },
        None,
    )
    .await
    .context("the backup before the upgrade failed")?;
    progress.backup_id = Some(backup_id.clone());
    let snapshot_dir: String = sqlx::query_scalar("SELECT local_path FROM snapshots WHERE id = ?")
        .bind(&backup_id)
        .fetch_one(&state.db)
        .await?;
    // The backup is encrypted at rest: its dump as plain files, removed when
    // this returns.
    let plain = backup::plain_database_files(&PathBuf::from(snapshot_dir), &db.name).await?;
    let dump_dir = plain.path().to_path_buf();

    step(
        state,
        &db.id,
        progress,
        &format!(
            "Starting version {} next to the current one",
            progress.to_version
        ),
    )
    .await;
    let _ = tokio::fs::remove_dir_all(staging).await;
    tokio::fs::create_dir_all(staging).await?;
    if matches!(db.db_type.as_str(), "redis" | "keydb") {
        backup::place_redis_dump(&dump_dir, staging).await?;
    }
    let mut next = db.clone();
    next.version = Some(progress.to_version.clone());
    next.tls = false;
    let spec = dbh::container_spec(&next, &staging.to_string_lossy(), &progress.to_image)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.0))?;
    let port = docker::find_free_port(&state.docker, 5400).await?;
    let temp_cid = docker::run_container_named(
        &state.docker,
        &spec.image,
        &temp_container_name(&db.name),
        spec.port,
        port,
        docker::Exposure::Loopback,
        spec.env,
        db.memory_limit,
        None,
        spec.binds,
        net,
        Some(&temp_container_name(&db.name)),
        spec.cmd,
    )
    .await
    .context("couldn't start the new version")?;
    db_exec::wait_ready(&state.docker, &temp_cid, &next, READY_TIMEOUT)
        .await
        .context("the new version didn't start")?;

    step(
        state,
        &db.id,
        progress,
        &format!("Copying the data into version {}", progress.to_version),
    )
    .await;
    backup::load_dump_into(
        &state.docker,
        &next,
        &temp_cid,
        &dump_dir,
        db.db_name.as_deref(),
    )
    .await?;
    Ok(())
}

async fn set_version(state: &AppState, id: &str, version: &str) -> Result<()> {
    sqlx::query("UPDATE databases SET version = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(version)
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Put the (old) container back on its networks.
async fn reconnect(state: &AppState, db: &ManagedDatabase) {
    if let Err(e) = networks::reconcile_database(state, db).await {
        tracing::warn!("Couldn't reconnect {} to its networks: {e:#}", db.name);
    }
}

/// Confirm an upgrade: the previous data directory is deleted.
pub async fn confirm(state: &AppState, db: &ManagedDatabase) -> Result<(), String> {
    let upgrade = awaiting(db)?;
    if let Some(dir) = &upgrade.rollback_dir {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
    remove_upgrade_dirs(&db.name).await;
    save_state(state, &db.id, None)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Roll an upgrade back: the previous data directory and version return.
/// Writes made since the upgrade are lost (they are in the new directory,
/// which is deleted).
pub async fn rollback(state: &AppState, db: &ManagedDatabase) -> Result<(), String> {
    let upgrade = awaiting(db)?;
    let previous = upgrade
        .rollback_dir
        .clone()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .ok_or_else(|| {
            "The previous data directory is gone, so the upgrade can't be rolled back.".to_string()
        })?;
    let result = async {
        if let Some(cid) = &db.container_id {
            let _ = docker::stop_container(&state.docker, cid).await;
        }
        let live = PathBuf::from(dbh::volume_path_for(db));
        let discarded = upgrade_root(&db.name).join("discarded");
        let _ = tokio::fs::remove_dir_all(&discarded).await;
        tokio::fs::rename(&live, &discarded).await?;
        tokio::fs::rename(&previous, &live).await?;
        set_version(state, &db.id, &upgrade.from_version).await?;
        dbh::recreate_container(state, load(state, &db.id).await?)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.0))?;
        let _ = tokio::fs::remove_dir_all(&discarded).await;
        remove_upgrade_dirs(&db.name).await;
        save_state(state, &db.id, None).await
    }
    .await;
    result.map_err(|e| format!("The rollback failed: {e:#}"))
}

fn awaiting(db: &ManagedDatabase) -> Result<DbUpgradeState, String> {
    db.upgrade_state()
        .filter(|u| u.status == "awaiting_confirmation")
        .ok_or_else(|| "No upgrade is waiting for confirmation.".to_string())
}

/// Clear a failed upgrade's state (the banner is dismissed).
pub async fn dismiss(state: &AppState, db: &ManagedDatabase) -> Result<(), String> {
    if db.upgrade_state().is_some_and(|u| u.status == "failed") {
        save_state(state, &db.id, None)
            .await
            .map_err(|e| format!("{e:#}"))?;
    }
    Ok(())
}

/// At startup: upgrades cut off by a restart are marked failed, and the old
/// version is put back if the switch hadn't completed.
pub async fn recover_interrupted(state: &AppState) {
    let rows: Vec<ManagedDatabase> = match sqlx::query_as(
        "SELECT * FROM databases WHERE json_extract(upgrade, '$.status') = 'running'",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Couldn't check for interrupted database upgrades: {e:#}");
            return;
        }
    };
    for row in rows {
        let Ok(db) = secrets::open_database(row) else {
            continue;
        };
        let Some(mut upgrade) = db.upgrade_state() else {
            continue;
        };
        let _ = docker::remove_container(&state.docker, &temp_container_name(&db.name)).await;
        let root = upgrade_root(&db.name);
        let live = PathBuf::from(dbh::volume_path_for(&db));
        let previous = root.join("previous");
        // Switched halfway (live moved aside, new not in place): move it back.
        if !live.exists() && previous.exists() {
            let _ = tokio::fs::rename(&previous, &live).await;
        }
        let _ = tokio::fs::remove_dir_all(root.join("new")).await;
        let _ = set_version(state, &db.id, &upgrade.from_version).await;
        if let Ok(fresh) = load(state, &db.id).await {
            let _ = dbh::recreate_container(state, fresh).await;
        }
        docker::remove_network(&state.docker, &upgrade_network(&db.name)).await;
        upgrade.status = "failed".to_string();
        upgrade.error = Some(format!(
            "Tond restarted during the upgrade. {} runs version {} again; nothing was lost.",
            db.name, upgrade.from_version
        ));
        upgrade.finished_at = Some(Utc::now().to_rfc3339());
        let _ = save_state(state, &db.id, Some(&upgrade)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(db_type: &str, version: &str) -> ManagedDatabase {
        let mut db = ManagedDatabase::new("shop".to_string(), db_type.to_string(), None);
        db.version = Some(version.to_string());
        db.status = "running".to_string();
        db.container_id = Some("c1".to_string());
        db
    }

    #[test]
    fn preflight_refuses_downgrades_custom_images_and_stopped_databases() {
        assert!(preflight(&running("postgres", "16"), "17").is_ok());
        assert!(
            preflight(&running("postgres", "17"), "16").is_err_and(|e| e.contains("Downgrades"))
        );
        let mut custom = running("postgres", "16");
        custom.custom_image = Some("me/pg:16".to_string());
        assert!(preflight(&custom, "17").is_err_and(|e| e.contains("custom image")));
        let mut stopped = running("postgres", "16");
        stopped.status = "stopped".to_string();
        assert!(preflight(&stopped, "17").is_err_and(|e| e.contains("Start the database")));
        assert!(preflight(&running("keydb", "latest"), "latest").is_err());
    }

    #[test]
    fn preflight_refuses_a_second_upgrade_but_allows_retrying_a_failed_one() {
        let mut db = running("postgres", "16");
        let state = |status: &str| DbUpgradeState {
            status: status.to_string(),
            from_version: "16".to_string(),
            to_version: "17".to_string(),
            from_image: String::new(),
            to_image: String::new(),
            step: String::new(),
            backup_id: None,
            started_at: String::new(),
            finished_at: None,
            error: None,
            rollback_dir: None,
        };
        db.upgrade = serde_json::to_string(&state("running")).ok();
        assert!(preflight(&db, "17").is_err_and(|e| e.contains("already running")));
        db.upgrade = serde_json::to_string(&state("awaiting_confirmation")).ok();
        assert!(preflight(&db, "18").is_err_and(|e| e.contains("Confirm or roll back")));
        db.upgrade = serde_json::to_string(&state("failed")).ok();
        assert!(preflight(&db, "17").is_ok());
    }

    #[test]
    fn working_paths_live_outside_the_databases_directory() {
        let root = upgrade_root("shop");
        assert!(root.ends_with("db-upgrades/shop"));
        assert!(!root.to_string_lossy().contains("/databases/"));
        assert_eq!(temp_container_name("shop"), "tond-db-shop-upgrade");
        assert_eq!(upgrade_network("shop"), "tond-upgrade-shop");
    }
}
