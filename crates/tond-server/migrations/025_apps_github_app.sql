-- Migration 025: Link apps to a registered GitHub App (Phase 4.4)
--
-- When an app is cloned using the GitHub App installation token instead of
-- a deploy key or plain HTTPS, this foreign key identifies which registered
-- GitHub App to use.

ALTER TABLE apps ADD COLUMN github_app_id TEXT REFERENCES github_apps(id);
