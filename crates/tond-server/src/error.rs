// error.rs — Axum error type for tond-server handlers
//
// The orphan rule prevents us from implementing `IntoResponse` for
// `TondError` directly (it is defined in `tond-common`, an external crate).
// Instead we define a local newtype `AppError(TondError)` that:
//
//   1. Implements `IntoResponse` so handlers can return it.
//   2. Implements `From<TondError>` so the `?` operator auto-converts.
//
// The response is the structured error envelope
//   {"success":false,"data":null,"error":{"code","message","hint","field"}}
// with status, code and default hint from tond-common's ERROR_CODES catalog.
// Server-side failures (5xx) are logged here, with their full detail, since
// the client only gets a generic message for database errors.

use axum::{Json, http::StatusCode, response::IntoResponse};
use tond_common::{ApiErrorBody, ApiResponse, TondError};

pub struct AppError(pub TondError);

impl From<TondError> for AppError {
    fn from(e: TondError) -> Self {
        AppError(e)
    }
}

// Allow `?` on sqlx queries directly in handlers (sqlx::Error → TondError::DatabaseError)
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError(TondError::DatabaseError(e))
    }
}

// Allow `?` on internal helpers (e.g. secret decryption) → 500 with the chain.
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError(TondError::Internal(format!("{e:#}")))
    }
}

/// The JSON error response for `error` (also used outside handlers, e.g.
/// by middleware that rejects a request).
pub fn error_response(status: StatusCode, body: ApiErrorBody) -> axum::response::Response {
    (status, Json(ApiResponse::<()>::error(body))).into_response()
}

/// The generic code for an HTTP status, for responses that aren't built
/// from a TondError (auth, webhook and middleware rejections).
pub fn code_for_status(status: StatusCode) -> &'static str {
    use tond_common::code;
    match status.as_u16() {
        400 | 422 => code::INVALID_INPUT,
        401 => code::UNAUTHENTICATED,
        403 => code::FORBIDDEN,
        404 => code::NOT_FOUND,
        405 => code::METHOD_NOT_ALLOWED,
        409 => code::CONFLICT,
        413 => code::PAYLOAD_TOO_LARGE,
        429 => code::RATE_LIMITED,
        503 => code::SERVICE_UNAVAILABLE,
        400..=499 => code::INVALID_INPUT,
        _ => code::INTERNAL,
    }
}

/// A structured error response with the generic code for `status`.
pub fn status_error(status: StatusCode, message: impl Into<String>) -> axum::response::Response {
    error_response(status, ApiErrorBody::new(code_for_status(status), message))
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status =
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_server_error() {
            tracing::warn!(code = self.0.code(), "Request failed: {}", self.0);
        }
        error_response(status, ApiErrorBody::from(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn body_of(e: TondError) -> (StatusCode, serde_json::Value) {
        let resp = AppError(e).into_response();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap_or_default();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    async fn app_errors_render_the_structured_envelope() {
        let (status, body) = body_of(TondError::AppNotFound("web".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["success"], false);
        assert_eq!(body["error"]["code"], "APP_NOT_FOUND");
        assert_eq!(body["error"]["message"], "App not found: web");
        assert!(body["error"]["hint"].is_string());

        let (status, body) =
            body_of(TondError::invalid_field("port", "Port must be 1-65535")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "INVALID_INPUT");
        assert_eq!(body["error"]["field"], "port");

        let (status, body) = body_of(TondError::DatabaseError(sqlx::Error::PoolTimedOut)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "DATABASE_ERROR");
        assert!(
            !body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("pool"),
            "SQL details stay in the log"
        );
    }
}
