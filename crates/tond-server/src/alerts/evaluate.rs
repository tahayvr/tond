// alerts/evaluate.rs — observe every enabled rule and advance its states
//
// `evaluate` runs on the metrics collector's 30 s tick and when something an
// event-style rule watches changes (deploys, snapshots, certificates, cron
// runs, container events). For each rule it gathers one observation per
// target (value + whether the condition holds, or "no data"), runs the state
// machine, and writes only what changed — so an idle server with quiet rules
// writes nothing. Notifications are queued as deliveries (sent by
// deliver.rs); quiet hours decide when each may go out.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tond_common::{
    ALERT_SNOOZE_UNTIL_RESOLVED, AlertComparison, AlertKindInfo, AlertRule, AlertScope,
    ChannelQuietHours, QuietHours, alert_kind,
};
use uuid::Uuid;

use super::machine::{self, Phase, Transition};
use super::quiet;
use super::sources::{self, Forecast, RequestWindow};
use crate::metrics;
use crate::notifications::NoticeEvent;

/// One target's reading for a rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub target: String,
    pub label: String,
    pub app_id: Option<String>,
    pub value: Option<f64>,
    /// None: no data (the state is held)
    pub holds: Option<bool>,
}

/// Readings the collector holds in memory.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    /// app id → (CPU in percent of one core, memory bytes)
    pub app_samples: HashMap<String, (f64, i64)>,
    pub disk_percent: Option<f64>,
    pub disk_forecast: Option<Forecast>,
    /// Apps whose uptime check is down.
    pub uptime_down: HashSet<String>,
}

/// Where live readings come from.
pub enum Source {
    /// The metrics collector's in-memory state.
    Live,
    /// Fixed readings (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(Inputs),
}

/// A sample older than this is not "current".
const SAMPLE_MAX_AGE: i64 = 120;

impl Source {
    fn app_sample(&self, app_id: &str, now: i64) -> Option<(f64, i64)> {
        match self {
            Source::Live => {
                metrics::current(&metrics::Owner::new(metrics::TargetKind::App, app_id))
                    .filter(|c| now - c.ts <= SAMPLE_MAX_AGE)
                    .map(|c| {
                        (
                            metrics::store::centi_to_percent(c.sample.cpu as f64),
                            c.sample.mem,
                        )
                    })
            }
            Source::Fixed(i) => i.app_samples.get(app_id).copied(),
        }
    }

    fn disk_percent(&self, now: i64) -> Option<f64> {
        match self {
            Source::Live => metrics::host::snapshot()
                .filter(|s| s.disk_total > 0 && now - s.collected_at <= SAMPLE_MAX_AGE)
                .map(|s| s.disk_percent()),
            Source::Fixed(i) => i.disk_percent,
        }
    }

    async fn disk_forecast(&self, db: &SqlitePool, now: i64) -> Forecast {
        match self {
            Source::Live => sources::disk_forecast(db, now).await,
            Source::Fixed(i) => i.disk_forecast.unwrap_or(Forecast::Unknown),
        }
    }

    /// The app's uptime check is down (probed through DNS, TLS and the proxy).
    fn uptime_down(&self, app_id: &str) -> bool {
        match self {
            Source::Live => crate::uptime::app_is_down(app_id),
            Source::Fixed(i) => i.uptime_down.contains(app_id),
        }
    }
}

/// What one evaluation did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub rules: usize,
    pub fired: usize,
    pub resolved: usize,
    /// Deliveries queued to be sent now or after quiet hours.
    pub queued: usize,
}

/// Only one evaluation at a time (tick and events may overlap).
static EVAL_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Latest value of every pending/firing (rule, target), for the UI.
static LIVE: LazyLock<Mutex<HashMap<(String, String), f64>>> = LazyLock::new(Mutex::default);

/// The latest value seen for a pending/firing alert.
pub fn live_value(rule_id: &str, target: &str) -> Option<f64> {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(rule_id.to_string(), target.to_string()))
        .copied()
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AppRow {
    id: String,
    name: String,
    status: String,
}

/// Lazily loaded data shared by the rules of one evaluation.
struct Ctx<'a> {
    db: &'a SqlitePool,
    now: i64,
    source: &'a Source,
    apps: Option<Vec<AppRow>>,
    mem_limits: Option<HashMap<String, i64>>,
    requests: HashMap<i64, Option<HashMap<String, RequestWindow>>>,
    global_quiet: Option<QuietHours>,
}

impl Ctx<'_> {
    async fn apps(&mut self) -> Result<&Vec<AppRow>> {
        if self.apps.is_none() {
            let rows =
                sqlx::query_as::<_, AppRow>("SELECT id, name, status FROM apps ORDER BY name")
                    .fetch_all(self.db)
                    .await?;
            self.apps = Some(rows);
        }
        Ok(self.apps.get_or_insert_with(Vec::new))
    }

    async fn scope_apps(&mut self, rule: &AlertRule) -> Result<Vec<AppRow>> {
        let apps = self.apps().await?;
        Ok(match &rule.app_id {
            Some(id) => apps.iter().filter(|a| &a.id == id).cloned().collect(),
            None => apps.clone(),
        })
    }

    async fn mem_limits(&mut self) -> Result<&HashMap<String, i64>> {
        if self.mem_limits.is_none() {
            let rows: Vec<(String, i64)> =
                sqlx::query_as("SELECT owner_id, mem_limit FROM metric_targets WHERE kind = 'app'")
                    .fetch_all(self.db)
                    .await?;
            self.mem_limits = Some(rows.into_iter().collect());
        }
        Ok(self.mem_limits.get_or_insert_with(HashMap::new))
    }

    async fn requests(&mut self, window: i64) -> Result<Option<HashMap<String, RequestWindow>>> {
        if !self.requests.contains_key(&window) {
            let w = sources::request_windows(self.db, self.now - window).await?;
            self.requests.insert(window, w);
        }
        Ok(self.requests.get(&window).cloned().flatten())
    }

    async fn global_quiet(&mut self) -> QuietHours {
        if self.global_quiet.is_none() {
            self.global_quiet = Some(quiet::load_global(self.db).await);
        }
        self.global_quiet.clone().unwrap_or_default()
    }
}

fn app_target(id: &str) -> String {
    format!("app:{id}")
}

fn holds(info: &AlertKindInfo, value: f64, threshold: f64) -> bool {
    info.comparison.holds(value, threshold)
}

/// Count metric events of `kinds` per app since `since`.
async fn event_counts(db: &SqlitePool, kinds: &[&str], since: i64) -> Result<HashMap<String, i64>> {
    let placeholders = vec!["?"; kinds.len()].join(",");
    let sql = format!(
        "SELECT t.owner_id, COUNT(*) FROM metric_events e
         JOIN metric_targets t ON t.id = e.target
         WHERE t.kind = 'app' AND e.ts >= ? AND e.kind IN ({placeholders})
         GROUP BY t.owner_id"
    );
    let mut q = sqlx::query_as::<_, (String, i64)>(sqlx::AssertSqlSafe(sql)).bind(since);
    for k in kinds {
        q = q.bind(*k);
    }
    Ok(q.fetch_all(db).await?.into_iter().collect())
}

/// Parse a stored timestamp (RFC 3339 or SQLite's "YYYY-MM-DD HH:MM:SS").
fn parse_ts(s: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|f| chrono::NaiveDateTime::parse_from_str(s, f).ok())
        .map(|n| n.and_utc().timestamp())
}

/// Observe every target of `rule`.
async fn observe(
    ctx: &mut Ctx<'_>,
    rule: &AlertRule,
    info: &AlertKindInfo,
) -> Result<Vec<Observation>> {
    let now = ctx.now;
    let thr = rule.threshold;
    let mut out = Vec::new();
    match info.scope {
        AlertScope::App => {
            let apps = ctx.scope_apps(rule).await?;
            let obs = |app: &AppRow, value: Option<f64>, h: Option<bool>| Observation {
                target: app_target(&app.id),
                label: app.name.clone(),
                app_id: Some(app.id.clone()),
                value,
                holds: h,
            };
            match rule.kind.as_str() {
                "cpu" => {
                    for app in &apps {
                        let v = ctx.source.app_sample(&app.id, now).map(|(cpu, _)| cpu);
                        // No current sample: the app isn't running, so not busy.
                        out.push(obs(app, v, Some(v.is_some_and(|v| holds(info, v, thr)))));
                    }
                }
                "memory" => {
                    let limits = ctx.mem_limits().await?.clone();
                    for app in &apps {
                        let limit = limits.get(&app.id).copied().unwrap_or(0);
                        if limit <= 0 {
                            continue; // no limit: nothing to compare with
                        }
                        let v = ctx
                            .source
                            .app_sample(&app.id, now)
                            .map(|(_, mem)| mem as f64 / limit as f64 * 100.0);
                        out.push(obs(app, v, Some(v.is_some_and(|v| holds(info, v, thr)))));
                    }
                }
                "app_down" => {
                    for app in &apps {
                        // Not running (crashed, failed to start), or running
                        // but unreachable from outside (uptime check down).
                        let down = matches!(app.status.as_str(), "failed" | "unhealthy")
                            || (app.status != "stopped" && ctx.source.uptime_down(&app.id));
                        out.push(obs(app, Some(f64::from(u8::from(down))), Some(down)));
                    }
                }
                "http_5xx_rate" | "http_p95_latency" => {
                    let window = rule.window_secs.max(60);
                    let windows = ctx.requests(window).await?;
                    for app in &apps {
                        let Some(windows) = &windows else {
                            out.push(obs(app, None, None));
                            continue;
                        };
                        let w = windows
                            .get(&app.id)
                            .filter(|w| w.requests >= sources::MIN_REQUESTS);
                        let v = w.and_then(|w| {
                            if rule.kind == "http_5xx_rate" {
                                Some(w.errors_5xx as f64 / w.requests as f64 * 100.0)
                            } else {
                                w.p95_ms
                            }
                        });
                        out.push(obs(app, v, Some(v.is_some_and(|v| holds(info, v, thr)))));
                    }
                }
                "oom" | "crash_loop" => {
                    let kinds: &[&str] = if rule.kind == "oom" {
                        &["oom"]
                    } else {
                        &["restart", "crash"]
                    };
                    let window = rule.window_secs.max(60);
                    let counts = event_counts(ctx.db, kinds, now - window).await?;
                    for app in &apps {
                        let n = counts.get(&app.id).copied().unwrap_or(0) as f64;
                        let h = if info.comparison == AlertComparison::None {
                            n > 0.0
                        } else {
                            holds(info, n, thr)
                        };
                        out.push(obs(app, Some(n), Some(h)));
                    }
                }
                "deploy_failed" => {
                    let latest: Vec<(String, String)> = sqlx::query_as(
                        "SELECT d.app_id, d.status FROM deployments d
                         WHERE d.status IN ('succeeded', 'failed')
                           AND d.created_at = (SELECT MAX(created_at) FROM deployments x
                                               WHERE x.app_id = d.app_id
                                                 AND x.status IN ('succeeded', 'failed'))",
                    )
                    .fetch_all(ctx.db)
                    .await?;
                    let latest: HashMap<String, String> = latest.into_iter().collect();
                    for app in &apps {
                        let failed = latest.get(&app.id).is_some_and(|s| s == "failed");
                        out.push(obs(app, Some(f64::from(u8::from(failed))), Some(failed)));
                    }
                }
                _ => {}
            }
        }
        AlertScope::Host => {
            let (value, h) = match rule.kind.as_str() {
                "disk_usage" => {
                    let v = ctx.source.disk_percent(now);
                    (v, v.map(|v| holds(info, v, thr)))
                }
                "disk_forecast" => match ctx.source.disk_forecast(ctx.db, now).await {
                    Forecast::Unknown => (None, None),
                    Forecast::NotFilling => (None, Some(false)),
                    Forecast::HoursLeft(h) => (Some(h), Some(holds(info, h, thr))),
                },
                _ => (None, None),
            };
            out.push(Observation {
                target: "host".to_string(),
                label: "Server".to_string(),
                app_id: None,
                value,
                holds: h,
            });
        }
        AlertScope::Certificate => {
            let certs: Vec<(String, String)> = sqlx::query_as(
                "SELECT domain, expires_at FROM certificates
                 WHERE cert_type IN ('acme', 'custom') AND status IN ('active', 'expired')",
            )
            .fetch_all(ctx.db)
            .await?;
            for (domain, expires_at) in certs {
                let Some(expires) = parse_ts(&expires_at) else {
                    continue;
                };
                let days = (expires - now) as f64 / 86_400.0;
                out.push(Observation {
                    target: format!("cert:{domain}"),
                    label: domain,
                    app_id: None,
                    value: Some(days),
                    holds: Some(holds(info, days, thr)),
                });
            }
        }
        AlertScope::Backup => {
            // The latest attempt per scope: full snapshots, each database's
            // own backups (scheduled or not) and pre-deploy snapshots are
            // judged separately, so a good full snapshot can't hide a
            // database whose scheduled backups keep failing.
            let latest: Vec<(String, String, Option<String>)> = sqlx::query_as(
                "SELECT scope, status, remote_status FROM (
                     SELECT scope, status, remote_status,
                            ROW_NUMBER() OVER (PARTITION BY scope ORDER BY created_at DESC) AS rn
                     FROM snapshots WHERE status IN ('completed', 'failed'))
                 WHERE rn = 1 ORDER BY scope",
            )
            .fetch_all(ctx.db)
            .await?;
            if latest.is_empty() {
                out.push(backup_observation("full", false));
            }
            for (scope, status, remote) in latest {
                let failed = status == "failed" || remote.as_deref() == Some("failed");
                out.push(backup_observation(&scope, failed));
            }
        }
        AlertScope::Cron => {
            let jobs: Vec<(String, String, Option<String>)> =
                sqlx::query_as("SELECT id, name, last_run_status FROM cron_jobs")
                    .fetch_all(ctx.db)
                    .await?;
            for (id, name, last) in jobs {
                let failed = last.as_deref() == Some("failed");
                out.push(Observation {
                    target: format!("cron:{id}"),
                    label: name,
                    app_id: None,
                    value: Some(f64::from(u8::from(failed))),
                    holds: Some(failed),
                });
            }
        }
    }
    Ok(out)
}

/// The "backup failed" observation of one snapshot scope: full snapshots
/// keep the target "backup"; a database's backups are "backup:database:<name>".
pub fn backup_observation(scope: &str, failed: bool) -> Observation {
    let (target, label) = match scope {
        "full" => ("backup".to_string(), "Backups".to_string()),
        s => {
            let label = match s.split_once(':') {
                Some(("database", name)) => format!("Backups of {name}"),
                Some(("app", name)) => format!("Pre-deploy snapshots of {name}"),
                _ => format!("Backups ({s})"),
            };
            (format!("backup:{s}"), label)
        }
    };
    Observation {
        target,
        label,
        app_id: None,
        value: Some(f64::from(u8::from(failed))),
        holds: Some(failed),
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StateRow {
    target: String,
    target_label: String,
    state: String,
    since: i64,
    value: Option<f64>,
    episode_id: Option<String>,
    snoozed_until: Option<i64>,
    fired_at: Option<i64>,
    acknowledged_at: Option<i64>,
    last_notified_at: Option<i64>,
}

impl StateRow {
    fn phase(&self) -> Phase {
        match self.state.as_str() {
            "pending" => Phase::Pending { since: self.since },
            "firing" => Phase::Firing {
                since: self.since,
                fired_at: self.fired_at.unwrap_or(self.since),
            },
            _ => Phase::Inactive,
        }
    }
}

/// Evaluate every enabled rule once.
pub async fn evaluate(db: &SqlitePool, now: i64, source: &Source) -> Result<Report> {
    let _guard = EVAL_LOCK.lock().await;
    let rules = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE enabled = 1")
        .fetch_all(db)
        .await?;
    let mut report = Report {
        rules: rules.len(),
        ..Report::default()
    };
    let mut ctx = Ctx {
        db,
        now,
        source,
        apps: None,
        mem_limits: None,
        requests: HashMap::new(),
        global_quiet: None,
    };
    let mut first_error = None;
    for rule in &rules {
        let Some(info) = alert_kind(&rule.kind) else {
            continue;
        };
        let result = async {
            let obs = observe(&mut ctx, rule, info).await?;
            apply(&mut ctx, rule, obs, &mut report).await
        }
        .await;
        if let Err(e) = result {
            tracing::warn!("alert rule '{}' ({}): {e:#}", rule.name, rule.id);
            first_error.get_or_insert(e);
        }
    }
    match first_error {
        Some(e) if report.rules == 1 => Err(e),
        _ => Ok(report),
    }
}

/// Advance the states of one rule from its observations.
async fn apply(
    ctx: &mut Ctx<'_>,
    rule: &AlertRule,
    observations: Vec<Observation>,
    report: &mut Report,
) -> Result<()> {
    let now = ctx.now;
    let states: Vec<StateRow> = sqlx::query_as(
        "SELECT s.target, s.target_label, s.state, s.since, s.value, s.episode_id,
                s.snoozed_until, h.fired_at, h.acknowledged_at, h.last_notified_at
         FROM alert_states s LEFT JOIN alert_history h ON h.id = s.episode_id
         WHERE s.rule_id = ?",
    )
    .bind(&rule.id)
    .fetch_all(ctx.db)
    .await?;
    let mut by_target: HashMap<String, StateRow> =
        states.into_iter().map(|s| (s.target.clone(), s)).collect();

    // Targets that vanished (app, certificate or cron job deleted) resolve.
    let seen: HashSet<String> = observations.iter().map(|o| o.target.clone()).collect();
    let mut all = observations;
    for s in by_target.values() {
        if !seen.contains(&s.target) && s.state != "inactive" {
            all.push(Observation {
                target: s.target.clone(),
                label: s.target_label.clone(),
                app_id: s.target.strip_prefix("app:").map(str::to_string),
                value: None,
                holds: Some(false),
            });
        }
    }

    let mut live = Vec::new();
    let mut tx: Option<Transaction<'static, Sqlite>> = None;
    for o in all {
        let st = by_target.remove(&o.target);
        let phase = st.as_ref().map(StateRow::phase).unwrap_or(Phase::Inactive);
        let (next, tr) = machine::step(phase, o.holds, now, rule.for_secs);
        let target_snooze = st.as_ref().and_then(|s| s.snoozed_until);
        if !matches!(next, Phase::Inactive)
            && let Some(v) = o.value.or(st.as_ref().and_then(|s| s.value))
        {
            live.push((o.target.clone(), v));
        }
        match tr {
            Transition::Stay => {
                let Phase::Firing { .. } = next else {
                    continue;
                };
                let Some(s) = &st else { continue };
                let Some(episode) = &s.episode_id else {
                    continue;
                };
                let muted = machine::muted(now, &[rule.snoozed_until, s.snoozed_until]);
                if let Some(event) = machine::due_notification(
                    s.last_notified_at,
                    now,
                    rule.repeat_secs,
                    s.acknowledged_at.is_some(),
                    muted,
                ) {
                    let tx = begin(ctx.db, &mut tx).await?;
                    report.queued += enqueue(tx, ctx, rule, episode, event).await?;
                    sqlx::query(
                        "UPDATE alert_history SET last_notified_at = ?, last_value = COALESCE(?, last_value)
                         WHERE id = ?",
                    )
                    .bind(now)
                    .bind(o.value)
                    .bind(episode)
                    .execute(&mut **tx)
                    .await?;
                }
            }
            Transition::BecamePending => {
                let tx = begin(ctx.db, &mut tx).await?;
                sqlx::query(
                    "INSERT INTO alert_states (rule_id, target, target_label, state, since, value)
                     VALUES (?, ?, ?, 'pending', ?, ?)
                     ON CONFLICT(rule_id, target) DO UPDATE SET
                       target_label = excluded.target_label, state = 'pending',
                       since = excluded.since, value = excluded.value, episode_id = NULL",
                )
                .bind(&rule.id)
                .bind(&o.target)
                .bind(&o.label)
                .bind(now)
                .bind(o.value)
                .execute(&mut **tx)
                .await?;
            }
            Transition::Cleared => {
                let tx = begin(ctx.db, &mut tx).await?;
                settle_inactive(tx, &rule.id, &o.target, target_snooze, now).await?;
            }
            Transition::Fired => {
                let Phase::Firing { since, .. } = next else {
                    continue;
                };
                report.fired += 1;
                let episode = Uuid::new_v4().to_string();
                let tx = begin(ctx.db, &mut tx).await?;
                let muted = machine::muted(now, &[rule.snoozed_until, target_snooze]);
                let notify = machine::due_notification(None, now, rule.repeat_secs, false, muted);
                sqlx::query(
                    "INSERT INTO alert_history (id, rule_id, rule_name, kind, severity, target,
                        target_label, app_id, threshold, value, last_value, pending_since, fired_at,
                        last_notified_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&episode)
                .bind(&rule.id)
                .bind(&rule.name)
                .bind(&rule.kind)
                .bind(&rule.severity)
                .bind(&o.target)
                .bind(&o.label)
                .bind(&o.app_id)
                .bind(rule.threshold)
                .bind(o.value)
                .bind(o.value)
                .bind(since)
                .bind(now)
                .bind(notify.map(|_| now))
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "INSERT INTO alert_states (rule_id, target, target_label, state, since, value, episode_id)
                     VALUES (?, ?, ?, 'firing', ?, ?, ?)
                     ON CONFLICT(rule_id, target) DO UPDATE SET
                       target_label = excluded.target_label, state = 'firing',
                       since = excluded.since, value = excluded.value,
                       episode_id = excluded.episode_id",
                )
                .bind(&rule.id)
                .bind(&o.target)
                .bind(&o.label)
                .bind(since)
                .bind(o.value)
                .bind(&episode)
                .execute(&mut **tx)
                .await?;
                if let Some(event) = notify {
                    report.queued += enqueue(tx, ctx, rule, &episode, event).await?;
                }
            }
            Transition::Resolved => {
                report.resolved += 1;
                let tx = begin(ctx.db, &mut tx).await?;
                if let Some(episode) = st.as_ref().and_then(|s| s.episode_id.as_deref()) {
                    sqlx::query(
                        "UPDATE alert_history SET resolved_at = ?, last_value = COALESCE(?, last_value)
                         WHERE id = ?",
                    )
                    .bind(now)
                    .bind(o.value)
                    .bind(episode)
                    .execute(&mut **tx)
                    .await?;
                    report.queued += enqueue(tx, ctx, rule, episode, NoticeEvent::Resolved).await?;
                }
                settle_inactive(tx, &rule.id, &o.target, target_snooze, now).await?;
            }
        }
    }
    if let Some(tx) = tx {
        tx.commit().await?;
    }

    let mut map = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|(r, _), _| r != &rule.id);
    for (target, v) in live {
        map.insert((rule.id.clone(), target), v);
    }
    Ok(())
}

async fn begin<'t>(
    db: &SqlitePool,
    tx: &'t mut Option<Transaction<'static, Sqlite>>,
) -> Result<&'t mut Transaction<'static, Sqlite>> {
    if tx.is_none() {
        *tx = Some(crate::db::begin_write(db).await?);
    }
    Ok(tx.as_mut().unwrap_or_else(|| unreachable!("just set")))
}

/// Back to inactive: drop the row unless a timed snooze must outlive it.
async fn settle_inactive(
    tx: &mut Transaction<'_, Sqlite>,
    rule_id: &str,
    target: &str,
    snoozed_until: Option<i64>,
    now: i64,
) -> Result<()> {
    if snoozed_until.is_some_and(|t| t > now && t != ALERT_SNOOZE_UNTIL_RESOLVED) {
        sqlx::query(
            "UPDATE alert_states SET state = 'inactive', episode_id = NULL, value = NULL
             WHERE rule_id = ? AND target = ?",
        )
        .bind(rule_id)
        .bind(target)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query("DELETE FROM alert_states WHERE rule_id = ? AND target = ?")
            .bind(rule_id)
            .bind(target)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Queue `event` for every channel of the rule. A resolve goes only to
/// channels that were told about the alert. Returns how many will be sent.
async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &mut Ctx<'_>,
    rule: &AlertRule,
    episode: &str,
    event: NoticeEvent,
) -> Result<usize> {
    let now = ctx.now;
    let channels: Vec<(String, i64, Option<String>)> = if event == NoticeEvent::Resolved {
        sqlx::query_as(
            "SELECT DISTINCT i.id, i.enabled, i.quiet_hours FROM alert_deliveries d
             JOIN integrations i ON i.id = d.integration_id
             WHERE d.episode_id = ? AND d.event IN ('firing', 'repeat') AND d.status != 'skipped'",
        )
        .bind(episode)
        .fetch_all(&mut **tx)
        .await?
    } else {
        sqlx::query_as(
            "SELECT i.id, i.enabled, i.quiet_hours FROM alert_rule_notifications n
             JOIN integrations i ON i.id = n.integration_id WHERE n.rule_id = ?",
        )
        .bind(&rule.id)
        .fetch_all(&mut **tx)
        .await?
    };
    let global = ctx.global_quiet().await;
    let mut queued = 0;
    for (integration, enabled, quiet_json) in channels {
        let channel_quiet: ChannelQuietHours = quiet_json
            .as_deref()
            .and_then(|q| serde_json::from_str(q).ok())
            .unwrap_or_default();
        let hold = quiet::effective(&channel_quiet, &global)
            .and_then(|q| quiet::quiet_until(q, &rule.severity, now));
        let (status, error) = if enabled == 0 {
            ("skipped", Some("The channel is switched off"))
        } else {
            queued += 1;
            ("pending", None)
        };
        sqlx::query(
            "INSERT INTO alert_deliveries (id, episode_id, integration_id, event, status,
                next_attempt_at, deferred, last_error, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(episode)
        .bind(&integration)
        .bind(event.as_str())
        .bind(status)
        .bind(hold.unwrap_or(now))
        .bind(i64::from(hold.is_some()))
        .bind(error)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(queued)
}

/// Close every open episode of a rule (disabled or deleted), without
/// notifications, and forget its states.
pub async fn close_rule(db: &SqlitePool, rule_id: &str, now: i64) -> Result<()> {
    let _guard = EVAL_LOCK.lock().await;
    let mut tx = crate::db::begin_write(db).await?;
    sqlx::query(
        "UPDATE alert_history SET resolved_at = ?
         WHERE resolved_at IS NULL AND id IN (SELECT episode_id FROM alert_states WHERE rule_id = ?)",
    )
    .bind(now)
    .bind(rule_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM alert_states WHERE rule_id = ? AND state != 'inactive'")
        .bind(rule_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(r, _), _| r != rule_id);
    Ok(())
}

/// Close open episodes no state points at any more (their rule was deleted
/// with its app). Returns how many.
pub async fn close_orphans(db: &SqlitePool, now: i64) -> Result<u64> {
    let _guard = EVAL_LOCK.lock().await;
    Ok(sqlx::query(
        "UPDATE alert_history SET resolved_at = ?
         WHERE resolved_at IS NULL
           AND id NOT IN (SELECT episode_id FROM alert_states WHERE episode_id IS NOT NULL)",
    )
    .bind(now)
    .execute(db)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_timestamps_parse_in_both_formats() {
        assert_eq!(parse_ts("1970-01-01T00:01:00+00:00"), Some(60));
        assert_eq!(parse_ts("1970-01-01 00:01:00"), Some(60));
        assert_eq!(parse_ts("nope"), None);
    }
}
