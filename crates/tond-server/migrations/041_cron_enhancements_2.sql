-- Phase 6.5 Cron Job Enhancements (batch 2)
--
-- Adds the remaining features from the 6.5 roadmap to cron_jobs/cron_job_runs.

-- Denormalised last-run outcome so the list view can show success/failure
-- without a subquery. Updated at the end of every run alongside status.
-- Values: 'success' | 'failed' | NULL (never run).
ALTER TABLE cron_jobs ADD COLUMN last_run_status TEXT;

-- Concurrency policy: what to do when a new fire time arrives while the
-- previous container is still running.
--   'skip'  (default) — skip the firing; preserves current behaviour.
--   'allow' — start a new container anyway (parallel runs permitted).
ALTER TABLE cron_jobs ADD COLUMN concurrency_policy TEXT NOT NULL DEFAULT 'skip';

-- Resource limits for the cron container. NULL = no limit.
--   memory_mb : host memory limit in MiB (passed as bytes to Docker).
--   cpu_shares: Docker CPU shares (relative weight, default 1024).
ALTER TABLE cron_jobs ADD COLUMN memory_mb INTEGER;
ALTER TABLE cron_jobs ADD COLUMN cpu_shares INTEGER;

-- IANA timezone name (e.g. "Europe/Berlin") for schedule evaluation.
-- NULL means UTC (previous behaviour).
ALTER TABLE cron_jobs ADD COLUMN timezone TEXT;

-- Automatic retry on non-zero exit.
--   max_retries         : how many times to re-run on failure (0 = no retries).
--   retry_delay_seconds : seconds to wait between retries.
ALTER TABLE cron_jobs ADD COLUMN max_retries INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cron_jobs ADD COLUMN retry_delay_seconds INTEGER NOT NULL DEFAULT 60;

-- Volume/bind-mount definitions stored as a JSON array of objects:
--   [{"host_path":"/opt/tond/data/volumes/job","container_path":"/data"}]
-- NULL means no mounts.
ALTER TABLE cron_jobs ADD COLUMN volumes TEXT;

-- Retry attempt number on each run row (1 = first attempt).
ALTER TABLE cron_job_runs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
