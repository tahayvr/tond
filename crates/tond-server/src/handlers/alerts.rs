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
//   POST   /api/alerts/{id}/test                 — send a test message to every channel
//
// A rule must always notify someone: creating one needs a channel or a webhook
// URL, and removing the last channel (or clearing the webhook) is refused.

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
    alerts::{channel_count, send_test_notifications},
    auth::{Authz, RateLimit, scope},
    error::AppError,
    secrets,
    state::AppState,
};

const NO_CHANNEL: &str =
    "Choose at least one notification channel or add a webhook URL, so this rule notifies someone.";

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

    // Must notify someone
    let webhook_url = req.webhook_url.trim().to_string();
    let mut integration_ids = req.integration_ids.clone();
    integration_ids.sort();
    integration_ids.dedup();
    if webhook_url.is_empty() && integration_ids.is_empty() {
        return Err(AppError(TondError::InvalidInput(NO_CHANNEL.to_string())));
    }
    for integration_id in &integration_ids {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM integrations WHERE id = ?)")
                .bind(integration_id)
                .fetch_one(&state.db)
                .await?;
        if !exists {
            return Err(AppError(TondError::InvalidInput(format!(
                "Notification channel {integration_id} doesn't exist"
            ))));
        }
    }

    let id = Uuid::new_v4().to_string();
    let threshold = req.threshold.unwrap_or(0.0);
    let events_json = events_to_json(&req.events);

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO alert_rules (id, app_id, name, metric, threshold, webhook_url, events)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&req.app_id)
    .bind(&req.name)
    .bind(&req.metric)
    .bind(threshold)
    .bind(secrets::seal_alert_webhook_url(&webhook_url)?)
    .bind(&events_json)
    .execute(&mut *tx)
    .await?;
    for integration_id in &integration_ids {
        sqlx::query(
            "INSERT INTO alert_rule_notifications (id, rule_id, integration_id) VALUES (?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&id)
        .bind(integration_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

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
    // Sealed URLs are empty exactly when no webhook is set.
    if channel_count(&state.db, &id, &webhook_url).await == 0 {
        return Err(AppError(TondError::InvalidInput(NO_CHANNEL.to_string())));
    }
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
    let webhook_url: Option<String> =
        sqlx::query_scalar("SELECT webhook_url FROM alert_rules WHERE id = ?")
            .bind(&rule_id)
            .fetch_optional(&state.db)
            .await?;
    let Some(webhook_url) = webhook_url else {
        return Err(AppError(TondError::NotFound(format!(
            "Alert rule {rule_id} not found"
        ))));
    };
    if channel_count(&state.db, &rule_id, &webhook_url).await <= 1 {
        return Err(AppError(TondError::InvalidInput(
            "This is the rule's only notification channel. Add another channel or a webhook URL first, or delete the rule."
                .to_string(),
        )));
    }

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

// ── Send a test notification ──

pub async fn test_alert(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let rule = sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("Alert rule {id} not found"))))?;

    let results = send_test_notifications(&state, &rule).await;
    if results.is_empty() {
        return Err(AppError(TondError::InvalidInput(NO_CHANNEL.to_string())));
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(results))))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{body::to_bytes, response::Response};
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;

    use super::*;
    use crate::{
        alerts::ChannelTestResult,
        auth::{Principal, RateLimiter},
    };

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to open in-memory SQLite: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to migrate: {err}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read body: {err}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("invalid JSON: {err}"))
    }

    async fn seed_integration(state: &AppState, id: &str, enabled: bool) {
        sqlx::query("INSERT INTO integrations (id, name, kind, config, enabled) VALUES (?, ?, 'slack', '{\"webhook_url\":\"https://hooks.invalid/x\"}', ?)")
            .bind(id)
            .bind(format!("channel {id}"))
            .bind(i64::from(enabled))
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to seed integration: {err}"));
    }

    fn request(webhook_url: &str, integration_ids: &[&str]) -> CreateAlertRuleRequest {
        serde_json::from_value(serde_json::json!({
            "name": "deploys",
            "events": ["deployment_failure"],
            "webhook_url": webhook_url,
            "integration_ids": integration_ids,
        }))
        .unwrap_or_else(|err| panic!("request should deserialize: {err}"))
    }

    async fn create(state: &AppState, req: CreateAlertRuleRequest) -> Result<String, AppError> {
        let response = create_alert(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(req),
        )
        .await?
        .into_response();
        let body = body_json(response).await;
        Ok(body["data"]["id"].as_str().unwrap_or_default().to_string())
    }

    #[tokio::test]
    async fn a_rule_without_any_channel_is_refused() {
        let state = test_state().await;
        let err = create(&state, request("  ", &[]))
            .await
            .err()
            .unwrap_or_else(|| panic!("a rule that notifies nobody must be refused"));
        assert!(
            matches!(err.0, TondError::InvalidInput(ref msg) if msg.contains("notification channel"))
        );

        // An unknown channel id is refused too.
        assert!(create(&state, request("", &["missing"])).await.is_err());
    }

    #[tokio::test]
    async fn channels_are_linked_on_creation_and_the_last_one_cannot_be_removed() {
        let state = test_state().await;
        seed_integration(&state, "int-a", true).await;
        seed_integration(&state, "int-b", true).await;
        let id = create(&state, request("", &["int-a", "int-b", "int-a"]))
            .await
            .unwrap_or_else(|err| panic!("rule with channels should be created: {}", err.0));
        assert_eq!(crate::alerts::channel_count(&state.db, &id, "").await, 2);

        assert!(
            unlink_alert_integration(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Path((id.clone(), "int-a".to_string()))
            )
            .await
            .is_ok(),
            "removing one of two channels is fine"
        );
        let err = unlink_alert_integration(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path((id.clone(), "int-b".to_string())),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("removing the only channel must be refused"));
        assert!(matches!(err.0, TondError::InvalidInput(_)));

        // Clearing the webhook of a rule with no other channel is refused too.
        let webhook_only = create(&state, request("https://hooks.invalid/a", &[]))
            .await
            .unwrap_or_else(|err| panic!("webhook-only rule should be created: {}", err.0));
        let patch: UpdateAlertRuleRequest =
            serde_json::from_value(serde_json::json!({ "webhook_url": "" }))
                .unwrap_or_else(|err| panic!("patch should deserialize: {err}"));
        assert!(
            update_alert(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Path(webhook_only),
                Json(patch)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn send_test_reports_each_channel() {
        let state = test_state().await;
        // A local webhook receiver that answers 200.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|err| panic!("failed to bind test listener: {err}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|err| panic!("no local addr: {err}"));
        tokio::spawn(async move {
            let app = axum::Router::new().route("/hook", axum::routing::post(|| async { "ok" }));
            let _ = axum::serve(listener, app).await;
        });
        seed_integration(&state, "int-off", false).await;
        let id = create(
            &state,
            request(&format!("http://{addr}/hook"), &["int-off"]),
        )
        .await
        .unwrap_or_else(|err| panic!("rule should be created: {}", err.0));

        let response = test_alert(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(id.clone()),
        )
        .await
        .unwrap_or_else(|err| panic!("test send should succeed: {}", err.0))
        .into_response();
        let body = body_json(response).await;
        let results: Vec<ChannelTestResult> = serde_json::from_value(body["data"].clone())
            .unwrap_or_else(|err| panic!("results should decode: {err}"));
        assert_eq!(results.len(), 2, "{results:?}");
        assert_eq!((results[0].kind.as_str(), results[0].ok), ("webhook", true));
        assert_eq!(results[1].kind, "slack");
        assert!(
            !results[1].ok,
            "a switched-off channel is reported, not silently skipped"
        );
        assert!(
            results[1]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("switched off")
        );

        // Firing a test never marks the rule as fired.
        let fired: Option<String> =
            sqlx::query_scalar("SELECT last_fired_at FROM alert_rules WHERE id = ?")
                .bind(&id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read rule: {err}"));
        assert_eq!(fired, None);
    }
}
