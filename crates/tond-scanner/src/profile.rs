// profile.rs — LanguageProfile trait
//
// Each supported tech stack implements this trait. To add support for a new
// language or runtime, create a new file under `src/profiles/`, implement
// `LanguageProfile`, and register it in `profiles.rs`.

use std::path::Path;

use crate::{BuildPlan, ScannerError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionOutcome {
    pub confidence: u8,
    pub reasons: Vec<String>,
}

/// A self-contained description of a supported tech stack.
///
/// Implementors handle both *detection* (does this repo use this stack?) and
/// *generation* (what Dockerfile should be emitted for it?). Keeping both
/// concerns in one place means adding a new language is a single, contained
/// change with no cross-cutting edits required.
pub trait LanguageProfile: Send + Sync {
    /// Human-readable name used in log messages (e.g. `"Node.js"`, `"Python"`).
    fn name(&self) -> &'static str;

    /// Return `true` if `repo_path` appears to contain a project for this stack.
    ///
    /// Detection should be fast (file-existence checks only — no file reads).
    /// Profiles are probed in priority order; the first match wins.
    fn detect(&self, repo_path: &Path) -> bool;

    /// Return an explicit detection outcome with confidence and reasons.
    ///
    /// The default implementation preserves the legacy boolean detection model,
    /// assigning a baseline confidence when `detect()` returns true.
    fn detect_with_confidence(&self, repo_path: &Path) -> Option<DetectionOutcome> {
        self.detect(repo_path).then_some(DetectionOutcome {
            confidence: 50,
            reasons: vec![format!("matched {} detection markers", self.name())],
        })
    }

    /// Produce a typed build plan for the project.
    ///
    /// The default implementation preserves the pre-BuildPlan behavior by
    /// delegating to `generate_dockerfile()` and wrapping the result in a
    /// minimal plan. Profiles can override this to expose richer metadata.
    fn build_plan(&self, repo_path: &Path, static_build: bool) -> Result<BuildPlan, ScannerError> {
        self.build_plan_in_dir(repo_path, repo_path, static_build)
    }

    /// Produce a typed build plan for a selected build root inside a repository.
    ///
    /// `repo_root` is the cloned or uploaded repository root. `build_root` is
    /// the directory TBuild should treat as the application root, which may be a
    /// nested subdirectory when `base_dir` is used.
    fn build_plan_in_dir(
        &self,
        repo_root: &Path,
        build_root: &Path,
        static_build: bool,
    ) -> Result<BuildPlan, ScannerError> {
        Ok(BuildPlan {
            profile_name: self.name().to_string(),
            stack: self.name().to_ascii_lowercase().replace(['.', ' '], "-"),
            confidence: 50,
            detection_reasons: vec![format!("matched {} detection markers", self.name())],
            builder_image: None,
            runtime_image: None,
            run_command: None,
            static_output_dir: None,
            dockerfile: self.generate_dockerfile_in_dir(repo_root, build_root, static_build)?,
        })
    }

    /// Emit an optimised, multi-stage Dockerfile for the project at `repo_path`.
    ///
    /// `static_build` — when `true`, the generated image should serve compiled
    /// output as a static site rather than launching a long-running server
    /// process.
    ///
    /// # Errors
    ///
    /// Returns [`ScannerError::UnsupportedFramework`] if detection succeeded
    /// but the project layout makes Dockerfile generation impossible (e.g. no
    /// recognisable entry point).
    fn generate_dockerfile(
        &self,
        repo_path: &Path,
        static_build: bool,
    ) -> Result<String, ScannerError> {
        self.generate_dockerfile_in_dir(repo_path, repo_path, static_build)
    }

    /// Emit an optimised, multi-stage Dockerfile for a selected build root.
    fn generate_dockerfile_in_dir(
        &self,
        _repo_root: &Path,
        build_root: &Path,
        static_build: bool,
    ) -> Result<String, ScannerError> {
        self.generate_dockerfile(build_root, static_build)
    }
}
