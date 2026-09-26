// deployments.rs — deployment history and preview deployment types

use serde::{Deserialize, Serialize};

// ── Deployment (deployment history entry) ──

/// A single deployment attempt for an app. One row per deploy invocation.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Deployment {
    pub id: String,
    pub app_id: String,
    /// Image this deployment ran: `tond-<app>:<deployment-id-prefix>` (compose
    /// apps record a `tond-compose-<app>` sentinel)
    pub image_tag: Option<String>,
    /// "pending" | "running" | "succeeded" | "failed"
    pub status: String,
    /// How this deployment was triggered: "manual" | "webhook" | "upload" | "rollback"
    pub trigger_source: String,
    pub generated_dockerfile: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    /// The image was removed by pruning; rolling back to it is impossible
    #[sqlx(default)]
    pub image_pruned: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentResponse {
    pub id: String,
    pub app_id: String,
    pub image_tag: Option<String>,
    pub status: String,
    /// How this deployment was triggered: "manual" | "webhook" | "upload" | "rollback"
    pub trigger_source: String,
    pub has_generated_dockerfile: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    /// The deployment's image was pruned from the host
    #[serde(default)]
    pub image_pruned: bool,
    /// This deployment's image is what the app currently runs
    #[serde(default)]
    pub is_current: bool,
    /// `POST /api/apps/{id}/rollback {"deployment_id": id}` would restore this
    /// deployment's exact image
    #[serde(default)]
    pub rollback_available: bool,
    /// Why rollback is unavailable (for a tooltip); `None` when available
    #[serde(default)]
    pub rollback_unavailable_reason: Option<String>,
}

impl From<Deployment> for DeploymentResponse {
    fn from(d: Deployment) -> Self {
        Self {
            id: d.id,
            app_id: d.app_id,
            image_tag: d.image_tag,
            status: d.status,
            trigger_source: d.trigger_source,
            has_generated_dockerfile: d.generated_dockerfile.is_some(),
            started_at: d.started_at,
            finished_at: d.finished_at,
            created_at: d.created_at,
            image_pruned: d.image_pruned,
            is_current: false,
            rollback_available: false,
            rollback_unavailable_reason: None,
        }
    }
}

// ── Phase 4.4: Preview Deployments ──

/// A single ephemeral preview deployment for an app.
/// Created via `POST /api/apps/{id}/preview` or `tond deploy <app> --preview`.
/// Automatically cleaned up after `ttl_seconds` (default 24 h).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PreviewDeployment {
    pub id: String,
    pub app_id: String,
    /// Short random hex prefix used to build the subdomain (e.g. "a1b2c3")
    pub slug: String,
    /// Full subdomain assigned (e.g. "a1b2c3-myapp.yourdomain.com")
    pub subdomain: Option<String>,
    pub container_id: Option<String>,
    pub host_port: Option<i64>,
    pub image_tag: Option<String>,
    /// TTL in seconds from created_at
    pub ttl_seconds: i64,
    /// "building" | "running" | "failed" | "expired"
    pub status: String,
    pub expires_at: String,
    pub created_at: String,
    /// Non-null when this preview was created as part of a project preview
    pub project_preview_id: Option<String>,
    // ── Phase 4.6: PR preview fields ──
    /// GitHub PR number that triggered this preview (e.g. 42)
    pub pr_number: Option<i64>,
    /// Git commit SHA at the head of the PR (e.g. "a1b2c3d4...")
    pub head_sha: Option<String>,
    /// Branch name at the head of the PR (e.g. "feature/login")
    pub head_ref: Option<String>,
    /// GitHub PR HTML URL (e.g. "https://github.com/org/repo/pull/42")
    pub pr_url: Option<String>,
    /// GitHub issue comment ID posted to the PR thread after deploy (used to PATCH on re-deploy)
    pub comment_id: Option<i64>,
    /// GitLab numeric project ID from the MR webhook payload (used to post commit statuses)
    pub gitlab_project_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreviewDeploymentResponse {
    pub id: String,
    pub app_id: String,
    pub slug: String,
    pub subdomain: Option<String>,
    pub host_port: Option<i64>,
    pub image_tag: Option<String>,
    pub ttl_seconds: i64,
    pub status: String,
    pub url: Option<String>,
    pub expires_at: String,
    pub created_at: String,
    pub project_preview_id: Option<String>,
    // ── Phase 4.6: PR preview fields ──
    pub pr_number: Option<i64>,
    pub head_sha: Option<String>,
    pub head_ref: Option<String>,
    pub pr_url: Option<String>,
    /// GitHub issue comment ID posted to the PR thread (used to avoid duplicates)
    pub comment_id: Option<i64>,
}

impl PreviewDeploymentResponse {
    pub fn from_preview(p: PreviewDeployment) -> Self {
        let url = p.subdomain.as_ref().map(|s| format!("https://{}", s));
        Self {
            id: p.id,
            app_id: p.app_id,
            slug: p.slug,
            subdomain: p.subdomain,
            host_port: p.host_port,
            image_tag: p.image_tag,
            ttl_seconds: p.ttl_seconds,
            status: p.status,
            url,
            expires_at: p.expires_at,
            created_at: p.created_at,
            project_preview_id: p.project_preview_id,
            pr_number: p.pr_number,
            head_sha: p.head_sha,
            head_ref: p.head_ref,
            pr_url: p.pr_url,
            comment_id: p.comment_id,
        }
    }
}

/// POST /api/apps/{id}/preview request body
#[derive(Debug, Deserialize)]
pub struct CreatePreviewRequest {
    /// TTL in hours (default 24)
    pub ttl_hours: Option<i64>,
    // ── Phase 4.6: PR-specific preview fields ──
    /// GitHub PR number (e.g. 42)
    pub pr_number: Option<i64>,
    /// Git commit SHA at the PR head (e.g. "a1b2c3d4...")
    pub head_sha: Option<String>,
    /// Branch name at the PR head (e.g. "feature/login")
    pub head_ref: Option<String>,
    /// GitHub PR HTML URL (e.g. "https://github.com/org/repo/pull/42")
    pub pr_url: Option<String>,
}

// ── Phase 6.2: Project-level Preview Environments ──

/// One "project preview" groups a preview deployment for every member app
/// under a shared slug. Stored in the `project_previews` table.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectPreview {
    pub id: String,
    pub project_id: String,
    /// Shared slug prefix used to build every member app's subdomain
    pub slug: String,
    pub ttl_seconds: i64,
    pub expires_at: String,
    pub created_at: String,
}

/// Aggregated status computed from child preview_deployments at query time.
/// Never stored in the DB.
///
/// Rules (evaluated in priority order):
///   any building  → "building"
///   all running   → "running"
///   mix run+fail  → "partially_failed"
///   all failed    → "failed"
///   all expired   → "expired"
///   empty         → "building"
pub fn aggregate_preview_status(statuses: &[String]) -> String {
    if statuses.is_empty() {
        return "building".to_string();
    }
    let building = statuses.iter().any(|s| s == "building");
    let running = statuses.iter().filter(|s| s.as_str() == "running").count();
    let failed = statuses.iter().filter(|s| s.as_str() == "failed").count();
    let expired = statuses.iter().filter(|s| s.as_str() == "expired").count();
    let total = statuses.len();

    if building {
        "building".to_string()
    } else if running == total {
        "running".to_string()
    } else if expired == total {
        "expired".to_string()
    } else if failed == total {
        "failed".to_string()
    } else if running > 0 && failed > 0 {
        "partially_failed".to_string()
    } else {
        // mixed with expired counts etc.
        "building".to_string()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectPreviewResponse {
    pub id: String,
    pub project_id: String,
    pub slug: String,
    pub ttl_seconds: i64,
    /// Computed from child preview_deployments
    pub status: String,
    pub expires_at: String,
    pub created_at: String,
    /// The child per-app preview deployments
    pub previews: Vec<PreviewDeploymentResponse>,
}

/// POST /api/projects/{id}/previews request body
#[derive(Debug, Deserialize)]
pub struct CreateProjectPreviewRequest {
    /// TTL in hours (default 24)
    pub ttl_hours: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_response_from_deployment_preserves_fields() {
        let deployment = Deployment {
            id: "deploy-1".to_string(),
            app_id: "app-1".to_string(),
            image_tag: Some("tond-demo:latest".to_string()),
            status: "succeeded".to_string(),
            trigger_source: "manual".to_string(),
            generated_dockerfile: Some("FROM node:20-alpine".to_string()),
            started_at: "2026-01-01T00:00:00Z".to_string(),
            finished_at: Some("2026-01-01T00:01:00Z".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            image_pruned: false,
        };

        let response = DeploymentResponse::from(deployment);

        assert_eq!(
            response.status, "succeeded",
            "DeploymentResponse should preserve the deployment status"
        );
        assert_eq!(
            response.image_tag.as_deref(),
            Some("tond-demo:latest"),
            "DeploymentResponse should preserve the image tag"
        );
        assert!(
            response.has_generated_dockerfile,
            "DeploymentResponse should expose whether a generated Dockerfile was persisted"
        );
    }

    #[test]
    fn preview_deployment_response_builds_https_url_when_subdomain_exists() {
        let preview = PreviewDeployment {
            id: "preview-1".to_string(),
            app_id: "app-1".to_string(),
            slug: "abc123".to_string(),
            subdomain: Some("abc123-demo.example.com".to_string()),
            container_id: Some("container-1".to_string()),
            host_port: Some(18080),
            image_tag: Some("tond-demo:preview".to_string()),
            ttl_seconds: 3600,
            status: "running".to_string(),
            expires_at: "2026-01-02T00:00:00Z".to_string(),
            created_at: "2026-01-01T23:00:00Z".to_string(),
            project_preview_id: Some("project-preview-1".to_string()),
            pr_number: Some(42),
            head_sha: Some("abcdef".to_string()),
            head_ref: Some("feature/login".to_string()),
            pr_url: Some("https://github.com/org/repo/pull/42".to_string()),
            comment_id: Some(123),
            gitlab_project_id: Some(99),
        };

        let response = PreviewDeploymentResponse::from_preview(preview);

        assert_eq!(
            response.url.as_deref(),
            Some("https://abc123-demo.example.com"),
            "PreviewDeploymentResponse should derive an HTTPS URL from the preview subdomain"
        );
    }

    #[test]
    fn preview_deployment_response_leaves_url_empty_without_subdomain() {
        let preview = PreviewDeployment {
            id: "preview-1".to_string(),
            app_id: "app-1".to_string(),
            slug: "abc123".to_string(),
            subdomain: None,
            container_id: None,
            host_port: None,
            image_tag: None,
            ttl_seconds: 3600,
            status: "building".to_string(),
            expires_at: "2026-01-02T00:00:00Z".to_string(),
            created_at: "2026-01-01T23:00:00Z".to_string(),
            project_preview_id: None,
            pr_number: None,
            head_sha: None,
            head_ref: None,
            pr_url: None,
            comment_id: None,
            gitlab_project_id: None,
        };

        let response = PreviewDeploymentResponse::from_preview(preview);

        assert_eq!(
            response.url, None,
            "PreviewDeploymentResponse should not synthesize a URL when no subdomain exists"
        );
    }

    #[test]
    fn aggregate_preview_status_applies_documented_priority_rules() {
        assert_eq!(
            aggregate_preview_status(&[]),
            "building",
            "empty preview status sets should default to building"
        );
        assert_eq!(
            aggregate_preview_status(&["building".to_string(), "failed".to_string()]),
            "building",
            "any building preview should force the aggregate status to building"
        );
        assert_eq!(
            aggregate_preview_status(&["running".to_string(), "running".to_string()]),
            "running",
            "all-running previews should aggregate to running"
        );
        assert_eq!(
            aggregate_preview_status(&["failed".to_string(), "failed".to_string()]),
            "failed",
            "all-failed previews should aggregate to failed"
        );
        assert_eq!(
            aggregate_preview_status(&["expired".to_string(), "expired".to_string()]),
            "expired",
            "all-expired previews should aggregate to expired"
        );
        assert_eq!(
            aggregate_preview_status(&["running".to_string(), "failed".to_string()]),
            "partially_failed",
            "a running/failed mix should aggregate to partially_failed"
        );
        assert_eq!(
            aggregate_preview_status(&["running".to_string(), "expired".to_string()]),
            "building",
            "mixed states outside the explicit rules should fall back to building"
        );
    }
}
