// handlers/requests.rs — request metric endpoints (tond-proxy traffic)
//
// Routes (scope metrics:read):
//   GET /api/apps/{id}/requests?range=1h|6h|24h|7d|30d|90d
//   GET /api/previews/{id}/requests?range=…
//     Series (≤ ~480 buckets: RPS by status class, 5xx %, p50/p95/p99,
//     bandwidth), a summary over the window, the last minute from memory,
//     and chart markers (deploys, rollbacks, restarts, crashes, OOM kills).
//     `{id}` may be the app's id or name.
//
//   GET /api/system/requests?range=…
//     Server-wide totals plus the busiest apps and the apps with the most
//     5xx responses (home page tile).
//
//   GET /api/previews/{id}/metrics?range=1h|24h|7d|30d
//     A preview's container metrics (same shape as the app endpoint).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use tond_common::traffic::{RouteKind, TrafficCounters};
use tond_common::*;

use crate::{
    auth::{Authz, RateLimit, scope},
    error::AppError,
    handlers::metrics::metrics_for,
    metrics::{Owner, TargetKind, requests, store},
    state::AppState,
};

fn parse_range(range: &str) -> Result<requests::Range, AppError> {
    requests::Range::parse(range).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "range must be one of {} (got '{range}')",
            requests::Range::ALLOWED
        )))
    })
}

/// Parse a stored timestamp (RFC 3339 or SQLite's "YYYY-MM-DD HH:MM:SS").
fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc().timestamp())
        })
}

/// Deploys and rollbacks of `app_id` that finished (or started) in the window.
async fn deploy_markers(
    state: &AppState,
    app_id: &str,
    from: i64,
) -> Result<Vec<RequestMarker>, AppError> {
    // Coarse filter on the text column (an hour of slack for long builds),
    // exact filter after parsing.
    let since = chrono::DateTime::from_timestamp(from - 3600, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string();
    let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, kind, status, started_at, finished_at FROM deployments
         WHERE app_id = ? AND created_at >= ? ORDER BY created_at DESC LIMIT 200",
    )
    .bind(app_id)
    .bind(&since)
    .fetch_all(&state.db)
    .await?;
    let mut markers: Vec<RequestMarker> = rows
        .into_iter()
        .filter_map(|(id, kind, status, started, finished)| {
            let t = finished
                .as_deref()
                .and_then(parse_time)
                .or_else(|| parse_time(&started))?;
            (t >= from).then(|| RequestMarker {
                t,
                kind: if kind == "rollback" {
                    "rollback"
                } else {
                    "deploy"
                }
                .to_string(),
                detail: Some(status),
                deployment_id: Some(id),
            })
        })
        .collect();
    markers.reverse();
    Ok(markers)
}

/// Restarts, crashes and OOM kills of a container target in the window.
async fn event_markers(
    state: &AppState,
    owner: &Owner,
    from: i64,
) -> Result<Vec<RequestMarker>, AppError> {
    let Some((target, _)) = store::find_target(&state.db, owner).await? else {
        return Ok(Vec::new());
    };
    Ok(store::query_events(&state.db, target, from)
        .await?
        .into_iter()
        .map(|e| RequestMarker {
            t: e.t,
            kind: e.kind,
            detail: e.detail,
            deployment_id: None,
        })
        .collect())
}

/// The request metrics response for one route.
pub async fn requests_for(
    state: &AppState,
    route: &requests::RouteId,
    range: requests::Range,
    now: i64,
) -> Result<RequestMetricsResponse, AppError> {
    let target = requests::find_target(&state.db, route).await?;
    let (from, series, summary) = requests::query(&state.db, target.as_slice(), range, now).await?;

    let (owner_kind, app_id) = match route.0 {
        RouteKind::App => (TargetKind::App, Some(route.1.as_str())),
        _ => (TargetKind::Preview, None),
    };
    let mut markers = match app_id {
        Some(id) => deploy_markers(state, id, from).await?,
        None => Vec::new(),
    };
    markers.extend(event_markers(state, &Owner::new(owner_kind, route.1.clone()), from).await?);
    markers.sort_by_key(|m| m.t);

    Ok(RequestMetricsResponse {
        range: range.as_str().to_string(),
        step: range.step(),
        from,
        to: now,
        series,
        summary,
        current: requests::current(route, now),
        markers,
        proxy_connected: requests::proxy_connected(),
    })
}

async fn lookup_app(state: &AppState, id_or_name: &str) -> Result<(String, String), AppError> {
    sqlx::query_as("SELECT id, name FROM apps WHERE id = ? OR name = ? LIMIT 1")
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::AppNotFound(id_or_name.to_string())))
}

async fn lookup_preview(state: &AppState, id: &str) -> Result<String, AppError> {
    sqlx::query_scalar("SELECT id FROM preview_deployments WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("preview '{id}'"))))
}

pub async fn get_app_requests(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RequestMetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let range = parse_range(&query.range)?;
    let (app_id, _) = lookup_app(&state, &id).await?;
    let body = requests_for(
        &state,
        &(RouteKind::App, app_id),
        range,
        Utc::now().timestamp(),
    )
    .await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

pub async fn get_preview_requests(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RequestMetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let range = parse_range(&query.range)?;
    let preview_id = lookup_preview(&state, &id).await?;
    let body = requests_for(
        &state,
        &(RouteKind::Preview, preview_id),
        range,
        Utc::now().timestamp(),
    )
    .await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

pub async fn get_preview_metrics(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let range = store::Range::parse(&query.range).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "range must be one of 1h, 24h, 7d, 30d (got '{}')",
            query.range
        )))
    })?;
    let preview_id = lookup_preview(&state, &id).await?;
    let body = metrics_for(
        &state,
        &Owner::new(TargetKind::Preview, preview_id),
        range,
        Utc::now().timestamp(),
    )
    .await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

fn app_traffic(id: String, name: String, c: &TrafficCounters, secs: i64) -> AppTrafficResponse {
    let s = requests::summarize(c, secs);
    AppTrafficResponse {
        id,
        name,
        requests: s.requests,
        rps: s.rps,
        s4xx: s.s4xx,
        s5xx: s.s5xx,
        err_pct: s.err_pct,
        p95: s.p95,
        bytes_out: s.bytes_out,
    }
}

/// Server-wide traffic for the window ending at `now`.
pub async fn overview(
    state: &AppState,
    range: requests::Range,
    now: i64,
) -> Result<TrafficOverviewResponse, AppError> {
    let from = range.from(now);
    let secs = now - from;
    let totals = requests::totals_by_target(&state.db, range, now).await?;
    let targets: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
        "SELECT t.id, t.kind, t.owner_id, a.name FROM metric_targets t
         LEFT JOIN apps a ON t.kind = 'app' AND a.id = t.owner_id
         WHERE t.kind IN ('app', 'preview', 'dashboard', 'unknown')",
    )
    .fetch_all(&state.db)
    .await?;

    let mut total = TrafficCounters::default();
    let mut unknown_requests = 0;
    let mut apps = Vec::new();
    for (target, kind, owner_id, name) in targets {
        let Some(c) = totals.get(&target) else {
            continue;
        };
        total.add(c);
        match (kind.as_str(), name) {
            ("unknown", _) => unknown_requests += c.requests,
            ("app", Some(name)) => apps.push(app_traffic(owner_id, name, c, secs)),
            _ => {}
        }
    }

    let mut busiest = apps.clone();
    busiest.sort_by(|a, b| b.requests.cmp(&a.requests).then(a.name.cmp(&b.name)));
    busiest.truncate(10);
    let mut errors: Vec<AppTrafficResponse> = apps.into_iter().filter(|a| a.s5xx > 0).collect();
    errors.sort_by(|a, b| b.s5xx.cmp(&a.s5xx).then(a.name.cmp(&b.name)));
    errors.truncate(5);

    Ok(TrafficOverviewResponse {
        range: range.as_str().to_string(),
        from,
        to: now,
        total: requests::summarize(&total, secs),
        unknown_requests,
        busiest,
        errors,
        proxy_connected: requests::proxy_connected(),
    })
}

pub async fn get_system_requests(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Query(query): Query<RequestMetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let range = parse_range(&query.range)?;
    let body = overview(&state, range, Utc::now().timestamp()).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bollard::{API_DEFAULT_VERSION, Docker};
    use tokio::sync::RwLock;

    use super::*;
    use crate::auth::RateLimiter;

    async fn state() -> AppState {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:1", 1, API_DEFAULT_VERSION)
                .unwrap_or_else(|e| panic!("docker: {e}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_app(state: &AppState, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, created_at, updated_at)
             VALUES (?, ?, 'git', 3000, 'running', datetime('now'), datetime('now'))",
        )
        .bind(id)
        .bind(name)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("seed app: {e}"));
    }

    fn traffic(requests: u64, s4xx: u64, s5xx: u64) -> TrafficCounters {
        let mut c = TrafficCounters {
            requests,
            status: [0, requests - s4xx - s5xx, 0, s4xx, s5xx],
            bytes_out: requests * 1000,
            ..Default::default()
        };
        c.latency[6] = requests;
        c
    }

    #[test]
    fn times_parse_in_both_stored_formats() {
        assert_eq!(parse_time("2026-01-01T00:00:10+00:00"), Some(1_767_225_610));
        assert_eq!(parse_time("2026-01-01 00:00:10"), Some(1_767_225_610));
        assert_eq!(parse_time("nope"), None);
        assert!(parse_range("2d").is_err());
    }

    #[tokio::test]
    async fn app_requests_combine_series_markers_and_summary() {
        let state = state().await;
        seed_app(&state, "a1", "web").await;
        let now = 2_000_000_000 / 60 * 60;
        let target = requests::ensure_target(&state.db, &(RouteKind::App, "a1".into()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        requests::write_minute(&state.db, now - 120, &[(target, traffic(120, 12, 6))], None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let finished = chrono::DateTime::from_timestamp(now - 100, 0)
            .unwrap_or_default()
            .to_rfc3339();
        sqlx::query(
            "INSERT INTO deployments (id, app_id, status, started_at, finished_at, created_at, kind)
             VALUES ('d1', 'a1', 'succeeded', ?1, ?1, ?1, 'deploy')",
        )
        .bind(&finished)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let container = store::ensure_target(&state.db, &Owner::new(TargetKind::App, "a1"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        store::record_event(
            &state.db,
            container,
            now - 50,
            store::EventKind::Restart,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));

        let r = requests_for(
            &state,
            &(RouteKind::App, "a1".into()),
            requests::Range::Hour,
            now,
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(r.range, "1h");
        assert_eq!(r.step, 60);
        assert_eq!(r.summary.requests, 120);
        assert_eq!(r.summary.s5xx, 6);
        assert_eq!(r.summary.err_pct, Some(5.0));
        assert_eq!(r.series.rps.iter().filter(|v| **v > 0.0).count(), 1);
        let kinds: Vec<&str> = r.markers.iter().map(|m| m.kind.as_str()).collect();
        assert_eq!(kinds, vec!["deploy", "restart"]);
        assert_eq!(r.markers[0].deployment_id.as_deref(), Some("d1"));

        // Unknown routes: an empty, zero-filled series.
        let empty = requests_for(
            &state,
            &(RouteKind::App, "zz".into()),
            requests::Range::Day,
            now,
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(empty.summary.requests, 0);
        assert!(empty.series.rps.iter().all(|v| *v == 0.0));
        assert!(empty.series.t.len() <= 481);
    }

    #[tokio::test]
    async fn the_overview_ranks_busy_and_failing_apps() {
        let state = state().await;
        seed_app(&state, "a1", "web").await;
        seed_app(&state, "a2", "api").await;
        seed_app(&state, "a3", "idle").await;
        let now = 2_000_000_000 / 60 * 60;
        let mut rows = Vec::new();
        for (kind, id, c) in [
            (RouteKind::App, "a1", traffic(100, 0, 1)),
            (RouteKind::App, "a2", traffic(500, 50, 0)),
            (RouteKind::Unknown, "", traffic(40, 40, 0)),
            (RouteKind::Dashboard, "", traffic(10, 0, 0)),
        ] {
            let t = requests::ensure_target(&state.db, &(kind, id.to_string()))
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            rows.push((t, c));
        }
        requests::ensure_target(&state.db, &(RouteKind::App, "a3".into()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        requests::write_minute(&state.db, now - 600, &rows, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        let o = overview(&state, requests::Range::Day, now)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(o.total.requests, 650);
        assert_eq!(o.unknown_requests, 40);
        let names: Vec<&str> = o.busiest.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["api", "web"], "idle apps are left out");
        assert_eq!(o.errors.len(), 1);
        assert_eq!(o.errors[0].name, "web");
        assert_eq!(o.errors[0].err_pct, Some(1.0));

        // Dashboard and unknown targets survive the orphan prune.
        store::prune(&state.db, now)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let o = overview(&state, requests::Range::Day, now)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(o.total.requests, 650);
    }
}
