-- Previews no longer inherit the production environment. They get only
-- their own overrides (preview_env_vars) plus the production variables an
-- admin explicitly shares with them (preview_shared_keys, a JSON array of
-- names). The old exclusion list is meaningless under the new rule, so it is
-- renamed and reset.
ALTER TABLE apps RENAME COLUMN preview_excluded_keys TO preview_shared_keys;
UPDATE apps SET preview_shared_keys = '[]';
