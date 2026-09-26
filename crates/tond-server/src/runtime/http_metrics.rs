// runtime/http_metrics.rs — per-route HTTP latency histograms
//
// A route layer (applied to the API router, so axum's MatchedPath gives the
// route template, never the raw path) times every request into a fixed set
// of buckets. Keys are "<METHOD> <route>", so the number of series is bounded
// by the router, not by traffic. WebSocket routes measure the upgrade only.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use tond_common::RouteLatency;

/// Bucket upper bounds in milliseconds; a final +Inf bucket follows.
pub const BUCKET_BOUNDS_MS: [f64; 12] = [
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 10_000.0,
];
const BUCKETS: usize = BUCKET_BOUNDS_MS.len() + 1;

#[derive(Clone)]
struct Histogram {
    buckets: [u64; BUCKETS],
    count: u64,
    errors: u64,
    sum_ms: f64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: [0; BUCKETS],
            count: 0,
            errors: 0,
            sum_ms: 0.0,
        }
    }
}

impl Histogram {
    fn observe(&mut self, ms: f64, error: bool) {
        let idx = BUCKET_BOUNDS_MS
            .iter()
            .position(|bound| ms <= *bound)
            .unwrap_or(BUCKETS - 1);
        self.buckets[idx] += 1;
        self.count += 1;
        self.sum_ms += ms;
        if error {
            self.errors += 1;
        }
    }

    /// Upper bound of the bucket holding quantile `q` (the last finite bound
    /// for the +Inf bucket).
    fn quantile(&self, q: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = (q * self.count as f64).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= target {
                return BUCKET_BOUNDS_MS
                    .get(i)
                    .copied()
                    .unwrap_or(BUCKET_BOUNDS_MS[BUCKET_BOUNDS_MS.len() - 1]);
            }
        }
        BUCKET_BOUNDS_MS[BUCKET_BOUNDS_MS.len() - 1]
    }
}

static ROUTES: LazyLock<Mutex<HashMap<String, Histogram>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn observe(method: &str, route: &str, ms: f64, error: bool) {
    let key = format!("{method} {route}");
    let mut routes = ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    routes.entry(key).or_default().observe(ms, error);
}

/// Middleware: time the request under its route template.
pub async fn track(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "(unmatched)".to_string());
    let started = Instant::now();
    let response = next.run(request).await;
    observe(
        &method,
        &route,
        started.elapsed().as_secs_f64() * 1000.0,
        response.status().is_server_error(),
    );
    response
}

/// Snapshot of every route, sorted by method and route.
pub fn snapshot() -> Vec<RouteLatency> {
    let routes = ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<RouteLatency> = routes
        .iter()
        .map(|(key, h)| {
            let (method, route) = key.split_once(' ').unwrap_or(("", key));
            RouteLatency {
                method: method.to_string(),
                route: route.to_string(),
                count: h.count,
                errors: h.errors,
                sum_ms: h.sum_ms,
                buckets: h.buckets.to_vec(),
                p50_ms: h.quantile(0.5),
                p95_ms: h.quantile(0.95),
            }
        })
        .collect();
    out.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.method.cmp(&b.method)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::StatusCode, routing::get};
    use tower::ServiceExt;

    #[test]
    fn buckets_and_quantiles() {
        let mut h = Histogram::default();
        for ms in [0.5, 3.0, 3.0, 40.0, 20_000.0] {
            h.observe(ms, false);
        }
        assert_eq!(h.count, 5);
        assert_eq!(h.buckets[0], 1); // <= 1 ms
        assert_eq!(h.buckets[1], 2); // <= 5 ms
        assert_eq!(h.buckets[4], 1); // <= 50 ms
        assert_eq!(h.buckets[BUCKETS - 1], 1); // +Inf
        assert_eq!(h.quantile(0.5), 5.0);
        assert_eq!(h.quantile(0.95), 10_000.0);
        assert_eq!(Histogram::default().quantile(0.5), 0.0);
    }

    #[tokio::test]
    async fn middleware_records_route_templates_not_raw_paths() {
        let app = Router::new()
            .route(
                "/api/test-latency/{id}",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route_layer(axum::middleware::from_fn(track));
        for id in ["a", "b"] {
            let _ = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(format!("/api/test-latency/{id}"))
                        .body(Body::empty())
                        .unwrap_or_default(),
                )
                .await;
        }
        let snap = snapshot();
        let entry = snap
            .iter()
            .find(|r| r.route == "/api/test-latency/{id}")
            .unwrap_or_else(|| panic!("route template recorded: {snap:?}"));
        assert_eq!(entry.method, "GET");
        assert_eq!(entry.count, 2);
        assert_eq!(entry.errors, 2);
        assert!(!snap.iter().any(|r| r.route.ends_with("/a")));
    }
}
