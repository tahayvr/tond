// main.rs — tond-server entry point and Axum router
//
// This is the HTTP API server for the entire Tond platform. It owns:
//   - The SQLite database connection (via AppState)
//   - The Docker daemon connection (via AppState)
//   - All HTTP route definitions
//   - Startup bootstrapping (migrations, first-run setup token)
//
// Architecture overview:
//   Every request flows through the Axum router defined here. Protected
//   routes require two extractors in the handler signature:
//     _rate: RateLimit   — per-IP token bucket, 60 req burst / 10 req/s
//     auth: Authz<scope::X> — authenticates (local Unix socket, Bearer token or
//                            session cookie) and checks the scope (auth/principal.rs)
//
// Routes:
//   The route table lives in routes.rs (one line per route, all under
//   /api/v1 except /health and /metrics); docs/openapi.json is generated from
//   it and served at /api/v1/openapi.json (openapi/). Errors use the
//   structured envelope from error.rs / api_errors.rs.
//
// Static SPA serving:
//   If TOND_DASHBOARD_DIR contains an index.html, all unmatched paths
//   fall through to it (SPA client-side routing support).
//
// Environment variables consumed here:
//   TOND_BIND          — bind address (default: 127.0.0.1)
//   TOND_PORT          — port (default: 3001)
//   TOND_DASHBOARD_DIR — path to the built SvelteKit SPA (default: ./dashboard)
//   TOND_SECRET_KEY    — at-rest encryption key (required in release builds;
//                        see secret_key.rs)
//   TOND_PREVIOUS_SECRET_KEY — old key to migrate from after a rotation
//   TOND_SOCKET_PATH   — local Unix socket for the CLI (default: tond.sock next
//                        to TOND_DB_PATH, i.e. /opt/tond/data/tond.sock)
//   TOND_PROXY_SOCKET  — tond-proxy's control socket (default: proxy.sock next
//                        to TOND_DB_PATH); see proxy_link.rs
//   TOND_DASHBOARD_DOMAIN — one-time seed for the dashboard_domain setting
//                        (written only while the setting is empty)
//   TOND_BUILD_CONCURRENCY — concurrent image builds (default 1); see deploy/queue.rs
//   TOND_BUILD_CGROUP_PARENT — cgroup parent for build containers
//                        (default tondbuild.slice; empty disables)
//   TOND_SHUTDOWN_GRACE_SECS — how long in-flight work may finish on SIGTERM
//
// Lifecycle:
//   start:    migrations, settings, reconcile.rs (interrupted work, app
//             status vs containers, resume queued deploys), background loops,
//             proxy link, listeners.
//   SIGTERM:  stop accepting connections and new work, let in-flight deploys,
//             cron runs and backups finish (grace period), mark the rest
//             interrupted, close the database (runtime/shutdown.rs).
//
// Listeners:
//   The same router is served on two listeners. TCP (TOND_BIND:TOND_PORT) is
//   what tond-proxy forwards dashboard traffic to, so it gets no implicit trust.
//   The Unix socket (mode 0600) is for the `tond` CLI on the host; its router
//   adds the `LocalSocket` extension, which authentication treats as root
//   (every scope, audited as `local-socket`).
//
// Browser protections (applied to every response / TCP request):
//   auth::csrf::enforce_same_origin    — Origin check on mutations and WebSocket
//                                        upgrades, X-Tond-CSRF header on
//                                        cookie-authenticated mutations. No CORS.
//   security_headers::add_security_headers — CSP, HSTS (behind the TLS proxy),
//                                        nosniff, Referrer-Policy, X-Frame-Options.
//
// One-time credentials (never logged; see auth/secret_file.rs):
//   <data dir>/setup-token   — first-run setup token, removed after setup
//   There is no root API key: automation gets a scoped token from
//   `tond token create` over the root-only socket.

mod access_logs;
mod alerts;
mod api_errors;
mod app_domains;
mod audit_log;
mod audit_ship;
mod auth;
mod backup;
mod compose_policy;
mod cron;
mod db;
mod db_credentials;
mod deploy;
mod dns_providers;
mod docker;
mod domains;
mod error;
mod handlers;
mod health;
mod idempotency;
mod janitor;
mod log_bus;
mod log_scrub;
mod metrics;
mod networks;
mod notifications;
mod openapi;
mod process;
mod proxy_link;
mod reconcile;
mod routes;
mod runtime;
mod secret_key;
mod secrets;
mod security_headers;
mod snapshot_archive;
mod state;
mod static_files;
mod storage;
mod upload;
mod uptime;
mod volumes;

use std::future::IntoFuture;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{Extension, routing::any};
use tower_http::trace::TraceLayer;
use tracing_subscriber::prelude::*;

use crate::runtime::{dbstats::DbStatsLayer, http_metrics, selfstats, shutdown};

pub use state::AppState;

/// Blocking-pool cap: argon2, file I/O and DNS never need hundreds of threads
/// (Tokio's default is 512), and TasksMax in tond-server.service bounds us.
const MAX_BLOCKING_THREADS: usize = 64;
/// How long open HTTP connections (log streams, terminals) get to finish
/// after SIGTERM before they are dropped.
const HTTP_DRAIN: Duration = Duration::from_secs(5);

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_keep_alive(Duration::from_secs(10))
        .build()?;
    let result = runtime.block_on(run());
    // Don't wait on blocking threads still parked in a read (terminal PTYs).
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}

async fn run() -> Result<()> {
    selfstats::mark_start();
    tond_common::install_crypto_provider();
    // Logging: the journal gets RUST_LOG (default info); DbStatsLayer counts
    // every SQL statement through its own filter (runtime/dbstats.rs).
    let log_format = tond_common::logging::LogFormat::from_env();
    tracing_subscriber::registry()
        .with(tond_common::logging::fmt_layer(log_format))
        .with(DbStatsLayer.with_filter(DbStatsLayer::filter()))
        .init();

    tracing::info!("Starting Tond server...");

    // Refuse to run a release build with the public dev encryption key
    secret_key::check_configured()?;

    // Initialize state (DB + Docker)
    let state = AppState::new().await?;

    // Run migrations
    db::migrate(&state.db).await?;

    // Move stored secrets onto a rotated key (when TOND_PREVIOUS_SECRET_KEY is set)
    secret_key::rotate_stored_secrets(&state.db).await?;

    tracing::info!("Database initialized");

    // Uploads interrupted by a restart leave their extraction dirs behind.
    upload::clear_upload_dirs(&volumes::data_dir());

    // Load persisted settings from DB into the in-memory cache
    {
        use tond_common::SettingRow;
        seed_dashboard_domain(&state.db).await?;
        let rows = sqlx::query_as::<_, SettingRow>("SELECT key, value FROM settings")
            .fetch_all(&state.db)
            .await?;
        let mut s = state.settings.write().await;
        for row in rows {
            match row.key.as_str() {
                "server_name" => s.server_name = row.value,
                "dashboard_domain" => s.dashboard_domain = row.value,
                "preview_domain" => s.preview_domain = row.value,
                "apps_domain" => s.apps_domain = row.value,
                "upload_max_bytes" => {
                    s.upload_max_bytes = row.value.parse().unwrap_or(s.upload_max_bytes)
                }
                "session_idle_timeout_hours" => {
                    if let Ok(v) = row.value.parse() {
                        s.session_idle_timeout_hours = v;
                    }
                }
                "session_max_age_days" => {
                    if let Ok(v) = row.value.parse() {
                        s.session_max_age_days = v;
                    }
                }
                _ => {}
            }
        }
        tracing::info!(
            "Settings loaded (server_name={:?}, dashboard_domain={:?}, preview_domain={:?})",
            s.server_name,
            s.dashboard_domain,
            s.preview_domain
        );
    }

    // While no admin exists, keep a one-time setup token in a root-only file
    auth::setup_token::ensure_setup_token(&state.db, &auth::setup_token::setup_token_path())
        .await?;

    // Attach databases to the networks their links call for (Docker state can
    // drift while the server is down). Once, in the background.
    {
        let state = state.clone();
        runtime::shutdown::spawn_tracked(
            async move { networks::reconcile_all_databases(&state).await },
        );
    }

    // Finish what a previous process left behind (and resume queued deploys)
    // before anything new starts.
    reconcile::at_startup(&state).await;

    // Background loops. Each sleeps until its next deadline or until a table
    // it watches changes (runtime/changes.rs), so an idle server barely wakes.
    tokio::spawn(health::run_health_checker(state.clone()));
    tokio::spawn(metrics::run_metrics_collector(state.clone()));
    tokio::spawn(alerts::run_alert_engine(state.clone()));
    tokio::spawn(cron::run_cron_scheduler(state.clone()));
    tokio::spawn(janitor::run_janitor(state.clone()));
    tokio::spawn(audit_ship::run(state.clone()));
    tokio::spawn(backup::backup_scheduler(state.clone()));
    tokio::spawn(run_preview_cleanup_loop(state.clone()));
    tokio::spawn(uptime::run(state.clone()));

    // Control connection to tond-proxy: route nudges, cutover acks, cert results.
    tokio::spawn(proxy_link::run(state.clone()));

    // API routes: the table in routes.rs, a JSON 404 for any other /api
    // path, and structured errors for axum's own plain-text rejections.
    let api = routes::api_router()
        .route("/api/{*rest}", any(api_errors::not_found))
        // Safe retries with Idempotency-Key (idempotency.rs).
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            idempotency::layer,
        ))
        // Latency histogram per route template (runtime/http_metrics.rs).
        .route_layer(axum::middleware::from_fn(http_metrics::track))
        .layer(axum::middleware::from_fn(api_errors::structured))
        .with_state(state.clone());

    // Serve the dashboard if the build directory exists, otherwise API-only mode
    let dashboard_dir =
        std::env::var("TOND_DASHBOARD_DIR").unwrap_or_else(|_| "./dashboard".to_string());

    let app = if std::path::Path::new(&dashboard_dir)
        .join("index.html")
        .exists()
    {
        tracing::info!("Serving dashboard from {}", dashboard_dir);
        security_headers::init_from_dashboard(Path::new(&dashboard_dir));
        api.fallback_service(static_files::dashboard_router(Path::new(&dashboard_dir)))
    } else {
        tracing::info!("No dashboard found at {}, running API-only", dashboard_dir);
        api
    };

    // No CORS layer on purpose: the dashboard is same-origin, and answering
    // no preflight is what makes the X-Tond-CSRF header unforgeable.
    let app = app
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::csrf::enforce_same_origin,
        ))
        .layer(axum::middleware::from_fn(
            security_headers::add_security_headers,
        ))
        .layer(TraceLayer::new_for_http());

    let bind = std::env::var("TOND_BIND").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("TOND_PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("{}:{}", bind, port);
    let socket_path = local_socket_path();
    tracing::info!("Tond listening on {} and {}", addr, socket_path.display());

    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    let unix_listener = bind_local_socket(&socket_path)?;

    let tcp = axum::serve(
        tcp_listener,
        app.clone()
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown::cancelled());
    let local = axum::serve(
        unix_listener,
        app.layer(Extension(auth::LocalSocket)).into_make_service(),
    )
    .with_graceful_shutdown(shutdown::cancelled());

    tokio::spawn(shutdown::wait_for_signal());
    let servers = async { tokio::try_join!(tcp.into_future(), local.into_future()) };
    tokio::pin!(servers);
    tokio::select! {
        result = &mut servers => { result?; }
        _ = shutdown::cancelled() => {
            // Listeners stopped accepting; give open connections a moment.
            let _ = tokio::time::timeout(HTTP_DRAIN, &mut servers).await;
        }
    }

    shutdown_gracefully(&state).await;
    Ok(())
}

/// After the listeners closed: let tracked work finish within the grace
/// period, mark whatever is left as interrupted, and close the database.
async fn shutdown_gracefully(state: &AppState) {
    let grace = shutdown::grace_from_env();
    if shutdown::drain(grace).await {
        tracing::info!("All in-flight work finished");
    } else {
        tracing::warn!(
            "{} task(s) still running after {}s; marking them interrupted",
            shutdown::in_flight(),
            grace.as_secs()
        );
        if let Err(e) = reconcile::mark_interrupted(&state.db, reconcile::SHUTDOWN_REASON).await {
            tracing::warn!("Could not mark interrupted work: {e:#}");
        }
    }
    state.db.close().await;
    tracing::info!("Tond server stopped");
}

/// TOND_DASHBOARD_DOMAIN seeds the dashboard_domain setting while it is empty
/// (install.sh sets it for tond-server; tond-proxy opens the database
/// read-only and no longer does this). After that the setting wins.
async fn seed_dashboard_domain(db: &sqlx::SqlitePool) -> Result<()> {
    let Some(domain) = std::env::var("TOND_DASHBOARD_DOMAIN")
        .ok()
        .map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
    else {
        return Ok(());
    };
    let seeded = sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ('dashboard_domain', ?, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
         WHERE settings.value = ''",
    )
    .bind(&domain)
    .execute(db)
    .await?
    .rows_affected();
    if seeded > 0 {
        tracing::info!("Dashboard domain seeded from TOND_DASHBOARD_DOMAIN: {domain}");
    }
    Ok(())
}

/// Path of the CLI's Unix socket: `TOND_SOCKET_PATH`, else `tond.sock` next to the DB.
fn local_socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("TOND_SOCKET_PATH") {
        return PathBuf::from(path);
    }
    let db_path =
        std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string());
    Path::new(&db_path).with_file_name("tond.sock")
}

/// Bind the CLI's Unix socket and restrict it to the owner (root in production).
///
/// Requests on this socket skip authentication, so its permissions are the
/// credential. The socket is created with the process umask (0755 under the
/// default 022) before the chmod; connecting needs write permission, so other
/// users can't connect in that window either.
fn bind_local_socket(path: &Path) -> Result<tokio::net::UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating socket directory {}", parent.display()))?;
    }

    // A socket left over from a previous run makes bind fail with EADDRINUSE.
    // Only remove it if it really is a socket, never some other file.
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)
            .with_context(|| format!("removing stale socket {}", path.display()))?,
        Ok(_) => anyhow::bail!(
            "{} exists and is not a socket; refusing to replace it",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", path.display())),
    }

    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("binding Unix socket {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting permissions on {}", path.display()))?;
    Ok(listener)
}

// ── Preview cleanup ──
//
// Sleeps until the earliest preview expiry (at most PREVIEW_CLEANUP_MAX_SLEEP)
// or until preview_deployments changes. Then finds all preview_deployments
// whose expires_at has passed and whose status is not yet 'expired', stops
// the container (best-effort) and marks the row as expired.

/// Longest sleep of the preview cleanup loop when nothing is about to expire.
const PREVIEW_CLEANUP_MAX_SLEEP: Duration = Duration::from_secs(3600);

async fn run_preview_cleanup_loop(state: AppState) {
    use crate::runtime::changes::{Table, Watch};
    let lp = runtime::loops::register("preview-cleanup");
    let mut watch = Watch::new(&[Table::PreviewDeployments]);
    loop {
        let run = lp.run();
        match run_preview_cleanup(&state).await {
            Ok(()) => drop(run),
            Err(e) => {
                tracing::warn!("Preview cleanup error: {}", e);
                run.fail(&e);
            }
        }
        let sleep = next_preview_expiry(&state)
            .await
            .map_or(PREVIEW_CLEANUP_MAX_SLEEP, |d| {
                d.min(PREVIEW_CLEANUP_MAX_SLEEP)
            });
        lp.wait(Some(sleep), &mut watch).await;
    }
}

/// Time until the next active preview expires (zero if one already has).
async fn next_preview_expiry(state: &AppState) -> Option<Duration> {
    let next: Option<String> = sqlx::query_scalar(
        "SELECT MIN(expires_at) FROM preview_deployments WHERE status != 'expired'",
    )
    .fetch_one(&state.db)
    .await
    .ok()
    .flatten();
    let next = next?;
    let at = chrono::DateTime::parse_from_rfc3339(&next)
        .map(|t| t.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&next, "%Y-%m-%d %H:%M:%S").map(|n| n.and_utc())
        })
        .ok()?;
    // +1 s so the comparison with datetime('now') in the cleanup query holds.
    Some((at - chrono::Utc::now()).to_std().unwrap_or(Duration::ZERO) + Duration::from_secs(1))
}

async fn run_preview_cleanup(state: &AppState) -> anyhow::Result<()> {
    let expired: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT id, container_id FROM preview_deployments
         WHERE status != 'expired' AND expires_at < datetime('now')",
    )
    .fetch_all(&state.db)
    .await?;

    for (preview_id, container_id) in expired {
        if let Some(ref cid) = container_id {
            let _ = docker::stop_container(&state.docker, cid).await;
            let _ = docker::remove_container(&state.docker, cid).await;
        }
        sqlx::query("UPDATE preview_deployments SET status = 'expired' WHERE id = ?")
            .bind(&preview_id)
            .execute(&state.db)
            .await?;
        tracing::info!("Preview {} expired and cleaned up", preview_id);
    }

    // Remove project_previews rows that have no remaining active (non-expired) children.
    sqlx::query(
        "DELETE FROM project_previews
         WHERE id NOT IN (
             SELECT DISTINCT project_preview_id
             FROM preview_deployments
             WHERE project_preview_id IS NOT NULL
               AND status != 'expired'
         )",
    )
    .execute(&state.db)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::sync::Arc;

    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    #[tokio::test]
    async fn bind_local_socket_creates_owner_only_socket() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let path = dir.path().join("nested").join("tond.sock");

        let _listener = bind_local_socket(&path)
            .unwrap_or_else(|err| panic!("failed to bind local socket: {err}"));

        let meta = std::fs::metadata(&path)
            .unwrap_or_else(|err| panic!("socket should exist after bind: {err}"));
        assert!(meta.file_type().is_socket());
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "the unauthenticated CLI socket must be owner-only"
        );
    }

    #[tokio::test]
    async fn bind_local_socket_replaces_stale_socket() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let path = dir.path().join("tond.sock");

        drop(
            bind_local_socket(&path)
                .unwrap_or_else(|err| panic!("failed to bind first socket: {err}")),
        );
        // The socket file outlives its listener, as after a crash or restart.
        assert!(path.exists());

        assert!(
            bind_local_socket(&path).is_ok(),
            "a stale socket from a previous run should be replaced"
        );
    }

    #[tokio::test]
    async fn bind_local_socket_refuses_to_replace_regular_file() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let path = dir.path().join("tond.sock");
        std::fs::write(&path, "not a socket")
            .unwrap_or_else(|err| panic!("failed to write file: {err}"));

        assert!(bind_local_socket(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).ok().as_deref(),
            Some("not a socket"),
            "a regular file at the socket path must be left untouched"
        );
    }

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db).await.unwrap_or_else(|err| {
            panic!("failed to run migrations for preview cleanup tests: {err}")
        });

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    #[tokio::test]
    async fn run_preview_cleanup_expires_old_previews_and_prunes_orphaned_project_previews() {
        let state = test_state().await;

        sqlx::query(
            "INSERT INTO projects (id, name, description, env_vars, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("project-1")
        .bind("Platform")
        .bind(Option::<String>::None)
        .bind("{}")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed project for preview cleanup test: {err}"));

        sqlx::query(
            "INSERT INTO project_previews (id, project_id, slug, ttl_seconds, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("pp-1")
        .bind("project-1")
        .bind("slug-a")
        .bind(3600)
        .bind("2025-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed project_previews row: {err}"));
        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_shared_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("app-1")
        .bind("frontend")
        .bind(Option::<String>::None)
        .bind("git")
        .bind(Some("https://example.com/repo.git".to_string()))
        .bind(Some("main".to_string()))
        .bind(3000)
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Some(268_435_456_i64))
        .bind(Some(500_000_000_i64))
        .bind("running")
        .bind(Some("{}".to_string()))
        .bind(Option::<String>::None)
        .bind(30)
        .bind(3)
        .bind(0)
        .bind("[]")
        .bind(30)
        .bind("tbuild")
        .bind(Option::<String>::None)
        .bind(0)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Some("project-1".to_string()))
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(0)
        .bind("[]")
        .bind("{}")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(0)
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app for preview cleanup test: {err}"));
        sqlx::query(
            "INSERT INTO preview_deployments (id, app_id, slug, subdomain, ttl_seconds, status, expires_at, created_at, project_preview_id, pr_number, head_sha, head_ref, pr_url, comment_id, gitlab_project_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("preview-expire")
        .bind("app-1")
        .bind("slug-a")
        .bind(Some("slug-a-app.preview.example.com"))
        .bind(3600)
        .bind("running")
        .bind("2000-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Some("pp-1".to_string()))
        .bind(Option::<i64>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<i64>::None)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed expired preview row: {err}"));

        run_preview_cleanup(&state)
            .await
            .unwrap_or_else(|err| panic!("expected run_preview_cleanup to succeed: {err}"));

        let status: String =
            sqlx::query_scalar("SELECT status FROM preview_deployments WHERE id = ?")
                .bind("preview-expire")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read preview status after cleanup: {err}"));
        assert_eq!(
            status, "expired",
            "run_preview_cleanup should mark expired preview rows as expired"
        );

        let remaining_project_previews: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM project_previews WHERE id = ?")
                .bind("pp-1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to count project_previews after cleanup: {err}")
                });
        assert_eq!(
            remaining_project_previews, 0,
            "run_preview_cleanup should prune project_preview parents with no active children"
        );
    }
}
