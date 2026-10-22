// disk_pressure.rs — free disk space automatically when the disk fills up
//
// The metrics collector samples the host disk on every tick (metrics/mod.rs)
// and hands the sample to `check`. When the fuller of the data directory's
// filesystem and `/` (where Docker usually keeps images) is at or above the
// `auto_prune_disk_percent` setting (default 85, 0 = off), Tond prunes, in
// order, stopping as soon as the disk is below the threshold again:
//
//   1. every app's per-deployment images beyond the kept releases (the same
//      prune that runs after each deploy, so normally a no-op) and dangling
//      images no container uses;
//   2. Docker's build cache.
//
// Volumes, kept rollback images, running images and anything else are never
// touched. A run is audit-logged (`disk_auto_prune`, actor "system") and
// remembered for the `disk_auto_prune` alert kind, which fires for an hour
// after a run. Runs are at least COOLDOWN apart, so a disk that stays full
// (data, not images) doesn't prune on every tick.
//
// No loop of its own: the collector's existing tick drives it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use bollard::query_parameters::{PruneBuildOptionsBuilder, PruneImagesOptionsBuilder};

use crate::audit_log::{AuditActor, AuditEntry, record};
use crate::metrics::host::{self, HostSnapshot};
use crate::state::AppState;

/// Minimum time between two automatic runs.
pub const COOLDOWN_SECS: i64 = 30 * 60;
/// How long the `disk_auto_prune` alert holds after a run.
pub const ALERT_HOLD_SECS: i64 = 60 * 60;

/// The latest automatic run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PruneRun {
    /// Unix seconds.
    pub at: i64,
    pub before_percent: f64,
    pub after_percent: f64,
    pub freed_bytes: i64,
}

static LAST: Mutex<Option<PruneRun>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// The latest automatic run, if any since startup.
pub fn last_run() -> Option<PruneRun> {
    LAST.lock().ok().and_then(|l| *l)
}

/// Percent used of the fuller of the data filesystem and `/`.
pub fn pressure(snapshot: &HostSnapshot) -> f64 {
    let pct = |used: u64, total: u64| {
        if total == 0 {
            0.0
        } else {
            used as f64 / total as f64 * 100.0
        }
    };
    let data = pct(snapshot.disk_used, snapshot.disk_total);
    let root = snapshot.root.as_ref().map_or(0.0, |r| pct(r.used, r.total));
    data.max(root)
}

/// Whether to prune now: on, at or above the threshold, and not within the
/// cooldown of the last run.
pub fn should_prune(percent: f64, threshold: i64, last_at: Option<i64>, now: i64) -> bool {
    threshold > 0 && percent >= threshold as f64 && last_at.is_none_or(|t| now - t >= COOLDOWN_SECS)
}

/// Called with every host sample; starts a prune in the background when the
/// disk is over the threshold.
pub async fn check(state: &AppState, snapshot: &HostSnapshot) {
    let threshold = state.settings.read().await.auto_prune_disk_percent;
    let percent = pressure(snapshot);
    if !should_prune(
        percent,
        threshold,
        last_run().map(|r| r.at),
        snapshot.collected_at,
    ) || RUNNING.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let state = state.clone();
    crate::runtime::shutdown::spawn_tracked(async move {
        let run = prune(&state, percent, threshold).await;
        if let Ok(mut l) = LAST.lock() {
            *l = Some(run);
        }
        RUNNING.store(false, Ordering::SeqCst);
        crate::alerts::kick();
    });
}

fn current_percent() -> Option<f64> {
    let data = crate::auth::secret_file::data_dir();
    let data_fs = host::fs_usage(&data)?;
    let root_fs = host::fs_usage(std::path::Path::new("/"));
    let snapshot = HostSnapshot {
        disk_used: data_fs.used,
        disk_total: data_fs.total,
        root: root_fs,
        ..HostSnapshot::default()
    };
    Some(pressure(&snapshot))
}

async fn prune(state: &AppState, before: f64, threshold: i64) -> PruneRun {
    let now = chrono::Utc::now().timestamp();
    tracing::warn!(
        "disk at {before:.0}% (threshold {threshold}%): pruning unused images and build cache"
    );
    let mut freed: i64 = 0;
    let mut steps: Vec<String> = Vec::new();

    // 1. Images beyond the kept releases, then dangling images.
    let apps: Vec<(String, String)> = sqlx::query_as("SELECT id, name FROM apps")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for (id, name) in &apps {
        crate::deploy::images::prune_app_images(state, id, name).await;
    }
    let dangling = std::collections::HashMap::from([("dangling", vec!["true"])]);
    match state
        .docker
        .prune_images(Some(
            PruneImagesOptionsBuilder::default()
                .filters(&dangling)
                .build(),
        ))
        .await
    {
        Ok(r) => {
            let bytes = r.space_reclaimed.unwrap_or(0);
            freed += bytes;
            steps.push(format!("images={bytes}"));
        }
        Err(e) => tracing::warn!("automatic prune: removing unused images failed: {e}"),
    }

    // 2. The build cache, only if that wasn't enough.
    let mid = current_percent().unwrap_or(before);
    if mid >= threshold as f64 {
        match state
            .docker
            .prune_build(Some(PruneBuildOptionsBuilder::default().all(true).build()))
            .await
        {
            Ok(r) => {
                let bytes = r.space_reclaimed.unwrap_or(0);
                freed += bytes;
                steps.push(format!("build_cache={bytes}"));
            }
            Err(e) => tracing::warn!("automatic prune: removing build cache failed: {e}"),
        }
    }
    let after = current_percent().unwrap_or(mid);

    record(
        &state.db,
        &AuditActor::system("disk pressure"),
        AuditEntry::new("disk_auto_prune")
            .kind("host")
            .detail(format!(
                "disk {before:.0}% -> {after:.0}% (threshold {threshold}%) freed={freed} {}",
                steps.join(" ")
            )),
    )
    .await;
    tracing::info!("automatic prune freed {freed} bytes; disk now at {after:.0}%");
    PruneRun {
        at: now,
        before_percent: before,
        after_percent: after,
        freed_bytes: freed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::host::FsUsage;

    #[test]
    fn prunes_over_the_threshold_at_most_once_per_cooldown() {
        assert!(should_prune(85.0, 85, None, 1_000));
        assert!(!should_prune(84.9, 85, None, 1_000));
        assert!(!should_prune(99.0, 0, None, 1_000), "0 turns it off");
        assert!(!should_prune(
            90.0,
            85,
            Some(1_000),
            1_000 + COOLDOWN_SECS - 1
        ));
        assert!(should_prune(90.0, 85, Some(1_000), 1_000 + COOLDOWN_SECS));
    }

    #[test]
    fn pressure_is_the_fuller_filesystem() {
        let mut s = HostSnapshot {
            disk_used: 50,
            disk_total: 100,
            ..HostSnapshot::default()
        };
        assert_eq!(pressure(&s), 50.0);
        s.root = Some(FsUsage {
            used: 90,
            total: 100,
            inodes_used: 0,
            inodes_total: 0,
        });
        assert_eq!(pressure(&s), 90.0, "Docker's images usually live on /");
        assert_eq!(pressure(&HostSnapshot::default()), 0.0);
    }
}
