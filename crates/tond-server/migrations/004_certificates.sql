-- Certificate tracking for ACME/TLS
CREATE TABLE IF NOT EXISTS certificates (
    id          TEXT PRIMARY KEY,
    domain      TEXT NOT NULL UNIQUE,
    -- "acme" | "self_signed"
    cert_type   TEXT NOT NULL DEFAULT 'acme',
    -- Filesystem paths to PEM files
    cert_path   TEXT NOT NULL,
    key_path    TEXT NOT NULL,
    -- Expiry tracking for renewal
    issued_at   TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    -- "active" | "pending" | "expired" | "failed"
    status      TEXT NOT NULL DEFAULT 'pending',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
