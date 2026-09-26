// handlers/app_domains.rs — the domains of an app
//
//   GET    /api/apps/{id}/domains                    list (primary, aliases, redirects)
//   POST   /api/apps/{id}/domains                    add {domain, kind?, redirect_to?, redirect_code?, service?, port?}
//   POST   /api/apps/{id}/domains/www-pair           add example.com + www.example.com, one redirecting
//   PATCH  /api/apps/{id}/domains/{domain}           {kind?, redirect_to?, redirect_code?} (set primary, redirect)
//   DELETE /api/apps/{id}/domains/{domain}           remove
//
// Rules live in app_domains.rs. Every change bumps route_state (SQL
// triggers), which nudges tond-proxy, and new domains get a certificate pass.
// Per-domain DNS / TLS status comes from GET /api/domains/check.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use super::fetch_app;
use crate::app_domains::{self, DomainContext};
use crate::audit_log::{AuditEntry, record};
use crate::auth::{Authz, RateLimit, scope};
use crate::error::AppError;
use crate::proxy_link;
use crate::state::AppState;

/// The list the dashboard renders after every change.
async fn domain_list(state: &AppState, app_id: &str) -> Result<Vec<AppDomainResponse>, AppError> {
    let ctx = DomainContext::load(state).await;
    Ok(app_domains::list(&state.db, app_id)
        .await?
        .into_iter()
        .map(|d| ctx.response(d))
        .collect())
}

fn serves_traffic(app: &App) -> Result<(), AppError> {
    if !app.readiness().serves_traffic() {
        return Err(AppError(TondError::InvalidInput(format!(
            "{} is a background worker: it serves no traffic, so it has no domains. Make it a \
             web app under Settings → Health first.",
            app.name
        ))));
    }
    Ok(())
}

pub async fn list_domains(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(domain_list(&state, &app.id).await?)),
    ))
}

pub async fn add_domain(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddAppDomainRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    serves_traffic(&app)?;
    let ctx = DomainContext::load(&state).await;
    let row = app_domains::add(&state.db, &ctx, &app.id, &req).await?;
    tokio::spawn(proxy_link::request_certs(Some(row.domain.clone())));
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("domain_add")
            .app(&app.id, &app.name)
            .detail(format!(
                "domain={} kind={}{}",
                row.domain,
                row.kind,
                row.redirect_to
                    .as_deref()
                    .map(|t| format!(" redirect_to={t} code={}", row.redirect_code))
                    .unwrap_or_default()
            )),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(domain_list(&state, &app.id).await?)),
    ))
}

pub async fn add_www_pair(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<WwwPairRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    serves_traffic(&app)?;
    let ctx = DomainContext::load(&state).await;
    let rows = app_domains::www_pair(&state.db, &ctx, &app.id, &req).await?;
    tokio::spawn(proxy_link::request_certs(None));
    let names: Vec<String> = rows
        .iter()
        .map(|r| match &r.redirect_to {
            Some(to) => format!("{}->{to}", r.domain),
            None => r.domain.clone(),
        })
        .collect();
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("domain_add")
            .app(&app.id, &app.name)
            .detail(format!("www_pair {}", names.join(" "))),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(domain_list(&state, &app.id).await?)),
    ))
}

pub async fn update_domain(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path((id, domain)): Path<(String, String)>,
    Json(req): Json<UpdateAppDomainRequest>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let row = app_domains::update(&state.db, &app.id, &domain, &req).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("domain_update")
            .app(&app.id, &app.name)
            .detail(format!(
                "domain={} kind={}{}",
                row.domain,
                row.kind,
                row.redirect_to
                    .as_deref()
                    .map(|t| format!(" redirect_to={t} code={}", row.redirect_code))
                    .unwrap_or_default()
            )),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(domain_list(&state, &app.id).await?)),
    ))
}

pub async fn remove_domain(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path((id, domain)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let row = app_domains::remove(&state.db, &app.id, &domain).await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("domain_remove")
            .app(&app.id, &app.name)
            .detail(format!("domain={}", row.domain)),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(domain_list(&state, &app.id).await?)),
    ))
}
