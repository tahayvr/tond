// handlers/metrics.rs — resource metric endpoints
//
// Routes:
//   GET /api/apps/{id}/metrics?range=1h|24h|7d|30d
//   GET /api/databases/{id}/metrics?range=…
//   GET /api/cron/{id}/metrics?range=…
//     Downsampled series (≤ 480 buckets, rates computed server-side), the
//     restarts / crashes / OOM kills in the window and the latest sample.
//     `{id}` may be the id or the name. Compose apps are the sum of their
//     services.
//
//   GET /api/system/metrics
//     Host CPU / memory / disk (the filesystem holding the data directory)
//     from the collector's cache — no sampling or sleeping in the request.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{TimeZone, Utc};
use sqlx::AssertSqlSafe;
use tond_common::*;

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    metrics::{self, Owner, TargetKind, host, store},
    state::AppState,
};

/// Build the metrics response for one owner.
pub async fn metrics_for(
    state: &AppState,
    owner: &Owner,
    range: store::Range,
    now: i64,
) -> Result<MetricsResponse, AppError> {
    let step = range.step();
    let from = (now - range.span()) / step * step;
    let Some((target, mem_limit)) = store::find_target(&state.db, owner).await? else {
        return Ok(MetricsResponse {
            range: range.as_str().to_string(),
            step,
            from,
            to: now,
            mem_limit: None,
            series: MetricSeries::default(),
            events: Vec::new(),
            current: None,
        });
    };
    let (from, series) = store::query_series(&state.db, target, range, now).await?;
    let events = store::query_events(&state.db, target, from).await?;

    // Latest sample: the in-memory one (has the memory peak), else the DB's
    // if it is fresh (the server restarted since).
    let fresh_since = now - 3 * metrics::TICK.as_secs() as i64;
    let current = match metrics::current(owner).filter(|c| c.ts >= fresh_since) {
        Some(c) => Some((c.ts, c.sample, c.mem_peak)),
        None => store::latest_sample(&state.db, target, fresh_since)
            .await?
            .map(|(ts, s)| (ts, s, None)),
    }
    .map(|(t, s, mem_peak)| MetricCurrentResponse {
        t,
        cpu: store::centi_to_percent(s.cpu as f64),
        mem: s.mem,
        mem_peak,
        rx: s.rx,
        tx: s.tx,
        pids: s.pids,
    });

    Ok(MetricsResponse {
        range: range.as_str().to_string(),
        step,
        from,
        to: now,
        mem_limit: (mem_limit > 0).then_some(mem_limit),
        series,
        events,
        current,
    })
}

fn parse_range(query: &MetricsQuery) -> Result<store::Range, AppError> {
    store::Range::parse(&query.range).ok_or_else(|| {
        AppError(TondError::InvalidInput(format!(
            "range must be one of 1h, 24h, 7d, 30d (got '{}')",
            query.range
        )))
    })
}

async fn respond(
    state: &AppState,
    owner: Owner,
    query: &MetricsQuery,
) -> Result<impl IntoResponse + use<>, AppError> {
    let range = parse_range(query)?;
    let body = metrics_for(state, &owner, range, Utc::now().timestamp()).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(body))))
}

/// Resolve `id_or_name` in `table` (a fixed table name) to its id.
async fn lookup(state: &AppState, table: &str, id_or_name: &str) -> sqlx::Result<Option<String>> {
    let sql = format!("SELECT id FROM {table} WHERE id = ? OR name = ? LIMIT 1");
    sqlx::query_scalar(AssertSqlSafe(sql.as_str()))
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(&state.db)
        .await
}

pub async fn get_app_metrics(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let app_id = lookup(&state, "apps", &id)
        .await?
        .ok_or_else(|| AppError(TondError::AppNotFound(id.clone())))?;
    respond(&state, Owner::new(TargetKind::App, app_id), &query).await
}

pub async fn get_database_metrics(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let db_id = lookup(&state, "databases", &id)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("database '{id}'"))))?;
    respond(&state, Owner::new(TargetKind::Database, db_id), &query).await
}

pub async fn get_cron_metrics(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let job_id = lookup(&state, "cron_jobs", &id)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("cron job '{id}'"))))?;
    respond(&state, Owner::new(TargetKind::Cron, job_id), &query).await
}

// ── System metrics ──

/// Build the system snapshot from the collector's cache.
pub async fn system_metrics(state: &AppState) -> SystemMetrics {
    // Before the collector's first tick (a few seconds after startup), take a
    // one-off sample: memory and disk are exact, CPU reads 0.
    let snap = match host::snapshot() {
        Some(s) => s,
        None => host::HostSampler::new(crate::auth::secret_file::data_dir())
            .sample(Utc::now().timestamp()),
    };
    let (apps_total, apps_running): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(status = 'running'), 0) FROM apps")
            .fetch_one(&state.db)
            .await
            .unwrap_or((0, 0));
    SystemMetrics {
        cpu_percent: (snap.cpu_percent * 10.0).round() / 10.0,
        mem_used: snap.mem_used as i64,
        mem_total: snap.mem_total as i64,
        mem_percent: snap.mem_percent(),
        disk_used: snap.disk_used as i64,
        disk_total: snap.disk_total as i64,
        disk_percent: snap.disk_percent(),
        disk_mount: snap.disk_mount.clone(),
        containers_running: metrics::containers_running(),
        apps_total,
        apps_running,
        collected_at: Utc
            .timestamp_opt(snap.collected_at, 0)
            .single()
            .unwrap_or_else(Utc::now)
            .to_rfc3339(),
    }
}

pub async fn get_system_metrics(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let body = system_metrics(&state).await;
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

    #[tokio::test]
    async fn unknown_targets_get_an_empty_series() {
        let state = state().await;
        let owner = Owner::new(TargetKind::App, "nope");
        let r = metrics_for(&state, &owner, store::Range::Day, 1_000_000)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(r.range, "24h");
        assert_eq!(r.step, 180);
        assert!(r.series.t.is_empty() && r.events.is_empty() && r.current.is_none());
    }

    #[tokio::test]
    async fn series_events_limit_and_current_come_from_storage() {
        let state = state().await;
        let owner = Owner::new(TargetKind::Database, "db-1");
        let target = store::ensure_target(&state.db, &owner)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let now = 2_000_000_000 / 30 * 30;
        store::insert_tick(
            &state.db,
            now - 30,
            &[store::TickRow {
                target,
                sample: metrics::Sample {
                    cpu: 12_345,
                    mem: 1_000,
                    rx: 10,
                    tx: 20,
                    pids: 4,
                },
                mem_limit: Some(4_096),
            }],
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        store::record_event(&state.db, target, now - 10, store::EventKind::Oom, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        let r = metrics_for(&state, &owner, store::Range::Hour, now)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(r.mem_limit, Some(4_096));
        assert_eq!(r.series.cpu, vec![123.5], "percent of one core, 1 decimal");
        assert_eq!(r.series.mem, vec![1_000]);
        assert_eq!(r.events.len(), 1);
        let current = r.current.unwrap_or_else(|| panic!("fresh sample expected"));
        assert_eq!((current.t, current.cpu, current.pids), (now - 30, 123.5, 4));
    }

    #[test]
    fn bad_ranges_are_rejected_with_the_allowed_values() {
        let q = MetricsQuery {
            range: "1y".to_string(),
        };
        let err = parse_range(&q).err().map(|e| e.0.to_string());
        assert!(err.is_some_and(|e| e.contains("1h, 24h, 7d, 30d")));
    }

    #[tokio::test]
    async fn system_metrics_reads_the_cache() {
        let state = state().await;
        let m = system_metrics(&state).await;
        assert!(m.mem_total > 0);
        assert!(m.disk_total > 0);
        assert!(!m.disk_mount.is_empty());
        assert_eq!(m.apps_total, 0);
    }
}
