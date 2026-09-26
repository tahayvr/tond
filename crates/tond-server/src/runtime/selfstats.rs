// runtime/selfstats.rs — assemble "Tond measuring itself"
//
// collect() builds the GET /api/system/self response from the pieces in this
// module tree (process sample, loops, DB statement counters, HTTP histograms,
// build queue) plus tond-proxy's own report over the control socket.
// prometheus() renders the same numbers in the Prometheus text format for
// /metrics (served only on the root-only Unix socket).
//
// CPU percentages are computed between consecutive calls (at least a second
// apart), so a dashboard polling every few seconds sees recent CPU use.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use tond_common::procstat::{ProcessSample, cpu_percent};
use tond_common::{
    BuildQueueStats, DbStats, ProxySelfStats, ServerProcessStats, SystemSelfResponse,
};

use super::{dbstats, http_metrics, loops};
use crate::deploy::queue;
use crate::proxy_link;

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Remember the process start (call early in main).
pub fn mark_start() {
    LazyLock::force(&STARTED);
}

/// Previous CPU sample and the last computed percentage.
#[derive(Default)]
struct CpuTracker {
    last: Option<(Instant, f64)>,
    percent: Option<f64>,
}

impl CpuTracker {
    fn update(&mut self, cpu_seconds: Option<f64>, since_start: f64) -> Option<f64> {
        let cpu = cpu_seconds?;
        let now = Instant::now();
        match self.last {
            Some((at, prev)) if now.duration_since(at).as_secs_f64() >= 1.0 => {
                let prev_sample = ProcessSample {
                    cpu_seconds: Some(prev),
                    ..Default::default()
                };
                let this = ProcessSample {
                    cpu_seconds: Some(cpu),
                    ..Default::default()
                };
                self.percent =
                    cpu_percent(&prev_sample, &this, now.duration_since(at).as_secs_f64());
                self.last = Some((now, cpu));
            }
            Some(_) => {}
            None => {
                // First sample: average since start.
                self.percent = (since_start > 0.0).then(|| cpu / since_start * 100.0);
                self.last = Some((now, cpu));
            }
        }
        self.percent
    }
}

static SERVER_CPU: LazyLock<Mutex<CpuTracker>> =
    LazyLock::new(|| Mutex::new(CpuTracker::default()));
static PROXY_CPU: LazyLock<Mutex<CpuTracker>> = LazyLock::new(|| Mutex::new(CpuTracker::default()));

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn db_path() -> String {
    std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string())
}

fn server_stats() -> ServerProcessStats {
    let process = ProcessSample::current();
    let uptime = STARTED.elapsed();
    let cpu = SERVER_CPU
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .update(process.cpu_seconds, uptime.as_secs_f64());
    let (alive, workers, parks) = match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let m = handle.metrics();
            let workers = m.num_workers();
            let parks: u64 = (0..workers).map(|w| m.worker_park_count(w)).sum();
            (m.num_alive_tasks() as u64, workers as u64, parks)
        }
        Err(_) => (0, 0, 0),
    };
    ServerProcessStats {
        process,
        cpu_percent: cpu,
        uptime_secs: uptime.as_secs(),
        tokio_alive_tasks: alive,
        tokio_workers: workers,
        tokio_parks: parks,
    }
}

fn db_stats() -> DbStats {
    let path = db_path();
    DbStats {
        size_bytes: file_size(Path::new(&path)),
        wal_bytes: file_size(Path::new(&format!("{path}-wal"))),
        queries_total: dbstats::queries_total(),
        slow_total: dbstats::slow_total(),
        slow_threshold_ms: dbstats::SLOW_THRESHOLD_MS,
        recent_slow: dbstats::recent_slow(),
        top_statements: dbstats::top_statements(25),
    }
}

/// Build the full self report.
pub async fn collect() -> SystemSelfResponse {
    let q = queue::global();
    let (queued, active) = q.counts();
    let proxy_stats = proxy_link::proxy_stats().await;
    let proxy_cpu = proxy_stats.as_ref().and_then(|s| {
        PROXY_CPU
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(s.process.cpu_seconds, s.uptime_secs as f64)
    });
    SystemSelfResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        server: server_stats(),
        loops: loops::snapshot(),
        db: db_stats(),
        http_bucket_bounds_ms: http_metrics::BUCKET_BOUNDS_MS.to_vec(),
        http: http_metrics::snapshot(),
        build_queue: BuildQueueStats {
            build_slots: q.build_slots(),
            active,
            queued,
        },
        proxy: ProxySelfStats {
            connected: proxy_link::is_connected(),
            stats: proxy_stats,
            cpu_percent: proxy_cpu,
        },
    }
}

fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn metric(out: &mut String, name: &str, help: &str, kind: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn opt(out: &mut String, name: &str, help: &str, kind: &str, value: Option<f64>) {
    if let Some(v) = value {
        metric(out, name, help, kind);
        let _ = writeln!(out, "{name} {v}");
    }
}

/// Render `report` in the Prometheus text exposition format.
pub fn prometheus(report: &SystemSelfResponse) -> String {
    let mut out = String::with_capacity(8 * 1024);
    let s = &report.server;
    opt(
        &mut out,
        "tond_process_resident_memory_bytes",
        "Resident set size of tond-server.",
        "gauge",
        s.process.rss_bytes.map(|v| v as f64),
    );
    opt(
        &mut out,
        "tond_process_cpu_seconds_total",
        "CPU time used by tond-server.",
        "counter",
        s.process.cpu_seconds,
    );
    opt(
        &mut out,
        "tond_process_open_fds",
        "Open file descriptors of tond-server.",
        "gauge",
        s.process.open_fds.map(|v| v as f64),
    );
    opt(
        &mut out,
        "tond_process_threads",
        "OS threads of tond-server.",
        "gauge",
        s.process.threads.map(|v| v as f64),
    );
    opt(
        &mut out,
        "tond_process_context_switches_total",
        "Context switches of tond-server (all threads).",
        "counter",
        s.process.context_switches.map(|v| v as f64),
    );
    opt(
        &mut out,
        "tond_uptime_seconds",
        "Seconds since tond-server started.",
        "gauge",
        Some(s.uptime_secs as f64),
    );
    opt(
        &mut out,
        "tond_tokio_alive_tasks",
        "Tokio tasks currently alive.",
        "gauge",
        Some(s.tokio_alive_tasks as f64),
    );
    opt(
        &mut out,
        "tond_tokio_worker_parks_total",
        "Times Tokio workers went idle.",
        "counter",
        Some(s.tokio_parks as f64),
    );

    metric(
        &mut out,
        "tond_loop_wakeups_total",
        "Background loop wakeups.",
        "counter",
    );
    for l in &report.loops {
        let _ = writeln!(
            out,
            "tond_loop_wakeups_total{{loop=\"{}\"}} {}",
            label(&l.name),
            l.wakeups
        );
    }
    metric(
        &mut out,
        "tond_loop_runs_total",
        "Background loop runs.",
        "counter",
    );
    for l in &report.loops {
        let _ = writeln!(
            out,
            "tond_loop_runs_total{{loop=\"{}\"}} {}",
            label(&l.name),
            l.runs
        );
    }
    metric(
        &mut out,
        "tond_loop_errors_total",
        "Background loop runs that failed.",
        "counter",
    );
    for l in &report.loops {
        let _ = writeln!(
            out,
            "tond_loop_errors_total{{loop=\"{}\"}} {}",
            label(&l.name),
            l.errors
        );
    }
    metric(
        &mut out,
        "tond_loop_last_duration_seconds",
        "Duration of the last run.",
        "gauge",
    );
    for l in &report.loops {
        if let Some(ms) = l.last_duration_ms {
            let _ = writeln!(
                out,
                "tond_loop_last_duration_seconds{{loop=\"{}\"}} {}",
                label(&l.name),
                ms / 1000.0
            );
        }
    }
    metric(
        &mut out,
        "tond_loop_max_lag_seconds",
        "Largest scheduler lag observed.",
        "gauge",
    );
    for l in &report.loops {
        let _ = writeln!(
            out,
            "tond_loop_max_lag_seconds{{loop=\"{}\"}} {}",
            label(&l.name),
            l.max_lag_ms / 1000.0
        );
    }

    let d = &report.db;
    opt(
        &mut out,
        "tond_db_size_bytes",
        "Size of tond.db.",
        "gauge",
        d.size_bytes.map(|v| v as f64),
    );
    opt(
        &mut out,
        "tond_db_wal_bytes",
        "Size of tond.db-wal.",
        "gauge",
        d.wal_bytes.map(|v| v as f64),
    );
    opt(
        &mut out,
        "tond_db_queries_total",
        "SQL statements executed by tond-server.",
        "counter",
        Some(d.queries_total as f64),
    );
    opt(
        &mut out,
        "tond_db_slow_queries_total",
        "SQL statements slower than the slow threshold.",
        "counter",
        Some(d.slow_total as f64),
    );

    metric(
        &mut out,
        "tond_http_request_duration_seconds",
        "HTTP request latency by route.",
        "histogram",
    );
    for r in &report.http {
        let labels = format!(
            "method=\"{}\",route=\"{}\"",
            label(&r.method),
            label(&r.route)
        );
        let mut cumulative = 0;
        for (i, n) in r.buckets.iter().enumerate() {
            cumulative += n;
            let le = report
                .http_bucket_bounds_ms
                .get(i)
                .map_or("+Inf".to_string(), |b| (b / 1000.0).to_string());
            let _ = writeln!(
                out,
                "tond_http_request_duration_seconds_bucket{{{labels},le=\"{le}\"}} {cumulative}"
            );
        }
        let _ = writeln!(
            out,
            "tond_http_request_duration_seconds_sum{{{labels}}} {}",
            r.sum_ms / 1000.0
        );
        let _ = writeln!(
            out,
            "tond_http_request_duration_seconds_count{{{labels}}} {}",
            r.count
        );
    }

    let b = &report.build_queue;
    opt(
        &mut out,
        "tond_build_slots",
        "Concurrent builds allowed.",
        "gauge",
        Some(f64::from(b.build_slots)),
    );
    opt(
        &mut out,
        "tond_deployments_active",
        "Deployments building or deploying.",
        "gauge",
        Some(f64::from(b.active)),
    );
    opt(
        &mut out,
        "tond_deployments_queued",
        "Deployments waiting in the queue.",
        "gauge",
        Some(f64::from(b.queued)),
    );

    opt(
        &mut out,
        "tond_proxy_connected",
        "tond-server holds a control connection to tond-proxy.",
        "gauge",
        Some(if report.proxy.connected { 1.0 } else { 0.0 }),
    );
    if let Some(p) = &report.proxy.stats {
        opt(
            &mut out,
            "tond_proxy_resident_memory_bytes",
            "Resident set size of tond-proxy.",
            "gauge",
            p.process.rss_bytes.map(|v| v as f64),
        );
        opt(
            &mut out,
            "tond_proxy_cpu_seconds_total",
            "CPU time used by tond-proxy.",
            "counter",
            p.process.cpu_seconds,
        );
        opt(
            &mut out,
            "tond_proxy_open_fds",
            "Open file descriptors of tond-proxy.",
            "gauge",
            p.process.open_fds.map(|v| v as f64),
        );
        opt(
            &mut out,
            "tond_proxy_routes",
            "Routes loaded by tond-proxy.",
            "gauge",
            Some(p.routes as f64),
        );
        opt(
            &mut out,
            "tond_proxy_route_version",
            "route_state.version loaded by tond-proxy.",
            "gauge",
            Some(p.route_version as f64),
        );
        opt(
            &mut out,
            "tond_proxy_route_reloads_total",
            "Routing table reloads.",
            "counter",
            Some(p.reloads as f64),
        );
        metric(
            &mut out,
            "tond_proxy_loop_wakeups_total",
            "tond-proxy background loop wakeups.",
            "counter",
        );
        for l in &p.loops {
            let _ = writeln!(
                out,
                "tond_proxy_loop_wakeups_total{{loop=\"{}\"}} {}",
                label(&l.name),
                l.wakeups
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tond_common::{LoopStats, RouteLatency};

    #[test]
    fn cpu_tracker_uses_the_average_first_then_deltas() {
        let mut t = CpuTracker::default();
        assert_eq!(t.update(Some(1.0), 10.0), Some(10.0));
        // Less than a second later: keeps the previous value.
        assert_eq!(t.update(Some(5.0), 10.1), Some(10.0));
        assert_eq!(t.update(None, 11.0), None);
    }

    #[test]
    fn prometheus_output_has_cumulative_buckets_and_escaped_labels() {
        let report = SystemSelfResponse {
            loops: vec![LoopStats {
                name: "health".into(),
                wakeups: 3,
                runs: 2,
                last_duration_ms: Some(12.0),
                ..Default::default()
            }],
            http_bucket_bounds_ms: vec![10.0, 100.0],
            http: vec![RouteLatency {
                method: "GET".into(),
                route: "/api/apps/{id}".into(),
                count: 3,
                errors: 0,
                sum_ms: 30.0,
                buckets: vec![1, 1, 1],
                p50_ms: 100.0,
                p95_ms: 100.0,
            }],
            ..Default::default()
        };
        let text = prometheus(&report);
        assert!(text.contains("tond_loop_wakeups_total{loop=\"health\"} 3"));
        assert!(text.contains("tond_loop_last_duration_seconds{loop=\"health\"} 0.012"));
        assert!(text.contains(
            "tond_http_request_duration_seconds_bucket{method=\"GET\",route=\"/api/apps/{id}\",le=\"0.1\"} 2"
        ));
        assert!(text.contains("le=\"+Inf\"} 3"));
        assert!(text.contains(
            "tond_http_request_duration_seconds_count{method=\"GET\",route=\"/api/apps/{id}\"} 3"
        ));
        assert!(text.contains("tond_proxy_connected 0"));
        assert_eq!(label("a\"b"), "a\\\"b");
    }

    #[tokio::test]
    async fn collect_reports_the_runtime_and_the_queue() {
        mark_start();
        let r = collect().await;
        assert!(r.server.tokio_workers >= 1);
        assert!(r.server.tokio_alive_tasks >= 1 || r.server.tokio_workers >= 1);
        assert_eq!(r.build_queue.build_slots, queue::DEFAULT_BUILD_CONCURRENCY);
        assert!(!r.proxy.connected);
        assert_eq!(
            r.http_bucket_bounds_ms.len(),
            http_metrics::BUCKET_BOUNDS_MS.len()
        );
    }
}
