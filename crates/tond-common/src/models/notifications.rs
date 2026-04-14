// notifications.rs — Integration / notification-channel models
//
// An Integration is a named, reusable notification channel (Telegram bot,
// Discord webhook, Slack/Mattermost webhook, or SMTP email config).
// Alert rules reference integrations via the `alert_rule_notifications` table.
//
// Config JSON shapes per kind:
//   telegram  → { "bot_token": "…", "chat_id": "…", "thread_id"?: "…" }
//   discord   → { "webhook_url": "…" }
//   slack     → { "webhook_url": "…" }   (Mattermost-compatible)
//   email     → { "from_name": "…", "from_addr": "…",
//                 "smtp_host": "…", "smtp_port": 587,
//                 "smtp_user": "…", "smtp_password": "…",
//                 "smtp_encryption": "tls"|"starttls"|"none",
//                 "to_addrs": ["user@example.com"] }

use serde::{Deserialize, Serialize};

// ── Database row ──────────────────────────────────────────────────────────────

/// Raw row from the `integrations` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Integration {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// JSON blob — parsed into `IntegrationConfig` for business logic.
    pub config: String,
    pub enabled: i64,
    pub created_at: String,
    pub updated_at: String,
}

// ── Typed config variants ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
    /// Optional topic/thread ID for supergroups
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    pub webhook_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    pub webhook_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub from_name: String,
    pub from_addr: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_password: String,
    /// "tls" | "starttls" | "none"
    pub smtp_encryption: String,
    pub to_addrs: Vec<String>,
}

/// Strongly-typed config parsed from the `config` JSON column.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IntegrationConfig {
    Telegram(TelegramConfig),
    Discord(DiscordConfig),
    Slack(SlackConfig),
    Email(EmailConfig),
}

impl IntegrationConfig {
    /// Parse from the stored JSON blob + kind string.
    pub fn parse(kind: &str, config_json: &str) -> anyhow::Result<Self> {
        Ok(match kind {
            "telegram" => IntegrationConfig::Telegram(serde_json::from_str(config_json)?),
            "discord" => IntegrationConfig::Discord(serde_json::from_str(config_json)?),
            "slack" => IntegrationConfig::Slack(serde_json::from_str(config_json)?),
            "email" => IntegrationConfig::Email(serde_json::from_str(config_json)?),
            other => anyhow::bail!("unknown integration kind: {}", other),
        })
    }
}

// ── API response shape ────────────────────────────────────────────────────────

/// Outbound representation of an integration — config is returned as a typed
/// object, and `enabled` is expressed as a bool.
#[derive(Debug, Serialize, Deserialize)]
pub struct IntegrationResponse {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Typed config object (with sensitive fields intact for editing).
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Integration> for IntegrationResponse {
    fn from(i: Integration) -> Self {
        // Return the raw config JSON as a Value; let the client display it.
        let config = serde_json::from_str(&i.config).unwrap_or(serde_json::Value::Null);
        Self {
            id: i.id,
            name: i.name,
            kind: i.kind,
            config,
            enabled: i.enabled != 0,
            created_at: i.created_at,
            updated_at: i.updated_at,
        }
    }
}

// ── Request bodies ───────────────────────────────────────────────────────────

/// POST /api/integrations
#[derive(Debug, Deserialize)]
pub struct CreateIntegrationRequest {
    pub name: String,
    /// "telegram" | "discord" | "slack" | "email"
    pub kind: String,
    /// Provider-specific config object (will be validated + serialised)
    pub config: serde_json::Value,
}

/// PATCH /api/integrations/:id — all fields optional
#[derive(Debug, Deserialize)]
pub struct UpdateIntegrationRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

// ── Alert rule ↔ integration link response ────────────────────────────────────

/// Response for GET /api/alerts/:id/integrations
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct AlertIntegrationLink {
    pub id: String,
    pub rule_id: String,
    pub integration_id: String,
    pub integration_name: String,
    pub integration_kind: String,
    pub created_at: String,
}

/// POST /api/alerts/:id/integrations body
#[derive(Debug, Deserialize)]
pub struct LinkAlertIntegrationRequest {
    pub integration_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_config_parse_accepts_all_supported_kinds() {
        let telegram = IntegrationConfig::parse(
            "telegram",
            r#"{"bot_token":"token","chat_id":"chat","thread_id":"42"}"#,
        )
        .unwrap_or_else(|err| panic!("failed to parse telegram config: {err}"));
        assert!(
            matches!(telegram, IntegrationConfig::Telegram(_)),
            "telegram configs should parse into the Telegram variant"
        );

        let discord =
            IntegrationConfig::parse("discord", r#"{"webhook_url":"https://discord.example"}"#)
                .unwrap_or_else(|err| panic!("failed to parse discord config: {err}"));
        assert!(
            matches!(discord, IntegrationConfig::Discord(_)),
            "discord configs should parse into the Discord variant"
        );

        let slack = IntegrationConfig::parse("slack", r#"{"webhook_url":"https://slack.example"}"#)
            .unwrap_or_else(|err| panic!("failed to parse slack config: {err}"));
        assert!(
            matches!(slack, IntegrationConfig::Slack(_)),
            "slack configs should parse into the Slack variant"
        );

        let email = IntegrationConfig::parse(
            "email",
            r#"{
                "from_name":"Tond",
                "from_addr":"ops@example.com",
                "smtp_host":"smtp.example.com",
                "smtp_port":587,
                "smtp_user":"ops",
                "smtp_password":"secret",
                "smtp_encryption":"starttls",
                "to_addrs":["dev@example.com"]
            }"#,
        )
        .unwrap_or_else(|err| panic!("failed to parse email config: {err}"));
        assert!(
            matches!(email, IntegrationConfig::Email(_)),
            "email configs should parse into the Email variant"
        );
    }

    #[test]
    fn integration_config_parse_rejects_unknown_kind_and_invalid_json() {
        let unknown = IntegrationConfig::parse("pagerduty", "{}");
        assert!(
            unknown.is_err(),
            "unknown integration kinds should be rejected"
        );
        let unknown_message = unknown
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected an error for unknown kind"))
            .to_string();
        assert!(
            unknown_message.contains("unknown integration kind"),
            "unknown integration kind errors should mention the invalid kind, got: {unknown_message}"
        );

        let invalid_json = IntegrationConfig::parse("telegram", "not json");
        assert!(
            invalid_json.is_err(),
            "invalid provider config JSON should be rejected"
        );
    }

    #[test]
    fn integration_response_from_row_parses_valid_json_and_bool_flag() {
        let row = Integration {
            id: "integration-1".to_string(),
            name: "Ops".to_string(),
            kind: "slack".to_string(),
            config: r#"{"webhook_url":"https://slack.example"}"#.to_string(),
            enabled: 1,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let response = IntegrationResponse::from(row);

        assert!(
            response.enabled,
            "IntegrationResponse should convert non-zero enabled values to true"
        );
        assert_eq!(
            response.config,
            serde_json::json!({ "webhook_url": "https://slack.example" }),
            "IntegrationResponse should deserialize the stored config JSON"
        );
    }

    #[test]
    fn integration_response_from_row_falls_back_to_null_for_invalid_json() {
        let row = Integration {
            id: "integration-1".to_string(),
            name: "Ops".to_string(),
            kind: "slack".to_string(),
            config: "not json".to_string(),
            enabled: 0,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let response = IntegrationResponse::from(row);

        assert!(
            !response.enabled,
            "IntegrationResponse should convert zero enabled values to false"
        );
        assert_eq!(
            response.config,
            serde_json::Value::Null,
            "invalid stored integration config JSON should fall back to null"
        );
    }
}
