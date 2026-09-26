// handlers/compose.rs — Docker Compose apps: services, per-service domains
//
// GET  /api/apps/{id}/services                      — services with status, ports, domains, usage
// PUT  /api/apps/{id}/services/{service}            — publish a container port, route domains to it
// POST /api/apps/{id}/services/{service}/restart    — restart one service (also stop, start)
// GET  /api/apps/{id}/services/{service}/logs       — recent log lines of one service (scrubbed)
// PUT  /api/apps/{id}/compose                       — replace the compose file / repository path
//
// Service state comes from Docker (one `docker ps` filtered by the compose
// project label per request) and is stored in compose_services. Routing is
// read by tond-proxy from app_domains + compose_services.host_port; a new
// published port takes effect on the next deploy (host ports are bound when
// compose creates the container), domains take effect at once when the
// service already publishes that port.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use bollard::query_parameters::{RestartContainerOptionsBuilder, StartContainerOptions};
use serde::Deserialize;
use serde_json::json;
use tond_common::*;

use crate::{
    audit_log::write_audit_log,
    auth::{Authz, RateLimit, scope},
    compose, compose_policy, deploy, docker,
    error::AppError,
    log_scrub, metrics, proxy_link,
    state::AppState,
    volumes,
};

use super::{check_app_domain, fetch_app};

fn invalid_field(field: &str, msg: impl Into<String>) -> AppError {
    AppError(TondError::invalid_field(field, msg))
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError(TondError::InvalidInput(msg.into()))
}

fn require_compose(app: &App) -> Result<(), AppError> {
    if app.source_type != "compose" {
        return Err(invalid(format!(
            "'{}' is not a Docker Compose app",
            app.name
        )));
    }
    Ok(())
}

async fn service_row(
    db: &sqlx::SqlitePool,
    app: &App,
    service: &str,
) -> Result<compose::ServiceRow, AppError> {
    compose::rows(db, &app.id)
        .await?
        .into_iter()
        .find(|r| r.name == service)
        .ok_or_else(|| {
            AppError(TondError::NotFound(format!(
                "service '{service}' in app '{}'",
                app.name
            )))
        })
}

/// The API view of the app's services (refreshed from Docker first when
/// `refresh` is set).
pub(crate) async fn services_response(
    state: &AppState,
    app: &App,
    refresh: bool,
) -> Result<Vec<ComposeServiceResponse>, AppError> {
    if refresh && let Err(e) = compose::refresh(state, &app.id, &app.name).await {
        tracing::debug!("compose refresh for {} failed: {e:#}", app.name);
    }
    let rows = compose::rows(&state.db, &app.id).await?;
    let domains = compose::domains_by_service(&state.db, &app.id).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let usage = metrics::service_current(&app.id, &r.name);
            let published_port = r.published_port.and_then(|p| u16::try_from(p).ok());
            let host_port = r.host_port.and_then(|p| u16::try_from(p).ok());
            ComposeServiceResponse {
                ports: r.declared_ports(),
                image: r.image.clone(),
                builds: r.builds != 0,
                has_healthcheck: r.has_healthcheck != 0,
                redeploy_required: published_port.is_some()
                    && r.routed_port.and_then(|p| u16::try_from(p).ok()) != published_port,
                published_port,
                host_port,
                domains: domains.get(&r.name).cloned().unwrap_or_default(),
                status: r.status.clone(),
                health: r.health.clone(),
                exit_code: r.exit_code,
                container_id: r
                    .container_id
                    .as_ref()
                    .map(|c| c.chars().take(12).collect()),
                running_image: r.running_image.clone(),
                image_digest: r.image_digest.clone(),
                cpu: usage.map(|u| u.0),
                mem: usage.map(|u| u.1),
                name: r.name,
            }
        })
        .collect())
}

// ── List ──

pub async fn list_services(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    require_compose(&app)?;
    let services = services_response(&state, &app, true).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(services))))
}

// ── Publish a port / route domains ──

pub async fn update_service(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path((id, service)): Path<(String, String)>,
    Json(req): Json<UpdateComposeServiceRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    require_compose(&app)?;
    let row = service_row(&state.db, &app, &service).await?;

    if req.published_port == Some(0) {
        return Err(invalid_field(
            "published_port",
            "The published port must be between 1 and 65535",
        ));
    }
    if req.domains.len() > 20 {
        return Err(invalid_field(
            "domains",
            "A service can have at most 20 domains",
        ));
    }
    if !req.domains.is_empty() && req.published_port.is_none() {
        return Err(invalid_field(
            "published_port",
            format!("Publish a container port of service '{service}' before routing domains to it"),
        ));
    }
    let mut domains: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for d in &req.domains {
        let (domain, warning) = check_app_domain(&state, d).await?;
        if domains.contains(&domain) {
            continue;
        }
        if let Some((owner_id, owner)) = compose::domain_owner(&state.db, &domain).await? {
            let own_service: Option<String> = sqlx::query_scalar(
                "SELECT service FROM app_domains WHERE domain = ? AND app_id = ?",
            )
            .bind(&domain)
            .bind(&app.id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
            if owner_id != app.id || own_service.is_none() {
                return Err(invalid_field(
                    "domains",
                    format!("{domain} is already used by the app '{owner}'"),
                ));
            }
        }
        warnings.extend(warning);
        domains.push(domain);
    }

    let published = req.published_port.map(i64::from);
    let port_changed = row.published_port != published;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE compose_services SET published_port = ?,
           host_port = CASE WHEN ? IS NULL THEN NULL ELSE host_port END,
           updated_at = datetime('now')
         WHERE app_id = ? AND name = ?",
    )
    .bind(published)
    .bind(published)
    .bind(&app.id)
    .bind(&service)
    .execute(&mut *tx)
    .await?;
    let old: Vec<String> =
        sqlx::query_scalar("SELECT domain FROM app_domains WHERE app_id = ? AND service = ?")
            .bind(&app.id)
            .bind(&service)
            .fetch_all(&mut *tx)
            .await?;
    for gone in old.iter().filter(|d| !domains.contains(d)) {
        sqlx::query("DELETE FROM app_domains WHERE domain = ?")
            .bind(gone)
            .execute(&mut *tx)
            .await?;
    }
    for domain in &domains {
        sqlx::query(
            "INSERT INTO app_domains (domain, app_id, service, port) VALUES (?, ?, ?, ?)
             ON CONFLICT(domain) DO UPDATE SET port = excluded.port",
        )
        .bind(domain)
        .bind(&app.id)
        .bind(&service)
        .bind(published)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    for domain in domains.iter().filter(|d| !old.contains(d)) {
        compose::queue_certificate(&state.db, domain).await;
    }
    // Routes changed (triggers bumped route_state): have the proxy reload.
    let _ = proxy_link::sync_routes(&state).await;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "service_update",
        &auth.audit_actor(),
        Some(&format!(
            "service={service} published_port={} domains={}",
            published.map_or("none".to_string(), |p| p.to_string()),
            domains.join(",")
        )),
    )
    .await;

    let services = services_response(&state, &app, false).await?;
    let updated = services.into_iter().find(|s| s.name == service);
    let redeploy_required = updated.as_ref().is_some_and(|s| s.redeploy_required);
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "service": updated,
            "port_changed": port_changed,
            "redeploy_required": redeploy_required,
            "warnings": warnings,
        }))),
    ))
}

// ── Restart / stop / start one service ──

pub async fn service_action(
    _rate: RateLimit,
    auth: Authz<scope::AppsControl>,
    State(state): State<AppState>,
    Path((id, service, action)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    require_compose(&app)?;
    service_row(&state.db, &app, &service).await?;
    let project = deploy::compose_project_name(&app.name);
    let containers = compose::service_containers(&state.docker, &project, &service).await;
    if containers.is_empty() {
        return Err(invalid(format!(
            "Service '{service}' has no container yet — deploy the app first"
        )));
    }
    for cid in &containers {
        let result = match action.as_str() {
            "restart" => state
                .docker
                .restart_container(
                    cid,
                    Some(RestartContainerOptionsBuilder::default().t(10).build()),
                )
                .await
                .map_err(anyhow::Error::from),
            "stop" => docker::stop_container(&state.docker, cid).await,
            "start" => state
                .docker
                .start_container(cid, None::<StartContainerOptions>)
                .await
                .map_err(anyhow::Error::from),
            other => {
                return Err(invalid(format!(
                    "Unknown action '{other}': use restart, stop or start"
                )));
            }
        };
        result.map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
    }
    let _ = compose::refresh(&state, &app.id, &app.name).await;
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        &format!("service_{action}"),
        &auth.audit_actor(),
        Some(&format!("service={service}")),
    )
    .await;
    let services = services_response(&state, &app, false).await?;
    let past = match action.as_str() {
        "restart" => "restarted",
        "stop" => "stopped",
        _ => "started",
    };
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "message": format!("Service {service} {past}"),
            "service": services.into_iter().find(|s| s.name == service),
        }))),
    ))
}

/// Stop, start or restart every container of a compose app (the app-level
/// Stop / Start / Restart buttons).
pub(crate) async fn control_all(state: &AppState, app: &App, action: &str) -> Result<(), AppError> {
    let project = deploy::compose_project_name(&app.name);
    let states = compose::discover(&state.docker, &project)
        .await
        .map_err(|e| AppError(TondError::DockerError(e.to_string())))?;
    if states.is_empty() {
        return Err(AppError(
            TondError::InvalidInput("App has no containers yet".to_string())
                .with_hint("Deploy the app first."),
        ));
    }
    for st in &states {
        let cid = st.container_id.as_str();
        let result = match action {
            "stop" => docker::stop_container(&state.docker, cid).await,
            "start" => docker::start_container(&state.docker, cid).await,
            _ => state
                .docker
                .restart_container(
                    cid,
                    Some(RestartContainerOptionsBuilder::default().t(10).build()),
                )
                .await
                .map_err(anyhow::Error::from),
        };
        // A one-shot service that already finished can't be stopped; keep going.
        if let Err(e) = result
            && action != "stop"
        {
            return Err(AppError(TondError::DockerError(format!(
                "service {}: {e}",
                st.service
            ))));
        }
    }
    let _ = compose::refresh(state, &app.id, &app.name).await;
    Ok(())
}

/// Recent log lines of every service, each prefixed with `[service]`.
pub(crate) async fn combined_logs(state: &AppState, app: &App, tail: usize) -> Vec<String> {
    let project = deploy::compose_project_name(&app.name);
    let mut lines: Vec<(String, String)> = Vec::new();
    for st in compose::discover(&state.docker, &project)
        .await
        .unwrap_or_default()
    {
        for raw in docker::get_logs(&state.docker, &st.container_id, tail)
            .await
            .unwrap_or_default()
        {
            // "<ts> <line>": sort by timestamp across services.
            let (ts, line) = raw.split_once(' ').unwrap_or(("", raw.as_str()));
            lines.push((ts.to_string(), format!("{ts} [{}] {line}", st.service)));
        }
    }
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    let start = lines.len().saturating_sub(tail);
    let lines: Vec<String> = lines.into_iter().skip(start).map(|(_, l)| l).collect();
    log_scrub::scrub_lines_for_app(&state.db, &app.id, lines).await
}

// ── Logs of one service ──

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ServiceLogsQuery {
    pub tail: Option<usize>,
}

pub async fn service_logs(
    _rate: RateLimit,
    _auth: Authz<scope::LogsRead>,
    State(state): State<AppState>,
    Path((id, service)): Path<(String, String)>,
    Query(q): Query<ServiceLogsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    require_compose(&app)?;
    service_row(&state.db, &app, &service).await?;
    let project = deploy::compose_project_name(&app.name);
    let tail = q.tail.unwrap_or(200).clamp(1, 2000);
    let mut lines = Vec::new();
    if let Some(cid) = compose::service_containers(&state.docker, &project, &service)
        .await
        .first()
    {
        lines = docker::get_logs(&state.docker, cid, tail)
            .await
            .unwrap_or_default();
    }
    let lines = log_scrub::scrub_lines_for_app(&state.db, &app.id, lines).await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "service": service, "lines": lines }),
        )),
    ))
}

// ── Replace the compose file ──

pub async fn update_compose(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateComposeRequest>,
) -> Result<impl IntoResponse, AppError> {
    // A compose file reaches the host, like creating a compose app.
    auth.require_to(Scope::SettingsWrite, "edit Docker Compose files")?;
    let app = fetch_app(&state.db, &id).await?;
    require_compose(&app)?;

    let git = req
        .source_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    let (content, source_url, branch, compose_path) = match git {
        Some(url) => {
            let path = deploy::compose_path_in_repo(req.compose_path.as_deref())
                .map_err(|e| invalid_field("compose_path", e.to_string()))?;
            (
                None,
                Some(url),
                req.branch
                    .clone()
                    .filter(|b| !b.trim().is_empty())
                    .or(Some("main".to_string())),
                Some(path.to_string_lossy().into_owned()),
            )
        }
        None => {
            let content = req
                .compose_content
                .clone()
                .filter(|c| !c.trim().is_empty())
                .ok_or_else(|| invalid_field("compose_content", "compose_content is required"))?;
            let volume_dir = volumes::app_volume_dir(&volumes::data_dir(), &app.name);
            compose_policy::validate(&content, &volume_dir).map_err(|errors| {
                invalid_field("compose_content", compose_policy::format_errors(&errors))
            })?;
            let specs = compose::service_specs(&content).map_err(invalid)?;
            compose::sync_specs(&state.db, &app.id, &specs).await?;
            (Some(content), None, None, None)
        }
    };
    sqlx::query(
        "UPDATE apps SET compose_content = ?, source_url = ?, branch = ?, compose_path = ?,
           updated_at = datetime('now') WHERE id = ?",
    )
    .bind(&content)
    .bind(&source_url)
    .bind(&branch)
    .bind(&compose_path)
    .bind(&app.id)
    .execute(&state.db)
    .await?;
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "compose_update",
        &auth.audit_actor(),
        Some(match &source_url {
            Some(_) => "source=git",
            None => "source=inline",
        }),
    )
    .await;
    let services = services_response(&state, &app, false).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "message": "Compose file saved. Deploy to apply it.",
            "services": services,
        }))),
    ))
}

#[cfg(test)]
mod tests;
