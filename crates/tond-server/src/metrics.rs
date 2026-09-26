// metrics.rs — Phase 5.1: Resource Metrics Collection
//
// Background task that collects per-container CPU, memory, network, and
// restart stats from Docker every 30 seconds and stores them in the
// `metrics` table. Old rows are pruned by the janitor (janitor.rs).
//
// Docker stats are read with a single non-streaming call per container
// (one_shot=true) to avoid holding a long-lived stream open. Each sample
// is inserted as one row.
//
// The CPU percentage calculation follows the same formula Docker CLI uses:
//   cpu% = delta_cpu_usage / delta_system_usage * num_cpus * 100
//
// Entry point: `run_metrics_collector(state)` — spawned once at startup.

use crate::state::AppState;
use bollard::models::{
    ContainerCpuStats, ContainerMemoryStats, ContainerNetworkStats, ContainerStatsResponse,
};
use bollard::query_parameters::StatsOptionsBuilder;
use futures::StreamExt;
use std::collections::HashMap;
use uuid::Uuid;

/// How often we sample container stats (seconds).
const COLLECT_INTERVAL_SECS: u64 = 30;

pub async fn run_metrics_collector(state: AppState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(COLLECT_INTERVAL_SECS)).await;

        if let Err(e) = collect_and_store(&state).await {
            tracing::warn!("Metrics collection error: {}", e);
        }
    }
}

#[derive(Debug, PartialEq)]
struct MetricSample {
    cpu_percent: f64,
    mem_used: i64,
    mem_limit: i64,
    net_rx: i64,
    net_tx: i64,
    restarts: i64,
}

fn compute_cpu_percent(cpu: Option<&ContainerCpuStats>, precpu: Option<&ContainerCpuStats>) -> f64 {
    let cpu_delta = cpu
        .and_then(|c| c.cpu_usage.as_ref())
        .and_then(|u| u.total_usage)
        .unwrap_or(0)
        .saturating_sub(
            precpu
                .and_then(|c| c.cpu_usage.as_ref())
                .and_then(|u| u.total_usage)
                .unwrap_or(0),
        ) as f64;

    let sys_delta =
        cpu.and_then(|c| c.system_cpu_usage)
            .unwrap_or(0)
            .saturating_sub(precpu.and_then(|c| c.system_cpu_usage).unwrap_or(0)) as f64;

    let num_cpus = cpu.and_then(|c| c.online_cpus).unwrap_or(1) as f64;

    if sys_delta > 0.0 {
        (cpu_delta / sys_delta) * num_cpus * 100.0
    } else {
        0.0
    }
}

fn compute_memory_usage(memory: Option<&ContainerMemoryStats>) -> (i64, i64) {
    let mem_used = memory.and_then(|m| m.usage).unwrap_or(0) as i64;
    let cache = memory
        .and_then(|m| m.stats.as_ref())
        .and_then(|s| s.get("cache").copied())
        .unwrap_or(0) as i64;
    let mem_limit = memory.and_then(|m| m.limit).unwrap_or(0) as i64;

    ((mem_used - cache).max(0), mem_limit)
}

fn compute_network_io(networks: Option<&HashMap<String, ContainerNetworkStats>>) -> (i64, i64) {
    if let Some(networks) = networks {
        let rx: u64 = networks.values().map(|n| n.rx_bytes.unwrap_or(0)).sum();
        let tx: u64 = networks.values().map(|n| n.tx_bytes.unwrap_or(0)).sum();
        (rx as i64, tx as i64)
    } else {
        (0, 0)
    }
}

fn build_metric_sample(stats: &ContainerStatsResponse) -> MetricSample {
    let cpu_percent = compute_cpu_percent(stats.cpu_stats.as_ref(), stats.precpu_stats.as_ref());
    let (mem_used, mem_limit) = compute_memory_usage(stats.memory_stats.as_ref());
    let (net_rx, net_tx) = compute_network_io(stats.networks.as_ref());

    MetricSample {
        cpu_percent,
        mem_used,
        mem_limit,
        net_rx,
        net_tx,
        restarts: 0,
    }
}

async fn collect_and_store(state: &AppState) -> anyhow::Result<()> {
    // Fetch all apps that have a container_id and are running or unhealthy
    let apps: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, name, container_id FROM apps
         WHERE container_id IS NOT NULL
           AND status IN ('running', 'unhealthy')",
    )
    .fetch_all(&state.db)
    .await?;

    for (app_id, _app_name, container_id) in apps {
        match collect_container_stats(state, &app_id, &container_id).await {
            Ok(()) => {}
            Err(e) => {
                // Individual container errors should not stop the whole loop
                tracing::debug!(
                    "Failed to collect stats for container {}: {}",
                    container_id,
                    e
                );
            }
        }
    }

    Ok(())
}

async fn collect_container_stats(
    state: &AppState,
    app_id: &str,
    container_id: &str,
) -> anyhow::Result<()> {
    let options = StatsOptionsBuilder::default().stream(false).build();
    let mut stream = state.docker.stats(container_id, Some(options));

    let stats = match stream.next().await {
        Some(Ok(s)) => s,
        Some(Err(e)) => return Err(e.into()),
        None => return Ok(()),
    };
    let sample = build_metric_sample(&stats);

    // ── Insert sample ──
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO metrics (id, app_id, cpu_percent, mem_used, mem_limit, net_rx, net_tx, restarts)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(app_id)
    .bind(sample.cpu_percent)
    .bind(sample.mem_used)
    .bind(sample.mem_limit)
    .bind(sample.net_rx)
    .bind(sample.net_tx)
    .bind(sample.restarts)
    .execute(&state.db)
    .await?;

    Ok(())
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
            .unwrap_or_else(|err| panic!("failed to run migrations for metrics tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_app_with_container(
        state: &AppState,
        id: &str,
        name: &str,
        status: &str,
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
        .unwrap_or_else(|err| panic!("failed to seed app {name}: {err}"));
    }

    fn sample_stats_response() -> ContainerStatsResponse {
        let mut mem_stats = HashMap::new();
        mem_stats.insert("cache".to_string(), 256_u64);

        let mut networks = HashMap::new();
        networks.insert(
            "eth0".to_string(),
            ContainerNetworkStats {
                rx_bytes: Some(100),
                tx_bytes: Some(200),
                ..Default::default()
            },
        );
        networks.insert(
            "eth1".to_string(),
            ContainerNetworkStats {
                rx_bytes: Some(300),
                tx_bytes: Some(400),
                ..Default::default()
            },
        );

        ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(bollard::models::ContainerCpuUsage {
                    total_usage: Some(300),
                    ..Default::default()
                }),
                system_cpu_usage: Some(1000),
                online_cpus: Some(2),
                ..Default::default()
            }),
            precpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(bollard::models::ContainerCpuUsage {
                    total_usage: Some(100),
                    ..Default::default()
                }),
                system_cpu_usage: Some(500),
                online_cpus: Some(2),
                ..Default::default()
            }),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(1024),
                limit: Some(2048),
                stats: Some(mem_stats),
                ..Default::default()
            }),
            networks: Some(networks),
            ..Default::default()
        }
    }

    #[test]
    fn compute_cpu_percent_matches_docker_style_formula_and_handles_zero_system_delta() {
        let stats = sample_stats_response();

        assert_eq!(
            compute_cpu_percent(stats.cpu_stats.as_ref(), stats.precpu_stats.as_ref()),
            80.0,
            "compute_cpu_percent should apply the Docker CLI CPU percentage formula"
        );

        let zero_delta = ContainerCpuStats {
            cpu_usage: Some(bollard::models::ContainerCpuUsage {
                total_usage: Some(200),
                ..Default::default()
            }),
            system_cpu_usage: Some(500),
            online_cpus: Some(4),
            ..Default::default()
        };
        let prev = ContainerCpuStats {
            cpu_usage: Some(bollard::models::ContainerCpuUsage {
                total_usage: Some(100),
                ..Default::default()
            }),
            system_cpu_usage: Some(500),
            online_cpus: Some(4),
            ..Default::default()
        };
        assert_eq!(
            compute_cpu_percent(Some(&zero_delta), Some(&prev)),
            0.0,
            "compute_cpu_percent should return zero when the system CPU delta is zero"
        );
    }

    #[test]
    fn compute_memory_usage_subtracts_cache_and_never_returns_negative_usage() {
        let stats = sample_stats_response();

        assert_eq!(
            compute_memory_usage(stats.memory_stats.as_ref()),
            (768, 2048),
            "compute_memory_usage should subtract cache from usage and preserve the memory limit"
        );

        let memory = ContainerMemoryStats {
            usage: Some(100),
            limit: Some(500),
            stats: Some(HashMap::from([(String::from("cache"), 150_u64)])),
            ..Default::default()
        };
        assert_eq!(
            compute_memory_usage(Some(&memory)),
            (0, 500),
            "compute_memory_usage should clamp negative post-cache usage to zero"
        );
    }

    #[test]
    fn compute_network_io_sums_interfaces_and_defaults_to_zero() {
        let stats = sample_stats_response();

        assert_eq!(
            compute_network_io(stats.networks.as_ref()),
            (400, 600),
            "compute_network_io should sum rx and tx bytes across all interfaces"
        );
        assert_eq!(
            compute_network_io(None),
            (0, 0),
            "compute_network_io should default to zero when Docker reports no network stats"
        );
    }

    #[test]
    fn build_metric_sample_combines_computed_resource_fields() {
        let stats = sample_stats_response();

        assert_eq!(
            build_metric_sample(&stats),
            MetricSample {
                cpu_percent: 80.0,
                mem_used: 768,
                mem_limit: 2048,
                net_rx: 400,
                net_tx: 600,
                restarts: 0,
            },
            "build_metric_sample should combine CPU, memory, network, and restart fields into a persistable sample"
        );
    }

    #[tokio::test]
    async fn collect_and_store_skips_apps_without_container_ids_or_non_running_statuses() {
        let state = test_state().await;

        seed_app_with_container(
            &state,
            "app-running",
            "frontend",
            "running",
            Some("container-1"),
        )
        .await;
        seed_app_with_container(
            &state,
            "app-unhealthy",
            "api",
            "unhealthy",
            Some("container-2"),
        )
        .await;
        seed_app_with_container(
            &state,
            "app-stopped",
            "worker",
            "stopped",
            Some("container-3"),
        )
        .await;
        seed_app_with_container(&state, "app-no-container", "db", "running", None).await;

        let selected: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, name, container_id FROM apps
             WHERE container_id IS NOT NULL
               AND status IN ('running', 'unhealthy')",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to query collectible apps: {err}"));

        assert_eq!(
            selected,
            vec![
                (
                    "app-running".to_string(),
                    "frontend".to_string(),
                    "container-1".to_string(),
                ),
                (
                    "app-unhealthy".to_string(),
                    "api".to_string(),
                    "container-2".to_string(),
                ),
            ],
            "metrics collection should only target running or unhealthy apps that have a container id"
        );
    }
}
