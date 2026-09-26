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
//     in CertStore, and injects the right cert+key into the SSL context.
//     Falls back to a self-signed cert for unknown domains.
//
//   Port 80 — run_http_server() (raw Tokio TCP)
//     A hand-rolled HTTP/1.1 server. Three behaviours:
//       /.well-known/acme-challenge/{token} — serves ACME HTTP-01 responses
//       a host Tond serves                  — 301 redirect to https://
//       anything else                       — 404 (no open redirect)
//     Hardened against slow clients: the request head must arrive within
//     HTTP_REQUEST_TIMEOUT, at most HTTP_MAX_CONNECTIONS are served at once,
//     and accept() errors never stop the listener (renewals depend on it).
//
//   Background thread (native OS thread + Tokio runtime):
//     - Route refresh loop: every 2 s opens a read-only SQLite connection,
//       queries running apps and previews with a stored host_port, and
//       atomically replaces the RouteMap. Also reads dashboard_domain and
//       preview_domain from the settings table so changes made via the
//       dashboard or CLI are picked up without a proxy restart. The proxy
//       never talks to Docker (it runs unprivileged, see tond-proxy.service).
//     - Certificate loop (issuance.rs): every 30 s, issues or renews certs
//       for routed domains and the dashboard domain, with per-domain
//       exponential backoff and a DNS pre-check; results (including the last
//       error and next retry time) are written to the certificates table.
//
// Logging: nothing is logged per request at info (routing decisions are at
// debug), and the route refresh only logs when a route is added, removed or
// changed. Set RUST_LOG=tond_proxy=debug to trace routing.
//
// Environment variables consumed here:
//   TOND_DB_PATH          — SQLite database path
//   TOND_DATA_DIR         — base dir for certs and ACME account creds
//   TOND_SERVER_ADDR      — tond-server address (default: 127.0.0.1:3001)
//   TOND_DASHBOARD_DOMAIN — (optional) seed value written to the DB on first
//                           startup when the DB value is empty. After that
//                           the DB value takes precedence and this env var
//                           is ignored. Use `tond settings set dashboard-domain`
//                           or the dashboard settings page to change it.
//   TOND_ACME_STAGING     — "1" or "true" to use Let's Encrypt staging

mod acme;
mod certs;
mod issuance;

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
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};

use crate::acme::ChallengeMap;
use crate::certs::CertStore;
use crate::issuance::IssuanceLoop;

/// Route map: domain -> upstream (host:port)
type RouteMap = Arc<RwLock<HashMap<String, String>>>;

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

// ── Proxy ──

struct TondProxy {
    routes: RouteMap,
    db_path: String,
    /// Live dashboard domain — reads from the DB on every 2 s poll.
    /// None means no dashboard domain is configured.
    dashboard_domain: DashboardDomain,
    /// Live preview domain — previews are served only under it, and nothing
    /// else is. None means previews are disabled.
    preview_domain: PreviewDomain,
    /// Address of the tond-server instance (default: 127.0.0.1:3001).
    server_addr: String,
}

#[derive(Debug, PartialEq, Eq)]
enum RouteTarget {
    Dashboard,
    Upstream(String),
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
    host_header
        .map(|h| h.split(':').next().unwrap_or(h).to_string())
        .unwrap_or_default()
}

fn resolve_route(
    host: &str,
    dashboard_domain: Option<&str>,
    preview_domain: Option<&str>,
    routes: &HashMap<String, String>,
) -> RouteTarget {
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
        let known = {
            let dd = self.dashboard_domain.read().await;
            let pd = self.preview_domain.read().await;
            let routes = self.routes.read().await;
            matches!(
                resolve_route(&host, dd.as_deref(), pd.as_deref(), &routes),
                RouteTarget::Dashboard | RouteTarget::Upstream(_)
            )
        };
        if !known {
            return HTTP_NOT_FOUND.to_string();
        }

        // Only an origin-form path goes into the Location header.
        let path = if path.starts_with('/') && !path.starts_with("//") {
            path
        } else {
            "/"
        };
        format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: https://{host}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }
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
    fn new(db_path: &str, initial_dashboard_domain: Option<String>, server_addr: String) -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            db_path: db_path.to_string(),
            dashboard_domain: Arc::new(RwLock::new(initial_dashboard_domain)),
            preview_domain: Arc::new(RwLock::new(None)),
            server_addr,
        }
    }

    async fn resolve(&self, host: &str) -> RouteTarget {
        let dd = self.dashboard_domain.read().await;
        let pd = self.preview_domain.read().await;
        let routes = self.routes.read().await;
        resolve_route(host, dd.as_deref(), pd.as_deref(), &routes)
    }

    /// Refresh routes and settings from SQLite.
    ///
    /// Reads the host_port stored in the DB (written by tond-server on every
    /// deploy), which makes routing deterministic across proxy restarts and
    /// keeps the proxy away from the Docker socket. Rows without a host_port
    /// (a deploy in progress) are skipped until the next refresh.
    ///
    /// Also reads `dashboard_domain` and `preview_domain` from the settings
    /// table so that changes made via the dashboard UI or `tond settings`
    /// CLI commands are picked up without restarting the proxy.
    async fn refresh_routes(&self) -> Result<()> {
        let pool = SqlitePool::connect(&format!("sqlite:{}?mode=ro", self.db_path)).await?;

        let settings: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM settings WHERE key IN ('dashboard_domain', 'preview_domain')",
        )
        .fetch_all(&pool)
        .await?;
        let setting = |key: &str| {
            settings
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let new_preview_domain = setting("preview_domain");
        {
            let new_domain = setting("dashboard_domain");
            let mut dd = self.dashboard_domain.write().await;
            if *dd != new_domain {
                tracing::info!("Dashboard domain updated: {:?} -> {:?}", *dd, new_domain);
                *dd = new_domain;
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

        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            // Background workers (readiness 'none') serve nothing: never routed,
            // even if a domain lingers on the row.
            "SELECT name, domain, host_port FROM apps
             WHERE status = 'running' AND domain IS NOT NULL AND host_port IS NOT NULL
               AND readiness != 'none'",
        )
        .fetch_all(&pool)
        .await?;

        let mut new_routes = HashMap::new();

        for (name, domain, host_port) in &rows {
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
            new_routes.insert(domain.clone(), upstream);
        }

        // Also load active preview deployments (subdomain routing).
        // Previews always have a stored host_port — no Docker fallback needed.
        let preview_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
            "SELECT subdomain, host_port FROM preview_deployments
             WHERE status = 'running'
               AND subdomain IS NOT NULL
               AND host_port IS NOT NULL
               AND expires_at > datetime('now')",
        )
        .fetch_all(&pool)
        .await
        .unwrap_or_default();

        for (subdomain, host_port) in preview_rows {
            if let Some(sub) = subdomain {
                // Previews are served only under the current preview domain.
                if !under_preview_domain(&sub) {
                    continue;
                }
                new_routes.insert(sub, format!("127.0.0.1:{}", host_port));
            }
        }

        let mut routes = self.routes.write().await;
        let changes = route_changes(&routes, &new_routes);
        if !changes.is_empty() {
            for change in &changes {
                tracing::info!("Route {}", change);
            }
            tracing::info!("Routing table now has {} route(s)", new_routes.len());
        }
        *routes = new_routes;

        pool.close().await;
        Ok(())
    }
}

#[async_trait]
impl ProxyHttp for TondProxy {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {}

    /// Check if the requested host has a known route. If not, write a
    /// friendly HTML error page directly and short-circuit (return Ok(true)).
    async fn request_filter(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> pingora::Result<bool> {
        let host = normalize_host(
            session
                .req_header()
                .headers
                .get("host")
                .and_then(|v| v.to_str().ok()),
        );

        let route_target = self.resolve(&host).await;

        match route_target {
            RouteTarget::Dashboard | RouteTarget::Upstream(_) => return Ok(false),
            RouteTarget::NotFound => {
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
            RouteTarget::Missing => {}
        }

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
        _ctx: &mut Self::CTX,
    ) -> pingora::Result<Box<HttpPeer>> {
        let host = normalize_host(
            session
                .req_header()
                .headers
                .get("host")
                .and_then(|v| v.to_str().ok()),
        );

        let route_target = self.resolve(&host).await;

        tracing::debug!("Routing request for host {:?} -> {:?}", host, route_target);

        match route_target {
            RouteTarget::Dashboard => {
                if let Some((addr, port)) = parse_upstream_addr(&self.server_addr, 3001) {
                    return Ok(Box::new(HttpPeer::new((addr, port), false, String::new())));
                }
            }
            RouteTarget::Upstream(upstream) => {
                if let Some((addr, port)) = parse_upstream_addr(&upstream, 8080) {
                    return Ok(Box::new(HttpPeer::new((addr, port), false, String::new())));
                }
            }
            RouteTarget::Missing | RouteTarget::NotFound => {}
        }

        // request_filter already handled the no-route case; if we somehow
        // reach here just use a dummy peer (should not happen in practice).
        tracing::warn!(
            "upstream_peer reached for unknown host '{}' — this should not happen",
            host
        );
        let peer = Box::new(HttpPeer::new(("127.0.0.1", 9999), false, String::new()));
        Ok(peer)
    }

    /// Requests for the dashboard domain carry the TCP peer as X-Forwarded-For.
    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut pingora::http::RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> pingora::Result<()> {
        let host = normalize_host(
            session
                .req_header()
                .headers
                .get("host")
                .and_then(|v| v.to_str().ok()),
        );
        let is_dashboard = self.dashboard_domain.read().await.as_deref() == Some(host.as_str());
        if is_dashboard {
            let client_ip = session
                .client_addr()
                .and_then(|addr| addr.as_inet())
                .map(|addr| addr.ip());
            set_dashboard_forwarding_headers(upstream_request, client_ip)?;
        }
        Ok(())
    }
}

fn main() {
    tond_common::install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Starting Tond Proxy (Pingora) with TLS support...");

    let db_path =
        std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string());
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let server_addr =
        std::env::var("TOND_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:3001".to_string());
    // TOND_DASHBOARD_DOMAIN is a one-time seed: if the DB has no dashboard_domain
    // and this env var is set, it is written to the DB on startup. After that the
    // DB value is the source of truth and this variable is ignored.
    let env_dashboard_domain = std::env::var("TOND_DASHBOARD_DOMAIN")
        .ok()
        .filter(|s| !s.is_empty());
    let certs_dir_path = PathBuf::from(&data_dir).join("certs");
    let acme_staging = std::env::var("TOND_ACME_STAGING")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Build the cert store, challenge map, and seed the DB in a temporary
    // Tokio runtime (Pingora's Server::run_forever owns the main event loop).
    let (cert_store, challenges, initial_dashboard_domain) = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let cert_store = CertStore::new(&certs_dir_path)
                .await
                .expect("Failed to initialize cert store");
            let challenges = acme::new_challenge_map();

            // Seed dashboard_domain from env var if the DB value is empty.
            let db_url = format!("sqlite:{}", db_path);
            let initial = match SqlitePool::connect(&db_url).await {
                Ok(pool) => {
                    let row: Option<(String,)> =
                        sqlx::query_as("SELECT value FROM settings WHERE key = 'dashboard_domain'")
                            .fetch_optional(&pool)
                            .await
                            .unwrap_or(None);
                    let db_value = row.map(|(v,)| v).filter(|v| !v.is_empty());

                    let resolved = if db_value.is_none() {
                        // DB is empty — write the env var value if present.
                        if let Some(ref env_val) = env_dashboard_domain {
                            tracing::info!(
                                "Seeding dashboard_domain from TOND_DASHBOARD_DOMAIN env var: {}",
                                env_val
                            );
                            let _ = sqlx::query(
                                "INSERT INTO settings (key, value, updated_at)
                                 VALUES ('dashboard_domain', ?, datetime('now'))
                                 ON CONFLICT(key) DO UPDATE
                                   SET value = excluded.value,
                                       updated_at = excluded.updated_at",
                            )
                            .bind(env_val)
                            .execute(&pool)
                            .await;
                            Some(env_val.clone())
                        } else {
                            None
                        }
                    } else {
                        db_value
                    };

                    pool.close().await;
                    resolved
                }
                Err(e) => {
                    // DB not yet created (first run before tond-server initialises).
                    tracing::warn!("Could not open DB to read dashboard_domain: {}", e);
                    env_dashboard_domain.clone()
                }
            };

            (cert_store, challenges, initial)
        })
    };

    if let Some(ref domain) = initial_dashboard_domain {
        tracing::info!("Dashboard domain: {} -> {}", domain, server_addr);
    }

    let mut server = Server::new(None).unwrap();
    server.bootstrap();

    let proxy = TondProxy::new(&db_path, initial_dashboard_domain, server_addr);

    // ── Background tasks ──

    let routes = proxy.routes.clone();
    let dashboard_domain_shared = proxy.dashboard_domain.clone();
    let preview_domain_shared = proxy.preview_domain.clone();
    let db_path_bg = db_path.clone();
    let cert_store_bg = cert_store.clone();
    let challenges_bg = challenges.clone();
    let data_dir_bg = PathBuf::from(&data_dir);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let proxy_for_refresh = TondProxy {
                routes: routes.clone(),
                db_path: db_path_bg,
                dashboard_domain: dashboard_domain_shared.clone(),
                preview_domain: preview_domain_shared.clone(),
                server_addr: String::new(),
            };
            let db_url = format!("sqlite:{}", proxy_for_refresh.db_path);
            let db_pool = SqlitePool::connect(&db_url).await.unwrap_or_else(|err| {
                panic!("failed to open SQLite database for the certificate loop: {err}")
            });

            // Spawn the HTTP server for port 80 (ACME challenges + redirect)
            let http_ctx = HttpContext {
                challenges: challenges_bg.clone(),
                routes: routes.clone(),
                dashboard_domain: dashboard_domain_shared.clone(),
                preview_domain: preview_domain_shared.clone(),
            };
            tokio::spawn(async move {
                if let Err(e) = run_http_server(http_ctx).await {
                    tracing::error!("HTTP server (port 80) failed to start: {}", e);
                }
            });

            // Certificate issuance/renewal loop (per-domain backoff, DNS
            // pre-check, state persisted for the dashboard).
            tokio::spawn(
                IssuanceLoop {
                    routes: routes.clone(),
                    dashboard_domain: dashboard_domain_shared.clone(),
                    cert_store: cert_store_bg,
                    challenges: challenges_bg,
                    data_dir: data_dir_bg,
                    db: db_pool,
                    staging: acme_staging,
                }
                .run(),
            );

            // Route refresh loop — also refreshes dashboard_domain and preview_domain from DB
            loop {
                if let Err(e) = proxy_for_refresh.refresh_routes().await {
                    tracing::warn!("Failed to refresh routes: {}", e);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    });

    // ── TLS Proxy on port 443 ──
    let tls_accept = TondTlsAccept {
        cert_store: cert_store.clone(),
    };
    let tls_settings =
        TlsSettings::with_callbacks(Box::new(tls_accept)).expect("Failed to create TLS settings");

    let mut proxy_service = pingora::proxy::http_proxy_service(&server.configuration, proxy);

    // Add TLS listener on port 443 with SNI-based cert selection
    proxy_service.add_tls_with_settings("0.0.0.0:443", None, tls_settings);

    server.add_service(proxy_service);

    tracing::info!("Tond Proxy listening on :443 (TLS) and :80 (HTTP redirect + ACME)");
    server.run_forever();
}

/// Most port-80 connections served at once; extra connections are closed.
const HTTP_MAX_CONNECTIONS: usize = 512;
/// A client must send its whole request head within this time.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request head we read.
const HTTP_MAX_HEAD_BYTES: usize = 8 * 1024;

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
    tracing::info!("HTTP redirect server listening on :80");

    let permits = Arc::new(Semaphore::new(HTTP_MAX_CONNECTIONS));
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

    async fn create_test_db(db_path: &Path) -> SqlitePool {
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let pool = SqlitePool::connect(&db_url).await.unwrap_or_else(|err| {
            panic!(
                "failed to open SQLite database at {}: {err}",
                db_path.display()
            )
        });

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
                container_port INTEGER NOT NULL DEFAULT 3000,
                status TEXT NOT NULL DEFAULT 'pending',
                host_port INTEGER,
                readiness TEXT NOT NULL DEFAULT 'http'
            )",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to create apps table: {err}"));

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
        let mut req = pingora::http::RequestHeader::build("POST", b"/api/auth/login", None)
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
                    .map(|(d, u)| (d.to_string(), u.to_string()))
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
            "/tmp/unused.db",
            Some("dashboard.example.com".to_string()),
            "127.0.0.1:not-a-port".to_string(),
        );
        proxy.routes.write().await.insert(
            "app.example.com".to_string(),
            "127.0.0.1:not-a-port".to_string(),
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
                let (addr, port) = parse_upstream_addr(&upstream, 8080)
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

        let proxy = TondProxy::new(
            &db_path.to_string_lossy(),
            Some("stale.example.com".to_string()),
            "127.0.0.1:3001".to_string(),
        );

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
            routes.get("frontend.example.com"),
            Some(&"127.0.0.1:4100".to_string()),
            "refresh_routes should route running apps with stored host_port values"
        );
        assert_eq!(
            routes.get("pr-42-frontend.previews.test"),
            Some(&"127.0.0.1:5100".to_string()),
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

        let proxy = TondProxy::new(
            &db_path.to_string_lossy(),
            Some("seed.example.com".to_string()),
            "127.0.0.1:3001".to_string(),
        );

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
            routes.get("api.example.com"),
            Some(&"127.0.0.1:6200".to_string()),
            "refresh_routes should load newly added running app routes"
        );
        assert_eq!(
            routes.get("pr-99-api.previews.test"),
            Some(&"127.0.0.1:6300".to_string()),
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

        let proxy = TondProxy::new(
            &db_path.to_string_lossy(),
            None,
            "127.0.0.1:3001".to_string(),
        );
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
        assert_eq!(
            proxy.resolve("squatter.previews.test").await,
            RouteTarget::NotFound
        );
    }
}
