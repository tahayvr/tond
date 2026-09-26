// metrics/store.rs — compact metric storage, rollups, retention and queries
//
// Tables (see migrations/001_baseline.sql):
//   metric_targets  (id, kind, owner_id, mem_limit)
//   metrics_raw     30 s samples                 kept RAW_KEEP_SECS  (25 h)
//   metrics_5m      5-minute rollups             kept M5_KEEP_SECS   (8 days)
//   metrics_1h      1-hour rollups               kept H1_KEEP_SECS   (90 days)
//   metric_events   restarts / crashes / OOMs    kept EVENT_KEEP_SECS (90 days)
//
// Each tick writes all samples in one transaction. Rollups are computed when a
// bucket closes (every 5 minutes / every hour) with INSERT OR REPLACE, so a
// re-run is harmless. All range scans go through the (target, ts) primary key
// with `target IN (SELECT id FROM metric_targets)`, never a full table scan.
//
// Queries return at most MAX_POINTS buckets, picking the coarsest table that
// still has the resolution the window needs:
//   1h  → raw, 30 s buckets (120 points)
//   24h → raw, 3 min buckets (480)
//   7d  → 5m,  25 min buckets (~400)
//   30d → 1h,  2 h buckets (360)

use std::time::Duration;

use sqlx::{AssertSqlSafe, SqlitePool};
use tond_common::{MetricEventResponse, MetricSeries};

/// Longest series a query returns.
pub const MAX_POINTS: i64 = 480;
pub const RAW_KEEP_SECS: i64 = 25 * 3600;
pub const M5_KEEP_SECS: i64 = 8 * 86_400;
pub const H1_KEEP_SECS: i64 = 90 * 86_400;
pub const EVENT_KEEP_SECS: i64 = 90 * 86_400;
const BATCH: i64 = 2_000;
const BATCH_PAUSE: Duration = Duration::from_millis(25);

/// What a target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    App,
    Database,
    Cron,
    Preview,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::App => "app",
            TargetKind::Database => "database",
            TargetKind::Cron => "cron",
            TargetKind::Preview => "preview",
        }
    }
}

/// The app / database / cron job / preview a container belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Owner {
    pub kind: TargetKind,
    pub id: String,
}

impl Owner {
    pub fn new(kind: TargetKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

/// One stored sample (all rates already computed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample {
    /// Hundredths of a percent of one core (10000 = one core).
    pub cpu: i64,
    pub mem: i64,
    pub rx: i64,
    pub tx: i64,
    pub pids: i64,
}

/// Event kinds recorded in metric_events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Restart,
    Crash,
    Oom,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Restart => "restart",
            EventKind::Crash => "crash",
            EventKind::Oom => "oom",
        }
    }
}

/// Id of `owner`'s target row, creating it on first use.
pub async fn ensure_target(db: &SqlitePool, owner: &Owner) -> sqlx::Result<i64> {
    sqlx::query("INSERT INTO metric_targets (kind, owner_id) VALUES (?, ?) ON CONFLICT DO NOTHING")
        .bind(owner.kind.as_str())
        .bind(&owner.id)
        .execute(db)
        .await?;
    sqlx::query_scalar("SELECT id FROM metric_targets WHERE kind = ? AND owner_id = ?")
        .bind(owner.kind.as_str())
        .bind(&owner.id)
        .fetch_one(db)
        .await
}

/// Existing target id and memory limit for `owner`.
pub async fn find_target(db: &SqlitePool, owner: &Owner) -> sqlx::Result<Option<(i64, i64)>> {
    sqlx::query_as("SELECT id, mem_limit FROM metric_targets WHERE kind = ? AND owner_id = ?")
        .bind(owner.kind.as_str())
        .bind(&owner.id)
        .fetch_optional(db)
        .await
}

/// A row to write this tick.
#[derive(Debug, Clone, Copy)]
pub struct TickRow {
    pub target: i64,
    pub sample: Sample,
    /// Some(limit) when the target's memory limit changed since last written.
    pub mem_limit: Option<i64>,
}

/// Write one tick's samples in a single transaction.
pub async fn insert_tick(db: &SqlitePool, ts: i64, rows: &[TickRow]) -> sqlx::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut tx = db.begin().await?;
    for row in rows {
        let s = row.sample;
        sqlx::query(
            "INSERT OR REPLACE INTO metrics_raw (target, ts, cpu, mem, rx, tx, pids)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.target)
        .bind(ts)
        .bind(s.cpu)
        .bind(s.mem)
        .bind(s.rx)
        .bind(s.tx)
        .bind(s.pids)
        .execute(&mut *tx)
        .await?;
        if let Some(limit) = row.mem_limit {
            sqlx::query("UPDATE metric_targets SET mem_limit = ? WHERE id = ?")
                .bind(limit)
                .bind(row.target)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await
}

/// Record a restart / crash / OOM event (duplicates in the same second collapse).
pub async fn record_event(
    db: &SqlitePool,
    target: i64,
    ts: i64,
    kind: EventKind,
    detail: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO metric_events (target, ts, kind, detail) VALUES (?, ?, ?, ?)",
    )
    .bind(target)
    .bind(ts)
    .bind(kind.as_str())
    .bind(detail)
    .execute(db)
    .await?;
    Ok(())
}

/// Roll raw samples in [from, to) up into 5-minute buckets.
pub async fn rollup_5m(db: &SqlitePool, from: i64, to: i64) -> sqlx::Result<()> {
    if from >= to {
        return Ok(());
    }
    sqlx::query(
        "INSERT OR REPLACE INTO metrics_5m (target, ts, cpu, cpu_max, mem, mem_max, rx, tx, pids)
         SELECT target, (ts / 300) * 300, CAST(AVG(cpu) AS INTEGER), MAX(cpu),
                CAST(AVG(mem) AS INTEGER), MAX(mem), CAST(AVG(rx) AS INTEGER),
                CAST(AVG(tx) AS INTEGER), MAX(pids)
         FROM metrics_raw
         WHERE target IN (SELECT id FROM metric_targets) AND ts >= ? AND ts < ?
         GROUP BY target, ts / 300",
    )
    .bind(from)
    .bind(to)
    .execute(db)
    .await?;
    Ok(())
}

/// Roll 5-minute buckets in [from, to) up into 1-hour buckets.
pub async fn rollup_1h(db: &SqlitePool, from: i64, to: i64) -> sqlx::Result<()> {
    if from >= to {
        return Ok(());
    }
    sqlx::query(
        "INSERT OR REPLACE INTO metrics_1h (target, ts, cpu, cpu_max, mem, mem_max, rx, tx, pids)
         SELECT target, (ts / 3600) * 3600, CAST(AVG(cpu) AS INTEGER), MAX(cpu_max),
                CAST(AVG(mem) AS INTEGER), MAX(mem_max), CAST(AVG(rx) AS INTEGER),
                CAST(AVG(tx) AS INTEGER), MAX(pids)
         FROM metrics_5m
         WHERE target IN (SELECT id FROM metric_targets) AND ts >= ? AND ts < ?
         GROUP BY target, ts / 3600",
    )
    .bind(from)
    .bind(to)
    .execute(db)
    .await?;
    Ok(())
}

/// Start of the newest bucket in a rollup table (None when empty). Used once
/// at startup to resume rolling up where the last run stopped.
pub async fn latest_bucket(db: &SqlitePool, table: RollupTable) -> sqlx::Result<Option<i64>> {
    let sql = format!("SELECT MAX(ts) FROM {}", table.name());
    sqlx::query_scalar(AssertSqlSafe(sql.as_str()))
        .fetch_one(db)
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollupTable {
    FiveMinutes,
    OneHour,
}

impl RollupTable {
    fn name(self) -> &'static str {
        match self {
            RollupTable::FiveMinutes => "metrics_5m",
            RollupTable::OneHour => "metrics_1h",
        }
    }
}

/// Drop samples, rollups and events past their retention, plus targets whose
/// owner no longer exists. Returns the number of rows deleted.
/// Delete expired rows. `event_keep_secs` is how long restart / crash / OOM
/// events are kept (`retention_app_events_days`, default EVENT_KEEP_SECS).
pub async fn prune(db: &SqlitePool, now: i64, event_keep_secs: i64) -> anyhow::Result<u64> {
    let mut deleted = 0;

    // Targets whose app / database / cron job / preview is gone, with all rows.
    let orphans: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM metric_targets t WHERE NOT EXISTS (
            SELECT 1 FROM apps WHERE t.kind = 'app' AND apps.id = t.owner_id
            UNION ALL SELECT 1 FROM databases WHERE t.kind = 'database' AND databases.id = t.owner_id
            UNION ALL SELECT 1 FROM cron_jobs WHERE t.kind = 'cron' AND cron_jobs.id = t.owner_id
            UNION ALL SELECT 1 FROM preview_deployments p WHERE t.kind = 'preview' AND p.id = t.owner_id)",
    )
    .fetch_all(db)
    .await?;
    for id in orphans {
        for table in ["metrics_raw", "metrics_5m", "metrics_1h", "metric_events"] {
            let sql = format!("DELETE FROM {table} WHERE target = ?");
            deleted += sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind(id)
                .execute(db)
                .await?
                .rows_affected();
        }
        deleted += sqlx::query("DELETE FROM metric_targets WHERE id = ?")
            .bind(id)
            .execute(db)
            .await?
            .rows_affected();
    }

    for (table, keep, key) in [
        ("metrics_raw", RAW_KEEP_SECS, "target, ts"),
        ("metrics_5m", M5_KEEP_SECS, "target, ts"),
        ("metrics_1h", H1_KEEP_SECS, "target, ts"),
        ("metric_events", event_keep_secs, "target, ts, kind"),
    ] {
        let sql = format!(
            "DELETE FROM {table} WHERE ({key}) IN (
                SELECT {key} FROM {table}
                WHERE target IN (SELECT id FROM metric_targets) AND ts < ? LIMIT ?)"
        );
        loop {
            let n = sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind(now - keep)
                .bind(BATCH)
                .execute(db)
                .await?
                .rows_affected();
            deleted += n;
            if n < BATCH as u64 {
                break;
            }
            tokio::time::sleep(BATCH_PAUSE).await;
        }
    }
    Ok(deleted)
}

/// A supported query window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Hour,
    Day,
    Week,
    Month,
}

impl Range {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1h" => Some(Range::Hour),
            "24h" => Some(Range::Day),
            "7d" => Some(Range::Week),
            "30d" => Some(Range::Month),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Range::Hour => "1h",
            Range::Day => "24h",
            Range::Week => "7d",
            Range::Month => "30d",
        }
    }

    pub fn span(self) -> i64 {
        match self {
            Range::Hour => 3600,
            Range::Day => 86_400,
            Range::Week => 7 * 86_400,
            Range::Month => 30 * 86_400,
        }
    }

    /// (source table, whether it is a rollup, its native resolution)
    fn source(self) -> (&'static str, bool, i64) {
        match self {
            Range::Hour | Range::Day => ("metrics_raw", false, 30),
            Range::Week => ("metrics_5m", true, 300),
            Range::Month => ("metrics_1h", true, 3600),
        }
    }

    /// Bucket width: a multiple of the source resolution giving ≤ MAX_POINTS.
    pub fn step(self) -> i64 {
        let (_, _, native) = self.source();
        let per_point = (self.span() + MAX_POINTS - 1) / MAX_POINTS;
        ((per_point + native - 1) / native).max(1) * native
    }
}

/// Downsampled series for `target` over `range` ending at `now`.
/// Returns (from, series).
pub async fn query_series(
    db: &SqlitePool,
    target: i64,
    range: Range,
    now: i64,
) -> sqlx::Result<(i64, MetricSeries)> {
    let step = range.step();
    let from = (now - range.span()) / step * step;
    let (table, rollup, _) = range.source();
    let (cpu_max, mem_max) = if rollup {
        ("MAX(cpu_max)", "MAX(mem_max)")
    } else {
        ("MAX(cpu)", "MAX(mem)")
    };
    let sql = format!(
        "SELECT (ts / ?1) * ?1 AS b, AVG(cpu), {cpu_max}, CAST(AVG(mem) AS INTEGER), {mem_max},
                CAST(AVG(rx) AS INTEGER), CAST(AVG(tx) AS INTEGER), MAX(pids)
         FROM {table} WHERE target = ?2 AND ts >= ?3
         GROUP BY b ORDER BY b"
    );
    #[allow(clippy::type_complexity)]
    let rows: Vec<(i64, f64, i64, i64, i64, i64, i64, i64)> =
        sqlx::query_as(AssertSqlSafe(sql.as_str()))
            .bind(step)
            .bind(target)
            .bind(from)
            .fetch_all(db)
            .await?;
    let mut series = MetricSeries::default();
    for (t, cpu, cpu_max, mem, mem_max, rx, tx, pids) in rows {
        series.t.push(t);
        series.cpu.push(centi_to_percent(cpu));
        series.cpu_max.push(centi_to_percent(cpu_max as f64));
        series.mem.push(mem);
        series.mem_max.push(mem_max);
        series.rx.push(rx);
        series.tx.push(tx);
        series.pids.push(pids);
    }
    Ok((from, series))
}

/// Events for `target` at or after `from` (oldest first, at most 500).
pub async fn query_events(
    db: &SqlitePool,
    target: i64,
    from: i64,
) -> sqlx::Result<Vec<MetricEventResponse>> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT ts, kind, detail FROM metric_events WHERE target = ? AND ts >= ?
         ORDER BY ts DESC LIMIT 500",
    )
    .bind(target)
    .bind(from)
    .fetch_all(db)
    .await?;
    let mut events: Vec<MetricEventResponse> = rows
        .into_iter()
        .map(|(t, kind, detail)| MetricEventResponse { t, kind, detail })
        .collect();
    events.reverse();
    Ok(events)
}

/// Latest raw sample of `target` newer than `since`: (ts, sample).
pub async fn latest_sample(
    db: &SqlitePool,
    target: i64,
    since: i64,
) -> sqlx::Result<Option<(i64, Sample)>> {
    let row: Option<(i64, i64, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT ts, cpu, mem, rx, tx, pids FROM metrics_raw
         WHERE target = ? AND ts >= ? ORDER BY ts DESC LIMIT 1",
    )
    .bind(target)
    .bind(since)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(ts, cpu, mem, rx, tx, pids)| {
        (
            ts,
            Sample {
                cpu,
                mem,
                rx,
                tx,
                pids,
            },
        )
    }))
}

/// Highest latest-sample CPU (percent of one core) across all app targets
/// sampled since `since`.
pub async fn max_app_cpu_since(db: &SqlitePool, since: i64) -> sqlx::Result<Option<f64>> {
    let v: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(cpu) FROM metrics_raw
         WHERE target IN (SELECT id FROM metric_targets WHERE kind = 'app') AND ts >= ?",
    )
    .bind(since)
    .fetch_one(db)
    .await?;
    Ok(v.map(|c| c as f64 / 100.0))
}

/// Hundredths of a percent → percent with one decimal.
pub fn centi_to_percent(v: f64) -> f64 {
    (v / 10.0).round() / 10.0
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

    fn sample(cpu: i64, mem: i64) -> Sample {
        Sample {
            cpu,
            mem,
            rx: 100,
            tx: 50,
            pids: 3,
        }
    }

    #[test]
    fn steps_stay_under_the_point_budget_and_align_to_the_source() {
        for (range, step) in [
            (Range::Hour, 30),
            (Range::Day, 180),
            (Range::Week, 1500),
            (Range::Month, 7200),
        ] {
            assert_eq!(range.step(), step, "{range:?}");
            assert!(range.span() / range.step() <= MAX_POINTS);
            assert_eq!(range.step() % range.source().2, 0);
        }
        assert_eq!(Range::parse("7d"), Some(Range::Week));
        assert_eq!(Range::parse("1y"), None);
    }

    #[tokio::test]
    async fn targets_are_created_once() {
        let db = db().await;
        let owner = Owner::new(TargetKind::App, "a1");
        let a = ensure_target(&db, &owner)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let b = ensure_target(&db, &owner)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a, b);
        let other = ensure_target(&db, &Owner::new(TargetKind::Database, "a1"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(a, other);
    }

    #[tokio::test]
    async fn tick_rows_rollups_and_queries_round_trip() {
        let db = db().await;
        let target = ensure_target(&db, &Owner::new(TargetKind::App, "a1"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let base = 1_700_000_100 / 3600 * 3600; // hour-aligned
        // 20 minutes of 30 s samples: cpu 10% then 30%, mem 100 then 300.
        for i in 0..40 {
            let (cpu, mem) = if i < 20 { (1000, 100) } else { (3000, 300) };
            insert_tick(
                &db,
                base + i * 30,
                &[TickRow {
                    target,
                    sample: sample(cpu, mem),
                    mem_limit: (i == 0).then_some(1024),
                }],
            )
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        }
        let limit = find_target(&db, &Owner::new(TargetKind::App, "a1"))
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .map(|(_, l)| l);
        assert_eq!(limit, Some(1024));

        // 1h query: raw, 30 s buckets.
        let (_, series) = query_series(&db, target, Range::Hour, base + 40 * 30)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(series.t.len(), 40);
        assert_eq!(series.cpu[0], 10.0);
        assert_eq!(series.cpu[39], 30.0);
        assert_eq!(series.rx[0], 100);

        // Roll up the first 20 minutes (4 buckets of 5 minutes), then an hour.
        rollup_5m(&db, base, base + 1200)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        rollup_5m(&db, base, base + 1200)
            .await
            .unwrap_or_else(|e| panic!("idempotent: {e}"));
        let rows: Vec<(i64, i64, i64, i64)> =
            sqlx::query_as("SELECT ts, cpu, cpu_max, mem_max FROM metrics_5m ORDER BY ts")
                .fetch_all(&db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], (base, 1000, 1000, 100));
        assert_eq!(rows[2], (base + 600, 3000, 3000, 300));
        assert_eq!(
            latest_bucket(&db, RollupTable::FiveMinutes)
                .await
                .ok()
                .flatten(),
            Some(base + 900)
        );

        rollup_1h(&db, base, base + 3600)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let hour: (i64, i64, i64) = sqlx::query_as("SELECT ts, cpu, cpu_max FROM metrics_1h")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(hour, (base, 2000, 3000));

        // 7d query reads the 5m table and keeps maxima.
        let (_, week) = query_series(&db, target, Range::Week, base + 3600)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!week.t.is_empty() && week.t.len() <= 2);
        assert_eq!(week.cpu_max.iter().cloned().fold(0.0, f64::max), 30.0);

        let latest = latest_sample(&db, target, base)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            latest.map(|(ts, s)| (ts, s.cpu)),
            Some((base + 39 * 30, 3000))
        );
        assert_eq!(
            max_app_cpu_since(&db, base).await.ok().flatten(),
            Some(30.0)
        );
    }

    #[tokio::test]
    async fn events_are_recorded_and_queried_oldest_first() {
        let db = db().await;
        let target = ensure_target(&db, &Owner::new(TargetKind::App, "a1"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        record_event(&db, target, 200, EventKind::Oom, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        record_event(&db, target, 100, EventKind::Restart, Some("exit code 1"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        record_event(&db, target, 100, EventKind::Restart, Some("dup"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let events = query_events(&db, target, 0)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "restart");
        assert_eq!(events[0].detail.as_deref(), Some("exit code 1"));
        assert_eq!(events[1].kind, "oom");
        assert!(query_events(&db, target, 150).await.map(|e| e.len()).ok() == Some(1));
    }

    #[tokio::test]
    async fn prune_applies_tier_retention_and_drops_orphans() {
        let db = db().await;
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, created_at, updated_at)
             VALUES ('live', 'live', 'git', 3000, 'running', datetime('now'), datetime('now'))",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("seed app: {e}"));
        let live = ensure_target(&db, &Owner::new(TargetKind::App, "live"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let gone = ensure_target(&db, &Owner::new(TargetKind::App, "gone"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let now = 10_000_000;
        for (target, ts) in [
            (live, now - RAW_KEEP_SECS - 30),
            (live, now - 60),
            (gone, now - 60),
        ] {
            insert_tick(
                &db,
                ts,
                &[TickRow {
                    target,
                    sample: sample(1, 1),
                    mem_limit: None,
                }],
            )
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        }
        sqlx::query("INSERT INTO metrics_1h (target, ts, cpu, cpu_max, mem, mem_max, rx, tx, pids) VALUES (?, ?, 0,0,0,0,0,0,0), (?, ?, 0,0,0,0,0,0,0)")
            .bind(live)
            .bind(now - H1_KEEP_SECS - 3600)
            .bind(live)
            .bind(now - M5_KEEP_SECS - 3600)
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        record_event(&db, gone, now, EventKind::Oom, None)
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        let deleted = prune(&db, now, EVENT_KEEP_SECS)
            .await
            .unwrap_or_else(|e| panic!("prune: {e}"));
        assert_eq!(deleted, 5, "old raw + old hourly + orphan raw/event/target");
        let raw: Vec<(i64, i64)> = sqlx::query_as("SELECT target, ts FROM metrics_raw")
            .fetch_all(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(raw, vec![(live, now - 60)]);
        let hourly: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics_1h")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(hourly, 1, "hourly rows are kept for 90 days");
        let targets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metric_targets")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(targets, 1);
    }
}
