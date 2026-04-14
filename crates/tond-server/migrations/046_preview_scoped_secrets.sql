-- Phase 4.6: Scoped Secrets for PR preview deployments
-- preview_excluded_keys: JSON array of env var keys to strip from preview containers
-- preview_env_vars: JSON object of key-value pairs that override prod env vars for previews only

ALTER TABLE apps ADD COLUMN preview_excluded_keys TEXT NOT NULL DEFAULT '[]';
ALTER TABLE apps ADD COLUMN preview_env_vars TEXT NOT NULL DEFAULT '{}';
