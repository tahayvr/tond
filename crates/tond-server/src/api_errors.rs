// api_errors.rs — structured errors for failures that don't come from a handler
//
// Handlers answer errors through AppError (error.rs). A few failures happen
// before a handler runs and axum answers them in plain text: a JSON body that
// doesn't parse or doesn't match the type (400/415/422), a bad path or query
// parameter (400), a body over the limit (413), a known path with the wrong
// method (405). `structured` rewrites those into the standard envelope
//   {"success":false,"data":null,"error":{"code","message","hint","field"}}
// so clients can switch on `code` for every API error. `field` is taken from
// serde's error path ("name: invalid type…", "missing field `name`").
//
// `not_found` answers every /api path no route matches (instead of the SPA's
// index.html), including the pre-v1 unversioned paths.

use axum::{
    body::to_bytes,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};
use tond_common::{ApiErrorBody, code};

use crate::error::{code_for_status, error_response};

/// Largest plain-text rejection body read back (axum's are one line).
const MAX_REJECTION_BYTES: usize = 8 * 1024;

/// Rewrite plain-text / empty error responses into the JSON error envelope.
pub async fn structured(req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    let status = res.status();
    if !(status.is_client_error() || status.is_server_error()) || is_json(&res) {
        return res;
    }
    let (parts, body) = res.into_parts();
    let text = to_bytes(body, MAX_REJECTION_BYTES)
        .await
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();
    let body = rejection_body(status, &text);
    let mut rewritten = error_response(status, body);
    // Keep headers such as Allow (405) and Retry-After (429).
    for (name, value) in parts.headers.iter() {
        if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
            rewritten.headers_mut().insert(name.clone(), value.clone());
        }
    }
    rewritten
}

fn is_json(res: &Response) -> bool {
    res.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

/// The error body for a plain-text rejection `text` with `status`.
pub fn rejection_body(status: StatusCode, text: &str) -> ApiErrorBody {
    const JSON_PREFIXES: [&str; 3] = [
        "Failed to deserialize the JSON body",
        "Failed to parse the request body as JSON",
        "Expected request with `Content-Type: application/json`",
    ];
    let is_json_rejection = JSON_PREFIXES.iter().any(|p| text.starts_with(p));
    let code = match status {
        StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY
        | StatusCode::UNSUPPORTED_MEDIA_TYPE
            if is_json_rejection =>
        {
            code::INVALID_JSON
        }
        StatusCode::UNSUPPORTED_MEDIA_TYPE => code::INVALID_INPUT,
        s => code_for_status(s),
    };
    let message = if text.is_empty() {
        default_message(status).to_string()
    } else {
        text.to_string()
    };
    let mut body = ApiErrorBody::new(code, message);
    body.field = serde_field(text);
    body.hint = match code {
        code::INVALID_JSON => {
            Some("Check the request body against the OpenAPI spec at /api/v1/openapi.json.".into())
        }
        code::METHOD_NOT_ALLOWED => {
            Some("The Allow header lists the methods this path accepts.".into())
        }
        _ => None,
    };
    body
}

fn default_message(status: StatusCode) -> &'static str {
    match status {
        StatusCode::METHOD_NOT_ALLOWED => "This endpoint doesn't accept this HTTP method.",
        StatusCode::PAYLOAD_TOO_LARGE => "The request body is too large.",
        StatusCode::NOT_FOUND => "Not found.",
        StatusCode::UNAUTHORIZED => "Authentication required.",
        StatusCode::FORBIDDEN => "Forbidden.",
        s if s.is_server_error() => "The server failed to handle the request.",
        _ => "The request was rejected.",
    }
}

/// The request field a serde error names: `address.port: invalid type…`
/// (serde_path_to_error, as axum reports it) or "missing field `name`".
fn serde_field(text: &str) -> Option<String> {
    let detail = text
        .split_once("into the target type: ")
        .map(|(_, rest)| rest)
        .or_else(|| {
            text.split_once("Failed to deserialize query string: ")
                .map(|(_, r)| r)
        })
        .or_else(|| text.split_once("Invalid URL: ").map(|(_, r)| r))?;
    let (path, inner) = match detail.split_once(": ") {
        Some((path, inner)) if !path.contains(' ') => (path, inner),
        _ => (".", detail),
    };
    let missing = inner
        .split_once("missing field `")
        .and_then(|(_, rest)| rest.split_once('`'))
        .map(|(name, _)| name);
    let path = (path != ".").then_some(path);
    match (path, missing) {
        (Some(p), Some(m)) => Some(format!("{p}.{m}")),
        (Some(p), None) => Some(p.to_string()),
        (None, Some(m)) => Some(m.to_string()),
        (None, None) => None,
    }
}

/// Every /api path without a route: a JSON 404 rather than the SPA.
pub async fn not_found(req: Request) -> Response {
    let path = req.uri().path();
    let mut body = ApiErrorBody::new(
        code::ROUTE_NOT_FOUND,
        format!("No API endpoint at {} {path}.", req.method()),
    );
    body.hint = Some(if path.starts_with("/api/v1/") {
        "See /api/v1/openapi.json for the list of endpoints.".to_string()
    } else {
        format!(
            "The API is versioned: use {} (spec at /api/v1/openapi.json).",
            path.replacen("/api/", "/api/v1/", 1)
        )
    });
    let mut res = error_response(StatusCode::NOT_FOUND, body);
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// A plain-text response, as axum's own rejections are (for tests).
#[cfg(test)]
fn plain(status: StatusCode, text: &str) -> Response {
    use axum::{body::Body, response::IntoResponse};
    (status, Body::from(text.to_string())).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::{
        Json, Router,
        routing::{any, get, post},
    };
    use serde::Deserialize;
    use tower::ServiceExt;

    use super::*;

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Body1 {
        name: String,
        port: u16,
    }

    async fn create(Json(_): Json<Body1>) -> &'static str {
        "ok"
    }

    fn app() -> Router {
        Router::new()
            .route("/api/v1/things", post(create))
            .route(
                "/api/v1/teapot",
                get(|| async { plain(StatusCode::IM_A_TEAPOT, "short and stout") }),
            )
            .route("/api/{*rest}", any(not_found))
            .layer(axum::middleware::from_fn(structured))
    }

    async fn send(req: Request) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
        let res = app().oneshot(req).await.unwrap_or_else(|e| match e {});
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap_or_default();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or_default(),
            headers,
        )
    }

    fn post_json(body: &str) -> Request {
        Request::post("/api/v1/things")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn json_rejections_become_invalid_json_with_the_field() {
        let (status, body, _) = send(post_json(r#"{"name":"x","port":"eighty"}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "INVALID_JSON");
        assert_eq!(body["error"]["field"], "port");
        assert!(body["error"]["hint"].is_string());

        let (_, body, _) = send(post_json(r#"{"port":80}"#)).await;
        assert_eq!(body["error"]["field"], "name", "{body}");

        let (status, body, _) = send(post_json("{not json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "INVALID_JSON");

        let (status, body, _) = send(
            Request::post("/api/v1/things")
                .body(Body::from("{}"))
                .unwrap_or_default(),
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body["error"]["code"], "INVALID_JSON");
    }

    #[tokio::test]
    async fn wrong_methods_and_unknown_paths_are_structured() {
        let (status, body, headers) = send(
            Request::get("/api/v1/things")
                .body(Body::empty())
                .unwrap_or_default(),
        )
        .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["error"]["code"], "METHOD_NOT_ALLOWED");
        assert!(
            headers.contains_key(header::ALLOW),
            "Allow survives the rewrite"
        );

        let (status, body, _) = send(
            Request::get("/api/v1/nope")
                .body(Body::empty())
                .unwrap_or_default(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "ROUTE_NOT_FOUND");

        let (_, body, _) = send(
            Request::get("/api/apps")
                .body(Body::empty())
                .unwrap_or_default(),
        )
        .await;
        assert_eq!(body["error"]["code"], "ROUTE_NOT_FOUND");
        assert!(
            body["error"]["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("/api/v1/apps"),
            "unversioned paths point at v1: {body}"
        );
    }

    #[tokio::test]
    async fn other_plain_errors_keep_their_status_and_text() {
        let (status, body, _) = send(
            Request::get("/api/v1/teapot")
                .body(Body::empty())
                .unwrap_or_default(),
        )
        .await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT);
        assert_eq!(body["error"]["message"], "short and stout");
        assert_eq!(body["error"]["code"], "INVALID_INPUT");
    }

    #[test]
    fn serde_fields_are_extracted() {
        assert_eq!(
            serde_field("Failed to deserialize the JSON body into the target type: env.port: invalid type: string \"x\", expected u16 at line 1 column 9").as_deref(),
            Some("env.port")
        );
        assert_eq!(
            serde_field("Failed to deserialize the JSON body into the target type: missing field `name` at line 1 column 2").as_deref(),
            Some("name")
        );
        assert_eq!(
            serde_field("Failed to parse the request body as JSON: EOF"),
            None
        );
    }
}
