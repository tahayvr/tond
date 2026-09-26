// traffic.rs — per-route request counters (tond_common::traffic)
//
// Every route (app, preview, the dashboard, and one shared bucket for hosts
// nothing is routed for) owns one fixed-size RouteStats: a few dozen atomic
// u64s. Pingora's `logging()` hook calls `RouteStats::record` once per
// request — relaxed atomic increments only, no lock and no allocation (a
// test below checks this with a counting allocator). Memory is bounded by the
// number of routes: about 420 bytes each.
//
// tond-server reads cumulative totals over the control socket (`traffic`) on
// its 30 s metrics tick and diffs them. When a route disappears its counters
// are kept until the next read, so the last requests of a stopped app are
// still reported, and then dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tond_common::traffic::{
    LATENCY_BUCKETS, RouteKind, RouteTraffic, TrafficCounters, TrafficSnapshot, bucket_for_micros,
};

/// Counters of one route. All fields are atomics: `record` never blocks.
pub struct RouteStats {
    generation: u64,
    requests: AtomicU64,
    status: [AtomicU64; 5],
    s502: AtomicU64,
    s504: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    latency: [AtomicU64; LATENCY_BUCKETS],
    upstream: [AtomicU64; LATENCY_BUCKETS],
    /// No longer routed: dropped after the next snapshot.
    retired: AtomicBool,
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

impl RouteStats {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            requests: AtomicU64::new(0),
            status: Default::default(),
            s502: AtomicU64::new(0),
            s504: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            latency: std::array::from_fn(|_| AtomicU64::new(0)),
            upstream: std::array::from_fn(|_| AtomicU64::new(0)),
            retired: AtomicBool::new(false),
        }
    }

    /// Count one finished request. `status` 0 means no response was sent
    /// (the client went away); `upstream` is the time to the upstream's
    /// response head when an upstream was contacted.
    #[inline]
    pub fn record(
        &self,
        status: u16,
        bytes_in: u64,
        bytes_out: u64,
        total: Duration,
        upstream: Option<Duration>,
    ) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if (100..600).contains(&status) {
            self.status[usize::from(status / 100 - 1)].fetch_add(1, Ordering::Relaxed);
        }
        match status {
            502 => {
                self.s502.fetch_add(1, Ordering::Relaxed);
            }
            504 => {
                self.s504.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if bytes_in > 0 {
            self.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        }
        if bytes_out > 0 {
            self.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        }
        self.latency[bucket_for_micros(micros(total))].fetch_add(1, Ordering::Relaxed);
        if let Some(up) = upstream {
            self.upstream[bucket_for_micros(micros(up))].fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn counters(&self) -> TrafficCounters {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        TrafficCounters {
            requests: load(&self.requests),
            status: std::array::from_fn(|i| load(&self.status[i])),
            s502: load(&self.s502),
            s504: load(&self.s504),
            bytes_in: load(&self.bytes_in),
            bytes_out: load(&self.bytes_out),
            latency: std::array::from_fn(|i| load(&self.latency[i])),
            upstream: std::array::from_fn(|i| load(&self.upstream[i])),
        }
    }
}

type Key = (RouteKind, Arc<str>);

/// All routes' counters.
pub struct Registry {
    routes: Mutex<HashMap<Key, Arc<RouteStats>>>,
    dashboard: Arc<RouteStats>,
    unknown: Arc<RouteStats>,
    next_generation: AtomicU64,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            routes: Mutex::new(HashMap::new()),
            dashboard: Arc::new(RouteStats::new(1)),
            unknown: Arc::new(RouteStats::new(2)),
            next_generation: AtomicU64::new(3),
        }
    }
}

impl Registry {
    pub fn dashboard(&self) -> &Arc<RouteStats> {
        &self.dashboard
    }

    pub fn unknown(&self) -> &Arc<RouteStats> {
        &self.unknown
    }

    /// Counters for an app / preview route (created on first use; called on
    /// route reloads, never per request).
    pub fn route(&self, kind: RouteKind, id: &str) -> Arc<RouteStats> {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(stats) = routes.get(&(kind, Arc::from(id))) {
            stats.retired.store(false, Ordering::Relaxed);
            return stats.clone();
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let stats = Arc::new(RouteStats::new(generation));
        routes.insert((kind, Arc::from(id)), stats.clone());
        stats
    }

    /// After a reload: mark routes not in `live` as retired.
    pub fn retire_except(&self, live: &[(RouteKind, &str)]) {
        let routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        for ((kind, id), stats) in routes.iter() {
            let keep = live.iter().any(|(k, i)| k == kind && *i == &**id);
            if !keep {
                stats.retired.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Cumulative counters of every route; retired routes are reported one
    /// last time and forgotten.
    pub fn snapshot(&self) -> TrafficSnapshot {
        let mut out = vec![
            RouteTraffic {
                kind: RouteKind::Dashboard,
                id: None,
                generation: self.dashboard.generation,
                counters: self.dashboard.counters(),
            },
            RouteTraffic {
                kind: RouteKind::Unknown,
                id: None,
                generation: self.unknown.generation,
                counters: self.unknown.counters(),
            },
        ];
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        routes.retain(|(kind, id), stats| {
            out.push(RouteTraffic {
                kind: *kind,
                id: Some(id.to_string()),
                generation: stats.generation,
                counters: stats.counters(),
            });
            !stats.retired.load(Ordering::Relaxed)
        });
        TrafficSnapshot {
            pid: std::process::id(),
            routes: out,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::time::Instant;

    use super::*;

    /// Counts allocations made by the current thread (tests run in parallel,
    /// so a process-wide count would be noisy).
    pub struct CountingAlloc;

    thread_local! {
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static GLOBAL: CountingAlloc = CountingAlloc;

    pub fn allocations() -> u64 {
        ALLOCATIONS.with(|c| c.get())
    }

    #[test]
    fn recording_counts_classes_bytes_and_latency() {
        let s = RouteStats::new(7);
        s.record(
            200,
            10,
            100,
            Duration::from_millis(4),
            Some(Duration::from_millis(3)),
        );
        s.record(
            404,
            0,
            20,
            Duration::from_micros(500),
            Some(Duration::from_micros(400)),
        );
        s.record(502, 0, 0, Duration::from_millis(30), None);
        s.record(504, 0, 0, Duration::from_secs(60), None);
        s.record(0, 0, 0, Duration::from_millis(1), None);
        s.record(
            101,
            0,
            0,
            Duration::from_millis(1),
            Some(Duration::from_millis(1)),
        );
        let c = s.counters();
        assert_eq!(c.requests, 6);
        assert_eq!(c.status, [1, 1, 0, 1, 2]);
        assert_eq!((c.s502, c.s504), (1, 1));
        assert_eq!((c.bytes_in, c.bytes_out), (10, 120));
        assert_eq!(c.latency.iter().sum::<u64>(), 6);
        assert_eq!(c.latency[LATENCY_BUCKETS - 1], 1, "60 s lands in overflow");
        assert_eq!(c.upstream.iter().sum::<u64>(), 3);
    }

    #[test]
    fn the_hot_path_never_allocates() {
        let registry = Registry::default();
        let app = registry.route(RouteKind::App, "a1");
        let before = allocations();
        for i in 0..10_000u64 {
            let status = [200u16, 301, 404, 500, 502][(i % 5) as usize];
            app.record(
                status,
                i,
                i * 2,
                Duration::from_micros(i * 37),
                Some(Duration::from_micros(i)),
            );
            registry
                .unknown()
                .record(404, 0, 0, Duration::from_micros(10), None);
            registry
                .dashboard()
                .record(200, 0, 10, Duration::from_micros(10), None);
        }
        assert_eq!(
            allocations() - before,
            0,
            "RouteStats::record must not allocate"
        );
    }

    #[test]
    fn recording_is_cheap() {
        // Generous bound (debug build, shared CI machine): the real cost is a
        // few relaxed atomic adds, tens of nanoseconds in release.
        let s = RouteStats::new(1);
        let n = 200_000u32;
        let started = Instant::now();
        for i in 0..n {
            s.record(200, 1, 1, Duration::from_micros(u64::from(i % 5000)), None);
        }
        let per_call = started.elapsed() / n;
        assert!(
            per_call < Duration::from_micros(5),
            "record took {per_call:?}"
        );
    }

    #[test]
    fn retired_routes_are_reported_once_then_dropped() {
        let registry = Registry::default();
        let a = registry.route(RouteKind::App, "a");
        let b = registry.route(RouteKind::Preview, "p");
        a.record(200, 0, 0, Duration::ZERO, None);
        b.record(200, 0, 0, Duration::ZERO, None);
        assert!(Arc::ptr_eq(&a, &registry.route(RouteKind::App, "a")));

        registry.retire_except(&[(RouteKind::App, "a")]);
        let snap = registry.snapshot();
        assert_eq!(
            snap.routes.len(),
            4,
            "dashboard, unknown, a and the retired p"
        );
        assert!(snap.routes.iter().any(|r| r.id.as_deref() == Some("p")));
        let snap = registry.snapshot();
        assert_eq!(snap.routes.len(), 3);

        // Coming back after being dropped: a new generation, fresh counters.
        let b2 = registry.route(RouteKind::Preview, "p");
        assert_ne!(b2.generation, b.generation);
        assert_eq!(b2.counters().requests, 0);
        // Retired then re-added before a snapshot: same counters, un-retired.
        registry.retire_except(&[]);
        let a2 = registry.route(RouteKind::App, "a");
        assert!(Arc::ptr_eq(&a, &a2));
        registry.snapshot();
        assert!(
            registry
                .snapshot()
                .routes
                .iter()
                .any(|r| r.id.as_deref() == Some("a"))
        );
    }
}
