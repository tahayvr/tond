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
//   TLS — TondTlsAccept (Pingora TlsAccept)
//     Implements SNI-based certificate selection. On each TLS handshake
//     it reads the server name from the ClientHello, looks up the cert
//     in CertStore, and injects the right cert+key into the SSL context.
//     Falls back to a self-signed cert for unknown domains.
//
//   Port 80 — run_http_server() (raw Tokio TCP)
//     A hand-rolled HTTP/1.1 server. Two behaviours:
//       /.well-known/acme-challenge/{token} — serves ACME HTTP-01 responses
//       everything else                     — 301 redirect to https://
//
//   Background thread (native OS thread + Tokio runtime):
//     - Route refresh loop: every 5 s opens a read-only SQLite connection,
//       queries running apps with a stored host_port, and atomically
//       replaces the RouteMap. Also reads dashboard_domain from the
//       settings table so changes made via the dashboard or CLI are picked
//       up without a proxy restart. Falls back to Docker inspect for apps
//       that were deployed before migration 006 (host_port IS NULL).
//     - ACME renewal loop: waits 10 s then runs every 6 h. For any domain
//       without a cert in CertStore, calls acme::provision_cert().
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
use tokio::sync::RwLock;

use crate::acme::ChallengeMap;
use crate::certs::CertStore;

/// Route map: domain -> upstream (host:port)
type RouteMap = Arc<RwLock<HashMap<String, String>>>;

/// Shared, live dashboard domain — updated on every DB poll.
type DashboardDomain = Arc<RwLock<Option<String>>>;

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
    /// Address of the tond-server instance (default: 127.0.0.1:3001).
    server_addr: String,
}

#[derive(Debug, PartialEq, Eq)]
enum RouteTarget {
    Dashboard,
    Upstream(String),
    Missing,
}

fn normalize_host(host_header: Option<&str>) -> String {
    host_header
        .map(|h| h.split(':').next().unwrap_or(h).to_string())
        .unwrap_or_default()
}

fn resolve_route(
    host: &str,
    dashboard_domain: Option<&str>,
    routes: &HashMap<String, String>,
) -> RouteTarget {
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

async fn build_http_redirect_or_challenge_response(
    request: &str,
    challenges: &ChallengeMap,
) -> String {
    let (path, host) = parse_http_request_line_and_host(request);

    if let Some(token) = path.strip_prefix("/.well-known/acme-challenge/") {
        let map = challenges.read().await;
        if let Some(key_auth) = map.get(token) {
            return format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n{}",
                key_auth.len(),
                key_auth
            );
        }

        return "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string();
    }

    let redirect_url = format!("https://{}{}", host, path);
    format!(
        "HTTP/1.1 301 Moved Permanently\r\nLocation: {}\r\nContent-Length: 0\r\n\r\n",
        redirect_url
    )
}

impl TondProxy {
    fn new(db_path: &str, initial_dashboard_domain: Option<String>, server_addr: String) -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            db_path: db_path.to_string(),
            dashboard_domain: Arc::new(RwLock::new(initial_dashboard_domain)),
            server_addr,
        }
    }

    /// Refresh routes and settings from SQLite.
    ///
    /// Reads the host_port stored in the DB (written by tond-server on every
    /// deploy). This avoids a Docker API call per container and makes routing
    /// deterministic across proxy restarts.
    ///
    /// Also reads `dashboard_domain` from the settings table so that changes
    /// made via the dashboard UI or `tond settings set` CLI command are
    /// picked up within 5 s without restarting the proxy.
    ///
    /// Falls back to Docker inspect for any app that has a NULL host_port
    /// (rows created before migration 006 or during an in-progress deploy).
    async fn refresh_routes(&self) -> Result<()> {
        let pool = SqlitePool::connect(&format!("sqlite:{}?mode=ro", self.db_path)).await?;

        // Read dashboard_domain from settings table and update the shared value.
        let setting: Option<(String,)> =
            sqlx::query_as("SELECT value FROM settings WHERE key = 'dashboard_domain'")
                .fetch_optional(&pool)
                .await?;
        {
            let new_domain = setting.map(|(v,)| v).filter(|v| !v.is_empty());
            let mut dd = self.dashboard_domain.write().await;
            if *dd != new_domain {
                tracing::info!("Dashboard domain updated: {:?} -> {:?}", *dd, new_domain);
                *dd = new_domain;
            }
        }

        // Prefer DB-stored host_port; fall back to NULL for legacy rows.
        let rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT name, domain, container_port, host_port FROM apps WHERE status = 'running' AND domain IS NOT NULL",
        )
        .fetch_all(&pool)
        .await?;

        tracing::info!("Route refresh: {} running app(s) with a domain", rows.len());

        let mut new_routes = HashMap::new();

        // Only open the Docker socket if at least one row needs a fallback lookup.
        let needs_docker = rows.iter().any(|(_, _, _, hp)| hp.is_none());
        let docker_opt = if needs_docker {
            match bollard::Docker::connect_with_local_defaults() {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!(
                        "Could not connect to Docker for fallback port lookup: {}",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        for (name, domain, container_port, host_port_db) in &rows {
            let host_port = if let Some(hp) = host_port_db {
                // Fast path: use the value stored in the DB.
                *hp as u16
            } else {
                // Legacy / in-progress: fall back to Docker inspect.
                let Some(ref docker) = docker_opt else {
                    tracing::warn!(
                        "App {} has no stored host_port and Docker is unavailable — skipping",
                        name
                    );
                    continue;
                };

                let container_name = format!("tond-{}", name);
                match docker.inspect_container(&container_name, None).await {
                    Ok(info) => {
                        let raw_ports = info
                            .network_settings
                            .as_ref()
                            .and_then(|ns| ns.ports.as_ref())
                            .map(|p| format!("{:?}", p))
                            .unwrap_or_else(|| "<none>".to_string());
                        tracing::info!(
                            "Container {} port map (fallback): {}",
                            container_name,
                            raw_ports
                        );

                        let host_port_opt = info.network_settings.and_then(|ns| {
                            let ports = ns.ports?;
                            let tcp_key = format!("{}/tcp", container_port);
                            let udp_key = format!("{}/udp", container_port);
                            let binding = ports
                                .get(&tcp_key)
                                .or_else(|| ports.get(&udp_key))
                                .and_then(|b| b.as_ref())
                                .and_then(|b| b.first())
                                .and_then(|b| b.host_port.clone());
                            if binding.is_some() {
                                return binding;
                            }
                            ports
                                .into_values()
                                .flatten()
                                .flat_map(|b| b.into_iter())
                                .find_map(|b| b.host_port)
                        });

                        match host_port_opt.and_then(|p| p.parse::<u16>().ok()) {
                            Some(p) => p,
                            None => {
                                tracing::warn!(
                                    "Container {} has no host port binding for port {}",
                                    container_name,
                                    container_port
                                );
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to inspect container {}: {}", container_name, e);
                        continue;
                    }
                }
            };

            let upstream = format!("127.0.0.1:{}", host_port);
            new_routes.insert(domain.clone(), upstream.clone());
            tracing::info!("Route added: {} -> {}", domain, upstream);
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

        tracing::info!("Route refresh: {} active preview(s)", preview_rows.len());

        for (subdomain, host_port) in preview_rows {
            if let Some(sub) = subdomain {
                let upstream = format!("127.0.0.1:{}", host_port);
                new_routes.insert(sub.clone(), upstream.clone());
                tracing::info!("Preview route added: {} -> {}", sub, upstream);
            }
        }

        let mut routes = self.routes.write().await;
        *routes = new_routes;
        tracing::info!("Refreshed {} routes", routes.len());

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

        let route_target = {
            let dd = self.dashboard_domain.read().await;
            let routes = self.routes.read().await;
            resolve_route(&host, dd.as_deref(), &routes)
        };

        if route_target != RouteTarget::Missing {
            return Ok(false);
        }

        // No route found — return a friendly HTML 502 page
        tracing::warn!("No route found for host '{}', returning 502 page", host);

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
            host = host
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

        let route_target = {
            let dd = self.dashboard_domain.read().await;
            let routes = self.routes.read().await;
            resolve_route(&host, dd.as_deref(), &routes)
        };

        tracing::info!(
            "Routing request for host: {:?} (route map has {} entries)",
            host,
            self.routes.read().await.len(),
        );

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
            RouteTarget::Missing => {}
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
                server_addr: String::new(),
            };
            let db_url = format!("sqlite:{}", proxy_for_refresh.db_path);
            let db_pool = SqlitePool::connect(&db_url).await.unwrap_or_else(|err| {
                panic!("failed to open SQLite database for ACME loop: {err}")
            });

            // Spawn the HTTP server for port 80 (ACME challenges + redirect)
            let challenges_http = challenges_bg.clone();
            tokio::spawn(async move {
                if let Err(e) = run_http_server(challenges_http).await {
                    tracing::error!("HTTP server (port 80) failed: {}", e);
                }
            });

            // Spawn ACME renewal loop (every 6 h) — handles cert renewal for
            // domains already in the CertStore plus the dashboard domain.
            let cert_store_acme = cert_store_bg.clone();
            let challenges_acme = challenges_bg.clone();
            let data_dir_acme = data_dir_bg.clone();
            let dashboard_domain_acme = dashboard_domain_shared.clone();
            let db_pool_acme = db_pool.clone();
            tokio::spawn(async move {
                // Initial delay to let routes populate
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;

                loop {
                    let mut domains = cert_store_acme.domains().await;
                    // Include the dashboard domain so it gets a cert too
                    {
                        let dd = dashboard_domain_acme.read().await;
                        if let Some(ref d) = *dd
                            && !domains.contains(d)
                        {
                            domains.push(d.clone());
                        }
                    }
                    if let Err(e) = acme::renewal_check(
                        &domains,
                        &data_dir_acme,
                        Some(&db_pool_acme),
                        &cert_store_acme,
                        &challenges_acme,
                        acme_staging,
                    )
                    .await
                    {
                        tracing::error!("ACME renewal check failed: {}", e);
                    }

                    // Check every 6 hours
                    tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                }
            });

            // Spawn preview cert provisioning loop (every 30 s) — provisions
            // TLS certs for preview subdomains that appear in the route map but
            // don't yet have a cert. Runs more frequently than the 6-hour renewal
            // loop so new previews get a valid cert within ~30 s of being routed.
            let cert_store_preview = cert_store_bg;
            let challenges_preview = challenges_bg;
            let data_dir_preview = data_dir_bg;
            let routes_preview = routes.clone();
            let db_pool_preview = db_pool.clone();
            tokio::spawn(async move {
                // Initial delay to let the first route refresh complete.
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;

                loop {
                    let route_keys: Vec<String> = {
                        let map = routes_preview.read().await;
                        map.keys().cloned().collect()
                    };

                    for domain in route_keys {
                        if !cert_store_preview.has_cert(&domain).await {
                            tracing::info!(
                                "[preview-cert] No cert for preview domain {}, provisioning…",
                                domain
                            );
                            if let Err(e) = acme::provision_cert(
                                &domain,
                                &data_dir_preview,
                                &cert_store_preview,
                                &challenges_preview,
                                acme_staging,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "[preview-cert] Failed to provision cert for {}: {}",
                                    domain,
                                    e
                                );
                                let _ = acme::persist_certificate_status(
                                    &db_pool_preview,
                                    &data_dir_preview,
                                    &domain,
                                    "failed",
                                    None,
                                    None,
                                )
                                .await;
                            } else {
                                let cert_path = data_dir_preview
                                    .join("certs")
                                    .join(&domain)
                                    .join("cert.pem");
                                if let Ok(cert_pem) = tokio::fs::read(&cert_path).await
                                    && let Ok((issued_at, expires_at)) =
                                        acme::certificate_timestamps(&cert_pem)
                                {
                                    let _ = acme::persist_certificate_status(
                                        &db_pool_preview,
                                        &data_dir_preview,
                                        &domain,
                                        "active",
                                        Some(issued_at),
                                        Some(expires_at),
                                    )
                                    .await;
                                }
                            }
                        }
                    }

                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });

            // Route refresh loop — also refreshes dashboard_domain from DB
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

/// Simple HTTP server on port 80 that:
/// - Serves ACME HTTP-01 challenge responses at `/.well-known/acme-challenge/{token}`
/// - Redirects all other traffic to HTTPS (301)
async fn run_http_server(challenges: ChallengeMap) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("0.0.0.0:80").await?;
    tracing::info!("HTTP redirect server listening on :80");

    loop {
        let (mut stream, _addr) = listener.accept().await?;
        let challenges = challenges.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let n = match stream.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };

            let request = String::from_utf8_lossy(&buf[..n]);
            let response = build_http_redirect_or_challenge_response(&request, &challenges).await;
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
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
                host_port INTEGER
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
                &routes
            ),
            RouteTarget::Dashboard,
            "resolve_route should prioritize the configured dashboard domain over app routes"
        );
        assert_eq!(
            resolve_route("app.example.com", Some("dashboard.example.com"), &routes),
            RouteTarget::Upstream("127.0.0.1:4200".to_string()),
            "resolve_route should return the configured upstream for known app hosts"
        );
        assert_eq!(
            resolve_route(
                "missing.example.com",
                Some("dashboard.example.com"),
                &routes
            ),
            RouteTarget::Missing,
            "resolve_route should report missing for unknown hosts"
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

    #[tokio::test]
    async fn build_http_redirect_or_challenge_response_returns_acme_body_for_known_token() {
        let challenges = acme::new_challenge_map();
        challenges
            .write()
            .await
            .insert("token-123".to_string(), "token-123.key-auth".to_string());

        let response = build_http_redirect_or_challenge_response(
            &sample_http_request(
                "/.well-known/acme-challenge/token-123",
                Some("app.example.com"),
            ),
            &challenges,
        )
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
        let challenges = acme::new_challenge_map();

        let response = build_http_redirect_or_challenge_response(
            &sample_http_request(
                "/.well-known/acme-challenge/missing-token",
                Some("app.example.com"),
            ),
            &challenges,
        )
        .await;

        assert_eq!(
            response, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            "unknown ACME challenge tokens should return a 404 without a response body"
        );
    }

    #[tokio::test]
    async fn build_http_redirect_or_challenge_response_redirects_other_requests_to_https() {
        let challenges = acme::new_challenge_map();

        let response = build_http_redirect_or_challenge_response(
            &sample_http_request("/dashboard", Some("dashboard.example.com")),
            &challenges,
        )
        .await;

        assert_eq!(
            response,
            "HTTP/1.1 301 Moved Permanently\r\nLocation: https://dashboard.example.com/dashboard\r\nContent-Length: 0\r\n\r\n",
            "non-ACME HTTP requests should be redirected to the equivalent HTTPS URL"
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
            resolve_route("dashboard.example.com", dd.as_deref(), &routes)
        };
        let app_target = {
            let dd = proxy.dashboard_domain.read().await;
            let routes = proxy.routes.read().await;
            resolve_route("app.example.com", dd.as_deref(), &routes)
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
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at)
             VALUES ('dashboard_domain', ?, datetime('now'))",
        )
        .bind(value)
        .execute(pool)
        .await
        .unwrap_or_else(|err| panic!("failed to insert dashboard_domain setting: {err}"));
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

        insert_preview(
            &pool,
            "preview-running",
            Some("pr-42-frontend.example.com"),
            Some(5100),
            "running",
            "2999-01-01T00:00:00Z",
        )
        .await;
        insert_preview(
            &pool,
            "preview-expired",
            Some("expired-frontend.example.com"),
            Some(5200),
            "running",
            "2000-01-01T00:00:00Z",
        )
        .await;
        insert_preview(
            &pool,
            "preview-failed",
            Some("failed-frontend.example.com"),
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
            routes.get("pr-42-frontend.example.com"),
            Some(&"127.0.0.1:5100".to_string()),
            "refresh_routes should route active preview deployments with stored host_port values"
        );
        assert!(
            !routes.contains_key("worker.example.com"),
            "refresh_routes should ignore apps that are not running"
        );
        assert!(
            !routes.contains_key("expired-frontend.example.com"),
            "refresh_routes should ignore expired preview deployments"
        );
        assert!(
            !routes.contains_key("failed-frontend.example.com"),
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
            Some("pr-99-api.example.com"),
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
            routes.get("pr-99-api.example.com"),
            Some(&"127.0.0.1:6300".to_string()),
            "refresh_routes should retain only active preview routes from the latest refresh"
        );

        let dashboard_domain = proxy.dashboard_domain.read().await.clone();
        assert_eq!(
            dashboard_domain, None,
            "refresh_routes should clear the shared dashboard domain when the setting becomes empty"
        );
    }
}
