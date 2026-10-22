// handlers/templates.rs — the one-click template catalog
//
//   GET  /api/templates              the catalog (tond_common::builtin_templates)
//   POST /api/templates/{id}/deploy  create the app and start its first deploy
//   GET  /api/apps/{id}/template     an app's template: update available, upgrade
//                                    notes, env values its domain change left stale
//   POST /api/apps/{id}/template/domain-env  update those values (optionally redeploy)
//   POST /api/apps/{id}/template/upgrade     apply the catalog's newer version
//
// A template deploy is one server-side step so every client gets the same
// behaviour: inputs are checked and generated, the app is created as an inline
// compose app whose env holds the (encrypted) inputs, its volume directories
// are created with the owners the images expect, and the first deployment is
// queued. If anything fails after the app row exists, the app is removed again
// so a retry starts clean. Generated credentials are returned once.

use std::collections::BTreeMap;
use std::path::Path as FsPath;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{
    ApiResponse, App, CreateAppRequest, Scope, Template, TemplateAppStatus,
    TemplateCatalogResponse, TemplateContext, TemplateCredential, TemplateDeployRequest,
    TemplateDeployResponse, TemplateDomainEnvRequest, TemplateDomainEnvResponse,
    TemplateHostMemory, TemplateResponse, TemplateUpgradeResponse, TondError, architecture_label,
    builtin_template, builtin_templates, domain_env_templates, host_architecture, resolve_inputs,
    template_version_newer,
};

use super::fetch_app;
use crate::{
    audit_log::write_audit_log,
    auth::{Authz, Principal, RateLimit, scope},
    backup, deploy, domains,
    error::AppError,
    handlers::{deploy_trigger, insert_app},
    metrics::host,
    secrets,
    state::AppState,
    template_apps, volumes,
};

const MIB: u64 = 1024 * 1024;

fn catalog_error(err: impl std::fmt::Display) -> AppError {
    AppError(TondError::Internal(format!(
        "the template catalog is invalid: {err}"
    )))
}

pub async fn list_templates(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let templates: Vec<TemplateResponse> = builtin_templates()
        .map_err(catalog_error)?
        .into_iter()
        .map(TemplateResponse::from)
        .collect();
    let memory = host::snapshot().map(|s| TemplateHostMemory {
        total_mb: s.mem_total / MIB,
        available_mb: s.mem_total.saturating_sub(s.mem_used) / MIB,
    });
    let apps_domain = state.settings.read().await.apps_domain.clone();
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(TemplateCatalogResponse {
            templates,
            memory,
            apps_domain,
            architecture: host_architecture().to_string(),
        })),
    ))
}

/// Refuse a template whose images don't exist for this server's CPU.
fn check_architecture(template: &Template, host: &str) -> Result<(), AppError> {
    if template.manifest.architectures.iter().any(|a| a == host) {
        return Ok(());
    }
    let available: Vec<&str> = template
        .manifest
        .architectures
        .iter()
        .map(|a| architecture_label(a))
        .collect();
    Err(AppError(
        TondError::Conflict(format!(
            "{} isn't available on {}: its images are published for {} only.",
            template.manifest.name,
            architecture_label(host),
            available.join(" and ")
        ))
        .with_hint("Pick another template, or deploy it on a server with a matching CPU."),
    ))
}

/// Free memory in bytes (total minus used, i.e. what the kernel reports as
/// available), from the host collector's last sample.
fn available_memory() -> Option<u64> {
    host::snapshot().map(|s| s.mem_total.saturating_sub(s.mem_used))
}

/// The 409 for a template that needs more memory than is free, unless the
/// caller chose to deploy anyway.
fn check_memory(
    template: &Template,
    available: Option<u64>,
    allow_low_memory: bool,
) -> Result<(), AppError> {
    let Some(available) = available else {
        return Ok(());
    };
    let needed = u64::from(template.manifest.ram_mb) * MIB;
    if allow_low_memory || needed <= available {
        return Ok(());
    }
    Err(AppError(TondError::InsufficientMemory(format!(
        "{} needs about {} MB of memory and this server has {} MB free.",
        template.manifest.name,
        template.manifest.ram_mb,
        available / MIB
    ))))
}

/// Refuse an app name whose volume directory already holds data: data left
/// by an earlier app of the same name would not match the new passwords.
fn check_no_leftover_data(data_dir: &FsPath, app_name: &str) -> Result<(), AppError> {
    let volume_dir = volumes::app_volume_dir(data_dir, app_name);
    if std::fs::read_dir(&volume_dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(AppError(TondError::InvalidInput(format!(
            "{} still holds data from an earlier app named {app_name}. Pick another name, or delete that directory on the server first.",
            volume_dir.display()
        ))));
    }
    Ok(())
}

/// Create the declared volume directories, with their owners, inside the
/// app's volume directory.
fn prepare_volumes(template: &Template, data_dir: &FsPath, app_name: &str) -> Result<(), AppError> {
    let volume_dir = volumes::app_volume_dir(data_dir, app_name);
    for volume in &template.manifest.volumes {
        let dir = volumes::prepare_host_dir(
            data_dir,
            &volume_dir,
            FsPath::new(volume.relative_path()),
            false,
        )
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "could not create the volume directory {}: {e:#}",
                volume.path
            )))
        })?;
        #[cfg(unix)]
        if let Some((uid, gid)) = volume.owner_ids()
            && let Err(e) = std::os::unix::fs::chown(&dir, Some(uid), Some(gid))
        {
            // Only root can chown; outside production (dev, tests) the
            // image may then fail to write, which its logs will say.
            tracing::warn!("could not give {} to {uid}:{gid}: {e}", dir.display());
        }
    }
    Ok(())
}

/// Undo a template deploy that failed after the app was created. Its volume
/// directory was new or empty (check_no_leftover_data), so removing it only
/// removes what this deploy created.
async fn remove_app(state: &AppState, app: &App, data_dir: &FsPath) {
    let _ = sqlx::query("DELETE FROM apps WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await;
    let _ = std::fs::remove_dir_all(volumes::app_volume_dir(data_dir, &app.name));
}

pub async fn deploy_template(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TemplateDeployRequest>,
) -> Result<impl IntoResponse, AppError> {
    let response = deploy_from_template(&state, &auth, &id, req, &volumes::data_dir()).await?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::ok(response))))
}

/// The deploy itself, with the data directory passed in (tests use a temp dir).
async fn deploy_from_template(
    state: &AppState,
    auth: &Principal,
    id: &str,
    req: TemplateDeployRequest,
    data_dir: &FsPath,
) -> Result<TemplateDeployResponse, AppError> {
    // Templates are compose apps (admin scope, checked by insert_app) that
    // are deployed right away.
    auth.require_to(Scope::AppsDeploy, "deploy apps")?;
    let template = builtin_template(id)
        .map_err(catalog_error)?
        .ok_or_else(|| AppError(TondError::NotFound(format!("No template named {id}"))))?;

    let name = req.name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 40
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || name.starts_with('-')
        || name.ends_with('-')
    {
        return Err(AppError(TondError::invalid_field(
            "name",
            "App name must be 1–40 lowercase letters, digits and hyphens",
        )));
    }
    check_architecture(&template, host_architecture())?;
    check_memory(&template, available_memory(), req.allow_low_memory)?;

    // The address the app will answer on, for inputs like {app_url}: the
    // custom domain, else the automatic one (<name>.<apps domain>).
    let domain = req
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(domains::normalize_domain);
    let apps_domain = state.settings.read().await.apps_domain.clone();
    let host = match &domain {
        Some(d) => d.clone(),
        None if !apps_domain.is_empty() => domains::app_host(&name, &apps_domain),
        None => format!("{name}.localhost"),
    };
    let ctx = TemplateContext::new(&name, &host);
    let resolved = resolve_inputs(&template.manifest, &req.inputs, &ctx).map_err(|e| {
        AppError(TondError::invalid_field(
            format!("inputs.{}", e.key),
            e.message,
        ))
    })?;

    // An existing app of this name owns its volume directory: say so (409)
    // rather than calling its data "left over".
    let taken: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM apps WHERE name = ?")
        .bind(&name)
        .fetch_one(&state.db)
        .await?;
    if taken > 0 {
        return Err(AppError(TondError::AppAlreadyExists(name)));
    }
    check_no_leftover_data(data_dir, &name)?;

    let public = &template.manifest.public;
    let env: BTreeMap<&str, &str> = resolved
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let create = CreateAppRequest {
        name: name.clone(),
        source_type: "compose".into(),
        source_url: None,
        branch: None,
        container_port: i64::from(public.port),
        env_vars: Some(serde_json::json!(env)),
        domain: domain.clone(),
        project_id: req.project_id.clone().filter(|p| !p.is_empty()),
        memory_limit: None,
        cpu_limit: None,
        health_check_path: public.health_path.clone(),
        health_check_interval: None,
        health_failure_threshold: None,
        drain_timeout: None,
        build_method: None,
        auto_deploy: false,
        watch_branch: None,
        volumes: None,
        registry_credential_id: None,
        github_app_id: None,
        dockerfile_content: None,
        compose_content: Some(template.compose.clone()),
        compose_path: None,
        // The manifest's public service gets the port and the domain.
        compose_service: Some(public.service.clone()),
        base_dir: None,
        static_build: false,
        readiness: Some(
            if public.health_path.is_some() {
                "http"
            } else {
                "tcp"
            }
            .into(),
        ),
        ready_after_secs: None,
    };
    let app = insert_app(state, auth, create).await?;

    if let Err(e) = prepare_volumes(&template, data_dir, &name) {
        remove_app(state, &app, data_dir).await;
        return Err(e);
    }

    sqlx::query("UPDATE apps SET template_id = ? WHERE id = ?")
        .bind(&template.manifest.id)
        .bind(&app.id)
        .execute(&state.db)
        .await?;
    // What upgrades and domain changes need later (template_apps.rs).
    let domain_env = template_apps::DomainEnv {
        keys: domain_env_templates(&template.manifest, &resolved.env, &ctx),
        host: host.clone(),
    };
    template_apps::record(
        &state.db,
        &app.id,
        Default::default(),
        &template.compose,
        &template.manifest.version,
        &domain_env,
    )
    .await?;

    let app_id = app.id.clone();
    let app_domain = app.domain.clone();
    let (trigger, detail) = deploy_trigger(auth);
    let queued = match deploy::start(
        state,
        app.clone(),
        deploy::DeployRequest::source(trigger, detail),
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            remove_app(state, &app, data_dir).await;
            return Err(AppError(TondError::Internal(format!(
                "{} was not created: the first deploy couldn't be queued ({e:#})",
                template.manifest.name
            ))));
        }
    };

    write_audit_log(
        &state.db,
        Some(&app_id),
        Some(&name),
        "template.deploy",
        &auth.audit_actor(),
        Some(&format!(
            "template={} version={} deployment={}",
            template.manifest.id, template.manifest.version, queued.deployment_id
        )),
    )
    .await;

    let response = TemplateDeployResponse {
        app_id,
        app_name: name,
        domain: app_domain.or_else(|| (!apps_domain.is_empty()).then_some(host)),
        deployment_id: queued.deployment_id.clone(),
        credentials: resolved
            .credentials
            .into_iter()
            .map(|(key, label, value)| TemplateCredential { key, label, value })
            .collect(),
        setup_notes: template.manifest.setup_notes.clone(),
    };
    Ok(response)
}

// ── Apps deployed from a template: status, domain env, upgrade ──

fn not_from_template(app: &App) -> AppError {
    AppError(TondError::InvalidInput(format!(
        "'{}' wasn't deployed from a template",
        app.name
    )))
}

/// Why a one-click upgrade of `app` to `template` is refused, if it is.
fn upgrade_blocker(app: &App, tracking: &template_apps::Tracking) -> Option<String> {
    if app.source_type != "compose" || app.source_url.is_some() {
        return Some("The app no longer runs the template's pasted compose file.".to_string());
    }
    let content = app.compose_content.as_deref().unwrap_or_default();
    match &tracking.compose_sha256 {
        None => Some(
            "Tond didn't record the compose file this app was deployed with, so it can't tell \
             whether it was edited. Upgrade by hand with the upgrade notes."
                .to_string(),
        ),
        Some(sha) if *sha != template_apps::compose_sha256(content) => Some(
            "The compose file was edited after it was deployed from the template, and an \
             upgrade would replace it. Apply the new version by hand with the upgrade notes, \
             or undo your edits first."
                .to_string(),
        ),
        Some(_) => None,
    }
}

async fn template_status(state: &AppState, app: &App) -> Result<TemplateAppStatus, AppError> {
    let template_id = app
        .template_id
        .clone()
        .ok_or_else(|| not_from_template(app))?;
    let template = builtin_template(&template_id).map_err(catalog_error)?;
    let tracking = template_apps::load(&state.db, &app.id).await?;
    let latest = template.as_ref().map(|t| t.manifest.version.clone());
    let update_available = match (&latest, &app.template_version) {
        (Some(latest), Some(current)) => template_version_newer(latest, current),
        _ => false,
    };
    let (domain_env, domain_env_edited) = match (
        &tracking.domain_env,
        template_apps::current_host(state, app).await,
    ) {
        (Some(recorded), Some(host)) => {
            let env = secrets::app_env(app)?;
            template_apps::domain_env_changes(recorded, &env, &app.name, &host)
        }
        _ => (Vec::new(), Vec::new()),
    };
    Ok(TemplateAppStatus {
        template_name: template.as_ref().map(|t| t.manifest.name.clone()),
        upgrade_notes: template
            .as_ref()
            .and_then(|t| t.manifest.upgrade_notes.clone()),
        upgrade_blocked: if update_available {
            upgrade_blocker(app, &tracking)
        } else {
            None
        },
        template_id,
        version: app.template_version.clone(),
        latest_version: latest,
        update_available,
        domain_env,
        domain_env_edited,
    })
}

pub async fn get_app_template(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let status = template_status(&state, &app).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(status))))
}

/// Update the env values derived from the app's address to its current
/// primary domain (only those still as Tond wrote them), optionally redeploying.
pub async fn sync_template_domain_env(
    _rate: RateLimit,
    auth: Authz<scope::EnvWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TemplateDomainEnvRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.redeploy {
        auth.require_to(Scope::AppsDeploy, "deploy apps")?;
    }
    let app = fetch_app(&state.db, &id).await?;
    if app.template_id.is_none() {
        return Err(not_from_template(&app));
    }
    let host = template_apps::current_host(&state, &app)
        .await
        .ok_or_else(|| {
            AppError(TondError::InvalidInput(format!(
                "'{}' has no domain to take the values from",
                app.name
            )))
        })?;

    let tracking = template_apps::load(&state.db, &app.id).await?;
    let mut tx = crate::db::begin_write(&state.db).await?;
    let app = super::env::fetch_app_in_tx(&mut tx, &app.id).await?;
    let Some(mut recorded) = tracking.domain_env.clone() else {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{}' has no values derived from its domain",
            app.name
        ))));
    };
    let mut env = secrets::app_env(&app)?;
    let (changes, _edited) = template_apps::domain_env_changes(&recorded, &env, &app.name, &host);
    for change in &changes {
        env.insert(change.key.clone(), change.proposed.clone());
    }
    recorded.host = host;
    sqlx::query(
        "UPDATE apps SET env_vars = ?, template_domain_env = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(secrets::seal_app_env(&env)?)
    .bind(serde_json::to_string(&recorded).unwrap_or_else(|_| "{}".into()))
    .bind(&app.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    crate::log_scrub::forget_app(&app.id);

    let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "env_update",
        &auth.audit_actor(),
        Some(&format!("set={} (template domain values)", keys.join(","))),
    )
    .await;

    let deployment_id = if req.redeploy && !changes.is_empty() {
        let app = fetch_app(&state.db, &app.id).await?;
        let (trigger, detail) = deploy_trigger(&auth);
        let queued = deploy::start(&state, app, deploy::DeployRequest::source(trigger, detail))
            .await
            .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
        Some(queued.deployment_id)
    } else {
        None
    };
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(TemplateDomainEnvResponse {
            updated: changes,
            deployment_id,
        })),
    ))
}

/// Move an app to the catalog's newer template version: snapshot its data,
/// replace the compose file (refused if the user edited it), keep its env and
/// generated secrets (new inputs are generated or defaulted), and deploy.
pub async fn upgrade_app_template(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let response = upgrade_from_template(&state, &auth, app, &volumes::data_dir(), true).await?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::ok(response))))
}

async fn upgrade_from_template(
    state: &AppState,
    auth: &Principal,
    app: App,
    data_dir: &FsPath,
    snapshot: bool,
) -> Result<TemplateUpgradeResponse, AppError> {
    // A compose file reaches the host (like editing one), and it deploys.
    auth.require_to(Scope::SettingsWrite, "edit Docker Compose files")?;
    auth.require_to(Scope::AppsDeploy, "deploy apps")?;
    let template_id = app
        .template_id
        .clone()
        .ok_or_else(|| not_from_template(&app))?;
    let template = builtin_template(&template_id)
        .map_err(catalog_error)?
        .ok_or_else(|| {
            AppError(TondError::NotFound(format!(
                "The template {template_id} is no longer in the catalog"
            )))
        })?;
    let to_version = template.manifest.version.clone();
    let from_version = app.template_version.clone();
    if !from_version
        .as_deref()
        .is_some_and(|v| template_version_newer(&to_version, v))
    {
        return Err(AppError(TondError::Conflict(format!(
            "{} is already on the catalog's version ({to_version})",
            app.name
        ))));
    }
    let tracking = template_apps::load(&state.db, &app.id).await?;
    if let Some(reason) = upgrade_blocker(&app, &tracking) {
        return Err(AppError(
            TondError::Conflict(reason).with_hint("See the template's upgrade notes."),
        ));
    }
    check_architecture(&template, host_architecture())?;

    // Keep every value the app has; fill inputs the new version added.
    let host = template_apps::current_host(state, &app)
        .await
        .unwrap_or_else(|| format!("{}.localhost", app.name));
    let ctx = TemplateContext::new(&app.name, &host);
    let mut env = secrets::app_env(&app)?;
    let missing_required: Vec<&str> = template
        .manifest
        .inputs
        .iter()
        .filter(|i| {
            !env.contains_key(&i.key)
                && i.value.is_none()
                && i.generate.is_none()
                && i.default.is_none()
                && i.required
        })
        .map(|i| i.label.as_str())
        .collect();
    if !missing_required.is_empty() {
        return Err(AppError(TondError::InvalidInput(format!(
            "{to_version} needs values Tond can't make up: {}. Add them as environment \
             variables first (see the upgrade notes), then upgrade.",
            missing_required.join(", ")
        ))));
    }
    let provided: BTreeMap<String, String> = template
        .manifest
        .inputs
        .iter()
        .filter(|i| i.value.is_none() && i.generate.is_none() && i.default.is_none())
        .map(|i| (i.key.clone(), env.get(&i.key).cloned().unwrap_or_default()))
        .collect();
    let fresh = resolve_inputs(&template.manifest, &provided, &ctx).map_err(|e| {
        AppError(TondError::InvalidInput(format!(
            "{to_version} can't use the app's {}: {}",
            e.key, e.message
        )))
    })?;
    let mut added_env = Vec::new();
    for (key, value) in &fresh.env {
        if !env.contains_key(key) {
            env.insert(key.clone(), value.clone());
            added_env.push(key.clone());
        }
    }
    let credentials: Vec<TemplateCredential> = fresh
        .credentials
        .into_iter()
        .filter(|(key, _, _)| added_env.contains(key))
        .map(|(key, label, value)| TemplateCredential { key, label, value })
        .collect();
    let domain_env = template_apps::DomainEnv {
        keys: domain_env_templates(&template.manifest, &env, &ctx),
        host,
    };

    // Nothing has changed yet: take the snapshot first, and stop if it fails.
    let snapshot_id = if snapshot {
        Some(
            backup::create_snapshot(
                state,
                backup::SnapshotTrigger::PreTemplateUpgrade {
                    app_name: app.name.clone(),
                    to_version: to_version.clone(),
                },
                None,
            )
            .await
            .map_err(|e| {
                AppError(TondError::Internal(format!(
                    "The snapshot before the upgrade failed, so nothing was changed: {e:#}"
                )))
            })?,
        )
    } else {
        None
    };

    prepare_volumes(&template, data_dir, &app.name)?;
    let mut tx = crate::db::begin_write(&state.db).await?;
    sqlx::query(
        "UPDATE apps SET compose_content = ?, env_vars = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&template.compose)
    .bind(secrets::seal_app_env(&env)?)
    .bind(&app.id)
    .execute(&mut *tx)
    .await?;
    template_apps::record(
        &mut *tx,
        &app.id,
        tracking.releases.clone(),
        &template.compose,
        &to_version,
        &domain_env,
    )
    .await?;
    tx.commit().await?;
    crate::log_scrub::forget_app(&app.id);

    let app = fetch_app(&state.db, &app.id).await?;
    let (trigger, detail) = deploy_trigger(auth);
    let queued = deploy::start(state, app.clone(), deploy::DeployRequest::source(trigger, detail))
        .await
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "The new compose file is saved but its deploy couldn't be queued ({e:#}); deploy the app to apply it"
            )))
        })?;
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "template.upgrade",
        &auth.audit_actor(),
        Some(&format!(
            "template={template_id} from={} to={to_version} added_env={} deployment={}",
            from_version.as_deref().unwrap_or("unknown"),
            added_env.join(","),
            queued.deployment_id
        )),
    )
    .await;
    Ok(TemplateUpgradeResponse {
        from_version,
        to_version,
        snapshot_id,
        deployment_id: queued.deployment_id,
        added_env,
        credentials,
        upgrade_notes: template.manifest.upgrade_notes.clone(),
    })
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, response::IntoResponse};

    use super::*;
    use crate::compose_policy;
    use tond_common::Role;

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn test_state() -> AppState {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations: {err}"));
        AppState {
            db,
            docker: bollard::Docker::connect_with_http(
                "http://127.0.0.1:2375",
                5,
                bollard::API_DEFAULT_VERSION,
            )
            .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(tond_common::ServerSettings {
                apps_domain: "apps.example.com".into(),
                ..Default::default()
            })),
        }
    }

    fn deploy_request(name: &str, inputs: &[(&str, &str)]) -> TemplateDeployRequest {
        TemplateDeployRequest {
            name: name.into(),
            domain: None,
            project_id: None,
            inputs: inputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            allow_low_memory: true,
        }
    }

    async fn deploy(
        state: &AppState,
        data_dir: &FsPath,
        id: &str,
        req: TemplateDeployRequest,
    ) -> Result<serde_json::Value, AppError> {
        let principal = Principal::test_user(Role::Admin);
        let response = deploy_from_template(state, &principal, id, req, data_dir).await?;
        Ok(serde_json::json!({ "data": response }))
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"))
    }

    async fn app_count(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM apps")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[tokio::test]
    async fn deploy_creates_a_compose_app_with_its_inputs_and_queues_a_deploy() {
        let state = test_state().await;
        let dir = tmp();
        let body = deploy(&state, dir.path(), "it-tools", deploy_request("tools", &[]))
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(body["data"]["app_name"], "tools");
        assert_eq!(body["data"]["domain"], "tools.apps.example.com");
        assert!(
            body["data"]["deployment_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );

        let app: App = sqlx::query_as("SELECT * FROM apps WHERE name = 'tools'")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(app.source_type, "compose");
        assert_eq!(app.container_port, 80);
        assert!(
            app.compose_content
                .as_deref()
                .is_some_and(|c| c.contains("it-tools"))
        );
        assert_eq!(app.health_check_path.as_deref(), Some("/"));
    }

    #[tokio::test]
    async fn deploy_returns_generated_credentials_once_and_stores_them_encrypted() {
        let state = test_state().await;
        let dir = tmp();
        let body = deploy(
            &state,
            dir.path(),
            "vaultwarden",
            deploy_request("vault", &[]),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        let creds = body["data"]["credentials"]
            .as_array()
            .unwrap_or_else(|| panic!("credentials"));
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0]["key"], "ADMIN_TOKEN");
        let token = creds[0]["value"].as_str().unwrap_or_default().to_string();
        assert_eq!(token.len(), 24);
        assert!(
            volumes::app_volume_dir(dir.path(), "vault")
                .join("data")
                .is_dir(),
            "declared volumes are created"
        );
        let app: App = sqlx::query_as("SELECT * FROM apps WHERE name = 'vault'")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let env = crate::secrets::app_env(&app).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(env["DOMAIN"], "https://vault.apps.example.com");
        assert_eq!(env["ADMIN_TOKEN"], token);
        assert!(
            !app.env_vars.as_deref().unwrap_or("").contains(&token),
            "env vars are stored encrypted"
        );

        // The name is taken while the app exists…
        let err = deploy(
            &state,
            dir.path(),
            "vaultwarden",
            deploy_request("vault", &[]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("a taken name is refused"));
        assert_eq!(err.0.status_code(), 409);

        // …and after it's deleted its data would not match new passwords.
        sqlx::query("DELETE FROM apps")
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            volumes::app_volume_dir(dir.path(), "vault").join("data/db.sqlite3"),
            "x",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let err = deploy(
            &state,
            dir.path(),
            "vaultwarden",
            deploy_request("vault", &[]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("leftover data is refused"));
        assert!(
            err.0.to_string().contains("earlier app named vault"),
            "{}",
            err.0
        );
        assert_eq!(app_count(&state).await, 0);
    }

    #[tokio::test]
    async fn deploy_refuses_bad_names_unknown_templates_and_missing_inputs() {
        let state = test_state().await;
        let dir = tmp();
        let err = deploy(
            &state,
            dir.path(),
            "it-tools",
            deploy_request("Bad Name", &[]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("bad names are refused"));
        assert_eq!(err.0.status_code(), 400);
        let err = deploy(&state, dir.path(), "nope", deploy_request("x", &[]))
            .await
            .err()
            .unwrap_or_else(|| panic!("unknown templates are refused"));
        assert_eq!(err.0.status_code(), 404);
        let err = deploy(&state, dir.path(), "pocketbase", deploy_request("pb", &[]))
            .await
            .err()
            .unwrap_or_else(|| panic!("a missing required input is refused"));
        assert!(err.0.to_string().contains("Superuser email"), "{}", err.0);
        let err = deploy(
            &state,
            dir.path(),
            "pocketbase",
            deploy_request("pb", &[("SUPERUSER_EMAIL", "nope")]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("an invalid email is refused"));
        assert!(err.0.to_string().contains("email address"), "{}", err.0);
        assert_eq!(app_count(&state).await, 0, "nothing is created on refusal");
    }

    #[tokio::test]
    async fn deploy_needs_the_compose_and_deploy_scopes() {
        let state = test_state().await;
        let result = deploy_template(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Viewer)),
            State(state.clone()),
            Path("it-tools".to_string()),
            Json(deploy_request("tools", &[])),
        )
        .await
        .map(|_| ());
        let Err(err) = result else {
            panic!("a viewer can't deploy templates");
        };
        assert_eq!(err.0.status_code(), 403);
        assert_eq!(app_count(&state).await, 0);
    }

    #[tokio::test]
    async fn list_templates_returns_the_catalog() {
        let response = list_templates(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(test_state().await),
        )
        .await
        .unwrap_or_else(|_| panic!("expected list_templates to succeed"))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(body["data"]["apps_domain"], "apps.example.com");
        let templates = body["data"]["templates"]
            .as_array()
            .unwrap_or_else(|| panic!("expected templates payload to be an array"));
        assert!(templates.len() >= 25);
        let umami = templates
            .iter()
            .find(|t| t["id"] == "umami")
            .unwrap_or_else(|| panic!("umami should be in the catalog"));
        assert_eq!(umami["public"]["service"], "umami");
        assert_eq!(umami["fits_2gb"], true);
        assert!(umami["services"].as_array().is_some_and(|s| s.len() == 2));
        assert!(
            umami["inputs"]
                .as_array()
                .is_some_and(|i| i.iter().all(|i| i["key"] != "DATABASE_URL")),
            "computed values are not form inputs"
        );
    }

    /// Every shipped compose file passes the same policy user compose files do.
    #[test]
    fn every_template_passes_compose_policy() {
        let volume_dir = FsPath::new("/opt/tond/data/volumes/app");
        for t in builtin_templates().unwrap_or_else(|e| panic!("{e}")) {
            if let Err(errors) = compose_policy::validate(&t.compose, volume_dir) {
                panic!(
                    "{}: {}",
                    t.manifest.id,
                    compose_policy::format_errors(&errors)
                );
            }
            // Every input reaches compose (no reserved names are dropped).
            let env: BTreeMap<String, String> = t
                .manifest
                .inputs
                .iter()
                .map(|i| (i.key.clone(), "x".to_string()))
                .collect();
            assert_eq!(
                compose_policy::interpolation_env(&t.compose, &env).len(),
                env.len(),
                "{}: every input should be passed to docker compose",
                t.manifest.id
            );
        }
    }

    #[tokio::test]
    async fn deploy_sets_the_primary_domain_and_records_the_template() {
        let state = test_state().await;
        let dir = tmp();
        let mut req = deploy_request("tools", &[]);
        req.domain = Some("Tools.Example.com".into());
        let body = deploy(&state, dir.path(), "it-tools", req)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(body["data"]["domain"], "tools.example.com");

        let (domain, template_id, version): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT domain, template_id, template_version FROM apps WHERE name = 'tools'",
            )
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(domain.as_deref(), Some("tools.example.com"));
        assert_eq!(template_id.as_deref(), Some("it-tools"));
        assert!(version.is_some());
        let primary: String = sqlx::query_scalar(
            "SELECT d.domain FROM app_domains d JOIN apps a ON a.id = d.app_id WHERE a.name = 'tools' AND d.kind = 'primary'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            primary, "tools.example.com",
            "the domain seeds app_domains' primary row"
        );
        // ...on the manifest's public service, which publishes its port.
        let t = builtin_template("it-tools")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("it-tools"));
        let (service, port): (Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT service, port FROM app_domains WHERE domain = 'tools.example.com'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(service.as_deref(), Some(t.manifest.public.service.as_str()));
        assert_eq!(port, Some(i64::from(t.manifest.public.port)));
        let published: Option<i64> = sqlx::query_scalar(
            "SELECT s.published_port FROM compose_services s JOIN apps a ON a.id = s.app_id
             WHERE a.name = 'tools' AND s.name = ?",
        )
        .bind(&t.manifest.public.service)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(published, Some(i64::from(t.manifest.public.port)));

        // The same domain can't go to a second app.
        let mut again = deploy_request("tools2", &[]);
        again.domain = Some("tools.example.com".into());
        let err = deploy(&state, dir.path(), "it-tools", again)
            .await
            .err()
            .unwrap_or_else(|| panic!("a taken domain is refused"));
        assert_eq!(err.0.status_code(), 400);
        assert_eq!(app_count(&state).await, 1);
    }

    #[test]
    fn architecture_check_refuses_missing_images() {
        let t = builtin_template("mattermost")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("mattermost"));
        assert!(check_architecture(&t, "amd64").is_ok());
        let err = check_architecture(&t, "arm64")
            .err()
            .unwrap_or_else(|| panic!("mattermost has no ARM64 image"));
        assert_eq!(err.0.status_code(), 409);
        assert!(
            err.0.message().contains("isn't available on ARM64"),
            "{}",
            err.0.message()
        );
        let umami = builtin_template("umami")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("umami"));
        assert!(check_architecture(&umami, "arm64").is_ok());
    }

    #[test]
    fn memory_check_warns_unless_overridden() {
        let t = builtin_template("umami")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("umami"));
        let need = u64::from(t.manifest.ram_mb) * MIB;
        assert!(check_memory(&t, Some(need), false).is_ok());
        assert!(
            check_memory(&t, None, false).is_ok(),
            "no sample yet: don't block"
        );
        let err = check_memory(&t, Some(need / 2), false)
            .err()
            .unwrap_or_else(|| panic!("expected insufficient_memory"));
        assert_eq!(err.0.code(), "INSUFFICIENT_MEMORY");
        assert!(err.0.hint().is_some_and(|h| h.contains("allow_low_memory")));
        assert_eq!(err.0.status_code(), 409);
        assert!(check_memory(&t, Some(need / 2), true).is_ok());
    }

    #[test]
    fn prepare_volumes_refuses_leftover_data() {
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let t = builtin_template("umami")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("umami"));
        check_no_leftover_data(tmp.path(), "stats").unwrap_or_else(|e| panic!("{}", e.0));
        prepare_volumes(&t, tmp.path(), "stats").unwrap_or_else(|e| panic!("{}", e.0));
        let dir = volumes::app_volume_dir(tmp.path(), "stats");
        for v in &t.manifest.volumes {
            assert!(dir.join(v.relative_path()).is_dir());
        }
        let err = check_no_leftover_data(tmp.path(), "stats")
            .err()
            .unwrap_or_else(|| panic!("an app's leftover data should be refused"));
        assert!(err.0.to_string().contains("earlier app named stats"));
    }

    async fn app_named(state: &AppState, name: &str) -> App {
        sqlx::query_as("SELECT * FROM apps WHERE name = ?")
            .bind(name)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[tokio::test]
    async fn a_domain_change_offers_to_update_the_values_made_from_the_old_one() {
        let state = test_state().await;
        let dir = tmp();
        deploy(&state, dir.path(), "linkding", deploy_request("links", &[]))
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        let app = app_named(&state, "links").await;
        let status = template_status(&state, &app)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert!(status.domain_env.is_empty(), "nothing changed yet");
        assert!(!status.update_available);

        // The primary domain moves; a hand-edited derived value is left alone.
        sqlx::query(
            "INSERT INTO app_domains (domain, app_id, kind) VALUES ('links.example.org', ?, 'primary')
             ON CONFLICT(domain) DO NOTHING",
        )
        .bind(&app.id)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        sqlx::query("UPDATE app_domains SET kind = 'alias' WHERE app_id = ? AND domain != 'links.example.org'")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        sqlx::query("UPDATE apps SET domain = 'links.example.org' WHERE id = ?")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let app = app_named(&state, "links").await;
        let status = template_status(&state, &app)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(
            status.domain_env,
            [tond_common::TemplateDomainEnvChange {
                key: "TRUSTED_ORIGIN".into(),
                current: "https://links.apps.example.com".into(),
                proposed: "https://links.example.org".into(),
            }]
        );

        let out = response_json(
            sync_template_domain_env(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Path("links".to_string()),
                Json(TemplateDomainEnvRequest { redeploy: false }),
            )
            .await
            .unwrap_or_else(|e| panic!("{}", e.0))
            .into_response(),
        )
        .await;
        assert_eq!(out["data"]["updated"][0]["key"], "TRUSTED_ORIGIN");
        assert_eq!(out["data"]["deployment_id"], serde_json::Value::Null);
        let app = app_named(&state, "links").await;
        let env = secrets::app_env(&app).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            env.get("TRUSTED_ORIGIN").map(String::as_str),
            Some("https://links.example.org")
        );
        let status = template_status(&state, &app)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert!(status.domain_env.is_empty(), "in sync again");
    }

    #[tokio::test]
    async fn upgrades_keep_env_and_secrets_and_refuse_an_edited_compose_file() {
        let state = test_state().await;
        let dir = tmp();
        deploy(&state, dir.path(), "linkding", deploy_request("links", &[]))
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        let admin = Principal::test_user(Role::Admin);
        let before = app_named(&state, "links").await;
        let env_before = secrets::app_env(&before).unwrap_or_else(|e| panic!("{e}"));

        // On the catalog's version: nothing to upgrade.
        let err = upgrade_from_template(&state, &admin, before.clone(), dir.path(), false)
            .await
            .err()
            .unwrap_or_else(|| panic!("already current"));
        assert!(err.0.to_string().contains("already on"), "{}", err.0);

        // An older install of the template, with a key the new version adds.
        let mut env = env_before.clone();
        env.remove("TRUSTED_ORIGIN");
        let old_compose = format!(
            "{}\n# older\n",
            before.compose_content.clone().unwrap_or_default()
        );
        sqlx::query(
            "UPDATE apps SET template_version = '0.0.1', compose_content = ?, env_vars = ? WHERE id = ?",
        )
        .bind(&old_compose)
        .bind(secrets::seal_app_env(&env).unwrap_or_else(|e| panic!("{e}")))
        .bind(&before.id)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let tracking = template_apps::load(&state.db, &before.id)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        template_apps::record(
            &state.db,
            &before.id,
            tracking.releases,
            &old_compose,
            "0.0.1",
            &tracking.domain_env.unwrap_or_default(),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let app = app_named(&state, "links").await;
        let status = template_status(&state, &app)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert!(status.update_available);
        assert!(status.upgrade_notes.is_some());
        assert_eq!(status.upgrade_blocked, None);

        // Edited by hand: refused, with the reason.
        sqlx::query("UPDATE apps SET compose_content = ? WHERE id = ?")
            .bind(format!("{old_compose}# my edit\n"))
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let edited = app_named(&state, "links").await;
        let status = template_status(&state, &edited)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert!(
            status
                .upgrade_blocked
                .as_deref()
                .is_some_and(|r| r.contains("edited"))
        );
        let err = upgrade_from_template(&state, &admin, edited, dir.path(), false)
            .await
            .err()
            .unwrap_or_else(|| panic!("edited files aren't replaced"));
        assert_eq!(err.0.status_code(), 409);

        // Unedited: the new file, the old values, the new key filled in.
        sqlx::query("UPDATE apps SET compose_content = ? WHERE id = ?")
            .bind(&old_compose)
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let app = app_named(&state, "links").await;
        let out = upgrade_from_template(&state, &admin, app, dir.path(), false)
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(out.from_version.as_deref(), Some("0.0.1"));
        assert_eq!(out.added_env, ["TRUSTED_ORIGIN"]);
        assert!(!out.deployment_id.is_empty());
        let after = app_named(&state, "links").await;
        assert_eq!(after.compose_content, before.compose_content);
        assert_eq!(after.template_version, before.template_version);
        let env_after = secrets::app_env(&after).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            env_after, env_before,
            "every value kept, the missing one made again"
        );

        // A rollback to the older file puts the app back on that version.
        let mut tx = crate::db::begin_write(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        template_apps::follow_restored_compose(&mut tx, &after.id, &old_compose)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        tx.commit().await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            app_named(&state, "links").await.template_version.as_deref(),
            Some("0.0.1")
        );
    }
}
