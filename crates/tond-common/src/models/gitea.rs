// gitea.rs — Gitea/Forgejo access token types

use serde::{Deserialize, Serialize};

/// DB row for the `gitea_tokens` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct GiteaToken {
    pub id: String,
    pub name: String,
    /// Encrypted access token (secret_key Purpose::GiteaToken)
    #[serde(skip_serializing)]
    pub token_enc: String,
    /// Instance URL, e.g. https://codeberg.org
    pub base_url: String,
    /// Account resolved at creation time via /api/v1/user
    pub username: String,
    pub created_at: String,
}

/// API response for a stored Gitea token (the token itself is never returned).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GiteaTokenResponse {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub username: String,
    pub created_at: String,
}

impl From<GiteaToken> for GiteaTokenResponse {
    fn from(t: GiteaToken) -> Self {
        Self {
            id: t.id,
            name: t.name,
            base_url: t.base_url,
            username: t.username,
            created_at: t.created_at,
        }
    }
}

/// POST /api/gitea-tokens — store a new token
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CreateGiteaTokenRequest {
    pub name: String,
    pub token: String,
    /// Instance URL (Gitea, Forgejo, Codeberg…)
    pub base_url: String,
}

/// PUT /api/apps/{id}/gitea-token — link or unlink a Gitea token
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SetGiteaTokenRequest {
    /// Token ID to link; `null` = detach.
    pub gitea_token_id: Option<String>,
}
