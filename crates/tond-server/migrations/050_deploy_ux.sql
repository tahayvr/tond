-- Deploy UX: deployments become the source of truth for what ran and why.
--
-- deployments
--   commit_sha / commit_message / commit_author
--       the commit a git deploy built (read from the clone)
--   trigger_detail
--       who or what started it: a user's email, "api-key", "cli",
--       the webhook provider, or the deployment a rollback restored
--   rollback_of
--       for rollbacks: the deployment whose image was restored
--   phases
--       JSON array of {phase, status, started_at, finished_at} for the
--       clone / build / start / health / release steps
--   failure
--       JSON {phase, code, title, hint, detail, excerpt[]}: the classified
--       likely cause of a failed deployment (NULL unless status = 'failed')
--
-- deploy_logs.deployment_id ties every log line to its deployment so one
-- deployment's log can be shown (and streamed) on its own.

ALTER TABLE deployments ADD COLUMN commit_sha TEXT;
ALTER TABLE deployments ADD COLUMN commit_message TEXT;
ALTER TABLE deployments ADD COLUMN commit_author TEXT;
ALTER TABLE deployments ADD COLUMN trigger_detail TEXT;
ALTER TABLE deployments ADD COLUMN rollback_of TEXT;
ALTER TABLE deployments ADD COLUMN phases TEXT NOT NULL DEFAULT '[]';
ALTER TABLE deployments ADD COLUMN failure TEXT;

ALTER TABLE deploy_logs ADD COLUMN deployment_id TEXT;
CREATE INDEX idx_deploy_logs_deployment ON deploy_logs(deployment_id, created_at);
