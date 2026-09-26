// cron.rs — Background cron job scheduler (Phase 6.4 + 6.5)
//
// Exports:
//   run_cron_scheduler(state)        — long-running Tokio task. On start it closes out
//                                      runs a previous process left 'running' (see
//                                      reconcile_interrupted_runs). Then it wakes every
//                                      30 s, finds enabled jobs whose next_run_at <= now,
//                                      advances next_run_at to the next slot (a
//                                      compare-and-set, *before* the run starts, so a
//                                      long 'allow' run is never re-started every tick)
//                                      and runs each one's Docker container.
//
//   run_cron_job(state, job, attempt) — runs a single cron job container (with an
//                                       internal retry loop), records each run in
//                                       cron_job_runs, updates the job's
//                                       last_run_at / last_run_status.
//
//   compute_next_run(schedule, tz)   — parses a 5-field cron expression and returns
//                                      the next datetime (in the given IANA tz, or UTC)
//                                      as an ISO-8601 string.
//
// Phase 6.5 additions:
//   • Timezone-aware next-run computation via chrono-tz.
//   • Concurrency policy: 'skip' (default) skips if already running; 'allow' starts anyway.
//   • Resource limits: memory_mb and cpu_shares wired into Docker HostConfig.
//   • Volume mounts: volumes JSON array wired into Docker HostConfig.binds, every
//     host path re-derived and confined by `volumes::cron_binds`.
//   • Isolation: the container runs on the job's own network, or its project's
//     (networks.rs), never on Docker's default bridge.
//   • Hardening: the same no-new-privileges / capability / pids / memory / log
//     rotation policy as app containers (`docker::harden_host_config`).
//   • Retry logic: on non-zero exit, re-queues the job up to max_retries times with
//     retry_delay_seconds delay. Attempt number is tracked on the run row.
//   • last_run_status: updated on the cron_jobs row at the end of each run (final attempt).

use anyhow::Result;
use bollard::query_parameters::{ListContainersOptionsBuilder, RemoveContainerOptionsBuilder};
use chrono::Utc;
use cron::Schedule;
use std::collections::HashMap;
use std::str::FromStr;
use tond_common::{CronJob, CronVolume};
use uuid::Uuid;

use crate::{alerts, docker, metrics, networks, secrets, state::AppState, volumes};

fn should_run_due_job(job: &CronJob) -> bool {
    !(job.status == "running" && job.concurrency_policy == "skip")
}

/// Decrypt the stored env into `KEY=value` strings. An env that cannot be
/// decrypted fails the run instead of silently running without it.
fn cron_env_vec(stored: Option<&str>) -> Result<Vec<String>> {
    Ok(secrets::cron_env(stored)?
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect())
}

/// Parse the stored volumes JSON and turn it into confined Docker bind strings.
/// Unparseable JSON fails the run rather than silently running without mounts.
fn cron_binds(
    data_dir: &std::path::Path,
    job_id: &str,
    volumes_json: Option<&str>,
) -> Result<Option<Vec<String>>> {
    let Some(json) = volumes_json.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let vols: Vec<CronVolume> = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("invalid volume configuration: {e}"))?;
    if vols.is_empty() {
        return Ok(None);
    }
    Ok(Some(volumes::cron_binds(data_dir, job_id, &vols)?))
}

fn cron_memory_limit_bytes(memory_mb: Option<i64>) -> Option<i64> {
    memory_mb.map(|mb| mb * 1024 * 1024)
}

fn cron_attempt_plan(max_retries: i64) -> i64 {
    max_retries + 1
}

// ── Next-run computation ──────────────────────────────────────────────────────

/// Parse a 5-field standard cron expression and return the next datetime
/// as an RFC 3339 string.  If `timezone` is a valid IANA tz name the schedule
/// is evaluated in that timezone; otherwise UTC is used.
pub fn compute_next_run(schedule_str: &str, timezone: Option<&str>) -> Result<String> {
    // The `cron` crate uses 7-field expressions: sec min hour dom month dow year.
    // Pad a 5-field expression by prepending "0 " (sec=0) and appending " *" (year=any).
    let padded = format!("0 {} *", schedule_str);
    let schedule = Schedule::from_str(&padded)?;

    if let Some(tz_name) = timezone {
        use chrono_tz::Tz;
        if let Ok(tz) = tz_name.parse::<Tz>() {
            let next = schedule
                .upcoming(tz)
                .next()
                .ok_or_else(|| anyhow::anyhow!("No upcoming time for '{}'", schedule_str))?;
            // Convert to UTC RFC 3339
            return Ok(next.with_timezone(&Utc).to_rfc3339());
        }
        // Unknown tz — fall through to UTC
        tracing::warn!("Unknown timezone '{}'; falling back to UTC", tz_name);
    }

    let next = schedule
        .upcoming(Utc)
        .next()
        .ok_or_else(|| anyhow::anyhow!("No upcoming time for schedule '{}'", schedule_str))?;
    Ok(next.to_rfc3339())
}

// ── Scheduler loop ────────────────────────────────────────────────────────────

/// What the scheduler does with a job whose `next_run_at` has passed.
#[derive(Debug, PartialEq, Eq)]
enum DueAction {
    /// Advance `next_run_at` and start a run.
    Run,
    /// Advance `next_run_at` without running ('skip' policy, previous run
    /// still going): the missed slot is dropped instead of piling up.
    SkipSlot,
}

fn due_action(job: &CronJob) -> DueAction {
    if should_run_due_job(job) {
        DueAction::Run
    } else {
        DueAction::SkipSlot
    }
}

pub async fn run_cron_scheduler(state: AppState) {
    // Runs left 'running' by a previous process can never finish; close them
    // out so the jobs are scheduled again.
    match reconcile_interrupted_runs(&state.db).await {
        Ok(0) => {}
        Ok(n) => tracing::warn!("Marked {} interrupted cron run(s) as failed", n),
        Err(e) => tracing::warn!("Cron startup reconciliation failed: {}", e),
    }
    remove_orphaned_cron_containers(&state.docker).await;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        schedule_due_jobs(&state).await;
    }
}

/// One scheduler tick: claim every due job (advancing `next_run_at` *before*
/// the run starts, so a long run is never started again on the next tick) and
/// spawn the runs.
async fn schedule_due_jobs(state: &AppState) {
    for job in claim_due_jobs(&state.db).await {
        let state_clone = state.clone();
        tokio::spawn(async move {
            run_cron_job(&state_clone, &job, 1).await;
        });
    }
}

/// Find due jobs, advance each one's schedule, and return the jobs that
/// should start a run now.
async fn claim_due_jobs(db: &sqlx::SqlitePool) -> Vec<CronJob> {
    let now = Utc::now().to_rfc3339();
    let due_jobs: Vec<CronJob> = match sqlx::query_as(
        "SELECT * FROM cron_jobs
         WHERE enabled = 1
           AND next_run_at IS NOT NULL
           AND next_run_at <= ?",
    )
    .bind(&now)
    .fetch_all(db)
    .await
    {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!("Cron scheduler DB query failed: {}", e);
            return Vec::new();
        }
    };

    let mut to_run = Vec::new();
    for job in due_jobs {
        match claim_due_job(db, &job).await {
            Ok(true) => {}
            Ok(false) => continue, // someone else advanced it (edit, disable, ...)
            Err(e) => {
                tracing::warn!("Cron job '{}': could not advance schedule: {}", job.name, e);
                continue;
            }
        }
        match due_action(&job) {
            DueAction::Run => to_run.push(job),
            DueAction::SkipSlot => {
                tracing::debug!("Cron job '{}' still running; skipping this slot", job.name);
            }
        }
    }
    to_run
}

/// Advance a due job's `next_run_at` to its next slot after now. The update is
/// a compare-and-set on the `next_run_at` value we read, so a slot is claimed
/// at most once. An unparseable schedule clears `next_run_at` (the job stops
/// firing instead of firing every tick). Returns whether a run may start.
async fn claim_due_job(db: &sqlx::SqlitePool, job: &CronJob) -> Result<bool> {
    let next = match compute_next_run(&job.schedule, job.timezone.as_deref()) {
        Ok(next) => Some(next),
        Err(e) => {
            tracing::warn!("Cron job '{}' has an invalid schedule: {}", job.name, e);
            None
        }
    };
    let claimed = sqlx::query(
        "UPDATE cron_jobs SET next_run_at = ?, updated_at = datetime('now')
         WHERE id = ? AND next_run_at = ?",
    )
    .bind(&next)
    .bind(&job.id)
    .bind(&job.next_run_at)
    .execute(db)
    .await?
    .rows_affected()
        == 1;
    Ok(claimed && next.is_some())
}

/// Close out runs a previous server process left 'running' (the process died
/// mid-run, so nothing will ever finish them) and reset their jobs so the
/// scheduler picks them up again. Returns the number of runs closed.
pub async fn reconcile_interrupted_runs(db: &sqlx::SqlitePool) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let runs = sqlx::query(
        "UPDATE cron_job_runs
         SET status = 'failed', finished_at = ?,
             output = COALESCE(output || char(10), '') ||
                      'Interrupted: tond-server restarted while this run was in progress.'
         WHERE status = 'running'",
    )
    .bind(&now)
    .execute(db)
    .await?
    .rows_affected();

    sqlx::query(
        "UPDATE cron_jobs
         SET status = 'failed', last_run_status = 'failed', updated_at = datetime('now')
         WHERE status = 'running'",
    )
    .execute(db)
    .await?;

    // Enabled jobs without a next slot get one now.
    let unscheduled: Vec<CronJob> =
        sqlx::query_as("SELECT * FROM cron_jobs WHERE enabled = 1 AND next_run_at IS NULL")
            .fetch_all(db)
            .await?;
    for job in unscheduled {
        if let Ok(next) = compute_next_run(&job.schedule, job.timezone.as_deref()) {
            sqlx::query("UPDATE cron_jobs SET next_run_at = ? WHERE id = ?")
                .bind(next)
                .bind(&job.id)
                .execute(db)
                .await?;
        }
    }

    Ok(runs)
}

/// Best-effort removal of cron containers orphaned by a restart (their runs
/// were just marked interrupted; nothing tracks them any more).
async fn remove_orphaned_cron_containers(docker: &bollard::Docker) {
    let filters: HashMap<&str, Vec<&str>> = HashMap::from([("label", vec!["tond-type=cron"])]);
    let Ok(containers) = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await
    else {
        return;
    };
    for id in containers.into_iter().filter_map(|c| c.id) {
        let _ = docker
            .remove_container(
                &id,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
    }
}

/// Set a job's status after one of its runs ends: 'running' while another run
/// of the same job ('allow' policy) is still in progress, else `status`.
/// `last_run_status` is only written when given (final attempt).
async fn finish_job_run(
    db: &sqlx::SqlitePool,
    job_id: &str,
    status: &str,
    finished_at: &str,
    last_run_status: Option<&str>,
) {
    let _ = sqlx::query(
        "UPDATE cron_jobs
         SET status = CASE WHEN EXISTS (
                 SELECT 1 FROM cron_job_runs WHERE cron_job_id = ?1 AND status = 'running'
             ) THEN 'running' ELSE ?2 END,
             last_run_at = ?3,
             last_run_status = COALESCE(?4, last_run_status),
             updated_at = datetime('now')
         WHERE id = ?1",
    )
    .bind(job_id)
    .bind(status)
    .bind(finished_at)
    .bind(last_run_status)
    .execute(db)
    .await;
}

// ── Single job execution ──────────────────────────────────────────────────────

/// Run a single cron job container, record the run, and update the job row.
/// Retries up to `job.max_retries` times on failure using a loop (no recursion).
/// This function is intentionally infallible from the caller's perspective —
/// all errors are logged and recorded in cron_job_runs.
pub async fn run_cron_job(state: &AppState, job: &CronJob, _initial_attempt: i64) {
    // Parse command once, outside the retry loop.
    let cmd_vec: Option<Vec<String>> = job
        .command
        .as_ref()
        .and_then(|cmd| serde_json::from_str(cmd).ok());

    let max_attempts = cron_attempt_plan(job.max_retries); // e.g. max_retries=2 → 3 total attempts
    let delay = std::time::Duration::from_secs(job.retry_delay_seconds as u64);

    for attempt in 1..=max_attempts {
        let run_id = Uuid::new_v4().to_string();
        let started_at = Utc::now().to_rfc3339();

        // Mark job as running
        let _ = sqlx::query(
            "UPDATE cron_jobs SET status = 'running', updated_at = datetime('now') WHERE id = ?",
        )
        .bind(&job.id)
        .execute(&state.db)
        .await;

        // Insert run record with status = running
        let _ = sqlx::query(
            "INSERT INTO cron_job_runs (id, cron_job_id, status, started_at, attempt)
             VALUES (?, ?, 'running', ?, ?)",
        )
        .bind(&run_id)
        .bind(&job.id)
        .bind(&started_at)
        .bind(attempt)
        .execute(&state.db)
        .await;

        let container_name = format!("tond-cron-{}-{}", &job.id[..8], &run_id[..8]);
        let result = run_cron_container(state, job, &container_name, cmd_vec.as_deref()).await;
        let finished_at = Utc::now().to_rfc3339();

        let (succeeded, exit_code_opt, output_str) = match result {
            Ok((exit_code, output)) => (exit_code == 0, Some(exit_code), output),
            Err(ref e) => {
                tracing::error!("Cron job '{}' failed to run: {}", job.name, e);
                (false, None, format!("Error: {}", e))
            }
        };

        // Update run row
        if let Some(exit_code) = exit_code_opt {
            let run_status = if succeeded { "success" } else { "failed" };
            let _ = sqlx::query(
                "UPDATE cron_job_runs
                 SET status = ?, exit_code = ?, output = ?, finished_at = ?
                 WHERE id = ?",
            )
            .bind(run_status)
            .bind(exit_code)
            .bind(&output_str)
            .bind(&finished_at)
            .bind(&run_id)
            .execute(&state.db)
            .await;
        } else {
            let _ = sqlx::query(
                "UPDATE cron_job_runs
                 SET status = 'failed', output = ?, finished_at = ?
                 WHERE id = ?",
            )
            .bind(&output_str)
            .bind(&finished_at)
            .bind(&run_id)
            .execute(&state.db)
            .await;
        }

        if succeeded {
            // Success. next_run_at was already advanced when the scheduler
            // claimed the slot.
            finish_job_run(&state.db, &job.id, "idle", &finished_at, Some("success")).await;

            tracing::info!("Cron job '{}' succeeded on attempt {}", job.name, attempt);

            alerts::fire_event_alerts(
                state,
                "cron_success",
                None,
                Some(job.name.as_str()),
                Some(&format!("exit_code={}", exit_code_opt.unwrap_or(0))),
            )
            .await;
            return;
        }

        // Failed attempt — is there another retry?
        if attempt < max_attempts {
            tracing::info!(
                "Cron job '{}' failed (attempt {}/{}), retrying in {}s",
                job.name,
                attempt,
                max_attempts,
                job.retry_delay_seconds
            );
            finish_job_run(&state.db, &job.id, "failed", &finished_at, None).await;

            tokio::time::sleep(delay).await;
            continue;
        }

        // All attempts exhausted — final failure
        finish_job_run(&state.db, &job.id, "failed", &finished_at, Some("failed")).await;

        tracing::warn!(
            "Cron job '{}' failed on all {} attempt(s)",
            job.name,
            max_attempts
        );

        alerts::fire_event_alerts(
            state,
            "cron_failure",
            None,
            Some(job.name.as_str()),
            Some(&output_str),
        )
        .await;
    }
}

// ── Docker execution helper ───────────────────────────────────────────────────

/// Start the cron job container, wait for it to finish, collect output.
/// Returns `(exit_code, output_string)` on success.
async fn run_cron_container(
    state: &AppState,
    job: &CronJob,
    container_name: &str,
    cmd: Option<&[String]>,
) -> Result<(i64, String)> {
    use bollard::Docker;
    use bollard::models::*;
    use bollard::query_parameters::{
        CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
        WaitContainerOptionsBuilder,
    };
    use futures::StreamExt;
    use std::collections::HashMap;

    let docker: &Docker = &state.docker;

    // Decrypt the stored env, e.g. {"KEY":"val"} → ["KEY=val"]
    let env_vec = cron_env_vec(job.env.as_deref())?;

    let cmd_vec: Option<Vec<&str>> = cmd.map(|c| c.iter().map(|s| s.as_str()).collect());

    // Parse volume mounts (confined to <data>/volumes, see volumes.rs)
    let binds = cron_binds(&volumes::data_dir(), &job.id, job.volumes.as_deref())?;

    // Resource limits
    let memory = cron_memory_limit_bytes(job.memory_mb);
    let cpu_shares: Option<i64> = job.cpu_shares;

    // The job's own network (or its project's) — never the default bridge.
    let network = networks::ensure_cron_home(docker, job).await?;

    let mut host_config = HostConfig {
        auto_remove: Some(false), // we remove manually after collecting logs
        memory,
        cpu_shares,
        binds,
        network_mode: Some(network),
        ..Default::default()
    };
    docker::harden_host_config(&mut host_config, docker::default_memory_limit());

    let container_config = ContainerCreateBody {
        image: Some(job.image.clone()),
        cmd: cmd_vec.map(|v| v.into_iter().map(String::from).collect()),
        env: Some(env_vec),
        labels: {
            let mut m = HashMap::new();
            m.insert("managed-by".to_string(), "tond".to_string());
            m.insert("tond-type".to_string(), "cron".to_string());
            m.insert(metrics::CRON_JOB_LABEL.to_string(), job.id.clone());
            Some(m)
        },
        host_config: Some(host_config),
        ..Default::default()
    };

    // Force-remove any stale container with the same name
    let _ = docker
        .remove_container(
            container_name,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await;

    // Create container
    docker::ensure_image(docker, &job.image).await?;
    let create_opts = CreateContainerOptionsBuilder::default()
        .name(container_name)
        .build();
    let created = docker
        .create_container(Some(create_opts), container_config)
        .await?;
    let cid = created.id;

    // Start it
    docker.start_container(&cid, None).await?;

    // Wait for it to exit, optionally bounded by timeout_seconds
    let wait_opts = WaitContainerOptionsBuilder::default()
        .condition("not-running")
        .build();

    let mut exit_code: i64 = 0;

    let timed_out = if let Some(secs) = job.timeout_seconds.filter(|&s| s > 0) {
        let wait_future = async {
            let mut wait_stream = docker.wait_container(&cid, Some(wait_opts));
            while let Some(result) = wait_stream.next().await {
                match result {
                    Ok(resp) => exit_code = resp.status_code,
                    Err(e) => tracing::warn!("Wait container error: {}", e),
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(secs as u64), wait_future)
            .await
            .is_err()
    } else {
        let mut wait_stream = docker.wait_container(&cid, Some(wait_opts));
        while let Some(result) = wait_stream.next().await {
            match result {
                Ok(resp) => exit_code = resp.status_code,
                Err(e) => tracing::warn!("Wait container error: {}", e),
            }
        }
        false
    };

    if timed_out {
        // Kill the container (best-effort) before collecting any partial logs
        let _ = docker.kill_container(&cid, None).await;
        tracing::warn!(
            "Cron job '{}' timed out after {}s",
            job.name,
            job.timeout_seconds.unwrap_or(0)
        );
    }

    // Collect logs (partial output is still useful on timeout)
    let log_opts = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .tail("200")
        .build();
    let mut log_stream = docker.logs(&cid, Some(log_opts));
    let mut lines: Vec<String> = Vec::new();
    while let Some(Ok(chunk)) = log_stream.next().await {
        let text = String::from_utf8_lossy(chunk.into_bytes().as_ref()).to_string();
        for line in text.lines() {
            lines.push(line.to_string());
        }
    }

    if timed_out {
        lines.push(format!(
            "Error: container timed out after {}s",
            job.timeout_seconds.unwrap_or(0)
        ));
    }

    let output = lines.join("\n");

    // Remove the container (best-effort)
    let _ = docker
        .remove_container(
            &cid,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await;

    if timed_out {
        // Surface as an error so the run is recorded as failed
        anyhow::bail!(
            "container timed out after {}s",
            job.timeout_seconds.unwrap_or(0)
        );
    }

    Ok((exit_code, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tond_common::CronJob;

    fn sample_job() -> CronJob {
        CronJob::new(
            "nightly".to_string(),
            "0 * * * *".to_string(),
            "alpine:3".to_string(),
            Some(r#"["echo","hello"]"#.to_string()),
            None,
            Some(60),
            secrets::seal_cron_env(&std::collections::BTreeMap::from([
                ("RUST_LOG".to_string(), "info".to_string()),
                ("MODE".to_string(), "prod".to_string()),
            ]))
            .unwrap_or_else(|err| panic!("seal: {err}")),
            "skip".to_string(),
            Some(256),
            Some(512),
            Some("UTC".to_string()),
            2,
            5,
            Some(
                r#"[{"name":"input","host_path":"/ignored","container_path":"/mnt/input"}]"#
                    .to_string(),
            ),
        )
    }

    async fn test_db() -> sqlx::SqlitePool {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for cron tests: {err}"));
        db
    }

    async fn seed_job(db: &sqlx::SqlitePool, id: &str, policy: &str, status: &str, schedule: &str) {
        sqlx::query(
            "INSERT INTO cron_jobs (id, name, schedule, image, status, concurrency_policy, next_run_at)
             VALUES (?, ?, ?, 'alpine', ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("job-{id}"))
        .bind(schedule)
        .bind(status)
        .bind(policy)
        .bind((Utc::now() - chrono::Duration::minutes(5)).to_rfc3339())
        .execute(db)
        .await
        .unwrap_or_else(|err| panic!("seed cron job: {err}"));
    }

    async fn next_run_at(db: &sqlx::SqlitePool, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT next_run_at FROM cron_jobs WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await
            .unwrap_or_else(|err| panic!("read next_run_at: {err}"))
    }

    #[tokio::test]
    async fn claim_advances_next_run_before_the_run_so_long_allow_runs_are_not_restarted() {
        let db = test_db().await;
        // An 'allow' job whose previous run is still going and whose slot is due.
        seed_job(&db, "allow", "allow", "running", "0 * * * *").await;

        let first = claim_due_jobs(&db).await;
        assert_eq!(
            first.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
            vec!["allow"],
            "a due 'allow' job starts a run even while another is running"
        );
        let next = next_run_at(&db, "allow")
            .await
            .unwrap_or_else(|| panic!("next_run_at should be set"));
        assert!(
            next > Utc::now().to_rfc3339(),
            "next_run_at must move into the future when the run starts"
        );

        // The next tick (30 s later) must not start another container.
        assert!(
            claim_due_jobs(&db).await.is_empty(),
            "a claimed slot must not be run again on the next tick"
        );
    }

    #[tokio::test]
    async fn skip_policy_drops_the_slot_while_running_and_invalid_schedules_stop_firing() {
        let db = test_db().await;
        seed_job(&db, "skip", "skip", "running", "0 * * * *").await;
        seed_job(&db, "broken", "skip", "idle", "not a cron").await;

        assert!(
            claim_due_jobs(&db).await.is_empty(),
            "neither a running 'skip' job nor an invalid schedule starts a run"
        );
        assert!(
            next_run_at(&db, "skip").await.unwrap_or_default() > Utc::now().to_rfc3339(),
            "a skipped slot still advances the schedule"
        );
        assert_eq!(
            next_run_at(&db, "broken").await,
            None,
            "an invalid schedule clears next_run_at instead of firing every tick"
        );
    }

    #[tokio::test]
    async fn reconcile_marks_interrupted_runs_failed_and_frees_the_job() {
        let db = test_db().await;
        seed_job(&db, "j1", "skip", "running", "0 * * * *").await;
        sqlx::query(
            "INSERT INTO cron_job_runs (id, cron_job_id, status, started_at) VALUES ('r1', 'j1', 'running', ?)",
        )
        .bind(Utc::now().to_rfc3339())
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("seed run: {err}"));

        let closed = reconcile_interrupted_runs(&db)
            .await
            .unwrap_or_else(|err| panic!("reconcile: {err}"));
        assert_eq!(closed, 1);

        let (status, finished, output): (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status, finished_at, output FROM cron_job_runs WHERE id = 'r1'")
                .fetch_one(&db)
                .await
                .unwrap_or_else(|err| panic!("read run: {err}"));
        assert_eq!(status, "failed");
        assert!(finished.is_some(), "interrupted runs get a finished_at");
        assert!(output.unwrap_or_default().contains("Interrupted"));

        let job_status: String = sqlx::query_scalar("SELECT status FROM cron_jobs WHERE id = 'j1'")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|err| panic!("read job: {err}"));
        assert_eq!(job_status, "failed");
        assert_eq!(
            claim_due_jobs(&db).await.len(),
            1,
            "the 'skip' job runs again once its stale 'running' status is cleared"
        );
    }

    #[tokio::test]
    async fn finishing_one_of_two_overlapping_runs_keeps_the_job_running() {
        let db = test_db().await;
        seed_job(&db, "j1", "allow", "running", "0 * * * *").await;
        sqlx::query(
            "INSERT INTO cron_job_runs (id, cron_job_id, status, started_at) VALUES ('other', 'j1', 'running', 'x')",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|err| panic!("seed run: {err}"));

        finish_job_run(&db, "j1", "idle", "2026-01-01T00:00:00Z", Some("success")).await;
        let (status, last): (String, Option<String>) =
            sqlx::query_as("SELECT status, last_run_status FROM cron_jobs WHERE id = 'j1'")
                .fetch_one(&db)
                .await
                .unwrap_or_else(|err| panic!("read job: {err}"));
        assert_eq!(status, "running", "another run is still in progress");
        assert_eq!(last.as_deref(), Some("success"));

        sqlx::query("UPDATE cron_job_runs SET status = 'success' WHERE id = 'other'")
            .execute(&db)
            .await
            .unwrap_or_else(|err| panic!("finish run: {err}"));
        finish_job_run(&db, "j1", "idle", "2026-01-01T00:01:00Z", None).await;
        let (status, last): (String, Option<String>) =
            sqlx::query_as("SELECT status, last_run_status FROM cron_jobs WHERE id = 'j1'")
                .fetch_one(&db)
                .await
                .unwrap_or_else(|err| panic!("read job: {err}"));
        assert_eq!(status, "idle");
        assert_eq!(
            last.as_deref(),
            Some("success"),
            "intermediate updates keep the last final status"
        );
    }

    #[test]
    fn compute_next_run_accepts_valid_schedule_and_falls_back_to_utc_for_unknown_timezone() {
        let utc = compute_next_run("0 * * * *", Some("UTC"))
            .unwrap_or_else(|err| panic!("UTC schedule computation should succeed: {err}"));
        let unknown_tz = compute_next_run("0 * * * *", Some("Mars/Phobos")).unwrap_or_else(|err| {
            panic!("unknown timezone schedule computation should fall back to UTC: {err}")
        });

        assert!(
            chrono::DateTime::parse_from_rfc3339(&utc).is_ok(),
            "compute_next_run should return an RFC3339 timestamp for valid schedules"
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(&unknown_tz).is_ok(),
            "compute_next_run should still return a valid RFC3339 timestamp when timezone parsing falls back to UTC"
        );
    }

    #[test]
    fn compute_next_run_rejects_invalid_cron_expressions() {
        assert!(
            compute_next_run("not a cron", Some("UTC")).is_err(),
            "compute_next_run should reject malformed cron expressions"
        );
    }

    #[test]
    fn should_run_due_job_honors_skip_and_allow_concurrency_policies() {
        let mut job = sample_job();
        job.status = "running".to_string();
        job.concurrency_policy = "skip".to_string();
        assert!(
            !should_run_due_job(&job),
            "should_run_due_job should skip already-running jobs when concurrency policy is 'skip'"
        );

        job.concurrency_policy = "allow".to_string();
        assert!(
            should_run_due_job(&job),
            "should_run_due_job should allow overlapping runs when concurrency policy is 'allow'"
        );

        job.status = "idle".to_string();
        job.concurrency_policy = "skip".to_string();
        assert!(
            should_run_due_job(&job),
            "should_run_due_job should run due jobs that are not currently running"
        );
    }

    #[test]
    fn cron_env_and_bind_helpers_parse_json_and_tolerate_invalid_input() {
        let job = sample_job();

        let env = cron_env_vec(job.env.as_deref()).unwrap_or_default();
        assert!(
            env.contains(&"RUST_LOG=info".to_string()) && env.contains(&"MODE=prod".to_string()),
            "cron_env_vec should convert the stored JSON object into KEY=value strings"
        );

        assert!(
            cron_env_vec(Some("not-json")).is_err(),
            "cron_env_vec should fail loudly on a value that does not decrypt"
        );
        assert_eq!(cron_env_vec(None).ok(), Some(Vec::new()));

        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let binds = cron_binds(tmp.path(), &job.id, job.volumes.as_deref())
            .unwrap_or_else(|e| panic!("cron_binds failed: {e}"))
            .unwrap_or_default();
        assert_eq!(binds.len(), 1);
        assert!(
            binds[0].ends_with(&format!("volumes/_cron/{}/input:/mnt/input", job.id)),
            "cron_binds should map stored volumes into the job's confined directory: {}",
            binds[0]
        );
        assert!(
            cron_binds(tmp.path(), &job.id, Some("not-json")).is_err(),
            "cron_binds should fail loudly on invalid volume JSON"
        );
        assert!(cron_binds(tmp.path(), &job.id, None).is_ok_and(|b| b.is_none()));

        // A tampered row pointing at the host root is re-derived from the name.
        let tampered = r#"[{"name":"etc","host_path":"/","container_path":"/host"}]"#;
        let binds = cron_binds(tmp.path(), &job.id, Some(tampered))
            .unwrap_or_else(|e| panic!("cron_binds failed: {e}"))
            .unwrap_or_default();
        assert!(!binds[0].starts_with("/:"), "{}", binds[0]);
    }

    #[test]
    fn cron_resource_and_retry_helpers_compute_expected_values() {
        assert_eq!(
            cron_memory_limit_bytes(Some(256)),
            Some(268_435_456),
            "cron_memory_limit_bytes should convert MiB to bytes for Docker host config"
        );
        assert_eq!(
            cron_memory_limit_bytes(None),
            None,
            "cron_memory_limit_bytes should preserve the absence of a memory limit"
        );
        assert_eq!(
            cron_attempt_plan(0),
            1,
            "cron_attempt_plan should include the initial attempt when no retries are configured"
        );
        assert_eq!(
            cron_attempt_plan(2),
            3,
            "cron_attempt_plan should include retries in the total attempt count"
        );
    }
}
