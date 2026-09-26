// handlers/dns_providers.rs — DNS providers for ACME DNS-01 (wildcards)
//
//   GET    /api/dns-providers            list (tokens are never returned)
//   POST   /api/dns-providers            create {name, kind, zones, token, config}
//   PATCH  /api/dns-providers/{id}       update; `token` replaced only when sent
//   DELETE /api/dns-providers/{id}
//   POST   /api/dns-providers/{id}/test  check the token can see every zone
//
// Adding, changing or removing a provider bumps route_state (SQL triggers on
// dns_providers), so tond-proxy reloads the zone list and orders (or stops
// ordering) wildcard certificates. See dns_providers/ for the design.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::auth::{Authz, RateLimit, scope};
use crate::dns_providers::{self, clean_zones, clients::Reqwest, kind_allowed, to_response};
use crate::error::AppError;
use crate::proxy_link;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;

fn invalid(msg: impl Into<String>) -> AppError {
    AppError(TondError::InvalidInput(msg.into()))
}

async fn fetch(state: &AppState, id: &str) -> Result<DnsProvider, AppError> {
    sqlx::query_as::<_, DnsProvider>("SELECT * FROM dns_providers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("DNS provider {id}"))))
}

fn check_config(kind: DnsProviderKind, config: &DnsProviderConfig) -> Result<(), AppError> {
    if kind == DnsProviderKind::Route53
        && config
            .access_key_id
            .as_deref()
            .is_none_or(|k| k.trim().is_empty())
    {
        return Err(invalid(
            "Route 53 needs the access key id (the secret access key goes in the token field)",
        ));
    }
    if let Some(url) = &config.url
        && !url.is_empty()
        && !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return Err(invalid("The URL must start with http:// or https://"));
    }
    Ok(())
}

/// Zones already claimed by another provider (one provider per zone keeps
/// "who publishes this record" unambiguous).
async fn check_zone_overlap(
    state: &AppState,
    zones: &[String],
    except: Option<&str>,
) -> Result<(), AppError> {
    for other in dns_providers::list(&state.db).await? {
        if Some(other.id.as_str()) == except {
            continue;
        }
        let theirs = dns_providers::parse_zones(&other.zones);
        if let Some(z) = zones.iter().find(|z| theirs.contains(z)) {
            return Err(invalid(format!(
                "{z} is already handled by the DNS provider '{}'",
                other.name
            )));
        }
    }
    Ok(())
}

pub async fn list_dns_providers(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let list: Vec<DnsProviderResponse> = dns_providers::list(&state.db)
        .await?
        .into_iter()
        .map(to_response)
        .collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(list))))
}

pub async fn create_dns_provider(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateDnsProviderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(invalid("Give the provider a name"));
    }
    if !kind_allowed(req.kind) {
        return Err(invalid(format!(
            "The {} provider is for tests only and is disabled on this server",
            req.kind.as_str()
        )));
    }
    if req.token.trim().is_empty() && req.kind != DnsProviderKind::Challtestsrv {
        return Err(invalid("Paste the API token"));
    }
    check_config(req.kind, &req.config)?;
    let zones = clean_zones(&req.zones).map_err(invalid)?;
    check_zone_overlap(&state, &zones, None).await?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO dns_providers (id, name, kind, zones, config, token_enc, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(req.kind.as_str())
    .bind(serde_json::to_string(&zones).unwrap_or_else(|_| "[]".into()))
    .bind(serde_json::to_string(&req.config).unwrap_or_else(|_| "{}".into()))
    .bind(secret_key::encrypt(Purpose::DnsProviderToken, req.token.trim())?)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;
    tokio::spawn(proxy_link::request_certs(None));
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("dns_provider_create")
            .resource("dns_provider", &id, name)
            .detail(format!(
                "kind={} zones={}",
                req.kind.as_str(),
                zones.join(",")
            )),
    )
    .await;
    let row = fetch(&state, &id).await?;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(to_response(row)))))
}

pub async fn update_dns_provider(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDnsProviderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch(&state, &id).await?;
    let kind = DnsProviderKind::parse(&row.kind).unwrap_or(DnsProviderKind::Cloudflare);
    let name = match &req.name {
        Some(n) if n.trim().is_empty() => return Err(invalid("Give the provider a name")),
        Some(n) => n.trim().to_string(),
        None => row.name.clone(),
    };
    let zones = match &req.zones {
        Some(z) => {
            let z = clean_zones(z).map_err(invalid)?;
            check_zone_overlap(&state, &z, Some(&id)).await?;
            serde_json::to_string(&z).unwrap_or_else(|_| "[]".into())
        }
        None => row.zones.clone(),
    };
    let config = match &req.config {
        Some(c) => {
            check_config(kind, c)?;
            serde_json::to_string(c).unwrap_or_else(|_| "{}".into())
        }
        None => row.config.clone(),
    };
    let token_enc = match req.token.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => secret_key::encrypt(Purpose::DnsProviderToken, t)?,
        _ => row.token_enc.clone(),
    };
    sqlx::query(
        "UPDATE dns_providers SET name = ?, zones = ?, config = ?, token_enc = ?, updated_at = ?,
                last_error = NULL WHERE id = ?",
    )
    .bind(&name)
    .bind(&zones)
    .bind(&config)
    .bind(&token_enc)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(&id)
    .execute(&state.db)
    .await?;
    tokio::spawn(proxy_link::request_certs(None));
    let mut changed: Vec<&str> = Vec::new();
    if req.name.is_some() {
        changed.push("name");
    }
    if req.zones.is_some() {
        changed.push("zones");
    }
    if req.config.is_some() {
        changed.push("config");
    }
    if req.token.as_deref().is_some_and(|t| !t.trim().is_empty()) {
        changed.push("token");
    }
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("dns_provider_update")
            .resource("dns_provider", &id, &name)
            .detail(format!("changed={}", changed.join(","))),
    )
    .await;
    let row = fetch(&state, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(to_response(row)))))
}

pub async fn delete_dns_provider(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch(&state, &id).await?;
    sqlx::query("DELETE FROM dns_providers WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("dns_provider_delete").resource("dns_provider", &id, &row.name),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": id }))),
    ))
}

pub async fn test_dns_provider(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = fetch(&state, &id).await?;
    let client = dns_providers::client(&row).map_err(|e| invalid(format!("{e:#}")))?;
    let mut zones = Vec::new();
    for zone in dns_providers::parse_zones(&row.zones) {
        let (ok, message) = match client.check_zone(&Reqwest, &zone).await {
            Ok(()) => (true, format!("{} can manage {zone}", client.kind.label())),
            Err(e) => (false, format!("{e:#}")),
        };
        zones.push(DnsZoneCheck { zone, ok, message });
    }
    let ok = zones.iter().all(|z| z.ok);
    let error = (!ok).then(|| {
        zones
            .iter()
            .filter(|z| !z.ok)
            .map(|z| z.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    });
    let _ = sqlx::query("UPDATE dns_providers SET last_error = ? WHERE id = ?")
        .bind(&error)
        .bind(&id)
        .execute(&state.db)
        .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(DnsProviderTestResult { ok, zones })),
    ))
}
