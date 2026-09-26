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
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            server_name: default_server_name(),
            dashboard_domain: String::new(),
            preview_domain: String::new(),
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
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateSettingsRequest {
    pub server_name: Option<String>,
    pub dashboard_domain: Option<String>,
    #[serde(default)]
    pub preview_domain: Option<String>,
}
