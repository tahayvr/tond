// compose_policy.rs — validate and harden user-supplied docker-compose files
//
// A compose file is a full container spec: `privileged: true`, a bind of
// `/var/run/docker.sock`, `pid: host` or `cap_add: [SYS_ADMIN]` each give the
// author root on the VPS. Tond therefore parses every compose file before it
// is stored or deployed and:
//
//   1. Rejects host-level options with a message naming the service, the
//      option and what to do instead (`validate`). All problems are reported
//      at once so the user can fix the file in one go.
//   2. Rewrites the file it hands to `docker compose` (`harden`):
//        - bind mounts point at absolute paths inside the app's volume
//          directory (`./data` → `<data>/volumes/<app>/data`)
//        - published ports bind to 127.0.0.1 (Docker's iptables rules bypass
//          UFW, so a 0.0.0.0 port is public no matter what the firewall says)
//        - every service gets no-new-privileges, the trimmed capability set,
//          a pids limit, a default memory limit and rotated json-file logs —
//          the same policy as containers Tond creates itself (docker.rs).
//
// The stored compose_content stays exactly as the user wrote it; only the file
// written next to the deploy is transformed.
//
// Variable substitution: compose interpolates `${VAR}` from the environment
// and `.env` *after* we validate, so `volumes: ["${X:-/}:/host"]` would slip
// past a literal check. Substitution is therefore refused in every
// security-relevant field, and `docker compose` runs with a cleared
// environment (see deploy/build.rs) so values like `${TOND_SECRET_KEY}` can't
// leak into containers. `environment:`, `image:`, `command:` etc. may still use
// `${VAR:-default}`; the variables come from the app's own env, and only the
// ones the file references (`interpolation_env`) — never names that steer the
// docker CLI itself (DOCKER_HOST, COMPOSE_*, LD_PRELOAD, PATH, …).

use std::path::{Component, Path, PathBuf};

use serde_yaml::{Mapping, Value};

use crate::docker;

/// Linux capabilities in Docker's default set. `cap_add` may only re-add these
/// (e.g. NET_RAW for ping); anything else (SYS_ADMIN, NET_ADMIN, ALL, …) is refused.
const DOCKER_DEFAULT_CAPS: &[&str] = &[
    "AUDIT_WRITE",
    "CHOWN",
    "DAC_OVERRIDE",
    "FOWNER",
    "FSETID",
    "KILL",
    "MKNOD",
    "NET_BIND_SERVICE",
    "NET_RAW",
    "SETFCAP",
    "SETGID",
    "SETPCAP",
    "SETUID",
    "SYS_CHROOT",
];

/// Result of hardening a compose file.
#[derive(Debug)]
pub struct HardenedCompose {
    /// The rewritten compose file to pass to `docker compose -f`.
    pub yaml: String,
    /// Bind sources relative to the app's volume directory; the caller must
    /// create/verify each with `volumes::prepare_host_dir` before `up`.
    pub bind_sources: Vec<PathBuf>,
}

struct Ctx<'a> {
    /// `<data>/volumes/<app>` — the only host directory binds may use.
    volume_dir: &'a Path,
    errors: Vec<String>,
    bind_sources: Vec<PathBuf>,
}

impl Ctx<'_> {
    fn err(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }
}

/// Validate a compose file. `Err` holds one human-readable line per problem.
pub fn validate(content: &str, volume_dir: &Path) -> Result<(), Vec<String>> {
    harden(content, volume_dir, docker::default_memory_limit()).map(|_| ())
}

/// Validate and rewrite a compose file (see module docs).
pub fn harden(
    content: &str,
    volume_dir: &Path,
    default_memory: i64,
) -> Result<HardenedCompose, Vec<String>> {
    let mut doc: Value = serde_yaml::from_str(content)
        .map_err(|e| vec![format!("The compose file is not valid YAML: {e}")])?;
    doc.apply_merge().map_err(|e| {
        vec![format!(
            "The compose file has an invalid '<<' merge key: {e}"
        )]
    })?;

    let mut ctx = Ctx {
        volume_dir,
        errors: Vec::new(),
        bind_sources: Vec::new(),
    };

    let Some(root) = untag_mut(&mut doc).as_mapping_mut() else {
        return Err(vec![
            "The compose file must be a YAML mapping with a top-level 'services:' key".to_string(),
        ]);
    };

    if root.contains_key("include") {
        ctx.err("'include' is not allowed: Tond deploys a single self-contained compose file");
    }
    check_top_level_volumes(root, &mut ctx);
    check_top_level_networks(root, &mut ctx);
    check_file_references(root, "secrets", &mut ctx);
    check_file_references(root, "configs", &mut ctx);

    match root.get_mut("services").map(untag_mut) {
        Some(Value::Mapping(services)) if !services.is_empty() => {
            for (name, service) in services.iter_mut() {
                let name = key_str(name);
                match untag_mut(service) {
                    Value::Mapping(svc) => check_service(&name, svc, default_memory, &mut ctx),
                    Value::Null => ctx.err(format!(
                        "service \"{name}\": is empty; give it an image or build"
                    )),
                    _ => ctx.err(format!("service \"{name}\": must be a mapping")),
                }
            }
        }
        _ => ctx.err("The compose file must define at least one service under 'services:'"),
    }

    if !ctx.errors.is_empty() {
        return Err(ctx.errors);
    }
    let yaml = serde_yaml::to_string(&doc)
        .map_err(|e| vec![format!("Failed to write the hardened compose file: {e}")])?;
    Ok(HardenedCompose {
        yaml,
        bind_sources: ctx.bind_sources,
    })
}

/// Format validation errors as one message for an API response.
pub fn format_errors(errors: &[String]) -> String {
    if errors.len() == 1 {
        return format!("Compose file rejected: {}", errors[0]);
    }
    let mut msg = format!("Compose file rejected ({} problems):", errors.len());
    for e in errors {
        msg.push_str("\n- ");
        msg.push_str(e);
    }
    msg
}

/// The variables `docker compose` may interpolate from: the app env vars the
/// file references, minus names that change how the docker CLI runs
/// (`tond_common::is_reserved_env_key`). Nothing else from the app env is
/// handed to the host process.
pub fn interpolation_env(
    content: &str,
    app_env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let Ok(referenced) = tond_common::compose_variables(content) else {
        return Vec::new();
    };
    referenced
        .into_iter()
        .filter(|key| !tond_common::is_reserved_env_key(key))
        .filter_map(|key| app_env.get(&key).map(|v| (key, v.clone())))
        .collect()
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn untag_mut(v: &mut Value) -> &mut Value {
    let mut v = v;
    while let Value::Tagged(t) = v {
        v = &mut t.value;
    }
    v
}

fn untag(v: &Value) -> &Value {
    let mut v = v;
    while let Value::Tagged(t) = v {
        v = &t.value;
    }
    v
}

fn key_str(v: &Value) -> String {
    match untag(v) {
        Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// Scalar as string (numbers and bools stringified), None for mappings/sequences/null.
fn scalar_str(v: &Value) -> Option<String> {
    match untag(v) {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// True when `s` uses compose variable substitution (`$VAR`, `${VAR}`); `$$` is a literal `$`.
fn has_interpolation(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            if b.get(i + 1) == Some(&b'$') {
                i += 2;
                continue;
            }
            return true;
        }
        i += 1;
    }
    false
}

/// Refuse substitution in a security-relevant field; returns false if refused.
fn no_interpolation(ctx: &mut Ctx, service: &str, field: &str, value: &str) -> bool {
    if has_interpolation(value) {
        ctx.err(format!(
            "service \"{service}\": variable substitution ('$') is not allowed in '{field}'; write the value literally"
        ));
        return false;
    }
    true
}

/// Items of a list field (sequence of scalars); a single scalar counts as one item.
fn list_items(v: &Value) -> Vec<String> {
    match untag(v) {
        Value::Sequence(seq) => seq.iter().filter_map(scalar_str).collect(),
        other => scalar_str(other).into_iter().collect(),
    }
}

/// Mode-like fields (`pid`, `ipc`, `network_mode`, …) that may not join the
/// host or another (non-project) container.
fn check_namespace_mode(svc: &Mapping, service: &str, field: &str, what: &str, ctx: &mut Ctx) {
    let Some(v) = svc.get(field) else { return };
    let Some(mode) = scalar_str(v) else {
        ctx.err(format!("service \"{service}\": '{field}' must be a string"));
        return;
    };
    if !no_interpolation(ctx, service, field, &mode) {
        return;
    }
    let lower = mode.trim().to_ascii_lowercase();
    if lower == "host" {
        ctx.err(format!(
            "service \"{service}\": '{field}: host' is not allowed — it shares the host's {what} with the container"
        ));
    } else if field == "network_mode" && (lower == "bridge" || lower == "default") {
        ctx.err(format!(
            "service \"{service}\": 'network_mode: {mode}' is not allowed — Docker's default bridge is shared with other containers; leave network_mode out to use this app's own network"
        ));
    } else if lower.starts_with("container:") {
        ctx.err(format!(
            "service \"{service}\": '{field}: {mode}' is not allowed — it joins a container outside this app; use 'service:<name>' for services in this file"
        ));
    }
}

// ── services ─────────────────────────────────────────────────────────────────

fn check_service(name: &str, svc: &mut Mapping, default_memory: i64, ctx: &mut Ctx) {
    // privileged: anything but an explicit false/absent is refused (a string
    // like "${PRIV:-true}" would become true after substitution).
    if let Some(v) = svc.get("privileged")
        && !matches!(untag(v), Value::Bool(false) | Value::Null)
    {
        ctx.err(format!(
            "service \"{name}\": 'privileged' is not allowed — privileged containers have full root access to the host"
        ));
    }

    for field in ["devices", "device_cgroup_rules", "gpus"] {
        if svc.contains_key(field) {
            ctx.err(format!(
                "service \"{name}\": '{field}' is not allowed — host devices can't be passed into containers"
            ));
        }
    }
    if let Some(deploy) = svc.get("deploy").map(untag)
        && let Some(res) = deploy.get("resources").map(untag)
        && let Some(resv) = res.get("reservations").map(untag)
        && resv.get("devices").is_some()
    {
        ctx.err(format!(
            "service \"{name}\": 'deploy.resources.reservations.devices' is not allowed — host devices can't be passed into containers"
        ));
    }

    check_namespace_mode(svc, name, "pid", "process namespace", ctx);
    check_namespace_mode(svc, name, "ipc", "IPC namespace", ctx);
    check_namespace_mode(svc, name, "network_mode", "network stack", ctx);
    check_namespace_mode(svc, name, "uts", "hostname namespace", ctx);
    check_namespace_mode(svc, name, "userns_mode", "user namespace", ctx);
    check_namespace_mode(svc, name, "cgroup", "cgroup namespace", ctx);
    if svc.contains_key("cgroup_parent") {
        ctx.err(format!(
            "service \"{name}\": 'cgroup_parent' is not allowed — Tond manages container cgroups"
        ));
    }

    // cap_add: only capabilities Docker grants by default.
    let mut added_caps: Vec<String> = Vec::new();
    if let Some(v) = svc.get("cap_add") {
        for cap in list_items(v) {
            if !no_interpolation(ctx, name, "cap_add", &cap) {
                continue;
            }
            let norm = cap.trim().to_ascii_uppercase();
            let norm = norm.strip_prefix("CAP_").unwrap_or(&norm).to_string();
            if DOCKER_DEFAULT_CAPS.contains(&norm.as_str()) {
                added_caps.push(norm);
            } else {
                ctx.err(format!(
                    "service \"{name}\": 'cap_add: {cap}' is not allowed — only capabilities in Docker's default set can be added"
                ));
            }
        }
    }

    // security_opt: nothing that switches confinement off.
    if let Some(v) = svc.get("security_opt") {
        for opt in list_items(v) {
            if !no_interpolation(ctx, name, "security_opt", &opt) {
                continue;
            }
            let lower = opt.to_ascii_lowercase().replace('=', ":");
            if lower.contains("unconfined")
                || lower.starts_with("label:disable")
                || lower.starts_with("no-new-privileges:false")
            {
                ctx.err(format!(
                    "service \"{name}\": 'security_opt: {opt}' is not allowed — it disables container confinement"
                ));
            }
        }
    }

    // volumes_from: only other services in this file.
    if let Some(v) = svc.get("volumes_from") {
        for src in list_items(v) {
            if src.trim().to_ascii_lowercase().starts_with("container:") {
                ctx.err(format!(
                    "service \"{name}\": 'volumes_from: {src}' is not allowed — it mounts volumes of a container outside this app"
                ));
            }
        }
    }

    if svc.contains_key("env_file") {
        ctx.err(format!(
            "service \"{name}\": 'env_file' is not supported for inline compose apps — put the variables under 'environment:'"
        ));
    }

    if let Some(ext) = svc.get("extends").map(untag)
        && ext.get("file").is_some()
    {
        ctx.err(format!(
            "service \"{name}\": 'extends.file' is not allowed — extend services from this same file instead"
        ));
    }

    if let Some(build) = svc.get("build") {
        check_build(name, build, ctx);
    }

    if let Some(v) = svc.get_mut("volumes") {
        check_service_volumes(name, v, ctx);
    }

    if let Some(v) = svc.get_mut("ports") {
        rewrite_ports(name, v, ctx);
    }
    check_replicas_ports(name, svc, ctx);

    if ctx.errors.is_empty() {
        apply_hardening(svc, &added_caps, default_memory);
    }
}

fn check_build(service: &str, build: &Value, ctx: &mut Ctx) {
    let check_ctx_path = |ctx: &mut Ctx, field: &str, value: &str| {
        if !no_interpolation(ctx, service, field, value) {
            return;
        }
        let lower = value.to_ascii_lowercase();
        let remote = [
            "http://",
            "https://",
            "git://",
            "git@",
            "ssh://",
            "github.com/",
        ]
        .iter()
        .any(|p| lower.starts_with(p));
        if remote {
            return;
        }
        let path = Path::new(value);
        if path.is_absolute()
            || value.starts_with('~')
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            ctx.err(format!(
                "service \"{service}\": '{field}: {value}' is not allowed — build contexts must be a git URL or a path inside the compose directory"
            ));
        }
    };
    match untag(build) {
        Value::Mapping(b) => {
            if let Some(c) = b.get("context").and_then(scalar_str) {
                check_ctx_path(ctx, "build.context", &c);
            }
            if let Some(d) = b.get("dockerfile").and_then(scalar_str) {
                check_ctx_path(ctx, "build.dockerfile", &d);
            }
            if let Some(Value::Mapping(extra)) = b.get("additional_contexts").map(untag) {
                for (_, v) in extra {
                    if let Some(p) = scalar_str(v)
                        && !p.starts_with("docker-image://")
                        && !p.starts_with("service:")
                    {
                        check_ctx_path(ctx, "build.additional_contexts", &p);
                    }
                }
            }
            for field in ["secrets", "ssh"] {
                if b.contains_key(field) {
                    ctx.err(format!(
                        "service \"{service}\": 'build.{field}' is not allowed — it reads files or agents from the host"
                    ));
                }
            }
        }
        other => {
            if let Some(c) = scalar_str(other) {
                check_ctx_path(ctx, "build", &c);
            }
        }
    }
}

/// Classify a short-syntax volume source: bind if it looks like a path.
fn is_path_like(src: &str) -> bool {
    src.starts_with('/') || src.starts_with('.') || src.starts_with('~')
}

/// Resolve a bind source to a path relative to the app's volume directory.
fn resolve_bind(ctx: &mut Ctx, service: &str, src: &str) -> Option<PathBuf> {
    let reject = |ctx: &mut Ctx| {
        ctx.err(format!(
            "service \"{service}\": bind mount '{src}' is outside this app's volume directory — use a relative path such as ./data (stored in {}/data) or a named volume",
            ctx.volume_dir.display()
        ));
        None
    };
    if !no_interpolation(ctx, service, "volumes", src) {
        return None;
    }
    let path = Path::new(src);
    let rel: PathBuf = if path.is_absolute() {
        match path.strip_prefix(ctx.volume_dir) {
            Ok(rest) => rest.to_path_buf(),
            Err(_) => return reject(ctx),
        }
    } else if src.starts_with('~') {
        return reject(ctx);
    } else {
        path.to_path_buf()
    };
    let mut clean = PathBuf::new();
    for c in rel.components() {
        match c {
            Component::Normal(p) => clean.push(p),
            Component::CurDir => {}
            _ => return reject(ctx),
        }
    }
    if clean.as_os_str().is_empty() {
        ctx.err(format!(
            "service \"{service}\": bind mount '{src}' mounts the whole volume directory — mount a subdirectory such as ./data instead"
        ));
        return None;
    }
    Some(clean)
}

fn check_service_volumes(service: &str, volumes: &mut Value, ctx: &mut Ctx) {
    let Value::Sequence(items) = untag_mut(volumes) else {
        ctx.err(format!("service \"{service}\": 'volumes' must be a list"));
        return;
    };
    for item in items.iter_mut() {
        match untag_mut(item) {
            Value::String(spec) => {
                let parts: Vec<&str> = spec.split(':').collect();
                if parts.len() < 2 {
                    // anonymous volume ("/data"): no host path involved
                    no_interpolation(ctx, service, "volumes", spec);
                    continue;
                }
                let src = parts[0];
                if !is_path_like(src) {
                    // named volume
                    no_interpolation(ctx, service, "volumes", spec);
                    continue;
                }
                if let Some(rel) = resolve_bind(ctx, service, src) {
                    let abs = ctx.volume_dir.join(&rel);
                    let mut rewritten = abs.to_string_lossy().into_owned();
                    for p in &parts[1..] {
                        rewritten.push(':');
                        rewritten.push_str(p);
                    }
                    ctx.bind_sources.push(rel);
                    *spec = rewritten;
                }
            }
            Value::Mapping(m) => {
                let typ = m
                    .get("type")
                    .and_then(scalar_str)
                    .unwrap_or_else(|| "volume".to_string());
                match typ.as_str() {
                    "bind" => {
                        let Some(src) = m.get("source").and_then(scalar_str) else {
                            ctx.err(format!(
                                "service \"{service}\": bind mount is missing 'source'"
                            ));
                            continue;
                        };
                        if let Some(rel) = resolve_bind(ctx, service, &src) {
                            let abs = ctx.volume_dir.join(&rel);
                            m.insert(
                                Value::String("source".into()),
                                Value::String(abs.to_string_lossy().into_owned()),
                            );
                            ctx.bind_sources.push(rel);
                        }
                    }
                    "volume" | "tmpfs" | "image" => {
                        if let Some(src) = m.get("source").and_then(scalar_str) {
                            no_interpolation(ctx, service, "volumes", &src);
                        }
                    }
                    other => ctx.err(format!(
                        "service \"{service}\": volume type '{other}' is not allowed"
                    )),
                }
            }
            _ => ctx.err(format!(
                "service \"{service}\": each 'volumes' entry must be a string or a mapping"
            )),
        }
    }
}

fn is_loopback(ip: &str) -> bool {
    let ip = ip.trim_start_matches('[').trim_end_matches(']');
    ip == "localhost"
        || ip
            .parse::<std::net::IpAddr>()
            .is_ok_and(|a| a.is_loopback())
}

/// Rewrite `ports:` so every published port binds to 127.0.0.1.
fn rewrite_ports(service: &str, ports: &mut Value, ctx: &mut Ctx) {
    let Value::Sequence(items) = untag_mut(ports) else {
        ctx.err(format!("service \"{service}\": 'ports' must be a list"));
        return;
    };
    let public_ip_err = |ctx: &mut Ctx, spec: &str| {
        ctx.err(format!(
            "service \"{service}\": port '{spec}' binds a public interface — Tond publishes ports on 127.0.0.1 and routes HTTP through its proxy; drop the IP (e.g. \"8080:80\")"
        ));
    };
    for item in items.iter_mut() {
        let item = untag_mut(item);
        match item {
            Value::Mapping(m) => {
                if let Some(ip) = m.get("host_ip").and_then(scalar_str) {
                    if !no_interpolation(ctx, service, "ports", &ip) {
                        continue;
                    }
                    if !is_loopback(&ip) {
                        public_ip_err(ctx, &ip);
                        continue;
                    }
                } else {
                    m.insert(
                        Value::String("host_ip".into()),
                        Value::String("127.0.0.1".into()),
                    );
                }
            }
            Value::String(_) | Value::Number(_) => {
                let spec = scalar_str(item).unwrap_or_default();
                if !no_interpolation(ctx, service, "ports", &spec) {
                    continue;
                }
                // Split off an optional IP: "[::1]:8080:80", "1.2.3.4:8080:80".
                let (ip, rest) = if let Some(stripped) = spec.strip_prefix('[') {
                    match stripped.split_once("]:") {
                        Some((ip, rest)) => (Some(ip.to_string()), rest.to_string()),
                        None => {
                            ctx.err(format!("service \"{service}\": invalid port '{spec}'"));
                            continue;
                        }
                    }
                } else {
                    let parts: Vec<&str> = spec.split(':').collect();
                    match parts.len() {
                        1 | 2 => (None, spec.clone()),
                        3 => (
                            Some(parts[0].to_string()),
                            format!("{}:{}", parts[1], parts[2]),
                        ),
                        _ => {
                            ctx.err(format!("service \"{service}\": invalid port '{spec}'"));
                            continue;
                        }
                    }
                };
                match ip {
                    Some(ip) if !ip.is_empty() && !is_loopback(&ip) => public_ip_err(ctx, &spec),
                    Some(ip) if !ip.is_empty() => {} // already loopback
                    _ => {
                        // "80" → "127.0.0.1::80" (ephemeral host port);
                        // "8080:80" → "127.0.0.1:8080:80"
                        let rewritten = if rest.contains(':') {
                            format!("127.0.0.1:{rest}")
                        } else {
                            format!("127.0.0.1::{rest}")
                        };
                        *item = Value::String(rewritten);
                    }
                }
            }
            _ => ctx.err(format!(
                "service \"{service}\": each 'ports' entry must be a string or a mapping"
            )),
        }
    }
}

/// More than one replica can't share a fixed host port: the second replica
/// fails with "port is already allocated" half-way through `up`. Refused up
/// front; an ephemeral port ("80") or a range sized for the replicas is fine.
fn check_replicas_ports(service: &str, svc: &Mapping, ctx: &mut Ctx) {
    let replicas = crate::compose::replicas(svc, &std::collections::BTreeMap::new()).unwrap_or(1);
    if replicas <= 1 {
        return;
    }
    let Some(Value::Sequence(items)) = svc.get("ports").map(untag) else {
        return;
    };
    for item in items {
        if let Some(host) = fixed_host_port(untag(item)) {
            ctx.err(format!(
                "service \"{service}\": runs {replicas} replicas but publishes the fixed host port {host} — every replica would need that same port. Publish only the container port (e.g. \"80\") so each replica gets its own, or run one replica (Tond routes domains to one container per service)"
            ));
            return;
        }
    }
}

/// The single fixed host port of a `ports:` entry, if it has one
/// ("127.0.0.1:8080:80" → 8080; "127.0.0.1::80", ranges and `{target: 80}` → None).
fn fixed_host_port(item: &Value) -> Option<String> {
    let host = match item {
        Value::Mapping(m) => m.get("published").and_then(scalar_str)?,
        _ => {
            let spec = scalar_str(item)?;
            let spec = spec.split('/').next().unwrap_or_default().to_string();
            let rest = match spec.strip_prefix('[') {
                Some(s) => s.split_once("]:").map(|(_, r)| r.to_string())?,
                None => spec.clone(),
            };
            let parts: Vec<&str> = rest.split(':').collect();
            match parts.as_slice() {
                [_, host, _] | [host, _] => (*host).to_string(),
                _ => return None,
            }
        }
    };
    let host = host.trim();
    (!host.is_empty() && !host.contains('-')).then(|| host.to_string())
}

/// Add Tond's container policy to a service (docs in docker.rs).
fn apply_hardening(svc: &mut Mapping, added_caps: &[String], default_memory: i64) {
    // security_opt += no-new-privileges:true
    let opts = svc
        .entry(Value::String("security_opt".into()))
        .or_insert_with(|| Value::Sequence(Vec::new()));
    if !matches!(untag(opts), Value::Sequence(_)) {
        *opts = Value::Sequence(Vec::new());
    }
    if let Value::Sequence(seq) = untag_mut(opts)
        && !seq
            .iter()
            .filter_map(scalar_str)
            .any(|o| o.starts_with("no-new-privileges"))
    {
        seq.push(Value::String("no-new-privileges:true".into()));
    }

    // cap_drop += policy caps, except ones the user explicitly re-added.
    let drops = svc
        .entry(Value::String("cap_drop".into()))
        .or_insert_with(|| Value::Sequence(Vec::new()));
    if !matches!(untag(drops), Value::Sequence(_)) {
        *drops = Value::Sequence(Vec::new());
    }
    if let Value::Sequence(seq) = untag_mut(drops) {
        for cap in docker::DROPPED_CAPABILITIES {
            let present = seq
                .iter()
                .filter_map(scalar_str)
                .any(|c| c.eq_ignore_ascii_case(cap));
            if !present && !added_caps.iter().any(|c| c == cap) {
                seq.push(Value::String((*cap).to_string()));
            }
        }
    }

    let has_positive = |v: Option<&Value>| {
        v.and_then(scalar_str)
            .is_some_and(|s| !s.trim().is_empty() && s.trim() != "0" && s.trim() != "-1")
    };

    if !has_positive(svc.get("pids_limit")) {
        let limit_in_deploy = svc
            .get("deploy")
            .and_then(|d| untag(d).get("resources"))
            .and_then(|r| untag(r).get("limits"))
            .is_some_and(|l| has_positive(untag(l).get("pids")));
        if !limit_in_deploy {
            svc.insert(
                Value::String("pids_limit".into()),
                Value::Number(docker::PIDS_LIMIT.into()),
            );
        }
    }

    if !has_positive(svc.get("mem_limit")) {
        let limit_in_deploy = svc
            .get("deploy")
            .and_then(|d| untag(d).get("resources"))
            .and_then(|r| untag(r).get("limits"))
            .is_some_and(|l| has_positive(untag(l).get("memory")));
        if !limit_in_deploy {
            svc.insert(
                Value::String("mem_limit".into()),
                Value::Number(default_memory.into()),
            );
        }
    }

    // Logging: keep an explicit driver choice, but make sure json-file/local
    // logs rotate.
    let default_logging = || {
        let mut options = Mapping::new();
        options.insert("max-size".into(), docker::LOG_MAX_SIZE.into());
        options.insert("max-file".into(), docker::LOG_MAX_FILE.into());
        let mut logging = Mapping::new();
        logging.insert("driver".into(), docker::LOG_DRIVER.into());
        logging.insert("options".into(), Value::Mapping(options));
        Value::Mapping(logging)
    };
    match svc.get_mut("logging").map(untag_mut) {
        Some(Value::Mapping(logging)) => {
            let driver = logging
                .get("driver")
                .and_then(scalar_str)
                .unwrap_or_else(|| docker::LOG_DRIVER.to_string());
            if driver == "json-file" || driver == "local" {
                let options = logging
                    .entry("options".into())
                    .or_insert_with(|| Value::Mapping(Mapping::new()));
                if let Value::Mapping(o) = untag_mut(options) {
                    o.entry("max-size".into())
                        .or_insert_with(|| docker::LOG_MAX_SIZE.into());
                    o.entry("max-file".into())
                        .or_insert_with(|| docker::LOG_MAX_FILE.into());
                }
            }
        }
        _ => {
            svc.insert("logging".into(), default_logging());
        }
    }
}

// ── top-level sections ───────────────────────────────────────────────────────

fn check_top_level_volumes(root: &Mapping, ctx: &mut Ctx) {
    let Some(Value::Mapping(vols)) = root.get("volumes").map(untag) else {
        return;
    };
    for (name, def) in vols {
        let name = key_str(name);
        let def = untag(def);
        // A named volume is private to this app (compose names it
        // <app>_<volume>, and snapshots back it up). `external` or a fixed
        // `name` would mount another app's volume.
        if def
            .get("external")
            .is_some_and(|v| !matches!(untag(v), Value::Bool(false) | Value::Null))
        {
            ctx.err(format!(
                "volume \"{name}\": 'external' is not allowed — it mounts a volume outside this app; leave it out and compose creates the volume for this app"
            ));
        }
        if def.get("name").is_some() {
            ctx.err(format!(
                "volume \"{name}\": 'name' is not allowed — a fixed name can mount another app's volume; leave it out and compose names the volume for this app"
            ));
        }
        if let Some(driver) = def.get("driver").and_then(scalar_str)
            && !driver.trim().eq_ignore_ascii_case("local")
        {
            ctx.err(format!(
                "volume \"{name}\": 'driver: {driver}' is not allowed — only local volumes can be used (Tond backs them up with the app)"
            ));
        }
        let Some(opts) = def.get("driver_opts").map(untag) else {
            continue;
        };
        let Value::Mapping(opts) = opts else { continue };
        let get = |k: &str| {
            opts.get(k)
                .and_then(scalar_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
        };
        let o = get("o");
        let typ = get("type");
        let bind = o
            .split(',')
            .any(|p| p.trim() == "bind" || p.trim() == "rbind")
            || typ == "none"
            || typ == "bind";
        let interpolated = opts
            .values()
            .filter_map(scalar_str)
            .any(|v| has_interpolation(&v));
        if bind || interpolated {
            ctx.err(format!(
                "volume \"{name}\": 'driver_opts' that bind-mount a host directory are not allowed — use a bind mount like ./{name} in the service instead"
            ));
        }
    }
}

/// Networks may only be private bridge networks of this compose project:
/// joining an existing network (`external`, a fixed `name`) would reach other
/// apps' containers or databases, and other drivers reach the host network.
fn check_top_level_networks(root: &Mapping, ctx: &mut Ctx) {
    let Some(Value::Mapping(nets)) = root.get("networks").map(untag) else {
        return;
    };
    for (name, def) in nets {
        let name = key_str(name);
        let def = untag(def);
        if def
            .get("external")
            .is_some_and(|v| !matches!(untag(v), Value::Bool(false) | Value::Null))
        {
            ctx.err(format!(
                "network \"{name}\": 'external' is not allowed — it joins a network outside this app; each app gets its own network, and databases are shared through links"
            ));
        }
        if def.get("name").is_some() {
            ctx.err(format!(
                "network \"{name}\": 'name' is not allowed — a fixed name can join another app's network; leave it out and compose names the network for this app"
            ));
        }
        if let Some(driver) = def.get("driver").and_then(scalar_str)
            && !driver.trim().eq_ignore_ascii_case("bridge")
        {
            ctx.err(format!(
                "network \"{name}\": 'driver: {driver}' is not allowed — only bridge networks can be used"
            ));
        }
    }
}

fn check_file_references(root: &Mapping, section: &str, ctx: &mut Ctx) {
    let Some(Value::Mapping(items)) = root.get(section).map(untag) else {
        return;
    };
    for (name, def) in items {
        if untag(def).get("file").is_some() {
            ctx.err(format!(
                "{section} \"{}\": 'file' is not allowed — it reads a file from the host; use 'environment:' as the source instead",
                key_str(name)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_env_passes_only_referenced_safe_vars() {
        let env: std::collections::BTreeMap<String, String> = [
            ("DB_PASSWORD", "s3cret"),
            ("UNUSED", "x"),
            ("DOCKER_HOST", "tcp://evil:2375"),
            ("LD_PRELOAD", "/tmp/x.so"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let compose = "services:\n  web:\n    image: x:1\n    environment:\n      P: ${DB_PASSWORD}\n      H: ${DOCKER_HOST}\n      L: $LD_PRELOAD\n";
        assert_eq!(
            interpolation_env(compose, &env),
            vec![("DB_PASSWORD".to_string(), "s3cret".to_string())],
            "only referenced vars reach docker compose, and never ones that steer the docker CLI"
        );
    }

    const VOL: &str = "/opt/tond/data/volumes/web";

    fn errors(yaml: &str) -> Vec<String> {
        validate_with(yaml).err().unwrap_or_default()
    }

    fn validate_with(yaml: &str) -> Result<HardenedCompose, Vec<String>> {
        harden(yaml, Path::new(VOL), 1_073_741_824)
    }

    fn hardened(yaml: &str) -> Value {
        let out = validate_with(yaml).unwrap_or_else(|e| panic!("expected valid compose: {e:?}"));
        serde_yaml::from_str(&out.yaml).unwrap_or_else(|e| panic!("hardened YAML invalid: {e}"))
    }

    fn assert_rejected(yaml: &str, needle: &str) {
        let errs = errors(yaml);
        assert!(
            errs.iter().any(|e| e.contains(needle)),
            "expected an error containing {needle:?} for:\n{yaml}\ngot: {errs:?}"
        );
    }

    #[test]
    fn a_plain_compose_file_is_accepted_and_hardened() {
        let doc = hardened(
            "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n    volumes:\n      - ./html:/usr/share/nginx/html:ro\n      - cache:/var/cache/nginx\nvolumes:\n  cache: {}\n",
        );
        let web = &doc["services"]["web"];
        assert_eq!(web["ports"][0], Value::String("127.0.0.1:8080:80".into()));
        assert_eq!(
            web["volumes"][0],
            Value::String(format!("{VOL}/html:/usr/share/nginx/html:ro"))
        );
        assert_eq!(
            web["volumes"][1],
            Value::String("cache:/var/cache/nginx".into())
        );
        assert_eq!(
            web["security_opt"],
            serde_yaml::from_str::<Value>("[\"no-new-privileges:true\"]").unwrap_or_default()
        );
        assert_eq!(web["pids_limit"], Value::Number(docker::PIDS_LIMIT.into()));
        assert_eq!(web["mem_limit"], Value::Number(1_073_741_824i64.into()));
        assert_eq!(web["logging"]["driver"], Value::String("json-file".into()));
        assert_eq!(
            web["logging"]["options"]["max-size"],
            Value::String("10m".into())
        );
        let drops: Vec<String> = list_items(&web["cap_drop"]);
        for cap in docker::DROPPED_CAPABILITIES {
            assert!(drops.contains(&cap.to_string()), "missing cap_drop {cap}");
        }
    }

    #[test]
    fn bind_sources_are_reported_relative_to_the_volume_dir() {
        let out = validate_with(
            "services:\n  a:\n    image: x\n    volumes:\n      - ./data:/data\n      - type: bind\n        source: /opt/tond/data/volumes/web/conf/app.conf\n        target: /etc/app.conf\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            out.bind_sources,
            vec![PathBuf::from("data"), PathBuf::from("conf/app.conf")]
        );
    }

    #[test]
    fn networks_must_be_private_bridges_of_the_app() {
        assert_rejected(
            "services:\n  a:\n    image: x\nnetworks:\n  shared:\n    external: true\n",
            "'external' is not allowed",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\nnetworks:\n  n:\n    name: tond-net-db-main\n",
            "'name' is not allowed",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\nnetworks:\n  n:\n    driver: macvlan\n",
            "only bridge networks",
        );
        assert!(
            errors(
                "services:\n  a:\n    image: x\n    networks: [back]\nnetworks:\n  back:\n    driver: bridge\n    internal: true\n"
            )
            .is_empty(),
            "private bridge networks declared in the file stay allowed"
        );
    }

    #[test]
    fn named_volumes_must_belong_to_the_app() {
        assert_rejected(
            "services:\n  a:\n    image: x\n    volumes: [d:/d]\nvolumes:\n  d:\n    external: true\n",
            "volume \"d\": 'external' is not allowed",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    volumes: [d:/d]\nvolumes:\n  d:\n    name: shop_data\n",
            "volume \"d\": 'name' is not allowed",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\nvolumes:\n  d:\n    driver: rexray/ebs\n",
            "only local volumes",
        );
        assert!(
            errors(
                "services:\n  a:\n    image: x\n    volumes: [d:/d]\nvolumes:\n  d:\n    driver: local\n    external: false\n"
            )
            .is_empty(),
            "plain local volumes stay allowed"
        );
    }

    #[test]
    fn privileged_and_host_namespaces_are_rejected() {
        assert_rejected(
            "services:\n  a:\n    image: x\n    privileged: true\n",
            "'privileged'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    privileged: \"${P:-true}\"\n",
            "'privileged'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    pid: host\n",
            "'pid: host'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    ipc: host\n",
            "'ipc: host'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    network_mode: host\n",
            "'network_mode: host'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    network_mode: \"container:tond-db-main\"\n",
            "joins a container outside this app",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    userns_mode: host\n",
            "'userns_mode: host'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    uts: host\n",
            "'uts: host'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    cgroup: host\n",
            "'cgroup: host'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    cgroup_parent: /\n",
            "'cgroup_parent'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    network_mode: bridge\n",
            "default bridge",
        );
        // Sharing namespaces between services of the same file is fine.
        assert!(errors("services:\n  a:\n    image: x\n  b:\n    image: y\n    network_mode: \"service:a\"\n    pid: \"service:a\"\n").is_empty());
        assert!(errors("services:\n  a:\n    image: x\n    privileged: false\n").is_empty());
    }

    #[test]
    fn capabilities_devices_and_security_opts_are_restricted() {
        assert_rejected(
            "services:\n  a:\n    image: x\n    cap_add: [SYS_ADMIN]\n",
            "'cap_add: SYS_ADMIN'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    cap_add: [ALL]\n",
            "'cap_add: ALL'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    cap_add: [CAP_NET_ADMIN]\n",
            "CAP_NET_ADMIN",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    devices: [\"/dev/sda:/dev/sda\"]\n",
            "'devices'",
        );
        assert_rejected("services:\n  a:\n    image: x\n    gpus: all\n", "'gpus'");
        for opt in [
            "apparmor:unconfined",
            "seccomp=unconfined",
            "label:disable",
            "no-new-privileges:false",
            "systempaths=unconfined",
        ] {
            assert_rejected(
                &format!("services:\n  a:\n    image: x\n    security_opt: [\"{opt}\"]\n"),
                "disables container confinement",
            );
        }
        // Re-adding a default capability is allowed and not dropped again.
        let doc = hardened("services:\n  a:\n    image: x\n    cap_add: [NET_RAW]\n");
        let drops = list_items(&doc["services"]["a"]["cap_drop"]);
        assert!(!drops.contains(&"NET_RAW".to_string()));
        assert!(drops.contains(&"MKNOD".to_string()));
    }

    #[test]
    fn host_binds_are_rejected_with_an_actionable_message() {
        for src in [
            "/",
            "/etc",
            "/var/run/docker.sock",
            "/opt/tond/data",
            "/opt/tond/data/volumes/other/data",
            "../other",
            "./data/../../x",
            "~/.ssh",
        ] {
            let yaml =
                format!("services:\n  a:\n    image: x\n    volumes:\n      - \"{src}:/mnt\"\n");
            assert_rejected(&yaml, "outside this app's volume directory");
        }
        assert_rejected(
            "services:\n  a:\n    image: x\n    volumes:\n      - type: bind\n        source: /var/run/docker.sock\n        target: /var/run/docker.sock\n",
            "outside this app's volume directory",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    volumes:\n      - .:/app\n",
            "mounts the whole volume directory",
        );
        let errs = errors("services:\n  a:\n    image: x\n    volumes:\n      - /etc:/etc\n");
        assert!(errs[0].contains("./data"), "should suggest a fix: {errs:?}");
    }

    #[test]
    fn interpolation_is_refused_in_security_relevant_fields() {
        assert_rejected(
            "services:\n  a:\n    image: x\n    volumes:\n      - \"${HOST:-/}:/host\"\n",
            "variable substitution",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    network_mode: \"${MODE}\"\n",
            "variable substitution",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    ports: [\"${IP}:80:80\"]\n",
            "variable substitution",
        );
        // ...but fine in environment and image, and $$ is a literal.
        assert!(errors("services:\n  a:\n    image: \"nginx:${TAG:-latest}\"\n    environment:\n      PASSWORD: ${PASSWORD:-secret}\n    command: echo $$HOME\n").is_empty());
    }

    #[test]
    fn merge_keys_and_anchors_cannot_hide_options() {
        assert_rejected(
            "x-base: &base\n  privileged: true\nservices:\n  a:\n    <<: *base\n    image: x\n",
            "'privileged'",
        );
        assert_rejected(
            "x-vols: &vols\n  - /:/host\nservices:\n  a:\n    image: x\n    volumes: *vols\n",
            "outside this app's volume directory",
        );
    }

    #[test]
    fn file_based_sources_and_includes_are_rejected() {
        assert_rejected(
            "include:\n  - other.yml\nservices:\n  a:\n    image: x\n",
            "'include'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    env_file: /etc/shadow\n",
            "'env_file'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\nsecrets:\n  s:\n    file: /etc/shadow\n",
            "'file' is not allowed",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\nconfigs:\n  c:\n    file: /root/.ssh/id_rsa\n",
            "'file' is not allowed",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    extends:\n      file: /etc/other.yml\n      service: b\n",
            "'extends.file'",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    volumes_from: [\"container:tond-db-main\"]\n",
            "'volumes_from",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\nvolumes:\n  root:\n    driver: local\n    driver_opts:\n      type: none\n      o: bind\n      device: /\n",
            "driver_opts",
        );
    }

    #[test]
    fn build_contexts_must_stay_inside_the_compose_directory() {
        assert_rejected("services:\n  a:\n    build: /\n", "build contexts");
        assert_rejected(
            "services:\n  a:\n    build:\n      context: ../..\n",
            "build contexts",
        );
        assert_rejected(
            "services:\n  a:\n    build:\n      context: .\n      secrets: [s]\n",
            "'build.secrets'",
        );
        assert!(
            errors("services:\n  a:\n    build: https://github.com/org/repo.git#main\n").is_empty()
        );
        assert!(errors("services:\n  a:\n    build:\n      context: .\n      dockerfile_inline: |\n        FROM alpine\n").is_empty());
    }

    #[test]
    fn replicas_refuse_a_fixed_host_port() {
        let errs = errors(
            "services:\n  web:\n    image: x\n    deploy:\n      replicas: 3\n    ports: [\"8080:80\"]\n",
        );
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("runs 3 replicas"), "{errs:?}");
        assert!(errs[0].contains("8080"), "{errs:?}");
        // scale: and the long syntax too.
        assert!(!errors(
            "services:\n  web:\n    image: x\n    scale: 2\n    ports:\n      - target: 80\n        published: 9000\n",
        )
        .is_empty());
        // An ephemeral host port, a range or one replica is fine.
        for ok in [
            "services:\n  web:\n    image: x\n    deploy:\n      replicas: 3\n    ports: [\"80\"]\n",
            "services:\n  web:\n    image: x\n    scale: 2\n    ports: [\"8000-8001:80\"]\n",
            "services:\n  web:\n    image: x\n    deploy:\n      replicas: 1\n    ports: [\"8080:80\"]\n",
            "services:\n  web:\n    image: x\n    deploy:\n      replicas: 2\n    ports:\n      - target: 80\n",
        ] {
            assert!(errors(ok).is_empty(), "{ok}: {:?}", errors(ok));
        }
    }

    #[test]
    fn ports_are_rewritten_to_loopback_and_public_ips_rejected() {
        let doc = hardened(
            "services:\n  a:\n    image: x\n    ports:\n      - 80\n      - \"8080:80\"\n      - \"127.0.0.1:9000:9000\"\n      - \"8000-8001:8000-8001/udp\"\n      - target: 443\n        published: 8443\n",
        );
        let ports = &doc["services"]["a"]["ports"];
        assert_eq!(ports[0], Value::String("127.0.0.1::80".into()));
        assert_eq!(ports[1], Value::String("127.0.0.1:8080:80".into()));
        assert_eq!(ports[2], Value::String("127.0.0.1:9000:9000".into()));
        assert_eq!(
            ports[3],
            Value::String("127.0.0.1:8000-8001:8000-8001/udp".into())
        );
        assert_eq!(ports[4]["host_ip"], Value::String("127.0.0.1".into()));

        assert_rejected(
            "services:\n  a:\n    image: x\n    ports: [\"0.0.0.0:5432:5432\"]\n",
            "binds a public interface",
        );
        assert_rejected(
            "services:\n  a:\n    image: x\n    ports:\n      - target: 80\n        host_ip: 0.0.0.0\n",
            "binds a public interface",
        );
        assert!(
            errors("services:\n  a:\n    image: x\n    ports: [\"[::1]:8080:80\"]\n").is_empty()
        );
    }

    #[test]
    fn explicit_limits_and_logging_are_kept() {
        let doc = hardened(
            "services:\n  a:\n    image: x\n    mem_limit: 256m\n    pids_limit: 50\n    logging:\n      driver: json-file\n      options:\n        max-size: 1m\n  b:\n    image: y\n    deploy:\n      resources:\n        limits:\n          memory: 128M\n    logging:\n      driver: none\n",
        );
        let a = &doc["services"]["a"];
        assert_eq!(a["mem_limit"], Value::String("256m".into()));
        assert_eq!(a["pids_limit"], Value::Number(50.into()));
        assert_eq!(
            a["logging"]["options"]["max-size"],
            Value::String("1m".into())
        );
        assert_eq!(
            a["logging"]["options"]["max-file"],
            Value::String("3".into())
        );
        let b = &doc["services"]["b"];
        assert!(
            b.get("mem_limit").is_none(),
            "deploy.resources.limits.memory counts"
        );
        assert_eq!(b["logging"]["driver"], Value::String("none".into()));
    }

    #[test]
    fn all_problems_are_reported_together_and_formatted() {
        let errs = errors(
            "services:\n  a:\n    image: x\n    privileged: true\n    pid: host\n    volumes: [\"/:/host\"]\n",
        );
        assert_eq!(errs.len(), 3, "{errs:?}");
        let msg = format_errors(&errs);
        assert!(msg.starts_with("Compose file rejected (3 problems):\n- "));
        assert_eq!(
            format_errors(&errs[..1]),
            format!("Compose file rejected: {}", errs[0])
        );
    }

    #[test]
    fn malformed_files_get_clear_errors() {
        assert!(errors("services: [")[0].contains("not valid YAML"));
        assert!(errors("- a\n- b\n")[0].contains("top-level 'services:'"));
        assert!(errors("version: '3'\n")[0].contains("at least one service"));
    }
}
