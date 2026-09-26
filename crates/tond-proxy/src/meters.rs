// meters.rs — health of tond-proxy's background loops
//
// Each loop (route sync, fallback poll, certificate loop, control socket)
// registers a Meter and reports wakeups, runs, errors, the last run's time and
// duration, and scheduler lag. Reported to tond-server over the control
// socket (`stats`), which shows them in GET /api/v1/system/self.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tond_common::LoopStats;

#[derive(Default)]
struct Details {
    last_run_at: Option<DateTime<Utc>>,
    last_duration: Option<Duration>,
    last_error: Option<String>,
    next_wake_at: Option<DateTime<Utc>>,
}

pub struct Meter {
    name: &'static str,
    wakeups: AtomicU64,
    runs: AtomicU64,
    errors: AtomicU64,
    max_lag_us: AtomicU64,
    details: Mutex<Details>,
}

static METERS: LazyLock<Mutex<Vec<Arc<Meter>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Register (or look up) the meter called `name`.
pub fn register(name: &'static str) -> Arc<Meter> {
    let mut meters = METERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = meters.iter().find(|m| m.name == name) {
        return m.clone();
    }
    let m = Arc::new(Meter {
        name,
        wakeups: AtomicU64::new(0),
        runs: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        max_lag_us: AtomicU64::new(0),
        details: Mutex::new(Details::default()),
    });
    meters.push(m.clone());
    m
}

pub fn snapshot() -> Vec<LoopStats> {
    let meters = METERS.lock().unwrap_or_else(|e| e.into_inner());
    meters.iter().map(|m| m.stats()).collect()
}

impl Meter {
    fn details(&self) -> std::sync::MutexGuard<'_, Details> {
        self.details.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Note the next planned wake-up (None: wait for an event only).
    pub fn planned(&self, sleep: Option<Duration>) {
        self.details().next_wake_at = sleep
            .and_then(|d| chrono::Duration::from_std(d).ok())
            .map(|d| Utc::now() + d);
    }

    /// Count a wake-up; `timer` is (when the sleep started, how long it was)
    /// if a timer fired, to record lag.
    pub fn woke(&self, timer: Option<(Instant, Duration)>) {
        self.wakeups.fetch_add(1, Ordering::Relaxed);
        if let Some((started, asked)) = timer {
            let lag = started.elapsed().saturating_sub(asked);
            let us = u64::try_from(lag.as_micros()).unwrap_or(u64::MAX);
            self.max_lag_us.fetch_max(us, Ordering::Relaxed);
        }
    }

    /// Record one run of `duration`, failed with `error` if given.
    pub fn ran(&self, started: Instant, error: Option<String>) {
        self.runs.fetch_add(1, Ordering::Relaxed);
        let mut d = self.details();
        d.last_run_at = Some(Utc::now());
        d.last_duration = Some(started.elapsed());
        if error.is_some() {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        d.last_error = error;
    }

    pub fn stats(&self) -> LoopStats {
        let d = self.details();
        LoopStats {
            name: self.name.to_string(),
            wakeups: self.wakeups.load(Ordering::Relaxed),
            runs: self.runs.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_run_at: d.last_run_at.map(|t| t.to_rfc3339()),
            last_duration_ms: d.last_duration.map(|t| t.as_secs_f64() * 1000.0),
            last_error: d.last_error.clone(),
            max_lag_ms: self.max_lag_us.load(Ordering::Relaxed) as f64 / 1000.0,
            next_wake_at: d.next_wake_at.map(|t| t.to_rfc3339()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_records_wakeups_runs_and_errors() {
        let m = register("test-meter");
        m.planned(Some(Duration::from_secs(60)));
        m.woke(Some((Instant::now(), Duration::ZERO)));
        m.ran(Instant::now(), None);
        m.ran(Instant::now(), Some("db locked".into()));
        let s = m.stats();
        assert_eq!((s.wakeups, s.runs, s.errors), (1, 2, 1));
        assert_eq!(s.last_error.as_deref(), Some("db locked"));
        assert!(s.next_wake_at.is_some());
        m.ran(Instant::now(), None);
        assert_eq!(m.stats().last_error, None);
        assert!(snapshot().iter().any(|l| l.name == "test-meter"));
        assert_eq!(register("test-meter").stats().runs, 3);
    }
}
