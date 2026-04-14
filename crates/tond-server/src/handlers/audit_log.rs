// handlers/audit_log.rs — Phase 5.5: Audit log query handler
//
// Routes:
//   GET /api/audit-log?app_id=<uuid>&limit=<n>
//     Returns the most recent audit log entries, optionally filtered by
//     app_id. Default limit is 200; maximum is 1000.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tond_common::*;

use crate::{
    auth::{RateLimit, RequireAuth},
    error::AppError,
    state::AppState,
};

#[derive(Deserialize)]
pub struct AuditLogQuery {
    pub app_id: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_audit_log(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Query(query): Query<AuditLogQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);

    let entries = if let Some(app_id) = &query.app_id {
        sqlx::query_as::<_, AuditLog>(
            "SELECT * FROM audit_log WHERE app_id = ?
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(app_id)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, AuditLog>("SELECT * FROM audit_log ORDER BY created_at DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&state.db)
            .await?
    };

    let resp: Vec<AuditLogResponse> = entries.into_iter().map(AuditLogResponse::from).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(resp))))
}
