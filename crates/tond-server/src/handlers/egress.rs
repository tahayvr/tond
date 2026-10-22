// handlers/egress.rs — an app's outbound network policy (see egress.rs)
//
//   GET /api/v1/apps/{id}/egress   the policy, whether it is enforced, and
//                                  what allowlisted hostnames resolve to
//   PUT /api/v1/apps/{id}/egress   change it (admins: settings:write)

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{ApiResponse, App, AppEgressResponse, Scope, TondError, UpdateEgressRequest};

use crate::{
    audit_log::write_audit_log,
    auth::{Authz, RateLimit, scope},
    db_access,
    egress::{self, Policy},
    error::AppError,
    state::AppState,
};

fn response(app: &App) -> AppEgressResponse {
    let status = egress::status();
    let allowlist = egress::allowlist(app);
    AppEgressResponse {
        policy: egress::app_policy(app).as_str().to_string(),
        preview_policy: egress::preview_policy(app).as_str().to_string(),
        firewall_available: status.available,
        warning: egress::app_warning(app, &status),
        preview_warning: egress::preview_warning(app, &status),
        hosts: egress::resolved_hosts(&allowlist),
        allowlist,
    }
}

pub async fn get_egress(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = super::fetch_app(&state.db, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(response(&app)))))
}

fn parse_policy(field: &str, value: &str) -> Result<Policy, AppError> {
    Policy::from_input(value.trim()).ok_or_else(|| {
        AppError(TondError::invalid_field(
            field,
            format!("'{value}' isn't a policy; use open, restricted or none"),
        ))
    })
}

/// Check a requested change. Returns (policy, canonical allowlist, preview policy).
pub fn validate(
    app: &App,
    req: &UpdateEgressRequest,
) -> Result<(Policy, Vec<String>, Policy), AppError> {
    let policy = parse_policy("policy", &req.policy)?;
    let preview = match &req.preview_policy {
        Some(p) => parse_policy("preview_policy", p)?,
        None => egress::preview_policy(app),
    };
    let allowlist = match &req.allowlist {
        Some(entries) => egress::normalize_allowlist(entries)
            .map_err(|msg| AppError(TondError::invalid_field("allowlist", msg)))?,
        None => egress::allowlist(app),
    };
    if allowlist.is_empty() && (policy == Policy::Restricted || preview == Policy::Restricted) {
        return Err(AppError(TondError::invalid_field(
            "allowlist",
            "A restricted policy needs at least one destination. Add the hosts or addresses \
             the app calls, or choose \"none\" to block the internet entirely.",
        )));
    }
    Ok((policy, allowlist, preview))
}

pub async fn update_egress(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateEgressRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_to(
        Scope::SettingsWrite,
        "change an app's outbound network policy",
    )?;
    let app = super::fetch_app(&state.db, &id).await?;
    let (policy, allowlist, preview) = validate(&app, &req)?;
    if policy != Policy::Open {
        db_access::firewall_usable().await.map_err(|e| {
            AppError(TondError::InvalidInput(format!(
                "This server can't enforce an egress policy ({e}). It needs iptables with \
                 Docker's DOCKER-USER chain; nftables-only hosts and rootless Docker don't \
                 have it. Keep the policy open."
            )))
        })?;
    }
    let allowlist_json = serde_json::to_string(&allowlist).unwrap_or_else(|_| "[]".to_string());
    sqlx::query(
        "UPDATE apps SET egress_policy = ?, egress_allowlist = ?, preview_egress_policy = ?,
                updated_at = datetime('now') WHERE id = ?",
    )
    .bind(policy.as_str())
    .bind(&allowlist_json)
    .bind(preview.as_str())
    .bind(&app.id)
    .execute(&state.db)
    .await?;
    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "app.egress_update",
        &auth.audit_actor(),
        Some(&format!(
            "policy={} preview_policy={} allowlist={}",
            policy.as_str(),
            preview.as_str(),
            allowlist.join(",")
        )),
    )
    .await;
    egress::reconcile_now(&state).await;
    let app = super::fetch_app(&state.db, &app.id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(response(&app)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new("web".into(), "git".into(), None, None, 3000)
    }

    fn req(policy: &str, allow: Option<&[&str]>, preview: Option<&str>) -> UpdateEgressRequest {
        UpdateEgressRequest {
            policy: policy.to_string(),
            allowlist: allow.map(|a| a.iter().map(|s| s.to_string()).collect()),
            preview_policy: preview.map(str::to_string),
        }
    }

    #[test]
    fn restricted_needs_a_destination_and_values_are_checked() {
        let a = app();
        assert!(validate(&a, &req("open", None, None)).is_ok());
        assert!(validate(&a, &req("none", None, None)).is_ok());
        assert!(validate(&a, &req("restricted", None, None)).is_err());
        assert!(validate(&a, &req("open", None, Some("restricted"))).is_err());
        assert!(validate(&a, &req("closed", None, None)).is_err());
        assert!(validate(&a, &req("open", None, Some("maybe"))).is_err());
        assert!(validate(&a, &req("restricted", Some(&["*.example.com"]), None)).is_err());
        let (policy, list, preview) =
            validate(&a, &req("restricted", Some(&["API.example.com:443"]), None))
                .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(policy, Policy::Restricted);
        assert_eq!(list, vec!["api.example.com:443".to_string()]);
        assert_eq!(preview, Policy::None, "previews keep their default");
    }

    #[test]
    fn omitted_fields_keep_the_stored_values() {
        let mut a = app();
        a.egress_allowlist = r#"["10.0.0.0/8"]"#.into();
        a.preview_egress_policy = "restricted".into();
        let (_, list, preview) =
            validate(&a, &req("restricted", None, None)).unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(list, vec!["10.0.0.0/8".to_string()]);
        assert_eq!(preview, Policy::Restricted);
    }
}
