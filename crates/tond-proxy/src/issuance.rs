// issuance.rs — when to ask the CA for a certificate, and what to record
//
// IssuanceLoop looks at the domains tond serves (the route map plus the
// dashboard domain) and, for each one that needs a certificate (none yet,
// unparseable, or expiring within 30 days), checks the per-domain retry state
// in the `certificates` table:
//
//   failure_count  consecutive failed attempts
//   next_retry_at  no attempt before this time (RFC 3339)
//   last_error     human-readable reason shown in the dashboard
//
// Event-driven: a pass runs when the routed domains change (the route sync
// pokes `wake`), when tond-server asks (control socket `certs`, e.g. "Retry
// now"), and otherwise when the earliest retry is due — at least every
// MAX_SLEEP to catch renewals. It used to wake every 30 s.
//
// tond-proxy opens tond.db read-only. Results are queued in the Outbox and
// sent to tond-server over the control socket (control.rs), which writes
// them to `certificates` and acknowledges; unacknowledged results override
// the stored state so the backoff holds meanwhile.
//
// Backoff: 1 min after the first failure, doubling up to a 6 h cap
// (backoff_delay). A success resets the state; tond-server resets it when a
// domain is saved again, and clears next_retry_at for "Retry now".
//
// DNS pre-check (dns_precheck): before ordering, resolve the domain and
// compare with this server's public addresses. Only a clear mismatch — the
// domain resolves exclusively to public IPs, we know our own public IP, and
// none match — skips the order (recorded as a failure with an actionable
// message, so it backs off too). Anything inconclusive (lookup failure,
// NXDOMAIN, private/loopback answers such as the e2e test's fake DNS, or an
// unknown server IP behind NAT) proceeds with the order. Once the backoff has
// reached its cap, a mismatch no longer blocks the order either, so domains
// behind a proxy/CDN (which resolve elsewhere but still route HTTP-01 to us)
// are attempted every 6 h instead of never.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::sync::{Notify, watch};
use tond_common::dns::{DnsCheck, classify_dns, server_addresses};
use tond_common::proxy_control::{CertOutcome, CertReport};

use crate::acme::{self, ChallengeMap};
use crate::certs::CertStore;
use crate::meters;
use crate::{DashboardDomain, RouteMap};

/// Delay after the first failure.
pub const BACKOFF_BASE: Duration = Duration::from_secs(60);
/// Longest delay between attempts.
pub const BACKOFF_CAP: Duration = Duration::from_secs(6 * 3600);
/// Longest sleep between passes (renewal checks).
const MAX_SLEEP: Duration = Duration::from_secs(12 * 3600);
/// Shortest sleep between passes.
const MIN_SLEEP: Duration = Duration::from_secs(1);
/// Sleep after a pass failed (e.g. the database was unreadable).
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(300);
/// First pass at the latest this long after start (normally the first route
/// load triggers it sooner).
const STARTUP_DELAY: Duration = Duration::from_secs(10);
/// Timeout for the DNS pre-check lookup.
const DNS_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest error message stored for the dashboard.
const MAX_ERROR_LEN: usize = 500;

/// Delay before the next attempt after `failures` consecutive failures.
pub fn backoff_delay(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let factor = 1u64.checked_shl(failures - 1).unwrap_or(u64::MAX);
    let secs = BACKOFF_BASE.as_secs().saturating_mul(factor);
    Duration::from_secs(secs).min(BACKOFF_CAP)
}

// ── DNS pre-check ─────────────────────────────────────────────────────────────
//
// The comparison itself (DnsCheck, classify_dns, server_addresses) lives in
// tond_common::dns, shared with tond-server's domain check endpoint.

async fn resolve(domain: &str) -> Option<Vec<IpAddr>> {
    let lookup = tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((domain, 80)))
        .await
        .ok()?
        .ok()?;
    Some(lookup.map(|addr| addr.ip()).collect())
}

pub async fn dns_precheck(domain: &str, server_ips: &[IpAddr]) -> DnsCheck {
    let resolved = resolve(domain).await;
    classify_dns(resolved.as_deref(), server_ips)
}

// ── Retry state and the outbox ────────────────────────────────────────────────

/// Retry state for one domain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetryState {
    pub failure_count: u32,
    pub next_retry_at: Option<DateTime<Utc>>,
}

impl RetryState {
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.next_retry_at.is_none_or(|t| t <= now)
    }
}

/// Stored retry state (read-only: tond-server writes the table).
pub async fn load_retry_states(db: &SqlitePool) -> Result<HashMap<String, RetryState>> {
    let rows: Vec<(String, i64, Option<String>)> =
        sqlx::query_as("SELECT domain, failure_count, next_retry_at FROM certificates")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(domain, failures, next)| {
            let state = RetryState {
                failure_count: u32::try_from(failures).unwrap_or(0),
                next_retry_at: next.as_deref().and_then(parse_time),
            };
            (domain, state)
        })
        .collect())
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn truncate_error(message: &str) -> String {
    if message.len() <= MAX_ERROR_LEN {
        return message.to_string();
    }
    let mut end = MAX_ERROR_LEN;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

#[derive(Default)]
struct OutboxInner {
    next_seq: u64,
    /// Unacknowledged reports by sequence number; at most one per domain.
    pending: BTreeMap<u64, CertReport>,
}

/// Certificate results waiting for tond-server to store them. tond-proxy
/// opens the database read-only, so every result travels over the control
/// socket; a report stays here (and is resent on every new connection) until
/// tond-server acknowledges it. Until then it also overrides the stored retry
/// state, so the backoff holds even while tond-server is away.
pub struct Outbox {
    inner: Mutex<OutboxInner>,
    latest: watch::Sender<u64>,
}

impl Default for Outbox {
    fn default() -> Self {
        Self {
            inner: Mutex::new(OutboxInner::default()),
            latest: watch::Sender::new(0),
        }
    }
}

impl Outbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, OutboxInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue a report (replacing an older unacknowledged one for the domain).
    pub fn push(&self, report: CertReport) -> u64 {
        let seq = {
            let mut inner = self.lock();
            inner.pending.retain(|_, r| r.domain != report.domain);
            inner.next_seq += 1;
            let seq = inner.next_seq;
            inner.pending.insert(seq, report);
            seq
        };
        self.latest.send_replace(seq);
        seq
    }

    /// tond-server stored the report `seq`.
    pub fn ack(&self, seq: u64) {
        self.lock().pending.remove(&seq);
    }

    /// Unacknowledged reports with a sequence number above `after`.
    pub fn pending_after(&self, after: u64) -> Vec<(u64, CertReport)> {
        self.lock()
            .pending
            .range(after + 1..)
            .map(|(seq, r)| (*seq, r.clone()))
            .collect()
    }

    /// Changes whenever a report is queued.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.latest.subscribe()
    }

    /// Retry state implied by unacknowledged reports.
    fn overlay(&self) -> HashMap<String, RetryState> {
        self.lock()
            .pending
            .values()
            .map(|r| {
                let state = match &r.outcome {
                    CertOutcome::Issued { .. } => RetryState::default(),
                    CertOutcome::Failed {
                        failure_count,
                        next_retry_at,
                        ..
                    } => RetryState {
                        failure_count: *failure_count,
                        next_retry_at: parse_time(next_retry_at),
                    },
                };
                (r.domain.clone(), state)
            })
            .collect()
    }
}

fn cert_paths(data_dir: &std::path::Path, domain: &str) -> (String, String) {
    let dir = data_dir.join("certs").join(domain);
    (
        dir.join("cert.pem").to_string_lossy().to_string(),
        dir.join("key.pem").to_string_lossy().to_string(),
    )
}

/// Record a failed (or skipped) attempt and schedule the next one.
/// `status` is 'active' when a still-valid certificate keeps serving
/// (failed renewal), 'pending' for a DNS skip, 'failed' otherwise.
pub fn record_failure(
    outbox: &Outbox,
    data_dir: &std::path::Path,
    domain: &str,
    status: &str,
    error: &str,
    failure_count: u32,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let next = now
        + chrono::Duration::from_std(backoff_delay(failure_count))
            .unwrap_or_else(|_| chrono::Duration::hours(6));
    let (cert_path, key_path) = cert_paths(data_dir, domain);
    outbox.push(CertReport {
        domain: domain.to_string(),
        cert_path,
        key_path,
        attempted_at: now.to_rfc3339(),
        outcome: CertOutcome::Failed {
            status: status.to_string(),
            error: truncate_error(error),
            failure_count,
            next_retry_at: next.to_rfc3339(),
        },
    });
    next
}

/// Record a successful issuance: status active, retry state cleared.
pub async fn record_success(
    outbox: &Outbox,
    data_dir: &std::path::Path,
    cert_store: &CertStore,
    domain: &str,
) -> Result<()> {
    let (cert_file, _) = cert_store.cert_paths(domain);
    let cert_pem = tokio::fs::read(&cert_file).await?;
    let (issued_at, expires_at) = acme::certificate_timestamps(&cert_pem)?;
    let (cert_path, key_path) = cert_paths(data_dir, domain);
    outbox.push(CertReport {
        domain: domain.to_string(),
        cert_path,
        key_path,
        attempted_at: Utc::now().to_rfc3339(),
        outcome: CertOutcome::Issued {
            issued_at,
            expires_at,
        },
    });
    Ok(())
}

// ── Loop ──────────────────────────────────────────────────────────────────────

/// Why a domain needs a certificate now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Issue,
    Renew,
}

async fn need_for(cert_store: &CertStore, domain: &str) -> Option<Need> {
    if !cert_store.has_cert(domain).await {
        return Some(Need::Issue);
    }
    match cert_store.days_until_expiry(domain).await {
        // Unparseable expiry: re-issue defensively.
        None => Some(Need::Issue),
        Some(days) if acme::should_renew(Some(i64::from(days))) => {
            if days > 0 {
                Some(Need::Renew)
            } else {
                Some(Need::Issue)
            }
        }
        Some(_) => None,
    }
}

/// Sleep until the earliest of `deadlines`, bounded by [MIN_SLEEP, MAX_SLEEP].
fn next_sleep(deadlines: impl Iterator<Item = DateTime<Utc>>, now: DateTime<Utc>) -> Duration {
    deadlines
        .map(|t| (t - now).to_std().unwrap_or(Duration::ZERO))
        .min()
        .unwrap_or(MAX_SLEEP)
        .clamp(MIN_SLEEP, MAX_SLEEP)
}

pub struct IssuanceLoop {
    pub routes: RouteMap,
    pub dashboard_domain: DashboardDomain,
    pub cert_store: CertStore,
    pub challenges: ChallengeMap,
    pub data_dir: PathBuf,
    /// Read-only connection (the retry state tond-server stores).
    pub db: SqlitePool,
    pub outbox: Arc<Outbox>,
    /// Poked when the routed domains change or tond-server asks for a pass.
    pub wake: Arc<Notify>,
    pub staging: bool,
}

impl IssuanceLoop {
    pub async fn run(self) {
        let meter = meters::register("certificates");
        // Let the first route load happen (it pokes `wake`), but don't wait
        // forever for it.
        let mut sleep = STARTUP_DELAY;
        loop {
            meter.planned(Some(sleep));
            let started = Instant::now();
            let timer = tokio::select! {
                _ = tokio::time::sleep(sleep) => true,
                _ = self.wake.notified() => false,
            };
            meter.woke(timer.then_some((started, sleep)));
            let run = Instant::now();
            match self.pass().await {
                Ok(next) => {
                    meter.ran(run, None);
                    sleep = next;
                }
                Err(e) => {
                    tracing::warn!("Certificate pass failed: {}", e);
                    meter.ran(run, Some(e.to_string()));
                    sleep = RETRY_AFTER_ERROR;
                }
            }
        }
    }

    async fn desired_domains(&self) -> Vec<String> {
        let mut domains: HashSet<String> = self.routes.read().await.keys().cloned().collect();
        if let Some(dd) = self.dashboard_domain.read().await.clone() {
            domains.insert(dd);
        }
        let mut domains: Vec<String> = domains.into_iter().collect();
        domains.sort();
        domains
    }

    /// One pass over every domain; returns how long to sleep until the next
    /// retry is due (renewals are re-checked at least every MAX_SLEEP).
    async fn pass(&self) -> Result<Duration> {
        let domains = self.desired_domains().await;
        if domains.is_empty() {
            return Ok(MAX_SLEEP);
        }
        let mut states = load_retry_states(&self.db).await?;
        states.extend(self.outbox.overlay());
        let mut server_ips: Option<Vec<IpAddr>> = None;
        let mut deadlines = Vec::new();

        for domain in domains {
            let Some(need) = need_for(&self.cert_store, &domain).await else {
                continue;
            };
            let state = states.get(&domain).cloned().unwrap_or_default();
            let now = Utc::now();
            if !state.is_due(now) {
                deadlines.extend(state.next_retry_at);
                continue;
            }
            let server_ips = server_ips.get_or_insert_with(server_addresses);
            deadlines.extend(self.attempt(&domain, need, &state, server_ips).await);
        }
        Ok(next_sleep(deadlines.into_iter(), Utc::now()))
    }

    /// Try one domain. Returns the next retry time after a failure.
    async fn attempt(
        &self,
        domain: &str,
        need: Need,
        state: &RetryState,
        server_ips: &[IpAddr],
    ) -> Option<DateTime<Utc>> {
        let failures = state.failure_count.saturating_add(1);
        // A failed renewal keeps the old (still valid) certificate serving.
        let failure_status = if need == Need::Renew {
            "active"
        } else {
            "failed"
        };

        let dns = dns_precheck(domain, server_ips).await;
        let at_cap = backoff_delay(state.failure_count) >= BACKOFF_CAP;
        if let Some(message) = dns.mismatch_message(domain)
            && !at_cap
        {
            let status = if need == Need::Renew {
                "active"
            } else {
                "pending"
            };
            let next = record_failure(
                &self.outbox,
                &self.data_dir,
                domain,
                status,
                &message,
                failures,
                Utc::now(),
            );
            tracing::info!(
                "[cert] {} skipped: {} Next check {}",
                domain,
                message,
                next.to_rfc3339()
            );
            return Some(next);
        }

        tracing::info!("[cert] Requesting certificate for {} ({:?})", domain, need);
        match acme::provision_cert(
            domain,
            &self.data_dir,
            &self.cert_store,
            &self.challenges,
            self.staging,
        )
        .await
        {
            Ok(()) => {
                if let Err(e) =
                    record_success(&self.outbox, &self.data_dir, &self.cert_store, domain).await
                {
                    tracing::warn!("[cert] issued {} but could not record it: {}", domain, e);
                }
                None
            }
            Err(e) => {
                let mut message = format!("{e:#}");
                if let Some(hint) = dns.mismatch_message(domain) {
                    message = format!("{hint} ({message})");
                }
                let next = record_failure(
                    &self.outbox,
                    &self.data_dir,
                    domain,
                    failure_status,
                    &message,
                    failures,
                    Utc::now(),
                );
                tracing::warn!(
                    "[cert] Certificate for {} failed (attempt {}): {}. Next attempt {}",
                    domain,
                    failures,
                    message,
                    next.to_rfc3339()
                );
                Some(next)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn backoff_doubles_from_one_minute_and_caps_at_six_hours() {
        assert_eq!(backoff_delay(0), Duration::ZERO);
        assert_eq!(backoff_delay(1), Duration::from_secs(60));
        assert_eq!(backoff_delay(2), Duration::from_secs(120));
        assert_eq!(backoff_delay(3), Duration::from_secs(240));
        assert_eq!(backoff_delay(9), Duration::from_secs(60 * 256));
        assert_eq!(backoff_delay(10), BACKOFF_CAP);
        assert_eq!(
            backoff_delay(64),
            BACKOFF_CAP,
            "huge counts must not overflow"
        );
        assert_eq!(backoff_delay(u32::MAX), BACKOFF_CAP);
    }

    #[test]
    fn retry_state_is_due_without_a_time_or_once_it_passes() {
        let now = Utc::now();
        assert!(RetryState::default().is_due(now));
        let later = RetryState {
            failure_count: 2,
            next_retry_at: Some(now + chrono::Duration::minutes(1)),
        };
        assert!(!later.is_due(now));
        assert!(later.is_due(now + chrono::Duration::minutes(2)));
    }

    fn outbox_states(outbox: &Outbox) -> HashMap<String, RetryState> {
        outbox.overlay()
    }

    #[test]
    fn record_failure_queues_a_report_with_backoff_that_overrides_stored_state() {
        let dir = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let outbox = Outbox::default();
        let now = Utc::now();

        let next = record_failure(&outbox, dir.path(), "x.com", "failed", "boom", 3, now);
        assert_eq!(next - now, chrono::Duration::minutes(4));
        // A second failure replaces the first unacknowledged report.
        record_failure(
            &outbox,
            dir.path(),
            "x.com",
            "pending",
            &"e".repeat(2000),
            4,
            now,
        );

        let pending = outbox.pending_after(0);
        assert_eq!(pending.len(), 1, "one report per domain");
        let (seq, report) = &pending[0];
        assert_eq!(*seq, 2);
        assert!(report.cert_path.ends_with("certs/x.com/cert.pem"));
        match &report.outcome {
            CertOutcome::Failed {
                status,
                error,
                failure_count,
                ..
            } => {
                assert_eq!(status, "pending");
                assert_eq!(*failure_count, 4);
                assert!(
                    error.chars().count() <= MAX_ERROR_LEN + 1,
                    "errors are truncated"
                );
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        let state = outbox_states(&outbox)
            .get("x.com")
            .cloned()
            .unwrap_or_default();
        assert_eq!(state.failure_count, 4);
        assert!(!state.is_due(now), "the next attempt is in the future");

        outbox.ack(*seq);
        assert!(outbox.pending_after(0).is_empty());
        assert!(
            outbox_states(&outbox).is_empty(),
            "acked: the stored state rules again"
        );
    }

    #[test]
    fn outbox_notifies_subscribers_and_resends_only_newer_reports() {
        let dir = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let outbox = Outbox::default();
        let mut rx = outbox.subscribe();
        record_failure(&outbox, dir.path(), "a.com", "failed", "x", 1, Utc::now());
        record_failure(&outbox, dir.path(), "b.com", "failed", "y", 1, Utc::now());
        assert!(rx.has_changed().unwrap_or(false));
        assert_eq!(*rx.borrow_and_update(), 2);
        assert_eq!(outbox.pending_after(1).len(), 1);
        assert_eq!(outbox.pending_after(0).len(), 2);
    }

    #[test]
    fn next_sleep_targets_the_earliest_retry_within_bounds() {
        let now = Utc::now();
        assert_eq!(next_sleep(std::iter::empty(), now), MAX_SLEEP);
        assert_eq!(
            next_sleep(
                [
                    now + chrono::Duration::minutes(5),
                    now + chrono::Duration::minutes(2)
                ]
                .into_iter(),
                now
            ),
            Duration::from_secs(120)
        );
        assert_eq!(
            next_sleep([now - chrono::Duration::minutes(1)].into_iter(), now),
            MIN_SLEEP
        );
        assert_eq!(
            next_sleep([now + chrono::Duration::days(30)].into_iter(), now),
            MAX_SLEEP
        );
    }

    #[tokio::test]
    async fn stored_retry_state_is_read_from_the_database() {
        let dir = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let url = format!("sqlite:{}?mode=rwc", dir.path().join("t.db").display());
        let db = SqlitePool::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("sqlite: {err}"));
        sqlx::query(
            "CREATE TABLE certificates (domain TEXT PRIMARY KEY, failure_count INTEGER NOT NULL, next_retry_at TEXT)",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("create: {err}"));
        sqlx::query("INSERT INTO certificates VALUES ('x.com', 2, '2999-01-01T00:00:00+00:00'), ('y.com', 0, NULL)")
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("insert: {err}"));
        let states = load_retry_states(&db)
            .await
            .unwrap_or_else(|err| panic!("load: {err}"));
        assert_eq!(states["x.com"].failure_count, 2);
        assert!(!states["x.com"].is_due(Utc::now()));
        assert!(states["y.com"].is_due(Utc::now()));
    }
}
