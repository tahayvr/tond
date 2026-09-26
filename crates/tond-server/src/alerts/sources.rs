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
// Disk-full forecast: the host history's persisted trend
// (`metrics::host::disk_full_eta`, least squares over 24 h and 7 d of
// minute/hour rows), so it survives restarts; the sooner ETA counts.

use std::collections::HashMap;
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

/// How long a computed forecast is reused (the history moves once a minute;
/// rules are evaluated every 30 s).
const FORECAST_TTL_SECS: i64 = 300;

static FORECAST: Mutex<Option<(i64, Forecast)>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Forecast {
    /// Not enough history yet.
    Unknown,
    /// Usage is flat or shrinking.
    NotFilling,
    /// Hours until the disk is full at the current rate.
    HoursLeft(f64),
}

/// Turn the host history's forecast into what the rule compares.
pub fn from_eta(eta: &crate::metrics::host::DiskEta) -> Forecast {
    match eta.eta_secs {
        Some(secs) => Forecast::HoursLeft(secs as f64 / 3600.0),
        None if eta.day.status == "stable" || eta.week.status == "stable" => Forecast::NotFilling,
        None => Forecast::Unknown,
    }
}

/// Disk-full forecast from the persisted host history
/// (`metrics::host::disk_full_eta`: 24 h and 7 d trends, survives restarts),
/// cached for a few minutes.
pub async fn disk_forecast(db: &SqlitePool, now: i64) -> Forecast {
    let cached = *FORECAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, f)) = cached
        && now - at < FORECAST_TTL_SECS
    {
        return f;
    }
    let f = match crate::metrics::host::disk_full_eta(db, now).await {
        Ok(eta) => from_eta(&eta),
        Err(e) => {
            tracing::debug!("alerts: disk forecast unavailable: {e}");
            Forecast::Unknown
        }
    };
    *FORECAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, f));
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forecasts_map_from_the_host_history() {
        use crate::metrics::host_history::{DiskEta, DiskForecast};
        let w = |window, status, eta_secs| DiskForecast {
            window,
            status,
            growth_per_day: 0.0,
            eta_secs,
            samples: 100,
        };
        let eta = |day, week, eta_secs| DiskEta {
            used: 1,
            total: 2,
            day,
            week,
            eta_secs,
        };
        assert_eq!(
            from_eta(&eta(
                w("24h", "filling", Some(7200)),
                w("7d", "stable", None),
                Some(7200)
            )),
            Forecast::HoursLeft(2.0)
        );
        assert_eq!(
            from_eta(&eta(
                w("24h", "stable", None),
                w("7d", "insufficient_data", None),
                None
            )),
            Forecast::NotFilling
        );
        assert_eq!(
            from_eta(&eta(
                w("24h", "insufficient_data", None),
                w("7d", "insufficient_data", None),
                None
            )),
            Forecast::Unknown
        );
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
