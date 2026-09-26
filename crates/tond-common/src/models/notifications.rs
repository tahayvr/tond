// notifications.rs — Integration / notification-channel models
//
// An Integration is a named, reusable notification channel (Telegram bot,
// Discord webhook, Slack/Mattermost webhook, SMTP email, ntfy, Pushover,
// Gotify, or a generic signed webhook).
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
//   ntfy      → { "server_url": "https://ntfy.sh", "topic": "…", "token"?: "…" }
//   pushover  → { "user_key": "…", "app_token": "…" }
//   gotify    → { "server_url": "https://gotify.example.com", "app_token": "…" }
//   webhook   → { "webhook_url": "…", "signing_secret"?: "…" }
//               (POSTs the alert JSON; with a secret, signs the body with
//               HMAC-SHA256 in `X-Tond-Signature-256: sha256=<hex>`)
//
// The secret fields (`INTEGRATION_SECRET_FIELDS`) are stored encrypted inside
// the JSON and are write-only over the API: responses list which ones are
// saved in `saved_secrets`, and an update that omits one (or sends "") keeps
// the saved value; `null` clears an optional one (ntfy token, signing secret).

use serde::{Deserialize, Serialize};

/// Config fields that hold secrets (bot tokens, webhook URLs, SMTP passwords,
/// ntfy topics — on a public server the topic name is the password — and
/// API tokens).
pub const INTEGRATION_SECRET_FIELDS: &[&str] = &[
    "bot_token",
    "webhook_url",
    "smtp_password",
    "topic",
    "token",
    "user_key",
    "app_token",
    "signing_secret",
];

/// Channel kinds.
pub const INTEGRATION_KINDS: &[&str] = &[
    "slack", "discord", "telegram", "email", "ntfy", "pushover", "gotify", "webhook",
];

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
    /// JSON `ChannelQuietHours`; None = follow the global setting
    pub quiet_hours: Option<String>,
    /// Unix seconds of the last successful delivery
    pub last_sent_at: Option<i64>,
    /// Last delivery error (cleared by the next success)
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
}

// ── Typed config variants ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
    /// Optional topic/thread ID for supergroups
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DiscordConfig {
    pub webhook_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SlackConfig {
    pub webhook_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

fn default_ntfy_server() -> String {
    "https://ntfy.sh".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NtfyConfig {
    #[serde(default = "default_ntfy_server")]
    pub server_url: String,
    pub topic: String,
    /// Access token for protected topics
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PushoverConfig {
    pub user_key: String,
    pub app_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GotifyConfig {
    pub server_url: String,
    pub app_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WebhookConfig {
    pub webhook_url: String,
    /// HMAC-SHA256 key for `X-Tond-Signature-256`
    #[serde(default)]
    pub signing_secret: Option<String>,
}

/// Strongly-typed config parsed from the `config` JSON column.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IntegrationConfig {
    Telegram(TelegramConfig),
    Discord(DiscordConfig),
    Slack(SlackConfig),
    Email(EmailConfig),
    Ntfy(NtfyConfig),
    Pushover(PushoverConfig),
    Gotify(GotifyConfig),
    Webhook(WebhookConfig),
}

impl IntegrationConfig {
    /// Parse from the stored JSON blob + kind string.
    pub fn parse(kind: &str, config_json: &str) -> anyhow::Result<Self> {
        Ok(match kind {
            "telegram" => IntegrationConfig::Telegram(serde_json::from_str(config_json)?),
            "discord" => IntegrationConfig::Discord(serde_json::from_str(config_json)?),
            "slack" => IntegrationConfig::Slack(serde_json::from_str(config_json)?),
            "email" => IntegrationConfig::Email(serde_json::from_str(config_json)?),
            "ntfy" => IntegrationConfig::Ntfy(serde_json::from_str(config_json)?),
            "pushover" => IntegrationConfig::Pushover(serde_json::from_str(config_json)?),
            "gotify" => IntegrationConfig::Gotify(serde_json::from_str(config_json)?),
            "webhook" => IntegrationConfig::Webhook(serde_json::from_str(config_json)?),
            other => anyhow::bail!("unknown integration kind: {}", other),
        })
    }
}

// ── API response shape ────────────────────────────────────────────────────────

/// Outbound representation of an integration — the non-secret config fields,
/// the names of the secret fields that are saved, and `enabled` as a bool.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IntegrationResponse {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Config object without its secret fields (see `INTEGRATION_SECRET_FIELDS`).
    pub config: serde_json::Value,
    /// Secret fields that have a saved value (e.g. `["bot_token"]`).
    pub saved_secrets: Vec<String>,
    pub enabled: bool,
    pub quiet_hours: crate::ChannelQuietHours,
    /// RFC 3339 of the last successful delivery
    pub last_sent_at: Option<String>,
    /// Last delivery error, until a delivery succeeds again
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn unix_rfc3339(ts: Option<i64>) -> Option<String> {
    ts.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.to_rfc3339())
}

impl From<Integration> for IntegrationResponse {
    fn from(i: Integration) -> Self {
        let mut saved_secrets = Vec::new();
        let config = match serde_json::from_str::<serde_json::Value>(&i.config) {
            Ok(serde_json::Value::Object(mut object)) => {
                for field in INTEGRATION_SECRET_FIELDS {
                    if let Some(value) = object.remove(*field)
                        && value.as_str().is_some_and(|s| !s.is_empty())
                    {
                        saved_secrets.push((*field).to_string());
                    }
                }
                serde_json::Value::Object(object)
            }
            _ => serde_json::Value::Null,
        };
        Self {
            id: i.id,
            name: i.name,
            kind: i.kind,
            config,
            saved_secrets,
            enabled: i.enabled != 0,
            quiet_hours: i
                .quiet_hours
                .as_deref()
                .and_then(|q| serde_json::from_str(q).ok())
                .unwrap_or_default(),
            last_sent_at: unix_rfc3339(i.last_sent_at),
            last_error: i.last_error,
            last_error_at: unix_rfc3339(i.last_error_at),
            created_at: i.created_at,
            updated_at: i.updated_at,
        }
    }
}

// ── Request bodies ───────────────────────────────────────────────────────────

/// POST /api/v1/integrations
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateIntegrationRequest {
    pub name: String,
    /// One of `INTEGRATION_KINDS`
    pub kind: String,
    /// Provider-specific config object (will be validated + serialised)
    pub config: serde_json::Value,
    #[serde(default)]
    pub quiet_hours: Option<crate::ChannelQuietHours>,
}

/// PATCH /api/v1/integrations/:id — all fields optional
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateIntegrationRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub quiet_hours: Option<crate::ChannelQuietHours>,
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
    fn integration_config_parse_accepts_the_push_and_webhook_kinds() {
        let ntfy = IntegrationConfig::parse("ntfy", r#"{"topic":"t"}"#)
            .unwrap_or_else(|err| panic!("ntfy: {err}"));
        match ntfy {
            IntegrationConfig::Ntfy(c) => {
                assert_eq!(c.server_url, "https://ntfy.sh");
                assert_eq!(c.token, None);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            IntegrationConfig::parse("pushover", r#"{"user_key":"u","app_token":"a"}"#),
            Ok(IntegrationConfig::Pushover(_))
        ));
        assert!(matches!(
            IntegrationConfig::parse("gotify", r#"{"server_url":"https://g","app_token":"a"}"#),
            Ok(IntegrationConfig::Gotify(_))
        ));
        assert!(matches!(
            IntegrationConfig::parse("webhook", r#"{"webhook_url":"https://h"}"#),
            Ok(IntegrationConfig::Webhook(_))
        ));
        assert!(IntegrationConfig::parse("pushover", r#"{"user_key":"u"}"#).is_err());
        for kind in INTEGRATION_KINDS {
            assert!(
                IntegrationConfig::parse(kind, "{}").is_err(),
                "{kind} needs some config"
            );
        }
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
            quiet_hours: None,
            last_sent_at: None,
            last_error: None,
            last_error_at: None,
        };

        let response = IntegrationResponse::from(row);

        assert!(
            response.enabled,
            "IntegrationResponse should convert non-zero enabled values to true"
        );
        assert_eq!(
            response.config,
            serde_json::json!({}),
            "IntegrationResponse must not return the (secret) webhook URL"
        );
        assert_eq!(response.saved_secrets, vec!["webhook_url".to_string()]);
    }

    #[test]
    fn integration_response_keeps_plain_fields_and_drops_every_secret() {
        let row = Integration {
            id: "integration-2".to_string(),
            name: "Mail".to_string(),
            kind: "email".to_string(),
            config:
                r#"{"smtp_host":"smtp.example.com","smtp_password":"v1.cipher","bot_token":""}"#
                    .to_string(),
            enabled: 1,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            quiet_hours: None,
            last_sent_at: None,
            last_error: None,
            last_error_at: None,
        };

        let response = IntegrationResponse::from(row);
        assert_eq!(
            response.config,
            serde_json::json!({ "smtp_host": "smtp.example.com" })
        );
        assert_eq!(
            response.saved_secrets,
            vec!["smtp_password".to_string()],
            "an empty secret field is not reported as saved"
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
            quiet_hours: None,
            last_sent_at: None,
            last_error: None,
            last_error_at: None,
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
