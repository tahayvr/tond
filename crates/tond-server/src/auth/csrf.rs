// auth/csrf.rs — same-origin enforcement for cookie-authenticated requests
//
// Previews are served on subdomains of the dashboard domain, which makes a
// preview page *same-site* with the dashboard: SameSite cookies alone don't
// stop it from sending the admin's cookie. This middleware closes that gap:
//
//   * Every state-changing request (anything but GET/HEAD/OPTIONS) and every
//     WebSocket upgrade that carries an `Origin` header must come from the
//     dashboard origin (`https://<dashboard_domain>`) or from a loopback origin
//     (Vite dev server, SSH tunnel to 127.0.0.1:3001).
//   * A state-changing request that carries a Tond cookie must also send the
//     `X-Tond-CSRF` header. Browsers only let a page add custom headers to a
//     cross-origin request after a CORS preflight, and tond-server answers no
//     preflight, so only the dashboard itself can send it. This also covers
//     body-less POSTs (deploy, restore, rollback) that need no preflight
//     otherwise.
//   * A WebSocket upgrade without an `Origin` header (CLI, scripts) has its
//     Cookie header removed: such clients must authenticate explicitly
//     (API token or terminal ticket), never with ambient browser cookies.
//
// Requests on the root-only Unix socket and API-token requests without cookies
// are not affected. There is intentionally no CORS layer: the dashboard is
// same-origin, so cross-origin browser access is simply not offered.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tond_common::ApiResponse;

use super::LocalSocket;
use crate::handlers::auth::{MFA_PENDING_COOKIE, SESSION_COOKIE, extract_cookie};
use crate::state::AppState;

/// Header the dashboard sends on every API request (any non-empty value).
pub const CSRF_HEADER: &str = "x-tond-csrf";

/// The request's `Origin` header, if present and valid UTF-8.
pub fn request_origin(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
}

/// Whether `origin` may drive the dashboard API with the user's cookies.
pub fn origin_allowed(origin: &str, dashboard_domain: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false; // includes the opaque "null" origin
    };
    if authority.is_empty() || authority.contains('/') {
        return false;
    }
    let scheme = scheme.to_ascii_lowercase();
    let authority = authority.to_ascii_lowercase();

    let dashboard = dashboard_domain.trim().to_ascii_lowercase();
    if !dashboard.is_empty() && scheme == "https" && authority == dashboard {
        return true;
    }

    (scheme == "http" || scheme == "https") && is_loopback_authority(&authority)
}

/// `localhost`, `*.localhost`, `127.0.0.0/8` or `[::1]`, with an optional port.
fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, "")) => host,
            Some((host, port)) if port.strip_prefix(':').is_some_and(is_port) => host,
            _ => return false,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if is_port(port) => host,
            Some(_) => return false,
            None => authority,
        }
    };

    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn is_port(s: &str) -> bool {
    !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Whether the request is a WebSocket upgrade handshake.
pub fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

fn has_tond_cookie(headers: &HeaderMap) -> bool {
    extract_cookie(headers, SESSION_COOKIE).is_some()
        || extract_cookie(headers, MFA_PENDING_COOKIE).is_some()
}

fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(ApiResponse::<()>::err(message))).into_response()
}

/// Decide whether a request passes the same-origin rules (see module docs).
/// Returns the rejection message, or None to let it through.
pub fn check_request(
    method: &Method,
    headers: &HeaderMap,
    dashboard_domain: &str,
) -> Option<&'static str> {
    let websocket = is_websocket_upgrade(headers);
    if !websocket && is_safe_method(method) {
        return None;
    }

    if let Some(origin) = request_origin(headers)
        && !origin_allowed(origin, dashboard_domain)
    {
        return Some("Cross-origin request blocked");
    }

    if websocket || !has_tond_cookie(headers) {
        return None;
    }

    // Cookie-authenticated mutation from a browser.
    let site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if site.eq_ignore_ascii_case("cross-site") || site.eq_ignore_ascii_case("same-site") {
        return Some("Cross-site request blocked");
    }
    let has_csrf_header = headers
        .get(CSRF_HEADER)
        .is_some_and(|v| !v.as_bytes().is_empty());
    if !has_csrf_header {
        return Some("Missing X-Tond-CSRF header");
    }
    None
}

/// Axum middleware applying `check_request` to every TCP request.
pub async fn enforce_same_origin(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if req.extensions().get::<LocalSocket>().is_some() {
        return next.run(req).await;
    }

    let dashboard_domain = state.settings.read().await.dashboard_domain.clone();
    if let Some(message) = check_request(req.method(), req.headers(), &dashboard_domain) {
        tracing::warn!(
            "Blocked {} {} (origin={:?}): {}",
            req.method(),
            req.uri().path(),
            request_origin(req.headers()),
            message
        );
        return forbidden(message);
    }

    if is_websocket_upgrade(req.headers()) && request_origin(req.headers()).is_none() {
        req.headers_mut().remove(header::COOKIE);
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::Request as HttpRequest, routing::any};
    use tower::ServiceExt;

    use axum::http::HeaderValue;

    use super::*;

    const DASH: &str = "dash.example.com";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                *name,
                HeaderValue::from_str(value).unwrap_or_else(|err| panic!("bad header: {err}")),
            );
        }
        map
    }

    const COOKIE: (&str, &str) = ("cookie", "__Host-tond_session=abc");

    #[test]
    fn origin_allowed_accepts_only_the_dashboard_and_loopback() {
        assert!(origin_allowed("https://dash.example.com", DASH));
        assert!(origin_allowed("https://DASH.example.com", DASH));
        assert!(origin_allowed("http://localhost:5173", DASH));
        assert!(origin_allowed("http://127.0.0.1:3001", DASH));
        assert!(origin_allowed("http://[::1]:3001", DASH));
        assert!(origin_allowed("http://app.localhost", ""));

        // Preview subdomains are same-site but must not be same-origin.
        assert!(!origin_allowed("https://pr-1.dash.example.com", DASH));
        assert!(!origin_allowed("http://dash.example.com", DASH));
        assert!(!origin_allowed("https://dash.example.com:8443", DASH));
        assert!(!origin_allowed("https://dash.example.com.evil.test", DASH));
        assert!(!origin_allowed("https://localhost.evil.test", DASH));
        assert!(!origin_allowed("null", DASH));
        assert!(!origin_allowed("https://dash.example.com", ""));
    }

    #[test]
    fn body_less_cookie_post_without_csrf_header_is_rejected() {
        // e.g. POST /api/apps/{id}/deploy from a preview page.
        let h = headers(&[COOKIE]);
        assert_eq!(
            check_request(&Method::POST, &h, DASH),
            Some("Missing X-Tond-CSRF header")
        );
    }

    #[test]
    fn cookie_mutation_with_header_and_dashboard_origin_passes() {
        let h = headers(&[
            COOKIE,
            ("x-tond-csrf", "1"),
            ("origin", "https://dash.example.com"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert_eq!(check_request(&Method::DELETE, &h, DASH), None);
    }

    #[test]
    fn foreign_origin_is_rejected_even_with_the_header() {
        let h = headers(&[
            COOKIE,
            ("x-tond-csrf", "1"),
            ("origin", "https://preview.dash.example.com"),
        ]);
        assert_eq!(
            check_request(&Method::POST, &h, DASH),
            Some("Cross-origin request blocked")
        );
    }

    #[test]
    fn same_site_fetch_metadata_is_rejected_for_cookie_mutations() {
        let h = headers(&[
            COOKIE,
            ("x-tond-csrf", "1"),
            ("sec-fetch-site", "same-site"),
        ]);
        assert_eq!(
            check_request(&Method::POST, &h, DASH),
            Some("Cross-site request blocked")
        );
    }

    #[test]
    fn reads_and_cookieless_api_key_requests_are_unaffected() {
        assert_eq!(check_request(&Method::GET, &headers(&[COOKIE]), DASH), None);
        let api_key = headers(&[("authorization", "Bearer k")]);
        assert_eq!(check_request(&Method::POST, &api_key, DASH), None);
        // Webhooks: no cookie, no Origin.
        assert_eq!(check_request(&Method::POST, &HeaderMap::new(), DASH), None);
    }

    #[test]
    fn websocket_upgrades_from_foreign_origins_are_rejected() {
        let hijack = headers(&[
            COOKIE,
            ("upgrade", "websocket"),
            ("origin", "https://evil.dash.example.com"),
        ]);
        assert_eq!(
            check_request(&Method::GET, &hijack, DASH),
            Some("Cross-origin request blocked")
        );
        let dashboard = headers(&[
            COOKIE,
            ("upgrade", "websocket"),
            ("origin", "https://dash.example.com"),
        ]);
        assert_eq!(check_request(&Method::GET, &dashboard, DASH), None);
    }

    async fn test_state() -> AppState {
        use std::sync::Arc;

        use bollard::{API_DEFAULT_VERSION, Docker};
        use sqlx::SqlitePool;
        use tokio::sync::RwLock;
        use tond_common::ServerSettings;

        use crate::auth::RateLimiter;

        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings {
                dashboard_domain: DASH.to_string(),
                ..ServerSettings::default()
            })),
        }
    }

    fn app(state: AppState) -> Router {
        Router::new()
            .route(
                "/echo",
                any(|headers: HeaderMap| async move {
                    if headers.contains_key(header::COOKIE) {
                        "cookie"
                    } else {
                        "no-cookie"
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                enforce_same_origin,
            ))
            .with_state(state)
    }

    async fn call(app: Router, req: HttpRequest<Body>) -> (StatusCode, String) {
        let res = app
            .oneshot(req)
            .await
            .unwrap_or_else(|err| panic!("request failed: {err}"));
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("body: {err}"));
        (status, String::from_utf8_lossy(&body).to_string())
    }

    #[tokio::test]
    async fn middleware_blocks_csrf_and_strips_cookies_from_originless_websockets() {
        let state = test_state().await;

        let csrf = HttpRequest::post("/echo")
            .header("cookie", "__Host-tond_session=abc")
            .body(Body::empty())
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(
            call(app(state.clone()), csrf).await.0,
            StatusCode::FORBIDDEN
        );

        let ws = HttpRequest::get("/echo")
            .header("cookie", "__Host-tond_session=abc")
            .header("upgrade", "websocket")
            .body(Body::empty())
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(
            call(app(state.clone()), ws).await,
            (StatusCode::OK, "no-cookie".to_string()),
            "an Origin-less WebSocket must not be authenticated by cookies"
        );

        let mut local = HttpRequest::post("/echo")
            .header("cookie", "__Host-tond_session=abc")
            .body(Body::empty())
            .unwrap_or_else(|err| panic!("{err}"));
        local.extensions_mut().insert(LocalSocket);
        assert_eq!(call(app(state), local).await.0, StatusCode::OK);
    }
}
