// deploy/images.rs — per-deployment image tags, rollback eligibility, pruning
//
// Every deployment runs an image tagged for that deployment:
//
//   tond-<app>:<deployment-id-prefix>
//
// Built images (git, inline Dockerfile, tarball upload) are tagged that way
// directly or re-tagged after the build; pulled images (`source_type=image`)
// are re-tagged locally after the pull so a later rollback runs the exact
// bytes that were deployed, even if `nginx:latest` has moved on since.
//
// The tag is stored on the deployment row, which is what rollback redeploys.
// After each successful deploy or rollback the app's older images are pruned,
// keeping the newest `KEEP_IMAGES` successful deployments plus whatever is
// running; pruned deployments get `image_pruned = 1` so the dashboard can
// disable rolling back to them.

use anyhow::Result;
use bollard::Docker;
use bollard::query_parameters::{
    ListImagesOptionsBuilder, RemoveImageOptionsBuilder, TagImageOptionsBuilder,
};

use crate::state::AppState;

/// How many successful deployment images to keep per app (plus the running one).
pub const KEEP_IMAGES: usize = 3;

/// Tag used for the image a tarball upload builds before `deploy()` re-tags it.
const UPLOAD_TAG: &str = "upload";
/// Prefix for preview images: `tond-<app>:preview-<slug>`.
const PREVIEW_PREFIX: &str = "preview-";

/// Image repository for an app. Docker repository names must be lowercase.
pub fn image_repo(app_name: &str) -> String {
    format!("tond-{}", app_name.to_ascii_lowercase())
}

/// `tond-<app>:<first 12 hex chars of the deployment id>`
pub fn deployment_tag(app_name: &str, deployment_id: &str) -> String {
    let short: String = deployment_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("{}:{}", image_repo(app_name), short)
}

/// Tag for a tarball upload's build output.
pub fn upload_tag(app_name: &str) -> String {
    format!("{}:{}", image_repo(app_name), UPLOAD_TAG)
}

/// Tag for a preview build.
pub fn preview_tag(app_name: &str, slug: &str) -> String {
    format!(
        "{}:{}{}",
        image_repo(app_name),
        PREVIEW_PREFIX,
        slug.to_ascii_lowercase()
    )
}

/// True when `tag` is a per-deployment image of this app (and so can be
/// rolled back to). Compose sentinels, upload/preview tags and legacy
/// untagged images are not.
pub fn is_deployment_image(app_name: &str, tag: &str) -> bool {
    let prefix = format!("{}:", image_repo(app_name));
    match tag.strip_prefix(&prefix) {
        Some(suffix) => {
            !suffix.is_empty()
                && suffix != UPLOAD_TAG
                && !suffix.starts_with(PREVIEW_PREFIX)
                && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// Tag `source` (an image ref or ID) as `target` (`repo:tag`).
pub async fn tag_image(docker: &Docker, source: &str, target: &str) -> Result<()> {
    let (repo, tag) = target
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("image tag '{target}' has no ':tag' part"))?;
    docker
        .tag_image(
            source,
            Some(
                TagImageOptionsBuilder::default()
                    .repo(repo)
                    .tag(tag)
                    .build(),
            ),
        )
        .await?;
    Ok(())
}

/// One deployment row as far as pruning is concerned.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeploymentImage {
    pub image_tag: String,
    pub status: String,
}

/// Decide which image tags to delete. `deployments` must be newest first.
/// Keeps the newest `keep` distinct tags of succeeded deployments and the
/// running tag; every other per-deployment tag (including those of failed
/// deployments) is returned once.
pub fn tags_to_prune(
    app_name: &str,
    deployments: &[DeploymentImage],
    running: Option<&str>,
    keep: usize,
) -> Vec<String> {
    let mut kept: Vec<&str> = Vec::new();
    for d in deployments {
        if d.status == "succeeded"
            && is_deployment_image(app_name, &d.image_tag)
            && !kept.contains(&d.image_tag.as_str())
            && kept.len() < keep
        {
            kept.push(&d.image_tag);
        }
    }
    if let Some(r) = running {
        kept.push(r);
    }
    // Deployments still in progress own their image too.
    for d in deployments {
        if d.status == "running" {
            kept.push(&d.image_tag);
        }
    }

    let mut prune: Vec<String> = Vec::new();
    for d in deployments {
        if is_deployment_image(app_name, &d.image_tag)
            && !kept.contains(&d.image_tag.as_str())
            && !prune.contains(&d.image_tag)
        {
            prune.push(d.image_tag.clone());
        }
    }
    prune
}

/// The image tag of the app's running container, if Docker knows it.
async fn running_image_tag(state: &AppState, container_id: Option<&str>) -> Option<String> {
    let cid = container_id?;
    let info = state.docker.inspect_container(cid, None).await.ok()?;
    info.config.and_then(|c| c.image)
}

/// Remove per-deployment images of this app that no deployment row refers to
/// any more (e.g. the janitor trimmed old rows) and that aren't running.
async fn prune_orphaned_images(
    state: &AppState,
    app_name: &str,
    referenced: &[String],
    running: Option<&str>,
) {
    let reference = format!("{}:*", image_repo(app_name));
    let filters = std::collections::HashMap::from([("reference".to_string(), vec![reference])]);
    let Ok(found) = state
        .docker
        .list_images(Some(
            ListImagesOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await
    else {
        return;
    };
    for tag in found.into_iter().flat_map(|img| img.repo_tags) {
        if is_deployment_image(app_name, &tag)
            && !referenced.contains(&tag)
            && running != Some(tag.as_str())
        {
            tracing::info!("[prune] removing orphaned image {tag}");
            let _ = state
                .docker
                .remove_image(
                    &tag,
                    Some(RemoveImageOptionsBuilder::default().build()),
                    None,
                )
                .await;
        }
    }
}

/// Remove `tond-<app>:preview-*` images whose preview is no longer live.
async fn prune_preview_images(state: &AppState, app_id: &str, app_name: &str) {
    let live: Vec<String> = sqlx::query_scalar(
        "SELECT image_tag FROM preview_deployments
         WHERE app_id = ? AND image_tag IS NOT NULL AND status IN ('building', 'running')",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let reference = format!("{}:{}*", image_repo(app_name), PREVIEW_PREFIX);
    let filters = std::collections::HashMap::from([("reference".to_string(), vec![reference])]);
    let Ok(found) = state
        .docker
        .list_images(Some(
            ListImagesOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await
    else {
        return;
    };
    for tag in found.into_iter().flat_map(|img| img.repo_tags) {
        if !live.contains(&tag) {
            let _ = state
                .docker
                .remove_image(
                    &tag,
                    Some(RemoveImageOptionsBuilder::default().build()),
                    None,
                )
                .await;
        }
    }
}

// ── Compose releases ──
//
// A compose deployment's `image_tag` is `tond-<app>:<id>` like any other,
// but no image carries it: it names the *release*, the set of pinned service
// images `tond-<app>/<service>:<id>`. Pruning a release removes all of them.

/// `tond-<app>/<service>:<suffix>` → the release tag `tond-<app>:<suffix>`.
pub fn compose_release_of(app_name: &str, service_tag: &str) -> Option<String> {
    let rest = service_tag.strip_prefix(&format!("{}/", image_repo(app_name)))?;
    let (_, suffix) = rest.rsplit_once(':')?;
    Some(format!("{}:{suffix}", image_repo(app_name)))
}

/// Every local service image tag of the app (`tond-<app>/*`).
async fn compose_service_tags(state: &AppState, app_name: &str) -> Option<Vec<String>> {
    let reference = format!("{}/*", image_repo(app_name));
    let filters = std::collections::HashMap::from([("reference".to_string(), vec![reference])]);
    let found = state
        .docker
        .list_images(Some(
            ListImagesOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await
        .ok()?;
    Some(
        found
            .into_iter()
            .flat_map(|img| img.repo_tags)
            .filter(|t| compose_release_of(app_name, t).is_some())
            .collect(),
    )
}

/// Remove the service images of one release. True when none is left.
async fn remove_compose_release(
    state: &AppState,
    tags: &[String],
    app_name: &str,
    release: &str,
) -> bool {
    let mut all_gone = true;
    for tag in tags
        .iter()
        .filter(|t| compose_release_of(app_name, t).as_deref() == Some(release))
    {
        match state
            .docker
            .remove_image(
                tag,
                Some(RemoveImageOptionsBuilder::default().build()),
                None,
            )
            .await
        {
            Ok(_)
            | Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => tracing::info!("[prune] removed old image {tag}"),
            Err(e) => {
                tracing::warn!("[prune] keeping image {tag}: {e}");
                all_gone = false;
            }
        }
    }
    all_gone
}

/// Record that a compose deployment's images are gone.
pub async fn mark_compose_release_pruned(state: &AppState, app_id: &str, deployment_id: &str) {
    let _ = sqlx::query(
        "UPDATE deployments SET image_pruned = 1 WHERE app_id = ? AND image_tag =
           (SELECT image_tag FROM deployments WHERE id = ?)",
    )
    .bind(app_id)
    .bind(deployment_id)
    .execute(&state.db)
    .await;
}

/// Compose flavour of [`prune_app_images`]: releases instead of images.
async fn prune_compose_images(state: &AppState, app_id: &str, app_name: &str) {
    let deployments: Vec<DeploymentImage> = sqlx::query_as(
        "SELECT image_tag, status FROM deployments
         WHERE app_id = ? AND image_tag IS NOT NULL AND image_pruned = 0
         ORDER BY created_at DESC",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let running: Option<String> = sqlx::query_scalar("SELECT container_id FROM apps WHERE id = ?")
        .bind(app_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten();
    let referenced: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT image_tag FROM deployments WHERE app_id = ? AND image_tag IS NOT NULL",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let Some(tags) = compose_service_tags(state, app_name).await else {
        return;
    };
    // Service images no deployment refers to any more.
    let mut orphans: Vec<String> = Vec::new();
    for tag in &tags {
        if let Some(release) = compose_release_of(app_name, tag)
            && !referenced.contains(&release)
            && running.as_deref() != Some(release.as_str())
            && !orphans.contains(&release)
        {
            orphans.push(release);
        }
    }
    for release in orphans {
        remove_compose_release(state, &tags, app_name, &release).await;
    }
    for release in tags_to_prune(app_name, &deployments, running.as_deref(), KEEP_IMAGES) {
        if remove_compose_release(state, &tags, app_name, &release).await {
            let _ = sqlx::query(
                "UPDATE deployments SET image_pruned = 1 WHERE app_id = ? AND image_tag = ?",
            )
            .bind(app_id)
            .bind(&release)
            .execute(&state.db)
            .await;
        }
    }
}

/// Remove every service image of a compose app (app deletion).
pub async fn remove_compose_images(state: &AppState, app_name: &str) {
    for tag in compose_service_tags(state, app_name)
        .await
        .unwrap_or_default()
    {
        let _ = state
            .docker
            .remove_image(
                &tag,
                Some(RemoveImageOptionsBuilder::default().force(true).build()),
                None,
            )
            .await;
    }
}

/// Remove the app's old deployment and preview images and mark pruned
/// deployments. Best-effort: an image Docker refuses to remove (still in use)
/// is kept.
pub async fn prune_app_images(state: &AppState, app_id: &str, app_name: &str) {
    let source_type: Option<String> =
        sqlx::query_scalar("SELECT source_type FROM apps WHERE id = ?")
            .bind(app_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    if source_type.as_deref() == Some("compose") {
        prune_compose_images(state, app_id, app_name).await;
        return;
    }
    prune_preview_images(state, app_id, app_name).await;

    let deployments: Vec<DeploymentImage> = match sqlx::query_as(
        "SELECT image_tag, status FROM deployments
         WHERE app_id = ? AND image_tag IS NOT NULL AND image_pruned = 0
         ORDER BY created_at DESC",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("[prune] could not list deployments for {app_name}: {e}");
            return;
        }
    };
    let container_id: Option<String> =
        sqlx::query_scalar("SELECT container_id FROM apps WHERE id = ?")
            .bind(app_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .flatten();
    let running = running_image_tag(state, container_id.as_deref()).await;

    let referenced: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT image_tag FROM deployments WHERE app_id = ? AND image_tag IS NOT NULL",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    if !referenced.is_empty() {
        prune_orphaned_images(state, app_name, &referenced, running.as_deref()).await;
    }

    for tag in tags_to_prune(app_name, &deployments, running.as_deref(), KEEP_IMAGES) {
        let removed = match state
            .docker
            .remove_image(
                &tag,
                Some(RemoveImageOptionsBuilder::default().build()),
                None,
            )
            .await
        {
            Ok(_) => true,
            // Already gone: record it so the dashboard stops offering it.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => true,
            Err(e) => {
                tracing::warn!("[prune] keeping image {tag}: {e}");
                false
            }
        };
        if removed {
            tracing::info!("[prune] removed old image {tag}");
            let _ = sqlx::query(
                "UPDATE deployments SET image_pruned = 1 WHERE app_id = ? AND image_tag = ?",
            )
            .bind(app_id)
            .bind(&tag)
            .execute(&state.db)
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(tag: &str, status: &str) -> DeploymentImage {
        DeploymentImage {
            image_tag: tag.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn tags_are_per_deployment_and_lowercase() {
        assert_eq!(image_repo("MyApp"), "tond-myapp");
        assert_eq!(
            deployment_tag("MyApp", "3F2A1B9C-1234-5678-9abc-def012345678"),
            "tond-myapp:3f2a1b9c1234"
        );
        assert_eq!(upload_tag("web"), "tond-web:upload");
        assert_eq!(preview_tag("web", "AB12"), "tond-web:preview-ab12");
    }

    #[test]
    fn only_per_deployment_tags_are_rollback_targets() {
        assert!(is_deployment_image("web", "tond-web:3f2a1b9c1234"));
        assert!(!is_deployment_image("web", "tond-web:upload"));
        assert!(!is_deployment_image("web", "tond-web:preview-ab12"));
        assert!(!is_deployment_image("web", "tond-web"));
        assert!(!is_deployment_image("web", "tond-compose-web"));
        assert!(!is_deployment_image("web", "nginx:latest"));
        assert!(!is_deployment_image("web", "tond-webapp:3f2a1b9c1234"));
    }

    #[test]
    fn compose_service_images_belong_to_their_release() {
        assert_eq!(
            compose_release_of("shop", "tond-shop/web:3f2a1b9c1234").as_deref(),
            Some("tond-shop:3f2a1b9c1234")
        );
        assert_eq!(compose_release_of("shop", "tond-shopx/web:1"), None);
        assert_eq!(compose_release_of("shop", "tond-shop:1"), None);
        assert!(is_deployment_image("shop", "tond-shop:3f2a1b9c1234"));
    }

    #[test]
    fn pruning_keeps_the_newest_successes_and_the_running_image() {
        let deployments = vec![
            d("tond-web:6", "succeeded"),
            d("tond-web:5", "failed"),
            d("tond-web:4", "succeeded"),
            d("tond-web:3", "succeeded"),
            d("tond-web:2", "succeeded"),
            d("tond-web:1", "succeeded"),
            d("tond-compose-web", "succeeded"),
        ];
        // Running an old image after a rollback keeps it too.
        let prune = tags_to_prune("web", &deployments, Some("tond-web:1"), 3);
        assert_eq!(
            prune,
            vec!["tond-web:5".to_string(), "tond-web:2".to_string()]
        );
    }

    #[test]
    fn rollback_deployments_reusing_a_tag_count_once() {
        let deployments = vec![
            d("tond-web:2", "succeeded"), // rollback to 2
            d("tond-web:3", "succeeded"),
            d("tond-web:2", "succeeded"),
            d("tond-web:1", "succeeded"),
        ];
        assert_eq!(
            tags_to_prune("web", &deployments, Some("tond-web:2"), 2),
            vec!["tond-web:1".to_string()]
        );
    }

    #[test]
    fn images_of_in_progress_deployments_are_kept() {
        let deployments = vec![d("tond-web:9", "running"), d("tond-web:8", "succeeded")];
        assert_eq!(
            tags_to_prune("web", &deployments, None, 0),
            vec!["tond-web:8".to_string()]
        );
    }
}
