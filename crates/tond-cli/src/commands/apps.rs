// commands/apps.rs — `tond apps` subcommand definitions and handler

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::Value;
use tond_common::{DatabaseCredentialsResponse, TemplateManifest, load_template_manifests};

use crate::client::TondClient;

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
    #[arg(long)]
    pub yes: bool,
}

#[derive(Subcommand)]
pub enum AppsAction {
    /// List all deployed apps
    List,

    /// Create a new app
    Create(Box<CreateAppOptions>),

    /// Deploy an app (build & start from stored source)
    Deploy {
        /// App name or ID
        app: String,
    },

    /// Deploy current directory to an app (tar + upload)
    Push {
        /// App name or ID
        app: String,
    },

    /// Show app details
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

    /// Delete an app
    Delete {
        /// App name or ID
        app: String,

        /// Skip confirmation
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

    /// Roll back an app to its previous deployment
    Rollback {
        /// App name or ID
        app: String,
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
    Previews {
        #[command(subcommand)]
        action: PreviewAction,
    },
}

#[derive(Subcommand)]
pub enum VolumeAction {
    /// Add a persistent volume mount to an app
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
    List {
        /// App name or ID
        app: String,
    },

    /// Remove a persistent volume mount from an app
    Remove {
        /// App name or ID
        app: String,

        /// Absolute container path to remove (e.g. /data)
        container_path: String,
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
    List {
        /// App name or ID
        app: String,
    },

    /// Delete a specific preview deployment
    Delete {
        /// App name or ID
        app: String,

        /// Preview deployment ID
        preview_id: String,
    },
}

pub async fn handle(client: &TondClient, action: AppsAction) -> Result<()> {
    match action {
        AppsAction::List => {
            let apps = client.list_apps().await?;
            if apps.is_empty() {
                println!("No apps deployed yet. Create one with: tond apps create <name>");
                return Ok(());
            }
            println!(
                "{:<20} {:<12} {:<10} {:<30}",
                "NAME", "STATUS", "PORT", "DOMAIN"
            );
            println!("{}", "-".repeat(72));
            for app in apps {
                println!(
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
            println!("App created: {} (ID: {})", app.name, app.id);
            println!("Deploy with: tond apps deploy {}", app.name);
            if let Some(template) = &template_manifest {
                print_template_follow_up(template);
            }
        }

        AppsAction::Deploy { app } => {
            println!("Deploying {}...", app);
            client.deploy_app(&app).await?;
            println!(
                "Deployment started! Check status with: tond apps info {}",
                app
            );
        }

        AppsAction::Push { app } => {
            println!("Packaging current directory...");
            client.push_deploy(&app, ".").await?;
            println!("Upload complete, building & deploying...");
            println!("Check status with: tond apps info {}", app);
        }

        AppsAction::Info { app } => {
            let info = client.get_app(&app).await?;
            println!("Name:    {}", info.name);
            println!("ID:      {}", info.id);
            println!("Status:  {}", info.status);
            println!("Source:  {}", info.source_type);
            if let Some(url) = &info.source_url {
                println!("URL:     {}", url);
            }
            println!("Port:    {}", info.container_port);
            match info.host_port {
                Some(hp) => println!("HostPort:{}", hp),
                None => println!("HostPort:-"),
            }
            println!(
                "Domain:  {}",
                info.domain.unwrap_or_else(|| "-".to_string())
            );
            match info.memory_limit {
                Some(bytes) => println!("Memory:  {} bytes ({} MiB)", bytes, bytes / 1_048_576),
                None => println!("Memory:  default"),
            }
            match info.cpu_limit {
                Some(nano) => println!(
                    "CPU:     {} nano_cpus ({:.2} cores)",
                    nano,
                    nano as f64 / 1_000_000_000.0
                ),
                None => println!("CPU:     default"),
            }
            if let Some(ref path) = info.health_check_path {
                println!(
                    "Health:  {} (every {}s, threshold {})",
                    path, info.health_check_interval, info.health_failure_threshold
                );
                println!("Failures:{}", info.health_consecutive_failures);
            } else {
                println!("Health:  disabled");
            }
            if info.volumes.is_empty() {
                println!("Volumes: none");
            } else {
                println!("Volumes:");
                for vol in &info.volumes {
                    println!("  {} -> {}", vol.host_path, vol.container_path);
                }
            }
            println!("Drain:   {}s", info.drain_timeout);
            println!("Created: {}", info.created_at);
        }

        AppsAction::Logs { app, follow } => {
            if follow {
                println!("Streaming logs for {} (Ctrl-C to stop)...", app);
                client.stream_logs(&app).await?;
            } else {
                let logs = client.get_logs(&app).await?;

                if let Some(deploy_logs) = logs.get("deploy_logs")
                    && let Some(arr) = deploy_logs.as_array()
                    && !arr.is_empty()
                {
                    println!("=== Deploy Logs ===");
                    for log in arr {
                        let level = log.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                        let msg = log.get("message").and_then(|v| v.as_str()).unwrap_or("");
                        let time = log.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
                        println!("[{}] {} {}", level.to_uppercase(), time, msg);
                    }
                }

                if let Some(container_logs) = logs.get("container_logs")
                    && let Some(arr) = container_logs.as_array()
                    && !arr.is_empty()
                {
                    println!("\n=== Container Logs ===");
                    for log in arr {
                        if let Some(s) = log.as_str() {
                            print!("{}", s);
                        }
                    }
                }
            }
        }

        AppsAction::Stop { app } => {
            client.action(&app, "stop").await?;
            println!("App {} stopped", app);
        }

        AppsAction::Start { app } => {
            client.action(&app, "start").await?;
            println!("App {} started", app);
        }

        AppsAction::Restart { app } => {
            client.action(&app, "restart").await?;
            println!("App {} restarted", app);
        }

        AppsAction::Delete { app, yes } => {
            if !yes {
                print!("Are you sure you want to delete {}? [y/N] ", app);
                use std::io::{self, Write};
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() != "y" {
                    println!("Cancelled");
                    return Ok(());
                }
            }
            client.delete_app(&app).await?;
            println!("App {} deleted", app);
        }

        AppsAction::Env { app, vars } => {
            let mut env = serde_json::Map::new();
            for var in &vars {
                if let Some((key, value)) = var.split_once('=') {
                    env.insert(
                        key.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                } else {
                    eprintln!("Invalid env var format: {} (expected KEY=VALUE)", var);
                    return Ok(());
                }
            }
            client
                .update_env(&app, serde_json::Value::Object(env))
                .await?;
            println!("Environment variables updated for {}", app);
            println!("Redeploy to apply changes: tond apps deploy {}", app);
        }

        AppsAction::Domain { app, domain } => {
            client.update_domain(&app, &domain).await?;
            println!("Domain set to {} for app {}", domain, app);
        }

        AppsAction::Health {
            app,
            path,
            interval,
            threshold,
            disable,
        } => {
            let health_path = if disable { None } else { path };
            client
                .update_health(&app, health_path.as_deref(), interval, threshold)
                .await?;
            if disable {
                println!("Health checks disabled for app {}", app);
            } else {
                println!("Health checks configured for app {}", app);
            }
        }

        AppsAction::DrainTimeout { app, seconds } => {
            client.update_drain_timeout(&app, seconds).await?;
            if seconds == 0 {
                println!("Graceful drain disabled for app {}", app);
            } else {
                println!("Drain timeout set to {}s for app {}", seconds, app);
            }
        }

        AppsAction::Deployments { app } => {
            let deployments = client.list_deployments(&app).await?;
            if deployments.is_empty() {
                println!("No deployments found for {}", app);
            } else {
                println!(
                    "{:<12} {:<12} {:<30} {:<25}",
                    "STATUS", "IMAGE", "STARTED", "FINISHED"
                );
                println!("{}", "-".repeat(79));
                for d in &deployments {
                    println!(
                        "{:<12} {:<12} {:<30} {:<25}",
                        d.status,
                        d.image_tag.as_deref().unwrap_or("-"),
                        d.started_at,
                        d.finished_at.as_deref().unwrap_or("-"),
                    );
                }
            }
        }

        AppsAction::Rollback { app } => {
            println!("Rolling back {}...", app);
            client.rollback_app(&app).await?;
            println!(
                "Rollback started! Check status with: tond apps info {}",
                app
            );
        }

        AppsAction::GitKey { app, generate } => {
            if generate {
                let key = client.generate_deploy_key(&app).await?;
                println!("Deploy key generated for {}:", app);
                println!();
                println!("{}", key);
                println!();
                println!("Add this public key to your repository's deploy keys.");
            } else {
                match client.get_deploy_key(&app).await {
                    Ok(key) => {
                        println!("Deploy key for {}:", app);
                        println!();
                        println!("{}", key);
                        println!();
                        println!("Add this key to your repository's deploy keys (read-only).");
                    }
                    Err(e) => {
                        eprintln!("No deploy key found for {}: {}", app, e);
                        eprintln!("Generate one with: tond apps git-key {} --generate", app);
                    }
                }
            }
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
                println!("Webhook secret set for {}", app);
            }
            let mut shown_secret: Option<String> = None;
            if rotate_secret {
                shown_secret = Some(client.rotate_webhook_secret(&app).await?.secret);
                println!(
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
                    println!("Auto-deploy disabled for {}", app);
                } else if enable {
                    println!(
                        "Auto-deploy enabled for {} (branch: {})",
                        app,
                        branch.as_deref().unwrap_or("any")
                    );
                } else {
                    println!(
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
            println!();
            println!("Webhook URLs for {}:", info.name);
            println!("  GitHub:        {}", urls.github);
            println!("  GitLab:        {}", urls.gitlab);
            println!("  Gitea/Forgejo: {}", urls.gitea);
            println!("  Generic:       {}", urls.generic);
            if config.urls.is_none() {
                println!("  (set dashboard_domain in Settings to get absolute URLs)");
            }
            if show_secret && shown_secret.is_none() {
                shown_secret = Some(client.reveal_webhook_secret(&info.id).await?.secret);
            }
            if let Some(secret) = shown_secret {
                println!();
                println!("Webhook secret: {secret}");
            } else {
                println!();
                println!("Webhook secret: hidden (use --show-secret)");
            }
            println!();
            println!(
                "Auto-deploy: {}",
                if info.auto_deploy {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!(
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
                client
                    .add_volume(&app, &container_path, name.as_deref())
                    .await?;
                println!(
                    "Volume {} added to {}. Redeploy to apply: tond apps deploy {}",
                    container_path, app, app
                );
            }
            VolumeAction::List { app } => {
                let info = client.get_app(&app).await?;
                if info.volumes.is_empty() {
                    println!("No volumes configured for {}", app);
                } else {
                    println!("{:<30} HOST PATH", "CONTAINER PATH");
                    println!("{}", "-".repeat(60));
                    for vol in &info.volumes {
                        println!("{:<30} {}", vol.container_path, vol.host_path);
                    }
                }
            }
            VolumeAction::Remove {
                app,
                container_path,
            } => {
                client.remove_volume(&app, &container_path).await?;
                println!(
                    "Volume {} removed from {}. Redeploy to apply: tond apps deploy {}",
                    container_path, app, app
                );
            }
        },

        AppsAction::Previews { action } => match action {
            PreviewAction::Create { app, ttl } => {
                println!("Creating preview deployment for {}...", app);
                let p = client.create_preview(&app, Some(ttl)).await?;
                println!("Preview deployment started (ID: {})", p.id);
                println!("Status:  {}", p.status);
                if let Some(url) = &p.url {
                    println!("URL:     https://{}", url);
                } else {
                    println!("URL:     (no domain configured)");
                }
                println!("Expires: {}", p.expires_at);
            }
            PreviewAction::List { app } => {
                let previews = client.list_previews(&app).await?;
                if previews.is_empty() {
                    println!("No active preview deployments for {}", app);
                } else {
                    println!(
                        "{:<28} {:<12} {:<40} {:<30}",
                        "ID", "STATUS", "URL", "EXPIRES"
                    );
                    println!("{}", "-".repeat(110));
                    for p in &previews {
                        println!(
                            "{:<28} {:<12} {:<40} {:<30}",
                            p.id,
                            p.status,
                            p.url.as_deref().unwrap_or("-"),
                            p.expires_at,
                        );
                    }
                }
            }
            PreviewAction::Delete { app, preview_id } => {
                client.delete_preview(&app, &preview_id).await?;
                println!("Preview {} deleted", preview_id);
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

fn print_template_follow_up(template: &TemplateManifest) {
    if let Some(notes) = &template.setup_notes {
        println!("Template notes: {notes}");
    }

    if !template.post_deploy_checks.is_empty() {
        println!("Post-deploy checks:");
        for check in &template.post_deploy_checks {
            println!("- {check}");
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
                "/api/projects",
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
                "/api/databases",
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
                "/api/apps",
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
                                    "preview_excluded_keys": [],
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

        handle(
            &client,
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
