// client.rs — typed HTTP client for the Tond API
//
// TondClient wraps reqwest::Client and provides one async method per API
// endpoint. All methods:
//   - Attach the Authorization: Bearer <token> header if an API token is set
//   - Deserialise the response into ApiResponse<T>
//   - Return the inner data on success, or bail with the error message
//
// This is the only place in the CLI that touches HTTP — main.rs only calls
// these typed methods and prints the results.
//
// Special case — push_deploy():
//   The CLI shells out to `tar czf` to package the working directory, then
//   sends the bytes as multipart/form-data to POST /api/apps/{id}/upload.
//   The server extracts the tarball, builds it, and deploys. The temp tarball
//   is cleaned up locally after the upload.
//
// Transports:
//   TondClient::local(socket) talks to tond-server over its root-only Unix
//   socket, where no credentials are needed (the CLI's default).
//   TondClient::new(url, token) talks HTTP(S) and sends the API token
//   (`tond_pat_…`, created with `tond token create` or in the dashboard).
//
// stream_logs():
//   Opens a WebSocket to GET /api/apps/{id}/logs/stream?token=<key> and
//   prints each JSON message as a human-readable line until the server
//   closes or the user presses Ctrl-C.
//
// All other methods are straightforward REST wrappers:
//   list_apps, create_app, get_app, deploy_app, action (stop/start/restart),
//   delete_app, get_logs, update_env, update_domain.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{Map, Value};
use tokio_tungstenite::tungstenite::Message;
use tond_common::{
    ApiResponse, AppResponse, AuditLogPage, AuditVerifyReport, BackupConfigResponse,
    CreateAppRequest, CreateCronJobRequest, CreateDatabaseRequest, CreateTokenRequest,
    CreatedDatabaseResponse, CreatedTokenResponse, CronJobResponse, CronJobRun,
    DatabaseCredentialsResponse, DeployKeyResponse, DeploymentResponse, IntegrationResponse,
    ManagedDatabaseDetail, ManagedDatabaseResponse, PasswordLinkResponse,
    PreviewDeploymentResponse, ProjectPreviewResponse, ProjectResponse, ServerSettings,
    SnapshotResponse, StorageProviderResponse, TemplateDeployRequest, TemplateDeployResponse,
    TemplateResponse, TokenResponse, UpdateCronJobRequest, UpdateSettingsRequest, UserResponse,
    WebhookConfigResponse, WebhookSecretResponse,
};

/// Body and response of `/api/settings/host-terminal`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HostTerminalSetting {
    enabled: bool,
}

pub struct TondClient {
    base_url: String,
    client: reqwest::Client,
    api_key: Option<String>,
    /// Set when talking to tond-server over its local Unix socket.
    socket_path: Option<PathBuf>,
}

/// Incoming messages of a WebSocket opened with `TondClient::ws_messages`.
pub type WsMessages = std::pin::Pin<
    Box<dyn futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Send>,
>;

/// Host used in request URLs sent over the Unix socket (the socket, not DNS, picks the peer).
const LOCAL_BASE_URL: &str = "http://localhost";

impl TondClient {
    /// Client for a server reached over HTTP(S), authenticated with the API token `api_key`.
    pub fn new(base_url: &str, api_key: Option<&str>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            api_key: api_key.map(|s| s.to_string()),
            socket_path: None,
        }
    }

    /// Client for the tond-server on this machine, via its root-only Unix socket.
    pub fn local(socket_path: &Path) -> Result<Self> {
        // Probe the socket first so a missing server or missing root privileges
        // produce an actionable message instead of a generic request error.
        if let Err(err) = std::os::unix::net::UnixStream::connect(socket_path) {
            let hint = "Run the CLI on the Tond server as root, or connect remotely with \
                        --server <url> --token <tond_pat_…>.";
            return Err(match err.kind() {
                std::io::ErrorKind::NotFound => anyhow::anyhow!(
                    "tond-server socket not found at {}. Is tond-server running? {hint}",
                    socket_path.display()
                ),
                std::io::ErrorKind::PermissionDenied => anyhow::anyhow!(
                    "permission denied on {} (the socket is root-only). {hint}",
                    socket_path.display()
                ),
                _ => anyhow::Error::new(err)
                    .context(format!("connecting to {}", socket_path.display())),
            });
        }

        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .build()
            .context("building Unix socket HTTP client")?;
        Ok(Self {
            base_url: LOCAL_BASE_URL.to_string(),
            client,
            api_key: None,
            socket_path: Some(socket_path.to_path_buf()),
        })
    }

    /// Add the Authorization header if an API token is configured.
    fn auth_header(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref key) = self.api_key {
            req.header("Authorization", format!("Bearer {}", key))
        } else {
            req
        }
    }

    /// Build the WebSocket URL (ws:// or wss://) from the HTTP base URL.
    fn ws_url(&self, path: &str) -> String {
        let base = self
            .base_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{}{}", base, path)
    }

    pub async fn list_apps(&self) -> Result<Vec<AppResponse>> {
        let req = self.client.get(format!("{}/api/apps", self.base_url));
        let resp: ApiResponse<Vec<AppResponse>> =
            self.auth_header(req).send().await?.json().await?;

        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_app(
        &self,
        name: &str,
        source_type: &str,
        source_url: Option<String>,
        branch: Option<String>,
        container_port: i64,
        env_vars: Option<Map<String, Value>>,
        project_id: Option<String>,
        domain: Option<String>,
        memory_limit: Option<i64>,
        cpu_limit: Option<i64>,
        health_check_path: Option<String>,
        health_check_interval: Option<i64>,
        health_failure_threshold: Option<i64>,
        drain_timeout: Option<i64>,
        build_method: Option<String>,
    ) -> Result<AppResponse> {
        let req = CreateAppRequest {
            name: name.to_string(),
            source_type: source_type.to_string(),
            source_url,
            branch,
            container_port,
            env_vars: env_vars.map(Value::Object),
            domain,
            project_id,
            memory_limit,
            cpu_limit,
            health_check_path,
            health_check_interval,
            health_failure_threshold,
            drain_timeout,
            build_method,
            auto_deploy: false,
            watch_branch: None,
            volumes: None,
            registry_credential_id: None,
            github_app_id: None,
            dockerfile_content: None,
            compose_content: None,
            base_dir: None,
            static_build: false,
            readiness: None,
            ready_after_secs: None,
        };

        let req = self
            .client
            .post(format!("{}/api/apps", self.base_url))
            .json(&req);
        let resp: ApiResponse<AppResponse> = self.auth_header(req).send().await?.json().await?;

        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn get_app(&self, id: &str) -> Result<AppResponse> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}", self.base_url, id));
        let resp: ApiResponse<AppResponse> = self.auth_header(req).send().await?.json().await?;

        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// GET /api/templates — the one-click catalog.
    pub async fn list_templates(&self) -> Result<Vec<TemplateResponse>> {
        let req = self.client.get(format!("{}/api/templates", self.base_url));
        let resp: ApiResponse<Vec<TemplateResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// POST /api/templates/{id}/deploy — create the app and queue its first deploy.
    pub async fn deploy_template(
        &self,
        id: &str,
        body: &TemplateDeployRequest,
    ) -> Result<TemplateDeployResponse> {
        let req = self
            .client
            .post(format!("{}/api/templates/{}/deploy", self.base_url, id))
            .json(body);
        let resp: ApiResponse<TemplateDeployResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Queue a deploy; returns the new deployment's id.
    pub async fn deploy_app(&self, id: &str) -> Result<Option<String>> {
        let req = self
            .client
            .post(format!("{}/api/apps/{}/deploy", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(resp
            .data
            .as_ref()
            .and_then(|d| d.get("deployment_id"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// GET /api/apps/{id}/deployments/{deployment_id} (deployment + log).
    pub async fn get_deployment(&self, id: &str, deployment_id: &str) -> Result<Value> {
        let req = self.client.get(format!(
            "{}/api/apps/{}/deployments/{}",
            self.base_url, id, deployment_id
        ));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Open a WebSocket at `path` (over the Unix socket or TCP) and return its
    /// incoming messages. The token, if any, travels as `?token=`.
    pub async fn ws_messages(&self, path: &str) -> Result<WsMessages> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = match self.api_key.as_deref() {
            Some(token) => self.ws_url(&format!("{path}{sep}token={token}")),
            None => self.ws_url(path),
        };
        Ok(match &self.socket_path {
            Some(socket_path) => {
                let stream = tokio::net::UnixStream::connect(socket_path)
                    .await
                    .context("Failed to connect to tond-server")?;
                let (ws, _) = tokio_tungstenite::client_async(url.as_str(), stream)
                    .await
                    .context("Failed to open the stream")?;
                Box::pin(ws.split().1)
            }
            None => {
                let (ws, _) = tokio_tungstenite::connect_async(&url)
                    .await
                    .context("Failed to open the stream")?;
                Box::pin(ws.split().1)
            }
        })
    }

    pub async fn push_deploy(&self, id: &str, dir: &str) -> Result<()> {
        // Create a tarball of the directory, respecting .tondignore and
        // applying sensible default exclusions.
        let tar_path = format!("/tmp/tond-push-{}.tar.gz", uuid::Uuid::new_v4());

        // Build the exclude list: hard-coded defaults + .tondignore entries
        let default_excludes = [
            ".git",
            "node_modules",
            "target",
            ".svelte-kit",
            ".next",
            "__pycache__",
            ".venv",
            "dist",
            "build",
            ".DS_Store",
        ];

        let mut excludes: Vec<String> = default_excludes.iter().map(|s| s.to_string()).collect();

        // Parse .tondignore if it exists
        let tondignore_path = format!("{}/.tondignore", dir);
        if let Ok(contents) = tokio::fs::read_to_string(&tondignore_path).await {
            for line in contents.lines() {
                let trimmed = line.trim();
                // Skip empty lines and comments
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // Strip trailing slashes for tar --exclude compatibility
                    let pattern = trimmed.trim_end_matches('/').to_string();
                    if !excludes.contains(&pattern) {
                        excludes.push(pattern);
                    }
                }
            }
        }

        // Build the tar command with all exclude arguments
        let mut args: Vec<String> = vec![
            "czf".to_string(),
            tar_path.clone(),
            "-C".to_string(),
            dir.to_string(),
        ];
        for exc in &excludes {
            args.push(format!("--exclude=./{}", exc));
        }
        args.push(".".to_string());

        let output = tokio::process::Command::new("tar")
            .args(&args)
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!("Failed to create tarball");
        }

        let file_bytes = tokio::fs::read(&tar_path).await?;
        let _ = tokio::fs::remove_file(&tar_path).await;

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name("upload.tar.gz")
            .mime_str("application/gzip")?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let req = self
            .client
            .post(format!("{}/api/apps/{}/upload", self.base_url, id))
            .multipart(form);
        let resp = self
            .auth_header(req)
            .send()
            .await
            .context("Upload failed")?;
        upload_result(resp.status(), &resp.text().await.unwrap_or_default())
    }

    pub async fn action(&self, id: &str, action: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/apps/{}/{}", self.base_url, id, action));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn delete_app(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/apps/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn get_logs(&self, id: &str) -> Result<Value> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}/logs", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Stream live logs over WebSocket, printing lines until server closes or Ctrl-C.
    /// The initial snapshot message is rendered first, then live events follow.
    pub async fn stream_logs(&self, id: &str) -> Result<()> {
        let mut read = self
            .ws_messages(&format!("/api/apps/{}/logs/stream", id))
            .await
            .context("Failed to connect to log stream")?;

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let val: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => {
                            println!("{}", text);
                            continue;
                        }
                    };

                    match val.get("type").and_then(|t| t.as_str()) {
                        Some("snapshot") => {
                            // Print historical deploy logs
                            if let Some(arr) = val.get("deploy_logs").and_then(|v| v.as_array())
                                && !arr.is_empty()
                            {
                                println!("=== Deploy Logs (snapshot) ===");
                                for log in arr {
                                    let level =
                                        log.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                                    let msg =
                                        log.get("message").and_then(|v| v.as_str()).unwrap_or("");
                                    let ts = log.get("ts").and_then(|v| v.as_str()).unwrap_or("");
                                    println!("[{}] {} {}", level.to_uppercase(), ts, msg);
                                }
                            }
                            // Print historical container logs
                            if let Some(arr) = val.get("container_logs").and_then(|v| v.as_array())
                                && !arr.is_empty()
                            {
                                println!("=== Container Logs (snapshot) ===");
                                for line in arr {
                                    if let Some(s) = line.as_str() {
                                        print!("{}", s);
                                    }
                                }
                            }
                            println!("=== Live stream ===");
                        }
                        Some("deploy") => {
                            let level = val.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                            let msg = val.get("message").and_then(|v| v.as_str()).unwrap_or("");
                            let ts = val.get("ts").and_then(|v| v.as_str()).unwrap_or("");
                            println!("[DEPLOY][{}] {} {}", level.to_uppercase(), ts, msg);
                        }
                        Some("container") => {
                            let line = val.get("line").and_then(|v| v.as_str()).unwrap_or("");
                            print!("{}", line);
                        }
                        _ => {
                            println!("{}", text);
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {} // ignore binary/ping/pong
                Err(e) => {
                    anyhow::bail!("WebSocket error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Set individual env vars. Variables that are not named are left unchanged.
    pub async fn update_env(&self, id: &str, env_vars: Value) -> Result<()> {
        let req = self
            .client
            .patch(format!("{}/api/apps/{}/env", self.base_url, id))
            .json(&serde_json::json!({ "set": env_vars }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn update_domain(&self, id: &str, domain: &str) -> Result<()> {
        let req = self
            .client
            .put(format!("{}/api/apps/{}/domain", self.base_url, id))
            .json(&serde_json::json!({ "domain": domain }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn update_health(
        &self,
        id: &str,
        path: Option<&str>,
        interval: i64,
        threshold: i64,
    ) -> Result<()> {
        let req = self
            .client
            .put(format!("{}/api/apps/{}/health", self.base_url, id))
            .json(&serde_json::json!({
                "health_check_path": path,
                "health_check_interval": interval,
                "health_failure_threshold": threshold,
            }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    /// Add a volume. The server stores it in `<data>/volumes/<app>/<name>`;
    /// `name` defaults to one derived from the container path.
    pub async fn add_volume(
        &self,
        id: &str,
        container_path: &str,
        name: Option<&str>,
    ) -> Result<()> {
        let mut body = serde_json::json!({ "container_path": container_path });
        if let Some(name) = name {
            body["name"] = serde_json::Value::String(name.to_string());
        }
        let req = self
            .client
            .post(format!("{}/api/apps/{}/volumes", self.base_url, id))
            .json(&body);
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn remove_volume(&self, id: &str, container_path: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/apps/{}/volumes", self.base_url, id))
            .json(&serde_json::json!({ "container_path": container_path }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn update_drain_timeout(&self, id: &str, seconds: i64) -> Result<()> {
        let req = self
            .client
            .put(format!("{}/api/apps/{}/drain-timeout", self.base_url, id))
            .json(&serde_json::json!({ "drain_timeout": seconds }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn list_deployments(&self, id: &str) -> Result<Vec<DeploymentResponse>> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}/deployments", self.base_url, id));
        let resp: ApiResponse<Vec<DeploymentResponse>> =
            self.auth_header(req).send().await?.json().await?;

        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn rollback_app(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/apps/{}/rollback", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    /// Generate (or regenerate) an Ed25519 deploy key for an app.
    /// Returns the public key string.
    pub async fn generate_deploy_key(&self, id: &str) -> Result<String> {
        let req = self
            .client
            .post(format!("{}/api/apps/{}/deploy-key", self.base_url, id));
        let resp: ApiResponse<DeployKeyResponse> =
            self.auth_header(req).send().await?.json().await?;

        if resp.success {
            resp.data.context("No data returned").map(|d| d.public_key)
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Get the current public deploy key for an app.
    pub async fn get_deploy_key(&self, id: &str) -> Result<String> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}/deploy-key", self.base_url, id));
        let resp: ApiResponse<DeployKeyResponse> =
            self.auth_header(req).send().await?.json().await?;

        if resp.success {
            resp.data.context("No data returned").map(|d| d.public_key)
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Webhook URLs for an app (the secret is revealed separately).
    pub async fn get_webhook_config(&self, id: &str) -> Result<WebhookConfigResponse> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}/webhook", self.base_url, id));
        let resp: ApiResponse<WebhookConfigResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Replace the webhook secret with a caller-chosen value.
    pub async fn set_webhook_secret(&self, id: &str, secret: &str) -> Result<()> {
        let req = self
            .client
            .put(format!("{}/api/apps/{}/webhook-secret", self.base_url, id))
            .json(&serde_json::json!({ "secret": secret }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    /// Reveal the app's webhook secret (admin only, audit-logged).
    pub async fn reveal_webhook_secret(&self, id: &str) -> Result<WebhookSecretResponse> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}/webhook-secret", self.base_url, id));
        let resp: ApiResponse<WebhookSecretResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Replace the webhook secret with a freshly generated one; returns it.
    pub async fn rotate_webhook_secret(&self, id: &str) -> Result<WebhookSecretResponse> {
        let req = self.client.post(format!(
            "{}/api/apps/{}/webhook-secret/rotate",
            self.base_url, id
        ));
        let resp: ApiResponse<WebhookSecretResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Enable or disable auto-deploy, optionally setting the watch branch.
    pub async fn set_auto_deploy(
        &self,
        id: &str,
        auto_deploy: bool,
        watch_branch: Option<&str>,
    ) -> Result<()> {
        let req = self
            .client
            .put(format!("{}/api/apps/{}/auto-deploy", self.base_url, id))
            .json(&serde_json::json!({
                "auto_deploy": auto_deploy,
                "watch_branch": watch_branch,
            }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    // ── Preview Deployments ──

    /// Create a preview deployment for an app. Returns the preview row immediately
    /// (status = "building"); the actual build runs in the background.
    pub async fn create_preview(
        &self,
        id: &str,
        ttl_hours: Option<i64>,
    ) -> Result<PreviewDeploymentResponse> {
        let body = serde_json::json!({ "ttl_hours": ttl_hours });
        let req = self
            .client
            .post(format!("{}/api/apps/{}/preview", self.base_url, id))
            .json(&body);
        let resp: ApiResponse<PreviewDeploymentResponse> =
            self.auth_header(req).send().await?.json().await?;

        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// List active (non-expired) preview deployments for an app.
    pub async fn list_previews(&self, id: &str) -> Result<Vec<PreviewDeploymentResponse>> {
        let req = self
            .client
            .get(format!("{}/api/apps/{}/previews", self.base_url, id));
        let resp: ApiResponse<Vec<PreviewDeploymentResponse>> =
            self.auth_header(req).send().await?.json().await?;

        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Stop and remove a specific preview deployment.
    pub async fn delete_preview(&self, id: &str, preview_id: &str) -> Result<()> {
        let req = self.client.delete(format!(
            "{}/api/apps/{}/previews/{}",
            self.base_url, id, preview_id
        ));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;

        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    // ── Projects ──

    pub async fn list_projects(&self) -> Result<Vec<ProjectResponse>> {
        let req = self.client.get(format!("{}/api/projects", self.base_url));
        let resp: ApiResponse<Vec<ProjectResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn create_project(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<ProjectResponse> {
        let body = serde_json::json!({ "name": name, "description": description });
        let req = self
            .client
            .post(format!("{}/api/projects", self.base_url))
            .json(&body);
        let resp: ApiResponse<ProjectResponse> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn get_project(&self, id: &str) -> Result<ProjectResponse> {
        let req = self
            .client
            .get(format!("{}/api/projects/{}", self.base_url, id));
        let resp: ApiResponse<ProjectResponse> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn delete_project(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/projects/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn deploy_project(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/projects/{}/deploy", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    /// Assign an app to a project (or remove it when project_id is None).
    pub async fn assign_app_project(&self, app_id: &str, project_id: Option<&str>) -> Result<()> {
        let body = serde_json::json!({ "project_id": project_id });
        let req = self
            .client
            .put(format!("{}/api/apps/{}/project", self.base_url, app_id))
            .json(&body);
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    // ── Project-level Previews ──

    /// Create a project preview (one preview deployment per member app).
    /// Returns the project preview row immediately with status "building".
    pub async fn create_project_preview(
        &self,
        project_id: &str,
        ttl_hours: Option<i64>,
    ) -> Result<ProjectPreviewResponse> {
        let body = serde_json::json!({ "ttl_hours": ttl_hours });
        let req = self
            .client
            .post(format!(
                "{}/api/projects/{}/previews",
                self.base_url, project_id
            ))
            .json(&body);
        let resp: ApiResponse<ProjectPreviewResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// List active project previews for a project.
    pub async fn list_project_previews(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectPreviewResponse>> {
        let req = self.client.get(format!(
            "{}/api/projects/{}/previews",
            self.base_url, project_id
        ));
        let resp: ApiResponse<Vec<ProjectPreviewResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Delete (stop + expire) a project preview.
    pub async fn delete_project_preview(&self, project_id: &str, preview_id: &str) -> Result<()> {
        let req = self.client.delete(format!(
            "{}/api/projects/{}/previews/{}",
            self.base_url, project_id, preview_id
        ));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    // ── Managed Databases ──

    pub async fn list_databases(&self) -> Result<Vec<ManagedDatabaseResponse>> {
        let req = self.client.get(format!("{}/api/databases", self.base_url));
        let resp: ApiResponse<Vec<ManagedDatabaseResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn create_database(
        &self,
        name: &str,
        db_type: &str,
        project_id: Option<&str>,
        memory_limit: Option<i64>,
    ) -> Result<CreatedDatabaseResponse> {
        let body = CreateDatabaseRequest {
            name: name.to_string(),
            db_type: db_type.to_string(),
            project_id: project_id.map(|s| s.to_string()),
            memory_limit,
            public_access: false,
        };
        let req = self
            .client
            .post(format!("{}/api/databases", self.base_url))
            .json(&body);
        let resp: ApiResponse<CreatedDatabaseResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Reveal a database's passwords and connection strings (admin only,
    /// audit-logged).
    pub async fn get_database_credentials(&self, id: &str) -> Result<DatabaseCredentialsResponse> {
        let req = self.client.get(format!(
            "{}/api/databases/{}/credentials",
            self.base_url, id
        ));
        let resp: ApiResponse<DatabaseCredentialsResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn get_database(&self, id: &str) -> Result<ManagedDatabaseDetail> {
        let req = self
            .client
            .get(format!("{}/api/databases/{}", self.base_url, id));
        let resp: ApiResponse<ManagedDatabaseDetail> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn delete_database(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/databases/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn stop_database(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/databases/{}/stop", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn start_database(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/databases/{}/start", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    // ── Cron Jobs ──

    pub async fn list_cron_jobs(&self) -> Result<Vec<CronJobResponse>> {
        let req = self.client.get(format!("{}/api/cron", self.base_url));
        let resp: ApiResponse<Vec<CronJobResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn create_cron_job(
        &self,
        name: &str,
        schedule: &str,
        image: &str,
        command: Option<&[String]>,
        project_id: Option<&str>,
    ) -> Result<CronJobResponse> {
        let body = CreateCronJobRequest {
            name: name.to_string(),
            schedule: schedule.to_string(),
            image: image.to_string(),
            command: command.map(|c| c.to_vec()),
            project_id: project_id.map(|s| s.to_string()),
            timeout_seconds: None,
            env: None,
            concurrency_policy: None,
            memory_mb: None,
            cpu_shares: None,
            timezone: None,
            max_retries: None,
            retry_delay_seconds: None,
            volumes: None,
        };
        let req = self
            .client
            .post(format!("{}/api/cron", self.base_url))
            .json(&body);
        let resp: ApiResponse<CronJobResponse> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn get_cron_job(&self, id: &str) -> Result<CronJobResponse> {
        let req = self
            .client
            .get(format!("{}/api/cron/{}", self.base_url, id));
        let resp: ApiResponse<CronJobResponse> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn update_cron_job(
        &self,
        id: &str,
        schedule: Option<&str>,
        image: Option<&str>,
        command: Option<&[String]>,
        enabled: Option<bool>,
    ) -> Result<CronJobResponse> {
        let body = UpdateCronJobRequest {
            name: None,
            schedule: schedule.map(|s| s.to_string()),
            image: image.map(|s| s.to_string()),
            command: command.map(|c| c.to_vec()),
            enabled,
            timeout_seconds: None,
            env: None,
            env_set: Default::default(),
            env_unset: vec![],
            project_id: None,
            concurrency_policy: None,
            memory_mb: None,
            cpu_shares: None,
            timezone: None,
            max_retries: None,
            retry_delay_seconds: None,
            volumes: None,
        };
        let req = self
            .client
            .patch(format!("{}/api/cron/{}", self.base_url, id))
            .json(&body);
        let resp: ApiResponse<CronJobResponse> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn delete_cron_job(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/cron/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn trigger_cron_job(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/cron/{}/trigger", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    pub async fn list_cron_job_runs(&self, id: &str) -> Result<Vec<CronJobRun>> {
        let req = self
            .client
            .get(format!("{}/api/cron/{}/runs", self.base_url, id));
        let resp: ApiResponse<Vec<CronJobRun>> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    // ── Phase 7.1 — Password Management ──

    /// Change the current user's password.
    /// The server verifies `current_password` with argon2, then re-hashes
    /// `new_password` and invalidates all other active sessions.
    pub async fn change_password(&self, current_password: &str, new_password: &str) -> Result<()> {
        let req = self
            .client
            .post(format!("{}/api/auth/change-password", self.base_url))
            .json(&serde_json::json!({
                "current_password": current_password,
                "new_password": new_password
            }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    // ── Settings ──

    /// Fetch the current server-wide settings.
    pub async fn get_settings(&self) -> Result<ServerSettings> {
        let req = self.client.get(format!("{}/api/settings", self.base_url));
        let resp: ApiResponse<ServerSettings> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Patch one or more server-wide settings. Only supplied fields are updated.
    pub async fn update_settings(&self, req_body: UpdateSettingsRequest) -> Result<ServerSettings> {
        let req = self
            .client
            .put(format!("{}/api/settings", self.base_url))
            .json(&req_body);
        let resp: ApiResponse<ServerSettings> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Whether the dashboard's host terminal is switched on.
    pub async fn get_host_terminal(&self) -> Result<bool> {
        let req = self
            .client
            .get(format!("{}/api/settings/host-terminal", self.base_url));
        let resp: ApiResponse<HostTerminalSetting> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.context("No data returned")?.enabled)
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Switch the dashboard's host terminal on or off. The server accepts this
    /// only over the local Unix socket.
    pub async fn set_host_terminal(&self, enabled: bool) -> Result<bool> {
        let req = self
            .client
            .put(format!("{}/api/settings/host-terminal", self.base_url))
            .json(&HostTerminalSetting { enabled });
        let resp: ApiResponse<HostTerminalSetting> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.context("No data returned")?.enabled)
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    // ── Notification Integrations ──

    /// List all notification integrations.
    pub async fn list_integrations(&self) -> Result<Vec<IntegrationResponse>> {
        let req = self
            .client
            .get(format!("{}/api/integrations", self.base_url));
        let resp: ApiResponse<Vec<IntegrationResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Get a single integration by ID.
    pub async fn get_integration(&self, id: &str) -> Result<IntegrationResponse> {
        let req = self
            .client
            .get(format!("{}/api/integrations/{}", self.base_url, id));
        let resp: ApiResponse<IntegrationResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Delete an integration.
    pub async fn delete_integration(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/integrations/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if !resp.success {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()));
        }
        Ok(())
    }

    /// Send a test notification through an integration.
    pub async fn test_integration(&self, id: &str) -> Result<String> {
        let req = self
            .client
            .post(format!("{}/api/integrations/{}/test", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp
                .data
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_else(|| "Test sent".into()))
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Enable or disable an integration.
    pub async fn set_integration_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<IntegrationResponse> {
        let req = self
            .client
            .patch(format!("{}/api/integrations/{}", self.base_url, id))
            .json(&serde_json::json!({ "enabled": enabled }));
        let resp: ApiResponse<IntegrationResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    // ── Storage providers ─────────────────────────────────────────────────────

    pub async fn list_storage_providers(&self) -> Result<Vec<StorageProviderResponse>> {
        let req = self
            .client
            .get(format!("{}/api/storage-providers", self.base_url));
        let resp: ApiResponse<Vec<StorageProviderResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_storage_provider(
        &self,
        name: &str,
        endpoint: &str,
        region: &str,
        bucket: &str,
        prefix: Option<&str>,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> Result<StorageProviderResponse> {
        let req = self
            .client
            .post(format!("{}/api/storage-providers", self.base_url))
            .json(&serde_json::json!({
                "name": name,
                "kind": "s3",
                "config": {
                    "endpoint": endpoint,
                    "region": region,
                    "bucket": bucket,
                    "prefix": prefix.unwrap_or(""),
                    "access_key_id": access_key_id,
                    "secret_access_key": secret_access_key,
                }
            }));
        let resp: ApiResponse<StorageProviderResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn test_storage_provider(&self, id: &str) -> Result<String> {
        let req = self.client.post(format!(
            "{}/api/storage-providers/{}/test",
            self.base_url, id
        ));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp
                .data
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_else(|| "Connection successful".into()))
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn delete_storage_provider(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/storage-providers/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    // ── Snapshots ─────────────────────────────────────────────────────────────

    /// Take a full snapshot. Returns once the local copy is complete; a remote
    /// upload (when configured) continues in the background.
    pub async fn create_snapshot(&self, name: Option<&str>) -> Result<SnapshotResponse> {
        let req = self
            .client
            .post(format!("{}/api/snapshots", self.base_url))
            .json(&serde_json::json!({ "name": name }));
        let resp: ApiResponse<SnapshotResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn list_snapshots(&self, app: Option<&str>) -> Result<Vec<SnapshotResponse>> {
        let mut url = format!("{}/api/snapshots", self.base_url);
        if let Some(a) = app {
            url.push_str(&format!("?app={}", a));
        }
        let req = self.client.get(url);
        let resp: ApiResponse<Vec<SnapshotResponse>> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp.data.unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn delete_snapshot(&self, id: &str) -> Result<()> {
        let req = self
            .client
            .delete(format!("{}/api/snapshots/{}", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn restore_snapshot_full(&self, id: &str) -> Result<String> {
        let req = self
            .client
            .post(format!("{}/api/snapshots/{}/restore", self.base_url, id));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(resp
                .data
                .and_then(|v| {
                    v.get("pre_restore_snapshot_id")
                        .and_then(|m| m.as_str())
                        .map(String::from)
                })
                .unwrap_or_default())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn restore_app_from_snapshot(&self, snapshot_id: &str, app_name: &str) -> Result<()> {
        let req = self
            .client
            .post(format!(
                "{}/api/snapshots/{}/restore-app",
                self.base_url, snapshot_id
            ))
            .json(&serde_json::json!({ "app_name": app_name }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn rollback_database(&self, snapshot_id: &str, db_name: &str) -> Result<()> {
        let req = self
            .client
            .post(format!(
                "{}/api/snapshots/{}/restore-database",
                self.base_url, snapshot_id
            ))
            .json(&serde_json::json!({ "db_name": db_name }));
        let resp: ApiResponse<Value> = self.auth_header(req).send().await?.json().await?;
        if resp.success {
            Ok(())
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn get_backup_config(&self) -> Result<BackupConfigResponse> {
        let req = self
            .client
            .get(format!("{}/api/snapshots/config", self.base_url));
        let resp: ApiResponse<BackupConfigResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    pub async fn update_backup_config(
        &self,
        body: serde_json::Value,
    ) -> Result<BackupConfigResponse> {
        let req = self
            .client
            .put(format!("{}/api/snapshots/config", self.base_url))
            .json(&body);
        let resp: ApiResponse<BackupConfigResponse> =
            self.auth_header(req).send().await?.json().await?;
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    // ── API tokens ──

    /// Unwrap an API envelope into its data.
    fn into_data<T: serde::Serialize>(resp: ApiResponse<T>) -> Result<T> {
        if resp.success {
            resp.data.context("No data returned")
        } else {
            anyhow::bail!("{}", resp.error.unwrap_or_else(|| "Unknown error".into()))
        }
    }

    /// Create an API token; the response carries the secret (shown once).
    pub async fn create_token(&self, body: &CreateTokenRequest) -> Result<CreatedTokenResponse> {
        let req = self
            .client
            .post(format!("{}/api/tokens", self.base_url))
            .json(body);
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }

    /// List API tokens (every user's when `all`).
    pub async fn list_tokens(&self, all: bool) -> Result<Vec<TokenResponse>> {
        let req = self
            .client
            .get(format!("{}/api/tokens?all={all}", self.base_url));
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }

    /// Revoke an API token by id.
    pub async fn revoke_token(&self, id: &str) -> Result<TokenResponse> {
        let req = self
            .client
            .delete(format!("{}/api/tokens/{}", self.base_url, id));
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }

    // ── Users ──

    pub async fn list_users(&self) -> Result<Vec<UserResponse>> {
        let req = self.client.get(format!("{}/api/users", self.base_url));
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }

    /// Issue a one-time set-password link for a user (invite or reset).
    pub async fn create_password_link(&self, user_id: &str) -> Result<PasswordLinkResponse> {
        let req = self.client.post(format!(
            "{}/api/users/{}/password-link",
            self.base_url, user_id
        ));
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }

    // ── Audit log ──

    /// Verify the audit log's hash chain.
    pub async fn verify_audit_log(&self) -> Result<AuditVerifyReport> {
        let req = self
            .client
            .get(format!("{}/api/audit-log/verify", self.base_url));
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }

    /// One page of the audit log, newest first.
    pub async fn list_audit_log(&self, query: &[(&str, String)]) -> Result<AuditLogPage> {
        let qs: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
            .collect();
        let req = self
            .client
            .get(format!("{}/api/audit-log?{}", self.base_url, qs.join("&")));
        Self::into_data(self.auth_header(req).send().await?.json().await?)
    }
}

/// Turn the upload endpoint's reply into a result. The server refuses unsafe
/// or oversized archives with a JSON error that says what to change; anything
/// else (a proxy's HTML page, an empty body) is reported with its status.
fn upload_result(status: reqwest::StatusCode, body: &str) -> Result<()> {
    match serde_json::from_str::<ApiResponse<Value>>(body) {
        Ok(resp) if resp.success => Ok(()),
        Ok(resp) => anyhow::bail!(
            "{}",
            resp.error
                .unwrap_or_else(|| format!("Upload failed ({status})"))
        ),
        Err(_) => anyhow::bail!(
            "Upload failed ({status}){}",
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim().chars().take(300).collect::<String>())
            }
        ),
    }
}

#[cfg(test)]
mod upload_result_tests {
    use super::*;

    #[test]
    fn upload_result_surfaces_server_errors_and_non_json_replies() {
        assert!(
            upload_result(
                reqwest::StatusCode::ACCEPTED,
                r#"{"success":true,"data":{}}"#
            )
            .is_ok()
        );
        let err = upload_result(
            reqwest::StatusCode::PAYLOAD_TOO_LARGE,
            r#"{"success":false,"error":"Too large: add build output to .tondignore"}"#,
        )
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
        assert!(
            err.contains(".tondignore"),
            "server message should be shown: {err}"
        );
        let err = upload_result(reqwest::StatusCode::BAD_GATEWAY, "<html>bad gateway</html>")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("502"),
            "status should be shown for non-JSON replies: {err}"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Clone, Default)]
    struct CaptureState {
        auth_header: Arc<Mutex<Option<String>>>,
        body: Arc<Mutex<Option<Value>>>,
    }

    async fn spawn_test_server(router: Router) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|err| panic!("failed to bind test server listener: {err}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|err| panic!("failed to read test server address: {err}"));
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .unwrap_or_else(|err| panic!("test server failed: {err}"));
        });
        addr
    }

    fn app_response_json() -> Value {
        json!({
            "id": "app-1",
            "name": "demo",
            "domain": null,
            "source_type": "git",
            "source_url": "https://example.com/repo.git",
            "status": "running",
            "container_port": 3000,
            "host_port": 18080,
            "memory_limit": 268435456,
            "cpu_limit": 500000000,
            "health_check_path": null,
            "health_check_interval": 30,
            "health_failure_threshold": 3,
            "health_consecutive_failures": 0,
            "volumes": [],
            "drain_timeout": 30,
            "build_method": "tbuild",
            "auto_deploy": false,
            "watch_branch": null,
            "deploy_key_pub": null,
            "project_id": null,
            "registry_credential_id": null,
            "github_app_id": null,
            "gitlab_token_id": null,
            "preview_enabled": false,
            "preview_shared_keys": [],
            "preview_env_vars": {},
            "dockerfile_content": null,
            "compose_content": null,
            "base_dir": null,
            "static_build": false,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        })
    }

    #[test]
    fn new_trims_trailing_slash_from_base_url() {
        let client = TondClient::new("https://tond.example.com/", None);

        assert_eq!(
            client.base_url, "https://tond.example.com",
            "TondClient::new should trim trailing slashes from the base URL"
        );
    }

    #[test]
    fn ws_url_converts_http_and_https_schemes() {
        let http_client = TondClient::new("http://localhost:3001", None);
        assert_eq!(
            http_client.ws_url("/api/apps/demo/logs/stream"),
            "ws://localhost:3001/api/apps/demo/logs/stream",
            "ws_url should convert http:// base URLs to ws://"
        );

        let https_client = TondClient::new("https://tond.example.com", None);
        assert_eq!(
            https_client.ws_url("/api/apps/demo/logs/stream"),
            "wss://tond.example.com/api/apps/demo/logs/stream",
            "ws_url should convert https:// base URLs to wss://"
        );
    }

    #[test]
    fn auth_header_adds_bearer_token_only_when_configured() {
        let client_with_key = TondClient::new("http://localhost:3001", Some("secret"));
        let request_with_key = client_with_key
            .auth_header(client_with_key.client.get("http://localhost:3001/api/apps"))
            .build()
            .unwrap_or_else(|err| panic!("failed to build authorized request: {err}"));
        assert_eq!(
            request_with_key
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer secret"),
            "auth_header should attach the configured bearer token"
        );

        let client_without_key = TondClient::new("http://localhost:3001", None);
        let request_without_key = client_without_key
            .auth_header(
                client_without_key
                    .client
                    .get("http://localhost:3001/api/apps"),
            )
            .build()
            .unwrap_or_else(|err| panic!("failed to build unauthenticated request: {err}"));
        assert_eq!(
            request_without_key.headers().get("authorization"),
            None,
            "auth_header should leave requests untouched when no API token is configured"
        );
    }

    #[tokio::test]
    async fn local_client_talks_to_server_over_unix_socket_without_api_key() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let socket_path = dir.path().join("tond.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path)
            .unwrap_or_else(|err| panic!("failed to bind test Unix socket: {err}"));
        let state = CaptureState::default();
        let router = Router::new()
            .route(
                "/api/apps",
                get(
                    |State(state): State<CaptureState>, headers: HeaderMap| async move {
                        *state.auth_header.lock().await = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        axum::Json(json!({ "success": true, "data": [], "error": null }))
                    },
                ),
            )
            .with_state(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .unwrap_or_else(|err| panic!("test server failed: {err}"));
        });

        let client = TondClient::local(&socket_path)
            .unwrap_or_else(|err| panic!("expected local client to connect: {err}"));
        let apps = client
            .list_apps()
            .await
            .unwrap_or_else(|err| panic!("expected list_apps over the socket to succeed: {err}"));

        assert!(apps.is_empty());
        assert_eq!(
            *state.auth_header.lock().await,
            None,
            "the local socket client should not send credentials"
        );
    }

    #[test]
    fn local_client_reports_missing_socket() {
        let dir =
            tempfile::tempdir().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let socket_path = dir.path().join("missing.sock");

        let error = TondClient::local(&socket_path)
            .err()
            .unwrap_or_else(|| panic!("connecting to a missing socket should fail"));

        assert!(
            error.to_string().contains("socket not found"),
            "a missing socket should produce an actionable message, got: {error}"
        );
    }

    #[tokio::test]
    async fn list_apps_returns_empty_vec_when_success_response_has_no_data() {
        let router = Router::new().route(
            "/api/apps",
            get(|| async { axum::Json(json!({ "success": true, "data": null, "error": null })) }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let apps = client
            .list_apps()
            .await
            .unwrap_or_else(|err| panic!("expected list_apps to succeed: {err}"));

        assert!(
            apps.is_empty(),
            "list_apps should fall back to an empty vector when the API omits data on success"
        );
    }

    #[tokio::test]
    async fn list_apps_returns_unknown_error_when_api_fails_without_message() {
        let router = Router::new().route(
            "/api/apps",
            get(|| async { axum::Json(json!({ "success": false, "data": null, "error": null })) }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let error = client
            .list_apps()
            .await
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected list_apps to fail"));

        assert_eq!(
            error.to_string(),
            "Unknown error",
            "list_apps should surface a fallback error message when the API does not provide one"
        );
    }

    #[tokio::test]
    async fn get_app_requires_data_in_successful_response() {
        let router = Router::new().route(
            "/api/apps/app-1",
            get(|| async { axum::Json(json!({ "success": true, "data": null, "error": null })) }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let error = client
            .get_app("app-1")
            .await
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected get_app to fail"));

        assert_eq!(
            error.to_string(),
            "No data returned",
            "get_app should fail when the API reports success but omits the app payload"
        );
    }

    #[tokio::test]
    async fn create_app_sends_expected_payload_and_authorization_header() {
        let capture = CaptureState::default();
        let router = Router::new()
            .route(
                "/api/apps",
                post(
                    |State(state): State<CaptureState>, headers: HeaderMap, body: Bytes| async move {
                        *state.auth_header.lock().await = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        *state.body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode create_app request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": app_response_json(),
                            "error": null
                        })))
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), Some("secret-key"));

        let response = client
            .create_app(
                "demo",
                "git",
                Some("https://example.com/repo.git".to_string()),
                Some("main".to_string()),
                3000,
                None,
                None,
                Some("demo.example.com".to_string()),
                Some(512),
                Some(123456789),
                Some("/health".to_string()),
                Some(15),
                Some(5),
                Some(45),
                Some("dockerfile".to_string()),
            )
            .await
            .unwrap_or_else(|err| panic!("expected create_app to succeed: {err}"));

        assert_eq!(
            response.name, "demo",
            "create_app should return the deserialized app payload on success"
        );

        let auth_header = capture.auth_header.lock().await.clone();
        assert_eq!(
            auth_header.as_deref(),
            Some("Bearer secret-key"),
            "create_app should attach the bearer token when an API token is configured"
        );

        let body = capture
            .body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected create_app request body to be captured"));
        assert_eq!(
            body.get("name"),
            Some(&json!("demo")),
            "create_app should send the requested app name"
        );
        assert_eq!(
            body.get("source_type"),
            Some(&json!("git")),
            "create_app should send the requested source type"
        );
        assert_eq!(
            body.get("auto_deploy"),
            Some(&json!(false)),
            "create_app should default auto_deploy to false"
        );
        assert_eq!(
            body.get("static_build"),
            Some(&json!(false)),
            "create_app should default static_build to false"
        );
        assert_eq!(
            body.get("build_method"),
            Some(&json!("dockerfile")),
            "create_app should forward the requested build method"
        );
    }

    #[tokio::test]
    async fn create_app_sends_image_template_payload_without_build_method() {
        let capture = CaptureState::default();
        let router = Router::new()
            .route(
                "/api/apps",
                post(
                    |State(state): State<CaptureState>, headers: HeaderMap, body: Bytes| async move {
                        *state.auth_header.lock().await = headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        *state.body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode create_app request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": app_response_json(),
                            "error": null
                        })))
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        client
            .create_app(
                "analytics",
                "image",
                Some("ghcr.io/plausible/community-edition".to_string()),
                None,
                8000,
                None,
                None,
                None,
                None,
                None,
                Some("/".to_string()),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap_or_else(|err| panic!("expected image template create_app to succeed: {err}"));

        let body = capture
            .body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected create_app request body to be captured"));
        assert_eq!(
            body.get("source_type"),
            Some(&json!("image")),
            "templated image creates should send image as the source type"
        );
        assert_eq!(
            body.get("source_url"),
            Some(&json!("ghcr.io/plausible/community-edition")),
            "templated image creates should send the template image reference"
        );
        assert_eq!(
            body.get("build_method"),
            Some(&Value::Null),
            "templated image creates should omit build_method so the server accepts the payload"
        );
    }

    #[tokio::test]
    async fn create_project_and_database_send_expected_payloads() {
        #[derive(Clone, Default)]
        struct MultiCaptureState {
            project_body: std::sync::Arc<tokio::sync::Mutex<Option<Value>>>,
            database_body: std::sync::Arc<tokio::sync::Mutex<Option<Value>>>,
        }

        let capture = MultiCaptureState::default();
        let router = Router::new()
            .route(
                "/api/projects",
                post(
                    |State(state): State<MultiCaptureState>, body: Bytes| async move {
                        *state.project_body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode project request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": {
                                "id": "project-1",
                                "name": "analytics",
                                "description": "Shared analytics services",
                                "env_vars": {},
                                "created_at": "2026-01-01T00:00:00Z",
                                "updated_at": "2026-01-01T00:00:00Z",
                                "active_preview_count": 0
                            },
                            "error": null
                        })))
                    },
                ),
            )
            .route(
                "/api/databases",
                post(
                    |State(state): State<MultiCaptureState>, body: Bytes| async move {
                        *state.database_body.lock().await = Some(
                            serde_json::from_slice(&body)
                                .unwrap_or_else(|err| panic!("failed to decode database request body: {err}")),
                        );

                        (StatusCode::OK, axum::Json(json!({
                            "success": true,
                            "data": {
                                "id": "db-1",
                                "name": "analytics-db",
                                "db_type": "postgres",
                                "project_id": "project-1",
                                "host_port": 5432,
                                "status": "running",
                                "db_name": "tond_analytics",
                                "db_user": "tond_analytics",
                                "has_password": true,
                                "memory_limit": null,
                                "custom_external_url": null,
                                "custom_internal_url": null,
                                "image_used": null,
                                "custom_image": null,
                                "created_at": "2026-01-01T00:00:00Z",
                                "updated_at": "2026-01-01T00:00:00Z",
                                "connection_string": "postgres://tond_analytics:••••••••@localhost:5432/tond_analytics",
                                "internal_connection_string": "postgres://tond_analytics:••••••••@analytics-db:5432/tond_analytics",
                                "credentials": {
                                    "db_user": "tond_analytics",
                                    "db_password": "secret",
                                    "root_password": null,
                                    "connection_string": "postgres://tond_analytics:secret@localhost:5432/tond_analytics",
                                    "public_connection_string": null,
                                    "internal_connection_string": "postgres://tond_analytics:secret@analytics-db:5432/tond_analytics"
                                }
                            },
                            "error": null
                        })))
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let project = client
            .create_project("analytics", Some("Shared analytics services"))
            .await
            .unwrap_or_else(|err| panic!("expected create_project to succeed: {err}"));
        let db = client
            .create_database("analytics-db", "postgres", Some(&project.id), None)
            .await
            .unwrap_or_else(|err| panic!("expected create_database to succeed: {err}"));

        assert_eq!(
            project.id, "project-1",
            "project creation should return the server id"
        );
        assert_eq!(
            db.detail.database.project_id.as_deref(),
            Some("project-1"),
            "database creation should preserve the supplied project id"
        );

        let project_body = capture
            .project_body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected project request body to be captured"));
        let database_body = capture
            .database_body
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| panic!("expected database request body to be captured"));

        assert_eq!(
            project_body.get("name"),
            Some(&json!("analytics")),
            "create_project should send the requested project name"
        );
        assert_eq!(
            database_body.get("project_id"),
            Some(&json!("project-1")),
            "create_database should send the created project id when attaching the dependency"
        );
    }

    #[tokio::test]
    async fn test_integration_uses_default_message_when_response_omits_one() {
        let router = Router::new().route(
            "/api/integrations/integration-1/test",
            post(|| async {
                axum::Json(json!({
                    "success": true,
                    "data": {},
                    "error": null
                }))
            }),
        );
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let message = client
            .test_integration("integration-1")
            .await
            .unwrap_or_else(|err| panic!("expected test_integration to succeed: {err}"));

        assert_eq!(
            message, "Test sent",
            "test_integration should fall back to a default success message when the API omits one"
        );
    }
}
