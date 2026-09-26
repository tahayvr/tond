// alerts — the alert engine
//
//   machine.rs   pure state machine: inactive → pending → firing → resolved,
//                notification timing (first, repeat, mute), retry backoff
//   evaluate.rs  observes every enabled rule and advances its states;
//                queues deliveries
//   deliver.rs   sends due deliveries, records failures, retries
//   quiet.rs     quiet hours (global and per channel)
//   sources.rs   request metrics and disk forecast interfaces
//
// When it runs — no polling loop of its own:
//   - on the metrics collector's 30 s tick (`on_metrics_tick`, inline in the
//     collector task, so it costs no extra wakeup), while any rule exists;
//   - in the "alerts" loop when something event-style rules watch changes:
//     deployments, snapshots, certificates, cron jobs, container events
//     (`kick`), or the rules themselves. With no rules the loop only watches
//     alert_rules and sleeps;
//   - the same loop sends deliveries and sleeps until the next retry or the
//     end of quiet hours.

pub mod deliver;
pub mod evaluate;
pub mod machine;
pub mod quiet;
pub mod sources;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use tond_common::AlertRule;

use crate::notifications::{self, Notice};
use crate::runtime::changes::{self, Table, Watch};
use crate::runtime::loops::{self, Wake};
use crate::{metrics, secrets, state::AppState};
use evaluate::Source;

/// Enabled rules, as last counted by the alert loop (-1 = not yet known).
static ENABLED_RULES: AtomicI64 = AtomicI64::new(-1);

/// Tables that wake the alert loop while rules exist.
const EVENT_TABLES: &[Table] = &[
    Table::AlertRules,
    Table::AlertKick,
    Table::Deployments,
    Table::Snapshots,
    Table::Certificates,
    Table::CronJobs,
];
/// With no rules only these (a new rule, or deliveries left to send).
const IDLE_TABLES: &[Table] = &[Table::AlertRules, Table::AlertKick];

/// Wake the alert loop: deliveries were queued, or something happened that
/// event-style rules watch (e.g. an OOM kill or a restart).
pub fn kick() {
    if ENABLED_RULES.load(Ordering::Relaxed) != 0 {
        changes::notify(Table::AlertKick);
    }
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Called by the metrics collector at the end of every tick.
pub async fn on_metrics_tick(state: &AppState, now: i64) {
    if let Some(s) = metrics::host::snapshot() {
        sources::record_disk(now, s.disk_used, s.disk_total);
    }
    if ENABLED_RULES.load(Ordering::Relaxed) == 0 {
        return;
    }
    match evaluate::evaluate(&state.db, now, &Source::Live).await {
        Ok(report) if report.queued > 0 => kick(),
        Ok(_) => {}
        Err(e) => tracing::warn!("alert evaluation failed: {e:#}"),
    }
}

async fn count_enabled_rules(db: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM alert_rules WHERE enabled = 1")
        .fetch_one(db)
        .await
        .unwrap_or(1)
}

/// The "alerts" background loop (spawned once at startup).
pub async fn run_alert_engine(state: AppState) {
    let lp = loops::register("alerts");
    if let Err(e) = evaluate::close_orphans(&state.db, now_unix()).await {
        tracing::warn!("alerts: cannot close orphaned alerts: {e:#}");
    }
    let mut watching_events: Option<bool> = None;
    let mut watch = Watch::new(IDLE_TABLES);
    let mut wake = Wake::Event;
    loop {
        let run = lp.run();
        let rules = count_enabled_rules(&state.db).await;
        ENABLED_RULES.store(rules, Ordering::Relaxed);
        let now = now_unix();
        let mut failed = None;
        if rules > 0
            && wake == Wake::Event
            && let Err(e) = evaluate::evaluate(&state.db, now, &Source::Live).await
        {
            failed = Some(e);
        }
        for _ in 0..20 {
            match deliver::process_due(&state, now_unix()).await {
                Ok(n) if n > 0 => continue,
                Ok(_) => break,
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        let next = deliver::next_due(&state.db).await.ok().flatten();
        match failed {
            Some(e) => {
                tracing::warn!("alerts: {e:#}");
                run.fail(&format!("{e:#}"));
            }
            None => drop(run),
        }

        let want_events = rules > 0;
        if watching_events != Some(want_events) {
            watch = Watch::new(if want_events {
                EVENT_TABLES
            } else {
                IDLE_TABLES
            });
            watching_events = Some(want_events);
        }
        let timeout = next.map(|t| Duration::from_secs((t - now_unix()).clamp(1, 86_400) as u64));
        wake = lp.wait(timeout, &mut watch).await;
    }
}

// ── Test notifications ──────────────────────────────────────────────────────

/// Outcome of sending a test notification to one channel.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ChannelTestResult {
    pub integration_id: String,
    pub kind: String,
    pub name: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Send a test message to one stored channel, remembering the outcome as the
/// channel's health.
pub async fn test_channel(
    state: &AppState,
    id: &str,
    name: &str,
    kind: &str,
    config_json: &str,
    title: Option<&str>,
) -> anyhow::Result<()> {
    let base = deliver::dashboard_base(state).await;
    let mut notice = Notice::test(name, now_unix(), base.map(|b| format!("{b}/alerts")));
    if let Some(title) = title {
        notice.rule_name = title.to_string();
    }
    let outcome = match secrets::open_integration_config(kind, config_json) {
        Ok(config) => notifications::dispatch(&config, &notice, "test").await,
        Err(e) => Err(anyhow::anyhow!("The channel settings can't be read: {e:#}")),
    };
    let error = outcome.as_ref().err().map(|e| format!("{e:#}"));
    let _ = deliver::record_channel_health(&state.db, id, now_unix(), error.as_deref()).await;
    outcome
}

/// Send a clearly labelled test to every channel of `rule`.
pub async fn send_test_notifications(state: &AppState, rule: &AlertRule) -> Vec<ChannelTestResult> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT i.id, i.name, i.kind, i.config, i.enabled
         FROM alert_rule_notifications n
         JOIN integrations i ON i.id = n.integration_id
         WHERE n.rule_id = ?
         ORDER BY i.name",
    )
    .bind(&rule.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let title = format!("Test of alert rule '{}'", rule.name);
    let mut results = Vec::with_capacity(rows.len());
    for (id, name, kind, config, enabled) in rows {
        let outcome = if enabled == 0 {
            Err(anyhow::anyhow!(
                "This channel is switched off on the Integrations page"
            ))
        } else {
            test_channel(state, &id, &name, &kind, &config, Some(&title)).await
        };
        results.push(ChannelTestResult {
            integration_id: id,
            kind,
            name,
            ok: outcome.is_ok(),
            error: outcome.err().map(|e| format!("{e:#}")),
        });
    }
    results
}
