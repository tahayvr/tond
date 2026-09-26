// deploy/core.rs — deployment orchestration
//
// This module coordinates the full lifecycle of deploying an app: from
// source code to a running Docker container.
//
// Public surface (re-exported from deploy/mod.rs):
//
//   start(state, app, request) -> queue::Enqueued
//     Create the deployment row (status 'queued') and hand it to the build
//     queue (queue.rs: one deploy per app, a newer request supersedes a
//     queued one, builds share the global build slots). Used by the deploy,
//     upload, rollback, webhook and project handlers, which return the id and
//     queue position so the dashboard can open the live deploy view.
//
//   create(state, app, request) + run(state, app, id, request, ctx)
//     The two halves; the queue's runner calls run() with a DeployCtx that
//     holds the build slot during the build phase and moves the row
//     queued -> running.
//
// A deployment walks through phases — clone, build, start, health, release —
// recorded by deploy::progress on the deployment row and published on the log
// bus. A failure is classified into a likely cause with a fix hint
// (deploy::failure) and stored on the row; the app keeps its previous status
// when the old container is still serving.
//
// Image tags: every deployment runs `tond-<app>:<deployment-id-prefix>` (see
// images.rs) — built images are tagged that way, pulled and uploaded images
// are re-tagged — and the tag is stored on the deployment row. Rollback
// re-runs that exact image. After a successful deploy or rollback older
// images are pruned (keeping the last few plus the running one).
//
//   deploy_preview(state, app, preview)
//     Deploy a short-lived preview of an app.
//
// Helpers private to this module:
//   update_status()            — update app.status in DB
//   log_deploy()               — insert timestamped deploy log entry (tied to
//                                the app's active deployment) and publish it on
//                                log_bus for live log streams
//   probe_until_ready()        — poll the new container until it answers
//   fetch_registry_credentials() — look up and decrypt registry credentials
//   pull_image()               — stream a docker pull with bollard

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use tond_common::{App, DeployFailure, DeployPhase, PreviewDeployment, Readiness, Volume};
use uuid::Uuid;

use crate::backup::{self, SnapshotTrigger};
use crate::docker;
use crate::domains;
use crate::handlers::{github_apps, gitlab_tokens};
use crate::log_bus::{self, DeployLogEvent};
use crate::log_scrub;
use crate::networks;
use crate::secret_key::{self, Purpose};
use crate::secrets;
use crate::state::AppState;
use crate::volumes;

use super::build::{
    BuildOptions, build_from_dir_with_method, build_from_git, build_from_inline_dockerfile,
    run_inline_compose,
};
use super::failure::{self, ContainerExit, FailureContext, ProbeResult};
use super::git::{GitHttpAuth, redact_url_credentials, split_url_credentials};
use super::images;
use super::progress;
use super::queue::{self, DeployCtx};
use crate::proxy_link;
use crate::runtime::shutdown;

/// Work out the clone URL and HTTPS credentials for a git app.
///
/// Credentials come from the linked GitHub App (installation token), the
/// linked GitLab PAT, or userinfo embedded in `source_url`; they are returned
/// separately so the URL passed to git (and logged) never contains them. When
/// `log_source` is set, which credential is used — or why it could not be —
/// is written to the deploy log.
async fn resolve_git_source(
    state: &AppState,
    app: &App,
    source_url: &str,
    log_source: Option<&str>,
) -> Result<(String, Option<GitHttpAuth>)> {
    let (clean_url, url_auth) = split_url_credentials(source_url);
    let log = |level: &'static str, message: String| async move {
        if let Some(trigger_source) = log_source {
            log_deploy(state, &app.id, level, trigger_source, &message).await
        } else {
            Ok(())
        }
    };

    if let Some(ref gh_app_id) = app.github_app_id
        && let Ok(Some(gh_app)) = sqlx::query_as::<_, tond_common::GitHubApp>(
            "SELECT * FROM github_apps WHERE id = ? LIMIT 1",
        )
        .bind(gh_app_id)
        .fetch_optional(&state.db)
        .await
    {
        match github_apps::generate_installation_token(&gh_app).await {
            Ok(token) => {
                log(
                    "info",
                    "Using GitHub App installation token for clone".into(),
                )
                .await?;
                return Ok((
                    clean_url,
                    Some(GitHttpAuth {
                        username: "x-access-token".to_string(),
                        password: token,
                    }),
                ));
            }
            Err(e) => {
                log(
                    "warn",
                    format!("GitHub App token failed ({e}); cloning without it"),
                )
                .await?;
            }
        }
    } else if let Some(ref gl_token_id) = app.gitlab_token_id
        && let Ok(Some(gl_token)) = sqlx::query_as::<_, tond_common::GitLabToken>(
            "SELECT * FROM gitlab_tokens WHERE id = ? LIMIT 1",
        )
        .bind(gl_token_id)
        .fetch_optional(&state.db)
        .await
    {
        match gitlab_tokens::gitlab_git_credentials(&gl_token) {
            Ok(auth) => {
                log("info", "Using GitLab PAT for clone".into()).await?;
                return Ok((clean_url, Some(auth)));
            }
            Err(e) => {
                log(
                    "warn",
                    format!("GitLab PAT could not be used ({e}); cloning without it"),
                )
                .await?;
            }
        }
    }

    Ok((clean_url, url_auth))
}

async fn persist_generated_dockerfile(
    state: &AppState,
    deployment_id: &str,
    dockerfile: Option<&str>,
) {
    if let Some(dockerfile) = dockerfile {
        let _ = sqlx::query("UPDATE deployments SET generated_dockerfile = ? WHERE id = ?")
            .bind(dockerfile)
            .bind(deployment_id)
            .execute(&state.db)
            .await;
    }
}

/// The automatic hostname for an app: `<app>.<apps_domain>`, or `None` when
/// no apps domain is configured. Apps never default to a name under the
/// dashboard domain: that would put user code on the dashboard's site.
fn auto_domain(apps_domain: &str, app_name: &str) -> Option<String> {
    (!apps_domain.is_empty()).then(|| domains::app_host(app_name, apps_domain))
}

/// Give an app without a domain its automatic hostname, or log how to reach
/// it when there is no apps domain.
async fn assign_automatic_domain(
    state: &AppState,
    app: &mut App,
    trigger_source: &str,
) -> Result<()> {
    // Background workers serve no traffic: no address.
    if app.domain.is_some() || !app.readiness().serves_traffic() {
        return Ok(());
    }
    let apps_domain = state.settings.read().await.apps_domain.clone();
    match auto_domain(&apps_domain, &app.name) {
        Some(domain) => {
            sqlx::query("UPDATE apps SET domain = ? WHERE id = ?")
                .bind(&domain)
                .bind(&app.id)
                .execute(&state.db)
                .await?;
            app.domain = Some(domain);
        }
        None => {
            log_deploy(
                state,
                &app.id,
                "info",
                trigger_source,
                "No apps domain is set, so this app has no public address yet. Add a custom \
                 domain on the app's page, or set an apps domain in Settings.",
            )
            .await?;
        }
    }
    Ok(())
}

/// The environment of a preview container: only the app variables listed in
/// `shared_keys_json`, then the preview overrides on top. Previews run
/// pull-request code, so production variables (database URLs, API keys) are
/// withheld unless an admin shared them explicitly.
fn preview_env_vars(
    base_env: BTreeMap<String, String>,
    shared_keys_json: &str,
    preview_overrides: BTreeMap<String, String>,
) -> HashMap<String, String> {
    let shared_keys: Vec<String> = serde_json::from_str(shared_keys_json).unwrap_or_default();
    let mut env_vars: HashMap<String, String> = shared_keys
        .iter()
        .filter_map(|key| base_env.get(key).map(|value| (key.clone(), value.clone())))
        .collect();
    env_vars.extend(preview_overrides);
    env_vars
}

/// What a deployment does.
#[derive(Debug, Clone)]
pub enum DeployKind {
    /// Build or pull the app's configured source.
    Source,
    /// Build the source uploaded to `dir` (tarball uploads), then deploy it.
    Upload { dir: String },
    /// Re-run the exact image of an earlier deployment.
    Rollback {
        image_tag: String,
        target_deployment_id: String,
    },
}

/// A request to deploy an app.
#[derive(Debug, Clone)]
pub struct DeployRequest {
    pub kind: DeployKind,
    /// "manual" | "webhook" | "upload" | "rollback" | "cli" | "api"
    pub trigger_source: String,
    /// Who or what triggered it (user email, "api-key", webhook provider…)
    pub trigger_detail: Option<String>,
}

impl DeployRequest {
    pub fn source(trigger_source: &str, trigger_detail: Option<String>) -> Self {
        Self {
            kind: DeployKind::Source,
            trigger_source: trigger_source.to_string(),
            trigger_detail,
        }
    }
}

/// How a deployment ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployOutcome {
    Succeeded,
    Failed(DeployFailure),
}

/// Insert the deployment row (status "queued") and return its id. The build
/// queue runs it with [`run`]; use [`start`] for both.
pub async fn create(state: &AppState, app: &App, req: &DeployRequest) -> Result<String> {
    let deployment_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let (image_tag, rollback_of) = match &req.kind {
        DeployKind::Rollback {
            image_tag,
            target_deployment_id,
        } => (Some(image_tag.clone()), Some(target_deployment_id.clone())),
        _ => (None, None),
    };
    let plan_source = match req.kind {
        DeployKind::Rollback { .. } => "rollback",
        DeployKind::Upload { .. } => "tarball",
        DeployKind::Source => app.source_type.as_str(),
    };
    let phases = progress::plan_for_app(plan_source, app);
    let phases_json = serde_json::to_string(&phases).unwrap_or_else(|_| "[]".to_string());
    let kind = match req.kind {
        DeployKind::Rollback { .. } => "rollback",
        DeployKind::Upload { .. } => "upload",
        DeployKind::Source => "deploy",
    };
    sqlx::query(
        "INSERT INTO deployments (id, app_id, image_tag, status, kind, trigger_source, trigger_detail, rollback_of, phases, started_at, created_at)
         VALUES (?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&deployment_id)
    .bind(&app.id)
    .bind(image_tag)
    .bind(kind)
    .bind(&req.trigger_source)
    .bind(&req.trigger_detail)
    .bind(rollback_of)
    .bind(phases_json)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;
    Ok(deployment_id)
}

/// Create a deployment and queue it (deploy/queue.rs). Returns its id and
/// place in line at once so the caller can hand them to a live deploy view.
pub async fn start(state: &AppState, app: App, req: DeployRequest) -> Result<queue::Enqueued> {
    let deployment_id = create(state, &app, &req).await?;
    Ok(queue::submit_request(state, &app.id, deployment_id, req).await)
}

/// Run a created deployment to completion: the pipeline, the result on the
/// deployment row (phases, failure cause), the app status and alerts.
pub async fn run(
    state: &AppState,
    app: &mut App,
    deployment_id: &str,
    req: &DeployRequest,
    ctx: &DeployCtx,
) -> DeployOutcome {
    let app_id = app.id.clone();
    // Leave the queue: wait for a build slot unless nothing is built
    // (rollbacks re-run an existing image). The row becomes 'running'.
    let started = if matches!(req.kind, DeployKind::Rollback { .. }) {
        ctx.begin(state).await
    } else {
        ctx.begin_build(state).await
    };
    if let Err(e) = started {
        tracing::warn!("Deployment {deployment_id} could not leave the queue: {e:#}");
    }
    let plan_source = match req.kind {
        DeployKind::Rollback { .. } => "rollback",
        DeployKind::Upload { .. } => "tarball",
        DeployKind::Source => app.source_type.as_str(),
    };
    let phases = progress::plan_for_app(plan_source, app);
    progress::begin(state, &app_id, deployment_id, phases).await;

    let previous_status = app.status.clone();
    let old_container_id = app.container_id.clone();
    let mut facts = HealthFacts::default();
    let result = pipeline(state, app, deployment_id, req, &mut facts, ctx).await;
    ctx.release_build();

    if let DeployKind::Upload { dir } = &req.kind {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    match result {
        Ok(()) => {
            progress::finish_ok(state, &app_id, deployment_id).await;
            DeployOutcome::Succeeded
        }
        Err(e) => {
            let error = format!("{e:#}");
            let recent = progress::recent_lines(&app_id);
            let phase = progress::current_phase(&app_id);
            let mut failure = failure::classify(&FailureContext {
                phase: facts.probe.as_ref().map(|_| DeployPhase::Health).or(phase),
                error: &error,
                recent_lines: &recent,
                container_port: app.container_port,
                health_check_path: app.health_check_path.as_deref(),
                health_timeout_secs: facts.timeout_secs,
                probe: facts.probe.clone(),
                container: facts.container.clone(),
                container_logs: &facts.container_logs,
                memory_limit: app.memory_limit,
                has_registry_credential: app.registry_credential_id.is_some(),
                readiness: app.readiness(),
            });
            // The excerpt may quote the app's own output: keep its secrets out.
            for line in failure.excerpt.iter_mut() {
                *line = log_scrub::scrub_for_app(&state.db, &app_id, line).await;
            }
            if let Some(detail) = failure.detail.as_mut() {
                *detail = log_scrub::scrub_for_app(&state.db, &app_id, detail).await;
            }
            let _ = log_deploy(
                state,
                &app_id,
                "error",
                &req.trigger_source,
                &format!("Deployment failed: {}", failure.title),
            )
            .await;
            progress::finish_failed(state, &app_id, deployment_id, &failure).await;

            // The previous container keeps serving when the failure happened
            // before the cutover; only a first deploy (or compose) is "failed".
            let restore = match (&old_container_id, app.source_type.as_str()) {
                (Some(_), "compose") | (None, _) => "failed".to_string(),
                (Some(_), _) if matches!(previous_status.as_str(), "building" | "pending") => {
                    "running".to_string()
                }
                (Some(_), _) => previous_status,
            };
            let _ = update_status(state, &app_id, &restore).await;
            app.status = restore;

            tracing::error!("Deploy failed for {}: {}", app.name, error);
            DeployOutcome::Failed(failure)
        }
    }
}

/// What the health phase saw, for classifying a failure.
#[derive(Debug, Default)]
struct HealthFacts {
    probe: Option<ProbeResult>,
    container: Option<ContainerExit>,
    container_logs: Vec<String>,
    timeout_secs: u64,
}

/// The deployment pipeline. Phase transitions are recorded as it goes; the
/// caller turns an error into a classified failure.
async fn pipeline(
    state: &AppState,
    app: &mut App,
    deployment_id: &str,
    req: &DeployRequest,
    facts: &mut HealthFacts,
    ctx: &DeployCtx,
) -> Result<()> {
    let app_id = app.id.clone();
    let app_name = app.name.clone();
    let trigger_source = req.trigger_source.as_str();
    let is_rollback = matches!(req.kind, DeployKind::Rollback { .. });

    // Remember the old container so we can stop it after a successful swap.
    let old_container_id = app.container_id.clone();

    // The image this deployment runs, whatever the source: tond-<app>:<id>.
    let deploy_tag = images::deployment_tag(&app_name, deployment_id);

    update_status(state, &app_id, "building").await?;
    log_deploy(
        state,
        &app_id,
        "info",
        trigger_source,
        if is_rollback {
            "Starting rollback..."
        } else {
            "Starting deployment..."
        },
    )
    .await?;

    // ── Pre-deploy snapshot (fire-and-forget) ────────────────────────────────
    // If the app has a volume, snapshot it now so the user can roll back in ~5s
    // if the new deploy breaks something. A snapshot failure must never block a
    // deployment — hence the fire-and-forget spawn with `let _ =`.
    {
        let volumes: Vec<Volume> = serde_json::from_str(app.volumes.as_str()).unwrap_or_default();
        if !volumes.is_empty() {
            let snap_state = state.clone();
            let snap_app_name = app_name.clone();
            let snap_deploy_id = deployment_id.to_string();
            shutdown::spawn_tracked(async move {
                let _ = backup::create_snapshot(
                    &snap_state,
                    SnapshotTrigger::PreDeploy {
                        app_name: snap_app_name,
                        deployment_id: snap_deploy_id,
                    },
                    None,
                )
                .await;
            });
        }
    }

    let image_name = match &req.kind {
        DeployKind::Rollback { image_tag, .. } => {
            if app.source_type == "compose" {
                anyhow::bail!(
                    "Rollback is not supported for compose apps. \
                     Update the compose content and trigger a fresh deploy instead."
                );
            }
            // The exact image must still be on the host (old ones are pruned).
            if state.docker.inspect_image(image_tag).await.is_err() {
                let _ = sqlx::query(
                    "UPDATE deployments SET image_pruned = 1 WHERE app_id = ? AND image_tag = ?",
                )
                .bind(&app_id)
                .bind(image_tag)
                .execute(&state.db)
                .await;
                anyhow::bail!(
                    "Image {} is no longer on this server (old images are pruned), so this deployment can't be restored",
                    image_tag
                );
            }
            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                &format!("Rolling back to image: {}", image_tag),
            )
            .await?;
            image_tag.clone()
        }
        DeployKind::Upload { dir } => {
            progress::start(state, &app_id, DeployPhase::Build).await;
            let build_opts = BuildOptions {
                method: &app.build_method,
                static_build: app.static_build != 0,
                base_dir: app.base_dir.as_deref(),
            };
            // Built as `tond-<app>:upload` (what a later redeploy re-runs),
            // then pinned under this deployment's tag.
            let upload_tag = images::upload_tag(&app_name);
            build_from_dir_with_method(
                state,
                &app_id,
                &upload_tag,
                dir,
                &build_opts,
                Some(deployment_id),
                trigger_source,
            )
            .await?;
            images::tag_image(&state.docker, &upload_tag, &deploy_tag).await?;
            deploy_tag.clone()
        }
        DeployKind::Source => {
            source_image(state, app, deployment_id, &deploy_tag, trigger_source).await?
        }
    };

    // The image exists: give the build slot to the next deployment.
    ctx.release_build();

    // Record the image tag on the deployment row now that we know it.
    let _ = sqlx::query("UPDATE deployments SET image_tag = ? WHERE id = ?")
        .bind(&image_name)
        .bind(deployment_id)
        .execute(&state.db)
        .await;

    // ── Compose early-return ──────────────────────────────────────────────────
    //
    // For compose apps `run_inline_compose` already started all containers via
    // `docker compose up -d`. There is no single "image" to re-run through the
    // standard ZDT machinery. Instead we:
    //   1. Inspect the running compose project to find the host port.
    //   2. Write host_port + status='running' directly to the DB.
    //   3. Return — the proxy will pick up the new port on its next poll.
    //
    // ZDT for compose is handled by docker compose itself: `up -d` only
    // restarts services whose image/config changed, so existing connections
    // keep flowing through unchanged services.
    if app.source_type == "compose" && !is_rollback {
        progress::start(state, &app_id, DeployPhase::Start).await;
        let container_port = app.container_port as u16;

        let host_port = docker::find_compose_host_port(
            &state.docker,
            &super::build::compose_project_name(&app_name),
            container_port,
        )
        .await
        .with_context(|| {
            format!("no compose service publishes container port {container_port}; set the app's port to the port your web service listens on")
        })?;

        progress::start(state, &app_id, DeployPhase::Release).await;
        sqlx::query(
            "UPDATE apps SET container_id = ?, host_port = ?, status = 'running', updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&image_name) // sentinel: "tond-compose-{app_name}"
        .bind(host_port as i64)
        .bind(&app_id)
        .execute(&state.db)
        .await?;

        app.host_port = Some(host_port as i64);
        app.status = "running".to_string();
        assign_automatic_domain(state, app, trigger_source).await?;

        log_deploy(
            state,
            &app_id,
            "info",
            trigger_source,
            &format!(
                "Compose deploy succeeded! Proxy will route to 127.0.0.1:{}",
                host_port
            ),
        )
        .await?;
        return Ok(());
    }

    // ── Start ──
    progress::start(state, &app_id, DeployPhase::Start).await;

    // Always allocate a fresh host port for the new container. On redeployments
    // the old container still holds the old port while the new one starts up
    // (zero-downtime staging swap), so we cannot reuse the existing port here.
    // On the very first deploy there is no old container and no reserved port,
    // so this also handles that case correctly.
    // Held until the new container owns its port (see queue::port_allocation).
    let port_guard = queue::port_allocation().await;
    let new_host_port = docker::find_free_port(&state.docker, 8100).await?;
    let container_port = app.container_port as u16;

    // Decrypt env vars (production deploy — no preview filtering)
    let env_vars: HashMap<String, String> = secrets::app_env(app)?.into_iter().collect();

    // Host paths are re-derived from volume names and verified to stay inside
    // <data>/volumes/<app> (no symlink escapes) right before the mount.
    let volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
    let binds = volumes::app_binds(&volumes::data_dir(), &app_name, &volumes)?;

    log_deploy(
        state,
        &app_id,
        "info",
        trigger_source,
        &format!(
            "Starting new container on port {}:{}",
            new_host_port, container_port
        ),
    )
    .await?;

    // The container joins only its home network: the project network, or the
    // app's own network (see networks.rs). Never the default bridge.
    let network = networks::ensure_app_home(&state.docker, app)
        .await
        .context("could not create the app's Docker network")?;

    // Start the NEW container.
    //
    // On redeployments we use a temporary staging name (`tond-{app_name}-next`)
    // so the old canonical container (`tond-{app_name}`) keeps serving traffic
    // while the new one starts up and passes the health check. Once healthy we
    // atomically cut over in the DB, wait for the proxy to re-read it, then
    // SIGTERM the old container and rename the staging container to canonical.
    //
    // On the very first deploy we just use the canonical name directly.
    let staging_name = format!("tond-{}-next", app_name);
    let canonical_name = format!("tond-{}", app_name);

    let new_container_id = if old_container_id.is_some() {
        // A staging container left over from an interrupted deploy would
        // block the name.
        let _ = docker::remove_container(&state.docker, &staging_name).await;
        docker::run_container_named(
            &state.docker,
            &image_name,
            &staging_name,
            container_port,
            new_host_port,
            docker::Exposure::Loopback,
            env_vars,
            app.memory_limit,
            app.cpu_limit,
            binds,
            &network,
            Some(&app_name),
            None,
        )
        .await
        .context("Docker could not start the new container")?
    } else {
        docker::run_container(
            &state.docker,
            &image_name,
            &app_name,
            container_port,
            new_host_port,
            docker::Exposure::Loopback,
            env_vars,
            app.memory_limit,
            app.cpu_limit,
            binds,
            &network,
        )
        .await
        .context("Docker could not start the new container")?
    };
    drop(port_guard);

    // ── Health ──
    //
    // Poll the new container directly on its host port (bypassing the proxy)
    // so that the old container keeps serving traffic until we are confident
    // the new one is ready. How depends on the app's readiness mode:
    //   http  a health check path must answer 2xx; without one, any HTTP
    //         answer on the app's port will do (so a wrong port is caught
    //         here instead of as 502s after the cutover)
    //   tcp   the container port accepts connections (probed on the
    //         container's own address: docker-proxy accepts any connection)
    //   none  background worker: the container stays up ready_after_secs
    progress::start(state, &app_id, DeployPhase::Health).await;
    let readiness = app.readiness();
    let port_timeout = (app.drain_timeout.max(1) as u64).max(PORT_PROBE_MIN_SECS);
    let (probe, timeout_secs) = match readiness {
        Readiness::Http => {
            let (path, require_2xx, timeout_secs) = match app.health_check_path.clone() {
                Some(path) => (path, true, app.drain_timeout.max(1) as u64),
                None => ("/".to_string(), false, port_timeout),
            };
            let url = format!("http://127.0.0.1:{}{}", new_host_port, path);
            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                &if require_2xx {
                    format!(
                        "Waiting up to {}s for the health check on port {}{} to pass",
                        timeout_secs, container_port, path
                    )
                } else {
                    format!(
                        "Waiting up to {}s for the app to answer on port {}",
                        timeout_secs, container_port
                    )
                },
            )
            .await?;
            let probe = probe_until_ready(
                &state.docker,
                &new_container_id,
                &url,
                require_2xx,
                timeout_secs,
            )
            .await?;
            (probe, timeout_secs)
        }
        Readiness::Tcp => {
            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                &format!(
                    "Waiting up to {}s for port {} to accept connections",
                    port_timeout, container_port
                ),
            )
            .await?;
            let probe = probe_tcp_until_ready(
                &state.docker,
                &new_container_id,
                container_port,
                port_timeout,
            )
            .await;
            (probe, port_timeout)
        }
        Readiness::None => {
            let secs = app.ready_after_secs.max(1) as u64;
            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                &format!("Background worker: waiting {secs}s to make sure it stays up"),
            )
            .await?;
            (
                wait_stays_up(&state.docker, &new_container_id, secs).await,
                secs,
            )
        }
    };
    if probe.result != ProbeResult::Healthy {
        facts.timeout_secs = timeout_secs;
        facts.probe = Some(probe.result);
        facts.container = probe.container;
        facts.container_logs = docker::get_logs(&state.docker, &new_container_id, 40)
            .await
            .unwrap_or_default()
            .iter()
            .map(|l| strip_docker_ts(l).to_string())
            .collect();
        let _ = docker::remove_container(&state.docker, &new_container_id).await;
        anyhow::bail!(
            "the new container did not become ready within {}s; the previous version keeps running",
            timeout_secs
        );
    }
    log_deploy(
        state,
        &app_id,
        "info",
        trigger_source,
        if readiness.serves_traffic() {
            "New container is ready — cutting over traffic"
        } else {
            "The worker stayed up — replacing the old container"
        },
    )
    .await?;

    // ── Release: atomic traffic cutover ──
    //
    // Update the DB in a single statement (the route_state trigger bumps the
    // route version), then have tond-proxy reload and acknowledge it before
    // the old container is stopped.
    progress::start(state, &app_id, DeployPhase::Release).await;
    sqlx::query(
        "UPDATE apps SET container_id = ?, host_port = ?, status = 'running', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&new_container_id)
    .bind(new_host_port as i64)
    .bind(&app_id)
    .execute(&state.db)
    .await?;

    app.container_id = Some(new_container_id.clone());
    app.host_port = Some(new_host_port as i64);
    app.status = "running".to_string();

    // ── Zero-downtime cutover: proxy ack → SIGTERM old → rename staging ──
    //
    // tond-proxy must be routing to the new port before the old container is
    // stopped: wait for its acknowledgement (tens of milliseconds).
    //
    // On the first deploy there is no old container and the new one already has
    // the canonical name, so skip this block entirely.
    if let Some(ref old_id) = old_container_id
        && old_id != &new_container_id
    {
        // Workers are never routed: nothing for the proxy to pick up.
        if readiness.serves_traffic() {
            wait_for_proxy_cutover(state, &app_id, trigger_source).await?;
        }

        log_deploy(
            state,
            &app_id,
            "info",
            trigger_source,
            &format!(
                "Draining old container {} ({}s grace)",
                &old_id[..12.min(old_id.len())],
                app.drain_timeout
            ),
        )
        .await?;
        let _ =
            docker::stop_container_with_timeout(&state.docker, old_id, app.drain_timeout as u64)
                .await;
        let _ = docker::remove_container(&state.docker, old_id).await;

        // Rename staging container to the canonical name so Docker labels,
        // `docker ps` output, and any tooling that looks up containers by
        // name continues to work correctly after the swap.
        let _ = docker::rename_container(&state.docker, &staging_name, &canonical_name).await;
    }

    assign_automatic_domain(state, app, trigger_source).await?;

    log_deploy(
        state,
        &app_id,
        "info",
        trigger_source,
        &format!(
            "{} Container: {}",
            if is_rollback {
                "Rollback succeeded!"
            } else {
                "Deployed successfully!"
            },
            &new_container_id[..12.min(new_container_id.len())]
        ),
    )
    .await?;

    images::prune_app_images(state, &app_id, &app_name).await;

    // The home network may have just been (re)created: make sure the
    // databases linked to this app are attached to it.
    if let Err(e) = networks::reconcile_databases_for_app(state, app).await {
        log_deploy(
            state,
            &app_id,
            "warn",
            trigger_source,
            &format!("Could not attach linked databases to the app network: {e}"),
        )
        .await?;
    }

    Ok(())
}

/// Wait until tond-proxy routes the app's new port (see proxy_link.rs), and
/// say in the deploy log how the cutover was confirmed.
async fn wait_for_proxy_cutover(
    state: &AppState,
    app_id: &str,
    trigger_source: &str,
) -> Result<()> {
    let outcome = proxy_link::sync_routes(state).await;
    log_deploy(state, app_id, "info", trigger_source, &outcome.describe()).await?;
    if let Some(wait) = outcome.fallback_wait() {
        tokio::time::sleep(wait).await;
    }
    Ok(())
}

/// Build, pull or re-tag the app's configured source into `deploy_tag`
/// (compose apps start their services here and return a sentinel).
async fn source_image(
    state: &AppState,
    app: &App,
    deployment_id: &str,
    deploy_tag: &str,
    trigger_source: &str,
) -> Result<String> {
    let app_id = app.id.as_str();
    let app_name = app.name.as_str();
    Ok(match app.source_type.as_str() {
        "git" => {
            progress::start(state, app_id, DeployPhase::Clone).await;
            let source_url = app
                .source_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Git source URL required"))?;
            let branch = app.branch.as_deref().unwrap_or("main");

            let (clone_url, git_auth) =
                resolve_git_source(state, app, source_url, Some(trigger_source)).await?;

            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                &format!("Building from git: {} ({})", clone_url, branch),
            )
            .await?;

            build_from_git(
                state,
                app_id,
                app_name,
                &clone_url,
                git_auth.as_ref(),
                branch,
                &BuildOptions {
                    method: &app.build_method,
                    static_build: app.static_build != 0,
                    base_dir: app.base_dir.as_deref(),
                },
                deploy_tag,
                Some(deployment_id),
                trigger_source,
            )
            .await?
        }
        "image" => {
            progress::start(state, app_id, DeployPhase::Clone).await;
            let image = app
                .source_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Docker image reference required"))?;

            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                &format!("Pulling image: {}", image),
            )
            .await?;

            // Look up registry credentials if one is attached to the app
            let credentials = if let Some(cred_id) = &app.registry_credential_id {
                fetch_registry_credentials(state, cred_id).await?
            } else {
                None
            };

            pull_image(&state.docker, image, credentials).await?;

            // Pin the pulled bytes under this deployment's tag so a rollback
            // runs exactly this image even after `image` moves on upstream.
            images::tag_image(&state.docker, image, deploy_tag).await?;
            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                &format!("Pinned {} as {}", image, deploy_tag),
            )
            .await?;
            deploy_tag.to_string()
        }
        "tarball" => {
            // A redeploy of an uploaded app re-runs the last uploaded build.
            progress::relabel(state, app_id, DeployPhase::Build, "Use uploaded image").await;
            progress::start(state, app_id, DeployPhase::Build).await;
            let upload_tag = images::upload_tag(app_name);
            images::tag_image(&state.docker, &upload_tag, deploy_tag)
                .await
                .with_context(|| format!("no uploaded image {upload_tag} to deploy"))?;
            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                &format!("Using uploaded image: {}", deploy_tag),
            )
            .await?;
            deploy_tag.to_string()
        }
        "dockerfile" => {
            progress::start(state, app_id, DeployPhase::Build).await;
            let content = app.dockerfile_content.as_deref().ok_or_else(|| {
                anyhow::anyhow!("dockerfile_content is required for source_type='dockerfile'")
            })?;
            persist_generated_dockerfile(state, deployment_id, Some(content)).await;

            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                "Building from inline Dockerfile...",
            )
            .await?;

            build_from_inline_dockerfile(
                state,
                app_id,
                app_name,
                content,
                deploy_tag,
                trigger_source,
            )
            .await?
        }
        "compose" => {
            progress::start(state, app_id, DeployPhase::Build).await;
            let content = app.compose_content.as_deref().ok_or_else(|| {
                anyhow::anyhow!("compose_content is required for source_type='compose'")
            })?;

            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                "Starting inline Docker Compose application...",
            )
            .await?;

            // `${VAR}` in the compose file is filled from the app's env
            // (templates keep their generated passwords there).
            let env = secrets::app_env(app)?;
            run_inline_compose(state, app_id, app_name, content, &env, trigger_source).await?
        }
        other => {
            anyhow::bail!("Unsupported source type: {}", other);
        }
    })
}

/// Auto-assign `<app>.<dashboard domain>` when the app has no domain.
/// Falls back to `<app>.localhost` if no dashboard domain is configured.
/// Without a health check path the port probe waits at least this long, so a
/// slow-booting app isn't failed by a short drain timeout.
const PORT_PROBE_MIN_SECS: u64 = 60;

/// Drop the RFC 3339 timestamp Docker prepends with `timestamps(true)`.
fn strip_docker_ts(raw: &str) -> &str {
    match raw.split_once(' ') {
        Some((ts, rest)) if ts.len() <= 35 && ts.contains('T') => rest.trim_end(),
        _ => raw.trim_end(),
    }
}

/// Deploy a preview of an app with a unique subdomain and TTL.
///
/// Steps:
///   1. Build/pull the image (same as a regular deploy).
///   2. Pick a free host port (starting at 8200 to keep separate from main deploys).
///   3. Start the container as `tond-preview-{slug}` with preview env vars and its own empty volumes.
///   4. Insert a `preview_deployments` row and update it once the container is running.
///
/// The caller must provide the `slug` (short random hex) and the `ttl_hours`.
/// The `subdomain` (under the preview domain) is set by the caller; see domains::preview_host.
pub async fn deploy_preview(
    state: &AppState,
    app: &App,
    preview: &mut PreviewDeployment,
) -> Result<()> {
    let app_id = app.id.clone();
    let app_name = app.name.clone();
    let slug = preview.slug.clone();

    tracing::info!("Starting preview deploy for {} (slug: {})", app_name, slug);

    // Previews build outside the per-app queue but share the global build
    // slots, and use their own build directory so they never race the app's
    // production build.
    let build_slot = queue::build_slot().await;
    let work_name = format!("{app_name}-preview-{slug}");

    let image_name = match app.source_type.as_str() {
        "git" => {
            let source_url = app
                .source_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Git source URL required"))?;
            // For PR previews, deploy the PR head branch; fall back to the
            // app's configured branch, then "main".
            let branch = preview
                .head_ref
                .as_deref()
                .or(app.branch.as_deref())
                .unwrap_or("main");

            tracing::info!(
                "Preview: cloning git {} ({})",
                redact_url_credentials(source_url),
                branch
            );

            let (clone_url, git_auth) = resolve_git_source(state, app, source_url, None).await?;

            build_from_git(
                state,
                &app_id,
                &work_name,
                &clone_url,
                git_auth.as_ref(),
                branch,
                &BuildOptions {
                    method: &app.build_method,
                    static_build: app.static_build != 0,
                    base_dir: app.base_dir.as_deref(),
                },
                &images::preview_tag(&app_name, &slug),
                None,
                "preview",
            )
            .await?
        }
        "image" => {
            let image = app
                .source_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Docker image reference required"))?;

            let credentials = if let Some(cred_id) = &app.registry_credential_id {
                fetch_registry_credentials(state, cred_id).await?
            } else {
                None
            };

            pull_image(&state.docker, image, credentials).await?;
            image.clone()
        }
        "tarball" => images::upload_tag(&app_name),
        "dockerfile" => {
            let content = app.dockerfile_content.as_deref().ok_or_else(|| {
                anyhow::anyhow!("dockerfile_content required for source_type='dockerfile'")
            })?;
            build_from_inline_dockerfile(
                state,
                &app_id,
                &work_name,
                content,
                &images::preview_tag(&app_name, &slug),
                "preview",
            )
            .await?
        }
        "compose" => {
            // Preview deployments are not supported for compose apps because
            // each preview would need its own isolated compose project, which
            // requires namespacing that is not yet implemented.
            anyhow::bail!("Preview deployments are not supported for compose apps.");
        }
        other => {
            anyhow::bail!("Unsupported source type for preview: {}", other);
        }
    };

    drop(build_slot);
    preview.image_tag = Some(image_name.clone());

    // Update image_tag in DB now that we know it
    let _ = sqlx::query("UPDATE preview_deployments SET image_tag = ? WHERE id = ?")
        .bind(&image_name)
        .bind(&preview.id)
        .execute(&state.db)
        .await;

    // Pick a free port in the preview range (8200+)
    let port_guard = queue::port_allocation().await;
    let host_port = docker::find_free_port(&state.docker, 8200).await?;
    let container_port = app.container_port as u16;

    let env_vars = preview_env_vars(
        secrets::app_env(app)?,
        app.preview_shared_keys.as_str(),
        secrets::app_preview_env(app)?,
    );

    // Previews run pull-request code: they get the app's volume layout but
    // their own empty directories under <data>/volumes/_preview/<slug>, never
    // the production data. The janitor removes them once the preview is gone.
    let volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
    let binds = volumes::preview_binds(&volumes::data_dir(), &slug, &volumes)?;

    let container_name = format!("tond-preview-{}", slug);

    tracing::info!(
        "Preview: starting container {} on port {}:{}",
        container_name,
        host_port,
        container_port
    );

    // Previews run pull-request code: they get a network of their own and
    // never join the project network, so they can't reach production
    // databases or apps.
    let network = networks::ensure_preview(&state.docker, &slug)
        .await
        .context("could not create the preview's Docker network")?;

    let container_id = docker::run_container_named(
        &state.docker,
        &image_name,
        &container_name,
        container_port,
        host_port,
        docker::Exposure::Loopback,
        env_vars,
        app.memory_limit,
        app.cpu_limit,
        binds,
        &network,
        Some(&app_name),
        None,
    )
    .await?;
    drop(port_guard);

    // Update the preview row with runtime details
    sqlx::query(
        "UPDATE preview_deployments
         SET container_id = ?, host_port = ?, status = 'running'
         WHERE id = ?",
    )
    .bind(&container_id)
    .bind(host_port as i64)
    .bind(&preview.id)
    .execute(&state.db)
    .await?;

    preview.container_id = Some(container_id);
    preview.host_port = Some(host_port as i64);
    preview.status = "running".to_string();

    tracing::info!(
        "Preview {} running on port {} (expires: {})",
        slug,
        host_port,
        preview.expires_at
    );

    // Post a "success" commit status and PR comment to GitHub if this is a PR preview.
    if let (Some(sha), Some(gh_app_id)) = (&preview.head_sha, &app.github_app_id) {
        let source_url = app.source_url.as_deref().unwrap_or("");
        if let Some((owner, repo)) = parse_github_owner_repo(source_url)
            && let Ok(Some(gh_app)) = sqlx::query_as::<_, tond_common::GitHubApp>(
                "SELECT * FROM github_apps WHERE id = ? LIMIT 1",
            )
            .bind(gh_app_id)
            .fetch_optional(&state.db)
            .await
        {
            let preview_url = preview.subdomain.as_ref().map(|s| format!("https://{}", s));
            let _ = github_apps::post_commit_status(
                &gh_app,
                &owner,
                &repo,
                sha,
                "success",
                "Preview environment is live",
                preview_url.as_deref(),
                "tond/preview",
            )
            .await
            .map_err(|e| {
                tracing::warn!("[preview] failed to post commit status: {}", e);
            });

            // Post (or update) a PR comment with the preview URL.
            if let (Some(pr_number), Some(url)) = (preview.pr_number, &preview_url) {
                // Fetch the current comment_id for this preview row (may have been set
                // by a previous synchronize cycle on the same row, though in practice
                // each synchronize creates a fresh row — use the old row's comment_id
                // which the webhook copied into this new row as None currently).
                let existing_comment_id: Option<i64> =
                    sqlx::query_scalar("SELECT comment_id FROM preview_deployments WHERE id = ?")
                        .bind(&preview.id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten()
                        .flatten();

                let comment_body = format!(
                    "### Tond Preview Deployment\n\n\
                     | | |\n\
                     |---|---|\n\
                     | **Status** | live |\n\
                     | **URL** | [{}]({}) |\n\
                     | **Branch** | `{}` |\n\
                     | **Commit** | `{}` |\n\n\
                     The preview will be automatically removed when the PR is closed.",
                    url,
                    url,
                    preview.head_ref.as_deref().unwrap_or("—"),
                    &sha[..sha.len().min(7)],
                );

                match github_apps::post_or_update_pr_comment(
                    &gh_app,
                    &owner,
                    &repo,
                    pr_number,
                    &comment_body,
                    existing_comment_id,
                )
                .await
                {
                    Ok(comment_id) => {
                        let _ = sqlx::query(
                            "UPDATE preview_deployments SET comment_id = ? WHERE id = ?",
                        )
                        .bind(comment_id)
                        .bind(&preview.id)
                        .execute(&state.db)
                        .await;
                    }
                    Err(e) => {
                        tracing::warn!("[preview] failed to post PR comment: {}", e);
                    }
                }
            }
        }
    }

    // Post a "success" commit status to GitLab if this is a GitLab MR preview.
    if let (Some(sha), Some(gl_token_id), Some(gl_project_id)) = (
        &preview.head_sha,
        &app.gitlab_token_id,
        &preview.gitlab_project_id,
    ) && let Ok(Some(gl_token)) = sqlx::query_as::<_, tond_common::GitLabToken>(
        "SELECT * FROM gitlab_tokens WHERE id = ? LIMIT 1",
    )
    .bind(gl_token_id)
    .fetch_optional(&state.db)
    .await
    {
        let preview_url = preview.subdomain.as_ref().map(|s| format!("https://{}", s));
        let _ = gitlab_tokens::post_gitlab_commit_status(
            &gl_token,
            *gl_project_id,
            sha,
            "success",
            "Preview environment is live",
            preview_url.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::warn!(
                "[preview] failed to post GitLab success commit status: {}",
                e
            );
        });
    }

    Ok(())
}

/// What [`probe_until_ready`] saw last.
#[derive(Debug)]
struct Probe {
    result: ProbeResult,
    /// Set when the container stopped (crash, OOM) before becoming ready.
    container: Option<ContainerExit>,
}

/// The container's address on its first Docker network.
async fn container_ip(docker: &bollard::Docker, container_id: &str) -> Option<std::net::IpAddr> {
    let info = docker.inspect_container(container_id, None).await.ok()?;
    let networks = info.network_settings?.networks?;
    networks
        .values()
        .filter_map(|n| n.ip_address.as_deref())
        .find_map(|ip| ip.parse().ok())
}

/// Poll until the container accepts TCP connections on `port`. Connects to
/// the container's own address: the loopback-published port is held by
/// docker-proxy, which accepts connections even when nothing listens.
async fn probe_tcp_until_ready(
    docker: &bollard::Docker,
    container_id: &str,
    port: u16,
    timeout_secs: u64,
) -> Probe {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut last = ProbeResult::NotListening;
    loop {
        if let Some(ip) = container_ip(docker, container_id).await {
            let addr = std::net::SocketAddr::new(ip, port);
            last = match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                tokio::net::TcpStream::connect(addr),
            )
            .await
            {
                Ok(Ok(_)) => {
                    return Probe {
                        result: ProbeResult::Healthy,
                        container: None,
                    };
                }
                Ok(Err(_)) => ProbeResult::NotListening,
                Err(_) => ProbeResult::Timeout,
            };
        }
        let exit = container_exit(docker, container_id).await;
        if let Some(exit) = exit.filter(|e| !e.running) {
            return Probe {
                result: last,
                container: Some(exit),
            };
        }
        if tokio::time::Instant::now() >= deadline {
            return Probe {
                result: last,
                container: container_exit(docker, container_id).await,
            };
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Background workers: ready once the container has stayed up `secs`
/// without exiting or being restarted.
async fn wait_stays_up(docker: &bollard::Docker, container_id: &str, secs: u64) -> Probe {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let exit = container_exit(docker, container_id).await;
        match exit {
            Some(exit) if !exit.running => {
                return Probe {
                    result: ProbeResult::NotListening,
                    container: Some(exit),
                };
            }
            None => {
                return Probe {
                    result: ProbeResult::NotListening,
                    container: Some(ContainerExit::default()),
                };
            }
            Some(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Probe {
                result: ProbeResult::Healthy,
                container: None,
            };
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Whether one HTTP answer means "ready".
fn probe_outcome(status: Option<u16>, timed_out: bool, require_2xx: bool) -> ProbeResult {
    match status {
        Some(code) if !require_2xx || (200..300).contains(&code) => ProbeResult::Healthy,
        Some(code) => ProbeResult::HttpStatus(code),
        None if timed_out => ProbeResult::Timeout,
        None => ProbeResult::NotListening,
    }
}

/// How the container is doing; `running` is false while Docker restarts a
/// crashed container.
async fn container_exit(docker: &bollard::Docker, container_id: &str) -> Option<ContainerExit> {
    let info = docker.inspect_container(container_id, None).await.ok()?;
    let crashed_before = info.restart_count.unwrap_or(0) > 0;
    let st = info.state?;
    Some(ContainerExit {
        running: st.running.unwrap_or(false) && !st.restarting.unwrap_or(false) && !crashed_before,
        exit_code: st.exit_code,
        oom_killed: st.oom_killed.unwrap_or(false),
    })
}

/// Poll `url` once per second until the app answers: any HTTP response, or a
/// 2xx when `require_2xx` (a health check path). Stops early when the
/// container exits.
async fn probe_until_ready(
    docker: &bollard::Docker,
    container_id: &str,
    url: &str,
    require_2xx: bool,
    timeout_secs: u64,
) -> Result<Probe> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building reqwest client for health check")?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        let last = match client.get(url).send().await {
            Ok(resp) => probe_outcome(Some(resp.status().as_u16()), false, require_2xx),
            Err(e) => probe_outcome(None, e.is_timeout(), require_2xx),
        };
        if last == ProbeResult::Healthy {
            return Ok(Probe {
                result: last,
                container: None,
            });
        }

        let exit = container_exit(docker, container_id).await;
        if let Some(exit) = exit.filter(|e| !e.running) {
            return Ok(Probe {
                result: last,
                container: Some(exit),
            });
        }

        if tokio::time::Instant::now() >= deadline {
            return Ok(Probe {
                result: last,
                container: container_exit(docker, container_id).await,
            });
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Extract (owner, repo) from a GitHub HTTPS clone URL.
///
/// Handles:
///   `https://github.com/owner/repo.git`  → ("owner", "repo")
///   `https://github.com/owner/repo`      → ("owner", "repo")
fn parse_github_owner_repo(url: &str) -> Option<(String, String)> {
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

/// Fetch and decrypt registry credentials for a given credential ID.
/// Returns None if the credential is not found (non-fatal — fall back to unauthenticated pull).
pub async fn fetch_registry_credentials(
    state: &AppState,
    cred_id: &str,
) -> Result<Option<bollard::auth::DockerCredentials>> {
    use tond_common::RegistryCredential;
    let cred =
        sqlx::query_as::<_, RegistryCredential>("SELECT * FROM registry_credentials WHERE id = ?")
            .bind(cred_id)
            .fetch_optional(&state.db)
            .await?;

    let Some(cred) = cred else {
        tracing::warn!(
            "Registry credential {} not found — pulling without auth",
            cred_id
        );
        return Ok(None);
    };

    let password = secret_key::decrypt(Purpose::RegistryPassword, &cred.password_enc)?;

    Ok(Some(bollard::auth::DockerCredentials {
        username: Some(cred.username),
        password: Some(password),
        serveraddress: Some(cred.server),
        ..Default::default()
    }))
}

/// Pull a Docker image, optionally using registry credentials.
pub async fn pull_image(
    docker: &bollard::Docker,
    image: &str,
    credentials: Option<bollard::auth::DockerCredentials>,
) -> Result<()> {
    use bollard::query_parameters::CreateImageOptionsBuilder;
    use futures::StreamExt;

    let options = Some(
        CreateImageOptionsBuilder::default()
            .from_image(image)
            .build(),
    );

    let mut stream = docker.create_image(options, None, credentials);

    while let Some(result) = stream.next().await {
        match result {
            Ok(info) => {
                if let Some(status) = info.status {
                    tracing::debug!("Pull: {}", status);
                }
            }
            Err(e) => anyhow::bail!("Failed to pull image: {}", e),
        }
    }

    Ok(())
}

/// Helper: update app status in DB
pub(super) async fn update_status(state: &AppState, app_id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE apps SET status = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(status)
        .bind(app_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Helper: insert a deploy log entry.
///
/// Every message is passed through `redact_url_credentials` first: deploy
/// logs are shown in the dashboard and copied into snapshots, so a URL with
/// `user:token@` must never reach them, whatever its source (clone output,
/// docker build output, error messages).
pub(super) async fn log_deploy(
    state: &AppState,
    app_id: &str,
    level: &str,
    trigger_source: &str,
    message: &str,
) -> Result<()> {
    let scrubbed = log_scrub::scrub_for_app(&state.db, app_id, message).await;
    let message = scrubbed.as_str();
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    // Preview builds share the app id but are not part of its deployment.
    let deployment_id = if trigger_source == "preview" {
        None
    } else {
        progress::active_deployment(app_id)
    };
    if deployment_id.is_some() {
        progress::record_line(app_id, message);
    }

    sqlx::query(
        "INSERT INTO deploy_logs (id, app_id, deployment_id, message, level, trigger_source, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(app_id)
    .bind(&deployment_id)
    .bind(message)
    .bind(level)
    .bind(trigger_source)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // Deploy logs live in the database (and the dashboard); journald only
    // gets problems. Build output (one call per line) stays at debug.
    match level {
        "error" => tracing::error!("[{}] {}", app_id, message),
        "warn" => tracing::warn!("[{}] {}", app_id, message),
        _ => tracing::debug!("[{}] {}", app_id, message),
    }

    log_bus::publish(DeployLogEvent {
        id,
        app_id: app_id.to_string(),
        deployment_id,
        level: level.to_string(),
        message: message.to_string(),
        created_at: now,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use super::*;
    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for deploy tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    #[test]
    fn parse_github_owner_repo_accepts_http_and_https_urls_and_rejects_invalid_paths() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/tond-dev/mvp-dev.git"),
            Some(("tond-dev".to_string(), "mvp-dev".to_string())),
            "parse_github_owner_repo should extract owner and repo from HTTPS clone URLs"
        );
        assert_eq!(
            parse_github_owner_repo("http://github.com/tond-dev/mvp-dev"),
            Some(("tond-dev".to_string(), "mvp-dev".to_string())),
            "parse_github_owner_repo should extract owner and repo from HTTP clone URLs without the .git suffix"
        );
        assert_eq!(
            parse_github_owner_repo("https://github.com/tond-dev"),
            None,
            "parse_github_owner_repo should reject GitHub URLs missing the repository segment"
        );
        assert_eq!(
            parse_github_owner_repo("git@github.com:tond-dev/mvp-dev.git"),
            None,
            "parse_github_owner_repo should reject unsupported SSH clone URL formats"
        );
    }

    #[test]
    fn probe_outcome_needs_2xx_only_for_a_health_path() {
        assert_eq!(probe_outcome(Some(404), false, false), ProbeResult::Healthy);
        assert_eq!(probe_outcome(Some(200), false, true), ProbeResult::Healthy);
        assert_eq!(
            probe_outcome(Some(503), false, true),
            ProbeResult::HttpStatus(503)
        );
        assert_eq!(probe_outcome(None, true, true), ProbeResult::Timeout);
        assert_eq!(probe_outcome(None, false, false), ProbeResult::NotListening);
    }

    #[test]
    fn strip_docker_ts_drops_the_timestamp_prefix() {
        assert_eq!(
            strip_docker_ts("2026-03-20T23:27:38.123456789Z listening on :80\n"),
            "listening on :80"
        );
        assert_eq!(strip_docker_ts("plain line"), "plain line");
    }

    #[tokio::test]
    async fn create_records_trigger_phases_and_rollback_target() {
        let state = test_state().await;
        let mut app = App::new("web".to_string(), "image".to_string(), None, None, 80);
        app.health_check_path = Some("/up".to_string());
        let id = create(
            &state,
            &app,
            &DeployRequest::source("manual", Some("ada@example.com".to_string())),
        )
        .await
        .unwrap_or_else(|e| panic!("create: {e}"));
        let row: tond_common::Deployment = sqlx::query_as("SELECT * FROM deployments WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(row.status, "queued", "the build queue moves it to running");
        assert_eq!(row.trigger_detail.as_deref(), Some("ada@example.com"));
        let phases = row.phase_records();
        assert_eq!(phases.len(), 5);
        assert_eq!(phases[0].label.as_deref(), Some("Pull image"));
        assert_eq!(phases[3].label.as_deref(), Some("Health check"));

        let rollback = DeployRequest {
            kind: DeployKind::Rollback {
                image_tag: "tond-web:abc".to_string(),
                target_deployment_id: id.clone(),
            },
            trigger_source: "rollback".to_string(),
            trigger_detail: None,
        };
        let rb = create(&state, &app, &rollback)
            .await
            .unwrap_or_else(|e| panic!("create: {e}"));
        let row: tond_common::Deployment = sqlx::query_as("SELECT * FROM deployments WHERE id = ?")
            .bind(&rb)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(row.rollback_of.as_deref(), Some(id.as_str()));
        assert_eq!(row.image_tag.as_deref(), Some("tond-web:abc"));
        assert_eq!(row.trigger_source, "rollback");
        assert_eq!(
            row.phase_records()[1].status,
            tond_common::PhaseStatus::Skipped
        );
    }

    #[tokio::test]
    async fn log_lines_are_attributed_to_the_active_deployment() {
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO apps (id, name, created_at, updated_at) VALUES ('attrib-app', 'attrib', 't', 't')",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("seed: {e}"));
        progress::begin(
            &state,
            "attrib-app",
            "dep-attrib",
            progress::plan_for("git", None),
        )
        .await;
        log_deploy(&state, "attrib-app", "info", "manual", "cloning")
            .await
            .unwrap_or_else(|e| panic!("log: {e}"));
        log_deploy(&state, "attrib-app", "info", "preview", "preview build")
            .await
            .unwrap_or_else(|e| panic!("log: {e}"));
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT message, deployment_id FROM deploy_logs WHERE app_id = 'attrib-app' ORDER BY rowid",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(
            rows,
            vec![
                ("cloning".to_string(), Some("dep-attrib".to_string())),
                ("preview build".to_string(), None),
            ]
        );
        assert_eq!(progress::recent_lines("attrib-app"), vec!["cloning"]);
        progress::finish_ok(&state, "attrib-app", "dep-attrib").await;
        assert!(progress::active_deployment("attrib-app").is_none());
    }

    #[test]
    fn auto_domain_uses_the_apps_domain_and_nothing_else() {
        assert_eq!(
            auto_domain("example-apps.com", "frontend").as_deref(),
            Some("frontend.example-apps.com"),
            "auto_domain should suffix the app name with the apps domain"
        );
        assert_eq!(
            auto_domain("", "frontend"),
            None,
            "without an apps domain there is no automatic hostname (never the dashboard's site)"
        );
    }

    #[test]
    fn preview_env_vars_passes_only_shared_keys_and_applies_overrides() {
        let map = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let env = preview_env_vars(
            map(&[
                ("DATABASE_URL", "prod"),
                ("PUBLIC_FLAG", "1"),
                ("KEEP", "yes"),
            ]),
            r#"["PUBLIC_FLAG","KEEP","MISSING"]"#,
            map(&[("PUBLIC_FLAG", "preview"), ("NEW_KEY", "value")]),
        );

        assert_eq!(
            env,
            HashMap::from([
                ("PUBLIC_FLAG".to_string(), "preview".to_string()),
                ("KEEP".to_string(), "yes".to_string()),
                ("NEW_KEY".to_string(), "value".to_string()),
            ]),
            "previews get only shared app variables (never DATABASE_URL here) plus their overrides"
        );

        let nothing_shared = preview_env_vars(map(&[("DATABASE_URL", "prod")]), "[]", map(&[]));
        assert!(
            nothing_shared.is_empty(),
            "by default a preview receives none of the production environment"
        );
    }

    #[tokio::test]
    async fn update_status_and_log_deploy_persist_rows_to_database() {
        let state = test_state().await;

        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, volumes, drain_timeout, build_method, auto_deploy, preview_enabled, preview_shared_keys, preview_env_vars, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind("app-1")
        .bind("frontend")
        .bind("git")
        .bind(3000_i64)
        .bind("pending")
        .bind("{}")
        .bind("[]")
        .bind(30_i64)
        .bind("tbuild")
        .bind(0_i64)
        .bind(0_i64)
        .bind("[]")
        .bind("{}")
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app for deploy helper tests: {err}"));

        update_status(&state, "app-1", "running")
            .await
            .unwrap_or_else(|err| panic!("update_status should succeed: {err}"));
        log_deploy(&state, "app-1", "info", "manual", "deployment started")
            .await
            .unwrap_or_else(|err| panic!("log_deploy should succeed: {err}"));

        let status: String = sqlx::query_scalar("SELECT status FROM apps WHERE id = ?")
            .bind("app-1")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to load updated app status: {err}"));
        let log_row: (String, String, String) = sqlx::query_as(
            "SELECT message, level, trigger_source FROM deploy_logs WHERE app_id = ?",
        )
        .bind("app-1")
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to load deploy log row: {err}"));

        assert_eq!(
            status, "running",
            "update_status should persist the new app status"
        );
        assert_eq!(
            log_row,
            (
                "deployment started".to_string(),
                "info".to_string(),
                "manual".to_string(),
            ),
            "log_deploy should persist the message, level, and trigger source"
        );

        log_deploy(
            &state,
            "app-1",
            "error",
            "manual",
            "fatal: could not read from https://oauth2:glpat-SECRET@gitlab.com/g/r.git",
        )
        .await
        .unwrap_or_else(|err| panic!("log_deploy should succeed: {err}"));
        let messages: Vec<String> =
            sqlx::query_scalar("SELECT message FROM deploy_logs WHERE app_id = ?")
                .bind("app-1")
                .fetch_all(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to load deploy logs: {err}"));
        assert!(
            messages
                .iter()
                .any(|m| m == "fatal: could not read from https://***@gitlab.com/g/r.git"),
            "log_deploy must redact URL credentials before storing: {messages:?}"
        );
        assert!(!messages.iter().any(|m| m.contains("glpat-SECRET")));
    }

    #[tokio::test]
    async fn resolve_git_source_moves_url_credentials_out_of_the_clone_url() {
        let state = test_state().await;
        let app = App::new(
            "private".to_string(),
            "git".to_string(),
            Some("https://deploy:tok3n@git.example.com/o/r.git".to_string()),
            None,
            3000,
        );
        let (url, auth) = resolve_git_source(
            &state,
            &app,
            app.source_url.as_deref().unwrap_or_default(),
            None,
        )
        .await
        .unwrap_or_else(|err| panic!("resolve_git_source should succeed: {err}"));
        assert_eq!(url, "https://git.example.com/o/r.git");
        let auth = auth.unwrap_or_else(|| panic!("URL credentials should become header auth"));
        assert_eq!(
            (auth.username.as_str(), auth.password.as_str()),
            ("deploy", "tok3n")
        );
    }

    #[tokio::test]
    async fn persist_generated_dockerfile_updates_deployment_row() {
        let state = test_state().await;

        sqlx::query(
            "INSERT INTO deployments (id, app_id, image_tag, status, trigger_source, generated_dockerfile, started_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("dep-1")
        .bind("app-1")
        .bind(Option::<String>::None)
        .bind("running")
        .bind("manual")
        .bind(Option::<String>::None)
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed deployment row: {err}"));

        persist_generated_dockerfile(&state, "dep-1", Some("FROM node:20-alpine")).await;

        let stored: Option<String> =
            sqlx::query_scalar("SELECT generated_dockerfile FROM deployments WHERE id = ?")
                .bind("dep-1")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to load generated Dockerfile from deployment row: {err}")
                });

        assert_eq!(
            stored.as_deref(),
            Some("FROM node:20-alpine"),
            "persist_generated_dockerfile should store the generated Dockerfile on the deployment row"
        );
    }

    #[tokio::test]
    async fn fetch_registry_credentials_returns_none_for_missing_id_and_decrypts_existing_records()
    {
        let state = test_state().await;

        assert_eq!(
            fetch_registry_credentials(&state, "missing")
                .await
                .unwrap_or_else(|err| panic!("missing credential lookup should succeed: {err}")),
            None,
            "fetch_registry_credentials should return None when the referenced credential does not exist"
        );

        let encrypted = secret_key::encrypt(Purpose::RegistryPassword, "super-secret")
            .unwrap_or_else(|err| panic!("password encryption should succeed: {err}"));
        sqlx::query(
            "INSERT INTO registry_credentials (id, name, server, username, password_enc)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind("cred-1")
        .bind("Docker Hub")
        .bind("registry.example.com")
        .bind("alice")
        .bind(&encrypted)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed registry credential: {err}"));

        let creds = fetch_registry_credentials(&state, "cred-1")
            .await
            .unwrap_or_else(|err| panic!("credential lookup should succeed: {err}"))
            .unwrap_or_else(|| panic!("expected seeded credential to be returned"));

        assert_eq!(
            creds.username.as_deref(),
            Some("alice"),
            "fetch_registry_credentials should return the stored registry username"
        );
        assert_eq!(
            creds.password.as_deref(),
            Some("super-secret"),
            "fetch_registry_credentials should decrypt the stored registry password"
        );
        assert_eq!(
            creds.serveraddress.as_deref(),
            Some("registry.example.com"),
            "fetch_registry_credentials should return the stored registry server address"
        );
    }
}
