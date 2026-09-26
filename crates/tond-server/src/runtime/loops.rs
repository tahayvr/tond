// runtime/loops.rs — health of every background loop
//
// Each background loop registers once and then reports through its handle:
//
//   let lp = loops::register("cron");
//   loop {
//       lp.wait(Some(next_deadline), &mut watch).await;   // timer or table change
//       let run = lp.run();                               // times the work
//       if let Err(e) = work().await { run.fail(&e); }
//   }
//
// Recorded per loop: wakeups, runs, errors, last run time/duration/error, the
// next scheduled wake-up, and scheduler lag (how much later than requested a
// timer actually fired — a direct measure of runtime starvation). Everything
// is atomics plus a small mutex per loop; nothing here wakes up on its own.
// Reported by GET /api/system/self and /metrics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tond_common::LoopStats;

use super::changes::Watch;

#[derive(Default)]
struct Details {
    last_run_at: Option<DateTime<Utc>>,
    last_duration: Option<Duration>,
    last_error: Option<String>,
    next_wake_at: Option<DateTime<Utc>>,
}

struct LoopState {
    name: &'static str,
    wakeups: AtomicU64,
    runs: AtomicU64,
    errors: AtomicU64,
    max_lag_us: AtomicU64,
    details: Mutex<Details>,
}

static REGISTRY: LazyLock<Mutex<Vec<Arc<LoopState>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Why a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Timer,
    Event,
}

#[derive(Clone)]
pub struct LoopHandle(Arc<LoopState>);

/// Register a loop (or return the existing one with that name).
pub fn register(name: &'static str) -> LoopHandle {
    let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = reg.iter().find(|l| l.name == name) {
        return LoopHandle(existing.clone());
    }
    let state = Arc::new(LoopState {
        name,
        wakeups: AtomicU64::new(0),
        runs: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        max_lag_us: AtomicU64::new(0),
        details: Mutex::new(Details::default()),
    });
    reg.push(state.clone());
    LoopHandle(state)
}

/// Snapshot of every registered loop, in registration order.
pub fn snapshot() -> Vec<LoopStats> {
    let reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    reg.iter().map(|l| LoopHandle(l.clone()).stats()).collect()
}

impl LoopHandle {
    fn details(&self) -> std::sync::MutexGuard<'_, Details> {
        self.0.details.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sleep for `timeout` (forever when None) or until `watch` reports a
    /// change, whichever comes first.
    pub async fn wait(&self, timeout: Option<Duration>, watch: &mut Watch) -> Wake {
        self.details().next_wake_at = timeout
            .and_then(|t| chrono::Duration::from_std(t).ok())
            .map(|t| Utc::now() + t);
        let started = Instant::now();
        let wake = match timeout {
            Some(t) => tokio::select! {
                _ = tokio::time::sleep(t) => Wake::Timer,
                _ = watch.changed() => Wake::Event,
            },
            None => {
                watch.changed().await;
                Wake::Event
            }
        };
        if let (Wake::Timer, Some(t)) = (wake, timeout) {
            self.record_lag(started.elapsed().saturating_sub(t));
        }
        self.0.wakeups.fetch_add(1, Ordering::Relaxed);
        wake
    }

    /// Plain timed sleep.
    pub async fn sleep(&self, duration: Duration) {
        self.details().next_wake_at = chrono::Duration::from_std(duration)
            .ok()
            .map(|t| Utc::now() + t);
        let started = Instant::now();
        tokio::time::sleep(duration).await;
        self.record_lag(started.elapsed().saturating_sub(duration));
        self.0.wakeups.fetch_add(1, Ordering::Relaxed);
    }

    fn record_lag(&self, lag: Duration) {
        let us = u64::try_from(lag.as_micros()).unwrap_or(u64::MAX);
        self.0.max_lag_us.fetch_max(us, Ordering::Relaxed);
    }

    /// Start timing one run of the loop's work. Dropping the guard records it.
    pub fn run(&self) -> RunGuard {
        self.details().last_run_at = Some(Utc::now());
        RunGuard {
            handle: self.clone(),
            started: Instant::now(),
            failed: false,
        }
    }

    pub fn stats(&self) -> LoopStats {
        let d = self.details();
        LoopStats {
            name: self.0.name.to_string(),
            wakeups: self.0.wakeups.load(Ordering::Relaxed),
            runs: self.0.runs.load(Ordering::Relaxed),
            errors: self.0.errors.load(Ordering::Relaxed),
            last_run_at: d.last_run_at.map(|t| t.to_rfc3339()),
            last_duration_ms: d.last_duration.map(|t| t.as_secs_f64() * 1000.0),
            last_error: d.last_error.clone(),
            max_lag_ms: self.0.max_lag_us.load(Ordering::Relaxed) as f64 / 1000.0,
            next_wake_at: d.next_wake_at.map(|t| t.to_rfc3339()),
        }
    }
}

pub struct RunGuard {
    handle: LoopHandle,
    started: Instant,
    failed: bool,
}

impl RunGuard {
    /// Mark this run as failed with `error`.
    pub fn fail(mut self, error: &dyn std::fmt::Display) {
        self.failed = true;
        self.handle.0.errors.fetch_add(1, Ordering::Relaxed);
        self.handle.details().last_error = Some(error.to_string());
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.handle.0.runs.fetch_add(1, Ordering::Relaxed);
        let mut d = self.handle.details();
        d.last_duration = Some(self.started.elapsed());
        if !self.failed {
            d.last_error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::changes::{Table, notify};

    #[tokio::test]
    async fn records_runs_errors_and_wakeups() {
        let lp = register("test-loop-a");
        lp.sleep(Duration::from_millis(1)).await;
        drop(lp.run());
        lp.run().fail(&"boom");
        let s = lp.stats();
        assert_eq!(s.name, "test-loop-a");
        assert_eq!(s.wakeups, 1);
        assert_eq!(s.runs, 2);
        assert_eq!(s.errors, 1);
        assert_eq!(s.last_error.as_deref(), Some("boom"));
        assert!(s.last_run_at.is_some() && s.last_duration_ms.is_some());

        drop(lp.run());
        assert_eq!(lp.stats().last_error, None, "a success clears the error");
        assert!(snapshot().iter().any(|l| l.name == "test-loop-a"));
        assert_eq!(
            register("test-loop-a").stats().runs,
            3,
            "same name, same loop"
        );
    }

    #[tokio::test]
    async fn wait_ends_on_timer_or_table_change() {
        let lp = register("test-loop-b");
        let mut w = Watch::new(&[Table::CronJobs]);
        assert_eq!(
            lp.wait(Some(Duration::from_millis(5)), &mut w).await,
            Wake::Timer
        );
        notify(Table::CronJobs);
        assert_eq!(
            lp.wait(Some(Duration::from_secs(5)), &mut w).await,
            Wake::Event
        );
        assert!(lp.stats().next_wake_at.is_some());
        assert_eq!(lp.stats().wakeups, 2);
    }
}
