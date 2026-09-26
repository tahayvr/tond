// docker.rs — Docker API abstraction layer
//
// Thin wrappers around the Bollard Docker client. Every function takes a
// &Docker (cheap to clone via Arc internally) and returns anyhow::Result.
// This keeps all Bollard-specific types contained here; callers only see
// plain Rust types (strings, vecs).
//
// Functions:
//
//   run_container(docker, image, name, port, host_port, exposure, env_vars, memory_limit, cpu_limit, binds, network)
//     Creates and starts a container named `tond-{name}`. Before creating,
//     it force-removes any existing container with the same name (handles
//     re-deploys). The container is configured with:
//       - Port binding: 127.0.0.1:{host_port} -> {port}/tcp (0.0.0.0 only for
//         Exposure::Public, i.e. a database with public access turned on).
//         Docker's iptables rules run before UFW, so a 0.0.0.0 binding is
//         reachable from the internet no matter what the firewall says.
//       - The hardening every Tond container gets (see `harden_host_config`):
//         no-new-privileges, a trimmed capability set, a pids limit, a
//         default memory limit and rotated json-file logs
//       - Restart policy: UnlessStopped (survives host reboots)
//       - Label: managed-by=tond (for identification)
//       - Env vars + injected PORT={port} (12-factor convention)
//       - Memory limit: memory_limit bytes (None = no limit)
//       - CPU limit: cpu_limit nano_cpus (None = no limit; 1e9 = 1 CPU)
//       - Bind mounts: binds as Vec<String> in "host_path:container_path" format
//       - network: the only Docker network the container joins (never the
//         default bridge), with the app name as its alias there (so peers on
//         that network reach it as http://appname:port). See networks.rs.
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
//   ensure_network(docker, network_name, icc)
//     Creates a labelled Docker bridge network with the given name if it does
//     not already exist (optionally with inter-container traffic off).
//     Idempotent — safe to call on every deploy.
//
//   remove_network(docker, network_name)
//     Removes a Docker network by name. Best-effort — logs a warning but
//     does not return an error if the network does not exist.
//
//   connect_network / disconnect_network / container_networks
//     Attach or detach an existing container, list the networks it is on.
//
//   prune_networks(docker)
//     Remove Tond-labelled networks that no container uses any more.

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

/// Which host interface a published container port binds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// 127.0.0.1 — reachable only from the host (tond-proxy, health checks,
    /// SSH tunnels). The default for everything.
    Loopback,
    /// 0.0.0.0 — reachable from the internet. Only for managed databases whose
    /// admin explicitly enabled public access.
    Public,
}

impl Exposure {
    pub fn host_ip(self) -> &'static str {
        match self {
            Exposure::Loopback => "127.0.0.1",
            Exposure::Public => "0.0.0.0",
        }
    }
}

// ── Container hardening ──────────────────────────────────────────────────────
//
// Capability policy: keep Docker's default set minus the capabilities that
// ordinary images never need at runtime but that widen the attack surface:
//
//   NET_RAW      raw/packet sockets — ARP/DNS spoofing of neighbours on the
//                shared bridge network (only `ping` misses it)
//   MKNOD        creating device nodes
//   AUDIT_WRITE  writing to the kernel audit log
//   SETFCAP      setting file capabilities (used in user-namespace escapes)
//
// `cap_drop: ALL` would be stricter but breaks the images people actually run:
// the postgres, mysql/mariadb, mongo and redis entrypoints chown their data
// directory and drop to an unprivileged user (CHOWN, DAC_OVERRIDE, FOWNER,
// SETUID, SETGID), and nginx binds :80 as root before dropping privileges
// (NET_BIND_SERVICE, SETUID, SETGID). Those all stay.
//
// no-new-privileges stops setuid binaries and file capabilities from raising
// privileges after exec; gosu/su-exec style entrypoints keep working because
// they start as root and only lower privileges.

/// Capabilities removed from Docker's default set for every Tond container.
pub const DROPPED_CAPABILITIES: &[&str] = &["NET_RAW", "MKNOD", "AUDIT_WRITE", "SETFCAP"];

/// Max processes + threads per container: stops fork bombs from taking the
/// host down while leaving plenty of headroom for thread-heavy databases.
pub const PIDS_LIMIT: i64 = 4096;

/// Log rotation for every container Tond starts: 3 × 10 MiB json-file logs
/// (json-file keeps `docker logs` and the dashboard log stream working).
pub const LOG_DRIVER: &str = "json-file";
pub const LOG_MAX_SIZE: &str = "10m";
pub const LOG_MAX_FILE: &str = "3";

/// Smallest default memory cap, for hosts where 75% of RAM is tiny.
const MIN_DEFAULT_MEMORY: i64 = 256 * 1024 * 1024;

/// The json-file log configuration with rotation.
pub fn log_config() -> HostConfigLogConfig {
    HostConfigLogConfig {
        typ: Some(LOG_DRIVER.to_string()),
        config: Some(HashMap::from([
            ("max-size".to_string(), LOG_MAX_SIZE.to_string()),
            ("max-file".to_string(), LOG_MAX_FILE.to_string()),
        ])),
    }
}

/// Default memory cap for containers without an explicit limit: 75% of the
/// host's RAM, so one runaway container can't OOM tond-server, tond-proxy
/// and sshd while a normal app never notices the limit.
pub fn default_memory_limit_for(total_bytes: u64) -> i64 {
    let three_quarters = (total_bytes / 4 * 3).min(i64::MAX as u64) as i64;
    three_quarters.max(MIN_DEFAULT_MEMORY)
}

/// [`default_memory_limit_for`] the current host (measured once).
pub fn default_memory_limit() -> i64 {
    static LIMIT: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        default_memory_limit_for(sys.total_memory())
    })
}

/// Apply Tond's container hardening to a HostConfig: no-new-privileges, the
/// trimmed capability set, a pids limit, a default memory limit when none is
/// set and rotated logs. Explicit (positive) memory / pids limits are kept.
pub fn harden_host_config(host_config: &mut HostConfig, default_memory: i64) {
    let security_opt = host_config.security_opt.get_or_insert_with(Vec::new);
    if !security_opt
        .iter()
        .any(|o| o.starts_with("no-new-privileges"))
    {
        security_opt.push("no-new-privileges:true".to_string());
    }
    let cap_drop = host_config.cap_drop.get_or_insert_with(Vec::new);
    for cap in DROPPED_CAPABILITIES {
        if !cap_drop.iter().any(|c| c.eq_ignore_ascii_case(cap)) {
            cap_drop.push((*cap).to_string());
        }
    }
    if host_config.pids_limit.is_none_or(|p| p <= 0) {
        host_config.pids_limit = Some(PIDS_LIMIT);
    }
    if host_config.memory.is_none_or(|m| m <= 0) {
        host_config.memory = Some(default_memory);
    }
    host_config.log_config = Some(log_config());
}

fn make_port_bindings(
    port: u16,
    host_port: u16,
    exposure: Exposure,
) -> HashMap<String, Option<Vec<PortBinding>>> {
    let port_binding = PortBinding {
        host_ip: Some(exposure.host_ip().to_string()),
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

/// HostConfig for a long-running service container (app, preview, database):
/// published port on the requested interface, restart policy, limits, binds,
/// plus [`harden_host_config`].
fn service_host_config(
    port: u16,
    host_port: u16,
    exposure: Exposure,
    memory_limit: Option<i64>,
    cpu_limit: Option<i64>,
    binds: Vec<String>,
) -> HostConfig {
    let mut host_config = HostConfig {
        port_bindings: Some(make_port_bindings(port, host_port, exposure)),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        memory: memory_limit,
        nano_cpus: cpu_limit,
        binds: if binds.is_empty() { None } else { Some(binds) },
        ..Default::default()
    };
    harden_host_config(&mut host_config, default_memory_limit());
    host_config
}

/// Put a container on exactly one network: `network_mode` makes it the
/// container's network instead of the default bridge, and the endpoint config
/// gives it `alias` there.
fn attach_network(
    mut host_config: HostConfig,
    network: &str,
    alias: &str,
) -> (HostConfig, Option<NetworkingConfig>) {
    host_config.network_mode = Some(network.to_string());
    let endpoints = HashMap::from([(
        network.to_string(),
        EndpointSettings {
            aliases: Some(vec![alias.to_string()]),
            ..Default::default()
        },
    )]);
    (
        host_config,
        Some(NetworkingConfig {
            endpoints_config: Some(endpoints),
        }),
    )
}

/// Create and start a container from a Docker image.
/// Returns the container ID.
///
/// The container joins `network` only (never the default bridge), with `name`
/// (the app name, without the `tond-` prefix) as its alias there so peer
/// containers on that network reach it via `http://<name>:<port>`.
#[allow(clippy::too_many_arguments)]
pub async fn run_container(
    docker: &Docker,
    image: &str,
    name: &str,
    port: u16,
    host_port: u16,
    exposure: Exposure,
    env_vars: HashMap<String, String>,
    memory_limit: Option<i64>,
    cpu_limit: Option<i64>,
    binds: Vec<String>,
    network: &str,
) -> anyhow::Result<String> {
    ensure_image(docker, image).await?;
    let env = env_with_port(env_vars, port);
    let host_config =
        service_host_config(port, host_port, exposure, memory_limit, cpu_limit, binds);

    let (host_config, networking_config) = attach_network(host_config, network, name);

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
/// `network_alias` is used as the alias on `network` (typically the app name,
/// without the `tond-preview-` prefix); the container joins only `network`.
/// `cmd` overrides the image's default command (managed Redis/KeyDB pass
/// their generated config file this way); `None` keeps the image default.
#[allow(clippy::too_many_arguments)]
pub async fn run_container_named(
    docker: &Docker,
    image: &str,
    container_name: &str,
    port: u16,
    host_port: u16,
    exposure: Exposure,
    env_vars: HashMap<String, String>,
    memory_limit: Option<i64>,
    cpu_limit: Option<i64>,
    binds: Vec<String>,
    network: &str,
    network_alias: Option<&str>,
    cmd: Option<Vec<String>>,
) -> anyhow::Result<String> {
    ensure_image(docker, image).await?;
    let env = env_with_port(env_vars, port);
    let host_config =
        service_host_config(port, host_port, exposure, memory_limit, cpu_limit, binds);

    let (host_config, networking_config) = attach_network(
        host_config,
        network,
        network_alias.unwrap_or(container_name),
    );

    let config = ContainerCreateBody {
        image: Some(image.to_string()),
        env: Some(env),
        cmd,
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

/// Force-remove every container of a compose project and its default network.
/// Best-effort, like `docker compose down` without touching named volumes.
pub async fn remove_compose_project(docker: &Docker, project_name: &str) {
    let filters = compose_project_filter(project_name);
    if let Ok(containers) = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await
    {
        for id in containers.into_iter().filter_map(|c| c.id) {
            let _ = remove_container(docker, &id).await;
        }
    }
    // The default network and any network the file declares.
    let _ = docker
        .remove_network(&format!("{project_name}_default"))
        .await;
    if let Ok(networks) = docker
        .list_networks(Some(
            ListNetworksOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await
    {
        for name in networks.into_iter().filter_map(|n| n.name) {
            let _ = docker.remove_network(&name).await;
        }
    }
}

/// Named volumes of a compose project (`<project>_<volume>`, labelled by
/// compose) with their mount points: `(volume name, mountpoint)`.
pub async fn compose_volumes(
    docker: &Docker,
    project_name: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let filters = compose_project_filter(project_name);
    let list = docker
        .list_volumes(Some(
            bollard::query_parameters::ListVolumesOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await?;
    Ok(list
        .volumes
        .unwrap_or_default()
        .into_iter()
        .filter(|v| v.driver == "local")
        .map(|v| (v.name, v.mountpoint))
        .collect())
}

/// Remove the named volumes of a compose project (app deletion).
pub async fn remove_compose_volumes(docker: &Docker, project_name: &str) {
    for (name, _) in compose_volumes(docker, project_name)
        .await
        .unwrap_or_default()
    {
        let _ = docker
            .remove_volume(
                &name,
                Some(
                    bollard::query_parameters::RemoveVolumeOptionsBuilder::default()
                        .force(true)
                        .build(),
                ),
            )
            .await;
    }
}

/// Host ports published by live containers, except those of the compose
/// project `exclude_project` (its containers are about to be recreated).
pub async fn used_host_ports(
    docker: &Docker,
    exclude_project: Option<&str>,
) -> anyhow::Result<Vec<u16>> {
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default().all(true).build(),
        ))
        .await?;
    Ok(containers
        .iter()
        .filter(|c| {
            exclude_project.is_none_or(|p| {
                c.labels
                    .as_ref()
                    .and_then(|l| l.get("com.docker.compose.project"))
                    .map(String::as_str)
                    != Some(p)
            })
        })
        .flat_map(|c| {
            c.ports.as_ref().map_or(vec![], |ports| {
                ports.iter().filter_map(|p| p.public_port).collect()
            })
        })
        .collect())
}

/// The first port at or above `start` not in `used`.
pub fn first_free_port(start: u16, used: &[u16]) -> anyhow::Result<u16> {
    next_free_port(start, used)
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

/// Pull `image` unless it is already present locally.
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

/// Label on every network Tond creates, so pruning never touches others.
pub const NETWORK_LABEL: (&str, &str) = ("managed-by", "tond");

/// Bridge driver option that switches off traffic between containers on the
/// same network (they can still reach the internet and published ports).
pub const ENABLE_ICC_OPTION: &str = "com.docker.network.bridge.enable_icc";

/// The create request for a Tond bridge network.
fn network_create_request(network_name: &str, icc: bool) -> NetworkCreateRequest {
    NetworkCreateRequest {
        name: network_name.to_string(),
        driver: Some("bridge".to_string()),
        labels: Some(HashMap::from([(
            NETWORK_LABEL.0.to_string(),
            NETWORK_LABEL.1.to_string(),
        )])),
        options: (!icc)
            .then(|| HashMap::from([(ENABLE_ICC_OPTION.to_string(), "false".to_string())])),
        ..Default::default()
    }
}

/// Ensure a Docker bridge network with the given name exists, creating it
/// (labelled `managed-by=tond`, and with inter-container traffic off unless
/// `icc`) if not. Idempotent and safe to call on every deploy.
pub async fn ensure_network(docker: &Docker, network_name: &str, icc: bool) -> anyhow::Result<()> {
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

    match docker
        .create_network(network_create_request(network_name, icc))
        .await
    {
        Ok(_) => {}
        // Lost a race with a concurrent deploy creating the same network.
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 409, ..
        }) => return Ok(()),
        Err(e) => return Err(e.into()),
    }
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

/// Attach an existing (running or stopped) container to a network with
/// `alias`. Already being connected is not an error.
pub async fn connect_network(
    docker: &Docker,
    network_name: &str,
    container: &str,
    alias: &str,
) -> anyhow::Result<()> {
    let req = NetworkConnectRequest {
        container: container.to_string(),
        endpoint_config: Some(EndpointSettings {
            aliases: Some(vec![alias.to_string()]),
            ..Default::default()
        }),
    };
    match docker.connect_network(network_name, req).await {
        Ok(()) => Ok(()),
        Err(bollard::errors::Error::DockerResponseServerError { message, .. })
            if message.contains("already exists") =>
        {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Detach a container from a network (forced, so a stopped container works
/// too). Not being connected is not an error.
pub async fn disconnect_network(
    docker: &Docker,
    network_name: &str,
    container: &str,
) -> anyhow::Result<()> {
    let req = NetworkDisconnectRequest {
        container: container.to_string(),
        force: Some(true),
    };
    match docker.disconnect_network(network_name, req).await {
        Ok(()) => Ok(()),
        Err(bollard::errors::Error::DockerResponseServerError { message, .. })
            if message.contains("is not connected") =>
        {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Names of the networks a container is attached to.
pub async fn container_networks(docker: &Docker, container: &str) -> anyhow::Result<Vec<String>> {
    let detail = docker.inspect_container(container, None).await?;
    let mut names: Vec<String> = detail
        .network_settings
        .and_then(|n| n.networks)
        .map(|n| n.into_keys().collect())
        .unwrap_or_default();
    names.sort();
    Ok(names)
}

/// Remove Tond-labelled networks without containers that are older than an
/// hour (so a network created a moment ago for a deploy in progress is never
/// raced away). Networks are cheap to recreate: every deploy ensures its own.
pub async fn prune_networks(docker: &Docker) -> anyhow::Result<usize> {
    let filters = HashMap::from([
        (
            "label".to_string(),
            vec![format!("{}={}", NETWORK_LABEL.0, NETWORK_LABEL.1)],
        ),
        ("until".to_string(), vec!["1h".to_string()]),
    ]);
    let response = docker
        .prune_networks(Some(
            bollard::query_parameters::PruneNetworksOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await?;
    Ok(response.networks_deleted.map_or(0, |n| n.len()))
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
    fn make_port_bindings_binds_loopback_by_default_and_all_interfaces_only_when_public() {
        let host_ip = |exposure| {
            make_port_bindings(3000, 8100, exposure)
                .get("3000/tcp")
                .and_then(|ports| ports.as_ref())
                .and_then(|ports| ports.first())
                .and_then(|binding| binding.host_ip.clone())
        };
        assert_eq!(host_ip(Exposure::Loopback).as_deref(), Some("127.0.0.1"));
        assert_eq!(host_ip(Exposure::Public).as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn harden_host_config_applies_the_container_policy() {
        let mut hc = HostConfig::default();
        harden_host_config(&mut hc, 1_000);
        assert_eq!(
            hc.security_opt,
            Some(vec!["no-new-privileges:true".to_string()])
        );
        assert_eq!(
            hc.cap_drop,
            Some(DROPPED_CAPABILITIES.iter().map(|c| c.to_string()).collect())
        );
        assert_eq!(hc.pids_limit, Some(PIDS_LIMIT));
        assert_eq!(hc.memory, Some(1_000));
        let log = hc.log_config.unwrap_or_default();
        assert_eq!(log.typ.as_deref(), Some("json-file"));
        let cfg = log.config.unwrap_or_default();
        assert_eq!(cfg.get("max-size").map(String::as_str), Some("10m"));
        assert_eq!(cfg.get("max-file").map(String::as_str), Some("3"));
    }

    #[test]
    fn harden_host_config_keeps_explicit_limits_and_does_not_duplicate_options() {
        let mut hc = HostConfig {
            memory: Some(512),
            pids_limit: Some(100),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            cap_drop: Some(vec!["net_raw".to_string()]),
            ..Default::default()
        };
        harden_host_config(&mut hc, 1_000);
        assert_eq!(hc.memory, Some(512));
        assert_eq!(hc.pids_limit, Some(100));
        assert_eq!(hc.security_opt.map(|o| o.len()), Some(1));
        assert_eq!(
            hc.cap_drop.map(|c| c.len()),
            Some(DROPPED_CAPABILITIES.len())
        );

        // Zero / negative limits mean "unset" and get the defaults.
        let mut hc = HostConfig {
            memory: Some(0),
            pids_limit: Some(-1),
            ..Default::default()
        };
        harden_host_config(&mut hc, 1_000);
        assert_eq!(hc.memory, Some(1_000));
        assert_eq!(hc.pids_limit, Some(PIDS_LIMIT));
    }

    #[test]
    fn default_memory_limit_is_three_quarters_of_ram_with_a_floor() {
        assert_eq!(
            default_memory_limit_for(4 * 1024 * 1024 * 1024),
            3 * 1024 * 1024 * 1024
        );
        assert_eq!(default_memory_limit_for(0), MIN_DEFAULT_MEMORY);
        assert!(default_memory_limit() >= MIN_DEFAULT_MEMORY);
    }

    #[test]
    fn service_host_config_is_hardened_and_loopback_bound() {
        let hc = service_host_config(80, 8100, Exposure::Loopback, None, None, vec![]);
        assert!(
            hc.security_opt
                .unwrap_or_default()
                .contains(&"no-new-privileges:true".to_string())
        );
        assert!(hc.log_config.is_some());
        assert!(hc.memory.is_some_and(|m| m > 0));
        assert!(hc.binds.is_none());
    }

    #[test]
    fn make_port_bindings_maps_container_port_to_requested_host_port() {
        let bindings = make_port_bindings(3000, 8100, Exposure::Loopback);
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
    fn network_create_request_labels_networks_and_can_switch_off_icc() {
        let isolated = network_create_request("tond-net-db-main", false);
        assert_eq!(
            isolated
                .labels
                .as_ref()
                .and_then(|l| l.get("managed-by"))
                .map(String::as_str),
            Some("tond"),
            "every Tond network carries the managed-by label so pruning only touches ours"
        );
        assert_eq!(
            isolated
                .options
                .as_ref()
                .and_then(|o| o.get(ENABLE_ICC_OPTION))
                .map(String::as_str),
            Some("false"),
            "single-tenant networks switch inter-container traffic off"
        );
        let shared = network_create_request("tond-net-app-web", true);
        assert!(shared.options.is_none(), "app/project networks keep icc on");
    }

    #[test]
    fn attach_network_replaces_the_default_bridge() {
        let (host_config, networking) =
            attach_network(HostConfig::default(), "tond-net-app-web", "web");
        assert_eq!(
            host_config.network_mode.as_deref(),
            Some("tond-net-app-web"),
            "network_mode must name the network so Docker never adds the default bridge"
        );
        let endpoints = networking
            .and_then(|n| n.endpoints_config)
            .unwrap_or_default();
        assert_eq!(endpoints.len(), 1, "exactly one network");
        assert_eq!(
            endpoints
                .get("tond-net-app-web")
                .and_then(|e| e.aliases.clone()),
            Some(vec!["web".to_string()])
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
