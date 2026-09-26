// handlers/metrics.rs — Phase 5.1 / 5.3 metric endpoints
//
// Routes:
//   GET /api/apps/{id}/metrics?period=1h|24h|7d
//     Returns time-series metric samples for a single app.
//     Requires auth. Responds with an array of MetricPointResponse.
//
//   GET /api/system/metrics
//     Returns a single point-in-time snapshot of host resources (CPU, memory,
//     disk) plus aggregate container / app counts.
//     Uses the `sysinfo` crate for host stats and Docker for container counts.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use std::path::Path as StdPath;
use sysinfo::System;
use tond_common::*;

use crate::{
    auth::{Authz, RateLimit, scope},
    error::AppError,
    state::AppState,
};

// ── App metrics ──

pub async fn get_app_metrics(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Resolve app (supports id or name)
    let app = sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = ? OR name = ? LIMIT 1")
        .bind(&id)
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError(TondError::AppNotFound(id.clone())))?;

    // Map period string to SQLite interval
    let interval = match query.period.as_str() {
        "24h" => "-24 hours",
        "7d" => "-7 days",
        _ => "-1 hours", // default "1h"
    };

    let points = sqlx::query_as::<_, MetricPoint>(
        "SELECT * FROM metrics
         WHERE app_id = ? AND collected_at >= datetime('now', ?)
         ORDER BY collected_at ASC",
    )
    .bind(&app.id)
    .bind(interval)
    .fetch_all(&state.db)
    .await?;

    let responses: Vec<MetricPointResponse> = points.into_iter().map(|p| p.into()).collect();
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── System metrics ──

pub async fn get_system_metrics(
    _rate: RateLimit,
    _auth: Authz<scope::MetricsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    // ── Host stats (sysinfo) ──
    let mut sys = System::new_all();
    sys.refresh_all();

    // CPU: sysinfo requires two samples with a pause between them to calculate
    // usage deltas — a single refresh always yields 0.0.
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu_usage();

    let cpu_percent = {
        let cpus = sys.cpus();
        if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| c.cpu_usage() as f64).sum::<f64>() / cpus.len() as f64
        }
    };

    let mem_used = sys.used_memory() as i64;
    let mem_total = sys.total_memory() as i64;
    let mem_percent = if mem_total > 0 {
        (mem_used as f64 / mem_total as f64) * 100.0
    } else {
        0.0
    };

    // Disk — use only the root filesystem ("/") to avoid summing tmpfs, devtmpfs,
    // overlay (Docker layer) mounts, etc., which would grossly inflate the total.
    let (disk_used, disk_total) = {
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let root = disks.iter().find(|d| d.mount_point() == StdPath::new("/"));
        match root {
            Some(d) => {
                let total = d.total_space();
                let avail = d.available_space();
                (total.saturating_sub(avail) as i64, total as i64)
            }
            None => (0, 0),
        }
    };

    let disk_percent = if disk_total > 0 {
        (disk_used as f64 / disk_total as f64) * 100.0
    } else {
        0.0
    };

    // ── Docker container count ──
    let containers_running = {
        use bollard::query_parameters::ListContainersOptionsBuilder;
        let opts = ListContainersOptionsBuilder::default()
            .all(false) // only running
            .build();
        state
            .docker
            .list_containers(Some(opts))
            .await
            .map(|list| list.len() as i64)
            .unwrap_or(0)
    };

    // ── Tond app counts ──
    let (apps_total, apps_running): (i64, i64) = {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM apps")
            .fetch_one(&state.db)
            .await
            .unwrap_or((0,));
        let running: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM apps WHERE status = 'running'")
            .fetch_one(&state.db)
            .await
            .unwrap_or((0,));
        (total.0, running.0)
    };

    let metrics = SystemMetrics {
        cpu_percent,
        mem_used,
        mem_total,
        mem_percent,
        disk_used,
        disk_total,
        disk_percent,
        containers_running,
        apps_total,
        apps_running,
        collected_at: Utc::now().to_rfc3339(),
    };

    Ok((StatusCode::OK, Json(ApiResponse::ok(metrics))))
}
