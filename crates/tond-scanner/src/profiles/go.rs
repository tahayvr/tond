// profiles/go.rs — Go detection and Dockerfile generation
//
// Fixes applied vs the original GO constant in generate.rs:
//   - Build path is detected from the repository layout:
//       If a `cmd/` directory exists and contains at least one subdirectory,
//       the first subdirectory (alphabetical) is used as the main-package path
//       (e.g. `./cmd/server`).  This is the idiomatic Go project layout for
//       multi-binary repos.
//       Otherwise the build targets the repo root (`.`), which is correct for
//       single-binary repos where `package main` lives at the top level.

use std::path::Path;

use crate::{
    BuildPlan, ScannerError,
    profile::{DetectionOutcome, LanguageProfile},
};

const GO_BUILDER_IMAGE: &str = "golang:1.24.13-bookworm";
const GO_RUNTIME_IMAGE: &str = "debian:bookworm-slim";

// ── Public profile struct ────────────────────────────────────────────────────

pub(crate) struct GoProfile;

impl LanguageProfile for GoProfile {
    fn name(&self) -> &'static str {
        "Go"
    }

    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join("go.mod").exists()
    }

    fn detect_with_confidence(&self, repo_path: &Path) -> Option<DetectionOutcome> {
        repo_path
            .join("go.mod")
            .exists()
            .then_some(DetectionOutcome {
                confidence: 75,
                reasons: vec!["found go.mod in build root".to_string()],
            })
    }

    fn generate_dockerfile(
        &self,
        repo_path: &Path,
        _static_build: bool,
    ) -> Result<String, ScannerError> {
        let build_path = detect_build_path(repo_path);

        tracing::debug!("tond-scanner: Go — build_path={}", build_path);

        Ok(render_dockerfile(&build_path))
    }

    fn build_plan(&self, repo_path: &Path, static_build: bool) -> Result<BuildPlan, ScannerError> {
        let build_path = detect_build_path(repo_path);

        Ok(BuildPlan {
            profile_name: self.name().to_string(),
            stack: "go".to_string(),
            confidence: 75,
            detection_reasons: vec!["found go.mod in build root".to_string()],
            builder_image: Some(GO_BUILDER_IMAGE.to_string()),
            runtime_image: Some(GO_RUNTIME_IMAGE.to_string()),
            run_command: (!static_build).then_some("./main".to_string()),
            static_output_dir: None,
            dockerfile: render_dockerfile(&build_path),
        })
    }
}

// ── Build path detection ─────────────────────────────────────────────────────

/// Return the Go build path (`"."` or `"./cmd/<name>"`).
///
/// Checks for the idiomatic `cmd/<name>/` layout first.  If the `cmd/`
/// directory exists and contains at least one subdirectory, the first
/// subdirectory in alphabetical order is used (e.g. if `cmd/server/` and
/// `cmd/migrate/` both exist, `./cmd/migrate` is chosen).  If the layout is
/// ambiguous or `cmd/` does not exist, we fall back to `"."` (root package).
fn detect_build_path(repo_path: &Path) -> String {
    let cmd_dir = repo_path.join("cmd");
    if cmd_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&cmd_dir)
    {
        let mut subdirs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        subdirs.sort(); // deterministic: alphabetical order

        if let Some(name) = subdirs.first() {
            return format!("./cmd/{name}");
        }
    }

    // Default: main package at repo root.
    ".".to_string()
}

// ── Dockerfile rendering ─────────────────────────────────────────────────────

fn render_dockerfile(build_path: &str) -> String {
    format!(
        "\
# Stage 1: Build
FROM {GO_BUILDER_IMAGE} AS builder
WORKDIR /app
COPY go.mod go.sum* ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -o main {build_path}

# Stage 2: Production
FROM {GO_RUNTIME_IMAGE}
WORKDIR /app
COPY --from=builder /app/main .
EXPOSE 8080
CMD [\"./main\"]
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
    fn detect_returns_true_when_go_mod_exists() {
        let repo = create_repo();
        fs::write(repo.path().join("go.mod"), "module example.com/demo\n")
            .unwrap_or_else(|err| panic!("failed to write go.mod: {err}"));

        let detected = GoProfile.detect(repo.path());

        assert!(
            detected,
            "GoProfile should detect repositories that contain a go.mod file"
        );
    }

    #[test]
    fn detect_returns_false_when_go_mod_is_missing() {
        let repo = create_repo();

        let detected = GoProfile.detect(repo.path());

        assert!(
            !detected,
            "GoProfile should ignore repositories without a go.mod file"
        );
    }

    #[test]
    fn detect_build_path_uses_root_when_cmd_directory_is_missing_or_empty() {
        let repo_without_cmd = create_repo();
        assert_eq!(
            detect_build_path(repo_without_cmd.path()),
            ".",
            "repositories without a cmd directory should build from the repo root"
        );

        let repo_with_empty_cmd = create_repo();
        fs::create_dir(repo_with_empty_cmd.path().join("cmd"))
            .unwrap_or_else(|err| panic!("failed to create cmd directory: {err}"));
        assert_eq!(
            detect_build_path(repo_with_empty_cmd.path()),
            ".",
            "repositories with an empty cmd directory should still build from the repo root"
        );
    }

    #[test]
    fn detect_build_path_chooses_first_cmd_subdirectory_alphabetically() {
        let repo = create_repo();
        fs::create_dir(repo.path().join("cmd"))
            .unwrap_or_else(|err| panic!("failed to create cmd directory: {err}"));
        fs::create_dir(repo.path().join("cmd").join("server"))
            .unwrap_or_else(|err| panic!("failed to create cmd/server: {err}"));
        fs::create_dir(repo.path().join("cmd").join("migrate"))
            .unwrap_or_else(|err| panic!("failed to create cmd/migrate: {err}"));
        fs::write(
            repo.path().join("cmd").join("README.md"),
            "not a directory\n",
        )
        .unwrap_or_else(|err| panic!("failed to write cmd/README.md: {err}"));

        let build_path = detect_build_path(repo.path());

        assert_eq!(
            build_path, "./cmd/migrate",
            "when multiple cmd subdirectories exist, the first alphabetical entry should be chosen deterministically"
        );
    }

    #[test]
    fn generate_dockerfile_includes_detected_build_path() {
        let repo = create_repo();
        fs::write(repo.path().join("go.mod"), "module example.com/demo\n")
            .unwrap_or_else(|err| panic!("failed to write go.mod: {err}"));
        fs::create_dir(repo.path().join("cmd"))
            .unwrap_or_else(|err| panic!("failed to create cmd directory: {err}"));
        fs::create_dir(repo.path().join("cmd").join("api"))
            .unwrap_or_else(|err| panic!("failed to create cmd/api: {err}"));

        let dockerfile = GoProfile
            .generate_dockerfile(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("RUN CGO_ENABLED=0 GOOS=linux go build -o main ./cmd/api"),
            "generated Dockerfile should build the detected cmd subdirectory, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("CMD [\"./main\"]"),
            "generated Go Dockerfiles should run the compiled binary, got: {dockerfile}"
        );
    }

    #[test]
    fn build_plan_captures_go_detection_details() {
        let repo = create_repo();
        fs::write(repo.path().join("go.mod"), "module example.com/demo\n")
            .unwrap_or_else(|err| panic!("failed to write go.mod: {err}"));
        fs::create_dir(repo.path().join("cmd"))
            .unwrap_or_else(|err| panic!("failed to create cmd directory: {err}"));
        fs::create_dir(repo.path().join("cmd").join("api"))
            .unwrap_or_else(|err| panic!("failed to create cmd/api: {err}"));

        let plan = GoProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected BuildPlan generation to succeed: {err}"));

        assert_eq!(
            plan.stack, "go",
            "Go BuildPlan should record a stable stack identifier"
        );
        assert_eq!(
            plan.builder_image.as_deref(),
            Some(GO_BUILDER_IMAGE),
            "Go BuildPlan should capture the builder image"
        );
        assert_eq!(
            plan.runtime_image.as_deref(),
            Some(GO_RUNTIME_IMAGE),
            "Go BuildPlan should capture the runtime image"
        );
        assert_eq!(
            plan.run_command.as_deref(),
            Some("./main"),
            "Go BuildPlan should expose the runtime command"
        );
        assert!(
            plan.dockerfile.contains("go build -o main ./cmd/api"),
            "BuildPlan should still carry the rendered Dockerfile contents, got: {}",
            plan.dockerfile
        );
    }
}
