// handlers/system_self.rs — Tond measuring itself
//
// GET /api/system/self — process, background loops, database, HTTP latency,
//                        build queue and tond-proxy stats (any signed-in user)
// GET /metrics         — the same in the Prometheus text format, only on the
//                        root-only Unix socket (404 over TCP), e.g.
//                        `curl --unix-socket /opt/tond/data/tond.sock http://tond/metrics`

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tond_common::ApiResponse;

use crate::auth::{LocalPeer, RateLimit, RequireAuth};
use crate::runtime::selfstats;

pub async fn get_self(_rate: RateLimit, _auth: RequireAuth) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(ApiResponse::ok(selfstats::collect().await)),
    )
}

pub async fn metrics(LocalPeer(local): LocalPeer) -> Response {
    if !local {
        return StatusCode::NOT_FOUND.into_response();
    }
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

    #[tokio::test]
    async fn metrics_are_only_served_on_the_local_socket() {
        assert_eq!(
            metrics(LocalPeer(false)).await.status(),
            StatusCode::NOT_FOUND
        );
        let local = metrics(LocalPeer(true)).await;
        assert_eq!(local.status(), StatusCode::OK);
        let body = axum::body::to_bytes(local.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("tond_db_queries_total"));
        assert!(text.contains("tond_build_slots 1"));
    }
}
