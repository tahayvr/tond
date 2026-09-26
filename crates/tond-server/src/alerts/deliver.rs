// alerts/deliver.rs — sending queued notifications, with retries
//
// Deliveries are rows in `alert_deliveries`. The alert loop sends every due
// one (next_attempt_at <= now). A failure is recorded on the delivery and on
// the channel (`integrations.last_error`), then retried with backoff
// (machine::retry_delay) up to MAX_ATTEMPTS, after which it is marked failed.
// Nothing fails silently: the Alerts page shows failed deliveries and the
// channel list shows the channel's last error.
//
// Before sending, a delivery that no longer makes sense is skipped with a
// reason: a firing/repeat message for an alert that already resolved (for
// example while quiet hours held it back), a resolve for a channel that was
// never told the alert fired, or a channel that was switched off.

use anyhow::Result;
use sqlx::SqlitePool;
use tond_common::IntegrationConfig;

use super::machine;
use crate::notifications::{self, Notice, NoticeEvent};
use crate::{secrets, state::AppState};

/// Deliveries sent per pass (the rest go on the next pass, right after).
const BATCH: i64 = 25;

#[derive(Debug, Clone, sqlx::FromRow)]
struct DueRow {
    id: String,
    episode_id: String,
    integration_id: String,
    event: String,
    attempts: i64,
    i_kind: String,
    i_config: String,
    i_enabled: i64,
    rule_id: Option<String>,
    rule_name: String,
    kind: String,
    severity: String,
    target: String,
    target_label: String,
    app_id: Option<String>,
    threshold: f64,
    value: Option<f64>,
    last_value: Option<f64>,
    pending_since: i64,
    fired_at: i64,
    resolved_at: Option<i64>,
}

impl DueRow {
    fn notice(&self, now: i64, base_url: Option<&str>) -> Notice {
        let event = NoticeEvent::parse(&self.event);
        let value = match event {
            NoticeEvent::Firing => self.value,
            _ => self.last_value.or(self.value),
        };
        Notice {
            event,
            episode_id: self.episode_id.clone(),
            rule_id: self.rule_id.clone(),
            rule_name: self.rule_name.clone(),
            kind: self.kind.clone(),
            severity: self.severity.clone(),
            target: self.target.clone(),
            target_label: self.target_label.clone(),
            app_id: self.app_id.clone(),
            value,
            threshold: self.threshold,
            pending_since: self.pending_since,
            fired_at: self.fired_at,
            resolved_at: self.resolved_at,
            now,
            url: base_url.map(|b| alert_url(b, &self.episode_id)),
        }
    }
}

/// Dashboard link to one alert.
pub fn alert_url(base: &str, episode_id: &str) -> String {
    format!("{}/alerts?alert={episode_id}", base.trim_end_matches('/'))
}

/// `https://<dashboard domain>` when one is set.
pub async fn dashboard_base(state: &AppState) -> Option<String> {
    let domain = state.settings.read().await.dashboard_domain.clone();
    let domain = domain.trim();
    (!domain.is_empty()).then(|| format!("https://{domain}"))
}

/// Why a due delivery should not be sent, if it shouldn't.
fn skip_reason(row: &DueRow, told_before: bool) -> Option<&'static str> {
    if row.i_enabled == 0 {
        return Some("The channel is switched off");
    }
    match row.event.as_str() {
        "firing" | "repeat" if row.resolved_at.is_some() => {
            Some("The alert resolved before this could be sent")
        }
        "resolved" if !told_before => Some("This channel was never told the alert fired"),
        _ => None,
    }
}

/// Send every due delivery. Returns how many were attempted.
pub async fn process_due(state: &AppState, now: i64) -> Result<usize> {
    let db = &state.db;
    let rows: Vec<DueRow> = sqlx::query_as(
        "SELECT d.id, d.episode_id, d.integration_id, d.event, d.attempts,
                i.kind AS i_kind, i.config AS i_config, i.enabled AS i_enabled,
                h.rule_id, h.rule_name, h.kind, h.severity, h.target, h.target_label, h.app_id,
                h.threshold, h.value, h.last_value, h.pending_since, h.fired_at, h.resolved_at
         FROM alert_deliveries d
         JOIN integrations i ON i.id = d.integration_id
         JOIN alert_history h ON h.id = d.episode_id
         WHERE d.status = 'pending' AND d.next_attempt_at <= ?
         ORDER BY d.next_attempt_at, d.created_at
         LIMIT ?",
    )
    .bind(now)
    .bind(BATCH)
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let base = dashboard_base(state).await;
    let mut attempted = 0;
    for row in rows {
        let told_before: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM alert_deliveries
                           WHERE episode_id = ? AND integration_id = ?
                             AND event IN ('firing', 'repeat') AND status = 'sent')",
        )
        .bind(&row.episode_id)
        .bind(&row.integration_id)
        .fetch_one(db)
        .await?;
        if let Some(reason) = skip_reason(&row, told_before) {
            sqlx::query(
                "UPDATE alert_deliveries SET status = 'skipped', last_error = ? WHERE id = ?",
            )
            .bind(reason)
            .bind(&row.id)
            .execute(db)
            .await?;
            continue;
        }
        attempted += 1;
        let outcome = match secrets::open_integration_config(&row.i_kind, &row.i_config) {
            Ok(config) => send(&config, &row.notice(now, base.as_deref()), &row.id).await,
            Err(e) => Err(anyhow::anyhow!(
                "The channel settings can't be read ({e:#}); save the channel again"
            )),
        };
        record_outcome(db, &row.id, &row.integration_id, row.attempts, now, outcome).await?;
    }
    Ok(attempted)
}

async fn send(config: &IntegrationConfig, notice: &Notice, delivery_id: &str) -> Result<()> {
    notifications::dispatch(config, notice, delivery_id).await
}

/// Store the result of one attempt on the delivery and its channel.
async fn record_outcome(
    db: &SqlitePool,
    delivery_id: &str,
    integration_id: &str,
    previous_attempts: i64,
    now: i64,
    outcome: Result<()>,
) -> Result<()> {
    let attempts = previous_attempts + 1;
    match outcome {
        Ok(()) => {
            sqlx::query(
                "UPDATE alert_deliveries SET status = 'sent', attempts = ?, sent_at = ?,
                        last_error = NULL WHERE id = ?",
            )
            .bind(attempts)
            .bind(now)
            .bind(delivery_id)
            .execute(db)
            .await?;
            record_channel_health(db, integration_id, now, None).await?;
        }
        Err(e) => {
            let error = format!("{e:#}");
            tracing::warn!("alert delivery {delivery_id} failed (attempt {attempts}): {error}");
            if attempts >= machine::MAX_ATTEMPTS {
                sqlx::query(
                    "UPDATE alert_deliveries SET status = 'failed', attempts = ?, last_error = ?
                     WHERE id = ?",
                )
                .bind(attempts)
                .bind(&error)
                .bind(delivery_id)
                .execute(db)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE alert_deliveries SET attempts = ?, last_error = ?, next_attempt_at = ?
                     WHERE id = ?",
                )
                .bind(attempts)
                .bind(&error)
                .bind(now + machine::retry_delay(attempts))
                .bind(delivery_id)
                .execute(db)
                .await?;
            }
            record_channel_health(db, integration_id, now, Some(&error)).await?;
        }
    }
    Ok(())
}

/// Remember a channel's last success or error (shown on the Integrations page).
pub async fn record_channel_health(
    db: &SqlitePool,
    integration_id: &str,
    now: i64,
    error: Option<&str>,
) -> Result<()> {
    match error {
        None => {
            sqlx::query(
                "UPDATE integrations SET last_sent_at = ?, last_error = NULL, last_error_at = NULL
                 WHERE id = ?",
            )
            .bind(now)
            .bind(integration_id)
            .execute(db)
            .await?;
        }
        Some(e) => {
            sqlx::query("UPDATE integrations SET last_error = ?, last_error_at = ? WHERE id = ?")
                .bind(e)
                .bind(now)
                .bind(integration_id)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// When the next pending delivery is due (unix seconds).
pub async fn next_due(db: &SqlitePool) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT MIN(next_attempt_at) FROM alert_deliveries WHERE status = 'pending'",
    )
    .fetch_one(db)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(event: &str, resolved: bool, enabled: bool) -> DueRow {
        DueRow {
            id: "d".into(),
            episode_id: "e".into(),
            integration_id: "i".into(),
            event: event.into(),
            attempts: 0,
            i_kind: "webhook".into(),
            i_config: "{}".into(),
            i_enabled: i64::from(enabled),
            rule_id: None,
            rule_name: "r".into(),
            kind: "cpu".into(),
            severity: "warning".into(),
            target: "host".into(),
            target_label: "Server".into(),
            app_id: None,
            threshold: 80.0,
            value: Some(90.0),
            last_value: Some(95.0),
            pending_since: 0,
            fired_at: 0,
            resolved_at: resolved.then_some(10),
        }
    }

    #[test]
    fn stale_messages_are_skipped_with_a_reason() {
        assert_eq!(skip_reason(&row("firing", false, true), false), None);
        assert!(skip_reason(&row("firing", true, true), false).is_some());
        assert!(skip_reason(&row("repeat", true, true), true).is_some());
        assert_eq!(skip_reason(&row("resolved", true, true), true), None);
        assert!(skip_reason(&row("resolved", true, true), false).is_some());
        assert!(skip_reason(&row("firing", false, false), false).is_some());
    }

    #[test]
    fn repeat_and_resolve_notices_carry_the_latest_value() {
        let r = row("repeat", false, true);
        assert_eq!(r.notice(5, None).value, Some(95.0));
        let f = row("firing", false, true);
        let n = f.notice(5, Some("https://tond.example.com/"));
        assert_eq!(n.value, Some(90.0));
        assert_eq!(
            n.url.as_deref(),
            Some("https://tond.example.com/alerts?alert=e")
        );
    }
}
