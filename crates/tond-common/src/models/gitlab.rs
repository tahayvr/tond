// gitlab.rs — GitLab PAT integration types

use serde::{Deserialize, Serialize};

// ── GitLab PAT Integration (Phase 4.5) ──────────────────────────────────────

/// DB row for the `gitlab_tokens` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitLabToken {
    pub id: String,
    pub name: String,
    /// AES-256-GCM encrypted PAT value, base64(nonce || ciphertext)
    #[serde(skip_serializing)]
    pub token_enc: String,
    /// GitLab instance URL (default: https://gitlab.com)
    pub gitlab_url: String,
    /// GitLab username resolved at creation time via /api/v4/user
    pub username: String,
    pub created_at: String,
}

/// API response for a stored GitLab token (PAT value never returned).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitLabTokenResponse {
    pub id: String,
    pub name: String,
    pub gitlab_url: String,
    pub username: String,
    pub created_at: String,
}

impl From<GitLabToken> for GitLabTokenResponse {
    fn from(t: GitLabToken) -> Self {
        Self {
            id: t.id,
            name: t.name,
            gitlab_url: t.gitlab_url,
            username: t.username,
            created_at: t.created_at,
        }
    }
}

/// POST /api/v1/gitlab-tokens — store a new PAT
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateGitLabTokenRequest {
    pub name: String,
    pub token: String,
    #[serde(default = "default_gitlab_url")]
    pub gitlab_url: String,
}

fn default_gitlab_url() -> String {
    "https://gitlab.com".to_string()
}

/// PUT /api/v1/apps/{id}/gitlab-token — link or unlink a GitLab token from an app
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SetGitLabTokenRequest {
    /// GitLab token ID to link. `null` = detach.
    pub gitlab_token_id: Option<String>,
}

/// Response for `GET /api/v1/gitlab-tokens/{id}/repos` — a single GitLab project entry.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitLabRepoResponse {
    pub id: i64,
    pub path_with_namespace: String,
    pub name: String,
    pub visibility: String,
    pub http_url_to_repo: String,
    pub ssh_url_to_repo: String,
    pub default_branch: Option<String>,
}

/// Response for `GET /api/v1/gitlab-tokens/{id}/repos/{project_id}/branches` — one branch.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitLabBranchResponse {
    pub name: String,
}
