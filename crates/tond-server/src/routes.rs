// routes.rs — the HTTP route table
//
// Every route is one line: `METHOD "path" => handler [layer <expr>];`.
// `api_routes!` turns the table into:
//   api_router()   the axum Router main.rs serves (both listeners)
//   route_table()  (tests) each route with its handler's extractors, from
//                  which openapi/ derives auth, scopes, parameters and the
//                  request body schema for docs/openapi.json
// So a route cannot exist without a spec entry: the spec is generated from
// this table, and a test fails when docs/openapi.json is out of date or a
// route has no summary in openapi/catalog.rs.
//
// Conventions:
//   - Versioned under /api/v1 (WebSockets too). /health and /metrics stay at
//     the root for probes and Prometheus.
//   - Every protected handler declares its scope with `Authz<scope::X>`
//     (x-tond-scopes in the spec comes from that type).
//   - Unknown /api paths answer a JSON ROUTE_NOT_FOUND (api_errors.rs), not
//     the dashboard SPA.

use axum::Router;

use crate::handlers;
use crate::openapi;
use crate::state::AppState;

macro_rules! api_routes {
    ( $( $method:ident $path:literal => $handler:path $( [layer $($layer:tt)+] )? ; )* ) => {
        /// The API router (without state and middleware; see main.rs).
        pub fn api_router() -> Router<AppState> {
            let router = Router::new();
            $(
                let router = router.route(
                    $path,
                    api_routes!(@method $method, $handler) $( .layer($($layer)+) )?,
                );
            )*
            router
        }

        /// Every route with its handler's extractors (for the OpenAPI spec).
        #[cfg(test)]
        pub fn route_table(
            generator: &mut schemars::SchemaGenerator,
        ) -> Vec<openapi::describe::RouteInfo> {
            vec![
                $(
                    openapi::describe::RouteInfo::new(
                        stringify!($method),
                        $path,
                        stringify!($handler),
                        openapi::describe::describe(&$handler, generator),
                    ),
                )*
            ]
        }
    };
    (@method GET, $h:path) => { axum::routing::get($h) };
    (@method POST, $h:path) => { axum::routing::post($h) };
    (@method PUT, $h:path) => { axum::routing::put($h) };
    (@method PATCH, $h:path) => { axum::routing::patch($h) };
    (@method DELETE, $h:path) => { axum::routing::delete($h) };
}

api_routes! {
    GET "/health" => handlers::health;
    // The OpenAPI 3.1 contract for everything below (public: it is a schema)
    GET "/api/v1/openapi.json" => openapi::spec;
    // Dashboard auth routes (they handle their own auth)
    GET "/api/v1/auth/status" => handlers::auth::auth_status;
    POST "/api/v1/auth/setup" => handlers::auth::setup;
    POST "/api/v1/auth/setup/check" => handlers::auth::setup_check;
    POST "/api/v1/auth/login" => handlers::auth::login;
    POST "/api/v1/auth/logout" => handlers::auth::logout;
    GET "/api/v1/auth/me" => handlers::auth::me;
    GET "/api/v1/auth/whoami" => handlers::tokens::whoami;
    // One-time set-password links (invites and admin resets); public
    POST "/api/v1/auth/password-link/check" => handlers::auth::password_link_check;
    POST "/api/v1/auth/password-link/complete" => handlers::auth::password_link_complete;
    // User management (users:admin)
    GET "/api/v1/users" => handlers::users::list_users;
    POST "/api/v1/users" => handlers::users::create_user;
    PATCH "/api/v1/users/{id}" => handlers::users::update_user;
    DELETE "/api/v1/users/{id}" => handlers::users::delete_user;
    POST "/api/v1/users/{id}/password-link" => handlers::users::create_password_link;
    GET "/api/v1/users/{id}/sessions" => handlers::users::list_user_sessions;
    DELETE "/api/v1/users/{id}/sessions" => handlers::users::revoke_user_sessions;
    DELETE "/api/v1/users/{id}/sessions/{session_id}" => handlers::users::revoke_user_session;
    // API tokens
    GET "/api/v1/tokens/catalog" => handlers::tokens::token_catalog;
    GET "/api/v1/tokens" => handlers::tokens::list_tokens;
    POST "/api/v1/tokens" => handlers::tokens::create_token;
    DELETE "/api/v1/tokens/{id}" => handlers::tokens::revoke_token;
    // Phase 7.1 — Password management & session management
    POST "/api/v1/auth/change-password" => handlers::auth::change_password;
    GET "/api/v1/auth/sessions" => handlers::auth::list_sessions;
    DELETE "/api/v1/auth/sessions" => handlers::auth::revoke_all_sessions;
    DELETE "/api/v1/auth/sessions/{id}" => handlers::auth::revoke_session;
    // Phase 7.1 — TOTP 2FA
    POST "/api/v1/auth/2fa/setup" => handlers::auth::totp_setup;
    POST "/api/v1/auth/2fa/confirm" => handlers::auth::totp_confirm;
    POST "/api/v1/auth/2fa/disable" => handlers::auth::totp_disable;
    POST "/api/v1/auth/2fa/verify" => handlers::auth::totp_verify;
    GET "/api/v1/onboarding/status" => handlers::onboarding::get_status;
    // App management routes (API token or session; each handler declares its scope)
    GET "/api/v1/apps" => handlers::list_apps;
    POST "/api/v1/apps" => handlers::create_app;
    GET "/api/v1/apps/{id}" => handlers::get_app;
    DELETE "/api/v1/apps/{id}" => handlers::delete_app;
    POST "/api/v1/apps/{id}/deploy" => handlers::deploy_app;
    POST "/api/v1/apps/{id}/stop" => handlers::stop_app;
    POST "/api/v1/apps/{id}/start" => handlers::start_app;
    POST "/api/v1/apps/{id}/restart" => handlers::restart_app;
    // Docker Compose apps: services, per-service domains and ports, the file
    GET "/api/v1/apps/{id}/services" => handlers::compose::list_services;
    PUT "/api/v1/apps/{id}/services/{service}" => handlers::compose::update_service;
    GET "/api/v1/apps/{id}/services/{service}/logs" => handlers::compose::service_logs;
    POST "/api/v1/apps/{id}/services/{service}/{action}" => handlers::compose::service_action;
    PUT "/api/v1/apps/{id}/compose" => handlers::compose::update_compose;
    GET "/api/v1/apps/{id}/logs" => handlers::app_logs;
    GET "/api/v1/apps/{id}/logs/stream" => handlers::logs::stream_logs;
    GET "/api/v1/system/logs/stream" => handlers::logs::stream_system_logs;
    GET "/api/v1/terminal/targets" => handlers::terminal::list_targets;
    POST "/api/v1/terminal/tickets" => handlers::terminal::create_ticket;
    GET "/api/v1/terminal/ws" => handlers::terminal::terminal_ws;
    GET "/api/v1/apps/{id}/env" => handlers::env::get_env;
    PATCH "/api/v1/apps/{id}/env" => handlers::env::patch_env;
    GET "/api/v1/apps/{id}/env/{key}" => handlers::env::reveal_env_var;
    PATCH "/api/v1/apps/{id}/preview-env" => handlers::env::patch_preview_env;
    GET "/api/v1/apps/{id}/preview-env/{key}" => handlers::env::reveal_preview_env_var;
    PUT "/api/v1/apps/{id}/domain" => handlers::update_domain;
    GET "/api/v1/apps/{id}/domains" => handlers::app_domains::list_domains;
    POST "/api/v1/apps/{id}/domains" => handlers::app_domains::add_domain;
    POST "/api/v1/apps/{id}/domains/www-pair" => handlers::app_domains::add_www_pair;
    PATCH "/api/v1/apps/{id}/domains/{domain}" => handlers::app_domains::update_domain;
    DELETE "/api/v1/apps/{id}/domains/{domain}" => handlers::app_domains::remove_domain;
    PUT "/api/v1/apps/{id}/health" => handlers::update_health;
    PUT "/api/v1/apps/{id}/port" => handlers::update_port;
    PUT "/api/v1/apps/{id}/readiness" => handlers::update_readiness;
    POST "/api/v1/compose/validate" => handlers::validate_compose;
    POST "/api/v1/apps/{id}/volumes" => handlers::add_volume;
    DELETE "/api/v1/apps/{id}/volumes" => handlers::remove_volume;
    PUT "/api/v1/apps/{id}/drain-timeout" => handlers::update_drain_timeout;
    PATCH "/api/v1/apps/{id}/resources" => handlers::update_app_resources;
    // Uploads stream through upload.rs, which enforces the configurable
    // `upload_max_bytes` cap itself, so axum's 2 MB default is lifted.
    POST "/api/v1/apps/{id}/upload" => handlers::upload_deploy [layer axum::extract::DefaultBodyLimit::disable()];
    // Deployment history and rollback
    GET "/api/v1/apps/{id}/deployments" => handlers::deployments::list_deployments;
    GET "/api/v1/apps/{id}/deployments/stats" => handlers::deploy_stats::deploy_stats;
    GET "/api/v1/apps/{id}/deployments/{deployment_id}" => handlers::deployments::get_deployment;
    GET "/api/v1/apps/{id}/deployments/{deployment_id}/stream" => handlers::deployments::stream_deployment;
    GET "/api/v1/apps/{id}/deployments/{deployment_id}/generated-dockerfile" => handlers::deployments::get_generated_dockerfile;
    // Source inspection for the new-app wizard, DNS checks for domains
    POST "/api/v1/git/branches" => handlers::git::list_remote_branches;
    POST "/api/v1/images/inspect" => handlers::images::inspect_image;
    GET "/api/v1/domains/check" => handlers::domains::check_domain;
    POST "/api/v1/apps/{id}/rollback" => handlers::deployments::rollback_app;
    // Certificate management
    GET "/api/v1/certs" => handlers::certificates::list_certs;
    POST "/api/v1/certs/{domain}/retry" => handlers::certificates::retry_cert;
    POST "/api/v1/certs/custom" => handlers::certificates::upload_cert;
    DELETE "/api/v1/certs/custom/{domain}" => handlers::certificates::delete_custom_cert;
    GET "/api/v1/dns-providers" => handlers::dns_providers::list_dns_providers;
    POST "/api/v1/dns-providers" => handlers::dns_providers::create_dns_provider;
    PATCH "/api/v1/dns-providers/{id}" => handlers::dns_providers::update_dns_provider;
    DELETE "/api/v1/dns-providers/{id}" => handlers::dns_providers::delete_dns_provider;
    POST "/api/v1/dns-providers/{id}/test" => handlers::dns_providers::test_dns_provider;
    // Server settings
    GET "/api/v1/settings" => handlers::settings::get_settings;
    PUT "/api/v1/settings" => handlers::settings::update_settings;
    GET "/api/v1/settings/retention" => handlers::retention::get_retention;
    PUT "/api/v1/settings/retention" => handlers::retention::update_retention;
    // Host terminal switch: readable by any authenticated caller, writable
    // only over the root-only Unix socket (`tond settings host-terminal`)
    GET "/api/v1/settings/host-terminal" => handlers::terminal::get_host_terminal;
    PUT "/api/v1/settings/host-terminal" => handlers::terminal::set_host_terminal;
    GET "/api/v1/templates" => handlers::templates::list_templates;
    POST "/api/v1/templates/{id}/deploy" => handlers::templates::deploy_template;
    // Git integration — auto-deploy toggle, webhook secret, deploy keys, source
    PUT "/api/v1/apps/{id}/auto-deploy" => handlers::git::update_auto_deploy;
    GET "/api/v1/apps/{id}/webhook" => handlers::git::get_webhook_config;
    PUT "/api/v1/apps/{id}/webhook-secret" => handlers::git::update_webhook_secret;
    GET "/api/v1/apps/{id}/webhook-secret" => handlers::git::reveal_webhook_secret;
    POST "/api/v1/apps/{id}/webhook-secret/rotate" => handlers::git::rotate_webhook_secret;
    POST "/api/v1/apps/{id}/deploy-key" => handlers::git::generate_deploy_key;
    GET "/api/v1/apps/{id}/deploy-key" => handlers::git::get_deploy_key;
    PUT "/api/v1/apps/{id}/source" => handlers::git::update_source;
    // Registry credentials (Phase 4.3)
    GET "/api/v1/registry-credentials" => handlers::registry_credentials::list_registry_credentials;
    POST "/api/v1/registry-credentials" => handlers::registry_credentials::create_registry_credential;
    DELETE "/api/v1/registry-credentials/{id}" => handlers::registry_credentials::delete_registry_credential;
    PUT "/api/v1/apps/{id}/registry-credential" => handlers::registry_credentials::set_app_registry_credential;
    // GitHub App integration (Phase 4.4) — protected routes
    GET "/api/v1/github-apps" => handlers::github_apps::list_github_apps;
    POST "/api/v1/github-apps/manifest" => handlers::github_apps::get_manifest_url;
    POST "/api/v1/github-apps/{id}/install" => handlers::github_apps::get_install_url;
    PATCH "/api/v1/github-apps/{id}" => handlers::github_apps::update_github_app;
    DELETE "/api/v1/github-apps/{id}" => handlers::github_apps::delete_github_app;
    GET "/api/v1/github-apps/{id}/repos" => handlers::github_apps::list_repos;
    GET "/api/v1/github-apps/{id}/repos/{owner}/{repo}/branches" => handlers::github_apps::list_branches;
    PUT "/api/v1/apps/{id}/github-app" => handlers::github_apps::set_app_github_app;
    GET "/api/v1/apps/{id}/github-prs" => handlers::github_apps::list_open_prs;
    // GitLab PAT integration (Phase 4.5)
    GET "/api/v1/gitlab-tokens" => handlers::gitlab_tokens::list_gitlab_tokens;
    POST "/api/v1/gitlab-tokens" => handlers::gitlab_tokens::create_gitlab_token;
    DELETE "/api/v1/gitlab-tokens/{id}" => handlers::gitlab_tokens::delete_gitlab_token;
    GET "/api/v1/gitlab-tokens/{id}/repos" => handlers::gitlab_tokens::list_gitlab_repos;
    GET "/api/v1/gitlab-tokens/{id}/repos/{project_id}/branches" => handlers::gitlab_tokens::list_gitlab_branches;
    PUT "/api/v1/apps/{id}/gitlab-token" => handlers::gitlab_tokens::set_app_gitlab_token;
    GET "/api/v1/gitea-tokens" => handlers::gitea_tokens::list_gitea_tokens;
    POST "/api/v1/gitea-tokens" => handlers::gitea_tokens::create_gitea_token;
    DELETE "/api/v1/gitea-tokens/{id}" => handlers::gitea_tokens::delete_gitea_token;
    PUT "/api/v1/apps/{id}/gitea-token" => handlers::gitea_tokens::set_app_gitea_token;
    // GitHub App callback routes — public (no auth extractor), called by GitHub
    GET "/api/v1/github-apps/callback" => handlers::github_apps::github_app_callback;
    POST "/api/v1/github-apps/complete" => handlers::github_apps::complete_github_flow;
    GET "/webhooks/source/github/install" => handlers::github_apps::github_install_callback;
    // Webhook endpoints — public (no auth extractor), called by GitHub/GitLab/etc.
    POST "/api/v1/webhooks/github/{app_id}" => handlers::webhooks::github_webhook;
    POST "/api/v1/webhooks/gitlab/{app_id}" => handlers::webhooks::gitlab_webhook;
    POST "/api/v1/webhooks/gitea/{app_id}" => handlers::webhooks::gitea_webhook;
    POST "/api/v1/webhooks/generic/{app_id}" => handlers::webhooks::generic_webhook;
    // GitHub App global webhook events (push + pull_request from the App)
    POST "/webhooks/source/github/events" => handlers::webhooks::github_app_events;
    // Preview deployments
    POST "/api/v1/apps/{id}/preview" => handlers::previews::create_preview;
    GET "/api/v1/apps/{id}/previews" => handlers::previews::list_previews;
    DELETE "/api/v1/apps/{id}/previews/{preview_id}" => handlers::previews::delete_preview;
    PUT "/api/v1/apps/{id}/preview-config" => handlers::previews::update_preview_config;
    // Metrics
    GET "/api/v1/apps/{id}/events" => handlers::app_events::list_app_events;
    GET "/api/v1/apps/{id}/metrics" => handlers::metrics::get_app_metrics;
    GET "/api/v1/databases/{id}/metrics" => handlers::metrics::get_database_metrics;
    GET "/api/v1/cron/{id}/metrics" => handlers::metrics::get_cron_metrics;
    GET "/api/v1/system/metrics" => handlers::metrics::get_system_metrics;
    // Request metrics (tond-proxy traffic) and access logs
    GET "/api/v1/apps/{id}/requests" => handlers::requests::get_app_requests;
    GET "/api/v1/previews/{id}/requests" => handlers::requests::get_preview_requests;
    GET "/api/v1/previews/{id}/metrics" => handlers::requests::get_preview_metrics;
    GET "/api/v1/system/requests" => handlers::requests::get_system_requests;
    GET "/api/v1/apps/{id}/access-log" => handlers::access_log::get_access_log;
    PUT "/api/v1/apps/{id}/access-log" => handlers::access_log::update_access_log;
    DELETE "/api/v1/apps/{id}/access-log" => handlers::access_log::delete_access_log;
    GET "/api/v1/apps/{id}/access-log/download" => handlers::access_log::download_access_log;
    GET "/api/v1/system/self" => handlers::system_self::get_self;
    // Host history, disk, Docker, heartbeat; uptime checks
    GET "/api/v1/system/host" => handlers::host::get_host;
    GET "/api/v1/system/host/history" => handlers::host::get_history;
    GET "/api/v1/system/host/forecast" => handlers::host::get_forecast;
    POST "/api/v1/system/disk/refresh" => handlers::host::refresh_disk;
    GET "/api/v1/system/disk/cleanup" => handlers::host::cleanup_preview;
    POST "/api/v1/system/disk/cleanup" => handlers::host::cleanup;
    GET "/api/v1/system/heartbeat" => handlers::host::get_heartbeat;
    PUT "/api/v1/system/heartbeat" => handlers::host::update_heartbeat;
    POST "/api/v1/system/heartbeat/test" => handlers::host::test_heartbeat;
    GET "/api/v1/uptime" => handlers::uptime::list_checks;
    GET "/api/v1/apps/{id}/uptime" => handlers::uptime::get_app_check;
    PUT "/api/v1/apps/{id}/uptime" => handlers::uptime::update_app_check;
    POST "/api/v1/apps/{id}/uptime/check" => handlers::uptime::check_app_now;
    GET "/api/v1/system/uptime" => handlers::uptime::get_dashboard_check;
    PUT "/api/v1/system/uptime" => handlers::uptime::update_dashboard_check;
    POST "/api/v1/system/uptime/check" => handlers::uptime::check_dashboard_now;
    GET "/metrics" => handlers::system_self::metrics;
    // Alerts: rules, live alerts, history, snooze, quiet hours
    GET "/api/v1/alerts" => handlers::alerts::list_alerts;
    POST "/api/v1/alerts" => handlers::alerts::create_alert;
    GET "/api/v1/alerts/kinds" => handlers::alerts::list_kinds;
    GET "/api/v1/alerts/active" => handlers::alerts::list_active;
    GET "/api/v1/alerts/history" => handlers::alerts::list_history;
    GET "/api/v1/alerts/history/{id}" => handlers::alerts::get_episode;
    POST "/api/v1/alerts/history/{id}/acknowledge" => handlers::alerts::acknowledge;
    GET "/api/v1/alerts/settings" => handlers::alerts::get_settings;
    PUT "/api/v1/alerts/settings" => handlers::alerts::put_settings;
    GET "/api/v1/alerts/{id}" => handlers::alerts::get_alert;
    PATCH "/api/v1/alerts/{id}" => handlers::alerts::update_alert;
    DELETE "/api/v1/alerts/{id}" => handlers::alerts::delete_alert;
    POST "/api/v1/alerts/{id}/test" => handlers::alerts::test_alert;
    POST "/api/v1/alerts/{id}/snooze" => handlers::alerts::snooze;
    POST "/api/v1/alerts/{id}/unsnooze" => handlers::alerts::unsnooze;
    // Notification integrations
    GET "/api/v1/integrations" => handlers::integrations::list_integrations;
    POST "/api/v1/integrations" => handlers::integrations::create_integration;
    GET "/api/v1/integrations/{id}" => handlers::integrations::get_integration;
    PATCH "/api/v1/integrations/{id}" => handlers::integrations::update_integration;
    DELETE "/api/v1/integrations/{id}" => handlers::integrations::delete_integration;
    POST "/api/v1/integrations/{id}/test" => handlers::integrations::test_integration;
    // Audit log
    GET "/api/v1/audit-log" => handlers::audit_log::list_audit_log;
    GET "/api/v1/audit-log/shipping" => handlers::audit_shipping::get_shipping;
    PUT "/api/v1/audit-log/shipping" => handlers::audit_shipping::update_shipping;
    POST "/api/v1/audit-log/shipping/test" => handlers::audit_shipping::test_shipping;
    GET "/api/v1/audit-log/verify" => handlers::audit_log::verify_audit_log;
    // Projects (Phase 6.1)
    GET "/api/v1/projects" => handlers::projects::list_projects;
    POST "/api/v1/projects" => handlers::projects::create_project;
    GET "/api/v1/projects/{id}" => handlers::projects::get_project;
    PUT "/api/v1/projects/{id}" => handlers::projects::update_project;
    DELETE "/api/v1/projects/{id}" => handlers::projects::delete_project;
    POST "/api/v1/projects/{id}/deploy" => handlers::projects::deploy_project;
    // Project-level previews (Phase 6.2)
    GET "/api/v1/projects/{id}/previews" => handlers::project_previews::list_project_previews;
    POST "/api/v1/projects/{id}/previews" => handlers::project_previews::create_project_preview;
    DELETE "/api/v1/projects/{id}/previews/{preview_id}" => handlers::project_previews::delete_project_preview;
    PUT "/api/v1/apps/{id}/project" => handlers::assign_project;
    // Managed Databases (Phase 6.3)
    GET "/api/v1/databases" => handlers::databases::list_databases;
    POST "/api/v1/databases" => handlers::databases::create_database;
    GET "/api/v1/databases/logs/stream" => handlers::logs::stream_all_database_logs;
    GET "/api/v1/databases/catalog" => handlers::database_ops::get_catalog;
    GET "/api/v1/databases/{id}/backup-schedule" => handlers::database_ops::get_backup_schedule;
    PUT "/api/v1/databases/{id}/backup-schedule" => handlers::database_ops::update_backup_schedule;
    GET "/api/v1/databases/{id}/extensions" => handlers::database_ops::list_extensions;
    POST "/api/v1/databases/{id}/extensions" => handlers::database_ops::create_extension;
    POST "/api/v1/databases/{id}/upgrade" => handlers::database_ops::start_upgrade;
    POST "/api/v1/databases/{id}/upgrade/confirm" => handlers::database_ops::confirm_upgrade;
    POST "/api/v1/databases/{id}/upgrade/rollback" => handlers::database_ops::rollback_upgrade;
    POST "/api/v1/databases/{id}/upgrade/dismiss" => handlers::database_ops::dismiss_upgrade;
    GET "/api/v1/databases/{id}" => handlers::databases::get_database;
    DELETE "/api/v1/databases/{id}" => handlers::databases::delete_database;
    PATCH "/api/v1/databases/{id}" => handlers::databases::update_database;
    POST "/api/v1/databases/{id}/stop" => handlers::databases::stop_database;
    POST "/api/v1/databases/{id}/start" => handlers::databases::start_database;
    POST "/api/v1/databases/{id}/redeploy" => handlers::databases::redeploy_database;
    GET "/api/v1/databases/{id}/backups" => handlers::backups::list_database_backups;
    POST "/api/v1/databases/{id}/backups" => handlers::backups::create_database_backup;
    PATCH "/api/v1/databases/{id}/credentials" => handlers::databases::update_database_credentials;
    GET "/api/v1/databases/{id}/credentials" => handlers::databases::reveal_database_credentials;
    // Database access: project members + links (networks.rs)
    GET "/api/v1/databases/{id}/access" => handlers::database_links::get_database_access;
    PUT "/api/v1/databases/{id}/links/{app}" => handlers::database_links::link_app;
    DELETE "/api/v1/databases/{id}/links/{app}" => handlers::database_links::unlink_app;
    GET "/api/v1/apps/{id}/databases" => handlers::database_links::list_app_databases;
    GET "/api/v1/databases/{id}/logs/stream" => handlers::logs::stream_database_logs;
    // Cron Jobs (Phase 6.4)
    GET "/api/v1/cron" => handlers::cron::list_cron_jobs;
    POST "/api/v1/cron" => handlers::cron::create_cron_job;
    GET "/api/v1/cron/schedule-preview" => handlers::cron::preview_schedule;
    GET "/api/v1/cron/{id}" => handlers::cron::get_cron_job;
    PATCH "/api/v1/cron/{id}" => handlers::cron::update_cron_job;
    DELETE "/api/v1/cron/{id}" => handlers::cron::delete_cron_job;
    POST "/api/v1/cron/{id}/trigger" => handlers::cron::trigger_cron_job;
    GET "/api/v1/cron/{id}/runs" => handlers::cron::list_cron_job_runs;
    GET "/api/v1/cron/{id}/runs/stream" => handlers::logs::stream_cron_runs;
    // Storage providers (Phase 9.2)
    GET "/api/v1/storage-providers" => handlers::storage_providers::list_storage_providers;
    POST "/api/v1/storage-providers" => handlers::storage_providers::create_storage_provider;
    GET "/api/v1/storage-providers/{id}" => handlers::storage_providers::get_storage_provider;
    PATCH "/api/v1/storage-providers/{id}" => handlers::storage_providers::update_storage_provider;
    DELETE "/api/v1/storage-providers/{id}" => handlers::storage_providers::delete_storage_provider;
    POST "/api/v1/storage-providers/{id}/test" => handlers::storage_providers::test_storage_provider;
    // Snapshots / Backup config (Phase 9.2)
    GET "/api/v1/snapshots" => handlers::backups::list_snapshots;
    POST "/api/v1/snapshots" => handlers::backups::create_snapshot;
    GET "/api/v1/snapshots/config" => handlers::backups::get_backup_config;
    PUT "/api/v1/snapshots/config" => handlers::backups::update_backup_config;
    DELETE "/api/v1/snapshots/{id}" => handlers::backups::delete_snapshot;
    GET "/api/v1/snapshots/{id}/logs/stream" => handlers::backups::stream_snapshot_logs;
    POST "/api/v1/snapshots/{id}/restore" => handlers::backups::restore_snapshot;
    POST "/api/v1/snapshots/{id}/restore-app" => handlers::backups::restore_app;
    POST "/api/v1/snapshots/{id}/restore-database" => handlers::backups::restore_database;
    POST "/api/v1/snapshots/{id}/restore-new-database" => handlers::backups::restore_to_new_database;
}
