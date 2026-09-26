// alerts.rs — alert rules, live alerts, history, deliveries and quiet hours
//
// A rule watches one condition (`kind`) on one or more targets (every app, one
// app, the server, every certificate, backups, every cron job). For each
// (rule, target) the engine keeps a state: inactive → pending (condition true,
// waiting `for_secs`) → firing (notified) → resolved (notified). Each firing
// episode is one history row; each notification to a channel is a delivery,
// retried with backoff when the channel fails.

use serde::{Deserialize, Serialize};

// ── Kinds ────────────────────────────────────────────────────────────────────

/// What a rule's targets are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertScope {
    /// One app (`app_id`) or every app (`app_id` = None).
    App,
    /// The server itself.
    Host,
    /// Every TLS certificate Tond manages.
    Certificate,
    /// Snapshots / backups.
    Backup,
    /// Every cron job.
    Cron,
}

/// How a value is compared with the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertComparison {
    /// value > threshold
    Above,
    /// value >= threshold
    AtLeast,
    /// value < threshold
    Below,
    /// A yes/no condition; the threshold is not used.
    None,
}

impl AlertComparison {
    pub fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            AlertComparison::Above => value > threshold,
            AlertComparison::AtLeast => value >= threshold,
            AlertComparison::Below => value < threshold,
            AlertComparison::None => value > 0.0,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            AlertComparison::Above => ">",
            AlertComparison::AtLeast => "≥",
            AlertComparison::Below => "<",
            AlertComparison::None => "",
        }
    }
}

/// Static description of one rule kind.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct AlertKindInfo {
    pub kind: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub scope: AlertScope,
    /// Unit of the threshold and value ("%", "ms", "days", "hours", "restarts", "")
    pub unit: &'static str,
    pub comparison: AlertComparison,
    /// Suggested threshold (None: the kind has no threshold)
    pub default_threshold: Option<f64>,
    /// Suggested look-back window in seconds (None: the kind has no window)
    pub default_window_secs: Option<i64>,
    pub default_for_secs: i64,
    pub default_severity: &'static str,
}

pub const ALERT_KINDS: &[AlertKindInfo] = &[
    AlertKindInfo {
        kind: "app_down",
        label: "App down",
        description: "The app is not running (it crashed or failed to start), or its public URL fails the uptime check. Apps you stopped don't count.",
        scope: AlertScope::App,
        unit: "",
        comparison: AlertComparison::None,
        default_threshold: None,
        default_window_secs: None,
        default_for_secs: 120,
        default_severity: "critical",
    },
    AlertKindInfo {
        kind: "crash_loop",
        label: "Crash loop",
        description: "The app restarted after crashing this many times within the window.",
        scope: AlertScope::App,
        unit: "restarts",
        comparison: AlertComparison::AtLeast,
        default_threshold: Some(3.0),
        default_window_secs: Some(600),
        default_for_secs: 0,
        default_severity: "critical",
    },
    AlertKindInfo {
        kind: "oom",
        label: "Out of memory",
        description: "A process in the app was killed for running out of memory within the window.",
        scope: AlertScope::App,
        unit: "",
        comparison: AlertComparison::None,
        default_threshold: None,
        default_window_secs: Some(900),
        default_for_secs: 0,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "cpu",
        label: "CPU usage",
        description: "CPU used by the app, in percent of one core.",
        scope: AlertScope::App,
        unit: "%",
        comparison: AlertComparison::Above,
        default_threshold: Some(80.0),
        default_window_secs: None,
        default_for_secs: 300,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "memory",
        label: "Memory usage",
        description: "Memory used by the app, in percent of its memory limit. Apps without a limit are skipped.",
        scope: AlertScope::App,
        unit: "%",
        comparison: AlertComparison::Above,
        default_threshold: Some(90.0),
        default_window_secs: None,
        default_for_secs: 300,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "http_5xx_rate",
        label: "5xx error rate",
        description: "Share of the app's requests that answered with a 5xx status within the window.",
        scope: AlertScope::App,
        unit: "%",
        comparison: AlertComparison::Above,
        default_threshold: Some(5.0),
        default_window_secs: Some(300),
        default_for_secs: 120,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "http_p95_latency",
        label: "Slow responses (p95)",
        description: "95th percentile response time of the app within the window.",
        scope: AlertScope::App,
        unit: "ms",
        comparison: AlertComparison::Above,
        default_threshold: Some(1000.0),
        default_window_secs: Some(300),
        default_for_secs: 300,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "deploy_failed",
        label: "Deploy failed",
        description: "The app's latest deploy failed. Resolves when a deploy succeeds.",
        scope: AlertScope::App,
        unit: "",
        comparison: AlertComparison::None,
        default_threshold: None,
        default_window_secs: None,
        default_for_secs: 0,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "disk_usage",
        label: "Disk usage",
        description: "How full the disk holding Tond's data is.",
        scope: AlertScope::Host,
        unit: "%",
        comparison: AlertComparison::Above,
        default_threshold: Some(90.0),
        default_window_secs: None,
        default_for_secs: 300,
        default_severity: "critical",
    },
    AlertKindInfo {
        kind: "disk_forecast",
        label: "Disk full forecast",
        description: "At the growth rate of the last 24 hours, the disk will be full within this many hours.",
        scope: AlertScope::Host,
        unit: "hours",
        comparison: AlertComparison::Below,
        default_threshold: Some(48.0),
        default_window_secs: None,
        default_for_secs: 900,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "cert_expiry",
        label: "Certificate expiry",
        description: "A TLS certificate expires within this many days (renewal should have happened).",
        scope: AlertScope::Certificate,
        unit: "days",
        comparison: AlertComparison::Below,
        default_threshold: Some(14.0),
        default_window_secs: None,
        default_for_secs: 0,
        default_severity: "warning",
    },
    AlertKindInfo {
        kind: "backup_failed",
        label: "Backup failed",
        description: "The latest snapshot or its upload failed. Resolves when a snapshot succeeds.",
        scope: AlertScope::Backup,
        unit: "",
        comparison: AlertComparison::None,
        default_threshold: None,
        default_window_secs: None,
        default_for_secs: 0,
        default_severity: "critical",
    },
    AlertKindInfo {
        kind: "cron_failed",
        label: "Cron job failed",
        description: "A cron job's latest run failed (after its retries). Resolves when it succeeds.",
        scope: AlertScope::Cron,
        unit: "",
        comparison: AlertComparison::None,
        default_threshold: None,
        default_window_secs: None,
        default_for_secs: 0,
        default_severity: "warning",
    },
];

/// Look up a kind by its slug.
pub fn alert_kind(kind: &str) -> Option<&'static AlertKindInfo> {
    ALERT_KINDS.iter().find(|k| k.kind == kind)
}

pub const ALERT_SEVERITIES: &[&str] = &["info", "warning", "critical"];

/// `snoozed_until` value meaning "until this alert resolves".
pub const ALERT_SNOOZE_UNTIL_RESOLVED: i64 = 253_402_300_799; // 9999-12-31

/// Default repeat interval while firing and unacknowledged (4 h).
pub const ALERT_DEFAULT_REPEAT_SECS: i64 = 4 * 3600;

/// One kind plus whether its data exists on this server.
#[derive(Debug, Clone, Serialize)]
pub struct AlertKindResponse {
    #[serde(flatten)]
    pub info: AlertKindInfo,
    pub available: bool,
    /// Why the kind can't be used yet (when `available` is false).
    pub unavailable_reason: Option<String>,
}

// ── Rules ────────────────────────────────────────────────────────────────────

/// Raw row of `alert_rules`.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub app_id: Option<String>,
    pub threshold: f64,
    pub window_secs: i64,
    pub for_secs: i64,
    pub severity: String,
    pub repeat_secs: i64,
    pub enabled: i64,
    pub snoozed_until: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// API shape of a rule.
#[derive(Debug, Serialize, Deserialize)]
pub struct AlertRuleResponse {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub threshold: f64,
    pub window_secs: i64,
    pub for_secs: i64,
    pub severity: String,
    pub repeat_secs: i64,
    pub enabled: bool,
    /// RFC 3339; the whole rule is muted until then
    pub snoozed_until: Option<String>,
    /// Linked notification channels
    pub integration_ids: Vec<String>,
    /// Targets firing right now
    pub firing: i64,
    /// Targets pending right now
    pub pending: i64,
    /// RFC 3339 of the latest episode start
    pub last_fired_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// POST /api/alerts
#[derive(Debug, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub app_id: Option<String>,
    /// Defaults to the kind's suggested threshold
    #[serde(default)]
    pub threshold: Option<f64>,
    #[serde(default)]
    pub window_secs: Option<i64>,
    #[serde(default)]
    pub for_secs: Option<i64>,
    #[serde(default)]
    pub severity: Option<String>,
    /// Defaults to 4 h; 0 = never repeat
    #[serde(default)]
    pub repeat_secs: Option<i64>,
    /// At least one channel is required
    #[serde(default)]
    pub integration_ids: Vec<String>,
}

/// PATCH /api/alerts/{id} — every field optional
#[derive(Debug, Default, Deserialize)]
pub struct UpdateAlertRuleRequest {
    pub name: Option<String>,
    pub threshold: Option<f64>,
    pub window_secs: Option<i64>,
    pub for_secs: Option<i64>,
    pub severity: Option<String>,
    pub repeat_secs: Option<i64>,
    pub enabled: Option<bool>,
    /// Replaces the linked channels (must not be empty)
    pub integration_ids: Option<Vec<String>>,
    /// Mute the whole rule until this RFC 3339 time; `""` unmutes
    pub snoozed_until: Option<String>,
}

// ── Live alerts and history ──────────────────────────────────────────────────

/// A pending or firing alert (GET /api/alerts/active).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub rule_id: String,
    pub rule_name: String,
    pub kind: String,
    pub severity: String,
    pub target: String,
    pub target_label: String,
    pub app_id: Option<String>,
    /// "pending" | "firing"
    pub state: String,
    /// RFC 3339: when the condition became true
    pub since: String,
    /// RFC 3339: when it fired (firing only)
    pub fired_at: Option<String>,
    pub value: Option<f64>,
    pub threshold: f64,
    pub unit: String,
    pub comparison: AlertComparison,
    /// History row of the firing episode
    pub episode_id: Option<String>,
    pub acknowledged_at: Option<String>,
    pub acknowledged_by: Option<String>,
    /// RFC 3339; muted until then (rule, target or episode snooze)
    pub snoozed_until: Option<String>,
    pub snoozed_until_resolved: bool,
    /// Deliveries of this episode that failed for good
    pub failed_deliveries: i64,
}

/// One firing episode (GET /api/alerts/history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEpisode {
    pub id: String,
    pub rule_id: Option<String>,
    pub rule_name: String,
    pub kind: String,
    pub severity: String,
    pub target: String,
    pub target_label: String,
    pub app_id: Option<String>,
    pub threshold: f64,
    pub unit: String,
    pub comparison: AlertComparison,
    pub value: Option<f64>,
    pub last_value: Option<f64>,
    /// "firing" | "resolved"
    pub state: String,
    pub pending_since: String,
    pub fired_at: String,
    pub resolved_at: Option<String>,
    pub acknowledged_at: Option<String>,
    pub acknowledged_by: Option<String>,
    pub notifications_sent: i64,
    pub failed_deliveries: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AlertHistoryPage {
    pub episodes: Vec<AlertEpisode>,
    /// Total matching the filter
    pub total: i64,
}

/// Query of GET /api/alerts/history
#[derive(Debug, Default, Deserialize)]
pub struct AlertHistoryQuery {
    pub rule_id: Option<String>,
    pub app_id: Option<String>,
    /// "firing" | "resolved"
    pub state: Option<String>,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub per_page: Option<i64>,
}

/// One notification to one channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertDelivery {
    pub id: String,
    pub episode_id: String,
    pub integration_id: String,
    pub integration_name: String,
    pub integration_kind: String,
    /// "firing" | "repeat" | "resolved"
    pub event: String,
    /// "pending" | "sent" | "failed" | "skipped"
    pub status: String,
    pub attempts: i64,
    /// Held back by quiet hours
    pub deferred: bool,
    pub last_error: Option<String>,
    /// RFC 3339
    pub next_attempt_at: Option<String>,
    pub sent_at: Option<String>,
    pub created_at: String,
}

/// GET /api/alerts/history/{id}
#[derive(Debug, Serialize, Deserialize)]
pub struct AlertEpisodeDetail {
    pub episode: AlertEpisode,
    pub deliveries: Vec<AlertDelivery>,
}

/// POST /api/alerts/{rule}/targets/{target}/snooze
#[derive(Debug, Deserialize)]
pub struct SnoozeAlertRequest {
    /// Mute for this many minutes …
    #[serde(default)]
    pub minutes: Option<i64>,
    /// … or until the alert resolves
    #[serde(default)]
    pub until_resolved: bool,
}

// ── Quiet hours ──────────────────────────────────────────────────────────────

/// A daily window during which notifications wait (e.g. 22:00–07:00).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    #[serde(default)]
    pub enabled: bool,
    /// "HH:MM"
    pub start: String,
    /// "HH:MM" (may be earlier than start: the window spans midnight)
    pub end: String,
    /// IANA time zone, e.g. "Europe/Berlin"
    pub timezone: String,
    /// Critical alerts are delivered anyway
    #[serde(default = "default_true")]
    pub critical_bypass: bool,
}

fn default_true() -> bool {
    true
}

impl Default for QuietHours {
    fn default() -> Self {
        Self {
            enabled: false,
            start: "22:00".to_string(),
            end: "07:00".to_string(),
            timezone: "UTC".to_string(),
            critical_bypass: true,
        }
    }
}

/// A channel's quiet hours: follow the global setting, none, or its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ChannelQuietHours {
    #[default]
    Global,
    Off,
    Custom(QuietHours),
}

/// GET/PUT /api/alerts/settings
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertSettings {
    pub quiet_hours: QuietHours,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_unique_and_has_consistent_defaults() {
        for (i, k) in ALERT_KINDS.iter().enumerate() {
            assert!(
                ALERT_KINDS[i + 1..].iter().all(|o| o.kind != k.kind),
                "duplicate kind {}",
                k.kind
            );
            assert!(ALERT_SEVERITIES.contains(&k.default_severity));
            assert_eq!(
                k.default_threshold.is_none(),
                k.comparison == AlertComparison::None,
                "{}: a threshold exactly when there is a comparison",
                k.kind
            );
        }
        assert!(alert_kind("cpu").is_some());
        assert!(alert_kind("nope").is_none());
    }

    #[test]
    fn comparisons() {
        assert!(AlertComparison::Above.holds(81.0, 80.0));
        assert!(!AlertComparison::Above.holds(80.0, 80.0));
        assert!(AlertComparison::AtLeast.holds(3.0, 3.0));
        assert!(AlertComparison::Below.holds(5.0, 14.0));
        assert!(AlertComparison::None.holds(1.0, 0.0));
        assert!(!AlertComparison::None.holds(0.0, 0.0));
    }

    #[test]
    fn channel_quiet_hours_round_trip() {
        let custom = ChannelQuietHours::Custom(QuietHours {
            enabled: true,
            ..QuietHours::default()
        });
        let json = serde_json::to_string(&custom).unwrap_or_default();
        assert!(json.contains("\"mode\":\"custom\""), "{json}");
        let back: ChannelQuietHours = serde_json::from_str(&json).unwrap_or(ChannelQuietHours::Off);
        assert_eq!(back, custom);
        let global: ChannelQuietHours =
            serde_json::from_str(r#"{"mode":"global"}"#).unwrap_or(ChannelQuietHours::Off);
        assert_eq!(global, ChannelQuietHours::Global);
    }
}
