-- Migration 026: gitlab_tokens table
--
-- Stores GitLab Personal Access Tokens (PATs) for private repo access.
-- The PAT value is stored AES-256-GCM encrypted (same scheme as registry_credentials).
-- On deletion the FK in apps will SET NULL via migration 027.

CREATE TABLE IF NOT EXISTS gitlab_tokens (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    token_enc   TEXT NOT NULL,
    gitlab_url  TEXT NOT NULL DEFAULT 'https://gitlab.com',
    username    TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
