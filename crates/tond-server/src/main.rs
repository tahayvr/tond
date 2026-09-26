// main.rs — tond-server entry point and Axum router
//
// This is the HTTP API server for the entire Tond platform. It owns:
//   - The SQLite database connection (via AppState)
//   - The Docker daemon connection (via AppState)
//   - All HTTP route definitions
//   - Startup bootstrapping (migrations, root API key)
//
// Architecture overview:
//   Every request flows through the Axum router defined here. Protected
//   routes require two extractors in the handler signature:
//     _rate: RateLimit   — per-IP token bucket, 60 req burst / 10 req/s
//     _auth: RequireAuth — local Unix socket, Bearer token OR session cookie
//
// Route groups:
//   /health              — public liveness probe
//   /api/auth/*          — session management (login, logout, setup)
//   /api/apps/*          — app lifecycle (CRUD, deploy, logs, env, domain)
//   /api/certs           — TLS certificate status
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
//                        (default tond-build.slice; empty disables)
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
//   adds the `LocalSocket` extension, which RequireAuth accepts in place of
//   credentials.
//
// Browser protections (applied to every response / TCP request):
//   auth::csrf::enforce_same_origin    — Origin check on mutations and WebSocket
//                                        upgrades, X-Tond-CSRF header on
//                                        cookie-authenticated mutations. No CORS.
//   security_headers::add_security_headers — CSP, HSTS (behind the TLS proxy),
//                                        nosniff, Referrer-Policy, X-Frame-Options.
//
// One-time credentials (never logged; see auth/secret_file.rs):
//   <data dir>/root-api-key  — root API key, written when first generated
//   <data dir>/setup-token   — first-run setup token, removed after setup

mod alerts;
mod audit_log;
mod auth;
mod backup;
mod compose_policy;
mod cron;
mod db;
mod db_credentials;
mod deploy;
mod docker;
mod domains;
mod error;
mod handlers;
mod health;
mod janitor;
mod log_bus;
mod metrics;
mod notifications;
mod proxy_link;
mod reconcile;
mod runtime;
mod secret_key;
mod security_headers;
mod state;
mod storage;
mod volumes;

use std::future::IntoFuture;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Extension, Router,
    routing::{delete, get, post, put},
};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
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
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        ))
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

    // Ensure a root API key exists (generates one on first run, into a root-only file)
    auth::ensure_root_key(
        &state.db,
        &auth::secret_file::data_dir().join(auth::secret_file::ROOT_API_KEY_FILE),
    )
    .await?;

    // While no admin exists, keep a one-time setup token in a root-only file
    auth::setup_token::ensure_setup_token(&state.db, &auth::setup_token::setup_token_path())
        .await?;

    // Finish what a previous process left behind (and resume queued deploys)
    // before anything new starts.
    reconcile::at_startup(&state).await;

    // Background loops. Each sleeps until its next deadline or until a table
    // it watches changes (runtime/changes.rs), so an idle server barely wakes.
    tokio::spawn(health::run_health_checker(state.clone()));
    tokio::spawn(metrics::run_metrics_collector(state.clone()));
    tokio::spawn(alerts::run_alert_checker(state.clone()));
    tokio::spawn(cron::run_cron_scheduler(state.clone()));
    tokio::spawn(janitor::run_janitor(state.clone()));
    tokio::spawn(backup::backup_scheduler(state.clone()));
    tokio::spawn(run_preview_cleanup_loop(state.clone()));

    // Control connection to tond-proxy: route nudges, cutover acks, cert results.
    tokio::spawn(proxy_link::run(state.clone()));

    // API routes
    let api = Router::new()
        .route("/health", get(handlers::health))
        // Dashboard auth routes (no RequireAuth -- they handle their own auth)
        .route("/api/auth/status", get(handlers::auth::auth_status))
        .route("/api/auth/setup", post(handlers::auth::setup))
        .route("/api/auth/setup/check", post(handlers::auth::setup_check))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/logout", post(handlers::auth::logout))
        .route("/api/auth/me", get(handlers::auth::me))
        .route("/api/auth/users", post(handlers::auth::create_user))
        .route("/api/auth/users", get(handlers::auth::list_users))
        // Phase 7.1 — Password management & session management
        .route(
            "/api/auth/change-password",
            post(handlers::auth::change_password),
        )
        .route("/api/auth/sessions", get(handlers::auth::list_sessions))
        .route(
            "/api/auth/sessions",
            delete(handlers::auth::revoke_all_sessions),
        )
        .route(
            "/api/auth/sessions/{id}",
            delete(handlers::auth::revoke_session),
        )
        // Phase 7.1 — TOTP 2FA
        .route("/api/auth/2fa/setup", post(handlers::auth::totp_setup))
        .route("/api/auth/2fa/confirm", post(handlers::auth::totp_confirm))
        .route("/api/auth/2fa/disable", post(handlers::auth::totp_disable))
        .route("/api/auth/2fa/verify", post(handlers::auth::totp_verify))
        .route(
            "/api/onboarding/status",
            get(handlers::onboarding::get_status),
        )
        // App management routes (require API key or session)
        .route("/api/apps", get(handlers::list_apps))
        .route("/api/apps", post(handlers::create_app))
        .route("/api/apps/{id}", get(handlers::get_app))
        .route("/api/apps/{id}", delete(handlers::delete_app))
        .route("/api/apps/{id}/deploy", post(handlers::deploy_app))
        .route("/api/apps/{id}/stop", post(handlers::stop_app))
        .route("/api/apps/{id}/start", post(handlers::start_app))
        .route("/api/apps/{id}/restart", post(handlers::restart_app))
        .route("/api/apps/{id}/logs", get(handlers::app_logs))
        .route(
            "/api/apps/{id}/logs/stream",
            get(handlers::logs::stream_logs),
        )
        .route(
            "/api/system/logs/stream",
            get(handlers::logs::stream_system_logs),
        )
        .route(
            "/api/terminal/targets",
            get(handlers::terminal::list_targets),
        )
        .route(
            "/api/terminal/tickets",
            post(handlers::terminal::create_ticket),
        )
        .route("/api/terminal/ws", get(handlers::terminal::terminal_ws))
        .route(
            "/api/apps/{id}/env",
            get(handlers::env::get_env).patch(handlers::env::patch_env),
        )
        .route(
            "/api/apps/{id}/env/{key}",
            get(handlers::env::reveal_env_var),
        )
        .route(
            "/api/apps/{id}/preview-env",
            axum::routing::patch(handlers::env::patch_preview_env),
        )
        .route(
            "/api/apps/{id}/preview-env/{key}",
            get(handlers::env::reveal_preview_env_var),
        )
        .route("/api/apps/{id}/domain", put(handlers::update_domain))
        .route("/api/apps/{id}/health", put(handlers::update_health))
        .route("/api/compose/validate", post(handlers::validate_compose))
        .route("/api/apps/{id}/volumes", post(handlers::add_volume))
        .route("/api/apps/{id}/volumes", delete(handlers::remove_volume))
        .route(
            "/api/apps/{id}/drain-timeout",
            put(handlers::update_drain_timeout),
        )
        .route(
            "/api/apps/{id}/resources",
            axum::routing::patch(handlers::update_app_resources),
        )
        .route("/api/apps/{id}/upload", post(handlers::upload_deploy))
        // Deployment history and rollback
        .route(
            "/api/apps/{id}/deployments",
            get(handlers::deployments::list_deployments),
        )
        .route(
            "/api/apps/{id}/deployments/{deployment_id}/generated-dockerfile",
            get(handlers::deployments::get_generated_dockerfile),
        )
        .route(
            "/api/apps/{id}/rollback",
            post(handlers::deployments::rollback_app),
        )
        // Certificate management
        .route("/api/certs", get(handlers::list_certs))
        .route("/api/certs/{domain}/retry", post(handlers::retry_cert))
        // Server settings
        .route("/api/settings", get(handlers::settings::get_settings))
        .route("/api/settings", put(handlers::settings::update_settings))
        // Host terminal switch: readable by any authenticated caller, writable
        // only over the root-only Unix socket (`tond settings host-terminal`)
        .route(
            "/api/settings/host-terminal",
            get(handlers::terminal::get_host_terminal),
        )
        .route(
            "/api/settings/host-terminal",
            put(handlers::terminal::set_host_terminal),
        )
        .route("/api/templates", get(handlers::templates::list_templates))
        // Git integration — auto-deploy toggle, webhook secret, deploy keys, source
        .route(
            "/api/apps/{id}/auto-deploy",
            put(handlers::git::update_auto_deploy),
        )
        .route(
            "/api/apps/{id}/webhook",
            get(handlers::git::get_webhook_config),
        )
        .route(
            "/api/apps/{id}/webhook-secret",
            put(handlers::git::update_webhook_secret),
        )
        .route(
            "/api/apps/{id}/webhook-secret/rotate",
            post(handlers::git::rotate_webhook_secret),
        )
        .route(
            "/api/apps/{id}/deploy-key",
            post(handlers::git::generate_deploy_key),
        )
        .route(
            "/api/apps/{id}/deploy-key",
            get(handlers::git::get_deploy_key),
        )
        .route("/api/apps/{id}/source", put(handlers::git::update_source))
        // Registry credentials (Phase 4.3)
        .route(
            "/api/registry-credentials",
            get(handlers::registry_credentials::list_registry_credentials),
        )
        .route(
            "/api/registry-credentials",
            post(handlers::registry_credentials::create_registry_credential),
        )
        .route(
            "/api/registry-credentials/{id}",
            delete(handlers::registry_credentials::delete_registry_credential),
        )
        .route(
            "/api/apps/{id}/registry-credential",
            put(handlers::registry_credentials::set_app_registry_credential),
        )
        // GitHub App integration (Phase 4.4) — protected routes
        .route(
            "/api/github-apps",
            get(handlers::github_apps::list_github_apps),
        )
        .route(
            "/api/github-apps/manifest",
            post(handlers::github_apps::get_manifest_url),
        )
        .route(
            "/api/github-apps/{id}/install",
            post(handlers::github_apps::get_install_url),
        )
        .route(
            "/api/github-apps/{id}",
            axum::routing::patch(handlers::github_apps::update_github_app),
        )
        .route(
            "/api/github-apps/{id}",
            delete(handlers::github_apps::delete_github_app),
        )
        .route(
            "/api/github-apps/{id}/repos",
            get(handlers::github_apps::list_repos),
        )
        .route(
            "/api/github-apps/{id}/repos/{owner}/{repo}/branches",
            get(handlers::github_apps::list_branches),
        )
        .route(
            "/api/apps/{id}/github-app",
            put(handlers::github_apps::set_app_github_app),
        )
        .route(
            "/api/apps/{id}/github-prs",
            get(handlers::github_apps::list_open_prs),
        )
        // GitLab PAT integration (Phase 4.5)
        .route(
            "/api/gitlab-tokens",
            get(handlers::gitlab_tokens::list_gitlab_tokens),
        )
        .route(
            "/api/gitlab-tokens",
            post(handlers::gitlab_tokens::create_gitlab_token),
        )
        .route(
            "/api/gitlab-tokens/{id}",
            delete(handlers::gitlab_tokens::delete_gitlab_token),
        )
        .route(
            "/api/gitlab-tokens/{id}/repos",
            get(handlers::gitlab_tokens::list_gitlab_repos),
        )
        .route(
            "/api/gitlab-tokens/{id}/repos/{project_id}/branches",
            get(handlers::gitlab_tokens::list_gitlab_branches),
        )
        .route(
            "/api/apps/{id}/gitlab-token",
            put(handlers::gitlab_tokens::set_app_gitlab_token),
        )
        // GitHub App callback routes — public (no RequireAuth), called by GitHub
        .route(
            "/api/github-apps/callback",
            get(handlers::github_apps::github_app_callback),
        )
        .route(
            "/api/github-apps/complete",
            post(handlers::github_apps::complete_github_flow),
        )
        .route(
            "/webhooks/source/github/install",
            get(handlers::github_apps::github_install_callback),
        )
        // Webhook endpoints — public (no RequireAuth), called by GitHub/GitLab/etc.
        .route(
            "/api/webhooks/github/{app_id}",
            post(handlers::webhooks::github_webhook),
        )
        .route(
            "/api/webhooks/gitlab/{app_id}",
            post(handlers::webhooks::gitlab_webhook),
        )
        .route(
            "/api/webhooks/gitea/{app_id}",
            post(handlers::webhooks::gitea_webhook),
        )
        .route(
            "/api/webhooks/generic/{app_id}",
            post(handlers::webhooks::generic_webhook),
        )
        // GitHub App global webhook events (push + pull_request from the App)
        .route(
            "/webhooks/source/github/events",
            post(handlers::webhooks::github_app_events),
        )
        // Preview deployments
        .route(
            "/api/apps/{id}/preview",
            post(handlers::previews::create_preview),
        )
        .route(
            "/api/apps/{id}/previews",
            get(handlers::previews::list_previews),
        )
        .route(
            "/api/apps/{id}/previews/{preview_id}",
            delete(handlers::previews::delete_preview),
        )
        .route(
            "/api/apps/{id}/preview-config",
            put(handlers::previews::update_preview_config),
        )
        // Metrics
        .route(
            "/api/apps/{id}/metrics",
            get(handlers::metrics::get_app_metrics),
        )
        .route(
            "/api/system/metrics",
            get(handlers::metrics::get_system_metrics),
        )
        .route("/api/system/self", get(handlers::system_self::get_self))
        .route("/metrics", get(handlers::system_self::metrics))
        // Alert rules
        .route("/api/alerts", get(handlers::alerts::list_alerts))
        .route("/api/alerts", post(handlers::alerts::create_alert))
        .route("/api/alerts/{id}", get(handlers::alerts::get_alert))
        .route(
            "/api/alerts/{id}",
            axum::routing::patch(handlers::alerts::update_alert),
        )
        .route("/api/alerts/{id}", delete(handlers::alerts::delete_alert))
        // Alert ↔ integration links
        .route(
            "/api/alerts/{id}/integrations",
            get(handlers::alerts::list_alert_integrations),
        )
        .route(
            "/api/alerts/{id}/integrations",
            post(handlers::alerts::link_alert_integration),
        )
        .route(
            "/api/alerts/{id}/integrations/{int_id}",
            delete(handlers::alerts::unlink_alert_integration),
        )
        // Notification integrations
        .route(
            "/api/integrations",
            get(handlers::integrations::list_integrations),
        )
        .route(
            "/api/integrations",
            post(handlers::integrations::create_integration),
        )
        .route(
            "/api/integrations/{id}",
            get(handlers::integrations::get_integration),
        )
        .route(
            "/api/integrations/{id}",
            axum::routing::patch(handlers::integrations::update_integration),
        )
        .route(
            "/api/integrations/{id}",
            delete(handlers::integrations::delete_integration),
        )
        .route(
            "/api/integrations/{id}/test",
            post(handlers::integrations::test_integration),
        )
        // Audit log
        .route("/api/audit-log", get(handlers::audit_log::list_audit_log))
        // Projects (Phase 6.1)
        .route("/api/projects", get(handlers::projects::list_projects))
        .route("/api/projects", post(handlers::projects::create_project))
        .route("/api/projects/{id}", get(handlers::projects::get_project))
        .route(
            "/api/projects/{id}",
            put(handlers::projects::update_project),
        )
        .route(
            "/api/projects/{id}",
            delete(handlers::projects::delete_project),
        )
        .route(
            "/api/projects/{id}/deploy",
            post(handlers::projects::deploy_project),
        )
        // Project-level previews (Phase 6.2)
        .route(
            "/api/projects/{id}/previews",
            get(handlers::project_previews::list_project_previews),
        )
        .route(
            "/api/projects/{id}/previews",
            post(handlers::project_previews::create_project_preview),
        )
        .route(
            "/api/projects/{id}/previews/{preview_id}",
            delete(handlers::project_previews::delete_project_preview),
        )
        .route("/api/apps/{id}/project", put(handlers::assign_project))
        // Managed Databases (Phase 6.3)
        .route("/api/databases", get(handlers::databases::list_databases))
        .route("/api/databases", post(handlers::databases::create_database))
        .route(
            "/api/databases/logs/stream",
            get(handlers::logs::stream_all_database_logs),
        )
        .route(
            "/api/databases/{id}",
            get(handlers::databases::get_database),
        )
        .route(
            "/api/databases/{id}",
            delete(handlers::databases::delete_database),
        )
        .route(
            "/api/databases/{id}",
            axum::routing::patch(handlers::databases::update_database),
        )
        .route(
            "/api/databases/{id}/stop",
            post(handlers::databases::stop_database),
        )
        .route(
            "/api/databases/{id}/start",
            post(handlers::databases::start_database),
        )
        .route(
            "/api/databases/{id}/redeploy",
            post(handlers::databases::redeploy_database),
        )
        .route(
            "/api/databases/{id}/credentials",
            axum::routing::patch(handlers::databases::update_database_credentials),
        )
        .route(
            "/api/databases/{id}/logs/stream",
            get(handlers::logs::stream_database_logs),
        )
        // Cron Jobs (Phase 6.4)
        .route("/api/cron", get(handlers::cron::list_cron_jobs))
        .route("/api/cron", post(handlers::cron::create_cron_job))
        .route("/api/cron/{id}", get(handlers::cron::get_cron_job))
        .route(
            "/api/cron/{id}",
            axum::routing::patch(handlers::cron::update_cron_job),
        )
        .route("/api/cron/{id}", delete(handlers::cron::delete_cron_job))
        .route(
            "/api/cron/{id}/trigger",
            post(handlers::cron::trigger_cron_job),
        )
        .route(
            "/api/cron/{id}/runs",
            get(handlers::cron::list_cron_job_runs),
        )
        .route(
            "/api/cron/{id}/runs/stream",
            get(handlers::logs::stream_cron_runs),
        )
        // Storage providers (Phase 9.2)
        .route(
            "/api/storage-providers",
            get(handlers::storage_providers::list_storage_providers),
        )
        .route(
            "/api/storage-providers",
            post(handlers::storage_providers::create_storage_provider),
        )
        .route(
            "/api/storage-providers/{id}",
            get(handlers::storage_providers::get_storage_provider),
        )
        .route(
            "/api/storage-providers/{id}",
            axum::routing::patch(handlers::storage_providers::update_storage_provider),
        )
        .route(
            "/api/storage-providers/{id}",
            delete(handlers::storage_providers::delete_storage_provider),
        )
        .route(
            "/api/storage-providers/{id}/test",
            post(handlers::storage_providers::test_storage_provider),
        )
        // Snapshots / Backup config (Phase 9.2)
        .route("/api/snapshots", get(handlers::backups::list_snapshots))
        .route("/api/snapshots", post(handlers::backups::create_snapshot))
        .route(
            "/api/snapshots/config",
            get(handlers::backups::get_backup_config),
        )
        .route(
            "/api/snapshots/config",
            put(handlers::backups::update_backup_config),
        )
        .route(
            "/api/snapshots/{id}",
            delete(handlers::backups::delete_snapshot),
        )
        .route(
            "/api/snapshots/{id}/logs/stream",
            get(handlers::backups::stream_snapshot_logs),
        )
        .route(
            "/api/snapshots/{id}/restore",
            post(handlers::backups::restore_snapshot),
        )
        .route(
            "/api/snapshots/{id}/restore-app",
            post(handlers::backups::restore_app),
        )
        .route(
            "/api/snapshots/{id}/restore-database",
            post(handlers::backups::restore_database),
        )
        // Latency histogram per route template (runtime/http_metrics.rs).
        .route_layer(axum::middleware::from_fn(http_metrics::track))
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
        let serve_dir = ServeDir::new(&dashboard_dir)
            .not_found_service(ServeFile::new(format!("{}/index.html", dashboard_dir)));
        api.fallback_service(serve_dir)
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
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_excluded_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
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
