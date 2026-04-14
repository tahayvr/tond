-- 021_indexes.sql — performance indexes for frequently queried columns
--
-- apps(status): health checker, list_apps filters, deploy loop
-- cron_jobs(next_run_at): cron scheduler polls this every 30 s
-- preview_deployments(expires_at): cleanup task and proxy poll both filter on this
--
-- Note: metrics(app_id, collected_at DESC) already exists in 015_metrics.sql.

CREATE INDEX IF NOT EXISTS idx_apps_status
    ON apps (status);

CREATE INDEX IF NOT EXISTS idx_cron_jobs_next_run_at
    ON cron_jobs (next_run_at);

CREATE INDEX IF NOT EXISTS idx_preview_deployments_expires_at
    ON preview_deployments (expires_at);
