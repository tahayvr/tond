// handlers/alerts.rs — alert rules, live alerts, history, snooze, settings
//
// Routes:
//   GET    /api/alerts                          rules (metrics:read)
//   POST   /api/alerts                          create a rule (settings:write)
//   GET    /api/alerts/kinds                    rule kinds and whether their data exists
//   GET    /api/alerts/active                   pending and firing alerts
//   GET    /api/alerts/history                  episodes, filtered and paginated
//   GET    /api/alerts/history/{id}             one episode with its deliveries
//   POST   /api/alerts/history/{id}/acknowledge stop repeats (apps:control)
//   GET    /api/alerts/settings                 global quiet hours
//   PUT    /api/alerts/settings                 (settings:write)
//   GET    /api/alerts/{id}                     one rule
//   PATCH  /api/alerts/{id}                     change a rule (settings:write)
//   DELETE /api/alerts/{id}                     delete a rule (settings:write)
//   POST   /api/alerts/{id}/test                send a test to every channel
//   POST   /api/alerts/{id}/snooze              mute a target or the rule (apps:control)
//   POST   /api/alerts/{id}/unsnooze            (apps:control)
//
// A rule must always notify someone: it needs at least one channel, and the
// channel list can't be emptied.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tond_common::*;
use uuid::Uuid;

use crate::alerts::{evaluate, quiet, send_test_notifications, sources};
use crate::audit_log::{AuditEntry, record};
use crate::{
    auth::{Authz, RateLimit, scope},
    error::AppError,
    state::AppState,
};

const NO_CHANNEL: &str = "Choose at least one notification channel, so this rule notifies someone.";
const MAX_WINDOW_SECS: i64 = 24 * 3600;
const MAX_FOR_SECS: i64 = 24 * 3600;
const MIN_REPEAT_SECS: i64 = 300;
const MAX_REPEAT_SECS: i64 = 7 * 86_400;
const MAX_SNOOZE_MINUTES: i64 = 30 * 24 * 60;

fn invalid(msg: impl Into<String>) -> AppError {
    AppError(TondError::InvalidInput(msg.into()))
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn opt_rfc3339(ts: Option<i64>) -> Option<String> {
    ts.map(rfc3339)
}

async fn fetch_rule(db: &sqlx::SqlitePool, id: &str) -> Result<AlertRule, AppError> {
    sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Alert rule {id} not found"))))
}

/// Build responses for `rules` with a handful of grouped queries.
async fn rule_responses(
    db: &sqlx::SqlitePool,
    rules: Vec<AlertRule>,
) -> Result<Vec<AlertRuleResponse>, AppError> {
    let links: Vec<(String, String)> =
        sqlx::query_as("SELECT rule_id, integration_id FROM alert_rule_notifications")
            .fetch_all(db)
            .await?;
    let mut by_rule: HashMap<String, Vec<String>> = HashMap::new();
    for (rule, integration) in links {
        by_rule.entry(rule).or_default().push(integration);
    }
    let counts: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT rule_id, state, COUNT(*) FROM alert_states
         WHERE state IN ('pending', 'firing') GROUP BY rule_id, state",
    )
    .fetch_all(db)
    .await?;
    let last: Vec<(String, i64)> = sqlx::query_as(
        "SELECT rule_id, MAX(fired_at) FROM alert_history WHERE rule_id IS NOT NULL GROUP BY rule_id",
    )
    .fetch_all(db)
    .await?;
    let last: HashMap<String, i64> = last.into_iter().collect();
    let apps: Vec<(String, String)> = sqlx::query_as("SELECT id, name FROM apps")
        .fetch_all(db)
        .await?;
    let apps: HashMap<String, String> = apps.into_iter().collect();
    let now = now_unix();

    Ok(rules
        .into_iter()
        .map(|r| {
            let count = |state: &str| {
                counts
                    .iter()
                    .find(|(id, s, _)| id == &r.id && s == state)
                    .map(|c| c.2)
                    .unwrap_or(0)
            };
            let mut integration_ids = by_rule.remove(&r.id).unwrap_or_default();
            integration_ids.sort();
            AlertRuleResponse {
                firing: count("firing"),
                pending: count("pending"),
                last_fired_at: last.get(&r.id).copied().map(rfc3339),
                app_name: r.app_id.as_ref().and_then(|a| apps.get(a).cloned()),
                snoozed_until: r.snoozed_until.filter(|t| *t > now).map(rfc3339),
                integration_ids,
                id: r.id,
                name: r.name,
                kind: r.kind,
                app_id: r.app_id,
                threshold: r.threshold,
                window_secs: r.window_secs,
                for_secs: r.for_secs,
                severity: r.severity,
                repeat_secs: r.repeat_secs,
                enabled: r.enabled != 0,
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
        .collect())
}

async fn one_response(
    db: &sqlx::SqlitePool,
    rule: AlertRule,
) -> Result<AlertRuleResponse, AppError> {
    rule_responses(db, vec![rule])
        .await?
        .pop()
        .ok_or_else(|| AppError(TondError::Internal("rule response missing".to_string())))
}

// ── Rules ────────────────────────────────────────────────────────────────────

pub async fn list_alerts(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let rules =
        sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules ORDER BY name COLLATE NOCASE")
            .fetch_all(&state.db)
            .await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(rule_responses(&state.db, rules).await?)),
    ))
}

pub async fn get_alert(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rule = fetch_rule(&state.db, &id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(one_response(&state.db, rule).await?)),
    ))
}

/// Kinds whose data this server can't provide yet, with the reason.
async fn unavailable_reason(db: &sqlx::SqlitePool, kind: &str) -> Option<String> {
    match kind {
        "http_5xx_rate" | "http_p95_latency" if !sources::request_metrics_available(db).await => Some(
            "Request metrics aren't collected on this server yet, so this rule couldn't be evaluated."
                .to_string(),
        ),
        _ => None,
    }
}

pub async fn list_kinds(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let mut kinds = Vec::with_capacity(ALERT_KINDS.len());
    for info in ALERT_KINDS {
        let reason = unavailable_reason(&state.db, info.kind).await;
        kinds.push(AlertKindResponse {
            info: *info,
            available: reason.is_none(),
            unavailable_reason: reason,
        });
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(kinds))))
}

/// Checked rule settings, ready to store.
struct RuleFields {
    threshold: f64,
    window_secs: i64,
    for_secs: i64,
    severity: String,
    repeat_secs: i64,
}

fn check_fields(
    info: &AlertKindInfo,
    threshold: Option<f64>,
    window_secs: Option<i64>,
    for_secs: Option<i64>,
    severity: Option<String>,
    repeat_secs: Option<i64>,
) -> Result<RuleFields, AppError> {
    let threshold = match info.default_threshold {
        None => 0.0,
        Some(default) => {
            let t = threshold.unwrap_or(default);
            let ok = t.is_finite()
                && match info.unit {
                    "%" if info.kind == "cpu" => t > 0.0 && t <= 12_800.0,
                    "%" => t > 0.0 && t <= 100.0,
                    "restarts" => (1.0..=1000.0).contains(&t),
                    _ => t > 0.0 && t <= 1_000_000.0,
                };
            if !ok {
                return Err(invalid(match info.unit {
                    "%" if info.kind == "cpu" => {
                        "CPU threshold must be above 0% (100% is one full core).".to_string()
                    }
                    "%" => "Threshold must be between 0% and 100%.".to_string(),
                    "restarts" => "Restarts must be between 1 and 1000.".to_string(),
                    unit => format!("Threshold must be a positive number of {unit}."),
                }));
            }
            t
        }
    };
    let window_secs = match info.default_window_secs {
        None => 0,
        Some(default) => {
            let w = window_secs.unwrap_or(default);
            if !(60..=MAX_WINDOW_SECS).contains(&w) {
                return Err(invalid("The window must be between 1 minute and 24 hours."));
            }
            w
        }
    };
    let for_secs = for_secs.unwrap_or(info.default_for_secs);
    if !(0..=MAX_FOR_SECS).contains(&for_secs) {
        return Err(invalid("\"For\" must be between 0 and 24 hours."));
    }
    let severity = severity.unwrap_or_else(|| info.default_severity.to_string());
    if !ALERT_SEVERITIES.contains(&severity.as_str()) {
        return Err(invalid("Severity must be info, warning or critical."));
    }
    let repeat_secs = repeat_secs.unwrap_or(ALERT_DEFAULT_REPEAT_SECS);
    if repeat_secs != 0 && !(MIN_REPEAT_SECS..=MAX_REPEAT_SECS).contains(&repeat_secs) {
        return Err(invalid(
            "Repeat must be off, or between 5 minutes and 7 days.",
        ));
    }
    Ok(RuleFields {
        threshold,
        window_secs,
        for_secs,
        severity,
        repeat_secs,
    })
}

async fn check_channels(db: &sqlx::SqlitePool, ids: &[String]) -> Result<Vec<String>, AppError> {
    let mut ids: Vec<String> = ids.iter().map(|s| s.trim().to_string()).collect();
    ids.sort();
    ids.dedup();
    ids.retain(|s| !s.is_empty());
    if ids.is_empty() {
        return Err(invalid(NO_CHANNEL));
    }
    for id in &ids {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM integrations WHERE id = ?)")
                .bind(id)
                .fetch_one(db)
                .await?;
        if !exists {
            return Err(invalid(format!("Notification channel {id} doesn't exist.")));
        }
    }
    Ok(ids)
}

fn check_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("Give the rule a name."));
    }
    if name.chars().count() > 100 {
        return Err(invalid("Keep the name under 100 characters."));
    }
    Ok(name.to_string())
}

pub async fn create_alert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateAlertRuleRequest>,
) -> Result<impl IntoResponse, AppError> {
    let name = check_name(&req.name)?;
    let info = alert_kind(&req.kind).ok_or_else(|| {
        invalid(format!(
            "Unknown rule kind '{}'. Choose one of: {}.",
            req.kind,
            ALERT_KINDS
                .iter()
                .map(|k| k.kind)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    if let Some(reason) = unavailable_reason(&state.db, info.kind).await {
        return Err(invalid(reason));
    }
    let app_id = match info.scope {
        AlertScope::App => req.app_id.filter(|a| !a.trim().is_empty()),
        _ => None,
    };
    if let Some(app) = &app_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM apps WHERE id = ?)")
            .bind(app)
            .fetch_one(&state.db)
            .await?;
        if !exists {
            return Err(invalid(format!("App {app} doesn't exist.")));
        }
    }
    let f = check_fields(
        info,
        req.threshold,
        req.window_secs,
        req.for_secs,
        req.severity,
        req.repeat_secs,
    )?;
    let channels = check_channels(&state.db, &req.integration_ids).await?;

    let id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO alert_rules (id, name, kind, app_id, threshold, window_secs, for_secs,
            severity, repeat_secs)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&name)
    .bind(info.kind)
    .bind(&app_id)
    .bind(f.threshold)
    .bind(f.window_secs)
    .bind(f.for_secs)
    .bind(&f.severity)
    .bind(f.repeat_secs)
    .execute(&mut *tx)
    .await?;
    for integration_id in &channels {
        sqlx::query("INSERT INTO alert_rule_notifications (rule_id, integration_id) VALUES (?, ?)")
            .bind(&id)
            .bind(integration_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let rule = fetch_rule(&state.db, &id).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_create")
            .resource("alert", &rule.id, &rule.name)
            .detail(format!("kind={}", rule.kind)),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(one_response(&state.db, rule).await?)),
    ))
}

pub async fn update_alert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAlertRuleRequest>,
) -> Result<impl IntoResponse, AppError> {
    let rule = fetch_rule(&state.db, &id).await?;
    let info = alert_kind(&rule.kind).ok_or_else(|| {
        invalid(format!(
            "Rule kind '{}' is not supported any more.",
            rule.kind
        ))
    })?;
    let name = match &req.name {
        Some(n) => check_name(n)?,
        None => rule.name.clone(),
    };
    let f = check_fields(
        info,
        req.threshold.or(Some(rule.threshold)),
        req.window_secs.or(Some(rule.window_secs)),
        req.for_secs.or(Some(rule.for_secs)),
        req.severity.clone().or(Some(rule.severity.clone())),
        req.repeat_secs.or(Some(rule.repeat_secs)),
    )?;
    let enabled = req.enabled.map(i64::from).unwrap_or(rule.enabled);
    let snoozed_until = match req.snoozed_until.as_deref().map(str::trim) {
        None => rule.snoozed_until,
        Some("") => None,
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| invalid("snoozed_until must be an RFC 3339 time."))?
                .timestamp(),
        ),
    };
    let channels = match &req.integration_ids {
        Some(ids) => Some(check_channels(&state.db, ids).await?),
        None => None,
    };

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE alert_rules
         SET name = ?, threshold = ?, window_secs = ?, for_secs = ?, severity = ?,
             repeat_secs = ?, enabled = ?, snoozed_until = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&name)
    .bind(f.threshold)
    .bind(f.window_secs)
    .bind(f.for_secs)
    .bind(&f.severity)
    .bind(f.repeat_secs)
    .bind(enabled)
    .bind(snoozed_until)
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    if let Some(channels) = &channels {
        sqlx::query("DELETE FROM alert_rule_notifications WHERE rule_id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        for integration_id in channels {
            sqlx::query(
                "INSERT INTO alert_rule_notifications (rule_id, integration_id) VALUES (?, ?)",
            )
            .bind(&id)
            .bind(integration_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    if rule.enabled != 0 && enabled == 0 {
        evaluate::close_rule(&state.db, &id, now_unix()).await?;
    }

    let updated = fetch_rule(&state.db, &id).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_update").resource("alert", &updated.id, &updated.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(one_response(&state.db, updated).await?)),
    ))
}

pub async fn delete_alert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rule = fetch_rule(&state.db, &id).await?;
    evaluate::close_rule(&state.db, &id, now_unix()).await?;
    sqlx::query("DELETE FROM alert_rules WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_delete").resource("alert", &id, &rule.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": id }))),
    ))
}

pub async fn test_alert(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rule = fetch_rule(&state.db, &id).await?;
    let results = send_test_notifications(&state, &rule).await;
    if results.is_empty() {
        return Err(invalid(NO_CHANNEL));
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(results))))
}

// ── Live alerts ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct ActiveRow {
    rule_id: String,
    rule_name: String,
    kind: String,
    severity: String,
    threshold: f64,
    rule_snoozed_until: Option<i64>,
    target: String,
    target_label: String,
    state: String,
    since: i64,
    value: Option<f64>,
    episode_id: Option<String>,
    snoozed_until: Option<i64>,
    fired_at: Option<i64>,
    acknowledged_at: Option<i64>,
    acknowledged_by: Option<String>,
    failed_deliveries: i64,
}

fn severity_order(s: &str) -> i32 {
    match s {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    }
}

/// Pending and firing alerts: firing first, then by severity and age.
pub async fn active_alerts(db: &sqlx::SqlitePool) -> Result<Vec<ActiveAlert>, AppError> {
    let rows: Vec<ActiveRow> = sqlx::query_as(
        "SELECT s.rule_id, r.name AS rule_name, r.kind, r.severity, r.threshold,
                r.snoozed_until AS rule_snoozed_until, s.target, s.target_label, s.state,
                s.since, s.value, s.episode_id, s.snoozed_until, h.fired_at,
                h.acknowledged_at, h.acknowledged_by,
                (SELECT COUNT(*) FROM alert_deliveries d
                 WHERE d.episode_id = s.episode_id AND d.status = 'failed') AS failed_deliveries
         FROM alert_states s
         JOIN alert_rules r ON r.id = s.rule_id
         LEFT JOIN alert_history h ON h.id = s.episode_id
         WHERE s.state IN ('pending', 'firing') AND r.enabled = 1",
    )
    .fetch_all(db)
    .await?;
    let now = now_unix();
    let mut out: Vec<ActiveAlert> = rows
        .into_iter()
        .map(|r| {
            let info = alert_kind(&r.kind);
            let until_resolved = r.snoozed_until == Some(ALERT_SNOOZE_UNTIL_RESOLVED);
            let snoozed_until = [r.rule_snoozed_until, r.snoozed_until]
                .into_iter()
                .flatten()
                .filter(|t| *t > now && *t != ALERT_SNOOZE_UNTIL_RESOLVED)
                .max();
            ActiveAlert {
                value: evaluate::live_value(&r.rule_id, &r.target).or(r.value),
                app_id: r.target.strip_prefix("app:").map(str::to_string),
                unit: info.map(|i| i.unit).unwrap_or("").to_string(),
                comparison: info.map(|i| i.comparison).unwrap_or(AlertComparison::None),
                since: rfc3339(r.since),
                fired_at: opt_rfc3339(r.fired_at),
                acknowledged_at: opt_rfc3339(r.acknowledged_at),
                acknowledged_by: r.acknowledged_by,
                snoozed_until: snoozed_until.map(rfc3339),
                snoozed_until_resolved: until_resolved,
                failed_deliveries: r.failed_deliveries,
                rule_id: r.rule_id,
                rule_name: r.rule_name,
                kind: r.kind,
                severity: r.severity,
                target: r.target,
                target_label: r.target_label,
                state: r.state,
                threshold: r.threshold,
                episode_id: r.episode_id,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.state != "firing", severity_order(&a.severity), &a.since).cmp(&(
            b.state != "firing",
            severity_order(&b.severity),
            &b.since,
        ))
    });
    Ok(out)
}

pub async fn list_active(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(active_alerts(&state.db).await?)),
    ))
}

// ── History ──────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct EpisodeRow {
    id: String,
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
    acknowledged_at: Option<i64>,
    acknowledged_by: Option<String>,
    notifications_sent: i64,
    failed_deliveries: i64,
}

impl From<EpisodeRow> for AlertEpisode {
    fn from(r: EpisodeRow) -> Self {
        let info = alert_kind(&r.kind);
        AlertEpisode {
            unit: info.map(|i| i.unit).unwrap_or("").to_string(),
            comparison: info.map(|i| i.comparison).unwrap_or(AlertComparison::None),
            state: if r.resolved_at.is_some() {
                "resolved"
            } else {
                "firing"
            }
            .to_string(),
            pending_since: rfc3339(r.pending_since),
            fired_at: rfc3339(r.fired_at),
            resolved_at: opt_rfc3339(r.resolved_at),
            acknowledged_at: opt_rfc3339(r.acknowledged_at),
            id: r.id,
            rule_id: r.rule_id,
            rule_name: r.rule_name,
            kind: r.kind,
            severity: r.severity,
            target: r.target,
            target_label: r.target_label,
            app_id: r.app_id,
            threshold: r.threshold,
            value: r.value,
            last_value: r.last_value,
            acknowledged_by: r.acknowledged_by,
            notifications_sent: r.notifications_sent,
            failed_deliveries: r.failed_deliveries,
        }
    }
}

const EPISODE_COLUMNS: &str = "h.id, h.rule_id, h.rule_name, h.kind, h.severity, h.target,
    h.target_label, h.app_id, h.threshold, h.value, h.last_value, h.pending_since, h.fired_at,
    h.resolved_at, h.acknowledged_at, h.acknowledged_by,
    (SELECT COUNT(*) FROM alert_deliveries d WHERE d.episode_id = h.id AND d.status = 'sent')
        AS notifications_sent,
    (SELECT COUNT(*) FROM alert_deliveries d WHERE d.episode_id = h.id AND d.status = 'failed')
        AS failed_deliveries";

pub async fn list_history(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Query(q): Query<AlertHistoryQuery>,
) -> Result<impl IntoResponse, AppError> {
    let per_page = q.per_page.unwrap_or(25).clamp(1, 100);
    let page = q.page.unwrap_or(1).max(1);
    let state_filter = match q.state.as_deref() {
        None | Some("") => "",
        Some("firing") => " AND h.resolved_at IS NULL",
        Some("resolved") => " AND h.resolved_at IS NOT NULL",
        Some(_) => return Err(invalid("state must be firing or resolved.")),
    };
    let rule = q.rule_id.filter(|s| !s.is_empty());
    let app = q.app_id.filter(|s| !s.is_empty());
    let filter = format!(
        "WHERE (?1 IS NULL OR h.rule_id = ?1) AND (?2 IS NULL OR h.app_id = ?2){state_filter}"
    );
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM alert_history h {filter}"
    )))
    .bind(&rule)
    .bind(&app)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<EpisodeRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {EPISODE_COLUMNS} FROM alert_history h {filter}
         ORDER BY h.fired_at DESC LIMIT ?3 OFFSET ?4"
    )))
    .bind(&rule)
    .bind(&app)
    .bind(per_page)
    .bind((page - 1) * per_page)
    .fetch_all(&state.db)
    .await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AlertHistoryPage {
            episodes: rows.into_iter().map(AlertEpisode::from).collect(),
            total,
        })),
    ))
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    id: String,
    episode_id: String,
    integration_id: String,
    integration_name: String,
    integration_kind: String,
    event: String,
    status: String,
    attempts: i64,
    deferred: i64,
    last_error: Option<String>,
    next_attempt_at: i64,
    sent_at: Option<i64>,
    created_at: i64,
}

pub async fn get_episode(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row: EpisodeRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {EPISODE_COLUMNS} FROM alert_history h WHERE h.id = ?"
    )))
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError(TondError::NotFound(format!("Alert {id} not found"))))?;
    let deliveries: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT d.id, d.episode_id, d.integration_id, i.name AS integration_name,
                i.kind AS integration_kind, d.event, d.status, d.attempts, d.deferred,
                d.last_error, d.next_attempt_at, d.sent_at, d.created_at
         FROM alert_deliveries d JOIN integrations i ON i.id = d.integration_id
         WHERE d.episode_id = ? ORDER BY d.created_at, i.name",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let deliveries = deliveries
        .into_iter()
        .map(|d| AlertDelivery {
            next_attempt_at: (d.status == "pending").then(|| rfc3339(d.next_attempt_at)),
            sent_at: opt_rfc3339(d.sent_at),
            created_at: rfc3339(d.created_at),
            deferred: d.deferred != 0,
            id: d.id,
            episode_id: d.episode_id,
            integration_id: d.integration_id,
            integration_name: d.integration_name,
            integration_kind: d.integration_kind,
            event: d.event,
            status: d.status,
            attempts: d.attempts,
            last_error: d.last_error,
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AlertEpisodeDetail {
            episode: row.into(),
            deliveries,
        })),
    ))
}

pub async fn acknowledge(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let actor = auth.audit_actor();
    let updated = sqlx::query(
        "UPDATE alert_history SET acknowledged_at = ?, acknowledged_by = ?
         WHERE id = ? AND resolved_at IS NULL",
    )
    .bind(now_unix())
    .bind(&actor.label)
    .bind(&id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError(TondError::NotFound(
            "That alert isn't firing any more.".to_string(),
        )));
    }
    let name: String = sqlx::query_scalar("SELECT rule_name FROM alert_history WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    record(
        &state.db,
        &actor,
        AuditEntry::new("alert_acknowledge").resource("alert", &id, &name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "acknowledged": id }))),
    ))
}

// ── Snooze ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SnoozeBody {
    /// Target to mute ("app:<id>", "host", …); None mutes the whole rule
    #[serde(default)]
    pub target: Option<String>,
    #[serde(flatten)]
    pub snooze: SnoozeAlertRequest,
}

#[derive(Debug, Deserialize)]
pub struct UnsnoozeBody {
    #[serde(default)]
    pub target: Option<String>,
}

pub async fn snooze(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SnoozeBody>,
) -> Result<impl IntoResponse, AppError> {
    let rule = fetch_rule(&state.db, &id).await?;
    let now = now_unix();
    let until = if body.snooze.until_resolved {
        if body.target.is_none() {
            return Err(invalid(
                "Snooze a single alert until it resolves, not a whole rule.",
            ));
        }
        ALERT_SNOOZE_UNTIL_RESOLVED
    } else {
        let minutes = body
            .snooze
            .minutes
            .ok_or_else(|| invalid("Say how long to snooze for, or until it resolves."))?;
        if !(1..=MAX_SNOOZE_MINUTES).contains(&minutes) {
            return Err(invalid("Snooze for between 1 minute and 30 days."));
        }
        now + minutes * 60
    };
    match &body.target {
        None => {
            sqlx::query("UPDATE alert_rules SET snoozed_until = ? WHERE id = ?")
                .bind(until)
                .bind(&id)
                .execute(&state.db)
                .await?;
        }
        Some(target) => {
            let updated = sqlx::query(
                "UPDATE alert_states SET snoozed_until = ? WHERE rule_id = ? AND target = ?",
            )
            .bind(until)
            .bind(&id)
            .bind(target)
            .execute(&state.db)
            .await?
            .rows_affected();
            if updated == 0 {
                if until == ALERT_SNOOZE_UNTIL_RESOLVED {
                    return Err(AppError(TondError::NotFound(
                        "That alert isn't pending or firing any more.".to_string(),
                    )));
                }
                sqlx::query(
                    "INSERT INTO alert_states (rule_id, target, target_label, state, since, snoozed_until)
                     VALUES (?, ?, ?, 'inactive', ?, ?)",
                )
                .bind(&id)
                .bind(target)
                .bind(target)
                .bind(now)
                .bind(until)
                .execute(&state.db)
                .await?;
            }
        }
    }
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_snooze")
            .resource("alert", &id, &rule.name)
            .detail(match (&body.target, until) {
                (Some(t), ALERT_SNOOZE_UNTIL_RESOLVED) => format!("{t} until resolved"),
                (Some(t), u) => format!("{t} until {}", rfc3339(u)),
                (None, u) => format!("rule until {}", rfc3339(u)),
            }),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({
            "snoozed_until": (until != ALERT_SNOOZE_UNTIL_RESOLVED).then(|| rfc3339(until)),
            "until_resolved": until == ALERT_SNOOZE_UNTIL_RESOLVED,
        }))),
    ))
}

pub async fn unsnooze(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UnsnoozeBody>,
) -> Result<impl IntoResponse, AppError> {
    let rule = fetch_rule(&state.db, &id).await?;
    match &body.target {
        None => {
            sqlx::query("UPDATE alert_rules SET snoozed_until = NULL WHERE id = ?")
                .bind(&id)
                .execute(&state.db)
                .await?;
        }
        Some(target) => {
            sqlx::query(
                "DELETE FROM alert_states WHERE rule_id = ? AND target = ? AND state = 'inactive'",
            )
            .bind(&id)
            .bind(target)
            .execute(&state.db)
            .await?;
            sqlx::query(
                "UPDATE alert_states SET snoozed_until = NULL WHERE rule_id = ? AND target = ?",
            )
            .bind(&id)
            .bind(target)
            .execute(&state.db)
            .await?;
        }
    }
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_unsnooze").resource("alert", &id, &rule.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "unsnoozed": true }))),
    ))
}

// ── Settings (global quiet hours) ────────────────────────────────────────────

pub async fn get_settings(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AlertSettings {
            quiet_hours: quiet::load_global(&state.db).await,
        })),
    ))
}

pub async fn put_settings(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<AlertSettings>,
) -> Result<impl IntoResponse, AppError> {
    quiet::validate(&req.quiet_hours).map_err(invalid)?;
    quiet::save_global(&state.db, &req.quiet_hours).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_settings_update").detail(if req.quiet_hours.enabled {
            format!(
                "quiet hours {}–{} {}",
                req.quiet_hours.start, req.quiet_hours.end, req.quiet_hours.timezone
            )
        } else {
            "quiet hours off".to_string()
        }),
    )
    .await;
    Ok((StatusCode::OK, Json(ApiResponse::ok(req))))
}

#[cfg(test)]
mod tests;
