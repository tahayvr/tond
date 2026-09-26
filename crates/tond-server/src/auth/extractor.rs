// auth/extractor.rs — RequireAuth Axum extractor
//
// RequireAuth is a zero-sized guard struct that implements
// FromRequestParts<AppState>. Adding `_auth: RequireAuth` to a handler
// signature causes Axum to run this check before the handler body. If
// the check fails, Axum short-circuits with a 401 JSON response and the
// handler never runs.
//
// Authentication is accepted via three methods, tried in order:
//
//   1. Local Unix socket (CLI on the same machine)
//      Requests accepted on the server's Unix socket (TOND_SOCKET_PATH,
//      mode 0600, owned by root) carry the `LocalSocket` extension and are
//      trusted without credentials — root on the host is the credential.
//      TCP connections are never trusted this way, not even from
//      127.0.0.1: tond-proxy forwards internet traffic to the server over
//      loopback TCP, so a loopback peer address says nothing about who the
//      caller is.
//
//   2. Bearer token (API key / programmatic access)
//      Reads the Authorization header. If it starts with "Bearer ",
//      extracts the raw key and calls auth::validate_key() which hashes
//      it and checks the api_keys table.
//
//   3. Session cookie (dashboard browser)
//      Reads the Cookie header for a `__Host-tond_session` value and calls
//      handlers::auth::validate_session() which joins sessions + users
//      and checks expiry.
//
// If no method succeeds, returns 401.

use std::convert::Infallible;

use axum::{
    Json,
    extract::FromRequestParts,
    http::{StatusCode, header, request::Parts},
};
use tond_common::ApiResponse;

use crate::handlers::auth::{extract_session_token, validate_session};
use crate::state::AppState;

/// Request extension marking a request accepted on the local Unix socket.
///
/// Only the Unix-socket listener in `main.rs` inserts it (as a router layer), so
/// it can't be set by a client — unlike a header, which any TCP caller could forge.
#[derive(Clone, Copy, Debug)]
pub struct LocalSocket;

/// Extractor reporting whether the request arrived on the local Unix socket.
/// Used by WebSocket handlers that authenticate inside the upgraded socket.
pub struct LocalPeer(pub bool);

impl<S: Send + Sync> FromRequestParts<S> for LocalPeer {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(LocalPeer(parts.extensions.get::<LocalSocket>().is_some()))
    }
}

/// Extractor that validates authentication via:
/// 1. The local Unix socket (`LocalSocket` extension) — CLI on the same machine, no credentials needed
/// 2. `Authorization: Bearer <api-key>` header
/// 3. `__Host-tond_session` cookie (dashboard browser sessions)
///
/// Usage in handlers:
/// ```rust
/// async fn my_handler(_auth: RequireAuth, State(state): State<AppState>) { ... }
/// ```
pub struct RequireAuth;

fn unauthorized_message_for_bearer_result(
    result: anyhow::Result<Option<super::ApiKey>>,
) -> Result<(), (StatusCode, Json<ApiResponse<()>>)> {
    match result {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::err("Invalid API key")),
        )),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err("Authentication check failed")),
        )),
    }
}

fn unauthorized_message_for_session_result(
    result: Result<Option<tond_common::User>, sqlx::Error>,
) -> Result<(), (StatusCode, Json<ApiResponse<()>>)> {
    match result {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::err("Session expired or invalid")),
        )),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err("Authentication check failed")),
        )),
    }
}

impl FromRequestParts<AppState> for RequireAuth {
    type Rejection = (StatusCode, Json<ApiResponse<()>>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Try 1: local Unix socket (see module docs for why loopback TCP isn't trusted).
        if parts.extensions.get::<LocalSocket>().is_some() {
            return Ok(RequireAuth);
        }

        // Try 2: Bearer token (API key)
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());

        if let Some(header_val) = auth_header
            && let Some(raw_key) = header_val.strip_prefix("Bearer ")
        {
            unauthorized_message_for_bearer_result(super::validate_key(&state.db, raw_key).await)?;
            return Ok(RequireAuth);
        }

        // Try 3: Session cookie (dashboard)
        if let Some(token) = extract_session_token(&parts.headers) {
            unauthorized_message_for_session_result(validate_session(&state.db, &token).await)?;
            return Ok(RequireAuth);
        }

        // No method succeeded
        Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::err(
                "Authentication required. Use Authorization: Bearer <api-key> or log in via the dashboard.",
            )),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::connect_info::MockConnectInfo;
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use super::*;
    use crate::auth::RateLimiter;

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
        // tond-proxy forwards internet traffic to tond-server over loopback TCP, so a
        // 127.0.0.1 peer must never be trusted on its own.
        let state = test_state().await;
        let mut parts = request_parts();
        parts.extensions.insert(MockConnectInfo(
            "127.0.0.1:54321"
                .parse::<std::net::SocketAddr>()
                .unwrap_or_else(|err| panic!("failed to parse loopback addr: {err}")),
        ));

        let rejection = RequireAuth::from_request_parts(&mut parts, &state)
            .await
            .err()
            .unwrap_or_else(|| panic!("a loopback peer without credentials must be rejected"));
        assert_eq!(rejection.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_socket_requests_are_trusted_without_credentials() {
        let state = test_state().await;
        let mut parts = request_parts();
        parts.extensions.insert(LocalSocket);

        assert!(
            RequireAuth::from_request_parts(&mut parts, &state)
                .await
                .is_ok(),
            "requests on the local Unix socket should not need credentials"
        );
        assert!(
            LocalPeer::from_request_parts(&mut parts, &state)
                .await
                .is_ok_and(|LocalPeer(local)| local),
            "LocalPeer should report requests carrying the LocalSocket extension"
        );
    }

    #[tokio::test]
    async fn local_socket_header_cannot_be_forged() {
        // Only the Unix-socket router can set the extension; headers are client-controlled.
        let state = test_state().await;
        let mut parts = request_parts();
        parts.headers.insert(
            "x-tond-local-socket",
            axum::http::HeaderValue::from_static("1"),
        );

        assert!(
            RequireAuth::from_request_parts(&mut parts, &state)
                .await
                .is_err()
        );
        assert!(
            LocalPeer::from_request_parts(&mut parts, &state)
                .await
                .is_ok_and(|LocalPeer(local)| !local)
        );
    }

    #[test]
    fn bearer_result_helper_maps_success_invalid_and_internal_error() {
        assert!(
            unauthorized_message_for_bearer_result(Ok(Some(super::super::ApiKey {
                id: "key-1".to_string(),
                name: "root".to_string(),
                key_hash: "hash".to_string(),
                created_at: "2026-04-08T12:00:00Z".to_string(),
                last_used_at: None,
            })))
            .is_ok(),
            "valid bearer authentication results should pass the extractor helper"
        );

        let invalid = unauthorized_message_for_bearer_result(Ok(None))
            .err()
            .unwrap_or_else(|| panic!("missing API keys should produce an unauthorized response"));
        assert_eq!(
            invalid.0,
            StatusCode::UNAUTHORIZED,
            "missing API keys should map to HTTP 401"
        );

        let internal = unauthorized_message_for_bearer_result(Err(sqlx::Error::RowNotFound.into()))
            .err()
            .unwrap_or_else(|| panic!("database errors should produce an internal error response"));
        assert_eq!(
            internal.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "bearer authentication database failures should map to HTTP 500"
        );
    }

    #[test]
    fn session_result_helper_maps_success_invalid_and_internal_error() {
        assert!(
            unauthorized_message_for_session_result(Ok(Some(tond_common::User {
                id: "user-1".to_string(),
                email: "admin@example.com".to_string(),
                password_hash: "hash".to_string(),
                role: "admin".to_string(),
                created_at: "2026-04-08T12:00:00Z".to_string(),
                updated_at: "2026-04-08T12:00:00Z".to_string(),
                totp_secret_enc: None,
                totp_enabled: false,
            })))
            .is_ok(),
            "valid session authentication results should pass the extractor helper"
        );

        let invalid = unauthorized_message_for_session_result(Ok(None))
            .err()
            .unwrap_or_else(|| panic!("invalid sessions should produce an unauthorized response"));
        assert_eq!(
            invalid.0,
            StatusCode::UNAUTHORIZED,
            "missing sessions should map to HTTP 401"
        );

        let internal = unauthorized_message_for_session_result(Err(sqlx::Error::RowNotFound))
            .err()
            .unwrap_or_else(|| panic!("database errors should produce an internal error response"));
        assert_eq!(
            internal.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "session authentication database failures should map to HTTP 500"
        );
    }
}
