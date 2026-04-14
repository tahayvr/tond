// profiles/node.rs — Node.js / JavaScript detection and Dockerfile generation
//
// Fixes applied vs the original generate.rs:
//   - bun.lock (new plain-text format, Bun ≥1.1) detected alongside bun.lockb
//   - node_has_start_script() uses proper brace-depth + in-string tracking so
//     script values containing `}` (e.g. arrow functions) no longer cause false
//     negatives
//   - Static-build output dir is detected from config files (vite.config.*,
//     next.config.*, svelte.config.*, astro.config.*, nuxt.config.*) instead of
//     being hardcoded to "build/"
//   - Node image version is read from .nvmrc / .node-version instead of always
//     using a single fallback image
//   - Non-static runner stage uses a single COPY --from=builder /app ./
//     (removes the redundant preceding COPY of node_modules that was overwritten)

use std::path::Path;

use crate::{
    BuildPlan, ScannerError,
    profile::{DetectionOutcome, LanguageProfile},
};

// ── Public profile struct ────────────────────────────────────────────────────

pub(crate) struct NodeJsProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceContext {
    app_rel_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeCommand {
    PackageStart,
    NodeFile(String),
}

impl LanguageProfile for NodeJsProfile {
    fn name(&self) -> &'static str {
        "Node.js"
    }

    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join("package.json").exists()
    }

    fn detect_with_confidence(&self, repo_path: &Path) -> Option<DetectionOutcome> {
        let package_json = repo_path.join("package.json");
        if !package_json.exists() {
            return None;
        }

        let mut reasons = vec!["found package.json in build root".to_string()];
        let mut confidence = 70;
        if repo_path.join("pnpm-lock.yaml").exists() {
            confidence += 10;
            reasons.push("found pnpm-lock.yaml".to_string());
        }
        if repo_path.join("bun.lock").exists() || repo_path.join("bun.lockb").exists() {
            confidence += 10;
            reasons.push("found Bun lockfile".to_string());
        }

        Some(DetectionOutcome {
            confidence,
            reasons,
        })
    }

    fn generate_dockerfile_in_dir(
        &self,
        repo_root: &Path,
        build_root: &Path,
        static_build: bool,
    ) -> Result<String, ScannerError> {
        if static_build {
            ensure_static_build_supported(build_root)?;
        }

        let pm = detect_package_manager(repo_root, build_root);
        let base_image = detect_node_image(build_root, &pm);
        let workspace = detect_workspace_context(repo_root, build_root, &pm);
        let runtime_command = if static_build {
            None
        } else {
            Some(infer_runtime_command(build_root)?)
        };
        let output_dir = if static_build {
            detect_output_dir(build_root)
        } else {
            // output_dir is only used for the static runner CMD; unused here
            "dist"
        };

        tracing::debug!(
            "tond-scanner: Node.js — pm={:?} image={} static={} has_start={} output_dir={} workspace={:?}",
            pm,
            base_image,
            static_build,
            runtime_command
                .as_ref()
                .is_some_and(|cmd| matches!(cmd, RuntimeCommand::PackageStart)),
            output_dir,
            workspace.as_ref(),
        );

        Ok(render_dockerfile(
            &pm,
            &base_image,
            output_dir,
            runtime_command.as_ref(),
            static_build,
            workspace.as_ref(),
        ))
    }

    fn build_plan_in_dir(
        &self,
        repo_root: &Path,
        build_root: &Path,
        static_build: bool,
    ) -> Result<BuildPlan, ScannerError> {
        if static_build {
            ensure_static_build_supported(build_root)?;
        }

        let pm = detect_package_manager(repo_root, build_root);
        let base_image = detect_node_image(build_root, &pm);
        let workspace = detect_workspace_context(repo_root, build_root, &pm);
        let runtime_command = if static_build {
            None
        } else {
            Some(infer_runtime_command(build_root)?)
        };
        let output_dir = if static_build {
            Some(detect_output_dir(build_root).to_string())
        } else {
            None
        };
        let run_command = if static_build {
            output_dir
                .as_deref()
                .map(|dir| format!("npx --yes serve -s {dir} -l 8080"))
        } else {
            runtime_command.as_ref().map(|command| match command {
                RuntimeCommand::PackageStart => match pm {
                    PackageManager::Npm | PackageManager::Pnpm => "npm start".to_string(),
                    PackageManager::Yarn => "yarn start".to_string(),
                    PackageManager::Bun => "bun run start".to_string(),
                },
                RuntimeCommand::NodeFile(file) => format!("node {file}"),
            })
        };

        Ok(BuildPlan {
            profile_name: self.name().to_string(),
            stack: "node".to_string(),
            confidence: 80,
            detection_reasons: vec!["found package.json in build root".to_string()],
            builder_image: Some(base_image.clone()),
            runtime_image: Some(if static_build {
                "node:22.20.0-bookworm-slim".to_string()
            } else {
                base_image.clone()
            }),
            run_command,
            static_output_dir: output_dir.clone(),
            dockerfile: render_dockerfile(
                &pm,
                &base_image,
                output_dir.as_deref().unwrap_or("dist"),
                runtime_command.as_ref(),
                static_build,
                workspace.as_ref(),
            ),
        })
    }
}

// ── Package manager ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

/// Detect which package manager the project uses from its lockfile.
///
/// Priority: bun > pnpm > yarn > npm (npm is the fallback because it requires
/// no lockfile — package-lock.json may simply not be committed).
fn detect_package_manager(repo_root: &Path, build_root: &Path) -> PackageManager {
    // bun.lockb  — binary lockfile used by Bun <1.1
    // bun.lock   — plain-text TOML lockfile introduced in Bun ≥1.1
    if build_root.join("bun.lockb").exists() || build_root.join("bun.lock").exists() {
        PackageManager::Bun
    } else if path_exists_between(build_root, repo_root, "pnpm-lock.yaml") {
        PackageManager::Pnpm
    } else if path_exists_between(build_root, repo_root, "yarn.lock") {
        PackageManager::Yarn
    } else {
        PackageManager::Npm
    }
}

fn detect_workspace_context(
    repo_root: &Path,
    build_root: &Path,
    pm: &PackageManager,
) -> Option<WorkspaceContext> {
    if *pm != PackageManager::Pnpm || repo_root == build_root {
        return None;
    }

    if !repo_root.join("pnpm-workspace.yaml").exists() {
        return None;
    }

    let rel = build_root.strip_prefix(repo_root).ok()?;
    let rel = rel.to_str()?.replace('\\', "/");
    if rel.is_empty() {
        return None;
    }

    Some(WorkspaceContext { app_rel_dir: rel })
}

// ── Node image version ───────────────────────────────────────────────────────

/// Return the Docker base image tag for the given package manager.
///
/// For Bun projects the image is always `oven/bun:1.3.11-alpine`. For all other
/// package managers the version is read from `.nvmrc` or `.node-version`; if
/// neither file is present (or the version string cannot be parsed) the current
/// LTS fallback (`node:22.20.0-bookworm-slim`) is used.
fn detect_node_image(repo_path: &Path, pm: &PackageManager) -> String {
    if *pm == PackageManager::Bun {
        return "oven/bun:1.3.11-alpine".to_string();
    }

    for file in [".nvmrc", ".node-version"] {
        if let Ok(contents) = std::fs::read_to_string(repo_path.join(file))
            && let Some(major) = parse_node_major(contents.trim())
        {
            tracing::debug!("tond-scanner: Node version {} read from {}", major, file);
            return format!("node:{major}-bookworm-slim");
        }
    }

    "node:22.20.0-bookworm-slim".to_string()
}

/// Parse a Node.js version string and return the major version number.
///
/// Accepts:
///   - `"20"` / `"20.x"` / `"20.11.1"` / `"v20.11.1"`
///   - `"lts/*"` / `"lts/iron"` / `"lts/hydrogen"` (mapped to a known major)
///   - `"latest"` / `"current"` (treated as current LTS)
fn parse_node_major(version: &str) -> Option<u32> {
    let v = version.trim_start_matches('v');

    // Named LTS aliases — update these as Node releases new LTS lines
    match v {
        "lts/*" | "latest" | "current" | "lts/jod" => return Some(22),
        "lts/iron" => return Some(20),
        "lts/hydrogen" => return Some(18),
        "lts/gallium" => return Some(16),
        _ => {}
    }

    // Bare LTS prefix without a codename
    if v.starts_with("lts/") {
        return Some(20); // safe default: current LTS
    }

    // Numeric: "22", "22.x", "22.11.1"
    v.split('.').next()?.parse::<u32>().ok()
}

// ── Static output directory ──────────────────────────────────────────────────

/// Detect the static build output directory by inspecting config files.
///
/// Since the Dockerfile is generated *before* the build runs, we cannot check
/// which directory actually exists. Instead we infer from well-known config
/// files that each framework places at the repo root.
fn detect_output_dir(repo_path: &Path) -> &'static str {
    // Next.js (static export writes to `out/`)
    if any_exist(
        repo_path,
        &["next.config.js", "next.config.ts", "next.config.mjs"],
    ) {
        return "out";
    }

    // SvelteKit with adapter-static writes to `build/`
    if any_exist(repo_path, &["svelte.config.js", "svelte.config.ts"]) {
        return "build";
    }

    // Astro writes to `dist/`
    if any_exist(
        repo_path,
        &["astro.config.js", "astro.config.mjs", "astro.config.ts"],
    ) {
        return "dist";
    }

    // Nuxt static generate writes to `.output/public`
    if any_exist(repo_path, &["nuxt.config.js", "nuxt.config.ts"]) {
        return ".output/public";
    }

    // Vite (React/Vue/Svelte etc.) writes to `dist/`
    if any_exist(
        repo_path,
        &["vite.config.js", "vite.config.ts", "vite.config.mjs"],
    ) {
        return "dist";
    }

    // Create React App (react-scripts) writes to `build/`.
    // Detected by the presence of `public/index.html` without a Vite config.
    if repo_path.join("public").join("index.html").exists() {
        return "build";
    }

    // Default: dist (Vite is the dominant modern bundler)
    "dist"
}

fn ensure_static_build_supported(repo_path: &Path) -> Result<(), ScannerError> {
    if let Some(config) = first_existing_path(
        repo_path,
        &["next.config.js", "next.config.ts", "next.config.mjs"],
    ) {
        let contents = std::fs::read_to_string(config).unwrap_or_default();
        if !contents.contains("output: 'export'")
            && !contents.contains("output:\"export\"")
            && !contents.contains("output = 'export'")
            && !contents.contains("output=\"export\"")
        {
            return Err(ScannerError::UnsupportedFramework(
                "Detected Next.js app does not appear configured for static export; use a Dockerfile or configure export output.".to_string(),
            ));
        }
    }

    if let Some(config) = first_existing_path(repo_path, &["svelte.config.js", "svelte.config.ts"])
    {
        let contents = std::fs::read_to_string(config).unwrap_or_default();
        if !contents.contains("adapter-static") && !contents.contains("adapterStatic") {
            return Err(ScannerError::UnsupportedFramework(
                "Detected SvelteKit app does not appear configured for adapter-static; use a Dockerfile or configure static output.".to_string(),
            ));
        }
    }

    if let Some(config) = first_existing_path(repo_path, &["nuxt.config.js", "nuxt.config.ts"]) {
        let contents = std::fs::read_to_string(config).unwrap_or_default();
        if !contents.contains("ssr: false")
            && !contents.contains("ssr:false")
            && !contents.contains("nitro: { preset: 'static' }")
            && !contents.contains("nitro:{preset:'static'}")
            && !contents.contains("nitro: {preset: 'static'}")
        {
            return Err(ScannerError::UnsupportedFramework(
                "Detected Nuxt app does not appear configured for static generation; use a Dockerfile or configure static output.".to_string(),
            ));
        }
    }

    Ok(())
}

// ── Start-script detection ───────────────────────────────────────────────────

/// Return `true` if `package.json` at `repo_path` contains a `"start"` key
/// inside the `"scripts"` object.
///
/// Uses proper brace-depth counting (respecting quoted strings so that `}`
/// characters inside script values do not prematurely close the scripts block)
/// rather than the original naïve first-`}` search.
fn node_has_start_script(repo_path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(repo_path.join("package.json")) else {
        return false;
    };

    // Find the "scripts" key.
    let Some(scripts_pos) = contents.find("\"scripts\"") else {
        return false;
    };

    let after_scripts = &contents[scripts_pos..];

    // Find the opening `{` of the scripts object.
    let Some(brace_offset) = after_scripts.find('{') else {
        return false;
    };

    let scripts_body = &after_scripts[brace_offset..];

    // Walk character-by-character tracking:
    //   - brace depth (to find the *matching* closing brace, not just the first)
    //   - whether we are inside a JSON string (so `{` / `}` inside values are
    //     ignored)
    //   - escape sequences inside strings
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let mut end = scripts_body.len(); // fallback: take the whole tail

    for (i, c) in scripts_body.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            match c {
                '\\' => escape_next = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    let scripts_section = &scripts_body[..end];
    scripts_section.contains("\"start\"")
}

fn infer_runtime_command(repo_path: &Path) -> Result<RuntimeCommand, ScannerError> {
    if node_has_start_script(repo_path) {
        return Ok(RuntimeCommand::PackageStart);
    }

    for candidate in ["server.js", "index.js", "main.js", "app.js"] {
        if repo_path.join(candidate).exists() {
            return Ok(RuntimeCommand::NodeFile(candidate.to_string()));
        }
    }

    Err(ScannerError::UnsupportedFramework(
        "Detected Node.js app has no start script or obvious server entrypoint; add a start script, use static_build, or provide a Dockerfile.".to_string(),
    ))
}

// ── Dockerfile rendering ─────────────────────────────────────────────────────

fn render_dockerfile(
    pm: &PackageManager,
    base_image: &str,
    output_dir: &str,
    runtime_command: Option<&RuntimeCommand>,
    static_build: bool,
    workspace: Option<&WorkspaceContext>,
) -> String {
    if let Some(workspace) = workspace
        && *pm == PackageManager::Pnpm
    {
        return render_workspace_dockerfile(
            base_image,
            &workspace.app_rel_dir,
            output_dir,
            runtime_command,
            static_build,
        );
    }

    let (install_cmd, build_cmd, copy_lock, pm_setup) = match pm {
        PackageManager::Npm => (
            "npm ci",
            "npm run build --if-present",
            "COPY package*.json ./",
            "",
        ),
        PackageManager::Pnpm => (
            "pnpm install --frozen-lockfile",
            "pnpm run build --if-present",
            "COPY package.json pnpm-lock.yaml ./",
            "RUN corepack enable pnpm\n",
        ),
        PackageManager::Yarn => (
            "yarn install --frozen-lockfile",
            "yarn build --if-present",
            "COPY package.json yarn.lock ./",
            "",
        ),
        PackageManager::Bun => (
            "bun install --frozen-lockfile",
            "bun run build --if-present",
            // bun.lock* matches both bun.lock (≥1.1) and bun.lockb (<1.1)
            "COPY package.json bun.lock* ./",
            "",
        ),
    };

    // The runner image is the same as the builder for all package managers
    // (Bun uses oven/bun:1.3.11-alpine for both stages).
    let pm_setup_line = pm_setup.to_string();

    let start_cmd = if let Some(command) = runtime_command {
        match command {
            RuntimeCommand::PackageStart => match pm {
                PackageManager::Npm | PackageManager::Pnpm => {
                    "CMD [\"npm\", \"start\"]".to_string()
                }
                PackageManager::Yarn => "CMD [\"yarn\", \"start\"]".to_string(),
                PackageManager::Bun => "CMD [\"bun\", \"run\", \"start\"]".to_string(),
            },
            RuntimeCommand::NodeFile(file) => format!("CMD [\"node\", \"{file}\"]"),
        }
    } else {
        // Static site: serve the compiled output directory with `serve`.
        format!("CMD [\"npx\", \"--yes\", \"serve\", \"-s\", \"{output_dir}\", \"-l\", \"8080\"]")
    };

    if static_build {
        // Production stage only needs the compiled output — node_modules are
        // not required at runtime when serving static files.
        format!(
            "\
# Stage 1: Build
FROM {base_image} AS builder
WORKDIR /app
{pm_setup_line}{copy_lock}
RUN {install_cmd}
COPY . .
RUN {build_cmd}

# Stage 2: Production (static file server — no node_modules needed)
FROM node:22.20.0-bookworm-slim AS runner
WORKDIR /app
COPY --from=builder /app/{output_dir} ./{output_dir}
EXPOSE 8080
{start_cmd}
"
        )
    } else {
        // Server app: copy the entire /app (source + node_modules + build
        // output) from the builder stage.  A single COPY is sufficient —
        // the previous pattern of COPY node_modules then COPY /app was
        // redundant because the second COPY overwrote the first.
        format!(
            "\
# Stage 1: Build
FROM {base_image} AS builder
WORKDIR /app
{pm_setup_line}{copy_lock}
RUN {install_cmd}
COPY . .
RUN {build_cmd}

# Stage 2: Production
FROM {base_image} AS runner
WORKDIR /app
COPY --from=builder /app ./
ENV NODE_ENV=production
EXPOSE 8080
{start_cmd}
"
        )
    }
}

fn render_workspace_dockerfile(
    base_image: &str,
    app_rel_dir: &str,
    output_dir: &str,
    runtime_command: Option<&RuntimeCommand>,
    static_build: bool,
) -> String {
    let start_cmd = if let Some(command) = runtime_command {
        match command {
            RuntimeCommand::PackageStart => "CMD [\"npm\", \"start\"]".to_string(),
            RuntimeCommand::NodeFile(file) => format!("CMD [\"node\", \"{file}\"]"),
        }
    } else {
        format!("CMD [\"npx\", \"--yes\", \"serve\", \"-s\", \"{output_dir}\", \"-l\", \"8080\"]")
    };

    if static_build {
        format!(
            "\
# Stage 1: Build
FROM {base_image} AS builder
WORKDIR /app
RUN corepack enable pnpm
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY {app_rel_dir}/package.json ./{app_rel_dir}/package.json
RUN pnpm install --frozen-lockfile
COPY . .
WORKDIR /app/{app_rel_dir}
RUN pnpm run build --if-present

# Stage 2: Production (static file server — no node_modules needed)
FROM node:22.20.0-bookworm-slim AS runner
WORKDIR /app
COPY --from=builder /app/{app_rel_dir}/{output_dir} ./{output_dir}
EXPOSE 8080
{start_cmd}
"
        )
    } else {
        format!(
            "\
# Stage 1: Build
FROM {base_image} AS builder
WORKDIR /app
RUN corepack enable pnpm
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY {app_rel_dir}/package.json ./{app_rel_dir}/package.json
RUN pnpm install --frozen-lockfile
COPY . .
WORKDIR /app/{app_rel_dir}
RUN pnpm run build --if-present

# Stage 2: Production
FROM {base_image} AS runner
WORKDIR /app/{app_rel_dir}
COPY --from=builder /app ./
ENV NODE_ENV=production
EXPOSE 8080
{start_cmd}
"
        )
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Return `true` if any of `files` exists directly under `base`.
fn any_exist(base: &Path, files: &[&str]) -> bool {
    files.iter().any(|f| base.join(f).exists())
}

fn first_existing_path(base: &Path, files: &[&str]) -> Option<std::path::PathBuf> {
    files
        .iter()
        .map(|f| base.join(f))
        .find(|path| path.exists())
}

fn path_exists_between(start: &Path, stop: &Path, name: &str) -> bool {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(name).exists() {
            return true;
        }
        if dir == stop {
            break;
        }
        current = dir.parent();
    }
    false
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
    fn detect_returns_true_when_package_json_exists() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let detected = NodeJsProfile.detect(repo.path());

        assert!(
            detected,
            "NodeJsProfile should detect repositories that contain a package.json file"
        );
    }

    #[test]
    fn detect_returns_false_when_package_json_is_missing() {
        let repo = create_repo();

        let detected = NodeJsProfile.detect(repo.path());

        assert!(
            !detected,
            "NodeJsProfile should ignore repositories without a package.json file"
        );
    }

    #[test]
    fn parse_node_major_accepts_numeric_versions_and_aliases() {
        assert_eq!(
            parse_node_major("20"),
            Some(20),
            "bare major versions should parse"
        );
        assert_eq!(
            parse_node_major("v22.11.1"),
            Some(22),
            "prefixed semantic versions should parse"
        );
        assert_eq!(
            parse_node_major("lts/iron"),
            Some(20),
            "known LTS aliases should map to their major version"
        );
        assert_eq!(
            parse_node_major("latest"),
            Some(22),
            "latest should map to the current LTS major"
        );
        assert_eq!(
            parse_node_major("lts/custom"),
            Some(20),
            "unknown LTS aliases should fall back to the safe current LTS default"
        );
        assert_eq!(
            parse_node_major("not-a-version"),
            None,
            "invalid version strings should not parse"
        );
    }

    #[test]
    fn detect_node_image_uses_bun_image_for_bun_projects() {
        let repo = create_repo();

        let image = detect_node_image(repo.path(), &PackageManager::Bun);

        assert_eq!(
            image, "oven/bun:1.3.11-alpine",
            "Bun projects should always use the Bun runtime image regardless of version files"
        );
    }

    #[test]
    fn detect_node_image_reads_nvmrc_major_version() {
        let repo = create_repo();
        fs::write(repo.path().join(".nvmrc"), "v18.19.0\n")
            .unwrap_or_else(|err| panic!("failed to write .nvmrc: {err}"));

        let image = detect_node_image(repo.path(), &PackageManager::Npm);

        assert_eq!(
            image, "node:18-bookworm-slim",
            "the Node image should use the major version parsed from .nvmrc"
        );
    }

    #[test]
    fn detect_node_image_falls_back_to_default_for_invalid_version_files() {
        let repo = create_repo();
        fs::write(repo.path().join(".node-version"), "bogus\n")
            .unwrap_or_else(|err| panic!("failed to write .node-version: {err}"));

        let image = detect_node_image(repo.path(), &PackageManager::Yarn);

        assert_eq!(
            image, "node:22.20.0-bookworm-slim",
            "invalid version files should fall back to the default LTS image"
        );
    }

    #[test]
    fn detect_output_dir_matches_framework_conventions() {
        let next_repo = create_repo();
        fs::write(
            next_repo.path().join("next.config.js"),
            "module.exports = {};\n",
        )
        .unwrap_or_else(|err| panic!("failed to write next.config.js: {err}"));
        assert_eq!(
            detect_output_dir(next_repo.path()),
            "out",
            "Next.js static exports should target out/"
        );

        let svelte_repo = create_repo();
        fs::write(
            svelte_repo.path().join("svelte.config.js"),
            "export default {};\n",
        )
        .unwrap_or_else(|err| panic!("failed to write svelte.config.js: {err}"));
        assert_eq!(
            detect_output_dir(svelte_repo.path()),
            "build",
            "SvelteKit static builds should target build/"
        );

        let nuxt_repo = create_repo();
        fs::write(
            nuxt_repo.path().join("nuxt.config.ts"),
            "export default {};\n",
        )
        .unwrap_or_else(|err| panic!("failed to write nuxt.config.ts: {err}"));
        assert_eq!(
            detect_output_dir(nuxt_repo.path()),
            ".output/public",
            "Nuxt static builds should target .output/public/"
        );

        let cra_repo = create_repo();
        fs::create_dir_all(cra_repo.path().join("public"))
            .unwrap_or_else(|err| panic!("failed to create public directory: {err}"));
        fs::write(
            cra_repo.path().join("public").join("index.html"),
            "<!doctype html>\n",
        )
        .unwrap_or_else(|err| panic!("failed to write public/index.html: {err}"));
        assert_eq!(
            detect_output_dir(cra_repo.path()),
            "build",
            "Create React App projects should target build/"
        );

        let default_repo = create_repo();
        assert_eq!(
            detect_output_dir(default_repo.path()),
            "dist",
            "unknown static projects should default to dist/"
        );
    }

    #[test]
    fn node_has_start_script_handles_braces_inside_script_values() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{
  "scripts": {
    "build": "node -e \"console.log(({ value: 1 }))\"",
    "start": "node server.js"
  }
}"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let has_start = node_has_start_script(repo.path());

        assert!(
            has_start,
            "start script detection should ignore braces that appear inside quoted script commands"
        );
    }

    #[test]
    fn node_has_start_script_returns_false_when_start_is_absent() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{
  "scripts": {
    "build": "vite build"
  }
}"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let has_start = node_has_start_script(repo.path());

        assert!(
            !has_start,
            "repositories without a start script should not be treated as long-running server apps"
        );
    }

    #[test]
    fn generate_dockerfile_for_server_project_uses_start_command() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{
  "name": "demo",
  "scripts": {
    "start": "node server.js",
    "build": "vite build"
  }
}"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let dockerfile = NodeJsProfile
            .generate_dockerfile(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("FROM node:22.20.0-bookworm-slim AS runner"),
            "server builds should reuse the Node runtime in the runner stage, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY --from=builder /app ./"),
            "server builds should copy the full application from the builder stage, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("CMD [\"npm\", \"start\"]"),
            "server builds with a start script should launch the application via npm start, got: {dockerfile}"
        );
    }

    #[test]
    fn generate_dockerfile_for_static_bun_project_uses_static_runner_and_framework_output() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{\"name\":\"demo\"}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(repo.path().join("bun.lock"), "[lockfile]\n")
            .unwrap_or_else(|err| panic!("failed to write bun.lock: {err}"));
        fs::write(
            repo.path().join("svelte.config.ts"),
            "import adapter from '@sveltejs/adapter-static';\nexport default { kit: { adapter: adapter() } };\n",
        )
            .unwrap_or_else(|err| panic!("failed to write svelte.config.ts: {err}"));

        let dockerfile = NodeJsProfile
            .generate_dockerfile(repo.path(), true)
            .unwrap_or_else(|err| panic!("expected Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("FROM oven/bun:1.3.11-alpine AS builder"),
            "Bun projects should build with the Bun image, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY package.json bun.lock* ./"),
            "Bun projects should copy the Bun lockfile glob, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY --from=builder /app/build ./build"),
            "Svelte static builds should copy the detected build directory, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains(
                "CMD [\"npx\", \"--yes\", \"serve\", \"-s\", \"build\", \"-l\", \"8080\"]"
            ),
            "static builds should serve the detected output directory, got: {dockerfile}"
        );
    }

    #[test]
    fn build_plan_captures_node_detection_details() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{
  "name": "demo",
  "scripts": {
    "start": "node server.js",
    "build": "vite build"
  }
}"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let plan = NodeJsProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected BuildPlan generation to succeed: {err}"));

        assert_eq!(
            plan.stack, "node",
            "Node.js BuildPlan should record a stable stack identifier"
        );
        assert_eq!(
            plan.run_command.as_deref(),
            Some("npm start"),
            "server-mode Node.js BuildPlans should expose the inferred run command"
        );
        assert_eq!(
            plan.builder_image.as_deref(),
            Some("node:22.20.0-bookworm-slim"),
            "BuildPlan should capture the selected Node.js builder image"
        );
        assert!(
            plan.dockerfile.contains("CMD [\"npm\", \"start\"]"),
            "BuildPlan should still carry the rendered Dockerfile contents, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn generate_dockerfile_for_pnpm_workspace_build_root_uses_workspace_root_files() {
        let repo = create_repo();
        let app_dir = repo.path().join("apps").join("web");
        fs::create_dir_all(&app_dir)
            .unwrap_or_else(|err| panic!("failed to create app directory: {err}"));
        fs::write(
            repo.path().join("package.json"),
            "{\"name\":\"workspace-root\"}",
        )
        .unwrap_or_else(|err| panic!("failed to write root package.json: {err}"));
        fs::write(
            repo.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pnpm-lock.yaml: {err}"));
        fs::write(
            repo.path().join("pnpm-workspace.yaml"),
            "packages:\n  - apps/*\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pnpm-workspace.yaml: {err}"));
        fs::write(
            app_dir.join("package.json"),
            "{\"name\":\"web\",\"scripts\":{\"build\":\"vite build\"}}",
        )
        .unwrap_or_else(|err| panic!("failed to write app package.json: {err}"));
        fs::write(app_dir.join("vite.config.ts"), "export default {};\n")
            .unwrap_or_else(|err| panic!("failed to write vite.config.ts: {err}"));

        let dockerfile = NodeJsProfile
            .generate_dockerfile_in_dir(repo.path(), &app_dir, true)
            .unwrap_or_else(|err| {
                panic!("expected workspace Dockerfile generation to succeed: {err}")
            });

        assert!(
            dockerfile.contains("COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./"),
            "workspace builds should copy root workspace manifests first, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY apps/web/package.json ./apps/web/package.json"),
            "workspace builds should copy the nested app manifest before install, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("WORKDIR /app/apps/web"),
            "workspace builds should switch to the nested app directory before running build commands, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY --from=builder /app/apps/web/dist ./dist"),
            "workspace static builds should copy the built output from the nested app directory, got: {dockerfile}"
        );
    }

    #[test]
    fn static_build_rejects_nextjs_without_explicit_export_configuration() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "name": "demo", "scripts": { "build": "next build" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(repo.path().join("next.config.js"), "module.exports = {};\n")
            .unwrap_or_else(|err| panic!("failed to write next.config.js: {err}"));

        let error = NodeJsProfile
            .generate_dockerfile(repo.path(), true)
            .err()
            .unwrap_or_else(|| {
                panic!("expected static Next.js build without export config to be rejected")
            });

        assert_eq!(
            error.to_string(),
            "Detected Next.js app does not appear configured for static export; use a Dockerfile or configure export output.",
            "Next.js static builds should require explicit export configuration"
        );
    }

    #[test]
    fn static_build_rejects_sveltekit_without_adapter_static_marker() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{\"name\":\"demo\"}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(
            repo.path().join("svelte.config.ts"),
            "export default { kit: {} };\n",
        )
        .unwrap_or_else(|err| panic!("failed to write svelte.config.ts: {err}"));

        let error = NodeJsProfile
            .generate_dockerfile(repo.path(), true)
            .err()
            .unwrap_or_else(|| {
                panic!("expected static SvelteKit build without adapter-static to be rejected")
            });

        assert_eq!(
            error.to_string(),
            "Detected SvelteKit app does not appear configured for adapter-static; use a Dockerfile or configure static output.",
            "SvelteKit static builds should require adapter-static markers"
        );
    }

    #[test]
    fn static_build_rejects_nuxt_without_static_generation_marker() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{\"name\":\"demo\"}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(
            repo.path().join("nuxt.config.ts"),
            "export default defineNuxtConfig({});\n",
        )
        .unwrap_or_else(|err| panic!("failed to write nuxt.config.ts: {err}"));

        let error = NodeJsProfile
            .generate_dockerfile(repo.path(), true)
            .err()
            .unwrap_or_else(|| {
                panic!("expected static Nuxt build without static marker to be rejected")
            });

        assert_eq!(
            error.to_string(),
            "Detected Nuxt app does not appear configured for static generation; use a Dockerfile or configure static output.",
            "Nuxt static builds should require explicit static-generation markers"
        );
    }

    #[test]
    fn static_build_allows_sveltekit_with_adapter_static_marker() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{\"name\":\"demo\"}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(
            repo.path().join("svelte.config.ts"),
            "import adapter from '@sveltejs/adapter-static';\nexport default { kit: { adapter: adapter() } };\n",
        )
        .unwrap_or_else(|err| panic!("failed to write svelte.config.ts: {err}"));

        let dockerfile = NodeJsProfile
            .generate_dockerfile(repo.path(), true)
            .unwrap_or_else(|err| {
                panic!("expected adapter-static SvelteKit project to succeed: {err}")
            });

        assert!(
            dockerfile.contains("COPY --from=builder /app/build ./build"),
            "static SvelteKit builds should still target the build directory when adapter-static is configured, got: {dockerfile}"
        );
    }

    #[test]
    fn server_mode_uses_server_js_when_no_start_script_exists() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{\"name\":\"demo\"}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(repo.path().join("server.js"), "console.log('hi')\n")
            .unwrap_or_else(|err| panic!("failed to write server.js: {err}"));

        let plan = NodeJsProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected server.js fallback to succeed: {err}"));

        assert_eq!(
            plan.run_command.as_deref(),
            Some("node server.js"),
            "server-mode Node.js apps should fall back to node server.js when no start script exists"
        );
        assert!(
            plan.dockerfile.contains("CMD [\"node\", \"server.js\"]"),
            "generated Dockerfile should run server.js directly when it is the inferred entrypoint, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn server_mode_uses_index_js_when_no_start_script_exists() {
        let repo = create_repo();
        fs::write(repo.path().join("package.json"), "{\"name\":\"demo\"}")
            .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));
        fs::write(repo.path().join("index.js"), "console.log('hi')\n")
            .unwrap_or_else(|err| panic!("failed to write index.js: {err}"));

        let plan = NodeJsProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected index.js fallback to succeed: {err}"));

        assert_eq!(
            plan.run_command.as_deref(),
            Some("node index.js"),
            "server-mode Node.js apps should fall back to node index.js when no start script exists"
        );
    }

    #[test]
    fn server_mode_rejects_missing_runtime_entrypoint() {
        let repo = create_repo();
        fs::write(
            repo.path().join("package.json"),
            r#"{ "name": "demo", "scripts": { "build": "vite build" } }"#,
        )
        .unwrap_or_else(|err| panic!("failed to write package.json: {err}"));

        let error = NodeJsProfile
            .build_plan(repo.path(), false)
            .err()
            .unwrap_or_else(|| {
                panic!("expected server-mode build without runtime entrypoint to fail")
            });

        assert_eq!(
            error.to_string(),
            "Detected Node.js app has no start script or obvious server entrypoint; add a start script, use static_build, or provide a Dockerfile.",
            "server-mode Node.js builds should fail clearly when no runtime command can be inferred"
        );
    }
}
