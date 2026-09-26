-- Indexes for the janitor (retention deletes), the log stream catch-up
-- queries and session expiry. Each retention DELETE walks one of these
-- instead of scanning the whole table.
CREATE INDEX idx_deploy_logs_created_at ON deploy_logs(created_at);
CREATE INDEX idx_deploy_logs_app_created ON deploy_logs(app_id, created_at);
-- Superseded by idx_deploy_logs_app_created (same leading column).
DROP INDEX idx_deploy_logs_app_id;

CREATE INDEX idx_cron_job_runs_job_started ON cron_job_runs(cron_job_id, started_at);
-- Superseded by idx_cron_job_runs_job_started.
DROP INDEX idx_cron_job_runs_job_id;
-- Startup reconciliation looks for runs left 'running'.
CREATE INDEX idx_cron_job_runs_running ON cron_job_runs(status) WHERE status = 'running';

CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX idx_metrics_collected_at ON metrics(collected_at);
