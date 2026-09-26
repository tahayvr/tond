// registry.rs — Docker registry credential types

use serde::{Deserialize, Serialize};

// ── Registry Credentials (Phase 4.3) ──────────────────────────────────────

/// A stored registry credential for pulling private Docker images.
///
/// The password is stored AES-256-GCM encrypted in the DB and never returned
/// via the API — only `id`, `name`, `server`, `username`, and timestamps are
/// exposed.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RegistryCredential {
    pub id: String,
    pub name: String,
    pub server: String,
    pub username: String,
    /// base64(nonce || AES-256-GCM ciphertext) — never exposed via API
    #[serde(skip_serializing)]
    pub password_enc: String,
    pub created_at: String,
    pub updated_at: String,
}

/// API view — password never included.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RegistryCredentialResponse {
    pub id: String,
    pub name: String,
    pub server: String,
    pub username: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<RegistryCredential> for RegistryCredentialResponse {
    fn from(c: RegistryCredential) -> Self {
        Self {
            id: c.id,
            name: c.name,
            server: c.server,
            username: c.username,
            created_at: c.created_at,
            updated_at: c.updated_at,
        }
    }
}

/// POST /api/v1/registry-credentials request body
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateRegistryCredentialRequest {
    pub name: String,
    pub server: String,
    pub username: String,
    pub password: String,
}

/// PUT /api/v1/apps/{id}/registry-credential request body
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SetRegistryCredentialRequest {
    /// Pass `null` to detach
    pub registry_credential_id: Option<String>,
}
