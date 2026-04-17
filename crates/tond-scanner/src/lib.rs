// tond-scanner — lightweight framework detector and Dockerfile generator
//
// This crate scans a
// repository root for well-known files, detecting the framework, and emitting
// a pre-baked, highly optimised multi-stage Dockerfile string.
//
// Architecture
// ────────────
// Each supported tech stack is encapsulated in a `LanguageProfile` implementor
// living under `src/profiles/`.  Detection and Dockerfile generation are both
// part of the same struct, so adding a new language requires only:
//   1. Create `src/profiles/<lang>.rs` and implement `LanguageProfile`.
//   2. Register the struct in `src/profiles.rs`.
//
// Public surface
// ──────────────
//   pub mod profile          — the `LanguageProfile` trait (implement to add a
//                               new tech stack)
//   scan_and_write()         — detect framework + generate + write Dockerfile
//                               and .dockerignore to disk
//   ScannerError             — error type (UnsupportedFramework | Io)

pub mod profile;
mod profiles;

use std::path::Path;

use thiserror::Error;

/// A typed summary of the selected build strategy for a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub profile_name: String,
    pub stack: String,
    pub confidence: u8,
    pub detection_reasons: Vec<String>,
    pub builder_image: Option<String>,
    pub runtime_image: Option<String>,
    pub run_command: Option<String>,
    pub static_output_dir: Option<String>,
    pub dockerfile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionMatch {
    pub profile_name: String,
    pub stack: String,
    pub confidence: u8,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplainOutput {
    pub repo_root: String,
    pub build_root: String,
    pub static_build: bool,
    pub build_plan: Option<BuildPlan>,
    pub matches: Vec<DetectionMatch>,
    pub failure_reason: Option<String>,
}

/// Errors that can be returned by the scanner.
#[derive(Error, Debug)]
pub enum ScannerError {
    /// The repository uses a framework that tond-scanner does not recognise
    /// and no existing Dockerfile was found.
    #[error("{0}")]
    UnsupportedFramework(String),

    /// A filesystem I/O error occurred while writing the generated file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Multiple profiles matched with the same confidence and TBuild cannot
    /// choose deterministically between them.
    #[error("{0}")]
    AmbiguousDetection(String),
}

/// Scan `repo_path`, generate a `Dockerfile` from the detected framework, and
/// write it (plus a `.dockerignore`) to the repository root.
///
/// An existing `.dockerignore` is **never overwritten** — if one already exists
/// it is left untouched.  The `Dockerfile` is always (re-)written so that
/// incremental deploys pick up framework changes.
///
/// When `static_build` is `true` the generated Dockerfile will serve the
/// compiled output as a static site rather than launching a server process.
///
/// # Examples
///
/// ```
/// # use std::fs;
/// # use std::time::{SystemTime, UNIX_EPOCH};
/// use tond_scanner::scan_and_write;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
/// let repo_path = std::env::temp_dir().join(format!("tond-scanner-doc-{unique}"));
/// fs::create_dir_all(&repo_path)?;
///
/// fs::write(
///     repo_path.join("package.json"),
///     r#"{ "name": "demo", "scripts": { "start": "node server.js" } }"#,
/// )?;
///
/// scan_and_write(&repo_path, false)?;
///
/// let dockerfile = fs::read_to_string(repo_path.join("Dockerfile"))?;
/// assert!(dockerfile.contains("CMD [\"npm\", \"start\"]"));
/// assert!(repo_path.join(".dockerignore").exists());
///
/// fs::remove_dir_all(&repo_path)?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// - [`ScannerError::UnsupportedFramework`] — no recognisable framework found.
/// - [`ScannerError::Io`] — could not write the file to disk.
pub fn scan(repo_path: &Path, static_build: bool) -> Result<BuildPlan, ScannerError> {
    scan_in_dir(repo_path, repo_path, static_build)
}

pub fn scan_in_dir(
    repo_root: &Path,
    build_root: &Path,
    static_build: bool,
) -> Result<BuildPlan, ScannerError> {
    let explain = explain_in_dir(repo_root, build_root, static_build)?;
    match (explain.build_plan, explain.failure_reason) {
        (Some(plan), _) => Ok(plan),
        (None, Some(reason)) if reason.contains("Ambiguous framework detection") => {
            Err(ScannerError::AmbiguousDetection(reason))
        }
        (None, Some(reason)) => Err(ScannerError::UnsupportedFramework(reason)),
        (None, None) => Err(ScannerError::UnsupportedFramework(
            "TBuild failed before producing a build plan.".to_string(),
        )),
    }
}

pub fn explain(repo_path: &Path, static_build: bool) -> Result<ExplainOutput, ScannerError> {
    explain_in_dir(repo_path, repo_path, static_build)
}

pub fn explain_in_dir(
    repo_root: &Path,
    build_root: &Path,
    static_build: bool,
) -> Result<ExplainOutput, ScannerError> {
    if repo_root == build_root && looks_like_workspace_root(repo_root) {
        let reason = "Detected a monorepo/workspace root. Set base_dir to the app subdirectory you want TBuild to build.";
        return Ok(ExplainOutput {
            repo_root: repo_root.display().to_string(),
            build_root: build_root.display().to_string(),
            static_build,
            build_plan: None,
            matches: vec![],
            failure_reason: Some(reason.to_string()),
        });
    }

    let matches = profiles::detect(build_root);
    let Some(best_match) = matches.first() else {
        return Ok(ExplainOutput {
            repo_root: repo_root.display().to_string(),
            build_root: build_root.display().to_string(),
            static_build,
            build_plan: None,
            matches: vec![],
            failure_reason: Some(
                "Unsupported framework and no Dockerfile provided. TBuild currently supports Node.js, Python, Go, and Rust repositories. Use base_dir for monorepos or provide a Dockerfile for unsupported stacks.".to_string(),
            ),
        });
    };

    if matches.len() > 1 && matches[0].confidence == matches[1].confidence {
        let tied_profiles = matches
            .iter()
            .take_while(|candidate| candidate.confidence == matches[0].confidence)
            .map(|candidate| format!("{} ({})", candidate.profile.name(), candidate.confidence))
            .collect::<Vec<_>>()
            .join(", ");
        let reason = format!(
            "Ambiguous framework detection for {}. TBuild found multiple equally likely profiles: {}. Narrow the build root with base_dir or add an explicit Dockerfile.",
            build_root.display(),
            tied_profiles
        );
        return Ok(ExplainOutput {
            repo_root: repo_root.display().to_string(),
            build_root: build_root.display().to_string(),
            static_build,
            build_plan: None,
            matches: matches
                .into_iter()
                .map(|candidate| DetectionMatch {
                    profile_name: candidate.profile.name().to_string(),
                    stack: candidate.stack,
                    confidence: candidate.confidence,
                    reasons: candidate.reasons,
                })
                .collect(),
            failure_reason: Some(reason),
        });
    }

    let mut plan = best_match
        .profile
        .build_plan_in_dir(repo_root, build_root, static_build)?;
    plan.confidence = best_match.confidence;
    plan.detection_reasons = best_match.reasons.clone();

    tracing::info!(
        "tond-scanner: detected {} for {} with confidence {}",
        best_match.profile.name(),
        build_root.display(),
        best_match.confidence
    );

    Ok(ExplainOutput {
        repo_root: repo_root.display().to_string(),
        build_root: build_root.display().to_string(),
        static_build,
        build_plan: Some(plan),
        matches: matches
            .into_iter()
            .map(|candidate| DetectionMatch {
                profile_name: candidate.profile.name().to_string(),
                stack: candidate.stack,
                confidence: candidate.confidence,
                reasons: candidate.reasons,
            })
            .collect(),
        failure_reason: None,
    })
}

pub fn scan_and_write(repo_path: &Path, static_build: bool) -> Result<(), ScannerError> {
    scan_and_write_in_dir(repo_path, repo_path, static_build)
}

pub fn scan_and_write_in_dir(
    repo_root: &Path,
    build_root: &Path,
    static_build: bool,
) -> Result<(), ScannerError> {
    let plan = scan_in_dir(repo_root, build_root, static_build)?;

    let dockerfile_path = build_root.join("Dockerfile");
    std::fs::write(&dockerfile_path, &plan.dockerfile)?;
    tracing::info!(
        "tond-scanner: wrote Dockerfile to {}",
        dockerfile_path.display()
    );

    // Write a sensible .dockerignore only if the project does not already
    // provide one.  We never overwrite a user-supplied .dockerignore.
    let dockerignore_path = build_root.join(".dockerignore");
    if !dockerignore_path.exists() {
        std::fs::write(&dockerignore_path, DOCKERIGNORE)?;
        tracing::info!(
            "tond-scanner: wrote .dockerignore to {}",
            dockerignore_path.display()
        );
    }

    Ok(())
}

fn looks_like_workspace_root(repo_root: &Path) -> bool {
    repo_root.join("pnpm-workspace.yaml").exists()
        || repo_root.join("turbo.json").exists()
        || cargo_workspace_members(repo_root)
}

fn cargo_workspace_members(repo_root: &Path) -> bool {
    let Ok(cargo_toml) = std::fs::read_to_string(repo_root.join("Cargo.toml")) else {
        return false;
    };

    cargo_toml.contains("[workspace]") && cargo_toml.contains("members")
}

// ── Default .dockerignore ────────────────────────────────────────────────────

/// A broad `.dockerignore` that covers the most common exclusions across all
/// supported tech stacks.  It is written alongside the generated Dockerfile
/// when no user-supplied `.dockerignore` exists.
///
/// Key groups:
///   - VCS metadata (.git) — never needed inside an image
///   - Dependency caches (node_modules, .venv, …) — reinstalled inside the
///     container, so including them bloats the build context for nothing
///   - Build artefacts (dist/, build/, .next/, …) — rebuilt fresh inside the
///     builder stage; a stale local build would silently pollute the context
///   - Secret files (.env*) — must never be baked into a production image
///   - Editor / OS noise (.idea/, .DS_Store, …)
const DOCKERIGNORE: &str = "\
# ── Version control ──────────────────────────────────────────────────────────
.git
.gitignore
.gitattributes
.gitmodules

# ── Docker files ──────────────────────────────────────────────────────────────
.dockerignore
Dockerfile
docker-compose*.yml

# ── Node.js ───────────────────────────────────────────────────────────────────
# Dependencies are installed fresh inside the container.
node_modules
.npm
.pnpm-store
.yarn/cache
.yarn/install-state.gz

# ── Python ────────────────────────────────────────────────────────────────────
__pycache__
*.pyc
*.pyo
*.pyd
.venv
venv
env
.eggs
*.egg-info
dist-info

# ── Environment variables — never bake secrets into images ────────────────────
.env
.env.*
*.env

# ── Build artefacts (rebuilt fresh inside the container) ─────────────────────
dist
build
out
.next
.svelte-kit
.nuxt
.output
.vercel

# ── Go ────────────────────────────────────────────────────────────────────────
vendor/

# ── IDE / editor ──────────────────────────────────────────────────────────────
.idea
.vscode
*.swp
*.swo
.editorconfig

# ── macOS / Windows ───────────────────────────────────────────────────────────
.DS_Store
Thumbs.db
desktop.ini

# ── Logs and temporary files ──────────────────────────────────────────────────
*.log
npm-debug.log*
yarn-debug.log*
yarn-error.log*
pnpm-debug.log*

# ── Test coverage ─────────────────────────────────────────────────────────────
coverage
.nyc_output

# ── Documentation ─────────────────────────────────────────────────────────────
README*
*.md
LICENCE
LICENSE
docs
";

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use tempfile::TempDir;

    use super::*;

    fn create_repo() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temporary repo: {err}"))
    }

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-fixtures")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixture_root().join(name)
    }

    fn read_expected_dockerfile(path: &Path) -> String {
        fs::read_to_string(path.join("Dockerfile.expected")).unwrap_or_else(|err| {
            panic!(
                "failed to read expected Dockerfile snapshot for {}: {err}",
                path.display()
            )
        })
    }

    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap_or_else(|err| {
            panic!(
                "failed to create fixture copy directory {}: {err}",
                dst.display()
            )
        });

        for entry in fs::read_dir(src).unwrap_or_else(|err| {
            panic!("failed to read fixture directory {}: {err}", src.display())
        }) {
            let entry = entry.unwrap_or_else(|err| {
                panic!("failed to read directory entry in {}: {err}", src.display())
            });
            let entry_path = entry.path();
            let destination = dst.join(entry.file_name());

            if entry
                .file_type()
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to inspect directory entry {}: {err}",
                        entry_path.display()
                    )
                })
                .is_dir()
            {
                copy_dir_all(&entry_path, &destination);
            } else {
                fs::copy(&entry_path, &destination).unwrap_or_else(|err| {
                    panic!(
                        "failed to copy fixture file {} to {}: {err}",
                        entry_path.display(),
                        destination.display()
                    )
                });
            }
        }
    }

    fn smoke_target_root(fixture: &str) -> TempDir {
        let temp = create_repo();
        copy_dir_all(&fixture_path(fixture), temp.path());
        temp
    }

    fn unique_smoke_name(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system clock before unix epoch: {err}"))
            .as_nanos();
        format!("tond-scanner-{prefix}-{nanos}")
    }

    fn docker_build(workdir: &Path, tag: &str) {
        let output = std::process::Command::new("docker")
            .args(["build", "-t", tag, "."])
            .current_dir(workdir)
            .output()
            .unwrap_or_else(|err| {
                panic!(
                    "failed to run docker build for {}: {err}",
                    workdir.display()
                )
            });

        assert!(
            output.status.success(),
            "docker build failed for {}\nstdout:\n{}\nstderr:\n{}",
            workdir.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn docker_run(tag: &str, container_name: &str) -> String {
        let output = std::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--rm",
                "--name",
                container_name,
                "-p",
                "127.0.0.1::8080",
                tag,
            ])
            .output()
            .unwrap_or_else(|err| panic!("failed to run docker container for image {tag}: {err}"));

        assert!(
            output.status.success(),
            "docker run failed for image {tag}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn docker_host_port(container_name: &str) -> String {
        let output = std::process::Command::new("docker")
            .args(["port", container_name, "8080/tcp"])
            .output()
            .unwrap_or_else(|err| {
                panic!("failed to inspect docker port mapping for {container_name}: {err}")
            });

        assert!(
            output.status.success(),
            "docker port failed for container {container_name}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let mapping = String::from_utf8_lossy(&output.stdout).trim().to_string();
        mapping
            .rsplit(':')
            .next()
            .unwrap_or_else(|| {
                panic!("unexpected docker port output for {container_name}: {mapping}")
            })
            .to_string()
    }

    fn cleanup_container(name: &str) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", name])
            .output();
    }

    fn cleanup_image(tag: &str) {
        let _ = std::process::Command::new("docker")
            .args(["rmi", "-f", tag])
            .output();
    }

    fn probe_http(port: &str) {
        let url = format!("http://127.0.0.1:{port}/");
        let mut last_error = String::new();
        let probe_port = port
            .parse::<u16>()
            .unwrap_or_else(|err| panic!("invalid probe port {port}: {err}"));

        for _ in 0..30 {
            match TcpStream::connect(("127.0.0.1", probe_port)) {
                Ok(mut stream) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

                    if let Err(err) = stream.write_all(
                        b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
                    ) {
                        last_error = err.to_string();
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }

                    let mut response = String::new();
                    match stream.read_to_string(&mut response) {
                        Ok(_)
                            if response.starts_with("HTTP/1.1 2")
                                || response.starts_with("HTTP/1.1 3")
                                || response.starts_with("HTTP/1.0 2")
                                || response.starts_with("HTTP/1.0 3") =>
                        {
                            return;
                        }
                        Ok(_) => {
                            let status_line = response.lines().next().unwrap_or("<empty response>");
                            last_error = format!("unexpected response from {url}: {status_line}");
                        }
                        Err(err) => {
                            last_error = err.to_string();
                        }
                    }
                }
                Err(err) => {
                    last_error = err.to_string();
                }
            }

            thread::sleep(Duration::from_millis(500));
        }

        panic!("HTTP probe failed for {url}: {last_error}");
    }

    fn smoke_build_fixture(fixture: &str, static_build: bool) {
        let repo = smoke_target_root(fixture);
        scan_and_write(repo.path(), static_build).unwrap_or_else(|err| {
            panic!("expected scan_and_write to succeed for smoke fixture {fixture}: {err}")
        });

        let image_tag = unique_smoke_name(fixture);
        let container_name = unique_smoke_name(&format!("{fixture}-container"));

        docker_build(repo.path(), &image_tag);
        docker_run(&image_tag, &container_name);

        let probe_result = std::panic::catch_unwind(|| {
            let port = docker_host_port(&container_name);
            probe_http(&port);
        });

        cleanup_container(&container_name);
        cleanup_image(&image_tag);

        if let Err(panic_payload) = probe_result {
            std::panic::resume_unwind(panic_payload);
        }
    }

    #[test]
    fn scan_and_write_unknown_repository_returns_unsupported_framework() {
        let repo = create_repo();

        let result = scan_and_write(repo.path(), false);

        match result {
            Err(ScannerError::UnsupportedFramework(message)) => {
                assert_eq!(
                    message,
                    "Unsupported framework and no Dockerfile provided. TBuild currently supports Node.js, Python, Go, and Rust repositories. Use base_dir for monorepos or provide a Dockerfile for unsupported stacks.",
                    "unknown repositories should return the documented unsupported framework message"
                );
            }
            other => panic!("expected UnsupportedFramework error, got {other:?}"),
        }
    }

    #[test]
    fn scan_and_write_detected_project_writes_dockerfile_and_default_dockerignore() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "name": "demo", "scripts": { "start": "node server.js" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(repo.path().join("server.js"), "console.log('hi')\n")
            .unwrap_or_else(|err| panic!("failed to write server.js: {err}"));

        let result = scan_and_write(repo.path(), false);

        assert!(
            result.is_ok(),
            "expected scan_and_write to succeed for a Node.js project, got {result:?}"
        );

        let dockerfile = fs::read_to_string(repo.path().join("Dockerfile"))
            .unwrap_or_else(|err| panic!("failed to read generated Dockerfile: {err}"));
        assert!(
            dockerfile.contains("FROM node:22.20.0-bookworm-slim AS builder"),
            "generated Dockerfile should use the default Node.js builder image, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("CMD [\"npm\", \"start\"]"),
            "projects with a start script should run via npm start, got: {dockerfile}"
        );

        let dockerignore = fs::read_to_string(repo.path().join(".dockerignore"))
            .unwrap_or_else(|err| panic!("failed to read generated .dockerignore: {err}"));
        assert_eq!(
            dockerignore, DOCKERIGNORE,
            "scan_and_write should write the bundled default .dockerignore when none exists"
        );
    }

    #[test]
    fn scan_and_write_preserves_existing_dockerignore_contents() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "name": "demo", "scripts": { "start": "node server.js" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(repo.path().join("server.js"), "console.log('hi')\n")
            .unwrap_or_else(|err| panic!("failed to write server.js: {err}"));
        fs::write(repo.path().join(".dockerignore"), "custom-ignore\n")
            .unwrap_or_else(|err| panic!("failed to seed existing .dockerignore: {err}"));

        let result = scan_and_write(repo.path(), false);

        assert!(
            result.is_ok(),
            "expected scan_and_write to succeed when a .dockerignore already exists, got {result:?}"
        );

        let dockerignore = fs::read_to_string(repo.path().join(".dockerignore"))
            .unwrap_or_else(|err| panic!("failed to read existing .dockerignore: {err}"));
        assert_eq!(
            dockerignore, "custom-ignore\n",
            "scan_and_write should not overwrite a user-supplied .dockerignore"
        );
    }

    #[test]
    fn scan_returns_build_plan_for_detected_project() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "name": "demo", "scripts": { "start": "node server.js" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let plan = scan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected scan to succeed for a Node.js project: {err}"));

        assert_eq!(
            plan.profile_name, "Node.js",
            "scan should record the detected profile name in the BuildPlan"
        );
        assert_eq!(
            plan.confidence, 70,
            "scan should record the selected detection confidence in the BuildPlan"
        );
        assert!(
            plan.detection_reasons
                .iter()
                .any(|reason| reason.contains("package.json")),
            "scan should retain human-readable detection reasons in the BuildPlan"
        );
        assert!(
            plan.dockerfile.contains("CMD [\"npm\", \"start\"]"),
            "BuildPlan should include the rendered Dockerfile contents, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn explain_reports_candidates_and_selected_plan() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "name": "demo", "scripts": { "start": "node server.js" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let output = explain(repo.path(), false).unwrap_or_else(|err| {
            panic!("expected explain to succeed for a Node.js project: {err}")
        });

        assert!(
            output.failure_reason.is_none(),
            "explain should not report a failure for a supported project"
        );
        assert_eq!(
            output.matches[0].profile_name, "Node.js",
            "explain should list the selected profile as the highest-confidence candidate"
        );
        assert_eq!(
            output
                .build_plan
                .as_ref()
                .unwrap_or_else(|| panic!("expected explain to include a build plan"))
                .stack,
            "node",
            "explain should include the selected build plan"
        );
    }

    #[test]
    fn explain_reports_actionable_failure_for_unknown_repositories() {
        let repo = create_repo();

        let output = explain(repo.path(), false).unwrap_or_else(|err| {
            panic!("expected explain to succeed with a failure summary: {err}")
        });

        assert!(
            output.build_plan.is_none(),
            "explain should not produce a build plan for unsupported repositories"
        );
        assert!(
            output
                .failure_reason
                .as_deref()
                .unwrap_or("")
                .contains("Node.js, Python, Go, and Rust"),
            "explain should report the supported stacks in unsupported-framework failures"
        );
    }

    #[test]
    fn scan_returns_build_plan_for_plain_static_site_without_package_manager() {
        let repo = create_repo();
        fs::write(
            repo.path().join("index.html"),
            "<!doctype html><html><body><h1>Hello</h1></body></html>\n",
        )
        .unwrap_or_else(|err| panic!("failed to write index.html: {err}"));
        fs::create_dir_all(repo.path().join("assets"))
            .unwrap_or_else(|err| panic!("failed to create assets directory: {err}"));
        fs::write(
            repo.path().join("assets").join("app.js"),
            "console.log('hi')\n",
        )
        .unwrap_or_else(|err| panic!("failed to write assets/app.js: {err}"));

        let plan = scan(repo.path(), true).unwrap_or_else(|err| {
            panic!("expected scan to succeed for a plain static site: {err}")
        });

        assert_eq!(
            plan.profile_name, "Static HTML",
            "plain static sites should be detected by the dedicated static-site profile"
        );
        assert_eq!(
            plan.stack, "static-html",
            "plain static sites should use a stable stack identifier"
        );
        assert_eq!(
            plan.static_output_dir.as_deref(),
            Some("."),
            "plain static sites should serve the repository root directly"
        );
        assert!(
            plan.dockerfile.contains("COPY . ./"),
            "plain static site Dockerfiles should copy the repository contents directly, got: {}",
            plan.dockerfile
        );
        assert!(
            plan.dockerfile
                .contains("CMD [\"npx\", \"--yes\", \"serve\", \"-s\", \".\", \"-l\", \"8080\"]"),
            "plain static site Dockerfiles should serve the repository root, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn fixture_dockerfiles_match_golden_outputs() {
        let cases = [
            ("plain-static-html", true),
            ("node-vite-static", true),
            ("python-fastapi", false),
            ("python-django", false),
            ("go-cmd-layout", false),
            ("rust-axum", false),
        ];

        for (fixture, static_build) in cases {
            let path = fixture_path(fixture);
            let expected = read_expected_dockerfile(&path);
            let plan = scan(&path, static_build).unwrap_or_else(|err| {
                panic!("expected fixture {fixture} to produce a BuildPlan: {err}")
            });

            assert_eq!(
                plan.dockerfile, expected,
                "generated Dockerfile for fixture {fixture} should match the committed golden output"
            );
        }
    }

    #[test]
    fn highest_value_fixtures_build_and_serve_http() {
        let cases = [
            ("plain-static-html", true),
            ("node-vite-static", true),
            ("python-fastapi", false),
            ("go-cmd-layout", false),
            ("rust-axum", false),
        ];

        for (fixture, static_build) in cases {
            smoke_build_fixture(fixture, static_build);
        }
    }

    #[test]
    fn scan_in_dir_uses_build_root_inside_monorepo_fixture() {
        let repo_root = fixture_path("node-pnpm-workspace-app");
        let build_root = repo_root.join("apps/web");

        let plan = scan_in_dir(&repo_root, &build_root, true).unwrap_or_else(|err| {
            panic!("expected monorepo build root to produce a BuildPlan: {err}")
        });

        assert_eq!(
            plan.profile_name, "Node.js",
            "scan_in_dir should detect the app inside the selected build root"
        );
        assert!(
            plan.dockerfile.contains("RUN corepack enable pnpm"),
            "monorepo build roots should still detect workspace-level pnpm usage when available, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn scan_rejects_workspace_root_without_explicit_build_root() {
        let repo_root = fixture_path("node-pnpm-workspace-app");

        let result = scan(&repo_root, true);

        match result {
            Err(ScannerError::UnsupportedFramework(message)) => {
                assert_eq!(
                    message,
                    "Detected a monorepo/workspace root. Set base_dir to the app subdirectory you want TBuild to build.",
                    "workspace roots should fail clearly instead of silently picking the wrong app"
                );
            }
            other => panic!("expected workspace root scan to be rejected, got {other:?}"),
        }
    }

    #[test]
    fn scan_and_write_in_dir_writes_dockerfile_into_selected_build_root() {
        let repo = create_repo();
        let apps_dir = repo.path().join("apps").join("web");
        fs::create_dir_all(&apps_dir)
            .unwrap_or_else(|err| panic!("failed to create monorepo app directory: {err}"));
        fs::write(
            repo.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pnpm-lock.yaml: {err}"));
        fs::write(
            apps_dir.join("package.json"),
            r#"{ "name": "web", "scripts": { "start": "node server.js" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write app package.json: {err}"));
        fs::write(apps_dir.join("server.js"), "console.log('hi')\n")
            .unwrap_or_else(|err| panic!("failed to write app server.js: {err}"));

        scan_and_write_in_dir(repo.path(), &apps_dir, false).unwrap_or_else(|err| {
            panic!("expected scan_and_write_in_dir to succeed for a nested app: {err}")
        });

        assert!(
            apps_dir.join("Dockerfile").exists(),
            "scan_and_write_in_dir should write the generated Dockerfile into the selected build root"
        );
        assert!(
            apps_dir.join(".dockerignore").exists(),
            "scan_and_write_in_dir should write .dockerignore into the selected build root"
        );
    }
}
