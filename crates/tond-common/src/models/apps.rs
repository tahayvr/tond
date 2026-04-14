// apps.rs — App entity, all app request/response types, Volume, DeployLog

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Volume (bind mount specification) ──

/// A bind-mount volume for a deployed app.
///
/// `host_path` is the absolute path on the VPS host
/// (e.g. `/opt/tond/data/volumes/<app-name>/data`).
/// `container_path` is the mount point inside the container (e.g. `/data`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
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
    /// Memory limit in bytes (None = no limit; default 256 MiB)
    pub memory_limit: Option<i64>,
    /// CPU limit as nano_cpus (None = no limit; default 0.5 CPU = 500_000_000)
    pub cpu_limit: Option<i64>,
    /// "building" | "running" | "stopped" | "failed" | "unhealthy"
    pub status: String,
    /// Environment variables as JSON object
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
    /// HMAC-SHA256 secret used to validate GitHub/GitLab webhook signatures.
    /// None = no signature validation (generic webhooks or not configured).
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
    /// JSON object of key-value overrides applied only to preview containers
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
    pub created_at: String,
    pub updated_at: String,
}

impl App {
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
            memory_limit: Some(268_435_456), // 256 MiB
            cpu_limit: Some(500_000_000),    // 0.5 CPU
            status: "pending".to_string(),
            env_vars: Some("{}".to_string()),
            health_check_path: None,
            health_check_interval: 30,
            health_failure_threshold: 3,
            health_consecutive_failures: 0,
            volumes: "[]".to_string(),
            drain_timeout: 30,
            build_method: "tbuild".to_string(),
            webhook_secret: None,
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
    /// Memory limit in bytes (None = use server default 256 MiB)
    pub memory_limit: Option<i64>,
    /// CPU limit as nano_cpus (None = use server default 0.5 CPU)
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
    /// Volume mounts: list of {host_path, container_path}
    pub volumes: Option<Vec<Volume>>,
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
    /// Key-value overrides applied only to preview containers
    pub preview_env_vars: serde_json::Value,
    // ── Inline source content ──
    /// Raw Dockerfile content (used when source_type = "dockerfile")
    pub dockerfile_content: Option<String>,
    /// Raw docker-compose.yml content (used when source_type = "compose")
    pub compose_content: Option<String>,
    /// Subdirectory used as build root (None = repo root)
    pub base_dir: Option<String>,
    /// Whether the build output is served as a static site
    pub static_build: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<App> for AppResponse {
    fn from(app: App) -> Self {
        let volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
        let preview_excluded_keys: Vec<String> =
            serde_json::from_str(&app.preview_excluded_keys).unwrap_or_default();
        let preview_env_vars: serde_json::Value =
            serde_json::from_str(&app.preview_env_vars).unwrap_or(serde_json::json!({}));
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
            preview_env_vars,
            dockerfile_content: app.dockerfile_content,
            compose_content: app.compose_content,
            base_dir: app.base_dir,
            static_build: app.static_build != 0,
            created_at: app.created_at,
            updated_at: app.updated_at,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateEnvRequest {
    pub env_vars: serde_json::Value,
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

#[derive(Debug, Deserialize)]
pub struct VolumeRequest {
    /// Mount point inside the container (e.g. "/data")
    pub container_path: String,
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

/// PUT /api/apps/{id}/webhook-secret — set or rotate the HMAC signing secret
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateWebhookSecretRequest {
    /// Plain-text secret that GitHub/GitLab will use to sign webhook payloads.
    /// Set to null to remove signature validation (accept all webhook calls).
    pub secret: Option<String>,
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

/// PUT /api/apps/{id}/preview-config — enable or disable PR preview deployments
#[derive(Debug, Deserialize)]
pub struct UpdatePreviewConfigRequest {
    /// true = automatically deploy on pull_request events from GitHub
    pub preview_enabled: bool,
    /// Env var keys to exclude from preview containers (replaces existing list when Some)
    pub preview_excluded_keys: Option<Vec<String>>,
    /// Key-value overrides applied only to preview containers (replaces existing when Some)
    pub preview_env_vars: Option<serde_json::Value>,
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
            app.memory_limit,
            Some(268_435_456),
            "new apps should default to a 256 MiB memory limit"
        );
        assert_eq!(
            app.cpu_limit,
            Some(500_000_000),
            "new apps should default to 0.5 CPU"
        );
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
            host_path: "/opt/tond/data/demo".to_string(),
            container_path: "/data".to_string(),
        }])
        .unwrap_or_else(|err| panic!("failed to serialize volumes fixture: {err}"));
        app.preview_excluded_keys = serde_json::to_string(&vec!["SECRET_KEY", "TOKEN"])
            .unwrap_or_else(|err| {
                panic!("failed to serialize preview_excluded_keys fixture: {err}")
            });
        app.preview_env_vars = serde_json::json!({ "MODE": "preview" }).to_string();

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
            response.preview_env_vars,
            serde_json::json!({ "MODE": "preview" }),
            "AppResponse should deserialize preview env vars from JSON"
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
        app.preview_env_vars = "this is not json either".to_string();

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
        assert_eq!(
            response.preview_env_vars,
            serde_json::json!({}),
            "invalid persisted preview_env_vars JSON should fall back to an empty object"
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
}
