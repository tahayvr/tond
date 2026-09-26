// audit_ship.rs — copy the audit log off the box, entry by entry
//
// The hash chain (audit_log.rs) detects edits and deletions *inside* the
// log, but someone with root can still cut off the newest entries, or
// rewrite the whole chain and its hashes. Shipping every entry, with its
// `prev_hash` and `hash`, to a system the server can't rewrite closes that
// gap: the remote copy is the reference to compare against. Off by default.
//
// Destinations (table `audit_shipping`, edited in Settings):
//
//   syslog   RFC 5424 messages over TCP, framed by octet counting (RFC 6587),
//            normally over TLS (RFC 5425, port 6514; the server certificate
//            is checked against the system trust store). One message per
//            entry: facility 13 (log audit), severity notice (warning for
//            failures), MSGID = action, `[meta sequenceId="<seq>"]`, and the
//            entry as JSON for MSG.
//   webhook  HTTPS POST of `{"type":"tond.audit","server":…,"entries":[…]}`
//            (up to BATCH entries, oldest first) with
//              X-Tond-Timestamp: <unix seconds>
//              X-Tond-Signature: sha256=<hex HMAC-SHA256(secret, "<timestamp>.<body>")>
//            Any 2xx is an acknowledgement.
//
// Queue: audit_log itself. `audit_ship_state.shipped_seq` is the last entry a
// destination accepted; everything after it is pending, so a restart loses
// nothing and the backlog is bounded by audit retention (entries pruned before
// delivery are counted in `dropped`). A new or re-pointed destination starts
// from the oldest retained entry, so the remote copy holds the whole chain.
//
// Scheduling: the "audit-shipping" loop sleeps until the audit log or the
// configuration changes (SQLite commit hooks, runtime/changes.rs) or until
// the next retry is due. With shipping off it waits on the configuration
// only: no wake-ups at all. Failures back off from RETRY_BASE doubling to
// RETRY_CAP.
//
// Verifying (also shown in Settings): take the shipped entries in seq order;
// for each, hash = sha256(prev_hash || canonical) where canonical is the JSON
// array [seq, created_at, actor_kind, actor_id, actor_label, action,
// resource_type, resource_id, resource_name, outcome, ip, detail] (compact
// serde_json), and prev_hash equals the previous entry's hash. Then compare
// with the server: `tond audit verify` checks the local chain, and the local
// newest seq/hash must match the remote copy. Remote entries missing locally,
// or a different hash for the same seq, mean the local log was rewritten.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::SqlitePool;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tond_common::AuditLog;

use crate::runtime::changes::{self, Table, Watch};
use crate::runtime::loops;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;

/// Entries per delivery.
pub const BATCH: i64 = 200;
/// Batches per wake-up before yielding (a big backfill continues next round).
const MAX_BATCHES_PER_WAKE: usize = 20;
/// Delay after the first failure.
const RETRY_BASE: Duration = Duration::from_secs(10);
/// Longest delay between attempts.
const RETRY_CAP: Duration = Duration::from_secs(30 * 60);
/// Connect / request timeout.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest error stored for the dashboard.
const MAX_ERROR_LEN: usize = 300;

pub const SYSLOG: &str = "syslog";
pub const WEBHOOK: &str = "webhook";

// ── Configuration ────────────────────────────────────────────────────────────

/// The stored row (webhook_secret still encrypted).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ShippingRow {
    pub syslog_enabled: bool,
    pub syslog_host: String,
    pub syslog_port: i64,
    pub syslog_tls: bool,
    pub webhook_enabled: bool,
    pub webhook_url: String,
    pub webhook_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyslogTarget {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct WebhookTarget {
    pub url: String,
    pub secret: String,
}

impl std::fmt::Debug for WebhookTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookTarget")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

pub async fn load_row(db: &SqlitePool) -> Result<ShippingRow> {
    Ok(sqlx::query_as::<_, ShippingRow>(
        "SELECT syslog_enabled, syslog_host, syslog_port, syslog_tls,
                webhook_enabled, webhook_url, webhook_secret
         FROM audit_shipping WHERE id = 'default'",
    )
    .fetch_one(db)
    .await?)
}

impl ShippingRow {
    pub fn syslog_target(&self) -> Option<SyslogTarget> {
        (self.syslog_enabled && !self.syslog_host.trim().is_empty()).then(|| SyslogTarget {
            host: self.syslog_host.trim().to_string(),
            port: u16::try_from(self.syslog_port).unwrap_or(6514),
            tls: self.syslog_tls,
        })
    }

    pub fn webhook_target(&self) -> Result<Option<WebhookTarget>> {
        if !self.webhook_enabled || self.webhook_url.trim().is_empty() {
            return Ok(None);
        }
        let secret = secret_key::decrypt(Purpose::AuditWebhookSecret, &self.webhook_secret)
            .context("the webhook signing secret cannot be decrypted")?;
        Ok(Some(WebhookTarget {
            url: self.webhook_url.trim().to_string(),
            secret,
        }))
    }
}

/// Why a syslog host/port can't be used, or None.
pub fn syslog_problem(host: &str, port: u16) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return Some("Enter the syslog server's host name or IP address.".into());
    }
    if host.len() > 253
        || host
            .chars()
            .any(|c| c.is_whitespace() || c == '/' || c == '@')
    {
        return Some(format!("'{host}' is not a valid host name."));
    }
    if port == 0 {
        return Some("The port must be between 1 and 65535.".into());
    }
    None
}

/// Why a webhook URL can't be used, or None. HTTPS only (plain HTTP just for
/// a receiver on this machine).
pub fn webhook_problem(url: &str) -> Option<String> {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return Some("Enter a full URL, e.g. https://logs.example.com/tond.".into());
    };
    let loopback = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    match parsed.scheme() {
        "https" => None,
        "http" if loopback => None,
        _ => Some("The webhook must use https://.".into()),
    }
}

// ── Formatting ───────────────────────────────────────────────────────────────

/// The entry as shipped: every column, hashes included.
pub fn entry_json(e: &AuditLog) -> serde_json::Value {
    serde_json::json!({
        "seq": e.seq,
        "created_at": e.created_at,
        "actor_kind": e.actor_kind,
        "actor_id": e.actor_id,
        "actor_label": e.actor_label,
        "action": e.action,
        "resource_type": e.resource_type,
        "resource_id": e.resource_id,
        "resource_name": e.resource_name,
        "outcome": e.outcome,
        "ip": e.ip,
        "detail": e.detail,
        "prev_hash": e.prev_hash,
        "hash": e.hash,
    })
}

/// Printable US-ASCII (33..=126), at most `max` characters, or "-".
fn header_field(value: &str, max: usize) -> String {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_graphic())
        .take(max)
        .collect();
    if cleaned.is_empty() {
        "-".into()
    } else {
        cleaned
    }
}

/// One RFC 5424 message, octet-counted (RFC 6587 §3.4.1 / RFC 5425 §4.3).
pub fn syslog_frame(e: &AuditLog, hostname: &str) -> Vec<u8> {
    // Facility 13 (log audit); notice, or warning for failures.
    let severity = if e.outcome == "failure" { 4 } else { 5 };
    let pri = 13 * 8 + severity;
    // sequenceId is 1..=2147483647 and wraps.
    let sequence = (e.seq.max(1) - 1) % 2_147_483_647 + 1;
    let msg = format!(
        "<{pri}>1 {} {} tond - {} [meta sequenceId=\"{sequence}\"] {}",
        header_field(&e.created_at, 64),
        header_field(hostname, 255),
        header_field(&e.action, 32),
        entry_json(e)
    );
    format!("{} {msg}", msg.len()).into_bytes()
}

/// Webhook request body for a batch.
pub fn webhook_body(server: &str, entries: &[AuditLog]) -> String {
    serde_json::json!({
        "type": "tond.audit",
        "server": server,
        "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
    })
    .to_string()
}

/// `X-Tond-Signature` value for a body sent at `timestamp`.
pub fn signature(secret: &str, timestamp: i64, body: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(format!("{timestamp}.{body}").as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

// ── Delivery ─────────────────────────────────────────────────────────────────

async fn write_frames<S: AsyncWrite + Unpin>(stream: &mut S, frames: &[Vec<u8>]) -> Result<()> {
    for frame in frames {
        stream.write_all(frame).await?;
    }
    stream.flush().await?;
    stream.shutdown().await?;
    Ok(())
}

pub async fn deliver_syslog(target: &SyslogTarget, frames: &[Vec<u8>]) -> Result<()> {
    let addr = (target.host.as_str(), target.port);
    let tcp = tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow::anyhow!("timed out connecting to {}:{}", target.host, target.port))?
        .with_context(|| format!("could not connect to {}:{}", target.host, target.port))?;
    let send = async {
        if target.tls {
            use rustls_platform_verifier::ConfigVerifierExt;
            let config = rustls::ClientConfig::with_platform_verifier()
                .context("could not load the system trust store")?;
            let name = rustls::pki_types::ServerName::try_from(target.host.clone())
                .context("invalid TLS server name")?;
            let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, tcp)
                .await
                .context("TLS handshake failed")?;
            write_frames(&mut tls, frames).await
        } else {
            let mut tcp = tcp;
            write_frames(&mut tcp, frames).await
        }
    };
    tokio::time::timeout(TIMEOUT * 3, send)
        .await
        .map_err(|_| anyhow::anyhow!("timed out sending to the syslog server"))?
}

pub async fn deliver_webhook(target: &WebhookTarget, body: String) -> Result<()> {
    let timestamp = Utc::now().timestamp();
    let response = reqwest::Client::new()
        .post(&target.url)
        .timeout(TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-Tond-Timestamp", timestamp.to_string())
        .header(
            "X-Tond-Signature",
            signature(&target.secret, timestamp, &body),
        )
        .body(body)
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    if !response.status().is_success() {
        bail!("the webhook answered HTTP {}", response.status().as_u16());
    }
    Ok(())
}

fn hostname() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "tond".into())
}

/// A made-up entry for "Send test" (seq 0, not part of the chain).
pub fn test_entry() -> AuditLog {
    AuditLog {
        seq: 0,
        created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        actor_kind: "system".into(),
        actor_id: None,
        actor_label: "tond".into(),
        action: "audit_shipping_test".into(),
        resource_type: None,
        resource_id: None,
        resource_name: None,
        outcome: "success".into(),
        ip: None,
        detail: Some("Test entry from Tond: shipping works. Not part of the audit chain.".into()),
        prev_hash: String::new(),
        hash: String::new(),
    }
}

/// Send one test entry to `destination` using the stored configuration.
pub async fn send_test(state: &AppState, destination: &str) -> Result<()> {
    let row = load_row(&state.db).await?;
    let entry = test_entry();
    match destination {
        SYSLOG => {
            if row.syslog_host.trim().is_empty() {
                bail!("Save a syslog server first.");
            }
            let target = SyslogTarget {
                host: row.syslog_host.trim().to_string(),
                port: u16::try_from(row.syslog_port).unwrap_or(6514),
                tls: row.syslog_tls,
            };
            deliver_syslog(&target, &[syslog_frame(&entry, &hostname())]).await
        }
        WEBHOOK => {
            if row.webhook_url.trim().is_empty() {
                bail!("Save a webhook URL first.");
            }
            let secret = secret_key::decrypt(Purpose::AuditWebhookSecret, &row.webhook_secret)
                .context("the webhook signing secret cannot be decrypted")?;
            let target = WebhookTarget {
                url: row.webhook_url.trim().to_string(),
                secret,
            };
            let server = state.settings.read().await.server_name.clone();
            deliver_webhook(&target, webhook_body(&server, &[entry])).await
        }
        other => bail!("Unknown destination '{other}'"),
    }
}

// ── Progress ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct ShipState {
    pub shipped_seq: i64,
    pub failures: i64,
    pub next_attempt_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub dropped: i64,
}

pub async fn load_state(db: &SqlitePool, destination: &str) -> Result<ShipState> {
    Ok(sqlx::query_as::<_, ShipState>(
        "SELECT shipped_seq, failures, next_attempt_at, last_attempt_at, last_success_at,
                last_error, dropped
         FROM audit_ship_state WHERE destination = ?",
    )
    .bind(destination)
    .fetch_optional(db)
    .await?
    .unwrap_or_default())
}

/// Forget a destination's progress (it was re-pointed): it starts over from
/// the oldest retained entry.
pub async fn reset_state(db: &SqlitePool, destination: &str) -> Result<()> {
    sqlx::query("DELETE FROM audit_ship_state WHERE destination = ?")
        .bind(destination)
        .execute(db)
        .await?;
    Ok(())
}

async fn save_state(db: &SqlitePool, destination: &str, s: &ShipState) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_ship_state (destination, shipped_seq, failures, next_attempt_at,
                                       last_attempt_at, last_success_at, last_error, dropped)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(destination) DO UPDATE SET
            shipped_seq = excluded.shipped_seq, failures = excluded.failures,
            next_attempt_at = excluded.next_attempt_at, last_attempt_at = excluded.last_attempt_at,
            last_success_at = excluded.last_success_at, last_error = excluded.last_error,
            dropped = excluded.dropped",
    )
    .bind(destination)
    .bind(s.shipped_seq)
    .bind(s.failures)
    .bind(&s.next_attempt_at)
    .bind(&s.last_attempt_at)
    .bind(&s.last_success_at)
    .bind(&s.last_error)
    .bind(s.dropped)
    .execute(db)
    .await?;
    Ok(())
}

/// Delay before the next attempt after `failures` consecutive failures.
pub fn retry_delay(failures: i64) -> Duration {
    let shift = u32::try_from(failures.saturating_sub(1).clamp(0, 20)).unwrap_or(20);
    RETRY_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(RETRY_CAP)
        .min(RETRY_CAP)
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn truncate(message: &str) -> String {
    if message.len() <= MAX_ERROR_LEN {
        return message.to_string();
    }
    let mut end = MAX_ERROR_LEN;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

enum Sink {
    Syslog(SyslogTarget, String),
    Webhook(WebhookTarget, String),
}

impl Sink {
    async fn deliver(&self, entries: &[AuditLog]) -> Result<()> {
        match self {
            Sink::Syslog(target, host) => {
                let frames: Vec<Vec<u8>> = entries.iter().map(|e| syslog_frame(e, host)).collect();
                deliver_syslog(target, &frames).await
            }
            Sink::Webhook(target, server) => {
                deliver_webhook(target, webhook_body(server, entries)).await
            }
        }
    }
}

/// Deliver what is pending for one destination. Returns when to try again
/// (after a failure), or None when it is caught up.
async fn ship_destination(
    db: &SqlitePool,
    destination: &str,
    sink: &Sink,
) -> Result<Option<DateTime<Utc>>> {
    let mut state = load_state(db, destination).await?;
    let now = Utc::now();
    if let Some(next) = state.next_attempt_at.as_deref().and_then(parse_time)
        && next > now
    {
        return Ok(Some(next));
    }
    for _ in 0..MAX_BATCHES_PER_WAKE {
        let entries = sqlx::query_as::<_, AuditLog>(
            "SELECT * FROM audit_log WHERE seq > ? ORDER BY seq LIMIT ?",
        )
        .bind(state.shipped_seq)
        .bind(BATCH)
        .fetch_all(db)
        .await?;
        let Some(first) = entries.first() else {
            return Ok(None);
        };
        let now = Utc::now().to_rfc3339();
        state.last_attempt_at = Some(now.clone());
        match sink.deliver(&entries).await {
            Ok(()) => {
                // Rows pruned by retention between two deliveries were never sent.
                if state.shipped_seq > 0 && first.seq > state.shipped_seq + 1 {
                    state.dropped += first.seq - state.shipped_seq - 1;
                }
                state.shipped_seq = entries.last().map_or(state.shipped_seq, |e| e.seq);
                state.failures = 0;
                state.next_attempt_at = None;
                state.last_success_at = Some(now);
                state.last_error = None;
                save_state(db, destination, &state).await?;
                if i64::try_from(entries.len()).unwrap_or(0) < BATCH {
                    return Ok(None);
                }
            }
            Err(e) => {
                state.failures += 1;
                let next = Utc::now()
                    + chrono::Duration::from_std(retry_delay(state.failures))
                        .unwrap_or_else(|_| chrono::Duration::minutes(30));
                state.next_attempt_at = Some(next.to_rfc3339());
                state.last_error = Some(truncate(&format!("{e:#}")));
                tracing::warn!(
                    "Audit shipping to {destination} failed (attempt {}): {e:#}",
                    state.failures
                );
                save_state(db, destination, &state).await?;
                return Ok(Some(next));
            }
        }
    }
    // More to send: come back right away.
    Ok(Some(Utc::now()))
}

/// One pass over every enabled destination; returns the earliest retry time.
pub async fn ship_pending(state: &AppState) -> Result<Option<DateTime<Utc>>> {
    let row = load_row(&state.db).await?;
    let server = state.settings.read().await.server_name.clone();
    let mut sinks = Vec::new();
    if let Some(target) = row.syslog_target() {
        sinks.push((SYSLOG, Sink::Syslog(target, hostname())));
    }
    match row.webhook_target() {
        Ok(Some(target)) => sinks.push((WEBHOOK, Sink::Webhook(target, server))),
        Ok(None) => {}
        Err(e) => tracing::warn!("Audit shipping to webhook skipped: {e:#}"),
    }
    let mut earliest: Option<DateTime<Utc>> = None;
    for (destination, sink) in &sinks {
        if let Some(next) = ship_destination(&state.db, destination, sink).await? {
            earliest = Some(earliest.map_or(next, |e| e.min(next)));
        }
    }
    Ok(earliest)
}

fn is_enabled(row: &ShippingRow) -> bool {
    row.syslog_target().is_some() || (row.webhook_enabled && !row.webhook_url.trim().is_empty())
}

/// The shipping loop (spawned once at startup).
pub async fn run(state: AppState) {
    let lp = loops::register("audit-shipping");
    let mut config_only = Watch::new(&[Table::AuditShipping]);
    let mut log_or_config = Watch::new(&[Table::AuditLog, Table::AuditShipping]);
    loop {
        let enabled = match load_row(&state.db).await {
            Ok(row) => is_enabled(&row),
            Err(e) => {
                tracing::warn!("Audit shipping: cannot read the configuration: {e:#}");
                false
            }
        };
        if !enabled {
            lp.wait(None, &mut config_only).await;
            tokio::time::sleep(changes::SETTLE).await;
            continue;
        }
        let run = lp.run();
        let next = match ship_pending(&state).await {
            Ok(next) => {
                drop(run);
                next
            }
            Err(e) => {
                run.fail(&e);
                Some(Utc::now() + chrono::Duration::from_std(RETRY_BASE).unwrap_or_default())
            }
        };
        let timeout = next.map(|t| (t - Utc::now()).to_std().unwrap_or(Duration::ZERO));
        lp.wait(timeout, &mut log_or_config).await;
        tokio::time::sleep(changes::SETTLE).await;
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;
    use crate::audit_log::{AuditActor, AuditEntry, record};

    fn entry(seq: i64, outcome: &str) -> AuditLog {
        AuditLog {
            seq,
            outcome: outcome.into(),
            action: "login failed!".into(),
            prev_hash: "aa".into(),
            hash: "bb".into(),
            ..test_entry()
        }
    }

    #[test]
    fn syslog_frames_are_octet_counted_rfc5424() {
        let frame =
            String::from_utf8(syslog_frame(&entry(42, "failure"), "vps 1")).unwrap_or_default();
        let (len, msg) = frame.split_once(' ').unwrap_or_default();
        assert_eq!(len.parse::<usize>().ok(), Some(msg.len()));
        // log audit (13) * 8 + warning (4)
        assert!(msg.starts_with("<108>1 "), "{msg}");
        let fields: Vec<&str> = msg.splitn(8, ' ').collect();
        assert_eq!(fields[2], "vps1", "hostname without spaces");
        assert_eq!(fields[3], "tond");
        assert_eq!(fields[5], "loginfailed!", "MSGID is printable ASCII");
        assert!(msg.contains("[meta sequenceId=\"42\"]"));
        let json: serde_json::Value =
            serde_json::from_str(msg.rsplit_once("] ").map(|(_, j)| j).unwrap_or_default())
                .unwrap_or_default();
        assert_eq!(json["seq"], 42);
        assert_eq!(json["hash"], "bb");
        assert_eq!(json["prev_hash"], "aa");
        assert!(
            String::from_utf8(syslog_frame(&entry(7, "success"), "h"))
                .unwrap_or_default()
                .contains("<109>1 ")
        );
    }

    #[test]
    fn signature_is_hmac_sha256_over_timestamp_and_body() {
        // Independently: python3 -c "import hmac,hashlib;print(hmac.new(b'k',b'1700000000.{}',hashlib.sha256).hexdigest())"
        assert_eq!(
            signature("k", 1_700_000_000, "{}"),
            "sha256=c6ef4734033cee2367414a0939385eca0397754aa7a59716da7463a9298b5146"
        );
        assert_ne!(signature("k", 1, "{}"), signature("k", 2, "{}"));
        assert_ne!(signature("k", 1, "{}"), signature("other", 1, "{}"));
    }

    #[test]
    fn targets_are_validated() {
        assert!(webhook_problem("https://logs.example.com/tond").is_none());
        assert!(webhook_problem("http://127.0.0.1:9000/x").is_none());
        assert!(webhook_problem("http://logs.example.com/x").is_some());
        assert!(webhook_problem("ftp://x").is_some());
        assert!(webhook_problem("not a url").is_some());
        assert!(syslog_problem("logs.example.com", 6514).is_none());
        assert!(syslog_problem("", 6514).is_some());
        assert!(syslog_problem("a b", 6514).is_some());
        assert!(syslog_problem("logs.example.com", 0).is_some());
    }

    #[test]
    fn retries_back_off_and_cap() {
        assert_eq!(retry_delay(1), Duration::from_secs(10));
        assert_eq!(retry_delay(2), Duration::from_secs(20));
        assert_eq!(retry_delay(4), Duration::from_secs(80));
        assert_eq!(retry_delay(40), RETRY_CAP);
    }

    /// A plain-TCP syslog receiver on a random port; returns its port and a
    /// handle yielding everything it received on one connection.
    async fn receiver() -> (u16, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        let handle = tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return String::new();
            };
            let mut buf = String::new();
            let _ = sock.read_to_string(&mut buf).await;
            buf
        });
        (port, handle)
    }

    #[tokio::test]
    async fn pending_entries_are_shipped_in_order_and_progress_is_kept() {
        let state = AppState::for_tests().await;
        let actor = AuditActor::system("test");
        for action in ["one", "two", "three"] {
            record(&state.db, &actor, AuditEntry::new(action)).await;
        }
        let (port, received) = receiver().await;
        sqlx::query(
            "UPDATE audit_shipping SET syslog_enabled = 1, syslog_host = '127.0.0.1',
                                       syslog_port = ?, syslog_tls = 0",
        )
        .bind(i64::from(port))
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("configure: {e}"));

        let next = ship_pending(&state)
            .await
            .unwrap_or_else(|e| panic!("ship: {e}"));
        assert_eq!(next, None, "caught up");
        let text = received.await.unwrap_or_default();
        let seqs: Vec<&str> = text
            .match_indices("sequenceId=\"")
            .map(|(i, m)| &text[i + m.len()..i + m.len() + 1])
            .collect();
        assert_eq!(seqs, vec!["1", "2", "3"], "{text}");

        let progress = load_state(&state.db, SYSLOG)
            .await
            .unwrap_or_else(|e| panic!("state: {e}"));
        assert_eq!(progress.shipped_seq, 3);
        assert!(progress.last_success_at.is_some() && progress.last_error.is_none());

        // Nothing new: no connection is made.
        assert_eq!(ship_pending(&state).await.ok().flatten(), None);
    }

    #[tokio::test]
    async fn failures_back_off_and_keep_entries_pending() {
        let state = AppState::for_tests().await;
        record(
            &state.db,
            &AuditActor::system("test"),
            AuditEntry::new("one"),
        )
        .await;
        // Port 9 (discard) is closed on test machines: connection refused.
        sqlx::query(
            "UPDATE audit_shipping SET syslog_enabled = 1, syslog_host = '127.0.0.1',
                                       syslog_port = 9, syslog_tls = 0",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("configure: {e}"));

        let next = ship_pending(&state)
            .await
            .unwrap_or_else(|e| panic!("ship: {e}"));
        assert!(next.is_some_and(|t| t > Utc::now()), "a retry is scheduled");
        let progress = load_state(&state.db, SYSLOG)
            .await
            .unwrap_or_else(|e| panic!("state: {e}"));
        assert_eq!(progress.shipped_seq, 0, "still pending");
        assert_eq!(progress.failures, 1);
        assert!(progress.last_error.is_some());

        // Within the backoff nothing is attempted.
        ship_pending(&state)
            .await
            .unwrap_or_else(|e| panic!("ship: {e}"));
        assert_eq!(
            load_state(&state.db, SYSLOG)
                .await
                .map(|s| s.failures)
                .unwrap_or_default(),
            1
        );
    }
}
