-- Migration 027: add gitlab_token_id to apps
--
-- Links an app to a stored GitLab PAT for authenticated git clones.
-- ON DELETE SET NULL means deleting a token automatically detaches it from all apps.

ALTER TABLE apps ADD COLUMN gitlab_token_id TEXT REFERENCES gitlab_tokens(id) ON DELETE SET NULL;
