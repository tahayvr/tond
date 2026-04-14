-- 022_trigger_source.sql — add trigger_source to deploy_logs and deployments
--
-- trigger_source records how a deployment was initiated:
--   "manual"  — triggered via the dashboard or API (POST /api/apps/{id}/deploy)
--   "webhook" — triggered by an incoming GitHub/GitLab/generic webhook
--   "upload"  — triggered by a tarball upload (POST /api/apps/{id}/upload)
--   "rollback"— triggered by a rollback operation
--
-- Uses DEFAULT 'manual' so existing rows keep a sensible value after migration.

ALTER TABLE deploy_logs ADD COLUMN trigger_source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE deployments ADD COLUMN trigger_source TEXT NOT NULL DEFAULT 'manual';
