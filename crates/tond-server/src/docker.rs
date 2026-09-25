// docker.rs — Docker API abstraction layer
//
// Thin wrappers around the Bollard Docker client. Every function takes a
// &Docker (cheap to clone via Arc internally) and returns anyhow::Result.
// This keeps all Bollard-specific types contained here; callers only see
// plain Rust types (strings, vecs).
//
// Functions:
//
//   run_container(docker, image, name, port, host_port, env_vars, memory_limit, cpu_limit, binds, network_name)
//     Creates and starts a container named `tond-{name}`. Before creating,
//     it force-removes any existing container with the same name (handles
//     re-deploys). The container is configured with:
//       - Port binding: 0.0.0.0:{host_port} -> {port}/tcp
//       - Restart policy: UnlessStopped (survives host reboots)
//       - Label: managed-by=tond (for identification)
//       - Env vars + injected PORT={port} (12-factor convention)
//       - Memory limit: memory_limit bytes (None = no limit)
//       - CPU limit: cpu_limit nano_cpus (None = no limit; 1e9 = 1 CPU)
//       - Bind mounts: binds as Vec<String> in "host_path:container_path" format
//       - network_name: if Some, connect container to that Docker network with
//         the app name as a network alias (so peers can reach it as http://appname:port)
//     Returns the Docker-assigned container ID.
//
//   stop_container(docker, container_id)   — graceful stop, 10 s timeout
//   start_container(docker, container_id)  — start a stopped container
//   remove_container(docker, container_id) — force remove
//
//   get_logs(docker, container_id, tail)
//     Streams stdout+stderr from a running/stopped container, returns the
//     last `tail` lines as Vec<String>.
//
//   find_free_port(docker, start)
//     Lists all containers (including stopped) and collects their bound
//     host ports. Increments from `start` until finding an unused one.
//     Used by deploy::deploy() to avoid port collisions between apps.
//
//   ensure_image(docker, image)
//     Pulls `image` unless it is already present locally. Managed databases
//     and cron jobs start from stock images that a fresh host doesn't have.
//
//   ensure_network(docker, network_name)
//     Creates a Docker bridge network with the given name if it does not
//     already exist. Idempotent — safe to call on every deploy.
//
//   remove_network(docker, network_name)
//     Removes a Docker network by name. Best-effort — logs a warning but
//     does not return an error if the network does not exist.

use bollard::Docker;
use bollard::models::*;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    ListNetworksOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    RenameContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use futures::StreamExt;
use std::collections::HashMap;

fn env_with_port(env_vars: HashMap<String, String>, port: u16) -> Vec<String> {
    let mut env: Vec<String> = env_vars
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    env.push(format!("PORT={}", port));
    env
}

fn make_port_bindings(port: u16, host_port: u16) -> HashMap<String, Option<Vec<PortBinding>>> {
    let port_binding = PortBinding {
        host_ip: Some("0.0.0.0".to_string()),
        host_port: Some(host_port.to_string()),
    };

    let mut port_bindings = HashMap::new();
    port_bindings.insert(format!("{}/tcp", port), Some(vec![port_binding]));
    port_bindings
}

fn next_free_port(start: u16, used_ports: &[u16]) -> anyhow::Result<u16> {
    let mut port = start;
    while used_ports.contains(&port) {
        port += 1;
        if port > 65000 {
            anyhow::bail!("No free ports available");
        }
    }

    Ok(port)
}

fn compose_project_filter(project_name: &str) -> HashMap<String, Vec<String>> {
    let mut filters = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("com.docker.compose.project={}", project_name)],
    );
    filters
}

fn exact_network_name_exists(existing: &[Network], network_name: &str) -> bool {
    existing
        .iter()
        .any(|network| network.name.as_deref() == Some(network_name))
}

/// Create and start a container from a Docker image.
/// Returns the container ID.
///
/// If `network_name` is Some, the container is connected to that Docker network
/// with `name` (the app name, without the `tond-` prefix) as the network alias
/// so that peer containers can reach this one via `http://<name>:<port>`.
#[allow(clippy::too_many_arguments)]
pub async fn run_container(
    docker: &Docker,
    image: &str,
    name: &str,
    port: u16,
    host_port: u16,
    env_vars: HashMap<String, String>,
    memory_limit: Option<i64>,
    cpu_limit: Option<i64>,
    binds: Vec<String>,
    network_name: Option<&str>,
) -> anyhow::Result<String> {
    ensure_image(docker, image).await?;
    let env = env_with_port(env_vars, port);
    let port_bindings = make_port_bindings(port, host_port);

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        memory: memory_limit,
        nano_cpus: cpu_limit,
        binds: if binds.is_empty() { None } else { Some(binds) },
        ..Default::default()
    };

    // Connect to a project network if specified, using the app name as the alias
    let networking_config = network_name.map(|net| {
        let mut endpoints = HashMap::new();
        endpoints.insert(
            net.to_string(),
            EndpointSettings {
                aliases: Some(vec![name.to_string()]),
                ..Default::default()
            },
        );
        NetworkingConfig {
            endpoints_config: Some(endpoints),
        }
    });

    let config = ContainerCreateBody {
        image: Some(image.to_string()),
        env: Some(env),
        exposed_ports: Some(vec![format!("{}/tcp", port)]),
        host_config: Some(host_config),
        networking_config,
        labels: Some({
            let mut labels = HashMap::new();
            labels.insert("managed-by".to_string(), "tond".to_string());
            labels
        }),
        ..Default::default()
    };

    let container_name = format!("tond-{}", name);

    // Remove existing container if it exists
    let _ = docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await;

    let response = docker
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&container_name)
                    .build(),
            ),
            config,
        )
        .await?;

    docker.start_container(&response.id, None).await?;

    tracing::info!("Container {} started (ID: {})", container_name, response.id);

    Ok(response.id)
}

/// Stop a container by ID
pub async fn stop_container(docker: &Docker, container_id: &str) -> anyhow::Result<()> {
    docker
        .stop_container(
            container_id,
            Some(StopContainerOptionsBuilder::default().t(10).build()),
        )
        .await?;
    Ok(())
}

/// Stop a container with a custom SIGTERM grace period (seconds) before Docker
/// sends SIGKILL. Use this for ZDT drain — callers that just want to force-stop
/// a container should use `stop_container` (hardcoded 10 s).
pub async fn stop_container_with_timeout(
    docker: &Docker,
    container_id: &str,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    docker
        .stop_container(
            container_id,
            Some(
                StopContainerOptionsBuilder::default()
                    .t(timeout_secs as i32)
                    .build(),
            ),
        )
        .await?;
    Ok(())
}

/// Start a stopped container by ID
pub async fn start_container(docker: &Docker, container_id: &str) -> anyhow::Result<()> {
    docker.start_container(container_id, None).await?;
    Ok(())
}

/// Remove a container by ID (force)
pub async fn remove_container(docker: &Docker, container_id: &str) -> anyhow::Result<()> {
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await?;
    Ok(())
}

/// Get container logs
pub async fn get_logs(
    docker: &Docker,
    container_id: &str,
    tail: usize,
) -> anyhow::Result<Vec<String>> {
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .timestamps(true)
        .tail(&tail.to_string())
        .build();

    let mut stream = docker.logs(container_id, Some(options));
    let mut logs = Vec::new();

    while let Some(log) = stream.next().await {
        match log {
            Ok(output) => logs.push(output.to_string()),
            Err(e) => {
                tracing::warn!("Error reading log: {}", e);
                break;
            }
        }
    }

    Ok(logs)
}

/// Create and start a container using an explicit full container name.
/// Unlike `run_container`, the `container_name` argument is used verbatim
/// (no `tond-` prefix is added). Used for preview deployments which use
/// the pattern `tond-preview-{slug}`.
///
/// `network_alias` is used as the network alias when connecting to
/// `network_name` (typically the app name, without the `tond-preview-` prefix).
#[allow(clippy::too_many_arguments)]
pub async fn run_container_named(
    docker: &Docker,
    image: &str,
    container_name: &str,
    port: u16,
    host_port: u16,
    env_vars: HashMap<String, String>,
    memory_limit: Option<i64>,
    cpu_limit: Option<i64>,
    binds: Vec<String>,
    network_name: Option<&str>,
    network_alias: Option<&str>,
) -> anyhow::Result<String> {
    ensure_image(docker, image).await?;
    let env = env_with_port(env_vars, port);
    let port_bindings = make_port_bindings(port, host_port);

    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        memory: memory_limit,
        nano_cpus: cpu_limit,
        binds: if binds.is_empty() { None } else { Some(binds) },
        ..Default::default()
    };

    // Connect to a project network if specified
    let networking_config = network_name.map(|net| {
        let alias = network_alias.unwrap_or(container_name);
        let mut endpoints = HashMap::new();
        endpoints.insert(
            net.to_string(),
            EndpointSettings {
                aliases: Some(vec![alias.to_string()]),
                ..Default::default()
            },
        );
        NetworkingConfig {
            endpoints_config: Some(endpoints),
        }
    });

    let config = ContainerCreateBody {
        image: Some(image.to_string()),
        env: Some(env),
        exposed_ports: Some(vec![format!("{}/tcp", port)]),
        host_config: Some(host_config),
        networking_config,
        labels: Some({
            let mut labels = HashMap::new();
            labels.insert("managed-by".to_string(), "tond".to_string());
            labels.insert("tond-type".to_string(), "preview".to_string());
            labels
        }),
        ..Default::default()
    };

    // Remove existing container with the same name if it exists
    let _ = docker
        .remove_container(
            container_name,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await;

    let response = docker
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(container_name)
                    .build(),
            ),
            config,
        )
        .await?;

    docker.start_container(&response.id, None).await?;

    tracing::info!(
        "Preview container {} started (ID: {})",
        container_name,
        response.id
    );

    Ok(response.id)
}

/// Rename a container from `current_name` to `new_name`.
/// Used after a zero-downtime swap to promote the staging container
/// (`tond-{app_name}-next`) to the canonical name (`tond-{app_name}`)
/// so the next deploy can use the staging slot again.
pub async fn rename_container(
    docker: &Docker,
    current_name: &str,
    new_name: &str,
) -> anyhow::Result<()> {
    docker
        .rename_container(
            current_name,
            RenameContainerOptionsBuilder::default()
                .name(new_name)
                .build(),
        )
        .await?;
    tracing::info!("Renamed container {} -> {}", current_name, new_name);
    Ok(())
}

/// Find the host port that Docker Compose bound for `container_port` inside
/// the compose project named `project_name`.
///
/// Docker Compose labels every container it manages with
/// `com.docker.compose.project=<project>`. We list those containers and look
/// for a port binding whose `private_port` matches `container_port`.
///
/// Returns an error (with a helpful message) if no such binding is found —
/// most likely because the compose file does not have a `ports:` entry for
/// the main service.
pub async fn find_compose_host_port(
    docker: &Docker,
    project_name: &str,
    container_port: u16,
) -> anyhow::Result<u16> {
    let filters = compose_project_filter(project_name);

    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await?;

    for container in &containers {
        if let Some(ref ports) = container.ports {
            for port in ports {
                if port.private_port == container_port
                    && let Some(public) = port.public_port
                {
                    return Ok(public);
                }
            }
        }
    }

    anyhow::bail!(
        "No host port found for compose project '{}' on container port {}. \
         Ensure the compose file has a 'ports' mapping for the main service \
         (e.g. ports: [\"{}:{}\"]).",
        project_name,
        container_port,
        container_port,
        container_port,
    )
}

/// Find a free host port starting from a base
pub async fn find_free_port(docker: &Docker, start: u16) -> anyhow::Result<u16> {
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default().all(true).build(),
        ))
        .await?;

    let used_ports: Vec<u16> = containers
        .iter()
        .flat_map(|c| {
            c.ports.as_ref().map_or(vec![], |ports| {
                ports.iter().filter_map(|p| p.public_port).collect()
            })
        })
        .collect();

    next_free_port(start, &used_ports)
}

/// Ensure a Docker bridge network with the given name exists.
/// Creates it if it does not already exist. Idempotent and safe to call on
/// every deploy — does nothing if the network is already present.
pub async fn ensure_image(docker: &Docker, image: &str) -> anyhow::Result<()> {
    if docker.inspect_image(image).await.is_ok() {
        return Ok(());
    }
    tracing::info!("Pulling image {}", image);
    let options = CreateImageOptionsBuilder::default()
        .from_image(image)
        .build();
    let mut stream = docker.create_image(Some(options), None, None);
    while let Some(progress) = stream.next().await {
        progress.map_err(|e| anyhow::anyhow!("Failed to pull image {image}: {e}"))?;
    }
    Ok(())
}

pub async fn ensure_network(docker: &Docker, network_name: &str) -> anyhow::Result<()> {
    let filters: HashMap<String, Vec<String>> = {
        let mut m = HashMap::new();
        m.insert("name".to_string(), vec![network_name.to_string()]);
        m
    };

    let existing = docker
        .list_networks(Some(
            ListNetworksOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await?;

    // Docker name filter is a substring match, so verify exact name
    let already_exists = exact_network_name_exists(&existing, network_name);

    if already_exists {
        return Ok(());
    }

    let config = NetworkCreateRequest {
        name: network_name.to_string(),
        driver: Some("bridge".to_string()),
        ..Default::default()
    };

    docker.create_network(config).await?;
    tracing::info!("Created Docker network: {}", network_name);

    Ok(())
}

/// Disconnect every container from a Docker network, then remove the network.
/// Best-effort — logs warnings on errors but never returns one.
///
/// Docker refuses to remove a network that has active endpoints, so we must
/// disconnect all member containers first. This is called during project delete
/// so that the project's bridge network is cleaned up even when member app
/// containers are still running.
pub async fn remove_network(docker: &Docker, network_name: &str) {
    // Inspect the network to find all currently-connected containers.
    if let Ok(detail) = docker.inspect_network(network_name, None).await {
        let containers = detail.containers.unwrap_or_default();
        for container_id in containers.keys() {
            let req = NetworkDisconnectRequest {
                container: container_id.clone(),
                force: Some(true),
            };
            if let Err(e) = docker.disconnect_network(network_name, req).await {
                tracing::warn!(
                    "Could not disconnect container {} from network {}: {}",
                    container_id,
                    network_name,
                    e
                );
            }
        }
    }

    match docker.remove_network(network_name).await {
        Ok(()) => tracing::info!("Removed Docker network: {}", network_name),
        Err(e) => tracing::warn!("Could not remove Docker network {}: {}", network_name, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_with_port_adds_port_variable_alongside_existing_env_vars() {
        let env = env_with_port(
            HashMap::from([
                (
                    "DATABASE_URL".to_string(),
                    "sqlite:///tmp/app.db".to_string(),
                ),
                ("RUST_LOG".to_string(), "info".to_string()),
            ]),
            8080,
        );

        assert!(
            env.contains(&"DATABASE_URL=sqlite:///tmp/app.db".to_string()),
            "env_with_port should preserve application environment variables"
        );
        assert!(
            env.contains(&"RUST_LOG=info".to_string()),
            "env_with_port should include all provided environment variables"
        );
        assert!(
            env.contains(&"PORT=8080".to_string()),
            "env_with_port should inject the PORT environment variable expected by 12-factor apps"
        );
    }

    #[test]
    fn make_port_bindings_maps_container_port_to_requested_host_port() {
        let bindings = make_port_bindings(3000, 8100);
        assert_eq!(
            bindings
                .get("3000/tcp")
                .and_then(|ports| ports.as_ref())
                .and_then(|ports| ports.first())
                .and_then(|binding| binding.host_port.as_deref()),
            Some("8100"),
            "make_port_bindings should bind the container TCP port to the requested host port"
        );
    }

    #[test]
    fn next_free_port_skips_used_ports_and_errors_when_range_is_exhausted() {
        assert_eq!(
            next_free_port(8100, &[8100, 8101, 8103])
                .unwrap_or_else(|err| panic!("next_free_port should find a free port: {err}")),
            8102,
            "next_free_port should return the first unused port at or above the starting value"
        );

        let used: Vec<u16> = (65000..=65000).collect();
        assert!(
            next_free_port(65000, &used)
                .err()
                .unwrap_or_else(|| panic!(
                    "expected next_free_port to fail when the search range is exhausted"
                ))
                .to_string()
                .contains("No free ports available"),
            "next_free_port should report a helpful error when no ports remain in the allowed range"
        );
    }

    #[test]
    fn compose_project_filter_uses_docker_compose_project_label() {
        assert_eq!(
            compose_project_filter("frontend"),
            HashMap::from([(
                "label".to_string(),
                vec!["com.docker.compose.project=frontend".to_string()],
            )]),
            "compose_project_filter should target containers belonging to the requested compose project"
        );
    }

    #[test]
    fn exact_network_name_exists_requires_exact_match_not_substring_match() {
        let existing = vec![
            Network {
                name: Some("tond-project-1-extra".to_string()),
                ..Default::default()
            },
            Network {
                name: Some("tond-project-1".to_string()),
                ..Default::default()
            },
        ];

        assert!(
            exact_network_name_exists(&existing, "tond-project-1"),
            "exact_network_name_exists should accept an exact network name match"
        );
        assert!(
            !exact_network_name_exists(&existing[..1], "tond-project-1"),
            "exact_network_name_exists should reject substring-only matches returned by Docker name filters"
        );
    }
}
