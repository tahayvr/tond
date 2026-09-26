// auth/rate_limit.rs — client IP resolution, request rate limiting and login backoff
//
//   client_ip(parts)
//     The address to attribute a request to. tond-proxy replaces any
//     client-supplied `X-Forwarded-For` with the TCP peer it accepted, so the
//     header is only believed when the TCP peer is loopback (the proxy); a
//     direct remote peer is attributed to its own address. Requests on the
//     root-only Unix socket have no client IP and are never limited.
//
//   RateLimiter
//     Shared (Arc) throttling state kept in AppState:
//       - a per-client token bucket for every API request (RateLimit extractor)
//       - failed-login backoff, per client and per account (login, 2FA verify)
//     All maps are bounded: idle entries are evicted first, then the least
//     recently seen, so a flood of distinct addresses can't grow memory.
//     IPv6 clients are grouped by /64, the smallest block a host usually gets.
//
//   RateLimit (Axum extractor)
//     Adding `_rate: RateLimit` to a handler rejects the request with 429 when
//     the client's bucket is empty.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{ConnectInfo, FromRequestParts},
    http::{StatusCode, request::Parts},
};
use tokio::sync::Mutex;
use tond_common::ApiResponse;

use super::LocalSocket;
use crate::state::AppState;

/// Maximum number of clients tracked by the request limiter.
const MAX_TRACKED_CLIENTS: usize = 10_000;
/// Maximum number of clients / accounts tracked by the login backoff.
const MAX_TRACKED_LOGIN_KEYS: usize = 10_000;

/// Failed logins from one client before backoff starts.
const CLIENT_FREE_FAILURES: u32 = 5;
/// Failed logins against one account (from anywhere) before backoff starts.
/// Higher than the per-client limit so one attacker can't lock the owner out
/// quickly; the CLI over the Unix socket is never affected.
const ACCOUNT_FREE_FAILURES: u32 = 10;
/// First lockout; doubles with every further failure.
const LOCKOUT_BASE: Duration = Duration::from_secs(2);
/// Longest lockout.
const LOCKOUT_MAX: Duration = Duration::from_secs(15 * 60);
/// Failures are forgotten after this long without a new one.
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);

// ── Client address ──────────────────────────────────────────────────────────

/// Resolve the client address of a request (see module docs).
/// Returns None for Unix-socket requests and when no peer is known.
pub fn client_ip(parts: &Parts) -> Option<IpAddr> {
    if parts.extensions.get::<LocalSocket>().is_some() {
        return None;
    }
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())?;
    if !peer.is_loopback() {
        return Some(peer);
    }
    // Loopback peer: tond-proxy (or a local process). The proxy overwrites
    // X-Forwarded-For with the address it accepted; take the last hop.
    let forwarded = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|v| v.trim().parse::<IpAddr>().ok());
    Some(forwarded.unwrap_or(peer))
}

/// Extractor for the resolved client address (None on the Unix socket).
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip(parts)))
    }
}

/// Throttling key for a client: the address, or its /64 for IPv6.
pub fn client_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
    }
}

// ── Bounded map ─────────────────────────────────────────────────────────────

struct Slot<V> {
    value: V,
    last_seen: Instant,
}

/// A HashMap with a size cap. Before inserting a new key into a full map,
/// entries idle for longer than `idle_ttl` are dropped; if that frees nothing,
/// the least recently seen entry is evicted.
struct BoundedMap<V> {
    entries: HashMap<String, Slot<V>>,
    max_entries: usize,
    idle_ttl: Duration,
}

impl<V> BoundedMap<V> {
    fn new(max_entries: usize, idle_ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: max_entries.max(1),
            idle_ttl,
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.entries.get(key).map(|slot| &slot.value)
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }

    fn entry(&mut self, key: &str, now: Instant, init: impl FnOnce() -> V) -> &mut V {
        if !self.entries.contains_key(key) && self.entries.len() >= self.max_entries {
            let ttl = self.idle_ttl;
            self.entries
                .retain(|_, slot| now.saturating_duration_since(slot.last_seen) < ttl);
            if self.entries.len() >= self.max_entries
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, slot)| slot.last_seen)
                    .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let slot = self.entries.entry(key.to_string()).or_insert_with(|| Slot {
            value: init(),
            last_seen: now,
        });
        slot.last_seen = now;
        &mut slot.value
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

// ── Limiter ─────────────────────────────────────────────────────────────────

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Default)]
struct Failures {
    count: u32,
    last_failure: Option<Instant>,
    locked_until: Option<Instant>,
}

impl Failures {
    /// Forget failures older than the window.
    fn expire(&mut self, now: Instant) {
        if self
            .last_failure
            .is_some_and(|t| now.saturating_duration_since(t) >= FAILURE_WINDOW)
        {
            *self = Failures::default();
        }
    }

    fn retry_after(&self, now: Instant) -> Option<Duration> {
        self.locked_until
            .filter(|until| *until > now)
            .map(|until| until - now)
    }

    fn record(&mut self, now: Instant, free_failures: u32) {
        self.expire(now);
        self.count = self.count.saturating_add(1);
        self.last_failure = Some(now);
        if self.count >= free_failures {
            let exponent = (self.count - free_failures).min(20);
            let lock = LOCKOUT_BASE
                .saturating_mul(1u32 << exponent)
                .min(LOCKOUT_MAX);
            self.locked_until = Some(now + lock);
        }
    }
}

struct LoginState {
    clients: BoundedMap<Failures>,
    accounts: BoundedMap<Failures>,
}

/// Per-client request limiter plus failed-login backoff. Cheap to clone.
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<Mutex<BoundedMap<Bucket>>>,
    login: Arc<Mutex<LoginState>>,
    capacity: u32,
    refill_rate: f64, // tokens per second
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// - `capacity`: max burst size per client
    /// - `refill_rate`: tokens restored per second
    pub fn new(capacity: u32, refill_rate: f64) -> Self {
        // A bucket idle long enough to refill completely is identical to a new one.
        let refill_secs = if refill_rate > 0.0 {
            (capacity as f64 / refill_rate).clamp(1.0, 3600.0)
        } else {
            3600.0
        };
        Self {
            buckets: Arc::new(Mutex::new(BoundedMap::new(
                MAX_TRACKED_CLIENTS,
                Duration::from_secs_f64(refill_secs),
            ))),
            login: Arc::new(Mutex::new(LoginState {
                clients: BoundedMap::new(MAX_TRACKED_LOGIN_KEYS, FAILURE_WINDOW),
                accounts: BoundedMap::new(MAX_TRACKED_LOGIN_KEYS, FAILURE_WINDOW),
            })),
            capacity,
            refill_rate,
        }
    }

    /// Try to consume one token for the given client key. Returns true if allowed.
    pub async fn check(&self, key: &str) -> bool {
        let mut buckets = self.buckets.lock().await;
        let now = Instant::now();
        let capacity = self.capacity as f64;

        let bucket = buckets.entry(key, now, || Bucket {
            tokens: capacity,
            last_refill: now,
        });

        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_rate).min(capacity);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// If logins from this client or for this account are locked out, how long until
    /// the next attempt is allowed. `account` is the normalized email (or user id).
    pub async fn login_retry_after(
        &self,
        client: Option<IpAddr>,
        account: Option<&str>,
    ) -> Option<Duration> {
        let login = self.login.lock().await;
        let now = Instant::now();
        let by_client = client
            .and_then(|ip| login.clients.get(&client_key(ip)))
            .and_then(|f| f.retry_after(now));
        let by_account = account
            .and_then(|a| login.accounts.get(a))
            .and_then(|f| f.retry_after(now));
        by_client.max(by_account)
    }

    /// Record a failed password or 2FA attempt. Pass `account` only for an
    /// existing account, so made-up emails can't fill the account map.
    pub async fn record_login_failure(&self, client: Option<IpAddr>, account: Option<&str>) {
        let mut login = self.login.lock().await;
        let now = Instant::now();
        if let Some(ip) = client {
            login
                .clients
                .entry(&client_key(ip), now, Failures::default)
                .record(now, CLIENT_FREE_FAILURES);
        }
        if let Some(account) = account {
            login
                .accounts
                .entry(account, now, Failures::default)
                .record(now, ACCOUNT_FREE_FAILURES);
        }
    }

    /// Clear the account's failure count after a successful login.
    pub async fn record_login_success(&self, account: &str) {
        self.login.lock().await.accounts.remove(account);
    }
}

/// Axum extractor that enforces rate limiting per client.
pub struct RateLimit;

impl FromRequestParts<AppState> for RateLimit {
    type Rejection = (StatusCode, Json<ApiResponse<()>>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The CLI on the root-only socket is not throttled.
        if parts.extensions.get::<LocalSocket>().is_some() {
            return Ok(RateLimit);
        }
        let key = client_ip(parts)
            .map(client_key)
            .unwrap_or_else(|| "unknown".to_string());

        if state.rate_limiter.check(&key).await {
            Ok(RateLimit)
        } else {
            Err(too_many_requests("Rate limit exceeded. Try again later."))
        }
    }
}

/// 429 response with the given message.
pub fn too_many_requests(message: &str) -> (StatusCode, Json<ApiResponse<()>>) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(ApiResponse::err(message)),
    )
}

/// Human-friendly lockout message for a failed-login backoff.
pub fn lockout_message(retry_after: Duration) -> String {
    let secs = retry_after.as_secs().max(1);
    let wait = if secs >= 120 {
        format!("{} minutes", secs.div_ceil(60))
    } else {
        format!("{secs} seconds")
    };
    format!("Too many failed sign-in attempts. Try again in {wait}.")
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderValue, Request};
    use tokio::time::{Duration, sleep};

    use super::*;

    fn parts_with(peer: Option<&str>, xff: Option<&str>) -> Parts {
        let mut parts = Request::builder()
            .uri("/api/auth/login")
            .body(())
            .unwrap_or_else(|err| panic!("failed to build request: {err}"))
            .into_parts()
            .0;
        if let Some(peer) = peer {
            let addr: SocketAddr = peer
                .parse()
                .unwrap_or_else(|err| panic!("bad peer addr {peer}: {err}"));
            parts.extensions.insert(ConnectInfo(addr));
        }
        if let Some(xff) = xff {
            parts.headers.insert(
                "x-forwarded-for",
                HeaderValue::from_str(xff).unwrap_or_else(|err| panic!("bad header: {err}")),
            );
        }
        parts
    }

    #[tokio::test]
    async fn rate_limiter_allows_up_to_capacity_then_blocks() {
        let limiter = RateLimiter::new(2, 0.0);

        assert!(limiter.check("203.0.113.1").await);
        assert!(limiter.check("203.0.113.1").await);
        assert!(
            !limiter.check("203.0.113.1").await,
            "requests beyond the configured token capacity should be rejected"
        );
    }

    #[tokio::test]
    async fn rate_limiter_refills_tokens_over_time() {
        let limiter = RateLimiter::new(1, 100.0);

        assert!(limiter.check("203.0.113.1").await);
        assert!(!limiter.check("203.0.113.1").await);
        sleep(Duration::from_millis(20)).await;
        assert!(
            limiter.check("203.0.113.1").await,
            "the limiter should allow requests again after a token was refilled"
        );
    }

    #[tokio::test]
    async fn rate_limiter_tracks_ips_independently() {
        let limiter = RateLimiter::new(1, 0.0);

        assert!(limiter.check("203.0.113.1").await);
        assert!(limiter.check("198.51.100.5").await);
        assert!(!limiter.check("203.0.113.1").await);
    }

    #[test]
    fn bounded_map_never_exceeds_its_cap() {
        let mut map: BoundedMap<u32> = BoundedMap::new(3, Duration::from_secs(3600));
        let start = Instant::now();
        for i in 0..100u64 {
            *map.entry(&format!("k{i}"), start + Duration::from_millis(i), || 0) += 1;
        }
        assert_eq!(map.len(), 3, "the map must stay at its configured cap");
        assert!(map.get("k99").is_some(), "the newest key must be kept");
        assert!(
            map.get("k0").is_none(),
            "the least recently seen key is evicted"
        );
    }

    #[test]
    fn bounded_map_evicts_idle_entries_first() {
        let mut map: BoundedMap<u32> = BoundedMap::new(2, Duration::from_secs(10));
        let start = Instant::now();
        map.entry("idle", start, || 1);
        map.entry("busy", start + Duration::from_secs(5), || 2);
        map.entry("busy", start + Duration::from_secs(12), || 2);
        map.entry("new", start + Duration::from_secs(12), || 3);
        assert!(map.get("idle").is_none());
        assert!(map.get("busy").is_some());
        assert!(map.get("new").is_some());
    }

    #[test]
    fn client_ip_ignores_forwarded_for_from_non_loopback_peers() {
        // A remote peer can't choose its own identity by sending a header.
        let parts = parts_with(Some("198.51.100.7:4000"), Some("203.0.113.9"));
        assert_eq!(
            client_ip(&parts),
            "198.51.100.7".parse::<IpAddr>().ok(),
            "X-Forwarded-For from a non-proxy peer must be ignored"
        );
    }

    #[test]
    fn client_ip_uses_proxy_set_forwarded_for_from_loopback() {
        let parts = parts_with(Some("127.0.0.1:4000"), Some("203.0.113.9"));
        assert_eq!(client_ip(&parts), "203.0.113.9".parse::<IpAddr>().ok());

        let no_header = parts_with(Some("127.0.0.1:4000"), None);
        assert_eq!(client_ip(&no_header), "127.0.0.1".parse::<IpAddr>().ok());

        let garbage = parts_with(Some("127.0.0.1:4000"), Some("not-an-ip"));
        assert_eq!(
            client_ip(&garbage),
            "127.0.0.1".parse::<IpAddr>().ok(),
            "an unparsable header falls back to the peer, never to a shared bucket"
        );
    }

    #[test]
    fn client_ip_is_none_on_the_local_socket() {
        let mut parts = parts_with(None, Some("203.0.113.9"));
        parts.extensions.insert(LocalSocket);
        assert_eq!(client_ip(&parts), None);
    }

    #[test]
    fn client_key_groups_ipv6_by_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1"
            .parse()
            .unwrap_or_else(|err| panic!("{err}"));
        let b: IpAddr = "2001:db8:1:2:ffff::9"
            .parse()
            .unwrap_or_else(|err| panic!("{err}"));
        let c: IpAddr = "2001:db8:1:3::1"
            .parse()
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(client_key(a), client_key(b));
        assert_ne!(client_key(a), client_key(c));
        let mapped: IpAddr = "::ffff:203.0.113.4"
            .parse()
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(client_key(mapped), "203.0.113.4");
    }

    #[tokio::test]
    async fn login_backoff_locks_a_client_after_repeated_failures() {
        let limiter = RateLimiter::new(60, 10.0);
        let ip: IpAddr = "203.0.113.1".parse().unwrap_or_else(|err| panic!("{err}"));
        for _ in 0..CLIENT_FREE_FAILURES - 1 {
            limiter.record_login_failure(Some(ip), None).await;
            assert!(limiter.login_retry_after(Some(ip), None).await.is_none());
        }
        limiter.record_login_failure(Some(ip), None).await;
        assert!(
            limiter.login_retry_after(Some(ip), None).await.is_some(),
            "the client must be locked out after {CLIENT_FREE_FAILURES} failures"
        );
        let other: IpAddr = "203.0.113.2".parse().unwrap_or_else(|err| panic!("{err}"));
        assert!(limiter.login_retry_after(Some(other), None).await.is_none());
    }

    #[tokio::test]
    async fn login_backoff_locks_an_account_across_clients_and_resets_on_success() {
        let limiter = RateLimiter::new(60, 10.0);
        for i in 0..ACCOUNT_FREE_FAILURES {
            // A different address each time: rotating IPs must not help.
            let ip: IpAddr = format!("198.51.100.{i}")
                .parse()
                .unwrap_or_else(|err| panic!("{err}"));
            limiter
                .record_login_failure(Some(ip), Some("admin@example.com"))
                .await;
        }
        let fresh: IpAddr = "192.0.2.200".parse().unwrap_or_else(|err| panic!("{err}"));
        assert!(
            limiter
                .login_retry_after(Some(fresh), Some("admin@example.com"))
                .await
                .is_some(),
            "the account must be locked regardless of the client address"
        );
        limiter.record_login_success("admin@example.com").await;
        assert!(
            limiter
                .login_retry_after(Some(fresh), Some("admin@example.com"))
                .await
                .is_none()
        );
    }

    #[test]
    fn lockout_grows_exponentially_and_is_capped() {
        let now = Instant::now();
        let mut f = Failures::default();
        let mut locks = Vec::new();
        for _ in 0..30 {
            f.record(now, 1);
            locks.push(f.retry_after(now).unwrap_or_default());
        }
        assert_eq!(locks[0], LOCKOUT_BASE);
        assert_eq!(locks[1], LOCKOUT_BASE * 2);
        assert_eq!(*locks.last().unwrap_or(&Duration::ZERO), LOCKOUT_MAX);
    }

    #[test]
    fn lockout_message_is_readable() {
        assert_eq!(
            lockout_message(Duration::from_secs(4)),
            "Too many failed sign-in attempts. Try again in 4 seconds."
        );
        assert_eq!(
            lockout_message(Duration::from_secs(600)),
            "Too many failed sign-in attempts. Try again in 10 minutes."
        );
    }
}
