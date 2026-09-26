// apps.rs — App entity, all app request/response types, Volume, DeployLog

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Volume (bind mount specification) ──

/// A bind-mount volume for a deployed app.
///
/// Users never pick the host side: the server maps `name` to
/// `<TOND_DATA_DIR>/volumes/<app-name>/<name>` and reports that as `host_path`
/// (informational — deploys re-derive the path from `name`).
/// `container_path` is the mount point inside the container (e.g. `/data`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Volume {
    /// Directory name under the app's volume directory (e.g. `data`)
    #[serde(default)]
    pub name: String,
    /// Absolute host path, computed by the server (read-only)
    pub host_path: String,
    pub container_path: String,
}

// ── App (a deployed service) ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct App {
    pub id: String,
    pub name: String,
    pub domain: Option<String>,
    /// "git" | "image" | "tarball" | "dockerfile" | "compose"
    pub source_type: String,
    /// Git URL, Docker image ref, or empty for tarball/dockerfile/compose
    pub source_url: Option<String>,
    pub branch: Option<String>,
    /// Port the app listens on inside the container
    pub container_port: i64,
    /// Docker container ID (set after deploy)
    pub container_id: Option<String>,
    /// Host port that the container is bound to (set after deploy)
    pub host_port: Option<i64>,
    /// Memory limit in bytes (None = the host default cap, 75% of RAM)
    pub memory_limit: Option<i64>,
    /// CPU limit as nano_cpus (None = no limit; 1 CPU = 1_000_000_000)
    pub cpu_limit: Option<i64>,
    /// "building" | "running" | "stopped" | "failed" | "unhealthy"
    pub status: String,
    /// Environment variables: a JSON object, stored encrypted by the server
    /// (`{}` when empty). Never serialized.
    #[serde(skip_serializing)]
    pub env_vars: Option<String>,
    /// HTTP path polled for health checks (None = disabled)
    pub health_check_path: Option<String>,
    /// Seconds between health check polls (default 30)
    pub health_check_interval: i64,
    /// Number of consecutive failures before marking unhealthy and restarting (default 3)
    pub health_failure_threshold: i64,
    /// Running count of consecutive failures since last successful check
    pub health_consecutive_failures: i64,
    /// Bind-mount volumes as a JSON array of `Volume` objects (default `[]`)
    pub volumes: String,
    /// Seconds to wait for the new container to pass health checks before
    /// cutting traffic and stopping the old container (default 30 s).
    /// Only effective when a `health_check_path` is configured; otherwise
    /// the swap happens immediately after the new container starts.
    pub drain_timeout: i64,
    /// Build method: "tbuild" | "dockerfile"
    pub build_method: String,
    // ── Phase 4: Git Integration & Automation ──
    /// Per-app webhook secret: the HMAC-SHA256 key for GitHub/Gitea/generic
    /// signatures and the GitLab `X-Gitlab-Token`. Generated at creation;
    /// webhooks are rejected when it is missing. Stored encrypted by the
    /// server; never serialized.
    #[serde(skip_serializing)]
    pub webhook_secret: Option<String>,
    /// 1 = automatically trigger a deploy when a webhook fires for the
    /// configured branch; 0 = disabled (webhook exists but does not auto-deploy).
    pub auto_deploy: i64,
    /// Branch to watch for webhook-triggered deploys (e.g. "main").
    /// None = any branch triggers (only relevant when auto_deploy = 1).
    pub watch_branch: Option<String>,
    /// Public half of the per-app SSH deploy key (Ed25519, OpenSSH format).
    /// None = deploy key not yet generated.
    pub deploy_key_pub: Option<String>,
    // ── Phase 6: Multi-App & Networking ──
    /// Project this app belongs to (None = no project)
    pub project_id: Option<String>,
    /// Registry credential linked to this app for pulling private images
    pub registry_credential_id: Option<String>,
    // ── Phase 4.4: GitHub App Integration ──
    /// GitHub App registration ID used to clone private repos via installation token
    pub github_app_id: Option<String>,
    // ── Phase 4.5: GitLab PAT Integration ──
    /// GitLab token ID used to clone private GitLab repos
    pub gitlab_token_id: Option<String>,
    // ── Phase 4.6: PR Preview Deployments ──
    /// 1 = automatically create a preview deployment when a GitHub pull_request
    /// webhook event is received for this app; 0 = opt-out (default).
    pub preview_enabled: i64,
    /// JSON array of env var keys excluded from preview containers (e.g. `["SECRET_KEY"]`)
    pub preview_excluded_keys: String,
    /// JSON object of key-value overrides applied only to preview containers,
    /// stored encrypted by the server. Never serialized.
    #[serde(skip_serializing)]
    pub preview_env_vars: String,
    // ── Inline source content ──
    /// Raw Dockerfile content supplied by the user (used when source_type = "dockerfile")
    pub dockerfile_content: Option<String>,
    /// Raw docker-compose.yml content supplied by the user (used when source_type = "compose")
    pub compose_content: Option<String>,
    /// Subdirectory to use as the build root (useful for monorepos); None = repo root
    pub base_dir: Option<String>,
    /// 1 = serve final build output as a static site; 0 = run as a process
    pub static_build: i64,
    /// How a deploy decides the new container is ready: "http" | "tcp" | "none"
    /// (see [`Readiness`])
    #[sqlx(default)]
    pub readiness: String,
    /// For readiness "none": seconds the container must stay up
    #[sqlx(default)]
    pub ready_after_secs: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Default for [`App::ready_after_secs`].
pub const DEFAULT_READY_AFTER_SECS: i64 = 10;
/// Bounds for [`App::ready_after_secs`].
pub const READY_AFTER_SECS_RANGE: std::ops::RangeInclusive<i64> = 1..=600;

/// How a deploy decides a new container is ready to take over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Readiness {
    /// The health check path answers 2xx, or (without one) any HTTP answer on
    /// the container port. Web apps; routed by domain.
    #[default]
    Http,
    /// The container port accepts TCP connections. Non-HTTP services.
    Tcp,
    /// Background worker: no port, no domain, never routed. Ready once the
    /// container has stayed up for `ready_after_secs` without exiting.
    None,
}

impl Readiness {
    /// Parse a stored value; anything unknown is `Http` (the old behaviour).
    pub fn parse(value: &str) -> Self {
        Self::from_input(value).unwrap_or(Readiness::Http)
    }

    /// Parse user input strictly.
    pub fn from_input(value: &str) -> Option<Self> {
        match value {
            "http" => Some(Readiness::Http),
            "tcp" => Some(Readiness::Tcp),
            "none" => Some(Readiness::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Readiness::Http => "http",
            Readiness::Tcp => "tcp",
            Readiness::None => "none",
        }
    }

    /// Whether the app serves traffic (gets a domain and a route).
    pub fn serves_traffic(self) -> bool {
        self != Readiness::None
    }
}

impl App {
    /// This app's readiness mode.
    pub fn readiness(&self) -> Readiness {
        Readiness::parse(&self.readiness)
    }

    pub fn new(
        name: String,
        source_type: String,
        source_url: Option<String>,
        branch: Option<String>,
        container_port: i64,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            domain: None,
            source_type,
            source_url,
            branch,
            container_port,
            container_id: None,
            host_port: None,
            // No explicit caps: containers get the host default (75% of RAM,
            // docker::default_memory_limit) and may use every CPU.
            memory_limit: None,
            cpu_limit: None,
            status: "pending".to_string(),
            env_vars: Some("{}".to_string()),
            health_check_path: None,
            health_check_interval: 30,
            health_failure_threshold: 3,
            health_consecutive_failures: 0,
            volumes: "[]".to_string(),
            drain_timeout: 30,
            build_method: "tbuild".to_string(),
            webhook_secret: Some(crate::generate_raw_key()),
            auto_deploy: 0,
            watch_branch: None,
            deploy_key_pub: None,
            project_id: None,
            registry_credential_id: None,
            github_app_id: None,
            gitlab_token_id: None,
            preview_enabled: 0,
            preview_excluded_keys: "[]".to_string(),
            preview_env_vars: "{}".to_string(),
            dockerfile_content: None,
            compose_content: None,
            base_dir: None,
            static_build: 0,
            readiness: Readiness::Http.as_str().to_string(),
            ready_after_secs: DEFAULT_READY_AFTER_SECS,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

// ── API request/response types ──

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateAppRequest {
    pub name: String,
    /// "git" | "image" | "dockerfile" | "compose"
    pub source_type: String,
    pub source_url: Option<String>,
    pub branch: Option<String>,
    #[serde(default = "default_port")]
    pub container_port: i64,
    pub env_vars: Option<serde_json::Value>,
    pub domain: Option<String>,
    /// Project to associate this app with
    pub project_id: Option<String>,
    /// Memory limit in bytes (None = the host default cap, 75% of RAM)
    pub memory_limit: Option<i64>,
    /// CPU limit as nano_cpus (None = no limit)
    pub cpu_limit: Option<i64>,
    /// HTTP path to poll for health checks (e.g. "/health"); None = disabled
    pub health_check_path: Option<String>,
    /// Seconds between health check polls (None = use default 30)
    pub health_check_interval: Option<i64>,
    /// Number of consecutive failures before marking unhealthy (None = use default 3)
    pub health_failure_threshold: Option<i64>,
    /// Seconds to wait for a new container to become healthy before traffic
    /// cutover during a graceful redeploy (None = use default 30 s)
    pub drain_timeout: Option<i64>,
    /// Build method: "tbuild" | "dockerfile" (None = "tbuild")
    pub build_method: Option<String>,
    /// Enable auto-deploy on webhook push (default false)
    #[serde(default)]
    pub auto_deploy: bool,
    /// Branch to watch for webhook-triggered deploys (None = any branch)
    pub watch_branch: Option<String>,
    /// Volume mounts: list of {container_path, name?}. Host paths are chosen by the server.
    pub volumes: Option<Vec<VolumeRequest>>,
    /// Registry credential to use for pulling private images
    pub registry_credential_id: Option<String>,
    /// GitHub App ID to use for private repo cloning
    pub github_app_id: Option<String>,
    /// Raw Dockerfile content (used when source_type = "dockerfile")
    pub dockerfile_content: Option<String>,
    /// Raw docker-compose.yml content (used when source_type = "compose")
    pub compose_content: Option<String>,
    /// Subdirectory to use as the build root (None = repo root)
    pub base_dir: Option<String>,
    /// Serve build output as a static site (default false)
    #[serde(default)]
    pub static_build: bool,
    /// "http" (default) | "tcp" | "none" (background worker: no port, no domain)
    #[serde(default)]
    pub readiness: Option<String>,
    /// For readiness "none": seconds the container must stay up (default 10)
    #[serde(default)]
    pub ready_after_secs: Option<i64>,
}

fn default_port() -> i64 {
    3000
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppResponse {
    pub id: String,
    pub name: String,
    pub domain: Option<String>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub status: String,
    pub container_port: i64,
    pub host_port: Option<i64>,
    pub memory_limit: Option<i64>,
    pub cpu_limit: Option<i64>,
    pub health_check_path: Option<String>,
    pub health_check_interval: i64,
    pub health_failure_threshold: i64,
    pub health_consecutive_failures: i64,
    /// Deserialized volumes list
    pub volumes: Vec<Volume>,
    /// Drain timeout in seconds for graceful deploys
    pub drain_timeout: i64,
    /// Build method for source-building apps: "tbuild" | "dockerfile"
    pub build_method: Option<String>,
    // ── Phase 4: Git Integration & Automation ──
    /// Whether auto-deploy on webhook push is enabled
    pub auto_deploy: bool,
    /// Branch filter for webhook deploys (None = any branch)
    pub watch_branch: Option<String>,
    /// Public SSH deploy key (Ed25519, OpenSSH format); None = not generated
    pub deploy_key_pub: Option<String>,
    // ── Phase 6 ──
    /// Project this app belongs to (None = no project)
    pub project_id: Option<String>,
    /// Registry credential linked to this app
    pub registry_credential_id: Option<String>,
    // ── Phase 4.4: GitHub App Integration ──
    /// GitHub App registration ID used to clone this app's repo
    pub github_app_id: Option<String>,
    // ── Phase 4.5: GitLab PAT Integration ──
    /// GitLab token ID used to clone this app's repo
    pub gitlab_token_id: Option<String>,
    // ── Phase 4.6: PR Preview Deployments ──
    /// Whether PR preview deployments are enabled for this app
    pub preview_enabled: bool,
    /// Env var keys excluded from preview containers
    pub preview_excluded_keys: Vec<String>,
    // Preview override values are secrets: read them (masked) through
    // GET /api/apps/{id}/env, never through the app response.
    // ── Inline source content ──
    /// Raw Dockerfile content (used when source_type = "dockerfile")
    pub dockerfile_content: Option<String>,
    /// Raw docker-compose.yml content (used when source_type = "compose")
    pub compose_content: Option<String>,
    /// Subdirectory used as build root (None = repo root)
    pub base_dir: Option<String>,
    /// Whether the build output is served as a static site
    pub static_build: bool,
    /// "http" | "tcp" | "none" (background worker)
    #[serde(default)]
    pub readiness: String,
    /// For readiness "none": seconds the container must stay up
    #[serde(default)]
    pub ready_after_secs: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<App> for AppResponse {
    fn from(app: App) -> Self {
        let volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
        let preview_excluded_keys: Vec<String> =
            serde_json::from_str(&app.preview_excluded_keys).unwrap_or_default();
        let expose_build_method =
            matches!(app.source_type.as_str(), "git" | "tarball" | "dockerfile");
        Self {
            id: app.id,
            name: app.name,
            domain: app.domain,
            source_type: app.source_type,
            source_url: app.source_url,
            status: app.status,
            container_port: app.container_port,
            host_port: app.host_port,
            memory_limit: app.memory_limit,
            cpu_limit: app.cpu_limit,
            health_check_path: app.health_check_path,
            health_check_interval: app.health_check_interval,
            health_failure_threshold: app.health_failure_threshold,
            health_consecutive_failures: app.health_consecutive_failures,
            volumes,
            drain_timeout: app.drain_timeout,
            build_method: expose_build_method.then_some(app.build_method),
            auto_deploy: app.auto_deploy != 0,
            watch_branch: app.watch_branch,
            deploy_key_pub: app.deploy_key_pub,
            project_id: app.project_id,
            registry_credential_id: app.registry_credential_id,
            github_app_id: app.github_app_id,
            gitlab_token_id: app.gitlab_token_id,
            preview_enabled: app.preview_enabled != 0,
            preview_excluded_keys,
            dockerfile_content: app.dockerfile_content,
            compose_content: app.compose_content,
            base_dir: app.base_dir,
            static_build: app.static_build != 0,
            readiness: Readiness::parse(&app.readiness).as_str().to_string(),
            ready_after_secs: app.ready_after_secs,
            created_at: app.created_at,
            updated_at: app.updated_at,
        }
    }
}

// ── Environment variables ──
//
// Env var values are secrets. They are never returned in bulk: the list
// endpoint returns keys with masked values, and a single value is only
// returned by an explicit reveal request. Writes are patches, so editing
// one variable never touches the others.

/// Masked value shown in place of every non-empty env var value.
pub const ENV_VALUE_MASK: &str = "••••••••";

/// One env var as returned by the list endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVarEntry {
    pub key: String,
    /// `ENV_VALUE_MASK` for a non-empty value, `""` for an empty one.
    pub masked: String,
}

/// Preview-only env configuration of an app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewEnvResponse {
    /// Whether PR preview deployments are enabled for this app
    pub enabled: bool,
    /// App env var keys that preview containers do not receive
    pub excluded_keys: Vec<String>,
    /// Values that replace or add env vars in preview containers only
    pub overrides: Vec<EnvVarEntry>,
}

/// GET /api/apps/{id}/env
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEnvResponse {
    pub vars: Vec<EnvVarEntry>,
    pub preview: PreviewEnvResponse,
}

/// GET /api/apps/{id}/env/{key} and /api/apps/{id}/preview-env/{key}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarValueResponse {
    pub key: String,
    pub value: String,
}

/// PATCH /api/apps/{id}/env — set and remove individual variables.
/// Keys that are not mentioned are left unchanged.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PatchEnvRequest {
    /// Variables to add or overwrite
    #[serde(default)]
    pub set: std::collections::BTreeMap<String, String>,
    /// Variables to remove (unknown keys are ignored)
    #[serde(default)]
    pub unset: Vec<String>,
}

/// PATCH /api/apps/{id}/preview-env — edit preview overrides and exclusions.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PatchPreviewEnvRequest {
    /// Preview overrides to add or overwrite
    #[serde(default)]
    pub set: std::collections::BTreeMap<String, String>,
    /// Preview overrides to remove
    #[serde(default)]
    pub unset: Vec<String>,
    /// App env var keys to hide from preview containers
    #[serde(default)]
    pub exclude: Vec<String>,
    /// App env var keys to pass to preview containers again
    #[serde(default)]
    pub include: Vec<String>,
}

/// Build the masked list of an env var map, sorted by key.
pub fn masked_env_entries(vars: &std::collections::BTreeMap<String, String>) -> Vec<EnvVarEntry> {
    vars.iter()
        .map(|(key, value)| EnvVarEntry {
            key: key.clone(),
            masked: if value.is_empty() {
                String::new()
            } else {
                ENV_VALUE_MASK.to_string()
            },
        })
        .collect()
}

/// Whether `key` is a valid environment variable name
/// (a letter or underscore, then letters, digits and underscores).
pub fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Debug, Deserialize)]
pub struct UpdateDomainRequest {
    pub domain: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateHealthRequest {
    /// HTTP path to poll (e.g. "/health"); set to null to disable
    pub health_check_path: Option<String>,
    /// Seconds between polls (default 30)
    pub health_check_interval: Option<i64>,
    /// Consecutive failures before restart (default 3)
    pub health_failure_threshold: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRequest {
    /// Mount point inside the container (e.g. "/data")
    pub container_path: String,
    /// Volume directory name under `volumes/<app>/` (default: derived from `container_path`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Only accepted when it already points at `<data>/volumes/<app>/<name>`;
    /// anything else is rejected. Prefer `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDrainTimeoutRequest {
    /// Seconds to wait for the new container to become healthy before
    /// cutting over traffic. Set to 0 to disable graceful draining.
    pub drain_timeout: i64,
}

/// PATCH /api/apps/{id}/resources — update memory and CPU limits
#[derive(Debug, Deserialize)]
pub struct UpdateAppResourcesRequest {
    /// Memory limit in bytes (None = no limit / unlimited)
    pub memory_limit: Option<i64>,
    /// CPU limit as nano_cpus (None = no limit / unlimited)
    pub cpu_limit: Option<i64>,
}

// ── Phase 4: Git Integration & Automation ──

/// PUT /api/apps/{id}/auto-deploy — toggle automatic deploys on webhook push
#[derive(Debug, Deserialize)]
pub struct UpdateAutoDeployRequest {
    /// true = enable auto-deploy when a webhook fires for the watch_branch
    pub auto_deploy: bool,
    /// Branch to watch (None = any branch triggers deploy)
    pub watch_branch: Option<String>,
}

/// PUT /api/apps/{id}/webhook-secret — replace the webhook secret with a
/// caller-chosen value. Verification cannot be disabled; see
/// `POST /api/apps/{id}/webhook-secret/rotate` for a server-generated secret.
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateWebhookSecretRequest {
    /// New secret (at least `MIN_WEBHOOK_SECRET_LEN` characters).
    pub secret: String,
}

/// Minimum length of a caller-chosen webhook secret.
pub const MIN_WEBHOOK_SECRET_LEN: usize = 16;

/// Per-provider webhook URL paths for an app (relative to the dashboard origin).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookUrls {
    pub github: String,
    pub gitlab: String,
    pub gitea: String,
    pub generic: String,
}

impl WebhookUrls {
    /// Webhook paths are addressed by app UUID only (never by name).
    pub fn for_app(app_id: &str) -> Self {
        Self {
            github: format!("/api/webhooks/github/{app_id}"),
            gitlab: format!("/api/webhooks/gitlab/{app_id}"),
            gitea: format!("/api/webhooks/gitea/{app_id}"),
            generic: format!("/api/webhooks/generic/{app_id}"),
        }
    }
}

/// GET /api/apps/{id}/webhook and POST /api/apps/{id}/webhook-secret/rotate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfigResponse {
    pub app_id: String,
    /// Paths relative to the server origin, e.g. `/api/webhooks/github/<uuid>`.
    pub paths: WebhookUrls,
    /// Absolute URLs when `dashboard_domain` is configured, else None.
    pub urls: Option<WebhookUrls>,
    /// Whether a webhook secret is set. The secret itself is write-only: read
    /// it with GET /api/apps/{id}/webhook-secret (admin only, audit-logged).
    pub has_secret: bool,
    pub auto_deploy: bool,
    pub watch_branch: Option<String>,
}

/// GET /api/apps/{id}/webhook-secret (reveal) and
/// POST /api/apps/{id}/webhook-secret/rotate (the new secret, shown once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSecretResponse {
    pub secret: String,
}

/// Response returned after generating or fetching a deploy key.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeployKeyResponse {
    pub app_id: String,
    pub public_key: String,
}

/// PUT /api/apps/{id}/source — update the deployment source
#[derive(Debug, Deserialize)]
pub struct UpdateSourceRequest {
    /// "git" | "image" | "tarball"
    pub source_type: String,
    /// Git URL, Docker image ref, or None for tarball
    pub source_url: Option<String>,
}

// ── Phase 4.6: PR Preview configuration ──

/// PUT /api/apps/{id}/preview-config — enable or disable PR preview deployments.
/// Preview env overrides and exclusions are edited with PATCH /api/apps/{id}/preview-env.
#[derive(Debug, Deserialize)]
pub struct UpdatePreviewConfigRequest {
    /// true = automatically deploy on pull_request events from GitHub
    pub preview_enabled: bool,
}

// ── Deploy log ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DeployLog {
    pub id: String,
    pub app_id: String,
    pub message: String,
    pub level: String, // "info" | "error" | "warn"
    /// How this deploy was triggered: "manual" | "webhook" | "upload" | "rollback"
    pub trigger_source: String,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_new_sets_documented_defaults() {
        let app = App::new(
            "demo".to_string(),
            "git".to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );

        assert_eq!(app.name, "demo", "new apps should preserve their name");
        assert_eq!(
            app.status, "pending",
            "new apps should start in pending status before first deploy"
        );
        assert_eq!(
            app.memory_limit, None,
            "new apps have no explicit memory cap (the host default applies)"
        );
        assert_eq!(app.cpu_limit, None, "new apps may use every CPU");
        assert_eq!(app.readiness(), Readiness::Http, "new apps are web apps");
        assert_eq!(app.ready_after_secs, DEFAULT_READY_AFTER_SECS);
        assert_eq!(Readiness::parse("bogus"), Readiness::Http);
        assert_eq!(Readiness::from_input("bogus"), None);
        assert!(!Readiness::None.serves_traffic());
        assert_eq!(
            app.env_vars.as_deref(),
            Some("{}"),
            "new apps should start with an empty env var map"
        );
        assert_eq!(
            app.volumes, "[]",
            "new apps should start with no configured bind mounts"
        );
        assert_eq!(
            app.drain_timeout, 30,
            "new apps should default to a 30 second graceful drain timeout"
        );
        assert_eq!(
            app.build_method, "tbuild",
            "new apps should default to the TBuild build method"
        );
        assert_eq!(
            app.auto_deploy, 0,
            "new apps should start with auto-deploy disabled"
        );
        assert_eq!(
            app.preview_enabled, 0,
            "new apps should start with preview deployments disabled"
        );
        assert_eq!(
            app.static_build, 0,
            "new apps should default to process mode instead of static hosting"
        );
    }

    #[test]
    fn create_app_request_deserializes_default_values() {
        let request: CreateAppRequest = serde_json::from_value(serde_json::json!({
            "name": "demo",
            "source_type": "git",
            "source_url": "https://example.com/repo.git"
        }))
        .unwrap_or_else(|err| panic!("failed to deserialize CreateAppRequest: {err}"));

        assert_eq!(
            request.container_port, 3000,
            "CreateAppRequest should default container_port to 3000"
        );
        assert!(
            !request.auto_deploy,
            "CreateAppRequest should default auto_deploy to false"
        );
        assert!(
            !request.static_build,
            "CreateAppRequest should default static_build to false"
        );
    }

    #[test]
    fn app_response_from_app_parses_valid_json_fields() {
        let mut app = App::new(
            "demo".to_string(),
            "git".to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        app.auto_deploy = 1;
        app.preview_enabled = 1;
        app.static_build = 1;
        app.volumes = serde_json::to_string(&vec![Volume {
            name: "data".to_string(),
            host_path: "/opt/tond/data/volumes/demo/data".to_string(),
            container_path: "/data".to_string(),
        }])
        .unwrap_or_else(|err| panic!("failed to serialize volumes fixture: {err}"));
        app.preview_excluded_keys = serde_json::to_string(&vec!["SECRET_KEY", "TOKEN"])
            .unwrap_or_else(|err| {
                panic!("failed to serialize preview_excluded_keys fixture: {err}")
            });

        let response = AppResponse::from(app);

        assert!(
            response.auto_deploy,
            "AppResponse should convert non-zero auto_deploy values to true"
        );
        assert!(
            response.preview_enabled,
            "AppResponse should convert non-zero preview_enabled values to true"
        );
        assert!(
            response.static_build,
            "AppResponse should convert non-zero static_build values to true"
        );
        assert_eq!(
            response.volumes.len(),
            1,
            "AppResponse should deserialize persisted volume JSON into typed volumes"
        );
        assert_eq!(
            response.preview_excluded_keys,
            vec!["SECRET_KEY".to_string(), "TOKEN".to_string()],
            "AppResponse should deserialize preview excluded keys from JSON"
        );
        assert_eq!(
            response.build_method.as_deref(),
            Some("tbuild"),
            "AppResponse should expose build_method for source-building apps"
        );
    }

    #[test]
    fn app_response_from_app_falls_back_for_invalid_json_fields() {
        let mut app = App::new("demo".to_string(), "git".to_string(), None, None, 3000);
        app.volumes = "not json".to_string();
        app.preview_excluded_keys = "{ definitely not an array }".to_string();

        let response = AppResponse::from(app);

        assert!(
            response.volumes.is_empty(),
            "invalid persisted volumes JSON should fall back to an empty list"
        );
        assert_eq!(
            response.preview_excluded_keys,
            Vec::<String>::new(),
            "invalid persisted preview_excluded_keys JSON should fall back to an empty list"
        );
    }

    #[test]
    fn app_response_omits_build_method_for_prebuilt_sources() {
        let mut app = App::new("demo".to_string(), "image".to_string(), None, None, 3000);
        app.build_method.clear();

        let response = AppResponse::from(app);

        assert!(
            response.build_method.is_none(),
            "AppResponse should omit build_method for sources that do not build source code"
        );
    }

    #[test]
    fn masked_env_entries_hides_values_and_marks_empty_ones() {
        let vars = std::collections::BTreeMap::from([
            ("B_KEY".to_string(), "secret".to_string()),
            ("A_KEY".to_string(), String::new()),
        ]);

        let entries = masked_env_entries(&vars);

        assert_eq!(
            entries,
            vec![
                EnvVarEntry {
                    key: "A_KEY".to_string(),
                    masked: String::new(),
                },
                EnvVarEntry {
                    key: "B_KEY".to_string(),
                    masked: ENV_VALUE_MASK.to_string(),
                },
            ],
            "entries should be sorted by key, mask non-empty values and keep empty ones empty"
        );
    }

    #[test]
    fn is_valid_env_key_accepts_posix_names_only() {
        for key in ["PORT", "_PRIVATE", "a1_B2"] {
            assert!(is_valid_env_key(key), "{key} should be a valid env key");
        }
        for key in ["", "1ABC", "WITH-DASH", "WITH SPACE", "A=B", "ÜMLAUT"] {
            assert!(!is_valid_env_key(key), "{key:?} should be rejected");
        }
    }
}
