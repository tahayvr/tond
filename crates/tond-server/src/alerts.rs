// alerts.rs — Background alert checker + event firing helper
//
// Threshold-based checks (run every 60 s via run_alert_checker):
//   - "cpu"        → fires if cpu_percent > threshold (app-scoped or MAX across all apps)
//   - "memory"     → fires if (mem_used/mem_limit)*100 > threshold (app-scoped only)
//   - "app_down"   → fires if app non-running for >= threshold minutes (specific app only)
//   - "disk_usage" → fires if host disk_percent > threshold (global; app_id ignored)
//
// Event-based firing (called by deploy/cron code on completion):
//   fire_event_alerts(state, event_slug, app_id?, app_name?, detail?)
//     Finds all enabled rules subscribed to that event slug and fires their webhooks
//     and any linked named integrations.
//
// When a rule fires, this module POSTs a JSON payload to the rule's webhook_url
// and dispatches to all linked named integrations, then updates last_fired_at.
// A rule will not re-fire within 5 minutes of its last firing (threshold-based
// only; event-based rules always fire).

use crate::{notifications, state::AppState};
use tond_common::{AlertRule, IntegrationConfig};

const CHECK_INTERVAL_SECS: u64 = 60;
/// Minimum gap between consecutive firings of the same threshold-based rule (seconds).
const MIN_REFIRE_SECS: i64 = 300;

/// Parse a datetime string that may be either RFC 3339
/// ("2026-03-12T13:43:39+00:00") or SQLite's local format
/// ("2026-03-12 13:43:39").  Returns a UTC DateTime on success.
fn parse_sqlite_dt(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(naive.and_utc());
    }
    None
}

// ── Threshold-based background checker ──────────────────────────────────────

pub async fn run_alert_checker(state: AppState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;

        if let Err(e) = check_all_rules(&state).await {
            tracing::warn!("Alert checker error: {}", e);
        }
    }
}

async fn check_all_rules(state: &AppState) -> anyhow::Result<()> {
    // Only fetch rules that have a metric set (event-only rules skip this path)
    let rules = sqlx::query_as::<_, AlertRule>(
        "SELECT * FROM alert_rules WHERE enabled = 1 AND metric IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;

    for rule in rules {
        // Respect minimum re-fire interval
        if let Some(last) = &rule.last_fired_at
            && let Some(last_dt) = parse_sqlite_dt(last)
        {
            let elapsed = chrono::Utc::now()
                .signed_duration_since(last_dt)
                .num_seconds();
            if elapsed < MIN_REFIRE_SECS {
                continue;
            }
        }

        let metric = match &rule.metric {
            Some(m) => m.as_str(),
            None => continue,
        };

        let result = match metric {
            "cpu" => check_cpu_rule(state, &rule).await.unwrap_or(None),
            "memory" => check_memory_rule(state, &rule).await.unwrap_or(None),
            "app_down" => check_app_down_rule(state, &rule).await.unwrap_or(None),
            "disk_usage" => check_disk_usage_rule(state, &rule).await.unwrap_or(None),
            _ => None,
        };

        if let Some(value) = result
            && let Err(e) = fire_threshold_alert(state, &rule, value).await
        {
            tracing::warn!("Failed to fire alert {}: {}", rule.id, e);
        }
    }

    Ok(())
}

async fn check_cpu_rule(state: &AppState, rule: &AlertRule) -> anyhow::Result<Option<f64>> {
    let row: Option<(f64,)> = if let Some(app_id) = &rule.app_id {
        sqlx::query_as(
            "SELECT cpu_percent FROM metrics WHERE app_id = ?
             ORDER BY collected_at DESC LIMIT 1",
        )
        .bind(app_id)
        .fetch_optional(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT MAX(cpu_percent) FROM metrics
             WHERE collected_at >= datetime('now', '-2 minutes')",
        )
        .fetch_optional(&state.db)
        .await?
    };

    Ok(row.and_then(|(v,)| if v > rule.threshold { Some(v) } else { None }))
}

async fn check_memory_rule(state: &AppState, rule: &AlertRule) -> anyhow::Result<Option<f64>> {
    let row: Option<(f64, f64)> = if let Some(app_id) = &rule.app_id {
        sqlx::query_as(
            "SELECT CAST(mem_used AS REAL), CAST(mem_limit AS REAL)
             FROM metrics WHERE app_id = ?
             ORDER BY collected_at DESC LIMIT 1",
        )
        .bind(app_id)
        .fetch_optional(&state.db)
        .await?
    } else {
        None
    };

    if let Some((used, limit)) = row
        && limit > 0.0
    {
        let pct = (used / limit) * 100.0;
        if pct > rule.threshold {
            return Ok(Some(pct));
        }
    }
    Ok(None)
}

async fn check_app_down_rule(state: &AppState, rule: &AlertRule) -> anyhow::Result<Option<f64>> {
    let app_id = match &rule.app_id {
        Some(id) => id.clone(),
        None => return Ok(None), // app_down rule must target a specific app
    };

    let row: Option<(String, String)> =
        sqlx::query_as("SELECT status, updated_at FROM apps WHERE id = ?")
            .bind(&app_id)
            .fetch_optional(&state.db)
            .await?;

    if let Some((status, updated_at)) = row
        && status != "running"
        && let Some(dt) = parse_sqlite_dt(&updated_at)
    {
        let mins_down = chrono::Utc::now().signed_duration_since(dt).num_minutes() as f64;
        if mins_down >= rule.threshold {
            return Ok(Some(mins_down));
        }
    }

    Ok(None)
}

async fn check_disk_usage_rule(_state: &AppState, rule: &AlertRule) -> anyhow::Result<Option<f64>> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();

    // Sum across all disks
    let (used, total) = disks
        .list()
        .iter()
        .fold((0u64, 0u64), |(u, t), d: &sysinfo::Disk| {
            (
                u + (d.total_space() - d.available_space()),
                t + d.total_space(),
            )
        });

    if total == 0 {
        return Ok(None);
    }

    let pct = (used as f64 / total as f64) * 100.0;
    if pct > rule.threshold {
        Ok(Some(pct))
    } else {
        Ok(None)
    }
}

async fn fire_threshold_alert(
    state: &AppState,
    rule: &AlertRule,
    value: f64,
) -> anyhow::Result<()> {
    let metric = rule.metric.as_deref().unwrap_or("unknown");

    tracing::info!(
        "Firing threshold alert '{}' (rule {}) metric={} → {}",
        rule.name,
        rule.id,
        metric,
        rule.webhook_url
    );

    let message = match metric {
        "cpu" => format!(
            "CPU usage {:.1}% exceeded threshold {:.1}%",
            value, rule.threshold
        ),
        "memory" => format!(
            "Memory usage {:.1}% exceeded threshold {:.1}%",
            value, rule.threshold
        ),
        "disk_usage" => format!(
            "Disk usage {:.1}% exceeded threshold {:.1}%",
            value, rule.threshold
        ),
        "app_down" => format!(
            "App has been down for {:.0} minutes (threshold: {:.0} min)",
            value, rule.threshold
        ),
        _ => format!(
            "Alert fired: value {:.2} exceeded threshold {:.2}",
            value, rule.threshold
        ),
    };

    post_webhook(rule, metric, value, rule.threshold, &message).await;

    let payload = notifications::NotificationPayload {
        rule_name: rule.name.clone(),
        event_or_metric: metric.to_string(),
        app_name: None,
        detail: Some(message),
        fired_at: chrono::Utc::now().to_rfc3339(),
    };
    dispatch_integrations(state, &rule.id, &payload).await;

    update_last_fired(state, &rule.id).await?;
    Ok(())
}

// ── Event-based firing (called externally) ──────────────────────────────────

/// Called by deployment and cron code when an event occurs.
///
/// `event`    — one of VALID_ALERT_EVENTS (e.g. "deployment_success")
/// `app_id`   — the app or cron job involved (for context in the payload; may be None)
/// `app_name` — human-readable name (for the webhook message)
/// `detail`   — optional extra context string
pub async fn fire_event_alerts(
    state: &AppState,
    event: &str,
    _app_id: Option<&str>,
    app_name: Option<&str>,
    detail: Option<&str>,
) {
    // Find all enabled rules that subscribe to this event.
    // We load all enabled rules and filter in memory to avoid complex JSON SQL.
    let rules: Vec<AlertRule> =
        match sqlx::query_as("SELECT * FROM alert_rules WHERE enabled = 1 AND events IS NOT NULL")
            .fetch_all(&state.db)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("fire_event_alerts: DB error: {}", e);
                return;
            }
        };

    for rule in rules {
        let subscribed = rule.parse_events();
        if !subscribed.iter().any(|e| e == event) {
            continue;
        }

        let message = build_event_message(event, app_name, detail);
        post_webhook(&rule, event, 0.0, 0.0, &message).await;

        let payload = notifications::NotificationPayload {
            rule_name: rule.name.clone(),
            event_or_metric: event.to_string(),
            app_name: app_name.map(|s| s.to_string()),
            detail: detail.map(|s| s.to_string()),
            fired_at: chrono::Utc::now().to_rfc3339(),
        };
        dispatch_integrations(state, &rule.id, &payload).await;

        if let Err(e) = update_last_fired(state, &rule.id).await {
            tracing::warn!("fire_event_alerts: failed to update last_fired_at: {}", e);
        }
    }
}

fn build_event_message(event: &str, app_name: Option<&str>, detail: Option<&str>) -> String {
    let name = app_name.unwrap_or("unknown");
    let base = match event {
        "deployment_success" => format!("Deployment succeeded for '{}'", name),
        "deployment_failure" => format!("Deployment failed for '{}'", name),
        "cron_success" => format!("Scheduled task '{}' completed successfully", name),
        "cron_failure" => format!("Scheduled task '{}' failed", name),
        _ => format!("Event '{}' occurred for '{}'", event, name),
    };
    match detail {
        Some(d) if !d.is_empty() => format!("{}: {}", base, d),
        _ => base,
    }
}

// ── Shared webhook helpers ───────────────────────────────────────────────────

async fn post_webhook(rule: &AlertRule, event: &str, value: f64, threshold: f64, message: &str) {
    if rule.webhook_url.is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "rule_id":   rule.id,
        "alert":     rule.name,
        "event":     event,
        "value":     value,
        "threshold": threshold,
        "app_id":    rule.app_id,
        "message":   message,
        "fired_at":  chrono::Utc::now().to_rfc3339(),
    });

    let client = reqwest::Client::new();
    let _ = client
        .post(&rule.webhook_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
}

async fn update_last_fired(state: &AppState, rule_id: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE alert_rules SET last_fired_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(rule_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

// ── Named integration dispatch ───────────────────────────────────────────────

/// Look up all enabled integrations linked to a rule and dispatch the payload
/// to each one.  Errors are logged but do not propagate (best-effort).
async fn dispatch_integrations(
    state: &AppState,
    rule_id: &str,
    payload: &notifications::NotificationPayload,
) {
    // Fetch linked integrations that are enabled
    let rows: Vec<(String, String)> = match sqlx::query_as(
        "SELECT i.kind, i.config
         FROM alert_rule_notifications arn
         JOIN integrations i ON i.id = arn.integration_id
         WHERE arn.rule_id = ? AND i.enabled = 1",
    )
    .bind(rule_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("dispatch_integrations: DB error: {}", e);
            return;
        }
    };

    for (kind, config_json) in rows {
        let config = match IntegrationConfig::parse(&kind, &config_json) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "dispatch_integrations: failed to parse config for kind={}: {}",
                    kind,
                    e
                );
                continue;
            }
        };
        if let Err(e) = notifications::dispatch(&config, payload).await {
            tracing::warn!("dispatch_integrations: failed to dispatch ({kind}): {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{AlertRule, App, ServerSettings};

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for alerts tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_app(state: &AppState, id: &str, name: &str, status: &str, updated_at: &str) {
        let mut app = App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        app.id = id.to_string();
        app.status = status.to_string();
        app.updated_at = updated_at.to_string();
        app.created_at = updated_at.to_string();

        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, host_port, memory_limit, cpu_limit, status, env_vars, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, deploy_key_pub, project_id, registry_credential_id, github_app_id, gitlab_token_id, preview_enabled, preview_excluded_keys, preview_env_vars, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.domain)
        .bind(&app.source_type)
        .bind(&app.source_url)
        .bind(&app.branch)
        .bind(app.container_port)
        .bind(&app.container_id)
        .bind(app.host_port)
        .bind(app.memory_limit)
        .bind(app.cpu_limit)
        .bind(&app.status)
        .bind(&app.env_vars)
        .bind(&app.health_check_path)
        .bind(app.health_check_interval)
        .bind(app.health_failure_threshold)
        .bind(app.health_consecutive_failures)
        .bind(&app.volumes)
        .bind(app.drain_timeout)
        .bind(&app.build_method)
        .bind(&app.webhook_secret)
        .bind(app.auto_deploy)
        .bind(&app.watch_branch)
        .bind(&app.deploy_key_pub)
        .bind(&app.project_id)
        .bind(&app.registry_credential_id)
        .bind(&app.github_app_id)
        .bind(&app.gitlab_token_id)
        .bind(app.preview_enabled)
        .bind(&app.preview_excluded_keys)
        .bind(&app.preview_env_vars)
        .bind(&app.dockerfile_content)
        .bind(&app.compose_content)
        .bind(&app.base_dir)
        .bind(app.static_build)
        .bind(&app.created_at)
        .bind(&app.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app {name}: {err}"));
    }

    async fn seed_alert_rule(state: &AppState, rule: TestAlertRule<'_>) {
        sqlx::query(
            "INSERT INTO alert_rules (id, app_id, name, metric, threshold, webhook_url, enabled, events, last_fired_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind(rule.id)
        .bind(rule.app_id)
        .bind(rule.name)
        .bind(rule.metric)
        .bind(rule.threshold)
        .bind("")
        .bind(rule.events)
        .bind(rule.last_fired_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed alert rule {}: {err}", rule.name));
    }

    async fn load_rule(state: &AppState, id: &str) -> AlertRule {
        sqlx::query_as::<_, AlertRule>("SELECT * FROM alert_rules WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to load alert rule {id}: {err}"))
    }

    struct TestAlertRule<'a> {
        id: &'a str,
        name: &'a str,
        app_id: Option<&'a str>,
        metric: Option<&'a str>,
        threshold: f64,
        events: Option<&'a str>,
        last_fired_at: Option<&'a str>,
    }

    #[test]
    fn parse_sqlite_dt_accepts_rfc3339_and_sqlite_formats() {
        assert!(
            parse_sqlite_dt("2026-03-12T13:43:39+00:00").is_some(),
            "parse_sqlite_dt should parse RFC3339 timestamps"
        );
        assert!(
            parse_sqlite_dt("2026-03-12 13:43:39").is_some(),
            "parse_sqlite_dt should parse SQLite datetime strings without fractional seconds"
        );
        assert!(
            parse_sqlite_dt("2026-03-12 13:43:39.123").is_some(),
            "parse_sqlite_dt should parse SQLite datetime strings with fractional seconds"
        );
        assert_eq!(
            parse_sqlite_dt("not-a-date"),
            None,
            "parse_sqlite_dt should return None for invalid datetime strings"
        );
    }

    #[test]
    fn build_event_message_formats_known_and_unknown_events() {
        assert_eq!(
            build_event_message("deployment_success", Some("frontend"), Some("sha abc123")),
            "Deployment succeeded for 'frontend': sha abc123",
            "build_event_message should use the deployment success template and append detail"
        );
        assert_eq!(
            build_event_message("cron_failure", Some("nightly"), None),
            "Scheduled task 'nightly' failed",
            "build_event_message should use the cron failure template when no detail is provided"
        );
        assert_eq!(
            build_event_message("custom_event", None, Some("extra")),
            "Event 'custom_event' occurred for 'unknown': extra",
            "build_event_message should fall back to the generic template for unknown events"
        );
    }

    #[tokio::test]
    async fn check_cpu_rule_uses_latest_app_metric_and_global_max() {
        let state = test_state().await;

        seed_app(
            &state,
            "app-1",
            "frontend",
            "running",
            "2026-01-01 00:00:00",
        )
        .await;

        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-app",
                name: "High app CPU",
                app_id: Some("app-1"),
                metric: Some("cpu"),
                threshold: 80.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;
        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-global",
                name: "High global CPU",
                app_id: None,
                metric: Some("cpu"),
                threshold: 75.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;

        sqlx::query(
            "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts, collected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("metric-1")
        .bind("app-1")
        .bind(72.0)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind("2026-01-01 00:00:00")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to insert first cpu metric: {err}"));
        sqlx::query(
            "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts, collected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind("metric-2")
        .bind("app-1")
        .bind(88.5)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to insert second cpu metric: {err}"));
        sqlx::query(
            "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts, collected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind("metric-3")
        .bind("app-2")
        .bind(91.0)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to insert global cpu metric: {err}"));

        let app_rule = load_rule(&state, "rule-app").await;
        let global_rule = load_rule(&state, "rule-global").await;

        assert_eq!(
            check_cpu_rule(&state, &app_rule)
                .await
                .unwrap_or_else(|err| panic!("app cpu rule check should succeed: {err}")),
            Some(88.5),
            "check_cpu_rule should use the latest metric sample for app-scoped rules"
        );
        assert_eq!(
            check_cpu_rule(&state, &global_rule)
                .await
                .unwrap_or_else(|err| panic!("global cpu rule check should succeed: {err}")),
            Some(91.0),
            "check_cpu_rule should use the maximum recent CPU sample for global rules"
        );
    }

    #[tokio::test]
    async fn check_memory_rule_returns_percentage_only_when_limit_is_non_zero_and_threshold_is_exceeded()
     {
        let state = test_state().await;

        seed_app(
            &state,
            "app-1",
            "frontend",
            "running",
            "2026-01-01 00:00:00",
        )
        .await;

        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-memory",
                name: "High memory",
                app_id: Some("app-1"),
                metric: Some("memory"),
                threshold: 70.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;
        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-no-app",
                name: "No app memory",
                app_id: None,
                metric: Some("memory"),
                threshold: 70.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;

        sqlx::query(
            "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts, collected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind("metric-memory")
        .bind("app-1")
        .bind(0.0)
        .bind(800_i64)
        .bind(1000_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to insert memory metric: {err}"));
        sqlx::query(
            "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts, collected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind("metric-memory-zero-limit")
        .bind("app-2")
        .bind(0.0)
        .bind(999_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to insert zero-limit memory metric: {err}"));

        let memory_rule = load_rule(&state, "rule-memory").await;
        let no_app_rule = load_rule(&state, "rule-no-app").await;

        assert_eq!(
            check_memory_rule(&state, &memory_rule)
                .await
                .unwrap_or_else(|err| panic!("memory rule check should succeed: {err}")),
            Some(80.0),
            "check_memory_rule should return used/limit percentage when the threshold is exceeded"
        );
        assert_eq!(
            check_memory_rule(&state, &no_app_rule)
                .await
                .unwrap_or_else(|err| panic!("global memory rule check should succeed: {err}")),
            None,
            "check_memory_rule should ignore rules that are not scoped to a specific app"
        );
    }

    #[tokio::test]
    async fn check_app_down_rule_only_fires_for_non_running_apps_past_threshold() {
        let state = test_state().await;

        seed_app(
            &state,
            "app-down",
            "frontend",
            "failed",
            "2000-01-01 00:00:00",
        )
        .await;
        seed_app(&state, "app-ok", "api", "running", "2000-01-01 00:00:00").await;

        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-down",
                name: "App down",
                app_id: Some("app-down"),
                metric: Some("app_down"),
                threshold: 5.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;
        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-running",
                name: "App running",
                app_id: Some("app-ok"),
                metric: Some("app_down"),
                threshold: 5.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;
        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-global",
                name: "Global app down",
                app_id: None,
                metric: Some("app_down"),
                threshold: 5.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;

        let down_rule = load_rule(&state, "rule-down").await;
        let running_rule = load_rule(&state, "rule-running").await;
        let global_rule = load_rule(&state, "rule-global").await;

        assert!(
            check_app_down_rule(&state, &down_rule)
                .await
                .unwrap_or_else(|err| panic!("app_down rule check should succeed: {err}"))
                .is_some(),
            "check_app_down_rule should fire when the target app has been non-running longer than the threshold"
        );
        assert_eq!(
            check_app_down_rule(&state, &running_rule)
                .await
                .unwrap_or_else(|err| panic!("running app_down rule check should succeed: {err}")),
            None,
            "check_app_down_rule should not fire for running apps"
        );
        assert_eq!(
            check_app_down_rule(&state, &global_rule)
                .await
                .unwrap_or_else(|err| panic!("global app_down rule check should succeed: {err}")),
            None,
            "check_app_down_rule should ignore rules that are not scoped to a specific app"
        );
    }

    #[tokio::test]
    async fn fire_event_alerts_updates_only_rules_subscribed_to_the_given_event() {
        let state = test_state().await;

        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-match",
                name: "Deploy success",
                app_id: None,
                metric: Some("cpu"),
                threshold: 90.0,
                events: Some(r#"["deployment_success"]"#),
                last_fired_at: None,
            },
        )
        .await;
        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-other-event",
                name: "Cron only",
                app_id: None,
                metric: Some("cpu"),
                threshold: 90.0,
                events: Some(r#"["cron_failure"]"#),
                last_fired_at: None,
            },
        )
        .await;
        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-no-events",
                name: "No events",
                app_id: None,
                metric: Some("cpu"),
                threshold: 90.0,
                events: None,
                last_fired_at: None,
            },
        )
        .await;

        fire_event_alerts(
            &state,
            "deployment_success",
            Some("app-1"),
            Some("frontend"),
            Some("sha abc123"),
        )
        .await;

        let matched_last_fired = load_rule(&state, "rule-match").await.last_fired_at;
        let other_event_last_fired = load_rule(&state, "rule-other-event").await.last_fired_at;
        let no_events_last_fired = load_rule(&state, "rule-no-events").await.last_fired_at;

        assert!(
            matched_last_fired.is_some(),
            "fire_event_alerts should update last_fired_at for rules subscribed to the matching event"
        );
        assert_eq!(
            other_event_last_fired, None,
            "fire_event_alerts should not update rules subscribed only to other events"
        );
        assert_eq!(
            no_events_last_fired, None,
            "fire_event_alerts should not update rules without event subscriptions"
        );
    }

    #[tokio::test]
    async fn check_all_rules_respects_min_refire_interval_for_threshold_rules() {
        let state = test_state().await;

        seed_app(
            &state,
            "app-1",
            "frontend",
            "running",
            "2026-01-01 00:00:00",
        )
        .await;

        seed_alert_rule(
            &state,
            TestAlertRule {
                id: "rule-recent",
                name: "Recently fired CPU",
                app_id: Some("app-1"),
                metric: Some("cpu"),
                threshold: 80.0,
                events: None,
                last_fired_at: Some(&chrono::Utc::now().to_rfc3339()),
            },
        )
        .await;
        sqlx::query(
            "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts, collected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind("metric-recent")
        .bind("app-1")
        .bind(99.0)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to insert cpu metric for refire test: {err}"));

        let before = load_rule(&state, "rule-recent")
            .await
            .last_fired_at
            .unwrap_or_else(|| panic!("expected seeded rule to have last_fired_at"));

        check_all_rules(&state)
            .await
            .unwrap_or_else(|err| panic!("check_all_rules should succeed: {err}"));

        let after = load_rule(&state, "rule-recent")
            .await
            .last_fired_at
            .unwrap_or_else(|| panic!("expected rule to keep last_fired_at after check"));

        assert_eq!(
            after, before,
            "check_all_rules should not re-fire threshold alerts within the minimum refire interval"
        );
    }
}
