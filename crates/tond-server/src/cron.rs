// cron.rs — Background cron job scheduler (Phase 6.4 + 6.5)
//
// Exports:
//   run_cron_scheduler(state)        — long-running Tokio task, wakes every 30 s,
//                                      finds all enabled jobs whose next_run_at <= now,
//                                      and runs each one's Docker container.
//
//   run_cron_job(state, job, attempt) — runs a single cron job container (with an
//                                       internal retry loop), records each run in
//                                       cron_job_runs, updates the job's
//                                       last_run_at / next_run_at / last_run_status.
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
//   • Hardening: the same no-new-privileges / capability / pids / memory / log
//     rotation policy as app containers (`docker::harden_host_config`).
//   • Retry logic: on non-zero exit, re-queues the job up to max_retries times with
//     retry_delay_seconds delay. Attempt number is tracked on the run row.
//   • last_run_status: updated on the cron_jobs row at the end of each run (final attempt).

use anyhow::Result;
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;
use tond_common::{CronJob, CronVolume};
use uuid::Uuid;

use crate::{alerts, docker, state::AppState, volumes};

fn should_run_due_job(job: &CronJob) -> bool {
    !(job.status == "running" && job.concurrency_policy == "skip")
}

fn cron_env_vec(env_json: Option<&str>) -> Vec<String> {
    env_json
        .and_then(|s| serde_json::from_str::<std::collections::HashMap<String, String>>(s).ok())
        .map(|m| m.into_iter().map(|(k, v)| format!("{}={}", k, v)).collect())
        .unwrap_or_default()
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

pub async fn run_cron_scheduler(state: AppState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        let now = Utc::now().to_rfc3339();

        // Jobs that should fire now.
        // For 'skip' policy we only pick up jobs that are NOT running.
        // For 'allow' policy we pick them up regardless.
        // We fetch all due enabled jobs and let the policy check happen here.
        let due_jobs: Vec<CronJob> = match sqlx::query_as(
            "SELECT * FROM cron_jobs
             WHERE enabled = 1
               AND next_run_at IS NOT NULL
               AND next_run_at <= ?",
        )
        .bind(&now)
        .fetch_all(&state.db)
        .await
        {
            Ok(jobs) => jobs,
            Err(e) => {
                tracing::warn!("Cron scheduler DB query failed: {}", e);
                continue;
            }
        };

        for job in due_jobs {
            // Concurrency policy: 'skip' won't start a new container if one is running.
            if !should_run_due_job(&job) {
                continue;
            }

            let state_clone = state.clone();
            let job_clone = job.clone();
            tokio::spawn(async move {
                run_cron_job(&state_clone, &job_clone, 1).await;
            });
        }
    }
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
            // Success — compute next run and persist final state
            let next_run = compute_next_run(&job.schedule, job.timezone.as_deref())
                .unwrap_or_else(|_| finished_at.clone());

            let _ = sqlx::query(
                "UPDATE cron_jobs
                 SET status = 'idle', last_run_at = ?, next_run_at = ?,
                     last_run_status = 'success',
                     updated_at = datetime('now')
                 WHERE id = ?",
            )
            .bind(&finished_at)
            .bind(&next_run)
            .bind(&job.id)
            .execute(&state.db)
            .await;

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
            let _ = sqlx::query(
                "UPDATE cron_jobs
                 SET status = 'failed', last_run_at = ?, updated_at = datetime('now')
                 WHERE id = ?",
            )
            .bind(&finished_at)
            .bind(&job.id)
            .execute(&state.db)
            .await;

            tokio::time::sleep(delay).await;
            continue;
        }

        // All attempts exhausted — final failure
        let next_run = compute_next_run(&job.schedule, job.timezone.as_deref())
            .unwrap_or_else(|_| finished_at.clone());

        let _ = sqlx::query(
            "UPDATE cron_jobs
             SET status = 'failed', last_run_at = ?, next_run_at = ?,
                 last_run_status = 'failed',
                 updated_at = datetime('now')
             WHERE id = ?",
        )
        .bind(&finished_at)
        .bind(&next_run)
        .bind(&job.id)
        .execute(&state.db)
        .await;

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

    // Build env vec from the stored JSON object string, e.g. {"KEY":"val"} → ["KEY=val"]
    let env_vec = cron_env_vec(job.env.as_deref());

    let cmd_vec: Option<Vec<&str>> = cmd.map(|c| c.iter().map(|s| s.as_str()).collect());

    // Parse volume mounts (confined to <data>/volumes, see volumes.rs)
    let binds = cron_binds(&volumes::data_dir(), &job.id, job.volumes.as_deref())?;

    // Resource limits
    let memory = cron_memory_limit_bytes(job.memory_mb);
    let cpu_shares: Option<i64> = job.cpu_shares;

    let mut host_config = HostConfig {
        auto_remove: Some(false), // we remove manually after collecting logs
        memory,
        cpu_shares,
        binds,
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
            Some(r#"{"RUST_LOG":"info","MODE":"prod"}"#.to_string()),
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

        let env = cron_env_vec(job.env.as_deref());
        assert!(
            env.contains(&"RUST_LOG=info".to_string()) && env.contains(&"MODE=prod".to_string()),
            "cron_env_vec should convert the stored JSON object into KEY=value strings"
        );

        assert_eq!(
            cron_env_vec(Some("not-json")),
            Vec::<String>::new(),
            "cron_env_vec should fall back to an empty environment on invalid JSON"
        );

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
