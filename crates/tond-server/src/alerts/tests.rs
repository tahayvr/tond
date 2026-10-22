// Engine tests with simulated time: rules are evaluated against fixed inputs
// and seeded tables at chosen unix times; deliveries go to a local mock
// webhook receiver.

use std::sync::{Arc, Mutex};

use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
use bollard::{API_DEFAULT_VERSION, Docker};
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tond_common::{ALERT_SNOOZE_UNTIL_RESOLVED, QuietHours, ServerSettings};

use super::evaluate::{Inputs, Source, evaluate};
use super::machine::MAX_ATTEMPTS;
use super::{deliver, quiet};
use crate::{auth::RateLimiter, secrets, state::AppState};

const T0: i64 = 1_800_000_000;

async fn test_state() -> AppState {
    let db = SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap_or_else(|err| panic!("sqlite: {err}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|err| panic!("migrate: {err}"));
    AppState {
        db,
        docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
            .unwrap_or_else(|err| panic!("docker client: {err}")),
        rate_limiter: RateLimiter::new(60, 10.0),
        settings: Arc::new(RwLock::new(ServerSettings::default())),
    }
}

async fn exec(db: &SqlitePool, sql: &'static str) {
    sqlx::query(sql)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("{sql}: {err}"));
}

async fn seed_app(db: &SqlitePool, id: &str, name: &str, status: &str) {
    sqlx::query(
        "INSERT INTO apps (id, name, status, created_at, updated_at) VALUES (?, ?, ?, 'x', 'x')",
    )
    .bind(id)
    .bind(name)
    .bind(status)
    .execute(db)
    .await
    .unwrap_or_else(|err| panic!("seed app: {err}"));
}

/// A generic webhook channel pointing at `url` (optionally signed).
async fn seed_webhook(db: &SqlitePool, id: &str, url: &str, secret: Option<&str>) {
    let mut config = serde_json::json!({ "webhook_url": url });
    if let Some(s) = secret {
        config["signing_secret"] = serde_json::Value::String(s.to_string());
    }
    let sealed = secrets::seal_integration_config("webhook", &config, None)
        .unwrap_or_else(|err| panic!("seal: {err}"));
    sqlx::query("INSERT INTO integrations (id, name, kind, config) VALUES (?, ?, 'webhook', ?)")
        .bind(id)
        .bind(format!("hook {id}"))
        .bind(sealed)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("seed integration: {err}"));
}

struct Rule<'a> {
    id: &'a str,
    kind: &'a str,
    app_id: Option<&'a str>,
    threshold: f64,
    window: i64,
    for_secs: i64,
    severity: &'a str,
    repeat: i64,
    channels: &'a [&'a str],
}

impl Default for Rule<'_> {
    fn default() -> Self {
        Rule {
            id: "r1",
            kind: "cpu",
            app_id: None,
            threshold: 80.0,
            window: 0,
            for_secs: 0,
            severity: "warning",
            repeat: 4 * 3600,
            channels: &["hook"],
        }
    }
}

async fn seed_rule(db: &SqlitePool, r: Rule<'_>) {
    sqlx::query(
        "INSERT INTO alert_rules (id, name, kind, app_id, threshold, window_secs, for_secs,
            severity, repeat_secs) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(r.id)
    .bind(format!("rule {}", r.id))
    .bind(r.kind)
    .bind(r.app_id)
    .bind(r.threshold)
    .bind(r.window)
    .bind(r.for_secs)
    .bind(r.severity)
    .bind(r.repeat)
    .execute(db)
    .await
    .unwrap_or_else(|err| panic!("seed rule: {err}"));
    for c in r.channels {
        sqlx::query("INSERT INTO alert_rule_notifications (rule_id, integration_id) VALUES (?, ?)")
            .bind(r.id)
            .bind(*c)
            .execute(db)
            .await
            .unwrap_or_else(|err| panic!("link: {err}"));
    }
}

fn cpu(app: &str, percent: f64) -> Source {
    let mut i = Inputs::default();
    i.app_samples.insert(app.to_string(), (percent, 0));
    Source::Fixed(i)
}

async fn eval(state: &AppState, now: i64, source: &Source) -> super::evaluate::Report {
    evaluate(&state.db, now, source)
        .await
        .unwrap_or_else(|err| panic!("evaluate: {err}"))
}

async fn state_of(db: &SqlitePool, rule: &str, target: &str) -> Option<String> {
    sqlx::query_scalar("SELECT state FROM alert_states WHERE rule_id = ? AND target = ?")
        .bind(rule)
        .bind(target)
        .fetch_optional(db)
        .await
        .unwrap_or_else(|err| panic!("state: {err}"))
}

/// (event, status, attempts) of every delivery, oldest first.
async fn deliveries(db: &SqlitePool) -> Vec<(String, String, i64)> {
    sqlx::query_as(
        "SELECT event, status, attempts FROM alert_deliveries ORDER BY created_at, rowid",
    )
    .fetch_all(db)
    .await
    .unwrap_or_else(|err| panic!("deliveries: {err}"))
}

#[derive(Debug, Clone)]
struct Received {
    headers: HeaderMap,
    body: Bytes,
}

/// A local webhook receiver answering `status`; returns its URL.
async fn receiver(status: u16) -> (String, Arc<Mutex<Vec<Received>>>) {
    let seen: Arc<Mutex<Vec<Received>>> = Arc::default();
    let store = seen.clone();
    let app = Router::new().route(
        "/hook",
        post(move |headers: HeaderMap, body: Bytes| {
            let store = store.clone();
            async move {
                store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Received { headers, body });
                axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|err| panic!("bind: {err}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|err| panic!("addr: {err}"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/hook"), seen)
}

fn received(seen: &Arc<Mutex<Vec<Received>>>) -> Vec<serde_json::Value> {
    seen.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap_or_default())
        .collect()
}

async fn send_due(state: &AppState, now: i64) -> usize {
    deliver::process_due(state, now)
        .await
        .unwrap_or_else(|err| panic!("deliver: {err}"))
}

#[tokio::test]
async fn pending_becomes_firing_after_for_and_notifies_once() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    let (url, seen) = receiver(200).await;
    seed_webhook(db, "hook", &url, Some("sig")).await;
    seed_rule(
        db,
        Rule {
            for_secs: 60,
            ..Rule::default()
        },
    )
    .await;

    let hot = cpu("a1", 93.0);
    let r = eval(&state, T0, &hot).await;
    assert_eq!((r.fired, r.queued), (0, 0));
    assert_eq!(
        state_of(db, "r1", "app:a1").await.as_deref(),
        Some("pending")
    );
    eval(&state, T0 + 30, &hot).await;
    assert_eq!(
        state_of(db, "r1", "app:a1").await.as_deref(),
        Some("pending")
    );
    let r = eval(&state, T0 + 60, &hot).await;
    assert_eq!((r.fired, r.queued), (1, 1));
    assert_eq!(
        state_of(db, "r1", "app:a1").await.as_deref(),
        Some("firing")
    );
    // Still firing: no second notification before the repeat interval.
    let r = eval(&state, T0 + 90, &hot).await;
    assert_eq!(r.queued, 0);

    assert_eq!(send_due(&state, T0 + 90).await, 1);
    let got = received(&seen);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["event"], "firing");
    assert_eq!(got[0]["alert"]["target_label"], "api");
    assert_eq!(got[0]["alert"]["value"], 93.0);
    assert_eq!(
        got[0]["alert"]["pending_since"],
        "2027-01-15T08:00:00+00:00"
    );
    let raw = seen.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
    assert_eq!(
        raw.headers
            .get("x-tond-signature-256")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        Some(crate::notifications::sign("sig", &raw.body))
    );
    assert_eq!(
        deliveries(db).await,
        vec![("firing".into(), "sent".into(), 1)]
    );
    let last_sent: Option<i64> = sqlx::query_scalar("SELECT last_sent_at FROM integrations")
        .fetch_one(db)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(last_sent, Some(T0 + 90));
}

#[tokio::test]
async fn flapping_never_fires_and_clears_its_state() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(
        db,
        Rule {
            for_secs: 120,
            ..Rule::default()
        },
    )
    .await;
    for i in 0..10 {
        let v = if i % 2 == 0 { 95.0 } else { 10.0 };
        let r = eval(&state, T0 + i * 30, &cpu("a1", v)).await;
        assert_eq!(r.fired, 0);
    }
    assert_eq!(state_of(db, "r1", "app:a1").await, None);
    assert!(deliveries(db).await.is_empty());
}

#[tokio::test]
async fn resolve_notifies_channels_that_heard_about_it_and_closes_history() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    let (url, seen) = receiver(200).await;
    seed_webhook(db, "hook", &url, None).await;
    seed_rule(db, Rule::default()).await;

    eval(&state, T0, &cpu("a1", 99.0)).await;
    send_due(&state, T0).await;
    let r = eval(&state, T0 + 600, &cpu("a1", 12.0)).await;
    assert_eq!((r.resolved, r.queued), (1, 1));
    assert_eq!(state_of(db, "r1", "app:a1").await, None);
    send_due(&state, T0 + 600).await;
    let got = received(&seen);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1]["event"], "resolved");
    assert_eq!(got[1]["alert"]["duration_secs"], 600);
    assert_eq!(got[1]["alert"]["value"], 12.0, "the value it recovered to");

    let (resolved_at, last_value): (Option<i64>, Option<f64>) =
        sqlx::query_as("SELECT resolved_at, last_value FROM alert_history")
            .fetch_one(db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!((resolved_at, last_value), (Some(T0 + 600), Some(12.0)));
}

#[tokio::test]
async fn repeats_every_interval_until_acknowledged() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    let (url, _seen) = receiver(200).await;
    seed_webhook(db, "hook", &url, None).await;
    seed_rule(
        db,
        Rule {
            repeat: 3600,
            ..Rule::default()
        },
    )
    .await;
    let hot = cpu("a1", 99.0);
    eval(&state, T0, &hot).await;
    assert_eq!(eval(&state, T0 + 3599, &hot).await.queued, 0);
    assert_eq!(eval(&state, T0 + 3600, &hot).await.queued, 1);
    assert_eq!(eval(&state, T0 + 3630, &hot).await.queued, 0);
    exec(
        db,
        "UPDATE alert_history SET acknowledged_at = 1, acknowledged_by = 'me'",
    )
    .await;
    assert_eq!(
        eval(&state, T0 + 3 * 3600, &hot).await.queued,
        0,
        "acknowledged"
    );
    let events: Vec<String> = deliveries(db).await.into_iter().map(|d| d.0).collect();
    assert_eq!(events, vec!["firing", "repeat"]);
}

#[tokio::test]
async fn snoozed_alerts_fire_silently_and_notify_when_the_snooze_ends() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(
        db,
        Rule {
            for_secs: 60,
            ..Rule::default()
        },
    )
    .await;
    let hot = cpu("a1", 99.0);
    eval(&state, T0, &hot).await; // pending
    // Snooze this target for 10 minutes while pending.
    sqlx::query("UPDATE alert_states SET snoozed_until = ? WHERE rule_id = 'r1'")
        .bind(T0 + 600)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    let r = eval(&state, T0 + 60, &hot).await;
    assert_eq!((r.fired, r.queued), (1, 0), "fires, but quietly");
    assert_eq!(eval(&state, T0 + 599, &hot).await.queued, 0);
    assert_eq!(eval(&state, T0 + 600, &hot).await.queued, 1, "snooze over");

    // Rule-level mute and "until resolved" work the same way.
    exec(db, "UPDATE alert_history SET last_notified_at = NULL").await;
    sqlx::query("UPDATE alert_states SET snoozed_until = ?")
        .bind(ALERT_SNOOZE_UNTIL_RESOLVED)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(eval(&state, T0 + 700, &hot).await.queued, 0);
    // Resolving drops an until-resolved snooze with the state.
    eval(&state, T0 + 800, &cpu("a1", 1.0)).await;
    assert_eq!(state_of(db, "r1", "app:a1").await, None);
}

#[tokio::test]
async fn a_timed_snooze_outlives_the_episode() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(db, Rule::default()).await;
    eval(&state, T0, &cpu("a1", 99.0)).await;
    sqlx::query("UPDATE alert_states SET snoozed_until = ?")
        .bind(T0 + 3600)
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    eval(&state, T0 + 60, &cpu("a1", 1.0)).await;
    assert_eq!(
        state_of(db, "r1", "app:a1").await.as_deref(),
        Some("inactive")
    );
    // Fires again within the hour: still muted.
    let r = eval(&state, T0 + 120, &cpu("a1", 99.0)).await;
    assert_eq!((r.fired, r.queued), (1, 0));
}

#[tokio::test]
async fn quiet_hours_defer_non_critical_alerts_and_drop_them_if_resolved_meanwhile() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    let (url, seen) = receiver(200).await;
    seed_webhook(db, "hook", &url, None).await;
    seed_rule(db, Rule::default()).await;
    seed_rule(
        db,
        Rule {
            id: "crit",
            severity: "critical",
            ..Rule::default()
        },
    )
    .await;
    // T0 is 08:00 UTC: quiet 07:00–09:00.
    quiet::save_global(
        db,
        &QuietHours {
            enabled: true,
            start: "07:00".into(),
            end: "09:00".into(),
            timezone: "UTC".into(),
            critical_bypass: true,
        },
    )
    .await
    .unwrap_or_else(|err| panic!("{err}"));
    eval(&state, T0, &cpu("a1", 99.0)).await;
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT h.rule_id, d.next_attempt_at, d.deferred FROM alert_deliveries d
         JOIN alert_history h ON h.id = d.episode_id ORDER BY h.rule_id",
    )
    .fetch_all(db)
    .await
    .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        rows,
        vec![
            ("crit".to_string(), T0, 0),
            ("r1".to_string(), T0 + 3600, 1)
        ],
        "critical bypasses; warning waits until 09:00"
    );
    assert_eq!(send_due(&state, T0).await, 1, "only the critical one now");
    // Resolved before quiet hours end: neither firing nor resolved is sent.
    eval(&state, T0 + 1800, &cpu("a1", 1.0)).await;
    send_due(&state, T0 + 3600).await;
    let got = received(&seen);
    assert_eq!(got.len(), 2, "critical fired + critical resolved");
    assert!(got.iter().all(|g| g["alert"]["rule_id"] == "crit"));
    let skipped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT d.event, d.last_error FROM alert_deliveries d JOIN alert_history h
         ON h.id = d.episode_id WHERE h.rule_id = 'r1' AND d.status = 'skipped'",
    )
    .fetch_all(db)
    .await
    .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(skipped.len(), 2, "both held messages: {skipped:?}");
    assert_eq!(skipped[0].0, "firing");
    assert!(
        skipped[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("resolved before")
    );
    assert_eq!(skipped[1].0, "resolved");
    assert!(
        skipped[1]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("never told")
    );
}

#[tokio::test]
async fn delivery_failures_are_recorded_retried_with_backoff_and_given_up() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    let (url, seen) = receiver(503).await;
    seed_webhook(db, "hook", &url, None).await;
    seed_rule(db, Rule::default()).await;
    eval(&state, T0, &cpu("a1", 99.0)).await;

    assert_eq!(send_due(&state, T0).await, 1);
    let (attempts, next, err): (i64, i64, Option<String>) =
        sqlx::query_as("SELECT attempts, next_attempt_at, last_error FROM alert_deliveries")
            .fetch_one(db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((attempts, next), (1, T0 + 30));
    assert!(
        err.as_deref().unwrap_or_default().contains("HTTP 503"),
        "{err:?}"
    );
    let channel_err: Option<String> = sqlx::query_scalar("SELECT last_error FROM integrations")
        .fetch_one(db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(channel_err.unwrap_or_default().contains("HTTP 503"));

    // Not due yet: nothing is sent.
    assert_eq!(send_due(&state, T0 + 29).await, 0);
    let mut now = T0;
    for _ in 1..MAX_ATTEMPTS {
        now = sqlx::query_scalar("SELECT next_attempt_at FROM alert_deliveries")
            .fetch_one(db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(send_due(&state, now).await, 1);
    }
    assert_eq!(
        deliveries(db).await,
        vec![("firing".into(), "failed".into(), MAX_ATTEMPTS)]
    );
    assert_eq!(
        seen.lock().unwrap_or_else(|e| e.into_inner()).len() as i64,
        MAX_ATTEMPTS
    );
    assert_eq!(deliver::next_due(db).await.ok().flatten(), None);
    assert!(now > T0);

    // The failure is visible on the live alert.
    let active = crate::handlers::alerts::active_alerts(db)
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].failed_deliveries, 1);
}

#[tokio::test]
async fn a_switched_off_channel_is_skipped_visibly() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    exec(db, "UPDATE integrations SET enabled = 0").await;
    seed_rule(db, Rule::default()).await;
    let r = eval(&state, T0, &cpu("a1", 99.0)).await;
    assert_eq!(r.queued, 0);
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT status, last_error FROM alert_deliveries")
            .fetch_all(db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "skipped");
    assert!(
        rows[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("switched off")
    );
}

#[tokio::test]
async fn event_style_rules_follow_their_tables() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_app(db, "a2", "web", "failed").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    for (id, kind, threshold, window) in [
        ("deploy", "deploy_failed", 0.0, 0),
        ("backup", "backup_failed", 0.0, 0),
        ("down", "app_down", 0.0, 0),
        ("loop", "crash_loop", 3.0, 600),
        ("oom", "oom", 0.0, 900),
        ("cert", "cert_expiry", 14.0, 0),
        ("cron", "cron_failed", 0.0, 0),
    ] {
        seed_rule(
            db,
            Rule {
                id,
                kind,
                threshold,
                window,
                ..Rule::default()
            },
        )
        .await;
    }
    let none = Source::Fixed(Inputs::default());

    // Deploys: a1's latest deploy failed.
    for (id, app, status, at) in [
        ("d1", "a1", "succeeded", "2026-01-01T00:00:00Z"),
        ("d2", "a1", "failed", "2026-01-02T00:00:00Z"),
        ("d3", "a2", "failed", "2026-01-01T00:00:00Z"),
        ("d4", "a2", "succeeded", "2026-01-02T00:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO deployments (id, app_id, status, started_at, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(app)
        .bind(status)
        .bind(at)
        .bind(at)
        .execute(db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    }
    exec(db, "INSERT INTO snapshots (id, name, status, created_at) VALUES ('s1', 's', 'failed', '2026-01-01 00:00:00')").await;
    // Four restarts and one OOM of a1 within the window.
    exec(
        db,
        "INSERT INTO metric_targets (id, kind, owner_id) VALUES (1, 'app', 'a1')",
    )
    .await;
    for (i, kind) in ["restart", "crash", "restart", "restart", "oom"]
        .iter()
        .enumerate()
    {
        sqlx::query("INSERT INTO metric_events (target, ts, kind) VALUES (1, ?, ?)")
            .bind(T0 - 60 * (i as i64 + 1))
            .bind(*kind)
            .execute(db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }
    sqlx::query(
        "INSERT INTO certificates (id, domain, cert_path, key_path, issued_at, expires_at, status)
         VALUES ('c1', 'soon.example.com', 'c', 'k', 'x', ?, 'active'),
                ('c2', 'later.example.com', 'c', 'k', 'x', ?, 'active')",
    )
    .bind(
        chrono::DateTime::from_timestamp(T0 + 5 * 86_400, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    )
    .bind(
        chrono::DateTime::from_timestamp(T0 + 60 * 86_400, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    )
    .execute(db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    // Uploaded certificates are never renewed: their expiry alerts matter most.
    sqlx::query(
        "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at, status)
         VALUES ('c3', 'upload.example.com', 'custom', 'c', 'k', 'x', ?, 'active')",
    )
    .bind(
        chrono::DateTime::from_timestamp(T0 + 3 * 86_400, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    )
    .execute(db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    exec(db, "INSERT INTO cron_jobs (id, name, schedule, image, last_run_status) VALUES ('j1', 'nightly', '* * * * *', 'alpine', 'failed')").await;

    let r = eval(&state, T0, &none).await;
    let firing: Vec<(String, String)> = sqlx::query_as(
        "SELECT rule_id, target FROM alert_states WHERE state = 'firing' ORDER BY rule_id, target",
    )
    .fetch_all(db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let expected: Vec<(String, String)> = [
        ("backup", "backup"),
        ("cert", "cert:soon.example.com"),
        ("cert", "cert:upload.example.com"),
        ("cron", "cron:j1"),
        ("deploy", "app:a1"),
        ("down", "app:a2"),
        ("loop", "app:a1"),
        ("oom", "app:a1"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(firing, expected);
    assert_eq!(r.fired, 8);

    // Recovery: a successful deploy, backup and cron run; events age out.
    exec(db, "INSERT INTO deployments (id, app_id, status, started_at, created_at) VALUES ('d5', 'a1', 'succeeded', '2026-01-03T00:00:00Z', '2026-01-03T00:00:00Z')").await;
    exec(db, "INSERT INTO snapshots (id, name, status, created_at) VALUES ('s2', 's', 'completed', '2026-01-02 00:00:00')").await;
    exec(db, "UPDATE cron_jobs SET last_run_status = 'success'").await;
    exec(db, "UPDATE apps SET status = 'running'").await;
    let r = eval(&state, T0 + 3600, &none).await;
    assert_eq!(r.resolved, 6, "everything but the certificate recovered");
    let left: Vec<String> = sqlx::query_scalar("SELECT rule_id FROM alert_states")
        .fetch_all(db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(left, vec!["cert", "cert"]);
}

#[tokio::test]
async fn a_failing_database_backup_fires_even_after_a_good_full_snapshot() {
    let state = test_state().await;
    let db = &state.db;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(
        db,
        Rule {
            id: "backup",
            kind: "backup_failed",
            ..Rule::default()
        },
    )
    .await;
    exec(
        db,
        "INSERT INTO snapshots (id, name, status, trigger, scope, created_at) VALUES
        ('s1', 's', 'failed', 'scheduled', 'database:shop', '2026-01-01 00:00:00'),
        ('s2', 's', 'completed', 'scheduled', 'full', '2026-01-02 00:00:00'),
        ('s3', 's', 'completed', 'scheduled', 'database:cache', '2026-01-02 00:00:00')",
    )
    .await;
    let none = Source::Fixed(Inputs::default());
    eval(&state, T0, &none).await;
    let firing: Vec<(String, String)> = sqlx::query_as(
        "SELECT target, target_label FROM alert_states WHERE state = 'firing' ORDER BY target",
    )
    .fetch_all(db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        firing,
        vec![(
            "backup:database:shop".to_string(),
            "Backups of shop".to_string()
        )]
    );

    // The next scheduled backup of shop succeeds: resolved.
    exec(
        db,
        "INSERT INTO snapshots (id, name, status, trigger, scope, created_at) VALUES
        ('s4', 's', 'completed', 'scheduled', 'database:shop', '2026-01-03 00:00:00')",
    )
    .await;
    let r = eval(&state, T0 + 60, &none).await;
    assert_eq!(r.resolved, 1);
}

#[tokio::test]
async fn http_rules_fire_from_the_proxy_request_metrics() {
    use tond_common::traffic::{RouteKind, TrafficCounters};
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(
        db,
        Rule {
            kind: "http_5xx_rate",
            threshold: 5.0,
            window: 300,
            ..Rule::default()
        },
    )
    .await;
    let none = Source::Fixed(Inputs::default());
    // No traffic yet: nothing to judge.
    assert_eq!(eval(&state, T0, &none).await.fired, 0);
    let target = crate::metrics::requests::ensure_target(db, &(RouteKind::App, "a1".into()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut minute = TrafficCounters {
        requests: 100,
        status: [0, 80, 0, 0, 20],
        ..Default::default()
    };
    minute.latency[11] = 100;
    crate::metrics::requests::write_minute(db, T0 - 60, &[(target, minute)], None)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(eval(&state, T0, &none).await.fired, 1);
    let v: Option<f64> = sqlx::query_scalar("SELECT value FROM alert_history")
        .fetch_one(db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v, Some(20.0));
}

#[tokio::test]
async fn memory_disk_and_forecast_rules_use_live_readings() {
    use super::sources::Forecast;
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_app(db, "a2", "nolimit", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    exec(db, "INSERT INTO metric_targets (kind, owner_id, mem_limit) VALUES ('app', 'a1', 1000), ('app', 'a2', 0)").await;
    seed_rule(
        db,
        Rule {
            id: "mem",
            kind: "memory",
            threshold: 50.0,
            ..Rule::default()
        },
    )
    .await;
    seed_rule(
        db,
        Rule {
            id: "disk",
            kind: "disk_usage",
            threshold: 90.0,
            ..Rule::default()
        },
    )
    .await;
    seed_rule(
        db,
        Rule {
            id: "fc",
            kind: "disk_forecast",
            threshold: 48.0,
            ..Rule::default()
        },
    )
    .await;
    let mut i = Inputs::default();
    i.app_samples.insert("a1".into(), (1.0, 800));
    i.app_samples.insert("a2".into(), (1.0, 999_999));
    i.disk_percent = Some(95.0);
    i.disk_forecast = Some(Forecast::HoursLeft(20.0));
    let r = eval(&state, T0, &Source::Fixed(i)).await;
    assert_eq!(r.fired, 3);
    let targets: Vec<(String, String, Option<f64>)> =
        sqlx::query_as("SELECT rule_id, target, value FROM alert_states ORDER BY rule_id")
            .fetch_all(db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        targets,
        vec![
            ("disk".into(), "host".into(), Some(95.0)),
            ("fc".into(), "host".into(), Some(20.0)),
            ("mem".into(), "app:a1".into(), Some(80.0)),
        ],
        "apps without a memory limit are skipped"
    );
    // Not enough disk history: the forecast holds its state.
    let i = Inputs {
        disk_percent: Some(10.0),
        disk_forecast: Some(Forecast::Unknown),
        ..Inputs::default()
    };
    let r = eval(&state, T0 + 30, &Source::Fixed(i)).await;
    assert_eq!(
        r.resolved, 2,
        "disk and memory (no sample) resolve; forecast holds"
    );
}

#[tokio::test]
async fn an_automatic_disk_cleanup_fires_for_an_hour() {
    let state = test_state().await;
    let db = &state.db;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(
        db,
        Rule {
            id: "prune",
            kind: "disk_auto_prune",
            threshold: 0.0,
            window: 0,
            ..Rule::default()
        },
    )
    .await;
    let ran = |at: i64| Inputs {
        auto_prune: Some((at, 72.0)),
        ..Inputs::default()
    };
    let r = eval(&state, T0, &Source::Fixed(ran(T0 - 10))).await;
    assert_eq!(r.fired, 1);
    let value: Option<f64> =
        sqlx::query_scalar("SELECT value FROM alert_states WHERE rule_id = 'prune'")
            .fetch_one(db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(value, Some(72.0), "the disk use after the cleanup");
    let r = eval(
        &state,
        T0 + crate::disk_pressure::ALERT_HOLD_SECS,
        &Source::Fixed(ran(T0 - 10)),
    )
    .await;
    assert_eq!(r.resolved, 1, "an hour later it resolves");
}

#[tokio::test]
async fn disabling_a_rule_closes_its_alerts_without_notifying() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(db, Rule::default()).await;
    eval(&state, T0, &cpu("a1", 99.0)).await;
    super::evaluate::close_rule(db, "r1", T0 + 10)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(state_of(db, "r1", "app:a1").await, None);
    let resolved: Option<i64> = sqlx::query_scalar("SELECT resolved_at FROM alert_history")
        .fetch_one(db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(resolved, Some(T0 + 10));
    let events: Vec<String> = deliveries(db).await.into_iter().map(|d| d.0).collect();
    assert_eq!(
        events,
        vec!["firing"],
        "no resolve message for a disabled rule"
    );
}

#[tokio::test]
async fn app_down_follows_the_uptime_check_of_a_running_app() {
    let state = test_state().await;
    let db = &state.db;
    seed_app(db, "a1", "api", "running").await;
    seed_app(db, "a2", "web", "stopped").await;
    seed_webhook(db, "hook", "http://127.0.0.1:1/hook", None).await;
    seed_rule(
        db,
        Rule {
            id: "down",
            kind: "app_down",
            threshold: 0.0,
            window: 0,
            ..Rule::default()
        },
    )
    .await;
    let mut i = Inputs::default();
    i.uptime_down.insert("a1".to_string());
    i.uptime_down.insert("a2".to_string());
    eval(&state, T0, &Source::Fixed(i)).await;
    assert!(
        state_of(db, "down", "app:a1").await.is_some(),
        "a running app whose public URL fails is down"
    );
    assert_eq!(
        state_of(db, "down", "app:a2").await,
        None,
        "a stopped app never counts"
    );
    eval(&state, T0 + 60, &Source::Fixed(Inputs::default())).await;
    assert_ne!(
        state_of(db, "down", "app:a1").await.as_deref(),
        Some("firing")
    );
}
