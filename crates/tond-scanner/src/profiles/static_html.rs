use std::path::Path;

use crate::{
    BuildPlan, ScannerError,
    profile::{DetectionOutcome, LanguageProfile},
};

pub(crate) struct StaticHtmlProfile;

impl LanguageProfile for StaticHtmlProfile {
    fn name(&self) -> &'static str {
        "Static HTML"
    }

    fn detect(&self, repo_path: &Path) -> bool {
        detect_document_root(repo_path).is_some()
    }

    fn detect_with_confidence(&self, repo_path: &Path) -> Option<DetectionOutcome> {
        let document_root = detect_document_root(repo_path)?;

        Some(DetectionOutcome {
            confidence: 30,
            reasons: vec![format!(
                "found static site entrypoint {}/{}",
                document_root,
                detect_index_file(repo_path, document_root).unwrap_or("index.html")
            )],
        })
    }

    fn generate_dockerfile_in_dir(
        &self,
        _repo_root: &Path,
        build_root: &Path,
        _static_build: bool,
    ) -> Result<String, ScannerError> {
        let document_root = detect_document_root(build_root).ok_or_else(|| {
            ScannerError::UnsupportedFramework(
                "Detected static site markers but could not find a supported document root."
                    .to_string(),
            )
        })?;

        Ok(render_dockerfile(document_root))
    }

    fn build_plan_in_dir(
        &self,
        _repo_root: &Path,
        build_root: &Path,
        _static_build: bool,
    ) -> Result<BuildPlan, ScannerError> {
        let document_root = detect_document_root(build_root).ok_or_else(|| {
            ScannerError::UnsupportedFramework(
                "Detected static site markers but could not find a supported document root."
                    .to_string(),
            )
        })?;
        let run_command = format!("npx --yes serve -s {document_root} -l 8080");

        Ok(BuildPlan {
            profile_name: self.name().to_string(),
            stack: "static-html".to_string(),
            confidence: 30,
            detection_reasons: vec![format!("found static site document root {document_root}")],
            builder_image: Some("node:22.20.0-bookworm-slim".to_string()),
            runtime_image: Some("node:22.20.0-bookworm-slim".to_string()),
            run_command: Some(run_command),
            static_output_dir: Some(document_root.to_string()),
            dockerfile: render_dockerfile(document_root),
        })
    }
}

fn detect_document_root(repo_path: &Path) -> Option<&'static str> {
    [".", "public", "www", "site", "dist", "build"]
        .into_iter()
        .find(|candidate| detect_index_file(repo_path, candidate).is_some())
}

fn detect_index_file(repo_path: &Path, base: &str) -> Option<&'static str> {
    ["index.html", "index.htm"]
        .into_iter()
        .find(|candidate| repo_path.join(base).join(candidate).exists())
}

fn render_dockerfile(document_root: &str) -> String {
    format!(
        "\
# Stage 1: Static assets
FROM node:22.20.0-bookworm-slim AS builder
WORKDIR /app
COPY . ./

# Stage 2: Production (static file server)
FROM node:22.20.0-bookworm-slim AS runner
WORKDIR /app
COPY --from=builder /app ./
EXPOSE 8080
CMD [\"npx\", \"--yes\", \"serve\", \"-s\", \"{document_root}\", \"-l\", \"8080\"]
"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::profile::LanguageProfile;

    fn create_repo() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temporary repo: {err}"))
    }

    #[test]
    fn detect_returns_true_for_root_index_html() {
        let repo = create_repo();
        fs::write(repo.path().join("index.html"), "<!doctype html>\n")
            .unwrap_or_else(|err| panic!("failed to write index.html: {err}"));

        assert!(
            StaticHtmlProfile.detect(repo.path()),
            "plain sites with a root index.html should be detected"
        );
    }

    #[test]
    fn detect_returns_true_for_public_index_html() {
        let repo = create_repo();
        fs::create_dir_all(repo.path().join("public"))
            .unwrap_or_else(|err| panic!("failed to create public directory: {err}"));
        fs::write(
            repo.path().join("public").join("index.html"),
            "<!doctype html>\n",
        )
        .unwrap_or_else(|err| panic!("failed to write public/index.html: {err}"));

        assert!(
            StaticHtmlProfile.detect(repo.path()),
            "plain sites with a public/index.html document root should be detected"
        );
    }

    #[test]
    fn detect_returns_false_without_supported_index_file() {
        let repo = create_repo();
        fs::write(repo.path().join("about.html"), "<!doctype html>\n")
            .unwrap_or_else(|err| panic!("failed to write about.html: {err}"));

        assert!(
            !StaticHtmlProfile.detect(repo.path()),
            "repositories without a supported document root should not be detected as static sites"
        );
    }

    #[test]
    fn detect_returns_true_for_common_static_site_directories() {
        for document_root in ["www", "site", "dist", "build"] {
            let repo = create_repo();
            fs::create_dir_all(repo.path().join(document_root))
                .unwrap_or_else(|err| panic!("failed to create {document_root} directory: {err}"));
            fs::write(
                repo.path().join(document_root).join("index.html"),
                "<!doctype html>\n",
            )
            .unwrap_or_else(|err| panic!("failed to write {document_root}/index.html: {err}"));

            assert!(
                StaticHtmlProfile.detect(repo.path()),
                "plain sites with {document_root}/index.html should be detected"
            );
            assert_eq!(
                detect_document_root(repo.path()),
                Some(document_root),
                "document root detection should recognize {document_root}/index.html"
            );
        }
    }

    #[test]
    fn detect_document_root_prefers_repo_root_over_nested_static_directories() {
        let repo = create_repo();
        fs::write(repo.path().join("index.html"), "<!doctype html>\n")
            .unwrap_or_else(|err| panic!("failed to write index.html: {err}"));
        fs::create_dir_all(repo.path().join("dist"))
            .unwrap_or_else(|err| panic!("failed to create dist directory: {err}"));
        fs::write(
            repo.path().join("dist").join("index.html"),
            "<!doctype html>\n",
        )
        .unwrap_or_else(|err| panic!("failed to write dist/index.html: {err}"));

        assert_eq!(
            detect_document_root(repo.path()),
            Some("."),
            "a repo-root index.html should win over nested static output directories"
        );
    }

    #[test]
    fn build_plan_serves_public_document_root_when_present() {
        let repo = create_repo();
        fs::create_dir_all(repo.path().join("public"))
            .unwrap_or_else(|err| panic!("failed to create public directory: {err}"));
        fs::write(
            repo.path().join("public").join("index.htm"),
            "<!doctype html>\n",
        )
        .unwrap_or_else(|err| panic!("failed to write public/index.htm: {err}"));

        let plan = StaticHtmlProfile
            .build_plan(repo.path(), true)
            .unwrap_or_else(|err| {
                panic!("expected static HTML BuildPlan generation to succeed: {err}")
            });

        assert_eq!(
            plan.static_output_dir.as_deref(),
            Some("public"),
            "public document roots should be served from the public directory"
        );
        assert!(
            plan.dockerfile.contains(
                "CMD [\"npx\", \"--yes\", \"serve\", \"-s\", \"public\", \"-l\", \"8080\"]"
            ),
            "the generated Dockerfile should serve the detected document root, got: {}",
            plan.dockerfile
        );
    }
}
