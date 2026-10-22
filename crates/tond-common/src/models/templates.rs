// templates.rs — the one-click template catalog: manifest format and validation
//
// A template is a directory `templates/<id>/` with two files:
//
//   template.toml  what the catalog shows and what the deploy flow asks for
//   compose.yml    the Docker Compose file Tond deploys (an inline compose app)
//
// Both are embedded into the binaries at build time (build.rs), so the catalog
// needs no files on the server and can't drift from the release.
//
// The compose file is deployed through the normal compose path: it is checked
// by compose_policy (no privileged, no host network, binds only inside the
// app's volume directory) and `${VAR}` references in it are filled from the
// app's (encrypted) environment when `docker compose up` runs. A template's
// inputs therefore become app env vars; generated passwords are stored the
// same way as any other secret env var.
//
// template.toml
// -------------
//   id, name, description   slug, display name, one-line summary (≤ 140 chars)
//   about                   optional paragraph for the details view
//   category                one of TEMPLATE_CATEGORIES
//   tags                    search keywords
//   logo                    optional Simple Icons slug the dashboard maps to a logo
//   version                 upstream app version the images are pinned to
//   website/docs/repository optional links
//   ram_mb                  typical steady-state memory of all services together
//   architectures           CPU architectures its images exist for (default
//                           both: ["amd64", "arm64"])
//   setup_notes             shown after deploy (first login, what to do next)
//   upgrade_notes           how to move to a newer version safely
//   post_deploy_checks      short manual checklist
//   first_visitor_admin     true when the app lets whoever opens it first create
//                           the admin (or claim the install page) and Tond can't
//                           pre-seed one: the app page warns until dismissed
//
//   [public]                the service that gets the app's domain
//     service, port         compose service and the container port it serves HTTP on
//     health_path           optional HTTP readiness path (else a TCP probe)
//
//   [[inputs]]              env vars the compose file references as ${KEY}
//     key, label            env var name and form label
//     description           optional help text
//     kind                  text | email | password | url (form field type)
//     generate              password | hex | base64 → generated when left empty
//     value                 fixed computed value, never asked ("{app_url}",
//                           "{app_host}", "{app_name}" are substituted)
//     default               pre-filled form value (same placeholders)
//     required              default true
//     credential            show the final value once after deploy (first login)
//
//   [[post_deploy]]         commands that pre-seed the admin (or replace a fixed
//                           default login) on the first deploy: run with
//                           `docker exec` in a service container once every
//                           service is ready and BEFORE the app is routed, so
//                           nobody can reach the app first. Retried until they
//                           succeed or time out; a failure doesn't fail the
//                           deploy, it turns into the first-visitor warning.
//     service               compose service to run in
//     user                  optional exec user (a name or "uid:gid")
//     command               argv (use ["sh", "-c", "..."] for a script; read inputs
//                           as "$KEY", never inline secrets)
//     env                   input keys passed to the command as env vars
//     description           what it does ("Create the administrator account")
//     timeout_secs          retry budget (default 120, at most 600)
//
//   [[volumes]]             every bind mount in the compose file, declared
//     service, path         compose service and bind source ("./data")
//     description           what is stored there
//     owner                 optional "uid:gid" the directory is created with, for
//                           images that don't run as root
//
// Rules (enforced by `validate`, tested for every shipped template): every
// image pinned to a version tag or digest (no `latest`), no builds, every
// service has `restart` and `mem_limit`, only the public service publishes a
// port and only as "<port>" (Tond picks the loopback host port), data lives in
// declared `./…` binds (named volumes aren't in backups), every ${VAR} is an
// input and every input is used (by compose.yml or a post_deploy `env`). A service `healthcheck:` sets `interval` (at
// most 30s) and, with a `start_period`, a `start_interval` of at most 5s, so a
// deploy is ready in seconds. A service whose image ships a slow HEALTHCHECK
// (Docker's default interval is 30s) overrides its timings the same way; the
// image's own test is inherited when `test:` is left out.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use super::compose::{
    BACKUP_DUMP_LABEL, BACKUP_RESTORE_LABEL, check_backup_hook, compose_label,
    database_engine_of_image,
};

/// Memory a template may need and still carry the "fits on 2 GB" badge: a
/// 2 GB VPS keeps roughly 1.6 GB after the OS, Docker and Tond, so an app up
/// to 1 GB leaves room for its peaks and a small neighbour.
pub const FITS_2GB_MAX_RAM_MB: u32 = 1024;

pub const TEMPLATE_CATEGORIES: &[&str] = &[
    "analytics",
    "automation",
    "cms",
    "communication",
    "developer",
    "documents",
    "finance",
    "media",
    "monitoring",
    "productivity",
    "security",
    "storage",
];

/// Image tags that move: never allowed in a template.
const FLOATING_TAGS: &[&str] = &[
    "latest", "stable", "release", "edge", "nightly", "main", "master", "dev", "develop", "beta",
    "alpha", "next", "canary", "unstable", "rolling", "current", "lts",
];

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("template {id}: {message}")]
    Invalid { id: String, message: String },
}

fn invalid(id: &str, message: impl Into<String>) -> TemplateError {
    TemplateError::Invalid {
        id: id.to_string(),
        message: message.into(),
    }
}

// ── Manifest ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TemplateManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub about: Option<String>,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub logo: Option<String>,
    pub version: String,
    pub website: Option<String>,
    pub docs: Option<String>,
    pub repository: Option<String>,
    pub ram_mb: u32,
    #[serde(default = "all_architectures")]
    pub architectures: Vec<String>,
    pub setup_notes: Option<String>,
    pub upgrade_notes: Option<String>,
    #[serde(default)]
    pub post_deploy_checks: Vec<String>,
    /// The first visitor creates the admin and no pre-seeding is possible.
    #[serde(default)]
    pub first_visitor_admin: bool,
    pub public: TemplatePublic,
    #[serde(default)]
    pub inputs: Vec<TemplateInput>,
    #[serde(default)]
    pub volumes: Vec<TemplateVolume>,
    #[serde(default)]
    pub post_deploy: Vec<TemplatePostDeploy>,
}

/// Default retry budget of a post-deploy command.
pub const POST_DEPLOY_DEFAULT_TIMEOUT_SECS: u32 = 120;
/// Longest retry budget a post-deploy command may ask for.
pub const POST_DEPLOY_MAX_TIMEOUT_SECS: u32 = 600;

/// A command run once in a service container after the first successful
/// start, before the app is routed (see the module docs).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TemplatePostDeploy {
    pub service: String,
    pub user: Option<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    pub description: String,
    pub timeout_secs: Option<u32>,
}

impl TemplatePostDeploy {
    pub fn timeout_secs(&self) -> u32 {
        self.timeout_secs
            .unwrap_or(POST_DEPLOY_DEFAULT_TIMEOUT_SECS)
    }
}

/// How a template's admin account comes to exist.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TemplateAdminSetup {
    /// No admin account to claim (or it is set from an input at start).
    #[default]
    None,
    /// Tond creates it (or replaces the default login) before routing the app.
    Seeded,
    /// Whoever opens the app first creates it: open it right after deploy.
    FirstVisitor,
}

impl TemplateManifest {
    pub fn admin_setup(&self) -> TemplateAdminSetup {
        if !self.post_deploy.is_empty() {
            TemplateAdminSetup::Seeded
        } else if self.first_visitor_admin {
            TemplateAdminSetup::FirstVisitor
        } else {
            TemplateAdminSetup::None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TemplatePublic {
    pub service: String,
    pub port: u16,
    pub health_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TemplateInputKind {
    #[default]
    Text,
    Email,
    Password,
    Url,
}

/// How an empty input is filled in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TemplateGenerate {
    /// 24 letters and digits: safe in URLs, shells and connection strings.
    Password,
    /// 64 hex characters (32 random bytes).
    Hex,
    /// 64 base64 characters (48 random bytes).
    Base64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TemplateInput {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    #[serde(default)]
    pub kind: TemplateInputKind,
    pub generate: Option<TemplateGenerate>,
    pub value: Option<String>,
    pub default: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub credential: bool,
}

fn default_true() -> bool {
    true
}

/// CPU architectures Tond runs on, in Docker's naming.
pub const TEMPLATE_ARCHITECTURES: &[&str] = &["amd64", "arm64"];

fn all_architectures() -> Vec<String> {
    TEMPLATE_ARCHITECTURES
        .iter()
        .map(|a| a.to_string())
        .collect()
}

/// This server's CPU architecture in Docker's naming ("amd64", "arm64").
pub fn host_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Human name for an architecture ("x86_64", "ARM64").
pub fn architecture_label(arch: &str) -> &str {
    match arch {
        "amd64" => "x86_64",
        "arm64" => "ARM64",
        other => other,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TemplateVolume {
    pub service: String,
    pub path: String,
    pub description: String,
    pub owner: Option<String>,
}

impl TemplateVolume {
    /// The bind source relative to the app's volume directory ("./data" → "data").
    pub fn relative_path(&self) -> &str {
        self.path.trim_start_matches("./").trim_end_matches('/')
    }

    /// `owner` as (uid, gid).
    pub fn owner_ids(&self) -> Option<(u32, u32)> {
        let (uid, gid) = self.owner.as_deref()?.split_once(':')?;
        Some((uid.parse().ok()?, gid.parse().ok()?))
    }
}

// ── A template: manifest + compose ───────────────────────────────────────────

/// A parsed, validated template.
#[derive(Debug, Clone)]
pub struct Template {
    pub manifest: TemplateManifest,
    pub compose: String,
    pub services: Vec<TemplateService>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateService {
    pub name: String,
    pub image: String,
    /// The service that gets the app's domain.
    pub public: bool,
    /// Its `mem_limit` in MiB.
    pub memory_limit_mb: u32,
}

impl Template {
    /// Parse and validate a template from its two files.
    pub fn parse(
        dir_name: &str,
        manifest_toml: &str,
        compose: &str,
    ) -> Result<Self, TemplateError> {
        let manifest: TemplateManifest = toml::from_str(manifest_toml)
            .map_err(|e| invalid(dir_name, format!("template.toml: {e}")))?;
        if manifest.id != dir_name {
            return Err(invalid(
                dir_name,
                format!("id \"{}\" must match its directory name", manifest.id),
            ));
        }
        let services = validate(&manifest, compose)?;
        Ok(Self {
            manifest,
            compose: compose.to_string(),
            services,
        })
    }

    pub fn fits_2gb(&self) -> bool {
        self.manifest.ram_mb <= FITS_2GB_MAX_RAM_MB
    }
}

mod embedded {
    // Generated by build.rs: `pub static TEMPLATES: &[(&str, &str, &str)]`
    // as (directory name, template.toml, compose.yml).
    include!(concat!(env!("OUT_DIR"), "/templates_embedded.rs"));
}

/// Every template shipped with this build, sorted by name. Parsed on each
/// call (a few ms): the catalog is read rarely and isn't kept in memory.
pub fn builtin_templates() -> Result<Vec<Template>, TemplateError> {
    let mut out = embedded::TEMPLATES
        .iter()
        .map(|(dir, manifest, compose)| Template::parse(dir, manifest, compose))
        .collect::<Result<Vec<_>, _>>()?;
    out.sort_by(|a, b| {
        a.manifest
            .name
            .to_lowercase()
            .cmp(&b.manifest.name.to_lowercase())
    });
    Ok(out)
}

/// One shipped template by id.
pub fn builtin_template(id: &str) -> Result<Option<Template>, TemplateError> {
    embedded::TEMPLATES
        .iter()
        .find(|(dir, _, _)| *dir == id)
        .map(|(dir, manifest, compose)| Template::parse(dir, manifest, compose))
        .transpose()
}

// ── Values: placeholders and generation ──────────────────────────────────────

/// Values `value`/`default` placeholders expand to.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub app_name: String,
    /// The app's host name (its domain), e.g. "stats.example.com".
    pub app_host: String,
    /// "https://<host>" (or "http://" for `.localhost` hosts).
    pub app_url: String,
}

impl TemplateContext {
    pub fn new(app_name: &str, app_host: &str) -> Self {
        let scheme = if app_host == "localhost" || app_host.ends_with(".localhost") {
            "http"
        } else {
            "https"
        };
        Self {
            app_name: app_name.to_string(),
            app_host: app_host.to_string(),
            app_url: format!("{scheme}://{app_host}"),
        }
    }

    pub fn expand(&self, s: &str) -> String {
        s.replace("{app_url}", &self.app_url)
            .replace("{app_host}", &self.app_host)
            .replace("{app_name}", &self.app_name)
    }
}

const PLACEHOLDERS: &[&str] = &["{app_url}", "{app_host}", "{app_name}"];

/// Characters of a generated password.
const PASSWORD_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

pub fn generate_value(kind: TemplateGenerate) -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    match kind {
        TemplateGenerate::Password => {
            let mut out = String::with_capacity(24);
            while out.len() < 24 {
                let mut b = [0u8; 1];
                rng.fill_bytes(&mut b);
                // 248 = 4 × 62: reject the rest so every character is equally likely.
                if b[0] < 248 {
                    out.push(PASSWORD_ALPHABET[(b[0] % 62) as usize] as char);
                }
            }
            out
        }
        TemplateGenerate::Hex => {
            let mut b = [0u8; 32];
            rng.fill_bytes(&mut b);
            hex::encode(b)
        }
        TemplateGenerate::Base64 => {
            let mut b = [0u8; 48];
            rng.fill_bytes(&mut b);
            base64_encode(&b)
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Why an input value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputError {
    pub key: String,
    pub message: String,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The resolved environment of a template deploy.
#[derive(Debug, Clone, Default)]
pub struct ResolvedInputs {
    pub env: BTreeMap<String, String>,
    /// (key, label, value) of `credential` inputs, to show once.
    pub credentials: Vec<(String, String, String)>,
}

/// Fill every input: fixed `value`s are computed, provided values are checked,
/// empty generated inputs are generated, empty others take their default.
pub fn resolve_inputs(
    manifest: &TemplateManifest,
    provided: &BTreeMap<String, String>,
    ctx: &TemplateContext,
) -> Result<ResolvedInputs, InputError> {
    if let Some(key) = provided
        .keys()
        .find(|k| !manifest.inputs.iter().any(|i| &i.key == *k))
    {
        return Err(InputError {
            key: key.clone(),
            message: format!("{} has no input named {key}", manifest.name),
        });
    }
    let mut out = ResolvedInputs::default();
    for input in &manifest.inputs {
        let value = if let Some(fixed) = &input.value {
            ctx.expand(fixed)
        } else {
            let given = provided
                .get(&input.key)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty());
            match (given, input.generate, &input.default) {
                (Some(v), _, _) => {
                    check_input_value(input, v)?;
                    v.to_string()
                }
                (None, Some(kind), _) => generate_value(kind),
                (None, None, Some(default)) => ctx.expand(default),
                (None, None, None) if input.required => {
                    return Err(InputError {
                        key: input.key.clone(),
                        message: format!("{} is required", input.label),
                    });
                }
                (None, None, None) => String::new(),
            }
        };
        if input.credential {
            out.credentials
                .push((input.key.clone(), input.label.clone(), value.clone()));
        }
        out.env.insert(input.key.clone(), value);
    }
    Ok(out)
}

fn check_input_value(input: &TemplateInput, value: &str) -> Result<(), InputError> {
    let err = |message: String| InputError {
        key: input.key.clone(),
        message,
    };
    if value.len() > 4096 {
        return Err(err(format!("{} is too long", input.label)));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(err(format!(
            "{} can't contain control characters",
            input.label
        )));
    }
    match input.kind {
        TemplateInputKind::Email => {
            let valid = value.split_once('@').is_some_and(|(user, host)| {
                !user.is_empty() && host.contains('.') && !value.contains(' ')
            });
            if !valid {
                return Err(err(format!("{} must be an email address", input.label)));
            }
        }
        TemplateInputKind::Url => {
            if !(value.starts_with("https://") || value.starts_with("http://")) {
                return Err(err(format!(
                    "{} must start with https:// or http://",
                    input.label
                )));
            }
        }
        TemplateInputKind::Password => {
            if value.len() < 8 {
                return Err(err(format!(
                    "{} must be at least 8 characters",
                    input.label
                )));
            }
        }
        TemplateInputKind::Text => {}
    }
    Ok(())
}

// ── Compose helpers shared with tond-server ──────────────────────────────────

/// Env var names Tond never hands to `docker compose` for interpolation: they
/// change what the docker CLI does (DOCKER_HOST, COMPOSE_FILE, LD_PRELOAD, …)
/// rather than what goes into a container.
pub fn is_reserved_env_key(key: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "DOCKER_",
        "COMPOSE_",
        "BUILDKIT_",
        "BUILDX_",
        "LD_",
        "DYLD_",
        "GO",
        "XDG_",
        "SSL_",
        "OTEL_",
        "TOND_",
    ];
    const EXACT: &[&str] = &[
        "PATH",
        "HOME",
        "TMPDIR",
        "SHELL",
        "USER",
        "PWD",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "ALL_PROXY",
    ];
    let upper = key.to_ascii_uppercase();
    EXACT.contains(&upper.as_str()) || PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// Names of the variables a compose file interpolates (`$VAR`, `${VAR}`,
/// `${VAR:-default}`, …, nested defaults included; `$$` is a literal `$`).
/// Only string values are scanned, so comments don't count.
pub fn compose_variables(content: &str) -> Result<BTreeSet<String>, String> {
    let doc: Value = serde_yaml::from_str(content).map_err(|e| format!("invalid YAML: {e}"))?;
    let mut out = BTreeSet::new();
    walk_strings(&doc, &mut |s| scan_variables(s, &mut out));
    Ok(out)
}

fn walk_strings(v: &Value, f: &mut impl FnMut(&str)) {
    match v {
        Value::String(s) => f(s),
        Value::Sequence(seq) => seq.iter().for_each(|x| walk_strings(x, f)),
        Value::Mapping(m) => m.values().for_each(|x| walk_strings(x, f)),
        Value::Tagged(t) => walk_strings(&t.value, f),
        _ => {}
    }
}

fn scan_variables(s: &str, out: &mut BTreeSet<String>) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            i += 1;
            continue;
        }
        match b.get(i + 1) {
            Some(b'$') => i += 2,
            Some(b'{') => {
                let start = i + 2;
                let end = start + name_len(&b[start..]);
                if end > start {
                    out.insert(s[start..end].to_string());
                }
                i = end.max(i + 2);
            }
            Some(_) => {
                let start = i + 1;
                let end = start + name_len(&b[start..]);
                if end > start {
                    out.insert(s[start..end].to_string());
                }
                i = end.max(i + 1);
            }
            None => i += 1,
        }
    }
}

fn name_len(b: &[u8]) -> usize {
    let mut n = 0;
    while n < b.len() && (b[n].is_ascii_alphanumeric() || b[n] == b'_') {
        if n == 0 && b[n].is_ascii_digit() {
            return 0;
        }
        n += 1;
    }
    n
}

/// "256m", "1g", "512M", "1073741824" → MiB.
pub fn parse_memory_mb(s: &str) -> Option<u32> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.strip_suffix('b').unwrap_or(&s);
    let (num, mult): (&str, f64) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1.0 / 1024.0),
        'm' => (&s[..s.len() - 1], 1.0),
        'g' => (&s[..s.len() - 1], 1024.0),
        c if c.is_ascii_digit() => (s, 1.0 / (1024.0 * 1024.0)),
        _ => return None,
    };
    let n: f64 = num.trim().parse().ok()?;
    let mb = n * mult;
    (mb >= 1.0 && mb < f64::from(u32::MAX)).then_some(mb.round() as u32)
}

/// A compose duration ("10s", "1m30s", "500ms", "1h") in seconds.
pub fn parse_compose_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut rest = s;
    while !rest.is_empty() {
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_len == 0 {
            return None;
        }
        let n: f64 = rest[..num_len].parse().ok()?;
        rest = &rest[num_len..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let factor = match &rest[..unit_len] {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 1e-3,
            "us" | "µs" => 1e-6,
            "ns" => 1e-9,
            _ => return None,
        };
        total += n * factor;
        rest = &rest[unit_len..];
    }
    Some(total)
}

/// Longest healthcheck `interval` a template may set.
pub const TEMPLATE_MAX_HEALTHCHECK_INTERVAL_SECS: f64 = 30.0;
/// Longest healthcheck `start_interval` a template may set.
pub const TEMPLATE_MAX_HEALTHCHECK_START_INTERVAL_SECS: f64 = 5.0;

/// A template service's `healthcheck:` keeps deploys fast: an explicit
/// `interval` of at most 30s and, with a `start_period`, a short
/// `start_interval` (checks run that often while the service boots).
fn check_template_healthcheck(name: &str, hc: &Value) -> Result<(), String> {
    let Some(hc) = hc.as_mapping() else {
        return Err(format!("service {name}: 'healthcheck' must be a mapping"));
    };
    if hc
        .get("disable")
        .is_some_and(|d| matches!(d, Value::Bool(true)))
    {
        return Ok(());
    }
    let duration = |key: &str| -> Result<Option<f64>, String> {
        match hc.get(key) {
            None => Ok(None),
            Some(v) => scalar(v)
                .as_deref()
                .and_then(parse_compose_duration)
                .map(Some)
                .ok_or_else(|| {
                    format!("service {name}: healthcheck {key} must be a duration like 10s")
                }),
        }
    };
    match duration("interval")? {
        None => {
            return Err(format!(
                "service {name}: set healthcheck interval (at most 30s; Docker's default of 30s keeps a deploy waiting)"
            ));
        }
        Some(i) if i > TEMPLATE_MAX_HEALTHCHECK_INTERVAL_SECS => {
            return Err(format!(
                "service {name}: healthcheck interval must be at most 30s so a deploy doesn't wait for it"
            ));
        }
        Some(_) => {}
    }
    let start_interval = duration("start_interval")?;
    if start_interval.is_some_and(|s| s > TEMPLATE_MAX_HEALTHCHECK_START_INTERVAL_SECS) {
        return Err(format!(
            "service {name}: healthcheck start_interval must be at most 5s"
        ));
    }
    if duration("start_period")?.is_some_and(|p| p > 0.0) && start_interval.is_none() {
        return Err(format!(
            "service {name}: a healthcheck start_period needs a start_interval (e.g. 2s) so the service is seen healthy as soon as it is up"
        ));
    }
    Ok(())
}

/// Whether an image reference is pinned: a digest, or a tag with a version
/// number that isn't one of the moving tags.
pub fn image_is_pinned(image: &str) -> bool {
    if image.contains("@sha256:") {
        return true;
    }
    // The tag is after the last ':' that follows the last '/' (a registry
    // port like "host:5000/app" isn't a tag).
    let last = image.rsplit('/').next().unwrap_or(image);
    let Some((_, tag)) = last.rsplit_once(':') else {
        return false;
    };
    let lower = tag.to_ascii_lowercase();
    tag.chars().any(|c| c.is_ascii_digit())
        && !FLOATING_TAGS.contains(&lower.as_str())
        && !lower.contains("nightly")
        && !lower.starts_with("latest")
}

// ── Validation ───────────────────────────────────────────────────────────────

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn is_env_key(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Tagged(t) => scalar(&t.value),
        _ => None,
    }
}

/// Validate a manifest against its compose file; returns the compose services.
pub fn validate(
    m: &TemplateManifest,
    compose: &str,
) -> Result<Vec<TemplateService>, TemplateError> {
    let id = m.id.as_str();
    let fail = |msg: String| Err(invalid(id, msg));

    if !is_slug(id) {
        return fail("id must be a lowercase slug (a-z, 0-9, -)".into());
    }
    for (field, value) in [
        ("name", &m.name),
        ("description", &m.description),
        ("version", &m.version),
    ] {
        if value.trim().is_empty() {
            return fail(format!("{field} must not be empty"));
        }
    }
    if m.description.chars().count() > 140 {
        return fail("description must be one line of at most 140 characters".into());
    }
    if !TEMPLATE_CATEGORIES.contains(&m.category.as_str()) {
        return fail(format!(
            "category \"{}\" must be one of: {}",
            m.category,
            TEMPLATE_CATEGORIES.join(", ")
        ));
    }
    if m.architectures.is_empty()
        || m.architectures
            .iter()
            .any(|a| !TEMPLATE_ARCHITECTURES.contains(&a.as_str()))
    {
        return fail(format!(
            "architectures must list some of: {}",
            TEMPLATE_ARCHITECTURES.join(", ")
        ));
    }
    if m.ram_mb == 0 {
        return fail("ram_mb must be greater than 0".into());
    }
    for (field, link) in [
        ("website", &m.website),
        ("docs", &m.docs),
        ("repository", &m.repository),
    ] {
        if let Some(link) = link
            && !link.starts_with("https://")
        {
            return fail(format!("{field} must be an https:// link"));
        }
    }
    if let Some(logo) = &m.logo
        && !is_slug(logo)
    {
        return fail("logo must be a Simple Icons slug".into());
    }
    if m.public.port == 0 {
        return fail("public.port must be greater than 0".into());
    }
    if let Some(path) = &m.public.health_path
        && !path.starts_with('/')
    {
        return fail("public.health_path must start with /".into());
    }

    // Inputs.
    let mut keys = BTreeSet::new();
    for input in &m.inputs {
        if !is_env_key(&input.key) {
            return fail(format!(
                "input {}: key must be an UPPER_SNAKE_CASE env var name",
                input.key
            ));
        }
        if is_reserved_env_key(&input.key) {
            return fail(format!(
                "input {}: this name is reserved (it would change how docker compose runs)",
                input.key
            ));
        }
        if !keys.insert(input.key.as_str()) {
            return fail(format!("input {} is declared twice", input.key));
        }
        if input.label.trim().is_empty() {
            return fail(format!("input {}: label must not be empty", input.key));
        }
        let sources = [
            input.value.is_some(),
            input.generate.is_some(),
            input.default.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if input.value.is_some() && sources > 1 {
            return fail(format!(
                "input {}: a fixed value can't also have generate or default",
                input.key
            ));
        }
        for text in [&input.value, &input.default].into_iter().flatten() {
            let mut rest = text.as_str();
            while let Some(start) = rest.find('{') {
                let Some(len) = rest[start..].find('}') else {
                    break;
                };
                let token = &rest[start..start + len + 1];
                if !PLACEHOLDERS.contains(&token) {
                    return fail(format!(
                        "input {}: unknown placeholder {token} (use {})",
                        input.key,
                        PLACEHOLDERS.join(", ")
                    ));
                }
                rest = &rest[start + len + 1..];
            }
        }
    }

    // Compose.
    let doc: Value = serde_yaml::from_str(compose)
        .map_err(|e| invalid(id, format!("compose.yml is not valid YAML: {e}")))?;
    let Some(root) = doc.as_mapping() else {
        return fail("compose.yml must be a mapping with 'services:'".into());
    };
    if root.contains_key("volumes") {
        return fail(
            "named volumes aren't allowed: they aren't in Tond's backups; bind ./<dir> instead"
                .into(),
        );
    }
    let Some(services) = root.get("services").and_then(Value::as_mapping) else {
        return fail("compose.yml must define 'services:'".into());
    };
    if !services
        .keys()
        .any(|k| scalar(k).as_deref() == Some(m.public.service.as_str()))
    {
        return fail(format!(
            "public.service \"{}\" is not a service in compose.yml",
            m.public.service
        ));
    }

    let mut out = Vec::new();
    let mut binds: BTreeSet<(String, String)> = BTreeSet::new();
    let mut total_limit = 0u32;
    for (name, svc) in services {
        let name = scalar(name).unwrap_or_default();
        let Some(svc) = svc.as_mapping() else {
            return fail(format!("service {name} must be a mapping"));
        };
        let get = |k: &str| svc.get(k);
        if get("build").is_some() {
            return fail(format!(
                "service {name}: templates use prebuilt images, not 'build'"
            ));
        }
        let Some(image) = get("image").and_then(scalar) else {
            return fail(format!("service {name}: 'image' is required"));
        };
        if image.contains('$') {
            return fail(format!(
                "service {name}: the image must be written literally"
            ));
        }
        if !image_is_pinned(&image) {
            return fail(format!(
                "service {name}: image {image} must be pinned to a version tag or digest, not a moving tag"
            ));
        }
        if get("restart").and_then(scalar).is_none_or(|r| r == "no") {
            return fail(format!(
                "service {name}: set 'restart: unless-stopped' so it comes back after a reboot"
            ));
        }
        let Some(limit) = get("mem_limit")
            .and_then(scalar)
            .and_then(|s| parse_memory_mb(&s))
        else {
            return fail(format!("service {name}: set 'mem_limit' (e.g. 256m)"));
        };
        total_limit = total_limit.saturating_add(limit);
        if get("container_name").is_some() {
            return fail(format!(
                "service {name}: 'container_name' clashes between apps; leave it out"
            ));
        }

        if let Some(hc) = get("healthcheck") {
            check_template_healthcheck(&name, hc).map_err(|e| invalid(id, e))?;
        }

        let public = name == m.public.service;
        match get("ports") {
            None if public => {
                return fail(format!(
                    "service {name}: the public service must publish its port as ports: [\"{}\"]",
                    m.public.port
                ));
            }
            None => {}
            Some(ports) => {
                if !public {
                    return fail(format!(
                        "service {name}: only the public service publishes a port; others are reached over the app network"
                    ));
                }
                let items: Vec<String> = ports
                    .as_sequence()
                    .map(|s| s.iter().filter_map(scalar).collect())
                    .unwrap_or_default();
                if items != [m.public.port.to_string()] {
                    return fail(format!(
                        "service {name}: publish exactly ports: [\"{}\"] (Tond picks the host port)",
                        m.public.port
                    ));
                }
            }
        }

        // Backup hooks: a database's files copied while it runs aren't
        // consistent, so a database that keeps data in a volume must declare
        // a dump (BACKUP_DUMP_LABEL).
        let labels = get("labels");
        let dump = labels.and_then(|l| compose_label(l, BACKUP_DUMP_LABEL));
        let restore = labels.and_then(|l| compose_label(l, BACKUP_RESTORE_LABEL));
        for (label, value) in [(BACKUP_DUMP_LABEL, &dump), (BACKUP_RESTORE_LABEL, &restore)] {
            if let Some(value) = value {
                check_backup_hook(label, value)
                    .map_err(|e| invalid(id, format!("service {name}: {e}")))?;
            }
        }
        if restore.is_some() && dump.is_none() {
            return fail(format!(
                "service {name}: {BACKUP_RESTORE_LABEL} needs a {BACKUP_DUMP_LABEL} to restore from"
            ));
        }
        if let Some(engine) = database_engine_of_image(&image)
            && get("volumes").is_some()
            && dump.is_none()
        {
            return fail(format!(
                "service {name}: {engine} data copied while it runs isn't consistent; add a \
                 '{BACKUP_DUMP_LABEL}' label (and '{BACKUP_RESTORE_LABEL}' to load it back)"
            ));
        }

        if let Some(vols) = get("volumes") {
            let Some(vols) = vols.as_sequence() else {
                return fail(format!("service {name}: 'volumes' must be a list"));
            };
            for v in vols {
                let Some(spec) = scalar(v) else {
                    return fail(format!(
                        "service {name}: write volumes in short form (./dir:/path)"
                    ));
                };
                let src = spec.split(':').next().unwrap_or_default();
                if spec.split(':').count() < 2 {
                    return fail(format!(
                        "service {name}: anonymous volume {spec} isn't backed up; bind ./<dir>:{spec}"
                    ));
                }
                if !src.starts_with("./") || src.contains("..") || src.len() <= 2 {
                    return fail(format!(
                        "service {name}: volume {spec} must bind a directory under ./ (named volumes aren't backed up)"
                    ));
                }
                binds.insert((name.clone(), src.trim_end_matches('/').to_string()));
            }
        }

        out.push(TemplateService {
            name: name.clone(),
            image,
            public,
            memory_limit_mb: limit,
        });
    }

    if m.ram_mb > total_limit {
        return fail(format!(
            "ram_mb ({}) is more than the services' mem_limits allow ({total_limit} MiB)",
            m.ram_mb
        ));
    }

    // Volumes: exactly the binds the compose file uses.
    let mut declared = BTreeSet::new();
    for v in &m.volumes {
        let path = v.path.trim_end_matches('/').to_string();
        if !path.starts_with("./") || path.contains("..") || path.len() <= 2 {
            return fail(format!("volume {}: path must be ./<dir>", v.path));
        }
        if v.description.trim().is_empty() {
            return fail(format!("volume {}: description must not be empty", v.path));
        }
        if v.owner.is_some() && v.owner_ids().is_none() {
            return fail(format!("volume {}: owner must be \"uid:gid\"", v.path));
        }
        if !binds.contains(&(v.service.clone(), path.clone())) {
            return fail(format!(
                "volume {} is declared for service {} but compose.yml doesn't bind it there",
                v.path, v.service
            ));
        }
        declared.insert((v.service.clone(), path));
    }
    if let Some((svc, path)) = binds.difference(&declared).next() {
        return fail(format!(
            "service {svc} binds {path}; declare it under [[volumes]] so users see what is stored"
        ));
    }

    // Post-deploy commands.
    if m.first_visitor_admin && !m.post_deploy.is_empty() {
        return fail(
            "first_visitor_admin means the admin can't be pre-seeded; drop it when post_deploy seeds one"
                .into(),
        );
    }
    let mut hook_env: BTreeSet<&str> = BTreeSet::new();
    for (i, hook) in m.post_deploy.iter().enumerate() {
        let n = i + 1;
        if !out.iter().any(|s| s.name == hook.service) {
            return fail(format!(
                "post_deploy {n}: service \"{}\" is not a service in compose.yml",
                hook.service
            ));
        }
        if hook.description.trim().is_empty() {
            return fail(format!("post_deploy {n}: description must not be empty"));
        }
        if hook.command.first().is_none_or(|c| c.trim().is_empty()) {
            return fail(format!("post_deploy {n}: command must not be empty"));
        }
        if let Some(user) = &hook.user
            && (user.is_empty()
                || !user
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':')))
        {
            return fail(format!(
                "post_deploy {n}: user must be a user name or uid[:gid]"
            ));
        }
        if hook.timeout_secs() == 0 || hook.timeout_secs() > POST_DEPLOY_MAX_TIMEOUT_SECS {
            return fail(format!(
                "post_deploy {n}: timeout_secs must be 1–{POST_DEPLOY_MAX_TIMEOUT_SECS}"
            ));
        }
        for key in &hook.env {
            if !keys.contains(key.as_str()) {
                return fail(format!("post_deploy {n}: env {key} isn't an input"));
            }
            hook_env.insert(key.as_str());
        }
    }

    // Variables: every ${VAR} is an input, every input is used.
    let vars = compose_variables(compose).map_err(|e| invalid(id, e))?;
    if let Some(var) = vars.iter().find(|v| !keys.contains(v.as_str())) {
        return fail(format!(
            "compose.yml uses ${{{var}}}, which isn't an input (write a literal $ as $$)"
        ));
    }
    if let Some(key) = keys
        .iter()
        .find(|k| !vars.contains(**k) && !hook_env.contains(**k))
    {
        return fail(format!(
            "input {key} isn't used in compose.yml or by a post_deploy command"
        ));
    }

    out.sort_by_key(|s| (!s.public, s.name.clone()));
    Ok(out)
}

// ── API shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateInputResponse {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: TemplateInputKind,
    pub required: bool,
    /// Left empty, a random value is generated.
    pub generated: bool,
    /// Pre-filled form value (placeholders unexpanded, e.g. "{app_url}").
    pub default: Option<String>,
    /// Shown once after deploy.
    pub credential: bool,
}

/// GET /api/templates item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub about: Option<String>,
    pub category: String,
    pub tags: Vec<String>,
    pub logo: Option<String>,
    pub version: String,
    pub website: Option<String>,
    pub docs: Option<String>,
    pub repository: Option<String>,
    pub ram_mb: u32,
    pub fits_2gb: bool,
    /// CPU architectures its images exist for ("amd64", "arm64").
    #[serde(default = "all_architectures")]
    pub architectures: Vec<String>,
    pub public: TemplatePublic,
    pub services: Vec<TemplateService>,
    pub volumes: Vec<TemplateVolume>,
    /// Inputs the deploy form shows (fixed computed values are left out).
    pub inputs: Vec<TemplateInputResponse>,
    pub setup_notes: Option<String>,
    pub upgrade_notes: Option<String>,
    pub post_deploy_checks: Vec<String>,
    /// How the admin account comes to exist: pre-seeded by Tond, created by
    /// the first visitor, or nothing to claim.
    #[serde(default)]
    pub admin_setup: TemplateAdminSetup,
    pub compose: String,
}

impl From<Template> for TemplateResponse {
    fn from(t: Template) -> Self {
        let fits_2gb = t.fits_2gb();
        let admin_setup = t.manifest.admin_setup();
        let m = t.manifest;
        Self {
            inputs: m
                .inputs
                .iter()
                .filter(|i| i.value.is_none())
                .map(|i| TemplateInputResponse {
                    key: i.key.clone(),
                    label: i.label.clone(),
                    description: i.description.clone(),
                    kind: i.kind,
                    required: i.required && i.generate.is_none() && i.default.is_none(),
                    generated: i.generate.is_some(),
                    default: i.default.clone(),
                    credential: i.credential,
                })
                .collect(),
            id: m.id,
            name: m.name,
            description: m.description,
            about: m.about,
            category: m.category,
            tags: m.tags,
            logo: m.logo,
            version: m.version,
            website: m.website,
            docs: m.docs,
            repository: m.repository,
            ram_mb: m.ram_mb,
            fits_2gb,
            architectures: m.architectures,
            public: m.public,
            services: t.services,
            volumes: m.volumes,
            setup_notes: m.setup_notes,
            upgrade_notes: m.upgrade_notes,
            post_deploy_checks: m.post_deploy_checks,
            admin_setup,
            compose: t.compose,
        }
    }
}

/// Host memory as the catalog shows it, from the host collector's last sample.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateHostMemory {
    pub total_mb: u64,
    /// What the kernel reports as available (total minus used).
    pub available_mb: u64,
}

/// GET /api/templates
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateCatalogResponse {
    pub templates: Vec<TemplateResponse>,
    /// None until the host collector's first sample.
    pub memory: Option<TemplateHostMemory>,
    /// Apps get `<name>.<apps_domain>` when no domain is given ("" = none).
    pub apps_domain: String,
    /// This server's CPU architecture ("amd64" | "arm64").
    #[serde(default)]
    pub architecture: String,
}

/// POST /api/templates/{id}/deploy
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDeployRequest {
    /// App name (lowercase letters, digits, hyphens).
    pub name: String,
    pub domain: Option<String>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Deploy even though the template needs more memory than is free.
    #[serde(default)]
    pub allow_low_memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateCredential {
    pub key: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDeployResponse {
    pub app_id: String,
    pub app_name: String,
    pub domain: Option<String>,
    pub deployment_id: String,
    /// First-login credentials and other `credential` inputs, shown once.
    pub credentials: Vec<TemplateCredential>,
    pub setup_notes: Option<String>,
}

// ── Apps deployed from a template: domain-derived env, upgrades ─────────────

/// True when `s` refers to the app's address.
fn mentions_host(s: &str) -> bool {
    s.contains("{app_url}") || s.contains("{app_host}")
}

/// The env keys of a template deploy whose value comes from the app's
/// address, with the placeholder text they were expanded from: fixed
/// `value`s that mention `{app_url}`/`{app_host}`, and `default`s that do
/// when the deploy kept the default (the resolved value is its expansion).
pub fn domain_env_templates(
    manifest: &TemplateManifest,
    env: &BTreeMap<String, String>,
    ctx: &TemplateContext,
) -> BTreeMap<String, String> {
    manifest
        .inputs
        .iter()
        .filter_map(|input| {
            let template = input.value.as_ref().or(input.default.as_ref())?;
            let derived = mentions_host(template)
                && (input.value.is_some() || env.get(&input.key) == Some(&ctx.expand(template)));
            derived.then(|| (input.key.clone(), template.clone()))
        })
        .collect()
}

/// Whether template version `candidate` is newer than `current`: dotted
/// parts compare numerically where both are numbers ("1.10" > "1.9"), a
/// leading "v" is ignored, and a longer version wins a tie ("2.0.1" > "2.0").
pub fn template_version_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<&str> {
        v.trim()
            .trim_start_matches(['v', 'V'])
            .split(['.', '-', '+'])
            .collect()
    }
    let (a, b) = (parts(candidate), parts(current));
    for (x, y) in a.iter().zip(b.iter()) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord == std::cmp::Ordering::Greater;
        }
    }
    a.len() > b.len()
}

/// One env value derived from the app's address that no longer matches it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDomainEnvChange {
    pub key: String,
    /// The value the app has now.
    pub current: String,
    /// The value for the app's current primary domain.
    pub proposed: String,
}

/// `GET /api/v1/apps/{id}/template`: where an app deployed from a template
/// stands against the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateAppStatus {
    pub template_id: String,
    pub template_name: Option<String>,
    /// The template version the app runs.
    pub version: Option<String>,
    /// The catalog's version (None when the catalog no longer has it).
    pub latest_version: Option<String>,
    /// The catalog has a newer version.
    pub update_available: bool,
    /// The newer version's upgrade notes.
    pub upgrade_notes: Option<String>,
    /// Why a one-click upgrade isn't possible (for example, the compose file
    /// was edited since it was deployed); None when it is.
    pub upgrade_blocked: Option<String>,
    /// Env values derived from the app's address (`{app_url}`, `{app_host}`)
    /// that don't match its current primary domain.
    pub domain_env: Vec<TemplateDomainEnvChange>,
    /// Domain-derived keys left alone because their value was edited by hand.
    pub domain_env_edited: Vec<String>,
}

/// `POST /api/v1/apps/{id}/template/domain-env`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDomainEnvRequest {
    /// Start a deploy afterwards so the containers get the new values.
    #[serde(default)]
    pub redeploy: bool,
}

/// `POST /api/v1/apps/{id}/template/domain-env` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateDomainEnvResponse {
    pub updated: Vec<TemplateDomainEnvChange>,
    /// The deploy started with `redeploy: true`.
    pub deployment_id: Option<String>,
}

/// `POST /api/v1/apps/{id}/template/upgrade` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TemplateUpgradeResponse {
    pub from_version: Option<String>,
    pub to_version: String,
    /// The snapshot of the app's data taken before anything changed.
    pub snapshot_id: Option<String>,
    pub deployment_id: String,
    /// Env vars the new version added (generated or defaulted).
    pub added_env: Vec<String>,
    /// New `credential` inputs, shown once.
    pub credentials: Vec<TemplateCredential>,
    pub upgrade_notes: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
id = "demo"
name = "Demo"
description = "A demo app"
category = "developer"
version = "1.2"
ram_mb = 200

[public]
service = "web"
port = 8080
health_path = "/health"

[[inputs]]
key = "ADMIN_PASSWORD"
label = "Admin password"
kind = "password"
generate = "password"
credential = true

[[inputs]]
key = "BASE_URL"
label = "URL"
value = "{app_url}"

[[inputs]]
key = "ADMIN_EMAIL"
label = "Admin email"
kind = "email"

[[volumes]]
service = "web"
path = "./data"
description = "Everything"
owner = "1000:1000"
"#;

    const COMPOSE: &str = r#"
services:
  web:
    image: example/demo:1.2.3
    restart: unless-stopped
    mem_limit: 256m
    ports: ["8080"]
    environment:
      PASSWORD: ${ADMIN_PASSWORD}
      URL: "${BASE_URL:-http://x}"
      EMAIL: $ADMIN_EMAIL
      LITERAL: "$$HOME"
    volumes:
      - ./data:/data
"#;

    fn demo() -> Template {
        Template::parse("demo", MANIFEST, COMPOSE).unwrap_or_else(|e| panic!("{e}"))
    }

    fn err(manifest: &str, compose: &str) -> String {
        match Template::parse("demo", manifest, compose) {
            Ok(_) => panic!("expected the template to be rejected"),
            Err(e) => e.to_string(),
        }
    }

    fn assert_err(manifest: &str, compose: &str, needle: &str) {
        let e = err(manifest, compose);
        assert!(e.contains(needle), "expected {needle:?} in {e:?}");
    }

    #[test]
    fn template_versions_compare_by_number() {
        assert!(template_version_newer("1.10", "1.9"));
        assert!(template_version_newer("v2.0.1", "2.0"));
        assert!(!template_version_newer("2.0", "2.0"));
        assert!(!template_version_newer("1.9.9", "1.10"));
        assert!(template_version_newer("2025.10", "2025.9.3"));
    }

    #[test]
    fn domain_derived_env_keys_are_recorded_with_their_placeholder() {
        let manifest: TemplateManifest = toml::from_str(MANIFEST).unwrap_or_else(|e| panic!("{e}"));
        let ctx = TemplateContext::new("stats", "stats.example.com");
        let given = BTreeMap::from([("ADMIN_EMAIL".to_string(), "a@example.com".to_string())]);
        let resolved = resolve_inputs(&manifest, &given, &ctx).unwrap_or_else(|e| panic!("{e}"));
        let derived = domain_env_templates(&manifest, &resolved.env, &ctx);
        assert_eq!(
            derived,
            BTreeMap::from([("BASE_URL".to_string(), "{app_url}".to_string())])
        );
    }

    #[test]
    fn every_shipped_template_is_valid() {
        let all = builtin_templates().unwrap_or_else(|e| panic!("{e}"));
        assert!(
            all.len() >= 25,
            "the catalog should ship 25+ templates, has {}",
            all.len()
        );
        let mut ids = BTreeSet::new();
        for t in &all {
            assert!(
                ids.insert(t.manifest.id.clone()),
                "duplicate id {}",
                t.manifest.id
            );
            assert!(
                t.manifest.setup_notes.is_some(),
                "{}: setup_notes tell users what to do first",
                t.manifest.id
            );
            assert!(
                t.manifest.upgrade_notes.is_some(),
                "{}: upgrade_notes are required",
                t.manifest.id
            );
            assert!(
                t.manifest.docs.is_some(),
                "{}: link the upstream docs",
                t.manifest.id
            );
            for s in &t.services {
                assert!(
                    image_is_pinned(&s.image),
                    "{}: {} not pinned",
                    t.manifest.id,
                    s.image
                );
            }
        }
        assert!(all.iter().any(|t| t.fits_2gb()) && all.iter().any(|t| !t.fits_2gb()));
    }

    #[test]
    fn builtin_template_finds_by_id() {
        assert!(
            builtin_template("umami")
                .unwrap_or_else(|e| panic!("{e}"))
                .is_some()
        );
        assert!(
            builtin_template("nope")
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    #[test]
    fn parses_services_volumes_and_inputs() {
        let t = demo();
        assert_eq!(t.services.len(), 1);
        assert!(t.services[0].public);
        assert_eq!(t.services[0].memory_limit_mb, 256);
        assert_eq!(t.manifest.volumes[0].owner_ids(), Some((1000, 1000)));
        assert_eq!(t.manifest.volumes[0].relative_path(), "data");
        assert!(t.fits_2gb());
        let resp = TemplateResponse::from(t);
        assert_eq!(resp.inputs.len(), 2, "fixed values are not form inputs");
        assert!(
            resp.inputs
                .iter()
                .any(|i| i.key == "ADMIN_PASSWORD" && i.generated && !i.required)
        );
        assert!(
            resp.inputs
                .iter()
                .any(|i| i.key == "ADMIN_EMAIL" && i.required)
        );
        assert_eq!(resp.admin_setup, TemplateAdminSetup::None);
    }

    const HOOK: &str = r#"
[[post_deploy]]
service = "web"
user = "1000:1000"
command = ["sh", "-c", "demo admin create --password \"$SEED_PASSWORD\""]
env = ["SEED_PASSWORD"]
description = "Create the administrator account"
"#;

    fn with_hook(hook: &str) -> String {
        format!(
            "{MANIFEST}\n[[inputs]]\nkey = \"SEED_PASSWORD\"\nlabel = \"Admin password\"\ngenerate = \"password\"\ncredential = true\n{hook}"
        )
    }

    #[test]
    fn post_deploy_commands_are_parsed_and_their_env_counts_as_use() {
        let t =
            Template::parse("demo", &with_hook(HOOK), COMPOSE).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.manifest.post_deploy.len(), 1);
        let hook = &t.manifest.post_deploy[0];
        assert_eq!(hook.user.as_deref(), Some("1000:1000"));
        assert_eq!(hook.env, ["SEED_PASSWORD"]);
        assert_eq!(hook.timeout_secs(), POST_DEPLOY_DEFAULT_TIMEOUT_SECS);
        assert_eq!(t.manifest.admin_setup(), TemplateAdminSetup::Seeded);
        assert_eq!(
            TemplateResponse::from(t).admin_setup,
            TemplateAdminSetup::Seeded
        );
        // Without the hook the input would be unused.
        assert_err(&with_hook(""), COMPOSE, "SEED_PASSWORD isn't used");
    }

    #[test]
    fn post_deploy_commands_are_validated() {
        assert_err(
            &with_hook(&HOOK.replace("service = \"web\"", "service = \"db\"")),
            COMPOSE,
            "post_deploy 1: service \"db\"",
        );
        assert_err(
            &with_hook(&HOOK.replace(
                "env = [\"SEED_PASSWORD\"]",
                "env = [\"SEED_PASSWORD\", \"NOPE\"]",
            )),
            COMPOSE,
            "env NOPE isn't an input",
        );
        assert_err(
            &with_hook(&HOOK.replace("user = \"1000:1000\"", "user = \"root; rm\"")),
            COMPOSE,
            "user must be",
        );
        assert_err(
            &with_hook(&format!("{HOOK}timeout_secs = 9000\n")),
            COMPOSE,
            "timeout_secs",
        );
        assert_err(
            &with_hook(&HOOK.replace(
                "command = [\"sh\", \"-c\", \"demo admin create --password \\\"$SEED_PASSWORD\\\"\"]",
                "command = []",
            )),
            COMPOSE,
            "command must not be empty",
        );
        assert_err(
            &with_hook(&HOOK.replace(
                "description = \"Create the administrator account\"",
                "description = \" \"",
            )),
            COMPOSE,
            "description must not be empty",
        );
        let both =
            with_hook(HOOK).replace("ram_mb = 200", "ram_mb = 200\nfirst_visitor_admin = true");
        assert_err(&both, COMPOSE, "first_visitor_admin");
    }

    #[test]
    fn first_visitor_flag_sets_the_admin_setup() {
        let m = MANIFEST.replace("ram_mb = 200", "ram_mb = 200\nfirst_visitor_admin = true");
        let t = Template::parse("demo", &m, COMPOSE).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.manifest.admin_setup(), TemplateAdminSetup::FirstVisitor);
    }

    /// Apps that hand the admin account to whoever opens them first, or ship
    /// a fixed default login: each template either pre-seeds the admin with
    /// a post_deploy command or carries the first-visitor warning.
    #[test]
    fn templates_with_a_claimable_admin_seed_it_or_warn() {
        let all = builtin_templates().unwrap_or_else(|e| panic!("{e}"));
        let expect = |id: &str, setup: TemplateAdminSetup| {
            let t = all
                .iter()
                .find(|t| t.manifest.id == id)
                .unwrap_or_else(|| panic!("{id} should be in the catalog"));
            assert_eq!(t.manifest.admin_setup(), setup, "{id}");
            if setup == TemplateAdminSetup::Seeded {
                assert!(
                    t.manifest.inputs.iter().any(|i| i.credential),
                    "{id}: a seeded admin's password is shown once (credential = true)"
                );
            }
        };
        for id in [
            "forgejo",
            "gitea",
            "mattermost",
            "actual",
            "audiobookshelf",
            "umami",
            "mealie",
        ] {
            expect(id, TemplateAdminSetup::Seeded);
        }
        for id in [
            "ghost",
            "uptime-kuma",
            "memos",
            "wikijs",
            "docmost",
            "twenty",
        ] {
            expect(id, TemplateAdminSetup::FirstVisitor);
        }
    }

    #[test]
    fn rejects_moving_or_missing_tags() {
        for image in [
            "example/demo",
            "example/demo:latest",
            "example/demo:stable",
            "localhost:5000/demo",
            "x/y:nightly-2024",
        ] {
            assert!(
                !image_is_pinned(image),
                "{image} should not count as pinned"
            );
        }
        for image in [
            "example/demo:1",
            "postgres:16-alpine",
            "ghcr.io/a/b:v2.3.1",
            "localhost:5000/demo:1.0",
            "x/y@sha256:abc",
        ] {
            assert!(image_is_pinned(image), "{image} should count as pinned");
        }
        assert_err(
            MANIFEST,
            &COMPOSE.replace("example/demo:1.2.3", "example/demo:latest"),
            "pinned",
        );
    }

    #[test]
    fn rejects_undeclared_variables_and_unused_inputs() {
        assert_err(
            MANIFEST,
            &COMPOSE.replace("$ADMIN_EMAIL", "${OTHER}"),
            "${OTHER}",
        );
        assert_err(
            MANIFEST,
            &COMPOSE.replace("EMAIL: $ADMIN_EMAIL", "EMAIL: x"),
            "ADMIN_EMAIL isn't used",
        );
    }

    #[test]
    fn rejects_undeclared_or_named_volumes() {
        assert_err(
            MANIFEST,
            &COMPOSE.replace("./data:/data", "./other:/data"),
            "declared for service web",
        );
        assert_err(
            MANIFEST,
            &COMPOSE.replace("./data:/data", "data:/data"),
            "must bind a directory under ./",
        );
        assert_err(
            MANIFEST,
            &format!("{COMPOSE}volumes:\n  data: {{}}\n"),
            "named volumes",
        );
    }

    #[test]
    fn databases_with_volumes_need_a_backup_dump_hook() {
        let manifest = format!(
            "{MANIFEST}\n[[volumes]]\nservice = \"db\"\npath = \"./pg\"\ndescription = \"Database\"\n"
        );
        let db = "  db:\n    image: postgres:17-alpine\n    restart: unless-stopped\n    mem_limit: 256m\n    volumes:\n      - ./pg:/var/lib/postgresql/data\n";
        let without = format!("{COMPOSE}{db}");
        assert_err(&manifest, &without, "add a 'tond.backup.dump' label");

        let hooked = format!(
            "{without}    labels:\n      tond.backup.dump: pg_dumpall -U \"$$POSTGRES_USER\" --clean --if-exists\n      tond.backup.restore: psql -U \"$$POSTGRES_USER\" -d postgres -q\n"
        );
        let t = Template::parse("demo", &manifest, &hooked).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.services.len(), 2);

        // A restore needs a dump; hook commands are single lines.
        let restore_only =
            format!("{without}    labels:\n      tond.backup.restore: psql -U postgres\n");
        assert_err(&manifest, &restore_only, "needs a tond.backup.dump");
        let multiline =
            format!("{without}    labels:\n      tond.backup.dump: \"pg_dumpall\\nrm -rf /\"\n");
        assert_err(&manifest, &multiline, "single-line");

        // A cache without a volume has nothing in backups: no hook needed.
        let cache = "  cache:\n    image: valkey/valkey:8-alpine\n    restart: unless-stopped\n    mem_limit: 64m\n";
        assert!(Template::parse("demo", MANIFEST, &format!("{COMPOSE}{cache}")).is_ok());
    }

    #[test]
    fn requires_restart_memory_and_a_single_public_port() {
        assert_err(
            MANIFEST,
            &COMPOSE.replace("    restart: unless-stopped\n", ""),
            "restart",
        );
        assert_err(
            MANIFEST,
            &COMPOSE.replace("    mem_limit: 256m\n", ""),
            "mem_limit",
        );
        assert_err(
            MANIFEST,
            &COMPOSE.replace("[\"8080\"]", "[\"9000:8080\"]"),
            "publish exactly",
        );
        assert_err(
            &MANIFEST.replace("ram_mb = 200", "ram_mb = 900"),
            COMPOSE,
            "mem_limits allow",
        );
        let extra = format!(
            "{COMPOSE}  db:\n    image: postgres:16-alpine\n    restart: unless-stopped\n    mem_limit: 128m\n    ports: [\"5432\"]\n"
        );
        assert_err(MANIFEST, &extra, "only the public service publishes");
    }

    #[test]
    fn parses_compose_durations() {
        assert_eq!(parse_compose_duration("10s"), Some(10.0));
        assert_eq!(parse_compose_duration("1m30s"), Some(90.0));
        assert_eq!(parse_compose_duration("1h"), Some(3600.0));
        assert_eq!(parse_compose_duration("500ms"), Some(0.5));
        assert_eq!(parse_compose_duration("1.5s"), Some(1.5));
        for bad in ["", "10", "s", "10x", "1m30"] {
            assert_eq!(parse_compose_duration(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn template_healthchecks_must_be_quick() {
        let with = |hc: &str| {
            COMPOSE.replace(
                "    volumes:\n",
                &format!("    healthcheck:\n{hc}    volumes:\n"),
            )
        };
        // Short timings pass; so does inheriting the image's test.
        demo();
        Template::parse(
            "demo",
            MANIFEST,
            &with("      interval: 10s\n      start_period: 30s\n      start_interval: 2s\n"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        Template::parse("demo", MANIFEST, &with("      disable: true\n"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_err(
            MANIFEST,
            &with("      test: [\"CMD\", \"true\"]\n"),
            "set healthcheck interval",
        );
        assert_err(MANIFEST, &with("      interval: 1m\n"), "at most 30s");
        assert_err(
            MANIFEST,
            &with("      interval: soon\n"),
            "duration like 10s",
        );
        assert_err(
            MANIFEST,
            &with("      interval: 10s\n      start_period: 30s\n"),
            "needs a start_interval",
        );
        assert_err(
            MANIFEST,
            &with("      interval: 10s\n      start_period: 30s\n      start_interval: 10s\n"),
            "start_interval must be at most 5s",
        );
    }

    /// Images that ship their own HEALTHCHECK with Docker's 30s default
    /// interval or longer (checked with `docker image inspect`): a template
    /// using one overrides the timings, or its deploys wait for the first
    /// check.
    const IMAGES_WITH_SLOW_HEALTHCHECKS: &[&str] = &[
        "appsmith/appsmith-ce",
        "excalidraw/excalidraw",
        "ghcr.io/immich-app/immich-server",
        "ghcr.io/immich-app/postgres",
        "ghcr.io/mealie-recipes/mealie",
        "ghcr.io/paperless-ngx/paperless-ngx",
        "gotify/server",
        "louislam/uptime-kuma",
        "mattermost/mattermost-team-edition",
        "sissbruecker/linkding",
        "stirlingtools/stirling-pdf",
        "vaultwarden/server",
    ];

    #[test]
    fn images_with_slow_healthchecks_get_quick_template_healthchecks() {
        for t in builtin_templates().unwrap_or_else(|e| panic!("{e}")) {
            let doc: Value = serde_yaml::from_str(&t.compose).unwrap_or_else(|e| panic!("{e}"));
            for svc in &t.services {
                let image = svc.image.split('@').next().unwrap_or_default();
                let repo = match image.rsplit_once(':') {
                    Some((repo, tag)) if !tag.contains('/') => repo,
                    _ => image,
                };
                if !IMAGES_WITH_SLOW_HEALTHCHECKS.contains(&repo) {
                    continue;
                }
                assert!(
                    doc["services"][svc.name.as_str()]
                        .get("healthcheck")
                        .is_some(),
                    "{}: service {} uses {} whose built-in healthcheck is slow; add a healthcheck with a short interval and start_interval",
                    t.manifest.id,
                    svc.name,
                    svc.image
                );
            }
        }
    }

    #[test]
    fn rejects_reserved_input_names_and_bad_placeholders() {
        let m = MANIFEST.replace("ADMIN_EMAIL", "DOCKER_HOST");
        let c = COMPOSE.replace("ADMIN_EMAIL", "DOCKER_HOST");
        assert_err(&m, &c, "reserved");
        assert_err(
            &MANIFEST.replace("{app_url}", "{app_port}"),
            COMPOSE,
            "unknown placeholder",
        );
        assert_err(
            &MANIFEST.replace("category = \"developer\"", "category = \"misc\""),
            COMPOSE,
            "category",
        );
    }

    #[test]
    fn resolve_generates_expands_and_checks_inputs() {
        let t = demo();
        let ctx = TemplateContext::new("demo", "demo.example.com");
        let mut provided = BTreeMap::new();
        assert_eq!(
            resolve_inputs(&t.manifest, &provided, &ctx)
                .err()
                .map(|e| e.key),
            Some("ADMIN_EMAIL".to_string())
        );
        provided.insert("ADMIN_EMAIL".into(), "not-an-email".into());
        assert!(resolve_inputs(&t.manifest, &provided, &ctx).is_err());
        provided.insert("ADMIN_EMAIL".into(), "me@example.com".into());
        let r = resolve_inputs(&t.manifest, &provided, &ctx).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.env["BASE_URL"], "https://demo.example.com");
        assert_eq!(r.env["ADMIN_PASSWORD"].len(), 24);
        assert_eq!(r.credentials.len(), 1);
        assert_eq!(r.credentials[0].2, r.env["ADMIN_PASSWORD"]);

        provided.insert("ADMIN_PASSWORD".into(), "short".into());
        assert!(
            resolve_inputs(&t.manifest, &provided, &ctx).is_err(),
            "short passwords are refused"
        );
        provided.insert("NOT_AN_INPUT".into(), "x".into());
        provided.remove("ADMIN_PASSWORD");
        assert!(
            resolve_inputs(&t.manifest, &provided, &ctx).is_err(),
            "unknown inputs are refused"
        );
    }

    #[test]
    fn generated_values_have_the_promised_shape() {
        let p = generate_value(TemplateGenerate::Password);
        assert!(p.len() == 24 && p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(p, generate_value(TemplateGenerate::Password));
        let h = generate_value(TemplateGenerate::Hex);
        assert!(h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(generate_value(TemplateGenerate::Base64).len(), 64);
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"hi!"), "aGkh");
    }

    #[test]
    fn scans_compose_variables() {
        let vars = compose_variables(
            "a: ${A}\nb: \"$B-x\"\nc: ${C:-${D}}\nd: $$E\ne: price $5\nlist: [\"${F:?need f}\"]\n# ${COMMENT}\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let got: Vec<&str> = vars.iter().map(String::as_str).collect();
        assert_eq!(got, ["A", "B", "C", "D", "F"]);
    }

    #[test]
    fn reserved_env_keys() {
        for k in [
            "DOCKER_HOST",
            "COMPOSE_FILE",
            "LD_PRELOAD",
            "PATH",
            "path",
            "GODEBUG",
            "TOND_SECRET_KEY",
        ] {
            assert!(is_reserved_env_key(k), "{k}");
        }
        for k in ["DATABASE_URL", "ADMIN_PASSWORD", "APP_SECRET"] {
            assert!(!is_reserved_env_key(k), "{k}");
        }
    }

    #[test]
    fn parses_memory_sizes() {
        assert_eq!(parse_memory_mb("256m"), Some(256));
        assert_eq!(parse_memory_mb("1g"), Some(1024));
        assert_eq!(parse_memory_mb("1.5G"), Some(1536));
        assert_eq!(parse_memory_mb("536870912"), Some(512));
        assert_eq!(parse_memory_mb("512mb"), Some(512));
        assert_eq!(parse_memory_mb("lots"), None);
    }

    #[test]
    fn context_uses_http_for_localhost() {
        assert_eq!(
            TemplateContext::new("a", "a.localhost").app_url,
            "http://a.localhost"
        );
        assert_eq!(
            TemplateContext::new("a", "a.example.com").expand("{app_url}/x {app_name}"),
            "https://a.example.com/x a"
        );
    }

    /// The dashboard's mock API serves the real catalog. Regenerate with
    /// `UPDATE_TEMPLATE_FIXTURE=1 cargo test -p tond-common templates`.
    #[test]
    fn dashboard_mock_catalog_matches_the_templates() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../dashboard/src/mocks/templates.json");
        let catalog: Vec<TemplateResponse> = builtin_templates()
            .unwrap_or_else(|e| panic!("{e}"))
            .into_iter()
            .map(TemplateResponse::from)
            .collect();
        let json = serde_json::to_string_pretty(&catalog).unwrap_or_else(|e| panic!("{e}")) + "\n";
        if std::env::var_os("UPDATE_TEMPLATE_FIXTURE").is_some() {
            std::fs::write(&path, &json).unwrap_or_else(|e| panic!("{e}"));
        }
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            current == json,
            "dashboard/src/mocks/templates.json is out of date; run UPDATE_TEMPLATE_FIXTURE=1 cargo test -p tond-common templates"
        );
    }
}
