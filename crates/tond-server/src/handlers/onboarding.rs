use std::{net::IpAddr, time::Duration};

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;

use crate::{
    AppState,
    auth::{RateLimit, RequireAuth},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainStatusResponse {
    pub configured: bool,
    pub expected_target: Option<String>,
    pub dns_matches_server: Option<bool>,
    pub https_ready: Option<bool>,
    pub certificate_status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OnboardingStatusResponse {
    pub dashboard_domain: String,
    pub is_fresh_install: bool,
    pub has_projects: bool,
    pub has_apps: bool,
    pub has_databases: bool,
    pub has_cron_jobs: bool,
    pub server_public_ip: Option<String>,
    pub domain_status: DomainStatusResponse,
}

pub async fn get_status(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let settings = state.settings.read().await.clone();
    let server_public_ip = fetch_public_ip().await;
    let resolved_ips = if settings.dashboard_domain.trim().is_empty() {
        None
    } else {
        resolve_domain_ips(&settings.dashboard_domain).await
    };

    let has_projects = has_any_rows(&state.db, "projects").await;
    let has_apps = has_any_rows(&state.db, "apps").await;
    let has_databases = has_any_rows(&state.db, "databases").await;
    let has_cron_jobs = has_any_rows(&state.db, "cron_jobs").await;
    let domain_status = build_domain_status(
        &settings.dashboard_domain,
        server_public_ip,
        resolved_ips.as_deref(),
        certificate_status(&state.db, &settings.dashboard_domain).await,
    );

    let status = OnboardingStatusResponse {
        dashboard_domain: settings.dashboard_domain,
        is_fresh_install: !(has_projects || has_apps || has_databases || has_cron_jobs),
        has_projects,
        has_apps,
        has_databases,
        has_cron_jobs,
        server_public_ip: domain_status.expected_target.clone(),
        domain_status,
    };

    (StatusCode::OK, Json(tond_common::ApiResponse::ok(status))).into_response()
}

async fn has_any_rows(db: &sqlx::SqlitePool, table: &'static str) -> bool {
    // `table` is always a compile-time literal, so interpolating it is injection-safe.
    let query = format!("SELECT 1 FROM {table} LIMIT 1");
    sqlx::query(AssertSqlSafe(query))
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .is_some()
}

fn build_domain_status(
    dashboard_domain: &str,
    server_public_ip: Option<IpAddr>,
    resolved_ips: Option<&[IpAddr]>,
    certificate_status: Option<String>,
) -> DomainStatusResponse {
    let expected_target = server_public_ip.map(|ip| ip.to_string());
    let configured = !dashboard_domain.trim().is_empty();

    let dns_matches_server = if configured {
        match (server_public_ip, resolved_ips) {
            (Some(server_ip), Some(resolved)) => Some(resolved.contains(&server_ip)),
            _ => None,
        }
    } else {
        None
    };

    let https_ready = if configured {
        certificate_status
            .as_deref()
            .map(|status| status == "active")
    } else {
        None
    };

    DomainStatusResponse {
        configured,
        expected_target,
        dns_matches_server,
        https_ready,
        certificate_status,
    }
}

async fn certificate_status(db: &sqlx::SqlitePool, dashboard_domain: &str) -> Option<String> {
    if dashboard_domain.trim().is_empty() {
        return None;
    }

    sqlx::query_scalar::<_, String>(
        "SELECT status FROM certificates WHERE domain = ? ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(dashboard_domain)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

pub(crate) async fn fetch_public_ip() -> Option<IpAddr> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;

    for url in ["https://api.ipify.org", "https://ifconfig.me/ip"] {
        let Ok(response) = client.get(url).send().await else {
            continue;
        };
        let Ok(body) = response.text().await else {
            continue;
        };
        if let Ok(ip) = body.trim().parse::<IpAddr>() {
            return Some(ip);
        }
    }

    None
}

async fn resolve_domain_ips(domain: &str) -> Option<Vec<IpAddr>> {
    let lookup = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((domain, 80)),
    )
    .await
    .ok()?
    .ok()?;

    let ips: Vec<IpAddr> = lookup.map(|addr| addr.ip()).collect();
    if ips.is_empty() { None } else { Some(ips) }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        extract::FromRequestParts,
        http::{HeaderValue, header},
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{ApiResponse, ServerSettings};

    use super::*;
    use crate::auth::RateLimiter;

    #[test]
    fn domain_status_is_unconfigured_without_domain() {
        let status = build_domain_status("", Some("203.0.113.10".parse().unwrap()), None, None);

        assert!(
            !status.configured,
            "blank dashboard domains should report unconfigured status"
        );
        assert_eq!(
            status.expected_target.as_deref(),
            Some("203.0.113.10"),
            "server public IP should still be exposed as the expected target"
        );
        assert_eq!(
            status.dns_matches_server, None,
            "DNS matching should be absent when no dashboard domain is configured"
        );
        assert_eq!(
            status.https_ready, None,
            "HTTPS readiness should be absent when no dashboard domain is configured"
        );
    }

    #[test]
    fn domain_status_reports_matching_dns_when_resolved_ip_matches_server() {
        let server_ip = "203.0.113.10"
            .parse()
            .unwrap_or_else(|err| panic!("failed to parse test IP: {err}"));
        let resolved = vec![server_ip];

        let status = build_domain_status(
            "dash.example.com",
            Some(server_ip),
            Some(&resolved),
            Some("active".to_string()),
        );

        assert!(
            status.configured,
            "dashboard domain should be marked configured"
        );
        assert_eq!(
            status.dns_matches_server,
            Some(true),
            "matching DNS should be reported as ready"
        );
        assert_eq!(
            status.https_ready,
            Some(true),
            "active certificates should report HTTPS as ready"
        );
    }

    #[test]
    fn domain_status_reports_dns_mismatch_when_resolved_ip_differs() {
        let server_ip = "203.0.113.10"
            .parse()
            .unwrap_or_else(|err| panic!("failed to parse server test IP: {err}"));
        let resolved = vec![
            "198.51.100.20"
                .parse()
                .unwrap_or_else(|err| panic!("failed to parse resolved test IP: {err}")),
        ];

        let status = build_domain_status(
            "dash.example.com",
            Some(server_ip),
            Some(&resolved),
            Some("pending".to_string()),
        );

        assert_eq!(
            status.dns_matches_server,
            Some(false),
            "mismatched DNS should be surfaced explicitly"
        );
        assert_eq!(
            status.https_ready,
            Some(false),
            "pending certificates should not report HTTPS as ready"
        );
    }

    #[tokio::test]
    async fn certificate_status_loads_latest_certificate_state_for_domain() {
        let state = test_state().await;

        sqlx::query(
            "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at, status, created_at, updated_at)
             VALUES (?, ?, 'acme', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("cert-1")
        .bind("dash.example.com")
        .bind("/tmp/cert.pem")
        .bind("/tmp/key.pem")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-04-01T00:00:00Z")
        .bind("active")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed certificate row: {err}"));

        let status = certificate_status(&state.db, "dash.example.com").await;
        assert_eq!(
            status.as_deref(),
            Some("active"),
            "certificate status helper should read the latest status for the configured domain"
        );
    }

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for onboarding tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_admin_session(state: &AppState) -> String {
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, 'admin', ?, ?)",
        )
        .bind("user-1")
        .bind("admin@example.com")
        .bind("hashed-password")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed admin user: {err}"));

        let session_id = "session-1".to_string();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, user_agent, ip_address, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(tond_common::hash_key(&session_id))
        .bind("user-1")
        .bind("curl/8.0")
        .bind("127.0.0.1")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind("2099-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed session for onboarding test: {err}"));

        session_id
    }

    async fn response_json<T: serde::de::DeserializeOwned + serde::Serialize>(
        response: axum::response::Response,
    ) -> ApiResponse<T> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read onboarding response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode onboarding response JSON: {err}"))
    }

    async fn require_auth_for_test(state: &AppState, session_token: &str) -> RequireAuth {
        let mut parts = axum::http::Request::builder()
            .uri("/api/onboarding/status")
            .body(())
            .unwrap_or_else(|err| panic!("failed to build onboarding test request: {err}"))
            .into_parts()
            .0;
        parts.headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("__Host-tond_session={session_token}"))
                .unwrap_or_else(|err| panic!("failed to build session cookie header: {err}")),
        );

        RequireAuth::from_request_parts(&mut parts, state)
            .await
            .unwrap_or_else(|err| panic!("RequireAuth should accept a valid session: {err:?}"))
    }

    #[tokio::test]
    async fn onboarding_status_reports_fresh_install_when_no_resources_exist() {
        let state = test_state().await;
        let session = seed_admin_session(&state).await;
        let auth = require_auth_for_test(&state, &session).await;

        let response = get_status(RateLimit, auth, State(state))
            .await
            .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "onboarding status should succeed for authenticated requests"
        );

        let body: ApiResponse<OnboardingStatusResponse> = response_json(response).await;
        let status = body
            .data
            .unwrap_or_else(|| panic!("onboarding status response should include data"));

        assert!(
            status.is_fresh_install,
            "fresh installs with no resources should be reported as fresh"
        );
        assert!(
            !status.has_projects
                && !status.has_apps
                && !status.has_databases
                && !status.has_cron_jobs,
            "fresh installs should report no managed resources"
        );
        assert!(
            status.dashboard_domain.is_empty(),
            "fresh installs should return the current dashboard domain setting"
        );
        assert!(
            !status.domain_status.configured,
            "fresh installs without a dashboard domain should report domain status as unconfigured"
        );
        assert_eq!(
            status.domain_status.https_ready, None,
            "HTTPS readiness should be absent when no dashboard domain is configured"
        );
    }

    #[tokio::test]
    async fn onboarding_status_reports_existing_resources_and_dashboard_domain() {
        let state = test_state().await;
        let session = seed_admin_session(&state).await;
        let auth = require_auth_for_test(&state, &session).await;

        {
            let mut settings = state.settings.write().await;
            settings.dashboard_domain = "dash.example.com".to_string();
        }

        sqlx::query(
            "INSERT INTO projects (id, name, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind("project-1")
        .bind("Core")
        .bind("Primary project")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed project: {err}"));

        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_excluded_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, NULL, 'git', ?, 'main', 3000, NULL, NULL, NULL, NULL, 'created', '{}', NULL, 30, 3, 0, '[]', 30, 'tbuild', NULL, 0, NULL, NULL, ?, NULL, NULL, NULL, 0, '[]', '{}', NULL, NULL, NULL, 0, ?, ?)",
        )
        .bind("app-1")
        .bind("web")
        .bind("https://github.com/example/web")
        .bind("project-1")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app: {err}"));

        sqlx::query(
            "INSERT INTO databases (id, name, db_type, project_id, container_id, host_port, status, db_name, db_user, db_password, root_password, volume_path, memory_limit, custom_external_url, custom_internal_url, image_used, custom_image, created_at, updated_at)
             VALUES (?, ?, 'postgres', ?, NULL, 5432, 'running', ?, ?, ?, ?, ?, NULL, NULL, NULL, ?, NULL, ?, ?)",
        )
        .bind("db-1")
        .bind("main-db")
        .bind("project-1")
        .bind("app")
        .bind("app")
        .bind("secret")
        .bind("rootsecret")
        .bind("/opt/tond/data/databases/main-db")
        .bind("postgres:16")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed database: {err}"));

        sqlx::query(
            "INSERT INTO cron_jobs (id, name, schedule, timezone, image, command, project_id, status, enabled, last_run_at, next_run_at, timeout_seconds, env, last_run_status, concurrency_policy, memory_mb, cpu_shares, max_retries, retry_delay_seconds, volumes, created_at, updated_at)
             VALUES (?, ?, ?, NULL, ?, NULL, ?, 'idle', 1, NULL, NULL, NULL, NULL, NULL, 'skip', NULL, NULL, 0, 60, NULL, ?, ?)",
        )
        .bind("cron-1")
        .bind("nightly")
        .bind("0 0 * * *")
        .bind("alpine:3")
        .bind("project-1")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed cron job: {err}"));

        sqlx::query(
            "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at, status, created_at, updated_at)
             VALUES (?, ?, 'acme', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("cert-1")
        .bind("dash.example.com")
        .bind("/tmp/cert.pem")
        .bind("/tmp/key.pem")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-04-01T00:00:00Z")
        .bind("active")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed certificate: {err}"));

        let response = get_status(RateLimit, auth, State(state))
            .await
            .into_response();
        let body: ApiResponse<OnboardingStatusResponse> = response_json(response).await;
        let status = body
            .data
            .unwrap_or_else(|| panic!("onboarding status response should include data"));

        assert!(
            !status.is_fresh_install,
            "servers with resources should not be reported as fresh installs"
        );
        assert!(status.has_projects, "seeded project should be reported");
        assert!(status.has_apps, "seeded app should be reported");
        assert!(status.has_databases, "seeded database should be reported");
        assert!(status.has_cron_jobs, "seeded cron job should be reported");
        assert_eq!(
            status.dashboard_domain, "dash.example.com",
            "current dashboard domain should be included in onboarding status"
        );
        assert!(
            status.domain_status.configured,
            "configured dashboard domains should be reflected in domain status"
        );
        assert_eq!(
            status.domain_status.https_ready,
            Some(true),
            "active certificates should report HTTPS readiness"
        );
        assert_eq!(
            status.domain_status.certificate_status.as_deref(),
            Some("active"),
            "certificate status should be exposed for the configured dashboard domain"
        );
    }
}
