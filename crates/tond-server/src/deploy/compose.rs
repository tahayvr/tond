// deploy/compose.rs — the deployment pipeline of a Docker Compose app
//
// Runs inside deploy::run (queue slot, phases, failure classification and
// status handling are shared with single-container apps):
//
//   clone    git-sourced compose apps: clone the repository and read the
//            compose file at `compose_path`; inline apps use compose_content;
//            rollbacks use the compose file of the target deployment
//   build    validate + harden (compose_policy), sync compose_services, then
//            produce every service's image: `docker build --cgroup-parent`
//            (the build memory cap) or a registry pull, pinned as
//            tond-<app>/<service>:<image id prefix>. Rollbacks reuse the target
//            deployment's pinned images.
//   start    allocate a stable loopback host port for each published service
//            port, write the pinned file, `docker compose up -d
//            --remove-orphans`
//   health   wait for compose healthchecks (healthy), for every service to
//            run (one-shot services may exit 0), then probe each published
//            service like a single-container app (http: health check path or
//            any answer; tcp: the port accepts connections)
//   release  record the deployment's images and compose file, mark the app
//            running (routes follow compose_services.host_port and
//            app_domains), assign the automatic domain to the web service,
//            wait for tond-proxy to route, prune old images
//
// Compose recreates only services whose image or config changed, so an
// unchanged service keeps serving through a redeploy.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tond_common::{App, ComposeServiceSpec, DeployPhase, DeploymentService, Readiness};

use crate::compose::{self, Pin};
use crate::runtime::shutdown;
use crate::state::AppState;
use crate::{compose_policy, docker, volumes};

use super::build::{
    build_compose_service, clone_repo, compose_file_path, compose_project_name, compose_up,
    compose_work_dir,
};
use super::core::{
    DeployKind, DeployRequest, HealthFacts, auto_domain, container_exit,
    fetch_registry_credentials, log_deploy, probe_tcp_until_ready, probe_until_ready, pull_image,
    resolve_git_source, strip_docker_ts, wait_for_proxy_cutover,
};
use super::failure::ProbeResult;
use super::queue::{self, DeployCtx};
use super::{images, progress};

/// Compose host ports are allocated from here up.
const COMPOSE_PORT_BASE: u16 = 8300;
/// Shortest wait for services to become ready.
const READY_MIN_SECS: u64 = 60;

/// Where the compose file of this deployment comes from.
struct Source {
    content: String,
    /// Directory relative build contexts resolve against.
    base_dir: PathBuf,
    /// Checkout to remove afterwards (git-sourced apps).
    checkout: Option<String>,
    /// Rollback: the target deployment's pinned images per service.
    reuse: Option<HashMap<String, DeploymentService>>,
}

/// Run the compose pipeline for `app` (see module docs).
pub(super) async fn pipeline(
    state: &AppState,
    app: &mut App,
    deployment_id: &str,
    req: &DeployRequest,
    facts: &mut HealthFacts,
    ctx: &DeployCtx,
) -> Result<()> {
    let source = fetch_source(state, app, deployment_id, req).await?;
    let result = run_with_source(state, app, deployment_id, req, facts, ctx, &source).await;
    if let Some(dir) = &source.checkout {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
    result
}

async fn fetch_source(
    state: &AppState,
    app: &App,
    deployment_id: &str,
    req: &DeployRequest,
) -> Result<Source> {
    let trigger = req.trigger_source.as_str();
    let data_dir = volumes::data_dir();
    let work_dir = PathBuf::from(compose_work_dir(&data_dir.to_string_lossy(), &app.name));
    match &req.kind {
        DeployKind::Upload { .. } => {
            anyhow::bail!(
                "Compose apps can't be deployed from an upload; edit the compose file instead"
            )
        }
        DeployKind::Rollback {
            target_deployment_id,
            ..
        } => {
            let content: Option<String> = sqlx::query_scalar(
                "SELECT compose_source FROM deployments WHERE id = ? AND app_id = ?",
            )
            .bind(target_deployment_id)
            .bind(&app.id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
            let content = content.ok_or_else(|| {
                anyhow::anyhow!(
                    "This deployment has no recorded compose file, so it can't be restored"
                )
            })?;
            let services = deployment_services(&state.db, target_deployment_id).await?;
            for svc in &services {
                if let Some(image) = &svc.image
                    && state.docker.inspect_image(image).await.is_err()
                {
                    images::mark_compose_release_pruned(state, &app.id, target_deployment_id).await;
                    anyhow::bail!(
                        "Image {image} of service \"{}\" is no longer on this server (old images are pruned), so this deployment can't be restored",
                        svc.service
                    );
                }
            }
            log_deploy(
                state,
                &app.id,
                "info",
                trigger,
                &format!(
                    "Restoring the compose file and {} service image(s) of deployment {}",
                    services.len(),
                    &target_deployment_id[..8.min(target_deployment_id.len())]
                ),
            )
            .await?;
            Ok(Source {
                content,
                base_dir: work_dir,
                checkout: None,
                reuse: Some(
                    services
                        .into_iter()
                        .map(|s| (s.service.clone(), s))
                        .collect(),
                ),
            })
        }
        DeployKind::Source => match app.source_url.as_deref().filter(|u| !u.trim().is_empty()) {
            Some(url) => {
                progress::start(state, &app.id, DeployPhase::Clone).await;
                let branch = app.branch.as_deref().unwrap_or("main");
                let (clone_url, auth) = resolve_git_source(state, app, url, Some(trigger)).await?;
                let checkout = clone_repo(
                    state,
                    &app.id,
                    &app.name,
                    &clone_url,
                    auth.as_ref(),
                    branch,
                    Some(deployment_id),
                    trigger,
                )
                .await?;
                let rel = compose_path_in_repo(app.compose_path.as_deref())?;
                let file = Path::new(&checkout).join(&rel);
                let content = tokio::fs::read_to_string(&file).await.with_context(|| {
                    format!("the repository has no compose file at {}", rel.display())
                })?;
                let base_dir = file
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(&checkout));
                log_deploy(
                    state,
                    &app.id,
                    "info",
                    trigger,
                    &format!("Using {} from the repository", rel.display()),
                )
                .await?;
                Ok(Source {
                    content,
                    base_dir,
                    checkout: Some(checkout),
                    reuse: None,
                })
            }
            None => Ok(Source {
                content: app
                    .compose_content
                    .clone()
                    .filter(|c| !c.trim().is_empty())
                    .ok_or_else(|| anyhow::anyhow!("This compose app has no compose file"))?,
                base_dir: work_dir,
                checkout: None,
                reuse: None,
            }),
        },
    }
}

/// Validate a compose file path inside a repository (relative, no `..`).
pub fn compose_path_in_repo(path: Option<&str>) -> Result<PathBuf> {
    let path = path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(compose::DEFAULT_COMPOSE_PATH);
    let rel = super::build::validate_base_dir(path)
        .map_err(|e| anyhow::anyhow!("compose file path: {e}"))?;
    if rel.is_empty() {
        anyhow::bail!("compose file path must name a file in the repository");
    }
    Ok(PathBuf::from(rel))
}

pub async fn deployment_services(
    db: &sqlx::SqlitePool,
    deployment_id: &str,
) -> sqlx::Result<Vec<DeploymentService>> {
    sqlx::query_as(
        "SELECT service, source, source_image, image, digest, status FROM deployment_services
         WHERE deployment_id = ? ORDER BY service",
    )
    .bind(deployment_id)
    .fetch_all(db)
    .await
}

async fn record_service(
    state: &AppState,
    app_id: &str,
    deployment_id: &str,
    svc: &DeploymentService,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO deployment_services (deployment_id, service, source, source_image, image, digest, status)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(deployment_id, service) DO UPDATE SET source = excluded.source,
           source_image = excluded.source_image, image = excluded.image,
           digest = excluded.digest, status = excluded.status",
    )
    .bind(deployment_id)
    .bind(&svc.service)
    .bind(&svc.source)
    .bind(&svc.source_image)
    .bind(&svc.image)
    .bind(&svc.digest)
    .bind(&svc.status)
    .execute(&state.db)
    .await?;
    crate::log_bus::publish_deployment(app_id, deployment_id);
    Ok(())
}

async fn set_services_status(
    state: &AppState,
    app_id: &str,
    deployment_id: &str,
    services: &[&str],
    status: &str,
) {
    for s in services {
        let _ = sqlx::query(
            "UPDATE deployment_services SET status = ? WHERE deployment_id = ? AND service = ?",
        )
        .bind(status)
        .bind(deployment_id)
        .bind(s)
        .execute(&state.db)
        .await;
    }
    crate::log_bus::publish_deployment(app_id, deployment_id);
}

/// Pin `source` (a built or pulled image) under its content tag
/// (compose::content_tag) and return that tag.
async fn pin_by_content(
    state: &AppState,
    app_name: &str,
    service: &str,
    source: &str,
) -> Result<String> {
    let info = state
        .docker
        .inspect_image(source)
        .await
        .with_context(|| format!("service \"{service}\": image {source} is missing"))?;
    let id = info.id.unwrap_or_default();
    let tag = compose::content_tag(app_name, service, &id);
    images::tag_image(&state.docker, source, &tag)
        .await
        .with_context(|| format!("service \"{service}\": could not pin {source}"))?;
    Ok(tag)
}

/// Image id of `image` plus the registry digest of a pulled image.
async fn image_digest(state: &AppState, image: &str) -> Option<String> {
    let info = state.docker.inspect_image(image).await.ok()?;
    info.repo_digests
        .and_then(|d| d.into_iter().next())
        .or(info.id)
}

#[allow(clippy::too_many_arguments)]
async fn run_with_source(
    state: &AppState,
    app: &mut App,
    deployment_id: &str,
    req: &DeployRequest,
    facts: &mut HealthFacts,
    ctx: &DeployCtx,
    source: &Source,
) -> Result<()> {
    let app_id = app.id.clone();
    let app_name = app.name.clone();
    let trigger = req.trigger_source.as_str();
    let data_dir = volumes::data_dir();
    let work_dir = compose_work_dir(&data_dir.to_string_lossy(), &app_name);

    // ── Build: validate, sync services, produce images ──
    progress::start(state, &app_id, DeployPhase::Build).await;
    let volume_dir = volumes::app_volume_dir(&data_dir, &app_name);
    let hardened =
        compose_policy::harden(&source.content, &volume_dir, docker::default_memory_limit())
            .map_err(|errors| anyhow::anyhow!(compose_policy::format_errors(&errors)))?;
    for rel in &hardened.bind_sources {
        volumes::prepare_host_dir(&data_dir, &volume_dir, rel, true)?;
    }
    // The app's environment variables are what `${VAR}` in the file resolves
    // against (Tond's own environment never reaches compose).
    // (compose_policy::interpolation_env picks the variables the file uses
    // and never reserved names; the same set reaches `docker compose`.)
    let interpolation =
        compose_policy::interpolation_env(&source.content, &crate::secrets::app_env(app)?);
    let app_env: std::collections::BTreeMap<String, String> =
        interpolation.iter().cloned().collect();
    let specs =
        compose::service_specs_env(&source.content, &app_env).map_err(anyhow::Error::msg)?;
    compose::sync_specs(&state.db, &app_id, &specs).await?;
    // Services in a profile (or scaled to zero) are not started: nothing to
    // build, pull or wait for.
    let inactive = compose::inactive_services(&source.content);
    let specs: Vec<ComposeServiceSpec> = specs
        .into_iter()
        .filter(|s| !inactive.contains(&s.name))
        .collect();
    ensure_web_service(state, app, &specs).await?;

    let builds = compose::build_specs(&hardened.yaml, &source.base_dir, &app_env);
    let credentials = match &app.registry_credential_id {
        Some(id) => fetch_registry_credentials(state, id).await?,
        None => None,
    };
    let mut pinned: HashMap<String, String> = HashMap::new();
    for spec in &specs {
        let name = spec.name.as_str();
        // Built under a per-deployment work tag, then pinned by content.
        let work_tag = compose::pinned_tag(&app_name, name, deployment_id);
        if let Some(reuse) = &source.reuse {
            let Some(prev) = reuse.get(name) else {
                continue;
            };
            let Some(image) = prev.image.clone() else {
                continue;
            };
            record_service(
                state,
                &app_id,
                deployment_id,
                &DeploymentService {
                    service: name.to_string(),
                    source: "reuse".into(),
                    source_image: prev.source_image.clone(),
                    image: Some(image.clone()),
                    digest: prev.digest.clone(),
                    status: "ready".into(),
                },
            )
            .await?;
            pinned.insert(name.to_string(), image);
            continue;
        }
        if let Some(build) = builds.get(name) {
            let mut row = DeploymentService {
                service: name.to_string(),
                source: "build".into(),
                source_image: Some(build.context.clone()),
                image: None,
                digest: None,
                status: "building".into(),
            };
            record_service(state, &app_id, deployment_id, &row).await?;
            log_deploy(
                state,
                &app_id,
                "info",
                trigger,
                &format!("[{name}] Building image"),
            )
            .await?;
            let built = match build_compose_service(
                state, &app_id, name, &work_tag, build, &work_dir, trigger,
            )
            .await
            {
                Ok(()) => pin_by_content(state, &app_name, name, &work_tag).await,
                Err(e) => Err(e),
            };
            // The work tag has served its purpose (the content tag stays).
            let _ = state
                .docker
                .remove_image(
                    &work_tag,
                    None::<bollard::query_parameters::RemoveImageOptions>,
                    None,
                )
                .await;
            let tag = match built {
                Ok(tag) => tag,
                Err(e) => {
                    row.status = "failed".into();
                    let _ = record_service(state, &app_id, deployment_id, &row).await;
                    return Err(e);
                }
            };
            row.image = Some(tag.clone());
            row.digest = image_digest(state, &tag).await;
            row.status = "ready".into();
            record_service(state, &app_id, deployment_id, &row).await?;
            pinned.insert(name.to_string(), tag);
        } else if let Some(image) = &spec.image {
            let mut row = DeploymentService {
                service: name.to_string(),
                source: "pull".into(),
                source_image: Some(image.clone()),
                image: None,
                digest: None,
                status: "pulling".into(),
            };
            record_service(state, &app_id, deployment_id, &row).await?;
            log_deploy(
                state,
                &app_id,
                "info",
                trigger,
                &format!("[{name}] Pulling {image}"),
            )
            .await?;
            let pulled = pull_image(&state.docker, image, credentials.clone())
                .await
                .with_context(|| format!("service \"{name}\": could not pull {image}"));
            let pulled = match pulled {
                Ok(()) => {
                    pin_by_content(
                        state,
                        &app_name,
                        name,
                        &super::core::with_default_tag(image),
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            let tag = match pulled {
                Ok(tag) => tag,
                Err(e) => {
                    row.status = "failed".into();
                    let _ = record_service(state, &app_id, deployment_id, &row).await;
                    return Err(e);
                }
            };
            row.image = Some(tag.clone());
            row.digest = image_digest(state, &tag).await;
            row.status = "ready".into();
            record_service(state, &app_id, deployment_id, &row).await?;
            log_deploy(
                state,
                &app_id,
                "info",
                trigger,
                &format!(
                    "[{name}] Pinned {image} as {tag}{}",
                    row.digest
                        .as_deref()
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default()
                ),
            )
            .await?;
            pinned.insert(name.to_string(), tag);
        }
    }
    // Every image exists: give the build slot to the next deployment.
    ctx.release_build();

    // ── Start: host ports, pinned file, compose up ──
    progress::start(state, &app_id, DeployPhase::Start).await;
    let project = compose_project_name(&app_name);
    let port_guard = queue::port_allocation().await;
    let publish = allocate_host_ports(state, &app_id).await?;
    let pins: Vec<Pin> = specs
        .iter()
        .map(|s| Pin {
            service: s.name.clone(),
            image: pinned.get(&s.name).cloned(),
            publish: publish.get(&s.name).copied(),
        })
        .collect();
    let file = compose::pin_file(&hardened.yaml, &pins).map_err(anyhow::Error::msg)?;
    tokio::fs::create_dir_all(&work_dir).await?;
    let compose_path = compose_file_path(&work_dir);
    tokio::fs::write(&compose_path, &file).await?;
    for (service, (host, container)) in &publish {
        log_deploy(
            state,
            &app_id,
            "info",
            trigger,
            &format!("[{service}] Publishing container port {container} on 127.0.0.1:{host}"),
        )
        .await?;
    }
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    set_services_status(state, &app_id, deployment_id, &names, "starting").await;
    log_deploy(
        state,
        &app_id,
        "info",
        trigger,
        "Starting services with docker compose up...",
    )
    .await?;
    let up = compose_up(
        state,
        &app_id,
        &app_name,
        &compose_path,
        &interpolation,
        trigger,
    )
    .await;
    drop(port_guard);
    if let Err(e) = up {
        set_services_status(state, &app_id, deployment_id, &names, "failed").await;
        return Err(e);
    }

    // ── Health ──
    progress::start(state, &app_id, DeployPhase::Health).await;
    let timeout = (app.drain_timeout.max(1) as u64).max(READY_MIN_SECS);
    facts.timeout_secs = timeout;
    if let Err(e) = wait_until_ready(
        state,
        app,
        deployment_id,
        &project,
        &specs,
        &publish,
        timeout,
        facts,
        trigger,
    )
    .await
    {
        let _ = compose::refresh(state, &app_id, &app_name).await;
        return Err(e);
    }

    // ── Release ──
    progress::start(state, &app_id, DeployPhase::Release).await;
    let release = images::deployment_tag(&app_name, deployment_id);
    let mut tx = state.db.begin().await?;
    for spec in &specs {
        let digest: Option<String> = sqlx::query_scalar(
            "SELECT digest FROM deployment_services WHERE deployment_id = ? AND service = ?",
        )
        .bind(deployment_id)
        .bind(&spec.name)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        sqlx::query(
            "UPDATE compose_services SET running_image = ?, image_digest = ?,
               routed_port = CASE WHEN host_port IS NULL THEN NULL ELSE published_port END
             WHERE app_id = ? AND name = ?",
        )
        .bind(pinned.get(&spec.name))
        .bind(digest)
        .bind(&app_id)
        .bind(&spec.name)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE deployments SET compose_source = ?, image_tag = COALESCE(image_tag, ?) WHERE id = ?",
    )
    .bind(&source.content)
    .bind(&release)
    .bind(deployment_id)
    .execute(&mut *tx)
    .await?;
    // A rollback restores the compose file too (inline apps).
    if source.reuse.is_some() && app.source_url.is_none() {
        sqlx::query("UPDATE apps SET compose_content = ? WHERE id = ?")
            .bind(&source.content)
            .bind(&app_id)
            .execute(&mut *tx)
            .await?;
        app.compose_content = Some(source.content.clone());
    }
    // The app's own domain/host_port are not routed for compose apps: routes
    // are the service domains (app_domains + compose_services.host_port).
    let running_release: Option<String> =
        sqlx::query_scalar("SELECT image_tag FROM deployments WHERE id = ?")
            .bind(deployment_id)
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
    let running_release = running_release.unwrap_or(release);
    sqlx::query(
        "UPDATE apps SET container_id = ?, host_port = NULL, status = 'running',
           updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&running_release)
    .bind(&app_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    app.container_id = Some(running_release);
    app.host_port = None;
    app.status = "running".into();
    set_services_status(state, &app_id, deployment_id, &names, "running").await;

    assign_service_domain(state, app, trigger).await?;
    let _ = compose::refresh(state, &app_id, &app_name).await;
    if !publish.is_empty() {
        wait_for_proxy_cutover(state, &app_id, trigger).await?;
    }
    log_deploy(
        state,
        &app_id,
        "info",
        trigger,
        &format!(
            "{} {} service(s) running",
            if source.reuse.is_some() {
                "Rollback succeeded!"
            } else {
                "Deployed successfully!"
            },
            specs.len()
        ),
    )
    .await?;
    images::prune_app_images(state, &app_id, &app_name).await;
    Ok(())
}

/// Make sure a web service is published when none is: the app's first
/// deploy picks one (the file's service declaring the app's port, else the
/// first service declaring a port) and moves a domain set at creation onto it.
async fn ensure_web_service(
    state: &AppState,
    app: &mut App,
    specs: &[ComposeServiceSpec],
) -> Result<()> {
    let published: Option<String> = sqlx::query_scalar(
        "SELECT name FROM compose_services WHERE app_id = ? AND published_port IS NOT NULL LIMIT 1",
    )
    .bind(&app.id)
    .fetch_optional(&state.db)
    .await?;
    let web = match published {
        Some(name) => Some(name),
        None => {
            let port = u16::try_from(app.container_port).unwrap_or(80);
            match compose::pick_web_service(specs, None, port) {
                Some((service, port)) => {
                    sqlx::query(
                        "UPDATE compose_services SET published_port = ? WHERE app_id = ? AND name = ?",
                    )
                    .bind(port as i64)
                    .bind(&app.id)
                    .bind(&service)
                    .execute(&state.db)
                    .await?;
                    Some(service)
                }
                None => None,
            }
        }
    };
    // Domains that name no service (the address given at creation, or one
    // added before the services were known) belong to the web service.
    if let Some(service) = web {
        let port: Option<i64> = sqlx::query_scalar(
            "SELECT published_port FROM compose_services WHERE app_id = ? AND name = ?",
        )
        .bind(&app.id)
        .bind(&service)
        .fetch_optional(&state.db)
        .await?
        .flatten();
        if let Some(port) = port {
            crate::app_domains::attach_to_service(&state.db, &app.id, &service, port).await?;
        }
    }
    Ok(())
}

/// Give the web service `<app>.<apps domain>` when the app has no domain at
/// all yet.
async fn assign_service_domain(state: &AppState, app: &App, trigger: &str) -> Result<()> {
    let has_domain: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_domains WHERE app_id = ?)")
            .bind(&app.id)
            .fetch_one(&state.db)
            .await?;
    if has_domain {
        return Ok(());
    }
    let web: Option<(String, i64)> = sqlx::query_as(
        "SELECT name, published_port FROM compose_services
         WHERE app_id = ? AND published_port IS NOT NULL AND host_port IS NOT NULL
         ORDER BY position LIMIT 1",
    )
    .bind(&app.id)
    .fetch_optional(&state.db)
    .await?;
    let Some((service, port)) = web else {
        return Ok(());
    };
    let apps_domain = state.settings.read().await.apps_domain.clone();
    let Some(domain) = auto_domain(&apps_domain, &app.name) else {
        log_deploy(
            state,
            &app.id,
            "info",
            trigger,
            "No apps domain is set, so this app has no public address yet. Add a domain to a \
             service on the app's page, or set an apps domain in Settings.",
        )
        .await?;
        return Ok(());
    };
    if !crate::app_domains::assign_automatic_to_service(&state.db, &app.id, &domain, &service, port)
        .await?
    {
        return Ok(()); // the name is taken by another app
    }
    shutdown::spawn_tracked(crate::proxy_link::request_certs(Some(domain.clone())));
    log_deploy(
        state,
        &app.id,
        "info",
        trigger,
        &format!("[{service}] Reachable at https://{domain}"),
    )
    .await?;
    Ok(())
}

/// Allocate (or keep) a loopback host port for every published service
/// port. A port stays the same across deploys unless another container took
/// it. Returns `service -> (host port, container port)`.
async fn allocate_host_ports(
    state: &AppState,
    app_id: &str,
) -> Result<HashMap<String, (u16, u16)>> {
    let rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT name, published_port, host_port FROM compose_services
         WHERE app_id = ? AND published_port IS NOT NULL ORDER BY position",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    // Ports live containers publish (this app's own included: a port the
    // file maps itself must not be handed out again) and ports stored for
    // other apps whose containers may be down right now.
    let mut used = docker::used_host_ports(&state.docker, None).await?;
    let reserved: Vec<i64> = sqlx::query_scalar(
        "SELECT host_port FROM compose_services WHERE app_id != ? AND host_port IS NOT NULL
         UNION SELECT host_port FROM apps WHERE host_port IS NOT NULL AND id != ?",
    )
    .bind(app_id)
    .bind(app_id)
    .fetch_all(&state.db)
    .await?;
    used.extend(reserved.into_iter().filter_map(|p| u16::try_from(p).ok()));
    let plan = plan_host_ports(&rows, used)?;
    for (name, (host, _)) in &plan {
        sqlx::query("UPDATE compose_services SET host_port = ? WHERE app_id = ? AND name = ?")
            .bind(*host as i64)
            .bind(app_id)
            .bind(name)
            .execute(&state.db)
            .await?;
    }
    Ok(plan)
}

/// Host ports for `(service, published port, stored host port)` rows: a
/// service keeps its stored port (its own container holds it, so it is
/// taken out of `used` first) unless something else uses it; new ones get
/// the first free port from COMPOSE_PORT_BASE.
pub fn plan_host_ports(
    rows: &[(String, i64, Option<i64>)],
    mut used: Vec<u16>,
) -> Result<HashMap<String, (u16, u16)>> {
    let own: Vec<u16> = rows
        .iter()
        .filter_map(|(_, _, h)| h.and_then(|h| u16::try_from(h).ok()))
        .collect();
    // Each own port once: a second occurrence belongs to something else.
    for port in &own {
        if let Some(i) = used.iter().position(|p| p == port) {
            used.remove(i);
        }
    }
    let mut out = HashMap::new();
    for (name, container, host) in rows {
        let container = u16::try_from(*container).context("published port out of range")?;
        let host = match host.and_then(|h| u16::try_from(h).ok()) {
            Some(h) if !used.contains(&h) => h,
            _ => docker::first_free_port(COMPOSE_PORT_BASE, &used)?,
        };
        used.push(host);
        out.insert(name.clone(), (host, container));
    }
    Ok(out)
}

/// Readiness of one service as seen in a `discover` pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceReadiness {
    Ready,
    Waiting(String),
    Failed(String),
}

/// Judge one service from its state. `restarts` counts restarts seen so far.
pub fn judge_service(
    spec: &ComposeServiceSpec,
    state: Option<&compose::ServiceState>,
) -> ServiceReadiness {
    let name = &spec.name;
    let Some(st) = state else {
        return ServiceReadiness::Waiting(format!("service \"{name}\" has no container yet"));
    };
    if compose::is_completed(&st.state, st.exit_code) {
        return ServiceReadiness::Ready;
    }
    match st.state.as_str() {
        "exited" | "dead" => ServiceReadiness::Failed(format!(
            "service \"{name}\" exited{}",
            st.exit_code
                .map(|c| format!(" with code {c}"))
                .unwrap_or_default()
        )),
        "running" => match st.health.as_deref() {
            Some("unhealthy") => {
                ServiceReadiness::Failed(format!("service \"{name}\" is unhealthy"))
            }
            Some("starting") => {
                ServiceReadiness::Waiting(format!("service \"{name}\" healthcheck is starting"))
            }
            None if spec.has_healthcheck => {
                ServiceReadiness::Waiting(format!("service \"{name}\" has no health result yet"))
            }
            _ => ServiceReadiness::Ready,
        },
        other => ServiceReadiness::Waiting(format!("service \"{name}\" is {other}")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn wait_until_ready(
    state: &AppState,
    app: &App,
    deployment_id: &str,
    project: &str,
    specs: &[ComposeServiceSpec],
    publish: &HashMap<String, (u16, u16)>,
    timeout: u64,
    facts: &mut HealthFacts,
    trigger: &str,
) -> Result<()> {
    let app_id = app.id.as_str();
    let with_healthcheck: Vec<&str> = specs
        .iter()
        .filter(|s| s.has_healthcheck)
        .map(|s| s.name.as_str())
        .collect();
    log_deploy(
        state,
        app_id,
        "info",
        trigger,
        &if with_healthcheck.is_empty() {
            format!("Waiting up to {timeout}s for every service to run")
        } else {
            format!(
                "Waiting up to {timeout}s for every service to run and the healthchecks of {} to pass",
                with_healthcheck.join(", ")
            )
        },
    )
    .await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout);
    let mut ready: Vec<String> = Vec::new();
    loop {
        let states = compose::discover(&state.docker, project).await?;
        let mut waiting = Vec::new();
        for spec in specs {
            let st = states.iter().find(|s| s.service == spec.name);
            match judge_service(spec, st) {
                ServiceReadiness::Ready => {
                    if !ready.contains(&spec.name) {
                        ready.push(spec.name.clone());
                        set_services_status(
                            state,
                            app_id,
                            deployment_id,
                            &[spec.name.as_str()],
                            "healthy",
                        )
                        .await;
                    }
                }
                ServiceReadiness::Waiting(why) => waiting.push(why),
                ServiceReadiness::Failed(why) => {
                    set_services_status(
                        state,
                        app_id,
                        deployment_id,
                        &[spec.name.as_str()],
                        "failed",
                    )
                    .await;
                    if let Some(st) = st {
                        facts.container = container_exit(&state.docker, &st.container_id).await;
                        facts.container_logs = service_logs(state, &st.container_id).await;
                    }
                    facts.probe = Some(ProbeResult::NotListening);
                    anyhow::bail!("{why}; the compose app did not become ready");
                }
            }
        }
        if waiting.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            facts.probe = Some(ProbeResult::Timeout);
            anyhow::bail!(
                "services did not become ready within {timeout}s: {}",
                waiting.join("; ")
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Probe every published service like a single-container app.
    let readiness = app.readiness();
    let states = compose::discover(&state.docker, project).await?;
    for (service, (host, container)) in publish {
        let Some(st) = states.iter().find(|s| &s.service == service) else {
            continue;
        };
        let probe = match readiness {
            Readiness::Tcp => {
                log_deploy(
                    state,
                    app_id,
                    "info",
                    trigger,
                    &format!("[{service}] Waiting for port {container} to accept connections"),
                )
                .await?;
                probe_tcp_until_ready(&state.docker, &st.container_id, *container, timeout).await
            }
            _ => {
                let (path, require_2xx) = match app.health_check_path.clone() {
                    Some(p) => (p, true),
                    None => ("/".to_string(), false),
                };
                log_deploy(
                    state,
                    app_id,
                    "info",
                    trigger,
                    &if require_2xx {
                        format!("[{service}] Waiting for the health check on port {container}{path} to pass")
                    } else {
                        format!("[{service}] Waiting for the service to answer on port {container}")
                    },
                )
                .await?;
                let url = format!("http://127.0.0.1:{host}{path}");
                probe_until_ready(&state.docker, &st.container_id, &url, require_2xx, timeout)
                    .await?
            }
        };
        if probe.result != ProbeResult::Healthy {
            facts.probe = Some(probe.result);
            facts.container = probe.container;
            facts.container_logs = service_logs(state, &st.container_id).await;
            set_services_status(state, app_id, deployment_id, &[service.as_str()], "failed").await;
            anyhow::bail!(
                "service \"{service}\" did not answer on port {container} within {timeout}s"
            );
        }
        log_deploy(
            state,
            app_id,
            "info",
            trigger,
            &format!("[{service}] Ready"),
        )
        .await?;
    }
    Ok(())
}

async fn service_logs(state: &AppState, container_id: &str) -> Vec<String> {
    docker::get_logs(&state.docker, container_id, 40)
        .await
        .unwrap_or_default()
        .iter()
        .map(|l| strip_docker_ts(l).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, healthcheck: bool) -> ComposeServiceSpec {
        ComposeServiceSpec {
            name: name.into(),
            image: Some("x".into()),
            builds: false,
            ports: vec![],
            has_healthcheck: healthcheck,
        }
    }

    fn st(state: &str, health: Option<&str>, exit: Option<i64>) -> compose::ServiceState {
        compose::ServiceState {
            service: "s".into(),
            container_id: "c".into(),
            state: state.into(),
            health: health.map(str::to_string),
            exit_code: exit,
            image: None,
            image_id: None,
        }
    }

    #[test]
    fn services_are_ready_when_running_healthy_or_done() {
        let plain = spec("web", false);
        let checked = spec("db", true);
        assert_eq!(
            judge_service(&plain, Some(&st("running", None, None))),
            ServiceReadiness::Ready
        );
        assert_eq!(
            judge_service(&plain, Some(&st("exited", None, Some(0)))),
            ServiceReadiness::Ready,
            "one-shot services may finish"
        );
        assert!(matches!(
            judge_service(&plain, Some(&st("exited", None, Some(2)))),
            ServiceReadiness::Failed(m) if m.contains("code 2")
        ));
        assert!(matches!(
            judge_service(&plain, Some(&st("restarting", None, None))),
            ServiceReadiness::Waiting(_)
        ));
        assert!(matches!(
            judge_service(&plain, None),
            ServiceReadiness::Waiting(_)
        ));
        assert!(matches!(
            judge_service(&checked, Some(&st("running", Some("starting"), None))),
            ServiceReadiness::Waiting(_)
        ));
        assert!(matches!(
            judge_service(&checked, Some(&st("running", None, None))),
            ServiceReadiness::Waiting(_)
        ));
        assert_eq!(
            judge_service(&checked, Some(&st("running", Some("healthy"), None))),
            ServiceReadiness::Ready
        );
        assert!(matches!(
            judge_service(&checked, Some(&st("running", Some("unhealthy"), None))),
            ServiceReadiness::Failed(m) if m.contains("unhealthy")
        ));
    }

    #[test]
    fn host_ports_are_stable_unless_taken() {
        let rows = vec![
            ("web".to_string(), 80, Some(8300)),
            ("admin".to_string(), 8080, Some(8301)),
            ("api".to_string(), 3000, None),
        ];
        // 8300 is web's own container; 8301 is also held by something else.
        let plan =
            plan_host_ports(&rows, vec![8300, 8301, 8301, 8302]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(plan["web"], (8300, 80), "a service keeps its port");
        assert_eq!(
            plan["admin"],
            (8303, 8080),
            "a port taken by someone else moves"
        );
        assert_eq!(plan["api"], (8304, 3000));
    }

    #[test]
    fn compose_paths_stay_inside_the_repository() {
        assert_eq!(
            compose_path_in_repo(None).unwrap_or_default(),
            PathBuf::from("docker-compose.yml")
        );
        assert_eq!(
            compose_path_in_repo(Some("./deploy/compose.yaml")).unwrap_or_default(),
            PathBuf::from("deploy/compose.yaml")
        );
        assert!(compose_path_in_repo(Some("../x.yml")).is_err());
        assert!(compose_path_in_repo(Some("/etc/x.yml")).is_err());
        assert!(compose_path_in_repo(Some(".")).is_err());
    }
}
