// metrics/host_history.rs — host history over time and the disk-full forecast
//
// The metrics collector samples the host every 30 s (host.rs). HostHistory
// folds those samples into one row a minute (CPU averaged and its maximum
// kept; every other number is the minute's last reading) and writes it to
// host_1m when the minute closes — one INSERT a minute, in the collector's
// existing tick, no extra wakeup. When an hour closes the hour is rolled up
// into host_1h. Retention (janitor): host_1m 48 h, host_1h
// `retention_host_history_days` (default 1 year).
//
// Queries return ≤ 480 points: 1h / 24h from host_1m, 7d / 30d from host_1h.
//
// Disk-full forecast (`disk_full_eta`): a least-squares line through the data
// directory filesystem's used bytes over the last 24 h (host_1m) and 7 days
// (host_1h). A rising line gives an ETA ("full in ~9 days"); a flat or
// falling one is "not filling". The alert engine calls
// `host::disk_full_eta(&db, now)` and reads `DiskEta::eta_secs` (the sooner
// of the two ETAs, None when neither window is filling).

use serde::Serialize;
use sqlx::{AssertSqlSafe, SqlitePool};

use super::host::HostSnapshot;

pub const M1_KEEP_SECS: i64 = 48 * 3600;
#[cfg(test)]
const H1_KEEP_SECS: i64 = 365 * 86_400;
/// Longest series a query returns.
const MAX_POINTS: i64 = 480;
/// Slower growth than this is noise, not "filling" (bytes per day).
const MIN_GROWTH_PER_DAY: f64 = 16.0 * 1024.0 * 1024.0;
/// ETAs further out than this are reported as "not filling".
const MAX_ETA_SECS: i64 = 365 * 86_400;

/// One stored row (host_1m and host_1h share the shape). Percentages and
/// load averages × 100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct HostRow {
    pub ts: i64,
    pub cpu: i64,
    pub cpu_max: i64,
    pub load1: i64,
    pub load5: i64,
    pub load15: i64,
    pub mem_used: i64,
    pub mem_total: i64,
    pub swap_used: i64,
    pub swap_total: i64,
    pub disk_used: i64,
    pub disk_total: i64,
    pub inodes_used: i64,
    pub inodes_total: i64,
    pub root_used: i64,
    pub root_total: i64,
    pub root_inodes_used: i64,
    pub root_inodes_total: i64,
}

const COLUMNS: &str = "ts, cpu, cpu_max, load1, load5, load15, mem_used, mem_total, swap_used, \
     swap_total, disk_used, disk_total, inodes_used, inodes_total, root_used, root_total, \
     root_inodes_used, root_inodes_total";

fn centi(v: f64) -> i64 {
    (v * 100.0).round() as i64
}

fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl HostRow {
    fn from_snapshot(ts: i64, cpu_avg: f64, cpu_max: f64, s: &HostSnapshot) -> Self {
        let root = s.root_fs();
        Self {
            ts,
            cpu: centi(cpu_avg),
            cpu_max: centi(cpu_max),
            load1: centi(s.load1),
            load5: centi(s.load5),
            load15: centi(s.load15),
            mem_used: int(s.mem_used),
            mem_total: int(s.mem_total),
            swap_used: int(s.swap_used),
            swap_total: int(s.swap_total),
            disk_used: int(s.disk_used),
            disk_total: int(s.disk_total),
            inodes_used: int(s.inodes_used),
            inodes_total: int(s.inodes_total),
            root_used: int(root.used),
            root_total: int(root.total),
            root_inodes_used: int(root.inodes_used),
            root_inodes_total: int(root.inodes_total),
        }
    }
}

/// Folds 30 s samples into minute rows.
#[derive(Debug, Default)]
pub struct MinuteAggregator {
    bucket: Option<i64>,
    samples: u32,
    cpu_sum: f64,
    cpu_max: f64,
    last: Option<HostSnapshot>,
}

impl MinuteAggregator {
    /// Add a sample; returns the previous minute's row when this sample
    /// starts a new minute.
    pub fn push(&mut self, s: &HostSnapshot) -> Option<HostRow> {
        let bucket = s.collected_at / 60 * 60;
        let closed = match (self.bucket, &self.last) {
            (Some(b), Some(last)) if b != bucket && self.samples > 0 => {
                Some(HostRow::from_snapshot(
                    b,
                    self.cpu_sum / f64::from(self.samples),
                    self.cpu_max,
                    last,
                ))
            }
            _ => None,
        };
        if self.bucket != Some(bucket) {
            self.bucket = Some(bucket);
            self.samples = 0;
            self.cpu_sum = 0.0;
            self.cpu_max = 0.0;
        }
        self.samples += 1;
        self.cpu_sum += s.cpu_percent;
        self.cpu_max = self.cpu_max.max(s.cpu_percent);
        self.last = Some(s.clone());
        closed
    }
}

/// The collector's host-history state: minute aggregation and the hourly
/// rollup cursor.
#[derive(Debug, Default)]
pub struct HostHistory {
    minutes: MinuteAggregator,
    /// Start of the next hour to roll up.
    next_1h: Option<i64>,
}

impl HostHistory {
    /// Record one sample: writes a minute row when a minute closes and rolls
    /// up hours as they close.
    pub async fn record(&mut self, db: &SqlitePool, s: &HostSnapshot) -> sqlx::Result<()> {
        let Some(row) = self.minutes.push(s) else {
            return Ok(());
        };
        insert(db, "host_1m", &row).await?;
        let hour = s.collected_at / 3600 * 3600;
        let next = match self.next_1h {
            Some(n) => n,
            None => latest(db, "host_1h")
                .await?
                .map(|t| t + 3600)
                .unwrap_or((s.collected_at - M1_KEEP_SECS) / 3600 * 3600),
        };
        if hour > next {
            rollup_1h(db, next, hour).await?;
        }
        self.next_1h = Some(hour.max(next));
        Ok(())
    }
}

async fn insert(db: &SqlitePool, table: &str, r: &HostRow) -> sqlx::Result<()> {
    let sql = format!(
        "INSERT OR REPLACE INTO {table} ({COLUMNS})
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    );
    sqlx::query(AssertSqlSafe(sql.as_str()))
        .bind(r.ts)
        .bind(r.cpu)
        .bind(r.cpu_max)
        .bind(r.load1)
        .bind(r.load5)
        .bind(r.load15)
        .bind(r.mem_used)
        .bind(r.mem_total)
        .bind(r.swap_used)
        .bind(r.swap_total)
        .bind(r.disk_used)
        .bind(r.disk_total)
        .bind(r.inodes_used)
        .bind(r.inodes_total)
        .bind(r.root_used)
        .bind(r.root_total)
        .bind(r.root_inodes_used)
        .bind(r.root_inodes_total)
        .execute(db)
        .await?;
    Ok(())
}

async fn latest(db: &SqlitePool, table: &str) -> sqlx::Result<Option<i64>> {
    let sql = format!("SELECT MAX(ts) FROM {table}");
    sqlx::query_scalar(AssertSqlSafe(sql.as_str()))
        .fetch_one(db)
        .await
}

/// Roll the minutes in [from, to) up into hours (re-running is harmless).
pub async fn rollup_1h(db: &SqlitePool, from: i64, to: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO host_1h (ts, cpu, cpu_max, load1, load5, load15, mem_used,
            mem_total, swap_used, swap_total, disk_used, disk_total, inodes_used, inodes_total,
            root_used, root_total, root_inodes_used, root_inodes_total)
         SELECT ts / 3600 * 3600 AS h, CAST(AVG(cpu) AS INTEGER), MAX(cpu_max),
            CAST(AVG(load1) AS INTEGER), CAST(AVG(load5) AS INTEGER), CAST(AVG(load15) AS INTEGER),
            CAST(AVG(mem_used) AS INTEGER), MAX(mem_total),
            CAST(AVG(swap_used) AS INTEGER), MAX(swap_total),
            CAST(AVG(disk_used) AS INTEGER), MAX(disk_total),
            CAST(AVG(inodes_used) AS INTEGER), MAX(inodes_total),
            CAST(AVG(root_used) AS INTEGER), MAX(root_total),
            CAST(AVG(root_inodes_used) AS INTEGER), MAX(root_inodes_total)
         FROM host_1m WHERE ts >= ? AND ts < ? GROUP BY h",
    )
    .bind(from)
    .bind(to)
    .execute(db)
    .await?;
    Ok(())
}

/// Drop rows past retention (`h1_keep_secs`: hourly rows, the
/// `retention_host_history_days` setting). Returns the number of rows deleted.
pub async fn prune(db: &SqlitePool, now: i64, h1_keep_secs: i64) -> sqlx::Result<u64> {
    let mut deleted = sqlx::query("DELETE FROM host_1m WHERE ts < ?")
        .bind(now - M1_KEEP_SECS)
        .execute(db)
        .await?
        .rows_affected();
    deleted += sqlx::query("DELETE FROM host_1h WHERE ts < ?")
        .bind(now.saturating_sub(h1_keep_secs))
        .execute(db)
        .await?
        .rows_affected();
    Ok(deleted)
}

// ── Queries ──

/// A supported history window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRange {
    Hour,
    Day,
    Week,
    Month,
}

impl HistoryRange {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1h" => Some(Self::Hour),
            "24h" => Some(Self::Day),
            "7d" => Some(Self::Week),
            "30d" => Some(Self::Month),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    pub fn span(self) -> i64 {
        match self {
            Self::Hour => 3600,
            Self::Day => 86_400,
            Self::Week => 7 * 86_400,
            Self::Month => 30 * 86_400,
        }
    }

    fn source(self) -> (&'static str, i64) {
        match self {
            Self::Hour | Self::Day => ("host_1m", 60),
            Self::Week | Self::Month => ("host_1h", 3600),
        }
    }

    /// Bucket width: a multiple of the source resolution giving ≤ MAX_POINTS.
    pub fn step(self) -> i64 {
        let (_, native) = self.source();
        let per_point = (self.span() + MAX_POINTS - 1) / MAX_POINTS;
        ((per_point + native - 1) / native).max(1) * native
    }
}

/// Parallel arrays, one entry per bucket. Percentages and loads as floats.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct HostSeries {
    pub t: Vec<i64>,
    pub cpu: Vec<f64>,
    pub cpu_max: Vec<f64>,
    pub load1: Vec<f64>,
    pub load5: Vec<f64>,
    pub load15: Vec<f64>,
    pub mem_used: Vec<i64>,
    pub mem_total: Vec<i64>,
    pub swap_used: Vec<i64>,
    pub swap_total: Vec<i64>,
    pub disk_used: Vec<i64>,
    pub disk_total: Vec<i64>,
    pub inodes_used: Vec<i64>,
    pub inodes_total: Vec<i64>,
    pub root_used: Vec<i64>,
    pub root_total: Vec<i64>,
    pub root_inodes_used: Vec<i64>,
    pub root_inodes_total: Vec<i64>,
}

/// Downsampled series over `range` ending at `now`. Returns (from, step, series).
pub async fn query(
    db: &SqlitePool,
    range: HistoryRange,
    now: i64,
) -> sqlx::Result<(i64, i64, HostSeries)> {
    let step = range.step();
    let from = (now - range.span()) / step * step;
    let (table, _) = range.source();
    let sql = format!(
        // The bucket is named `ts` for FromRow; WHERE still sees the column.
        "SELECT (ts / ?1) * ?1 AS ts, CAST(AVG(cpu) AS INTEGER) AS cpu, MAX(cpu_max) AS cpu_max,
            CAST(AVG(load1) AS INTEGER) AS load1, CAST(AVG(load5) AS INTEGER) AS load5,
            CAST(AVG(load15) AS INTEGER) AS load15,
            CAST(AVG(mem_used) AS INTEGER) AS mem_used, MAX(mem_total) AS mem_total,
            CAST(AVG(swap_used) AS INTEGER) AS swap_used, MAX(swap_total) AS swap_total,
            MAX(disk_used) AS disk_used, MAX(disk_total) AS disk_total,
            MAX(inodes_used) AS inodes_used, MAX(inodes_total) AS inodes_total,
            MAX(root_used) AS root_used, MAX(root_total) AS root_total,
            MAX(root_inodes_used) AS root_inodes_used, MAX(root_inodes_total) AS root_inodes_total
         FROM {table} WHERE ts >= ?2 GROUP BY 1 ORDER BY 1"
    );
    let rows: Vec<HostRow> = sqlx::query_as(AssertSqlSafe(sql.as_str()))
        .bind(step)
        .bind(from)
        .fetch_all(db)
        .await?;
    // Each column is allocated once at its final size (up to 480 points).
    let ints = |f: fn(&HostRow) -> i64| rows.iter().map(f).collect::<Vec<_>>();
    let centi =
        |f: fn(&HostRow) -> i64| rows.iter().map(|r| f(r) as f64 / 100.0).collect::<Vec<_>>();
    let s = HostSeries {
        t: ints(|r| r.ts),
        cpu: centi(|r| r.cpu),
        cpu_max: centi(|r| r.cpu_max),
        load1: centi(|r| r.load1),
        load5: centi(|r| r.load5),
        load15: centi(|r| r.load15),
        mem_used: ints(|r| r.mem_used),
        mem_total: ints(|r| r.mem_total),
        swap_used: ints(|r| r.swap_used),
        swap_total: ints(|r| r.swap_total),
        disk_used: ints(|r| r.disk_used),
        disk_total: ints(|r| r.disk_total),
        inodes_used: ints(|r| r.inodes_used),
        inodes_total: ints(|r| r.inodes_total),
        root_used: ints(|r| r.root_used),
        root_total: ints(|r| r.root_total),
        root_inodes_used: ints(|r| r.root_inodes_used),
        root_inodes_total: ints(|r| r.root_inodes_total),
    };
    Ok((from, step, s))
}

// ── Disk-full forecast ──

/// The trend of one window.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct DiskForecast {
    /// "24h" | "7d"
    pub window: &'static str,
    /// "filling" | "stable" | "insufficient_data"
    pub status: &'static str,
    /// Growth of used space, bytes per day (negative when shrinking).
    pub growth_per_day: f64,
    /// Seconds until full at this rate (only when filling).
    pub eta_secs: Option<i64>,
    /// Points the line was fitted to.
    pub samples: usize,
}

/// Both windows plus the headline ETA.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct DiskEta {
    pub used: i64,
    pub total: i64,
    pub day: DiskForecast,
    pub week: DiskForecast,
    /// The sooner of the two ETAs; None when neither window is filling.
    pub eta_secs: Option<i64>,
}

/// Least-squares slope (bytes per second) of `points` (ts, used).
fn slope(points: &[(i64, i64)]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let t0 = points[0].0;
    let n = points.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (t, y) in points {
        let x = (t - t0) as f64;
        let y = *y as f64;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < f64::EPSILON {
        return None;
    }
    Some((n * sxy - sx * sy) / denom)
}

/// Fit a line through `points` and project when `used` reaches `total`.
/// `min_span` is how much time the points must cover to say anything.
pub fn forecast(
    window: &'static str,
    points: &[(i64, i64)],
    used: i64,
    total: i64,
    min_span: i64,
) -> DiskForecast {
    let span = match (points.first(), points.last()) {
        (Some(a), Some(b)) => b.0 - a.0,
        _ => 0,
    };
    let insufficient = DiskForecast {
        window,
        status: "insufficient_data",
        growth_per_day: 0.0,
        eta_secs: None,
        samples: points.len(),
    };
    if span < min_span {
        return insufficient;
    }
    let Some(per_sec) = slope(points) else {
        return insufficient;
    };
    let per_day = per_sec * 86_400.0;
    let free = (total - used).max(0) as f64;
    let eta = (per_day >= MIN_GROWTH_PER_DAY)
        .then(|| (free / per_sec).round() as i64)
        .filter(|eta| *eta <= MAX_ETA_SECS);
    DiskForecast {
        window,
        status: if eta.is_some() { "filling" } else { "stable" },
        growth_per_day: per_day.round(),
        eta_secs: eta,
        samples: points.len(),
    }
}

/// Disk-full forecast for the data directory's filesystem, from the stored
/// history. For the alert engine: `disk_full_eta(&db, now).await?.eta_secs`.
pub async fn disk_full_eta(db: &SqlitePool, now: i64) -> sqlx::Result<DiskEta> {
    let day: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT ts, disk_used, disk_total FROM host_1m WHERE ts >= ? ORDER BY ts")
            .bind(now - 86_400)
            .fetch_all(db)
            .await?;
    let week: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT ts, disk_used, disk_total FROM host_1h WHERE ts >= ? ORDER BY ts")
            .bind(now - 7 * 86_400)
            .fetch_all(db)
            .await?;
    // Current numbers: the live sample, else the newest stored row.
    let (used, total) = match super::host::snapshot() {
        Some(s) if s.disk_total > 0 => (int(s.disk_used), int(s.disk_total)),
        _ => day
            .last()
            .or(week.last())
            .map(|r| (r.1, r.2))
            .unwrap_or((0, 0)),
    };
    let pts = |rows: &[(i64, i64, i64)]| rows.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>();
    let day = forecast("24h", &pts(&day), used, total, 2 * 3600);
    let week = forecast("7d", &pts(&week), used, total, 2 * 86_400);
    let eta_secs = [day.eta_secs, week.eta_secs].into_iter().flatten().min();
    Ok(DiskEta {
        used,
        total,
        day,
        week,
        eta_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    fn snap(ts: i64, cpu: f64, disk_used: u64) -> HostSnapshot {
        HostSnapshot {
            cpu_percent: cpu,
            load1: 0.5,
            mem_used: 100,
            mem_total: 1000,
            disk_used,
            disk_total: 10_000,
            inodes_used: 5,
            inodes_total: 50,
            collected_at: ts,
            ..Default::default()
        }
    }

    #[test]
    fn minutes_average_cpu_keep_the_max_and_close_on_the_next_minute() {
        let mut agg = MinuteAggregator::default();
        assert_eq!(agg.push(&snap(600, 10.0, 1)), None);
        assert_eq!(agg.push(&snap(630, 30.0, 2)), None);
        let row = agg
            .push(&snap(660, 50.0, 3))
            .unwrap_or_else(|| panic!("the minute should close"));
        assert_eq!(row.ts, 600);
        assert_eq!(row.cpu, 2000, "average of 10 % and 30 %");
        assert_eq!(row.cpu_max, 3000);
        assert_eq!(row.disk_used, 2, "the minute's last reading");
        assert_eq!(row.load1, 50);
        assert_eq!(
            (row.root_used, row.root_total),
            (2, 10_000),
            "/ is the data filesystem when no separate root was sampled"
        );
    }

    #[test]
    fn forecast_projects_growth_and_ignores_flat_or_short_series() {
        let gib = 1024 * 1024 * 1024_i64;
        // 1 GiB a day for a day, 5 GiB free: full in ~5 days.
        let rising: Vec<(i64, i64)> = (0..=24)
            .map(|h| (h * 3600, 10 * gib + gib * h / 24))
            .collect();
        let f = forecast("24h", &rising, 11 * gib, 16 * gib, 7200);
        assert_eq!(f.status, "filling");
        let days = f.eta_secs.unwrap_or_default() as f64 / 86_400.0;
        assert!((days - 5.0).abs() < 0.05, "got {days} days");

        let flat: Vec<(i64, i64)> = (0..=24).map(|h| (h * 3600, 10 * gib)).collect();
        let f = forecast("24h", &flat, 10 * gib, 16 * gib, 7200);
        assert_eq!((f.status, f.eta_secs), ("stable", None));

        let shrinking: Vec<(i64, i64)> = (0..=24).map(|h| (h * 3600, 10 * gib - h)).collect();
        assert_eq!(
            forecast("24h", &shrinking, 10 * gib, 16 * gib, 7200).status,
            "stable"
        );

        let short = [(0, 1), (60, 2)];
        assert_eq!(
            forecast("24h", &short, 2, 100, 7200).status,
            "insufficient_data"
        );
        assert_eq!(forecast("24h", &[], 0, 0, 7200).status, "insufficient_data");

        // Growing, but a year and more away: not worth a warning.
        let slow: Vec<(i64, i64)> = (0..=24)
            .map(|h| (h * 3600, gib + 20 * 1024 * 1024 * h / 24))
            .collect();
        assert_eq!(
            forecast("24h", &slow, gib, 100_000 * gib, 7200).status,
            "stable"
        );
    }

    #[tokio::test]
    async fn history_is_stored_rolled_up_queried_and_pruned() {
        let db = db().await;
        let mut h = HostHistory::default();
        let base = 1_700_000_000 / 3600 * 3600;
        // Two hours of 30 s samples, disk growing.
        for i in 0..(2 * 120 + 1) {
            let ts = base + i * 30;
            h.record(&db, &snap(ts, 20.0, 1000 + i as u64))
                .await
                .unwrap_or_else(|e| panic!("record: {e}"));
        }
        let minutes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM host_1m")
            .fetch_one(&db)
            .await
            .unwrap_or_default();
        assert_eq!(minutes, 120);
        let hours: Vec<(i64, i64)> = sqlx::query_as("SELECT ts, cpu FROM host_1h ORDER BY ts")
            .fetch_all(&db)
            .await
            .unwrap_or_default();
        assert_eq!(
            hours,
            vec![(base, 2000), (base + 3600, 2000)],
            "both hours closed once the third began"
        );

        let (from, step, series) = query(&db, HistoryRange::Day, base + 7200)
            .await
            .unwrap_or_else(|e| panic!("query: {e}"));
        assert_eq!(step, 180);
        assert!(from <= base);
        assert_eq!(series.t.len(), 40);
        assert_eq!(series.cpu[0], 20.0);
        assert_eq!(series.load1[0], 0.5);

        let eta = disk_full_eta(&db, base + 7200)
            .await
            .unwrap_or_else(|e| panic!("eta: {e}"));
        assert_eq!(eta.day.samples, 120);
        assert_eq!(eta.week.status, "insufficient_data");

        let deleted = prune(&db, base + 7200 + H1_KEEP_SECS + 1, H1_KEEP_SECS)
            .await
            .unwrap_or_else(|e| panic!("prune: {e}"));
        assert_eq!(deleted, 122);
    }
}
