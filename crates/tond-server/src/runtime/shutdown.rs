// runtime/shutdown.rs — graceful shutdown and in-flight work tracking
//
// On SIGTERM (systemctl stop/restart) or SIGINT tond-server:
//   1. stops accepting new work: the HTTP listeners stop accepting
//      connections, the build queue starts nothing new (queued deployments
//      stay 'queued' and are picked up again at the next start), schedulers
//      stop claiming cron runs and backups;
//   2. lets tracked work (deploys, cron runs, backups) finish for up to the
//      grace period (TOND_SHUTDOWN_GRACE_SECS, default 45 s; systemd's
//      TimeoutStopSec in tond-server.service is larger);
//   3. marks whatever is still running as interrupted (reconcile.rs) and exits.
//
// Work is tracked by spawning it through `spawn_tracked`, which also counts it
// per kind for the dashboard / metrics.

use std::future::Future;
use std::sync::LazyLock;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Shutdown state: a cancellation token plus a tracker of in-flight work.
pub struct Shutdown {
    token: CancellationToken,
    tracker: TaskTracker,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self {
            token: CancellationToken::new(),
            tracker: TaskTracker::new(),
        }
    }
}

impl Shutdown {
    pub fn is_shutting_down(&self) -> bool {
        self.token.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub fn begin(&self) {
        self.token.cancel();
    }

    pub fn spawn_tracked<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tracker.spawn(future)
    }

    pub fn in_flight(&self) -> usize {
        self.tracker.len()
    }

    /// Wait up to `grace` for tracked work to finish. True if it all did.
    pub async fn drain(&self, grace: Duration) -> bool {
        self.tracker.close();
        let n = self.tracker.len();
        if n > 0 {
            tracing::info!(
                "Waiting up to {}s for {} in-flight task(s) (deploys, cron runs, backups)",
                grace.as_secs(),
                n
            );
        }
        tokio::time::timeout(grace, self.tracker.wait())
            .await
            .is_ok()
    }
}

static GLOBAL: LazyLock<Shutdown> = LazyLock::new(Shutdown::default);

/// Default time in-flight work gets to finish after SIGTERM.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(45);

/// True once shutdown has begun: start no new work.
pub fn is_shutting_down() -> bool {
    GLOBAL.is_shutting_down()
}

/// Resolves when shutdown begins.
pub async fn cancelled() {
    GLOBAL.cancelled().await;
}

/// Begin shutdown (normally called from `wait_for_signal`).
pub fn begin() {
    GLOBAL.begin();
}

/// Spawn work that shutdown should wait for.
pub fn spawn_tracked<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    GLOBAL.spawn_tracked(future)
}

/// Number of tracked tasks still running.
pub fn in_flight() -> usize {
    GLOBAL.in_flight()
}

/// Wait up to `grace` for tracked work to finish. Returns true if it all did.
pub async fn drain(grace: Duration) -> bool {
    GLOBAL.drain(grace).await
}

/// Grace period from TOND_SHUTDOWN_GRACE_SECS.
pub fn grace_from_env() -> Duration {
    std::env::var("TOND_SHUTDOWN_GRACE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_GRACE)
}

/// Wait for SIGTERM or SIGINT, then begin shutdown.
pub async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Cannot listen for SIGTERM ({e}); graceful shutdown disabled");
            std::future::pending::<()>().await;
            return;
        }
    };
    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM received: shutting down gracefully"),
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received: shutting down gracefully"),
        _ = cancelled() => {}
    }
    begin();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tracked_work_is_awaited_and_times_out() {
        let sd = Shutdown::default();
        assert!(!sd.is_shutting_down());
        let quick = sd.spawn_tracked(async { tokio::time::sleep(Duration::from_millis(10)).await });
        let slow = sd.spawn_tracked(async { tokio::time::sleep(Duration::from_secs(30)).await });
        assert_eq!(sd.in_flight(), 2);
        sd.begin();
        assert!(sd.is_shutting_down());
        sd.cancelled().await;
        assert!(
            !sd.drain(Duration::from_millis(200)).await,
            "slow task outlives the grace"
        );
        assert!(quick.is_finished());
        slow.abort();
        let _ = slow.await;
        assert!(sd.drain(Duration::from_millis(200)).await);
        assert!(!is_shutting_down(), "the process-wide state is untouched");
    }
}
