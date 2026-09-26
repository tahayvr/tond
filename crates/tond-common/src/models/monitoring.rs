// monitoring.rs — metrics, alert rules, audit log types

use serde::{Deserialize, Serialize};

// ── MetricPoint (single resource sample for one app) ──

/// One 30-second resource sample stored in the `metrics` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MetricPoint {
    pub id: String,
    pub app_id: String,
    /// CPU usage as a percentage (0.0 – 100.0)
    pub cpu_percent: f64,
    /// Memory used by the container in bytes
    pub mem_used: i64,
    /// Memory limit for the container in bytes (0 = no limit)
    pub mem_limit: i64,
    /// Cumulative bytes received on all network interfaces
    pub net_rx: i64,
    /// Cumulative bytes transmitted on all network interfaces
    pub net_tx: i64,
    /// Container restart count at sample time
    pub restarts: i64,
    pub collected_at: String,
}

/// API response shape for a single metric data point.
#[derive(Debug, Serialize, Deserialize)]
pub struct MetricPointResponse {
    pub cpu_percent: f64,
    pub mem_used: i64,
    pub mem_limit: i64,
    pub mem_percent: f64,
    pub net_rx: i64,
    pub net_tx: i64,
    pub restarts: i64,
    pub collected_at: String,
}

impl From<MetricPoint> for MetricPointResponse {
    fn from(m: MetricPoint) -> Self {
        let mem_percent = if m.mem_limit > 0 {
            (m.mem_used as f64 / m.mem_limit as f64) * 100.0
        } else {
            0.0
        };
        Self {
            cpu_percent: m.cpu_percent,
            mem_used: m.mem_used,
            mem_limit: m.mem_limit,
            mem_percent,
            net_rx: m.net_rx,
            net_tx: m.net_tx,
            restarts: m.restarts,
            collected_at: m.collected_at,
        }
    }
}

/// Query parameters for `GET /api/apps/{id}/metrics?period=`
#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    /// Time window: "1h" | "24h" | "7d" (default "1h")
    #[serde(default = "default_period")]
    pub period: String,
}

fn default_period() -> String {
    "1h".to_string()
}

// ── SystemMetrics (host-level snapshot, not persisted) ──

/// Point-in-time snapshot of host-level system resources.
/// Returned by `GET /api/system/metrics`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// Host CPU usage as a percentage (0.0 – 100.0)
    pub cpu_percent: f64,
    /// Host memory used in bytes
    pub mem_used: i64,
    /// Total host memory in bytes
    pub mem_total: i64,
    /// Memory usage percentage
    pub mem_percent: f64,
    /// Host disk used in bytes
    pub disk_used: i64,
    /// Total host disk in bytes
    pub disk_total: i64,
    /// Disk usage percentage
    pub disk_percent: f64,
    /// Number of running Docker containers (all, not just tond apps)
    pub containers_running: i64,
    /// Total number of apps managed by tond
    pub apps_total: i64,
    /// Number of apps with status = "running"
    pub apps_running: i64,
    /// Timestamp of this snapshot
    pub collected_at: String,
}

// ── AlertRule (configurable monitoring alert) ──

/// Valid event slugs for event-based alert subscriptions.
/// These are fired by the background checker or deployment/cron hooks.
///
/// Groups:
///   Deployments  → deployment_success, deployment_failure
///   Scheduled Tasks → cron_success, cron_failure
pub const VALID_ALERT_EVENTS: &[&str] = &[
    "deployment_success",
    "deployment_failure",
    "cron_success",
    "cron_failure",
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
    /// Optional webhook POSTed on every firing (empty or missing = none).
    #[serde(default)]
    pub webhook_url: String,
    /// Event slugs to subscribe to (optional)
    pub events: Option<Vec<String>>,
    /// Notification channels (integration ids) to link on creation. A rule
    /// needs at least one channel or a webhook URL, so it always notifies someone.
    #[serde(default)]
    pub integration_ids: Vec<String>,
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

// ── AuditLog (state-changing operation record) ──

/// A single entry in the immutable audit log.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    pub id: String,
    /// Which app this action affected (None for server-wide actions)
    pub app_id: Option<String>,
    /// Name of the app at the time of the action (denormalised for readability)
    pub app_name: Option<String>,
    /// Short verb: "create" | "deploy" | "delete" | "stop" | "start" | "restart"
    ///   | "env_update" | "domain_update" | "health_update" | "volume_add"
    ///   | "volume_remove" | "rollback" | "settings_update"
    pub action: String,
    /// Who performed the action: "api_key:<name>" | "session:<email>" | "system"
    pub actor: String,
    /// Optional extra context (JSON blob, e.g. changed keys)
    pub detail: Option<String>,
    pub created_at: String,
}

/// API response for an audit log entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub id: String,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub action: String,
    pub actor: String,
    pub detail: Option<String>,
    pub created_at: String,
}

impl From<AuditLog> for AuditLogResponse {
    fn from(e: AuditLog) -> Self {
        Self {
            id: e.id,
            app_id: e.app_id,
            app_name: e.app_name,
            action: e.action,
            actor: e.actor,
            detail: e.detail,
            created_at: e.created_at,
        }
    }
}

/// Query parameters for `GET /api/audit-log`
#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    /// Filter by app_id (optional)
    pub app_id: Option<String>,
    /// Maximum number of rows to return (default 50, max 200)
    pub limit: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_point_response_computes_memory_percentage_and_handles_zero_limit() {
        let with_limit = MetricPoint {
            id: "metric-1".to_string(),
            app_id: "app-1".to_string(),
            cpu_percent: 12.5,
            mem_used: 50,
            mem_limit: 200,
            net_rx: 100,
            net_tx: 200,
            restarts: 1,
            collected_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let with_limit_response = MetricPointResponse::from(with_limit);
        assert_eq!(
            with_limit_response.mem_percent, 25.0,
            "MetricPointResponse should compute mem_percent from mem_used / mem_limit"
        );

        let without_limit = MetricPoint {
            id: "metric-2".to_string(),
            app_id: "app-1".to_string(),
            cpu_percent: 12.5,
            mem_used: 50,
            mem_limit: 0,
            net_rx: 100,
            net_tx: 200,
            restarts: 1,
            collected_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let without_limit_response = MetricPointResponse::from(without_limit);
        assert_eq!(
            without_limit_response.mem_percent, 0.0,
            "MetricPointResponse should report 0.0 mem_percent when no memory limit is set"
        );
    }

    #[test]
    fn metrics_query_defaults_period_to_one_hour() {
        let query: MetricsQuery = serde_json::from_value(serde_json::json!({}))
            .unwrap_or_else(|err| panic!("failed to deserialize MetricsQuery: {err}"));

        assert_eq!(
            query.period, "1h",
            "MetricsQuery should default the period to 1h"
        );
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
