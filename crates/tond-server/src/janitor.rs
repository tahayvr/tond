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
//                    building, deploying) and the last few succeeded deployments with an
//                    image (the rollback candidates) are never deleted. Rows
//                    whose app no longer exists are deleted.
//   cron_job_runs  — per job, keep the last N runs and drop runs older than M
//                    days (the latest run is always kept; running rows never
//                    deleted).
//   audit_log      — drop rows older than N days (security data: default 90).
//   metrics        — drop samples older than N days (was a full-table scan
//                    every 30 s in metrics.rs; now indexed and hourly).
//   sessions / mfa_pending — drop expired rows.
//   terminal_tickets / github_app_flows — drop expired rows.
//   webhook_deliveries — drop replay IDs older than the 72 h replay window.
//   preview_deployments — drop expired/failed rows a week after they expired.
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
use crate::state::AppState;

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

/// Retention settings. Each field maps to a key in the `settings` table.
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
    /// `retention_metrics_days` — max age of metric samples.
    pub metrics_days: i64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            deploy_log_deployments: 20,
            deploy_log_days: 30,
            deployments_per_app: 50,
            cron_runs_per_job: 100,
            cron_run_days: 30,
            audit_log_days: 90,
            metrics_days: 7,
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

    fn from_pairs<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut policy = Self::default();
        for (key, value) in pairs {
            let Ok(n) = value.trim().parse::<i64>() else {
                continue;
            };
            let field = match key {
                "retention_deploy_logs_deployments" => &mut policy.deploy_log_deployments,
                "retention_deploy_logs_days" => &mut policy.deploy_log_days,
                "retention_deployments_per_app" => &mut policy.deployments_per_app,
                "retention_cron_runs_per_job" => &mut policy.cron_runs_per_job,
                "retention_cron_runs_days" => &mut policy.cron_run_days,
                "retention_audit_log_days" => &mut policy.audit_log_days,
                "retention_metrics_days" => &mut policy.metrics_days,
                _ => continue,
            };
            *field = n;
        }
        policy
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
    pub sessions: u64,
}

impl JanitorReport {
    fn total(&self) -> u64 {
        self.deploy_logs
            + self.deployments
            + self.cron_job_runs
            + self.audit_log
            + self.metrics
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
                  AND status NOT IN ('queued', 'building', 'deploying')
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
                          AND status NOT IN ('queued', 'building', 'deploying')
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

    // ── audit_log (created_at uses SQLite's datetime('now') format) ──
    if policy.audit_log_days > 0 {
        let modifier = format!("-{} days", policy.audit_log_days);
        report.audit_log += delete_in_batches(|| {
            sqlx::query(
                "DELETE FROM audit_log WHERE rowid IN (
                    SELECT rowid FROM audit_log WHERE created_at < datetime('now', ?) LIMIT ?)",
            )
            .bind(&modifier)
            .bind(BATCH_SIZE)
            .execute(db)
        })
        .await?;
    }

    // ── metrics (collected_at uses datetime('now') format) ──
    if policy.metrics_days > 0 {
        let modifier = format!("-{} days", policy.metrics_days);
        report.metrics += delete_in_batches(|| {
            sqlx::query(
                "DELETE FROM metrics WHERE rowid IN (
                    SELECT rowid FROM metrics WHERE collected_at < datetime('now', ?) LIMIT ?)",
            )
            .bind(&modifier)
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
                ("retention_metrics_days", "not-a-number"),
                ("retention_unknown", "5"),
                ("retention_cron_runs_per_job", " 0 "),
            ]
            .into_iter(),
        );
        assert_eq!(policy.audit_log_days, 365);
        assert_eq!(policy.metrics_days, 7, "garbage falls back to the default");
        assert_eq!(policy.cron_runs_per_job, 0, "0 disables the rule");
        assert_eq!(policy.deploy_log_deployments, 20);
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
        seed_deployment(&db, "stuck", "a1", "building", None, &ts(8, 0)).await;
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

        sqlx::query("INSERT INTO audit_log (id, action, actor, created_at) VALUES ('au-old', 'x', 'y', datetime('now', '-100 days')), ('au-new', 'x', 'y', datetime('now', '-1 days'))")
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("seed audit: {err}"));
        sqlx::query("INSERT INTO metrics (id, app_id, collected_at) VALUES ('m-old', 'a', datetime('now', '-8 days')), ('m-new', 'a', datetime('now', '-1 days'))")
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
        assert_eq!(ids(&db, "SELECT id FROM audit_log").await, vec!["au-new"]);
        assert_eq!(ids(&db, "SELECT id FROM metrics").await, vec!["m-new"]);
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
