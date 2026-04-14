-- Migration 024: GitHub App registrations (Phase 4.4)
--
-- Stores the credentials returned by GitHub after the manifest-flow App
-- creation, plus the installation_id written back when the user installs
-- the App on their repos.

CREATE TABLE IF NOT EXISTS github_apps (
    id              TEXT PRIMARY KEY,
    -- Human-readable name chosen during manifest creation (e.g. "tond-myserver")
    name            TEXT NOT NULL,
    -- GitHub numeric App ID (returned by the manifest callback)
    app_id          INTEGER NOT NULL,
    -- OAuth client_id (returned by the manifest callback)
    client_id       TEXT NOT NULL,
    -- OAuth client_secret (returned by the manifest callback, store encrypted in future)
    client_secret   TEXT NOT NULL,
    -- HMAC-SHA256 secret for validating webhook payloads from this App
    webhook_secret  TEXT NOT NULL,
    -- RSA PEM private key (returned by the manifest callback) — used to sign JWTs
    private_key     TEXT NOT NULL,
    -- GitHub installation ID (written once the user installs the App on their org/repos)
    installation_id INTEGER,
    -- GitHub App slug (e.g. "tond-myserver") — used to build the install URL
    slug            TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
