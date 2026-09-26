// errors.rs — unified error type and the stable error-code catalog
//
// TondError is the canonical error enum shared across tond-server and
// tond-common. Every logical failure mode has its own variant so callers
// can pattern-match on the exact cause.
//
// Every variant maps to:
//   status_code() — the HTTP status handlers return (via AppError, the thin
//                   newtype in tond-server that implements IntoResponse)
//   code()        — a stable SCREAMING_SNAKE code from ERROR_CODES
//   message()     — the human sentence sent to clients
//   hint()        — optional "what to do next"
//
// The catalog (ERROR_CODES) is the contract shared by the API
// (`{"success":false,"error":{"code","message","hint"}}`), the CLI's exit
// codes (`tond help exit-codes`) and MCP errors. Codes never change meaning
// once released; add new ones instead.

use thiserror::Error;

/// Stable error codes. Clients switch on these, never on messages.
pub mod code {
    pub const INVALID_INPUT: &str = "INVALID_INPUT";
    pub const INVALID_JSON: &str = "INVALID_JSON";
    pub const METHOD_NOT_ALLOWED: &str = "METHOD_NOT_ALLOWED";
    pub const PAYLOAD_TOO_LARGE: &str = "PAYLOAD_TOO_LARGE";
    pub const UNAUTHENTICATED: &str = "UNAUTHENTICATED";
    pub const SESSION_EXPIRED: &str = "SESSION_EXPIRED";
    pub const INVALID_CREDENTIALS: &str = "INVALID_CREDENTIALS";
    pub const TOKEN_INVALID: &str = "TOKEN_INVALID";
    pub const TOKEN_REVOKED: &str = "TOKEN_REVOKED";
    pub const TOKEN_EXPIRED: &str = "TOKEN_EXPIRED";
    pub const ACCOUNT_DISABLED: &str = "ACCOUNT_DISABLED";
    pub const IP_NOT_ALLOWED: &str = "IP_NOT_ALLOWED";
    pub const CSRF_REJECTED: &str = "CSRF_REJECTED";
    pub const FORBIDDEN: &str = "FORBIDDEN";
    pub const SCOPE_DENIED: &str = "SCOPE_DENIED";
    pub const RESOURCE_DENIED: &str = "RESOURCE_DENIED";
    pub const APPROVAL_REQUIRED: &str = "APPROVAL_REQUIRED";
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const APP_NOT_FOUND: &str = "APP_NOT_FOUND";
    pub const ROUTE_NOT_FOUND: &str = "ROUTE_NOT_FOUND";
    pub const CONFLICT: &str = "CONFLICT";
    pub const APP_ALREADY_EXISTS: &str = "APP_ALREADY_EXISTS";
    pub const DEPLOY_IN_PROGRESS: &str = "DEPLOY_IN_PROGRESS";
    pub const INSUFFICIENT_MEMORY: &str = "INSUFFICIENT_MEMORY";
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    pub const IDEMPOTENCY_KEY_IN_USE: &str = "IDEMPOTENCY_KEY_IN_USE";
    pub const BUILD_FAILED: &str = "BUILD_FAILED";
    pub const DEPLOY_FAILED: &str = "DEPLOY_FAILED";
    pub const RATE_LIMITED: &str = "RATE_LIMITED";
    pub const DOCKER_ERROR: &str = "DOCKER_ERROR";
    pub const DATABASE_ERROR: &str = "DATABASE_ERROR";
    pub const INTERNAL: &str = "INTERNAL";
    pub const SERVICE_UNAVAILABLE: &str = "SERVICE_UNAVAILABLE";
}

/// CLI exit codes (`tond help exit-codes`). Each error code maps to one.
pub mod exit {
    pub const OK: u8 = 0;
    /// Anything not covered below (server errors, unexpected failures).
    pub const GENERIC: u8 = 1;
    /// Bad arguments or input rejected by the server.
    pub const USAGE: u8 = 2;
    /// The app, database, deployment… does not exist.
    pub const NOT_FOUND: u8 = 3;
    /// Not signed in, token invalid, or missing a scope.
    pub const DENIED: u8 = 4;
    /// Conflicts with the current state (exists already, deploy running, key reused).
    pub const CONFLICT: u8 = 5;
    /// The build or deploy ran and failed.
    pub const FAILED: u8 = 6;
    /// A human must approve this action first.
    pub const APPROVAL_REQUIRED: u8 = 7;
    /// The server could not be reached, is unavailable, or asked us to back off.
    pub const UNAVAILABLE: u8 = 8;

    /// (code, name, meaning) for help output and docs, in numeric order.
    pub const TABLE: &[(u8, &str, &str)] = &[
        (OK, "ok", "Success."),
        (
            GENERIC,
            "error",
            "Unexpected failure (server error, Docker error, anything not listed).",
        ),
        (
            USAGE,
            "usage",
            "Invalid arguments or input (bad flags, a value the server rejected, missing --yes without a TTY).",
        ),
        (
            NOT_FOUND,
            "not_found",
            "The app, database, deployment or other resource does not exist.",
        ),
        (
            DENIED,
            "denied",
            "Not authenticated, the token is invalid/expired, or it lacks the required scope.",
        ),
        (
            CONFLICT,
            "conflict",
            "Conflicts with the current state (already exists, deploy in progress, idempotency key reused).",
        ),
        (
            FAILED,
            "failed",
            "The build or deploy ran and failed (`deploy --wait`).",
        ),
        (
            APPROVAL_REQUIRED,
            "approval_required",
            "The action waits for a human approval.",
        ),
        (
            UNAVAILABLE,
            "unavailable",
            "The server could not be reached, is restarting, or rate-limited the request.",
        ),
    ];
}

/// One entry of the error-code catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCodeInfo {
    pub code: &'static str,
    /// HTTP status the API answers with.
    pub status: u16,
    /// CLI exit code (see [`exit`]).
    pub exit: u8,
    pub description: &'static str,
    /// Default hint when the error carries none of its own.
    pub hint: Option<&'static str>,
}

const fn info(
    code: &'static str,
    status: u16,
    exit: u8,
    description: &'static str,
    hint: Option<&'static str>,
) -> ErrorCodeInfo {
    ErrorCodeInfo {
        code,
        status,
        exit,
        description,
        hint,
    }
}

/// Every error code the API can return. Documented in the OpenAPI spec and
/// `tond help exit-codes`; a test keeps TondError, the catalog and the
/// dashboard in sync.
pub const ERROR_CODES: &[ErrorCodeInfo] = &[
    info(
        code::INVALID_INPUT,
        400,
        exit::USAGE,
        "A value in the request is invalid; `field` names it when known.",
        None,
    ),
    info(
        code::INVALID_JSON,
        400,
        exit::USAGE,
        "The request body is not valid JSON for this endpoint.",
        Some("Check the request body against the OpenAPI spec at /api/v1/openapi.json."),
    ),
    info(
        code::METHOD_NOT_ALLOWED,
        405,
        exit::USAGE,
        "The path exists but not with this HTTP method.",
        None,
    ),
    info(
        code::PAYLOAD_TOO_LARGE,
        413,
        exit::USAGE,
        "The request body exceeds a configured limit.",
        None,
    ),
    info(
        code::UNAUTHENTICATED,
        401,
        exit::DENIED,
        "No credentials were sent.",
        Some("Send Authorization: Bearer <token>, or sign in to the dashboard."),
    ),
    info(
        code::SESSION_EXPIRED,
        401,
        exit::DENIED,
        "The dashboard session ended.",
        Some("Sign in again."),
    ),
    info(
        code::INVALID_CREDENTIALS,
        401,
        exit::DENIED,
        "A password, one-time code, setup token or link was wrong.",
        None,
    ),
    info(
        code::TOKEN_INVALID,
        401,
        exit::DENIED,
        "The API token is unknown or malformed.",
        Some("Check that you copied the whole token (it starts with tond_pat_)."),
    ),
    info(
        code::TOKEN_REVOKED,
        401,
        exit::DENIED,
        "The API token was revoked.",
        Some("Create a new token with `tond token create` or in Settings → Tokens."),
    ),
    info(
        code::TOKEN_EXPIRED,
        401,
        exit::DENIED,
        "The API token expired.",
        Some("Create a new token with `tond token create` or in Settings → Tokens."),
    ),
    info(
        code::ACCOUNT_DISABLED,
        401,
        exit::DENIED,
        "The account (or the token's owner) is disabled.",
        Some("Ask an admin of this server to re-enable the account."),
    ),
    info(
        code::IP_NOT_ALLOWED,
        403,
        exit::DENIED,
        "The API token can't be used from this address.",
        Some("Use the token from an allowed address, or edit its IP allowlist."),
    ),
    info(
        code::CSRF_REJECTED,
        403,
        exit::DENIED,
        "A browser request failed the same-origin checks.",
        Some("Send the X-Tond-CSRF header from the dashboard's own origin, or use an API token."),
    ),
    info(
        code::FORBIDDEN,
        403,
        exit::DENIED,
        "The caller's role can't do this.",
        None,
    ),
    info(
        code::SCOPE_DENIED,
        403,
        exit::DENIED,
        "The role or token lacks the scope this operation needs (see x-tond-scopes).",
        Some("Use a token that includes the scope, or ask an admin for a role that has it."),
    ),
    info(
        code::RESOURCE_DENIED,
        403,
        exit::DENIED,
        "The token is limited to other apps or projects.",
        Some("Use a token whose app/project filter includes this resource."),
    ),
    info(
        code::APPROVAL_REQUIRED,
        403,
        exit::APPROVAL_REQUIRED,
        "The action needs a human approval before it runs.",
        Some("Approve the request in the dashboard, then retry."),
    ),
    info(
        code::NOT_FOUND,
        404,
        exit::NOT_FOUND,
        "The resource does not exist.",
        None,
    ),
    info(
        code::APP_NOT_FOUND,
        404,
        exit::NOT_FOUND,
        "No app has this id or name.",
        Some("List apps with `tond apps list` (or GET /api/v1/apps)."),
    ),
    info(
        code::ROUTE_NOT_FOUND,
        404,
        exit::NOT_FOUND,
        "No API endpoint matches this path.",
        Some("See /api/v1/openapi.json for the list of endpoints."),
    ),
    info(
        code::CONFLICT,
        409,
        exit::CONFLICT,
        "The request conflicts with the resource's current state.",
        None,
    ),
    info(
        code::APP_ALREADY_EXISTS,
        409,
        exit::CONFLICT,
        "An app with this name already exists.",
        Some("Pick another name, or deploy the existing app."),
    ),
    info(
        code::DEPLOY_IN_PROGRESS,
        409,
        exit::CONFLICT,
        "A deploy of this app is running; the action has to wait for it.",
        Some("Wait for the deploy to finish (`tond apps deploy <app> --wait`), then retry."),
    ),
    info(
        code::INSUFFICIENT_MEMORY,
        409,
        exit::CONFLICT,
        "The server has less free memory than the template needs.",
        Some(
            "Stop or remove an app to free memory, or retry with allow_low_memory: true (`--allow-low-memory`).",
        ),
    ),
    info(
        code::IDEMPOTENCY_KEY_REUSED,
        422,
        exit::CONFLICT,
        "The Idempotency-Key was already used with a different request.",
        Some("Use a new Idempotency-Key for a different request."),
    ),
    info(
        code::IDEMPOTENCY_KEY_IN_USE,
        409,
        exit::CONFLICT,
        "A request with this Idempotency-Key is still being processed.",
        Some("Retry shortly with the same key to get its result."),
    ),
    info(
        code::BUILD_FAILED,
        422,
        exit::FAILED,
        "The image build failed.",
        Some(
            "Read the build log (`tond apps deployments <app>`), fix the source and deploy again.",
        ),
    ),
    info(
        code::DEPLOY_FAILED,
        422,
        exit::FAILED,
        "The deploy failed after the build (container start, health check or cutover).",
        Some("Read the deploy log and the app's logs, then deploy again."),
    ),
    info(
        code::RATE_LIMITED,
        429,
        exit::UNAVAILABLE,
        "Too many requests; see the Retry-After header.",
        Some("Wait for the Retry-After seconds, then retry."),
    ),
    info(
        code::DOCKER_ERROR,
        500,
        exit::GENERIC,
        "Docker failed to carry out the operation.",
        Some("Check that Docker is running on the server (`systemctl status docker`)."),
    ),
    info(
        code::DATABASE_ERROR,
        500,
        exit::GENERIC,
        "Tond's own database failed.",
        Some("Check tond-server's logs (`journalctl -u tond-server`)."),
    ),
    info(
        code::INTERNAL,
        500,
        exit::GENERIC,
        "An unexpected server error.",
        Some("Check tond-server's logs (`journalctl -u tond-server`)."),
    ),
    info(
        code::SERVICE_UNAVAILABLE,
        503,
        exit::UNAVAILABLE,
        "The server is starting, shutting down or temporarily unable to serve.",
        Some("Retry in a moment."),
    ),
];

/// Catalog entry for `code`, if it is a known code.
pub fn error_code_info(code: &str) -> Option<&'static ErrorCodeInfo> {
    ERROR_CODES.iter().find(|c| c.code == code)
}

/// CLI exit code for an API error: by code when known, else by HTTP status.
pub fn exit_code_for(code: Option<&str>, status: u16) -> u8 {
    if let Some(info) = code.and_then(error_code_info) {
        return info.exit;
    }
    match status {
        400 | 405 | 413 | 415 | 422 => exit::USAGE,
        401 | 403 => exit::DENIED,
        404 => exit::NOT_FOUND,
        409 => exit::CONFLICT,
        429 | 502 | 503 | 504 => exit::UNAVAILABLE,
        _ => exit::GENERIC,
    }
}

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

    /// A validation failure tied to one request field.
    #[error("Invalid input: {message}")]
    InvalidField { field: String, message: String },

    /// The caller is authenticated but lacks the role this action needs.
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// The caller is authenticated but its role or token lacks a scope (or
    /// the resource is outside the token's filter). Carries a stable error
    /// code for clients: `SCOPE_DENIED` or `RESOURCE_DENIED`.
    #[error("{message}")]
    ScopeDenied { code: &'static str, message: String },

    /// The action needs a human approval first.
    #[error("{0}")]
    ApprovalRequired(String),

    /// The request conflicts with the resource's current state.
    #[error("{0}")]
    Conflict(String),

    /// A deploy of the app is running and the action can't overlap it.
    #[error("{0}")]
    DeployInProgress(String),

    /// A template needs more memory than the server has free (retry with
    /// `allow_low_memory` to deploy anyway).
    #[error("{0}")]
    InsufficientMemory(String),

    /// The image build failed.
    #[error("{0}")]
    BuildFailed(String),

    /// The server can't serve the request right now (starting, stopping, a
    /// dependency down).
    #[error("{0}")]
    Unavailable(String),

    #[error("Internal error: {0}")]
    Internal(String),

    /// The request body (e.g. an uploaded archive) exceeds a configured limit.
    #[error("Too large: {0}")]
    PayloadTooLarge(String),

    /// Any error with a specific hint for the client.
    #[error("{error}")]
    Hinted { error: Box<TondError>, hint: String },
}

impl TondError {
    /// Attach a "what to do next" hint shown to the client.
    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        TondError::Hinted {
            error: Box::new(self),
            hint: hint.into(),
        }
    }

    /// Validation failure for `field`.
    pub fn invalid_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        TondError::InvalidField {
            field: field.into(),
            message: message.into(),
        }
    }

    pub fn status_code(&self) -> u16 {
        error_code_info(self.code()).map_or(500, |c| c.status)
    }

    /// Stable machine-readable error code (always from ERROR_CODES).
    pub fn code(&self) -> &'static str {
        match self {
            TondError::AppNotFound(_) => code::APP_NOT_FOUND,
            TondError::AppAlreadyExists(_) => code::APP_ALREADY_EXISTS,
            TondError::NotFound(_) => code::NOT_FOUND,
            TondError::DockerError(_) => code::DOCKER_ERROR,
            TondError::DatabaseError(_) => code::DATABASE_ERROR,
            TondError::InvalidInput(_) | TondError::InvalidField { .. } => code::INVALID_INPUT,
            TondError::Forbidden(_) => code::FORBIDDEN,
            TondError::ScopeDenied { code, .. } => code,
            TondError::ApprovalRequired(_) => code::APPROVAL_REQUIRED,
            TondError::Conflict(_) => code::CONFLICT,
            TondError::DeployInProgress(_) => code::DEPLOY_IN_PROGRESS,
            TondError::InsufficientMemory(_) => code::INSUFFICIENT_MEMORY,
            TondError::BuildFailed(_) => code::BUILD_FAILED,
            TondError::Unavailable(_) => code::SERVICE_UNAVAILABLE,
            TondError::Internal(_) => code::INTERNAL,
            TondError::PayloadTooLarge(_) => code::PAYLOAD_TOO_LARGE,
            TondError::Hinted { error, .. } => error.code(),
        }
    }

    /// The human message for clients, without the variant prefix the
    /// Display impl adds for logs ("Invalid input: …").
    pub fn message(&self) -> String {
        match self {
            TondError::AppNotFound(name) => format!("App not found: {name}"),
            TondError::AppAlreadyExists(name) => format!("App already exists: {name}"),
            TondError::NotFound(m)
            | TondError::InvalidInput(m)
            | TondError::Forbidden(m)
            | TondError::ApprovalRequired(m)
            | TondError::Conflict(m)
            | TondError::DeployInProgress(m)
            | TondError::InsufficientMemory(m)
            | TondError::BuildFailed(m)
            | TondError::Unavailable(m)
            | TondError::PayloadTooLarge(m) => m.clone(),
            TondError::InvalidField { message, .. } => message.clone(),
            TondError::ScopeDenied { message, .. } => message.clone(),
            TondError::DockerError(m) => format!("Docker error: {m}"),
            // SQL error text can name tables and constraints; the detail goes
            // to the server log (AppError logs it), not to the client.
            TondError::DatabaseError(_) => "Tond's database failed to complete the request.".into(),
            TondError::Internal(m) => format!("Internal error: {m}"),
            TondError::Hinted { error, .. } => error.message(),
        }
    }

    /// The hint for clients: the attached one, else the code's default.
    pub fn hint(&self) -> Option<String> {
        match self {
            TondError::Hinted { hint, .. } => Some(hint.clone()),
            other => error_code_info(other.code()).and_then(|c| c.hint.map(str::to_string)),
        }
    }

    /// The request field the error is about, when known.
    pub fn field(&self) -> Option<&str> {
        match self {
            TondError::InvalidField { field, .. } => Some(field),
            TondError::Hinted { error, .. } => error.field(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn status_code_maps_variants_to_expected_http_codes() {
        let cases = [
            (TondError::AppNotFound("demo".into()), 404),
            (TondError::NotFound("missing".into()), 404),
            (TondError::AppAlreadyExists("demo".into()), 409),
            (TondError::InvalidInput("bad".into()), 400),
            (TondError::invalid_field("port", "bad"), 400),
            (TondError::Forbidden("admins only".into()), 403),
            (TondError::DockerError("boom".into()), 500),
            (TondError::Internal("boom".into()), 500),
            (TondError::PayloadTooLarge("big".into()), 413),
            (TondError::Conflict("busy".into()), 409),
            (TondError::DeployInProgress("busy".into()), 409),
            (TondError::ApprovalRequired("wait".into()), 403),
            (TondError::BuildFailed("exit 1".into()), 422),
            (TondError::Unavailable("stopping".into()), 503),
            (TondError::NotFound("x".into()).with_hint("try y"), 404),
        ];
        for (error, status) in cases {
            assert_eq!(error.status_code(), status, "{error:?}");
        }
    }

    #[test]
    fn every_variant_code_is_in_the_catalog() {
        let variants = [
            TondError::AppNotFound(String::new()),
            TondError::AppAlreadyExists(String::new()),
            TondError::NotFound(String::new()),
            TondError::DockerError(String::new()),
            TondError::DatabaseError(sqlx::Error::RowNotFound),
            TondError::InvalidInput(String::new()),
            TondError::invalid_field("f", ""),
            TondError::Forbidden(String::new()),
            TondError::ScopeDenied {
                code: code::SCOPE_DENIED,
                message: String::new(),
            },
            TondError::ScopeDenied {
                code: code::RESOURCE_DENIED,
                message: String::new(),
            },
            TondError::ApprovalRequired(String::new()),
            TondError::Conflict(String::new()),
            TondError::DeployInProgress(String::new()),
            TondError::InsufficientMemory(String::new()),
            TondError::BuildFailed(String::new()),
            TondError::Unavailable(String::new()),
            TondError::Internal(String::new()),
            TondError::PayloadTooLarge(String::new()),
        ];
        for v in &variants {
            assert!(
                error_code_info(v.code()).is_some(),
                "{} is missing from ERROR_CODES",
                v.code()
            );
        }
    }

    #[test]
    fn catalog_codes_are_unique_screaming_snake() {
        let mut seen = HashSet::new();
        for c in ERROR_CODES {
            assert!(seen.insert(c.code), "duplicate code {}", c.code);
            assert!(
                c.code
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit()),
                "{} is not SCREAMING_SNAKE",
                c.code
            );
            assert!(
                exit::TABLE.iter().any(|(n, _, _)| *n == c.exit),
                "{} maps to undocumented exit code {}",
                c.code,
                c.exit
            );
        }
    }

    #[test]
    fn roadmap_codes_map_to_their_exit_codes() {
        assert_eq!(exit_code_for(Some("APP_NOT_FOUND"), 404), exit::NOT_FOUND);
        assert_eq!(
            exit_code_for(Some("DEPLOY_IN_PROGRESS"), 409),
            exit::CONFLICT
        );
        assert_eq!(exit_code_for(Some("SCOPE_DENIED"), 403), exit::DENIED);
        assert_eq!(
            exit_code_for(Some("APPROVAL_REQUIRED"), 403),
            exit::APPROVAL_REQUIRED
        );
        assert_eq!(exit_code_for(Some("BUILD_FAILED"), 422), exit::FAILED);
        // Unknown codes fall back to the status.
        assert_eq!(exit_code_for(Some("SOMETHING_NEW"), 404), exit::NOT_FOUND);
        assert_eq!(exit_code_for(None, 503), exit::UNAVAILABLE);
        assert_eq!(exit_code_for(None, 500), exit::GENERIC);
    }

    #[test]
    fn messages_and_hints() {
        assert_eq!(
            TondError::InvalidInput("port must be positive".into()).message(),
            "port must be positive"
        );
        assert_eq!(
            TondError::InvalidInput("port must be positive".into()).to_string(),
            "Invalid input: port must be positive",
        );
        assert_eq!(
            TondError::AppNotFound("api".into()).message(),
            "App not found: api"
        );
        let e = TondError::invalid_field("name", "Name is required").with_hint("Pick a name");
        assert_eq!(e.field(), Some("name"));
        assert_eq!(e.hint().as_deref(), Some("Pick a name"));
        assert_eq!(e.code(), code::INVALID_INPUT);
        assert!(TondError::AppNotFound("x".into()).hint().is_some());
        assert!(
            !TondError::DatabaseError(sqlx::Error::RowNotFound)
                .message()
                .contains("RowNotFound"),
            "SQL details stay out of client messages"
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
