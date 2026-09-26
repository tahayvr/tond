// alerts/sources.rs — data the alert rules read that other subsystems own
//
// Request metrics (5xx rate, p95 latency) come from the per-app request
// rollups the proxy's counters feed (`requests_1m`). The alert engine only
// needs "requests, 5xx and p95 per app over the last N seconds", so that is
// the whole interface: `request_windows`. It expects
//
//   requests_1m(app_id TEXT, ts INTEGER, requests INTEGER,
//               status_5xx INTEGER, p95_ms REAL)
//
// and reports "not available" (rules hold their state; creating such a rule
// is refused with a clear message) while that table or a column is missing.
// The window's p95 is approximated by the request-weighted mean of the
// per-minute p95 values.
//
// Disk-full forecast: a linear regression over the last 24 hours of host disk
// usage. The metrics collector hands the engine a disk reading every tick
// (`record_disk`); one point is kept per 5 minutes (at most 288 points, a few
// KiB) in memory, so a restart starts a fresh history.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use sqlx::SqlitePool;

/// Requests of one app within a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestWindow {
    pub requests: i64,
    pub errors_5xx: i64,
    pub p95_ms: Option<f64>,
}

/// Fewer requests than this in the window: the rate is not meaningful.
pub const MIN_REQUESTS: i64 = 20;

const REQUEST_COLUMNS: &[&str] = &["app_id", "ts", "requests", "status_5xx", "p95_ms"];

/// Whether per-app request metrics exist on this server.
pub async fn request_metrics_available(db: &SqlitePool) -> bool {
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('requests_1m')")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    REQUEST_COLUMNS
        .iter()
        .all(|c| cols.iter().any(|have| have == c))
}

/// Per-app request totals since `since` (unix seconds); None while request
/// metrics are not available.
pub async fn request_windows(
    db: &SqlitePool,
    since: i64,
) -> anyhow::Result<Option<HashMap<String, RequestWindow>>> {
    if !request_metrics_available(db).await {
        return Ok(None);
    }
    let rows: Vec<(String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT app_id, SUM(requests), SUM(status_5xx),
                SUM(p95_ms * requests) / NULLIF(SUM(CASE WHEN p95_ms IS NOT NULL THEN requests END), 0)
         FROM requests_1m WHERE ts >= ? GROUP BY app_id",
    )
    .bind(since)
    .fetch_all(db)
    .await?;
    Ok(Some(
        rows.into_iter()
            .map(|(app, requests, errors_5xx, p95_ms)| {
                (
                    app,
                    RequestWindow {
                        requests,
                        errors_5xx,
                        p95_ms,
                    },
                )
            })
            .collect(),
    ))
}

// ── Disk forecast ────────────────────────────────────────────────────────────

const DISK_STEP_SECS: i64 = 300;
const DISK_SPAN_SECS: i64 = 24 * 3600;
/// Need at least this much history before forecasting.
const DISK_MIN_SPAN_SECS: i64 = 3600;

/// (unix seconds, used bytes, total bytes)
static DISK: Mutex<VecDeque<(i64, f64, f64)>> = Mutex::new(VecDeque::new());

/// Remember a disk reading (called on every metrics tick; one point is kept
/// per 5 minutes).
pub fn record_disk(now: i64, used: u64, total: u64) {
    if total == 0 {
        return;
    }
    let mut ring = DISK.lock().unwrap_or_else(|e| e.into_inner());
    push_point(&mut ring, now, used as f64, total as f64);
}

fn push_point(ring: &mut VecDeque<(i64, f64, f64)>, now: i64, used: f64, total: f64) {
    if ring
        .back()
        .is_some_and(|(t, _, _)| now - t < DISK_STEP_SECS)
    {
        return;
    }
    ring.push_back((now, used, total));
    while ring
        .front()
        .is_some_and(|(t, _, _)| now - t > DISK_SPAN_SECS)
    {
        ring.pop_front();
    }
}

/// Forecast from the in-memory history.
pub fn disk_forecast() -> Forecast {
    let ring = DISK.lock().unwrap_or_else(|e| e.into_inner());
    forecast(ring.iter().copied())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Forecast {
    /// Not enough history yet.
    Unknown,
    /// Usage is flat or shrinking.
    NotFilling,
    /// Hours until the disk is full at the current rate.
    HoursLeft(f64),
}

/// Least-squares slope of used bytes over time, projected to the total.
pub fn forecast(points: impl Iterator<Item = (i64, f64, f64)>) -> Forecast {
    let pts: Vec<(i64, f64, f64)> = points.collect();
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
        return Forecast::Unknown;
    };
    if pts.len() < 6 || last.0 - first.0 < DISK_MIN_SPAN_SECS {
        return Forecast::Unknown;
    }
    let n = pts.len() as f64;
    let t0 = first.0;
    let mean_t = pts.iter().map(|p| (p.0 - t0) as f64).sum::<f64>() / n;
    let mean_u = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for p in &pts {
        let dt = (p.0 - t0) as f64 - mean_t;
        num += dt * (p.1 - mean_u);
        den += dt * dt;
    }
    if den <= 0.0 {
        return Forecast::Unknown;
    }
    let slope = num / den; // bytes per second
    // Below ~1 MiB/day is noise, not a trend.
    if slope <= 1_048_576.0 / 86_400.0 {
        return Forecast::NotFilling;
    }
    let free = (last.2 - last.1).max(0.0);
    Forecast::HoursLeft(free / slope / 3600.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    #[test]
    fn steady_growth_is_projected_to_full() {
        // 100 GiB disk, 50 GiB used, +1 GiB per hour for 6 hours.
        let pts = (0..=72).map(|i| (i * 300, 50.0 * GIB + i as f64 * GIB / 12.0, 100.0 * GIB));
        match forecast(pts) {
            Forecast::HoursLeft(h) => assert!((h - 44.0).abs() < 0.5, "{h}"),
            other => panic!("expected a forecast, got {other:?}"),
        }
    }

    #[test]
    fn flat_or_short_histories_do_not_forecast() {
        let flat = (0..=72).map(|i| (i * 300, 50.0 * GIB, 100.0 * GIB));
        assert_eq!(forecast(flat), Forecast::NotFilling);
        let short = (0..5).map(|i| (i * 300, i as f64 * GIB, 100.0 * GIB));
        assert_eq!(forecast(short), Forecast::Unknown);
        assert_eq!(forecast(std::iter::empty()), Forecast::Unknown);
    }

    #[test]
    fn the_ring_keeps_one_point_per_step_for_a_day() {
        let mut ring = VecDeque::new();
        for t in (0..(2 * 86_400)).step_by(30) {
            push_point(&mut ring, t, 1.0, 2.0);
        }
        assert!(ring.len() <= 289, "{}", ring.len());
        let span = ring.back().map(|p| p.0).unwrap_or(0) - ring.front().map(|p| p.0).unwrap_or(0);
        assert!(span <= DISK_SPAN_SECS);
    }

    #[tokio::test]
    async fn request_windows_read_the_rollup_table_when_it_exists() {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("db: {err}"));
        assert!(!request_metrics_available(&db).await);
        assert_eq!(
            request_windows(&db, 0)
                .await
                .unwrap_or_else(|err| panic!("query: {err}")),
            None
        );
        sqlx::query(
            "CREATE TABLE requests_1m(app_id TEXT, ts INTEGER, requests INTEGER,
                                      status_5xx INTEGER, p95_ms REAL)",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("create: {err}"));
        for (app, ts, req, e, p95) in [
            ("a", 60, 100, 10, 200.0),
            ("a", 120, 300, 0, 400.0),
            ("a", 0, 999, 999, 9999.0), // outside the window
            ("b", 120, 5, 5, 50.0),
        ] {
            sqlx::query("INSERT INTO requests_1m VALUES (?, ?, ?, ?, ?)")
                .bind(app)
                .bind(ts)
                .bind(req)
                .bind(e)
                .bind(p95)
                .execute(&db)
                .await
                .unwrap_or_else(|err| panic!("insert: {err}"));
        }
        let w = request_windows(&db, 60)
            .await
            .unwrap_or_else(|err| panic!("query: {err}"))
            .unwrap_or_else(|| panic!("available"));
        let a = w.get("a").copied().unwrap_or_else(|| panic!("app a"));
        assert_eq!((a.requests, a.errors_5xx), (400, 10));
        assert_eq!(a.p95_ms, Some(350.0), "request-weighted");
        assert_eq!(w.get("b").map(|b| b.requests), Some(5));
    }
}
