// commands/apps.rs — `tond apps` subcommand definitions and handler
//
// JSON output (--json): list commands print arrays, `info`/`create` print the
// app, mutations without a resource print `{"ok":true,"action":…}`
// (Output::done), `deploy`/`push` stream NDJSON events while waiting (see
// deploy_follow.rs) or print the queued deployment with --no-wait, and
// `logs --follow` prints the server's log frames as NDJSON.

use anyhow::Result;
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tond_common::{DatabaseCredentialsResponse, TemplateManifest, load_template_manifests};

use crate::client::TondClient;
use crate::deploy_follow;
use crate::error::{self, CliError};
use crate::output::{Output, out, outln};

#[derive(Args)]
pub struct CreateAppOptions {
    /// App name (alphanumeric + hyphens)
    pub name: String,

    /// Curated in-repo template to use for app creation
    #[arg(long, conflicts_with_all = ["source_type", "source", "branch", "build_method"])]
    pub template: Option<String>,

    /// Source type: git, image
    #[arg(short = 't', long, default_value = "git")]
    pub source_type: String,

    /// Git URL or Docker image
    #[arg(short, long)]
    pub source: Option<String>,

    /// Git branch
    #[arg(short, long, default_value = "main")]
    pub branch: String,

    /// Container port
    #[arg(short, long, default_value = "3000")]
    pub port: i64,

    /// Custom domain
    #[arg(short, long)]
    pub domain: Option<String>,

    /// Environment variable override in KEY=VALUE format. Repeat for multiple values.
    #[arg(long = "env")]
    pub env: Vec<String>,

    /// Memory limit in megabytes (default: 256)
    #[arg(long)]
    pub memory_mb: Option<i64>,

    /// CPU limit as a float (e.g. 0.5 = half a core, 1.0 = one core; default: 0.5)
    #[arg(long)]
    pub cpus: Option<f64>,

    /// HTTP path for health checks (e.g. /health); omit to disable
    #[arg(long)]
    pub health_path: Option<String>,

    /// Seconds between health check polls (default: 30)
    #[arg(long)]
    pub health_interval: Option<i64>,

    /// Consecutive failures before marking unhealthy (default: 3)
    #[arg(long)]
    pub health_threshold: Option<i64>,

    /// Seconds to wait for the new container to pass health checks before
    /// cutting traffic over during a graceful redeploy (default: 30)
    #[arg(long)]
    pub drain_timeout: Option<i64>,

    /// Build method for git/tarball sources: tbuild (server default) or dockerfile.
    /// "tbuild" scans the repo and auto-generates a Dockerfile. Only sent when given,
    /// since the server rejects a build method for image sources.
    #[arg(long)]
    pub build_method: Option<String>,

    /// Skip interactive prompts and fail if required template inputs are missing.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// `--wait` / `--no-wait` of commands that start a deployment.
#[derive(Args, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WaitArgs {
    /// Stream the build and exit non-zero if it fails (the default)
    #[arg(long, conflicts_with = "no_wait")]
    pub wait: bool,

    /// Return as soon as the deploy is queued and print the deployment id
    #[arg(long, visible_alias = "no-follow")]
    pub no_wait: bool,
}

impl WaitArgs {
    pub fn should_wait(&self) -> bool {
        !self.no_wait
    }
}

#[derive(Subcommand)]
pub enum AppsAction {
    /// List all deployed apps
    #[command(visible_alias = "ls")]
    List,

    /// Create a new app
    Create(Box<CreateAppOptions>),

    /// Deploy an app (build & start from stored source); waits and streams the
    /// build unless --no-wait. Exits 6 if the build or deploy fails.
    Deploy {
        /// App name or ID
        app: String,

        #[command(flatten)]
        wait: WaitArgs,
    },

    /// Deploy the current directory to an app (tar + upload); waits like `deploy`
    Push {
        /// App name or ID
        app: String,

        #[command(flatten)]
        wait: WaitArgs,
    },

    /// Show app details
    #[command(visible_aliases = ["show", "get"])]
    Info {
        /// App name or ID
        app: String,
    },

    /// View app logs
    Logs {
        /// App name or ID
        app: String,

        /// Stream live logs (press Ctrl-C to stop)
        #[arg(short, long)]
        follow: bool,
    },

    /// Stop an app
    Stop {
        /// App name or ID
        app: String,
    },

    /// Start a stopped app
    Start {
        /// App name or ID
        app: String,
    },

    /// Restart an app
    Restart {
        /// App name or ID
        app: String,
    },

    /// Delete an app (asks first; --yes to skip)
    #[command(visible_alias = "rm")]
    Delete {
        /// App name or ID
        app: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Set environment variables
    Env {
        /// App name or ID
        app: String,

        /// Environment variables in KEY=VALUE format
        #[arg(required = true)]
        vars: Vec<String>,
    },

    /// Set custom domain for an app
    Domain {
        /// App name or ID
        app: String,

        /// Domain name
        domain: String,
    },

    /// Configure health checks for an app
    Health {
        /// App name or ID
        app: String,

        /// HTTP path to poll (e.g. /health); omit to disable health checks
        #[arg(long)]
        path: Option<String>,

        /// Seconds between health check polls (default: 30)
        #[arg(long, default_value = "30")]
        interval: i64,

        /// Consecutive failures before auto-restart (default: 3)
        #[arg(long, default_value = "3")]
        threshold: i64,

        /// Disable health checks for this app
        #[arg(long)]
        disable: bool,
    },

    /// Set the graceful-deploy drain timeout for an app
    DrainTimeout {
        /// App name or ID
        app: String,

        /// Seconds to wait for the new container to become healthy before
        /// cutting traffic over. Set to 0 to disable graceful draining.
        seconds: i64,
    },

    /// Show deployment history for an app
    Deployments {
        /// App name or ID
        app: String,
    },

    /// Roll back an app to its previous deployment (asks first; --yes to skip)
    Rollback {
        /// App name or ID
        app: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Show or generate the SSH deploy key for an app
    GitKey {
        /// App name or ID
        app: String,

        /// Generate (or regenerate) a new Ed25519 deploy key
        #[arg(long)]
        generate: bool,
    },

    /// Manage webhook URL and auto-deploy settings for an app
    GitWebhook {
        /// App name or ID
        app: String,

        /// Enable auto-deploy on webhook push
        #[arg(long, conflicts_with = "disable")]
        enable: bool,

        /// Disable auto-deploy (webhooks will still return 200 but won't deploy)
        #[arg(long, conflicts_with = "enable")]
        disable: bool,

        /// Branch to watch (only deploy pushes to this branch)
        #[arg(long)]
        branch: Option<String>,

        /// Replace the webhook secret with your own value (min. 16 characters).
        /// Webhook verification is mandatory and cannot be turned off.
        #[arg(long, conflicts_with = "rotate_secret")]
        secret: Option<String>,

        /// Replace the webhook secret with a newly generated one
        #[arg(long)]
        rotate_secret: bool,

        /// Print the webhook secret
        #[arg(long)]
        show_secret: bool,
    },

    /// Manage persistent volumes for an app
    Volume {
        #[command(subcommand)]
        action: VolumeAction,
    },

    /// Manage preview deployments for an app
    #[command(visible_alias = "preview")]
    Previews {
        #[command(subcommand)]
        action: PreviewAction,
    },
}

#[derive(Subcommand)]
pub enum VolumeAction {
    /// Add a persistent volume mount to an app
    #[command(visible_alias = "create")]
    Add {
        /// App name or ID
        app: String,

        /// Absolute path inside the container (e.g. /data)
        container_path: String,

        /// Directory name under the app's volume folder (default: derived from the path)
        #[arg(long)]
        name: Option<String>,
    },

    /// List volumes for an app
    #[command(visible_alias = "ls")]
    List {
        /// App name or ID
        app: String,
    },

    /// Remove a persistent volume mount from an app (asks first; --yes to skip)
    #[command(visible_aliases = ["delete", "rm"])]
    Remove {
        /// App name or ID
        app: String,

        /// Absolute container path to remove (e.g. /data)
        container_path: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum PreviewAction {
    /// Create a new preview deployment
    Create {
        /// App name or ID
        app: String,

        /// TTL in hours (1–168, default 24)
        #[arg(long, default_value = "24")]
        ttl: i64,
    },

    /// List active preview deployments for an app
    #[command(visible_alias = "ls")]
    List {
        /// App name or ID
        app: String,
    },

    /// Delete a specific preview deployment (asks first; --yes to skip)
    #[command(visible_alias = "rm")]
    Delete {
        /// App name or ID
        app: String,

        /// Preview deployment ID
        preview_id: String,

        /// Don't ask for confirmation (required when stdin is not a terminal)
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// After a deploy or upload was queued: follow it (`--wait`, the default) or
/// report the queued deployment (`--no-wait`).
async fn wait_or_report(
    client: &TondClient,
    out: &Output,
    app: &str,
    queued: Value,
    wait: WaitArgs,
) -> Result<()> {
    let deployment_id = queued
        .get("deployment_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    match deployment_id {
        Some(id) if wait.should_wait() => {
            out.event(&json!({
                "event": "queued",
                "app": app,
                "deployment_id": id,
                "status": queued.get("status").cloned().unwrap_or_else(|| json!("queued")),
                "queue_position": queued.get("queue_position").cloned().unwrap_or(Value::Null),
            }))?;
            let outcome = deploy_follow::follow(client, out, app, &id).await?;
            deploy_follow::finish(out, app, &id, outcome)
        }
        _ => {
            let mut doc = match queued {
                Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };
            doc.entry("app").or_insert_with(|| json!(app));
            doc.entry("deployment_id").or_insert(Value::Null);
            doc.entry("status").or_insert_with(|| json!("queued"));
            out.json(&doc)?;
            match deployment_id {
                Some(id) => outln!(
                    out,
                    "Deployment {id} queued. Follow it with: tond apps deployments {app}"
                ),
                None => outln!(
                    out,
                    "Deployment queued. Follow it with: tond apps deployments {app}"
                ),
            }
            Ok(())
        }
    }
}

/// Render one frame of the live log stream.
fn render_log_frame(out: &Output, text: &str) -> Result<()> {
    let Ok(val) = serde_json::from_str::<Value>(text) else {
        outln!(out, "{text}");
        return out.event(&json!({"type": "raw", "line": text}));
    };
    out.event(&val)?;
    let field = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match val.get("type").and_then(Value::as_str) {
        Some("snapshot") => {
            if let Some(arr) = val.get("deploy_logs").and_then(Value::as_array)
                && !arr.is_empty()
            {
                outln!(out, "=== Deploy Logs (snapshot) ===");
                for log in arr {
                    let level = log.get("level").and_then(Value::as_str).unwrap_or("info");
                    outln!(
                        out,
                        "[{}] {} {}",
                        level.to_uppercase(),
                        field(log, "ts"),
                        field(log, "message")
                    );
                }
            }
            if let Some(arr) = val.get("container_logs").and_then(Value::as_array)
                && !arr.is_empty()
            {
                outln!(out, "=== Container Logs (snapshot) ===");
                for line in arr.iter().filter_map(Value::as_str) {
                    out!(out, "{line}");
                }
            }
            outln!(out, "=== Live stream ===");
        }
        Some("deploy") => {
            let level = val.get("level").and_then(Value::as_str).unwrap_or("info");
            outln!(
                out,
                "[DEPLOY][{}] {} {}",
                level.to_uppercase(),
                field(&val, "ts"),
                field(&val, "message")
            );
        }
        Some("container") => out!(out, "{}", field(&val, "line")),
        _ => outln!(out, "{text}"),
    }
    Ok(())
}

pub async fn handle(client: &TondClient, out: &Output, action: AppsAction) -> Result<()> {
    match action {
        AppsAction::List => {
            let apps = client.list_apps().await?;
            out.json(&apps)?;
            if apps.is_empty() {
                outln!(
                    out,
                    "No apps deployed yet. Create one with: tond apps create <name>"
                );
                return Ok(());
            }
            outln!(
                out,
                "{:<20} {:<12} {:<10} {:<30}",
                "NAME",
                "STATUS",
                "PORT",
                "DOMAIN"
            );
            outln!(out, "{}", "-".repeat(72));
            for app in apps {
                outln!(
                    out,
                    "{:<20} {:<12} {:<10} {:<30}",
                    app.name,
                    app.status,
                    app.container_port,
                    app.domain.unwrap_or_else(|| "-".to_string()),
                );
            }
        }

        AppsAction::Create(create) => {
            let CreateAppOptions {
                name,
                template,
                source_type,
                source,
                branch,
                port,
                domain,
                env,
                memory_mb,
                cpus,
                health_path,
                health_interval,
                health_threshold,
                drain_timeout,
                build_method,
                yes,
            } = *create;
            let memory_limit = memory_mb.map(|m| m * 1_048_576);
            let cpu_limit = cpus.map(|c| (c * 1_000_000_000.0) as i64);
            let template_manifest = template.as_deref().map(resolve_template).transpose()?;
            let resolved_source_type = template_manifest
                .as_ref()
                .map(|template| template.source.kind.clone())
                .unwrap_or(source_type);
            let resolved_source = template_manifest
                .as_ref()
                .map(template_source_url)
                .unwrap_or(source);
            let resolved_branch = template_manifest
                .as_ref()
                .and_then(template_branch)
                .or(Some(branch));
            let resolved_port = template_manifest
                .as_ref()
                .map(|template| i64::from(template.deploy.default_port))
                .unwrap_or(port);
            let resolved_health_path = template_manifest
                .as_ref()
                .and_then(|template| template.deploy.health_check_path.clone())
                .or(health_path);
            let resolved_build_method = template_manifest
                .as_ref()
                .and_then(template_build_method)
                .or(build_method);
            let created_project = if let Some(template) = &template_manifest {
                create_template_project(client, template).await?
            } else {
                None
            };
            let created_dependencies = if let Some(template) = &template_manifest {
                create_template_dependencies(client, template, created_project.as_ref()).await?
            } else {
                Vec::new()
            };
            let resolved_env = template_manifest
                .as_ref()
                .map(|template| resolve_template_env(template, &env, yes, &created_dependencies))
                .transpose()?;
            let app = client
                .create_app(
                    &name,
                    &resolved_source_type,
                    resolved_source,
                    resolved_branch,
                    resolved_port,
                    resolved_env,
                    created_project.as_ref().map(|project| project.id.clone()),
                    domain,
                    memory_limit,
                    cpu_limit,
                    resolved_health_path,
                    health_interval,
                    health_threshold,
                    drain_timeout,
                    resolved_build_method,
                )
                .await?;
            out.json(&app)?;
            outln!(out, "App created: {} (ID: {})", app.name, app.id);
            outln!(out, "Deploy with: tond apps deploy {}", app.name);
            if let Some(template) = &template_manifest {
                print_template_follow_up(out, template);
            }
        }

        AppsAction::Deploy { app, wait } => {
            outln!(out, "Deploying {}...", app);
            let queued = client.deploy_app(&app).await?;
            wait_or_report(client, out, &app, queued, wait).await?;
        }

        AppsAction::Push { app, wait } => {
            outln!(out, "Packaging current directory...");
            let queued = client.push_deploy(&app, ".").await?;
            outln!(out, "Upload complete, building & deploying...");
            wait_or_report(client, out, &app, queued, wait).await?;
        }

        AppsAction::Info { app } => {
            let info = client.get_app(&app).await?;
            out.json(&info)?;
            outln!(out, "Name:    {}", info.name);
            outln!(out, "ID:      {}", info.id);
            outln!(out, "Status:  {}", info.status);
            outln!(out, "Source:  {}", info.source_type);
            if let Some(url) = &info.source_url {
                outln!(out, "URL:     {}", url);
            }
            outln!(out, "Port:    {}", info.container_port);
            match info.host_port {
                Some(hp) => outln!(out, "HostPort:{}", hp),
                None => outln!(out, "HostPort:-"),
            }
            outln!(out, "Domain:  {}", info.domain.as_deref().unwrap_or("-"));
            match info.memory_limit {
                Some(bytes) => outln!(out, "Memory:  {} bytes ({} MiB)", bytes, bytes / 1_048_576),
                None => outln!(out, "Memory:  default"),
            }
            match info.cpu_limit {
                Some(nano) => outln!(
                    out,
                    "CPU:     {} nano_cpus ({:.2} cores)",
                    nano,
                    nano as f64 / 1_000_000_000.0
                ),
                None => outln!(out, "CPU:     default"),
            }
            if let Some(ref path) = info.health_check_path {
                outln!(
                    out,
                    "Health:  {} (every {}s, threshold {})",
                    path,
                    info.health_check_interval,
                    info.health_failure_threshold
                );
                outln!(out, "Failures:{}", info.health_consecutive_failures);
            } else {
                outln!(out, "Health:  disabled");
            }
            if info.volumes.is_empty() {
                outln!(out, "Volumes: none");
            } else {
                outln!(out, "Volumes:");
                for vol in &info.volumes {
                    outln!(out, "  {} -> {}", vol.host_path, vol.container_path);
                }
            }
            outln!(out, "Drain:   {}s", info.drain_timeout);
            outln!(out, "Created: {}", info.created_at);
        }

        AppsAction::Logs { app, follow } => {
            if follow {
                outln!(out, "Streaming logs for {} (Ctrl-C to stop)...", app);
                let mut stream = client.log_stream(&app).await?;
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(Message::Text(text)) => render_log_frame(out, &text)?,
                        Ok(Message::Close(frame)) => {
                            if let Some(err) = frame
                                .as_ref()
                                .and_then(|f| CliError::from_close(u16::from(f.code), &f.reason))
                            {
                                return Err(err.into());
                            }
                            break;
                        }
                        Ok(_) => {} // ignore binary/ping/pong
                        Err(e) => {
                            return Err(CliError::unreachable(format!(
                                "The log stream broke: {e}"
                            ))
                            .into());
                        }
                    }
                }
            } else {
                let logs = client.get_logs(&app).await?;
                out.json(&logs)?;

                if let Some(arr) = logs.get("deploy_logs").and_then(Value::as_array)
                    && !arr.is_empty()
                {
                    outln!(out, "=== Deploy Logs ===");
                    for log in arr {
                        let level = log.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                        let msg = log.get("message").and_then(|v| v.as_str()).unwrap_or("");
                        let time = log.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
                        outln!(out, "[{}] {} {}", level.to_uppercase(), time, msg);
                    }
                }

                if let Some(arr) = logs.get("container_logs").and_then(Value::as_array)
                    && !arr.is_empty()
                {
                    outln!(out, "\n=== Container Logs ===");
                    for s in arr.iter().filter_map(Value::as_str) {
                        out!(out, "{}", s);
                    }
                }
            }
        }

        AppsAction::Stop { app } => {
            let data = client.action(&app, "stop").await?;
            out.done("stop", "app", &app, data)?;
            outln!(out, "App {} stopped", app);
        }

        AppsAction::Start { app } => {
            let data = client.action(&app, "start").await?;
            out.done("start", "app", &app, data)?;
            outln!(out, "App {} started", app);
        }

        AppsAction::Restart { app } => {
            let data = client.action(&app, "restart").await?;
            out.done("restart", "app", &app, data)?;
            outln!(out, "App {} restarted", app);
        }

        AppsAction::Delete { app, yes } => {
            out.confirm(
                yes,
                &format!("Delete app {app}? Its containers, image and volumes are removed. This cannot be undone."),
                &format!("delete app {app}"),
            )?;
            let data = client.delete_app(&app).await?;
            out.done("delete", "app", &app, data)?;
            outln!(out, "App {} deleted", app);
        }

        AppsAction::Env { app, vars } => {
            let mut env = serde_json::Map::new();
            for var in &vars {
                let Some((key, value)) = var.split_once('=') else {
                    return Err(CliError::usage(format!(
                        "Invalid env var format: {var} (expected KEY=VALUE)"
                    ))
                    .into());
                };
                env.insert(key.to_string(), Value::String(value.to_string()));
            }
            let data = client.update_env(&app, Value::Object(env)).await?;
            out.done("set_env", "app", &app, data)?;
            outln!(out, "Environment variables updated for {}", app);
            outln!(out, "Redeploy to apply changes: tond apps deploy {}", app);
        }

        AppsAction::Domain { app, domain } => {
            let data = client.update_domain(&app, &domain).await?;
            out.done("set_domain", "app", &app, data)?;
            outln!(out, "Domain set to {} for app {}", domain, app);
        }

        AppsAction::Health {
            app,
            path,
            interval,
            threshold,
            disable,
        } => {
            let health_path = if disable { None } else { path };
            let data = client
                .update_health(&app, health_path.as_deref(), interval, threshold)
                .await?;
            out.done("set_health", "app", &app, data)?;
            if disable {
                outln!(out, "Health checks disabled for app {}", app);
            } else {
                outln!(out, "Health checks configured for app {}", app);
            }
        }

        AppsAction::DrainTimeout { app, seconds } => {
            let data = client.update_drain_timeout(&app, seconds).await?;
            out.done("set_drain_timeout", "app", &app, data)?;
            if seconds == 0 {
                outln!(out, "Graceful drain disabled for app {}", app);
            } else {
                outln!(out, "Drain timeout set to {}s for app {}", seconds, app);
            }
        }

        AppsAction::Deployments { app } => {
            let deployments = client.list_deployments(&app).await?;
            out.json(&deployments)?;
            if deployments.is_empty() {
                outln!(out, "No deployments found for {}", app);
            } else {
                outln!(
                    out,
                    "{:<12} {:<12} {:<30} {:<25}",
                    "STATUS",
                    "IMAGE",
                    "STARTED",
                    "FINISHED"
                );
                outln!(out, "{}", "-".repeat(79));
                for d in &deployments {
                    outln!(
                        out,
                        "{:<12} {:<12} {:<30} {:<25}",
                        d.status,
                        d.image_tag.as_deref().unwrap_or("-"),
                        d.started_at,
                        d.finished_at.as_deref().unwrap_or("-"),
                    );
                }
            }
        }

        AppsAction::Rollback { app, yes } => {
            out.confirm(
                yes,
                &format!("Roll {app} back to its previous deployment?"),
                &format!("roll back app {app}"),
            )?;
            outln!(out, "Rolling back {}...", app);
            let data = client.rollback_app(&app).await?;
            out.done("rollback", "app", &app, data)?;
            outln!(
                out,
                "Rollback started! Check status with: tond apps info {}",
                app
            );
        }

        AppsAction::GitKey { app, generate } => {
            let key = if generate {
                client.generate_deploy_key(&app).await?
            } else {
                match client.get_deploy_key(&app).await {
                    Ok(key) => key,
                    Err(e) => {
                        let mut e = error::describe(&e);
                        e.message = format!("No deploy key found for {app}: {}", e.message);
                        e.hint = Some(format!(
                            "Generate one with: tond apps git-key {app} --generate"
                        ));
                        return Err(e.into());
                    }
                }
            };
            out.json(&json!({ "app": app, "public_key": key }))?;
            if generate {
                outln!(out, "Deploy key generated for {}:", app);
            } else {
                outln!(out, "Deploy key for {}:", app);
            }
            outln!(out);
            outln!(out, "{}", key);
            outln!(out);
            outln!(
                out,
                "Add this public key to your repository's deploy keys (read-only)."
            );
        }

        AppsAction::GitWebhook {
            app,
            enable,
            disable,
            branch,
            secret,
            rotate_secret,
            show_secret,
        } => {
            if let Some(ref s) = secret {
                client.set_webhook_secret(&app, s).await?;
                outln!(out, "Webhook secret set for {}", app);
            }
            let mut shown_secret: Option<String> = None;
            if rotate_secret {
                shown_secret = Some(client.rotate_webhook_secret(&app).await?.secret);
                outln!(
                    out,
                    "Webhook secret rotated for {} — update it in your git provider",
                    app
                );
            }

            // Apply auto-deploy toggle
            if enable || disable || branch.is_some() {
                let auto_deploy = if disable { false } else { enable };
                client
                    .set_auto_deploy(&app, auto_deploy, branch.as_deref())
                    .await?;
                if disable {
                    outln!(out, "Auto-deploy disabled for {}", app);
                } else if enable {
                    outln!(
                        out,
                        "Auto-deploy enabled for {} (branch: {})",
                        app,
                        branch.as_deref().unwrap_or("any")
                    );
                } else {
                    outln!(
                        out,
                        "Watch branch set to {} for {}",
                        branch.as_deref().unwrap_or("any"),
                        app
                    );
                }
            }

            // Always print the webhook URLs
            let info = client.get_app(&app).await?;
            let config = client.get_webhook_config(&info.id).await?;
            let urls = config.urls.clone().unwrap_or(config.paths.clone());
            if show_secret && shown_secret.is_none() {
                shown_secret = Some(client.reveal_webhook_secret(&info.id).await?.secret);
            }
            out.json(&json!({
                "app": info.name,
                "urls": urls,
                "absolute_urls": config.urls.is_some(),
                "secret": shown_secret,
                "auto_deploy": info.auto_deploy,
                "watch_branch": info.watch_branch,
            }))?;
            outln!(out);
            outln!(out, "Webhook URLs for {}:", info.name);
            outln!(out, "  GitHub:        {}", urls.github);
            outln!(out, "  GitLab:        {}", urls.gitlab);
            outln!(out, "  Gitea/Forgejo: {}", urls.gitea);
            outln!(out, "  Generic:       {}", urls.generic);
            if config.urls.is_none() {
                outln!(
                    out,
                    "  (set dashboard_domain in Settings to get absolute URLs)"
                );
            }
            outln!(out);
            match shown_secret {
                Some(secret) => outln!(out, "Webhook secret: {secret}"),
                None => outln!(out, "Webhook secret: hidden (use --show-secret)"),
            }
            outln!(out);
            outln!(
                out,
                "Auto-deploy: {}",
                if info.auto_deploy {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            outln!(
                out,
                "Watch branch: {}",
                info.watch_branch.as_deref().unwrap_or("any")
            );
        }

        AppsAction::Volume { action } => match action {
            VolumeAction::Add {
                app,
                container_path,
                name,
            } => {
                let data = client
                    .add_volume(&app, &container_path, name.as_deref())
                    .await?;
                out.done("add_volume", "app", &app, data)?;
                outln!(
                    out,
                    "Volume {} added to {}. Redeploy to apply: tond apps deploy {}",
                    container_path,
                    app,
                    app
                );
            }
            VolumeAction::List { app } => {
                let info = client.get_app(&app).await?;
                out.json(&info.volumes)?;
                if info.volumes.is_empty() {
                    outln!(out, "No volumes configured for {}", app);
                } else {
                    outln!(out, "{:<30} HOST PATH", "CONTAINER PATH");
                    outln!(out, "{}", "-".repeat(60));
                    for vol in &info.volumes {
                        outln!(out, "{:<30} {}", vol.container_path, vol.host_path);
                    }
                }
            }
            VolumeAction::Remove {
                app,
                container_path,
                yes,
            } => {
                out.confirm(
                    yes,
                    &format!(
                        "Remove volume {container_path} from {app}? The app no longer mounts it after the next deploy."
                    ),
                    &format!("remove volume {container_path} from app {app}"),
                )?;
                let data = client.remove_volume(&app, &container_path).await?;
                out.done("remove_volume", "app", &app, data)?;
                outln!(
                    out,
                    "Volume {} removed from {}. Redeploy to apply: tond apps deploy {}",
                    container_path,
                    app,
                    app
                );
            }
        },

        AppsAction::Previews { action } => match action {
            PreviewAction::Create { app, ttl } => {
                outln!(out, "Creating preview deployment for {}...", app);
                let p = client.create_preview(&app, Some(ttl)).await?;
                out.json(&p)?;
                outln!(out, "Preview deployment started (ID: {})", p.id);
                outln!(out, "Status:  {}", p.status);
                match &p.url {
                    Some(url) => outln!(out, "URL:     https://{}", url),
                    None => outln!(out, "URL:     (no domain configured)"),
                }
                outln!(out, "Expires: {}", p.expires_at);
            }
            PreviewAction::List { app } => {
                let previews = client.list_previews(&app).await?;
                out.json(&previews)?;
                if previews.is_empty() {
                    outln!(out, "No active preview deployments for {}", app);
                } else {
                    outln!(
                        out,
                        "{:<28} {:<12} {:<40} {:<30}",
                        "ID",
                        "STATUS",
                        "URL",
                        "EXPIRES"
                    );
                    outln!(out, "{}", "-".repeat(110));
                    for p in &previews {
                        outln!(
                            out,
                            "{:<28} {:<12} {:<40} {:<30}",
                            p.id,
                            p.status,
                            p.url.as_deref().unwrap_or("-"),
                            p.expires_at,
                        );
                    }
                }
            }
            PreviewAction::Delete {
                app,
                preview_id,
                yes,
            } => {
                out.confirm(
                    yes,
                    &format!("Delete preview {preview_id} of {app}? Its container is stopped."),
                    &format!("delete preview {preview_id}"),
                )?;
                let data = client.delete_preview(&app, &preview_id).await?;
                out.done("delete", "preview", &preview_id, data)?;
                outln!(out, "Preview {} deleted", preview_id);
            }
        },
    }

    Ok(())
}

fn resolve_template(template_id: &str) -> Result<TemplateManifest> {
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("templates");
    let manifests = load_template_manifests(&template_dir)?;

    manifests
        .into_iter()
        .map(|(_, manifest)| manifest)
        .find(|manifest| manifest.id == template_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown template '{template_id}'"))
}

fn template_source_url(template: &TemplateManifest) -> Option<String> {
    Some(template.source.url.clone())
}

fn template_branch(template: &TemplateManifest) -> Option<String> {
    (template.source.kind == "git")
        .then(|| template.source.r#ref.clone())
        .flatten()
}

fn template_build_method(template: &TemplateManifest) -> Option<String> {
    match template.deploy.method.as_str() {
        "tbuild" | "dockerfile" => Some(template.deploy.method.clone()),
        _ => None,
    }
}

fn resolve_template_env(
    template: &TemplateManifest,
    env_pairs: &[String],
    yes: bool,
    dependencies: &[CreatedTemplateDependency],
) -> Result<serde_json::Map<String, Value>> {
    let mut env = serde_json::Map::new();

    for pair in env_pairs {
        let (key, value) = parse_env_pair(pair)?;
        env.insert(key, Value::String(value));
    }

    for required in &template.required_env {
        if env.contains_key(&required.key) {
            continue;
        }

        if let Some(value) = &required.value {
            let resolved = interpolate_template_value(value, dependencies)?;
            env.insert(required.key.clone(), Value::String(resolved));
            continue;
        }

        if yes {
            anyhow::bail!(
                "Template '{}' requires env var '{}' and --yes disables prompting",
                template.id,
                required.key
            );
        }

        let value = crate::commands::settings::read_line_prompt(&format!(
            "{} ({}): ",
            required.key, required.description
        ))?;
        if value.trim().is_empty() {
            anyhow::bail!("Template env var '{}' cannot be empty", required.key);
        }
        env.insert(required.key.clone(), Value::String(value));
    }

    Ok(env)
}

#[derive(Debug)]
struct CreatedTemplateDependency {
    name: String,
    /// Returned once when the database is created
    credentials: DatabaseCredentialsResponse,
}

async fn create_template_project(
    client: &TondClient,
    template: &TemplateManifest,
) -> Result<Option<tond_common::ProjectResponse>> {
    let Some(project) = &template.project else {
        return Ok(None);
    };

    let created = client
        .create_project(&project.name, project.description.as_deref())
        .await?;
    Ok(Some(created))
}

async fn create_template_dependencies(
    client: &TondClient,
    template: &TemplateManifest,
    project: Option<&tond_common::ProjectResponse>,
) -> Result<Vec<CreatedTemplateDependency>> {
    let mut created = Vec::new();

    for dependency in &template.dependencies {
        if dependency.kind != "database" {
            continue;
        }

        let name = dependency
            .name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("database template dependencies require a name"))?;
        let engine = dependency
            .engine
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("database template dependencies require an engine"))?;

        let created_db = client
            .create_database(
                &name,
                engine,
                project.map(|project| project.id.as_str()),
                None,
            )
            .await?;
        let credentials = created_db.credentials.ok_or_else(|| {
            anyhow::anyhow!(
                "created database '{name}', but this token may not read database credentials \
                 (db:credentials), which the template needs"
            )
        })?;
        created.push(CreatedTemplateDependency { name, credentials });
    }

    Ok(created)
}

fn interpolate_template_value(
    value: &str,
    dependencies: &[CreatedTemplateDependency],
) -> Result<String> {
    let Some(reference) = value
        .strip_prefix("${db.")
        .and_then(|value| value.strip_suffix('}'))
    else {
        return Ok(value.to_string());
    };

    let (dependency_name, field) = reference.split_once('.').ok_or_else(|| {
        anyhow::anyhow!("template dependency interpolation must use ${{db.<name>.<field>}} syntax")
    })?;
    let dependency = dependencies
        .iter()
        .find(|dependency| dependency.name == dependency_name)
        .ok_or_else(|| anyhow::anyhow!("unknown template dependency '{dependency_name}'"))?;

    match field {
        "connection_string" => dependency
            .credentials
            .connection_string
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!("dependency '{dependency_name}' has no connection string")
            }),
        "internal_connection_string" => dependency
            .credentials
            .internal_connection_string
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!("dependency '{dependency_name}' has no internal connection string")
            }),
        _ => Err(anyhow::anyhow!(
            "unsupported template dependency field '{field}'"
        )),
    }
}

fn parse_env_pair(pair: &str) -> Result<(String, String)> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("env values must use KEY=VALUE format"))?;

    if key.trim().is_empty() {
        anyhow::bail!("env values must use a non-empty KEY=VALUE format");
    }

    Ok((key.trim().to_string(), value.to_string()))
}

fn print_template_follow_up(out: &Output, template: &TemplateManifest) {
    if let Some(notes) = &template.setup_notes {
        outln!(out, "Template notes: {notes}");
    }

    if !template.post_deploy_checks.is_empty() {
        outln!(out, "Post-deploy checks:");
        for check in &template.post_deploy_checks {
            outln!(out, "- {check}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
    use serde_json::json;
    use tokio::{net::TcpListener, sync::Mutex};

    use super::*;
    use crate::client::TondClient;

    #[derive(Clone, Default)]
    struct TemplateFlowCaptureState {
        requests: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    }

    async fn spawn_test_server(router: Router) -> std::net::SocketAddr {
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

    fn sample_template() -> TemplateManifest {
        TemplateManifest::from_toml_str(
            std::path::Path::new("template.toml"),
            r#"
id = "plausible"
name = "Plausible"
description = "Analytics"
category = "analytics"
setup_notes = "Configure your site before sharing the dashboard."
post_deploy_checks = ["Open /", "Add a site domain"]

[source]
kind = "image"
url = "ghcr.io/plausible/community-edition"

[deploy]
method = "image"
default_port = 8000

[[required_env]]
key = "BASE_URL"
description = "Public URL"
required = true

[[required_env]]
key = "SECRET_KEY_BASE"
description = "Secret key"
required = true
"#,
        )
        .unwrap_or_else(|err| panic!("failed to build sample template: {err}"))
    }

    #[test]
    fn resolve_template_env_uses_provided_values() {
        let env = resolve_template_env(
            &sample_template(),
            &[
                "BASE_URL=https://stats.example.com".to_string(),
                "SECRET_KEY_BASE=abc123".to_string(),
            ],
            true,
            &[],
        )
        .unwrap_or_else(|err| panic!("expected provided env vars to satisfy template: {err}"));

        assert_eq!(
            env,
            serde_json::Map::from_iter([
                ("BASE_URL".to_string(), json!("https://stats.example.com"),),
                ("SECRET_KEY_BASE".to_string(), json!("abc123")),
            ]),
            "template env resolution should preserve provided required values"
        );
    }

    #[test]
    fn resolve_template_env_rejects_missing_required_values_in_yes_mode() {
        let err = resolve_template_env(
            &sample_template(),
            &["BASE_URL=https://stats.example.com".to_string()],
            true,
            &[],
        )
        .expect_err("--yes should reject missing required template env vars");

        assert!(
            err.to_string()
                .contains("requires env var 'SECRET_KEY_BASE' and --yes disables prompting"),
            "missing required envs should produce a clear non-interactive error, got: {err}"
        );
    }

    #[test]
    fn parse_env_pair_rejects_invalid_input() {
        let err = parse_env_pair("BROKEN")
            .expect_err("invalid env pairs should be rejected before app creation");

        assert_eq!(
            err.to_string(),
            "env values must use KEY=VALUE format",
            "invalid env pair errors should explain the expected syntax"
        );
    }

    #[test]
    fn interpolate_template_value_resolves_database_connection_strings() {
        let value = interpolate_template_value(
            "${db.analytics-db.internal_connection_string}",
            &[CreatedTemplateDependency {
                name: "analytics-db".to_string(),
                credentials: DatabaseCredentialsResponse {
                    db_user: Some("tond_analytics".to_string()),
                    db_password: Some("secret".to_string()),
                    root_password: None,
                    public_connection_string: None,
                    connection_string: Some(
                        "postgres://tond_analytics:secret@localhost:5432/tond_analytics"
                            .to_string(),
                    ),
                    internal_connection_string: Some(
                        "postgres://tond_analytics:secret@analytics-db:5432/tond_analytics"
                            .to_string(),
                    ),
                },
            }],
        )
        .unwrap_or_else(|err| panic!("expected dependency interpolation to succeed: {err}"));

        assert_eq!(
            value, "postgres://tond_analytics:secret@analytics-db:5432/tond_analytics",
            "dependency interpolation should expose the requested created database field"
        );
    }

    #[tokio::test]
    async fn create_template_flow_creates_project_database_and_app_with_interpolated_env() {
        let capture = TemplateFlowCaptureState::default();
        let router = Router::new()
            .route(
                "/api/v1/projects",
                post(
                    |State(state): State<TemplateFlowCaptureState>, body: Bytes| async move {
                        let request: serde_json::Value = serde_json::from_slice(&body)
                            .unwrap_or_else(|err| panic!("failed to decode project request body: {err}"));
                        state
                            .requests
                            .lock()
                            .await
                            .push(("project".to_string(), request));

                        (
                            StatusCode::OK,
                            axum::Json(json!({
                                "success": true,
                                "data": {
                                    "id": "project-1",
                                    "name": "plausible",
                                    "description": "Plausible app and supporting services",
                                    "env_vars": {},
                                    "created_at": "2026-01-01T00:00:00Z",
                                    "updated_at": "2026-01-01T00:00:00Z",
                                    "active_preview_count": 0
                                },
                                "error": null
                            })),
                        )
                    },
                ),
            )
            .route(
                "/api/v1/databases",
                post(
                    |State(state): State<TemplateFlowCaptureState>, body: Bytes| async move {
                        let request: serde_json::Value = serde_json::from_slice(&body)
                            .unwrap_or_else(|err| panic!("failed to decode database request body: {err}"));
                        state
                            .requests
                            .lock()
                            .await
                            .push(("database".to_string(), request));

                        (
                            StatusCode::OK,
                            axum::Json(json!({
                                "success": true,
                                "data": {
                                    "id": "db-1",
                                    "name": "plausible-db",
                                    "db_type": "postgres",
                                    "project_id": "project-1",
                                    "host_port": 5432,
                                    "status": "running",
                                    "db_name": "tond_plausible",
                                    "db_user": "tond_plausible",
                                    "has_password": true,
                                    "memory_limit": null,
                                    "custom_external_url": null,
                                    "custom_internal_url": null,
                                    "image_used": null,
                                    "custom_image": null,
                                    "created_at": "2026-01-01T00:00:00Z",
                                    "updated_at": "2026-01-01T00:00:00Z",
                                    "connection_string": "postgres://tond_plausible:••••••••@localhost:5432/tond_plausible",
                                    "internal_connection_string": "postgres://tond_plausible:••••••••@plausible-db:5432/tond_plausible",
                                    "credentials": {
                                        "db_user": "tond_plausible",
                                        "db_password": "secret",
                                        "root_password": null,
                                        "connection_string": "postgres://tond_plausible:secret@localhost:5432/tond_plausible",
                                        "public_connection_string": null,
                                        "internal_connection_string": "postgres://tond_plausible:secret@plausible-db:5432/tond_plausible"
                                    }
                                },
                                "error": null
                            })),
                        )
                    },
                ),
            )
            .route(
                "/api/v1/apps",
                post(
                    |State(state): State<TemplateFlowCaptureState>, body: Bytes| async move {
                        let request: serde_json::Value = serde_json::from_slice(&body)
                            .unwrap_or_else(|err| panic!("failed to decode app request body: {err}"));
                        state
                            .requests
                            .lock()
                            .await
                            .push(("app".to_string(), request));

                        (
                            StatusCode::OK,
                            axum::Json(json!({
                                "success": true,
                                "data": {
                                    "id": "app-1",
                                    "name": "plausible-app",
                                    "domain": null,
                                    "source_type": "image",
                                    "source_url": "ghcr.io/plausible/community-edition",
                                    "status": "pending",
                                    "container_port": 8000,
                                    "host_port": null,
                                    "memory_limit": null,
                                    "cpu_limit": null,
                                    "health_check_path": "/",
                                    "health_check_interval": 30,
                                    "health_failure_threshold": 3,
                                    "health_consecutive_failures": 0,
                                    "volumes": [],
                                    "drain_timeout": 30,
                                    "build_method": null,
                                    "auto_deploy": false,
                                    "watch_branch": null,
                                    "deploy_key_pub": null,
                                    "project_id": "project-1",
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
                                },
                                "error": null
                            })),
                        )
                    },
                ),
            )
            .with_state(capture.clone());
        let addr = spawn_test_server(router).await;
        let client = TondClient::new(&format!("http://{}", addr), None);

        let (out, _, _) = Output::capture(false, false, "");
        handle(
            &client,
            &out,
            AppsAction::Create(Box::new(CreateAppOptions {
                name: "plausible-app".to_string(),
                template: Some("plausible".to_string()),
                source_type: "git".to_string(),
                source: None,
                branch: "main".to_string(),
                port: 3000,
                domain: None,
                env: vec![
                    "BASE_URL=https://stats.example.com".to_string(),
                    "SECRET_KEY_BASE=super-secret".to_string(),
                ],
                memory_mb: None,
                cpus: None,
                health_path: None,
                health_interval: None,
                health_threshold: None,
                drain_timeout: None,
                build_method: None,
                yes: true,
            })),
        )
        .await
        .unwrap_or_else(|err| panic!("expected templated app create command to succeed: {err}"));

        let requests = capture.requests.lock().await.clone();
        assert_eq!(
            requests.len(),
            3,
            "template create flow should issue project, database, and app requests"
        );
        assert_eq!(
            requests[0].0, "project",
            "template create flow should create the project before dependencies"
        );
        assert_eq!(
            requests[1].0, "database",
            "template create flow should create dependencies before the app"
        );
        assert_eq!(
            requests[2].0, "app",
            "template create flow should create the app last after dependency interpolation"
        );
        assert_eq!(
            requests[1].1["project_id"],
            serde_json::Value::String("project-1".to_string()),
            "template dependency creation should attach managed databases to the created project"
        );
        assert_eq!(
            requests[2].1["project_id"],
            serde_json::Value::String("project-1".to_string()),
            "templated app creation should attach the app to the created project"
        );
        assert_eq!(
            requests[2].1["env_vars"]["DATABASE_URL"],
            serde_json::Value::String(
                "postgres://tond_plausible:secret@plausible-db:5432/tond_plausible".to_string(),
            ),
            "templated app creation should interpolate dependency connection strings into env vars"
        );
        assert_eq!(
            requests[2].1["source_type"],
            serde_json::Value::String("image".to_string()),
            "templated app creation should preserve the template source type"
        );
    }
}
