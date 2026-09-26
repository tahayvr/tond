// auth/extractor.rs — the local Unix socket marker
//
// Requests accepted on the server's Unix socket (TOND_SOCKET_PATH, mode 0600,
// owned by root) carry the `LocalSocket` extension, which only the socket
// listener in main.rs adds (as a router layer). Principal resolution
// (principal.rs) treats them as break-glass root: every scope, audited as
// `local-socket`. TCP connections are never trusted this way, not even from
// 127.0.0.1: tond-proxy forwards internet traffic to the server over loopback
// TCP, so a loopback peer address says nothing about who the caller is. A
// header can't grant it either — headers are client-controlled.

use std::convert::Infallible;

use axum::{extract::FromRequestParts, http::request::Parts};

/// Request extension marking a request accepted on the local Unix socket.
#[derive(Clone, Copy, Debug)]
pub struct LocalSocket;

/// Extractor reporting whether the request arrived on the local Unix socket.
/// Used by endpoints only root on the host may call (e.g. the host terminal switch).
pub struct LocalPeer(pub bool);

impl<S: Send + Sync> FromRequestParts<S> for LocalPeer {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(LocalPeer(parts.extensions.get::<LocalSocket>().is_some()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{StatusCode, header};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{Role, Scope, ServerSettings};

    use super::*;
    use crate::auth::{Principal, RateLimiter};
    use crate::state::AppState;

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for extractor tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    fn request_parts() -> Parts {
        axum::http::Request::builder()
            .uri("/api/apps")
            .body(())
            .unwrap_or_else(|err| panic!("failed to build test request: {err}"))
            .into_parts()
            .0
    }

    #[tokio::test]
    async fn loopback_tcp_peer_without_credentials_is_rejected() {
        let state = test_state().await;
        let mut parts = request_parts();
        parts.extensions.insert(MockConnectInfo(
            "127.0.0.1:54321"
                .parse::<std::net::SocketAddr>()
                .unwrap_or_else(|err| panic!("failed to parse loopback addr: {err}")),
        ));

        let rejection = Principal::from_request_parts(&mut parts, &state)
            .await
            .err()
            .unwrap_or_else(|| panic!("a loopback peer without credentials must be rejected"));
        assert_eq!(rejection.status, StatusCode::UNAUTHORIZED);
        assert_eq!(rejection.code, "UNAUTHENTICATED");
    }

    #[tokio::test]
    async fn local_socket_requests_are_root_without_credentials() {
        let state = test_state().await;
        let mut parts = request_parts();
        parts.extensions.insert(LocalSocket);

        let principal = Principal::from_request_parts(&mut parts, &state)
            .await
            .unwrap_or_else(|_| panic!("the socket needs no credentials"));
        assert!(principal.is_local_socket());
        assert!(Scope::ALL.iter().all(|s| principal.has(*s)));
        assert!(
            LocalPeer::from_request_parts(&mut parts, &state)
                .await
                .is_ok_and(|LocalPeer(local)| local)
        );
    }

    #[tokio::test]
    async fn local_socket_header_cannot_be_forged() {
        let state = test_state().await;
        let mut parts = request_parts();
        parts.headers.insert(
            "x-tond-local-socket",
            axum::http::HeaderValue::from_static("1"),
        );
        assert!(
            Principal::from_request_parts(&mut parts, &state)
                .await
                .is_err()
        );
        assert!(
            LocalPeer::from_request_parts(&mut parts, &state)
                .await
                .is_ok_and(|LocalPeer(local)| !local)
        );
    }

    #[tokio::test]
    async fn sessions_resolve_to_the_users_role_scopes() {
        let state = test_state().await;
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES ('u1', 'v@example.com', 'hash', 'viewer', ?, ?)",
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("seed user: {err}"));
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, 'u1', ?, ?)",
        )
        .bind(tond_common::hash_key("tok"))
        .bind((now + chrono::Duration::hours(1)).to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("seed session: {err}"));
        let mut parts = request_parts();
        parts.headers.insert(
            header::COOKIE,
            axum::http::HeaderValue::from_static("__Host-tond_session=tok"),
        );
        let principal = Principal::from_request_parts(&mut parts, &state)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(principal.scopes(), Role::Viewer.scopes());
        assert_eq!(principal.session_user_id(), Some("u1"));

        let mut bad = request_parts();
        bad.headers.insert(
            header::COOKIE,
            axum::http::HeaderValue::from_static("__Host-tond_session=other"),
        );
        let err = Principal::from_request_parts(&mut bad, &state)
            .await
            .err()
            .unwrap_or_else(|| panic!("unknown sessions are refused"));
        assert_eq!(err.code, "SESSION_EXPIRED");
    }

    #[tokio::test]
    async fn malformed_bearer_values_are_invalid_tokens() {
        let state = test_state().await;
        let mut parts = request_parts();
        parts.headers.insert(
            header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer 0000"),
        );
        let err = Principal::from_request_parts(&mut parts, &state)
            .await
            .err()
            .unwrap_or_else(|| panic!("garbage tokens are refused"));
        assert_eq!(
            (err.status, err.code),
            (StatusCode::UNAUTHORIZED, "TOKEN_INVALID")
        );
    }
}
