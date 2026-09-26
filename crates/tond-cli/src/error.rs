// error.rs — typed CLI errors and the exit code each one maps to
//
// Every failure the CLI reports is (or wraps) a `CliError`:
//   - API errors: the server's structured `error` object (code, message, hint,
//     field) plus the HTTP status; the exit code comes from the shared catalog
//     (`tond_common::exit_code_for`).
//   - Replies that are not a Tond envelope (a proxy's HTML 502): the status is
//     reported and decides the exit code; `code` stays empty.
//   - Local errors the CLI raises itself, with the CLI-only codes in
//     `local_code` (server unreachable, usage refusals, deploy outcomes).
//
// Anything else (an I/O error, a bad template file) exits 1 without a code.
// `report` prints the error on stderr: `Error: …` / `Hint: …` for humans, or
// one JSON line `{"error":{…},"exit_code":N}` with `--json`.

use std::fmt;

use serde_json::{Value, json};
use tond_common::{ApiErrorBody, exit, exit_code_for};

/// Error codes the CLI produces itself (never sent by the server).
/// Listed in `tond help exit-codes`.
pub mod local_code {
    /// Bad arguments, or a destructive command without --yes and no terminal.
    pub const USAGE: &str = "USAGE";
    /// The confirmation prompt was answered with no.
    pub const ABORTED: &str = "ABORTED";
    /// No connection to tond-server (socket missing, connection refused, …).
    pub const SERVER_UNREACHABLE: &str = "SERVER_UNREACHABLE";
    /// The local socket exists but is root-only.
    pub const SOCKET_PERMISSION_DENIED: &str = "SOCKET_PERMISSION_DENIED";
    /// The server's reply could not be understood.
    pub const INVALID_RESPONSE: &str = "INVALID_RESPONSE";
    /// A newer deploy replaced the one being followed.
    pub const DEPLOY_SUPERSEDED: &str = "DEPLOY_SUPERSEDED";
    /// The deployment being followed was cancelled.
    pub const DEPLOY_CANCELLED: &str = "DEPLOY_CANCELLED";
    /// The deployment stream ended before the outcome was known.
    pub const DEPLOY_OUTCOME_UNKNOWN: &str = "DEPLOY_OUTCOME_UNKNOWN";

    /// (code, exit code, description) for help output.
    pub const TABLE: &[(&str, u8, &str)] = &[
        (
            USAGE,
            super::exit::USAGE,
            "Bad arguments, or a destructive command run without --yes while stdin is not a terminal.",
        ),
        (
            ABORTED,
            super::exit::GENERIC,
            "The confirmation prompt was answered with no.",
        ),
        (
            SERVER_UNREACHABLE,
            super::exit::UNAVAILABLE,
            "tond-server could not be reached (socket missing, connection refused).",
        ),
        (
            SOCKET_PERMISSION_DENIED,
            super::exit::DENIED,
            "The local socket is root-only; run as root or use --server/--token.",
        ),
        (
            INVALID_RESPONSE,
            super::exit::GENERIC,
            "The server's reply could not be understood.",
        ),
        (
            DEPLOY_SUPERSEDED,
            super::exit::CONFLICT,
            "deploy --wait: a newer deploy replaced this one.",
        ),
        (
            DEPLOY_CANCELLED,
            super::exit::CONFLICT,
            "deploy --wait: the deployment was cancelled.",
        ),
        (
            DEPLOY_OUTCOME_UNKNOWN,
            super::exit::GENERIC,
            "deploy --wait: the stream ended before the deployment finished.",
        ),
    ];
}

/// A failure with a stable code, a hint and the exit code it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    /// HTTP status, for errors that came from a response.
    pub status: Option<u16>,
    /// Stable error code (API catalog or `local_code`); `None` when the server
    /// sent none (e.g. a proxy error page).
    pub code: Option<String>,
    pub message: String,
    pub hint: Option<String>,
    pub field: Option<String>,
    /// Process exit code.
    pub exit: u8,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl CliError {
    fn local(code: &str, exit: u8, message: impl Into<String>) -> Self {
        Self {
            status: None,
            code: Some(code.to_string()),
            message: message.into(),
            hint: None,
            field: None,
            exit,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The server's structured error.
    pub fn api(status: u16, body: ApiErrorBody) -> Self {
        let exit = exit_code_for(Some(&body.code), status);
        Self {
            status: Some(status),
            code: Some(body.code),
            message: body.message,
            hint: body.hint,
            field: body.field,
            exit,
        }
    }

    /// A reply without a structured error (non-JSON body, legacy string
    /// error): the status decides the exit code.
    pub fn status_only(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: Some(status),
            code: None,
            message: message.into(),
            hint: None,
            field: None,
            exit: exit_code_for(None, status),
        }
    }

    /// A WebSocket close frame the server sends to refuse a stream: codes
    /// 4000 + HTTP status (4004 not found, 4403 forbidden, …).
    pub fn from_close(code: u16, reason: &str) -> Option<Self> {
        (4000..5000)
            .contains(&code)
            .then(|| Self::status_only(code - 4000, reason))
    }

    /// A local refusal: bad arguments, missing --yes.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::local(local_code::USAGE, exit::USAGE, message)
    }

    pub fn aborted() -> Self {
        Self::local(local_code::ABORTED, exit::GENERIC, "Cancelled.")
    }

    /// tond-server could not be reached.
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::local(local_code::SERVER_UNREACHABLE, exit::UNAVAILABLE, message)
    }

    pub fn socket_permission_denied(message: impl Into<String>) -> Self {
        Self::local(local_code::SOCKET_PERMISSION_DENIED, exit::DENIED, message)
    }

    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::local(local_code::INVALID_RESPONSE, exit::GENERIC, message)
    }

    /// Any code, local or from the catalog (`BUILD_FAILED`, …), with its exit.
    pub fn coded(code: &str, exit: u8, message: impl Into<String>) -> Self {
        Self::local(code, exit, message)
    }

    /// The `error` object of the JSON error line.
    pub fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "hint": self.hint,
            "field": self.field,
        })
    }
}

/// The `CliError` inside `err`, if any (searching through `context` layers).
pub fn find(err: &anyhow::Error) -> Option<&CliError> {
    err.chain().find_map(|e| e.downcast_ref::<CliError>())
}

/// Exit code for any error the CLI returns.
pub fn exit_code(err: &anyhow::Error) -> u8 {
    find(err).map(|e| e.exit).unwrap_or(exit::GENERIC)
}

/// The error as a `CliError`: the typed one (with any context added in front
/// of its message), or a code-less generic error.
pub fn describe(err: &anyhow::Error) -> CliError {
    match find(err) {
        Some(e) => {
            let mut e = e.clone();
            // Context added on top ("Failed to …: <message>") stays visible.
            let full = format!("{err:#}");
            if !full.is_empty() && full != e.message {
                e.message = full;
            }
            e
        }
        None => CliError {
            status: None,
            code: None,
            message: format!("{err:#}"),
            hint: None,
            field: None,
            exit: exit::GENERIC,
        },
    }
}

/// The text written to stderr for `err`.
pub fn render(err: &anyhow::Error, json: bool) -> String {
    let e = describe(err);
    if json {
        json!({ "error": e.to_json(), "exit_code": e.exit }).to_string()
    } else {
        match &e.hint {
            Some(hint) => format!("Error: {}\nHint: {hint}", e.message),
            None => format!("Error: {}", e.message),
        }
    }
}

/// Print `err` on stderr and return the process exit code.
pub fn report(err: &anyhow::Error, json: bool) -> u8 {
    eprintln!("{}", render(err, json));
    exit_code(err)
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    fn api(status: u16, code: &str) -> anyhow::Error {
        CliError::api(status, ApiErrorBody::new(code, "boom")).into()
    }

    #[test]
    fn exit_codes_follow_the_catalog() {
        assert_eq!(exit_code(&api(404, "APP_NOT_FOUND")), exit::NOT_FOUND);
        assert_eq!(exit_code(&api(403, "SCOPE_DENIED")), exit::DENIED);
        assert_eq!(exit_code(&api(409, "DEPLOY_IN_PROGRESS")), exit::CONFLICT);
        assert_eq!(
            exit_code(&api(403, "APPROVAL_REQUIRED")),
            exit::APPROVAL_REQUIRED
        );
        assert_eq!(exit_code(&api(422, "BUILD_FAILED")), exit::FAILED);
        assert_eq!(
            exit_code(&CliError::unreachable("refused").into()),
            exit::UNAVAILABLE
        );
        assert_eq!(exit_code(&CliError::usage("no").into()), exit::USAGE);
        assert_eq!(
            exit_code(&CliError::status_only(502, "bad gateway").into()),
            exit::UNAVAILABLE
        );
        assert_eq!(exit_code(&anyhow::anyhow!("plain")), exit::GENERIC);
    }

    #[test]
    fn context_keeps_the_exit_code_and_shows_in_the_message() {
        let err = Err::<(), _>(api(404, "APP_NOT_FOUND"))
            .context("Looking up web")
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("unreachable"));
        assert_eq!(exit_code(&err), exit::NOT_FOUND);
        assert_eq!(describe(&err).message, "Looking up web: boom");
    }

    #[test]
    fn json_and_human_rendering() {
        let err: anyhow::Error = CliError::api(
            404,
            ApiErrorBody::new("APP_NOT_FOUND", "App not found: web").with_hint("tond apps list"),
        )
        .into();
        assert_eq!(
            serde_json::from_str::<Value>(&render(&err, true)).ok(),
            Some(json!({
                "error": {"code": "APP_NOT_FOUND", "message": "App not found: web", "hint": "tond apps list", "field": null},
                "exit_code": 3
            }))
        );
        assert_eq!(
            render(&err, false),
            "Error: App not found: web\nHint: tond apps list"
        );
        let plain = anyhow::anyhow!("disk full");
        assert_eq!(
            serde_json::from_str::<Value>(&render(&plain, true)).ok(),
            Some(json!({
                "error": {"code": null, "message": "disk full", "hint": null, "field": null},
                "exit_code": 1
            }))
        );
    }
}
