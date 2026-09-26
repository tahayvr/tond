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
// Disk-full forecast: the host history's persisted trend
// (`metrics::host::disk_full_eta`, least squares over 24 h and 7 d of
// minute/hour rows), so it survives restarts; the sooner ETA counts.

use std::collections::HashMap;
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
