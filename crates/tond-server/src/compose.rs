// compose.rs — Docker Compose apps: services, pinning, discovery
//
// A compose app is a set of services. Tond keeps one `compose_services` row
// per service (migration 010): what the file declares (image, build, ports,
// healthcheck), what the user chose (the container port Tond publishes for
// routing, and through `app_domains` the domains routed to it) and what
// Docker reports (container, state, health).
//
// Deploy flow (deploy/compose.rs uses the helpers here):
//
//   1. The file is validated and hardened by compose_policy (unchanged).
//   2. `service_specs` reads the services from it; `sync_specs` upserts the
//      rows (user choices survive) and removes services that left the file.
//   3. Each service's image is produced by Tond, not by `docker compose`:
//      built with `docker build --cgroup-parent` (so compose builds share the
//      build memory cap) or pulled, then tagged
//      `tond-<app>/<service>:<deployment-id-prefix>` (`pinned_tag`).
//   4. `pin_file` rewrites the hardened file: every resolvable service runs
//      its pinned tag (`build:` removed, `pull_policy: never`) and a routed
//      service gets `127.0.0.1:<host port>:<container port>` added.
//   5. `docker compose up -d --remove-orphans` starts the set; `discover`
//      reads each service's container, state and health from the compose
//      labels (`com.docker.compose.project` / `.service`).
//
// Rollback re-runs a previous deployment's compose file with its pinned tags
// (deployment_services), so it restores exactly that set of images.
//
// Interpolation: `docker compose` runs with a cleared environment, so
// `${VAR:-default}` becomes `default` and `${VAR}` becomes empty.
// `interpolate` applies the same rule where Tond reads values itself (image
// refs and build args).

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use bollard::Docker;
use bollard::models::ContainerSummaryHealthStatusEnum;
use bollard::query_parameters::ListContainersOptionsBuilder;
use serde_yaml::{Mapping, Value};
use tond_common::ComposeServiceSpec;

/// Label Docker Compose puts on every container of a project.
pub const PROJECT_LABEL: &str = "com.docker.compose.project";
/// Label naming the compose service of a container.
pub const SERVICE_LABEL: &str = "com.docker.compose.service";
/// Default compose file path for git-sourced compose apps.
pub const DEFAULT_COMPOSE_PATH: &str = "docker-compose.yml";

// ── Parsing ──────────────────────────────────────────────────────────────────

fn untag(v: &Value) -> &Value {
    let mut v = v;
    while let Value::Tagged(t) = v {
        v = &t.value;
    }
    v
}

fn untag_mut(v: &mut Value) -> &mut Value {
    let mut v = v;
    while let Value::Tagged(t) = v {
        v = &mut t.value;
    }
    v
}

fn scalar(v: &Value) -> Option<String> {
    match untag(v) {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn load(content: &str) -> Result<Value, String> {
    let mut doc: Value = serde_yaml::from_str(content)
        .map_err(|e| format!("The compose file is not valid YAML: {e}"))?;
    doc.apply_merge()
        .map_err(|e| format!("The compose file has an invalid '<<' merge key: {e}"))?;
    Ok(doc)
}

/// Resolve compose variable substitution against an empty environment:
/// `${VAR:-default}` / `${VAR-default}` → default, `${VAR}` / `$VAR` → "",
/// `$$` → `$`.
pub fn interpolate(s: &str) -> String {
    interpolate_env(s, &BTreeMap::new())
}

/// Compose variable substitution against `env` (the app's environment, the
/// only variables `docker compose` sees): `${VAR}` / `$VAR` → the value or
/// "", `${VAR:-d}` → d when unset or empty, `${VAR-d}` → d when unset,
/// `$$` → `$`.
pub fn interpolate_env(s: &str, env: &BTreeMap<String, String>) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            let ch = s[i..].chars().next().unwrap_or_default();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        match b.get(i + 1) {
            Some(b'$') => {
                out.push('$');
                i += 2;
            }
            Some(b'{') => match closing_brace(&s[i + 2..]) {
                Some(end) => {
                    let inner = &s[i + 2..i + 2 + end];
                    let name_len = inner
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(inner.len());
                    let (name, rest) = inner.split_at(name_len);
                    let value = env.get(name);
                    if let Some(default) = rest.strip_prefix(":-") {
                        match value.filter(|v| !v.is_empty()) {
                            Some(v) => out.push_str(v),
                            None => out.push_str(&interpolate_env(default, env)),
                        }
                    } else if let Some(default) = rest.strip_prefix('-') {
                        match value {
                            Some(v) => out.push_str(v),
                            None => out.push_str(&interpolate_env(default, env)),
                        }
                    } else if let Some(v) = value {
                        // `${VAR}`, and `${VAR:?err}` / `${VAR?err}` when set.
                        out.push_str(v);
                    }
                    i += 3 + end;
                }
                None => {
                    out.push_str(&s[i..]);
                    break;
                }
            },
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => {
                let start = i + 1;
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if let Some(v) = env.get(&s[start..i]) {
                    out.push_str(v);
                }
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

/// Index of the `}` closing a `${`, skipping nested `${...}` defaults.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Container port of one `ports:` or `expose:` entry: "80", "8080:80",
/// "127.0.0.1:8080:80/tcp", {target: 80}. Ranges and UDP ports are skipped.
fn container_port(entry: &Value) -> Option<u16> {
    match untag(entry) {
        Value::Mapping(m) => {
            let proto = m.get("protocol").and_then(scalar).unwrap_or_default();
            if proto.eq_ignore_ascii_case("udp") {
                return None;
            }
            m.get("target").and_then(scalar)?.trim().parse().ok()
        }
        other => {
            let spec = interpolate(&scalar(other)?);
            let (spec, proto) = spec.split_once('/').unwrap_or((spec.as_str(), "tcp"));
            if proto.eq_ignore_ascii_case("udp") {
                return None;
            }
            // "[::1]:8080:80" → strip the bracketed address first.
            let spec = match spec.strip_prefix('[') {
                Some(rest) => rest.split_once("]:").map_or(rest, |(_, r)| r),
                None => spec,
            };
            spec.rsplit(':').next()?.trim().parse().ok()
        }
    }
}

fn declared_ports(svc: &Mapping) -> Vec<u16> {
    let mut ports = Vec::new();
    for key in ["ports", "expose"] {
        if let Some(Value::Sequence(items)) = svc.get(key).map(untag) {
            for item in items {
                if let Some(p) = container_port(item)
                    && !ports.contains(&p)
                {
                    ports.push(p);
                }
            }
        }
    }
    ports
}

fn has_healthcheck(svc: &Mapping) -> bool {
    match svc.get("healthcheck").map(untag) {
        Some(Value::Mapping(hc)) => !hc
            .get("disable")
            .is_some_and(|d| matches!(untag(d), Value::Bool(true))),
        _ => false,
    }
}

/// The services a compose file declares, in file order (variables resolved
/// against an empty environment).
pub fn service_specs(content: &str) -> Result<Vec<ComposeServiceSpec>, String> {
    service_specs_env(content, &BTreeMap::new())
}

/// [`service_specs`] with variables resolved against the app's environment.
pub fn service_specs_env(
    content: &str,
    env: &BTreeMap<String, String>,
) -> Result<Vec<ComposeServiceSpec>, String> {
    let doc = load(content)?;
    let services = match untag(&doc).get("services").map(untag) {
        Some(Value::Mapping(s)) => s,
        _ => return Err("The compose file must define services under 'services:'".to_string()),
    };
    let mut specs = Vec::new();
    for (name, svc) in services {
        let Some(name) = scalar(name) else { continue };
        let empty = Mapping::new();
        let svc = match untag(svc) {
            Value::Mapping(m) => m,
            _ => &empty,
        };
        specs.push(ComposeServiceSpec {
            image: svc
                .get("image")
                .and_then(scalar)
                .map(|i| interpolate_env(&i, env))
                .filter(|i| !i.trim().is_empty()),
            builds: svc.contains_key("build"),
            ports: declared_ports(svc),
            has_healthcheck: has_healthcheck(svc),
            name,
        });
    }
    Ok(specs)
}

/// Volumes a compose file uses: named volumes (top-level `volumes:` keys and
/// named sources in services) and whether any service bind-mounts a path.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VolumeUsage {
    pub named: Vec<String>,
    pub binds: bool,
}

pub fn volume_usage(content: &str) -> VolumeUsage {
    let mut usage = VolumeUsage::default();
    let Ok(doc) = load(content) else {
        return usage;
    };
    if let Some(Value::Mapping(vols)) = untag(&doc).get("volumes").map(untag) {
        for name in vols.keys().filter_map(scalar) {
            if !usage.named.contains(&name) {
                usage.named.push(name);
            }
        }
    }
    if let Some(Value::Mapping(services)) = untag(&doc).get("services").map(untag) {
        for svc in services.values() {
            let Some(Value::Sequence(items)) = untag(svc).get("volumes").map(untag) else {
                continue;
            };
            for item in items {
                match untag(item) {
                    Value::String(spec) => {
                        let mut parts = spec.split(':');
                        let src = parts.next().unwrap_or_default();
                        if parts.next().is_none() {
                            continue; // anonymous volume
                        }
                        if src.starts_with(['/', '.', '~']) {
                            usage.binds = true;
                        } else if !usage.named.iter().any(|n| n == src) {
                            usage.named.push(src.to_string());
                        }
                    }
                    Value::Mapping(m) => match m.get("type").and_then(scalar).as_deref() {
                        Some("bind") => usage.binds = true,
                        Some("volume") | None => {
                            if let Some(src) = m.get("source").and_then(scalar)
                                && !usage.named.contains(&src)
                            {
                                usage.named.push(src);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
    usage
}

/// The service that answers web traffic: `preferred` if the file has it,
/// else the first service declaring `port`, else the first service declaring
/// any port. Returns the service and the container port to publish.
pub fn pick_web_service(
    specs: &[ComposeServiceSpec],
    preferred: Option<&str>,
    port: u16,
) -> Option<(String, u16)> {
    if let Some(name) = preferred.filter(|n| !n.trim().is_empty()) {
        let spec = specs.iter().find(|s| s.name == name)?;
        let p = if spec.ports.contains(&port) || spec.ports.is_empty() {
            port
        } else {
            spec.ports[0]
        };
        return Some((spec.name.clone(), p));
    }
    if let Some(spec) = specs.iter().find(|s| s.ports.contains(&port)) {
        return Some((spec.name.clone(), port));
    }
    specs
        .iter()
        .find(|s| !s.ports.is_empty())
        .map(|s| (s.name.clone(), s.ports[0]))
}

/// Build instructions of one service, resolved against the directory that
/// holds the compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    /// Build context: an absolute path or a git URL
    pub context: String,
    /// Dockerfile path (relative to the context, as `docker build -f` takes
    /// it when the context is local), if not the default
    pub dockerfile: Option<String>,
    /// Inline Dockerfile content (`dockerfile_inline`)
    pub dockerfile_inline: Option<String>,
    pub args: Vec<(String, String)>,
    pub target: Option<String>,
}

fn is_remote_context(ctx: &str) -> bool {
    let lower = ctx.to_ascii_lowercase();
    [
        "http://",
        "https://",
        "git://",
        "git@",
        "ssh://",
        "github.com/",
    ]
    .iter()
    .any(|p| lower.starts_with(p))
}

/// Build specs of every service with a `build:` section. Paths were checked
/// by compose_policy (inside the compose directory, no `..`).
pub fn build_specs(
    content: &str,
    base_dir: &std::path::Path,
    env: &BTreeMap<String, String>,
) -> BTreeMap<String, BuildSpec> {
    let mut out = BTreeMap::new();
    let Ok(doc) = load(content) else {
        return out;
    };
    let Some(Value::Mapping(services)) = untag(&doc).get("services").map(untag) else {
        return out;
    };
    for (name, svc) in services {
        let Some(name) = scalar(name) else { continue };
        let Some(build) = untag(svc).get("build").map(untag) else {
            continue;
        };
        let (context, dockerfile, inline, args, target) = match build {
            Value::Mapping(b) => {
                let args = match b.get("args").map(untag) {
                    Some(Value::Mapping(m)) => m
                        .iter()
                        .filter_map(|(k, v)| {
                            Some((
                                scalar(k)?,
                                interpolate_env(&scalar(v).unwrap_or_default(), env),
                            ))
                        })
                        .collect(),
                    Some(Value::Sequence(items)) => items
                        .iter()
                        .filter_map(scalar)
                        .map(|kv| match kv.split_once('=') {
                            Some((k, v)) => (k.to_string(), interpolate_env(v, env)),
                            None => (kv, String::new()),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                (
                    b.get("context")
                        .and_then(scalar)
                        .unwrap_or_else(|| ".".into()),
                    b.get("dockerfile").and_then(scalar),
                    b.get("dockerfile_inline").and_then(scalar),
                    args,
                    b.get("target")
                        .and_then(scalar)
                        .map(|t| interpolate_env(&t, env)),
                )
            }
            other => (
                scalar(other).unwrap_or_else(|| ".".into()),
                None,
                None,
                Vec::new(),
                None,
            ),
        };
        let context = if is_remote_context(&context) {
            context
        } else {
            base_dir.join(&context).to_string_lossy().into_owned()
        };
        out.insert(
            name,
            BuildSpec {
                context,
                dockerfile,
                dockerfile_inline: inline,
                args,
                target,
            },
        );
    }
    out
}

// ── Pinning ──────────────────────────────────────────────────────────────────

/// Docker repository path component for a service name: lowercase, only
/// `[a-z0-9._-]`, starting with a letter or digit.
pub fn repo_component(service: &str) -> String {
    let mut s: String = service
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.starts_with(['.', '_', '-']) {
        s.remove(0);
    }
    while s.ends_with(['.', '_', '-']) {
        s.pop();
    }
    if s.is_empty() { "svc".to_string() } else { s }
}

/// `tond-<app>/<service>` — the local repository of a service's images.
pub fn service_repo(app_name: &str, service: &str) -> String {
    format!(
        "{}/{}",
        crate::deploy::images::image_repo(app_name),
        repo_component(service)
    )
}

/// `tond-<app>/<service>:<deployment-id-prefix>`: the pinned image a
/// deployment runs for a service.
pub fn pinned_tag(app_name: &str, service: &str, deployment_id: &str) -> String {
    let release = crate::deploy::images::deployment_tag(app_name, deployment_id);
    let suffix = release.rsplit_once(':').map_or("", |(_, t)| t);
    format!("{}:{suffix}", service_repo(app_name, service))
}

/// How one service runs in a deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub service: String,
    /// Pinned local image (None: leave the service to compose)
    pub image: Option<String>,
    /// `(host port, container port)` Tond publishes on 127.0.0.1
    pub publish: Option<(u16, u16)>,
}

/// Rewrite a hardened compose file so each service runs its pinned image and
/// routed services publish their port on the Tond-allocated loopback port.
pub fn pin_file(hardened_yaml: &str, pins: &[Pin]) -> Result<String, String> {
    let mut doc = load(hardened_yaml)?;
    let Some(Value::Mapping(services)) = untag_mut(&mut doc)
        .as_mapping_mut()
        .and_then(|root| root.get_mut("services"))
        .map(untag_mut)
    else {
        return Err("The compose file has no services".to_string());
    };
    for pin in pins {
        let Some(Value::Mapping(svc)) = services
            .get_mut(Value::String(pin.service.clone()))
            .map(untag_mut)
        else {
            continue;
        };
        if let Some(image) = &pin.image {
            svc.insert("image".into(), Value::String(image.clone()));
            svc.remove("build");
            svc.insert("pull_policy".into(), "never".into());
        }
        if let Some((host, container)) = pin.publish {
            let ports = svc
                .entry("ports".into())
                .or_insert_with(|| Value::Sequence(Vec::new()));
            if !matches!(untag(ports), Value::Sequence(_)) {
                *ports = Value::Sequence(Vec::new());
            }
            if let Value::Sequence(seq) = untag_mut(ports) {
                seq.push(Value::String(format!("127.0.0.1:{host}:{container}")));
            }
        }
    }
    serde_yaml::to_string(&doc).map_err(|e| format!("Failed to write the compose file: {e}"))
}

// ── Discovery ────────────────────────────────────────────────────────────────

/// What Docker reports about one service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub service: String,
    pub container_id: String,
    /// "running" | "restarting" | "paused" | "created" | "exited" | "dead" | …
    pub state: String,
    pub health: Option<String>,
    pub exit_code: Option<i64>,
    /// Image ref the container was created from
    pub image: Option<String>,
    pub image_id: Option<String>,
}

/// Health from a `docker ps` status string: "Up 3 minutes (healthy)".
pub fn health_from_status(status: &str) -> Option<String> {
    let lower = status.to_ascii_lowercase();
    if lower.contains("(unhealthy)") {
        Some("unhealthy".into())
    } else if lower.contains("(healthy)") {
        Some("healthy".into())
    } else if lower.contains("health: starting") {
        Some("starting".into())
    } else {
        None
    }
}

/// Exit code from a `docker ps` status string: "Exited (137) 2 hours ago".
pub fn exit_code_from_status(status: &str) -> Option<i64> {
    let rest = status.trim().strip_prefix("Exited (")?;
    rest.split_once(')')?.0.trim().parse().ok()
}

fn project_filter(project: &str, service: Option<&str>) -> HashMap<String, Vec<String>> {
    let mut labels = vec![format!("{PROJECT_LABEL}={project}")];
    if let Some(s) = service {
        labels.push(format!("{SERVICE_LABEL}={s}"));
    }
    HashMap::from([("label".to_string(), labels)])
}

/// Every container of a compose project (running or not), one per service
/// (a running replica wins over a stopped one).
pub async fn discover(docker: &Docker, project: &str) -> Result<Vec<ServiceState>> {
    let filters = project_filter(project, None);
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await?;
    let mut by_service: BTreeMap<String, ServiceState> = BTreeMap::new();
    for c in containers {
        let labels = c.labels.unwrap_or_default();
        let (Some(service), Some(id)) = (labels.get(SERVICE_LABEL).cloned(), c.id) else {
            continue;
        };
        let status = c.status.unwrap_or_default();
        let state = c
            .state
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "created".to_string());
        let health = match c.health.and_then(|h| h.status) {
            Some(ContainerSummaryHealthStatusEnum::HEALTHY) => Some("healthy".to_string()),
            Some(ContainerSummaryHealthStatusEnum::UNHEALTHY) => Some("unhealthy".to_string()),
            Some(ContainerSummaryHealthStatusEnum::STARTING) => Some("starting".to_string()),
            _ => health_from_status(&status),
        };
        let st = ServiceState {
            exit_code: exit_code_from_status(&status),
            service: service.clone(),
            container_id: id,
            state,
            health,
            image: c.image,
            image_id: c.image_id,
        };
        let replace = match by_service.get(&service) {
            None => true,
            Some(prev) => prev.state != "running" && st.state == "running",
        };
        if replace {
            by_service.insert(service, st);
        }
    }
    Ok(by_service.into_values().collect())
}

/// Container ids of one service (all replicas, running or not).
pub async fn service_containers(docker: &Docker, project: &str, service: &str) -> Vec<String> {
    let filters = project_filter(project, Some(service));
    docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await
        .map(|cs| cs.into_iter().filter_map(|c| c.id).collect())
        .unwrap_or_default()
}

/// Whether a container state is a finished one-shot run (exit 0).
pub fn is_completed(state: &str, exit_code: Option<i64>) -> bool {
    state == "exited" && exit_code == Some(0)
}

// ── Rows ─────────────────────────────────────────────────────────────────────

/// One compose_services row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ServiceRow {
    pub name: String,
    pub image: Option<String>,
    pub builds: i64,
    pub ports: String,
    pub has_healthcheck: i64,
    pub published_port: Option<i64>,
    pub host_port: Option<i64>,
    pub routed_port: Option<i64>,
    pub container_id: Option<String>,
    pub status: String,
    pub health: Option<String>,
    pub exit_code: Option<i64>,
    pub running_image: Option<String>,
    pub image_digest: Option<String>,
}

impl ServiceRow {
    pub fn declared_ports(&self) -> Vec<u16> {
        serde_json::from_str(&self.ports).unwrap_or_default()
    }
}

/// The app's services in file order.
pub async fn rows(db: &sqlx::SqlitePool, app_id: &str) -> sqlx::Result<Vec<ServiceRow>> {
    sqlx::query_as("SELECT * FROM compose_services WHERE app_id = ? ORDER BY position, name")
        .bind(app_id)
        .fetch_all(db)
        .await
}

/// Upsert the services of the file and delete the ones that left it (with
/// their domains). User choices (published port) are kept.
pub async fn sync_specs(
    db: &sqlx::SqlitePool,
    app_id: &str,
    specs: &[ComposeServiceSpec],
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT name FROM compose_services WHERE app_id = ?")
            .bind(app_id)
            .fetch_all(&mut *tx)
            .await?;
    for gone in existing
        .iter()
        .filter(|n| !specs.iter().any(|s| &s.name == *n))
    {
        sqlx::query("DELETE FROM app_domains WHERE app_id = ? AND service = ?")
            .bind(app_id)
            .bind(gone)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM compose_services WHERE app_id = ? AND name = ?")
            .bind(app_id)
            .bind(gone)
            .execute(&mut *tx)
            .await?;
    }
    for (position, spec) in specs.iter().enumerate() {
        let ports = serde_json::to_string(&spec.ports).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO compose_services (app_id, name, position, image, builds, ports, has_healthcheck)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(app_id, name) DO UPDATE SET position = excluded.position,
               image = excluded.image, builds = excluded.builds, ports = excluded.ports,
               has_healthcheck = excluded.has_healthcheck, updated_at = datetime('now')",
        )
        .bind(app_id)
        .bind(&spec.name)
        .bind(position as i64)
        .bind(&spec.image)
        .bind(spec.builds as i64)
        .bind(ports)
        .bind(spec.has_healthcheck as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Write what Docker reports into the rows. Services without a container are
/// "missing" (never started, or removed behind Tond's back).
pub async fn store_states(
    db: &sqlx::SqlitePool,
    app_id: &str,
    states: &[ServiceState],
) -> sqlx::Result<()> {
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM compose_services WHERE app_id = ?")
            .bind(app_id)
            .fetch_all(db)
            .await?;
    let mut tx = db.begin().await?;
    for name in names {
        let st = states.iter().find(|s| s.service == name);
        sqlx::query(
            "UPDATE compose_services SET container_id = ?, status = ?, health = ?, exit_code = ?,
               updated_at = datetime('now')
             WHERE app_id = ? AND name = ?
               AND (container_id IS NOT ? OR status IS NOT ? OR health IS NOT ? OR exit_code IS NOT ?)",
        )
        .bind(st.map(|s| s.container_id.as_str()))
        .bind(st.map_or("missing", |s| s.state.as_str()))
        .bind(st.and_then(|s| s.health.as_deref()))
        .bind(st.and_then(|s| s.exit_code))
        .bind(app_id)
        .bind(&name)
        .bind(st.map(|s| s.container_id.as_str()))
        .bind(st.map_or("missing", |s| s.state.as_str()))
        .bind(st.and_then(|s| s.health.as_deref()))
        .bind(st.and_then(|s| s.exit_code))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Ask Docker for the app's services and store the result.
pub async fn refresh(state: &crate::state::AppState, app_id: &str, app_name: &str) -> Result<()> {
    let project = crate::deploy::compose_project_name(app_name);
    let states = discover(&state.docker, &project).await?;
    store_states(&state.db, app_id, &states).await?;
    Ok(())
}

/// Name of the app that uses `domain` (its own domain or a service domain).
pub async fn domain_owner(
    db: &sqlx::SqlitePool,
    domain: &str,
) -> sqlx::Result<Option<(String, String)>> {
    sqlx::query_as(
        "SELECT id, name FROM apps WHERE domain = ?
         UNION ALL
         SELECT a.id, a.name FROM app_domains d JOIN apps a ON a.id = d.app_id WHERE d.domain = ?
         LIMIT 1",
    )
    .bind(domain)
    .bind(domain)
    .fetch_optional(db)
    .await
}

/// Create (or reset) the pending certificate row of a newly routed domain;
/// tond-proxy issues it (see the Certificates page for its status).
pub async fn queue_certificate(db: &sqlx::SqlitePool, domain: &str) {
    let data_dir = crate::volumes::data_dir();
    let certs_dir = data_dir.join("certs").join(domain);
    let now = chrono::Utc::now().to_rfc3339();
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(90)).to_rfc3339();
    let _ = sqlx::query(
        "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at, status, created_at, updated_at)
         VALUES (?, ?, 'acme', ?, ?, ?, ?, 'pending', ?, ?)
         ON CONFLICT(domain) DO UPDATE SET status = 'pending', updated_at = excluded.updated_at,
            last_error = NULL, next_retry_at = NULL, failure_count = 0",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(domain)
    .bind(certs_dir.join("cert.pem").to_string_lossy().into_owned())
    .bind(certs_dir.join("key.pem").to_string_lossy().into_owned())
    .bind(&now)
    .bind(&expires_at)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await;
}

/// The first domain of each compose app (its first service with a domain,
/// in file order): `app id -> domain`.
pub async fn primary_domains(db: &sqlx::SqlitePool) -> sqlx::Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT d.app_id, d.domain FROM app_domains d
         JOIN compose_services s ON s.app_id = d.app_id AND s.name = d.service
         ORDER BY s.position DESC, d.created_at DESC, d.domain DESC",
    )
    .fetch_all(db)
    .await?;
    // Later rows overwrite earlier ones: the first in order wins.
    Ok(rows.into_iter().collect())
}

/// Domains routed to each service of an app.
pub async fn domains_by_service(
    db: &sqlx::SqlitePool,
    app_id: &str,
) -> sqlx::Result<HashMap<String, Vec<String>>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT service, domain FROM app_domains
         WHERE app_id = ? AND service IS NOT NULL ORDER BY created_at, domain",
    )
    .bind(app_id)
    .fetch_all(db)
    .await?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (service, domain) in rows {
        map.entry(service).or_default().push(domain);
    }
    Ok(map)
}

#[cfg(test)]
mod tests;
