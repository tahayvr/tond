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
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            server_name: default_server_name(),
            dashboard_domain: String::new(),
            preview_domain: String::new(),
            apps_domain: String::new(),
            upload_max_bytes: DEFAULT_UPLOAD_MAX_BYTES,
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
}
