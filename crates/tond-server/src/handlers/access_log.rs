// handlers/access_log.rs — opt-in per-app access logs
//
// Routes:
//   GET    /api/apps/{id}/access-log?lines=500&q=   (logs:read)
//     Settings, size on disk and the last matching lines.
//   GET    /api/apps/{id}/access-log/download       (logs:read)
//     The whole log (rotated half first) as text/plain.
//   PUT    /api/apps/{id}/access-log                (apps:write)
//     { enabled, include_query }: tond-proxy picks it up on its next route
//     reload (the column is in a route_state trigger).
//   DELETE /api/apps/{id}/access-log                (apps:write)
//     Delete the files; the proxy reopens a fresh file on its next line.
//
// The files live in <data>/access, written by tond-proxy (see
// crates/tond-proxy/src/access_log.rs and access_logs.rs for how they are
// opened safely).

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::io::AsyncReadExt;
use tond_common::traffic::ACCESS_LOG_MAX_BYTES;
use tond_common::*;

use crate::{
    access_logs,
    audit_log::{AuditEntry, record},
    auth::{Authz, RateLimit, scope, secret_file},
    error::AppError,
    state::AppState,
};

const DEFAULT_LINES: usize = 500;
const MAX_LINES: usize = 5000;

async fn app_settings(
    state: &AppState,
    id: &str,
) -> Result<(String, String, bool, bool), AppError> {
    sqlx::query_as(
        "SELECT id, name, access_log, access_log_query FROM apps WHERE id = ? OR name = ? LIMIT 1",
    )
    .bind(id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError(TondError::AppNotFound(id.to_string())))
}

pub async fn get_access_log(
    _rate: RateLimit,
    _auth: Authz<scope::LogsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AccessLogQuery>,
) -> Result<impl IntoResponse, AppError> {
    let (app_id, _, enabled, include_query) = app_settings(&state, &id).await?;
    let limit = query.lines.unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);
    let data_dir = secret_file::data_dir();
    let filter = query.q.clone();
    let id = app_id.clone();
    let (lines, truncated, size_bytes) = tokio::task::spawn_blocking(move || {
        let (lines, truncated) = access_logs::tail(&data_dir, &id, limit, filter.as_deref())?;
        anyhow::Ok((lines, truncated, access_logs::size(&data_dir, &id)))
    })
    .await
    .map_err(|e| AppError(TondError::Internal(e.to_string())))?
    .map_err(|e| AppError(TondError::Internal(format!("{e:#}"))))?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AccessLogResponse {
            enabled,
            include_query,
            size_bytes,
            max_bytes: ACCESS_LOG_MAX_BYTES,
            lines,
            truncated,
        })),
    ))
}

pub async fn download_access_log(
    _rate: RateLimit,
    _auth: Authz<scope::LogsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let (app_id, name, _, _) = app_settings(&state, &id).await?;
    let files: Vec<tokio::fs::File> =
        access_logs::open_for_download(&secret_file::data_dir(), &app_id)
            .into_iter()
            .map(tokio::fs::File::from_std)
            .collect();
    let stream = futures::stream::unfold(
        (files.into_iter(), None::<tokio::fs::File>),
        |(mut rest, mut current)| async move {
            loop {
                if current.is_none() {
                    current = Some(rest.next()?);
                }
                let file = current.as_mut()?;
                let mut buf = vec![0u8; 64 * 1024];
                match file.read(&mut buf).await {
                    Ok(0) => current = None,
                    Ok(n) => {
                        buf.truncate(n);
                        return Some((Ok::<_, std::io::Error>(buf), (rest, current)));
                    }
                    Err(e) => return Some((Err(e), (rest, None))),
                }
            }
        },
    );
    // App names are [a-z0-9-] (validated at creation); keep the header safe anyway.
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{safe}-access.log\""),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .map_err(|e| AppError(TondError::Internal(e.to_string())))
}

pub async fn update_access_log(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AccessLogSettings>,
) -> Result<impl IntoResponse, AppError> {
    let (app_id, name, _, _) = app_settings(&state, &id).await?;
    sqlx::query(
        "UPDATE apps SET access_log = ?, access_log_query = ?, updated_at = ? WHERE id = ?",
    )
    .bind(body.enabled)
    .bind(body.enabled && body.include_query)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(&app_id)
    .execute(&state.db)
    .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("access_log_update")
            .app(&app_id, &name)
            .detail(format!(
                "enabled={}, include_query={}",
                body.enabled,
                body.enabled && body.include_query
            )),
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(AccessLogSettings {
            enabled: body.enabled,
            include_query: body.enabled && body.include_query,
        })),
    ))
}

pub async fn delete_access_log(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let (app_id, name, _, _) = app_settings(&state, &id).await?;
    let data_dir = secret_file::data_dir();
    let removed = {
        let id = app_id.clone();
        tokio::task::spawn_blocking(move || access_logs::remove(&data_dir, &id))
            .await
            .unwrap_or(0)
    };
    // Make the proxy reload (it closes its files and reopens on the next line).
    sqlx::query("UPDATE route_state SET version = version + 1 WHERE id = 1")
        .execute(&state.db)
        .await?;
    record(
        &state.db,
        &auth.audit_actor(),
        AuditEntry::new("access_log_delete")
            .app(&app_id, &name)
            .detail(format!("{removed} file(s)")),
    )
    .await;
    Ok((StatusCode::OK, Json(ApiResponse::ok(()))))
}
