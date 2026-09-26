// handlers/deploy_stats.rs — how reliable and how fast an app's deploys are
//
//   deploy_stats()  GET /api/v1/apps/{id}/deployments/stats
//
// Over the last TREND_WINDOW finished deployments: success rate (succeeded
// out of succeeded + failed; superseded/cancelled ones don't count either
// way), the median duration of successful ones, and each outcome oldest
// first for the trend strip on the Deployments tab.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::DateTime;
use sqlx::SqlitePool;
use tond_common::*;

use super::fetch_app;
use crate::auth::{Authz, RateLimit, scope};
use crate::error::AppError;
use crate::state::AppState;

pub const TREND_WINDOW: i64 = 30;

/// id, status, started_at, finished_at, commit_sha
type OutcomeRow = (String, String, String, Option<String>, Option<String>);

fn duration_secs(started: &str, finished: Option<&str>) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(started).ok()?;
    let end = DateTime::parse_from_rfc3339(finished?).ok()?;
    Some((end - start).num_seconds().max(0))
}

/// Median of `values` (mean of the middle two for an even count), rounded.
pub fn median(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid] + 1) / 2
    } else {
        values[mid]
    })
}

pub async fn stats_for(db: &SqlitePool, app_id: &str) -> Result<DeployStatsResponse, sqlx::Error> {
    let rows: Vec<OutcomeRow> = sqlx::query_as(
        "SELECT id, status, started_at, finished_at, commit_sha FROM deployments
         WHERE app_id = ? AND status IN ('succeeded', 'failed', 'superseded', 'cancelled')
         ORDER BY created_at DESC LIMIT ?",
    )
    .bind(app_id)
    .bind(TREND_WINDOW)
    .fetch_all(db)
    .await?;

    let mut outcomes: Vec<DeployOutcome> = rows
        .into_iter()
        .map(
            |(id, status, started_at, finished_at, commit_sha)| DeployOutcome {
                duration_secs: duration_secs(&started_at, finished_at.as_deref()),
                id,
                status,
                started_at,
                commit_sha,
            },
        )
        .collect();
    outcomes.reverse();

    let succeeded = outcomes.iter().filter(|o| o.status == "succeeded").count() as i64;
    let failed = outcomes.iter().filter(|o| o.status == "failed").count() as i64;
    let decided = succeeded + failed;
    Ok(DeployStatsResponse {
        window: outcomes.len() as i64,
        succeeded,
        failed,
        success_rate: (decided > 0).then(|| succeeded as f64 / decided as f64),
        median_duration_secs: median(
            outcomes
                .iter()
                .filter(|o| o.status == "succeeded")
                .filter_map(|o| o.duration_secs)
                .collect(),
        ),
        outcomes,
    })
}

pub async fn deploy_stats(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let stats = stats_for(&state.db, &app.id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(stats))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medians() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![30, 10, 20]), Some(20));
        assert_eq!(median(vec![10, 20, 30, 41]), Some(25));
        assert_eq!(median(vec![1, 2]), Some(2), "rounds half up");
    }

    #[tokio::test]
    async fn stats_cover_the_last_finished_deploys() {
        let state = AppState::for_tests().await;
        sqlx::query(
            "INSERT INTO apps (id, name, created_at, updated_at) VALUES ('a1', 'web', 'x', 'x')",
        )
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("app: {e}"));
        // 33 deploys: every 4th failed, one running, one superseded.
        for i in 0..35 {
            let status = match i {
                34 => "running",
                33 => "superseded",
                i if i % 4 == 0 => "failed",
                _ => "succeeded",
            };
            let start = format!("2026-09-01T{:02}:{:02}:00+00:00", i / 60, i % 60);
            let end = format!("2026-09-01T{:02}:{:02}:30+00:00", i / 60, i % 60);
            sqlx::query(
                "INSERT INTO deployments (id, app_id, status, started_at, finished_at, created_at)
                 VALUES (?, 'a1', ?, ?, ?, ?)",
            )
            .bind(format!("d{i:02}"))
            .bind(status)
            .bind(&start)
            .bind((status != "running").then_some(end))
            .bind(&start)
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("deploy: {e}"));
        }
        let stats = stats_for(&state.db, "a1")
            .await
            .unwrap_or_else(|e| panic!("stats: {e}"));
        assert_eq!(stats.window, 30);
        // d04..d33: failed at 4,8,...,32 (8 of them), d33 superseded.
        assert_eq!(stats.failed, 8);
        assert_eq!(stats.succeeded, 21);
        assert_eq!(stats.success_rate.map(|r| (r * 100.0).round()), Some(72.0));
        assert_eq!(stats.median_duration_secs, Some(30));
        assert_eq!(stats.outcomes.first().map(|o| o.id.as_str()), Some("d04"));
        assert_eq!(stats.outcomes.last().map(|o| o.id.as_str()), Some("d33"));
    }
}
