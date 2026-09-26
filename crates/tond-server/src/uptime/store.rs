// uptime/store.rs — check configuration, probe results, rollups, incidents
//
// Tables (migrations/030_host_history_uptime.sql):
//   uptime_checks     one per app with a domain + the dashboard
//   uptime_1m         one row per probe, kept 48 h
//   uptime_1h         hourly rollups, kept 90 days
//   uptime_incidents  down periods, kept 90 days after they end
//
// Every range scan goes through a (check_id, ts) primary key, with
// `check_id IN (SELECT id FROM uptime_checks)` when all checks are read.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::{AssertSqlSafe, SqlitePool};

use super::probe::Cause;

pub const M1_KEEP_SECS: i64 = 48 * 3600;
pub const H1_KEEP_SECS: i64 = 90 * 86_400;
pub const INCIDENT_KEEP_SECS: i64 = 90 * 86_400;

/// A check's stored configuration.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CheckRow {
    pub id: i64,
    pub kind: String,
    pub owner_id: String,
    pub enabled: bool,
    pub path: String,
    pub interval_secs: i64,
    pub expected_status: Option<i64>,
    pub timeout_secs: i64,
}

/// A check joined with its app (None for the dashboard or a vanished app).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CheckWithApp {
    #[sqlx(flatten)]
    pub check: CheckRow,
    pub app_name: Option<String>,
    pub app_domain: Option<String>,
    pub app_status: Option<String>,
    pub app_readiness: Option<String>,
}

/// Create checks for apps that got a domain, then list every check.
pub async fn load_checks(db: &SqlitePool) -> sqlx::Result<Vec<CheckWithApp>> {
    sqlx::query(
        "INSERT OR IGNORE INTO uptime_checks (kind, owner_id)
         SELECT 'app', id FROM apps WHERE domain IS NOT NULL AND domain != ''",
    )
    .execute(db)
    .await?;
    sqlx::query_as(
        "SELECT c.id, c.kind, c.owner_id, c.enabled, c.path, c.interval_secs, c.expected_status,
                c.timeout_secs, a.name AS app_name, a.domain AS app_domain,
                a.status AS app_status, a.readiness AS app_readiness
         FROM uptime_checks c LEFT JOIN apps a ON c.kind = 'app' AND a.id = c.owner_id
         ORDER BY c.id",
    )
    .fetch_all(db)
    .await
}

/// The check of an app (created if the app has a domain).
pub async fn check_for(
    db: &SqlitePool,
    kind: &str,
    owner_id: &str,
) -> sqlx::Result<Option<CheckRow>> {
    if kind == "app" {
        sqlx::query(
            "INSERT OR IGNORE INTO uptime_checks (kind, owner_id)
             SELECT 'app', id FROM apps WHERE id = ? AND domain IS NOT NULL AND domain != ''",
        )
        .bind(owner_id)
        .execute(db)
        .await?;
    }
    sqlx::query_as(
        "SELECT id, kind, owner_id, enabled, path, interval_secs, expected_status, timeout_secs
         FROM uptime_checks WHERE kind = ? AND owner_id = ?",
    )
    .bind(kind)
    .bind(owner_id)
    .fetch_optional(db)
    .await
}

/// Settings a user can change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSettings {
    pub enabled: bool,
    pub path: String,
    pub interval_secs: i64,
    pub expected_status: Option<i64>,
    pub timeout_secs: i64,
}

pub async fn update_check(db: &SqlitePool, id: i64, s: &CheckSettings) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE uptime_checks SET enabled = ?, path = ?, interval_secs = ?, expected_status = ?,
                timeout_secs = ? WHERE id = ?",
    )
    .bind(s.enabled)
    .bind(&s.path)
    .bind(s.interval_secs)
    .bind(s.expected_status)
    .bind(s.timeout_secs)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// One probe result to store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultRow {
    pub check_id: i64,
    pub ts: i64,
    /// 0 up, else Cause::code
    pub result: i64,
    pub status: Option<i64>,
    pub latency_ms: Option<i64>,
}

/// Store a round's results in one statement per 100 rows.
pub async fn insert_results(db: &SqlitePool, rows: &[ResultRow]) -> sqlx::Result<()> {
    for chunk in rows.chunks(100) {
        let placeholders = vec!["(?, ?, ?, ?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT OR REPLACE INTO uptime_1m (check_id, ts, result, status, latency_ms) VALUES {placeholders}"
        );
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for r in chunk {
            q = q
                .bind(r.check_id)
                .bind(r.ts)
                .bind(r.result)
                .bind(r.status)
                .bind(r.latency_ms);
        }
        q.execute(db).await?;
    }
    Ok(())
}

// ── Incidents ──

/// A down period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Incident {
    pub id: i64,
    pub check_id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub cause: String,
    pub detail: String,
}

pub async fn open_incident(
    db: &SqlitePool,
    check_id: i64,
    at: i64,
    cause: Cause,
    detail: &str,
) -> sqlx::Result<i64> {
    let id = sqlx::query(
        "INSERT INTO uptime_incidents (check_id, started_at, cause, detail) VALUES (?, ?, ?, ?)",
    )
    .bind(check_id)
    .bind(at)
    .bind(cause.as_str())
    .bind(detail)
    .execute(db)
    .await?
    .last_insert_rowid();
    Ok(id)
}

pub async fn close_incident(db: &SqlitePool, id: i64, at: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE uptime_incidents SET ended_at = ? WHERE id = ? AND ended_at IS NULL")
        .bind(at)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Incidents still open (after a restart).
pub async fn open_incidents(db: &SqlitePool) -> sqlx::Result<Vec<Incident>> {
    sqlx::query_as(
        "SELECT id, check_id, started_at, ended_at, cause, detail FROM uptime_incidents
         WHERE check_id IN (SELECT id FROM uptime_checks) AND ended_at IS NULL",
    )
    .fetch_all(db)
    .await
}

/// A check's incidents, newest first.
pub async fn incidents(db: &SqlitePool, check_id: i64, limit: i64) -> sqlx::Result<Vec<Incident>> {
    sqlx::query_as(
        "SELECT id, check_id, started_at, ended_at, cause, detail FROM uptime_incidents
         WHERE check_id = ? ORDER BY started_at DESC LIMIT ?",
    )
    .bind(check_id)
    .bind(limit)
    .fetch_all(db)
    .await
}

// ── Rollups, retention ──

/// Newest hourly rollup (the start of the next hour to roll up is +3600).
pub async fn latest_rollup(db: &SqlitePool) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT MAX(ts) FROM uptime_1h WHERE check_id IN (SELECT id FROM uptime_checks)",
    )
    .fetch_one(db)
    .await
}

/// Roll the probes in [from, to) up into hours (re-running is harmless).
pub async fn rollup_1h(db: &SqlitePool, from: i64, to: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO uptime_1h (check_id, ts, probes, up, latency_ms)
         SELECT check_id, ts / 3600 * 3600 AS h, COUNT(*), SUM(result = 0),
                CAST(AVG(CASE WHEN result = 0 THEN latency_ms END) AS INTEGER)
         FROM uptime_1m
         WHERE check_id IN (SELECT id FROM uptime_checks) AND ts >= ? AND ts < ?
         GROUP BY check_id, h",
    )
    .bind(from)
    .bind(to)
    .execute(db)
    .await?;
    Ok(())
}

/// Drop rows past retention. Returns the number of rows deleted.
pub async fn prune(db: &SqlitePool, now: i64) -> sqlx::Result<u64> {
    let mut deleted = 0;
    for (table, cutoff) in [
        ("uptime_1m", now - M1_KEEP_SECS),
        ("uptime_1h", now - H1_KEEP_SECS),
    ] {
        let sql = format!(
            "DELETE FROM {table} WHERE (check_id, ts) IN (
                SELECT check_id, ts FROM {table}
                WHERE check_id IN (SELECT id FROM uptime_checks) AND ts < ? LIMIT 5000)"
        );
        loop {
            let n = sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind(cutoff)
                .execute(db)
                .await?
                .rows_affected();
            deleted += n;
            if n < 5000 {
                break;
            }
        }
    }
    deleted += sqlx::query(
        "DELETE FROM uptime_incidents WHERE started_at < ? AND ended_at IS NOT NULL AND ended_at < ?",
    )
    .bind(now - INCIDENT_KEEP_SECS)
    .bind(now - INCIDENT_KEEP_SECS)
    .execute(db)
    .await?
    .rows_affected();
    Ok(deleted)
}

// ── Uptime percentages and bars ──

/// Probes and successes over a window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub probes: i64,
    pub up: i64,
}

impl Tally {
    fn add(&mut self, probes: i64, up: i64) {
        self.probes += probes;
        self.up += up;
    }

    /// Percentage up (None without probes).
    pub fn percent(&self) -> Option<f64> {
        (self.probes > 0).then(|| self.up as f64 / self.probes as f64 * 100.0)
    }
}

/// Uptime of each check over 24 h / 7 d / 30 d.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Windows {
    pub day: Tally,
    pub week: Tally,
    pub month: Tally,
}

/// Uptime tallies for every check. `rolled_until` is where hourly rollups
/// end: probes before it are read from uptime_1h, after it from uptime_1m.
pub async fn windows(
    db: &SqlitePool,
    now: i64,
    rolled_until: i64,
) -> sqlx::Result<HashMap<i64, Windows>> {
    let mut out: HashMap<i64, Windows> = HashMap::new();
    let day: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT check_id, COUNT(*), SUM(result = 0) FROM uptime_1m
         WHERE check_id IN (SELECT id FROM uptime_checks) AND ts >= ? GROUP BY check_id",
    )
    .bind(now - 86_400)
    .fetch_all(db)
    .await?;
    for (id, probes, up) in day {
        out.entry(id).or_default().day.add(probes, up);
    }
    for (span, pick) in [(7 * 86_400, 0usize), (30 * 86_400, 1usize)] {
        let since = now - span;
        let hours: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT check_id, SUM(probes), SUM(up) FROM uptime_1h
             WHERE check_id IN (SELECT id FROM uptime_checks) AND ts >= ? AND ts < ?
             GROUP BY check_id",
        )
        .bind(since)
        .bind(rolled_until)
        .fetch_all(db)
        .await?;
        let recent: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT check_id, COUNT(*), SUM(result = 0) FROM uptime_1m
             WHERE check_id IN (SELECT id FROM uptime_checks) AND ts >= ? GROUP BY check_id",
        )
        .bind(since.max(rolled_until))
        .fetch_all(db)
        .await?;
        for (id, probes, up) in hours.into_iter().chain(recent) {
            let w = out.entry(id).or_default();
            if pick == 0 {
                w.week.add(probes, up);
            } else {
                w.month.add(probes, up);
            }
        }
    }
    Ok(out)
}

/// One bar of a status strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Bar {
    /// Bucket start, unix seconds.
    pub t: i64,
    pub probes: i64,
    pub up: i64,
}

/// `count` buckets of `width` seconds ending at `now` (from uptime_1m, so
/// at most 48 h back). Buckets without probes have probes = 0.
pub async fn recent_bars(
    db: &SqlitePool,
    check_id: i64,
    now: i64,
    width: i64,
    count: i64,
) -> sqlx::Result<Vec<Bar>> {
    let end = now / width * width + width;
    let start = end - width * count;
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT (ts / ?1) * ?1 AS b, COUNT(*), SUM(result = 0) FROM uptime_1m
         WHERE check_id = ?2 AND ts >= ?3 GROUP BY b",
    )
    .bind(width)
    .bind(check_id)
    .bind(start)
    .fetch_all(db)
    .await?;
    Ok(fill_bars(start, width, count, &rows))
}

/// Daily bars for the last `days` days (from uptime_1h plus today's probes).
pub async fn daily_bars(
    db: &SqlitePool,
    check_id: i64,
    now: i64,
    days: i64,
    rolled_until: i64,
) -> sqlx::Result<Vec<Bar>> {
    let width = 86_400;
    let end = now / width * width + width;
    let start = end - width * days;
    let mut rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT (ts / 86400) * 86400 AS b, SUM(probes), SUM(up) FROM uptime_1h
         WHERE check_id = ? AND ts >= ? AND ts < ? GROUP BY b",
    )
    .bind(check_id)
    .bind(start)
    .bind(rolled_until)
    .fetch_all(db)
    .await?;
    let recent: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT (ts / 86400) * 86400 AS b, COUNT(*), SUM(result = 0) FROM uptime_1m
         WHERE check_id = ? AND ts >= ? GROUP BY b",
    )
    .bind(check_id)
    .bind(start.max(rolled_until))
    .fetch_all(db)
    .await?;
    rows.extend(recent);
    Ok(fill_bars(start, width, days, &rows))
}

fn fill_bars(start: i64, width: i64, count: i64, rows: &[(i64, i64, i64)]) -> Vec<Bar> {
    let mut bars: Vec<Bar> = (0..count)
        .map(|i| Bar {
            t: start + i * width,
            probes: 0,
            up: 0,
        })
        .collect();
    for (t, probes, up) in rows {
        let i = (t - start) / width;
        if let Some(bar) = usize::try_from(i).ok().and_then(|i| bars.get_mut(i)) {
            bar.probes += probes;
            bar.up += up;
        }
    }
    bars
}

/// Average response time of successful probes in `width` buckets since `from`.
pub async fn latency_series(
    db: &SqlitePool,
    check_id: i64,
    from: i64,
    width: i64,
) -> sqlx::Result<(Vec<i64>, Vec<i64>)> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT (ts / ?1) * ?1 AS b, CAST(AVG(latency_ms) AS INTEGER) FROM uptime_1m
         WHERE check_id = ?2 AND ts >= ?3 AND result = 0 AND latency_ms IS NOT NULL
         GROUP BY b ORDER BY b",
    )
    .bind(width)
    .bind(check_id)
    .bind(from)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().unzip())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    async fn app(db: &SqlitePool, id: &str, domain: Option<&str>) {
        sqlx::query(
            "INSERT INTO apps (id, name, domain, created_at, updated_at, status)
             VALUES (?, ?, ?, '2026-01-01', '2026-01-01', 'running')",
        )
        .bind(id)
        .bind(format!("app-{id}"))
        .bind(domain)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("app: {e}"));
    }

    #[tokio::test]
    async fn checks_are_created_for_apps_with_domains_and_removed_with_them() {
        let db = db().await;
        app(&db, "a", Some("a.example.com")).await;
        app(&db, "b", None).await;
        let checks = load_checks(&db).await.unwrap_or_default();
        let kinds: Vec<(&str, &str)> = checks
            .iter()
            .map(|c| (c.check.kind.as_str(), c.check.owner_id.as_str()))
            .collect();
        assert_eq!(kinds, vec![("dashboard", ""), ("app", "a")]);
        assert_eq!(checks[0].check.path, "/health");
        assert_eq!(checks[1].app_domain.as_deref(), Some("a.example.com"));
        assert_eq!(checks[1].check.interval_secs, 60);

        let id = checks[1].check.id;
        insert_results(
            &db,
            &[ResultRow {
                check_id: id,
                ts: 60,
                result: 0,
                status: Some(200),
                latency_ms: Some(12),
            }],
        )
        .await
        .unwrap_or_else(|e| panic!("insert: {e}"));
        open_incident(&db, id, 120, Cause::Timeout, "slow")
            .await
            .unwrap_or_else(|e| panic!("incident: {e}"));
        sqlx::query("DELETE FROM apps WHERE id = 'a'")
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("delete: {e}"));
        let left: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM uptime_checks WHERE kind = 'app')
                  + (SELECT COUNT(*) FROM uptime_1m) + (SELECT COUNT(*) FROM uptime_incidents)",
        )
        .fetch_one(&db)
        .await
        .unwrap_or(-1);
        assert_eq!(left, 0, "the check and its history go with the app");
    }

    #[tokio::test]
    async fn windows_combine_rollups_and_recent_probes_and_bars_fill_gaps() {
        let db = db().await;
        let id: i64 = sqlx::query_scalar("SELECT id FROM uptime_checks WHERE kind = 'dashboard'")
            .fetch_one(&db)
            .await
            .unwrap_or_default();
        let hour = 1_700_000_000 / 3600 * 3600;
        // Two hours of minutely probes, one failure in the first hour.
        let rows: Vec<ResultRow> = (0..120)
            .map(|m| ResultRow {
                check_id: id,
                ts: hour + m * 60,
                result: if m == 30 { Cause::Timeout.code() } else { 0 },
                status: (m != 30).then_some(200),
                latency_ms: Some(10 + m % 3),
            })
            .collect();
        insert_results(&db, &rows)
            .await
            .unwrap_or_else(|e| panic!("insert: {e}"));
        rollup_1h(&db, hour, hour + 3600)
            .await
            .unwrap_or_else(|e| panic!("rollup: {e}"));
        let rolled: (i64, i64) =
            sqlx::query_as("SELECT probes, up FROM uptime_1h WHERE check_id = ?")
                .bind(id)
                .fetch_one(&db)
                .await
                .unwrap_or_default();
        assert_eq!(rolled, (60, 59));
        assert_eq!(latest_rollup(&db).await.unwrap_or_default(), Some(hour));

        let now = hour + 7200;
        let w = windows(&db, now, hour + 3600).await.unwrap_or_default();
        let w = w.get(&id).copied().unwrap_or_default();
        assert_eq!(
            w.day,
            Tally {
                probes: 120,
                up: 119
            }
        );
        assert_eq!(
            w.week, w.day,
            "rolled-up hour + the hour after it, no double counting"
        );
        assert_eq!(w.month, w.day);
        assert!((w.day.percent().unwrap_or_default() - 99.1666).abs() < 0.01);

        let bars = recent_bars(&db, id, now, 1800, 8).await.unwrap_or_default();
        assert_eq!(bars.len(), 8);
        assert_eq!(bars.iter().map(|b| b.probes).sum::<i64>(), 120);
        assert_eq!(bars[0].probes, 0, "before the first probe");
        let days = daily_bars(&db, id, now, 3, hour + 3600)
            .await
            .unwrap_or_default();
        assert_eq!(days.iter().map(|b| b.probes).sum::<i64>(), 120);
        let (t, ms) = latency_series(&db, id, hour, 3600)
            .await
            .unwrap_or_default();
        assert_eq!(t, vec![hour, hour + 3600]);
        assert!(ms.iter().all(|m| (10..=12).contains(m)));

        // Retention.
        close_incident(
            &db,
            open_incident(&db, id, hour, Cause::Tls, "x")
                .await
                .unwrap_or_default(),
            hour + 60,
        )
        .await
        .unwrap_or_else(|e| panic!("close: {e}"));
        assert!(open_incidents(&db).await.unwrap_or_default().is_empty());
        let gone = prune(&db, now + INCIDENT_KEEP_SECS + 1)
            .await
            .unwrap_or_default();
        assert_eq!(gone, 120 + 1 + 1);
    }
}
