// requests.rs — request metrics and access log DTOs
//
//   GET /api/apps/{id}/requests?range=       RequestMetricsResponse
//   GET /api/previews/{id}/requests?range=   RequestMetricsResponse
//   GET /api/system/requests?range=          TrafficOverviewResponse
//   GET /api/apps/{id}/access-log            AccessLogResponse
//   PUT /api/apps/{id}/access-log            AccessLogSettings

use serde::{Deserialize, Serialize};

/// Query string of the request metric endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct RequestMetricsQuery {
    /// "1h" | "6h" | "24h" | "7d" | "30d" | "90d" (default "24h")
    #[serde(default = "default_request_range")]
    pub range: String,
}

fn default_request_range() -> String {
    "24h".to_string()
}

/// Request series, one entry per bucket (parallel arrays). Rates are per
/// second averaged over the bucket; latency is in milliseconds and null for
/// buckets without requests.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSeries {
    /// Bucket start, unix seconds.
    pub t: Vec<i64>,
    pub rps: Vec<f64>,
    pub rps_1xx: Vec<f64>,
    pub rps_2xx: Vec<f64>,
    pub rps_3xx: Vec<f64>,
    pub rps_4xx: Vec<f64>,
    pub rps_5xx: Vec<f64>,
    /// Share of responses that were 5xx, percent (null without requests).
    pub err_pct: Vec<Option<f64>>,
    pub p50: Vec<Option<f64>>,
    pub p95: Vec<Option<f64>>,
    pub p99: Vec<Option<f64>>,
    /// Request / response body bytes per second.
    pub bytes_in: Vec<f64>,
    pub bytes_out: Vec<f64>,
}

/// Totals over a window.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSummary {
    pub requests: u64,
    /// Average requests per second over the window.
    pub rps: f64,
    pub s1xx: u64,
    pub s2xx: u64,
    pub s3xx: u64,
    pub s4xx: u64,
    pub s5xx: u64,
    pub s502: u64,
    pub s504: u64,
    /// 5xx share of responses, percent (null without requests).
    pub err_pct: Option<f64>,
    /// Total latency percentiles, ms.
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    /// Upstream (time to the app's response head) percentiles, ms.
    pub upstream_p50: Option<f64>,
    pub upstream_p95: Option<f64>,
    pub upstream_p99: Option<f64>,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// The last minute, from tond-server's memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestCurrent {
    /// End of the window, unix seconds.
    pub t: i64,
    /// Seconds covered.
    pub secs: i64,
    pub rps: f64,
    pub err_pct: Option<f64>,
    pub p95: Option<f64>,
}

/// Something to draw on the charts: a deploy or a container event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestMarker {
    /// Unix seconds.
    pub t: i64,
    /// "deploy" | "rollback" | "restart" | "crash" | "oom"
    pub kind: String,
    /// Deployment status for deploys ("success", "failed", …), the event
    /// detail otherwise.
    pub detail: Option<String>,
    /// Deployment id for deploys.
    pub deployment_id: Option<String>,
}

/// Response of GET /api/apps/{id}/requests and /api/previews/{id}/requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestMetricsResponse {
    pub range: String,
    /// Bucket width in seconds.
    pub step: i64,
    pub from: i64,
    pub to: i64,
    pub series: RequestSeries,
    pub summary: RequestSummary,
    pub current: Option<RequestCurrent>,
    pub markers: Vec<RequestMarker>,
    /// False when tond-proxy did not answer the last scrape (numbers may lag).
    pub proxy_connected: bool,
}

/// One app in the traffic overview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppTrafficResponse {
    pub id: String,
    pub name: String,
    pub requests: u64,
    pub rps: f64,
    pub s4xx: u64,
    pub s5xx: u64,
    pub err_pct: Option<f64>,
    pub p95: Option<f64>,
    pub bytes_out: u64,
}

/// Response of GET /api/system/requests: server-wide traffic and the apps
/// that served the most requests and errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrafficOverviewResponse {
    pub range: String,
    pub from: i64,
    pub to: i64,
    /// Every route together (apps, previews, dashboard, unknown hosts).
    pub total: RequestSummary,
    /// Requests for hosts nothing is routed for.
    pub unknown_requests: u64,
    /// Apps with requests in the window, busiest first (at most 10).
    pub busiest: Vec<AppTrafficResponse>,
    /// Apps with 5xx responses in the window, most errors first (at most 5).
    pub errors: Vec<AppTrafficResponse>,
    pub proxy_connected: bool,
}

/// Access log settings of an app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessLogSettings {
    pub enabled: bool,
    /// Also log query strings (they often carry tokens; off by default).
    #[serde(default)]
    pub include_query: bool,
}

/// Query string of GET /api/apps/{id}/access-log.
#[derive(Debug, Clone, Deserialize)]
pub struct AccessLogQuery {
    /// Most recent lines to return (default 500, at most 5000).
    pub lines: Option<usize>,
    /// Only lines containing this text (case-insensitive).
    pub q: Option<String>,
}

/// Response of GET /api/apps/{id}/access-log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessLogResponse {
    pub enabled: bool,
    pub include_query: bool,
    /// Bytes on disk (current + rotated file).
    pub size_bytes: u64,
    /// Most bytes the log may use.
    pub max_bytes: u64,
    /// Matching lines, oldest first.
    pub lines: Vec<String>,
    /// More matching lines exist than were returned.
    pub truncated: bool,
}
