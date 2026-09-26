// notifications.rs — formatting and sending alert notifications
//
// One `Notice` (an alert firing, still firing, resolved, or a test) is turned
// into a request per channel kind:
//
//   slack     incoming webhook, {"text": …} (Mattermost-compatible)
//   discord   webhook, {"content": …}
//   telegram  Bot API sendMessage, HTML with the text escaped
//   email     SMTP via lettre, plain text
//   ntfy      JSON publish to the server root ({"topic", "title", …}), with
//             an optional Bearer token
//   pushover  POST api.pushover.net/1/messages.json
//   gotify    POST <server>/message with X-Gotify-Key
//   webhook   POST of the documented JSON payload (see `webhook_payload`),
//             signed with HMAC-SHA256 in X-Tond-Signature-256 when a signing
//             secret is set
//
// Every message carries the same template: title (state + rule name +
// target), severity, target, value vs threshold, how long it has been firing,
// and a link to the dashboard when the dashboard domain is known.
//
// Sending never fails silently: every error is returned to the caller, which
// records it on the delivery and the channel (alerts/deliver.rs).

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tond_common::{AlertComparison, EmailConfig, IntegrationConfig, alert_kind};

const TIMEOUT: Duration = Duration::from_secs(10);
const PUSHOVER_URL: &str = "https://api.pushover.net/1/messages.json";
/// Header carrying the webhook body signature.
pub const SIGNATURE_HEADER: &str = "X-Tond-Signature-256";

/// One shared HTTP client: one connection pool and TLS config for all sends.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("tond/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
});

// ── The message ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeEvent {
    Firing,
    Repeat,
    Resolved,
    Test,
}

impl NoticeEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeEvent::Firing => "firing",
            NoticeEvent::Repeat => "repeat",
            NoticeEvent::Resolved => "resolved",
            NoticeEvent::Test => "test",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "repeat" => NoticeEvent::Repeat,
            "resolved" => NoticeEvent::Resolved,
            "test" => NoticeEvent::Test,
            _ => NoticeEvent::Firing,
        }
    }
}

/// Everything a channel needs to describe one alert.
#[derive(Debug, Clone)]
pub struct Notice {
    pub event: NoticeEvent,
    /// alert_history id ("" for tests)
    pub episode_id: String,
    pub rule_id: Option<String>,
    pub rule_name: String,
    pub kind: String,
    pub severity: String,
    pub target: String,
    pub target_label: String,
    pub app_id: Option<String>,
    pub value: Option<f64>,
    pub threshold: f64,
    /// Unix seconds
    pub pending_since: i64,
    pub fired_at: i64,
    pub resolved_at: Option<i64>,
    /// When the message is sent (unix seconds)
    pub now: i64,
    /// Dashboard link to the alert, when the dashboard domain is set
    pub url: Option<String>,
}

impl Notice {
    /// A test message for a channel.
    pub fn test(channel_name: &str, now: i64, url: Option<String>) -> Self {
        Notice {
            event: NoticeEvent::Test,
            episode_id: String::new(),
            rule_id: None,
            rule_name: format!("Test from Tond to {channel_name}"),
            kind: "test".to_string(),
            severity: "info".to_string(),
            target: String::new(),
            target_label: String::new(),
            app_id: None,
            value: None,
            threshold: 0.0,
            pending_since: now,
            fired_at: now,
            resolved_at: None,
            now,
            url,
        }
    }

    fn unit(&self) -> &'static str {
        alert_kind(&self.kind).map(|k| k.unit).unwrap_or("")
    }

    fn comparison(&self) -> AlertComparison {
        alert_kind(&self.kind)
            .map(|k| k.comparison)
            .unwrap_or(AlertComparison::None)
    }

    fn label(&self) -> &'static str {
        alert_kind(&self.kind).map(|k| k.label).unwrap_or("Alert")
    }

    /// "Firing", "Still firing", "Resolved", "Test"
    pub fn state_word(&self) -> &'static str {
        match self.event {
            NoticeEvent::Firing => "Firing",
            NoticeEvent::Repeat => "Still firing",
            NoticeEvent::Resolved => "Resolved",
            NoticeEvent::Test => "Test",
        }
    }

    /// "Firing: High CPU · api"
    pub fn title(&self) -> String {
        if self.event == NoticeEvent::Test {
            return self.rule_name.clone();
        }
        if self.target_label.is_empty() {
            format!("{}: {}", self.state_word(), self.rule_name)
        } else {
            format!(
                "{}: {} · {}",
                self.state_word(),
                self.rule_name,
                self.target_label
            )
        }
    }

    /// "CPU usage is 93.2% (threshold: above 80%)"
    pub fn summary(&self) -> String {
        if self.event == NoticeEvent::Test {
            return "If you can read this, alerts reach this channel.".to_string();
        }
        let unit = self.unit();
        let cmp = self.comparison();
        if cmp == AlertComparison::None {
            let what = condition_phrase(&self.kind);
            return if self.event == NoticeEvent::Resolved {
                format!("Recovered: {what} no longer.")
            } else {
                format!("{}{}.", capitalize(what), self.count_suffix())
            };
        }
        let value = self
            .value
            .map(|v| format_value(v, unit))
            .unwrap_or_else(|| "unknown".to_string());
        let threshold = format_value(self.threshold, unit);
        let dir = match cmp {
            AlertComparison::Above => "above",
            AlertComparison::AtLeast => "at least",
            AlertComparison::Below => "below",
            AlertComparison::None => "",
        };
        if self.event == NoticeEvent::Resolved {
            format!(
                "{} is back to normal: {value} (alert at {dir} {threshold}).",
                self.label()
            )
        } else {
            format!(
                "{} is {value} (threshold: {dir} {threshold}).",
                self.label()
            )
        }
    }

    fn count_suffix(&self) -> String {
        match self.value {
            Some(n) if n > 1.0 && self.kind == "oom" => format!(" ({n:.0} times)"),
            _ => String::new(),
        }
    }

    /// "Firing for 12 min" / "Was firing for 1 h 5 min"
    pub fn duration_line(&self) -> Option<String> {
        match self.event {
            NoticeEvent::Test => None,
            NoticeEvent::Resolved => {
                let end = self.resolved_at.unwrap_or(self.now);
                Some(format!(
                    "Was firing for {}",
                    format_duration(end - self.fired_at)
                ))
            }
            _ => Some(format!(
                "Firing for {}",
                format_duration(self.now - self.fired_at)
            )),
        }
    }

    /// Body lines under the title.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.summary()];
        if self.event != NoticeEvent::Test {
            lines.push(format!("Severity: {}", self.severity));
            if !self.target_label.is_empty() {
                lines.push(format!("Target: {}", self.target_label));
            }
            if let Some(d) = self.duration_line() {
                lines.push(d);
            }
        }
        if let Some(url) = &self.url {
            lines.push(url.clone());
        }
        lines
    }

    /// Title and body as plain text.
    pub fn text(&self) -> String {
        let mut out = self.title();
        for line in self.lines() {
            out.push('\n');
            out.push_str(&line);
        }
        out
    }

    /// The generic webhook payload (version 1). Documented in the dashboard
    /// channel form and in docs:
    ///
    /// ```json
    /// { "version": 1, "event": "firing|repeat|resolved|test",
    ///   "delivery_id": "…", "sent_at": "RFC 3339",
    ///   "alert": { "id", "rule_id", "rule_name", "kind", "severity",
    ///              "target", "target_label", "app_id", "value", "threshold",
    ///              "unit", "comparison", "summary", "pending_since",
    ///              "fired_at", "resolved_at", "duration_secs", "url" },
    ///   "text": "title and body as plain text" }
    /// ```
    pub fn webhook_payload(&self, delivery_id: &str) -> serde_json::Value {
        let end = self.resolved_at.unwrap_or(self.now);
        serde_json::json!({
            "version": 1,
            "event": self.event.as_str(),
            "delivery_id": delivery_id,
            "sent_at": rfc3339(self.now),
            "alert": {
                "id": self.episode_id,
                "rule_id": self.rule_id,
                "rule_name": self.rule_name,
                "kind": self.kind,
                "severity": self.severity,
                "target": self.target,
                "target_label": self.target_label,
                "app_id": self.app_id,
                "value": self.value,
                "threshold": self.threshold,
                "unit": self.unit(),
                "comparison": self.comparison(),
                "summary": self.summary(),
                "pending_since": rfc3339(self.pending_since),
                "fired_at": rfc3339(self.fired_at),
                "resolved_at": self.resolved_at.map(rfc3339),
                "duration_secs": (end - self.fired_at).max(0),
                "url": self.url,
            },
            "text": self.text(),
        })
    }
}

fn condition_phrase(kind: &str) -> &'static str {
    match kind {
        "app_down" => "the app is not running",
        "oom" => "a process was killed for running out of memory",
        "deploy_failed" => "the latest deploy failed",
        "backup_failed" => "the latest backup failed",
        "cron_failed" => "the latest run failed",
        _ => "the condition is true",
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

/// "93.2%", "1240 ms", "5.5 days", "31 hours", "4 restarts"
pub fn format_value(v: f64, unit: &str) -> String {
    match unit {
        "%" => format!("{v:.1}%"),
        "ms" => format!("{v:.0} ms"),
        "days" | "hours" => {
            let n = if v >= 10.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.1}")
            };
            format!("{n} {unit}")
        }
        "restarts" => {
            if (v - 1.0).abs() < f64::EPSILON {
                "1 restart".to_string()
            } else {
                format!("{v:.0} restarts")
            }
        }
        "" => format!("{v:.0}"),
        other => format!("{v:.1} {other}"),
    }
}

/// "45 s", "12 min", "3 h 5 min", "2 d 4 h"
pub fn format_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        let (h, m) = (secs / 3600, secs % 3600 / 60);
        if m == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {m} min")
        }
    } else {
        let (d, h) = (secs / 86_400, secs % 86_400 / 3600);
        if h == 0 {
            format!("{d} d")
        } else {
            format!("{d} d {h} h")
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ── Requests ─────────────────────────────────────────────────────────────────

/// An HTTP request to one channel, built before it is sent (so the format
/// can be tested without the network).
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl OutboundRequest {
    fn json(url: String, body: &serde_json::Value) -> Self {
        OutboundRequest {
            url,
            headers: Vec::new(),
            content_type: "application/json",
            body: body.to_string().into_bytes(),
        }
    }

    fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }
}

fn severity_rank(severity: &str) -> i64 {
    match severity {
        "critical" => 2,
        "warning" => 1,
        _ => 0,
    }
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Build the request for a channel; None for email (SMTP, not HTTP).
pub fn build_request(
    config: &IntegrationConfig,
    notice: &Notice,
    delivery_id: &str,
) -> Result<Option<OutboundRequest>> {
    let text = notice.text();
    let resolved = notice.event == NoticeEvent::Resolved;
    Ok(Some(match config {
        IntegrationConfig::Slack(cfg) => OutboundRequest::json(
            cfg.webhook_url.clone(),
            &serde_json::json!({ "text": text }),
        ),
        IntegrationConfig::Discord(cfg) => OutboundRequest::json(
            cfg.webhook_url.clone(),
            &serde_json::json!({ "content": text }),
        ),
        IntegrationConfig::Telegram(cfg) => {
            let mut html = format!("<b>{}</b>", html_escape(&notice.title()));
            for line in notice.lines() {
                html.push('\n');
                html.push_str(&html_escape(&line));
            }
            let mut body = serde_json::json!({
                "chat_id": cfg.chat_id,
                "text": html,
                "parse_mode": "HTML",
                "disable_web_page_preview": true,
            });
            if let Some(tid) = cfg.thread_id.as_deref().filter(|t| !t.is_empty()) {
                body["message_thread_id"] = serde_json::Value::String(tid.to_string());
            }
            OutboundRequest::json(
                format!("https://api.telegram.org/bot{}/sendMessage", cfg.bot_token),
                &body,
            )
        }
        IntegrationConfig::Email(_) => return Ok(None),
        IntegrationConfig::Ntfy(cfg) => {
            // ntfy priorities: 1 min … 3 default … 5 max/urgent
            let priority = if resolved {
                3
            } else {
                3 + severity_rank(&notice.severity)
            };
            let tag = match notice.event {
                NoticeEvent::Resolved => "white_check_mark",
                NoticeEvent::Test => "bell",
                _ if notice.severity == "critical" => "rotating_light",
                _ => "warning",
            };
            let mut body = serde_json::json!({
                "topic": cfg.topic,
                "title": notice.title(),
                "message": notice.lines().join("\n"),
                "priority": priority,
                "tags": [tag],
            });
            if let Some(url) = &notice.url {
                body["click"] = serde_json::Value::String(url.clone());
            }
            let mut req =
                OutboundRequest::json(cfg.server_url.trim_end_matches('/').to_string(), &body);
            if let Some(token) = cfg.token.as_deref().filter(|t| !t.is_empty()) {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            req
        }
        IntegrationConfig::Pushover(cfg) => {
            // Pushover priorities: -1 quiet, 0 normal, 1 high (bypasses the
            // user's own quiet hours)
            let priority = if resolved {
                -1
            } else {
                severity_rank(&notice.severity) - 1
            };
            let mut body = serde_json::json!({
                "token": cfg.app_token,
                "user": cfg.user_key,
                "title": notice.title(),
                "message": notice.lines().join("\n"),
                "priority": priority,
                "timestamp": notice.now,
            });
            if let Some(url) = &notice.url {
                body["url"] = serde_json::Value::String(url.clone());
                body["url_title"] = serde_json::Value::String("Open in Tond".to_string());
            }
            OutboundRequest::json(PUSHOVER_URL.to_string(), &body)
        }
        IntegrationConfig::Gotify(cfg) => {
            // Gotify priorities: 0–10; ≥ 8 is shown as high on Android
            let priority = if resolved {
                2
            } else {
                [4, 6, 8][severity_rank(&notice.severity) as usize]
            };
            let mut body = serde_json::json!({
                "title": notice.title(),
                "message": notice.lines().join("\n"),
                "priority": priority,
            });
            if let Some(url) = &notice.url {
                body["extras"] = serde_json::json!({
                    "client::notification": { "click": { "url": url } }
                });
            }
            OutboundRequest::json(join_url(&cfg.server_url, "message"), &body)
                .header("X-Gotify-Key", cfg.app_token.clone())
        }
        IntegrationConfig::Webhook(cfg) => {
            let body = notice.webhook_payload(delivery_id);
            let mut req = OutboundRequest::json(cfg.webhook_url.clone(), &body)
                .header("X-Tond-Event", notice.event.as_str())
                .header("X-Tond-Delivery", delivery_id);
            if let Some(secret) = cfg.signing_secret.as_deref().filter(|s| !s.is_empty()) {
                let signature = sign(secret, &req.body);
                req = req.header(SIGNATURE_HEADER, signature);
            }
            req
        }
    }))
}

/// `sha256=<hex HMAC-SHA256 of body>`, as GitHub does it.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Send a built request; any non-2xx answer is an error with a short excerpt.
pub async fn send_request(req: &OutboundRequest) -> Result<()> {
    let mut builder = CLIENT
        .post(&req.url)
        .header(reqwest::header::CONTENT_TYPE, req.content_type)
        .body(req.body.clone());
    for (name, value) in &req.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let response = builder.send().await.map_err(|e| {
        // reqwest errors can include the URL, which may embed a token.
        let what = if e.is_timeout() {
            "timed out".to_string()
        } else if e.is_connect() {
            "could not connect".to_string()
        } else {
            "request failed".to_string()
        };
        anyhow::anyhow!("{what} ({})", host_of(&req.url))
    })?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let excerpt: String = text.trim().chars().take(200).collect();
        if excerpt.is_empty() {
            anyhow::bail!("HTTP {} from {}", status.as_u16(), host_of(&req.url));
        }
        anyhow::bail!(
            "HTTP {} from {}: {excerpt}",
            status.as_u16(),
            host_of(&req.url)
        );
    }
    Ok(())
}

fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "the channel".to_string())
}

/// Send `notice` to one channel.
pub async fn dispatch(
    config: &IntegrationConfig,
    notice: &Notice,
    delivery_id: &str,
) -> Result<()> {
    match build_request(config, notice, delivery_id)? {
        Some(req) => send_request(&req).await,
        None => match config {
            IntegrationConfig::Email(cfg) => send_email(cfg, notice).await,
            _ => Ok(()),
        },
    }
}

// ── Email (SMTP via lettre) ──────────────────────────────────────────────────

fn build_email_message(cfg: &EmailConfig, notice: &Notice) -> Result<lettre::Message> {
    use lettre::{Message, message::header::ContentType};

    let from_mailbox: lettre::message::Mailbox = format!("{} <{}>", cfg.from_name, cfg.from_addr)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid from address: {}", e))?;

    let mut builder = Message::builder()
        .from(from_mailbox)
        .subject(format!("[Tond] {}", notice.title()));

    for addr in &cfg.to_addrs {
        let mailbox: lettre::message::Mailbox = addr
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid to address {}: {}", addr, e))?;
        builder = builder.to(mailbox);
    }

    builder
        .header(ContentType::TEXT_PLAIN)
        .body(notice.lines().join("\n"))
        .map_err(|e| anyhow::anyhow!("Failed to build email: {}", e))
}

async fn send_email(cfg: &EmailConfig, notice: &Notice) -> Result<()> {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
        transport::smtp::authentication::Credentials,
    };
    let email = build_email_message(cfg, notice)?;
    let creds = Credentials::new(cfg.smtp_user.clone(), cfg.smtp_password.clone());

    let transport: AsyncSmtpTransport<Tokio1Executor> = match cfg.smtp_encryption.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host)
            .context("SMTP relay error")?
            .port(cfg.smtp_port)
            .credentials(creds)
            .timeout(Some(TIMEOUT))
            .build(),
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
            .context("SMTP STARTTLS error")?
            .port(cfg.smtp_port)
            .credentials(creds)
            .timeout(Some(TIMEOUT))
            .build(),
        _ => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
            .port(cfg.smtp_port)
            .credentials(creds)
            .timeout(Some(TIMEOUT))
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
    use std::sync::{Arc, Mutex};

    use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
    use tond_common::{
        DiscordConfig, GotifyConfig, NtfyConfig, PushoverConfig, SlackConfig, TelegramConfig,
        WebhookConfig,
    };

    use super::*;

    fn notice(event: NoticeEvent) -> Notice {
        Notice {
            event,
            episode_id: "ep-1".to_string(),
            rule_id: Some("rule-1".to_string()),
            rule_name: "High CPU".to_string(),
            kind: "cpu".to_string(),
            severity: "critical".to_string(),
            target: "app:a1".to_string(),
            target_label: "api".to_string(),
            app_id: Some("a1".to_string()),
            value: Some(93.25),
            threshold: 80.0,
            pending_since: 1_000,
            fired_at: 1_300,
            resolved_at: None,
            now: 2_020,
            url: Some("https://tond.example.com/alerts?alert=ep-1".to_string()),
        }
    }

    #[test]
    fn template_has_title_severity_target_value_duration_and_link() {
        let n = notice(NoticeEvent::Firing);
        assert_eq!(n.title(), "Firing: High CPU · api");
        assert_eq!(
            n.lines(),
            vec![
                "CPU usage is 93.2% (threshold: above 80.0%).".to_string(),
                "Severity: critical".to_string(),
                "Target: api".to_string(),
                "Firing for 12 min".to_string(),
                "https://tond.example.com/alerts?alert=ep-1".to_string(),
            ]
        );
        let resolved = Notice {
            event: NoticeEvent::Resolved,
            value: Some(20.0),
            resolved_at: Some(1_300 + 3_900),
            ..n.clone()
        };
        assert_eq!(resolved.title(), "Resolved: High CPU · api");
        assert!(
            resolved
                .summary()
                .starts_with("CPU usage is back to normal: 20.0%")
        );
        assert_eq!(
            resolved.duration_line().as_deref(),
            Some("Was firing for 1 h 5 min")
        );
        let repeat = Notice {
            event: NoticeEvent::Repeat,
            ..n.clone()
        };
        assert!(repeat.title().starts_with("Still firing:"));

        let down = Notice {
            kind: "app_down".to_string(),
            value: Some(1.0),
            ..n
        };
        assert_eq!(down.summary(), "The app is not running.");
    }

    #[test]
    fn values_and_durations_read_naturally() {
        assert_eq!(format_value(3.0, "restarts"), "3 restarts");
        assert_eq!(format_value(1.0, "restarts"), "1 restart");
        assert_eq!(format_value(1240.4, "ms"), "1240 ms");
        assert_eq!(format_value(5.52, "days"), "5.5 days");
        assert_eq!(format_value(31.2, "hours"), "31 hours");
        assert_eq!(format_duration(42), "42 s");
        assert_eq!(format_duration(7_200), "2 h");
        assert_eq!(format_duration(90_000), "1 d 1 h");
    }

    #[test]
    fn webhook_payload_is_versioned_and_complete() {
        let p = notice(NoticeEvent::Firing).webhook_payload("d-1");
        assert_eq!(p["version"], 1);
        assert_eq!(p["event"], "firing");
        assert_eq!(p["delivery_id"], "d-1");
        assert_eq!(p["alert"]["rule_name"], "High CPU");
        assert_eq!(p["alert"]["threshold"], 80.0);
        assert_eq!(p["alert"]["unit"], "%");
        assert_eq!(p["alert"]["comparison"], "above");
        assert_eq!(p["alert"]["duration_secs"], 720);
        assert!(p["alert"]["resolved_at"].is_null());
    }

    #[test]
    fn signature_matches_a_known_hmac() {
        // echo -n 'hello' | openssl dgst -sha256 -hmac 'key'
        assert_eq!(
            sign("key", b"hello"),
            "sha256=9307b3b915efb5171ff14d8cb55fbcc798c6c0ef1456d66ded1a6aa723a58b7b"
        );
    }

    #[test]
    fn telegram_escapes_html() {
        let n = Notice {
            rule_name: "<b>x</b> & y".to_string(),
            ..notice(NoticeEvent::Firing)
        };
        let cfg = IntegrationConfig::Telegram(TelegramConfig {
            bot_token: "123:abc".to_string(),
            chat_id: "42".to_string(),
            thread_id: Some("7".to_string()),
        });
        let req = build_request(&cfg, &n, "d")
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("telegram request"));
        assert_eq!(req.url, "https://api.telegram.org/bot123:abc/sendMessage");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let text = body["text"].as_str().unwrap_or_default();
        assert!(
            text.starts_with("<b>Firing: &lt;b&gt;x&lt;/b&gt; &amp; y · api</b>\n"),
            "{text}"
        );
        assert_eq!(body["message_thread_id"], "7");
        assert_eq!(body["parse_mode"], "HTML");
    }

    #[test]
    fn email_message_has_subject_and_body() {
        let cfg = EmailConfig {
            from_name: "Tond Alerts".to_string(),
            from_addr: "alerts@example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_user: "user".to_string(),
            smtp_password: "pass".to_string(),
            smtp_encryption: "tls".to_string(),
            to_addrs: vec!["ops@example.com".to_string()],
        };
        let email = build_email_message(&cfg, &notice(NoticeEvent::Firing))
            .unwrap_or_else(|err| panic!("email: {err}"));
        let formatted = String::from_utf8(email.formatted()).unwrap_or_default();
        assert!(
            formatted.contains("Subject: [Tond] Firing: High CPU"),
            "{formatted}"
        );
        assert!(formatted.contains("Severity: critical"));
        let bad = EmailConfig {
            to_addrs: vec!["nope".to_string()],
            ..cfg
        };
        assert!(build_email_message(&bad, &notice(NoticeEvent::Firing)).is_err());
    }

    /// A captured request: path, headers and body.
    #[derive(Debug, Clone)]
    struct Captured {
        path: String,
        headers: HeaderMap,
        body: Bytes,
    }

    /// Mock HTTP server recording every POST; answers `status`.
    async fn mock(status: u16) -> (String, Arc<Mutex<Vec<Captured>>>) {
        let seen: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let store = seen.clone();
        let app = Router::new().fallback(post(
            move |uri: axum::http::Uri, headers: HeaderMap, body: Bytes| {
                let store = store.clone();
                async move {
                    store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(Captured {
                            path: uri.to_string(),
                            headers,
                            body,
                        });
                    (
                        axum::http::StatusCode::from_u16(status)
                            .unwrap_or(axum::http::StatusCode::OK),
                        "nope from mock",
                    )
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|err| panic!("bind: {err}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|err| panic!("addr: {err}"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), seen)
    }

    /// Point a built request at the mock server, keeping path and query.
    fn retarget(mut req: OutboundRequest, base: &str) -> OutboundRequest {
        let url = reqwest::Url::parse(&req.url).unwrap_or_else(|err| panic!("url: {err}"));
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_string(),
        };
        req.url = format!("{base}{path}");
        req
    }

    async fn send_to_mock(config: IntegrationConfig, n: &Notice) -> Captured {
        let (base, seen) = mock(200).await;
        let req = build_request(&config, n, "delivery-1")
            .unwrap_or_else(|err| panic!("build: {err}"))
            .unwrap_or_else(|| panic!("an HTTP channel"));
        send_request(&retarget(req, &base))
            .await
            .unwrap_or_else(|err| panic!("send: {err}"));
        let got = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(got.len(), 1);
        got[0].clone()
    }

    fn json(c: &Captured) -> serde_json::Value {
        assert_eq!(
            c.headers.get("content-type").and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        serde_json::from_slice(&c.body).unwrap_or_else(|err| panic!("json body: {err}"))
    }

    #[tokio::test]
    async fn slack_and_discord_post_the_text() {
        let n = notice(NoticeEvent::Firing);
        let c = send_to_mock(
            IntegrationConfig::Slack(SlackConfig {
                webhook_url: "https://hooks.slack.com/services/T/B/x".to_string(),
            }),
            &n,
        )
        .await;
        assert_eq!(c.path, "/services/T/B/x");
        assert_eq!(json(&c), serde_json::json!({ "text": n.text() }));

        let c = send_to_mock(
            IntegrationConfig::Discord(DiscordConfig {
                webhook_url: "https://discord.com/api/webhooks/1/y".to_string(),
            }),
            &n,
        )
        .await;
        assert_eq!(c.path, "/api/webhooks/1/y");
        assert_eq!(json(&c)["content"], n.text());
    }

    #[tokio::test]
    async fn ntfy_publishes_json_with_priority_click_and_token() {
        let n = notice(NoticeEvent::Firing);
        let c = send_to_mock(
            IntegrationConfig::Ntfy(NtfyConfig {
                server_url: "https://ntfy.example.com/".to_string(),
                topic: "tond-secret-topic".to_string(),
                token: Some("tk_abc".to_string()),
            }),
            &n,
        )
        .await;
        assert_eq!(c.path, "/");
        assert_eq!(
            c.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer tk_abc")
        );
        let body = json(&c);
        assert_eq!(body["topic"], "tond-secret-topic");
        assert_eq!(body["title"], "Firing: High CPU · api");
        assert_eq!(body["priority"], 5, "critical is urgent");
        assert_eq!(body["tags"], serde_json::json!(["rotating_light"]));
        assert_eq!(body["click"], "https://tond.example.com/alerts?alert=ep-1");

        let resolved = Notice {
            event: NoticeEvent::Resolved,
            ..n
        };
        let c = send_to_mock(
            IntegrationConfig::Ntfy(NtfyConfig {
                server_url: "https://ntfy.sh".to_string(),
                topic: "t".to_string(),
                token: None,
            }),
            &resolved,
        )
        .await;
        assert!(c.headers.get("authorization").is_none());
        assert_eq!(json(&c)["priority"], 3);
    }

    #[tokio::test]
    async fn pushover_sends_token_user_and_priority() {
        let n = Notice {
            severity: "warning".to_string(),
            ..notice(NoticeEvent::Firing)
        };
        let c = send_to_mock(
            IntegrationConfig::Pushover(PushoverConfig {
                user_key: "ukey".to_string(),
                app_token: "atok".to_string(),
            }),
            &n,
        )
        .await;
        assert_eq!(c.path, "/1/messages.json");
        let body = json(&c);
        assert_eq!(body["token"], "atok");
        assert_eq!(body["user"], "ukey");
        assert_eq!(body["priority"], 0);
        assert_eq!(body["url_title"], "Open in Tond");
        assert_eq!(body["timestamp"], 2_020);
    }

    #[tokio::test]
    async fn gotify_uses_the_key_header_and_message_path() {
        let n = notice(NoticeEvent::Firing);
        let c = send_to_mock(
            IntegrationConfig::Gotify(GotifyConfig {
                server_url: "https://gotify.example.com/base/".to_string(),
                app_token: "gtok".to_string(),
            }),
            &n,
        )
        .await;
        assert_eq!(c.path, "/base/message");
        assert_eq!(
            c.headers.get("x-gotify-key").and_then(|v| v.to_str().ok()),
            Some("gtok")
        );
        let body = json(&c);
        assert_eq!(body["priority"], 8);
        assert_eq!(
            body["extras"]["client::notification"]["click"]["url"],
            "https://tond.example.com/alerts?alert=ep-1"
        );
    }

    #[tokio::test]
    async fn webhook_is_signed_when_a_secret_is_set() {
        let n = notice(NoticeEvent::Firing);
        let c = send_to_mock(
            IntegrationConfig::Webhook(WebhookConfig {
                webhook_url: "https://hooks.example.com/tond?x=1".to_string(),
                signing_secret: Some("s3cret".to_string()),
            }),
            &n,
        )
        .await;
        assert_eq!(c.path, "/tond?x=1");
        let header = |name: &str| {
            c.headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(header("x-tond-event").as_deref(), Some("firing"));
        assert_eq!(header("x-tond-delivery").as_deref(), Some("delivery-1"));
        assert_eq!(
            header("x-tond-signature-256"),
            Some(sign("s3cret", &c.body)),
            "the signature covers the exact body bytes"
        );
        assert_eq!(json(&c)["alert"]["id"], "ep-1");

        let unsigned = send_to_mock(
            IntegrationConfig::Webhook(WebhookConfig {
                webhook_url: "https://hooks.example.com/x".to_string(),
                signing_secret: None,
            }),
            &n,
        )
        .await;
        assert!(unsigned.headers.get("x-tond-signature-256").is_none());
    }

    #[tokio::test]
    async fn a_failing_channel_reports_status_and_excerpt() {
        let (base, _) = mock(500).await;
        let req = build_request(
            &IntegrationConfig::Slack(SlackConfig {
                webhook_url: format!("{base}/hook"),
            }),
            &notice(NoticeEvent::Firing),
            "d",
        )
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("request"));
        let err = send_request(&req)
            .await
            .err()
            .unwrap_or_else(|| panic!("a 500 must be an error"));
        assert_eq!(err.to_string(), "HTTP 500 from 127.0.0.1: nope from mock");

        // Connection refused: no URL (which may hold a token) in the message.
        let req = OutboundRequest::json(
            "http://127.0.0.1:1/secret-token".to_string(),
            &serde_json::json!({}),
        );
        let err = send_request(&req).await.err().map(|e| e.to_string());
        assert_eq!(err.as_deref(), Some("could not connect (127.0.0.1)"));
    }
}
