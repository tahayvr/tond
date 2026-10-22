// restore_resume.rs — finish a full restore after the restart
//
// A full restore replaces Tond's database and exits; systemd starts
// tond-server again on the restored database. That database doesn't know a
// few things, so the restore leaves a marker file
// (<data>/backups/restore-finish.json) for the next start:
//
// Before reconcile (`before_reconcile`, startup):
//   - the snapshot that was restored and the pre-restore safety net are put
//     back into the snapshot list (the restored copy of the first one was
//     still 'running' when it was taken; the second didn't exist yet);
//   - snapshot rows whose local copy isn't on this disk (a server move) lose
//     their local path, and rows with no copy anywhere are dropped;
//   - the apps and databases that were up when the snapshot was taken are
//     remembered before reconcile marks them stopped.
//
// After the server is up (`finish`, tracked background task):
//   - tond-proxy is restarted (best-effort) so it reopens the new database
//     and loads the restored certificates;
//   - each database that was up is started, or recreated when its
//     container doesn't exist (a fresh server);
//   - each app that was up is started, or redeployed through the build queue
//     when its containers don't exist; compose apps then get their services'
//     backup dumps loaded (`tond.backup.restore` hooks);
//   - one audit entry `snapshot_restore_finished` records the outcome.
//
// The marker is removed as soon as it is read, so a crash while finishing
// never loops.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use bollard::query_parameters::InspectContainerOptions;
use serde::{Deserialize, Serialize};
use tond_common::{App, ManagedDatabase, Snapshot, SnapshotResponse};

use crate::audit_log::{AuditActor, AuditEntry, record};
use crate::deploy::{self, DeployRequest};
use crate::state::AppState;
use crate::{backup, docker};

/// What a finished full restore leaves for the next start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub snapshot: SnapshotResponse,
    pub pre_restore: Option<SnapshotResponse>,
    pub restored_at: String,
    /// Apps (id, status) that were up when the snapshot was taken; filled
    /// in at startup before reconcile rewrites their status.
    #[serde(default)]
    pub apps_up: Vec<(String, String)>,
}

fn marker_path() -> PathBuf {
    PathBuf::from(std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string()))
        .join("backups")
        .join("restore-finish.json")
}

/// Record a finished restore for the next start (root-only file).
pub fn write_marker(
    snapshot: &Snapshot,
    pre_restore: Option<&Snapshot>,
    _manifest: &serde_json::Value,
) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let marker = Marker {
        snapshot: SnapshotResponse::from(snapshot.clone()),
        pre_restore: pre_restore.cloned().map(SnapshotResponse::from),
        restored_at: chrono::Utc::now().to_rfc3339(),
        apps_up: Vec::new(),
    };
    let path = marker_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(&serde_json::to_vec_pretty(&marker)?)?;
    file.sync_all()?;
    Ok(())
}

/// Read (and remove) the marker; fix up the snapshot list. Startup, before
/// reconcile. None when no restore just finished.
pub async fn before_reconcile(state: &AppState) -> Option<Marker> {
    let path = marker_path();
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut marker: Marker = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Ignoring an unreadable restore marker: {e}");
            return None;
        }
    };
    tracing::info!(
        "Finishing the full restore of snapshot {}",
        marker.snapshot.id
    );
    if let Err(e) = fix_snapshot_rows(state, &marker).await {
        tracing::warn!("Could not update the snapshot list after the restore: {e:#}");
    }
    marker.apps_up = sqlx::query_as(
        "SELECT id, status FROM apps WHERE status IN ('running', 'unhealthy', 'building')",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    Some(marker)
}

async fn fix_snapshot_rows(state: &AppState, marker: &Marker) -> Result<()> {
    for snap in std::iter::once(&marker.snapshot).chain(marker.pre_restore.iter()) {
        upsert_snapshot(state, snap).await?;
    }
    // A server move: local copies of the old server aren't on this disk.
    let rows: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, local_path, remote_key FROM snapshots")
            .fetch_all(&state.db)
            .await?;
    for (id, local, remote) in rows {
        let present = local.as_deref().is_some_and(|p| Path::new(p).is_dir());
        if present || local.is_none() {
            continue;
        }
        if remote.is_some() {
            sqlx::query("UPDATE snapshots SET local_path = NULL WHERE id = ?")
                .bind(&id)
                .execute(&state.db)
                .await?;
        } else {
            sqlx::query("DELETE FROM snapshots WHERE id = ?")
                .bind(&id)
                .execute(&state.db)
                .await?;
        }
    }
    Ok(())
}

/// Insert or replace a snapshot row, dropping references the restored
/// database can't satisfy (a storage provider or deployment it doesn't have).
pub(crate) async fn upsert_snapshot(state: &AppState, s: &SnapshotResponse) -> Result<()> {
    let provider: Option<String> = match &s.storage_provider_id {
        Some(id) => sqlx::query_scalar("SELECT id FROM storage_providers WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?,
        None => None,
    };
    sqlx::query(
        "INSERT OR REPLACE INTO snapshots
           (id, name, status, trigger, scope, trigger_app, deployment_id, parent_snapshot_id,
            created_at, completed_at, delta_bytes, total_bytes, local_path, remote_key,
            storage_provider_id, checksum_sha256, error_message, notes, remote_status,
            remote_error, archive_bytes)
         VALUES (?, ?, 'completed', ?, ?, ?, NULL, NULL, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.name)
    .bind(&s.trigger)
    .bind(&s.scope)
    .bind(&s.trigger_app)
    .bind(&s.created_at)
    .bind(&s.completed_at)
    .bind(s.delta_bytes)
    .bind(s.total_bytes)
    .bind(&s.local_path)
    .bind(provider.as_ref().and(s.remote_key.as_ref()))
    .bind(&provider)
    .bind(&s.checksum_sha256)
    .bind(&s.notes)
    .bind(provider.as_ref().and(s.remote_status.as_ref()))
    .bind(&s.remote_error)
    .bind(s.archive_bytes)
    .execute(&state.db)
    .await
    .context("failed to record the snapshot")?;
    Ok(())
}

/// Bring the restored server up (see module docs).
pub async fn finish(state: &AppState, marker: Marker) {
    // tond-proxy still holds the replaced database file open.
    let _ = crate::process::command("systemctl")
        .args(["try-restart", "tond-proxy.service"])
        .output()
        .await;

    let mut notes: Vec<String> = Vec::new();
    let dbs: Vec<ManagedDatabase> = sqlx::query_as("SELECT * FROM databases")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let mut dbs_up = 0;
    for db in dbs.into_iter().filter(|d| d.status != "stopped") {
        match start_database(state, db.clone()).await {
            Ok(()) => dbs_up += 1,
            Err(e) => {
                tracing::warn!("Restore: database {} didn't start: {e:#}", db.name);
                notes.push(format!("database {}: {e:#}", db.name));
            }
        }
    }

    let statuses: HashMap<String, String> = marker.apps_up.iter().cloned().collect();
    let apps: Vec<App> = sqlx::query_as("SELECT * FROM apps")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let snapshot_dir = marker.snapshot.local_path.clone().map(PathBuf::from);
    let mut started = 0;
    let mut redeployed = 0;
    let mut waits = Vec::new();
    for app in apps.into_iter().filter(|a| statuses.contains_key(&a.id)) {
        match start_or_redeploy(state, &app).await {
            Ok(Resumed::Started) => {
                started += 1;
                if app.source_type == "compose"
                    && let Some(dir) = &snapshot_dir
                    && let Err(e) = backup::restore_compose_dumps(state, dir, &app.name).await
                {
                    notes.push(format!("app {}: {e:#}", app.name));
                }
            }
            Ok(Resumed::Redeploying(deployment_id)) => {
                redeployed += 1;
                if app.source_type == "compose"
                    && let Some(dir) = snapshot_dir.clone()
                {
                    let state = state.clone();
                    waits.push(tokio::spawn(async move {
                        load_dumps_after_deploy(&state, &app.name, &deployment_id, &dir).await
                    }));
                }
            }
            Err(e) => {
                tracing::warn!("Restore: app {} didn't come back: {e:#}", app.name);
                notes.push(format!("app {}: {e:#}", app.name));
            }
        }
    }
    for wait in waits {
        if let Ok(Err(e)) = wait.await {
            notes.push(e.to_string());
        }
    }

    let summary = format!(
        "snapshot={} databases_started={dbs_up} apps_started={started} apps_redeployed={redeployed}{}",
        marker.snapshot.id,
        if notes.is_empty() {
            String::new()
        } else {
            format!(" problems: {}", notes.join("; "))
        }
    );
    tracing::info!("Full restore finished: {summary}");
    record(
        &state.db,
        &AuditActor::system("restore"),
        AuditEntry::new("snapshot_restore_finished")
            .resource("snapshot", &marker.snapshot.id, &marker.snapshot.name)
            .detail(summary),
    )
    .await;
}

enum Resumed {
    Started,
    Redeploying(String),
}

async fn container_exists(state: &AppState, id: &str) -> bool {
    state
        .docker
        .inspect_container(id, None::<InspectContainerOptions>)
        .await
        .is_ok()
}

/// Start the database's container, or recreate it when it doesn't exist.
async fn start_database(state: &AppState, db: ManagedDatabase) -> Result<()> {
    if let Some(cid) = db.container_id.as_deref()
        && container_exists(state, cid).await
    {
        docker::start_container(&state.docker, cid).await?;
        return Ok(());
    }
    crate::handlers::databases::recreate_container(state, db)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.0))?;
    Ok(())
}

/// Start the app's containers, or queue a redeploy when they don't exist.
async fn start_or_redeploy(state: &AppState, app: &App) -> Result<Resumed> {
    let containers: Vec<String> = if app.source_type == "compose" {
        let project = deploy::compose_project_name(&app.name);
        crate::compose::discover(&state.docker, &project)
            .await
            .map(|v| v.into_iter().map(|s| s.container_id).collect())
            .unwrap_or_default()
    } else {
        match app.container_id.as_deref() {
            Some(cid) if container_exists(state, cid).await => vec![cid.to_string()],
            _ => Vec::new(),
        }
    };
    if !containers.is_empty() {
        for cid in &containers {
            docker::start_container(&state.docker, cid).await?;
        }
        sqlx::query("UPDATE apps SET status = 'running', updated_at = datetime('now') WHERE id = ?")
            .bind(&app.id)
            .execute(&state.db)
            .await?;
        return Ok(Resumed::Started);
    }
    let queued = deploy::start(
        state,
        app.clone(),
        DeployRequest::source("restore", Some("full restore".to_string())),
    )
    .await?;
    Ok(Resumed::Redeploying(queued.deployment_id))
}

/// Wait for a compose app's redeploy, then load its services' dumps.
async fn load_dumps_after_deploy(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
    snapshot_dir: &Path,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3 * 3600);
    loop {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM deployments WHERE id = ?")
            .bind(deployment_id)
            .fetch_optional(&state.db)
            .await?;
        match status.as_deref() {
            Some("succeeded") => break,
            Some("queued" | "running") if tokio::time::Instant::now() < deadline => {
                if crate::runtime::shutdown::is_shutting_down() {
                    anyhow::bail!("app {app_name}: Tond stopped before its redeploy finished");
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            other => anyhow::bail!(
                "app {app_name}: the redeploy ended {}; its backup dumps weren't loaded",
                other.unwrap_or("missing")
            ),
        }
    }
    backup::restore_compose_dumps(state, snapshot_dir, app_name)
        .await
        .map_err(|e| anyhow::anyhow!("app {app_name}: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state() -> AppState {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        AppState {
            db,
            docker: bollard::Docker::connect_with_http_defaults()
                .unwrap_or_else(|e| panic!("docker: {e}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(
                tond_common::ServerSettings::default(),
            )),
        }
    }

    fn snapshot(id: &str, local: Option<&str>, remote: Option<&str>) -> SnapshotResponse {
        SnapshotResponse {
            id: id.to_string(),
            name: id.to_string(),
            status: "running".to_string(),
            trigger: "manual".to_string(),
            scope: "full".to_string(),
            trigger_app: None,
            deployment_id: None,
            parent_snapshot_id: Some("gone".to_string()),
            created_at: "2026-10-01T00:00:00Z".to_string(),
            completed_at: None,
            delta_bytes: None,
            total_bytes: None,
            local_path: local.map(str::to_string),
            remote_key: remote.map(str::to_string),
            storage_provider_id: remote.map(|_| "missing-provider".to_string()),
            checksum_sha256: None,
            error_message: None,
            notes: None,
            remote_status: None,
            remote_error: None,
            archive_bytes: None,
        }
    }

    #[tokio::test]
    async fn the_snapshot_list_is_fixed_up_after_a_server_move() {
        let state = state().await;
        let here = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let here_path = here.path().to_string_lossy().into_owned();
        // Rows of the old server, as found in the restored database.
        for (id, local, remote) in [
            ("restored", Some("/old/server/restored"), None),
            ("old-local-only", Some("/old/server/x"), None),
            ("old-remote", Some("/old/server/y"), Some("snapshots/y.tondsnap")),
        ] {
            sqlx::query(
                "INSERT INTO snapshots (id, name, status, local_path, remote_key) VALUES (?, ?, 'running', ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(local)
            .bind(remote)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("seed: {e}"));
        }
        let marker = Marker {
            snapshot: snapshot("restored", Some(&here_path), None),
            pre_restore: Some(snapshot("safety-net", Some(&here_path), None)),
            restored_at: "2026-10-02T00:00:00Z".to_string(),
            apps_up: Vec::new(),
        };
        fix_snapshot_rows(&state, &marker)
            .await
            .unwrap_or_else(|e| panic!("fix: {e:#}"));

        let rows: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, status, local_path FROM snapshots ORDER BY id")
                .fetch_all(&state.db)
                .await
                .unwrap_or_else(|e| panic!("rows: {e}"));
        assert_eq!(
            rows,
            vec![
                ("old-remote".into(), "running".into(), None),
                ("restored".into(), "completed".into(), Some(here_path.clone())),
                ("safety-net".into(), "completed".into(), Some(here_path)),
            ],
            "local-only rows of the old server are gone, remote ones lose the local path"
        );
    }
}
