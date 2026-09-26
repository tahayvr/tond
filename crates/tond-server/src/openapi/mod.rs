// openapi/ — the API contract (OpenAPI 3.1)
//
// docs/openapi.json is generated from the route table (routes.rs) by the
// tests in this module and embedded in the binary; GET /api/v1/openapi.json
// serves it (public: it's a schema, it holds no data). Nothing is computed
// at runtime, and the schema machinery (schemars) is a dev-dependency only.
//
//   describe.rs  (tests) what each handler's extractors say: auth and
//                scopes (Authz<scope::X>), JSON body and query types,
//                WebSocket upgrades, rate limiting
//   catalog.rs   (tests) the human part: summary, response type and status
//                per route; a route without an entry fails the tests
//   generate.rs  (tests) builds the document, checks it, and compares it
//                with docs/openapi.json
//
// Regenerate after changing routes or request/response types:
//   just openapi        (= TOND_UPDATE_OPENAPI=1 cargo test -p tond-server openapi)

use axum::{
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};

#[cfg(test)]
mod catalog;
#[cfg(test)]
pub mod describe;
#[cfg(test)]
mod generate;

/// The generated spec, embedded at build time.
pub const SPEC: &str = include_str!("../../../../docs/openapi.json");

/// GET /api/v1/openapi.json
pub async fn spec() -> Response {
    let mut res = SPEC.into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    res
}
