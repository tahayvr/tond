// handlers/cron.rs — Cron Job handlers (Phase 6.4 + 6.5)
//
// Creates and manages scheduled jobs that run Docker containers on a cron schedule.
//
// Routes (all require RateLimit + an Authz scope, see each handler):
//   GET    /api/cron                      — list all cron jobs
//   POST   /api/cron                      — create a new cron job
//   GET    /api/cron/{id}                 — get cron job details
//   PATCH  /api/cron/{id}                 — update fields
//   DELETE /api/cron/{id}                 — delete cron job
//   POST   /api/cron/{id}/trigger         — manually trigger a run immediately
//   GET    /api/cron/{id}/runs            — get execution history (supports ?limit=&offset=)

use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tond_common::*;

use crate::{
    audit_log::write_audit_log,
    auth::{Authz, RateLimit, scope},
    cron,
    cron::compute_next_run,
    error::AppError,
    secrets,
    state::AppState,
    volumes,
};

// ── Shared lookup helper ──

async fn fetch_cron_job(pool: &sqlx::SqlitePool, id_or_name: &str) -> Result<CronJob, AppError> {
    let job: Option<CronJob> =
        sqlx::query_as("SELECT * FROM cron_jobs WHERE id = ? OR name = ? LIMIT 1")
            .bind(id_or_name)
            .bind(id_or_name)
            .fetch_optional(pool)
            .await?;
    job.ok_or_else(|| AppError(TondError::NotFound(format!("cron job '{}'", id_or_name))))
}

/// Map cron volume requests into confined host directories (see `volumes.rs`)
/// and serialise them for storage. Empty list → `None`.
async fn confined_cron_volumes(
    pool: &sqlx::SqlitePool,
    job_id: &str,
    reqs: &[CronVolumeRequest],
) -> Result<Option<String>, AppError> {
    if reqs.is_empty() {
        return Ok(None);
    }
    let app_names: Vec<String> = sqlx::query_scalar("SELECT name FROM apps")
        .fetch_all(pool)
        .await?;
    let vols = volumes::cron_volumes(&volumes::data_dir(), job_id, reqs, |app| {
        app_names.iter().any(|n| n == app)
    })
    .map_err(|msg| AppError(TondError::InvalidInput(msg)))?;
    Ok(Some(serde_json::to_string(&vols).map_err(|e| {
        AppError(TondError::Internal(e.to_string()))
    })?))
}

/// API view of a job: env keys with masked values.
fn cron_response(job: CronJob) -> Result<CronJobResponse, AppError> {
    let env = secrets::cron_env(job.env.as_deref())?;
    Ok(CronJobResponse::new(job, &env))
}

fn validate_env_keys<'a>(keys: impl IntoIterator<Item = &'a String>) -> Result<(), AppError> {
    for key in keys {
        if !is_valid_env_key(key) {
            return Err(AppError(TondError::InvalidInput(format!(
                "'{key}' is not a valid variable name: use letters, digits and underscores, \
                 not starting with a digit"
            ))));
        }
    }
    Ok(())
}

// ── List cron jobs ──

pub async fn list_cron_jobs(
    _rate: RateLimit,
    _auth: Authz<scope::CronRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let jobs: Vec<CronJob> = sqlx::query_as("SELECT * FROM cron_jobs ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    let mut responses: Vec<CronJobResponse> = Vec::with_capacity(jobs.len());
    for job in jobs {
        responses.push(cron_response(job)?);
    }
    Ok((StatusCode::OK, Json(ApiResponse::ok(responses))))
}

// ── Get cron job ──

pub async fn get_cron_job(
    _rate: RateLimit,
    _auth: Authz<scope::CronRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let job = fetch_cron_job(&state.db, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(cron_response(job)?))))
}

// ── Create cron job ──

pub async fn create_cron_job(
    _rate: RateLimit,
    auth: Authz<scope::CronWrite>,
    State(state): State<AppState>,
    Json(req): Json<CreateCronJobRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate name
    if !req.name.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(AppError(TondError::InvalidInput(
            "Cron job name must be alphanumeric (hyphens allowed)".to_string(),
        )));
    }

    // Validate concurrency policy
    let concurrency_policy = req.concurrency_policy.unwrap_or_else(|| "skip".to_string());
    if concurrency_policy != "skip" && concurrency_policy != "allow" {
        return Err(AppError(TondError::InvalidInput(
            "concurrency_policy must be 'skip' or 'allow'".to_string(),
        )));
    }

    // Validate cron expression by attempting to compute the next run time
    let next_run = compute_next_run(&req.schedule, req.timezone.as_deref()).map_err(|_| {
        AppError(TondError::InvalidInput(format!(
            "Invalid cron expression: '{}'",
            req.schedule
        )))
    })?;

    // Verify project exists if provided
    if let Some(ref pid) = req.project_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?)")
            .bind(pid)
            .fetch_one(&state.db)
            .await?;
        if !exists {
            return Err(AppError(TondError::NotFound(format!("project '{}'", pid))));
        }
    }

    // Serialize command array to JSON string if provided
    let command_str = req
        .command
        .as_ref()
        .filter(|cmd| !cmd.is_empty())
        .map(|cmd| serde_json::to_string(cmd).unwrap_or_default());

    // Env map → encrypted JSON for storage
    let env: BTreeMap<String, String> = req.env.clone().unwrap_or_default().into_iter().collect();
    validate_env_keys(env.keys())?;
    let env_str = secrets::seal_cron_env(&env)?;

    let mut job = CronJob::new(
        req.name.clone(),
        req.schedule.clone(),
        req.image.clone(),
        command_str,
        req.project_id.clone(),
        req.timeout_seconds,
        env_str,
        concurrency_policy,
        req.memory_mb,
        req.cpu_shares,
        req.timezone.clone(),
        req.max_retries.unwrap_or(0),
        req.retry_delay_seconds.unwrap_or(60),
        None,
    );
    job.next_run_at = Some(next_run);
    // Volumes are keyed by the job id, so resolve them once the id exists.
    job.volumes =
        confined_cron_volumes(&state.db, &job.id, req.volumes.as_deref().unwrap_or(&[])).await?;

    sqlx::query(
        "INSERT INTO cron_jobs
            (id, name, schedule, image, command, project_id, enabled,
             last_run_at, next_run_at, status, created_at, updated_at,
             timeout_seconds, env, last_run_status, concurrency_policy,
             memory_mb, cpu_shares, timezone, max_retries, retry_delay_seconds,
             volumes)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&job.id)
    .bind(&job.name)
    .bind(&job.schedule)
    .bind(&job.image)
    .bind(&job.command)
    .bind(&job.project_id)
    .bind(job.enabled)
    .bind(&job.last_run_at)
    .bind(&job.next_run_at)
    .bind(&job.status)
    .bind(&job.created_at)
    .bind(&job.updated_at)
    .bind(job.timeout_seconds)
    .bind(&job.env)
    .bind(&job.last_run_status)
    .bind(&job.concurrency_policy)
    .bind(job.memory_mb)
    .bind(job.cpu_shares)
    .bind(&job.timezone)
    .bind(job.max_retries)
    .bind(job.retry_delay_seconds)
    .bind(&job.volumes)
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
        None,
        None,
        "cron_create",
        &auth.audit_actor(),
        Some(&format!("name={} schedule={}", job.name, job.schedule)),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(CronJobResponse::new(job, &env))),
    ))
}

// ── Update cron job ──

pub async fn update_cron_job(
    _rate: RateLimit,
    auth: Authz<scope::CronWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateCronJobRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mut job = fetch_cron_job(&state.db, &id).await?;

    // ── name ──────────────────────────────────────────────────────────────────
    if let Some(ref new_name) = req.name {
        if !new_name.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return Err(AppError(TondError::InvalidInput(
                "Cron job name must be alphanumeric (hyphens allowed)".to_string(),
            )));
        }
        job.name = new_name.clone();
    }

    // ── schedule ──────────────────────────────────────────────────────────────
    if let Some(ref schedule) = req.schedule {
        let next_run = compute_next_run(schedule, job.timezone.as_deref()).map_err(|_| {
            AppError(TondError::InvalidInput(format!(
                "Invalid cron expression: '{}'",
                schedule
            )))
        })?;
        job.schedule = schedule.clone();
        job.next_run_at = Some(next_run);
    }

    // ── image ─────────────────────────────────────────────────────────────────
    if let Some(ref image) = req.image {
        job.image = image.clone();
    }

    // ── command — empty array = clear ─────────────────────────────────────────
    if let Some(ref cmd) = req.command {
        job.command = if cmd.is_empty() {
            None
        } else {
            Some(serde_json::to_string(cmd).unwrap_or_default())
        };
    }

    // ── enabled ───────────────────────────────────────────────────────────────
    if let Some(enabled) = req.enabled {
        job.enabled = if enabled { 1 } else { 0 };
        if enabled {
            if let Ok(next) = compute_next_run(&job.schedule, job.timezone.as_deref()) {
                job.next_run_at = Some(next);
            }
        } else {
            job.next_run_at = None;
        }
    }

    // ── timeout_seconds: Option<Option<i64>> ─────────────────────────────────
    if let Some(new_timeout) = req.timeout_seconds {
        job.timeout_seconds = new_timeout;
    }

    // ── env: full replace, then set/unset patches ─────────────────────────────
    if req.env.is_some() || !req.env_set.is_empty() || !req.env_unset.is_empty() {
        let mut env: BTreeMap<String, String> = match req.env {
            Some(ref new_env) => new_env.clone().into_iter().collect(),
            None => secrets::cron_env(job.env.as_deref())?,
        };
        validate_env_keys(env.keys().chain(req.env_set.keys()))?;
        env.extend(req.env_set.clone());
        for key in &req.env_unset {
            env.remove(key);
        }
        job.env = secrets::seal_cron_env(&env)?;
    }

    // ── project_id: Option<Option<String>> — None=no change, Some(None)=unlink ──
    if let Some(new_pid) = req.project_id {
        if let Some(ref pid) = new_pid {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?)")
                    .bind(pid)
                    .fetch_one(&state.db)
                    .await?;
            if !exists {
                return Err(AppError(TondError::NotFound(format!("project '{}'", pid))));
            }
        }
        job.project_id = new_pid;
    }

    // ── concurrency_policy ────────────────────────────────────────────────────
    if let Some(ref policy) = req.concurrency_policy {
        if policy != "skip" && policy != "allow" {
            return Err(AppError(TondError::InvalidInput(
                "concurrency_policy must be 'skip' or 'allow'".to_string(),
            )));
        }
        job.concurrency_policy = policy.clone();
    }

    // ── memory_mb: Option<Option<i64>> ────────────────────────────────────────
    if let Some(new_mem) = req.memory_mb {
        job.memory_mb = new_mem;
    }

    // ── cpu_shares: Option<Option<i64>> ───────────────────────────────────────
    if let Some(new_cpu) = req.cpu_shares {
        job.cpu_shares = new_cpu;
    }

    // ── timezone: Option<Option<String>> — Some(None) reverts to UTC ─────────
    if let Some(new_tz) = req.timezone {
        job.timezone = new_tz;
        // Recompute next_run with the new timezone if job is enabled
        if job.enabled != 0
            && let Ok(next) = compute_next_run(&job.schedule, job.timezone.as_deref())
        {
            job.next_run_at = Some(next);
        }
    }

    // ── max_retries ───────────────────────────────────────────────────────────
    if let Some(retries) = req.max_retries {
        job.max_retries = retries;
    }

    // ── retry_delay_seconds ───────────────────────────────────────────────────
    if let Some(delay) = req.retry_delay_seconds {
        job.retry_delay_seconds = delay;
    }

    // ── volumes ───────────────────────────────────────────────────────────────
    if let Some(ref vols) = req.volumes {
        job.volumes = confined_cron_volumes(&state.db, &job.id, vols).await?;
    }

    sqlx::query(
        "UPDATE cron_jobs
         SET name = ?, schedule = ?, image = ?, command = ?, enabled = ?,
             next_run_at = ?, timeout_seconds = ?, env = ?,
             project_id = ?, concurrency_policy = ?,
             memory_mb = ?, cpu_shares = ?, timezone = ?,
             max_retries = ?, retry_delay_seconds = ?,
             volumes = ?,
             updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(&job.name)
    .bind(&job.schedule)
    .bind(&job.image)
    .bind(&job.command)
    .bind(job.enabled)
    .bind(&job.next_run_at)
    .bind(job.timeout_seconds)
    .bind(&job.env)
    .bind(&job.project_id)
    .bind(&job.concurrency_policy)
    .bind(job.memory_mb)
    .bind(job.cpu_shares)
    .bind(&job.timezone)
    .bind(job.max_retries)
    .bind(job.retry_delay_seconds)
    .bind(&job.volumes)
    .bind(&job.id)
    .execute(&state.db)
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            AppError(TondError::AppAlreadyExists(job.name.clone()))
        } else {
            AppError(TondError::DatabaseError(e))
        }
    })?;

    write_audit_log(
        &state.db,
        None,
        None,
        "cron_update",
        &auth.audit_actor(),
        Some(&format!("name={}", job.name)),
    )
    .await;

    // Re-fetch to get fresh updated_at
    let updated = fetch_cron_job(&state.db, &job.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(cron_response(updated)?)),
    ))
}

// ── Delete cron job ──

pub async fn delete_cron_job(
    _rate: RateLimit,
    auth: Authz<scope::CronWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let job = fetch_cron_job(&state.db, &id).await?;

    sqlx::query("DELETE FROM cron_jobs WHERE id = ?")
        .bind(&job.id)
        .execute(&state.db)
        .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "cron_delete",
        &auth.audit_actor(),
        Some(&format!("name={}", job.name)),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(serde_json::json!({ "deleted": job.id }))),
    ))
}

// ── Trigger cron job manually ──

pub async fn trigger_cron_job(
    _rate: RateLimit,
    auth: Authz<scope::CronTrigger>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let job = fetch_cron_job(&state.db, &id).await?;

    // Spawn the run in the background (same as the scheduler does)
    let state_clone = state.clone();
    let job_clone = job.clone();
    tokio::spawn(async move {
        cron::run_cron_job(&state_clone, &job_clone, 1).await;
    });

    write_audit_log(
        &state.db,
        None,
        None,
        "cron_trigger",
        &auth.audit_actor(),
        Some(&format!("name={}", job.name)),
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(
            serde_json::json!({ "message": "Cron job triggered", "cron_job_id": job.id }),
        )),
    ))
}

// ── List cron job runs ──

/// Query parameters for the runs endpoint.
#[derive(Debug, Deserialize)]
pub struct CronRunsQuery {
    /// Maximum number of runs to return (default 50, max 200).
    pub limit: Option<i64>,
    /// Number of rows to skip before returning results (cursor-style paging).
    pub offset: Option<i64>,
}

pub async fn list_cron_job_runs(
    _rate: RateLimit,
    _auth: Authz<scope::CronRead>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CronRunsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let job = fetch_cron_job(&state.db, &id).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);

    let runs: Vec<CronJobRun> = sqlx::query_as(
        "SELECT * FROM cron_job_runs WHERE cron_job_id = ?
         ORDER BY started_at DESC
         LIMIT ? OFFSET ?",
    )
    .bind(&job.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(ApiResponse::ok(runs))))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

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
    use tond_common::{CreateCronJobRequest, ServerSettings, UpdateCronJobRequest};

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
            .unwrap_or_else(|err| panic!("failed to run migrations for cron handler tests: {err}"));

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

    async fn seed_cron_job(state: &AppState, name: &str) -> CronJob {
        let mut job = CronJob::new(
            name.to_string(),
            "0 * * * *".to_string(),
            "alpine:latest".to_string(),
            Some("[\"echo\",\"hello\"]".to_string()),
            None,
            Some(60),
            secrets::seal_cron_env(&BTreeMap::from([("MODE".to_string(), "prod".to_string())]))
                .unwrap_or_else(|err| panic!("seal: {err}")),
            "skip".to_string(),
            Some(256),
            Some(1024),
            Some("UTC".to_string()),
            0,
            60,
            Some("[{\"host_path\":\"/tmp\",\"container_path\":\"/data\"}]".to_string()),
        );
        job.next_run_at = Some(
            compute_next_run(&job.schedule, job.timezone.as_deref())
                .unwrap_or_else(|err| panic!("failed to compute seeded cron next_run_at: {err}")),
        );

        sqlx::query(
            "INSERT INTO cron_jobs (id, name, schedule, image, command, project_id, enabled, last_run_at, next_run_at, status, created_at, updated_at, timeout_seconds, env, last_run_status, concurrency_policy, memory_mb, cpu_shares, timezone, max_retries, retry_delay_seconds, volumes)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&job.id)
        .bind(&job.name)
        .bind(&job.schedule)
        .bind(&job.image)
        .bind(&job.command)
        .bind(&job.project_id)
        .bind(job.enabled)
        .bind(&job.last_run_at)
        .bind(&job.next_run_at)
        .bind(&job.status)
        .bind(&job.created_at)
        .bind(&job.updated_at)
        .bind(job.timeout_seconds)
        .bind(&job.env)
        .bind(&job.last_run_status)
        .bind(&job.concurrency_policy)
        .bind(job.memory_mb)
        .bind(job.cpu_shares)
        .bind(&job.timezone)
        .bind(job.max_retries)
        .bind(job.retry_delay_seconds)
        .bind(&job.volumes)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed cron job: {err}"));

        job
    }

    #[tokio::test]
    async fn create_cron_job_rejects_invalid_name() {
        let state = test_state().await;

        let error = create_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateCronJobRequest {
                name: "bad name".to_string(),
                schedule: "0 * * * *".to_string(),
                image: "alpine:latest".to_string(),
                command: None,
                project_id: None,
                timeout_seconds: None,
                env: None,
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_cron_job to reject invalid names"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: Cron job name must be alphanumeric (hyphens allowed)",
            "create_cron_job should reject names with spaces or unsupported characters"
        );
    }

    #[tokio::test]
    async fn create_cron_job_rejects_invalid_concurrency_policy() {
        let state = test_state().await;

        let error = create_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateCronJobRequest {
                name: "nightly".to_string(),
                schedule: "0 * * * *".to_string(),
                image: "alpine:latest".to_string(),
                command: None,
                project_id: None,
                timeout_seconds: None,
                env: None,
                concurrency_policy: Some("queue".to_string()),
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| {
            panic!("expected create_cron_job to reject invalid concurrency policies")
        });

        assert_eq!(
            error.0.to_string(),
            "Invalid input: concurrency_policy must be 'skip' or 'allow'",
            "create_cron_job should reject unsupported concurrency policies"
        );
    }

    #[tokio::test]
    async fn create_cron_job_rejects_invalid_schedule() {
        let state = test_state().await;

        let error = create_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateCronJobRequest {
                name: "nightly".to_string(),
                schedule: "not a cron".to_string(),
                image: "alpine:latest".to_string(),
                command: None,
                project_id: None,
                timeout_seconds: None,
                env: None,
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_cron_job to reject invalid schedules"));

        assert_eq!(
            error.0.to_string(),
            "Invalid input: Invalid cron expression: 'not a cron'",
            "create_cron_job should reject invalid cron expressions"
        );
    }

    #[tokio::test]
    async fn create_cron_job_rejects_unknown_project() {
        let state = test_state().await;

        let error = create_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Json(CreateCronJobRequest {
                name: "nightly".to_string(),
                schedule: "0 * * * *".to_string(),
                image: "alpine:latest".to_string(),
                command: None,
                project_id: Some("missing-project".to_string()),
                timeout_seconds: None,
                env: None,
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected create_cron_job to reject unknown project IDs"));

        assert_eq!(
            error.0.to_string(),
            "Not found: project 'missing-project'",
            "create_cron_job should reject project IDs that do not exist"
        );
    }

    #[tokio::test]
    async fn create_cron_job_persists_defaults_and_serialized_fields() {
        let state = test_state().await;
        seed_project(&state, "project-1", "Platform").await;
        let mut env = HashMap::new();
        env.insert("MODE".to_string(), "prod".to_string());

        let response = create_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(CreateCronJobRequest {
                name: "nightly".to_string(),
                schedule: "0 * * * *".to_string(),
                image: "alpine:latest".to_string(),
                command: Some(vec!["echo".to_string(), "hello".to_string()]),
                project_id: Some("project-1".to_string()),
                timeout_seconds: Some(120),
                env: Some(env),
                concurrency_policy: None,
                memory_mb: Some(256),
                cpu_shares: Some(1024),
                timezone: Some("UTC".to_string()),
                max_retries: None,
                retry_delay_seconds: None,
                volumes: Some(vec![CronVolumeRequest {
                    app: None,
                    name: Some("scratch".to_string()),
                    host_path: None,
                    container_path: "/data".to_string(),
                }]),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected create_cron_job to succeed: {}", err.0))
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "create_cron_job should create valid jobs"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["concurrency_policy"],
            serde_json::json!("skip"),
            "create_cron_job should default concurrency_policy to skip"
        );
        assert_eq!(
            body["data"]["retry_delay_seconds"],
            serde_json::json!(60),
            "create_cron_job should default retry_delay_seconds to 60"
        );
        assert_eq!(
            body["data"]["max_retries"],
            serde_json::json!(0),
            "create_cron_job should default max_retries to 0"
        );
        assert!(
            body["data"]["next_run_at"].as_str().is_some(),
            "create_cron_job should compute and persist next_run_at for valid schedules"
        );
        let host_path = body["data"]["volumes"][0]["host_path"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let job_id = body["data"]["id"].as_str().unwrap_or_default();
        assert!(
            host_path.ends_with(&format!("/volumes/_cron/{job_id}/scratch")),
            "cron volumes should be confined to the job's own directory, got {host_path}"
        );
    }

    #[tokio::test]
    async fn create_cron_job_rejects_host_paths_outside_the_volume_directory() {
        let state = test_state().await;
        let error = create_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Json(CreateCronJobRequest {
                name: "escape".to_string(),
                schedule: "0 * * * *".to_string(),
                image: "alpine:latest".to_string(),
                command: None,
                project_id: None,
                timeout_seconds: None,
                env: None,
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: Some(vec![CronVolumeRequest {
                    app: None,
                    name: None,
                    host_path: Some("/".to_string()),
                    container_path: "/host".to_string(),
                }]),
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("a cron bind of / must be rejected"));
        assert_eq!(error.0.status_code(), 400);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cron_jobs")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("count failed: {err}"));
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn update_cron_job_rejects_invalid_name_schedule_and_concurrency_policy() {
        let state = test_state().await;
        let job = seed_cron_job(&state, "nightly").await;

        let bad_name = update_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(job.id.clone()),
            Json(UpdateCronJobRequest {
                name: Some("bad name".to_string()),
                schedule: None,
                image: None,
                command: None,
                enabled: None,
                timeout_seconds: None,
                env: None,
                env_set: Default::default(),
                env_unset: vec![],
                project_id: None,
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected update_cron_job to reject invalid names"));
        assert_eq!(
            bad_name.0.to_string(),
            "Invalid input: Cron job name must be alphanumeric (hyphens allowed)",
            "update_cron_job should reject invalid job names"
        );

        let bad_schedule = update_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(job.id.clone()),
            Json(UpdateCronJobRequest {
                name: None,
                schedule: Some("not a cron".to_string()),
                image: None,
                command: None,
                enabled: None,
                timeout_seconds: None,
                env: None,
                env_set: Default::default(),
                env_unset: vec![],
                project_id: None,
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected update_cron_job to reject invalid schedules"));
        assert_eq!(
            bad_schedule.0.to_string(),
            "Invalid input: Invalid cron expression: 'not a cron'",
            "update_cron_job should reject invalid cron expressions"
        );

        let bad_policy = update_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Path(job.id),
            Json(UpdateCronJobRequest {
                name: None,
                schedule: None,
                image: None,
                command: None,
                enabled: None,
                timeout_seconds: None,
                env: None,
                env_set: Default::default(),
                env_unset: vec![],
                project_id: None,
                concurrency_policy: Some("queue".to_string()),
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| {
            panic!("expected update_cron_job to reject invalid concurrency policies")
        });
        assert_eq!(
            bad_policy.0.to_string(),
            "Invalid input: concurrency_policy must be 'skip' or 'allow'",
            "update_cron_job should reject unsupported concurrency policies"
        );
    }

    #[tokio::test]
    async fn update_cron_job_supports_clearing_and_setting_patch_fields() {
        let state = test_state().await;
        seed_project(&state, "project-1", "Platform").await;
        let job = seed_cron_job(&state, "nightly").await;

        let response = update_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(job.id.clone()),
            Json(UpdateCronJobRequest {
                name: Some("nightly-updated".to_string()),
                schedule: Some("0 0 * * *".to_string()),
                image: Some("busybox:latest".to_string()),
                command: Some(vec![]),
                enabled: Some(false),
                timeout_seconds: Some(None),
                env: Some(HashMap::new()),
                env_set: Default::default(),
                env_unset: vec![],
                project_id: Some(Some("project-1".to_string())),
                concurrency_policy: Some("allow".to_string()),
                memory_mb: Some(None),
                cpu_shares: Some(Some(2048)),
                timezone: Some(None),
                max_retries: Some(3),
                retry_delay_seconds: Some(15),
                volumes: Some(vec![]),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected update_cron_job to succeed: {}", err.0))
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "update_cron_job should succeed for valid patches"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["name"],
            serde_json::json!("nightly-updated"),
            "update_cron_job should update the job name"
        );
        assert_eq!(
            body["data"]["enabled"],
            serde_json::json!(false),
            "update_cron_job should disable the job when requested"
        );
        assert_eq!(
            body["data"]["command"],
            serde_json::Value::Null,
            "empty command arrays should clear the stored command"
        );
        assert_eq!(
            body["data"]["env"],
            serde_json::json!([]),
            "empty env maps should clear stored env vars"
        );
        assert_eq!(
            body["data"]["volumes"],
            serde_json::Value::Null,
            "empty volumes should clear stored volume mounts"
        );
        assert_eq!(
            body["data"]["timeout_seconds"],
            serde_json::Value::Null,
            "Some(None) timeout patch should clear timeout_seconds"
        );
        assert_eq!(
            body["data"]["memory_mb"],
            serde_json::Value::Null,
            "Some(None) memory patch should clear memory_mb"
        );
        assert_eq!(
            body["data"]["cpu_shares"],
            serde_json::json!(2048),
            "Some(Some(value)) cpu patch should update cpu_shares"
        );
        assert_eq!(
            body["data"]["timezone"],
            serde_json::Value::Null,
            "Some(None) timezone patch should clear the timezone"
        );
        assert_eq!(
            body["data"]["project_id"],
            serde_json::json!("project-1"),
            "project_id patch should link the specified project"
        );
        assert_eq!(
            body["data"]["concurrency_policy"],
            serde_json::json!("allow"),
            "update_cron_job should update concurrency_policy"
        );
        assert_eq!(
            body["data"]["max_retries"],
            serde_json::json!(3),
            "update_cron_job should update max_retries"
        );
        assert_eq!(
            body["data"]["retry_delay_seconds"],
            serde_json::json!(15),
            "update_cron_job should update retry_delay_seconds"
        );
        assert_eq!(
            body["data"]["next_run_at"],
            serde_json::Value::Null,
            "disabling a cron job should clear next_run_at"
        );
    }

    #[tokio::test]
    async fn update_cron_job_rejects_unknown_project_on_patch() {
        let state = test_state().await;
        let job = seed_cron_job(&state, "nightly").await;

        let error = update_cron_job(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Path(job.id),
            Json(UpdateCronJobRequest {
                name: None,
                schedule: None,
                image: None,
                command: None,
                enabled: None,
                timeout_seconds: None,
                env: None,
                env_set: Default::default(),
                env_unset: vec![],
                project_id: Some(Some("missing-project".to_string())),
                concurrency_policy: None,
                memory_mb: None,
                cpu_shares: None,
                timezone: None,
                max_retries: None,
                retry_delay_seconds: None,
                volumes: None,
            }),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected update_cron_job to reject unknown project IDs"));

        assert_eq!(
            error.0.to_string(),
            "Not found: project 'missing-project'",
            "update_cron_job should reject project IDs that do not exist"
        );
    }

    #[tokio::test]
    async fn list_cron_job_runs_applies_limit_clamp_and_offset_floor() {
        let state = test_state().await;
        let job = seed_cron_job(&state, "nightly").await;

        for index in 0..3 {
            sqlx::query(
                "INSERT INTO cron_job_runs (id, cron_job_id, started_at, finished_at, exit_code, status, output, attempt) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(format!("run-{index}"))
            .bind(&job.id)
            .bind(format!("2026-01-01T00:0{index}:00Z"))
            .bind(format!("2026-01-01T00:0{index}:30Z"))
            .bind(0)
            .bind("success")
            .bind(format!("output-{index}"))
            .bind(1)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to seed cron run {index}: {err}"));
        }

        let response = list_cron_job_runs(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state),
            Path(job.id),
            Query(CronRunsQuery {
                limit: Some(500),
                offset: Some(-10),
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("expected list_cron_job_runs to succeed: {}", err.0))
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "list_cron_job_runs should succeed for existing jobs"
        );
        let body = response_json(response).await;
        let runs = body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("expected cron runs response data to be an array"));
        assert_eq!(
            runs.len(),
            3,
            "list_cron_job_runs should clamp oversized limits and floor negative offsets"
        );
    }
}
