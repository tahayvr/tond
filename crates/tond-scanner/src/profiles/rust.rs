// profiles/rust.rs — Rust detection and Dockerfile generation

use std::path::Path;

use crate::{
    BuildPlan, ScannerError,
    profile::{DetectionOutcome, LanguageProfile},
};

const RUST_BUILDER_IMAGE: &str = "rust:1.94.1-slim-bookworm";
const RUST_RUNTIME_IMAGE: &str = "debian:bookworm-slim";

pub(crate) struct RustProfile;

impl LanguageProfile for RustProfile {
    fn name(&self) -> &'static str {
        "Rust"
    }

    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join("Cargo.toml").exists()
    }

    fn detect_with_confidence(&self, repo_path: &Path) -> Option<DetectionOutcome> {
        repo_path
            .join("Cargo.toml")
            .exists()
            .then_some(DetectionOutcome {
                confidence: 75,
                reasons: vec!["found Cargo.toml in build root".to_string()],
            })
    }

    fn generate_dockerfile(
        &self,
        repo_path: &Path,
        _static_build: bool,
    ) -> Result<String, ScannerError> {
        let binary_name = detect_binary_name(repo_path)?;

        Ok(render_dockerfile(&binary_name))
    }

    fn build_plan(&self, repo_path: &Path, static_build: bool) -> Result<BuildPlan, ScannerError> {
        let binary_name = detect_binary_name(repo_path)?;

        Ok(BuildPlan {
            profile_name: self.name().to_string(),
            stack: "rust".to_string(),
            confidence: 75,
            detection_reasons: vec!["found Cargo.toml in build root".to_string()],
            builder_image: Some(RUST_BUILDER_IMAGE.to_string()),
            runtime_image: Some(RUST_RUNTIME_IMAGE.to_string()),
            run_command: (!static_build).then_some(format!("./{binary_name}")),
            static_output_dir: None,
            dockerfile: render_dockerfile(&binary_name),
        })
    }
}

fn detect_binary_name(repo_path: &Path) -> Result<String, ScannerError> {
    let cargo_toml = std::fs::read_to_string(repo_path.join("Cargo.toml"))?;

    let package_section = cargo_toml
        .split("[package]")
        .nth(1)
        .unwrap_or(cargo_toml.as_str());

    for line in package_section.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name")
            && let Some((_, value)) = rest.split_once('=')
        {
            let name = value.trim().trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Ok(name.to_string());
            }
        }
    }

    Err(ScannerError::UnsupportedFramework(
        "Rust project is missing a package.name in Cargo.toml".to_string(),
    ))
}

fn render_dockerfile(binary_name: &str) -> String {
    format!(
        "\
# Stage 1: Build
FROM {RUST_BUILDER_IMAGE} AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release

# Stage 2: Production
FROM {RUST_RUNTIME_IMAGE}
WORKDIR /app
COPY --from=builder /app/target/release/{binary_name} ./{binary_name}
EXPOSE 8080
CMD [\"./{binary_name}\"]
"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::profile::LanguageProfile;

    use super::{RUST_BUILDER_IMAGE, RUST_RUNTIME_IMAGE, RustProfile};

    fn create_repo() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temporary repo: {err}"))
    }

    #[test]
    fn detect_returns_true_when_cargo_toml_exists() {
        let repo = create_repo();
        fs::write(
            repo.path().join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write Cargo.toml: {err}"));

        let detected = RustProfile.detect(repo.path());

        assert!(
            detected,
            "RustProfile should detect repositories that contain a Cargo.toml file"
        );
    }

    #[test]
    fn build_plan_captures_rust_detection_details() {
        let repo = create_repo();
        fs::create_dir(repo.path().join("src"))
            .unwrap_or_else(|err| panic!("failed to create src directory: {err}"));
        fs::write(
            repo.path().join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write Cargo.toml: {err}"));
        fs::write(repo.path().join("src/main.rs"), "fn main() {}\n")
            .unwrap_or_else(|err| panic!("failed to write src/main.rs: {err}"));

        let plan = RustProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Rust BuildPlan generation to succeed: {err}"));

        assert_eq!(
            plan.stack, "rust",
            "Rust BuildPlan should record a stable stack identifier"
        );
        assert_eq!(
            plan.builder_image.as_deref(),
            Some(RUST_BUILDER_IMAGE),
            "Rust BuildPlan should capture the builder image"
        );
        assert_eq!(
            plan.runtime_image.as_deref(),
            Some(RUST_RUNTIME_IMAGE),
            "Rust BuildPlan should capture the runtime image"
        );
        assert_eq!(
            plan.run_command.as_deref(),
            Some("./demo"),
            "Rust BuildPlan should expose the inferred binary command"
        );
    }

    #[test]
    fn generate_dockerfile_uses_release_binary_name_from_package_manifest() {
        let repo = create_repo();
        fs::create_dir(repo.path().join("src"))
            .unwrap_or_else(|err| panic!("failed to create src directory: {err}"));
        fs::write(
            repo.path().join("Cargo.toml"),
            "[package]\nname='demo-api'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write Cargo.toml: {err}"));
        fs::write(repo.path().join("src/main.rs"), "fn main() {}\n")
            .unwrap_or_else(|err| panic!("failed to write src/main.rs: {err}"));

        let dockerfile = RustProfile
            .generate_dockerfile(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Rust Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("RUN cargo build --release"),
            "Rust Dockerfiles should build release binaries, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY --from=builder /app/target/release/demo-api ./demo-api"),
            "Rust Dockerfiles should copy the named release binary from the builder stage, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("CMD [\"./demo-api\"]"),
            "Rust Dockerfiles should execute the named binary, got: {dockerfile}"
        );
    }
}
