// health.rs — background health check poller
//
// run_health_checker(state) is spawned once at startup. It sleeps until the
// next app's health check is due (checks due within CHECK_SLACK of each other
// share one wakeup) or until the apps table changes; with no health-checked
// app it sleeps for up to IDLE_SLEEP. It used to wake every second.
//
// For each running app with a health_check_path set:
//   1. HTTP GET http://127.0.0.1:{host_port}{health_check_path} (5 s timeout)
//   2. Success (2xx): reset health_consecutive_failures = 0
//   3. Failure (non-2xx or connection error):
//        - Increment health_consecutive_failures
//        - If >= health_failure_threshold:
//            a. Set status = 'unhealthy'
//            b. Docker stop + start the container
//            c. Reset health_consecutive_failures = 0
//            d. Set status = 'running'
//
// The last_health_check timestamp is tracked in memory (HashMap keyed by
// app_id) to avoid storing it in the DB and to reset cleanly on restart.

use crate::runtime::changes::{Table, Watch};
use crate::runtime::loops;
use crate::state::AppState;
use crate::{docker::start_container, docker::stop_container};
use std::collections::HashMap;
use tokio::time::Duration;
use tond_common::App;

/// Checks due within this window of each other run in the same wakeup.
const CHECK_SLACK: Duration = Duration::from_secs(5);
/// Longest sleep when no app has a health check (a change to apps wakes us).
const IDLE_SLEEP: Duration = Duration::from_secs(3600);
/// Never re-query more often than this.
const MIN_SLEEP: Duration = Duration::from_secs(1);

/// When the next check is due, given each app's last check and interval.
fn next_due(
    last_checked: &HashMap<String, tokio::time::Instant>,
    apps: &[(String, i64)],
    now: tokio::time::Instant,
) -> Duration {
    apps.iter()
        .map(|(id, interval)| {
            let interval = Duration::from_secs((*interval).max(1) as u64);
            last_checked.get(id).map_or(Duration::ZERO, |last| {
                (*last + interval).saturating_duration_since(now)
            })
        })
        .min()
        .map_or(IDLE_SLEEP, |d| d.max(MIN_SLEEP))
}

#[derive(Debug, PartialEq, Eq)]
enum HealthUpdate {
    None,
    ResetFailures,
    IncrementFailures {
        new_failures: i64,
    },
    MarkUnhealthy {
        new_failures: i64,
        should_restart: bool,
    },
}

fn should_run_check(
    last_checked: Option<tokio::time::Instant>,
    now: tokio::time::Instant,
    interval_secs: i64,
) -> bool {
    let interval = Duration::from_secs(interval_secs.max(0) as u64);
    match last_checked {
        Some(last) => now.duration_since(last) >= interval,
        None => true,
    }
}

fn health_check_url(host_port: Option<i64>, path: Option<&str>) -> Option<String> {
    Some(format!("http://127.0.0.1:{}{}", host_port? as u16, path?))
}

fn evaluate_health_result(
    check_ok: bool,
    consecutive_failures: i64,
    health_failure_threshold: i64,
    has_container_id: bool,
) -> HealthUpdate {
    if check_ok {
        return if consecutive_failures > 0 {
            HealthUpdate::ResetFailures
        } else {
            HealthUpdate::None
        };
    }

    let new_failures = consecutive_failures + 1;
    if new_failures >= health_failure_threshold {
        HealthUpdate::MarkUnhealthy {
            new_failures,
            should_restart: has_container_id,
        }
    } else {
        HealthUpdate::IncrementFailures { new_failures }
    }
}

pub async fn run_health_checker(state: AppState) {
    // Track when we last checked each app (app_id -> last check Instant)
    let mut last_checked: HashMap<String, tokio::time::Instant> = HashMap::new();

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to build reqwest client for health checker");

    let lp = loops::register("health");
    let mut watch = Watch::new(&[Table::Apps]);
    let mut sleep_for = Duration::ZERO;

    loop {
        lp.wait(Some(sleep_for), &mut watch).await;
        let _run = lp.run();

        // Fetch all running apps that have a health check path configured
        let apps: Vec<App> = match sqlx::query_as::<_, App>(
            "SELECT * FROM apps WHERE status IN ('running', 'unhealthy') AND health_check_path IS NOT NULL AND host_port IS NOT NULL",
        )
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("Health checker: failed to query apps: {}", e);
                sleep_for = Duration::from_secs(30);
                continue;
            }
        };

        let now = tokio::time::Instant::now();
        let schedule: Vec<(String, i64)> = apps
            .iter()
            .map(|a| (a.id.clone(), a.health_check_interval))
            .collect();
        // Forget apps that no longer have a health check.
        last_checked.retain(|id, _| schedule.iter().any(|(s, _)| s == id));

        for app in apps {
            let last = last_checked.get(&app.id).copied();

            // Skip if the interval hasn't elapsed yet (checks due within
            // CHECK_SLACK run now, sharing this wakeup)
            if !should_run_check(last, now + CHECK_SLACK, app.health_check_interval) {
                continue;
            }

            last_checked.insert(app.id.clone(), now);

            let Some(url) = health_check_url(app.host_port, app.health_check_path.as_deref())
            else {
                continue;
            };

            let check_ok = match http.get(&url).send().await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => false,
            };

            match evaluate_health_result(
                check_ok,
                app.health_consecutive_failures,
                app.health_failure_threshold,
                app.container_id.is_some(),
            ) {
                HealthUpdate::None => {}
                HealthUpdate::ResetFailures => {
                    let _ = sqlx::query(
                        "UPDATE apps SET health_consecutive_failures = 0, updated_at = datetime('now') WHERE id = ?",
                    )
                    .bind(&app.id)
                    .execute(&state.db)
                    .await;
                    tracing::info!("Health check recovered for app {}", app.name);
                }
                HealthUpdate::IncrementFailures { new_failures } => {
                    tracing::warn!(
                        "Health check failed for app {} ({}/{} failures)",
                        app.name,
                        new_failures,
                        app.health_failure_threshold
                    );

                    let _ = sqlx::query(
                        "UPDATE apps SET health_consecutive_failures = ?, updated_at = datetime('now') WHERE id = ?",
                    )
                    .bind(new_failures)
                    .bind(&app.id)
                    .execute(&state.db)
                    .await;
                }
                HealthUpdate::MarkUnhealthy {
                    new_failures,
                    should_restart,
                } => {
                    tracing::warn!(
                        "Health check failed for app {} ({}/{} failures)",
                        app.name,
                        new_failures,
                        app.health_failure_threshold
                    );

                    let _ = sqlx::query(
                        "UPDATE apps SET status = 'unhealthy', health_consecutive_failures = ?, updated_at = datetime('now') WHERE id = ?",
                    )
                    .bind(new_failures)
                    .bind(&app.id)
                    .execute(&state.db)
                    .await;

                    if should_restart {
                        tracing::warn!(
                            "App {} marked unhealthy after {} consecutive failures — restarting",
                            app.name,
                            new_failures
                        );

                        if let Some(ref container_id) = app.container_id {
                            let _ = stop_container(&state.docker, container_id).await;

                            match start_container(&state.docker, container_id).await {
                                Ok(()) => {
                                    let _ = sqlx::query(
                                    "UPDATE apps SET status = 'running', health_consecutive_failures = 0, updated_at = datetime('now') WHERE id = ?",
                                )
                                .bind(&app.id)
                                .execute(&state.db)
                                .await;
                                    // Reset the interval timer so we don't immediately re-check
                                    last_checked
                                        .insert(app.id.clone(), tokio::time::Instant::now());
                                    tracing::info!(
                                        "App {} restarted after health check failure",
                                        app.name
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to restart app {} after health failure: {}",
                                        app.name,
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        sleep_for = next_due(&last_checked, &schedule, tokio::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{App, ServerSettings};

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for health tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_health_app(
        state: &AppState,
        id: &str,
        name: &str,
        status: &str,
        host_port: Option<i64>,
        health_check_path: Option<&str>,
        container_id: Option<&str>,
    ) {
        let mut app = App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        app.id = id.to_string();
        app.status = status.to_string();
        app.host_port = host_port;
        app.health_check_path = health_check_path.map(ToString::to_string);
        app.container_id = container_id.map(ToString::to_string);

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
        .unwrap_or_else(|err| panic!("failed to seed health app {name}: {err}"));
    }

    #[test]
    fn next_due_sleeps_until_the_earliest_check_and_long_when_idle() {
        let now = tokio::time::Instant::now();
        let mut last = HashMap::new();
        assert_eq!(next_due(&last, &[], now), IDLE_SLEEP, "nothing to check");
        assert_eq!(
            next_due(&last, &[("a".into(), 30)], now),
            MIN_SLEEP,
            "never-checked apps are due now (but never busy-loop)"
        );
        last.insert("a".to_string(), now);
        last.insert("b".to_string(), now - Duration::from_secs(50));
        assert_eq!(
            next_due(&last, &[("a".into(), 30), ("b".into(), 60)], now),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn should_run_check_respects_interval_and_runs_when_missing_previous_timestamp() {
        let now = tokio::time::Instant::now();

        assert!(
            should_run_check(None, now, 30),
            "health checks should run immediately when there is no previous timestamp"
        );
        assert!(
            !should_run_check(Some(now), now, 30),
            "health checks should not run again before the interval has elapsed"
        );
        assert!(
            should_run_check(Some(now - Duration::from_secs(31)), now, 30),
            "health checks should run once the interval has elapsed"
        );
    }

    #[test]
    fn health_check_url_builds_local_probe_url_only_when_all_parts_exist() {
        assert_eq!(
            health_check_url(Some(3000), Some("/health")),
            Some("http://127.0.0.1:3000/health".to_string()),
            "health_check_url should build the localhost probe URL from host port and path"
        );
        assert_eq!(
            health_check_url(None, Some("/health")),
            None,
            "health_check_url should return None when the host port is missing"
        );
        assert_eq!(
            health_check_url(Some(3000), None),
            None,
            "health_check_url should return None when the health check path is missing"
        );
    }

    #[test]
    fn evaluate_health_result_distinguishes_recovery_increment_and_restart_paths() {
        assert_eq!(
            evaluate_health_result(true, 0, 3, true),
            HealthUpdate::None,
            "successful checks should do nothing when there are no prior failures"
        );
        assert_eq!(
            evaluate_health_result(true, 2, 3, true),
            HealthUpdate::ResetFailures,
            "successful checks should reset the failure counter when recovering from prior failures"
        );
        assert_eq!(
            evaluate_health_result(false, 1, 3, true),
            HealthUpdate::IncrementFailures { new_failures: 2 },
            "failing checks below threshold should increment the failure counter"
        );
        assert_eq!(
            evaluate_health_result(false, 2, 3, true),
            HealthUpdate::MarkUnhealthy {
                new_failures: 3,
                should_restart: true,
            },
            "failing checks at threshold should mark the app unhealthy and request a restart when a container id exists"
        );
        assert_eq!(
            evaluate_health_result(false, 2, 3, false),
            HealthUpdate::MarkUnhealthy {
                new_failures: 3,
                should_restart: false,
            },
            "failing checks at threshold should still mark the app unhealthy even when no container can be restarted"
        );
    }

    #[tokio::test]
    async fn health_checker_query_targets_only_running_or_unhealthy_apps_with_probe_configuration()
    {
        let state = test_state().await;

        seed_health_app(
            &state,
            "app-running",
            "frontend",
            "running",
            Some(3000),
            Some("/health"),
            Some("container-1"),
        )
        .await;
        seed_health_app(
            &state,
            "app-unhealthy",
            "api",
            "unhealthy",
            Some(3001),
            Some("/healthz"),
            Some("container-2"),
        )
        .await;
        seed_health_app(
            &state,
            "app-stopped",
            "worker",
            "stopped",
            Some(3002),
            Some("/health"),
            Some("container-3"),
        )
        .await;
        seed_health_app(
            &state,
            "app-no-port",
            "db",
            "running",
            None,
            Some("/health"),
            Some("container-4"),
        )
        .await;
        seed_health_app(
            &state,
            "app-no-path",
            "cache",
            "running",
            Some(3004),
            None,
            Some("container-5"),
        )
        .await;

        let selected: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, name FROM apps
             WHERE status IN ('running', 'unhealthy') AND health_check_path IS NOT NULL AND host_port IS NOT NULL
             ORDER BY id",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to query health-checkable apps: {err}"));

        assert_eq!(
            selected,
            vec![
                ("app-running".to_string(), "frontend".to_string()),
                ("app-unhealthy".to_string(), "api".to_string()),
            ],
            "health checker should only poll running or unhealthy apps that have both a probe path and a host port"
        );
    }
}
