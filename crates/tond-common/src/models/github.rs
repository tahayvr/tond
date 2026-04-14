// github.rs — GitHub App integration types

use serde::{Deserialize, Serialize};

// ── Phase 4.4: GitHub App Integration ────────────────────────────────────────

/// A registered GitHub App (created via the manifest flow).
/// One row per App registration; typically there is only one.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// UUID generated at manifest-creation time; echoed back by GitHub in the
    /// setup_url redirect so we can route installation_id to the exact row.
    pub manifest_state: Option<String>,
}

/// API response for a GitHub App — omits all sensitive credentials.
#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubAppResponse {
    pub id: String,
    pub name: String,
    pub app_id: i64,
    pub client_id: String,
    pub slug: String,
    pub installation_id: Option<i64>,
    /// Whether the App has been installed on at least one org/repo
    pub installed: bool,
    /// URL the user visits to install the App on their repos
    pub install_url: String,
    pub created_at: String,
    pub updated_at: String,
}

impl GitHubAppResponse {
    pub fn from_app(a: GitHubApp) -> Self {
        let install_url = format!("https://github.com/apps/{}/installations/new", a.slug);
        let installed = a.installation_id.is_some();
        Self {
            id: a.id,
            name: a.name,
            app_id: a.app_id,
            client_id: a.client_id,
            slug: a.slug,
            installation_id: a.installation_id,
            installed,
            install_url,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

/// Response from `GET /api/github-apps/{id}/repos`
#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubRepoResponse {
    pub id: i64,
    pub full_name: String,
    pub name: String,
    pub private: bool,
    pub clone_url: String,
    pub ssh_url: String,
    pub default_branch: String,
}

/// Response from `GET /api/github-apps/{id}/repos/{owner}/{repo}/branches`
#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubBranchResponse {
    pub name: String,
}

/// Query params for the GitHub App manifest callback
/// GitHub appends `?code=<code>` and preserves any extra params we put in
/// `redirect_url`, so we embed `state=<manifest_state>` there too.
#[derive(Debug, Deserialize)]
pub struct GitHubCallbackQuery {
    pub code: String,
    /// Our manifest_state UUID, echoed back from the redirect_url we supplied
    pub state: Option<String>,
}

/// Query params for the GitHub App installation callback
/// GitHub appends `?installation_id=<id>&setup_action=<action>` and preserves
/// any extra params we put in `setup_url`, so we embed `state=<manifest_state>`.
#[derive(Debug, Deserialize)]
pub struct GitHubInstallQuery {
    pub installation_id: Option<i64>,
    pub setup_action: Option<String>,
    /// Our manifest_state UUID, echoed back from the setup_url we supplied
    pub state: Option<String>,
}

/// PATCH /api/github-apps/{id} — update a registered app (e.g. rename or set installation_id)
#[derive(Debug, Deserialize)]
pub struct UpdateGitHubAppRequest {
    pub name: Option<String>,
    pub installation_id: Option<i64>,
}

/// PUT /api/apps/{id}/github-app — link or unlink a GitHub App from an app
#[derive(Debug, Deserialize)]
pub struct SetGitHubAppRequest {
    /// GitHub App registration ID to link. `null` = detach.
    pub github_app_id: Option<String>,
}
