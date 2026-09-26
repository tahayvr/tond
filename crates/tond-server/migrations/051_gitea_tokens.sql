-- Phase 3: Gitea/Forgejo access tokens, for commit statuses, PR preview
-- comments and cloning private repositories (like gitlab_tokens).
CREATE TABLE gitea_tokens(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  -- secret_key Purpose::GiteaToken
  token_enc TEXT NOT NULL,
  -- Instance URL, e.g. https://codeberg.org
  base_url TEXT NOT NULL,
  -- Account the token belongs to (from /api/v1/user at creation)
  username TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT(datetime('now'))
);

ALTER TABLE apps ADD COLUMN gitea_token_id TEXT REFERENCES gitea_tokens(id) ON DELETE SET NULL;
