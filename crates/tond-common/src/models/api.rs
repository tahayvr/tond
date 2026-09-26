// api.rs — generic API response envelope
//
// `ApiResponse<T>` is the single JSON wrapper used for every API response:
//   success: { "success": true,  "data": T }
//   failure: { "success": false, "data": null,
//              "error": { "code": "APP_NOT_FOUND", "message": "…", "hint": "…", "field": "…" } }
//
// `error.code` is a stable, machine-readable code from `ERROR_CODES`
// (errors.rs); clients switch on it, never on the message. `hint` says what
// to do next and `field` names the offending request field; both are
// omitted when absent. `message` on a success carries an informational note
// (webhooks: "received, but auto-deploy is off").

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::errors::TondError;

/// The `error` object of a failed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApiErrorBody {
    /// Stable error code, e.g. `APP_NOT_FOUND` (see the ErrorCode enum).
    pub code: String,
    /// Human-readable sentence; may change between releases.
    pub message: String,
    /// What to do next, when there is something useful to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The request field the error is about (validation errors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl ApiErrorBody {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            hint: None,
            field: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl From<&TondError> for ApiErrorBody {
    fn from(e: &TondError) -> Self {
        Self {
            code: e.code().to_string(),
            message: e.message(),
            hint: e.hint(),
            field: e.field().map(str::to_string),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: DeserializeOwned"))]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    #[serde(default)]
    pub data: Option<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiErrorBody>,
    /// Informational note on a success (see module docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            message: None,
        }
    }

    /// Failure with a stable error code.
    pub fn err(code: &str, msg: impl Into<String>) -> Self {
        Self::error(ApiErrorBody::new(code, msg))
    }

    /// Failure from a full error body.
    pub fn error(body: ApiErrorBody) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(body),
            message: None,
        }
    }

    /// Successful response with an informational message (no data).
    /// Used by webhook handlers to signal "received OK but no deploy triggered",
    /// e.g. auto_deploy not enabled or branch not watched.
    pub fn ok_info(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            data: None,
            error: None,
            message: Some(msg.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_response_contains_data_without_error() {
        let response = ApiResponse::ok(vec!["one".to_string(), "two".to_string()]);
        assert!(response.success);
        assert_eq!(
            response.data,
            Some(vec!["one".to_string(), "two".to_string()])
        );
        assert_eq!(response.error, None);
    }

    #[test]
    fn err_response_serializes_the_structured_error() {
        let response: ApiResponse<Vec<String>> = ApiResponse::error(
            ApiErrorBody::new("APP_NOT_FOUND", "App not found: web").with_hint("List apps"),
        );
        assert_eq!(
            serde_json::to_value(&response).unwrap_or_default(),
            serde_json::json!({
                "success": false,
                "data": null,
                "error": {"code": "APP_NOT_FOUND", "message": "App not found: web", "hint": "List apps"}
            })
        );
    }

    #[test]
    fn tond_errors_become_error_bodies() {
        let body = ApiErrorBody::from(&TondError::invalid_field("port", "Port must be 1-65535"));
        assert_eq!(body.code, "INVALID_INPUT");
        assert_eq!(body.message, "Port must be 1-65535");
        assert_eq!(body.field.as_deref(), Some("port"));
    }

    #[test]
    fn ok_info_response_carries_a_message() {
        let response: ApiResponse<Vec<String>> = ApiResponse::ok_info("received but skipped");
        assert!(response.success);
        assert_eq!(response.data, None);
        assert_eq!(response.message.as_deref(), Some("received but skipped"));
    }

    #[test]
    fn api_response_serializes_and_deserializes_consistently() {
        let response = ApiResponse::ok(vec!["alpha".to_string(), "beta".to_string()]);
        let serialized = serde_json::to_value(&response)
            .unwrap_or_else(|err| panic!("failed to serialize ApiResponse: {err}"));
        let roundtrip: ApiResponse<Vec<String>> = serde_json::from_value(serialized.clone())
            .unwrap_or_else(|err| panic!("failed to deserialize ApiResponse: {err}"));
        assert_eq!(
            serialized,
            serde_json::json!({ "success": true, "data": ["alpha", "beta"] })
        );
        assert!(roundtrip.success);
        assert_eq!(
            roundtrip.data,
            Some(vec!["alpha".to_string(), "beta".to_string()])
        );
    }
}
