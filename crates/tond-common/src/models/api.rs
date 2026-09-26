// api.rs — generic API response envelope
//
// `ApiResponse<T>` is the single JSON wrapper used for every API response:
//   { success: bool, data: T | null, error: string | null, code?: string }
//
// `code` is a stable, machine-readable error code on some failures (e.g.
// `SCOPE_DENIED`, `TOKEN_EXPIRED`); clients switch on it instead of the message.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: DeserializeOwned"))]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    /// Stable error code (see module docs). Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            code: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
            code: None,
        }
    }

    /// Failure with a stable error code.
    pub fn err_code(code: &str, msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
            code: Some(code.to_string()),
        }
    }

    /// Successful response with an informational message (no data).
    /// Used by webhook handlers to signal "received OK but no deploy triggered",
    /// e.g. auto_deploy not enabled or branch not watched.
    pub fn ok_info(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            data: None,
            error: Some(msg.into()),
            code: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_response_contains_data_without_error() {
        let response = ApiResponse::ok(vec!["one".to_string(), "two".to_string()]);

        assert!(response.success, "ok responses should be marked successful");
        assert_eq!(
            response.data,
            Some(vec!["one".to_string(), "two".to_string()]),
            "ok responses should store the provided payload in data"
        );
        assert_eq!(
            response.error, None,
            "ok responses should not carry an error message"
        );
    }

    #[test]
    fn err_response_contains_error_without_data() {
        let response: ApiResponse<Vec<String>> = ApiResponse::err("something failed");

        assert!(
            !response.success,
            "err responses should be marked unsuccessful"
        );
        assert_eq!(response.data, None, "err responses should not contain data");
        assert_eq!(
            response.error,
            Some("something failed".to_string()),
            "err responses should contain the provided error message"
        );
    }

    #[test]
    fn ok_info_response_uses_successful_envelope_with_message_only() {
        let response: ApiResponse<Vec<String>> = ApiResponse::ok_info("received but skipped");

        assert!(
            response.success,
            "ok_info responses should still be marked successful"
        );
        assert_eq!(response.data, None, "ok_info responses should omit data");
        assert_eq!(
            response.error,
            Some("received but skipped".to_string()),
            "ok_info responses should carry the informational message in the error field"
        );
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
            serde_json::json!({
                "success": true,
                "data": ["alpha", "beta"],
                "error": null
            }),
            "ApiResponse JSON shape should match the documented API envelope"
        );
        assert!(
            roundtrip.success,
            "round-tripped response should stay successful"
        );
        assert_eq!(
            roundtrip.data,
            Some(vec!["alpha".to_string(), "beta".to_string()]),
            "round-tripped response should preserve its payload"
        );
    }
}
