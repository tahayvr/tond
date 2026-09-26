// runtime.rs — Tond measuring itself (GET /api/system/self)
//
// tond-server reports its own process, background loops, database and HTTP
// latency, plus the same for tond-proxy (fetched over the proxy control
// socket). Counters are cumulative since the process started so a caller
// (the dashboard, e2e/budget.sh) can compute rates from two samples.

use serde::{Deserialize, Serialize};

use crate::procstat::ProcessSample;

/// Health of one background loop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopStats {
    pub name: String,
    /// Times the loop woke up (timer or event), since start.
    pub wakeups: u64,
    /// Times the loop did work, since start.
    pub runs: u64,
    /// Runs that failed, since start.
    pub errors: u64,
    /// RFC 3339 time the last run started.
    pub last_run_at: Option<String>,
    /// Duration of the last run in milliseconds.
    pub last_duration_ms: Option<f64>,
    /// Message of the most recent failure (cleared by a later success).
    pub last_error: Option<String>,
    /// Longest scheduler lag seen (a timer firing later than asked), ms.
    pub max_lag_ms: f64,
    /// When the loop will wake next if nothing happens (RFC 3339); None
    /// while it waits only for an event.
    pub next_wake_at: Option<String>,
}

/// One statement that exceeded the slow-statement threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlowStatement {
    pub sql: String,
    pub elapsed_ms: f64,
    pub at: String,
}

/// How often one distinct statement ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementCount {
    pub sql: String,
    pub count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DbStats {
    pub size_bytes: Option<u64>,
    pub wal_bytes: Option<u64>,
    /// Statements executed by tond-server since start.
    pub queries_total: u64,
    /// Statements slower than `slow_threshold_ms`, since start.
    pub slow_total: u64,
    pub slow_threshold_ms: u64,
    /// The most recent slow statements, newest first.
    pub recent_slow: Vec<SlowStatement>,
    /// The most frequently executed statements (bounded list).
    pub top_statements: Vec<StatementCount>,
}

/// Latency histogram of one HTTP route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteLatency {
    pub method: String,
    /// Route template, e.g. `/api/apps/{id}`.
    pub route: String,
    pub count: u64,
    pub errors: u64,
    pub sum_ms: f64,
    /// Requests per bucket (non-cumulative); the last bucket is +Inf.
    pub buckets: Vec<u64>,
    /// Approximate percentiles from the buckets (upper bound of the bucket).
    pub p50_ms: f64,
    pub p95_ms: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerProcessStats {
    #[serde(flatten)]
    pub process: ProcessSample,
    /// CPU use in % of one core since the previous sample (or since start).
    pub cpu_percent: Option<f64>,
    pub uptime_secs: u64,
    /// Tokio tasks currently alive.
    pub tokio_alive_tasks: u64,
    pub tokio_workers: u64,
    /// Times Tokio worker threads parked (went idle), since start.
    pub tokio_parks: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildQueueStats {
    /// Builds allowed to run at once.
    pub build_slots: u32,
    /// Deployments currently building or deploying.
    pub active: u32,
    /// Deployments waiting.
    pub queued: u32,
}

/// What tond-proxy reports about itself over the control socket.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProxyStats {
    pub pid: u32,
    #[serde(flatten)]
    pub process: ProcessSample,
    pub uptime_secs: u64,
    /// `route_state.version` the loaded routing table corresponds to.
    pub route_version: i64,
    pub routes: u64,
    /// Times the routing table was reloaded from the database.
    pub reloads: u64,
    /// Access log lines dropped because the writer fell behind, since start.
    #[serde(default)]
    pub access_log_dropped: u64,
    pub loops: Vec<LoopStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProxySelfStats {
    /// tond-server holds a live control connection to the proxy.
    pub connected: bool,
    /// Present when connected and the proxy answered.
    pub stats: Option<ProxyStats>,
    /// Proxy CPU use in % of one core since the previous sample.
    pub cpu_percent: Option<f64>,
}

/// GET /api/system/self
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemSelfResponse {
    pub version: String,
    pub server: ServerProcessStats,
    pub loops: Vec<LoopStats>,
    pub db: DbStats,
    /// Upper bounds (ms) of the HTTP latency buckets; the last is +Inf.
    pub http_bucket_bounds_ms: Vec<f64>,
    pub http: Vec<RouteLatency>,
    pub build_queue: BuildQueueStats,
    pub proxy: ProxySelfStats,
}
