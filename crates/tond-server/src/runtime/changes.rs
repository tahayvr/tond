// runtime/changes.rs — "this table changed" notifications from SQLite hooks
//
// Background loops used to wake on a fixed timer to look for work. Instead
// they now sleep until their next known deadline *or* until a table they care
// about changes. Every pooled connection of tond-server gets three SQLite
// hooks (installed by db::open_pool):
//
//   update hook    — records which watched tables a statement touched in a
//                    per-connection bit mask (rows changed by triggers count
//                    too, so the route_state triggers are seen here);
//   commit hook    — publishes the mask to the per-table watch channels;
//   rollback hook  — forgets the mask.
//
// SQLite runs the commit hook just *before* the commit becomes visible, so a
// waiter debounces briefly (SETTLE) before reading. Code that needs certainty
// (the proxy cutover) re-reads until it sees the new value.
//
// Watchers use tokio watch channels: a change that happens while a loop is
// busy is not lost, it wakes the next wait immediately.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::watch;

/// Tables whose changes wake someone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Apps,
    PreviewDeployments,
    CronJobs,
    AlertRules,
    BackupConfig,
    RouteState,
    Deployments,
    Databases,
    /// uptime_checks rows; the heartbeat settings handler notifies it too.
    Uptime,
}

impl Table {
    const ALL: [Table; 9] = [
        Table::Apps,
        Table::PreviewDeployments,
        Table::CronJobs,
        Table::AlertRules,
        Table::BackupConfig,
        Table::RouteState,
        Table::Deployments,
        Table::Databases,
        Table::Uptime,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn bit(self) -> u32 {
        1 << self.index()
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "apps" => Table::Apps,
            "preview_deployments" => Table::PreviewDeployments,
            "cron_jobs" => Table::CronJobs,
            "alert_rules" => Table::AlertRules,
            "backup_config" => Table::BackupConfig,
            "route_state" => Table::RouteState,
            "deployments" => Table::Deployments,
            "databases" => Table::Databases,
            "uptime_checks" => Table::Uptime,
            _ => return None,
        })
    }
}

/// How long a woken waiter waits for the committing transaction to land.
pub const SETTLE: Duration = Duration::from_millis(20);

static CHANNELS: LazyLock<Vec<watch::Sender<u64>>> =
    LazyLock::new(|| Table::ALL.iter().map(|_| watch::Sender::new(0)).collect());

/// Announce that `table` changed (used by the commit hook, and directly by
/// code that changes state outside the database).
pub fn notify(table: Table) {
    CHANNELS[table.index()].send_modify(|n| *n = n.wrapping_add(1));
}

fn publish(mask: u32) {
    for table in Table::ALL {
        if mask & table.bit() != 0 {
            notify(table);
        }
    }
}

/// Subscribe to changes of `table`. The first `changed()` resolves on the
/// next change after this call.
pub fn subscribe(table: Table) -> watch::Receiver<u64> {
    CHANNELS[table.index()].subscribe()
}

/// A set of subscriptions a loop waits on.
pub struct Watch {
    receivers: Vec<watch::Receiver<u64>>,
}

impl Watch {
    pub fn new(tables: &[Table]) -> Self {
        Self {
            receivers: tables.iter().map(|t| subscribe(*t)).collect(),
        }
    }

    /// Wait until one of the tables changes (then let the commit settle).
    /// With no tables this never returns.
    pub async fn changed(&mut self) {
        if self.receivers.is_empty() {
            std::future::pending::<()>().await;
        }
        let futures = self
            .receivers
            .iter_mut()
            .map(|rx| Box::pin(async move { rx.changed().await }));
        // A closed channel cannot happen (the senders are static).
        let _ = futures::future::select_all(futures).await;
        for rx in &mut self.receivers {
            rx.mark_unchanged();
        }
        tokio::time::sleep(SETTLE).await;
    }
}

/// Per-connection hook state: the tables touched by the open transaction.
#[derive(Clone, Default)]
pub struct HookState {
    mask: Arc<AtomicU32>,
}

impl HookState {
    /// Update-hook body.
    pub fn on_row_change(&self, table: &str) {
        if let Some(t) = Table::from_name(table) {
            self.mask.fetch_or(t.bit(), Ordering::Relaxed);
        }
    }

    /// Commit-hook body. Never vetoes the commit.
    pub fn on_commit(&self) -> bool {
        let mask = self.mask.swap(0, Ordering::Relaxed);
        if mask != 0 {
            publish(mask);
        }
        true
    }

    /// Rollback-hook body.
    pub fn on_rollback(&self) {
        self.mask.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commit_publishes_only_touched_watched_tables() {
        let mut apps = subscribe(Table::Apps);
        let mut cron = subscribe(Table::CronJobs);
        let hooks = HookState::default();
        hooks.on_row_change("apps");
        hooks.on_row_change("deploy_logs"); // not watched
        assert!(hooks.on_commit());
        assert!(apps.has_changed().unwrap_or(false));
        assert!(!cron.has_changed().unwrap_or(true));
        apps.mark_unchanged();

        hooks.on_row_change("cron_jobs");
        hooks.on_rollback();
        hooks.on_commit();
        assert!(
            !cron.has_changed().unwrap_or(true),
            "rolled back changes are not published"
        );
        let _ = cron.borrow_and_update();
    }

    #[tokio::test]
    async fn watch_wakes_on_any_of_its_tables_and_keeps_changes_made_while_busy() {
        let mut w = Watch::new(&[Table::AlertRules, Table::BackupConfig]);
        notify(Table::BackupConfig); // happens before we wait: not lost
        tokio::time::timeout(Duration::from_secs(1), w.changed())
            .await
            .unwrap_or_else(|_| panic!("a change made before waiting must wake the waiter"));
        let quiet = tokio::time::timeout(Duration::from_millis(50), w.changed()).await;
        assert!(quiet.is_err(), "no change, no wakeup");
    }
}
