// issuance.rs — when to ask the CA for a certificate, and what to record
//
// run_issuance_loop() replaces the old "retry every domain without a cert
// every 30 s" loop. Every LOOP_INTERVAL it looks at the domains tond serves
// (the route map plus the dashboard domain) and, for each one that needs a
// certificate (none yet, unparseable, or expiring within 30 days), checks the
// per-domain retry state stored in the `certificates` table:
//
//   failure_count  consecutive failed attempts
//   next_retry_at  no attempt before this time (RFC 3339)
//   last_error     human-readable reason shown in the dashboard
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

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tond_common::dns::{DnsCheck, classify_dns, server_addresses};

use crate::acme::{self, ChallengeMap};
use crate::certs::CertStore;
use crate::{DashboardDomain, RouteMap};

/// Delay after the first failure.
pub const BACKOFF_BASE: Duration = Duration::from_secs(60);
/// Longest delay between attempts.
pub const BACKOFF_CAP: Duration = Duration::from_secs(6 * 3600);
/// How often the loop looks for domains that need a certificate.
const LOOP_INTERVAL: Duration = Duration::from_secs(30);
/// Let routes populate before the first pass.
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

// ── Retry state persistence ───────────────────────────────────────────────────

/// Stored retry state for one domain.
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
                next_retry_at: next
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|t| t.with_timezone(&Utc)),
            };
            (domain, state)
        })
        .collect())
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

/// Record a failed (or skipped) attempt and schedule the next one.
/// `status` is 'active' when a still-valid certificate keeps serving
/// (failed renewal), 'pending' for a DNS skip, 'failed' otherwise.
pub async fn record_failure(
    db: &SqlitePool,
    data_dir: &std::path::Path,
    domain: &str,
    status: &str,
    error: &str,
    failure_count: u32,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let next = now
        + chrono::Duration::from_std(backoff_delay(failure_count))
            .unwrap_or_else(|_| chrono::Duration::hours(6));
    let certs_dir = data_dir.join("certs").join(domain);
    let now_s = now.to_rfc3339();
    sqlx::query(
        "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at,
                                   status, created_at, updated_at,
                                   last_error, last_attempt_at, next_retry_at, failure_count)
         VALUES (?1, ?2, 'acme', ?3, ?4, ?5, ?5, ?6, ?5, ?5, ?7, ?5, ?8, ?9)
         ON CONFLICT(domain) DO UPDATE SET
            status = excluded.status,
            updated_at = excluded.updated_at,
            last_error = excluded.last_error,
            last_attempt_at = excluded.last_attempt_at,
            next_retry_at = excluded.next_retry_at,
            failure_count = excluded.failure_count",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(domain)
    .bind(certs_dir.join("cert.pem").to_string_lossy().to_string())
    .bind(certs_dir.join("key.pem").to_string_lossy().to_string())
    .bind(&now_s)
    .bind(status)
    .bind(truncate_error(error))
    .bind(next.to_rfc3339())
    .bind(i64::from(failure_count))
    .execute(db)
    .await
    .context("recording certificate failure")?;
    Ok(next)
}

/// Record a successful issuance: status active, retry state cleared.
pub async fn record_success(
    db: &SqlitePool,
    data_dir: &std::path::Path,
    cert_store: &CertStore,
    domain: &str,
) -> Result<()> {
    let (cert_path, _) = cert_store.cert_paths(domain);
    let cert_pem = tokio::fs::read(&cert_path).await?;
    let (issued_at, expires_at) = acme::certificate_timestamps(&cert_pem)?;
    acme::persist_certificate_status(
        db,
        data_dir,
        domain,
        "active",
        Some(issued_at),
        Some(expires_at),
    )
    .await?;
    sqlx::query(
        "UPDATE certificates
         SET last_error = NULL, last_attempt_at = ?, next_retry_at = NULL, failure_count = 0
         WHERE domain = ?",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(domain)
    .execute(db)
    .await?;
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

pub struct IssuanceLoop {
    pub routes: RouteMap,
    pub dashboard_domain: DashboardDomain,
    pub cert_store: CertStore,
    pub challenges: ChallengeMap,
    pub data_dir: PathBuf,
    pub db: SqlitePool,
    pub staging: bool,
}

impl IssuanceLoop {
    pub async fn run(self) {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            if let Err(e) = self.pass().await {
                tracing::warn!("Certificate pass failed: {}", e);
            }
            tokio::time::sleep(LOOP_INTERVAL).await;
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

    async fn pass(&self) -> Result<()> {
        let domains = self.desired_domains().await;
        if domains.is_empty() {
            return Ok(());
        }
        let states = load_retry_states(&self.db).await?;
        let mut server_ips: Option<Vec<IpAddr>> = None;

        for domain in domains {
            let Some(need) = need_for(&self.cert_store, &domain).await else {
                continue;
            };
            let state = states.get(&domain).cloned().unwrap_or_default();
            let now = Utc::now();
            if !state.is_due(now) {
                continue;
            }
            let server_ips = server_ips.get_or_insert_with(server_addresses);
            self.attempt(&domain, need, &state, server_ips).await;
        }
        Ok(())
    }

    async fn attempt(&self, domain: &str, need: Need, state: &RetryState, server_ips: &[IpAddr]) {
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
            match record_failure(
                &self.db,
                &self.data_dir,
                domain,
                status,
                &message,
                failures,
                Utc::now(),
            )
            .await
            {
                Ok(next) => tracing::info!(
                    "[cert] {} skipped: {} Next check {}",
                    domain,
                    message,
                    next.to_rfc3339()
                ),
                Err(e) => tracing::warn!("[cert] could not record DNS check for {}: {}", domain, e),
            }
            return;
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
                    record_success(&self.db, &self.data_dir, &self.cert_store, domain).await
                {
                    tracing::warn!("[cert] issued {} but could not record it: {}", domain, e);
                }
            }
            Err(e) => {
                let mut message = format!("{e:#}");
                if let Some(hint) = dns.mismatch_message(domain) {
                    message = format!("{hint} ({message})");
                }
                match record_failure(
                    &self.db,
                    &self.data_dir,
                    domain,
                    failure_status,
                    &message,
                    failures,
                    Utc::now(),
                )
                .await
                {
                    Ok(next) => tracing::warn!(
                        "[cert] Certificate for {} failed (attempt {}): {}. Next attempt {}",
                        domain,
                        failures,
                        message,
                        next.to_rfc3339()
                    ),
                    Err(err) => tracing::warn!(
                        "[cert] Certificate for {} failed: {} (could not record: {})",
                        domain,
                        message,
                        err
                    ),
                }
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

    async fn test_db(dir: &TempDir) -> SqlitePool {
        let url = format!("sqlite:{}?mode=rwc", dir.path().join("t.db").display());
        let pool = SqlitePool::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("sqlite: {err}"));
        sqlx::query(
            "CREATE TABLE certificates (
                id TEXT PRIMARY KEY, domain TEXT NOT NULL UNIQUE,
                cert_type TEXT NOT NULL DEFAULT 'acme', cert_path TEXT NOT NULL, key_path TEXT NOT NULL,
                issued_at TEXT NOT NULL, expires_at TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_error TEXT, last_attempt_at TEXT, next_retry_at TEXT,
                failure_count INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("create table: {err}"));
        pool
    }

    #[tokio::test]
    async fn record_failure_persists_backoff_and_round_trips_through_load() {
        let dir = TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let db = test_db(&dir).await;
        let now = Utc::now();

        let next = record_failure(&db, dir.path(), "x.com", "failed", "boom", 3, now)
            .await
            .unwrap_or_else(|err| panic!("record_failure: {err}"));
        assert_eq!(next - now, chrono::Duration::minutes(4));

        // A second failure updates the same row.
        record_failure(
            &db,
            dir.path(),
            "x.com",
            "pending",
            &"e".repeat(2000),
            4,
            now,
        )
        .await
        .unwrap_or_else(|err| panic!("record_failure: {err}"));

        let states = load_retry_states(&db)
            .await
            .unwrap_or_else(|err| panic!("load: {err}"));
        let state = states.get("x.com").cloned().unwrap_or_default();
        assert_eq!(state.failure_count, 4);
        assert!(!state.is_due(now), "the next attempt is in the future");

        let (status, error): (String, String) =
            sqlx::query_as("SELECT status, last_error FROM certificates WHERE domain = 'x.com'")
                .fetch_one(&db)
                .await
                .unwrap_or_else(|err| panic!("read: {err}"));
        assert_eq!(status, "pending");
        assert!(
            error.chars().count() <= MAX_ERROR_LEN + 1,
            "errors are truncated"
        );
    }
}
