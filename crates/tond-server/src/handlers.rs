// handlers.rs — all app management HTTP handlers
//
// This file is the main request-handling surface of tond-server. Each
// function maps directly to an Axum route registered in main.rs.
//
// All handlers except `health` require both:
//   _rate: RateLimit          — enforces per-IP token bucket (429 if exceeded)
//   auth: Authz<scope::X>     — authenticated caller holding scope X (401/403)
//
// Return type convention:
//   All handlers return Result<impl IntoResponse, AppError>. AppError wraps
//   TondError and implements IntoResponse (in crate::error), mapping to the
//   appropriate HTTP status code with a consistent JSON error body.
//
// Handler summary:
//
//   health()            GET  /health              — liveness probe, no auth
//   list_apps()         GET  /api/apps             — all apps ordered by created_at
//   get_app()           GET  /api/apps/{id}        — single app by ID or name
//   create_app()        POST /api/apps             — validate name, insert, 409 on duplicate
//   delete_app()        DEL  /api/apps/{id}        — remove container (best-effort) then DB row
//   deploy_app()        POST /api/apps/{id}/deploy — spawns background deploy task, returns 202
//   stop_app()          POST /api/apps/{id}/stop   — Docker stop + set status=stopped
//   start_app()         POST /api/apps/{id}/start  — Docker start + set status=running
//   restart_app()       POST /api/apps/{id}/restart— sequential stop+start
//   app_logs()          GET  /api/apps/{id}/logs   — last 50 deploy_logs + 100 container lines
//   (env vars live in handlers/env.rs: masked GET, PATCH, per-key reveal)
//   update_domain()     PUT  /api/apps/{id}/domain — set domain, upsert 'pending' cert record
//   update_port()       PUT  /api/apps/{id}/port   — container port (next deploy)
//   (certificates live in handlers/certificates.rs)
//   upload_deploy()     POST /api/apps/{id}/upload — accept .tar.gz, extract, build,
//                                                    then deploy — all in background, returns 202
//
// Deploy flow for git source (triggered by deploy_app or upload_deploy):
//   handler spawns tokio::spawn -> deploy::deploy() -> docker::run_container()
//   The handler returns 202 immediately; the caller polls app status.

pub mod alerts;
pub mod audit_log;
pub mod auth;
pub mod backups;
pub mod certificates;
pub mod cron;
pub mod database_links;
pub mod databases;
pub mod deployments;
pub mod domains;
pub mod env;
pub mod git;
pub mod github_apps;
pub mod gitlab_tokens;
pub mod images;
pub mod integrations;
pub mod logs;
pub mod metrics;
pub mod onboarding;
pub mod previews;
pub mod project_previews;
pub mod projects;
pub mod registry_credentials;
pub mod settings;
pub mod storage_providers;
pub mod system_self;
pub mod templates;
pub mod terminal;
pub mod tokens;
pub mod users;
pub mod webhooks;

use axum::{
    Json,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::{
    audit_log::write_audit_log,
    auth::{Authz, Principal, RateLimit, principal::Actor, scope},
    compose_policy, deploy, docker, domains as domain_rules,
    error::AppError,
    networks, secrets,
    state::AppState,
    upload, volumes,
};

// ── Health ──

pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok", "service": "tond" }))
}

/// Env vars from a request's JSON object: string values are kept, other
/// scalars are stringified, keys must be valid variable names.
pub(crate) fn env_map_from_json(
    value: &serde_json::Value,
) -> Result<std::collections::BTreeMap<String, String>, AppError> {
    if value.is_null() {
        return Ok(Default::default());
    }
    let object = value.as_object().ok_or_else(|| {
        AppError(TondError::InvalidInput(
            "env_vars must be an object of NAME: value pairs".to_string(),
        ))
    })?;
    let mut env = std::collections::BTreeMap::new();
    for (key, value) in object {
        if !is_valid_env_key(key) {
            return Err(AppError(TondError::InvalidInput(format!(
                "'{key}' is not a valid variable name: use letters, digits and underscores, \
                 not starting with a digit"
            ))));
        }
        let value = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        };
        env.insert(key.clone(), value);
    }
    Ok(env)
}

// ── Shared lookup helper ──
//
// Accepts either a UUID or an app name in the `id_or_name` path segment.
// Returns TondError::AppNotFound when the row is missing.

async fn fetch_app(db: &sqlx::SqlitePool, id_or_name: &str) -> Result<App, AppError> {
    let app = sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1")
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(db)
        .await?;

    app.ok_or_else(|| AppError(TondError::AppNotFound(id_or_name.to_string())))
}

// ── List Apps ──
//
// Supports an optional `?name=<app_name>` query parameter for programmatic
// name-based lookup. When present, returns only the matching app (or 404).

#[derive(Deserialize)]
pub struct ListAppsQuery {
    pub name: Option<String>,
}

pub async fn list_apps(
    _rate: RateLimit,
    auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Query(query): Query<ListAppsQuery>,
) -> Result<impl IntoResponse, AppError> {
    // If ?name= is provided, return just that one app (or 404).
    if let Some(name) = query.name {
        let app = fetch_app(&state.db, &name).await?;
        if !auth.can_see_app(&app.id, app.project_id.as_deref()) {
            return Err(AppError(TondError::AppNotFound(name)));
        }
        let responses = vec![AppResponse::from(app)];
        return Ok((StatusCode::OK, Json(ApiResponse::ok(responses))));
    }

    let apps = sqlx::query_as::<_, App>("SELECT * FROM apps ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;

    // Tokens limited to some apps only see those.
    let responses: Vec<AppResponse> = apps
        .into_iter()
        .filter(|a| auth.can_see_app(&a.id, a.project_id.as_deref()))
        .map(AppResponse::from)
        .collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── Get App ──

pub async fn get_app(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let mut response = AppResponse::from(app);
    response.domain_warning = domain_warning(&state, response.domain.as_deref()).await;
    Ok((StatusCode::OK, Json(ApiResponse::ok(response))))
}

/// The "same site as your dashboard" warning for an app domain, if it applies.
async fn domain_warning(state: &AppState, domain: Option<&str>) -> Option<String> {
    let domain = domain?;
    let dashboard = state.settings.read().await.dashboard_domain.clone();
    domain_rules::same_site(domain, &dashboard)
        .then(|| domain_rules::same_site_warning(domain, &dashboard))
}

/// Normalize and validate a custom app domain against the current settings.
/// Returns the normalized domain and the same-site warning, if any.
async fn check_app_domain(
    state: &AppState,
    domain: &str,
) -> Result<(String, Option<String>), AppError> {
    let domain = domain_rules::normalize_domain(domain);
    let (dashboard, preview) = {
        let s = state.settings.read().await;
        (s.dashboard_domain.clone(), s.preview_domain.clone())
    };
    let warning = domain_rules::check_custom_app_domain(&domain, &dashboard, &preview)
        .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    Ok((domain, warning))
}

// ── Create App ──

pub async fn create_app(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateAppRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Volumes and compose files reach the host: they also need an admin scope.
    if req.volumes.as_ref().is_some_and(|v| !v.is_empty()) {
        auth.require_to(Scope::SettingsWrite, "configure volumes")?;
    }
    if req.source_type == "compose" {
        auth.require_to(Scope::SettingsWrite, "deploy Docker Compose apps")?;
    }

    // Validate name (alphanumeric + hyphens)
    if !req.name.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(AppError(TondError::InvalidInput(
            "App name must be alphanumeric (hyphens allowed)".to_string(),
        )));
    }

    // Validate source_type
    if !matches!(
        req.source_type.as_str(),
        "git" | "image" | "tarball" | "dockerfile" | "compose"
    ) {
        return Err(AppError(TondError::InvalidInput(
            "source_type must be one of: git, image, tarball, dockerfile, compose".to_string(),
        )));
    }

    // Validate inline content is present for the inline source types
    if req.source_type == "dockerfile"
        && req
            .dockerfile_content
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(AppError(TondError::InvalidInput(
            "dockerfile_content is required when source_type is 'dockerfile'".to_string(),
        )));
    }
    if req.source_type == "compose"
        && req
            .compose_content
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(AppError(TondError::InvalidInput(
            "compose_content is required when source_type is 'compose'".to_string(),
        )));
    }
    // Reject host-level compose options up front (privileged, host binds, …) so
    // the user sees every problem now instead of at deploy time.
    if req.source_type == "compose"
        && let Some(content) = req.compose_content.as_deref()
    {
        let volume_dir = volumes::app_volume_dir(&volumes::data_dir(), &req.name);
        compose_policy::validate(content, &volume_dir).map_err(|errors| {
            AppError(TondError::InvalidInput(compose_policy::format_errors(
                &errors,
            )))
        })?;
    }

    let requested_build_method = req.build_method.clone();
    if let Some(build_method) = requested_build_method.as_deref()
        && !matches!(build_method, "dockerfile" | "tbuild")
    {
        return Err(AppError(TondError::InvalidInput(
            "build_method must be one of: tbuild, dockerfile".to_string(),
        )));
    }

    match req.source_type.as_str() {
        "dockerfile" => {
            if let Some(build_method) = requested_build_method.as_deref()
                && build_method != "dockerfile"
            {
                return Err(AppError(TondError::InvalidInput(
                    "build_method must be 'dockerfile' when source_type is 'dockerfile'"
                        .to_string(),
                )));
            }
        }
        "git" | "tarball" => {}
        _ => {
            if requested_build_method.is_some() {
                return Err(AppError(TondError::InvalidInput(
                    "build_method is only supported for git, tarball, and dockerfile sources"
                        .to_string(),
                )));
            }
        }
    }

    let mut app = App::new(
        req.name.clone(),
        req.source_type,
        req.source_url,
        req.branch,
        req.container_port,
    );

    if let Some(domain) = req.domain.filter(|d| !d.trim().is_empty()) {
        app.domain = Some(check_app_domain(&state, &domain).await?.0);
    }

    let env = match req.env_vars {
        Some(value) => env_map_from_json(&value)?,
        None => Default::default(),
    };
    secrets::seal_new_app(&mut app, &env)?;

    if let Some(memory_limit) = req.memory_limit {
        app.memory_limit = Some(memory_limit);
    }

    if let Some(cpu_limit) = req.cpu_limit {
        app.cpu_limit = Some(cpu_limit);
    }

    if let Some(drain_timeout) = req.drain_timeout {
        app.drain_timeout = drain_timeout;
    }

    if let Some(build_method) = requested_build_method {
        app.build_method = build_method;
    }

    match app.source_type.as_str() {
        "dockerfile" => {
            app.build_method = "dockerfile".to_string();
        }
        "image" | "compose" => {
            app.build_method.clear();
        }
        _ => {}
    }

    if let Some(health_path) = req.health_check_path {
        app.health_check_path = Some(health_path);
    }

    if let Some(interval) = req.health_check_interval {
        app.health_check_interval = interval;
    }

    if let Some(threshold) = req.health_failure_threshold {
        app.health_failure_threshold = threshold;
    }

    // Phase 4 fields
    app.auto_deploy = if req.auto_deploy { 1 } else { 0 };
    app.watch_branch = req.watch_branch;

    // Optional association fields
    if let Some(project_id) = req.project_id {
        app.project_id = Some(project_id);
    }
    if let Some(github_app_id) = req.github_app_id {
        app.github_app_id = Some(github_app_id);
    }
    if let Some(registry_credential_id) = req.registry_credential_id {
        app.registry_credential_id = Some(registry_credential_id);
    }
    if let Some(reqs) = req.volumes
        && !reqs.is_empty()
    {
        let vols = volumes::app_volumes(&volumes::data_dir(), &app.name, &reqs)
            .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
        app.volumes = serde_json::to_string(&vols)
            .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    }
    if let Some(content) = req.dockerfile_content {
        app.dockerfile_content = Some(content);
    }
    if let Some(content) = req.compose_content {
        app.compose_content = Some(content);
    }
    if let Some(dir) = req.base_dir {
        let dir = deploy::validate_base_dir(&dir)
            .map_err(|e| AppError(TondError::InvalidInput(e.to_string())))?;
        app.base_dir = (!dir.is_empty()).then_some(dir);
    }
    app.static_build = if req.static_build { 1 } else { 0 };
    apply_readiness(
        &mut app,
        req.readiness.as_deref(),
        req.ready_after_secs,
        ReadinessChange::Create,
    )?;

    sqlx::query(
		"INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, status, env_vars, memory_limit, cpu_limit, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, webhook_secret, auto_deploy, watch_branch, project_id, github_app_id, registry_credential_id, dockerfile_content, compose_content, base_dir, static_build, readiness, ready_after_secs, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
	)
	.bind(&app.id)
	.bind(&app.name)
	.bind(&app.domain)
	.bind(&app.source_type)
	.bind(&app.source_url)
	.bind(&app.branch)
	.bind(app.container_port)
	.bind(&app.container_id)
	.bind(&app.status)
	.bind(&app.env_vars)
	.bind(app.memory_limit)
	.bind(app.cpu_limit)
	.bind(&app.health_check_path)
	.bind(app.health_check_interval)
	.bind(app.health_failure_threshold)
	.bind(app.health_consecutive_failures)
	.bind(&app.volumes)
	.bind(app.drain_timeout)
	.bind(&app.build_method)
	.bind(&app.webhook_secret)
	.bind(app.auto_deploy)
	.bind(&app.watch_branch)
	.bind(&app.project_id)
	.bind(&app.github_app_id)
	.bind(&app.registry_credential_id)
	.bind(&app.dockerfile_content)
	.bind(&app.compose_content)
	.bind(&app.base_dir)
	.bind(app.static_build)
	.bind(&app.readiness)
	.bind(app.ready_after_secs)
	.bind(&app.created_at)
	.bind(&app.updated_at)
	.execute(&state.db)
	.await
	.map_err(|e| {
		if e.to_string().contains("UNIQUE") {
			AppError(TondError::AppAlreadyExists(req.name.clone()))
		} else {
			AppError(TondError::DatabaseError(e))
		}
	})?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "create",
        &auth.audit_actor(),
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(AppResponse::from(app))),
    ))
}

// ── Validate Compose ──
//
// POST /api/compose/validate — lets the dashboard show compose policy problems
// under the editor while the user types, before they create the app.

#[derive(Deserialize)]
pub struct ValidateComposeRequest {
    /// App name, used to show where bind mounts will be stored (default "app")
    pub app_name: Option<String>,
    pub compose_content: String,
}

pub async fn validate_compose(
    _rate: RateLimit,
    _auth: Authz<scope::AppsWrite>,
    Json(req): Json<ValidateComposeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let name = req
        .app_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '-'))
        .unwrap_or("app");
    let volume_dir = volumes::app_volume_dir(&volumes::data_dir(), name);
    let errors = match compose_policy::validate(&req.compose_content, &volume_dir) {
        Ok(()) => Vec::new(),
        Err(errors) => errors,
    };
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "valid": errors.is_empty(), "errors": errors }),
        )),
    ))
}

// ── Delete App ──

pub async fn delete_app(
    _rate: RateLimit,
    auth: Authz<scope::AppsDelete>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    // Stop and remove container if running (best-effort). Compose apps store a
    // sentinel instead of a container id, so remove the whole compose project.
    if app.source_type == "compose" {
        docker::remove_compose_project(&state.docker, &deploy::compose_project_name(&app.name))
            .await;
    } else if let Some(container_id) = &app.container_id {
        let _ = docker::remove_container(&state.docker, container_id).await;
    }
    // The app's own network (linked databases are detached with it; the
    // links themselves go with the row).
    if app.project_id.is_none() {
        docker::remove_network(&state.docker, &networks::app_network(&app.name)).await;
    }

    sqlx::query("DELETE FROM apps WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "delete",
        &auth.audit_actor(),
        None,
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "deleted": app.id }))),
    ))
}

// ── Deploy App ──

/// How a deploy request was triggered, for the deployment row:
/// `(trigger_source, trigger_detail)`.
pub(crate) fn deploy_trigger(principal: &Principal) -> (&'static str, Option<String>) {
    match principal.actor() {
        Actor::LocalSocket => ("cli", Some("tond CLI on the server".to_string())),
        Actor::Token { name, .. } => ("api", Some(format!("token {name}"))),
        Actor::User { email, .. } => ("manual", Some(email.clone())),
    }
}

/// 202 body for a started deployment.
pub(crate) fn deployment_started(
    message: &str,
    app_id: &str,
    queued: &deploy::queue::Enqueued,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    (
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(queued_response(message, app_id, queued))),
    )
}

pub async fn deploy_app(
    _rate: RateLimit,
    auth: Authz<scope::AppsDeploy>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let app_id = app.id.clone();
    let app_name = app.name.clone();

    let (trigger, detail) = deploy_trigger(&auth);
    let queued = deploy::start(&state, app, deploy::DeployRequest::source(trigger, detail))
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    write_audit_log(
        &state.db,
        Some(&app_id),
        Some(&app_name),
        "deploy",
        &auth.audit_actor(),
        Some(&format!("deployment={}", queued.deployment_id)),
    )
    .await;

    Ok(deployment_started("Deployment queued", &app_id, &queued))
}

/// Body of a 202 answer to a queued deployment (deploy/queue.rs).
pub(crate) fn queued_response(
    message: &str,
    app_id: &str,
    queued: &deploy::queue::Enqueued,
) -> serde_json::Value {
    json!({
        "message": if queued.queue_position > 1 {
            format!("{message} (position {} in the queue)", queued.queue_position)
        } else {
            message.to_string()
        },
        "app_id": app_id,
        "deployment_id": queued.deployment_id,
        "status": queued.status,
        "queue_position": queued.queue_position,
        "superseded": queued.superseded,
    })
}

// ── Stop App ──

pub async fn stop_app(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let container_id = app
        .container_id
        .as_deref()
        .ok_or_else(|| AppError(TondError::InvalidInput("App has no container".to_string())))?;

    docker::stop_container(&state.docker, container_id)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    sqlx::query("UPDATE apps SET status = 'stopped', updated_at = datetime('now') WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "stop",
        &auth.audit_actor(),
        None,
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "App stopped" }))),
    ))
}

// ── Start App ──

pub async fn start_app(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let container_id = app
        .container_id
        .as_deref()
        .ok_or_else(|| AppError(TondError::InvalidInput("App has no container".to_string())))?;

    docker::start_container(&state.docker, container_id)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    sqlx::query("UPDATE apps SET status = 'running', updated_at = datetime('now') WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "start",
        &auth.audit_actor(),
        None,
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "App started" }))),
    ))
}

// ── Restart App ──

pub async fn restart_app(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let container_id = app
        .container_id
        .as_deref()
        .ok_or_else(|| AppError(TondError::InvalidInput("App has no container".to_string())))?;

    // stop is best-effort; ignore the error and proceed to start
    let _ = docker::stop_container(&state.docker, container_id).await;

    docker::start_container(&state.docker, container_id)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;

    sqlx::query("UPDATE apps SET status = 'running', updated_at = datetime('now') WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "restart",
        &auth.audit_actor(),
        None,
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "App restarted" }))),
    ))
}

// ── App Logs ──

pub async fn app_logs(
    _rate: RateLimit,
    _auth: Authz<scope::LogsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    // Get deploy logs from DB
    let deploy_logs = sqlx::query_as::<_, DeployLog>(
        "SELECT * FROM deploy_logs WHERE app_id = ? ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&app.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Get container logs if running
    let container_logs = if let Some(container_id) = &app.container_id {
        docker::get_logs(&state.docker, container_id, 100)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "deploy_logs": deploy_logs,
            "container_logs": container_logs,
        }))),
    ))
}

// ── Update Domain ──

pub async fn update_domain(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDomainRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let (domain, warning) = check_app_domain(&state, &req.domain).await?;
    let req = UpdateDomainRequest { domain };

    let taken: Option<String> =
        sqlx::query_scalar("SELECT name FROM apps WHERE domain = ? AND id != ? LIMIT 1")
            .bind(&req.domain)
            .bind(&app.id)
            .fetch_optional(&state.db)
            .await?;
    if let Some(other) = taken {
        return Err(AppError(TondError::InvalidInput(format!(
            "{} is already used by the app '{other}'",
            req.domain
        ))));
    }

    sqlx::query("UPDATE apps SET domain = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&req.domain)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    // Create a pending certificate record for ACME provisioning
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let certs_dir = format!("{}/certs", data_dir);
    let cert_path = format!("{}/{}/cert.pem", certs_dir, req.domain);
    let key_path = format!("{}/{}/key.pem", certs_dir, req.domain);

    let cert_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    // Placeholder expiry — updated when cert is actually provisioned
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(90)).to_rfc3339();

    // Insert or update certificate record (best-effort; don't fail the domain update)
    let _ = sqlx::query(
		"INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at, status, created_at, updated_at)
         VALUES (?, ?, 'acme', ?, ?, ?, ?, 'pending', ?, ?)
         ON CONFLICT(domain) DO UPDATE SET status = 'pending', updated_at = ?,
            last_error = NULL, next_retry_at = NULL, failure_count = 0",
	)
	.bind(&cert_id)
	.bind(&req.domain)
	.bind(&cert_path)
	.bind(&key_path)
	.bind(&now)
	.bind(&expires_at)
	.bind(&now)
	.bind(&now)
	.bind(&now)
	.execute(&state.db)
	.await;

    tracing::info!(
        "Domain set to {}, certificate provisioning queued",
        req.domain
    );

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "domain_update",
        &auth.audit_actor(),
        Some(&format!("domain={}", req.domain)),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "message": "Domain updated, certificate provisioning queued",
            "domain": req.domain,
            "warning": warning,
        }))),
    ))
}

// ── Update Health Check Config ──

pub async fn update_health(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateHealthRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let interval = req.health_check_interval.unwrap_or(30);
    let threshold = req.health_failure_threshold.unwrap_or(3);

    sqlx::query(
        "UPDATE apps SET health_check_path = ?, health_check_interval = ?, health_failure_threshold = ?, health_consecutive_failures = 0, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&req.health_check_path)
    .bind(interval)
    .bind(threshold)
    .bind(&app.id)
    .execute(&state.db)
    .await?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("health_update").resource("app", &app.id, &app.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Health check configuration updated" }),
        )),
    ))
}

// ── Add Volume ──

pub async fn add_volume(
    _rate: RateLimit,
    auth: Authz<(scope::AppsWrite, scope::SettingsWrite)>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<VolumeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let mut volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();

    // Map the request into <data>/volumes/<app>/<name>; arbitrary host paths are refused.
    let volume = volumes::add_app_volume(&volumes::data_dir(), &app.name, &volumes, &req)
        .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    let detail = format!("{} -> {}", volume.host_path, volume.container_path);
    volumes.push(volume);

    let volumes_json = serde_json::to_string(&volumes)
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    sqlx::query("UPDATE apps SET volumes = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&volumes_json)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "volume_add",
        &auth.audit_actor(),
        Some(&detail),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Volume added. Redeploy to apply.", "volumes": volumes }),
        )),
    ))
}

// ── Remove Volume ──

pub async fn remove_volume(
    _rate: RateLimit,
    auth: Authz<(scope::AppsWrite, scope::SettingsWrite)>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<VolumeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    let mut volumes: Vec<Volume> = serde_json::from_str(&app.volumes).unwrap_or_default();
    let before = volumes.len();
    volumes.retain(|v| v.container_path != req.container_path);

    if volumes.len() == before {
        return Err(AppError(TondError::InvalidInput(format!(
            "No volume at {}",
            req.container_path
        ))));
    }

    let volumes_json = serde_json::to_string(&volumes)
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    sqlx::query("UPDATE apps SET volumes = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&volumes_json)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("volume_remove")
            .resource("app", &app.id, &app.name)
            .detail(req.container_path.clone()),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Volume removed. Redeploy to apply.", "volumes": volumes }),
        )),
    ))
}

// ── Update Resource Limits ──

pub async fn update_app_resources(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAppResourcesRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    sqlx::query(
        "UPDATE apps SET memory_limit = ?, cpu_limit = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(req.memory_limit)
    .bind(req.cpu_limit)
    .bind(&app.id)
    .execute(&state.db)
    .await?;

    let updated = fetch_app(&state.db, &app.id).await?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "resources_update",
        &auth.audit_actor(),
        Some(&format!(
            "memory_limit={:?},cpu_limit={:?}",
            req.memory_limit, req.cpu_limit
        )),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

// ── Update Drain Timeout ──

pub async fn update_drain_timeout(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDrainTimeoutRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    if req.drain_timeout < 0 {
        return Err(AppError(TondError::InvalidInput(
            "drain_timeout must be non-negative".to_string(),
        )));
    }

    sqlx::query("UPDATE apps SET drain_timeout = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(req.drain_timeout)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("drain_timeout_update").resource("app", &app.id, &app.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Drain timeout updated", "drain_timeout": req.drain_timeout }),
        )),
    ))
}

/// Stream the multipart `file` field into `upload::extract_archive`, running
/// on a blocking thread. Errors are user-facing (400/413 with what to do).
async fn receive_upload(
    multipart: &mut Multipart,
    dest: &std::path::Path,
    max_bytes: u64,
) -> Result<(), AppError> {
    let mut extracted = false;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                return Err(AppError(TondError::InvalidInput(format!(
                    "Failed to read upload: {e}"
                ))));
            }
        };
        if field.name() != Some("file") || extracted {
            continue;
        }
        extracted = true;

        let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<bytes::Bytes>>(8);
        let dest_owned = dest.to_path_buf();
        let job = tokio::task::spawn_blocking(move || {
            upload::extract_archive(
                upload::ArchiveReader::new(rx, max_bytes),
                &dest_owned,
                upload::ExtractLimits::with_max_bytes(max_bytes),
            )
        });

        let mut field = field;
        let mut received: u64 = 0;
        let mut extractor_stopped = false;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    received += chunk.len() as u64;
                    // A send error means the extractor already stopped (it
                    // refused the archive); its result says why. Keep reading
                    // (and discarding) a bounded amount so the client gets
                    // the error response instead of a reset connection.
                    if !extractor_stopped && tx.send(Ok(chunk)).await.is_err() {
                        extractor_stopped = true;
                    }
                    if extractor_stopped && received > max_bytes.saturating_mul(2) {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx
                        .send(Err(std::io::Error::other(format!(
                            "the upload was interrupted: {e}"
                        ))))
                        .await;
                    break;
                }
            }
        }
        drop(tx);

        let result = job.await.map_err(|e| {
            AppError(TondError::Internal(format!(
                "upload extraction failed: {e}"
            )))
        })?;
        match result {
            Ok(stats) => tracing::info!(
                "Upload extracted: {} files, {} bytes",
                stats.files,
                stats.bytes
            ),
            Err(upload::UploadError::TooLarge(msg)) => {
                return Err(AppError(TondError::PayloadTooLarge(msg)));
            }
            Err(upload::UploadError::Invalid(msg)) => {
                return Err(AppError(TondError::InvalidInput(format!(
                    "Upload refused: {msg}"
                ))));
            }
            Err(upload::UploadError::Io(msg)) => {
                return Err(AppError(TondError::Internal(format!(
                    "Could not unpack the upload: {msg}"
                ))));
            }
        }
    }
    if !extracted {
        return Err(AppError(TondError::InvalidInput(
            "The upload has no 'file' part: send the archive as multipart field 'file'".to_string(),
        )));
    }
    Ok(())
}

// ── Readiness (web app / TCP service / background worker) ──

/// Whether a readiness change comes from creating the app or editing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadinessChange {
    Create,
    Update,
}

/// Validate and apply a readiness mode to `app`. Background workers have no
/// domain and no health check path: on create a domain is refused (it would
/// never be routed); on update switching to a worker clears them.
pub(crate) fn apply_readiness(
    app: &mut App,
    readiness: Option<&str>,
    ready_after_secs: Option<i64>,
    change: ReadinessChange,
) -> Result<(), AppError> {
    let invalid = |msg: &str| AppError(TondError::InvalidInput(msg.to_string()));
    if let Some(value) = readiness {
        let mode = Readiness::from_input(value)
            .ok_or_else(|| invalid("readiness must be one of: http, tcp, none"))?;
        app.readiness = mode.as_str().to_string();
    }
    if let Some(secs) = ready_after_secs {
        if !READY_AFTER_SECS_RANGE.contains(&secs) {
            return Err(invalid("ready_after_secs must be between 1 and 600"));
        }
        app.ready_after_secs = secs;
    }
    if app.readiness() == Readiness::None {
        if app.source_type == "compose" {
            return Err(invalid(
                "Compose apps can't be background workers; publish a web service port instead",
            ));
        }
        if change == ReadinessChange::Create && app.domain.is_some() {
            return Err(invalid(
                "Background workers have no domain: they serve no traffic. Leave the domain empty.",
            ));
        }
        app.domain = None;
        app.health_check_path = None;
    }
    Ok(())
}

/// PUT /api/apps/{id}/readiness body.
#[derive(Debug, Deserialize)]
pub struct UpdateReadinessRequest {
    /// "http" | "tcp" | "none"
    pub readiness: String,
    /// For "none": seconds the container must stay up (default: unchanged)
    pub ready_after_secs: Option<i64>,
}

// PUT /api/apps/{id}/readiness
// How deploys decide the new container is ready. Switching to a background
// worker ("none") removes the app's domain and health check path: a worker
// serves no traffic and is never routed. Takes effect on the next deploy.
pub async fn update_readiness(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateReadinessRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mut app = fetch_app(&state.db, &id).await?;
    let had_domain = app.domain.clone();
    apply_readiness(
        &mut app,
        Some(&req.readiness),
        req.ready_after_secs,
        ReadinessChange::Update,
    )?;
    sqlx::query(
        "UPDATE apps SET readiness = ?, ready_after_secs = ?, domain = ?, health_check_path = ?,
                updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&app.readiness)
    .bind(app.ready_after_secs)
    .bind(&app.domain)
    .bind(&app.health_check_path)
    .bind(&app.id)
    .execute(&state.db)
    .await?;
    let removed = match (&had_domain, &app.domain) {
        (Some(d), None) => format!(" domain_removed={d}"),
        _ => String::new(),
    };
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "readiness_update",
        &auth.audit_actor(),
        Some(&format!(
            "readiness={} ready_after_secs={}{removed}",
            app.readiness, app.ready_after_secs
        )),
    )
    .await;
    let updated = fetch_app(&state.db, &app.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

// ── Update Container Port ──
//
// PUT /api/apps/{id}/port  { "container_port": 80 }
// The port the app listens on inside its container (also passed as $PORT).
// Takes effect on the next deploy.

#[derive(Debug, Deserialize)]
pub struct UpdatePortRequest {
    pub container_port: i64,
}

pub async fn update_port(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdatePortRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    if !(1..=65535).contains(&req.container_port) {
        return Err(AppError(TondError::InvalidInput(
            "container_port must be between 1 and 65535".to_string(),
        )));
    }
    sqlx::query("UPDATE apps SET container_port = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(req.container_port)
        .bind(&app.id)
        .execute(&state.db)
        .await?;
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "port_update",
        &auth.audit_actor(),
        Some(&format!("container_port={}", req.container_port)),
    )
    .await;
    let updated = fetch_app(&state.db, &app.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AppResponse::from(updated))),
    ))
}

pub async fn upload_deploy(
    _rate: RateLimit,
    auth: Authz<scope::AppsDeploy>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let mut app = fetch_app(&state.db, &id).await?;

    // Extract the archive while it streams in, into a fresh private directory
    // (see upload.rs for everything that is refused).
    let data_dir = volumes::data_dir();
    let extract_path = upload::create_upload_dir(&data_dir).map_err(|e| {
        AppError(TondError::Internal(format!(
            "Failed to create upload dir: {e}"
        )))
    })?;
    let max_bytes = state.settings.read().await.upload_max_bytes;
    if let Err(e) = receive_upload(&mut multipart, &extract_path, max_bytes).await {
        let _ = tokio::fs::remove_dir_all(&extract_path).await;
        return Err(e);
    }
    let extract_dir = extract_path.to_string_lossy().into_owned();

    // Update source type
    sqlx::query(
        "UPDATE apps SET source_type = 'tarball', updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&app.id)
    .execute(&state.db)
    .await?;
    app.source_type = "tarball".to_string();

    let app_id = app.id.clone();
    let app_name = app.name.clone();

    // Build and deploy in the background as one deployment (build phase
    // included), so its log and progress show in the live deploy view.
    let queued = deploy::start(
        &state,
        app,
        deploy::DeployRequest {
            kind: deploy::DeployKind::Upload { dir: extract_dir },
            trigger_source: "upload".to_string(),
            trigger_detail: deploy_trigger(&auth).1,
        },
    )
    .await
    .map_err(|e| AppError(TondError::Internal(e.to_string())))?;

    write_audit_log(
        &state.db,
        Some(&app_id),
        Some(&app_name),
        "deploy",
        &auth.audit_actor(),
        Some(&format!(
            "source=upload deployment={}",
            queued.deployment_id
        )),
    )
    .await;

    Ok(deployment_started(
        "Upload received; build queued",
        &app_id,
        &queued,
    ))
}

// ── Assign / Remove Project ──
//
// PUT /api/apps/{id}/project
//   { "project_id": "<uuid>" }  — assign app to project
//   { "project_id": null }      — remove app from project

pub async fn assign_project(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AssignProjectRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;

    // Changing the project moves the app to another Docker network, which
    // changes which databases it can reach (networks.rs).
    if req.project_id != app.project_id {
        auth.require_to(Scope::DbWrite, "move an app to another project's network")?;
    }

    // If a project_id is provided, verify it exists
    if let Some(ref pid) = req.project_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?)")
            .bind(pid)
            .fetch_one(&state.db)
            .await?;
        if !exists {
            return Err(AppError(TondError::NotFound(format!("project '{}'", pid))));
        }
    }

    sqlx::query("UPDATE apps SET project_id = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&req.project_id)
        .bind(&app.id)
        .execute(&state.db)
        .await?;

    // Move the running container to its new home network (project or own)
    // and re-attach linked databases there; drop the now-unused own network.
    if req.project_id != app.project_id {
        let moved = App {
            project_id: req.project_id.clone(),
            ..app.clone()
        };
        networks::reconcile_app(&state, &moved)
            .await
            .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
        if moved.project_id.is_some() {
            docker::remove_network(&state.docker, &networks::app_network(&app.name)).await;
        }
    }

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "assign_project",
        &auth.audit_actor(),
        Some(&format!(
            "project_id={}",
            req.project_id.as_deref().unwrap_or("null")
        )),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "App project assignment updated" }),
        )),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Json,
        body::to_bytes,
        extract::{Path, Query, State},
        http::StatusCode,
        response::IntoResponse,
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{
        AssignProjectRequest, CreateAppRequest, ServerSettings, UpdateDomainRequest,
        UpdateDrainTimeoutRequest, UpdateHealthRequest, VolumeRequest,
    };

    use super::*;
    use crate::{
        auth::{Authz, Principal, RateLimiter, rate_limit::RateLimit},
        state::AppState,
    };

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for app handler tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn seed_project(state: &AppState, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO projects (id, name, description, env_vars, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(Option::<String>::None)
        .bind("{}")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed project: {err}"));
    }

    async fn seed_app(state: &AppState, name: &str) -> App {
        let app = App::new(
            name.to_string(),
            "git".to_string(),
            Some("https://example.com/repo.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        sqlx::query(
            "INSERT INTO apps (id, name, domain, source_type, source_url, branch, container_port, container_id, status, env_vars, memory_limit, cpu_limit, health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures, volumes, drain_timeout, build_method, auto_deploy, watch_branch, project_id, github_app_id, registry_credential_id, dockerfile_content, compose_content, base_dir, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.domain)
        .bind(&app.source_type)
        .bind(&app.source_url)
        .bind(&app.branch)
        .bind(app.container_port)
        .bind(&app.container_id)
        .bind(&app.status)
        .bind(&app.env_vars)
        .bind(app.memory_limit)
        .bind(app.cpu_limit)
        .bind(&app.health_check_path)
        .bind(app.health_check_interval)
        .bind(app.health_failure_threshold)
        .bind(app.health_consecutive_failures)
        .bind(&app.volumes)
        .bind(app.drain_timeout)
        .bind(&app.build_method)
        .bind(app.auto_deploy)
        .bind(&app.watch_branch)
        .bind(&app.project_id)
        .bind(&app.github_app_id)
        .bind(&app.registry_credential_id)
        .bind(&app.dockerfile_content)
        .bind(&app.compose_content)
        .bind(&app.base_dir)
        .bind(app.static_build)
        .bind(&app.created_at)
        .bind(&app.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app: {err}"));
        app
    }

    #[tokio::test]
    async fn create_app_rejects_invalid_name() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "bad name".to_string(),
                source_type: "git".to_string(),
                source_url: Some("https://example.com/repo.git".to_string()),
                branch: Some("main".to_string()),
                container_port: 3000,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: None,
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject invalid app names"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: App name must be alphanumeric (hyphens allowed)",
            "create_app should reject names containing spaces or other unsupported characters"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_invalid_source_type() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo".to_string(),
                source_type: "svn".to_string(),
                source_url: Some("https://example.com/repo.git".to_string()),
                branch: Some("main".to_string()),
                container_port: 3000,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: None,
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject invalid source_type"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: source_type must be one of: git, image, tarball, dockerfile, compose",
            "create_app should reject unsupported source types"
        );
    }

    #[tokio::test]
    async fn create_app_requires_inline_content_for_dockerfile_source() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo".to_string(),
                source_type: "dockerfile".to_string(),
                source_url: None,
                branch: None,
                container_port: 3000,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: None,
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to require dockerfile_content"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: dockerfile_content is required when source_type is 'dockerfile'",
            "create_app should require inline Dockerfile content for dockerfile source type"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_invalid_build_method() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo".to_string(),
                source_type: "git".to_string(),
                source_url: Some("https://example.com/repo.git".to_string()),
                branch: Some("main".to_string()),
                container_port: 3000,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: Some("manual".to_string()),
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject invalid build_method"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: build_method must be one of: tbuild, dockerfile",
            "create_app should reject unsupported build methods"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_auto_build_method() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo".to_string(),
                source_type: "git".to_string(),
                source_url: Some("https://example.com/repo.git".to_string()),
                branch: Some("main".to_string()),
                container_port: 3000,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: Some("auto".to_string()),
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject auto build_method"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: build_method must be one of: tbuild, dockerfile",
            "create_app should reject the removed auto build method"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_nixpacks_build_method() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo".to_string(),
                source_type: "git".to_string(),
                source_url: Some("https://example.com/repo.git".to_string()),
                branch: Some("main".to_string()),
                container_port: 3000,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: Some("nixpacks".to_string()),
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject nixpacks build_method"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: build_method must be one of: tbuild, dockerfile",
            "create_app should reject the removed nixpacks build method"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_build_method_for_image_source() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo-image".to_string(),
                source_type: "image".to_string(),
                source_url: Some("nginx:alpine".to_string()),
                branch: None,
                container_port: 8080,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: Some("tbuild".to_string()),
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject build_method for image source"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: build_method is only supported for git, tarball, and dockerfile sources",
            "create_app should reject build_method when the source type does not build source code"
        );
    }

    #[tokio::test]
    async fn create_app_rejects_non_dockerfile_build_method_for_dockerfile_source() {
        let state = test_state().await;

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "demo-dockerfile".to_string(),
                source_type: "dockerfile".to_string(),
                source_url: None,
                branch: None,
                container_port: 8080,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: Some("tbuild".to_string()),
                dockerfile_content: Some("FROM nginx:alpine".to_string()),
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_app to reject non-dockerfile build_method"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: build_method must be 'dockerfile' when source_type is 'dockerfile'",
            "create_app should reject incompatible build_method for dockerfile source"
        );
    }

    #[tokio::test]
    async fn create_app_normalizes_dockerfile_source_build_method() {
        let state = test_state().await;
        let response = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "inline-dockerfile".to_string(),
                source_type: "dockerfile".to_string(),
                source_url: None,
                branch: None,
                container_port: 8080,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: None,
                dockerfile_content: Some("FROM nginx:alpine".to_string()),
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected dockerfile source creation to succeed: {}", err.0))
        .into_response();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body = response_json(response).await;
        assert_eq!(
            body["data"]["build_method"],
            serde_json::json!("dockerfile"),
            "dockerfile source apps should always expose dockerfile as the build method"
        );
    }

    #[tokio::test]
    async fn create_app_omits_build_method_for_image_source_response() {
        let state = test_state().await;
        let response = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateAppRequest {
                name: "image-app".to_string(),
                source_type: "image".to_string(),
                source_url: Some("nginx:alpine".to_string()),
                branch: None,
                container_port: 8080,
                domain: None,
                env_vars: None,
                memory_limit: None,
                cpu_limit: None,
                health_check_path: None,
                health_check_interval: None,
                health_failure_threshold: None,
                auto_deploy: false,
                watch_branch: None,
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: None,
                drain_timeout: None,
                build_method: None,
                dockerfile_content: None,
                compose_content: None,
                base_dir: None,
                static_build: false,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected image app creation to succeed: {}", err.0))
        .into_response();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body = response_json(response).await;
        assert!(
            body["data"]["build_method"].is_null(),
            "image source apps should omit build_method in API responses"
        );
    }

    #[tokio::test]
    async fn create_app_persists_selected_optional_fields() {
        let state = test_state().await;
        let response = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(CreateAppRequest {
                name: "demo".to_string(),
                source_type: "git".to_string(),
                source_url: Some("https://example.com/repo.git".to_string()),
                branch: Some("main".to_string()),
                container_port: 3000,
                domain: Some("demo.example.com".to_string()),
                env_vars: Some(serde_json::json!({ "MODE": "prod" })),
                memory_limit: Some(536_870_912),
                cpu_limit: Some(1_000_000_000),
                health_check_path: Some("/health".to_string()),
                health_check_interval: Some(15),
                health_failure_threshold: Some(5),
                auto_deploy: true,
                watch_branch: Some("main".to_string()),
                project_id: None,
                github_app_id: None,
                registry_credential_id: None,
                volumes: Some(vec![VolumeRequest {
                    container_path: "/data".to_string(),
                    name: None,
                    host_path: None,
                }]),
                drain_timeout: Some(45),
                build_method: Some("tbuild".to_string()),
                dockerfile_content: None,
                compose_content: None,
                base_dir: Some("services/api".to_string()),
                static_build: true,
                readiness: None,
                ready_after_secs: None,
            }),
        )
        .await
        .unwrap_or_else(|err| {
            panic!(
                "expected create_app to succeed with valid optional fields: {}",
                err.0
            )
        })
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "create_app should create the app when all provided fields are valid"
        );

        let body = response_json(response).await;
        assert_eq!(
            body["data"]["build_method"],
            serde_json::json!("tbuild"),
            "create_app should expose the selected build method in the response"
        );
        assert_eq!(
            body["data"]["static_build"],
            serde_json::json!(true),
            "create_app should persist the static_build flag"
        );
        assert_eq!(
            body["data"]["drain_timeout"],
            serde_json::json!(45),
            "create_app should persist the requested drain timeout"
        );
    }

    #[tokio::test]
    async fn list_apps_filters_by_name_query() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;
        let _other = seed_app(&state, "other").await;

        let response = list_apps(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Query(ListAppsQuery {
                name: Some("demo".to_string()),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected list_apps query filter to succeed: {}", err.0))
        .into_response();

        let body = response_json(response).await;
        let data = body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("expected list_apps response data to be an array"));
        assert_eq!(
            data.len(),
            1,
            "list_apps with ?name should only return the matching app"
        );
        assert_eq!(
            data[0]["id"],
            serde_json::json!(app.id),
            "list_apps with ?name should return the matching app record"
        );
    }

    #[tokio::test]
    async fn update_port_validates_and_saves_the_container_port() {
        let state = test_state().await;
        let app = seed_app(&state, "portapp").await;
        let bad = update_port(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(UpdatePortRequest { container_port: 0 }),
        )
        .await;
        assert!(bad.is_err(), "port 0 is rejected");
        update_port(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(UpdatePortRequest { container_port: 80 }),
        )
        .await
        .unwrap_or_else(|err| panic!("update_port should succeed: {}", err.0));
        let port: i64 = sqlx::query_scalar("SELECT container_port FROM apps WHERE id = ?")
            .bind(&app.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("read port: {err}"));
        assert_eq!(port, 80);
    }

    #[test]
    fn readiness_rules_for_workers() {
        let mut app = App::new("w".into(), "image".into(), None, None, 3000);
        app.domain = Some("w.example.com".into());
        app.health_check_path = Some("/health".into());
        assert!(
            apply_readiness(
                &mut app.clone(),
                Some("none"),
                None,
                ReadinessChange::Create
            )
            .is_err(),
            "a worker can't be created with a domain"
        );
        apply_readiness(&mut app, Some("none"), Some(20), ReadinessChange::Update)
            .unwrap_or_else(|e| panic!("update to worker: {}", e.0));
        assert_eq!(app.readiness(), Readiness::None);
        assert_eq!(app.ready_after_secs, 20);
        assert!(app.domain.is_none() && app.health_check_path.is_none());
        assert!(apply_readiness(&mut app, Some("udp"), None, ReadinessChange::Update).is_err());
        assert!(apply_readiness(&mut app, None, Some(0), ReadinessChange::Update).is_err());
        let mut compose = App::new("c".into(), "compose".into(), None, None, 80);
        assert!(
            apply_readiness(&mut compose, Some("none"), None, ReadinessChange::Create).is_err()
        );
        let mut tcp = App::new("t".into(), "image".into(), None, None, 6379);
        apply_readiness(&mut tcp, Some("tcp"), None, ReadinessChange::Create)
            .unwrap_or_else(|e| panic!("tcp: {}", e.0));
        assert_eq!(tcp.readiness(), Readiness::Tcp);
    }

    #[tokio::test]
    async fn update_readiness_switches_to_a_worker_and_drops_the_domain() {
        let state = test_state().await;
        let app = seed_app(&state, "jobs").await;
        sqlx::query("UPDATE apps SET domain = 'jobs.example.com' WHERE id = ?")
            .bind(&app.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("seed domain: {err}"));
        update_readiness(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(UpdateReadinessRequest {
                readiness: "none".into(),
                ready_after_secs: Some(15),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("update_readiness should succeed: {}", err.0));
        let (readiness, secs, domain): (String, i64, Option<String>) =
            sqlx::query_as("SELECT readiness, ready_after_secs, domain FROM apps WHERE id = ?")
                .bind(&app.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("read: {err}"));
        assert_eq!((readiness.as_str(), secs, domain), ("none", 15, None));
    }

    #[test]
    fn deploy_trigger_names_the_caller() {
        assert_eq!(deploy_trigger(&Principal::local_socket()).0, "cli");
        assert_eq!(
            deploy_trigger(&Principal::test_token(ScopeSet::all(), None)),
            ("api", Some("token test-token".to_string()))
        );
        assert_eq!(
            deploy_trigger(&Principal::test_user(Role::Deployer)),
            ("manual", Some("deployer@example.com".to_string()))
        );
    }

    #[tokio::test]
    async fn update_domain_sets_domain_and_creates_pending_certificate_record() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        let response = update_domain(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(UpdateDomainRequest {
                domain: "demo.example.com".to_string(),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected update_domain to succeed: {}", err.0))
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "update_domain should succeed for existing apps"
        );
        let domain: Option<String> = sqlx::query_scalar("SELECT domain FROM apps WHERE id = ?")
            .bind(&app.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to read updated domain: {err}"));
        assert_eq!(
            domain.as_deref(),
            Some("demo.example.com"),
            "update_domain should persist the requested domain on the app"
        );

        let cert: (String, String, String) =
            sqlx::query_as("SELECT domain, status, cert_path FROM certificates WHERE domain = ?")
                .bind("demo.example.com")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read queued certificate record: {err}"));
        assert_eq!(
            cert.0, "demo.example.com",
            "certificate record should target the requested domain"
        );
        assert_eq!(
            cert.1, "pending",
            "domain updates should queue certificate provisioning as pending"
        );
        assert!(
            cert.2.ends_with("/demo.example.com/cert.pem"),
            "certificate record should point at the expected on-disk cert path"
        );
    }

    #[tokio::test]
    async fn update_health_uses_defaults_when_optional_values_are_omitted() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        let response = update_health(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(UpdateHealthRequest {
                health_check_path: Some("/healthz".to_string()),
                health_check_interval: None,
                health_failure_threshold: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected update_health to succeed: {}", err.0))
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "update_health should succeed for existing apps"
        );
        let row: (Option<String>, i64, i64, i64) = sqlx::query_as(
            "SELECT health_check_path, health_check_interval, health_failure_threshold, health_consecutive_failures FROM apps WHERE id = ?",
        )
        .bind(&app.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to read updated health config: {err}"));
        assert_eq!(
            row.0.as_deref(),
            Some("/healthz"),
            "update_health should persist the new health path"
        );
        assert_eq!(
            row.1, 30,
            "update_health should default interval to 30 seconds when omitted"
        );
        assert_eq!(
            row.2, 3,
            "update_health should default threshold to 3 when omitted"
        );
        assert_eq!(
            row.3, 0,
            "update_health should reset consecutive failure count after updates"
        );
    }

    #[tokio::test]
    async fn add_volume_rejects_relative_container_paths() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        let error = add_volume(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Path(app.id),
            Json(VolumeRequest {
                container_path: "data".to_string(),
                name: None,
                host_path: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected add_volume to reject relative paths"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: Container path 'data' must be an absolute path inside the container, e.g. /data",
            "add_volume should require absolute container paths"
        );
    }

    #[tokio::test]
    async fn add_and_remove_volume_updates_persisted_volume_list() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        let add_response = add_volume(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(VolumeRequest {
                container_path: "/data/cache".to_string(),
                name: None,
                host_path: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected add_volume to succeed: {}", err.0))
        .into_response();
        assert_eq!(
            add_response.status(),
            StatusCode::OK,
            "add_volume should succeed for new absolute paths"
        );

        let volumes_after_add: String = sqlx::query_scalar("SELECT volumes FROM apps WHERE id = ?")
            .bind(&app.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to read volumes after add_volume: {err}"));
        assert!(
            volumes_after_add.contains("/data/cache"),
            "add_volume should persist the requested container path in the app volumes JSON"
        );
        assert!(
            volumes_after_add.contains("/volumes/demo/data_cache"),
            "add_volume should derive a sanitized host path under the app volume directory"
        );

        let error = add_volume(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(VolumeRequest {
                container_path: "/data/cache".to_string(),
                name: None,
                host_path: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected add_volume to reject duplicate mounts"));
        assert_eq!(
            error.0.to_string(),
            "Invalid input: A volume is already mounted at /data/cache",
            "add_volume should reject duplicate container paths"
        );

        let remove_response = remove_volume(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(VolumeRequest {
                container_path: "/data/cache".to_string(),
                name: None,
                host_path: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected remove_volume to succeed: {}", err.0))
        .into_response();
        assert_eq!(
            remove_response.status(),
            StatusCode::OK,
            "remove_volume should succeed for existing mounts"
        );

        let volumes_after_remove: String =
            sqlx::query_scalar("SELECT volumes FROM apps WHERE id = ?")
                .bind(&app.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read volumes after remove_volume: {err}"));
        assert_eq!(
            volumes_after_remove, "[]",
            "remove_volume should persist an empty volume list after deleting the only mount"
        );
    }

    #[tokio::test]
    async fn add_volume_rejects_host_paths_outside_the_app_directory() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        for host_path in ["/", "/etc", "/var/run/docker.sock", "/opt/tond/data"] {
            let error = add_volume(
                RateLimit,
                Authz::unchecked(Principal::test_user(Role::Admin)),
                State(state.clone()),
                Path(app.id.clone()),
                Json(VolumeRequest {
                    container_path: "/host".to_string(),
                    name: None,
                    host_path: Some(host_path.to_string()),
                }),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("host_path {host_path} must be rejected"));
            assert_eq!(error.0.status_code(), 400);
            assert!(
                error.0.to_string().contains("is not allowed"),
                "error should explain why: {}",
                error.0
            );
        }

        let stored: String = sqlx::query_scalar("SELECT volumes FROM apps WHERE id = ?")
            .bind(&app.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to read volumes: {err}"));
        assert_eq!(stored, "[]", "rejected volumes must not be persisted");
    }

    #[tokio::test]
    async fn create_app_confines_volumes_and_rejects_host_root() {
        let state = test_state().await;
        let mut request = CreateAppRequest {
            name: "rooted".to_string(),
            source_type: "image".to_string(),
            source_url: Some("alpine:3".to_string()),
            branch: None,
            container_port: 3000,
            env_vars: None,
            domain: None,
            project_id: None,
            memory_limit: None,
            cpu_limit: None,
            health_check_path: None,
            health_check_interval: None,
            health_failure_threshold: None,
            drain_timeout: None,
            build_method: None,
            auto_deploy: false,
            watch_branch: None,
            volumes: Some(vec![VolumeRequest {
                container_path: "/host".to_string(),
                name: None,
                host_path: Some("/".to_string()),
            }]),
            registry_credential_id: None,
            github_app_id: None,
            dockerfile_content: None,
            compose_content: None,
            base_dir: None,
            static_build: false,
            readiness: None,
            ready_after_secs: None,
        };

        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(request),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("host_path '/' must be rejected"));
        assert_eq!(error.0.status_code(), 400);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM apps WHERE name = 'rooted'")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("count failed: {err}"));
        assert_eq!(count, 0, "no app row may be created for a rejected request");

        // Members may not set volumes at all.
        request = CreateAppRequest {
            name: "member-app".to_string(),
            source_type: "image".to_string(),
            source_url: Some("alpine:3".to_string()),
            branch: None,
            container_port: 3000,
            env_vars: None,
            domain: None,
            project_id: None,
            memory_limit: None,
            cpu_limit: None,
            health_check_path: None,
            health_check_interval: None,
            health_failure_threshold: None,
            drain_timeout: None,
            build_method: None,
            auto_deploy: false,
            watch_branch: None,
            volumes: Some(vec![VolumeRequest {
                container_path: "/data".to_string(),
                name: None,
                host_path: None,
            }]),
            registry_credential_id: None,
            github_app_id: None,
            dockerfile_content: None,
            compose_content: None,
            base_dir: None,
            static_build: false,
            readiness: None,
            ready_after_secs: None,
        };
        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Deployer)),
            State(state.clone()),
            Json(request),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("members must not configure volumes"));
        assert_eq!(error.0.status_code(), 403);
    }

    fn compose_request(name: &str, content: &str) -> CreateAppRequest {
        CreateAppRequest {
            name: name.to_string(),
            source_type: "compose".to_string(),
            source_url: None,
            branch: None,
            container_port: 80,
            env_vars: None,
            domain: None,
            project_id: None,
            memory_limit: None,
            cpu_limit: None,
            health_check_path: None,
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
            compose_content: Some(content.to_string()),
            base_dir: None,
            static_build: false,
            readiness: None,
            ready_after_secs: None,
        }
    }

    #[tokio::test]
    async fn create_app_rejects_privileged_compose_files_with_every_problem_listed() {
        let state = test_state().await;
        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(compose_request(
                "evil",
                "services:\n  web:\n    image: nginx\n    privileged: true\n    volumes:\n      - /var/run/docker.sock:/var/run/docker.sock\n",
            )),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("a privileged compose file must be rejected"));
        assert_eq!(error.0.status_code(), 400);
        let msg = error.0.to_string();
        assert!(msg.contains("'privileged' is not allowed"), "{msg}");
        assert!(msg.contains("/var/run/docker.sock"), "{msg}");

        // Members can't deploy compose apps at all; admins can deploy clean ones.
        let error = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Deployer)),
            State(state.clone()),
            Json(compose_request(
                "member-compose",
                "services:\n  web:\n    image: nginx\n",
            )),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("members must not create compose apps"));
        assert_eq!(error.0.status_code(), 403);

        let response = create_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(compose_request(
                "clean",
                "services:\n  web:\n    image: nginx\n    ports: [\"8080:80\"]\n    volumes: [\"./html:/usr/share/nginx/html\"]\n",
            )),
        )
        .await
        .unwrap_or_else(|err| panic!("a clean compose file should be accepted: {}", err.0))
        .into_response();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn validate_compose_lists_problems_without_failing_the_request() {
        let response = validate_compose(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            Json(ValidateComposeRequest {
                app_name: Some("web".to_string()),
                compose_content: "services:\n  a:\n    image: x\n    privileged: true\n"
                    .to_string(),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("validate_compose should not error: {}", err.0))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(body["data"]["valid"], serde_json::json!(false));
        assert!(
            body["data"]["errors"][0]
                .as_str()
                .is_some_and(|e| e.contains("privileged"))
        );

        let response = validate_compose(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            Json(ValidateComposeRequest {
                app_name: None,
                compose_content: "services:\n  a:\n    image: x\n".to_string(),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("validate_compose should not error: {}", err.0))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(body["data"]["valid"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn update_drain_timeout_rejects_negative_values() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        let error = update_drain_timeout(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Path(app.id),
            Json(UpdateDrainTimeoutRequest { drain_timeout: -1 }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected update_drain_timeout to reject negative values"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: drain_timeout must be non-negative",
            "update_drain_timeout should reject negative graceful drain values"
        );
    }

    #[tokio::test]
    async fn assign_project_rejects_unknown_project_and_accepts_existing_project() {
        let state = test_state().await;
        let app = seed_app(&state, "demo").await;

        let error = assign_project(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(AssignProjectRequest {
                project_id: Some("missing-project".to_string()),
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected assign_project to reject unknown project IDs"));
        assert_eq!(
            error.0.to_string(),
            "Not found: project 'missing-project'",
            "assign_project should reject project IDs that do not exist"
        );

        seed_project(&state, "project-1", "Platform").await;
        let response = assign_project(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(AssignProjectRequest {
                project_id: Some("project-1".to_string()),
            }),
        )
        .await
        .unwrap_or_else(|err| {
            panic!(
                "expected assign_project to succeed for existing project: {}",
                err.0
            )
        })
        .into_response();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "assign_project should succeed for existing project IDs"
        );

        let assigned_project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM apps WHERE id = ?")
                .bind(&app.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read assigned project_id: {err}"));
        assert_eq!(
            assigned_project.as_deref(),
            Some("project-1"),
            "assign_project should persist the selected project ID on the app"
        );
    }

    /// Run `receive_upload` behind a real axum router so the Multipart
    /// extractor parses a genuine multipart body.
    async fn post_upload(body: Vec<u8>, max_bytes: u64) -> (StatusCode, String, tempfile::TempDir) {
        use tower::ServiceExt;

        let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dest = dir.path().to_path_buf();
        let router = axum::Router::new().route(
            "/upload",
            axum::routing::post(move |mut multipart: Multipart| {
                let dest = dest.clone();
                async move {
                    match receive_upload(&mut multipart, &dest, max_bytes).await {
                        Ok(()) => StatusCode::OK.into_response(),
                        Err(e) => e.into_response(),
                    }
                }
            })
            .layer(axum::extract::DefaultBodyLimit::disable()),
        );
        let boundary = "tondtestboundary";
        let mut payload = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.tar.gz\"\r\nContent-Type: application/gzip\r\n\r\n"
        )
        .into_bytes();
        payload.extend_from_slice(&body);
        payload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(axum::body::Body::from(payload))
            .unwrap_or_else(|e| panic!("request: {e}"));
        let response = router
            .oneshot(request)
            .await
            .unwrap_or_else(|e| panic!("router: {e}"));
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        (status, String::from_utf8_lossy(&bytes).into_owned(), dir)
    }

    fn gzip_tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, *content)
                .unwrap_or_else(|e| panic!("append: {e}"));
        }
        let tar = builder.into_inner().unwrap_or_else(|e| panic!("tar: {e}"));
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&tar).unwrap_or_else(|e| panic!("gz: {e}"));
        enc.finish().unwrap_or_else(|e| panic!("gz: {e}"))
    }

    #[tokio::test]
    async fn receive_upload_extracts_a_streamed_multipart_archive() {
        let archive = gzip_tar(&[("Dockerfile", b"FROM scratch\n"), ("src/app.txt", b"hi")]);
        let (status, body, dir) = post_upload(archive, 1024 * 1024).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "valid upload should extract: {body}"
        );
        assert!(dir.path().join("Dockerfile").is_file());
        assert!(dir.path().join("src/app.txt").is_file());
    }

    #[tokio::test]
    async fn receive_upload_refuses_oversized_archives_with_413() {
        let big = vec![0u8; 3 * 1024 * 1024];
        let archive = gzip_tar(&[("big.bin", &big)]);
        let (status, body, _dir) = post_upload(archive, 1024 * 1024).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "body: {body}");
        assert!(
            body.contains(".tondignore"),
            "the error should say how to fix it: {body}"
        );
    }

    #[tokio::test]
    async fn receive_upload_refuses_malicious_archives_with_400() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        builder
            .append_link(&mut header, "passwd", "/etc/passwd")
            .unwrap_or_else(|e| panic!("append link: {e}"));
        let tar = builder.into_inner().unwrap_or_else(|e| panic!("tar: {e}"));
        let (status, body, _dir) = post_upload(tar, 1024 * 1024).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
        assert!(body.contains("outside the project"), "body: {body}");
    }

    #[tokio::test]
    async fn update_domain_warns_on_the_dashboard_site_and_refuses_unroutable_domains() {
        let state = test_state().await;
        {
            let mut s = state.settings.write().await;
            s.dashboard_domain = "dash.example.com".to_string();
            s.preview_domain = "example-previews.com".to_string();
        }
        let app = seed_app(&state, "demo").await;
        let set = |domain: &str| {
            let state = state.clone();
            let id = app.id.clone();
            let domain = domain.to_string();
            async move {
                update_domain(
                    RateLimit,
                    Authz::unchecked(Principal::test_user(Role::Admin)),
                    State(state),
                    Path(id),
                    Json(UpdateDomainRequest { domain }),
                )
                .await
                .map(IntoResponse::into_response)
            }
        };

        let response = set(" Shop.Example.com ")
            .await
            .unwrap_or_else(|err| panic!("same-site custom domains are allowed: {}", err.0));
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        assert_eq!(
            json["data"]["domain"], "shop.example.com",
            "domains are normalized"
        );
        assert!(
            json["data"]["warning"]
                .as_str()
                .is_some_and(|w| w.contains("same site as your dashboard")),
            "a same-site domain comes back with a warning: {json}"
        );

        let detail = get_app(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
        )
        .await
        .unwrap_or_else(|err| panic!("get_app: {}", err.0))
        .into_response();
        let body = to_bytes(detail.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        assert!(
            json["data"]["domain_warning"].is_string(),
            "the app detail carries the warning too: {json}"
        );

        for bad in [
            "dash.example.com",
            "pr.example-previews.com",
            "not a domain",
        ] {
            assert!(set(bad).await.is_err(), "{bad} must be refused");
        }
    }
}
