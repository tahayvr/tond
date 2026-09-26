// reconcile.rs — make the database agree with reality after a restart
//
// A restart (or crash, or `systemctl stop` whose grace period ran out) can
// leave work half done. Nothing would ever finish it, so:
//
// At shutdown, after the grace period (mark_interrupted):
//   deployments 'building'/'deploying' -> 'failed' ("Tond was stopped ...")
//   snapshots   'running'/'pending'    -> 'failed'
//
// At startup (at_startup), before any listener or loop starts:
//   - the same marking, for work a crash left behind ("Tond restarted ...");
//   - deployments still 'queued' never started: they are handed back to the
//     build queue in their original order — except uploads, whose files lived
//     in the service's private /tmp and are gone;
//   - apps whose recorded status disagrees with their container (status
//     'building' with nothing deploying, 'running' with a stopped or missing
//     container, 'stopped' with a running one) get the status Docker reports;
//     an app left running in its staging container (tond-<app>-next, a deploy
//     cut off right after cutover) gets the swap finished.
//
// Cron runs are reconciled by the cron scheduler itself
// (cron::reconcile_interrupted_runs, which also removes orphaned containers).

use anyhow::Result;
use bollard::query_parameters::InspectContainerOptions;
use tond_common::App;

use crate::deploy::queue::{self, Job, JobKind};
use crate::state::AppState;

pub const RESTART_REASON: &str =
    "Interrupted: Tond restarted while this was in progress. Deploy again to retry.";
pub const SHUTDOWN_REASON: &str =
    "Interrupted: Tond was stopped before this finished. Deploy again to retry.";
const LOST_UPLOAD_REASON: &str =
    "The uploaded files were lost when Tond restarted before this deploy started. Upload again.";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub deployments_interrupted: u64,
    pub snapshots_interrupted: u64,
    pub deployments_resumed: u64,
    pub apps_corrected: u64,
}

impl Report {
    fn is_empty(&self) -> bool {
        *self == Report::default()
    }
}

/// Mark in-flight deployments and snapshots as interrupted with `reason`.
pub async fn mark_interrupted(db: &sqlx::SqlitePool, reason: &str) -> Result<Report> {
    let now = chrono::Utc::now().to_rfc3339();
    let deployments = sqlx::query(
        "UPDATE deployments SET status = 'failed', error = ?, finished_at = ?
         WHERE status IN ('building', 'deploying')",
    )
    .bind(reason)
    .bind(&now)
    .execute(db)
    .await?
    .rows_affected();
    let snapshots = sqlx::query(
        "UPDATE snapshots SET status = 'failed', error_message = ?, completed_at = ?
         WHERE status IN ('running', 'pending')",
    )
    .bind(reason)
    .bind(&now)
    .execute(db)
    .await?
    .rows_affected();
    Ok(Report {
        deployments_interrupted: deployments,
        snapshots_interrupted: snapshots,
        ..Report::default()
    })
}

/// Hand 'queued' deployments back to the build queue (oldest first).
async fn resume_queued(state: &AppState) -> Result<u64> {
    let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, app_id, kind, trigger_source, image_tag FROM deployments
         WHERE status = 'queued' ORDER BY created_at",
    )
    .fetch_all(&state.db)
    .await?;
    let mut resumed = 0;
    for (id, app_id, kind, trigger_source, image_tag) in rows {
        let kind = match (kind.as_str(), image_tag) {
            ("rollback", Some(image_tag)) => JobKind::Rollback { image_tag },
            ("deploy", _) => JobKind::Deploy,
            _ => {
                queue::finish(state, &id, "failed", Some(LOST_UPLOAD_REASON)).await?;
                continue;
            }
        };
        queue::resume(
            state,
            Job {
                deployment_id: id,
                app_id,
                kind,
                trigger_source,
            },
        )
        .await;
        resumed += 1;
    }
    Ok(resumed)
}

/// The status an app should have given what Docker reports for its
/// container, or None when the recorded one is fine.
fn corrected_status(recorded: &str, container: Option<bool>) -> Option<&'static str> {
    match (recorded, container) {
        // A deploy was cut off: trust the container.
        ("building", Some(true)) => Some("running"),
        ("building", Some(false)) => Some("stopped"),
        ("building", None) => Some("failed"),
        ("running" | "unhealthy", Some(false)) => Some("stopped"),
        ("running" | "unhealthy", None) => Some("failed"),
        ("stopped" | "failed", Some(true)) => Some("running"),
        _ => None,
    }
}

async fn reconcile_apps(state: &AppState) -> Result<u64> {
    // Apps with a deploy about to resume keep their state until it runs.
    let apps: Vec<App> = sqlx::query_as(
        "SELECT * FROM apps WHERE source_type != 'compose'
           AND id NOT IN (SELECT app_id FROM deployments WHERE status = 'queued')",
    )
    .fetch_all(&state.db)
    .await?;
    let mut corrected = 0;
    for app in apps {
        let container = match app.container_id.as_deref() {
            Some(id) => match state
                .docker
                .inspect_container(id, None::<InspectContainerOptions>)
                .await
            {
                Ok(info) => {
                    if info.name.as_deref() == Some(format!("/tond-{}-next", app.name).as_str()) {
                        promote_staging_container(state, &app.name, id).await;
                    }
                    Some(info.state.and_then(|s| s.running).unwrap_or(false))
                }
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => None,
                // Docker unreachable or another error: leave the app alone.
                Err(_) => continue,
            },
            None if app.status == "building" => None,
            None => continue,
        };
        if let Some(status) = corrected_status(&app.status, container) {
            tracing::info!(
                "App {}: status {} -> {} (container {})",
                app.name,
                app.status,
                status,
                match container {
                    Some(true) => "running",
                    Some(false) => "not running",
                    None => "missing",
                }
            );
            sqlx::query("UPDATE apps SET status = ?, updated_at = datetime('now') WHERE id = ?")
                .bind(status)
                .bind(&app.id)
                .execute(&state.db)
                .await?;
            corrected += 1;
        }
    }
    Ok(corrected)
}

/// A deploy cut off after its cutover leaves the app running in the staging
/// container (`tond-<app>-next`). The next deploy would reuse (and remove)
/// that name, so finish the swap now: drop a stale canonical container and
/// rename staging to `tond-<app>`.
async fn promote_staging_container(state: &AppState, app_name: &str, container_id: &str) {
    use bollard::query_parameters::RemoveContainerOptionsBuilder;
    let canonical = format!("tond-{app_name}");
    if let Ok(existing) = state
        .docker
        .inspect_container(&canonical, None::<InspectContainerOptions>)
        .await
        && existing.id.as_deref() != Some(container_id)
    {
        let _ = state
            .docker
            .remove_container(
                &canonical,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
    }
    match crate::docker::rename_container(
        &state.docker,
        &format!("tond-{app_name}-next"),
        &canonical,
    )
    .await
    {
        Ok(()) => tracing::info!("App {app_name}: finished an interrupted container swap"),
        Err(e) => {
            tracing::warn!("App {app_name}: could not finish an interrupted container swap: {e}")
        }
    }
}

/// Startup reconciliation (see module docs). Never fatal: failures are logged.
pub async fn at_startup(state: &AppState) -> Report {
    let mut report = match mark_interrupted(&state.db, RESTART_REASON).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Startup reconciliation (interrupted work) failed: {e:#}");
            Report::default()
        }
    };
    match reconcile_apps(state).await {
        Ok(n) => report.apps_corrected = n,
        Err(e) => tracing::warn!("Startup reconciliation (app status) failed: {e:#}"),
    }
    match resume_queued(state).await {
        Ok(n) => report.deployments_resumed = n,
        Err(e) => tracing::warn!("Startup reconciliation (queued deploys) failed: {e:#}"),
    }
    if !report.is_empty() {
        tracing::info!(
            "Startup reconciliation: {} deployment(s) and {} snapshot(s) marked interrupted, {} queued deployment(s) resumed, {} app status(es) corrected",
            report.deployments_interrupted,
            report.snapshots_interrupted,
            report.deployments_resumed,
            report.apps_corrected
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> sqlx::SqlitePool {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    async fn seed_deployment(db: &sqlx::SqlitePool, id: &str, status: &str) {
        sqlx::query(
            "INSERT INTO deployments (id, app_id, status, started_at, created_at)
             VALUES (?, 'a', ?, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(status)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("seed: {e}"));
    }

    #[tokio::test]
    async fn in_flight_work_is_marked_interrupted_and_finished_work_is_untouched() {
        let db = db().await;
        for (id, status) in [
            ("b", "building"),
            ("d", "deploying"),
            ("q", "queued"),
            ("ok", "succeeded"),
        ] {
            seed_deployment(&db, id, status).await;
        }
        sqlx::query("INSERT INTO snapshots (id, name, status) VALUES ('s1', 'x', 'running'), ('s2', 'y', 'completed')")
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("seed snapshots: {e}"));

        let report = mark_interrupted(&db, RESTART_REASON)
            .await
            .unwrap_or_else(|e| panic!("mark: {e}"));
        assert_eq!(report.deployments_interrupted, 2);
        assert_eq!(report.snapshots_interrupted, 1);

        let rows: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, status, error FROM deployments ORDER BY id")
                .fetch_all(&db)
                .await
                .unwrap_or_else(|e| panic!("rows: {e}"));
        assert_eq!(
            rows[0],
            ("b".into(), "failed".into(), Some(RESTART_REASON.into()))
        );
        assert_eq!(rows[1].1, "failed");
        assert_eq!(rows[2], ("ok".into(), "succeeded".into(), None));
        assert_eq!(
            rows[3],
            ("q".into(), "queued".into(), None),
            "queued work is resumed, not failed"
        );
        let snap: String = sqlx::query_scalar("SELECT status FROM snapshots WHERE id = 's2'")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("snap: {e}"));
        assert_eq!(snap, "completed");
    }

    #[test]
    fn app_status_follows_the_container() {
        assert_eq!(corrected_status("building", Some(true)), Some("running"));
        assert_eq!(corrected_status("building", None), Some("failed"));
        assert_eq!(corrected_status("running", Some(false)), Some("stopped"));
        assert_eq!(corrected_status("running", None), Some("failed"));
        assert_eq!(corrected_status("stopped", Some(true)), Some("running"));
        assert_eq!(corrected_status("running", Some(true)), None);
        assert_eq!(corrected_status("stopped", Some(false)), None);
        assert_eq!(corrected_status("pending", None), None);
    }
}
