// monitoring.rs — metrics, alert rules, audit log types

use serde::{Deserialize, Serialize};

// ── Resource metrics (per app / database / cron job) ──

/// Query parameters for `GET /api/{apps,databases,cron}/{id}/metrics?range=`
#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    /// Time window: "1h" | "24h" | "7d" | "30d" (default "1h")
    #[serde(default = "default_range")]
    pub range: String,
}

fn default_range() -> String {
    "1h".to_string()
}

/// Downsampled time series, one entry per bucket, as parallel arrays (about
/// half the JSON of an array of objects). Rates are computed server-side.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSeries {
    /// Bucket start, unix seconds.
    pub t: Vec<i64>,
    /// Average CPU in the bucket, percent of one core (100 = one full core).
    pub cpu: Vec<f64>,
    /// Highest CPU sample in the bucket.
    pub cpu_max: Vec<f64>,
    /// Average working-set memory in bytes.
    pub mem: Vec<i64>,
    /// Highest memory sample in the bucket.
    pub mem_max: Vec<i64>,
    /// Received bytes per second.
    pub rx: Vec<i64>,
    /// Transmitted bytes per second.
    pub tx: Vec<i64>,
    /// Highest process count in the bucket.
    pub pids: Vec<i64>,
}

/// A restart, crash or OOM kill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricEventResponse {
    /// Unix seconds.
    pub t: i64,
    /// "restart" | "crash" | "oom"
    pub kind: String,
    pub detail: Option<String>,
}

/// The most recent sample (not averaged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricCurrentResponse {
    pub t: i64,
    pub cpu: f64,
    pub mem: i64,
    /// Peak memory since the container started, when the kernel reports it.
    pub mem_peak: Option<i64>,
    pub rx: i64,
    pub tx: i64,
    pub pids: i64,
}

/// Response of the metrics endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsResponse {
    pub range: String,
    /// Bucket width in seconds.
    pub step: i64,
    /// Window start / end, unix seconds.
    pub from: i64,
    pub to: i64,
    /// Memory limit in bytes (None when unlimited or never measured).
    pub mem_limit: Option<i64>,
    pub series: MetricSeries,
    pub events: Vec<MetricEventResponse>,
    pub current: Option<MetricCurrentResponse>,
}

// ── SystemMetrics (host-level snapshot, not persisted) ──

/// Point-in-time snapshot of host-level system resources, sampled by the
/// metrics collector every tick. Returned by `GET /api/system/metrics`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// Host CPU usage as a percentage of all cores (0.0 – 100.0)
    pub cpu_percent: f64,
    /// Host memory used in bytes (total − available)
    pub mem_used: i64,
    /// Total host memory in bytes
    pub mem_total: i64,
    /// Memory usage percentage
    pub mem_percent: f64,
    /// Used bytes on the filesystem holding the Tond data directory
    pub disk_used: i64,
    /// Size of that filesystem in bytes
    pub disk_total: i64,
    /// Disk usage percentage (the number the disk alert compares)
    pub disk_percent: f64,
    /// Mount point of that filesystem (usually "/")
    pub disk_mount: String,
    /// Number of running containers Tond manages (apps, databases, cron,
    /// compose services, previews)
    pub containers_running: i64,
    /// Total number of apps managed by tond
    pub apps_total: i64,
    /// Number of apps with status = "running"
    pub apps_running: i64,
    /// Timestamp of the sample (RFC 3339)
    pub collected_at: String,
}

// ── AuditLog (hash-chained security and change record) ──

/// A single entry in the append-only, hash-chained audit log.
///
/// The actor is always derived from the request's authentication (a user, an
/// API token, the root-only local socket) or is `system` for background work;
/// it is never taken from a client-supplied field.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    /// Position in the chain (1, 2, 3, … with no gaps).
    pub seq: i64,
    pub created_at: String,
    /// "user" | "token" | "local_socket" | "system" | "anonymous"
    pub actor_kind: String,
    /// User id or token id (None for local_socket/system/anonymous)
    pub actor_id: Option<String>,
    /// Human-readable actor: an email, a token name, "local-socket", …
    pub actor_label: String,
    /// Short verb, e.g. "deploy", "login", "login_failed", "token_create"
    pub action: String,
    /// "app" | "database" | "cron" | "project" | "user" | "token" | "snapshot" | "settings" | …
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    /// Name at the time of the action (denormalised for readability)
    pub resource_name: Option<String>,
    /// "success" | "failure"
    pub outcome: String,
    /// Client address the request came from (None for the socket/system)
    pub ip: Option<String>,
    /// Optional extra context
    pub detail: Option<String>,
    /// Hash of the previous entry (or the retention checkpoint)
    pub prev_hash: String,
    /// sha256(prev_hash || canonical row)
    pub hash: String,
}

/// API response for an audit log entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub seq: i64,
    pub created_at: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub actor_label: String,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub outcome: String,
    pub ip: Option<String>,
    pub detail: Option<String>,
}

impl From<AuditLog> for AuditLogResponse {
    fn from(e: AuditLog) -> Self {
        Self {
            seq: e.seq,
            created_at: e.created_at,
            actor_kind: e.actor_kind,
            actor_id: e.actor_id,
            actor_label: e.actor_label,
            action: e.action,
            resource_type: e.resource_type,
            resource_id: e.resource_id,
            resource_name: e.resource_name,
            outcome: e.outcome,
            ip: e.ip,
            detail: e.detail,
        }
    }
}

/// One page of GET /api/audit-log.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditLogPage {
    pub entries: Vec<AuditLogResponse>,
    /// Pass as `before` to get the next (older) page; None on the last page.
    pub next_before: Option<i64>,
}

/// Result of verifying the audit log's hash chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditVerifyReport {
    pub ok: bool,
    /// Entries checked.
    pub checked: i64,
    /// Entries before this sequence number were removed by retention.
    pub checkpoint_seq: i64,
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    /// The first entry that failed verification, if any.
    pub broken_at: Option<i64>,
    /// Why verification failed.
    pub problem: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_query_defaults_range_to_one_hour() {
        let query: MetricsQuery = serde_json::from_value(serde_json::json!({}))
            .unwrap_or_else(|err| panic!("failed to deserialize MetricsQuery: {err}"));
        assert_eq!(query.range, "1h");
    }
}
