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

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;
    use crate::auth::Principal;

    async fn seed(state: &AppState, readiness: &str) -> String {
        sqlx::query(
            "INSERT INTO apps (id, name, readiness, created_at, updated_at)
             VALUES ('a1', 'web', ?, 'now', 'now')",
        )
        .bind(readiness)
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("seed: {e}"));
        "a1".into()
    }

    fn admin<R: crate::auth::principal::ScopeReq>() -> Authz<R> {
        Authz::unchecked(Principal::test_user(Role::Admin))
    }

    async fn body(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    #[tokio::test]
    async fn adding_domains_returns_the_list_and_audits_it() {
        let state = AppState::for_tests().await;
        let id = seed(&state, "http").await;
        let resp = add_domain(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(id.clone()),
            Json(AddAppDomainRequest {
                domain: "shop.example.org".into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("add: {}", e.0))
        .into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let resp = add_www_pair(
            RateLimit,
            admin(),
            State(state.clone()),
            Path(id.clone()),
            Json(WwwPairRequest {
                domain: "example.org".into(),
                canonical: CanonicalHost::Www,
                primary: Some(false),
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("pair: {}", e.0))
        .into_response();
        let json = body(resp).await;
        let list: Vec<(String, String)> = json["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|d| {
                (
                    d["domain"].as_str().unwrap_or_default().to_string(),
                    d["kind"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            list,
            vec![
                ("shop.example.org".into(), "primary".into()),
                ("www.example.org".into(), "alias".into()),
                ("example.org".into(), "redirect".into()),
            ]
        );
        let audited: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'domain_add'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_default();
        assert_eq!(audited, 2);
    }

    #[tokio::test]
    async fn background_workers_get_no_domains() {
        let state = AppState::for_tests().await;
        let id = seed(&state, "none").await;
        let refused = add_domain(
            RateLimit,
            admin(),
            State(state),
            Path(id),
            Json(AddAppDomainRequest {
                domain: "jobs.example.org".into(),
                ..Default::default()
            }),
        )
        .await;
        assert!(
            matches!(&refused, Err(AppError(TondError::InvalidInput(m))) if m.contains("background worker"))
        );
    }
}
