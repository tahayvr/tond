-- 043_preview_pr_fields.sql
-- PR Preview Deployments (Phase 4.6).
-- Adds metadata columns so a preview_deployment row can be linked back to the
-- GitHub pull request that triggered it. All columns are nullable so existing
-- manually-created and project-level previews are unaffected.

ALTER TABLE preview_deployments ADD COLUMN pr_number INTEGER;
ALTER TABLE preview_deployments ADD COLUMN head_sha  TEXT;
ALTER TABLE preview_deployments ADD COLUMN head_ref  TEXT;
ALTER TABLE preview_deployments ADD COLUMN pr_url    TEXT;
