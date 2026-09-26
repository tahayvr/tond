// audit_shipping.rs — remote copies of the audit log (syslog / webhook)

use serde::{Deserialize, Serialize};

/// Where audit entries go and how delivery is going (GET /api/v1/audit-log/shipping).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditShippingResponse {
    pub syslog: SyslogDestination,
    pub webhook: WebhookDestination,
    /// Newest audit entry (0 when the log is empty).
    pub latest_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SyslogDestination {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// RFC 5425 (TLS) when true, plain TCP otherwise.
    pub tls: bool,
    pub status: DeliveryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WebhookDestination {
    pub enabled: bool,
    pub url: String,
    /// Whether a signing secret is stored (the secret itself is never returned).
    pub has_secret: bool,
    pub status: DeliveryStatus,
}

/// Delivery progress of one destination.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeliveryStatus {
    /// Last audit entry the destination accepted.
    pub shipped_seq: i64,
    /// Entries waiting to be delivered.
    pub pending: i64,
    pub failures: i64,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub next_attempt_at: Option<String>,
    /// Entries removed by retention before they could be delivered.
    pub dropped: i64,
}

/// PUT /api/v1/audit-log/shipping. Omitted sections are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateAuditShippingRequest {
    pub syslog: Option<UpdateSyslogDestination>,
    pub webhook: Option<UpdateWebhookDestination>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateSyslogDestination {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateWebhookDestination {
    pub enabled: bool,
    pub url: String,
    /// A new signing secret; None keeps the stored one.
    #[serde(default)]
    pub secret: Option<String>,
}

/// POST /api/v1/audit-log/shipping/test body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditShippingTestRequest {
    /// "syslog" | "webhook"
    pub destination: String,
}

/// Result of sending a test entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditShippingTestResult {
    pub ok: bool,
    pub message: String,
}
