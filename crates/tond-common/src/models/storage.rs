// storage.rs — StorageProvider model and associated types
//
// A StorageProvider is a named, reusable remote object-storage backend.
// It mirrors the `Integration` pattern from notifications.rs: a `kind`
// discriminator and a `config` JSON blob. Currently only "s3" is supported,
// but "gcs" and "azure_blob" are reserved for future use.
//
// The `secret_access_key` inside the config JSON is AES-GCM encrypted using
// the same `encrypt_password` / `decrypt_password` helpers from
// registry_credentials.rs.

use serde::{Deserialize, Serialize};

// ── Database row ──────────────────────────────────────────────────────────────

/// Raw row from the `storage_providers` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StorageProvider {
    pub id: String,
    pub name: String,
    /// "s3" — covers AWS S3, Cloudflare R2, Backblaze B2, MinIO, etc.
    /// Future: "gcs", "azure_blob"
    pub kind: String,
    /// JSON blob — parsed into `StorageProviderConfig` for business logic.
    pub config: String,
    pub created_at: String,
    pub updated_at: String,
}

// ── Typed config variants ────────────────────────────────────────────────────

/// S3-compatible config (used for AWS, R2, B2, MinIO).
/// `endpoint` is None for real AWS (uses the standard regional endpoint).
/// `secret_access_key_enc` is AES-GCM encrypted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct S3Config {
    /// Custom endpoint URL, e.g. "https://<account>.r2.cloudflarestorage.com".
    /// Leave None for real AWS S3.
    pub endpoint: Option<String>,
    /// e.g. "auto" for Cloudflare R2, "us-east-1" for AWS
    pub region: String,
    pub bucket: String,
    /// Optional key prefix, e.g. "tond-backups/"
    pub prefix: Option<String>,
    pub access_key_id: String,
    /// AES-GCM encrypted secret. Same scheme as other secrets in this codebase.
    pub secret_access_key_enc: String,
}

/// Strongly-typed config parsed from the `config` JSON column.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StorageProviderConfig {
    S3(S3Config),
}

impl StorageProviderConfig {
    /// Parse from the stored JSON blob + kind string.
    pub fn parse(kind: &str, config_json: &str) -> anyhow::Result<Self> {
        Ok(match kind {
            "s3" => StorageProviderConfig::S3(serde_json::from_str(config_json)?),
            other => anyhow::bail!("unknown storage provider kind: {}", other),
        })
    }
}

// ── API response shape ────────────────────────────────────────────────────────

/// Outbound representation of a storage provider.
/// `secret_access_key_enc` is intentionally omitted — the client must send a
/// new value to update the secret.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StorageProviderResponse {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Config returned as a typed object with the secret key stripped.
    pub config: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

// ── Request bodies ───────────────────────────────────────────────────────────

/// POST /api/v1/storage-providers
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateStorageProviderRequest {
    pub name: String,
    /// "s3" (only supported kind for now)
    pub kind: String,
    /// Provider-specific config including the plaintext `secret_access_key`
    /// (will be encrypted before storage).
    pub config: serde_json::Value,
}

/// PATCH /api/v1/storage-providers/:id — all fields optional
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateStorageProviderRequest {
    pub name: Option<String>,
    /// Partial config update. Include `secret_access_key` only if rotating.
    pub config: Option<serde_json::Value>,
}
