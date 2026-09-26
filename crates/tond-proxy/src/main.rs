// main.rs — tond-proxy: Pingora TLS reverse proxy
//
// This binary sits in front of all user traffic. It owns port 443 (TLS)
// and port 80 (HTTP redirect + ACME challenges). It does NOT share a
// process with tond-server; it shares only the SQLite database file.
//
// Architecture:
//
//   Port 443 — TondProxy (Pingora ProxyHttp)
//     Per-request routing: reads the Host header, looks it up in the
//     in-memory RouteMap (domain -> 127.0.0.1:{host_port}), and forwards
//     the request to the correct running container. If the host matches
//     dashboard_domain, it routes to tond-server (127.0.0.1:3001).
//     An app has any number of domains (app_domains): primary and aliases
//     are proxied; redirect domains are answered here with a 301/308 to
//     their target domain, path and query preserved, without touching the
//     container (port 80 sends them straight to the target over HTTPS).
//     If no route is found, `request_filter` short-circuits and returns a
//     friendly HTML 502 error page directly (no upstream connection made).
//
//     Preview isolation: hosts under preview_domain (a separate registrable
//     domain, validated by tond-server) are answered ONLY from preview routes
//     — never the dashboard/API or an app — and get a plain 404 when no
//     preview matches. Preview routes are loaded only if they live under
//     preview_domain, so a preview is never served on the dashboard domain.
//
//   TLS — TondTlsAccept (Pingora TlsAccept)
//     Implements SNI-based certificate selection. On each TLS handshake
//     it reads the server name from the ClientHello, looks up the cert
//     in CertStore (exact name, else the wildcard one label up), and
//     injects the right cert+key into the SSL context.
//     Falls back to a self-signed cert for unknown domains.
//
//   Port 80 — run_http_server() (raw Tokio TCP)
//     Both ports listen on IPv4 (0.0.0.0) and, when the host has IPv6, on
//     [::] with IPV6_V6ONLY (a host booted without IPv6 gets IPv4 only).
//     A hand-rolled HTTP/1.1 server. Three behaviours:
//       /.well-known/acme-challenge/{token} — serves ACME HTTP-01 responses
//       a host Tond serves                  — 301 redirect to https://
//       anything else                       — 404 (no open redirect)
//     Hardened against slow clients: the request head must arrive within
//     HTTP_REQUEST_TIMEOUT, at most HTTP_MAX_CONNECTIONS are served at once,
//     and accept() errors never stop the listener (renewals depend on it).
//
//   Background thread (native OS thread + current-thread Tokio runtime):
//     - Routing: one persistent read-only SQLite connection (mode=ro; tond.db
//       is 0640 root:tond-proxy, see tond-server's db.rs). tond-server bumps
//       `route_state.version` (SQL triggers) and nudges the proxy over the
//       control socket; the proxy reloads — running apps, compose service
//       domains (app_domains -> compose_services.host_port) and previews with a
//       stored host_port, plus dashboard_domain / preview_domain — only when
//       the version moved, in one read transaction, and acknowledges with the
//       version it loaded. A deploy stops the old container only after that
//       acknowledgement. A fallback check every FALLBACK_POLL_SECS (60 s)
//       catches a missed nudge. The proxy never talks to Docker (it runs
//       unprivileged, see tond-proxy.service).
//     - Control socket (control.rs): <data dir>/proxy.sock, owner-only,
//       root/own-uid peers only.
//     - Certificate loop (issuance.rs): event-driven (route changes, "Retry
//       now", the earliest due retry), issues or renews certs for routed
//       domains and the dashboard domain with per-domain exponential backoff
//       and a DNS pre-check. Results go to tond-server over the control
//       socket; tond-proxy never writes the database.
//
// Request metrics (traffic.rs): every request is counted per route (app,
// preview, dashboard, one bucket for unknown hosts) in Pingora's `logging()`
// hook — status class, bytes, a fixed latency histogram — with atomic
// increments only. tond-server reads the totals over the control socket.
// Opt-in per-app access logs (access_log.rs) are queued to a writer task.
//
// Logging: nothing is logged per request at info (routing decisions are at
// debug), and the route refresh only logs when a route is added, removed or
// changed. Set RUST_LOG=tond_proxy=debug to trace routing.
//
// Environment variables consumed here:
//   TOND_DB_PATH          — SQLite database path
//   TOND_DATA_DIR         — base dir for certs and ACME account creds
//   TOND_SERVER_ADDR      — tond-server address (default: 127.0.0.1:3001)
//   TOND_PROXY_SOCKET     — control socket (default: <TOND_DATA_DIR>/proxy.sock)
//   TOND_ACME_STAGING     — "1" or "true" to use Let's Encrypt staging
//
// (TOND_DASHBOARD_DOMAIN moved to tond-server, which seeds the setting; the
// proxy reads the dashboard domain from the database like everything else.)

mod access_log;
mod acme;
mod certs;
mod control;
mod dns01;
mod issuance;
mod log_limit;
mod meters;
mod traffic;

use anyhow::Result;
use async_trait::async_trait;
use pingora::listeners::TlsAccept;
use pingora::listeners::tls::TlsSettings;
use pingora::prelude::*;
use pingora::proxy::{ProxyHttp, Session};
use pingora::tls::ext::{ssl_add_chain_cert, ssl_use_certificate, ssl_use_private_key};
use pingora::tls::ssl::NameType;
use pingora::upstreams::peer::HttpPeer;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock, Semaphore};
use tond_common::proxy_control::FALLBACK_POLL_SECS;
use tond_common::traffic::RouteKind;

use crate::access_log::{AccessConfig, AccessLog};
use crate::acme::ChallengeMap;
use crate::certs::CertStore;
use crate::dns01::DnsBridge;
use crate::issuance::{CertConfig, IssuanceLoop, Outbox, SharedCertConfig};
use crate::traffic::{Registry, RouteStats};

/// A redirect domain: answered by the proxy itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// Target host (another domain of the same app).
    pub to: String,
    /// 301 or 308.
    pub code: u16,
}

/// One routed domain.
#[derive(Clone)]
pub struct Route {
    /// host:port of the container.
    pub upstream: String,
    /// The app's / preview's request counters.
    pub stats: Arc<RouteStats>,
    /// Access logging, when the app opted in.
    pub access: Option<AccessConfig>,
    /// Set for redirect domains: no upstream request is made.
    pub redirect: Option<Redirect>,
}

impl Route {
    /// What the route does, for change logs.
    fn describe(&self) -> String {
        match &self.redirect {
            Some(r) => format!("{} redirect to {}", r.code, r.to),
            None => self.upstream.clone(),
        }
    }
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// `Location` for a redirect domain: the target host with the request's
/// path and query (an origin-form path only; anything else becomes `/`).
fn redirect_location(to: &str, path_and_query: &str) -> String {
    let path = if path_and_query.starts_with('/') {
        path_and_query
    } else {
        "/"
    };
    format!("https://{to}{path}")
}

/// Route map: domain -> route
type RouteMap = Arc<RwLock<HashMap<String, Arc<Route>>>>;

/// Shared, live dashboard domain — updated on every DB poll.
type DashboardDomain = Arc<RwLock<Option<String>>>;

/// Shared, live preview domain — updated on every DB poll.
type PreviewDomain = Arc<RwLock<Option<String>>>;

// ── TLS Accept Callback ──

/// SNI-based certificate selection during TLS handshake.
struct TondTlsAccept {
    cert_store: CertStore,
}

#[async_trait]
impl TlsAccept for TondTlsAccept {
    async fn certificate_callback(&self, ssl: &mut pingora::protocols::tls::TlsRef) {
        // Copy the SNI hostname before mutating ssl
        let sni = ssl
            .servername(NameType::HOST_NAME)
            .unwrap_or("unknown")
            .to_string();

        let pair = self.cert_store.get(&sni).await;

        if let Err(e) = ssl_use_certificate(ssl, &pair.cert) {
            tracing::error!("Failed to set certificate for {}: {}", sni, e);
            return;
        }
        for intermediate in &pair.chain {
            if let Err(e) = ssl_add_chain_cert(ssl, intermediate) {
                tracing::error!("Failed to add chain certificate for {}: {}", sni, e);
                return;
            }
        }
        if let Err(e) = ssl_use_private_key(ssl, &pair.key) {
            tracing::error!("Failed to set private key for {}: {}", sni, e);
        }
    }
}

// ── Route database ──

/// The proxy's single, persistent, read-only connection to tond.db and the
/// route_state.version its routing table corresponds to.
pub struct RouteDb {
    pool: SqlitePool,
    loaded_version: AtomicI64,
    reloads: AtomicU64,
    /// Serialises reloads (control socket and fallback poll).
    lock: tokio::sync::Mutex<()>,
}

impl RouteDb {
    /// Open lazily: the file may not exist until tond-server first starts.
    fn open(db_path: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(&format!("sqlite:{db_path}"))?
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_lazy_with(options);
        Ok(Self {
            pool,
            loaded_version: AtomicI64::new(-1),
            reloads: AtomicU64::new(0),
            lock: tokio::sync::Mutex::new(()),
        })
    }

    fn loaded_version(&self) -> i64 {
        self.loaded_version.load(Ordering::Relaxed)
    }

    fn reloads(&self) -> u64 {
        self.reloads.load(Ordering::Relaxed)
    }
}

// ── Proxy ──

#[derive(Clone)]
struct TondProxy {
    routes: RouteMap,
    db: Arc<RouteDb>,
    /// Live dashboard domain, reloaded with the routes.
    /// None means no dashboard domain is configured.
    dashboard_domain: DashboardDomain,
    /// Live preview domain — previews are served only under it, and nothing
    /// else is. None means previews are disabled.
    preview_domain: PreviewDomain,
    /// Address of the tond-server instance (default: 127.0.0.1:3001).
    server_addr: String,
    /// Poked when the set of served domains changes (certificate loop).
    domains_changed: Arc<Notify>,
    /// Per-route request counters.
    traffic: Arc<Registry>,
    /// Opt-in access logs.
    access: Arc<AccessLog>,
    /// Apps / preview domains and DNS provider zones for the certificate loop.
    cert_config: SharedCertConfig,
}

/// Per-request state (lives inline in Pingora's session: no allocation).
pub struct RequestCtx {
    started: Instant,
    /// Counters this request is recorded in (set by request_filter).
    stats: Option<Arc<RouteStats>>,
    route: Option<Arc<Route>>,
    dashboard: bool,
    upstream_started: Option<Instant>,
    upstream_ttfb: Option<Duration>,
}

#[derive(Debug, PartialEq, Eq)]
enum RouteTarget<V = Arc<Route>> {
    Dashboard,
    Upstream(V),
    /// Unknown host: friendly 502 page.
    Missing,
    /// Host under the preview domain without a matching preview: plain 404.
    NotFound,
}

/// `host` equals `domain` or is a subdomain of it.
fn is_under_domain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn normalize_host(host_header: Option<&str>) -> String {
    host_str(host_header).to_string()
}

/// The host without a port, borrowed (the per-request path avoids allocating).
fn host_str(host_header: Option<&str>) -> &str {
    host_header
        .map(|h| h.split(':').next().unwrap_or(h))
        .unwrap_or_default()
}

/// The request's Host header, without the port.
fn request_host(session: &Session) -> &str {
    host_str(
        session
            .req_header()
            .headers
            .get("host")
            .and_then(|v| v.to_str().ok()),
    )
}

/// Status for a failed proxy attempt: 504 for upstream timeouts, otherwise
/// what Pingora answers by default (502 upstream, 400 client, 500 internal,
/// 0 when the client is gone).
fn failure_status(e: &pingora::Error) -> u16 {
    use pingora::{ErrorSource, ErrorType};
    match e.etype() {
        ErrorType::HTTPStatus(code) => *code,
        ErrorType::ConnectTimedout | ErrorType::ReadTimedout | ErrorType::WriteTimedout
            if e.esource() == &ErrorSource::Upstream =>
        {
            504
        }
        _ => match e.esource() {
            ErrorSource::Upstream => 502,
            ErrorSource::Downstream => match e.etype() {
                ErrorType::WriteError | ErrorType::ReadError | ErrorType::ConnectionClosed => 0,
                _ => 400,
            },
            ErrorSource::Internal | ErrorSource::Unset => 500,
        },
    }
}

fn resolve_route<V: Clone>(
    host: &str,
    dashboard_domain: Option<&str>,
    preview_domain: Option<&str>,
    routes: &HashMap<String, V>,
) -> RouteTarget<V> {
    // Checked first: the preview domain never reaches the dashboard/API.
    // `routes` only holds hosts under the preview domain that are previews
    // (refresh_routes drops app domains under it).
    if let Some(pd) = preview_domain
        && is_under_domain(host, pd)
    {
        return routes
            .get(host)
            .cloned()
            .map(RouteTarget::Upstream)
            .unwrap_or(RouteTarget::NotFound);
    }

    if dashboard_domain == Some(host) {
        return RouteTarget::Dashboard;
    }

    routes
        .get(host)
        .cloned()
        .map(RouteTarget::Upstream)
        .unwrap_or(RouteTarget::Missing)
}

fn parse_upstream_addr(upstream: &str, fallback_port: u16) -> Option<(&str, u16)> {
    let parts: Vec<&str> = upstream.split(':').collect();
    if parts.len() != 2 {
        return None;
    }

    Some((parts[0], parts[1].parse().unwrap_or(fallback_port)))
}

/// Set the forwarding headers on a request bound for tond-server.
///
/// tond-server attributes requests from a loopback peer (this proxy) to the
/// address in `X-Forwarded-For` for rate limiting and login backoff, so any
/// client-supplied value is replaced, never appended to. `X-Forwarded-Proto`
/// tells it the request arrived over TLS (it sends HSTS only then).
fn set_dashboard_forwarding_headers(
    req: &mut pingora::http::RequestHeader,
    client_ip: Option<std::net::IpAddr>,
) -> pingora::Result<()> {
    req.remove_header("x-forwarded-for");
    req.remove_header("x-real-ip");
    req.remove_header("forwarded");
    if let Some(ip) = client_ip {
        req.insert_header("X-Forwarded-For", ip.to_string())?;
    }
    req.insert_header("X-Forwarded-Proto", "https")?;
    Ok(())
}

fn parse_http_request_line_and_host(request: &str) -> (&str, &str) {
    let first_line = request.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    let path = if parts.len() >= 2 { parts[1] } else { "/" };

    let host = request
        .lines()
        .find(|line| line.to_lowercase().starts_with("host:"))
        .map(|line| line[5..].trim())
        .unwrap_or("localhost");

    (path, host)
}

/// Escape text for inclusion in HTML.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// A syntactically valid DNS hostname (what we are willing to put in a
/// Location header).
fn is_valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

const HTTP_NOT_FOUND: &str =
    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_BAD_REQUEST: &str =
    "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Shared state for the port-80 server.
#[derive(Clone)]
struct HttpContext {
    challenges: ChallengeMap,
    routes: RouteMap,
    dashboard_domain: DashboardDomain,
    preview_domain: PreviewDomain,
}

impl HttpContext {
    /// Build the response for one request head: an ACME challenge answer, a
    /// redirect to HTTPS for hosts Tond serves, or 404/400 for anything else
    /// (redirecting arbitrary Host headers would be an open redirect).
    async fn respond(&self, request: &str) -> String {
        let (path, host) = parse_http_request_line_and_host(request);

        if let Some(token) = path.strip_prefix("/.well-known/acme-challenge/") {
            let map = self.challenges.read().await;
            if let Some(key_auth) = map.get(token) {
                return format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    key_auth.len(),
                    key_auth
                );
            }
            return HTTP_NOT_FOUND.to_string();
        }

        let host = normalize_host(Some(host));
        if !is_valid_hostname(&host) {
            return HTTP_BAD_REQUEST.to_string();
        }
        // Redirect only hosts that HTTPS would actually serve: unknown hosts
        // and unmatched hosts under the preview domain get a 404.
        // Redirect domains go straight to their target (one hop, not two).
        let target = {
            let dd = self.dashboard_domain.read().await;
            let pd = self.preview_domain.read().await;
            let routes = self.routes.read().await;
            match resolve_route(&host, dd.as_deref(), pd.as_deref(), &routes) {
                RouteTarget::Dashboard => Some((host.clone(), 301)),
                RouteTarget::Upstream(route) => Some(match &route.redirect {
                    Some(r) => (r.to.clone(), r.code),
                    None => (host.clone(), 301),
                }),
                RouteTarget::Missing | RouteTarget::NotFound => None,
            }
        };
        let Some((to, code)) = target else {
            return HTTP_NOT_FOUND.to_string();
        };

        // Only an origin-form path goes into the Location header.
        let path = if path.starts_with('/') && !path.starts_with("//") {
            path
        } else {
            "/"
        };
        let reason = if code == 308 {
            "Permanent Redirect"
        } else {
            "Moved Permanently"
        };
        format!(
            "HTTP/1.1 {code} {reason}\r\nLocation: https://{to}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }
}

/// domain -> upstream view of a route map (for change logging).
fn upstreams(routes: &HashMap<String, Arc<Route>>) -> HashMap<String, String> {
    routes
        .iter()
        .map(|(d, r)| (d.clone(), r.describe()))
        .collect()
}

/// Human-readable differences between two route maps, sorted by domain:
/// "added a -> b", "removed a", "changed a: b -> c". Empty when identical.
fn route_changes(old: &HashMap<String, String>, new: &HashMap<String, String>) -> Vec<String> {
    let mut changes: Vec<(String, String)> = Vec::new();
    for (domain, upstream) in new {
        match old.get(domain) {
            None => changes.push((domain.clone(), format!("added {domain} -> {upstream}"))),
            Some(prev) if prev != upstream => changes.push((
                domain.clone(),
                format!("changed {domain}: {prev} -> {upstream}"),
            )),
            Some(_) => {}
        }
    }
    for domain in old.keys() {
        if !new.contains_key(domain) {
            changes.push((domain.clone(), format!("removed {domain}")));
        }
    }
    changes.sort();
    changes.into_iter().map(|(_, c)| c).collect()
}

impl TondProxy {
    fn new(db: Arc<RouteDb>, server_addr: String) -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            db,
            dashboard_domain: Arc::new(RwLock::new(None)),
            preview_domain: Arc::new(RwLock::new(None)),
            server_addr,
            domains_changed: Arc::new(Notify::new()),
            traffic: Arc::new(Registry::default()),
            // Replaced by with_access_log in main(); without a writer every
            // line is dropped (and counted).
            access: Arc::new(access_log::channel(std::path::Path::new("/nonexistent")).0),
            cert_config: Arc::new(RwLock::new(CertConfig::default())),
        }
    }

    fn with_access_log(mut self, access: Arc<AccessLog>) -> Self {
        self.access = access;
        self
    }

    fn route_db(&self) -> &RouteDb {
        &self.db
    }

    async fn route_count(&self) -> usize {
        self.routes.read().await.len()
    }

    /// Reload the routing table if route_state.version moved. Returns the
    /// loaded version and the number of routes.
    async fn sync(&self) -> Result<(i64, u64)> {
        let meter = meters::register("route-sync");
        let started = Instant::now();
        let result = async {
            let _guard = self.db.lock.lock().await;
            let version: i64 = sqlx::query_scalar("SELECT version FROM route_state WHERE id = 1")
                .fetch_one(&self.db.pool)
                .await?;
            if version != self.db.loaded_version() {
                self.reload_locked().await?;
            }
            anyhow::Ok((self.db.loaded_version(), self.route_count().await as u64))
        }
        .await;
        meter.ran(started, result.as_ref().err().map(|e| format!("{e:#}")));
        result
    }

    async fn resolve(&self, host: &str) -> RouteTarget<Arc<Route>> {
        let dd = self.dashboard_domain.read().await;
        let pd = self.preview_domain.read().await;
        let routes = self.routes.read().await;
        resolve_route(host, dd.as_deref(), pd.as_deref(), &routes)
    }

    /// Reload routes and settings from SQLite unconditionally.
    async fn refresh_routes(&self) -> Result<()> {
        let _guard = self.db.lock.lock().await;
        self.reload_locked().await
    }

    /// Reload (caller holds `db.lock`). Everything is read in one read
    /// transaction, so the table matches exactly one route_state.version.
    ///
    /// Reads the host_port stored in the DB (written by tond-server on every
    /// deploy), which makes routing deterministic across proxy restarts and
    /// keeps the proxy away from the Docker socket. Rows without a host_port
    /// (a deploy in progress) are skipped.
    ///
    /// Also reads `dashboard_domain` and `preview_domain` from the settings
    /// table so that changes made via the dashboard UI or `tond settings`
    /// CLI commands are picked up without restarting the proxy.
    async fn reload_locked(&self) -> Result<()> {
        let mut tx = self.db.pool.begin().await?;
        let version: i64 = sqlx::query_scalar("SELECT version FROM route_state WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;

        let settings: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM settings
             WHERE key IN ('dashboard_domain', 'preview_domain', 'apps_domain')",
        )
        .fetch_all(&mut *tx)
        .await?;
        let setting = |key: &str| {
            settings
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let new_preview_domain = setting("preview_domain");
        let mut domains_changed = false;
        {
            let new_domain = setting("dashboard_domain");
            let mut dd = self.dashboard_domain.write().await;
            if *dd != new_domain {
                tracing::info!("Dashboard domain updated: {:?} -> {:?}", *dd, new_domain);
                *dd = new_domain;
                domains_changed = true;
            }
        }
        {
            let mut pd = self.preview_domain.write().await;
            if *pd != new_preview_domain {
                tracing::info!(
                    "Preview domain updated: {:?} -> {:?}",
                    *pd,
                    new_preview_domain
                );
                *pd = new_preview_domain.clone();
            }
        }
        let under_preview_domain = |host: &str| {
            new_preview_domain
                .as_deref()
                .is_some_and(|pd| is_under_domain(host, pd))
        };

        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            Option<String>,
            i64,
            String,
            String,
            i64,
            bool,
            bool,
        )> = sqlx::query_as(
            // Background workers (readiness 'none') serve nothing: never
            // routed, even if a domain lingers. Domains of compose
            // services (service IS NOT NULL) are routed by the compose
            // workstream's per-service ports, not the app's host_port.
            "SELECT d.domain, d.kind, d.redirect_to, d.redirect_code,
                        a.id, a.name, a.host_port, a.access_log, a.access_log_query
                 FROM app_domains d JOIN apps a ON a.id = d.app_id
                 WHERE a.status = 'running' AND a.host_port IS NOT NULL
                   AND a.readiness != 'none' AND d.service IS NULL",
        )
        .fetch_all(&mut *tx)
        .await?;

        // Zones a DNS provider can publish ACME DNS-01 records in (never
        // the tokens: the proxy has no use for them and can't decrypt them).
        let zone_lists: Vec<String> = sqlx::query_scalar("SELECT zones FROM dns_providers")
            .fetch_all(&mut *tx)
            .await?;
        let mut dns_zones: Vec<String> = zone_lists
            .iter()
            .flat_map(|z| serde_json::from_str::<Vec<String>>(z).unwrap_or_default())
            .collect();
        dns_zones.sort();
        dns_zones.dedup();
        let new_cert_config = CertConfig {
            apps_domain: setting("apps_domain"),
            preview_domain: new_preview_domain.clone(),
            dns_zones,
        };
        {
            let mut cfg = self.cert_config.write().await;
            if *cfg != new_cert_config {
                *cfg = new_cert_config;
                domains_changed = true;
            }
        }

        let mut new_routes: HashMap<String, Arc<Route>> = HashMap::new();
        let mut live: Vec<(RouteKind, String)> = Vec::new();

        for (
            domain,
            kind,
            redirect_to,
            redirect_code,
            id,
            name,
            host_port,
            access_log,
            access_query,
        ) in &rows
        {
            // The preview domain is reserved for previews.
            if under_preview_domain(domain) {
                tracing::warn!(
                    "App {} uses domain {} under the preview domain — not routed",
                    name,
                    domain
                );
                continue;
            }
            let upstream = format!("127.0.0.1:{}", host_port);
            let access = access_log.then(|| AccessConfig {
                app_id: Arc::from(id.as_str()),
                include_query: *access_query,
            });
            let redirect = match (kind.as_str(), redirect_to) {
                ("redirect", Some(to)) => Some(Redirect {
                    to: to.clone(),
                    code: u16::try_from(*redirect_code).unwrap_or(308),
                }),
                _ => None,
            };
            new_routes.insert(
                domain.clone(),
                Arc::new(Route {
                    upstream,
                    stats: self.traffic.route(RouteKind::App, id),
                    access,
                    redirect,
                }),
            );
            if !live.iter().any(|(k, l)| *k == RouteKind::App && l == id) {
                live.push((RouteKind::App, id.clone()));
            }
        }

        // Compose services: domain -> (app, service) -> the loopback host port
        // tond-server allocated for the service's published container port.
        // The port is stable across deploys and compose only recreates
        // changed services, so routes stay up while the app is building.
        // Requests are counted per app (all its services share one bucket).
        #[allow(clippy::type_complexity)]
        let service_rows: Vec<(String, String, String, String, i64, bool, bool)> =
            sqlx::query_as(tond_common::route_queries::COMPOSE_SERVICE_ROUTES)
                .fetch_all(&mut *tx)
                .await?;
        for (domain, id, name, service, host_port, access_log, access_query) in &service_rows {
            if under_preview_domain(domain) {
                tracing::warn!(
                    "Service {}/{} uses domain {} under the preview domain — not routed",
                    name,
                    service,
                    domain
                );
                continue;
            }
            if new_routes.contains_key(domain) {
                continue; // an app's own domain wins over a stale service row
            }
            let access = access_log.then(|| AccessConfig {
                app_id: Arc::from(id.as_str()),
                include_query: *access_query,
            });
            new_routes.insert(
                domain.clone(),
                Arc::new(Route {
                    upstream: format!("127.0.0.1:{}", host_port),
                    stats: self.traffic.route(RouteKind::App, id),
                    access,
                    redirect: None,
                }),
            );
            if !live.iter().any(|(k, l)| *k == RouteKind::App && l == id) {
                live.push((RouteKind::App, id.clone()));
            }
        }

        // Redirect domains of compose apps: answered here, no upstream (the
        // app-domain query above only covers apps with their own host_port).
        #[allow(clippy::type_complexity)]
        let compose_redirects: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT d.domain, a.id, d.redirect_to, d.redirect_code
             FROM app_domains d JOIN apps a ON a.id = d.app_id
             WHERE d.kind = 'redirect' AND d.redirect_to IS NOT NULL
               AND a.source_type = 'compose'
               AND a.status IN ('running', 'building', 'unhealthy')",
        )
        .fetch_all(&mut *tx)
        .await?;
        for (domain, id, to, code) in compose_redirects {
            if under_preview_domain(&domain) || new_routes.contains_key(&domain) {
                continue;
            }
            new_routes.insert(
                domain,
                Arc::new(Route {
                    upstream: String::new(),
                    stats: self.traffic.route(RouteKind::App, &id),
                    access: None,
                    redirect: Some(Redirect {
                        to,
                        code: u16::try_from(code).unwrap_or(308),
                    }),
                }),
            );
        }

        // Also load active preview deployments (subdomain routing).
        // Previews always have a stored host_port — no Docker fallback needed.
        let preview_rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
            "SELECT id, subdomain, host_port FROM preview_deployments
             WHERE status = 'running'
               AND subdomain IS NOT NULL
               AND host_port IS NOT NULL
               AND expires_at > datetime('now')",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        for (id, subdomain, host_port) in preview_rows {
            if let Some(sub) = subdomain {
                // Previews are served only under the current preview domain.
                if !under_preview_domain(&sub) {
                    continue;
                }
                new_routes.insert(
                    sub,
                    Arc::new(Route {
                        upstream: format!("127.0.0.1:{}", host_port),
                        stats: self.traffic.route(RouteKind::Preview, &id),
                        access: None,
                        redirect: None,
                    }),
                );
                live.push((RouteKind::Preview, id));
            }
        }
        let live: Vec<(RouteKind, &str)> = live.iter().map(|(k, id)| (*k, id.as_str())).collect();
        self.traffic.retire_except(&live);
        self.access.routes_reloaded();

        let mut routes = self.routes.write().await;
        let changes = route_changes(&upstreams(&routes), &upstreams(&new_routes));
        if !changes.is_empty() {
            for change in &changes {
                tracing::info!("Route {}", change);
            }
            tracing::info!("Routing table now has {} route(s)", new_routes.len());
        }
        let domain_set_changed =
            routes.len() != new_routes.len() || new_routes.keys().any(|d| !routes.contains_key(d));
        *routes = new_routes;
        drop(routes);

        let first_load = self.db.loaded_version() < 0;
        self.db.loaded_version.store(version, Ordering::Relaxed);
        self.db.reloads.fetch_add(1, Ordering::Relaxed);
        if first_load || domains_changed || domain_set_changed {
            self.domains_changed.notify_one();
        }
        Ok(())
    }
}

#[async_trait]
impl ProxyHttp for TondProxy {
    type CTX = RequestCtx;

    fn new_ctx(&self) -> Self::CTX {
        RequestCtx {
            started: Instant::now(),
            stats: None,
            route: None,
            dashboard: false,
            upstream_started: None,
            upstream_ttfb: None,
        }
    }

    /// Resolve the route (once per request; later hooks use `ctx`). Unknown
    /// hosts get a friendly HTML error page written directly (Ok(true)).
    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<bool> {
        let route_target = self.resolve(request_host(session)).await;

        match route_target {
            RouteTarget::Dashboard => {
                ctx.stats = Some(self.traffic.dashboard().clone());
                ctx.dashboard = true;
                return Ok(false);
            }
            RouteTarget::Upstream(route) => {
                ctx.stats = Some(route.stats.clone());
                if let Some(redirect) = &route.redirect {
                    let location = redirect_location(
                        &redirect.to,
                        session
                            .req_header()
                            .uri
                            .path_and_query()
                            .map_or("/", |pq| pq.as_str()),
                    );
                    let mut resp = pingora::http::ResponseHeader::build(redirect.code, None)?;
                    resp.insert_header("Location", location)?;
                    resp.insert_header("Content-Length", "0")?;
                    resp.insert_header("Cache-Control", "no-store")?;
                    ctx.route = Some(route);
                    session.write_response_header(Box::new(resp), true).await?;
                    return Ok(true);
                }
                ctx.route = Some(route);
                return Ok(false);
            }
            RouteTarget::NotFound => {
                ctx.stats = Some(self.traffic.unknown().clone());
                let body = "404 Not Found\n";
                let mut resp = pingora::http::ResponseHeader::build(404, None)?;
                resp.insert_header("Content-Type", "text/plain; charset=utf-8")?;
                resp.insert_header("Content-Length", body.len().to_string())?;
                resp.insert_header("Cache-Control", "no-store")?;
                session.write_response_header(Box::new(resp), false).await?;
                session
                    .write_response_body(Some(bytes::Bytes::from_static(body.as_bytes())), true)
                    .await?;
                return Ok(true);
            }
            RouteTarget::Missing => {
                ctx.stats = Some(self.traffic.unknown().clone());
            }
        }

        let host = request_host(session).to_string();
        // No route found — return a friendly HTML 502 page. Debug only:
        // scanners hit unknown hosts constantly.
        tracing::debug!("No route found for host '{}', returning 502 page", host);

        let body = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>502 — No Route</title>
  <style>
    body {{ font-family: system-ui, sans-serif; background: #0f0f0f; color: #e5e5e5;
            display: flex; align-items: center; justify-content: center;
            min-height: 100vh; margin: 0; }}
    .card {{ text-align: center; padding: 2rem; max-width: 400px; }}
    h1 {{ font-size: 4rem; margin: 0; color: #f87171; }}
    p {{ color: #a1a1aa; margin: 0.5rem 0; }}
    code {{ background: #1c1c1c; padding: 0.2em 0.5em; border-radius: 4px;
             font-size: 0.85rem; color: #d4d4d8; }}
  </style>
</head>
<body>
  <div class="card">
    <h1>502</h1>
    <p>No route configured for <code>{host}</code>.</p>
    <p>Powered by <strong>Tond</strong></p>
  </div>
</body>
</html>"#,
            host = html_escape(&host)
        );

        let mut resp = pingora::http::ResponseHeader::build(502, None)?;
        resp.insert_header("Content-Type", "text/html; charset=utf-8")?;
        resp.insert_header("Content-Length", body.len().to_string())?;
        resp.insert_header("Cache-Control", "no-store")?;

        session.write_response_header(Box::new(resp), false).await?;
        session
            .write_response_body(Some(bytes::Bytes::from(body)), true)
            .await?;

        Ok(true) // short-circuit; do not proxy
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<Box<HttpPeer>> {
        ctx.upstream_started = Some(Instant::now());
        tracing::debug!(
            "Routing request for host {:?} -> {:?}",
            request_host(session),
            ctx.route
        );

        if ctx.dashboard {
            if let Some((addr, port)) = parse_upstream_addr(&self.server_addr, 3001) {
                return Ok(Box::new(HttpPeer::new((addr, port), false, String::new())));
            }
        } else if let Some(route) = &ctx.route
            && let Some((addr, port)) = parse_upstream_addr(&route.upstream, 8080)
        {
            return Ok(Box::new(HttpPeer::new((addr, port), false, String::new())));
        }

        // request_filter already handled the no-route case; if we somehow
        // reach here just use a dummy peer (should not happen in practice).
        tracing::warn!(
            "upstream_peer reached for unknown host '{}' — this should not happen",
            request_host(session)
        );
        let peer = Box::new(HttpPeer::new(("127.0.0.1", 9999), false, String::new()));
        Ok(peer)
    }

    /// Requests for the dashboard domain carry the TCP peer as X-Forwarded-For.
    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut pingora::http::RequestHeader,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        if ctx.dashboard {
            let client_ip = session
                .client_addr()
                .and_then(|addr| addr.as_inet())
                .map(|addr| addr.ip());
            set_dashboard_forwarding_headers(upstream_request, client_ip)?;
        }
        Ok(())
    }

    /// The upstream answered: its time to first byte.
    async fn upstream_response_filter(
        &self,
        _session: &mut Session,
        _upstream_response: &mut pingora::http::ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        if ctx.upstream_ttfb.is_none() {
            ctx.upstream_ttfb = ctx.upstream_started.map(|t| t.elapsed());
        }
        Ok(())
    }

    /// Pingora logs every failed request at ERROR; they are counted in the
    /// request metrics, so the journal gets the first one and then at most
    /// one an hour (log_limit.rs), plus a summary of what was held back.
    fn suppress_error_log(&self, session: &Session, _ctx: &Self::CTX, e: &pingora::Error) -> bool {
        tracing::debug!("Fail to proxy: {e}, Host: {}", request_host(session));
        !log_limit::LIMITER.allow(log_limit::FAIL_TO_PROXY, Instant::now(), || {
            format!("Fail to proxy: {e}, Host: {}", request_host(session))
        })
    }

    /// Pingora's default error response, except that upstream timeouts are
    /// a 504 (so request metrics can tell them from a refused connection).
    async fn fail_to_proxy(
        &self,
        session: &mut Session,
        e: &pingora::Error,
        _ctx: &mut Self::CTX,
    ) -> pingora::proxy::FailToProxy {
        let code = failure_status(e);
        if code > 0 {
            session.respond_error(code).await.unwrap_or_else(|e| {
                tracing::error!("failed to send error response to downstream: {e}");
            });
        }
        pingora::proxy::FailToProxy {
            error_code: code,
            can_reuse_downstream: false,
        }
    }

    /// Count the finished request (atomic increments, no allocation) and,
    /// if the app opted in, queue its access line.
    async fn logging(
        &self,
        session: &mut Session,
        _e: Option<&pingora::Error>,
        ctx: &mut Self::CTX,
    ) {
        let total = ctx.started.elapsed();
        let status = session.response_written().map_or(0, |r| r.status.as_u16());
        let upstream = ctx
            .upstream_ttfb
            .or_else(|| ctx.upstream_started.map(|t| t.elapsed()));
        let bytes_out = session.body_bytes_sent() as u64;
        let stats = ctx.stats.as_ref().unwrap_or(self.traffic.unknown());
        stats.record(
            status,
            session.body_bytes_read() as u64,
            bytes_out,
            total,
            upstream,
        );

        if let Some(cfg) = ctx.route.as_ref().and_then(|r| r.access.as_ref()) {
            let req = session.req_header();
            self.access.log(
                cfg,
                &access_log::Request {
                    method: req.method.as_str(),
                    path: req.uri.path(),
                    query: req.uri.query(),
                    status,
                    duration: total,
                    bytes_out,
                    client: session
                        .client_addr()
                        .and_then(|a| a.as_inet())
                        .map(|a| a.ip()),
                },
            );
        }
    }
}

fn main() {
    tond_common::install_crypto_provider();
    {
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry()
            .with(
                tond_common::logging::fmt_layer(tond_common::logging::LogFormat::from_env())
                    .with_filter(log_limit::RateLimit),
            )
            .init();
    }

    tracing::info!("Starting Tond Proxy (Pingora) with TLS support...");

    let db_path =
        std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string());
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let server_addr =
        std::env::var("TOND_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:3001".to_string());
    let certs_dir_path = PathBuf::from(&data_dir).join("certs");
    let acme_staging = std::env::var("TOND_ACME_STAGING")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Build the cert store in a temporary runtime (Pingora's
    // Server::run_forever owns the main event loop).
    let cert_store = current_thread_runtime().block_on(async {
        CertStore::new(&certs_dir_path)
            .await
            .expect("Failed to initialize cert store")
    });
    let challenges = acme::new_challenge_map();

    // Pingora waits 300 s (+ a final step) for connections on SIGTERM by
    // default, which makes every restart of tond-proxy crawl. Proxied requests
    // are short; give them a few seconds to finish.
    let mut server = Server::new_with_opt_and_conf(
        None,
        pingora::server::configuration::ServerConf {
            grace_period_seconds: Some(5),
            graceful_shutdown_timeout_seconds: Some(5),
            ..pingora::server::configuration::ServerConf::new().unwrap_or_default()
        },
    );
    server.bootstrap();

    let route_db = Arc::new(
        RouteDb::open(&db_path)
            .unwrap_or_else(|err| panic!("invalid database path {db_path}: {err}")),
    );
    let (access_log, access_writer) = access_log::channel(std::path::Path::new(&data_dir));
    let proxy = TondProxy::new(route_db, server_addr).with_access_log(Arc::new(access_log));

    // ── Background tasks: one OS thread, one current-thread runtime ──

    let proxy_bg = proxy.clone();
    let cert_store_bg = cert_store.clone();
    let challenges_bg = challenges.clone();
    let data_dir_bg = PathBuf::from(&data_dir);
    let started = Instant::now();

    std::thread::Builder::new()
        .name("tond-proxy-bg".to_string())
        .spawn(move || {
            current_thread_runtime().block_on(background(
                proxy_bg,
                access_writer,
                cert_store_bg,
                challenges_bg,
                data_dir_bg,
                acme_staging,
                started,
            ));
        })
        .expect("failed to start the background thread");

    // ── TLS Proxy on port 443 ──
    let tls_settings = || {
        let tls_accept = TondTlsAccept {
            cert_store: cert_store.clone(),
        };
        TlsSettings::with_callbacks(Box::new(tls_accept)).expect("Failed to create TLS settings")
    };

    let mut proxy_service = pingora::proxy::http_proxy_service(&server.configuration, proxy);

    // TLS listeners on port 443 with SNI-based cert selection: IPv4, and
    // IPv6 (IPv6-only, so it never collides with the IPv4 socket) when the
    // host has IPv6 at all.
    proxy_service.add_tls_with_settings("0.0.0.0:443", None, tls_settings());
    let ipv6 = ipv6_available();
    if ipv6 {
        let mut v6only = pingora::listeners::TcpSocketOptions::default();
        v6only.ipv6_only = Some(true);
        proxy_service.add_tls_with_settings("[::]:443", Some(v6only), tls_settings());
    } else {
        tracing::info!("IPv6 is not available on this host: listening on IPv4 only");
    }

    server.add_service(proxy_service);

    tracing::info!(
        "Tond Proxy listening on :443 (TLS) and :80 (HTTP redirect + ACME), {}",
        if ipv6 { "IPv4 and IPv6" } else { "IPv4 only" }
    );
    server.run_forever();
}

/// A single-threaded Tokio runtime: the proxy's background work (routing,
/// certificates, port 80, control socket) is light and mostly idle.
fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build the background Tokio runtime")
}

/// Everything besides the TLS proxy itself.
async fn background(
    proxy: TondProxy,
    access_writer: access_log::Writer,
    cert_store: CertStore,
    challenges: ChallengeMap,
    data_dir: PathBuf,
    staging: bool,
    started: Instant,
) {
    if let Err(e) = proxy.refresh_routes().await {
        tracing::warn!("Initial route load failed (retrying on the next nudge or check): {e:#}");
    }

    // Opt-in access logs: appended off the request path.
    tokio::spawn(access_writer.run());

    // Port 80: ACME challenges + redirect to HTTPS.
    let http_ctx = HttpContext {
        challenges: challenges.clone(),
        routes: proxy.routes.clone(),
        dashboard_domain: proxy.dashboard_domain.clone(),
        preview_domain: proxy.preview_domain.clone(),
    };
    tokio::spawn(async move {
        if let Err(e) = run_http_server(http_ctx).await {
            tracing::error!("HTTP server (port 80) failed to start: {}", e);
        }
    });

    // Certificate loop: results go to tond-server through the outbox;
    // DNS-01 records are published by tond-server through the bridge.
    let outbox = Arc::new(Outbox::default());
    let dns = Arc::new(DnsBridge::default());
    tokio::spawn(
        IssuanceLoop {
            routes: proxy.routes.clone(),
            dashboard_domain: proxy.dashboard_domain.clone(),
            config: proxy.cert_config.clone(),
            dns: dns.clone(),
            cert_store: cert_store.clone(),
            challenges,
            data_dir: data_dir.clone(),
            db: proxy.db.pool.clone(),
            outbox: outbox.clone(),
            wake: proxy.domains_changed.clone(),
            staging,
        }
        .run(),
    );

    // Control socket for tond-server.
    let socket = control::socket_path(&data_dir);
    match control::bind(&socket) {
        Ok(listener) => {
            use std::os::unix::fs::MetadataExt;
            // We created the socket, so its owner is our effective uid.
            let own_uid = std::fs::metadata(&socket).map(|m| m.uid()).unwrap_or(0);
            tracing::info!("Control socket listening on {}", socket.display());
            let ctl = Arc::new(control::Control {
                proxy: proxy.clone(),
                outbox,
                cert_wake: proxy.domains_changed.clone(),
                cert_store,
                dns,
                started,
            });
            tokio::spawn(control::serve(listener, own_uid, ctl));
        }
        Err(e) => tracing::error!(
            "Control socket unavailable ({e:#}); routes follow the database only through the {FALLBACK_POLL_SECS}s fallback check"
        ),
    }

    // Safety net for a missed nudge: cheap version check once a minute.
    let meter = meters::register("route-poll");
    let interval = Duration::from_secs(FALLBACK_POLL_SECS);
    loop {
        meter.planned(Some(interval));
        let slept = Instant::now();
        tokio::time::sleep(interval).await;
        meter.woke(Some((slept, interval)));
        let run = Instant::now();
        let before = proxy.db.loaded_version();
        let result = proxy.sync().await;
        match &result {
            Ok((version, _)) if *version != before && before >= 0 => {
                tracing::warn!(
                    "Fallback check found route version {version} (had {before}): a nudge from tond-server was missed"
                );
            }
            Err(e) => tracing::warn!("Route check failed: {e:#}"),
            _ => {}
        }
        meter.ran(run, result.err().map(|e| format!("{e:#}")));
        if let Some((n, example)) = log_limit::LIMITER.take_summary(Instant::now()) {
            tracing::warn!(
                "{n} repeated warning(s) from the proxy's libraries were not logged in the last hour, e.g. \"{example}\". Failed requests are counted in the request metrics; set RUST_LOG=debug for every one."
            );
        }
    }
}

/// Most port-80 connections served at once; extra connections are closed.
const HTTP_MAX_CONNECTIONS: usize = 512;
/// A client must send its whole request head within this time.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request head we read.
const HTTP_MAX_HEAD_BYTES: usize = 8 * 1024;

/// Whether this host can open IPv6 sockets at all. A kernel booted with
/// `ipv6.disable=1` can't (EAFNOSUPPORT), and binding `[::]` there would stop
/// the proxy from starting; with IPv6 merely unconfigured the listener binds
/// and simply gets no traffic.
fn ipv6_available() -> bool {
    std::net::TcpListener::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).is_ok()
}

/// A listening socket on `[::]:port` that accepts IPv6 only (IPV6_V6ONLY),
/// so it coexists with the `0.0.0.0:port` socket whatever the host's
/// `net.ipv6.bindv6only` default is.
fn bind_ipv6_only(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_only_v6(true)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    let addr = std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port));
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(socket.into())
}

/// HTTP server on port 80 that:
/// - Serves ACME HTTP-01 challenge responses at `/.well-known/acme-challenge/{token}`
/// - Redirects requests for hosts Tond serves to HTTPS (301); 404 otherwise
///
/// Only a bind failure returns an error: accept errors (e.g. out of file
/// descriptors) are logged and the loop keeps going, since certificate
/// issuance and renewal depend on this listener.
async fn run_http_server(ctx: HttpContext) -> Result<()> {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("0.0.0.0:80").await?;
    let permits = Arc::new(Semaphore::new(HTTP_MAX_CONNECTIONS));
    // IPv6 as well when the host has it; a failure there never costs IPv4.
    if ipv6_available() {
        match bind_ipv6_only(80) {
            Ok(v6) => {
                tokio::spawn(serve_http_listener(v6, ctx.clone(), permits.clone()));
                tracing::info!("HTTP redirect server listening on :80 (IPv4 and IPv6)");
            }
            Err(e) => {
                tracing::warn!("HTTP server: can't listen on [::]:80 ({e}); IPv4 only");
            }
        }
    } else {
        tracing::info!("HTTP redirect server listening on :80 (IPv4 only)");
    }
    serve_http_listener(listener, ctx, permits).await;
    Ok(())
}

/// Accept loop of one port-80 listener; `permits` is shared by all of them.
async fn serve_http_listener(
    listener: tokio::net::TcpListener,
    ctx: HttpContext,
    permits: Arc<Semaphore>,
) {
    let mut last_accept_warning: Option<Instant> = None;

    loop {
        let stream = match listener.accept().await {
            Ok((stream, _addr)) => stream,
            Err(e) => {
                if last_accept_warning.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                    tracing::warn!("HTTP server (port 80) accept error: {}", e);
                    last_accept_warning = Some(Instant::now());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            // At capacity: shed the connection instead of queueing it.
            drop(stream);
            continue;
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = tokio::time::timeout(HTTP_REQUEST_TIMEOUT, serve_http_connection(stream, &ctx))
                .await;
        });
    }
}

/// Read one request head (up to the blank line or HTTP_MAX_HEAD_BYTES) and
/// write the response. The caller bounds the total time.
async fn serve_http_connection<S>(mut stream: S, ctx: &HttpContext)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= HTTP_MAX_HEAD_BYTES {
                    break;
                }
            }
        }
    }
    if buf.is_empty() {
        return;
    }
    let request = String::from_utf8_lossy(&buf);
    let response = ctx.respond(&request).await;
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use sqlx::SqlitePool;
    use tempfile::TempDir;

    use super::*;

    fn temp_dir() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"))
    }

    fn temp_db_path(dir: &Path) -> PathBuf {
        dir.join("proxy-routes.db")
    }

    fn test_route_db(db_path: &Path) -> Arc<RouteDb> {
        Arc::new(
            RouteDb::open(&db_path.to_string_lossy())
                .unwrap_or_else(|err| panic!("route db: {err}")),
        )
    }

    async fn create_test_db(db_path: &Path) -> SqlitePool {
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let pool = SqlitePool::connect(&db_url).await.unwrap_or_else(|err| {
            panic!(
                "failed to open SQLite database at {}: {err}",
                db_path.display()
            )
        });

        sqlx::query(
            "CREATE TABLE route_state (id INTEGER PRIMARY KEY, version INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create route_state table: {err}"));
        sqlx::query("INSERT INTO route_state (id, version) VALUES (1, 0)")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to seed route_state: {err}"));

        sqlx::query(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create settings table: {err}"));

        sqlx::query(
            "CREATE TABLE apps (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL UNIQUE,
                domain TEXT,
                source_type TEXT NOT NULL DEFAULT 'git',
                container_port INTEGER NOT NULL DEFAULT 3000,
                status TEXT NOT NULL DEFAULT 'pending',
                host_port INTEGER,
                readiness TEXT NOT NULL DEFAULT 'http',
                access_log INTEGER NOT NULL DEFAULT 0,
                access_log_query INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create apps table: {err}"));

        sqlx::query(
            "CREATE TABLE app_domains (
                domain TEXT PRIMARY KEY NOT NULL,
                app_id TEXT NOT NULL,
                service TEXT,
                port INTEGER,
                kind TEXT NOT NULL DEFAULT 'alias',
                redirect_to TEXT,
                redirect_code INTEGER NOT NULL DEFAULT 308
            )",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create app_domains table: {err}"));
        sqlx::query(
            "CREATE TABLE dns_providers (id TEXT PRIMARY KEY, zones TEXT NOT NULL DEFAULT '[]')",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create dns_providers table: {err}"));

        sqlx::query(
            "CREATE TABLE preview_deployments (
                id TEXT PRIMARY KEY NOT NULL,
                subdomain TEXT,
                host_port INTEGER,
                status TEXT NOT NULL DEFAULT 'building',
                expires_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create preview_deployments table: {err}"));

        for stmt in [
            "CREATE TABLE compose_services (app_id TEXT NOT NULL, name TEXT NOT NULL, host_port INTEGER, PRIMARY KEY(app_id, name))",
        ] {
            sqlx::query(stmt)
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("{stmt}: {err}"));
        }

        pool
    }

    fn sample_http_request(path: &str, host: Option<&str>) -> String {
        match host {
            Some(host) => format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
            None => format!("GET {path} HTTP/1.1\r\nUser-Agent: test\r\n\r\n"),
        }
    }

    #[test]
    fn normalize_host_strips_port_and_defaults_to_empty_string() {
        assert_eq!(
            normalize_host(Some("app.example.com:443")),
            "app.example.com",
            "normalize_host should strip the port suffix from host headers"
        );
        assert_eq!(
            normalize_host(Some("app.example.com")),
            "app.example.com",
            "normalize_host should preserve bare hostnames"
        );
        assert_eq!(
            normalize_host(None),
            "",
            "normalize_host should return an empty string when the host header is missing"
        );
    }

    #[test]
    fn resolve_route_prefers_dashboard_domain_then_known_routes_then_missing() {
        let routes = HashMap::from([
            (
                "dashboard.example.com".to_string(),
                "127.0.0.1:4100".to_string(),
            ),
            ("app.example.com".to_string(), "127.0.0.1:4200".to_string()),
        ]);

        assert_eq!(
            resolve_route(
                "dashboard.example.com",
                Some("dashboard.example.com"),
                None,
                &routes
            ),
            RouteTarget::Dashboard,
            "resolve_route should prioritize the configured dashboard domain over app routes"
        );
        assert_eq!(
            resolve_route(
                "app.example.com",
                Some("dashboard.example.com"),
                None,
                &routes
            ),
            RouteTarget::Upstream("127.0.0.1:4200".to_string()),
            "resolve_route should return the configured upstream for known app hosts"
        );
        assert_eq!(
            resolve_route(
                "missing.example.com",
                Some("dashboard.example.com"),
                None,
                &routes
            ),
            RouteTarget::Missing,
            "resolve_route should report missing for unknown hosts"
        );
    }

    #[test]
    fn preview_domain_serves_only_previews_and_never_the_dashboard() {
        let routes = HashMap::from([
            (
                "abc-frontend.previews.test".to_string(),
                "127.0.0.1:5100".to_string(),
            ),
            ("app.example.com".to_string(), "127.0.0.1:4200".to_string()),
        ]);
        let resolve = |host: &str| {
            resolve_route(
                host,
                Some("dash.example.com"),
                Some("previews.test"),
                &routes,
            )
        };

        assert_eq!(
            resolve("abc-frontend.previews.test"),
            RouteTarget::Upstream("127.0.0.1:5100".to_string()),
            "a known preview host is routed to its container"
        );
        assert_eq!(
            resolve("unknown.previews.test"),
            RouteTarget::NotFound,
            "unknown hosts under the preview domain get a 404"
        );
        assert_eq!(
            resolve("previews.test"),
            RouteTarget::NotFound,
            "the preview domain apex is never the dashboard"
        );
        assert_eq!(
            resolve("dash.example.com"),
            RouteTarget::Dashboard,
            "the dashboard domain still reaches the dashboard"
        );
        assert_eq!(
            resolve_route(
                "dash.previews.test",
                Some("dash.previews.test"),
                Some("previews.test"),
                &routes
            ),
            RouteTarget::NotFound,
            "even a (misconfigured) dashboard domain under the preview domain is not served there"
        );
        assert_eq!(
            resolve("notpreviews.test"),
            RouteTarget::Missing,
            "suffix matching respects label boundaries"
        );
    }

    #[test]
    fn parse_upstream_addr_parses_valid_pairs_and_applies_port_fallback_for_invalid_ports() {
        assert_eq!(
            parse_upstream_addr("127.0.0.1:5000", 8080),
            Some(("127.0.0.1", 5000)),
            "parse_upstream_addr should parse well-formed host:port pairs"
        );
        assert_eq!(
            parse_upstream_addr("127.0.0.1:not-a-port", 8080),
            Some(("127.0.0.1", 8080)),
            "parse_upstream_addr should use the provided fallback port when the port is invalid"
        );
        assert_eq!(
            parse_upstream_addr("127.0.0.1", 8080),
            None,
            "parse_upstream_addr should reject malformed upstream values without a host-port separator"
        );
    }

    #[test]
    fn dashboard_forwarding_headers_replace_client_supplied_values() {
        let mut req = pingora::http::RequestHeader::build("POST", b"/api/v1/auth/login", None)
            .unwrap_or_else(|err| panic!("failed to build request header: {err}"));
        req.insert_header("X-Forwarded-For", "1.2.3.4")
            .unwrap_or_else(|err| panic!("{err}"));
        req.append_header("X-Forwarded-For", "5.6.7.8")
            .unwrap_or_else(|err| panic!("{err}"));
        req.insert_header("X-Real-IP", "9.9.9.9")
            .unwrap_or_else(|err| panic!("{err}"));
        req.insert_header("X-Forwarded-Proto", "http")
            .unwrap_or_else(|err| panic!("{err}"));

        set_dashboard_forwarding_headers(&mut req, "203.0.113.7".parse().ok())
            .unwrap_or_else(|err| panic!("{err}"));

        let xff: Vec<_> = req.headers.get_all("x-forwarded-for").iter().collect();
        assert_eq!(
            xff,
            vec!["203.0.113.7"],
            "spoofed X-Forwarded-For values must be dropped"
        );
        assert!(req.headers.get("x-real-ip").is_none());
        assert_eq!(
            req.headers.get("x-forwarded-proto").map(|v| v.as_bytes()),
            Some(&b"https"[..])
        );
    }

    #[test]
    fn parse_http_request_line_and_host_extracts_path_and_defaults_host_when_missing() {
        let request = sample_http_request("/hello?x=1", Some("app.example.com"));
        let (path, host) = parse_http_request_line_and_host(&request);
        assert_eq!(
            path, "/hello?x=1",
            "HTTP request parsing should extract the request path from the request line"
        );
        assert_eq!(
            host, "app.example.com",
            "HTTP request parsing should extract the Host header value"
        );

        let request = sample_http_request("/", None);
        let (path, host) = parse_http_request_line_and_host(&request);
        assert_eq!(
            path, "/",
            "HTTP request parsing should default the path to '/' when present in the request line"
        );
        assert_eq!(
            host, "localhost",
            "HTTP request parsing should default the host to localhost when the Host header is missing"
        );
    }

    fn test_route(upstream: &str) -> Arc<Route> {
        Arc::new(Route {
            upstream: upstream.to_string(),
            stats: Registry::default().route(RouteKind::App, "test"),
            access: None,
            redirect: None,
        })
    }

    fn upstream_of<'a>(routes: &'a HashMap<String, Arc<Route>>, domain: &str) -> Option<&'a str> {
        routes.get(domain).map(|r| r.upstream.as_str())
    }

    fn http_ctx(routes: &[(&str, &str)], dashboard: Option<&str>) -> HttpContext {
        http_ctx_with_preview(routes, dashboard, None)
    }

    fn http_ctx_with_preview(
        routes: &[(&str, &str)],
        dashboard: Option<&str>,
        preview: Option<&str>,
    ) -> HttpContext {
        HttpContext {
            challenges: acme::new_challenge_map(),
            routes: Arc::new(RwLock::new(
                routes
                    .iter()
                    .map(|(d, u)| (d.to_string(), test_route(u)))
                    .collect(),
            )),
            dashboard_domain: Arc::new(RwLock::new(dashboard.map(str::to_string))),
            preview_domain: Arc::new(RwLock::new(preview.map(str::to_string))),
        }
    }

    #[tokio::test]
    async fn port_80_redirects_previews_but_not_unmatched_preview_domain_hosts() {
        let ctx = http_ctx_with_preview(
            &[("abc-web.previews.test", "127.0.0.1:5100")],
            Some("dash.example.com"),
            Some("previews.test"),
        );
        let redirect = ctx
            .respond(&sample_http_request("/", Some("abc-web.previews.test")))
            .await;
        assert!(
            redirect.starts_with("HTTP/1.1 301"),
            "a routed preview host should be redirected to HTTPS, got: {redirect}"
        );
        for host in ["previews.test", "dash.previews.test", "nope.previews.test"] {
            let resp = ctx.respond(&sample_http_request("/", Some(host))).await;
            assert!(
                resp.starts_with("HTTP/1.1 404"),
                "{host} must get a 404 on port 80, got: {resp}"
            );
        }
    }

    #[tokio::test]
    async fn build_http_redirect_or_challenge_response_returns_acme_body_for_known_token() {
        let ctx = http_ctx(&[], None);
        ctx.challenges
            .write()
            .await
            .insert("token-123".to_string(), "token-123.key-auth".to_string());

        let response = ctx
            .respond(&sample_http_request(
                "/.well-known/acme-challenge/token-123",
                Some("app.example.com"),
            ))
            .await;

        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "known ACME challenge tokens should return an HTTP 200 response"
        );
        assert!(
            response.contains("Content-Type: application/octet-stream\r\n"),
            "known ACME challenge tokens should return the documented content type"
        );
        assert!(
            response.ends_with("\r\n\r\ntoken-123.key-auth"),
            "known ACME challenge tokens should return the stored key authorization body"
        );
    }

    #[tokio::test]
    async fn build_http_redirect_or_challenge_response_returns_404_for_unknown_acme_token() {
        let ctx = http_ctx(&[], None);

        let response = ctx
            .respond(&sample_http_request(
                "/.well-known/acme-challenge/missing-token",
                Some("app.example.com"),
            ))
            .await;

        assert_eq!(
            response, HTTP_NOT_FOUND,
            "unknown ACME challenge tokens should return a 404 without a response body"
        );
    }

    #[tokio::test]
    async fn build_http_redirect_or_challenge_response_redirects_other_requests_to_https() {
        let ctx = http_ctx(
            &[("app.example.com", "127.0.0.1:4100")],
            Some("dashboard.example.com"),
        );

        let response = ctx
            .respond(&sample_http_request(
                "/dashboard",
                Some("dashboard.example.com:80"),
            ))
            .await;
        assert_eq!(
            response,
            "HTTP/1.1 301 Moved Permanently\r\nLocation: https://dashboard.example.com/dashboard\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "requests for the dashboard domain should be redirected to the equivalent HTTPS URL"
        );

        let response = ctx
            .respond(&sample_http_request("/x?y=1", Some("app.example.com")))
            .await;
        assert!(
            response.contains("Location: https://app.example.com/x?y=1\r\n"),
            "requests for routed apps should be redirected to HTTPS"
        );
    }

    #[tokio::test]
    async fn port_80_refuses_to_redirect_unknown_or_malformed_hosts() {
        let ctx = http_ctx(&[("app.example.com", "127.0.0.1:4100")], None);

        for host in [Some("evil.example.net"), None] {
            assert_eq!(
                ctx.respond(&sample_http_request("/", host)).await,
                HTTP_NOT_FOUND,
                "unknown hosts must not be redirected (open redirect)"
            );
        }
        assert_eq!(
            ctx.respond(&sample_http_request("/", Some("bad host<script>")))
                .await,
            HTTP_BAD_REQUEST
        );
        // Protocol-relative paths must not turn into another host.
        assert!(
            ctx.respond(&sample_http_request(
                "//evil.example.net/x",
                Some("app.example.com")
            ))
            .await
            .contains("Location: https://app.example.com/\r\n")
        );
    }

    #[tokio::test]
    async fn port_80_connection_answers_a_complete_request_head() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let ctx = http_ctx(&[("app.example.com", "127.0.0.1:4100")], None);
        let (mut client, server) = tokio::io::duplex(4096);
        let handle = tokio::spawn(async move { serve_http_connection(server, &ctx).await });

        // Send the head in two pieces, as a slow client would.
        let _ = client.write_all(b"GET /a HTTP/1.1\r\nHost: app.exa").await;
        let _ = client.write_all(b"mple.com\r\n\r\n").await;
        let mut response = String::new();
        let _ = client.read_to_string(&mut response).await;
        let _ = handle.await;
        assert!(response.starts_with("HTTP/1.1 301"), "got {response:?}");
    }

    #[tokio::test]
    async fn port_80_slow_client_is_cut_off_by_the_request_timeout() {
        let ctx = http_ctx(&[], None);
        let (_client, server) = tokio::io::duplex(64);
        // The client never sends anything; the timeout must end the task.
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            serve_http_connection(server, &ctx),
        )
        .await;
        assert!(
            result.is_err(),
            "a silent client keeps the handler waiting until the timeout"
        );
    }

    #[test]
    fn html_escape_neutralises_markup() {
        assert_eq!(
            html_escape(r#"<img src=x onerror="a">&'"#),
            "&lt;img src=x onerror=&quot;a&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn upstream_timeouts_are_504_and_other_failures_keep_pingoras_codes() {
        use pingora::{Error, ErrorType};
        assert_eq!(
            failure_status(&Error::new_up(ErrorType::ConnectTimedout)),
            504
        );
        assert_eq!(failure_status(&Error::new_up(ErrorType::ReadTimedout)), 504);
        assert_eq!(
            failure_status(&Error::new_up(ErrorType::ConnectRefused)),
            502
        );
        assert_eq!(
            failure_status(&Error::new_down(ErrorType::ReadTimedout)),
            400
        );
        assert_eq!(
            failure_status(&Error::new_down(ErrorType::ConnectionClosed)),
            0
        );
        assert_eq!(
            failure_status(&Error::new_in(ErrorType::InternalError)),
            500
        );
        assert_eq!(
            failure_status(&Error::new_in(ErrorType::HTTPStatus(413))),
            413
        );
    }

    #[test]
    fn route_changes_reports_only_differences() {
        let old = HashMap::from([
            ("a.com".to_string(), "127.0.0.1:1".to_string()),
            ("b.com".to_string(), "127.0.0.1:2".to_string()),
            ("c.com".to_string(), "127.0.0.1:3".to_string()),
        ]);
        assert!(
            route_changes(&old, &old.clone()).is_empty(),
            "no change, no log lines"
        );

        let mut new = old.clone();
        new.remove("b.com");
        new.insert("c.com".to_string(), "127.0.0.1:30".to_string());
        new.insert("d.com".to_string(), "127.0.0.1:4".to_string());
        assert_eq!(
            route_changes(&old, &new),
            vec![
                "removed b.com".to_string(),
                "changed c.com: 127.0.0.1:3 -> 127.0.0.1:30".to_string(),
                "added d.com -> 127.0.0.1:4".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn dashboard_target_uses_server_addr_and_invalid_ports_fall_back_to_documented_defaults()
    {
        let proxy = TondProxy::new(
            test_route_db(Path::new("/tmp/unused.db")),
            "127.0.0.1:not-a-port".to_string(),
        );
        *proxy.dashboard_domain.write().await = Some("dashboard.example.com".to_string());
        proxy.routes.write().await.insert(
            "app.example.com".to_string(),
            test_route("127.0.0.1:not-a-port"),
        );

        let dashboard_target = {
            let dd = proxy.dashboard_domain.read().await;
            let routes = proxy.routes.read().await;
            resolve_route("dashboard.example.com", dd.as_deref(), None, &routes)
        };
        let app_target = {
            let dd = proxy.dashboard_domain.read().await;
            let routes = proxy.routes.read().await;
            resolve_route("app.example.com", dd.as_deref(), None, &routes)
        };

        let dashboard_peer = match dashboard_target {
            RouteTarget::Dashboard => {
                let (addr, port) = parse_upstream_addr(&proxy.server_addr, 3001)
                    .unwrap_or_else(|| panic!("expected server_addr to produce a peer address"));
                HttpPeer::new((addr, port), false, String::new())
            }
            other => panic!("expected dashboard route target, got {other:?}"),
        };
        let app_peer = match app_target {
            RouteTarget::Upstream(upstream) => {
                let (addr, port) = parse_upstream_addr(&upstream.upstream, 8080)
                    .unwrap_or_else(|| panic!("expected app upstream to produce a peer address"));
                HttpPeer::new((addr, port), false, String::new())
            }
            other => panic!("expected app route target, got {other:?}"),
        };

        assert_eq!(
            dashboard_peer._address.to_string(),
            "127.0.0.1:3001",
            "dashboard upstream resolution should fall back to port 3001 when TOND_SERVER_ADDR has an invalid port"
        );
        assert_eq!(
            app_peer._address.to_string(),
            "127.0.0.1:8080",
            "app upstream resolution should fall back to port 8080 when a stored route has an invalid port"
        );
    }

    async fn insert_setting(pool: &SqlitePool, value: &str) {
        insert_setting_key(pool, "dashboard_domain", value).await;
    }

    async fn insert_setting_key(pool: &SqlitePool, key: &str, value: &str) {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at)
             VALUES (?, ?, datetime('now'))",
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("failed to insert {key} setting: {err}"));
    }

    async fn insert_app(
        pool: &SqlitePool,
        id: &str,
        name: &str,
        domain: Option<&str>,
        status: &str,
        container_port: i64,
        host_port: Option<i64>,
    ) {
        sqlx::query(
            "INSERT INTO apps (id, name, domain, container_port, status, host_port)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(domain)
        .bind(container_port)
        .bind(status)
        .bind(host_port)
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("failed to insert app {name}: {err}"));
        // tond-server's trigger seeds the primary domain the same way.
        if let Some(domain) = domain {
            insert_domain(pool, id, domain, "primary", None).await;
        }
    }

    async fn insert_domain(
        pool: &SqlitePool,
        app_id: &str,
        domain: &str,
        kind: &str,
        redirect_to: Option<(&str, i64)>,
    ) {
        sqlx::query(
            "INSERT INTO app_domains (domain, app_id, kind, redirect_to, redirect_code)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(domain)
        .bind(app_id)
        .bind(kind)
        .bind(redirect_to.map(|(to, _)| to))
        .bind(redirect_to.map_or(308, |(_, code)| code))
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("failed to insert domain {domain}: {err}"));
    }

    async fn insert_preview(
        pool: &SqlitePool,
        id: &str,
        subdomain: Option<&str>,
        host_port: Option<i64>,
        status: &str,
        expires_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO preview_deployments (id, subdomain, host_port, status, expires_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(subdomain)
        .bind(host_port)
        .bind(status)
        .bind(expires_at)
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("failed to insert preview {id}: {err}"));
    }

    #[tokio::test]
    async fn ipv6_listener_coexists_with_the_ipv4_one_on_the_same_port() {
        if !ipv6_available() {
            return; // an IPv4-only host: the proxy skips IPv6 (tested by the smoke run)
        }
        let v4 = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
            .unwrap_or_else(|e| panic!("v4: {e}"));
        let port = v4.local_addr().map(|a| a.port()).unwrap_or_default();
        let v6 =
            bind_ipv6_only(port).unwrap_or_else(|e| panic!("[::]:{port} next to 0.0.0.0: {e}"));
        let accept = tokio::spawn(async move { v6.accept().await.map(|(_, peer)| peer) });
        let _client = tokio::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port))
            .await
            .unwrap_or_else(|e| panic!("connect [::1]:{port}: {e}"));
        let peer = accept
            .await
            .unwrap_or_else(|e| panic!("join: {e}"))
            .unwrap_or_else(|e| panic!("accept: {e}"));
        assert!(peer.is_ipv6(), "{peer}");
    }

    #[tokio::test]
    async fn refresh_routes_routes_compose_service_domains_to_their_host_ports() {
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let pool = create_test_db(&db_path).await;
        insert_setting_key(&pool, "preview_domain", "previews.test").await;
        insert_app(&pool, "shop", "shop", None, "running", 80, None).await;
        insert_app(&pool, "down", "down", None, "stopped", 80, None).await;
        insert_app(&pool, "blog", "blog", None, "building", 80, None).await;
        for stmt in [
            "INSERT INTO compose_services (app_id, name, host_port) VALUES
               ('shop', 'web', 8300), ('shop', 'admin', 8301), ('shop', 'cache', NULL),
               ('down', 'web', 8302), ('blog', 'web', 8303)",
            "INSERT INTO app_domains (domain, app_id, service, port) VALUES
               ('shop.example.com', 'shop', 'web', 80),
               ('www.shop.example.com', 'shop', 'web', 80),
               ('admin.shop.example.com', 'shop', 'admin', 8080),
               ('cache.shop.example.com', 'shop', 'cache', 6379),
               ('x.previews.test', 'shop', 'web', 80),
               ('down.example.com', 'down', 'web', 80),
               ('blog.example.com', 'blog', 'web', 80),
               ('single.example.com', 'shop', NULL, NULL)",
            "UPDATE apps SET source_type = 'compose' WHERE id IN ('shop', 'down', 'blog')",
            "INSERT INTO app_domains (domain, app_id, service, port, kind, redirect_to) VALUES
               ('old-shop.example.com', 'shop', 'web', 80, 'redirect', 'shop.example.com')",
        ] {
            sqlx::query(stmt)
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("{stmt}: {err}"));
        }
        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".to_string());
        proxy
            .refresh_routes()
            .await
            .unwrap_or_else(|err| panic!("refresh: {err}"));
        let routes = upstreams(&*proxy.routes.read().await);
        let expected: HashMap<String, String> = [
            ("shop.example.com", "127.0.0.1:8300"),
            ("www.shop.example.com", "127.0.0.1:8300"),
            ("admin.shop.example.com", "127.0.0.1:8301"),
            // A redeploy keeps serving: unchanged services keep their port.
            ("blog.example.com", "127.0.0.1:8303"),
            ("old-shop.example.com", "308 redirect to shop.example.com"),
        ]
        .into_iter()
        .map(|(d, u)| (d.to_string(), u.to_string()))
        .collect();
        assert_eq!(
            routes, expected,
            "unpublished services, stopped apps and preview-domain hosts are not routed"
        );
    }

    #[tokio::test]
    async fn refresh_routes_loads_running_app_and_preview_routes_and_updates_dashboard_domain() {
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let pool = create_test_db(&db_path).await;

        insert_setting(&pool, "dashboard.example.com").await;
        insert_setting_key(&pool, "preview_domain", "previews.test").await;
        insert_app(
            &pool,
            "app-running",
            "frontend",
            Some("frontend.example.com"),
            "running",
            3000,
            Some(4100),
        )
        .await;
        insert_app(
            &pool,
            "app-pending",
            "worker",
            Some("worker.example.com"),
            "pending",
            3001,
            Some(4200),
        )
        .await;
        insert_app(
            &pool,
            "app-no-domain",
            "internal",
            None,
            "running",
            3002,
            Some(4300),
        )
        .await;

        insert_app(
            &pool,
            "app-worker",
            "queue-worker",
            Some("queue-worker.example.com"),
            "running",
            3003,
            Some(4400),
        )
        .await;
        sqlx::query("UPDATE apps SET readiness = 'none' WHERE id = 'app-worker'")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to mark worker: {err}"));

        insert_preview(
            &pool,
            "preview-running",
            Some("pr-42-frontend.previews.test"),
            Some(5100),
            "running",
            "2999-01-01T00:00:00Z",
        )
        .await;
        insert_preview(
            &pool,
            "preview-expired",
            Some("expired-frontend.previews.test"),
            Some(5200),
            "running",
            "2000-01-01T00:00:00Z",
        )
        .await;
        insert_preview(
            &pool,
            "preview-failed",
            Some("failed-frontend.previews.test"),
            Some(5300),
            "failed",
            "2999-01-01T00:00:00Z",
        )
        .await;

        pool.close().await;

        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".to_string());

        proxy
            .refresh_routes()
            .await
            .unwrap_or_else(|err| panic!("refresh_routes should succeed: {err}"));

        let routes = proxy.routes.read().await.clone();
        assert_eq!(
            routes.len(),
            2,
            "refresh_routes should load exactly the running app route and active preview route"
        );
        assert_eq!(
            upstream_of(&routes, "frontend.example.com"),
            Some("127.0.0.1:4100"),
            "refresh_routes should route running apps with stored host_port values"
        );
        assert_eq!(
            upstream_of(&routes, "pr-42-frontend.previews.test"),
            Some("127.0.0.1:5100"),
            "refresh_routes should route active preview deployments with stored host_port values"
        );
        assert!(
            !routes.contains_key("worker.example.com"),
            "refresh_routes should ignore apps that are not running"
        );
        assert!(
            !routes.contains_key("queue-worker.example.com"),
            "background workers are never routed"
        );
        assert!(
            !routes.contains_key("expired-frontend.previews.test"),
            "refresh_routes should ignore expired preview deployments"
        );
        assert!(
            !routes.contains_key("failed-frontend.previews.test"),
            "refresh_routes should ignore previews that are not running"
        );

        let dashboard_domain = proxy.dashboard_domain.read().await.clone();
        assert_eq!(
            dashboard_domain.as_deref(),
            Some("dashboard.example.com"),
            "refresh_routes should update the shared dashboard domain from the settings table"
        );
    }

    #[tokio::test]
    async fn refresh_routes_replaces_stale_routes_and_clears_dashboard_domain_when_setting_is_empty()
     {
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let pool = create_test_db(&db_path).await;

        insert_setting(&pool, "dashboard.example.com").await;
        insert_setting_key(&pool, "preview_domain", "previews.test").await;
        insert_app(
            &pool,
            "app-old",
            "frontend",
            Some("frontend.example.com"),
            "running",
            3000,
            Some(4100),
        )
        .await;

        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".to_string());

        proxy
            .refresh_routes()
            .await
            .unwrap_or_else(|err| panic!("initial refresh_routes should succeed: {err}"));

        sqlx::query("UPDATE settings SET value = '', updated_at = datetime('now') WHERE key = 'dashboard_domain'")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to clear dashboard_domain setting: {err}"));
        sqlx::query("DELETE FROM apps WHERE id = ?")
            .bind("app-old")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to delete stale app route: {err}"));
        insert_app(
            &pool,
            "app-new",
            "api",
            Some("api.example.com"),
            "running",
            8080,
            Some(6200),
        )
        .await;
        insert_preview(
            &pool,
            "preview-new",
            Some("pr-99-api.previews.test"),
            Some(6300),
            "running",
            "2999-01-01T00:00:00Z",
        )
        .await;

        pool.close().await;

        proxy
            .refresh_routes()
            .await
            .unwrap_or_else(|err| panic!("second refresh_routes should succeed: {err}"));

        let routes = proxy.routes.read().await.clone();
        assert_eq!(
            routes.len(),
            2,
            "refresh_routes should replace the previous route map instead of accumulating stale entries"
        );
        assert!(
            !routes.contains_key("frontend.example.com"),
            "refresh_routes should remove app routes that no longer exist in the database"
        );
        assert_eq!(
            upstream_of(&routes, "api.example.com"),
            Some("127.0.0.1:6200"),
            "refresh_routes should load newly added running app routes"
        );
        assert_eq!(
            upstream_of(&routes, "pr-99-api.previews.test"),
            Some("127.0.0.1:6300"),
            "refresh_routes should retain only active preview routes from the latest refresh"
        );

        let dashboard_domain = proxy.dashboard_domain.read().await.clone();
        assert_eq!(
            dashboard_domain, None,
            "refresh_routes should clear the shared dashboard domain when the setting becomes empty"
        );
    }

    #[tokio::test]
    async fn refresh_routes_keeps_previews_and_apps_on_their_own_domains() {
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let pool = create_test_db(&db_path).await;

        insert_setting(&pool, "dash.example.com").await;
        insert_setting_key(&pool, "preview_domain", "previews.test").await;
        insert_app(
            &pool,
            "app-squatter",
            "squatter",
            Some("squatter.previews.test"),
            "running",
            3000,
            Some(4100),
        )
        .await;
        insert_app(
            &pool,
            "app-deploying",
            "deploying",
            Some("deploying.example.com"),
            "running",
            3000,
            None,
        )
        .await;
        insert_preview(
            &pool,
            "preview-on-dashboard",
            Some("abc-frontend.dash.example.com"),
            Some(5100),
            "running",
            "2999-01-01T00:00:00Z",
        )
        .await;
        insert_preview(
            &pool,
            "preview-ok",
            Some("def-frontend.previews.test"),
            Some(5200),
            "running",
            "2999-01-01T00:00:00Z",
        )
        .await;
        pool.close().await;

        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".to_string());
        proxy
            .refresh_routes()
            .await
            .unwrap_or_else(|err| panic!("refresh_routes should succeed: {err}"));

        let routes = proxy.routes.read().await.clone();
        assert_eq!(
            routes.keys().collect::<Vec<_>>(),
            vec!["def-frontend.previews.test"],
            "only the preview under the preview domain is routed: apps under the preview \
             domain, previews outside it and apps without a host port are dropped"
        );
        assert_eq!(
            proxy.preview_domain.read().await.as_deref(),
            Some("previews.test"),
            "refresh_routes should load the preview domain"
        );
        assert!(matches!(
            proxy.resolve("squatter.previews.test").await,
            RouteTarget::NotFound
        ));
    }

    async fn bump_version(pool: &SqlitePool) {
        sqlx::query("UPDATE route_state SET version = version + 1")
            .execute(pool)
            .await
            .unwrap_or_else(|err| panic!("bump: {err}"));
    }

    #[tokio::test]
    async fn sync_reloads_only_when_the_route_version_moves() {
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let pool = create_test_db(&db_path).await;
        insert_app(
            &pool,
            "a1",
            "web",
            Some("web.example.com"),
            "running",
            3000,
            Some(4100),
        )
        .await;
        bump_version(&pool).await;

        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".to_string());
        let (version, routes) = proxy
            .sync()
            .await
            .unwrap_or_else(|err| panic!("first sync: {err}"));
        assert_eq!((version, routes), (1, 1));
        assert_eq!(proxy.route_db().reloads(), 1);

        // A write that did not bump the version is not picked up...
        insert_app(
            &pool,
            "a2",
            "api",
            Some("api.example.com"),
            "running",
            3000,
            Some(4200),
        )
        .await;
        let (version, routes) = proxy
            .sync()
            .await
            .unwrap_or_else(|err| panic!("second sync: {err}"));
        assert_eq!((version, routes), (1, 1), "same version: no reload");
        assert_eq!(proxy.route_db().reloads(), 1);

        // ...until the version moves (tond-server's triggers do this).
        bump_version(&pool).await;
        let (version, routes) = proxy
            .sync()
            .await
            .unwrap_or_else(|err| panic!("third sync: {err}"));
        assert_eq!((version, routes), (2, 2));
        assert_eq!(proxy.route_db().reloads(), 2);
    }

    #[test]
    fn redirect_locations_keep_path_and_query() {
        assert_eq!(
            redirect_location("example.com", "/shop?item=1&x=%20"),
            "https://example.com/shop?item=1&x=%20"
        );
        assert_eq!(
            redirect_location("example.com", "/"),
            "https://example.com/"
        );
        assert_eq!(
            redirect_location("example.com", "http://evil.test/x"),
            "https://example.com/",
            "absolute-form targets never pick the host"
        );
    }

    #[tokio::test]
    async fn every_domain_of_an_app_is_routed_and_redirects_carry_their_target() {
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let pool = create_test_db(&db_path).await;
        insert_setting_key(&pool, "apps_domain", "apps.test").await;
        insert_app(
            &pool,
            "a1",
            "web",
            Some("example.com"),
            "running",
            3000,
            Some(4100),
        )
        .await;
        insert_domain(&pool, "a1", "shop.example.com", "alias", None).await;
        insert_domain(
            &pool,
            "a1",
            "www.example.com",
            "redirect",
            Some(("example.com", 301)),
        )
        .await;
        // Compose service domains are not routed on the app's port.
        sqlx::query("INSERT INTO app_domains (domain, app_id, kind, service) VALUES ('db.example.com', 'a1', 'alias', 'admin')")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("insert: {err}"));
        sqlx::query("INSERT INTO dns_providers (id, zones) VALUES ('p1', '[\"apps.test\"]')")
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("insert: {err}"));

        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".into());
        proxy
            .refresh_routes()
            .await
            .unwrap_or_else(|err| panic!("refresh: {err}"));
        let routes = proxy.routes.read().await;
        assert_eq!(upstream_of(&routes, "example.com"), Some("127.0.0.1:4100"));
        assert_eq!(
            upstream_of(&routes, "shop.example.com"),
            Some("127.0.0.1:4100")
        );
        assert!(routes["example.com"].redirect.is_none());
        assert_eq!(
            routes["www.example.com"].redirect,
            Some(Redirect {
                to: "example.com".into(),
                code: 301
            })
        );
        assert!(!routes.contains_key("db.example.com"));
        assert!(
            Arc::ptr_eq(
                &routes["example.com"].stats,
                &routes["www.example.com"].stats
            ),
            "all domains of an app count into the app's metrics"
        );
        drop(routes);
        let cfg = proxy.cert_config.read().await.clone();
        assert_eq!(cfg.apps_domain.as_deref(), Some("apps.test"));
        assert_eq!(cfg.dns_zones, vec!["apps.test".to_string()]);

        // Port 80 sends a redirect domain straight to its target.
        let ctx = HttpContext {
            challenges: acme::new_challenge_map(),
            routes: proxy.routes.clone(),
            dashboard_domain: proxy.dashboard_domain.clone(),
            preview_domain: proxy.preview_domain.clone(),
        };
        let response = ctx
            .respond(&sample_http_request("/a?b=c", Some("www.example.com")))
            .await;
        assert!(
            response.starts_with(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: https://example.com/a?b=c\r\n"
            ),
            "{response}"
        );
        let response = ctx
            .respond(&sample_http_request("/a", Some("shop.example.com")))
            .await;
        assert!(
            response.contains("Location: https://shop.example.com/a\r\n"),
            "{response}"
        );
    }

    #[tokio::test]
    async fn the_route_connection_is_read_only_and_follows_a_wal_writer() {
        // Server-like writer: WAL mode, files 0440 for us (read-only access,
        // like the tond-proxy group on 0640 files).
        let dir = temp_dir();
        let db_path = temp_db_path(dir.path());
        let writer_opts = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_path.display()))
            .unwrap_or_else(|err| panic!("opts: {err}"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let writer = SqlitePool::connect_with(writer_opts)
            .await
            .unwrap_or_else(|err| panic!("writer: {err}"));
        for stmt in [
            "CREATE TABLE route_state (id INTEGER PRIMARY KEY, version INTEGER NOT NULL)",
            "INSERT INTO route_state VALUES (1, 1)",
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            "CREATE TABLE apps (id TEXT PRIMARY KEY, name TEXT, domain TEXT, status TEXT, source_type TEXT NOT NULL DEFAULT 'git', host_port INTEGER, readiness TEXT NOT NULL DEFAULT 'http', access_log INTEGER NOT NULL DEFAULT 0, access_log_query INTEGER NOT NULL DEFAULT 0)",
            "CREATE TABLE preview_deployments (id TEXT PRIMARY KEY, subdomain TEXT, host_port INTEGER, status TEXT, expires_at TEXT)",
            "CREATE TABLE compose_services (app_id TEXT NOT NULL, name TEXT NOT NULL, host_port INTEGER, PRIMARY KEY(app_id, name))",
            "INSERT INTO apps (id, name, domain, status, host_port) VALUES ('a', 'web', 'web.example.com', 'running', 4100)",
            "CREATE TABLE app_domains (domain TEXT PRIMARY KEY, app_id TEXT, service TEXT, port INTEGER, kind TEXT, redirect_to TEXT, redirect_code INTEGER NOT NULL DEFAULT 308)",
            "INSERT INTO app_domains (domain, app_id, kind) VALUES ('web.example.com', 'a', 'primary')",
            "CREATE TABLE dns_providers (id TEXT PRIMARY KEY, zones TEXT NOT NULL)",
        ] {
            sqlx::query(stmt)
                .execute(&writer)
                .await
                .unwrap_or_else(|err| panic!("{stmt}: {err}"));
        }
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in ["", "-wal", "-shm"] {
                let f = PathBuf::from(format!("{}{suffix}", db_path.display()));
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o440))
                    .unwrap_or_else(|err| panic!("chmod {}: {err}", f.display()));
            }
        }

        let proxy = TondProxy::new(test_route_db(&db_path), "127.0.0.1:3001".to_string());
        assert_eq!(
            proxy
                .sync()
                .await
                .unwrap_or_else(|err| panic!("sync: {err}")),
            (1, 1)
        );
        let write = sqlx::query("UPDATE route_state SET version = 99")
            .execute(&proxy.db.pool)
            .await;
        assert!(
            write.is_err(),
            "the proxy's connection must not be able to write"
        );

        sqlx::query("INSERT INTO apps (id, name, domain, status, host_port) VALUES ('b', 'api', 'api.example.com', 'running', 4200)")
            .execute(&writer)
            .await
            .unwrap_or_else(|err| panic!("insert: {err}"));
        sqlx::query(
            "INSERT INTO app_domains (domain, app_id, kind) VALUES ('api.example.com', 'b', 'primary')",
        )
        .execute(&writer)
        .await
        .unwrap_or_else(|err| panic!("insert domain: {err}"));
        sqlx::query("UPDATE route_state SET version = 2")
            .execute(&writer)
            .await
            .unwrap_or_else(|err| panic!("bump: {err}"));
        assert_eq!(
            proxy
                .sync()
                .await
                .unwrap_or_else(|err| panic!("sync: {err}")),
            (2, 2),
            "a persistent read-only connection sees the writer's new commits"
        );
    }
}
