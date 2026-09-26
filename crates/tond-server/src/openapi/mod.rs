// openapi/ — the API contract (OpenAPI 3.1)
//
// docs/openapi.json is generated from the route table (routes.rs) by the
// tests in this module; a gzipped copy (openapi.json.gz, ~7% of the size) is
// embedded in the binary and GET /api/v1/openapi.json serves it (public:
// it's a schema, it holds no data). Nothing is computed at runtime, and the
// schema machinery (schemars) is a dev-dependency only.
//
//   describe.rs  (tests) what each handler's extractors say: auth and
//                scopes (Authz<scope::X>), JSON body and query types,
//                WebSocket upgrades, rate limiting
//   catalog.rs   (tests) the human part: summary, response type and status
//                per route; a route without an entry fails the tests
//   generate.rs  (tests) builds the document, checks it, and compares it
//                with docs/openapi.json and openapi.json.gz
//
// Regenerate after changing routes or request/response types:
//   just openapi        (= TOND_UPDATE_OPENAPI=1 cargo test -p tond-server openapi)

use std::io::Read;

use axum::{
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};

#[cfg(test)]
mod catalog;
#[cfg(test)]
pub mod describe;
#[cfg(test)]
mod generate;

/// The generated spec, gzipped, embedded at build time.
pub const SPEC_GZ: &[u8] = include_bytes!("openapi.json.gz");

/// The spec as text (decompressed; for clients that don't take gzip).
pub fn spec_text() -> String {
    let mut text = String::new();
    let _ = flate2::read::GzDecoder::new(SPEC_GZ).read_to_string(&mut text);
    text
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|e| e.trim().split(';').next() == Some("gzip"))
        })
}

/// GET /api/v1/openapi.json
pub async fn spec(headers: HeaderMap) -> Response {
    let mut res = if accepts_gzip(&headers) {
        let mut res = SPEC_GZ.into_response();
        res.headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        res
    } else {
        spec_text().into_response()
    };
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    res
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn serves_gzip_or_plain_json() {
        let mut gz = HeaderMap::new();
        gz.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip;q=0.8"),
        );
        let res = spec(gz).await;
        assert_eq!(
            res.headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("gzip")
        );
        let res = spec(HeaderMap::new()).await;
        assert!(res.headers().get(header::CONTENT_ENCODING).is_none());
        let body = to_bytes(res.into_body(), 16 << 20)
            .await
            .unwrap_or_default();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        assert_eq!(doc["openapi"], "3.1.0");
    }
}
