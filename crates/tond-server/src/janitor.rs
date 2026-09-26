// janitor.rs — retention for every table that grows without bound
//
// run_janitor(state) is spawned once at startup. It waits a couple of minutes
// (so it never competes with startup work), then runs every hour:
//
//   deploy_logs    — per app, keep the logs of the last N deployments
//                    (logs are not linked to a deployment row, so "logs of a
//                    deployment" means rows written at or after its start).
//                    Older logs are also dropped after M days, except the logs
//                    of the app's most recent deployment, which are always kept.
//   deployments    — per app, keep the last N rows. Unfinished rows (queued,
//                    running) and the last few succeeded deployments with an
//                    image (the rollback candidates) are never deleted. Rows
//                    whose app no longer exists are deleted.
//   cron_job_runs  — per job, keep the last N runs and drop runs older than M
//                    days (the latest run is always kept; running rows never
//                    deleted).
//   audit_log      — drop rows older than N days (security data: default 180)
//                    through audit_log::prune_before, which removes a prefix
//                    of the hash chain and records a checkpoint so the rest
//                    still verifies.
//   metrics        — tiered retention (raw 25 h, 5-minute rollups 8 days,
//                    hourly rollups 90 days; restart/crash/OOM events for
//                    `retention_app_events_days`) plus targets whose owner
//                    is gone; see metrics::store::prune. Host history
//                    (1-minute 48 h, hourly `retention_host_history_days`;
//                    metrics::host_history) and uptime results (1-minute
//                    48 h, hourly and incidents `retention_uptime_days`;
//                    uptime::store) are counted here too.
//   alert_history  — drop episodes resolved more than N days ago (default 90;
//                    their deliveries go with them) and close episodes whose
//                    rule disappeared with its app.
//   sessions / mfa_pending / password_links — drop expired rows (sessions
//                    also when idle past the idle timeout).
//   terminal_tickets / github_app_flows — drop expired rows.
//   webhook_deliveries — drop replay IDs older than the 72 h replay window.
//   preview_deployments — drop expired/failed rows a week after they expired.
//   Docker networks — remove Tond-labelled networks no container uses (older
//                    than an hour; see docker::prune_networks).
//
// Every delete runs in batches of BATCH_SIZE rows with a short pause between
// batches, so a single statement never holds SQLite's write lock for long
// (tond-proxy shares the database file). Each batch selects its victims
// through an index (see the retention indexes in 001_baseline.sql).
//
// Retention values come from the `settings` table (keys below) with built-in
// defaults. A value <= 0 disables that rule. After each pass the janitor runs
// `PRAGMA optimize` so the query planner statistics stay fresh.

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::runtime::loops;
use crate::{alerts, docker, metrics, state::AppState};

/// Rows deleted per statement.
const BATCH_SIZE: i64 = 2_000;
/// Pause between batches so other writers (deploy logs, proxy) get the lock.
const BATCH_PAUSE: Duration = Duration::from_millis(25);
/// Delay before the first pass after startup.
const STARTUP_DELAY: Duration = Duration::from_secs(120);
/// Interval between passes.
const INTERVAL: Duration = Duration::from_secs(3600);
/// Succeeded deployments whose image is still on the host that are always
/// kept (rollback targets). deploy/images.rs keeps at most `KEEP_IMAGES` (3)
/// such images plus the running one, so this never trims a live target.
const KEEP_ROLLBACK_IMAGES: i64 = 5;

/// Retention settings. Each field maps to a key in the `settings` table;
/// defaults and bounds are `tond_common::RETENTION_RULES`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// `retention_deploy_logs_deployments` — deployments per app whose logs are kept.
    pub deploy_log_deployments: i64,
    /// `retention_deploy_logs_days` — max age of deploy logs (latest deployment exempt).
    pub deploy_log_days: i64,
    /// `retention_deployments_per_app` — deployment rows kept per app.
    pub deployments_per_app: i64,
    /// `retention_cron_runs_per_job` — cron runs kept per job.
    pub cron_runs_per_job: i64,
    /// `retention_cron_runs_days` — max age of cron runs (latest run exempt).
    pub cron_run_days: i64,
    /// `retention_audit_log_days` — max age of audit log rows.
    pub audit_log_days: i64,
    /// `retention_app_events_days` — max age of restart / crash / OOM events.
    pub app_events_days: i64,
    /// `retention_alert_history_days` — how long resolved alerts are kept.
    pub alert_history_days: i64,
    /// `retention_uptime_days` — hourly uptime rollups and ended incidents.
    pub uptime_days: i64,
    /// `retention_host_history_days` — hourly host history.
    pub host_history_days: i64,
    /// `retention_request_metrics_days` — hourly request metrics.
    pub request_metrics_days: i64,
}

fn rule_default(key: &str) -> i64 {
    tond_common::retention_rule(key).map_or(0, |r| r.default)
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            deploy_log_deployments: rule_default("retention_deploy_logs_deployments"),
            deploy_log_days: rule_default("retention_deploy_logs_days"),
            deployments_per_app: rule_default("retention_deployments_per_app"),
            cron_runs_per_job: rule_default("retention_cron_runs_per_job"),
            cron_run_days: rule_default("retention_cron_runs_days"),
            audit_log_days: rule_default("retention_audit_log_days"),
            app_events_days: rule_default("retention_app_events_days"),
            alert_history_days: rule_default("retention_alert_history_days"),
            uptime_days: rule_default("retention_uptime_days"),
            host_history_days: rule_default("retention_host_history_days"),
            request_metrics_days: rule_default("retention_request_metrics_days"),
        }
    }
}

impl RetentionPolicy {
    /// Load the policy from the settings table, falling back to defaults for
    /// missing or unparseable values.
    pub async fn load(db: &SqlitePool) -> Self {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'retention_%'")
                .fetch_all(db)
                .await
                .unwrap_or_default();
        Self::from_pairs(rows.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    pub(crate) fn from_pairs<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut policy = Self::default();
        for (key, value) in pairs {
            let Ok(n) = value.trim().parse::<i64>() else {
                continue;
            };
            if let Some(field) = policy.field_mut(key) {
                *field = n;
            }
        }
        policy
    }

    fn field_mut(&mut self, key: &str) -> Option<&mut i64> {
        Some(match key {
            "retention_deploy_logs_deployments" => &mut self.deploy_log_deployments,
            "retention_deploy_logs_days" => &mut self.deploy_log_days,
            "retention_deployments_per_app" => &mut self.deployments_per_app,
            "retention_cron_runs_per_job" => &mut self.cron_runs_per_job,
            "retention_cron_runs_days" => &mut self.cron_run_days,
            "retention_audit_log_days" => &mut self.audit_log_days,
            "retention_app_events_days" => &mut self.app_events_days,
            "retention_alert_history_days" => &mut self.alert_history_days,
            "retention_uptime_days" => &mut self.uptime_days,
            "retention_host_history_days" => &mut self.host_history_days,
            "retention_request_metrics_days" => &mut self.request_metrics_days,
            _ => return None,
        })
    }

    /// The value for a settings key (None for an unknown key).
    pub fn get(&self, key: &str) -> Option<i64> {
        let mut copy = self.clone();
        copy.field_mut(key).map(|v| *v)
    }
}

/// What a janitor pass deleted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JanitorReport {
    pub deploy_logs: u64,
    pub deployments: u64,
    pub cron_job_runs: u64,
    pub audit_log: u64,
    pub metrics: u64,
    pub alerts: u64,
    pub sessions: u64,
}

impl JanitorReport {
    fn total(&self) -> u64 {
        self.deploy_logs
            + self.deployments
            + self.cron_job_runs
            + self.audit_log
            + self.metrics
            + self.alerts
            + self.sessions
    }
}

pub async fn run_janitor(state: AppState) {
    let lp = loops::register("janitor");
    lp.sleep(STARTUP_DELAY).await;
    loop {
        let run = lp.run();
        let policy = RetentionPolicy::load(&state.db).await;
        match run_once(&state.db, &policy).await {
            Ok(report) if report.total() > 0 => {
                tracing::info!("Janitor: deleted {:?}", report);
                drop(run);
            }
            Ok(_) => {
                tracing::debug!("Janitor: nothing to delete");
                drop(run);
            }
            Err(e) => {
                tracing::warn!("Janitor pass failed: {}", e);
                run.fail(&e);
            }
        }
        // Refresh planner statistics for tables whose shape changed.
        let _ = sqlx::query("PRAGMA optimize").execute(&state.db).await;
        // Volume folders of previews that no longer exist.
        match prune_preview_volumes(&state.db, &crate::volumes::data_dir()).await {
            Ok(n) if n > 0 => tracing::info!("Janitor: removed {n} preview volume folder(s)"),
            Ok(_) => {}
            Err(e) => tracing::warn!("Janitor: preview volume prune failed: {e}"),
        }
        // Access logs of deleted apps (<data>/access, written by tond-proxy).
        match crate::access_logs::prune_orphans(&state.db, &crate::auth::secret_file::data_dir())
            .await
        {
            Ok(n) if n > 0 => tracing::info!("Janitor: removed {n} access log file(s)"),
            Ok(_) => {}
            Err(e) => tracing::warn!("Janitor: access log prune failed: {e}"),
        }
        // Networks of finished previews and cron runs, deleted apps, …
        match docker::prune_networks(&state.docker).await {
            Ok(n) if n > 0 => tracing::info!("Janitor: removed {n} unused Docker network(s)"),
            Ok(_) => {}
            Err(e) => tracing::warn!("Janitor: network prune failed: {e}"),
        }
        lp.sleep(INTERVAL).await;
    }
}

/// Run one full retention pass.
pub async fn run_once(db: &SqlitePool, policy: &RetentionPolicy) -> Result<JanitorReport> {
    let mut report = JanitorReport::default();
    let now = Utc::now();

    let app_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM apps")
        .fetch_all(db)
        .await?;

    // ── deploy_logs ──
    let deploy_log_age_cutoff = (policy.deploy_log_days > 0)
        .then(|| (now - chrono::Duration::days(policy.deploy_log_days)).to_rfc3339());
    for app_id in &app_ids {
        let nth_start = nth_newest(
            db,
            "SELECT created_at FROM deployments WHERE app_id = ? ORDER BY created_at DESC LIMIT 1 OFFSET ?",
            app_id,
            policy.deploy_log_deployments,
        )
        .await?;
        let latest_start: Option<String> =
            sqlx::query_scalar("SELECT MAX(created_at) FROM deployments WHERE app_id = ?")
                .bind(app_id)
                .fetch_one(db)
                .await?;
        let Some(cutoff) = retention_cutoff(
            nth_start.as_deref(),
            latest_start.as_deref(),
            deploy_log_age_cutoff.as_deref(),
        ) else {
            continue;
        };
        report.deploy_logs += delete_in_batches(|| {
            sqlx::query(
                "DELETE FROM deploy_logs WHERE rowid IN (
                    SELECT rowid FROM deploy_logs WHERE app_id = ? AND created_at < ? LIMIT ?)",
            )
            .bind(app_id)
            .bind(&cutoff)
            .bind(BATCH_SIZE)
            .execute(db)
        })
        .await?;
    }

    // ── deployments ──
    report.deployments += delete_in_batches(|| {
        sqlx::query(
            "DELETE FROM deployments WHERE rowid IN (
                SELECT rowid FROM deployments
                WHERE app_id NOT IN (SELECT id FROM apps)
                  AND status NOT IN ('queued', 'running', 'pending')
                LIMIT ?)",
        )
        .bind(BATCH_SIZE)
        .execute(db)
    })
    .await?;
    if policy.deployments_per_app > 0 {
        for app_id in &app_ids {
            report.deployments += delete_in_batches(|| {
                sqlx::query(
                    "DELETE FROM deployments WHERE rowid IN (
                        SELECT rowid FROM deployments
                        WHERE app_id = ?1
                          AND status NOT IN ('queued', 'running', 'pending')
                          AND id NOT IN (
                              SELECT id FROM deployments WHERE app_id = ?1
                              ORDER BY created_at DESC LIMIT ?2)
                          AND id NOT IN (
                              SELECT id FROM deployments
                              WHERE app_id = ?1 AND status = 'succeeded' AND image_tag IS NOT NULL
                                AND image_pruned = 0
                              ORDER BY created_at DESC LIMIT ?3)
                        LIMIT ?4)",
                )
                .bind(app_id)
                .bind(policy.deployments_per_app)
                .bind(KEEP_ROLLBACK_IMAGES)
                .bind(BATCH_SIZE)
                .execute(db)
            })
            .await?;
        }
    }

    // ── cron_job_runs ──
    let cron_age_cutoff = (policy.cron_run_days > 0)
        .then(|| (now - chrono::Duration::days(policy.cron_run_days)).to_rfc3339());
    let job_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM cron_jobs")
        .fetch_all(db)
        .await?;
    for job_id in &job_ids {
        let nth_start = nth_newest(
            db,
            "SELECT started_at FROM cron_job_runs WHERE cron_job_id = ? ORDER BY started_at DESC LIMIT 1 OFFSET ?",
            job_id,
            policy.cron_runs_per_job,
        )
        .await?;
        let latest_start: Option<String> =
            sqlx::query_scalar("SELECT MAX(started_at) FROM cron_job_runs WHERE cron_job_id = ?")
                .bind(job_id)
                .fetch_one(db)
                .await?;
        let Some(cutoff) = retention_cutoff(
            nth_start.as_deref(),
            latest_start.as_deref(),
            cron_age_cutoff.as_deref(),
        ) else {
            continue;
        };
        report.cron_job_runs += delete_in_batches(|| {
            sqlx::query(
                "DELETE FROM cron_job_runs WHERE rowid IN (
                    SELECT rowid FROM cron_job_runs
                    WHERE cron_job_id = ? AND started_at < ? AND status != 'running'
                    LIMIT ?)",
            )
            .bind(job_id)
            .bind(&cutoff)
            .bind(BATCH_SIZE)
            .execute(db)
        })
        .await?;
    }

    // ── audit_log (RFC 3339 timestamps; keeps the hash chain verifiable) ──
    if policy.audit_log_days > 0 {
        let cutoff = (now - chrono::Duration::days(policy.audit_log_days))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        report.audit_log += crate::audit_log::prune_before(db, &cutoff, BATCH_SIZE).await?;
    }

    // ── metrics (fixed tiers, see metrics/store.rs) ──
    let event_keep_secs = if policy.app_events_days > 0 {
        policy.app_events_days.saturating_mul(86_400)
    } else {
        i64::MAX / 2
    };
    report.metrics += metrics::store::prune(db, now.timestamp(), event_keep_secs).await?;
    let days = |d: i64| {
        if d > 0 {
            d.saturating_mul(86_400)
        } else {
            i64::MAX / 2
        }
    };
    report.metrics +=
        metrics::host_history::prune(db, now.timestamp(), days(policy.host_history_days)).await?;
    report.metrics +=
        crate::uptime::store::prune(db, now.timestamp(), days(policy.uptime_days)).await?;
    report.metrics +=
        metrics::requests::prune(db, now.timestamp(), days(policy.request_metrics_days)).await?;

    // ── alert history (unix seconds; deliveries cascade) ──
    let _ = alerts::evaluate::close_orphans(db, now.timestamp()).await;
    if policy.alert_history_days > 0 {
        let cutoff = now.timestamp() - policy.alert_history_days * 86_400;
        report.alerts += delete_in_batches(|| {
            sqlx::query(
                "DELETE FROM alert_history WHERE id IN (
                    SELECT id FROM alert_history WHERE resolved_at < ? LIMIT ?)",
            )
            .bind(cutoff)
            .bind(BATCH_SIZE)
            .execute(db)
        })
        .await?;
    }

    // ── expired sessions and MFA challenges (RFC 3339 timestamps) ──
    let now_str = now.to_rfc3339();
    report.sessions += delete_in_batches(|| {
        sqlx::query(
            "DELETE FROM sessions WHERE rowid IN (
                SELECT rowid FROM sessions WHERE expires_at < ? LIMIT ?)",
        )
        .bind(&now_str)
        .bind(BATCH_SIZE)
        .execute(db)
    })
    .await?;
    let _ = sqlx::query("DELETE FROM mfa_pending WHERE expires_at < ?")
        .bind(&now_str)
        .execute(db)
        .await?;
    let _ = sqlx::query("DELETE FROM password_links WHERE expires_at < ?")
        .bind(&now_str)
        .execute(db)
        .await?;
    // Sessions idle past the timeout can never be used again.
    let idle_hours: i64 = sqlx::query_scalar(
        "SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'session_idle_timeout_hours'",
    )
    .fetch_optional(db)
    .await?
    .filter(|h: &i64| *h > 0)
    .unwrap_or(tond_common::DEFAULT_SESSION_IDLE_HOURS);
    let idle_cutoff = (now - chrono::Duration::hours(idle_hours)).to_rfc3339();
    report.sessions += delete_in_batches(|| {
        sqlx::query(
            "DELETE FROM sessions WHERE rowid IN (
                SELECT rowid FROM sessions WHERE COALESCE(last_used_at, created_at) < ? LIMIT ?)",
        )
        .bind(&idle_cutoff)
        .bind(BATCH_SIZE)
        .execute(db)
    })
    .await?;

    // ── expired one-time tokens and flows ──
    for sql in [
        "DELETE FROM terminal_tickets WHERE expires_at < ?",
        "DELETE FROM github_app_flows WHERE expires_at < ?",
    ] {
        sqlx::query(sql).bind(&now_str).execute(db).await?;
    }

    // ── webhook replay IDs past the replay window (webhooks.rs keeps 72 h) ──
    let replay_cutoff = (now - chrono::Duration::hours(72)).to_rfc3339();
    delete_in_batches(|| {
        sqlx::query(
            "DELETE FROM webhook_deliveries WHERE rowid IN (
                SELECT rowid FROM webhook_deliveries WHERE received_at < ? LIMIT ?)",
        )
        .bind(&replay_cutoff)
        .bind(BATCH_SIZE)
        .execute(db)
    })
    .await?;

    // ── idempotency keys past their 24 h window, and the row cap ──
    crate::idempotency::prune(db, now.timestamp()).await?;

    // ── finished preview rows (their containers are already gone) ──
    let preview_cutoff = (now - chrono::Duration::days(7)).to_rfc3339();
    sqlx::query(
        "DELETE FROM preview_deployments
         WHERE status IN ('expired', 'failed') AND expires_at < ?",
    )
    .bind(&preview_cutoff)
    .execute(db)
    .await?;

    Ok(report)
}

/// Remove `<data>/volumes/_preview/<slug>` folders whose preview is gone or
/// expired. Returns how many folders were removed.
pub async fn prune_preview_volumes(db: &SqlitePool, data_dir: &std::path::Path) -> Result<u64> {
    let root = crate::volumes::preview_volumes_root(data_dir);
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return Ok(0);
    };
    let live: std::collections::HashSet<String> =
        sqlx::query_scalar("SELECT slug FROM preview_deployments WHERE status != 'expired'")
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        // Never follow a symlink out of the volumes tree; plain dirs only.
        if !entry.file_type().await?.is_dir() || live.contains(&name) {
            continue;
        }
        tokio::fs::remove_dir_all(entry.path()).await?;
        removed += 1;
    }
    Ok(removed)
}

/// The timestamp of the `n`-th newest row (1-based) returned by `sql`, which
/// must take `(key, offset)` binds. `None` when there are fewer than `n` rows
/// or `n <= 0` (rule disabled).
async fn nth_newest(
    db: &SqlitePool,
    sql: &'static str,
    key: &str,
    n: i64,
) -> Result<Option<String>> {
    if n <= 0 {
        return Ok(None);
    }
    Ok(sqlx::query_scalar(sql)
        .bind(key)
        .bind(n - 1)
        .fetch_optional(db)
        .await?)
}

/// Combine the count rule and the age rule into one "delete rows older than"
/// cutoff (all timestamps are RFC 3339 strings, which sort chronologically).
///
/// - `nth_start`: start of the N-th newest parent (deployment / run); rows
///   before it fall outside the "keep last N" window.
/// - `latest_start`: start of the newest parent; the age rule never deletes
///   rows at or after it, so the latest deployment's logs / latest run survive.
/// - `age_cutoff`: now minus the max age.
///
/// A row is deleted when it violates either rule, i.e. when it is older than
/// the later of the two cutoffs.
pub fn retention_cutoff(
    nth_start: Option<&str>,
    latest_start: Option<&str>,
    age_cutoff: Option<&str>,
) -> Option<String> {
    let by_age = match (age_cutoff, latest_start) {
        (Some(age), Some(latest)) => Some(age.min(latest)),
        (Some(age), None) => Some(age),
        (None, _) => None,
    };
    match (nth_start, by_age) {
        (Some(a), Some(b)) => Some(a.max(b).to_string()),
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    }
}

/// Run a batched DELETE until it removes fewer than BATCH_SIZE rows.
async fn delete_in_batches<F, Fut>(mut run_batch: F) -> Result<u64>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>>,
{
    let mut total = 0;
    loop {
        let deleted = run_batch().await?.rows_affected();
        total += deleted;
        if deleted < BATCH_SIZE as u64 {
            return Ok(total);
        }
        tokio::time::sleep(BATCH_PAUSE).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for janitor tests: {err}"));
        db
    }

    fn ts(days_ago: i64, minutes: i64) -> String {
        (Utc::now() - chrono::Duration::days(days_ago) + chrono::Duration::minutes(minutes))
            .to_rfc3339()
    }

    async fn seed_app(db: &SqlitePool, id: &str) {
        sqlx::query("INSERT INTO apps (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(format!("name-{id}"))
            .bind(ts(0, 0))
            .bind(ts(0, 0))
            .execute(db)
            .await
            .unwrap_or_else(|err| panic!("seed app: {err}"));
    }

    async fn seed_deployment(
        db: &SqlitePool,
        id: &str,
        app_id: &str,
        status: &str,
        image: Option<&str>,
        created_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO deployments (id, app_id, image_tag, status, started_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(app_id)
        .bind(image)
        .bind(status)
        .bind(created_at)
        .bind(created_at)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("seed deployment: {err}"));
    }

    async fn seed_log(db: &SqlitePool, id: &str, app_id: &str, created_at: &str) {
        sqlx::query(
            "INSERT INTO deploy_logs (id, app_id, message, level, created_at) VALUES (?, ?, 'x', 'info', ?)",
        )
        .bind(id)
        .bind(app_id)
        .bind(created_at)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("seed log: {err}"));
    }

    async fn ids(db: &SqlitePool, sql: &'static str) -> Vec<String> {
        sqlx::query_scalar(sql)
            .fetch_all(db)
            .await
            .unwrap_or_else(|err| panic!("query ids: {err}"))
    }

    #[test]
    fn retention_cutoff_takes_the_later_of_count_and_age_rules() {
        // Count rule only.
        assert_eq!(
            retention_cutoff(Some("2026-01-05"), Some("2026-01-09"), None).as_deref(),
            Some("2026-01-05")
        );
        // Age rule only; never past the latest parent's start.
        assert_eq!(
            retention_cutoff(None, Some("2026-01-09"), Some("2026-02-01")).as_deref(),
            Some("2026-01-09"),
            "the age rule must keep rows of the most recent deployment/run"
        );
        assert_eq!(
            retention_cutoff(None, None, Some("2026-02-01")).as_deref(),
            Some("2026-02-01")
        );
        // Both: whichever deletes more.
        assert_eq!(
            retention_cutoff(Some("2026-01-05"), Some("2026-03-01"), Some("2026-02-01")).as_deref(),
            Some("2026-02-01")
        );
        assert_eq!(
            retention_cutoff(Some("2026-02-05"), Some("2026-03-01"), Some("2026-02-01")).as_deref(),
            Some("2026-02-05")
        );
        assert_eq!(retention_cutoff(None, Some("2026-01-09"), None), None);
    }

    #[test]
    fn policy_reads_settings_and_ignores_garbage() {
        let policy = RetentionPolicy::from_pairs(
            [
                ("retention_audit_log_days", "365"),
                ("retention_deploy_logs_days", "not-a-number"),
                ("retention_unknown", "5"),
                ("retention_cron_runs_per_job", " 0 "),
            ]
            .into_iter(),
        );
        assert_eq!(policy.audit_log_days, 365);
        assert_eq!(
            policy.deploy_log_days, 90,
            "garbage falls back to the default"
        );
        assert_eq!(policy.cron_runs_per_job, 0, "0 disables the rule");
        assert_eq!(policy.deploy_log_deployments, 30);
        assert_eq!(policy.app_events_days, 90);
        assert_eq!(policy.get("retention_audit_log_days"), Some(365));
        assert_eq!(policy.get("retention_nope"), None);
    }

    #[tokio::test]
    async fn deploy_logs_keep_last_n_deployments_and_latest_deployment_regardless_of_age() {
        let db = test_db().await;
        seed_app(&db, "a1").await;
        // Three deployments, 60, 50 and 40 days ago; each wrote one log line.
        for (i, days) in [(1, 60), (2, 50), (3, 40)] {
            seed_deployment(&db, &format!("d{i}"), "a1", "succeeded", None, &ts(days, 0)).await;
            seed_log(&db, &format!("l{i}"), "a1", &ts(days, 1)).await;
        }
        let policy = RetentionPolicy {
            deploy_log_deployments: 2,
            deploy_log_days: 30,
            ..RetentionPolicy::default()
        };

        let report = run_once(&db, &policy)
            .await
            .unwrap_or_else(|err| panic!("janitor pass: {err}"));

        // l1 is outside the last-2 window; l2 is older than 30 days and not the
        // latest deployment; l3 belongs to the latest deployment and survives.
        assert_eq!(
            ids(&db, "SELECT id FROM deploy_logs ORDER BY id").await,
            vec!["l3"]
        );
        assert_eq!(report.deploy_logs, 2);
    }

    #[tokio::test]
    async fn deployments_keep_last_n_rollback_images_running_rows_and_drop_orphans() {
        let db = test_db().await;
        seed_app(&db, "a1").await;
        // Oldest first: an image-bearing success, a failure, a running row, then
        // three recent failures.
        seed_deployment(&db, "old-ok", "a1", "succeeded", Some("img:1"), &ts(10, 0)).await;
        seed_deployment(&db, "old-fail", "a1", "failed", None, &ts(9, 0)).await;
        seed_deployment(&db, "stuck", "a1", "running", None, &ts(8, 0)).await;
        for i in 0..3 {
            seed_deployment(&db, &format!("new-{i}"), "a1", "failed", None, &ts(1, i)).await;
        }
        // Deployment of an app that was deleted.
        seed_deployment(&db, "orphan", "gone", "failed", None, &ts(1, 0)).await;

        let policy = RetentionPolicy {
            deployments_per_app: 3,
            ..RetentionPolicy::default()
        };
        run_once(&db, &policy)
            .await
            .unwrap_or_else(|err| panic!("janitor pass: {err}"));

        assert_eq!(
            ids(&db, "SELECT id FROM deployments ORDER BY id").await,
            vec!["new-0", "new-1", "new-2", "old-ok", "stuck"],
            "keeps last 3, the rollback image and the running row; drops old failures and orphans"
        );
    }

    #[tokio::test]
    async fn resolved_alerts_are_kept_90_days_and_orphans_are_closed() {
        let db = test_db().await;
        let now = Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO integrations (id, name, kind, config) VALUES ('i', 'i', 'webhook', '{}')",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("seed integration: {err}"));
        for (id, resolved) in [
            ("old", Some(now - 91 * 86_400)),
            ("recent", Some(now - 89 * 86_400)),
            ("open", None),
        ] {
            sqlx::query(
                "INSERT INTO alert_history (id, rule_name, kind, severity, target, target_label,
                    threshold, pending_since, fired_at, resolved_at)
                 VALUES (?, 'r', 'cpu', 'warning', 'host', 'Server', 1, 0, 0, ?)",
            )
            .bind(id)
            .bind(resolved)
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed episode: {err}"));
            sqlx::query(
                "INSERT INTO alert_deliveries (id, episode_id, integration_id, event, next_attempt_at, created_at)
                 VALUES (?, ?, 'i', 'firing', 0, 0)",
            )
            .bind(format!("d-{id}"))
            .bind(id)
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed delivery: {err}"));
        }
        let report = run_once(&db, &RetentionPolicy::default())
            .await
            .unwrap_or_else(|err| panic!("janitor pass: {err}"));
        assert_eq!(report.alerts, 1);
        assert_eq!(
            ids(&db, "SELECT id FROM alert_history ORDER BY id").await,
            vec!["open", "recent"]
        );
        assert_eq!(
            ids(&db, "SELECT id FROM alert_deliveries ORDER BY id").await,
            vec!["d-open", "d-recent"],
            "deliveries go with their episode"
        );
        let open_resolved: Option<i64> =
            sqlx::query_scalar("SELECT resolved_at FROM alert_history WHERE id = 'open'")
                .fetch_one(&db)
                .await
                .unwrap_or_else(|err| panic!("read: {err}"));
        assert!(
            open_resolved.is_some(),
            "an episode no state points at is closed"
        );
    }

    #[tokio::test]
    async fn cron_runs_audit_metrics_and_sessions_are_pruned() {
        let db = test_db().await;
        sqlx::query("INSERT INTO cron_jobs (id, name, schedule, image) VALUES ('j1', 'job', '* * * * *', 'alpine')")
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed job: {err}"));
        // Five runs: 40, 35 days ago (old), and three recent. One old run is still 'running'.
        let runs = [
            ("r-old-1", 40, "success"),
            ("r-old-running", 35, "running"),
            ("r-new-1", 3, "success"),
            ("r-new-2", 2, "failed"),
            ("r-new-3", 1, "success"),
        ];
        for (id, days, status) in runs {
            sqlx::query(
                "INSERT INTO cron_job_runs (id, cron_job_id, status, started_at) VALUES (?, 'j1', ?, ?)",
            )
            .bind(id)
            .bind(status)
            .bind(ts(days, 0))
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed run: {err}"));
        }

        let actor = crate::audit_log::AuditActor::system("test");
        for (action, days) in [("au-old", 200), ("au-new", 1)] {
            crate::audit_log::record_at(
                &db,
                &actor,
                crate::audit_log::AuditEntry::new(action),
                &(chrono::Utc::now() - chrono::Duration::days(days))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            )
            .await;
        }
        // Metric samples of cron job j1: one past the 25 h raw retention, one fresh.
        sqlx::query("INSERT INTO metric_targets (id, kind, owner_id) VALUES (1, 'cron', 'j1')")
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed target: {err}"));
        let now = Utc::now().timestamp();
        sqlx::query("INSERT INTO metrics_raw (target, ts, cpu, mem, rx, tx, pids) VALUES (1, ?, 0, 0, 0, 0, 0), (1, ?, 0, 0, 0, 0, 0)")
            .bind(now - 26 * 3600)
            .bind(now - 60)
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed metrics: {err}"));
        sqlx::query("INSERT INTO users (id, email, password_hash, created_at, updated_at) VALUES ('u1', 'a@b.c', 'h', 'now', 'now')")
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed user: {err}"));
        for (id, expires) in [("s-expired", ts(1, 0)), ("s-live", ts(-1, 0))] {
            sqlx::query("INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, 'u1', ?, 'now')")
                .bind(id)
                .bind(expires)
                .execute(&db)
                .await
                .unwrap_or_else(|err| panic!("seed session: {err}"));
        }

        let policy = RetentionPolicy {
            cron_runs_per_job: 2,
            cron_run_days: 30,
            ..RetentionPolicy::default()
        };
        let report = run_once(&db, &policy)
            .await
            .unwrap_or_else(|err| panic!("janitor pass: {err}"));

        assert_eq!(
            ids(&db, "SELECT id FROM cron_job_runs ORDER BY id").await,
            vec!["r-new-2", "r-new-3", "r-old-running"],
            "keeps the last 2 runs and never deletes a running row"
        );
        assert_eq!(
            ids(&db, "SELECT action FROM audit_log ORDER BY seq").await,
            vec!["au-new", "audit_pruned"],
            "old entries are pruned and the pruning is recorded"
        );
        assert!(
            crate::audit_log::verify(&db)
                .await
                .unwrap_or_else(|err| panic!("verify: {err}"))
                .ok,
            "the chain still verifies after retention"
        );
        let raw: Vec<i64> = sqlx::query_scalar("SELECT ts FROM metrics_raw")
            .fetch_all(&db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(raw, vec![now - 60]);
        assert_eq!(report.metrics, 1);
        assert_eq!(ids(&db, "SELECT id FROM sessions").await, vec!["s-live"]);
        assert_eq!(report.cron_job_runs, 2);
        assert_eq!(report.sessions, 1);
    }

    #[tokio::test]
    async fn expired_tokens_old_replay_ids_and_finished_previews_are_pruned() {
        let db = test_db().await;
        seed_app(&db, "a1").await;
        let past = (Utc::now() - chrono::Duration::days(10)).to_rfc3339();
        let future = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at)
             VALUES ('u1', 'a@b.c', 'x', 'admin', 'now', 'now')",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("seed user: {err}"));

        for (state, expires) in [("flow-old", &past), ("flow-live", &future)] {
            sqlx::query(
                "INSERT INTO github_app_flows (state, kind, user_id, expires_at, created_at)
                 VALUES (?, 'manifest', 'u1', ?, ?)",
            )
            .bind(state)
            .bind(expires)
            .bind(&past)
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed flow: {err}"));
        }
        for (key, at) in [("old", &past), ("recent", &future)] {
            sqlx::query("INSERT INTO webhook_deliveries (key, received_at) VALUES (?, ?)")
                .bind(key)
                .bind(at)
                .execute(&db)
                .await
                .unwrap_or_else(|err| panic!("seed delivery: {err}"));
        }
        for (id, status, expires) in [
            ("p-expired", "expired", &past),
            ("p-running", "running", &past),
            ("p-recent", "expired", &future),
        ] {
            sqlx::query(
                "INSERT INTO preview_deployments (id, app_id, slug, status, expires_at)
                 VALUES (?, 'a1', ?, ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(status)
            .bind(expires)
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed preview: {err}"));
        }

        run_once(&db, &RetentionPolicy::default())
            .await
            .unwrap_or_else(|err| panic!("janitor pass: {err}"));

        assert_eq!(
            ids(&db, "SELECT state FROM github_app_flows").await,
            vec!["flow-live"]
        );
        assert_eq!(
            ids(&db, "SELECT key FROM webhook_deliveries").await,
            vec!["recent"]
        );
        assert_eq!(
            ids(&db, "SELECT id FROM preview_deployments ORDER BY id").await,
            vec!["p-recent", "p-running"],
            "running previews and recently expired rows stay"
        );
    }

    #[tokio::test]
    async fn preview_volume_folders_of_gone_previews_are_removed() {
        let db = test_db().await;
        seed_app(&db, "a1").await;
        for (id, status) in [("live1", "running"), ("gone1", "expired")] {
            sqlx::query(
                "INSERT INTO preview_deployments (id, app_id, slug, status, expires_at)
                 VALUES (?, 'a1', ?, ?, '2099-01-01T00:00:00Z')",
            )
            .bind(id)
            .bind(id)
            .bind(status)
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed preview: {err}"));
        }
        let dir = tempfile::TempDir::new().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let root = crate::volumes::preview_volumes_root(dir.path());
        for slug in ["live1", "gone1", "orphan"] {
            std::fs::create_dir_all(root.join(slug).join("data"))
                .unwrap_or_else(|err| panic!("mkdir: {err}"));
        }

        let removed = prune_preview_volumes(&db, dir.path())
            .await
            .unwrap_or_else(|err| panic!("prune: {err}"));

        assert_eq!(removed, 2);
        assert!(root.join("live1").exists(), "a live preview keeps its data");
        assert!(!root.join("gone1").exists());
        assert!(!root.join("orphan").exists());
    }

    #[tokio::test]
    async fn batched_deletes_remove_more_than_one_batch() {
        let db = test_db().await;
        seed_app(&db, "a1").await;
        seed_deployment(&db, "d-new", "a1", "succeeded", None, &ts(0, 0)).await;
        seed_deployment(&db, "d-old", "a1", "succeeded", None, &ts(5, 0)).await;
        let old = ts(5, 1);
        let total = BATCH_SIZE as usize + 10;
        let mut tx = db
            .begin()
            .await
            .unwrap_or_else(|err| panic!("begin: {err}"));
        for i in 0..total {
            sqlx::query("INSERT INTO deploy_logs (id, app_id, message, created_at) VALUES (?, 'a1', 'x', ?)")
                .bind(format!("l{i}"))
                .bind(&old)
                .execute(&mut *tx)
                .await
                .unwrap_or_else(|err| panic!("seed log: {err}"));
        }
        tx.commit()
            .await
            .unwrap_or_else(|err| panic!("commit: {err}"));

        let policy = RetentionPolicy {
            deploy_log_deployments: 1,
            ..RetentionPolicy::default()
        };
        let report = run_once(&db, &policy)
            .await
            .unwrap_or_else(|err| panic!("janitor pass: {err}"));
        assert_eq!(report.deploy_logs, total as u64);
    }
}
