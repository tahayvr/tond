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
//   1. Loopback peer address (CLI on the same machine)
//      If the TCP connection originates from 127.0.0.1 or ::1, the
//      request is unconditionally trusted. The server only listens on
//      127.0.0.1:3001, so this address can never arrive from outside the
//      machine — SSH access is the implicit credential.
//
//   2. Bearer token (API key / programmatic access)
//      Reads the Authorization header. If it starts with "Bearer ",
//      extracts the raw key and calls auth::validate_key() which hashes
//      it and checks the api_keys table.
//
//   3. Session cookie (dashboard browser)
//      Reads the Cookie header for a `tond_session` value and calls
//      handlers::auth::validate_session() which joins sessions + users
//      and checks expiry.
//
// If no method succeeds, returns 401.

use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, FromRequestParts},
    http::{StatusCode, header, request::Parts},
};
use tond_common::ApiResponse;

use crate::handlers::auth::{extract_session_token, validate_session};
use crate::state::AppState;

/// Extractor that validates authentication via:
/// 1. Loopback peer IP (127.0.0.1 / ::1) — CLI on the same machine, no credentials needed
/// 2. `Authorization: Bearer <api-key>` header
/// 3. `tond_session` cookie (dashboard browser sessions)
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
        // Try 1: loopback — trust any connection from 127.0.0.1 or ::1.
        // The server only binds to 127.0.0.1:3001, so this can never be
        // spoofed from outside the machine.
        if let Ok(ConnectInfo(addr)) =
            ConnectInfo::<SocketAddr>::from_request_parts(parts, state).await
            && addr.ip().is_loopback()
        {
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
    use super::*;

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
