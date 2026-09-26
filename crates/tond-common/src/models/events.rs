// events.rs — the per-app event feed (GET /api/apps/{id}/events)
//
// Assembled on request from the tables that already record each fact
// (deployments, metric_events, certificates, audit_log, alert_history);
// nothing is stored
// twice.

use serde::{Deserialize, Serialize};

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppEventKind {
    /// A deployment is queued or running.
    DeployStarted,
    DeploySucceeded,
    DeployFailed,
    /// Superseded by a newer deploy or cancelled.
    DeployCancelled,
    /// The container restarted (Docker restart policy or health check).
    Restart,
    /// The container exited with an error.
    Crash,
    /// The kernel killed the container for using too much memory.
    Oom,
    CertIssued,
    CertFailed,
    DomainChanged,
    /// Memory / CPU limits changed.
    ResourcesChanged,
    /// Other configuration (port, health check, environment, volumes, source).
    ConfigChanged,
    /// Started, stopped or restarted by someone.
    Control,
    /// An alert rule started firing for this app.
    AlertFired,
    /// A firing alert resolved.
    AlertResolved,
    /// The app's public URL stopped answering (uptime check).
    UptimeDown,
    /// The app's public URL answers again.
    UptimeUp,
}

impl AppEventKind {
    /// "info" | "success" | "warning" | "danger" — how the dashboard colours it.
    pub fn severity(self) -> &'static str {
        match self {
            Self::DeploySucceeded | Self::CertIssued | Self::AlertResolved | Self::UptimeUp => {
                "success"
            }
            Self::AlertFired | Self::UptimeDown => "danger",
            Self::DeployFailed | Self::Crash | Self::Oom | Self::CertFailed => "danger",
            Self::Restart | Self::DeployCancelled => "warning",
            _ => "info",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEvent {
    /// RFC 3339 (UTC).
    pub at: String,
    pub kind: AppEventKind,
    /// "info" | "success" | "warning" | "danger"
    pub severity: String,
    /// One line, e.g. "Deploy succeeded in 42 s".
    pub title: String,
    /// Extra context (commit message, exit code, error), if any.
    pub detail: Option<String>,
    /// Who did it (for changes made by a person or token).
    pub actor: Option<String>,
    /// The deployment this event belongs to.
    pub deployment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEventsResponse {
    /// Newest first.
    pub events: Vec<AppEvent>,
    /// Pass as `before` to get older events; None when there are no more.
    pub next_before: Option<String>,
}

// ── Deploy success-rate trend (GET /api/apps/{id}/deployments/stats) ──

/// One finished deployment in the trend window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployOutcome {
    pub id: String,
    /// "succeeded" | "failed" | "superseded" | "cancelled"
    pub status: String,
    pub started_at: String,
    pub duration_secs: Option<i64>,
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStatsResponse {
    /// How many of the latest finished deployments the numbers cover (at most 30).
    pub window: i64,
    pub succeeded: i64,
    pub failed: i64,
    /// succeeded / (succeeded + failed); None before the first result.
    pub success_rate: Option<f64>,
    /// Median wall-clock time of successful deployments.
    pub median_duration_secs: Option<i64>,
    /// Oldest first.
    pub outcomes: Vec<DeployOutcome>,
}
