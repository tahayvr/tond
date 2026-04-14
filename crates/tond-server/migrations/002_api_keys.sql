-- API keys for authentication
CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT
);

-- Index for fast key lookup during auth
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash);
