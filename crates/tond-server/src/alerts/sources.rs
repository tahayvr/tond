// alerts/sources.rs — data the alert rules read that other subsystems own
//
// Request metrics (5xx rate, p95 latency) come from the per-app request
// rows the proxy's counters feed (metrics/requests.rs: `requests_1m`, keyed
// by metric_targets of kind 'app'). The alert engine only needs "requests,
// 5xx and p95 per app over the last N seconds", so that is the whole
// interface: `request_windows`. The window's p95 comes from the summed
// latency histograms; the minute the collector has not written yet is
// included, so a rule sees traffic from the tick it runs on.
//
// Disk-full forecast: a linear regression over the last 24 hours of host disk
// usage. The metrics collector hands the engine a disk reading every tick
// (`record_disk`); one point is kept per 5 minutes (at most 288 points, a few
// KiB) in memory, so a restart starts a fresh history.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use sqlx::SqlitePool;
use tond_common::traffic::{RouteKind, TrafficCounters, decode_histogram, percentile};

use crate::metrics::requests;

/// Requests of one app within a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestWindow {
    pub requests: i64,
    pub errors_5xx: i64,
    pub p95_ms: Option<f64>,
}

/// Fewer requests than this in the window: the rate is not meaningful.
pub const MIN_REQUESTS: i64 = 20;

/// Requests, 5xx and p95 per app id since `since` (unix seconds). Always
/// Some: request metrics are collected on every server (the Option is kept
/// for callers that treat "no data source" separately).
pub async fn request_windows(
    db: &SqlitePool,
    since: i64,
) -> anyhow::Result<Option<HashMap<String, RequestWindow>>> {
    let rows: Vec<(String, i64, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT t.owner_id, r.requests, r.s5xx, r.latency
         FROM requests_1m r JOIN metric_targets t ON t.id = r.target
         WHERE t.kind = 'app' AND r.ts >= ?",
    )
    .bind(since)
    .fetch_all(db)
    .await?;
    let mut sums: HashMap<String, TrafficCounters> = HashMap::new();
    for (app, requests, s5xx, latency) in rows {
        let c = sums.entry(app).or_default();
        c.requests += u64::try_from(requests).unwrap_or(0);
        c.status[4] += u64::try_from(s5xx).unwrap_or(0);
        let hist = decode_histogram(&latency);
        for (sum, v) in c.latency.iter_mut().zip(hist) {
            *sum += v;
        }
    }
    if let Some((minute, pending)) = requests::pending_all()
        && minute >= since
    {
        for ((kind, id), c) in pending {
            if kind == RouteKind::App {
                sums.entry(id).or_default().add(&c);
            }
        }
    }
    Ok(Some(
        sums.into_iter()
            .map(|(app, c)| {
                (
                    app,
                    RequestWindow {
                        requests: i64::try_from(c.requests).unwrap_or(i64::MAX),
                        errors_5xx: i64::try_from(c.status[4]).unwrap_or(i64::MAX),
                        p95_ms: percentile(&c.latency, 0.95),
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
    async fn request_windows_sum_apps_and_merge_latency_histograms() {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("db: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        let target = |kind: RouteKind, id: &str| {
            let db = db.clone();
            let id = id.to_string();
            async move {
                requests::ensure_target(&db, &(kind, id))
                    .await
                    .unwrap_or_else(|err| panic!("target: {err}"))
            }
        };
        let a = target(RouteKind::App, "a").await;
        let b = target(RouteKind::App, "b").await;
        let preview = target(RouteKind::Preview, "p").await;
        let counters = |requests: u64, s5xx: u64, bucket: usize| {
            let mut c = TrafficCounters {
                requests,
                status: [0, requests - s5xx, 0, 0, s5xx],
                ..Default::default()
            };
            c.latency[bucket] = requests;
            c
        };
        for (ts, rows) in [
            (
                60,
                vec![(a, counters(100, 10, 12)), (preview, counters(50, 50, 0))],
            ),
            (120, vec![(a, counters(300, 0, 14)), (b, counters(5, 5, 3))]),
            (0, vec![(a, counters(999, 999, 19))]), // outside the window
        ] {
            requests::write_minute(&db, ts, &rows, None)
                .await
                .unwrap_or_else(|err| panic!("write: {err}"));
        }
        let w = request_windows(&db, 60)
            .await
            .unwrap_or_else(|err| panic!("query: {err}"))
            .unwrap_or_else(|| panic!("available"));
        let app_a = w.get("a").copied().unwrap_or_else(|| panic!("app a"));
        assert_eq!((app_a.requests, app_a.errors_5xx), (400, 10));
        // 300 of 400 requests in (400, 600] ms: p95 falls in that bucket.
        assert!(
            app_a.p95_ms.is_some_and(|p| p > 400.0 && p <= 600.0),
            "{:?}",
            app_a.p95_ms
        );
        assert_eq!(w.get("b").map(|b| b.requests), Some(5));
        assert!(!w.contains_key("p"), "previews are not apps");
        assert!(
            request_windows(&db, 10_000)
                .await
                .ok()
                .flatten()
                .is_some_and(|w| w.is_empty())
        );
    }
}
