// errors.rs — unified error type for the workspace
//
// TondError is the canonical error enum shared across tond-server and
// tond-common. Every logical failure mode has its own variant so callers
// can pattern-match on the exact cause.
//
// status_code() maps each variant to the HTTP status code that handlers
// should return to the client via AppError (the thin newtype wrapper that
// implements Axum's IntoResponse to satisfy the orphan rule).

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TondError {
    #[error("App not found: {0}")]
    AppNotFound(String),

    #[error("App already exists: {0}")]
    AppAlreadyExists(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Docker error: {0}")]
    DockerError(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// The caller is authenticated but lacks the role this action needs.
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// The caller is authenticated but its role or token lacks a scope (or
    /// the resource is outside the token's filter). Carries a stable error
    /// code for clients: `SCOPE_DENIED` or `RESOURCE_DENIED`.
    #[error("{message}")]
    ScopeDenied { code: &'static str, message: String },

    #[error("Internal error: {0}")]
    Internal(String),
}

impl TondError {
    pub fn status_code(&self) -> u16 {
        match self {
            TondError::AppNotFound(_) => 404,
            TondError::NotFound(_) => 404,
            TondError::AppAlreadyExists(_) => 409,
            TondError::InvalidInput(_) => 400,
            TondError::Forbidden(_) => 403,
            TondError::ScopeDenied { .. } => 403,
            _ => 500,
        }
    }

    /// Stable machine-readable error code, when the variant has one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            TondError::ScopeDenied { code, .. } => Some(code),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_code_maps_variants_to_expected_http_codes() {
        assert_eq!(
            TondError::AppNotFound("demo".to_string()).status_code(),
            404,
            "AppNotFound should map to HTTP 404"
        );
        assert_eq!(
            TondError::NotFound("missing".to_string()).status_code(),
            404,
            "NotFound should map to HTTP 404"
        );
        assert_eq!(
            TondError::AppAlreadyExists("demo".to_string()).status_code(),
            409,
            "AppAlreadyExists should map to HTTP 409"
        );
        assert_eq!(
            TondError::InvalidInput("bad".to_string()).status_code(),
            400,
            "InvalidInput should map to HTTP 400"
        );
        assert_eq!(
            TondError::Forbidden("admins only".to_string()).status_code(),
            403,
            "Forbidden should map to HTTP 403"
        );
        assert_eq!(
            TondError::DockerError("boom".to_string()).status_code(),
            500,
            "DockerError should map to HTTP 500"
        );
        assert_eq!(
            TondError::Internal("boom".to_string()).status_code(),
            500,
            "Internal should map to HTTP 500"
        );
    }

    #[test]
    fn display_messages_are_human_readable() {
        assert_eq!(
            TondError::AppNotFound("api".to_string()).to_string(),
            "App not found: api",
            "AppNotFound display output should include the app name"
        );
        assert_eq!(
            TondError::AppAlreadyExists("api".to_string()).to_string(),
            "App already exists: api",
            "AppAlreadyExists display output should include the conflicting app name"
        );
        assert_eq!(
            TondError::InvalidInput("port must be positive".to_string()).to_string(),
            "Invalid input: port must be positive",
            "InvalidInput display output should preserve the validation message"
        );
    }

    #[test]
    fn sqlx_errors_convert_into_database_error_variant() {
        let error = TondError::from(sqlx::Error::RowNotFound);

        match error {
            TondError::DatabaseError(sqlx::Error::RowNotFound) => {}
            other => panic!("expected DatabaseError(RowNotFound), got {other:?}"),
        }
    }
}
