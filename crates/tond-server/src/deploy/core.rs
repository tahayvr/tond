// deploy/core.rs — deployment orchestration
//
// This module coordinates the full lifecycle of deploying an app: from
// source code to a running Docker container. It is called from two places:
//   handlers::deploy_app()    — triggered by POST /api/apps/{id}/deploy
//   handlers::upload_deploy() — triggered after a tarball upload
//
// Public surface (re-exported from deploy/mod.rs):
//
//   deploy(state, app, trigger_source)
//     Top-level deploy function. Always runs in a background tokio task.
//
//   deploy_image(state, app, image_tag)
//     Skip the build step and re-run a specific image (used for rollbacks).
//
//   deploy_preview(state, app, preview)
//     Deploy a short-lived preview of an app.
//
// Helpers private to this module:
//   update_status()            — update app.status in DB
//   log_deploy()               — insert timestamped deploy log entry
//   wait_for_healthy()         — poll new container health endpoint
//   fetch_registry_credentials() — look up and decrypt registry credentials
//   pull_image()               — stream a docker pull with bollard

use anyhow::{Context, Result};
use std::collections::HashMap;
use tond_common::{App, PreviewDeployment, Volume};
use uuid::Uuid;

use crate::backup::{self, SnapshotTrigger};
use crate::docker;
use crate::handlers::{github_apps, gitlab_tokens};
use crate::secret_key::{self, Purpose};
use crate::state::AppState;
use crate::volumes;

use super::build::{
    BuildOptions, build_from_git, build_from_inline_dockerfile, run_inline_compose,
};

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

fn auto_domain(base_domain: &str, app_name: &str) -> String {
    let base = if base_domain.is_empty() {
        "localhost"
    } else {
        base_domain
    };
    format!("{}.{}", app_name, base)
}

fn preview_env_vars(
    base_env: &str,
    excluded_keys_json: &str,
    preview_overrides_json: &str,
) -> HashMap<String, String> {
    let mut env_vars: HashMap<String, String> = serde_json::from_str(base_env).unwrap_or_default();
    let excluded_keys: Vec<String> = serde_json::from_str(excluded_keys_json).unwrap_or_default();
    for key in &excluded_keys {
        env_vars.remove(key);
    }

    let preview_overrides: HashMap<String, String> =
        serde_json::from_str(preview_overrides_json).unwrap_or_default();
    env_vars.extend(preview_overrides);
    env_vars
}

/// Deploy an app by building source code into an image or pulling an image.
///
/// `trigger_source` records how the deployment was initiated:
///   "manual"   — via the dashboard or API
///   "webhook"  — via a GitHub/GitLab/generic webhook
///   "upload"   — via a tarball upload
pub async fn deploy(state: &AppState, app: &mut App, trigger_source: &str) -> Result<()> {
    let app_id = app.id.clone();
    let app_name = app.name.clone();

    // Remember the old container so we can stop it after a successful swap.
    let old_container_id = app.container_id.clone();

    // ── Create a deployment history row ──
    let deployment_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO deployments (id, app_id, image_tag, status, trigger_source, started_at, created_at)
         VALUES (?, ?, NULL, 'running', ?, ?, ?)",
    )
    .bind(&deployment_id)
    .bind(&app_id)
    .bind(trigger_source)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    // Update status to building
    update_status(state, &app_id, "building").await?;
    log_deploy(
        state,
        &app_id,
        "info",
        trigger_source,
        "Starting deployment...",
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
            let snap_deploy_id = deployment_id.clone();
            tokio::spawn(async move {
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

    let image_name = match app.source_type.as_str() {
        "git" => {
            let source_url = app
                .source_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Git source URL required"))?;
            let branch = app.branch.as_deref().unwrap_or("main");

            // If this app is linked to a GitHub App registration, generate a
            // short-lived installation access token and inject it into the URL.
            // Otherwise, if it is linked to a GitLab PAT, inject oauth2:<pat>@
            // into the HTTPS URL.
            let effective_url = if let Some(ref gh_app_id) = app.github_app_id {
                match sqlx::query_as::<_, tond_common::GitHubApp>(
                    "SELECT * FROM github_apps WHERE id = ? LIMIT 1",
                )
                .bind(gh_app_id)
                .fetch_optional(&state.db)
                .await
                {
                    Ok(Some(gh_app)) => {
                        match github_apps::build_clone_url_with_token(&gh_app, source_url).await {
                            Ok(url) => {
                                log_deploy(
                                    state,
                                    &app_id,
                                    "info",
                                    trigger_source,
                                    "Using GitHub App installation token for clone",
                                )
                                .await?;
                                url
                            }
                            Err(e) => {
                                log_deploy(
                                    state,
                                    &app_id,
                                    "warn",
                                    trigger_source,
                                    &format!(
                                        "GitHub App token failed ({}); falling back to plain URL",
                                        e
                                    ),
                                )
                                .await?;
                                source_url.clone()
                            }
                        }
                    }
                    _ => source_url.clone(),
                }
            } else if let Some(ref gl_token_id) = app.gitlab_token_id {
                match sqlx::query_as::<_, tond_common::GitLabToken>(
                    "SELECT * FROM gitlab_tokens WHERE id = ? LIMIT 1",
                )
                .bind(gl_token_id)
                .fetch_optional(&state.db)
                .await
                {
                    Ok(Some(gl_token)) => {
                        match gitlab_tokens::build_clone_url_with_token(
                            &gl_token, source_url, state,
                        )
                        .await
                        {
                            Ok(url) => {
                                log_deploy(
                                    state,
                                    &app_id,
                                    "info",
                                    trigger_source,
                                    "Using GitLab PAT for clone",
                                )
                                .await?;
                                url
                            }
                            Err(e) => {
                                log_deploy(
                                    state,
                                    &app_id,
                                    "warn",
                                    trigger_source,
                                    &format!(
                                        "GitLab PAT injection failed ({}); falling back to plain URL",
                                        e
                                    ),
                                )
                                .await?;
                                source_url.clone()
                            }
                        }
                    }
                    _ => source_url.clone(),
                }
            } else {
                source_url.clone()
            };

            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                &format!("Building from git: {} ({})", source_url, branch),
            )
            .await?;

            build_from_git(
                state,
                &app_id,
                &app_name,
                &effective_url,
                branch,
                &BuildOptions {
                    method: &app.build_method,
                    static_build: app.static_build != 0,
                    base_dir: app.base_dir.as_deref(),
                },
                Some(&deployment_id),
                trigger_source,
            )
            .await?
        }
        "image" => {
            let image = app
                .source_url
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Docker image reference required"))?;

            log_deploy(
                state,
                &app_id,
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
            image.clone()
        }
        "tarball" => {
            let image_tag = format!("tond-{}", app_name);
            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                &format!("Using pre-built image: {}", image_tag),
            )
            .await?;
            image_tag
        }
        "dockerfile" => {
            let content = app.dockerfile_content.as_deref().ok_or_else(|| {
                anyhow::anyhow!("dockerfile_content is required for source_type='dockerfile'")
            })?;
            persist_generated_dockerfile(state, &deployment_id, Some(content)).await;

            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                "Building from inline Dockerfile...",
            )
            .await?;

            build_from_inline_dockerfile(state, &app_id, &app_name, content, trigger_source).await?
        }
        "compose" => {
            let content = app.compose_content.as_deref().ok_or_else(|| {
                anyhow::anyhow!("compose_content is required for source_type='compose'")
            })?;

            log_deploy(
                state,
                &app_id,
                "info",
                trigger_source,
                "Starting inline Docker Compose application...",
            )
            .await?;

            run_inline_compose(state, &app_id, &app_name, content, trigger_source).await?
        }
        other => {
            anyhow::bail!("Unsupported source type: {}", other);
        }
    };

    // Record the image tag on the deployment row now that we know it.
    let _ = sqlx::query("UPDATE deployments SET image_tag = ? WHERE id = ?")
        .bind(&image_name)
        .bind(&deployment_id)
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
    if app.source_type == "compose" {
        let container_port = app.container_port as u16;

        let host_port = docker::find_compose_host_port(
            &state.docker,
            &super::build::compose_project_name(&app_name),
            container_port,
        )
        .await
        .map_err(|e| {
            tracing::error!("[compose] could not discover host port: {}", e);
            e
        })?;

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

        // Auto-assign a domain if none is set (same logic as regular deploy).
        if app.domain.is_none() {
            let base = {
                let s = state.settings.read().await;
                s.dashboard_domain.clone()
            };
            let domain = auto_domain(&base, &app_name);
            sqlx::query("UPDATE apps SET domain = ? WHERE id = ?")
                .bind(&domain)
                .bind(&app_id)
                .execute(&state.db)
                .await?;
        }

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

        let finished = chrono::Utc::now().to_rfc3339();
        let _ = sqlx::query(
            "UPDATE deployments SET status = 'succeeded', finished_at = ? WHERE id = ?",
        )
        .bind(&finished)
        .bind(&deployment_id)
        .execute(&state.db)
        .await;

        return Ok(());
    }

    // Always allocate a fresh host port for the new container. On redeployments
    // the old container still holds the old port while the new one starts up
    // (zero-downtime staging swap), so we cannot reuse the existing port here.
    // On the very first deploy there is no old container and no reserved port,
    // so this also handles that case correctly.
    let new_host_port = docker::find_free_port(&state.docker, 8100).await?;
    let container_port = app.container_port as u16;

    // Parse env vars (production deploy — no preview filtering)
    let env_vars: HashMap<String, String> = app
        .env_vars
        .as_ref()
        .and_then(|e| serde_json::from_str(e).ok())
        .unwrap_or_default();

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

    // If the app belongs to a project, ensure the project's Docker network exists
    // and connect this container to it (lazy creation on first deploy).
    let network_name = if let Some(ref pid) = app.project_id {
        let net = format!("tond-{}", pid);
        if let Err(e) = docker::ensure_network(&state.docker, &net).await {
            tracing::warn!("Could not ensure project network {}: {}", net, e);
        }
        Some(net)
    } else {
        None
    };

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
            network_name.as_deref(),
            Some(&app_name),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to start new (staging) container: {}", e);
            e
        })?
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
            network_name.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to start new container: {}", e);
            e
        })?
    };

    // ── Graceful health-check drain ──
    //
    // Poll the new container directly on its host port (bypassing the proxy)
    // so that the old container keeps serving traffic until we are confident
    // the new one is ready. On the very first deploy there is no old container
    // to keep serving, but we still run the health check so a broken first
    // deploy is caught early and the DB is never updated to "running".
    if let Some(ref path) = app.health_check_path.clone() {
        let drain_timeout = app.drain_timeout as u64;
        let url = format!("http://127.0.0.1:{}{}", new_host_port, path);

        log_deploy(
            state,
            &app_id,
            "info",
            trigger_source,
            &format!(
                "Waiting up to {}s for new container to become healthy ({})",
                drain_timeout, url
            ),
        )
        .await?;

        let healthy = wait_for_healthy(&url, drain_timeout).await?;

        if !healthy {
            // Rollback: tear down the new container, restore the old
            // container's running status so it keeps serving traffic.
            log_deploy(
                state,
                &app_id,
                "error",
                trigger_source,
                &format!(
                    "New container did not become healthy within {}s — rolling back",
                    drain_timeout
                ),
            )
            .await?;

            let _ = docker::remove_container(&state.docker, &new_container_id).await;

            // Restore appropriate status:
            // - If there was an old container it is still serving → "running"
            // - First deploy with no old container → "failed"
            let restore_status = if old_container_id.is_some() {
                "running"
            } else {
                "failed"
            };
            update_status(state, &app_id, restore_status).await?;

            // Mark deployment as failed
            let finished = chrono::Utc::now().to_rfc3339();
            let _ = sqlx::query(
                "UPDATE deployments SET status = 'failed', finished_at = ? WHERE id = ?",
            )
            .bind(&finished)
            .bind(&deployment_id)
            .execute(&state.db)
            .await;

            // Return Ok — the old container is still serving so the app is
            // healthy. The caller must not overwrite status to "failed".
            return Ok(());
        }

        log_deploy(
            state,
            &app_id,
            "info",
            trigger_source,
            "New container is healthy — cutting over traffic",
        )
        .await?;
    }

    // ── Atomic traffic cutover ──
    //
    // Update the DB in a single statement. tond-proxy reads host_port every
    // 5 s; after this write the old port stops receiving new requests.
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

    // ── Zero-downtime cutover: wait → SIGTERM old → rename staging ──
    //
    // After writing the new port to the DB the proxy needs up to PROXY_POLL_SECS
    // to pick up the change and start routing to the new container. Only then
    // can we safely SIGTERM the old one.
    //
    // On the first deploy there is no old container and the new one already has
    // the canonical name, so skip this block entirely.
    const PROXY_POLL_SECS: u64 = 3; // must be > the proxy's 2 s poll interval
    if let Some(ref old_id) = old_container_id
        && old_id != &new_container_id
    {
        log_deploy(
            state,
            &app_id,
            "info",
            trigger_source,
            &format!(
                "Waiting {}s for proxy to pick up new port before stopping old container",
                PROXY_POLL_SECS
            ),
        )
        .await?;
        tokio::time::sleep(std::time::Duration::from_secs(PROXY_POLL_SECS)).await;

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

    // Auto-assign a domain if none set.
    // Use <app-name>.<dashboard-domain> so the app gets a real, routable
    // subdomain on the same server.  Fall back to <app-name>.localhost only
    // if dashboard_domain has not been configured yet.
    if app.domain.is_none() {
        let base = {
            let s = state.settings.read().await;
            s.dashboard_domain.clone()
        };
        let domain = auto_domain(&base, &app_name);
        sqlx::query("UPDATE apps SET domain = ? WHERE id = ?")
            .bind(&domain)
            .bind(&app_id)
            .execute(&state.db)
            .await?;
    }

    log_deploy(
        state,
        &app_id,
        "info",
        trigger_source,
        &format!(
            "Deployed successfully! Container: {}",
            &new_container_id[..12.min(new_container_id.len())]
        ),
    )
    .await?;

    // Mark deployment as succeeded
    let finished = chrono::Utc::now().to_rfc3339();
    let _ =
        sqlx::query("UPDATE deployments SET status = 'succeeded', finished_at = ? WHERE id = ?")
            .bind(&finished)
            .bind(&deployment_id)
            .execute(&state.db)
            .await;

    Ok(())
}

/// Deploy from a specific Docker image tag (used for rollbacks).
/// Skips the build step and re-runs the given image directly.
pub async fn deploy_image(state: &AppState, app: &mut App, image_tag: &str) -> Result<()> {
    // Rollback is not supported for compose apps: there is no single image to
    // re-run. Re-deploy with the desired compose content instead.
    if app.source_type == "compose" {
        anyhow::bail!(
            "Rollback is not supported for compose apps. \
             Update the compose content and trigger a fresh deploy instead."
        );
    }

    let app_id = app.id.clone();
    let app_name = app.name.clone();
    let old_container_id = app.container_id.clone();

    // Deployment row
    let deployment_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO deployments (id, app_id, image_tag, status, trigger_source, started_at, created_at)
         VALUES (?, ?, ?, 'running', 'rollback', ?, ?)",
    )
    .bind(&deployment_id)
    .bind(&app_id)
    .bind(image_tag)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    update_status(state, &app_id, "building").await?;
    log_deploy(
        state,
        &app_id,
        "info",
        "rollback",
        &format!("Rolling back to image: {}", image_tag),
    )
    .await?;

    // Always allocate a fresh host port (same reasoning as in deploy()).
    // For rollbacks the old container is still running on the existing port
    // while we start the rollback container on a new port.
    let new_host_port = docker::find_free_port(&state.docker, 8100).await?;
    let container_port = app.container_port as u16;

    let env_vars: HashMap<String, String> = app
        .env_vars
        .as_ref()
        .and_then(|e| serde_json::from_str(e).ok())
        .unwrap_or_default();

    // Host paths are re-derived from volume names and verified to stay inside
    // <data>/volumes/<app> (no symlink escapes) right before the mount.
    let volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
    let binds = volumes::app_binds(&volumes::data_dir(), &app_name, &volumes)?;

    log_deploy(
        state,
        &app_id,
        "info",
        "rollback",
        &format!(
            "Starting rollback container on port {}:{}",
            new_host_port, container_port
        ),
    )
    .await?;

    // Connect to project network if applicable (same as regular deploy).
    let network_name = if let Some(ref pid) = app.project_id {
        let net = format!("tond-{}", pid);
        if let Err(e) = docker::ensure_network(&state.docker, &net).await {
            tracing::warn!("Could not ensure project network {}: {}", net, e);
        }
        Some(net)
    } else {
        None
    };

    // Use staging name for rollbacks when there is an existing container,
    // so traffic continues to flow to the old container until the rollback
    // container is healthy and the DB has been updated.
    let staging_name_rb = format!("tond-{}-next", app_name);
    let canonical_name_rb = format!("tond-{}", app_name);

    let new_container_id = if old_container_id.is_some() {
        docker::run_container_named(
            &state.docker,
            image_tag,
            &staging_name_rb,
            container_port,
            new_host_port,
            docker::Exposure::Loopback,
            env_vars,
            app.memory_limit,
            app.cpu_limit,
            binds,
            network_name.as_deref(),
            Some(&app_name),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to start rollback (staging) container: {}", e);
            e
        })?
    } else {
        docker::run_container(
            &state.docker,
            image_tag,
            &app_name,
            container_port,
            new_host_port,
            docker::Exposure::Loopback,
            env_vars,
            app.memory_limit,
            app.cpu_limit,
            binds,
            network_name.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to start rollback container: {}", e);
            e
        })?
    };

    // Graceful drain (same logic as regular deploy)
    if let Some(ref path) = app.health_check_path.clone() {
        let drain_timeout = app.drain_timeout as u64;
        let url = format!("http://127.0.0.1:{}{}", new_host_port, path);

        log_deploy(
            state,
            &app_id,
            "info",
            "rollback",
            &format!(
                "Waiting up to {}s for rollback container to become healthy ({})",
                drain_timeout, url
            ),
        )
        .await?;

        let healthy = wait_for_healthy(&url, drain_timeout).await?;
        if !healthy {
            log_deploy(
                state,
                &app_id,
                "error",
                "rollback",
                &format!(
                    "Rollback container did not become healthy within {}s",
                    drain_timeout
                ),
            )
            .await?;
            let _ = docker::remove_container(&state.docker, &new_container_id).await;
            let restore_status = if old_container_id.is_some() {
                "running"
            } else {
                "failed"
            };
            update_status(state, &app_id, restore_status).await?;

            let finished = chrono::Utc::now().to_rfc3339();
            let _ = sqlx::query(
                "UPDATE deployments SET status = 'failed', finished_at = ? WHERE id = ?",
            )
            .bind(&finished)
            .bind(&deployment_id)
            .execute(&state.db)
            .await;

            anyhow::bail!(
                "Rollback failed: container did not pass health checks within {}s",
                drain_timeout
            );
        }

        log_deploy(
            state,
            &app_id,
            "info",
            "rollback",
            "Rollback container is healthy — cutting over traffic",
        )
        .await?;
    }

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

    if let Some(ref old_id) = old_container_id
        && old_id != &new_container_id
    {
        const PROXY_POLL_SECS: u64 = 3;
        log_deploy(
            state,
            &app_id,
            "info",
            "rollback",
            &format!(
                "Waiting {}s for proxy to pick up new port before stopping old container",
                PROXY_POLL_SECS
            ),
        )
        .await?;
        tokio::time::sleep(std::time::Duration::from_secs(PROXY_POLL_SECS)).await;

        log_deploy(
            state,
            &app_id,
            "info",
            "rollback",
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

        // Rename staging → canonical
        let _ = docker::rename_container(&state.docker, &staging_name_rb, &canonical_name_rb).await;
    }

    log_deploy(
        state,
        &app_id,
        "info",
        "rollback",
        &format!(
            "Rollback succeeded! Container: {}",
            &new_container_id[..12.min(new_container_id.len())]
        ),
    )
    .await?;

    let finished = chrono::Utc::now().to_rfc3339();
    let _ =
        sqlx::query("UPDATE deployments SET status = 'succeeded', finished_at = ? WHERE id = ?")
            .bind(&finished)
            .bind(&deployment_id)
            .execute(&state.db)
            .await;

    Ok(())
}

/// Deploy a preview of an app with a unique subdomain and TTL.
///
/// Steps:
///   1. Build/pull the image (same as a regular deploy).
///   2. Pick a free host port (starting at 8200 to keep separate from main deploys).
///   3. Start the container as `tond-preview-{slug}` with the same env vars and volumes.
///   4. Insert a `preview_deployments` row and update it once the container is running.
///
/// The caller must provide the `slug` (short random hex) and the `ttl_hours`.
/// The `subdomain` is set by the caller after reading the dashboard domain from settings.
pub async fn deploy_preview(
    state: &AppState,
    app: &App,
    preview: &mut PreviewDeployment,
) -> Result<()> {
    let app_id = app.id.clone();
    let app_name = app.name.clone();
    let slug = preview.slug.clone();

    tracing::info!("Starting preview deploy for {} (slug: {})", app_name, slug);

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

            tracing::info!("Preview: cloning git {} ({})", source_url, branch);

            // Use GitHub App token if the app is linked to one, otherwise
            // use GitLab PAT if linked.
            let effective_url = if let Some(ref gh_app_id) = app.github_app_id {
                match sqlx::query_as::<_, tond_common::GitHubApp>(
                    "SELECT * FROM github_apps WHERE id = ? LIMIT 1",
                )
                .bind(gh_app_id)
                .fetch_optional(&state.db)
                .await
                {
                    Ok(Some(gh_app)) => {
                        match github_apps::build_clone_url_with_token(&gh_app, source_url).await {
                            Ok(url) => url,
                            Err(_) => source_url.clone(),
                        }
                    }
                    _ => source_url.clone(),
                }
            } else if let Some(ref gl_token_id) = app.gitlab_token_id {
                match sqlx::query_as::<_, tond_common::GitLabToken>(
                    "SELECT * FROM gitlab_tokens WHERE id = ? LIMIT 1",
                )
                .bind(gl_token_id)
                .fetch_optional(&state.db)
                .await
                {
                    Ok(Some(gl_token)) => {
                        match gitlab_tokens::build_clone_url_with_token(
                            &gl_token, source_url, state,
                        )
                        .await
                        {
                            Ok(url) => url,
                            Err(_) => source_url.clone(),
                        }
                    }
                    _ => source_url.clone(),
                }
            } else {
                source_url.clone()
            };

            build_from_git(
                state,
                &app_id,
                &app_name,
                &effective_url,
                branch,
                &BuildOptions {
                    method: &app.build_method,
                    static_build: app.static_build != 0,
                    base_dir: app.base_dir.as_deref(),
                },
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
        "tarball" => {
            format!("tond-{}", app_name)
        }
        "dockerfile" => {
            let content = app.dockerfile_content.as_deref().ok_or_else(|| {
                anyhow::anyhow!("dockerfile_content required for source_type='dockerfile'")
            })?;
            build_from_inline_dockerfile(state, &app_id, &app_name, content, "preview").await?
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

    preview.image_tag = Some(image_name.clone());

    // Update image_tag in DB now that we know it
    let _ = sqlx::query("UPDATE preview_deployments SET image_tag = ? WHERE id = ?")
        .bind(&image_name)
        .bind(&preview.id)
        .execute(&state.db)
        .await;

    // Pick a free port in the preview range (8200+)
    let host_port = docker::find_free_port(&state.docker, 8200).await?;
    let container_port = app.container_port as u16;

    let env_vars = preview_env_vars(
        app.env_vars.as_deref().unwrap_or("{}"),
        app.preview_excluded_keys.as_str(),
        app.preview_env_vars.as_str(),
    );

    // Host paths are re-derived from volume names and verified to stay inside
    // <data>/volumes/<app> (no symlink escapes) right before the mount.
    let volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
    let binds = volumes::app_binds(&volumes::data_dir(), &app_name, &volumes)?;

    let container_name = format!("tond-preview-{}", slug);

    tracing::info!(
        "Preview: starting container {} on port {}:{}",
        container_name,
        host_port,
        container_port
    );

    // Connect preview containers to the project network if the app has one.
    let network_name = if let Some(ref pid) = app.project_id {
        let net = format!("tond-{}", pid);
        if let Err(e) = docker::ensure_network(&state.docker, &net).await {
            tracing::warn!("Could not ensure project network {}: {}", net, e);
        }
        Some(net)
    } else {
        None
    };

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
        network_name.as_deref(),
        Some(&app_name),
    )
    .await?;

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

/// Poll `url` once per second until it returns a 2xx status code.
/// Returns `true` if healthy within `timeout_secs`, `false` if timed out.
async fn wait_for_healthy(url: &str, timeout_secs: u64) -> anyhow::Result<bool> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .context("building reqwest client for health check")?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if let Ok(resp) = client.get(url).send().await
            && resp.status().is_success()
        {
            return Ok(true);
        }

        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
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
async fn fetch_registry_credentials(
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
async fn pull_image(
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

/// Helper: insert a deploy log entry
pub(super) async fn log_deploy(
    state: &AppState,
    app_id: &str,
    level: &str,
    trigger_source: &str,
    message: &str,
) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO deploy_logs (id, app_id, message, level, trigger_source, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(app_id)
    .bind(message)
    .bind(level)
    .bind(trigger_source)
    .bind(&now)
    .execute(&state.db)
    .await?;

    match level {
        "error" => tracing::error!("[{}] {}", app_id, message),
        "warn" => tracing::warn!("[{}] {}", app_id, message),
        _ => tracing::info!("[{}] {}", app_id, message),
    }

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
    fn auto_domain_uses_dashboard_domain_or_localhost_fallback() {
        assert_eq!(
            auto_domain("dashboard.example.com", "frontend"),
            "frontend.dashboard.example.com",
            "auto_domain should suffix the app name with the configured dashboard domain"
        );
        assert_eq!(
            auto_domain("", "frontend"),
            "frontend.localhost",
            "auto_domain should fall back to localhost when the dashboard domain is not configured"
        );
    }

    #[test]
    fn preview_env_vars_removes_excluded_keys_and_applies_overrides() {
        let env = preview_env_vars(
            r#"{"DATABASE_URL":"prod","PUBLIC_FLAG":"1","KEEP":"yes"}"#,
            r#"["DATABASE_URL","MISSING"]"#,
            r#"{"PUBLIC_FLAG":"preview","NEW_KEY":"value"}"#,
        );

        assert_eq!(
            env,
            HashMap::from([
                ("PUBLIC_FLAG".to_string(), "preview".to_string()),
                ("KEEP".to_string(), "yes".to_string()),
                ("NEW_KEY".to_string(), "value".to_string()),
            ]),
            "preview_env_vars should remove excluded keys and apply preview-specific overrides on top of the base environment"
        );
    }

    #[tokio::test]
    async fn update_status_and_log_deploy_persist_rows_to_database() {
        let state = test_state().await;

        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, volumes, drain_timeout, build_method, auto_deploy, preview_enabled, preview_excluded_keys, preview_env_vars, static_build, created_at, updated_at)
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
