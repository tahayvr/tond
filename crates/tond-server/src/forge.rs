// forge.rs — commit statuses and pull-request comments on GitHub, GitLab
// and Gitea/Forgejo
//
// An app talks to its forge through whatever credential it is linked to:
// a GitHub App installation (`github_app_id`), a GitLab personal access
// token (`gitlab_token_id`) or a Gitea/Forgejo token (`gitea_token_id`).
// The repository comes from the app's `source_url`.
//
// What Tond reports:
//   tond/deploy   on the deployed commit: pending when a git deploy has
//                 checked out its commit, success when it went live,
//                 failure when it failed — "Details" opens the deployment
//                 page in the dashboard (deploy/progress.rs calls
//                 `announce_deploy`).
//   tond/preview  on a pull/merge request's head commit, plus one comment
//                 with the preview URL that later pushes update in place
//                 (webhooks.rs, deploy/core.rs).
//
// Everything here is best effort: a forge that is down or a token without
// the right scope is logged and never fails a deploy.

use anyhow::{Context, Result};
use tond_common::{App, GitHubApp, GitLabToken, GiteaToken, PreviewDeployment};

use crate::handlers::github_apps;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;

/// A commit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Success,
    Failure,
}

impl Status {
    fn github(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Success => "success",
            Status::Failure => "failure",
        }
    }

    fn gitlab(self) -> &'static str {
        match self {
            Status::Pending => "running",
            Status::Success => "success",
            Status::Failure => "failed",
        }
    }

    fn gitea(self) -> &'static str {
        self.github()
    }
}

/// Where an app's repository lives, with the credential to talk to it.
#[derive(Debug, Clone)]
pub enum Forge {
    GitHub {
        app: GitHubApp,
        owner: String,
        repo: String,
    },
    GitLab {
        token: GitLabToken,
        /// Numeric project id, or the URL-encoded "group/project" path
        project: String,
    },
    Gitea {
        token: GiteaToken,
        owner: String,
        repo: String,
    },
}

/// The repository path of `source_url` on the instance at `base`
/// ("https://git.example.com/group/sub/repo.git" → "group/sub/repo").
pub fn repo_path(source_url: &str, base: &str) -> Option<String> {
    let strip_scheme = |u: &str| {
        u.trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    let base = strip_scheme(base);
    let url = source_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    // Case-insensitive host match; the path keeps its case.
    if url.len() <= base.len() || !url[..base.len()].eq_ignore_ascii_case(&base) {
        return None;
    }
    let path = url[base.len()..].strip_prefix('/')?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (path.contains('/') && !path.starts_with('/') && !path.contains("..")).then(|| path.to_string())
}

/// "owner/repo" → ("owner", "repo") (exactly two segments).
fn owner_repo(path: &str) -> Option<(String, String)> {
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .then(|| (owner.to_string(), repo.to_string()))
}

impl Forge {
    /// The forge an app reports to, if it is linked to one and its source
    /// URL belongs to that forge.
    pub async fn for_app(db: &sqlx::SqlitePool, app: &App) -> Option<Forge> {
        let source = app.source_url.as_deref()?;
        if let Some(id) = &app.github_app_id {
            let gh: GitHubApp = sqlx::query_as("SELECT * FROM github_apps WHERE id = ?")
                .bind(id)
                .fetch_optional(db)
                .await
                .ok()??;
            let (owner, repo) = owner_repo(&repo_path(source, "https://github.com")?)?;
            return Some(Forge::GitHub {
                app: gh,
                owner,
                repo,
            });
        }
        if let Some(id) = &app.gitlab_token_id {
            let token: GitLabToken = sqlx::query_as("SELECT * FROM gitlab_tokens WHERE id = ?")
                .bind(id)
                .fetch_optional(db)
                .await
                .ok()??;
            let path = repo_path(source, &token.gitlab_url)?;
            return Some(Forge::GitLab {
                token,
                project: urlencoding::encode(&path).into_owned(),
            });
        }
        if let Some(id) = &app.gitea_token_id {
            let token: GiteaToken = sqlx::query_as("SELECT * FROM gitea_tokens WHERE id = ?")
                .bind(id)
                .fetch_optional(db)
                .await
                .ok()??;
            let (owner, repo) = owner_repo(&repo_path(source, &token.base_url)?)?;
            return Some(Forge::Gitea { token, owner, repo });
        }
        None
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Forge::GitHub { .. } => "github",
            Forge::GitLab { .. } => "gitlab",
            Forge::Gitea { .. } => "gitea",
        }
    }

    /// Set a commit status.
    pub async fn post_status(
        &self,
        sha: &str,
        status: Status,
        description: &str,
        target_url: Option<&str>,
        context: &str,
    ) -> Result<()> {
        // Forges cap descriptions (GitHub: 140 characters).
        let description: String = description.chars().take(140).collect();
        match self {
            Forge::GitHub { app, owner, repo } => {
                github_apps::post_commit_status(
                    app,
                    owner,
                    repo,
                    sha,
                    status.github(),
                    &description,
                    target_url,
                    context,
                )
                .await
            }
            Forge::GitLab { token, project } => {
                let mut body = serde_json::json!({
                    "state": status.gitlab(),
                    "description": description,
                    "name": context,
                });
                if let Some(u) = target_url {
                    body["target_url"] = u.into();
                }
                let url = format!(
                    "{}/api/v4/projects/{project}/statuses/{sha}",
                    token.gitlab_url.trim_end_matches('/')
                );
                send(gitlab(token, reqwest::Client::new().post(&url))?.json(&body)).await?;
                Ok(())
            }
            Forge::Gitea { token, owner, repo } => {
                let mut body = serde_json::json!({
                    "state": status.gitea(),
                    "description": description,
                    "context": context,
                });
                if let Some(u) = target_url {
                    body["target_url"] = u.into();
                }
                let url = format!(
                    "{}/api/v1/repos/{owner}/{repo}/statuses/{sha}",
                    token.base_url.trim_end_matches('/')
                );
                send(gitea(token, reqwest::Client::new().post(&url))?.json(&body)).await?;
                Ok(())
            }
        }
    }

    /// Create the preview comment on pull/merge request `number`, or update
    /// the existing one. Returns the comment id to keep.
    pub async fn upsert_comment(
        &self,
        number: i64,
        body: &str,
        existing: Option<i64>,
    ) -> Result<i64> {
        let payload = serde_json::json!({ "body": body });
        let client = reqwest::Client::new();
        let json = match self {
            Forge::GitHub { app, owner, repo } => {
                return github_apps::post_or_update_pr_comment(
                    app, owner, repo, number, body, existing,
                )
                .await;
            }
            Forge::GitLab { token, project } => {
                let base = format!(
                    "{}/api/v4/projects/{project}/merge_requests/{number}/notes",
                    token.gitlab_url.trim_end_matches('/')
                );
                let req = match existing {
                    Some(id) => client.put(format!("{base}/{id}")),
                    None => client.post(base),
                };
                send(gitlab(token, req)?.json(&payload)).await?
            }
            Forge::Gitea { token, owner, repo } => {
                let base = format!(
                    "{}/api/v1/repos/{owner}/{repo}/issues",
                    token.base_url.trim_end_matches('/')
                );
                let req = match existing {
                    Some(id) => client.patch(format!("{base}/comments/{id}")),
                    None => client.post(format!("{base}/{number}/comments")),
                };
                send(gitea(token, req)?.json(&payload)).await?
            }
        };
        json["id"]
            .as_i64()
            .context("the forge's reply has no comment id")
    }
}

fn gitlab(token: &GitLabToken, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
    let pat = secret_key::decrypt(Purpose::GitlabToken, &token.token_enc)?;
    Ok(req.header("PRIVATE-TOKEN", pat))
}

fn gitea(token: &GiteaToken, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
    let pat = secret_key::decrypt(Purpose::GiteaToken, &token.token_enc)?;
    Ok(req.header("Authorization", format!("token {pat}")))
}

/// Send, turning an error status into an error that quotes the reply.
async fn send(req: reqwest::RequestBuilder) -> Result<serde_json::Value> {
    let resp = req
        .header("User-Agent", "tond-server")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

// ── Deploys ────────────────────────────────────────────────────────────────

/// The dashboard page of a deployment, when the dashboard has a domain.
pub fn deployment_url(dashboard_domain: &str, app_id: &str, deployment_id: &str) -> Option<String> {
    let domain = dashboard_domain.trim();
    (!domain.is_empty())
        .then(|| format!("https://{domain}/apps/{app_id}/deployments/{deployment_id}"))
}

/// What the tond/deploy status says.
pub fn deploy_description(status: Status, failure: Option<&str>) -> String {
    match (status, failure) {
        (Status::Pending, _) => "Deploying on Tond…".to_string(),
        (Status::Success, _) => "Deployed on Tond".to_string(),
        (Status::Failure, Some(why)) => format!("Deploy failed: {why}"),
        (Status::Failure, None) => "Deploy failed".to_string(),
    }
}

/// The commit status for a deployment status.
pub fn status_for(deployment_status: &str) -> Status {
    match deployment_status {
        "succeeded" => Status::Success,
        "failed" | "superseded" | "cancelled" => Status::Failure,
        _ => Status::Pending,
    }
}

/// One report at a time per deployment, each reading the row's current
/// state: a slow "pending" request can never land after "success".
fn report_lock(deployment_id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};
    static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut map = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, l| Arc::strong_count(l) > 1);
    map.entry(deployment_id.to_string()).or_default().clone()
}

/// Report a deployment's current state on its commit (in the background).
/// Does nothing for deployments without a commit or apps without a forge.
pub fn announce_deploy(state: &AppState, app_id: &str, deployment_id: &str) {
    let state = state.clone();
    let (app_id, deployment_id) = (app_id.to_string(), deployment_id.to_string());
    tokio::spawn(async move {
        let lock = report_lock(&deployment_id);
        let _guard = lock.lock().await;
        if let Err(e) = post_deploy_status(&state, &app_id, &deployment_id).await {
            tracing::warn!("[forge] couldn't report deployment {deployment_id}: {e:#}");
        }
    });
}

async fn post_deploy_status(state: &AppState, app_id: &str, deployment_id: &str) -> Result<()> {
    let row: Option<(Option<String>, Option<String>, String, String)> = sqlx::query_as(
        "SELECT commit_sha, json_extract(failure, '$.title'), kind, status
         FROM deployments WHERE id = ?",
    )
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((Some(sha), failure, kind, deployment_status)) = row else {
        return Ok(());
    };
    if kind != "deploy" {
        return Ok(()); // rollbacks re-run an old image; the commit isn't new
    }
    let status = status_for(&deployment_status);
    let app: Option<App> = sqlx::query_as("SELECT * FROM apps WHERE id = ?")
        .bind(app_id)
        .fetch_optional(&state.db)
        .await?;
    let Some(app) = app else { return Ok(()) };
    let Some(forge) = Forge::for_app(&state.db, &app).await else {
        return Ok(());
    };
    let domain = state.settings.read().await.dashboard_domain.clone();
    forge
        .post_status(
            &sha,
            status,
            &deploy_description(status, failure.as_deref()),
            deployment_url(&domain, app_id, deployment_id).as_deref(),
            "tond/deploy",
        )
        .await
}

// ── Previews ───────────────────────────────────────────────────────────────

/// The pull-request comment for a live preview.
pub fn preview_comment(url: &str, branch: Option<&str>, sha: &str) -> String {
    format!(
        "### Tond preview\n\n\
         | | |\n\
         |---|---|\n\
         | **Status** | live |\n\
         | **URL** | [{url}]({url}) |\n\
         | **Branch** | `{}` |\n\
         | **Commit** | `{}` |\n\n\
         The preview is removed when the pull request is closed or merged.",
        branch.unwrap_or("—"),
        &sha[..sha.len().min(7)],
    )
}

/// The forge for a preview: the app's, with GitLab's numeric project id from
/// the merge request event when known.
async fn preview_forge(state: &AppState, app: &App, preview: &PreviewDeployment) -> Option<Forge> {
    let mut forge = Forge::for_app(&state.db, app).await?;
    if let (Forge::GitLab { project, .. }, Some(id)) = (&mut forge, preview.gitlab_project_id) {
        *project = id.to_string();
    }
    Some(forge)
}

/// A preview started building: a pending tond/preview status.
pub async fn announce_preview_building(state: &AppState, app: &App, preview: &PreviewDeployment) {
    let (Some(sha), Some(forge)) = (&preview.head_sha, preview_forge(state, app, preview).await)
    else {
        return;
    };
    if let Err(e) = forge
        .post_status(
            sha,
            Status::Pending,
            "Building the preview…",
            None,
            "tond/preview",
        )
        .await
    {
        tracing::warn!(
            "[forge] {}: pending preview status failed: {e:#}",
            forge.kind()
        );
    }
}

/// A preview is live: success status and the comment with its URL.
pub async fn announce_preview_live(state: &AppState, app: &App, preview: &PreviewDeployment) {
    let (Some(sha), Some(forge)) = (&preview.head_sha, preview_forge(state, app, preview).await)
    else {
        return;
    };
    let url = preview.subdomain.as_ref().map(|s| format!("https://{s}"));
    if let Err(e) = forge
        .post_status(
            sha,
            Status::Success,
            "The preview is live",
            url.as_deref(),
            "tond/preview",
        )
        .await
    {
        tracing::warn!("[forge] {}: preview status failed: {e:#}", forge.kind());
    }
    let (Some(number), Some(url)) = (preview.pr_number, url) else {
        return;
    };
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT comment_id FROM preview_deployments WHERE id = ?")
            .bind(&preview.id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .flatten();
    let body = preview_comment(&url, preview.head_ref.as_deref(), sha);
    match forge.upsert_comment(number, &body, existing).await {
        Ok(id) => {
            let _ = sqlx::query("UPDATE preview_deployments SET comment_id = ? WHERE id = ?")
                .bind(id)
                .bind(&preview.id)
                .execute(&state.db)
                .await;
        }
        Err(e) => tracing::warn!("[forge] {}: preview comment failed: {e:#}", forge.kind()),
    }
}

/// A preview failed to deploy: failure status.
pub async fn announce_preview_failed(
    state: &AppState,
    app: &App,
    preview: &PreviewDeployment,
    error: &str,
) {
    let (Some(sha), Some(forge)) = (&preview.head_sha, preview_forge(state, app, preview).await)
    else {
        return;
    };
    if let Err(e) = forge
        .post_status(
            sha,
            Status::Failure,
            &format!("Preview failed: {error}"),
            None,
            "tond/preview",
        )
        .await
    {
        tracing::warn!("[forge] {}: failure status failed: {e:#}", forge.kind());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_paths_come_from_the_source_url_on_the_forge_host() {
        assert_eq!(
            repo_path("https://github.com/tond-dev/app.git", "https://github.com").as_deref(),
            Some("tond-dev/app")
        );
        assert_eq!(
            repo_path(
                "https://GitLab.example.com/group/sub/proj/",
                "https://gitlab.example.com/"
            )
            .as_deref(),
            Some("group/sub/proj")
        );
        assert_eq!(
            repo_path("https://codeberg.org/me/site", "https://codeberg.org").as_deref(),
            Some("me/site")
        );
        assert_eq!(
            repo_path("https://evil.com/github.com/a/b", "https://github.com"),
            None
        );
        assert_eq!(
            repo_path("https://github.com.evil.com/a/b", "https://github.com"),
            None
        );
        assert_eq!(
            repo_path("https://github.com/onlyowner", "https://github.com"),
            None
        );
        assert_eq!(
            repo_path("https://github.com/a/../b", "https://github.com"),
            None
        );
        assert_eq!(
            owner_repo("group/sub/proj"),
            None,
            "GitHub and Gitea repos are owner/repo"
        );
    }

    #[test]
    fn statuses_map_to_each_forges_vocabulary() {
        assert_eq!(Status::Pending.github(), "pending");
        assert_eq!(Status::Pending.gitlab(), "running");
        assert_eq!(Status::Failure.gitlab(), "failed");
        assert_eq!(Status::Failure.gitea(), "failure");
    }

    #[test]
    fn deploy_statuses_link_the_deployment_page() {
        assert_eq!(
            deployment_url("tond.example.com", "a1", "d1").as_deref(),
            Some("https://tond.example.com/apps/a1/deployments/d1")
        );
        assert_eq!(deployment_url(" ", "a1", "d1"), None);
        assert_eq!(
            deploy_description(Status::Failure, Some("The build failed")),
            "Deploy failed: The build failed"
        );
        assert_eq!(
            deploy_description(Status::Success, None),
            "Deployed on Tond"
        );
    }

    #[test]
    fn the_preview_comment_carries_the_url_branch_and_short_commit() {
        let body = preview_comment("https://x.preview.test", Some("feat"), "0123456789abcdef");
        assert!(body.contains("[https://x.preview.test](https://x.preview.test)"));
        assert!(body.contains("`feat`") && body.contains("`0123456`"));
    }

    #[test]
    fn deployment_states_map_to_commit_statuses() {
        assert_eq!(status_for("queued"), Status::Pending);
        assert_eq!(status_for("running"), Status::Pending);
        assert_eq!(status_for("succeeded"), Status::Success);
        assert_eq!(status_for("failed"), Status::Failure);
        assert_eq!(status_for("superseded"), Status::Failure);
    }

    #[tokio::test]
    async fn apps_report_to_the_forge_they_are_linked_to() {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        sqlx::query(
            "INSERT INTO gitea_tokens (id, name, token_enc, base_url, username)
             VALUES ('g1', 'codeberg', 'x', 'https://codeberg.org', 'me')",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let mut app = App::new(
            "site".to_string(),
            "git".to_string(),
            Some("https://codeberg.org/me/site.git".to_string()),
            Some("main".to_string()),
            3000,
        );
        assert!(Forge::for_app(&db, &app).await.is_none(), "not linked");
        app.gitea_token_id = Some("g1".to_string());
        match Forge::for_app(&db, &app).await {
            Some(Forge::Gitea { owner, repo, .. }) => {
                assert_eq!((owner.as_str(), repo.as_str()), ("me", "site"))
            }
            other => panic!("expected Gitea, got {other:?}"),
        }
        app.source_url = Some("https://github.com/me/site.git".to_string());
        assert!(
            Forge::for_app(&db, &app).await.is_none(),
            "a token for another host never receives this repository's statuses"
        );
    }
}
