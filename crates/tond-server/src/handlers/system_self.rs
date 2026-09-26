// handlers/system_self.rs — Tond measuring itself
//
// GET /api/system/self — (metrics:read) process, background loops, database, HTTP latency,
//                        build queue and tond-proxy stats (any signed-in user)
// GET /metrics         — the same in the Prometheus text format (metrics:read):
//                        over TCP with an API token as a bearer token (a
//                        Prometheus `authorization` credential), or on the
//                        root-only Unix socket without one, e.g.
//                        `curl --unix-socket /opt/tond/data/tond.sock http://tond/metrics`

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tond_common::ApiResponse;

use crate::auth::{Authz, RateLimit, scope};
use crate::runtime::selfstats;

pub async fn get_self(_rate: RateLimit, _auth: Authz<scope::MetricsRead>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(selfstats::collect().await)),
    )
}

pub async fn metrics(_rate: RateLimit, _auth: Authz<scope::MetricsRead>) -> Response {
    let report = selfstats::collect().await;
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        selfstats::prometheus(&report),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Principal;
    use tond_common::Role;

    #[tokio::test]
    async fn metrics_render_in_the_prometheus_format() {
        let response = metrics(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Viewer)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("tond_db_queries_total"));
        assert!(text.contains("tond_build_slots 1"));
    }
}
