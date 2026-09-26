// deploy/build.rs — image build helpers
//
// Provides everything needed to turn source code into a Docker image:
//   build_from_dir_with_method() is also the entry point for tarball uploads
//   build_from_git()             — clone a git repo then call build_from_dir_with_method()
//   build_from_dir_with_method() — dispatch to the right builder based on build_method
//   build_with_dockerfile()      — run `docker build` with streaming log output
//   build_from_inline_dockerfile() — write inline Dockerfile then call build_with_dockerfile()
//   clone_repo()                 — clone a repository into the build directory
//   build_compose_service()      — build one compose service (build cgroup parent)
//   compose_up()                 — `docker compose up -d` on a pinned compose file
//
// All builder functions write timestamped log entries via super::core::log_deploy().

use anyhow::Result;
use tond_common::DeployPhase;

use crate::state::AppState;

use super::progress;

use super::core::log_deploy;
use super::git::{
    COMMIT_LOG_ARGS, CommitInfo, GIT_ALLOW_PROTOCOL, GitHttpAuth, clone_args, ensure_known_hosts,
    git_auth_env, known_hosts_path, parse_commit_log, redact_url_credentials, ssh_command,
};

/// The commit checked out in `dir` (best-effort).
async fn read_commit(dir: &str) -> Option<CommitInfo> {
    let output = crate::process::command("git")
        .args(COMMIT_LOG_ARGS)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_commit_log(&String::from_utf8_lossy(&output.stdout))
}

fn build_work_dir(data_dir: &str, app_name: &str) -> String {
    format!("{}/builds/{}", data_dir, app_name)
}

fn deploy_key_path(data_dir: &str, app_id: &str) -> String {
    format!("{}/keys/{}/id_ed25519", data_dir, app_id)
}

pub(crate) fn compose_work_dir(data_dir: &str, app_name: &str) -> String {
    format!("{}/compose/{}", data_dir, app_name)
}

pub(crate) fn compose_file_path(work_dir: &str) -> String {
    format!("{}/docker-compose.yml", work_dir)
}

/// `<data>/compose/<app>`: where an app's pinned compose file lives.
pub fn compose_work_dir_for(app_name: &str) -> String {
    compose_work_dir(&crate::volumes::data_dir().to_string_lossy(), app_name)
}

/// Variables of tond-server's own environment `docker compose` needs.
const COMPOSE_HOST_ENV: [&str; 5] = [
    "PATH",
    "HOME",
    "DOCKER_HOST",
    "DOCKER_CONFIG",
    "XDG_RUNTIME_DIR",
];

/// The app's environment variables `docker compose` may interpolate: all of
/// them except names that steer docker itself (PATH, DOCKER_*, COMPOSE_*).
pub fn compose_app_env(
    env: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    env.iter()
        .filter(|(k, _)| {
            !COMPOSE_HOST_ENV.contains(&k.as_str())
                && !k.starts_with("DOCKER_")
                && !k.starts_with("COMPOSE_")
                && !k.starts_with("BUILDKIT_")
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// `docker compose -p <app> -f <file> <args…>` with a cleared environment:
/// compose interpolates `${VAR}` from its own environment, and tond-server's
/// holds TOND_SECRET_KEY. Only what docker needs is passed on, plus the
/// app's own variables (`app_env`, see [`compose_app_env`]).
pub fn compose_command(
    compose_path: &str,
    app_name: &str,
    args: &[&str],
    app_env: &std::collections::BTreeMap<String, String>,
) -> tokio::process::Command {
    let mut cmd = crate::process::command("docker");
    cmd.env_clear();
    cmd.envs(compose_app_env(app_env));
    for key in COMPOSE_HOST_ENV {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    cmd.args([
        "compose",
        "-p",
        &compose_project_name(app_name),
        "-f",
        compose_path,
    ]);
    cmd.args(args);
    cmd
}

/// Compose project names must be lowercase; this is also the label value
/// compose labels its containers with (`com.docker.compose.project`).
pub fn compose_project_name(app_name: &str) -> String {
    app_name.to_ascii_lowercase()
}

/// Default cgroup parent for build containers: a systemd slice that
/// install.sh caps at MemoryMax=60% (see release/tondbuild.slice).
pub const DEFAULT_BUILD_CGROUP_PARENT: &str = "tondbuild.slice";

/// Cgroup parent for `docker build` RUN steps: TOND_BUILD_CGROUP_PARENT, else
/// DEFAULT_BUILD_CGROUP_PARENT; an empty value disables it.
///
/// Why a cgroup parent: BuildKit (the default builder behind `docker build`)
/// ignores the classic builder's --memory flag, but honours --cgroup-parent
/// for the containers that run RUN steps (the docker driver). With Docker's
/// systemd cgroup driver the value names a slice, so every build step lands
/// in tondbuild.slice and the slice's MemoryMax bounds all builds together.
/// If the slice unit is missing, systemd creates it with no limit, so the flag
/// is always safe to pass.
fn build_cgroup_parent() -> Option<String> {
    match std::env::var("TOND_BUILD_CGROUP_PARENT") {
        Ok(v) if v.trim().is_empty() => None,
        Ok(v) => Some(v.trim().to_string()),
        Err(_) => Some(DEFAULT_BUILD_CGROUP_PARENT.to_string()),
    }
}

fn docker_build_args<'a>(
    image_tag: &'a str,
    context_dir: &'a str,
    dockerfile_path: Option<&'a str>,
    cgroup_parent: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["build"];
    if let Some(parent) = cgroup_parent {
        args.push("--cgroup-parent");
        args.push(parent);
    }
    if let Some(path) = dockerfile_path {
        args.push("-f");
        args.push(path);
    }
    args.push("-t");
    args.push(image_tag);
    args.push(context_dir);
    args
}

/// Validate an app's `base_dir`: a relative path inside the checkout, without
/// `..`, absolute/root components, backslashes or control characters.
/// Returns the normalised form (no leading `./`, no trailing `/`).
pub fn validate_base_dir(base_dir: &str) -> Result<String> {
    use std::path::Component;

    let trimmed = base_dir.trim();
    if trimmed.len() > 512 {
        anyhow::bail!("base_dir is too long");
    }
    if trimmed.contains('\\') || trimmed.chars().any(char::is_control) {
        anyhow::bail!("base_dir must be a plain relative path");
    }
    let mut parts = Vec::new();
    for component in std::path::Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => anyhow::bail!("base_dir must not contain '..'"),
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("base_dir must be relative to the repository root")
            }
        }
    }
    Ok(parts.join("/"))
}

/// Resolve the build root for `base_dir` inside the checkout at `dir`, and
/// make sure it really is inside it (symlinks included).
fn resolve_build_root(dir: &str, base_dir: Option<&str>) -> Result<std::path::PathBuf> {
    let root = std::path::Path::new(dir);
    let base = match base_dir {
        Some(base) => validate_base_dir(base)?,
        None => String::new(),
    };
    let build_root = if base.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&base)
    };

    if !build_root.exists() {
        anyhow::bail!("base_dir '{}' does not exist inside the repository", base);
    }

    let canonical_root = root.canonicalize()?;
    let canonical_build_root = build_root.canonicalize()?;
    if !canonical_build_root.starts_with(&canonical_root) {
        anyhow::bail!("base_dir '{}' resolves outside the repository", base);
    }
    if !canonical_build_root.is_dir() {
        anyhow::bail!("base_dir '{}' is not a directory", base);
    }

    Ok(build_root)
}

async fn log_build_plan_metadata(
    state: &AppState,
    app_id: &str,
    trigger_source: &str,
    plan: &tond_scanner::BuildPlan,
) -> Result<()> {
    log_deploy(
        state,
        app_id,
        "info",
        trigger_source,
        &format!(
            "TBuild detection: stack={} profile={} confidence={}",
            plan.stack, plan.profile_name, plan.confidence
        ),
    )
    .await?;

    if !plan.detection_reasons.is_empty() {
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!("TBuild reasons: {}", plan.detection_reasons.join("; ")),
        )
        .await?;
    }

    if let Some(builder_image) = &plan.builder_image {
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!("TBuild builder image: {}", builder_image),
        )
        .await?;
    }

    if let Some(runtime_image) = &plan.runtime_image {
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!("TBuild runtime image: {}", runtime_image),
        )
        .await?;
    }

    if let Some(run_command) = &plan.run_command {
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!("TBuild run command: {}", run_command),
        )
        .await?;
    }

    if let Some(static_output_dir) = &plan.static_output_dir {
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!("TBuild static output dir: {}", static_output_dir),
        )
        .await?;
    }

    Ok(())
}

/// Options that control how source code is built into a container image.
pub(super) struct BuildOptions<'a> {
    pub method: &'a str,
    pub static_build: bool,
    pub base_dir: Option<&'a str>,
}

/// Clone a git repository and build a Docker image from it.
///
/// `build_opts.method` controls which builder is used:
///   "dockerfile" — always use the `Dockerfile` at the repo root.
///   "tbuild"     — run tond-scanner to auto-generate a Dockerfile, then
///                  build with Docker.
///
/// If a deploy key exists on disk at `$TOND_DATA_DIR/keys/<app_id>/id_ed25519`,
/// it is injected via `GIT_SSH_COMMAND` (with a per-app known_hosts file) to
/// enable cloning private repositories over SSH. HTTPS credentials in `auth`
/// are passed as a header via git's environment config — see deploy/git.rs.
/// `git_url` must not contain credentials.
#[allow(clippy::too_many_arguments)]
pub(super) async fn build_from_git(
    state: &AppState,
    app_id: &str,
    app_name: &str,
    git_url: &str,
    auth: Option<&GitHttpAuth>,
    branch: &str,
    build_opts: &BuildOptions<'_>,
    image_tag: &str,
    deployment_id: Option<&str>,
    trigger_source: &str,
) -> Result<String> {
    let work_dir = clone_repo(
        state,
        app_id,
        app_name,
        git_url,
        auth,
        branch,
        deployment_id,
        trigger_source,
    )
    .await?;

    if deployment_id.is_some() {
        progress::start(state, app_id, DeployPhase::Build).await;
    }

    let image_tag = build_from_dir_with_method(
        state,
        app_id,
        image_tag,
        &work_dir,
        build_opts,
        deployment_id,
        trigger_source,
    )
    .await?;

    // Clean up
    let _ = tokio::fs::remove_dir_all(&work_dir).await;

    Ok(image_tag)
}

/// Clone `git_url` at `branch` into the app's build directory (replacing a
/// previous checkout), record the commit on the deployment and drop `.git`.
/// Returns the checkout directory; the caller removes it when done.
#[allow(clippy::too_many_arguments)]
pub(super) async fn clone_repo(
    state: &AppState,
    app_id: &str,
    app_name: &str,
    git_url: &str,
    auth: Option<&GitHttpAuth>,
    branch: &str,
    deployment_id: Option<&str>,
    trigger_source: &str,
) -> Result<String> {
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let work_dir = build_work_dir(&data_dir, app_name);

    // Clean up any previous build
    let _ = tokio::fs::remove_dir_all(&work_dir).await;
    tokio::fs::create_dir_all(&work_dir).await?;

    // Resolve deploy key path. If the file exists we inject it via
    // GIT_SSH_COMMAND so private SSH repos can be cloned without touching
    // the global ~/.ssh config.
    let key_path = deploy_key_path(&data_dir, app_id);
    let key_exists = tokio::fs::metadata(&key_path).await.is_ok();

    // Clone the repo (the caller has started the clone phase)
    log_deploy(
        state,
        app_id,
        "info",
        trigger_source,
        &format!("Cloning {} ({})", git_url, branch),
    )
    .await?;

    let mut cmd = crate::process::command("git");
    cmd.args(clone_args(branch, git_url, &work_dir))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", GIT_ALLOW_PROTOCOL);

    if let Some(auth) = auth {
        cmd.envs(git_auth_env(git_url, auth));
    }

    if key_exists {
        let known_hosts = known_hosts_path(&data_dir, app_id);
        ensure_known_hosts(&known_hosts).await?;
        cmd.env("GIT_SSH_COMMAND", ssh_command(&key_path, &known_hosts));
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!("Using deploy key: {}", key_path),
        )
        .await?;
    }

    let clone_output = cmd.output().await?;

    if !clone_output.status.success() {
        let stderr = String::from_utf8_lossy(&clone_output.stderr);
        anyhow::bail!(
            "Git clone failed: {}",
            redact_url_credentials(stderr.trim())
        );
    }

    // Record which commit this deployment builds (before .git goes away).
    if let Some(deployment_id) = deployment_id
        && let Some(commit) = read_commit(&work_dir).await
    {
        log_deploy(
            state,
            app_id,
            "info",
            trigger_source,
            &format!(
                "Commit {} by {}: {}",
                &commit.sha[..commit.sha.len().min(7)],
                commit.author,
                commit.message
            ),
        )
        .await?;
        progress::record_commit(state, app_id, deployment_id, &commit).await;
    }

    // Drop the repository metadata before building: .git/config and history
    // must never end up in the build context or an image.
    tokio::fs::remove_dir_all(std::path::Path::new(&work_dir).join(".git")).await?;

    Ok(work_dir)
}

/// Build from a local directory using the appropriate build method and tag
/// the result `image_tag`.
///
/// `build_opts.method` controls which builder is used (same semantics as above).
pub(super) async fn build_from_dir_with_method(
    state: &AppState,
    app_id: &str,
    image_tag: &str,
    dir: &str,
    build_opts: &BuildOptions<'_>,
    deployment_id: Option<&str>,
    trigger_source: &str,
) -> Result<String> {
    let build_root = resolve_build_root(dir, build_opts.base_dir)?;

    let dockerfile_path = build_root.join("Dockerfile");
    let has_dockerfile = tokio::fs::metadata(&dockerfile_path).await.is_ok();

    match build_opts.method {
        "dockerfile" => {
            if !has_dockerfile {
                anyhow::bail!(
                    "build_method is 'dockerfile' but no Dockerfile was found in the repo root"
                );
            }
            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                "Building with Dockerfile",
            )
            .await?;
            build_with_dockerfile(
                state,
                app_id,
                image_tag,
                build_root
                    .to_str()
                    .expect("build root path should be valid UTF-8"),
                Some(dir),
                trigger_source,
            )
            .await?;
        }

        "tbuild" => {
            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                "Building with TBuild (tond-scanner)",
            )
            .await?;
            let repo_path = std::path::Path::new(dir);
            match tond_scanner::explain_in_dir(repo_path, &build_root, build_opts.static_build) {
                Ok(output) => {
                    if let Some(plan) = output.build_plan {
                        log_build_plan_metadata(state, app_id, trigger_source, &plan).await?;
                        tokio::fs::write(build_root.join("Dockerfile"), &plan.dockerfile).await?;
                        if let Some(deployment_id) = deployment_id {
                            let _ = sqlx::query(
                                "UPDATE deployments SET generated_dockerfile = ? WHERE id = ?",
                            )
                            .bind(&plan.dockerfile)
                            .bind(deployment_id)
                            .execute(&state.db)
                            .await;
                        }
                    } else {
                        let msg = format!(
                            "TBuild failed: {}",
                            output.failure_reason.unwrap_or_else(|| {
                                "failed before a build plan could be generated".to_string()
                            })
                        );
                        log_deploy(state, app_id, "error", trigger_source, &msg).await?;
                        anyhow::bail!(msg);
                    }
                }
                Err(e) => {
                    let msg = format!("TBuild failed: {}", e);
                    log_deploy(state, app_id, "error", trigger_source, &msg).await?;
                    anyhow::bail!(msg);
                }
            }
            log_deploy(
                state,
                app_id,
                "info",
                trigger_source,
                "TBuild generated Dockerfile — building with Docker",
            )
            .await?;
            build_with_dockerfile(
                state,
                app_id,
                image_tag,
                build_root
                    .to_str()
                    .expect("build root path should be valid UTF-8"),
                Some(dir),
                trigger_source,
            )
            .await?;
        }

        _ => {
            anyhow::bail!(
                "unsupported build_method '{}'; expected 'tbuild' or 'dockerfile'",
                build_opts.method
            );
        }
    }

    Ok(image_tag.to_string())
}

/// Build an image using `docker build` with the Dockerfile at the repo root.
/// Each line of stdout/stderr is written to `deploy_logs` in real time.
pub(super) async fn build_with_dockerfile(
    state: &AppState,
    app_id: &str,
    image_tag: &str,
    dir: &str,
    context_dir: Option<&str>,
    trigger_source: &str,
) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let dockerfile = format!("{dir}/Dockerfile");
    let cgroup_parent = build_cgroup_parent();
    let args = docker_build_args(
        image_tag,
        context_dir.unwrap_or(dir),
        Some(&dockerfile),
        cgroup_parent.as_deref(),
    );

    let mut child = crate::process::command("docker")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let state_stdout = state.clone();
    let app_id_stdout = app_id.to_string();
    let trigger_stdout = trigger_source.to_string();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = log_deploy(
                &state_stdout,
                &app_id_stdout,
                "info",
                &trigger_stdout,
                &line,
            )
            .await;
        }
    });

    let state_stderr = state.clone();
    let app_id_stderr = app_id.to_string();
    let trigger_stderr = trigger_source.to_string();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = log_deploy(
                &state_stderr,
                &app_id_stderr,
                "info",
                &trigger_stderr,
                &line,
            )
            .await;
        }
    });

    let status = child.wait().await?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        anyhow::bail!("Docker build failed (exit code: {})", status);
    }

    tracing::info!("Docker build succeeded: {}", image_tag);
    Ok(image_tag.to_string())
}

/// Build an image from an inline Dockerfile string (no git clone needed).
///
/// Writes the content to a temp directory as `Dockerfile`, then calls
/// `docker build` on that directory.
pub(super) async fn build_from_inline_dockerfile(
    state: &AppState,
    app_id: &str,
    app_name: &str,
    dockerfile_content: &str,
    image_tag: &str,
    trigger_source: &str,
) -> Result<String> {
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let work_dir = build_work_dir(&data_dir, app_name);

    // Clean and recreate the build directory
    let _ = tokio::fs::remove_dir_all(&work_dir).await;
    tokio::fs::create_dir_all(&work_dir).await?;

    // Write the inline Dockerfile
    let dockerfile_path = format!("{}/Dockerfile", work_dir);
    tokio::fs::write(&dockerfile_path, dockerfile_content).await?;

    log_deploy(
        state,
        app_id,
        "info",
        trigger_source,
        "Wrote inline Dockerfile to build directory",
    )
    .await?;

    build_with_dockerfile(state, app_id, image_tag, &work_dir, None, trigger_source).await
}

/// Run `cmd` and write every stdout/stderr line to the deploy log (prefixed
/// with `[prefix] ` when given). Returns the exit status.
pub(super) async fn run_streaming(
    state: &AppState,
    app_id: &str,
    trigger_source: &str,
    prefix: Option<&str>,
    mut cmd: tokio::process::Command,
) -> Result<std::process::ExitStatus> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let forward = |reader: Box<dyn tokio::io::AsyncRead + Unpin + Send>| {
        let state = state.clone();
        let app_id = app_id.to_string();
        let trigger = trigger_source.to_string();
        let prefix = prefix.map(|p| format!("[{p}] "));
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = match &prefix {
                    Some(p) => format!("{p}{line}"),
                    None => line,
                };
                let _ = log_deploy(&state, &app_id, "info", &trigger, &line).await;
            }
        })
    };
    let out_task = forward(Box::new(stdout));
    let err_task = forward(Box::new(stderr));
    let status = child.wait().await?;
    let _ = out_task.await;
    let _ = err_task.await;
    Ok(status)
}

/// `docker build` arguments for one compose service (see compose::BuildSpec):
/// the build cgroup parent (so compose builds share the build memory cap),
/// the Dockerfile, build args and target.
pub(super) fn compose_build_args(
    tag: &str,
    spec: &crate::compose::BuildSpec,
    dockerfile: Option<&str>,
    cgroup_parent: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    if let Some(parent) = cgroup_parent {
        args.push("--cgroup-parent".into());
        args.push(parent.into());
    }
    if let Some(path) = dockerfile {
        args.push("-f".into());
        args.push(path.into());
    }
    for (k, v) in &spec.args {
        args.push("--build-arg".into());
        args.push(format!("{k}={v}"));
    }
    if let Some(target) = &spec.target {
        args.push("--target".into());
        args.push(target.clone());
    }
    args.push("-t".into());
    args.push(tag.into());
    args.push(spec.context.clone());
    args
}

/// Build one compose service's image as `tag` (a pinned
/// work tag, pinned by content afterwards). `scratch_dir` holds inline Dockerfiles.
pub(super) async fn build_compose_service(
    state: &AppState,
    app_id: &str,
    service: &str,
    tag: &str,
    spec: &crate::compose::BuildSpec,
    scratch_dir: &str,
    trigger_source: &str,
) -> Result<()> {
    let local = !spec.context.contains("://") && !spec.context.starts_with("git@");
    let dockerfile = if let Some(inline) = &spec.dockerfile_inline {
        tokio::fs::create_dir_all(scratch_dir).await?;
        let path = format!(
            "{scratch_dir}/Dockerfile.{}",
            crate::compose::repo_component(service)
        );
        tokio::fs::write(&path, inline).await?;
        Some(path)
    } else {
        spec.dockerfile.as_ref().map(|d| {
            if local {
                std::path::Path::new(&spec.context)
                    .join(d)
                    .to_string_lossy()
                    .into_owned()
            } else {
                d.clone()
            }
        })
    };
    if local && tokio::fs::metadata(&spec.context).await.is_err() {
        anyhow::bail!(
            "service \"{service}\": build context {} does not exist",
            spec.context
        );
    }
    let cgroup_parent = build_cgroup_parent();
    let args = compose_build_args(tag, spec, dockerfile.as_deref(), cgroup_parent.as_deref());
    let mut cmd = crate::process::command("docker");
    cmd.args(&args);
    let status = run_streaming(state, app_id, trigger_source, Some(service), cmd).await?;
    if !status.success() {
        anyhow::bail!(
            "Docker build of service \"{service}\" failed (exit code: {})",
            status
        );
    }
    Ok(())
}

/// `docker compose up -d --remove-orphans` on the app's pinned compose file.
/// Every service already has its image (built or pulled and pinned), so
/// nothing is built here.
pub(super) async fn compose_up(
    state: &AppState,
    app_id: &str,
    app_name: &str,
    compose_path: &str,
    app_env: &std::collections::BTreeMap<String, String>,
    trigger_source: &str,
) -> Result<()> {
    let cmd = compose_command(
        compose_path,
        app_name,
        &["up", "-d", "--remove-orphans"],
        app_env,
    );
    let status = run_streaming(state, app_id, trigger_source, None, cmd).await?;
    if !status.success() {
        anyhow::bail!("docker compose up failed (exit code: {})", status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::{API_DEFAULT_VERSION, Docker};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::RwLock;
    use tond_common::ServerSettings;

    use crate::{auth::RateLimiter, state::AppState};

    async fn test_state() -> AppState {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for deploy build tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    #[test]
    fn compose_sees_the_apps_variables_but_not_ones_that_steer_docker() {
        let env = std::collections::BTreeMap::from([
            ("DATABASE_URL".to_string(), "postgres://x".to_string()),
            ("PATH".to_string(), "/evil".to_string()),
            ("DOCKER_HOST".to_string(), "tcp://evil:2375".to_string()),
            ("COMPOSE_FILE".to_string(), "/etc/x.yml".to_string()),
        ]);
        let kept = compose_app_env(&env);
        assert_eq!(kept.keys().collect::<Vec<_>>(), vec!["DATABASE_URL"]);
        let cmd = compose_command("/c/docker-compose.yml", "Shop", &["ps"], &env);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            ["compose", "-p", "shop", "-f", "/c/docker-compose.yml", "ps"]
        );
        let envs: Vec<_> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert!(envs.contains(&("DATABASE_URL".to_string(), "postgres://x".to_string())));
        assert!(
            !envs
                .iter()
                .any(|(k, v)| k == "DOCKER_HOST" && v.contains("evil"))
        );
        assert!(!envs.iter().any(|(k, _)| k == "TOND_SECRET_KEY"));
    }

    #[test]
    fn path_helpers_build_expected_deploy_and_compose_locations() {
        let repo = TempDir::new()
            .unwrap_or_else(|err| panic!("failed to create temp repo directory: {err}"));
        std::fs::create_dir_all(repo.path().join("apps").join("web"))
            .unwrap_or_else(|err| panic!("failed to create nested build root: {err}"));

        assert_eq!(
            build_work_dir("/tmp/tond", "frontend"),
            "/tmp/tond/builds/frontend",
            "build_work_dir should place git and inline Dockerfile builds under the per-app builds directory"
        );
        assert_eq!(
            deploy_key_path("/tmp/tond", "app-123"),
            "/tmp/tond/keys/app-123/id_ed25519",
            "deploy_key_path should resolve the per-app SSH deploy key path"
        );
        assert_eq!(
            compose_work_dir("/tmp/tond", "frontend"),
            "/tmp/tond/compose/frontend",
            "compose_work_dir should place inline compose projects under the compose directory"
        );
        assert_eq!(
            compose_file_path("/tmp/tond/compose/frontend"),
            "/tmp/tond/compose/frontend/docker-compose.yml",
            "compose_file_path should point to the persisted inline compose file"
        );
        assert_eq!(
            resolve_build_root(
                repo.path()
                    .to_str()
                    .expect("tempdir path should be valid UTF-8"),
                Some("apps/web")
            )
            .unwrap_or_else(|err| panic!("expected nested build root to resolve: {err}")),
            repo.path().join("apps").join("web"),
            "resolve_build_root should append base_dir beneath the repository root"
        );
    }

    #[test]
    fn validate_base_dir_accepts_relative_paths_and_rejects_traversal() {
        for (input, expected) in [
            ("apps/web", "apps/web"),
            ("./apps/web/", "apps/web"),
            ("", ""),
            (".", ""),
        ] {
            assert_eq!(
                validate_base_dir(input).unwrap_or_else(|err| panic!("{input}: {err}")),
                expected
            );
        }
        for input in [
            "../etc",
            "apps/../../etc",
            "/etc",
            "apps\\..\\etc",
            "apps/\u{0}x",
        ] {
            assert!(
                validate_base_dir(input).is_err(),
                "base_dir {input:?} must be rejected"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolve_build_root_refuses_symlinks_that_leave_the_checkout() {
        let repo = TempDir::new()
            .unwrap_or_else(|err| panic!("failed to create temp repo directory: {err}"));
        let outside = TempDir::new()
            .unwrap_or_else(|err| panic!("failed to create outside directory: {err}"));
        std::os::unix::fs::symlink(outside.path(), repo.path().join("escape"))
            .unwrap_or_else(|err| panic!("failed to create symlink: {err}"));

        let repo_dir = repo.path().to_string_lossy().to_string();
        assert!(resolve_build_root(&repo_dir, Some("escape")).is_err());
        assert!(resolve_build_root(&repo_dir, Some("../")).is_err());
        assert!(resolve_build_root(&repo_dir, None).is_ok());
    }

    #[test]
    fn docker_build_args_include_custom_dockerfile_for_nested_build_roots() {
        assert_eq!(
            docker_build_args(
                "tond-web",
                "/tmp/repo",
                Some("/tmp/repo/apps/web/Dockerfile"),
                None,
            ),
            vec![
                "build",
                "-f",
                "/tmp/repo/apps/web/Dockerfile",
                "-t",
                "tond-web",
                "/tmp/repo",
            ],
            "nested builds should point docker build at the selected Dockerfile while keeping the repo root as the build context"
        );
    }

    #[test]
    fn builds_run_under_the_build_slice() {
        assert_eq!(
            docker_build_args("t", "/ctx", None, Some(DEFAULT_BUILD_CGROUP_PARENT)),
            vec![
                "build",
                "--cgroup-parent",
                "tondbuild.slice",
                "-t",
                "t",
                "/ctx"
            ]
        );
        // SAFETY: only this test reads TOND_BUILD_CGROUP_PARENT.
        unsafe { std::env::remove_var("TOND_BUILD_CGROUP_PARENT") };
        assert_eq!(
            build_cgroup_parent().as_deref(),
            Some(DEFAULT_BUILD_CGROUP_PARENT)
        );
        unsafe { std::env::set_var("TOND_BUILD_CGROUP_PARENT", " ") };
        assert_eq!(
            build_cgroup_parent(),
            None,
            "empty disables the cgroup parent"
        );
        unsafe { std::env::remove_var("TOND_BUILD_CGROUP_PARENT") };
    }

    #[tokio::test]
    async fn build_from_dir_with_method_rejects_missing_root_dockerfile() {
        let state = test_state().await;
        let temp_dir = TempDir::new()
            .unwrap_or_else(|err| panic!("failed to create temp repo directory: {err}"));
        let build_opts = BuildOptions {
            method: "dockerfile",
            static_build: false,
            base_dir: None,
        };

        let error = build_from_dir_with_method(
            &state,
            "app-1",
            "frontend",
            temp_dir
                .path()
                .to_str()
                .expect("tempdir path should be valid UTF-8"),
            &build_opts,
            None,
            "manual",
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected dockerfile build to fail without a root Dockerfile"));

        assert_eq!(
            error.to_string(),
            "build_method is 'dockerfile' but no Dockerfile was found in the repo root",
            "dockerfile builds should fail clearly when the repo root has no Dockerfile"
        );
    }

    #[tokio::test]
    async fn build_from_dir_with_method_rejects_invalid_persisted_build_method() {
        let state = test_state().await;
        let temp_dir = TempDir::new()
            .unwrap_or_else(|err| panic!("failed to create temp repo directory: {err}"));
        let build_opts = BuildOptions {
            method: "manual",
            static_build: false,
            base_dir: None,
        };

        let error = build_from_dir_with_method(
            &state,
            "app-1",
            "frontend",
            temp_dir
                .path()
                .to_str()
                .expect("tempdir path should be valid UTF-8"),
            &build_opts,
            None,
            "manual",
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("expected invalid persisted build_method to fail"));

        assert_eq!(
            error.to_string(),
            "unsupported build_method 'manual'; expected 'tbuild' or 'dockerfile'",
            "deploy runtime should fail clearly when it encounters an invalid persisted build method"
        );
    }

    #[tokio::test]
    async fn log_build_plan_metadata_writes_stack_and_runtime_details_to_deploy_logs() {
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, volumes, drain_timeout, build_method, auto_deploy, preview_enabled, preview_shared_keys, preview_env_vars, static_build, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))",
        )
        .bind("app-1")
        .bind("frontend")
        .bind("git")
        .bind(3000_i64)
        .bind("pending")
        .bind("{}")
        .bind("[]")
        .bind(30_i64)
        .bind("tbuild")
        .bind(0_i64)
        .bind(0_i64)
        .bind("[]")
        .bind("{}")
        .bind(0_i64)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app for build metadata test: {err}"));

        log_build_plan_metadata(
            &state,
            "app-1",
            "manual",
            &tond_scanner::BuildPlan {
                profile_name: "Node.js".to_string(),
                stack: "node".to_string(),
                confidence: 80,
                detection_reasons: vec!["found package.json in build root".to_string()],
                builder_image: Some("node:20-alpine".to_string()),
                runtime_image: Some("node:20-alpine".to_string()),
                run_command: Some("npm start".to_string()),
                static_output_dir: None,
                dockerfile: "FROM node:20-alpine".to_string(),
            },
        )
        .await
        .unwrap_or_else(|err| panic!("expected build metadata logging to succeed: {err}"));

        let messages: Vec<String> = sqlx::query_scalar(
            "SELECT message FROM deploy_logs WHERE app_id = ? ORDER BY created_at ASC, id ASC",
        )
        .bind("app-1")
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to load deploy metadata logs: {err}"));

        assert!(
            messages.iter().any(|message| message
                .contains("TBuild detection: stack=node profile=Node.js confidence=80")),
            "deploy logs should include the selected TBuild stack and confidence"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("TBuild builder image: node:20-alpine")),
            "deploy logs should include the selected builder image"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("TBuild run command: npm start")),
            "deploy logs should include the inferred runtime command"
        );
    }
}
