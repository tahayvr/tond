// notifications.rs — Notification dispatch for named integrations
//
// Supported channels:
//   telegram  → Telegram Bot API sendMessage
//   discord   → Discord webhook (content field)
//   slack     → Slack/Mattermost incoming webhook (text field)
//   email     → SMTP via lettre

use anyhow::Result;
use tond_common::{EmailConfig, IntegrationConfig};

/// Data carried from the alert checker / test endpoint into each dispatcher.
pub struct NotificationPayload {
    pub rule_name: String,
    pub event_or_metric: String,
    pub app_name: Option<String>,
    pub detail: Option<String>,
    pub fired_at: String,
}

impl NotificationPayload {
    /// Build a human-readable one-liner for simple channel types.
    fn message(&self) -> String {
        let mut parts = vec![format!("[Tond] {}", self.rule_name)];
        if let Some(ref app) = self.app_name {
            parts.push(format!("app={}", app));
        }
        parts.push(format!("event={}", self.event_or_metric));
        if let Some(ref d) = self.detail
            && !d.is_empty()
        {
            parts.push(d.clone());
        }
        parts.push(format!("at {}", self.fired_at));
        parts.join(" | ")
    }
}

fn build_telegram_request(
    cfg: &tond_common::TelegramConfig,
    payload: &NotificationPayload,
) -> (String, serde_json::Value) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", cfg.bot_token);

    let mut body = serde_json::json!({
        "chat_id": cfg.chat_id,
        "text": payload.message(),
        "parse_mode": "HTML",
    });

    if let Some(ref tid) = cfg.thread_id {
        body["message_thread_id"] = serde_json::Value::String(tid.clone());
    }

    (url, body)
}

fn build_discord_request(payload: &NotificationPayload) -> serde_json::Value {
    serde_json::json!({ "content": payload.message() })
}

fn build_slack_request(payload: &NotificationPayload) -> serde_json::Value {
    serde_json::json!({ "text": payload.message() })
}

fn build_email_message(
    cfg: &EmailConfig,
    payload: &NotificationPayload,
) -> Result<lettre::Message> {
    use lettre::{Message, message::header::ContentType};

    let from_mailbox: lettre::message::Mailbox = format!("{} <{}>", cfg.from_name, cfg.from_addr)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid from address: {}", e))?;

    let mut builder = Message::builder()
        .from(from_mailbox)
        .subject(format!("[Tond Alert] {}", payload.rule_name));

    for addr in &cfg.to_addrs {
        let mailbox: lettre::message::Mailbox = addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid to address {}: {}", addr, e))?;
        builder = builder.to(mailbox);
    }

    builder
        .header(ContentType::TEXT_PLAIN)
        .body(payload.message())
        .map_err(|e| anyhow::anyhow!("Failed to build email: {}", e))
}

/// Dispatch a notification to all configured channels.
pub async fn dispatch(config: &IntegrationConfig, payload: &NotificationPayload) -> Result<()> {
    match config {
        IntegrationConfig::Telegram(cfg) => send_telegram(cfg, payload).await,
        IntegrationConfig::Discord(cfg) => send_discord(cfg, payload).await,
        IntegrationConfig::Slack(cfg) => send_slack(cfg, payload).await,
        IntegrationConfig::Email(cfg) => send_email(cfg, payload).await,
    }
}

// ── Telegram ─────────────────────────────────────────────────────────────────

async fn send_telegram(
    cfg: &tond_common::TelegramConfig,
    payload: &NotificationPayload,
) -> Result<()> {
    let (url, body) = build_telegram_request(cfg, payload);

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        // reqwest errors print the URL, which carries the bot token or
        // webhook secret: never let it reach logs or the dashboard.
        .map_err(reqwest::Error::without_url)?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Telegram API error: {}", text);
    }

    Ok(())
}

// ── Discord ───────────────────────────────────────────────────────────────────

async fn send_discord(
    cfg: &tond_common::DiscordConfig,
    payload: &NotificationPayload,
) -> Result<()> {
    let body = build_discord_request(payload);

    let client = reqwest::Client::new();
    let resp = client
        .post(&cfg.webhook_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        // reqwest errors print the URL, which carries the bot token or
        // webhook secret: never let it reach logs or the dashboard.
        .map_err(reqwest::Error::without_url)?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Discord webhook error: {}", text);
    }

    Ok(())
}

// ── Slack / Mattermost ────────────────────────────────────────────────────────

async fn send_slack(cfg: &tond_common::SlackConfig, payload: &NotificationPayload) -> Result<()> {
    let body = build_slack_request(payload);

    let client = reqwest::Client::new();
    let resp = client
        .post(&cfg.webhook_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        // reqwest errors print the URL, which carries the bot token or
        // webhook secret: never let it reach logs or the dashboard.
        .map_err(reqwest::Error::without_url)?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Slack webhook error: {}", text);
    }

    Ok(())
}

// ── Email (SMTP via lettre) ───────────────────────────────────────────────────

async fn send_email(cfg: &EmailConfig, payload: &NotificationPayload) -> Result<()> {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
        transport::smtp::authentication::Credentials,
    };
    let email = build_email_message(cfg, payload)?;

    let creds = Credentials::new(cfg.smtp_user.clone(), cfg.smtp_password.clone());

    let transport: AsyncSmtpTransport<Tokio1Executor> = match cfg.smtp_encryption.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host)
            .map_err(|e| anyhow::anyhow!("SMTP relay error: {}", e))?
            .port(cfg.smtp_port)
            .credentials(creds)
            .build(),
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
            .map_err(|e| anyhow::anyhow!("SMTP starttls error: {}", e))?
            .port(cfg.smtp_port)
            .credentials(creds)
            .build(),
        _ => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
            .port(cfg.smtp_port)
            .credentials(creds)
            .build(),
    };

    transport
        .send(email)
        .await
        .map_err(|e| anyhow::anyhow!("SMTP send error: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tond_common::{DiscordConfig, EmailConfig, SlackConfig, TelegramConfig};

    #[tokio::test]
    async fn delivery_errors_never_contain_the_webhook_url() {
        // Nothing listens on port 9: the request fails before any response.
        let cfg = DiscordConfig {
            webhook_url: "http://127.0.0.1:9/api/webhooks/123/SUPERSECRET".to_string(),
        };
        let err = send_discord(&cfg, &sample_payload())
            .await
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default();
        assert!(!err.is_empty(), "the request must fail");
        assert!(!err.contains("SUPERSECRET"), "leaked the URL: {err}");
    }

    fn sample_payload() -> NotificationPayload {
        NotificationPayload {
            rule_name: "High CPU".to_string(),
            event_or_metric: "cpu".to_string(),
            app_name: Some("frontend".to_string()),
            detail: Some("usage exceeded 90%".to_string()),
            fired_at: "2026-04-08T12:34:56Z".to_string(),
        }
    }

    #[test]
    fn notification_payload_message_includes_optional_fields_and_skips_empty_detail() {
        let payload = sample_payload();
        assert_eq!(
            payload.message(),
            "[Tond] High CPU | app=frontend | event=cpu | usage exceeded 90% | at 2026-04-08T12:34:56Z",
            "notification payload messages should include rule name, app, event, detail, and fired_at"
        );

        let payload = NotificationPayload {
            app_name: None,
            detail: Some(String::new()),
            ..sample_payload()
        };
        assert_eq!(
            payload.message(),
            "[Tond] High CPU | event=cpu | at 2026-04-08T12:34:56Z",
            "notification payload messages should omit absent app names and empty detail strings"
        );
    }

    #[test]
    fn build_telegram_request_uses_bot_url_and_optional_thread_id() {
        let payload = sample_payload();
        let cfg = TelegramConfig {
            bot_token: "bot-token".to_string(),
            chat_id: "chat-123".to_string(),
            thread_id: Some("42".to_string()),
        };

        let (url, body) = build_telegram_request(&cfg, &payload);
        assert_eq!(
            url, "https://api.telegram.org/botbot-token/sendMessage",
            "telegram requests should target the bot sendMessage endpoint"
        );
        assert_eq!(
            body,
            serde_json::json!({
                "chat_id": "chat-123",
                "text": "[Tond] High CPU | app=frontend | event=cpu | usage exceeded 90% | at 2026-04-08T12:34:56Z",
                "parse_mode": "HTML",
                "message_thread_id": "42"
            }),
            "telegram requests should include the formatted message and optional thread id"
        );
    }

    #[test]
    fn build_discord_and_slack_requests_use_channel_specific_fields() {
        let payload = sample_payload();

        assert_eq!(
            build_discord_request(&payload),
            serde_json::json!({
                "content": "[Tond] High CPU | app=frontend | event=cpu | usage exceeded 90% | at 2026-04-08T12:34:56Z"
            }),
            "discord requests should send the formatted message in the content field"
        );
        assert_eq!(
            build_slack_request(&payload),
            serde_json::json!({
                "text": "[Tond] High CPU | app=frontend | event=cpu | usage exceeded 90% | at 2026-04-08T12:34:56Z"
            }),
            "slack requests should send the formatted message in the text field"
        );
    }

    #[test]
    fn build_email_message_sets_subject_recipients_and_body() {
        let payload = sample_payload();
        let cfg = EmailConfig {
            from_name: "Tond Alerts".to_string(),
            from_addr: "alerts@example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_user: "user".to_string(),
            smtp_password: "pass".to_string(),
            smtp_encryption: "tls".to_string(),
            to_addrs: vec!["ops@example.com".to_string(), "dev@example.com".to_string()],
        };

        let email = build_email_message(&cfg, &payload)
            .unwrap_or_else(|err| panic!("email message should build successfully: {err}"));
        let formatted = String::from_utf8(email.formatted())
            .unwrap_or_else(|err| panic!("formatted email should be valid UTF-8: {err}"));
        let envelope = email.envelope();

        assert!(
            formatted.contains("Subject: [Tond Alert] High CPU\r\n"),
            "email messages should include the alert rule name in the subject"
        );
        assert!(
            envelope.from().map(|addr| addr.to_string()).as_deref() == Some("alerts@example.com"),
            "email messages should use the configured from address"
        );
        assert!(
            envelope
                .to()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                == vec!["ops@example.com".to_string(), "dev@example.com".to_string()],
            "email messages should include all configured recipients"
        );
        assert!(
            formatted.contains("Content-Type: text/plain"),
            "email messages should be emitted as plain text content"
        );
    }

    #[test]
    fn build_email_message_rejects_invalid_addresses() {
        let payload = sample_payload();
        let invalid_from = EmailConfig {
            from_name: "Tond Alerts".to_string(),
            from_addr: "not-an-email".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_user: "user".to_string(),
            smtp_password: "pass".to_string(),
            smtp_encryption: "tls".to_string(),
            to_addrs: vec!["ops@example.com".to_string()],
        };
        let err = build_email_message(&invalid_from, &payload)
            .err()
            .unwrap_or_else(|| panic!("invalid from addresses should fail email construction"));
        assert!(
            err.to_string().contains("Invalid from address"),
            "invalid from addresses should produce a helpful error message"
        );

        let invalid_recipient = EmailConfig {
            from_addr: "alerts@example.com".to_string(),
            to_addrs: vec!["not-an-email".to_string()],
            ..invalid_from
        };
        let err = build_email_message(&invalid_recipient, &payload)
            .err()
            .unwrap_or_else(|| {
                panic!("invalid recipient addresses should fail email construction")
            });
        assert!(
            err.to_string().contains("Invalid to address not-an-email"),
            "invalid recipient addresses should produce a helpful error message"
        );
    }

    #[test]
    fn dispatch_matches_supported_integration_variants() {
        let payload = sample_payload();

        let telegram = IntegrationConfig::Telegram(TelegramConfig {
            bot_token: "token".to_string(),
            chat_id: "chat".to_string(),
            thread_id: None,
        });
        let discord = IntegrationConfig::Discord(DiscordConfig {
            webhook_url: "https://discord.example.com".to_string(),
        });
        let slack = IntegrationConfig::Slack(SlackConfig {
            webhook_url: "https://slack.example.com".to_string(),
        });
        let email = IntegrationConfig::Email(EmailConfig {
            from_name: "Tond Alerts".to_string(),
            from_addr: "alerts@example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_user: "user".to_string(),
            smtp_password: "pass".to_string(),
            smtp_encryption: "tls".to_string(),
            to_addrs: vec!["ops@example.com".to_string()],
        });

        match telegram {
            IntegrationConfig::Telegram(cfg) => {
                let (url, _) = build_telegram_request(&cfg, &payload);
                assert!(
                    url.contains("/sendMessage"),
                    "telegram integrations should route through the Telegram request builder"
                );
            }
            _ => panic!("expected telegram integration variant"),
        }
        match discord {
            IntegrationConfig::Discord(_) => {
                assert!(
                    build_discord_request(&payload).get("content").is_some(),
                    "discord integrations should use the Discord payload shape"
                );
            }
            _ => panic!("expected discord integration variant"),
        }
        match slack {
            IntegrationConfig::Slack(_) => {
                assert!(
                    build_slack_request(&payload).get("text").is_some(),
                    "slack integrations should use the Slack payload shape"
                );
            }
            _ => panic!("expected slack integration variant"),
        }
        match email {
            IntegrationConfig::Email(cfg) => {
                assert!(
                    build_email_message(&cfg, &payload).is_ok(),
                    "email integrations should build an SMTP email message successfully for valid configs"
                );
            }
            _ => panic!("expected email integration variant"),
        }
    }
}
