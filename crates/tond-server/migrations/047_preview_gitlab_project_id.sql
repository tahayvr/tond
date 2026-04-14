-- 047_preview_gitlab_project_id.sql
-- Store the GitLab project ID alongside preview deployments so that
-- deploy_preview() can post a "success" commit status to GitLab without
-- needing to re-parse it from the webhook payload.

ALTER TABLE preview_deployments
    ADD COLUMN gitlab_project_id INTEGER;
