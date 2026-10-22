// egress.rs — what an app's containers may connect to (outbound policy)
//
// Every app has an egress policy, and so do its previews:
//
//   open        anything (the default for apps)
//   restricted  only the allowlist: IPv4 addresses, CIDR ranges and
//               hostnames, each with optional ports
//   none        no internet at all (the default for previews)
//
// Whatever the policy, DNS works (port 53, so names resolve through Docker's
// embedded resolver, which forwards from the container's own network
// namespace), and so does everything on the container's own Docker networks:
// linked databases, the other members of its project, the other services of
// a compose stack. Replies to connections made *to* the app (tond-proxy,
// published ports) are never affected.
//
// ## Enforcement (kernel, like db_access.rs)
//
// Container traffic leaving its bridge is forwarded by the host and passes
// Docker's DOCKER-USER chain first. tond-server owns a chain TOND-EGRESS and a
// jump to it from DOCKER-USER (db_access::load_chain). For every source it
// holds, in order:
//
//   -s <src> -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
//   -s <src> -d <own network subnet> -j RETURN           ...each network
//   -s <src> -p udp --dport 53 -j RETURN
//   -s <src> -p tcp --dport 53 -j RETURN
//   -s <src> -d <allowed> [-p tcp|udp -m multiport --dports p,q] -j RETURN
//   -s <src> -p tcp -j REJECT --reject-with tcp-reset
//   -s <src> -j REJECT --reject-with icmp-port-unreachable
//
// REJECT rather than DROP, so a blocked connection fails at once instead of
// hanging until a timeout. Each rule carries a comment (`tond-egress:app:<n>`
// or `tond-egress:preview:<slug>`) for `iptables -S` readers.
//
// Sources:
//   * an app's containers, by their address on each network (`/32`): apps in
//     a project share the project network, so the network can't be the key.
//     The container (`tond-<app>`, the staging `tond-<app>-next`, or every
//     container of a compose app) is looked up when its address may have
//     changed: deploys call `reconcile_now` right after the container starts,
//     the loop wakes on every apps change, and re-checks every few minutes.
//   * a preview's network subnet (`tond-net-preview-<slug>`): it holds only
//     the preview, and it exists before the container starts, so the deploy
//     loads the rules first and a preview never runs unrestricted.
//     A project preview's apps share one network: the strictest policy among
//     them applies; when all are restricted, the union of their allowlists.
//
// Hostnames are resolved (IPv4, the host's resolver) when the rules are
// loaded and again every REFRESH; an address change in between isn't seen
// until then, and a name that resolves to a CDN may share addresses with
// unrelated sites. CIDR ranges are the reliable form. Tond's networks are
// IPv4-only, so there is no IPv6 egress to police.
//
// Not covered: connections to the host itself (INPUT, not FORWARD: services
// listening on the server's own addresses, e.g. SSH and tond-proxy), and a
// short moment after an app container starts and before its address is
// loaded. Hosts without iptables or the DOCKER-USER chain can't enforce a
// policy: saving a non-open app policy is refused there, and a preview
// policy is reported as not enforced.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use anyhow::Result;
use tond_common::{App, EgressHostResponse};

use crate::db_access;
use crate::deploy::compose_project_name;
use crate::networks;
use crate::runtime::changes::{Table, Watch};
use crate::runtime::loops;
use crate::state::AppState;

/// Chain holding Tond's egress rules.
pub const CHAIN: &str = "TOND-EGRESS";
/// Most destinations one allowlist may hold.
pub const MAX_ALLOWLIST: usize = 64;
/// Most ports per destination (iptables multiport takes 15).
pub const MAX_PORTS: usize = 15;
/// How often hostnames are resolved again and container addresses
/// re-checked while any policy is in force.
pub const REFRESH: Duration = Duration::from_secs(300);

// ── Policy and allowlist ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Policy {
    /// Ordered strictest first (`min` is the strictest of two).
    None,
    Restricted,
    Open,
}

impl Policy {
    pub fn from_input(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Policy::Open),
            "restricted" => Some(Policy::Restricted),
            "none" => Some(Policy::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Open => "open",
            Policy::Restricted => "restricted",
            Policy::None => "none",
        }
    }
}

/// An app's own policy (anything unknown is "open", the column default).
pub fn app_policy(app: &App) -> Policy {
    Policy::from_input(&app.egress_policy).unwrap_or(Policy::Open)
}

/// An app's previews' policy (anything unknown is "none", the default).
pub fn preview_policy(app: &App) -> Policy {
    Policy::from_input(&app.preview_egress_policy).unwrap_or(Policy::None)
}

/// The stored allowlist (already canonical).
pub fn allowlist(app: &App) -> Vec<String> {
    serde_json::from_str(&app.egress_allowlist).unwrap_or_default()
}

/// Where an allowlist entry points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// "a.b.c.d" or "a.b.c.d/nn" (canonical network address)
    Net(String),
    /// A DNS name, resolved when the rules are loaded
    Host(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub target: Target,
    /// Empty: every port (TCP and UDP)
    pub ports: Vec<u16>,
}

impl std::fmt::Display for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.target {
            Target::Net(n) | Target::Host(n) => write!(f, "{n}")?,
        }
        if !self.ports.is_empty() {
            let ports: Vec<String> = self.ports.iter().map(u16::to_string).collect();
            write!(f, ":{}", ports.join(","))?;
        }
        Ok(())
    }
}

fn parse_net(entry: &str, addr: &str, prefix: Option<&str>) -> Result<String, String> {
    let ip: Ipv4Addr = addr
        .parse()
        .map_err(|_| format!("'{entry}' isn't an IPv4 address, a CIDR range or a hostname"))?;
    let prefix: u8 = match prefix {
        Some(p) => p
            .parse()
            .ok()
            .filter(|p| *p <= 32)
            .ok_or_else(|| format!("'{entry}' has an invalid prefix length (0–32)"))?,
        None => 32,
    };
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    let net = Ipv4Addr::from(u32::from(ip) & mask);
    Ok(if prefix == 32 {
        net.to_string()
    } else {
        format!("{net}/{prefix}")
    })
}

fn valid_hostname(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        // The last label is a TLD, never all digits (that's an address).
        && !host
            .rsplit('.')
            .next()
            .is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()))
}

/// Parse one allowlist entry: `<host-or-ip-or-cidr>[:<port>[,<port>…]]`.
pub fn parse_entry(raw: &str) -> Result<Entry, String> {
    let entry = raw.trim().to_ascii_lowercase();
    if entry.contains('[') || entry.matches(':').count() > 1 {
        return Err(format!(
            "'{}' is IPv6. Tond's networks are IPv4 only; allow IPv4 addresses or hostnames.",
            raw.trim()
        ));
    }
    let (dest, ports) = match entry.split_once(':') {
        Some((d, p)) => (d, Some(p)),
        None => (entry.as_str(), None),
    };
    let mut parsed_ports: Vec<u16> = Vec::new();
    if let Some(ports) = ports {
        for p in ports.split(',') {
            let port: u16 = p
                .trim()
                .parse()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("'{entry}' has an invalid port '{p}' (1–65535)"))?;
            if !parsed_ports.contains(&port) {
                parsed_ports.push(port);
            }
        }
        if parsed_ports.len() > MAX_PORTS {
            return Err(format!(
                "'{entry}' lists more than {MAX_PORTS} ports; split it into several entries"
            ));
        }
    }
    if dest.starts_with("*.") {
        return Err(format!(
            "'{entry}': wildcards can't be resolved to addresses. List the hostnames the app \
             uses, or the provider's published CIDR ranges."
        ));
    }
    let looks_numeric = dest
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '/');
    let target = if looks_numeric {
        let (addr, prefix) = match dest.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (dest, None),
        };
        let net = parse_net(&entry, addr, prefix)?;
        if net == "0.0.0.0/0" && parsed_ports.is_empty() {
            return Err(
                "0.0.0.0/0 on every port allows everything; choose the open policy instead, \
                 or add ports (0.0.0.0/0:443)"
                    .to_string(),
            );
        }
        Target::Net(net)
    } else if valid_hostname(dest) {
        Target::Host(dest.to_string())
    } else {
        return Err(format!(
            "'{}' isn't an IPv4 address, a CIDR range or a hostname",
            raw.trim()
        ));
    };
    Ok(Entry {
        target,
        ports: parsed_ports,
    })
}

/// Parse, validate and canonicalise an allowlist. Duplicates are dropped.
pub fn normalize_allowlist(entries: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in entries {
        if raw.trim().is_empty() {
            continue;
        }
        let canonical = parse_entry(raw)?.to_string();
        if !out.contains(&canonical) {
            out.push(canonical);
        }
    }
    if out.len() > MAX_ALLOWLIST {
        return Err(format!(
            "At most {MAX_ALLOWLIST} destinations per app; use CIDR ranges to group them"
        ));
    }
    Ok(out)
}

// ── Ruleset ──────────────────────────────────────────────────────────────────

/// An allowed destination with hostnames resolved.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Allowed {
    /// An address or CIDR range
    pub dest: String,
    pub ports: Vec<u16>,
}

/// One set of addresses the policy applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// "app:<name>" or "preview:<slug>" (the rules' comment)
    pub label: String,
    /// `-s` values: container addresses or a network subnet
    pub addrs: Vec<String>,
    /// Subnets of the networks the source is on (always reachable)
    pub own_nets: Vec<String>,
    /// Restricted or None (Open sources have no rules)
    pub policy: Policy,
    /// Used when `policy` is Restricted
    pub allowed: Vec<Allowed>,
}

/// The iptables-restore input that replaces TOND-EGRESS with rules for
/// `sources`. Declaring the chain flushes it atomically.
pub fn ruleset(sources: &[Source]) -> String {
    let mut out = String::from("*filter\n");
    out.push_str(&format!(":{CHAIN} - [0:0]\n"));
    for source in sources.iter().filter(|s| s.policy != Policy::Open) {
        let comment = format!("-m comment --comment tond-egress:{}", source.label);
        for src in &source.addrs {
            let rule =
                |m: &str, target: &str| format!("-A {CHAIN} -s {src} {m}{comment} -j {target}\n");
            out.push_str(&rule(
                "-m conntrack --ctstate ESTABLISHED,RELATED ",
                "RETURN",
            ));
            for net in &source.own_nets {
                out.push_str(&rule(&format!("-d {net} "), "RETURN"));
            }
            out.push_str(&rule("-p udp --dport 53 ", "RETURN"));
            out.push_str(&rule("-p tcp --dport 53 ", "RETURN"));
            if source.policy == Policy::Restricted {
                for allowed in &source.allowed {
                    let d = format!("-d {} ", allowed.dest);
                    if allowed.ports.is_empty() {
                        out.push_str(&rule(&d, "RETURN"));
                    } else {
                        let ports: Vec<String> = allowed.ports.iter().map(u16::to_string).collect();
                        let ports = ports.join(",");
                        for proto in ["tcp", "udp"] {
                            out.push_str(&rule(
                                &format!("{d}-p {proto} -m multiport --dports {ports} "),
                                "RETURN",
                            ));
                        }
                    }
                }
            }
            out.push_str(&rule("-p tcp ", "REJECT --reject-with tcp-reset"));
            out.push_str(&rule("", "REJECT --reject-with icmp-port-unreachable"));
        }
    }
    out.push_str("COMMIT\n");
    out
}

/// Resolve an allowlist into destinations, using `resolve` for hostnames
/// (an empty answer leaves the name out). Sorted, without duplicates.
pub fn resolve_entries(
    entries: &[String],
    resolve: &dyn Fn(&str) -> Vec<Ipv4Addr>,
) -> Vec<Allowed> {
    let mut out: BTreeSet<Allowed> = BTreeSet::new();
    for entry in entries.iter().filter_map(|e| parse_entry(e).ok()) {
        match &entry.target {
            Target::Net(net) => {
                out.insert(Allowed {
                    dest: net.clone(),
                    ports: entry.ports.clone(),
                });
            }
            Target::Host(host) => {
                for ip in resolve(host) {
                    out.insert(Allowed {
                        dest: ip.to_string(),
                        ports: entry.ports.clone(),
                    });
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The hostnames in an allowlist.
pub fn hostnames(entries: &[String]) -> Vec<String> {
    let mut out: Vec<String> = entries
        .iter()
        .filter_map(|e| match parse_entry(e).ok()?.target {
            Target::Host(h) => Some(h),
            Target::Net(_) => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// One preview network's policy: the strictest among the apps sharing it,
/// and the union of their allowlists when that is "restricted".
pub fn merge_preview_policies(apps: &[(Policy, Vec<String>)]) -> (Policy, Vec<String>) {
    let policy = apps.iter().map(|(p, _)| *p).min().unwrap_or(Policy::None);
    let mut entries: Vec<String> = Vec::new();
    if policy == Policy::Restricted {
        for (_, list) in apps {
            for e in list {
                if !entries.contains(e) {
                    entries.push(e.clone());
                }
            }
        }
    }
    (policy, entries)
}

/// The subnet of an interface address: ("172.18.0.5", 16) → "172.18.0.0/16".
pub fn subnet_of(ip: &str, prefix: i64) -> Option<String> {
    let prefix = u8::try_from(prefix).ok().filter(|p| (1..=32).contains(p))?;
    parse_net(ip, ip, Some(&prefix.to_string())).ok()
}

// ── Status ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct EgressStatus {
    /// iptables with a DOCKER-USER chain is usable
    pub available: bool,
    /// Why the firewall can't be used, or the last load failed
    pub error: Option<String>,
    /// Labels whose rules are loaded ("app:<name>", "preview:<slug>")
    pub applied: BTreeSet<String>,
    /// Apps with a non-open policy but no running container found
    pub idle: BTreeSet<String>,
}

static STATUS: LazyLock<RwLock<EgressStatus>> =
    LazyLock::new(|| RwLock::new(EgressStatus::default()));
/// The ruleset last loaded (skip reloading identical rules).
static LOADED: LazyLock<tokio::sync::Mutex<Option<String>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));
/// Last good answer per hostname (kept when a lookup fails).
static RESOLVED: LazyLock<RwLock<BTreeMap<String, Vec<Ipv4Addr>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

pub fn status() -> EgressStatus {
    STATUS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_status(status: EgressStatus) {
    *STATUS.write().unwrap_or_else(|e| e.into_inner()) = status;
}

/// What each hostname of an allowlist resolves to right now (as last loaded).
pub fn resolved_hosts(entries: &[String]) -> Vec<EgressHostResponse> {
    let cache = RESOLVED.read().unwrap_or_else(|e| e.into_inner());
    hostnames(entries)
        .into_iter()
        .map(|host| EgressHostResponse {
            addresses: cache
                .get(&host)
                .map(|ips| ips.iter().map(Ipv4Addr::to_string).collect())
                .unwrap_or_default(),
            host,
        })
        .collect()
}

/// Why an app's policy isn't in force, if it isn't.
pub fn app_warning(app: &App, status: &EgressStatus) -> Option<String> {
    if app_policy(app) == Policy::Open {
        return None;
    }
    if !status.available {
        return Some(format!(
            "Not enforced: {}. The app can reach the internet freely.",
            status
                .error
                .as_deref()
                .unwrap_or("the server firewall (iptables DOCKER-USER chain) isn't available")
        ));
    }
    let label = format!("app:{}", app.name);
    if status.applied.contains(&label) || status.idle.contains(&app.name) {
        return None;
    }
    Some(match &status.error {
        Some(e) => format!("The policy couldn't be loaded into the firewall: {e}"),
        None => "The policy is saved but not loaded into the firewall yet.".to_string(),
    })
}

/// Why the previews' policy isn't in force, if it isn't.
pub fn preview_warning(app: &App, status: &EgressStatus) -> Option<String> {
    if preview_policy(app) == Policy::Open || status.available {
        return None;
    }
    Some(format!(
        "Not enforced on this server ({}): previews can reach the internet freely.",
        status
            .error
            .as_deref()
            .unwrap_or("the iptables DOCKER-USER chain isn't available")
    ))
}

// ── Loading ──────────────────────────────────────────────────────────────────

async fn resolve_host(host: &str) -> Option<Vec<Ipv4Addr>> {
    let lookup = tokio::net::lookup_host(format!("{host}:0"));
    let addrs = tokio::time::timeout(Duration::from_secs(5), lookup)
        .await
        .ok()?
        .ok()?;
    let mut ips: Vec<Ipv4Addr> = addrs
        .filter_map(|a| match a.ip() {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        })
        .collect();
    ips.sort();
    ips.dedup();
    (!ips.is_empty()).then_some(ips)
}

/// Resolve every hostname in `entries` (refreshing the cache) and return the
/// cache's view.
async fn refresh_hosts(entries: &[String]) -> BTreeMap<String, Vec<Ipv4Addr>> {
    for host in hostnames(entries) {
        match resolve_host(&host).await {
            Some(ips) => {
                RESOLVED
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(host, ips);
            }
            None => tracing::warn!("egress: couldn't resolve {host}; keeping its last addresses"),
        }
    }
    RESOLVED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// (address, subnet) of a running container on each of its networks.
async fn container_addresses(
    docker: &bollard::Docker,
    container: &str,
) -> Vec<(String, Option<String>)> {
    let Ok(info) = docker.inspect_container(container, None).await else {
        return Vec::new();
    };
    if !info.state.as_ref().and_then(|s| s.running).unwrap_or(false) {
        return Vec::new();
    }
    info.network_settings
        .and_then(|n| n.networks)
        .map(|nets| {
            nets.into_values()
                .filter_map(|ep| {
                    let ip = ep.ip_address.filter(|ip| !ip.is_empty())?;
                    let subnet = ep.ip_prefix_len.and_then(|p| subnet_of(&ip, p));
                    Some((ip, subnet))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The containers an app runs: `tond-<app>` and a staging `tond-<app>-next`,
/// or every container of a compose app's project.
async fn app_containers(docker: &bollard::Docker, app: &App) -> Vec<String> {
    if app.source_type == "compose" {
        return crate::docker::compose_containers(docker, &compose_project_name(&app.name)).await;
    }
    vec![
        format!("tond-{}", app.name),
        format!("tond-{}-next", app.name),
    ]
}

/// The subnet of a Docker network, if it exists.
async fn network_subnet(docker: &bollard::Docker, name: &str) -> Option<String> {
    let net = docker.inspect_network(name, None).await.ok()?;
    net.ipam?
        .config?
        .into_iter()
        .filter_map(|c| c.subnet)
        .find(|s| s.parse::<std::net::Ipv6Addr>().is_err() && !s.contains(':'))
}

/// Everything TOND-EGRESS should hold right now.
async fn desired_sources(state: &AppState) -> Result<(Vec<Source>, BTreeSet<String>)> {
    let apps: Vec<App> = sqlx::query_as(
        "SELECT * FROM apps WHERE egress_policy != 'open' OR preview_egress_policy != 'open'",
    )
    .fetch_all(&state.db)
    .await?;
    let mut sources = Vec::new();
    let mut idle = BTreeSet::new();
    if apps.is_empty() {
        return Ok((sources, idle));
    }
    let every_entry: Vec<String> = apps.iter().flat_map(allowlist).collect();
    let cache = refresh_hosts(&every_entry).await;
    let resolve = |h: &str| cache.get(h).cloned().unwrap_or_default();

    for app in apps.iter().filter(|a| app_policy(a) != Policy::Open) {
        let mut addrs = BTreeSet::new();
        let mut own = BTreeSet::new();
        for container in app_containers(&state.docker, app).await {
            for (ip, subnet) in container_addresses(&state.docker, &container).await {
                addrs.insert(format!("{ip}/32"));
                own.extend(subnet);
            }
        }
        if addrs.is_empty() {
            idle.insert(app.name.clone());
            continue;
        }
        sources.push(Source {
            label: format!("app:{}", app.name),
            addrs: addrs.into_iter().collect(),
            own_nets: own.into_iter().collect(),
            policy: app_policy(app),
            allowed: resolve_entries(&allowlist(app), &resolve),
        });
    }

    // Previews: one source per preview network, keyed by slug.
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT p.slug, p.app_id FROM preview_deployments p
         JOIN apps a ON a.id = p.app_id
         WHERE p.status IN ('building', 'running') AND a.preview_egress_policy != 'open'
         ORDER BY p.slug",
    )
    .fetch_all(&state.db)
    .await?;
    let mut by_slug: BTreeMap<String, Vec<(Policy, Vec<String>)>> = BTreeMap::new();
    for (slug, app_id) in rows {
        if let Some(app) = apps.iter().find(|a| a.id == app_id) {
            by_slug
                .entry(slug)
                .or_default()
                .push((preview_policy(app), allowlist(app)));
        }
    }
    for (slug, members) in by_slug {
        let Some(subnet) = network_subnet(&state.docker, &networks::preview_network(&slug)).await
        else {
            continue;
        };
        let (policy, entries) = merge_preview_policies(&members);
        sources.push(Source {
            label: format!("preview:{slug}"),
            addrs: vec![subnet.clone()],
            own_nets: vec![subnet],
            policy,
            allowed: resolve_entries(&entries, &resolve),
        });
    }
    Ok((sources, idle))
}

/// Load the rules the apps and previews call for, unless they are loaded
/// already. `force` reloads anyway (the periodic re-check).
async fn reconcile(state: &AppState, force: bool) -> Result<()> {
    let (sources, idle) = desired_sources(state).await?;
    let text = ruleset(&sources);
    let labels: BTreeSet<String> = sources
        .iter()
        .filter(|s| s.policy != Policy::Open)
        .map(|s| s.label.clone())
        .collect();
    let mut loaded = LOADED.lock().await;
    if !force && loaded.as_deref() == Some(text.as_str()) {
        let mut st = status();
        st.idle = idle;
        set_status(st);
        return Ok(());
    }
    if labels.is_empty() && loaded.is_none() {
        // Nothing to enforce and nothing of ours loaded: leave the firewall
        // alone, but know whether a policy could be enforced.
        *loaded = Some(text);
        let available = db_access::probe_firewall().await;
        set_status(EgressStatus {
            available: available.is_ok(),
            error: available.err().map(|e| format!("{e:#}")),
            applied: BTreeSet::new(),
            idle,
        });
        return Ok(());
    }
    match db_access::load_chain(CHAIN, &text).await {
        Ok(()) => {
            set_status(EgressStatus {
                available: true,
                error: None,
                applied: labels,
                idle,
            });
            *loaded = Some(text);
            Ok(())
        }
        Err(e) => {
            set_status(EgressStatus {
                available: db_access::probe_firewall().await.is_ok(),
                error: Some(format!("{e:#}")),
                applied: BTreeSet::new(),
                idle,
            });
            *loaded = None;
            Err(e)
        }
    }
}

/// Apply policy changes right away (after a save, or a container start).
pub async fn reconcile_now(state: &AppState) {
    if let Err(e) = reconcile(state, false).await {
        tracing::warn!("egress: couldn't load the egress rules: {e:#}");
    }
}

/// Before a preview container starts on network `tond-net-preview-<slug>`:
/// load its rules, and refuse to go on when its policy isn't open, the
/// firewall works, and the rules still couldn't be loaded (pull-request code
/// must not run with more access than configured). Without a usable firewall
/// the preview runs unrestricted; the app's Network settings say so.
pub async fn prepare_preview(state: &AppState, app: &App, slug: &str) -> Result<()> {
    if preview_policy(app) == Policy::Open {
        return Ok(());
    }
    let result = reconcile(state, false).await;
    let st = status();
    if !st.available {
        tracing::warn!(
            "egress: preview {slug} of {} runs without its egress policy: the firewall isn't available",
            app.name
        );
        return Ok(());
    }
    result?;
    anyhow::ensure!(
        st.applied.contains(&format!("preview:{slug}")),
        "the preview's egress policy couldn't be loaded into the firewall"
    );
    Ok(())
}

/// The "egress" loop: keeps TOND-EGRESS in line with the apps and previews.
/// Wakes on apps and preview changes; while any rule is loaded, re-resolves
/// hostnames and re-checks container addresses every REFRESH.
pub async fn run_loop(state: AppState) {
    let lp = loops::register("egress");
    let mut watch = Watch::new(&[Table::Apps, Table::PreviewDeployments]);
    let mut force = true;
    loop {
        let run = lp.run();
        match reconcile(&state, force).await {
            Ok(()) => drop(run),
            Err(e) => {
                tracing::warn!("egress: {e:#}");
                run.fail(&format!("{e:#}"));
            }
        }
        let st = status();
        let timeout = (!st.applied.is_empty() || !st.idle.is_empty()).then_some(REFRESH);
        force = lp.wait(timeout, &mut watch).await == loops::Wake::Timer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allowlist_entries_are_validated_and_canonicalised() {
        assert_eq!(
            normalize_allowlist(&list(&[
                " API.Stripe.com:443 ",
                "198.51.100.77/24",
                "203.0.113.7:5432,6432,5432",
                "203.0.113.7/32",
                "0.0.0.0/0:443",
                "api.stripe.com:443",
                "",
            ])),
            Ok(list(&[
                "api.stripe.com:443",
                "198.51.100.0/24",
                "203.0.113.7:5432,6432",
                "203.0.113.7",
                "0.0.0.0/0:443",
            ]))
        );
        for bad in [
            "not a host",
            "10.0.0.0/33",
            "2001:db8::1",
            "[2001:db8::1]:443",
            "0.0.0.0/0",
            "example.com:0",
            "example.com:99999",
            "example.com:http",
            "*.example.com",
            "localhost",
            "-bad.example.com",
            "1.2.3.4.5",
            "300.1.1.1",
        ] {
            assert!(
                normalize_allowlist(&list(&[bad])).is_err(),
                "{bad} must be refused"
            );
        }
        let ports: Vec<String> = (1..=16).map(|p| p.to_string()).collect();
        assert!(normalize_allowlist(&[format!("example.com:{}", ports.join(","))]).is_err());
        let many: Vec<String> = (0..=MAX_ALLOWLIST).map(|i| format!("10.0.{i}.1")).collect();
        assert!(normalize_allowlist(&many).is_err());
    }

    #[test]
    fn policies_parse_and_order_strictest_first() {
        for p in ["open", "restricted", "none"] {
            assert_eq!(Policy::from_input(p).map(Policy::as_str), Some(p));
        }
        assert_eq!(Policy::from_input("closed"), None);
        assert!(Policy::None < Policy::Restricted && Policy::Restricted < Policy::Open);
        let mut app = App::new("web".into(), "git".into(), None, None, 3000);
        assert_eq!(app_policy(&app), Policy::Open, "apps default to open");
        assert_eq!(
            preview_policy(&app),
            Policy::None,
            "previews default to none"
        );
        app.preview_egress_policy = String::new();
        assert_eq!(preview_policy(&app), Policy::None);
    }

    fn resolver(host: &str) -> Vec<Ipv4Addr> {
        match host {
            "api.example.com" => vec![Ipv4Addr::new(192, 0, 2, 10), Ipv4Addr::new(192, 0, 2, 11)],
            _ => Vec::new(),
        }
    }

    #[test]
    fn hostnames_resolve_into_addresses_and_unresolved_names_drop_out() {
        let allowed = resolve_entries(
            &list(&["api.example.com:443", "gone.example.com", "198.51.100.0/24"]),
            &resolver,
        );
        assert_eq!(
            allowed,
            vec![
                Allowed {
                    dest: "192.0.2.10".into(),
                    ports: vec![443]
                },
                Allowed {
                    dest: "192.0.2.11".into(),
                    ports: vec![443]
                },
                Allowed {
                    dest: "198.51.100.0/24".into(),
                    ports: vec![]
                },
            ]
        );
        assert_eq!(
            hostnames(&list(&[
                "b.example.com",
                "a.example.com:1",
                "10.0.0.1",
                "a.example.com"
            ])),
            list(&["a.example.com", "b.example.com"])
        );
    }

    fn rules(text: &str) -> Vec<&str> {
        text.lines().filter(|l| l.starts_with("-A")).collect()
    }

    #[test]
    fn restricted_sources_allow_their_networks_dns_and_allowlist_then_reject() {
        let source = Source {
            label: "app:web".into(),
            addrs: vec!["172.18.0.5/32".into()],
            own_nets: vec!["172.18.0.0/16".into()],
            policy: Policy::Restricted,
            allowed: resolve_entries(
                &list(&["api.example.com:443,80", "198.51.100.0/24"]),
                &resolver,
            ),
        };
        let text = ruleset(&[source]);
        assert!(text.starts_with("*filter\n:TOND-EGRESS - [0:0]\n"));
        assert!(text.ends_with("COMMIT\n"));
        let c = "-m comment --comment tond-egress:app:web";
        let s = "-A TOND-EGRESS -s 172.18.0.5/32";
        assert_eq!(
            rules(&text),
            vec![
                format!("{s} -m conntrack --ctstate ESTABLISHED,RELATED {c} -j RETURN"),
                format!("{s} -d 172.18.0.0/16 {c} -j RETURN"),
                format!("{s} -p udp --dport 53 {c} -j RETURN"),
                format!("{s} -p tcp --dport 53 {c} -j RETURN"),
                format!("{s} -d 192.0.2.10 -p tcp -m multiport --dports 443,80 {c} -j RETURN"),
                format!("{s} -d 192.0.2.10 -p udp -m multiport --dports 443,80 {c} -j RETURN"),
                format!("{s} -d 192.0.2.11 -p tcp -m multiport --dports 443,80 {c} -j RETURN"),
                format!("{s} -d 192.0.2.11 -p udp -m multiport --dports 443,80 {c} -j RETURN"),
                format!("{s} -d 198.51.100.0/24 {c} -j RETURN"),
                format!("{s} -p tcp {c} -j REJECT --reject-with tcp-reset"),
                format!("{s} {c} -j REJECT --reject-with icmp-port-unreachable"),
            ],
            "replies, own networks and DNS first; the allowlist; everything else rejected last"
        );
    }

    #[test]
    fn none_ignores_the_allowlist_and_open_sources_get_no_rules() {
        let preview = Source {
            label: "preview:abc123".into(),
            addrs: vec!["172.30.0.0/16".into()],
            own_nets: vec!["172.30.0.0/16".into()],
            policy: Policy::None,
            allowed: vec![Allowed {
                dest: "192.0.2.1".into(),
                ports: vec![],
            }],
        };
        let open = Source {
            label: "app:free".into(),
            addrs: vec!["172.19.0.2/32".into()],
            own_nets: vec![],
            policy: Policy::Open,
            allowed: vec![],
        };
        let text = ruleset(&[preview, open]);
        assert!(!text.contains("192.0.2.1"), "none never uses the allowlist");
        assert!(!text.contains("172.19.0.2"), "open sources have no rules");
        let lines = rules(&text);
        assert_eq!(lines.len(), 6);
        assert!(lines.iter().all(|l| l.contains("-s 172.30.0.0/16")
            && l.contains("tond-egress:preview:abc123")));
        assert!(lines[5].ends_with("-j REJECT --reject-with icmp-port-unreachable"));
        assert_eq!(ruleset(&[]), "*filter\n:TOND-EGRESS - [0:0]\nCOMMIT\n");
    }

    #[test]
    fn shared_preview_networks_take_the_strictest_policy() {
        let a = (Policy::Restricted, list(&["a.example.com"]));
        let b = (
            Policy::Restricted,
            list(&["b.example.com", "a.example.com"]),
        );
        assert_eq!(
            merge_preview_policies(&[a.clone(), b]),
            (
                Policy::Restricted,
                list(&["a.example.com", "b.example.com"])
            )
        );
        assert_eq!(
            merge_preview_policies(&[a.clone(), (Policy::None, vec![])]),
            (Policy::None, vec![])
        );
        assert_eq!(
            merge_preview_policies(&[(Policy::Open, vec![]), a.clone()]).0,
            Policy::Restricted
        );
    }

    #[test]
    fn interface_addresses_map_to_their_subnet() {
        assert_eq!(
            subnet_of("172.18.0.5", 16).as_deref(),
            Some("172.18.0.0/16")
        );
        assert_eq!(subnet_of("10.1.2.3", 24).as_deref(), Some("10.1.2.0/24"));
        assert_eq!(subnet_of("10.1.2.3", 0), None);
        assert_eq!(subnet_of("fe80::1", 64), None);
    }

    #[test]
    fn warnings_say_why_a_policy_isnt_in_force() {
        let mut app = App::new("web".into(), "git".into(), None, None, 3000);
        let down = EgressStatus {
            available: false,
            error: Some("iptables isn't installed".into()),
            ..Default::default()
        };
        assert_eq!(app_warning(&app, &down), None, "open needs nothing");
        assert!(
            preview_warning(&app, &down).is_some_and(|w| w.contains("iptables isn't installed"))
        );
        app.egress_policy = "none".into();
        assert!(app_warning(&app, &down).is_some_and(|w| w.contains("Not enforced")));
        let up = EgressStatus {
            available: true,
            applied: BTreeSet::from(["app:web".to_string()]),
            ..Default::default()
        };
        assert_eq!(app_warning(&app, &up), None);
        assert_eq!(preview_warning(&app, &up), None);
        let stopped = EgressStatus {
            available: true,
            idle: BTreeSet::from(["web".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            app_warning(&app, &stopped),
            None,
            "no container, nothing to police yet"
        );
        let pending = EgressStatus {
            available: true,
            ..Default::default()
        };
        assert!(app_warning(&app, &pending).is_some_and(|w| w.contains("not loaded")));
    }
}
