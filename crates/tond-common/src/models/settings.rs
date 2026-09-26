// settings.rs — server-wide settings types

use serde::{Deserialize, Serialize};

// ── Server Settings ──
//
// A single flat struct that represents the full set of server-wide settings.
// Stored in the `settings` table as individual key-value rows and assembled
// here for API consumption.

fn default_server_name() -> String {
    "Tond Server".to_string()
}

/// Default cap on the unpacked size of an uploaded source archive (1 GiB).
pub const DEFAULT_UPLOAD_MAX_BYTES: u64 = 1024 * 1024 * 1024;

fn default_upload_max_bytes() -> u64 {
    DEFAULT_UPLOAD_MAX_BYTES
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    /// A human-readable name for this Tond server instance (e.g. "My VPS").
    #[serde(default = "default_server_name")]
    pub server_name: String,
    /// The domain at which the Tond dashboard itself is accessible.
    /// When set, tond-proxy will route this domain to the dashboard.
    pub dashboard_domain: String,
    /// The domain previews are served under (`<slug>-<app>.<preview_domain>`).
    /// Must be a different registrable domain from `dashboard_domain`, so
    /// preview pages are cross-site to the dashboard. Empty = previews disabled.
    #[serde(default)]
    pub preview_domain: String,
    /// The domain automatic app hostnames live under (`<app>.<apps_domain>`).
    /// Must be a different registrable domain from both `dashboard_domain`
    /// and `preview_domain`. Empty = apps get no automatic hostname and need
    /// a custom domain.
    #[serde(default)]
    pub apps_domain: String,
    /// Max unpacked size in bytes of an uploaded source archive (`tond push`).
    /// The compressed upload is capped at the same size.
    #[serde(default = "default_upload_max_bytes")]
    pub upload_max_bytes: u64,
    /// Dashboard sessions end after this many hours without a request
    /// (settings key `session_idle_timeout_hours`).
    #[serde(default = "default_session_idle_hours")]
    pub session_idle_timeout_hours: i64,
    /// Dashboard sessions end this many days after sign-in, however active
    /// (settings key `session_max_age_days`).
    #[serde(default = "default_session_max_age_days")]
    pub session_max_age_days: i64,
}

/// Default idle timeout for dashboard sessions.
pub const DEFAULT_SESSION_IDLE_HOURS: i64 = 12;
/// Default absolute lifetime of dashboard sessions.
pub const DEFAULT_SESSION_MAX_AGE_DAYS: i64 = 30;
/// Accepted range for `session_idle_timeout_hours` (1 hour … 30 days).
pub const SESSION_IDLE_HOURS_RANGE: std::ops::RangeInclusive<i64> = 1..=720;
/// Accepted range for `session_max_age_days` (1 day … 1 year).
pub const SESSION_MAX_AGE_DAYS_RANGE: std::ops::RangeInclusive<i64> = 1..=365;

fn default_session_idle_hours() -> i64 {
    DEFAULT_SESSION_IDLE_HOURS
}

fn default_session_max_age_days() -> i64 {
    DEFAULT_SESSION_MAX_AGE_DAYS
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            server_name: default_server_name(),
            dashboard_domain: String::new(),
            preview_domain: String::new(),
            apps_domain: String::new(),
            upload_max_bytes: DEFAULT_UPLOAD_MAX_BYTES,
            session_idle_timeout_hours: DEFAULT_SESSION_IDLE_HOURS,
            session_max_age_days: DEFAULT_SESSION_MAX_AGE_DAYS,
        }
    }
}

/// Raw DB row for the settings key-value table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SettingRow {
    pub key: String,
    pub value: String,
}

/// PUT /api/settings request body — all fields are optional so the caller
/// can update only the keys they care about.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateSettingsRequest {
    pub server_name: Option<String>,
    pub dashboard_domain: Option<String>,
    #[serde(default)]
    pub preview_domain: Option<String>,
    #[serde(default)]
    pub apps_domain: Option<String>,
    #[serde(default)]
    pub upload_max_bytes: Option<u64>,
    pub session_idle_timeout_hours: Option<i64>,
    #[serde(default)]
    pub session_max_age_days: Option<i64>,
}

// ── Retention ──
//
// Each retention rule is one row in the `settings` table. The janitor
// (tond-server janitor.rs) reads them every pass; the dashboard edits them
// through GET/PUT /api/settings/retention within these bounds.

/// One retention setting: its settings key, default and accepted range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetentionRule {
    pub key: &'static str,
    pub default: i64,
    pub min: i64,
    pub max: i64,
    /// What the number counts: "deploys", "days" or "runs".
    pub unit: &'static str,
}

/// Every retention setting, in display order. Defaults follow the roadmap:
/// deploy logs of the last 30 deploys per app (max 90 days), cron runs last
/// 100 per job (max 30 days), audit log 180 days, app events 90 days.
pub const RETENTION_RULES: &[RetentionRule] = &[
    RetentionRule {
        key: "retention_deploy_logs_deployments",
        default: 30,
        min: 1,
        max: 500,
        unit: "deploys",
    },
    RetentionRule {
        key: "retention_deploy_logs_days",
        default: 90,
        min: 1,
        max: 365,
        unit: "days",
    },
    RetentionRule {
        key: "retention_deployments_per_app",
        default: 50,
        min: 30,
        max: 1000,
        unit: "deploys",
    },
    RetentionRule {
        key: "retention_cron_runs_per_job",
        default: 100,
        min: 10,
        max: 1000,
        unit: "runs",
    },
    RetentionRule {
        key: "retention_cron_runs_days",
        default: 30,
        min: 1,
        max: 365,
        unit: "days",
    },
    RetentionRule {
        key: "retention_audit_log_days",
        default: 180,
        min: 30,
        max: 3650,
        unit: "days",
    },
    RetentionRule {
        key: "retention_app_events_days",
        default: 90,
        min: 7,
        max: 365,
        unit: "days",
    },
    RetentionRule {
        key: "retention_alert_history_days",
        default: 90,
        min: 7,
        max: 365,
        unit: "days",
    },
    // Hourly uptime rollups and incidents (probe-level rows: fixed 48 h).
    RetentionRule {
        key: "retention_uptime_days",
        default: 90,
        min: 7,
        max: 365,
        unit: "days",
    },
    // Hourly host history (minute rows: fixed 48 h).
    RetentionRule {
        key: "retention_host_history_days",
        default: 365,
        min: 30,
        max: 1095,
        unit: "days",
    },
];

/// The rule for a settings key.
pub fn retention_rule(key: &str) -> Option<&'static RetentionRule> {
    RETENTION_RULES.iter().find(|r| r.key == key)
}

/// A retention setting with its current value (GET /api/settings/retention).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionField {
    pub key: String,
    pub value: i64,
    pub default: i64,
    pub min: i64,
    pub max: i64,
    pub unit: String,
}

/// Size of one table (its indexes included).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSize {
    pub name: String,
    pub bytes: u64,
}

/// How big the database is and where the space goes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSize {
    /// tond.db on disk.
    pub file_bytes: u64,
    /// The write-ahead log next to it.
    pub wal_bytes: u64,
    /// Pages inside the file that are free for reuse.
    pub free_bytes: u64,
    /// Largest first. Empty when SQLite's dbstat table is unavailable.
    pub tables: Vec<TableSize>,
    /// RFC 3339 time of the measurement (it is cached for a few minutes).
    pub measured_at: String,
}

/// GET /api/settings/retention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionResponse {
    pub fields: Vec<RetentionField>,
    pub database: Option<DatabaseSize>,
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    #[test]
    fn defaults_are_inside_their_bounds_and_keys_are_unique() {
        let mut keys = std::collections::HashSet::new();
        for rule in RETENTION_RULES {
            assert!(
                rule.min <= rule.default && rule.default <= rule.max,
                "{rule:?}"
            );
            assert!(rule.key.starts_with("retention_"));
            assert!(keys.insert(rule.key), "duplicate {}", rule.key);
        }
        assert_eq!(
            retention_rule("retention_audit_log_days").map(|r| r.default),
            Some(180)
        );
    }
}
