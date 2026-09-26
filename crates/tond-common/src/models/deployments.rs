// deployments.rs — deployment history and preview deployment types

use serde::{Deserialize, Serialize};

// ── Deployment (deployment history entry) ──

/// The steps of a deployment, in order. On a single host images never leave
/// the machine, so there is no registry push: `release` (switching traffic to
/// the new container and draining the old one) is the step users wait on
/// instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum DeployPhase {
    /// Fetch the source: git clone, image pull, or the uploaded image.
    Clone,
    /// Build the image (TBuild or Dockerfile); compose runs `up --build` here.
    Build,
    /// Start the new container.
    Start,
    /// Wait for the new container to answer (health path or port probe).
    Health,
    /// Route traffic to the new container and stop the old one.
    Release,
}

impl DeployPhase {
    pub const ALL: [DeployPhase; 5] = [
        DeployPhase::Clone,
        DeployPhase::Build,
        DeployPhase::Start,
        DeployPhase::Health,
        DeployPhase::Release,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DeployPhase::Clone => "clone",
            DeployPhase::Build => "build",
            DeployPhase::Start => "start",
            DeployPhase::Health => "health",
            DeployPhase::Release => "release",
        }
    }
}

/// Where a phase is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum PhaseStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    /// Not part of this deployment (e.g. no build for an image app).
    Skipped,
}

/// One phase's state and timing, stored as JSON in `deployments.phases`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PhaseRecord {
    pub phase: DeployPhase,
    pub status: PhaseStatus,
    /// Short label for this deployment's flavour of the phase ("Pull image").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

impl PhaseRecord {
    pub fn pending(phase: DeployPhase) -> Self {
        Self {
            phase,
            status: PhaseStatus::Pending,
            label: None,
            started_at: None,
            finished_at: None,
        }
    }
}

/// The classified likely cause of a failed deployment, stored as JSON in
/// `deployments.failure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeployFailure {
    /// Phase that failed, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<DeployPhase>,
    /// Stable machine-readable cause, e.g. "build_failed", "health_check_failed".
    pub code: String,
    /// One-line summary: "The build failed (exit code 1)".
    pub title: String,
    /// What to do about it.
    pub hint: String,
    /// The raw error, for reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The log (or container output) lines that show the problem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excerpt: Vec<String>,
}

/// A single deployment attempt for an app. One row per deploy invocation.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Deployment {
    pub id: String,
    pub app_id: String,
    /// Image this deployment ran: `tond-<app>:<deployment-id-prefix>`. For
    /// compose apps the same tag names the deployment's *set* of service
    /// images (`tond-<app>/<service>:<deployment-id-prefix>`, see
    /// `deployment_services`).
    pub image_tag: Option<String>,
    /// "queued" | "running" | "succeeded" | "failed" | "superseded"
    pub status: String,
    /// How this deployment was triggered: "manual" | "webhook" | "upload" |
    /// "rollback" | "cron" | "cli"
    pub trigger_source: String,
    pub generated_dockerfile: Option<String>,
    /// When work began (equals `created_at` while queued)
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    /// The image was removed by pruning; rolling back to it is impossible
    #[sqlx(default)]
    pub image_pruned: bool,
    /// "deploy" | "rollback" | "upload"
    #[sqlx(default)]
    pub kind: String,
    /// Why it failed, was superseded or was interrupted
    #[sqlx(default)]
    pub error: Option<String>,
    /// The deployment that replaced this one while it was queued
    #[sqlx(default)]
    pub superseded_by: Option<String>,
    #[sqlx(default)]
    pub commit_sha: Option<String>,
    #[sqlx(default)]
    pub commit_message: Option<String>,
    #[sqlx(default)]
    pub commit_author: Option<String>,
    /// Who or what triggered it (user email, "api-key", webhook provider…)
    #[sqlx(default)]
    pub trigger_detail: Option<String>,
    /// For rollbacks: the deployment whose image was restored
    #[sqlx(default)]
    pub rollback_of: Option<String>,
    /// JSON array of [`PhaseRecord`]
    #[sqlx(default)]
    pub phases: String,
    /// JSON [`DeployFailure`] for failed deployments
    #[sqlx(default)]
    pub failure: Option<String>,
    /// Compose apps: the compose file this deployment ran (for rollback)
    #[sqlx(default)]
    #[serde(default, skip_serializing)]
    pub compose_source: Option<String>,
}

impl Deployment {
    /// Parsed phase records (empty for rows that predate phase tracking).
    pub fn phase_records(&self) -> Vec<PhaseRecord> {
        serde_json::from_str(&self.phases).unwrap_or_default()
    }

    /// Parsed failure, if any.
    pub fn failure_info(&self) -> Option<DeployFailure> {
        self.failure
            .as_deref()
            .and_then(|f| serde_json::from_str(f).ok())
    }
}

/// Seconds between two RFC 3339 timestamps, if both parse.
pub fn duration_secs(started_at: &str, finished_at: Option<&str>) -> Option<i64> {
    let start = chrono::DateTime::parse_from_rfc3339(started_at).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(finished_at?).ok()?;
    Some((end - start).num_seconds().max(0))
}

/// Deployment statuses that are not final yet.
pub const UNFINISHED_DEPLOYMENT_STATUSES: [&str; 2] = ["queued", "running"];

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeploymentResponse {
    pub id: String,
    pub app_id: String,
    pub image_tag: Option<String>,
    /// "queued" | "building" | "deploying" | "succeeded" | "failed" | "superseded"
    pub status: String,
    /// How this deployment was triggered: "manual" | "webhook" | "upload" |
    /// "rollback" | "cron" | "cli"
    pub trigger_source: String,
    /// "deploy" | "rollback" | "upload"
    #[serde(default)]
    pub kind: String,
    /// Why it failed, was superseded or was interrupted
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    /// 1-based place in the build queue while status is "queued"
    #[serde(default)]
    pub queue_position: Option<u32>,
    /// Who or what triggered it (user email, "api-key", webhook provider…)
    #[serde(default)]
    pub trigger_detail: Option<String>,
    pub has_generated_dockerfile: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    /// Wall-clock duration in seconds, once finished
    #[serde(default)]
    pub duration_secs: Option<i64>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub commit_message: Option<String>,
    #[serde(default)]
    pub commit_author: Option<String>,
    /// For rollbacks: the deployment whose image was restored
    #[serde(default)]
    pub rollback_of: Option<String>,
    /// Step progress (clone, build, start, health, release)
    #[serde(default)]
    pub phases: Vec<PhaseRecord>,
    /// Likely cause of a failure, with a fix hint
    #[serde(default)]
    pub failure: Option<DeployFailure>,
    /// The deployment's image was pruned from the host
    #[serde(default)]
    pub image_pruned: bool,
    /// This deployment's image is what the app currently runs
    #[serde(default)]
    pub is_current: bool,
    /// `POST /api/v1/apps/{id}/rollback {"deployment_id": id}` would restore this
    /// deployment's exact image
    #[serde(default)]
    pub rollback_available: bool,
    /// Why rollback is unavailable (for a tooltip); `None` when available
    #[serde(default)]
    pub rollback_unavailable_reason: Option<String>,
    /// Compose apps: per-service images and progress of this deployment
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<crate::DeploymentService>,
}

impl From<Deployment> for DeploymentResponse {
    fn from(d: Deployment) -> Self {
        let phases = d.phase_records();
        let failure = d.failure_info();
        let duration_secs = duration_secs(&d.started_at, d.finished_at.as_deref());
        Self {
            id: d.id,
            app_id: d.app_id,
            image_tag: d.image_tag,
            status: d.status,
            trigger_source: d.trigger_source,
            kind: d.kind,
            error: d.error,
            superseded_by: d.superseded_by,
            queue_position: None,
            trigger_detail: d.trigger_detail,
            has_generated_dockerfile: d.generated_dockerfile.is_some(),
            started_at: d.started_at,
            finished_at: d.finished_at,
            created_at: d.created_at,
            duration_secs,
            commit_sha: d.commit_sha,
            commit_message: d.commit_message,
            commit_author: d.commit_author,
            rollback_of: d.rollback_of,
            phases,
            failure,
            image_pruned: d.image_pruned,
            is_current: false,
            rollback_available: false,
            rollback_unavailable_reason: None,
            services: Vec::new(),
        }
    }
}

/// One deploy log line of a deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeploymentLogLine {
    pub level: String,
    pub message: String,
    pub ts: String,
}

/// `GET /api/v1/apps/{id}/deployments/{deployment_id}`: the deployment plus its
/// log (the newest lines, capped server-side).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeploymentDetailResponse {
    #[serde(flatten)]
    pub deployment: DeploymentResponse,
    pub logs: Vec<DeploymentLogLine>,
    /// True when older lines were left out
    #[serde(default)]
    pub logs_truncated: bool,
    /// Port the app is expected to listen on (for the result card)
    pub container_port: i64,
    /// Health check path Tond polls, if any
    #[serde(default)]
    pub health_check_path: Option<String>,
    /// The app's source type ("git", "image", ...)
    pub source_type: String,
}

// ── Phase 4.4: Preview Deployments ──

/// A single ephemeral preview deployment for an app.
/// Created via `POST /api/v1/apps/{id}/preview` or `tond deploy <app> --preview`.
/// Automatically cleaned up after `ttl_seconds` (default 24 h).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// POST /api/v1/apps/{id}/preview request body
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// POST /api/v1/projects/{id}/previews request body
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
            kind: "deploy".to_string(),
            error: Some("boom".to_string()),
            superseded_by: None,
            commit_sha: Some("0123456789abcdef".to_string()),
            commit_message: Some("Fix login".to_string()),
            commit_author: Some("Ada".to_string()),
            trigger_detail: Some("ada@example.com".to_string()),
            rollback_of: None,
            phases: r#"[{"phase":"build","status":"succeeded","started_at":"2026-01-01T00:00:00Z","finished_at":"2026-01-01T00:00:40Z"}]"#.to_string(),
            failure: None,
            compose_source: None,
        };

        let response = DeploymentResponse::from(deployment);
        assert_eq!(response.duration_secs, Some(60));
        assert_eq!(response.commit_message.as_deref(), Some("Fix login"));
        assert_eq!(response.phases.len(), 1);
        assert_eq!(response.phases[0].phase, DeployPhase::Build);
        assert_eq!(response.phases[0].status, PhaseStatus::Succeeded);
        assert!(response.failure.is_none());
        assert_eq!(response.kind, "deploy");
        assert_eq!(response.error.as_deref(), Some("boom"));
        assert_eq!(response.queue_position, None);

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
    fn deployment_parses_failure_and_tolerates_bad_json() {
        let mut d = Deployment {
            id: "d".into(),
            app_id: "a".into(),
            image_tag: None,
            status: "failed".into(),
            trigger_source: "manual".into(),
            generated_dockerfile: None,
            started_at: "2026-01-01T00:00:00Z".into(),
            finished_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            image_pruned: false,
            kind: "deploy".into(),
            error: None,
            superseded_by: None,
            commit_sha: None,
            commit_message: None,
            commit_author: None,
            trigger_detail: None,
            rollback_of: None,
            phases: "not json".into(),
            failure: Some(
                r#"{"phase":"health","code":"health_check_failed","title":"t","hint":"h"}"#.into(),
            ),
            compose_source: None,
        };
        assert!(d.phase_records().is_empty());
        let failure = d
            .failure_info()
            .unwrap_or_else(|| panic!("failure should parse"));
        assert_eq!(failure.phase, Some(DeployPhase::Health));
        assert!(failure.excerpt.is_empty());
        d.failure = Some("{".into());
        assert!(d.failure_info().is_none());
        assert_eq!(duration_secs(&d.started_at, None), None);
        assert_eq!(
            duration_secs("2026-01-01T00:00:00Z", Some("2026-01-01T00:02:05Z")),
            Some(125)
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
