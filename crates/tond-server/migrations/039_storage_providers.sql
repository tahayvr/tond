-- 039_storage_providers.sql
-- Creates the `storage_providers` table for named remote object storage backends.
-- Each row stores a provider kind and a JSON config blob (mirrors the `integrations`
-- pattern). Currently only "s3" is supported; "gcs" and "azure_blob" are reserved for
-- future use. The `secret_access_key` inside the JSON config is AES-GCM encrypted.

CREATE TABLE IF NOT EXISTS storage_providers (
    id         TEXT NOT NULL PRIMARY KEY,
    name       TEXT NOT NULL,
    -- "s3" (covers AWS S3, Cloudflare R2, Backblaze B2, MinIO, etc.)
    kind       TEXT NOT NULL DEFAULT 's3',
    -- Provider-specific JSON config (see storage.rs for shape)
    config     TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
