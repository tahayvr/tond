-- Migration 023: Registry credentials for private Docker registries
--
-- registry_credentials: stores named username/password pairs per registry.
-- The password is stored encrypted at rest (AES-256-GCM via the server's
-- TOND_SECRET_KEY env var); the API never returns it in responses.
--
-- apps gains a nullable FK so one credential can be linked to an app.
-- At deploy time, if registry_credential_id is set and source_type = 'image',
-- the credential is fetched and passed to Bollard's create_image as
-- X-Registry-Auth.

CREATE TABLE IF NOT EXISTS registry_credentials (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL,
    server       TEXT NOT NULL,
    username     TEXT NOT NULL,
    -- AES-256-GCM encrypted: base64(nonce || ciphertext)
    password_enc TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

ALTER TABLE apps ADD COLUMN registry_credential_id TEXT REFERENCES registry_credentials(id) ON DELETE SET NULL;
