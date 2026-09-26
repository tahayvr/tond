use std::sync::Arc;

use bollard::{API_DEFAULT_VERSION, Docker};
use tokio::sync::RwLock as TokioRwLock;
use tond_common::ServerSettings;

use super::*;
use crate::auth::RateLimiter;

pub(crate) async fn test_state() -> AppState {
    let db = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap_or_else(|e| panic!("pool: {e}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|e| panic!("migrate: {e}"));
    AppState {
        db,
        docker: Docker::connect_with_http("http://127.0.0.1:1", 1, API_DEFAULT_VERSION)
            .unwrap_or_else(|e| panic!("docker: {e}")),
        rate_limiter: RateLimiter::new(60, 10.0),
        settings: Arc::new(TokioRwLock::new(ServerSettings::default())),
    }
}

fn row(id: i64, kind: &str, status: &str, domain: Option<&str>) -> store::CheckWithApp {
    store::CheckWithApp {
        check: store::CheckRow {
            id,
            kind: kind.into(),
            owner_id: if kind == "app" {
                format!("app-{id}")
            } else {
                String::new()
            },
            enabled: true,
            path: "/".into(),
            interval_secs: 60,
            expected_status: None,
            timeout_secs: 10,
        },
        app_name: (kind == "app").then(|| format!("web-{id}")),
        app_domain: domain.map(str::to_string),
        app_status: (kind == "app").then(|| status.to_string()),
        app_readiness: (kind == "app").then(|| "http".to_string()),
    }
}

#[test]
fn checks_pause_for_stopped_apps_missing_domains_and_no_dashboard_domain() {
    let checks = build_checks(
        vec![
            row(1, "dashboard", "", None),
            row(2, "app", "running", Some("web.example.com")),
            row(3, "app", "stopped", Some("stopped.example.com")),
            row(4, "app", "running", None),
            row(5, "app", "failed", Some("broken.example.com")),
        ],
        "",
    );
    let active: Vec<(i64, bool)> = checks.iter().map(|c| (c.id, c.active)).collect();
    assert_eq!(
        active,
        vec![(1, false), (2, true), (3, false), (4, false), (5, true)],
        "a failed app is probed (it is down), a stopped one is not"
    );
    assert_eq!(checks[0].paused_reason, Some("No dashboard domain is set"));
    assert_eq!(checks[1].spec.host, "web.example.com");
    assert_eq!(checks[1].app_id.as_deref(), Some("app-2"));

    let with_dash = build_checks(vec![row(1, "dashboard", "", None)], "dash.example.com");
    assert!(with_dash[0].active);
    assert_eq!(with_dash[0].name, "Dashboard");

    let mut disabled = row(2, "app", "running", Some("web.example.com"));
    disabled.check.enabled = false;
    disabled.check.interval_secs = 5;
    let c = &build_checks(vec![disabled], "")[0];
    assert!(!c.active);
    assert_eq!(c.interval_secs, MIN_INTERVAL_SECS, "clamped");
}

#[test]
fn due_times_sit_on_a_shared_minute_grid() {
    assert_eq!(next_grid(0), GRID_OFFSET);
    assert_eq!(next_grid(GRID_OFFSET), GRID_OFFSET);
    assert_eq!(next_grid(GRID_OFFSET + 1), 60 + GRID_OFFSET);
    assert_eq!(next_grid(125), 120 + GRID_OFFSET);
    // Checks with different intervals stay on the grid, so they share wakeups.
    let d = next_grid(1000);
    assert_eq!(advance(d, 60, d + 5) % 60, GRID_OFFSET);
    assert_eq!(advance(d, 300, d + 5), d + 300);
    // A round that overran skips to the next grid point, never the past.
    assert_eq!(advance(d, 60, d + 130), next_grid(d + 130));
}

fn result(ok: bool) -> ProbeResult {
    ProbeResult {
        ok,
        cause: (!ok).then_some(Cause::Http5xx),
        detail: if ok {
            "HTTP 200".into()
        } else {
            "HTTP 502".into()
        },
        status: Some(if ok { 200 } else { 502 }),
        latency_ms: 42,
        ip: None,
        dns: DnsState::Overridden,
        cert_expires_at: None,
    }
}

#[tokio::test]
async fn failures_open_an_incident_and_recovery_closes_it_with_events() {
    let state = test_state().await;
    sqlx::query(
        "INSERT INTO apps (id, name, domain, created_at, updated_at, status)
         VALUES ('app-x', 'shop', 'shop.example.com', '2026-01-01', '2026-01-01', 'running')",
    )
    .execute(&state.db)
    .await
    .unwrap_or_else(|e| panic!("app: {e}"));
    let checks = load(&state).await.unwrap_or_default();
    let check = checks
        .into_iter()
        .find(|c| c.app_id.as_deref() == Some("app-x"))
        .unwrap_or_else(|| panic!("the app got a check"));
    let mut events = subscribe();

    record(&state, 6000, &[(check.clone(), result(false))])
        .await
        .unwrap_or_else(|e| panic!("record: {e}"));
    let down = loop {
        let e = events.recv().await.unwrap_or_else(|e| panic!("event: {e}"));
        if e.check_id == check.id {
            break e;
        }
    };
    assert_eq!(down.kind, EventKind::Down);
    assert_eq!(down.cause, Cause::Http5xx);
    assert_eq!(down.app_id.as_deref(), Some("app-x"));
    let status = live_status(check.id);
    assert_eq!(status.down_since, Some(6000));
    assert_eq!(status.last.map(|l| l.result.status), Some(Some(502)));

    // Still down: no second incident.
    record(&state, 6060, &[(check.clone(), result(false))])
        .await
        .unwrap_or_else(|e| panic!("record: {e}"));
    record(&state, 6180, &[(check.clone(), result(true))])
        .await
        .unwrap_or_else(|e| panic!("record: {e}"));
    let up = loop {
        let e = events.recv().await.unwrap_or_else(|e| panic!("event: {e}"));
        if e.check_id == check.id {
            break e;
        }
    };
    assert_eq!((up.kind, up.down_secs), (EventKind::Up, Some(180)));
    assert_eq!(up.incident_id, down.incident_id);
    let incidents = store::incidents(&state.db, check.id, 10)
        .await
        .unwrap_or_default();
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].ended_at, Some(6180));
    assert_eq!(incidents[0].cause, "http_5xx");
    let probes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uptime_1m WHERE check_id = ?")
        .bind(check.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_default();
    assert_eq!(probes, 3);

    // Down again, then the app is stopped: the incident closes.
    record(&state, 6240, &[(check.clone(), result(false))])
        .await
        .unwrap_or_else(|e| panic!("record: {e}"));
    let paused = Check {
        active: false,
        paused_reason: Some("The app is stopped"),
        ..check.clone()
    };
    close_paused(&state, &[paused], 6300)
        .await
        .unwrap_or_else(|e| panic!("close: {e}"));
    assert!(
        store::open_incidents(&state.db)
            .await
            .unwrap_or_default()
            .is_empty()
    );
    assert_eq!(live_status(check.id).down_since, None);
}
