// github.rs — GitHub App integration types

use serde::{Deserialize, Serialize};

// ── Phase 4.4: GitHub App Integration ────────────────────────────────────────

/// A registered GitHub App (created via the manifest flow).
/// One row per App registration; typically there is only one.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitHubApp {
    pub id: String,
    /// Human-readable name (e.g. "tond-myserver")
    pub name: String,
    /// GitHub numeric App ID
    pub app_id: i64,
    /// OAuth client_id
    pub client_id: String,
    /// OAuth client_secret (sensitive — skip in API responses)
    #[serde(skip_serializing)]
    pub client_secret: String,
    /// HMAC-SHA256 webhook secret (sensitive — skip in API responses)
    #[serde(skip_serializing)]
    pub webhook_secret: String,
    /// RSA PEM private key for JWT signing (sensitive — skip in API responses)
    #[serde(skip_serializing)]
    pub private_key: String,
    /// GitHub installation ID (set after the user installs the App on their repos)
    pub installation_id: Option<i64>,
    /// GitHub App slug (used to build the install URL)
    pub slug: String,
    pub created_at: String,
    pub updated_at: String,
}

/// API response for a GitHub App — omits all sensitive credentials.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitHubAppResponse {
    pub id: String,
    pub name: String,
    pub app_id: i64,
    pub client_id: String,
    pub slug: String,
    pub installation_id: Option<i64>,
    /// Whether the App has been installed on at least one org/repo
    pub installed: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl GitHubAppResponse {
    pub fn from_app(a: GitHubApp) -> Self {
        let installed = a.installation_id.is_some();
        Self {
            id: a.id,
            name: a.name,
            app_id: a.app_id,
            client_id: a.client_id,
            slug: a.slug,
            installation_id: a.installation_id,
            installed,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

/// Response from `GET /api/v1/github-apps/{id}/repos`
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitHubRepoResponse {
    pub id: i64,
    pub full_name: String,
    pub name: String,
    pub private: bool,
    pub clone_url: String,
    pub ssh_url: String,
    pub default_branch: String,
}

/// Response from `GET /api/v1/github-apps/{id}/repos/{owner}/{repo}/branches`
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitHubBranchResponse {
    pub name: String,
}

/// Query params for the GitHub App manifest callback.
/// GitHub appends `?code=<code>&state=<state>`, echoing the `state` we put on
/// the `github.com/settings/apps/new?state=` form action.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitHubCallbackQuery {
    pub code: Option<String>,
    /// Pending flow state created by `POST /api/v1/github-apps/manifest`
    pub state: Option<String>,
}

/// POST /api/v1/github-apps/complete — finish a manifest or install flow from the
/// dashboard page GitHub's redirect lands on.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompleteGitHubFlowRequest {
    /// "manifest" or "install"
    pub kind: String,
    pub state: Option<String>,
    pub code: Option<String>,
    pub installation_id: Option<i64>,
}

/// Query params for the GitHub App installation callback.
/// GitHub appends `?installation_id=<id>&setup_action=<action>&state=<state>`,
/// echoing the `state` we put on the installation URL.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GitHubInstallQuery {
    pub installation_id: Option<i64>,
    pub setup_action: Option<String>,
    /// Pending flow state created by `POST /api/v1/github-apps/{id}/install`
    pub state: Option<String>,
}

/// PATCH /api/v1/github-apps/{id} — update a registered app (e.g. rename or set installation_id)
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateGitHubAppRequest {
    pub name: Option<String>,
    pub installation_id: Option<i64>,
}

/// PUT /api/v1/apps/{id}/github-app — link or unlink a GitHub App from an app
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SetGitHubAppRequest {
    /// GitHub App registration ID to link. `null` = detach.
    pub github_app_id: Option<String>,
}
