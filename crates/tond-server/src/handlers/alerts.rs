// handlers/alerts.rs — Alert rule CRUD + alert↔integration linking
//
// Routes:
//   GET    /api/alerts              — list all alert rules
//   GET    /api/alerts/{id}         — get a single alert rule
//   POST   /api/alerts              — create a new alert rule
//   PATCH  /api/alerts/{id}         — update an existing rule (partial)
//   DELETE /api/alerts/{id}         — delete a rule
//   GET    /api/alerts/{id}/integrations        — list linked integrations for a rule
//   POST   /api/alerts/{id}/integrations        — link an integration to a rule
//   DELETE /api/alerts/{id}/integrations/{int}  — unlink an integration from a rule

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;
use uuid::Uuid;

use crate::audit_log::{AuditEntry, record};
use crate::{
    auth::{Authz, RateLimit, scope},
    error::AppError,
    secrets,
    state::AppState,
};

fn rule_response(rule: AlertRule) -> Result<AlertRuleResponse, AppError> {
    let url = secrets::alert_webhook_url(&rule.webhook_url)?;
    Ok(AlertRuleResponse::new(rule, &url))
}

// ── List ──

pub async fn list_alerts(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let rules =
        sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules ORDER BY created_at DESC")
            .fetch_all(&state.db)
            .await?;

    let mut resp: Vec<AlertRuleResponse> = Vec::with_capacity(rules.len());
    for rule in rules {
        resp.push(rule_response(rule)?);
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(resp))))
}

// ── Get single ──

pub async fn get_alert(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rule = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Alert rule {id} not found"))))?;

    Ok((StatusCode::OK, Json(ApiResponse::ok(rule_response(rule)?))))
}

// ── Helpers ──

fn validate_metric(metric: &str) -> Result<(), AppError> {
    if !VALID_ALERT_METRICS.contains(&metric) {
        return Err(AppError(TondError::InvalidInput(format!(
            "metric must be one of: {}",
            VALID_ALERT_METRICS.join(", ")
        ))));
    }
    Ok(())
}

fn validate_events(events: &[String]) -> Result<(), AppError> {
    for event in events {
        if !VALID_ALERT_EVENTS.contains(&event.as_str()) {
            return Err(AppError(TondError::InvalidInput(format!(
                "unknown event '{}'; valid events: {}",
                event,
                VALID_ALERT_EVENTS.join(", ")
            ))));
        }
    }
    Ok(())
}

fn events_to_json(events: &Option<Vec<String>>) -> Option<String> {
    events
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
}

// ── Create ──

pub async fn create_alert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateAlertRuleRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate metric if provided
    if let Some(ref m) = req.metric {
        validate_metric(m)?;
    }
    // Validate events if provided
    if let Some(ref evts) = req.events {
        validate_events(evts)?;
    }
    // Must have at least one of metric or events
    if req.metric.is_none() && req.events.as_ref().map(|e| e.is_empty()).unwrap_or(true) {
        return Err(AppError(TondError::InvalidInput(
            "alert rule must have at least one metric or one event subscription".to_string(),
        )));
    }

    let id = Uuid::new_v4().to_string();
    let threshold = req.threshold.unwrap_or(0.0);
    let events_json = events_to_json(&req.events);

    sqlx::query(
        "INSERT INTO alert_rules (id, app_id, name, metric, threshold, webhook_url, events)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.app_id)
    .bind(&req.name)
    .bind(&req.metric)
    .bind(threshold)
    .bind(secrets::seal_alert_webhook_url(req.webhook_url.trim())?)
    .bind(&events_json)
    .execute(&state.db)
    .await?;

    let rule = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_create").resource("alert", &rule.id, &rule.name),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(rule_response(rule)?)),
    ))
}

// ── Update (PATCH — partial) ──

pub async fn update_alert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAlertRuleRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Ensure rule exists
    let rule = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Alert rule {id} not found"))))?;

    let name = req.name.unwrap_or(rule.name);
    let metric = match req.metric {
        Some(m) => {
            validate_metric(&m)?;
            Some(m)
        }
        None => rule.metric,
    };
    let threshold = req.threshold.unwrap_or(rule.threshold);
    // Write-only: absent keeps the saved URL, "" clears it.
    let webhook_url = match req.webhook_url {
        Some(url) => secrets::seal_alert_webhook_url(url.trim())?,
        None => rule.webhook_url,
    };
    let enabled: i64 = req
        .enabled
        .map(|b| if b { 1 } else { 0 })
        .unwrap_or(rule.enabled);
    let events_json = match req.events {
        Some(ref evts) => {
            validate_events(evts)?;
            Some(serde_json::to_string(evts).unwrap_or_else(|_| "[]".to_string()))
        }
        None => rule.events,
    };

    sqlx::query(
        "UPDATE alert_rules
         SET name = ?, metric = ?, threshold = ?, webhook_url = ?, enabled = ?, events = ?,
             updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&name)
    .bind(&metric)
    .bind(threshold)
    .bind(&webhook_url)
    .bind(enabled)
    .bind(&events_json)
    .bind(&id)
    .execute(&state.db)
    .await?;

    let updated = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_update").resource("alert", &updated.id, &updated.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(rule_response(updated)?)),
    ))
}

// ── Delete ──

pub async fn delete_alert(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Cascade-delete alert_rule_notifications rows first
    sqlx::query("DELETE FROM alert_rule_notifications WHERE rule_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;

    let deleted = sqlx::query("DELETE FROM alert_rules WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?
        .rows_affected();

    if deleted == 0 {
        return Err(AppError(TondError::NotFound(format!(
            "Alert rule {id} not found"
        ))));
    }

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("alert_delete").resource("alert", &id, &id),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": id }))),
    ))
}

// ── List integrations linked to an alert rule ──

pub async fn list_alert_integrations(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Ensure the rule exists
    let _rule = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Alert rule {id} not found"))))?;

    let links = sqlx::query_as::<_, AlertIntegrationLink>(
        "SELECT arn.id, arn.rule_id, arn.integration_id,
                i.name AS integration_name, i.kind AS integration_kind,
                arn.created_at
         FROM alert_rule_notifications arn
         JOIN integrations i ON i.id = arn.integration_id
         WHERE arn.rule_id = ?
         ORDER BY arn.created_at DESC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(ApiResponse::ok(links))))
}

// ── Link an integration to an alert rule ──

pub async fn link_alert_integration(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LinkAlertIntegrationRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Ensure rule exists
    let exists_rule: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alert_rules WHERE id = ?)")
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
    if !exists_rule {
        return Err(AppError(TondError::NotFound(format!(
            "Alert rule {id} not found"
        ))));
    }

    // Ensure integration exists
    let exists_int: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM integrations WHERE id = ?)")
            .bind(&req.integration_id)
            .fetch_one(&state.db)
            .await?;
    if !exists_int {
        return Err(AppError(TondError::NotFound(format!(
            "Integration {} not found",
            req.integration_id
        ))));
    }

    // Prevent duplicate links
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM alert_rule_notifications WHERE rule_id = ? AND integration_id = ?)",
    )
    .bind(&id)
    .bind(&req.integration_id)
    .fetch_one(&state.db)
    .await?;
    if already {
        return Err(AppError(TondError::InvalidInput(
            "Integration already linked to this rule".to_string(),
        )));
    }

    let link_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO alert_rule_notifications (id, rule_id, integration_id) VALUES (?, ?, ?)",
    )
    .bind(&link_id)
    .bind(&id)
    .bind(&req.integration_id)
    .execute(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(
            serde_json::json!({ "id": link_id, "rule_id": id, "integration_id": req.integration_id }),
        )),
    ))
}

// ── Unlink an integration from an alert rule ──

pub async fn unlink_alert_integration(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path((rule_id, integration_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let deleted = sqlx::query(
        "DELETE FROM alert_rule_notifications WHERE rule_id = ? AND integration_id = ?",
    )
    .bind(&rule_id)
    .bind(&integration_id)
    .execute(&state.db)
    .await?
    .rows_affected();

    if deleted == 0 {
        return Err(AppError(TondError::NotFound("Link not found".to_string())));
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            serde_json::json!({ "unlinked": true, "rule_id": rule_id, "integration_id": integration_id }),
        )),
    ))
}
