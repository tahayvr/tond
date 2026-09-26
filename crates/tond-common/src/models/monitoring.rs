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

// ── AlertRule (configurable monitoring alert) ──

/// Valid event slugs for event-based alert subscriptions.
/// These are fired by the background checker or deployment/cron hooks.
///
/// Groups:
///   Deployments  → deployment_success, deployment_failure
///   Scheduled Tasks → cron_success, cron_failure
///   Containers   → container_restart (crashed and restarted, or exited),
///                  container_oom (a process was killed for running out of memory)
pub const VALID_ALERT_EVENTS: &[&str] = &[
    "deployment_success",
    "deployment_failure",
    "cron_success",
    "cron_failure",
    "container_restart",
    "container_oom",
];

/// Valid metric slugs for threshold-based alert rules.
///
/// Groups:
///   Resources → cpu, memory, app_down
///   Server    → disk_usage
pub const VALID_ALERT_METRICS: &[&str] = &["cpu", "memory", "app_down", "disk_usage"];

/// An alert rule that triggers a webhook notification when a condition is met.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AlertRule {
    pub id: String,
    /// Optional: scope this rule to a specific app (None = all apps)
    pub app_id: Option<String>,
    /// Human-readable name for this alert
    pub name: String,
    /// Metric to watch: "cpu" | "memory" | "app_down" | "disk_usage"
    /// NULL/empty means this is a pure event-based rule.
    pub metric: Option<String>,
    /// Threshold value:
    ///   - for cpu/memory/disk_usage: percentage (0–100)
    ///   - for app_down: minutes the app must be non-running before alerting
    ///
    /// Ignored when metric is NULL.
    pub threshold: f64,
    /// Webhook URL to POST to when the condition fires
    pub webhook_url: String,
    /// Whether this rule is active (1 = active, 0 = paused)
    pub enabled: i64,
    /// JSON array of event slugs: ["deployment_success", "cron_failure", …]
    /// NULL or '[]' means no event subscriptions.
    pub events: Option<String>,
    /// ISO-8601 timestamp of the last time this alert was fired
    pub last_fired_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl AlertRule {
    /// Parse the `events` JSON column into a Vec of event slug strings.
    pub fn parse_events(&self) -> Vec<String> {
        self.events
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }
}

/// API response for an alert rule (same shape, with `enabled` as bool)
#[derive(Debug, Serialize, Deserialize)]
pub struct AlertRuleResponse {
    pub id: String,
    pub app_id: Option<String>,
    pub name: String,
    pub metric: Option<String>,
    pub threshold: f64,
    pub webhook_url: String,
    pub enabled: bool,
    /// Parsed event slugs (never null in the response — empty array if none).
    pub events: Vec<String>,
    pub last_fired_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<AlertRule> for AlertRuleResponse {
    fn from(r: AlertRule) -> Self {
        let events = r.parse_events();
        Self {
            id: r.id,
            app_id: r.app_id,
            name: r.name,
            metric: r.metric,
            threshold: r.threshold,
            webhook_url: r.webhook_url,
            enabled: r.enabled != 0,
            events,
            last_fired_at: r.last_fired_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// POST /api/alerts request body
#[derive(Debug, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub app_id: Option<String>,
    pub name: String,
    /// "cpu" | "memory" | "app_down" | "disk_usage" — optional for pure event rules
    pub metric: Option<String>,
    pub threshold: Option<f64>,
    pub webhook_url: String,
    /// Event slugs to subscribe to (optional)
    pub events: Option<Vec<String>>,
}

/// PATCH /api/alerts/{id} request body — all optional
#[derive(Debug, Deserialize)]
pub struct UpdateAlertRuleRequest {
    pub name: Option<String>,
    pub metric: Option<String>,
    pub threshold: Option<f64>,
    pub webhook_url: Option<String>,
    pub enabled: Option<bool>,
    /// Replace the full events list (None = leave unchanged)
    pub events: Option<Vec<String>>,
}

// ── AlertRuleNotification (join: alert rule → integration) ──

/// Records that a named integration should be notified when an alert rule fires.
/// The `integration_id` references a future `integrations` table (not yet implemented).
/// This table is created now so the schema is ready when the Integrations page is built.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AlertRuleNotification {
    pub id: String,
    pub rule_id: String,
    /// Foreign key to the future `integrations` table.
    pub integration_id: String,
    pub created_at: String,
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

    #[test]
    fn alert_rule_parse_events_returns_empty_for_missing_or_invalid_json() {
        let valid = AlertRule {
            id: "rule-1".to_string(),
            app_id: None,
            name: "CPU high".to_string(),
            metric: Some("cpu".to_string()),
            threshold: 80.0,
            webhook_url: "https://example.com/hook".to_string(),
            enabled: 1,
            events: Some("[\"deployment_success\",\"cron_failure\"]".to_string()),
            last_fired_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(
            valid.parse_events(),
            vec!["deployment_success".to_string(), "cron_failure".to_string()],
            "AlertRule::parse_events should deserialize valid JSON arrays"
        );

        let invalid = AlertRule {
            events: Some("not json".to_string()),
            ..valid.clone()
        };
        assert_eq!(
            invalid.parse_events(),
            Vec::<String>::new(),
            "AlertRule::parse_events should fall back to an empty list for invalid JSON"
        );

        let missing = AlertRule {
            events: None,
            ..valid
        };
        assert_eq!(
            missing.parse_events(),
            Vec::<String>::new(),
            "AlertRule::parse_events should return an empty list when no events are configured"
        );
    }

    #[test]
    fn alert_rule_response_converts_enabled_flag_and_parsed_events() {
        let rule = AlertRule {
            id: "rule-1".to_string(),
            app_id: Some("app-1".to_string()),
            name: "CPU high".to_string(),
            metric: Some("cpu".to_string()),
            threshold: 80.0,
            webhook_url: "https://example.com/hook".to_string(),
            enabled: 1,
            events: Some("[\"deployment_failure\"]".to_string()),
            last_fired_at: Some("2026-01-01T01:00:00Z".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let response = AlertRuleResponse::from(rule);

        assert!(
            response.enabled,
            "AlertRuleResponse should convert non-zero enabled flags to true"
        );
        assert_eq!(
            response.events,
            vec!["deployment_failure".to_string()],
            "AlertRuleResponse should expose parsed events as a vector"
        );
    }
}
