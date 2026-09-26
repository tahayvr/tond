// auth/authz_tests.rs — end-to-end authorization through a real router
//
// Drives real handlers through axum routing (so `MatchedPath` and path params
// are present, as in production) with API tokens, to check scope denial,
// resource filters, token status and IP allow-lists together.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
    routing::get,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tond_common::{CreateTokenRequest, ServerSettings};
use tower::ServiceExt;

use crate::auth::{Principal, RateLimiter, tokens};
use crate::handlers;
use crate::state::AppState;

async fn test_state() -> AppState {
    let db = SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap_or_else(|err| panic!("pool: {err}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|err| panic!("migrate: {err}"));
    for (id, name, project) in [
        ("app-1", "web", None),
        ("app-2", "api", Some("proj-1")),
        ("app-3", "worker", None),
    ] {
        if let Some(p) = project {
            sqlx::query("INSERT OR IGNORE INTO projects (id, name) VALUES (?, 'shop')")
                .bind(p)
                .execute(&db)
                .await
                .unwrap_or_else(|err| panic!("seed project: {err}"));
        }
        sqlx::query(
            "INSERT INTO apps (id, name, project_id, created_at, updated_at) VALUES (?, ?, ?, 'x', 'x')",
        )
        .bind(id)
        .bind(name)
        .bind(project)
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("seed app: {err}"));
    }
    AppState {
        db,
        docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
            .unwrap_or_else(|err| panic!("docker: {err}")),
        rate_limiter: RateLimiter::new(1000, 1000.0),
        settings: Arc::new(RwLock::new(ServerSettings::default())),
    }
}

fn router(state: AppState, peer: &str) -> Router {
    Router::new()
        .route("/api/apps", get(handlers::list_apps))
        .route("/api/apps/{id}", get(handlers::get_app))
        .route("/api/projects/{id}", get(handlers::projects::get_project))
        .route("/api/databases", get(handlers::databases::list_databases))
        .route("/api/audit-log", get(handlers::audit_log::list_audit_log))
        .route("/metrics", get(handlers::system_self::metrics))
        .with_state(state)
        .layer(Extension(ConnectInfo(
            peer.parse::<SocketAddr>()
                .unwrap_or_else(|err| panic!("peer: {err}")),
        )))
}

async fn token(state: &AppState, req: CreateTokenRequest) -> (String, String) {
    let created = tokens::create_token(&state.db, &Principal::local_socket(), &req)
        .await
        .unwrap_or_else(|err| panic!("create token: {err}"));
    (created.secret, created.token.id)
}

fn req(name: &str, scopes: &[&str]) -> CreateTokenRequest {
    CreateTokenRequest {
        name: name.into(),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        ..CreateTokenRequest::default()
    }
}

/// GET `path` with the token; returns (status, JSON body).
async fn call(app: &Router, path: &str, secret: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap_or_else(|err| panic!("request: {err}")),
        )
        .await
        .unwrap_or_else(|err| panic!("oneshot: {err}"));
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|err| panic!("body: {err}"));
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn missing_scopes_get_403_scope_denied() {
    let state = test_state().await;
    let app = router(state.clone(), "127.0.0.1:1234");
    let (secret, _) = token(&state, req("reader", &["apps:read"])).await;

    let (status, _) = call(&app, "/api/apps", &secret).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&app, "/api/databases", &secret).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "SCOPE_DENIED");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("db:read")
    );
}

#[tokio::test]
async fn resource_filters_limit_tokens_to_their_apps() {
    let state = test_state().await;
    let app = router(state.clone(), "127.0.0.1:1234");
    let (secret, _) = token(
        &state,
        CreateTokenRequest {
            app_ids: vec!["web".into()],
            ..req("ci", &["apps:read", "db:read", "events:read"])
        },
    )
    .await;

    let (status, body) = call(&app, "/api/apps", &secret).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(names, vec!["web"], "the list only shows allowed apps");

    let (status, _) = call(&app, "/api/apps/web", &secret).await;
    assert_eq!(status, StatusCode::OK, "by name");
    let (status, _) = call(&app, "/api/apps/app-1", &secret).await;
    assert_eq!(status, StatusCode::OK, "by id");
    let (status, body) = call(&app, "/api/apps/api", &secret).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "RESOURCE_DENIED");
    let (status, body) = call(&app, "/api/apps/does-not-exist", &secret).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("RESOURCE_DENIED")),
        "unknown apps look the same as foreign ones"
    );
    let (status, body) = call(&app, "/api/databases", &secret).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("RESOURCE_DENIED")),
        "routes outside the filter are refused even with the scope"
    );
    let (status, _) = call(&app, "/api/audit-log", &secret).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn project_filters_cover_the_projects_apps() {
    let state = test_state().await;
    let app = router(state.clone(), "127.0.0.1:1234");
    let (secret, _) = token(
        &state,
        CreateTokenRequest {
            project_ids: vec!["shop".into()],
            ..req("proj", &["apps:read"])
        },
    )
    .await;
    assert_eq!(call(&app, "/api/apps/api", &secret).await.0, StatusCode::OK);
    assert_eq!(
        call(&app, "/api/projects/shop", &secret).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "/api/apps/web", &secret).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn revoked_and_expired_tokens_get_401() {
    let state = test_state().await;
    let app = router(state.clone(), "127.0.0.1:1234");
    let (secret, id) = token(&state, req("old", &["apps:read"])).await;
    tokens::revoke_token(&state.db, &id)
        .await
        .unwrap_or_else(|err| panic!("revoke: {err}"));
    let (status, body) = call(&app, "/api/apps", &secret).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("TOKEN_REVOKED"))
    );

    let (secret, id) = token(&state, req("short", &["apps:read"])).await;
    sqlx::query("UPDATE api_tokens SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("expire: {err}"));
    let (status, body) = call(&app, "/api/apps", &secret).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("TOKEN_EXPIRED"))
    );
}

#[tokio::test]
async fn allowed_ips_are_checked_against_the_client_address() {
    let state = test_state().await;
    let (secret, id) = token(
        &state,
        CreateTokenRequest {
            allowed_ips: vec!["203.0.113.0/24".into()],
            ..req("pinned", &["apps:read"])
        },
    )
    .await;
    let inside = router(state.clone(), "203.0.113.50:4000");
    assert_eq!(call(&inside, "/api/apps", &secret).await.0, StatusCode::OK);
    let outside = router(state.clone(), "198.51.100.7:4000");
    let (status, body) = call(&outside, "/api/apps", &secret).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("IP_NOT_ALLOWED"))
    );

    let used: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT last_used_at, last_used_ip FROM api_tokens WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("load: {err}"));
    assert!(used.0.is_some(), "last use is recorded");
    assert_eq!(used.1.as_deref(), Some("203.0.113.50"));
}

#[tokio::test]
async fn prometheus_scrapes_metrics_over_tcp_with_a_metrics_read_token() {
    let state = test_state().await;
    let app = router(state.clone(), "203.0.113.9:4000");
    let (scraper, _) = token(&state, req("prometheus", &["metrics:read"])).await;
    let (other, _) = token(&state, req("reader", &["apps:read"])).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header(header::AUTHORIZATION, format!("Bearer {scraper}"))
                .body(Body::empty())
                .unwrap_or_else(|err| panic!("request: {err}")),
        )
        .await
        .unwrap_or_else(|err| panic!("oneshot: {err}"));
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/plain; version=0.0.4"))
    );

    let (status, body) = call(&app, "/metrics", &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "SCOPE_DENIED");

    let anonymous = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap_or_else(|err| panic!("request: {err}")),
        )
        .await
        .unwrap_or_else(|err| panic!("oneshot: {err}"));
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}
