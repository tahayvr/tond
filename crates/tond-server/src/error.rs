// error.rs — Axum error type for tond-server handlers
//
// The orphan rule prevents us from implementing `IntoResponse` for
// `TondError` directly (it is defined in `tond-common`, an external crate).
// Instead we define a local newtype `AppError(TondError)` that:
//
//   1. Implements `IntoResponse` so handlers can return it.
//   2. Implements `From<TondError>` so the `?` operator auto-converts.
//
// Status code mapping (mirrors TondError::status_code()):
//   AppNotFound      → 404 Not Found
//   AppAlreadyExists → 409 Conflict
//   InvalidInput     → 400 Bad Request
//   DatabaseError    → 500 Internal Server Error
//   DockerError      → 500 Internal Server Error
//   Internal         → 500 Internal Server Error

use axum::{Json, http::StatusCode, response::IntoResponse};
use tond_common::{ApiResponse, TondError};

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

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status =
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(ApiResponse::<()>::err(self.0.to_string()));
        (status, body).into_response()
    }
}
