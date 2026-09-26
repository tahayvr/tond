use std::sync::Arc;

use axum::{body::to_bytes, response::Response};
use bollard::{API_DEFAULT_VERSION, Docker};
use sqlx::SqlitePool;
use tokio::sync::RwLock;

use super::*;
use crate::alerts::evaluate::{Inputs, Source};
use crate::auth::{Principal, RateLimiter};
use crate::handlers::integrations;

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

fn admin<S: crate::auth::principal::ScopeReq>() -> Authz<S> {
    Authz::unchecked(Principal::test_user(Role::Admin))
}

async fn body_json(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|err| panic!("failed to read body: {err}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("invalid JSON: {err}"))
}

async fn data(r: Result<impl IntoResponse, AppError>) -> serde_json::Value {
    let response = r
        .unwrap_or_else(|err| panic!("handler failed: {}", err.0))
        .into_response();
    body_json(response).await["data"].clone()
}

fn err_text<T>(r: Result<T, AppError>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.0.to_string(),
    }
}

async fn channel(state: &AppState, kind: &str, config: serde_json::Value) -> String {
    let req: CreateIntegrationRequest = serde_json::from_value(serde_json::json!({
        "name": format!("{kind} channel"),
        "kind": kind,
        "config": config,
    }))
    .unwrap_or_else(|err| panic!("request: {err}"));
    let d = data(
        integrations::create_integration(RateLimit, admin(), State(state.clone()), Json(req)).await,
    )
    .await;
    d["id"].as_str().unwrap_or_default().to_string()
}

async fn webhook(state: &AppState) -> String {
    channel(
        state,
        "webhook",
        serde_json::json!({ "webhook_url": "https://hooks.example.com/x" }),
    )
    .await
}

fn create_req(v: serde_json::Value) -> CreateAlertRuleRequest {
    serde_json::from_value(v).unwrap_or_else(|err| panic!("request should deserialize: {err}"))
}

async fn create(state: &AppState, v: serde_json::Value) -> Result<serde_json::Value, AppError> {
    let response = create_alert(
        RateLimit,
        admin(),
        State(state.clone()),
        Json(create_req(v)),
    )
    .await?
    .into_response();
    Ok(body_json(response).await["data"].clone())
}

#[tokio::test]
async fn rules_need_a_channel_a_known_kind_and_sane_values() {
    let state = test_state().await;
    let hook = webhook(&state).await;

    let msg = err_text(create(&state, serde_json::json!({ "name": "cpu", "kind": "cpu" })).await);
    assert!(msg.contains("notification channel"), "{msg}");
    let msg = err_text(
        create(
            &state,
            serde_json::json!({ "name": "x", "kind": "nope", "integration_ids": [hook] }),
        )
        .await,
    );
    assert!(msg.contains("Unknown rule kind"), "{msg}");
    let msg = err_text(
        create(
            &state,
            serde_json::json!({ "name": "x", "kind": "memory", "threshold": 150, "integration_ids": [hook] }),
        )
        .await,
    );
    assert!(msg.contains("between 0% and 100%"), "{msg}");
    let msg = err_text(
        create(
            &state,
            serde_json::json!({ "name": "x", "kind": "cpu", "repeat_secs": 10, "integration_ids": [hook] }),
        )
        .await,
    );
    assert!(msg.contains("Repeat"), "{msg}");
    // HTTP rules are accepted: request metrics come from tond-proxy.
    let http = create(
        &state,
        serde_json::json!({ "name": "errors", "kind": "http_5xx_rate", "integration_ids": [hook] }),
    )
    .await
    .unwrap_or_else(|e| panic!("HTTP rules must be accepted: {}", e.0));
    assert_eq!(http["kind"], "http_5xx_rate");

    // Defaults come from the kind.
    let rule = create(
        &state,
        serde_json::json!({ "name": "  Slow disk  ", "kind": "disk_forecast", "app_id": "ignored",
                            "integration_ids": [hook, hook] }),
    )
    .await
    .unwrap_or_else(|e| panic!("create: {}", e.0));
    assert_eq!(rule["name"], "Slow disk");
    assert_eq!(rule["threshold"], 48.0);
    assert_eq!(rule["for_secs"], 900);
    assert_eq!(rule["severity"], "warning");
    assert_eq!(rule["repeat_secs"], 14_400);
    assert_eq!(
        rule["app_id"],
        serde_json::Value::Null,
        "host rules have no app"
    );
    assert_eq!(rule["integration_ids"], serde_json::json!([hook]));
}

#[tokio::test]
async fn kinds_report_whether_their_data_exists() {
    let state = test_state().await;
    let kinds = data(list_kinds(RateLimit, admin(), State(state.clone())).await).await;
    let find = |k: &str| {
        kinds
            .as_array()
            .and_then(|a| a.iter().find(|x| x["kind"] == k).cloned())
            .unwrap_or_default()
    };
    assert_eq!(find("cpu")["available"], true);
    assert_eq!(find("http_p95_latency")["available"], true);
    assert_eq!(find("http_5xx_rate")["available"], true);
    assert!(find("http_p95_latency")["unavailable_reason"].is_null());
    assert_eq!(find("crash_loop")["unit"], "restarts");
}

#[tokio::test]
async fn updates_replace_channels_but_never_empty_them_and_disabling_closes_alerts() {
    let state = test_state().await;
    let a = webhook(&state).await;
    let b = channel(
        &state,
        "ntfy",
        serde_json::json!({ "topic": "tond-x", "server_url": "https://ntfy.example.com" }),
    )
    .await;
    let rule = create(
        &state,
        serde_json::json!({ "name": "down", "kind": "disk_usage", "integration_ids": [a] }),
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    let id = rule["id"].as_str().unwrap_or_default().to_string();

    let patch = |v: serde_json::Value| -> UpdateAlertRuleRequest {
        serde_json::from_value(v).unwrap_or_else(|err| panic!("patch: {err}"))
    };
    let msg = err_text(
        update_alert(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(id.clone()),
            Json(patch(serde_json::json!({ "integration_ids": [] }))),
        )
        .await,
    );
    assert!(msg.contains("notification channel"));
    let d = data(
        update_alert(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(id.clone()),
            Json(patch(
                serde_json::json!({ "integration_ids": [b], "for_secs": 0, "severity": "critical" }),
            )),
        )
        .await,
    )
    .await;
    assert_eq!(d["integration_ids"], serde_json::json!([b]));
    assert_eq!(d["severity"], "critical");

    // Fire it, then disable: the alert closes.
    let inputs = Inputs {
        disk_percent: Some(99.0),
        ..Inputs::default()
    };
    crate::alerts::evaluate::evaluate(&state.db, now_unix(), &Source::Fixed(inputs))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let active = data(list_active(RateLimit, admin(), State(state.clone())).await).await;
    assert_eq!(active.as_array().map(Vec::len), Some(1));
    assert_eq!(active[0]["state"], "firing");
    assert_eq!(active[0]["target_label"], "Server");
    let d = data(
        update_alert(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(id.clone()),
            Json(patch(serde_json::json!({ "enabled": false }))),
        )
        .await,
    )
    .await;
    assert_eq!(d["enabled"], false);
    let active = data(list_active(RateLimit, admin(), State(state.clone())).await).await;
    assert_eq!(active, serde_json::json!([]));
    let history = data(
        list_history(
            RateLimit,
            admin(),
            State(state.clone()),
            Query(AlertHistoryQuery::default()),
        )
        .await,
    )
    .await;
    assert_eq!(history["total"], 1);
    assert_eq!(history["episodes"][0]["state"], "resolved");
}

async fn fire_disk_alert(state: &AppState) -> (String, String) {
    let hook = webhook(state).await;
    let rule = create(
        state,
        serde_json::json!({ "name": "disk", "kind": "disk_usage", "for_secs": 0,
                            "integration_ids": [hook] }),
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    let inputs = Inputs {
        disk_percent: Some(97.5),
        ..Inputs::default()
    };
    crate::alerts::evaluate::evaluate(&state.db, now_unix(), &Source::Fixed(inputs))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let episode: String = sqlx::query_scalar("SELECT id FROM alert_history")
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    (rule["id"].as_str().unwrap_or_default().to_string(), episode)
}

#[tokio::test]
async fn snooze_unsnooze_and_acknowledge() {
    let state = test_state().await;
    let (rule, episode) = fire_disk_alert(&state).await;
    let snooze_body = |v: serde_json::Value| -> SnoozeBody {
        serde_json::from_value(v).unwrap_or_else(|err| panic!("body: {err}"))
    };

    let d = data(
        snooze(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(rule.clone()),
            Json(snooze_body(
                serde_json::json!({ "target": "host", "minutes": 60 }),
            )),
        )
        .await,
    )
    .await;
    assert_eq!(d["until_resolved"], false);
    let active = active_alerts(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert!(active[0].snoozed_until.is_some());
    assert!(!active[0].snoozed_until_resolved);

    data(
        snooze(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(rule.clone()),
            Json(snooze_body(
                serde_json::json!({ "target": "host", "until_resolved": true }),
            )),
        )
        .await,
    )
    .await;
    let active = active_alerts(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert!(active[0].snoozed_until_resolved);
    assert_eq!(active[0].snoozed_until, None);

    let msg = err_text(
        snooze(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(rule.clone()),
            Json(snooze_body(serde_json::json!({ "until_resolved": true }))),
        )
        .await,
    );
    assert!(msg.contains("single alert"), "{msg}");
    let msg = err_text(
        snooze(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(rule.clone()),
            Json(snooze_body(serde_json::json!({ "minutes": 999_999 }))),
        )
        .await,
    );
    assert!(msg.contains("30 days"), "{msg}");

    let unsnooze_body: UnsnoozeBody =
        serde_json::from_value(serde_json::json!({ "target": "host" }))
            .unwrap_or_else(|err| panic!("body: {err}"));
    data(
        unsnooze(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(rule.clone()),
            Json(unsnooze_body),
        )
        .await,
    )
    .await;
    let active = active_alerts(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert!(!active[0].snoozed_until_resolved);

    data(
        acknowledge(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(episode.clone()),
        )
        .await,
    )
    .await;
    let active = active_alerts(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert!(active[0].acknowledged_at.is_some());
    assert!(active[0].acknowledged_by.is_some());

    let detail =
        data(get_episode(RateLimit, admin(), State(state.clone()), Path(episode)).await).await;
    assert_eq!(detail["episode"]["state"], "firing");
    assert_eq!(detail["episode"]["value"], 97.5);
    assert_eq!(detail["deliveries"][0]["event"], "firing");
    assert_eq!(detail["deliveries"][0]["status"], "pending");
    assert_eq!(detail["deliveries"][0]["integration_kind"], "webhook");
}

#[tokio::test]
async fn history_filters_and_paginates() {
    let state = test_state().await;
    for i in 0..30 {
        sqlx::query(
            "INSERT INTO alert_history (id, rule_id, rule_name, kind, severity, target, target_label,
                app_id, threshold, pending_since, fired_at, resolved_at)
             VALUES (?, NULL, 'r', 'cpu', 'warning', 'app:a', 'a', ?, 80, ?, ?, ?)",
        )
        .bind(format!("e{i:02}"))
        .bind(if i % 3 == 0 { "a1" } else { "a2" })
        .bind(i)
        .bind(i)
        .bind(if i < 25 { Some(i + 1) } else { None })
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    }
    let q = |v: serde_json::Value| -> AlertHistoryQuery {
        serde_json::from_value(v).unwrap_or_else(|err| panic!("query: {err}"))
    };
    let page = data(
        list_history(
            RateLimit,
            admin(),
            State(state.clone()),
            Query(q(serde_json::json!({ "page": 2, "per_page": 10 }))),
        )
        .await,
    )
    .await;
    assert_eq!(page["total"], 30);
    assert_eq!(page["episodes"][0]["id"], "e19", "newest first");
    let firing = data(
        list_history(
            RateLimit,
            admin(),
            State(state.clone()),
            Query(q(serde_json::json!({ "state": "firing" }))),
        )
        .await,
    )
    .await;
    assert_eq!(firing["total"], 5);
    let app = data(
        list_history(
            RateLimit,
            admin(),
            State(state.clone()),
            Query(q(
                serde_json::json!({ "app_id": "a1", "state": "resolved" }),
            )),
        )
        .await,
    )
    .await;
    assert_eq!(app["total"], 9);
    let msg = err_text(
        list_history(
            RateLimit,
            admin(),
            State(state.clone()),
            Query(q(serde_json::json!({ "state": "weird" }))),
        )
        .await,
    );
    assert!(msg.contains("firing or resolved"));
}

#[tokio::test]
async fn global_quiet_hours_are_validated_and_saved() {
    let state = test_state().await;
    let d = data(get_settings(RateLimit, admin(), State(state.clone())).await).await;
    assert_eq!(d["quiet_hours"]["enabled"], false);
    let bad: AlertSettings = serde_json::from_value(serde_json::json!({
        "quiet_hours": { "enabled": true, "start": "22:00", "end": "07:00", "timezone": "Nowhere/City" }
    }))
    .unwrap_or_else(|err| panic!("{err}"));
    assert!(
        err_text(put_settings(RateLimit, admin(), State(state.clone()), Json(bad)).await)
            .contains("time zone")
    );
    let good: AlertSettings = serde_json::from_value(serde_json::json!({
        "quiet_hours": { "enabled": true, "start": "22:00", "end": "07:00", "timezone": "Europe/Berlin" }
    }))
    .unwrap_or_else(|err| panic!("{err}"));
    data(put_settings(RateLimit, admin(), State(state.clone()), Json(good)).await).await;
    let d = data(get_settings(RateLimit, admin(), State(state.clone())).await).await;
    assert_eq!(d["quiet_hours"]["timezone"], "Europe/Berlin");
    assert_eq!(d["quiet_hours"]["critical_bypass"], true);
}

#[tokio::test]
async fn channels_keep_secrets_write_only_and_store_quiet_hours() {
    let state = test_state().await;
    let req: CreateIntegrationRequest = serde_json::from_value(serde_json::json!({
        "name": "Phone",
        "kind": "pushover",
        "config": { "user_key": "u-123", "app_token": "a-456" },
        "quiet_hours": { "mode": "custom", "enabled": true, "start": "23:00", "end": "06:00",
                         "timezone": "UTC", "critical_bypass": false }
    }))
    .unwrap_or_else(|err| panic!("{err}"));
    let d = data(
        integrations::create_integration(RateLimit, admin(), State(state.clone()), Json(req)).await,
    )
    .await;
    let text = d.to_string();
    assert!(!text.contains("u-123") && !text.contains("a-456"), "{text}");
    assert_eq!(
        d["saved_secrets"],
        serde_json::json!(["user_key", "app_token"])
    );
    assert_eq!(d["quiet_hours"]["mode"], "custom");
    assert_eq!(d["quiet_hours"]["critical_bypass"], false);

    let bad: CreateIntegrationRequest = serde_json::from_value(serde_json::json!({
        "name": "Hook", "kind": "webhook", "config": { "webhook_url": "ftp://x" }
    }))
    .unwrap_or_else(|err| panic!("{err}"));
    assert!(
        err_text(
            integrations::create_integration(RateLimit, admin(), State(state.clone()), Json(bad))
                .await
        )
        .contains("http")
    );
    let bad_quiet: CreateIntegrationRequest = serde_json::from_value(serde_json::json!({
        "name": "Hook", "kind": "webhook", "config": { "webhook_url": "https://x.example.com" },
        "quiet_hours": { "mode": "custom", "enabled": true, "start": "9", "end": "10", "timezone": "UTC" }
    }))
    .unwrap_or_else(|err| panic!("{err}"));
    assert!(
        err_text(
            integrations::create_integration(
                RateLimit,
                admin(),
                State(state.clone()),
                Json(bad_quiet)
            )
            .await
        )
        .contains("Quiet hours")
    );
}

#[tokio::test]
async fn send_test_reports_each_channel() {
    let state = test_state().await;
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
    let good = channel(
        &state,
        "webhook",
        serde_json::json!({ "webhook_url": format!("http://{addr}/hook") }),
    )
    .await;
    let off = channel(
        &state,
        "gotify",
        serde_json::json!({ "server_url": "https://gotify.example.com", "app_token": "t" }),
    )
    .await;
    sqlx::query("UPDATE integrations SET enabled = 0 WHERE id = ?")
        .bind(&off)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let rule = create(
        &state,
        serde_json::json!({ "name": "deploys", "kind": "deploy_failed", "integration_ids": [good, off] }),
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    let id = rule["id"].as_str().unwrap_or_default().to_string();
    let results = data(test_alert(RateLimit, admin(), State(state.clone()), Path(id)).await).await;
    let results: Vec<crate::alerts::ChannelTestResult> =
        serde_json::from_value(results).unwrap_or_else(|err| panic!("results: {err}"));
    assert_eq!(results.len(), 2, "{results:?}");
    let by_kind = |k: &str| {
        results
            .iter()
            .find(|r| r.kind == k)
            .cloned()
            .unwrap_or_else(|| panic!("{k} result"))
    };
    assert!(by_kind("webhook").ok);
    assert!(!by_kind("gotify").ok);
    assert!(
        by_kind("gotify")
            .error
            .unwrap_or_default()
            .contains("switched off"),
        "a switched-off channel is reported, not silently skipped"
    );
    // A test never creates alert history.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_history")
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(n, 0);
}
