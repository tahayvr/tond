-- 020_cron_jobs.sql — Cron Jobs (Phase 6.4)
--
-- cron_jobs: one row per scheduled job definition.
-- cron_job_runs: execution history for each cron job.

CREATE TABLE IF NOT EXISTS cron_jobs (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL UNIQUE,
    schedule    TEXT NOT NULL,          -- cron expression, e.g. "0 * * * *"
    image       TEXT NOT NULL,          -- Docker image to run
    command     TEXT,                   -- optional override command (JSON array or plain string)
    project_id  TEXT,                   -- optional project association (FK to projects.id)
    enabled     INTEGER NOT NULL DEFAULT 1,  -- 1 = enabled, 0 = paused
    last_run_at TEXT,                   -- ISO-8601 of last execution
    next_run_at TEXT,                   -- ISO-8601 of next scheduled execution
    status      TEXT NOT NULL DEFAULT 'idle',  -- "idle" | "running" | "failed"
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS cron_job_runs (
    id          TEXT PRIMARY KEY NOT NULL,
    cron_job_id TEXT NOT NULL REFERENCES cron_jobs(id) ON DELETE CASCADE,
    status      TEXT NOT NULL,          -- "running" | "success" | "failed"
    exit_code   INTEGER,
    output      TEXT,                   -- last N lines of container stdout+stderr
    started_at  TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_cron_job_runs_job_id ON cron_job_runs(cron_job_id);
