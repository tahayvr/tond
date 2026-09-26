// static_files.rs — serve the dashboard SPA with compression and caching
//
//   Precompressed  The dashboard build writes foo.js.br and foo.js.gz next to
//                  every text asset (adapter-static `precompress`). ServeDir
//                  picks the best variant the browser accepts and sets
//                  Content-Encoding; nothing is compressed at request time.
//   Caching        /_app/immutable/* file names contain a content hash, so
//                  they are cached for a year and never revalidated.
//                  Everything else (index.html, version.json, robots.txt) is
//                  `no-cache`: the browser revalidates with If-Modified-Since
//                  and gets a tiny 304 until the next deploy.
//   SPA fallback   Unknown paths get index.html with 200 (client-side
//                  routes). A missing file under /_app/ is a real 404 — an old
//                  tab asking for a chunk from a previous build must not get
//                  HTML (with a year-long cache header) instead of JavaScript.

use std::path::Path;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use tower_http::services::{ServeDir, ServeFile};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

/// Router serving the built dashboard in `dir` for every path it is mounted on.
pub fn dashboard_router(dir: &Path) -> Router {
    let index = ServeFile::new(dir.join("index.html"))
        .precompressed_br()
        .precompressed_gzip();
    let serve_dir = ServeDir::new(dir)
        .precompressed_br()
        .precompressed_gzip()
        .fallback(index);
    Router::new()
        .fallback_service(serve_dir)
        .layer(middleware::from_fn(cache_headers))
}

async fn cache_headers(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));

    if path.starts_with("/_app/") && is_html {
        // The SPA fallback answered for a missing build file.
        *res.status_mut() = StatusCode::NOT_FOUND;
    }
    let ok = res.status().is_success() || res.status() == StatusCode::NOT_MODIFIED;
    let policy = if ok && !is_html && path.starts_with("/_app/immutable/") {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    let headers = res.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    if !headers.contains_key(header::VARY) {
        headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    res
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use tower::ServiceExt;

    use super::*;

    fn build_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let write = |rel: &str, body: &[u8]| {
            let p = dir.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir: {e}"));
            }
            std::fs::write(p, body).unwrap_or_else(|e| panic!("write {rel}: {e}"));
        };
        write("index.html", b"<html>shell</html>");
        write("index.html.br", b"BR-SHELL");
        write("index.html.gz", b"GZ-SHELL");
        write("_app/immutable/chunks/app.abc123.js", b"console.log(1)");
        write("_app/immutable/chunks/app.abc123.js.br", b"BR-JS");
        write("_app/immutable/chunks/app.abc123.js.gz", b"GZ-JS");
        write("_app/version.json", b"{\"version\":\"1\"}");
        write("robots.txt", b"User-agent: *");
        dir
    }

    async fn get(dir: &Path, path: &str, accept: Option<&str>) -> Response {
        let mut req = Request::builder().uri(path);
        if let Some(a) = accept {
            req = req.header(header::ACCEPT_ENCODING, a);
        }
        dashboard_router(dir)
            .oneshot(req.body(Body::empty()).unwrap_or_else(|e| panic!("{e}")))
            .await
            .unwrap_or_else(|e| match e {})
    }

    fn header_of(res: &Response, name: header::HeaderName) -> Option<&str> {
        res.headers().get(name).and_then(|v| v.to_str().ok())
    }

    async fn body(res: Response) -> String {
        let bytes = to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap_or_else(|e| panic!("body: {e}"));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn hashed_assets_are_immutable_and_precompressed() {
        let dir = build_dir();
        let res = get(
            dir.path(),
            "/_app/immutable/chunks/app.abc123.js",
            Some("gzip, deflate, br"),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header_of(&res, header::CACHE_CONTROL), Some(IMMUTABLE));
        assert_eq!(header_of(&res, header::CONTENT_ENCODING), Some("br"));
        assert!(
            header_of(&res, header::VARY)
                .is_some_and(|v| v.to_ascii_lowercase().contains("accept-encoding"))
        );
        assert!(header_of(&res, header::CONTENT_TYPE).is_some_and(|v| v.contains("javascript")));
        assert_eq!(body(res).await, "BR-JS");

        let gz = get(
            dir.path(),
            "/_app/immutable/chunks/app.abc123.js",
            Some("gzip"),
        )
        .await;
        assert_eq!(header_of(&gz, header::CONTENT_ENCODING), Some("gzip"));
        assert_eq!(body(gz).await, "GZ-JS");

        let plain = get(dir.path(), "/_app/immutable/chunks/app.abc123.js", None).await;
        assert_eq!(header_of(&plain, header::CONTENT_ENCODING), None);
        assert_eq!(body(plain).await, "console.log(1)");
    }

    #[tokio::test]
    async fn index_and_spa_routes_revalidate() {
        let dir = build_dir();
        for path in ["/", "/apps/web", "/settings"] {
            let res = get(dir.path(), path, Some("br")).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header_of(&res, header::CACHE_CONTROL),
                Some(REVALIDATE),
                "{path}"
            );
            assert_eq!(
                header_of(&res, header::CONTENT_ENCODING),
                Some("br"),
                "{path}"
            );
            assert_eq!(body(res).await, "BR-SHELL");
        }
        for path in ["/_app/version.json", "/robots.txt"] {
            let res = get(dir.path(), path, None).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header_of(&res, header::CACHE_CONTROL),
                Some(REVALIDATE),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn revalidation_returns_not_modified() {
        let dir = build_dir();
        let first = get(dir.path(), "/", None).await;
        let last_modified = header_of(&first, header::LAST_MODIFIED)
            .unwrap_or_else(|| panic!("Last-Modified expected"))
            .to_string();
        let res = dashboard_router(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::IF_MODIFIED_SINCE, last_modified)
                    .body(Body::empty())
                    .unwrap_or_else(|e| panic!("{e}")),
            )
            .await
            .unwrap_or_else(|e| match e {});
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn missing_build_files_are_404_not_the_shell() {
        let dir = build_dir();
        let res = get(dir.path(), "/_app/immutable/chunks/old.deadbeef.js", None).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(header_of(&res, header::CACHE_CONTROL), Some(REVALIDATE));
    }
}
