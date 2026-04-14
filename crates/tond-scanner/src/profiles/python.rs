// profiles/python.rs — Python detection and Dockerfile generation
//
// Fixes applied vs the original PYTHON constant in generate.rs:
//   - Entry point is detected from the repository layout in priority order:
//       manage.py  → Django  → CMD ["python", "manage.py", "runserver", …]
//       asgi.py    → ASGI app via uvicorn
//       wsgi.py    → WSGI app via gunicorn
//       app.py     → Flask / FastAPI / generic
//       main.py    → FastAPI / generic
//       server.py  → generic
//       run.py     → generic
//     If none of the above are found the Dockerfile emits a clear comment so
//     the developer knows they must supply a CMD override.
//   - Package manager is detected from the lockfile / manifest:
//       poetry.lock      → Poetry  (pip install poetry && poetry install)
//       uv.lock          → uv      (pip install uv && uv sync)
//       requirements.txt → pip     (pip install -r requirements.txt)
//       pyproject.toml   → pip .   (pip install .)   — PEP 517 / 518 projects
//     This avoids the previous bug where pyproject.toml-only projects received
//     a template that called `pip install -r requirements.txt` which would
//     fail because requirements.txt does not exist.

use std::path::Path;

use crate::{
    BuildPlan, ScannerError,
    profile::{DetectionOutcome, LanguageProfile},
};

const PYTHON_BASE_IMAGE: &str = "python:3.13.13-slim-bookworm";

// ── Public profile struct ────────────────────────────────────────────────────

pub(crate) struct PythonProfile;

impl LanguageProfile for PythonProfile {
    fn name(&self) -> &'static str {
        "Python"
    }

    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join("requirements.txt").exists() || repo_path.join("pyproject.toml").exists()
    }

    fn detect_with_confidence(&self, repo_path: &Path) -> Option<DetectionOutcome> {
        let has_requirements = repo_path.join("requirements.txt").exists();
        let has_pyproject = repo_path.join("pyproject.toml").exists();
        if !has_requirements && !has_pyproject {
            return None;
        }

        let mut reasons = vec![];
        let mut confidence = 60;
        if has_requirements {
            reasons.push("found requirements.txt".to_string());
            confidence += 10;
        }
        if has_pyproject {
            reasons.push("found pyproject.toml".to_string());
            confidence += 10;
        }

        Some(DetectionOutcome {
            confidence,
            reasons,
        })
    }

    fn generate_dockerfile(
        &self,
        repo_path: &Path,
        _static_build: bool,
    ) -> Result<String, ScannerError> {
        let pm = detect_package_manager(repo_path);
        let entry = detect_entry_point(repo_path);

        tracing::debug!("tond-scanner: Python — pm={} entry={:?}", pm.name(), entry,);

        Ok(render_dockerfile(&pm, &entry))
    }

    fn build_plan(&self, repo_path: &Path, static_build: bool) -> Result<BuildPlan, ScannerError> {
        let pm = detect_package_manager(repo_path);
        let entry = detect_entry_point(repo_path);

        let run_command = if static_build {
            None
        } else {
            Some(match &entry {
                EntryPoint::Script(file) => format!("python {file}"),
                EntryPoint::Asgi(target) => {
                    format!("uvicorn {target} --host 0.0.0.0 --port 8080")
                }
                EntryPoint::Wsgi(target) => format!("gunicorn {target} --bind 0.0.0.0:8080"),
                EntryPoint::Django => "python manage.py runserver 0.0.0.0:8080".to_string(),
                EntryPoint::Unknown => {
                    "python -c \"import sys; print('ERROR: no entry point configured'); sys.exit(1)\""
                        .to_string()
                }
            })
        };

        Ok(BuildPlan {
            profile_name: self.name().to_string(),
            stack: "python".to_string(),
            confidence: 70,
            detection_reasons: vec!["found Python project manifest".to_string()],
            builder_image: Some(PYTHON_BASE_IMAGE.to_string()),
            runtime_image: Some(PYTHON_BASE_IMAGE.to_string()),
            run_command,
            static_output_dir: None,
            dockerfile: render_dockerfile(&pm, &entry),
        })
    }
}

// ── Package manager ──────────────────────────────────────────────────────────

enum PythonPackageManager {
    /// requirements.txt is present — use `pip install -r requirements.txt`.
    Pip,
    /// poetry.lock is present — use Poetry.
    Poetry,
    /// uv.lock is present — use uv.
    Uv,
    /// Only pyproject.toml is present (no lockfile, no requirements.txt).
    /// Use `pip install .` which respects PEP 517/518 build backends.
    Pyproject,
}

impl PythonPackageManager {
    fn name(&self) -> &'static str {
        match self {
            PythonPackageManager::Pip => "pip",
            PythonPackageManager::Poetry => "poetry",
            PythonPackageManager::Uv => "uv",
            PythonPackageManager::Pyproject => "pip (pyproject.toml)",
        }
    }
}

fn detect_package_manager(repo_path: &Path) -> PythonPackageManager {
    if repo_path.join("poetry.lock").exists() {
        PythonPackageManager::Poetry
    } else if repo_path.join("uv.lock").exists() {
        PythonPackageManager::Uv
    } else if repo_path.join("requirements.txt").exists() {
        PythonPackageManager::Pip
    } else {
        // pyproject.toml present (detected by PythonProfile::detect) but no
        // lockfile and no requirements.txt — treat as a PEP 517 project.
        PythonPackageManager::Pyproject
    }
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// The command used to start the application inside the container.
#[derive(Debug)]
enum EntryPoint {
    /// `python <file>` — plain script entry point.
    Script(String),
    /// `uvicorn <module>:app --host 0.0.0.0 --port 8080`.
    Asgi(String),
    /// `gunicorn <module>:app --bind 0.0.0.0:8080`.
    Wsgi(String),
    /// Django — `python manage.py runserver 0.0.0.0:8080`.
    Django,
    /// No recognisable entry point was found.
    Unknown,
}

fn detect_entry_point(repo_path: &Path) -> EntryPoint {
    // Django: manage.py is the canonical marker.
    if repo_path.join("manage.py").exists() {
        return EntryPoint::Django;
    }

    if repo_path.join("asgi.py").exists() {
        return EntryPoint::Asgi("asgi:app".to_string());
    }

    if repo_path.join("wsgi.py").exists() {
        return EntryPoint::Wsgi("wsgi:app".to_string());
    }

    if repo_path.join("app.py").exists() {
        return EntryPoint::Script("app.py".to_string());
    }

    if repo_path.join("main.py").exists() {
        let contents = std::fs::read_to_string(repo_path.join("main.py")).unwrap_or_default();
        if contents.contains("FastAPI") || contents.contains("app = FastAPI(") {
            return EntryPoint::Asgi("main:app".to_string());
        }

        return EntryPoint::Script("main.py".to_string());
    }

    // Common single-file entry points in priority order.
    for name in &["server.py", "run.py"] {
        if repo_path.join(name).exists() {
            return EntryPoint::Script(name.to_string());
        }
    }

    EntryPoint::Unknown
}

fn cmd_line(entry: &EntryPoint) -> String {
    match entry {
        EntryPoint::Script(f) => format!("CMD [\"python\", \"{f}\"]"),
        EntryPoint::Asgi(target) => format!(
            "CMD [\"uvicorn\", \"{target}\", \"--host\", \"0.0.0.0\", \"--port\", \"8080\"]"
        ),
        EntryPoint::Wsgi(target) => {
            format!("CMD [\"gunicorn\", \"{target}\", \"--bind\", \"0.0.0.0:8080\"]")
        }
        EntryPoint::Django => {
            "CMD [\"python\", \"manage.py\", \"runserver\", \"0.0.0.0:8080\"]".to_string()
        }
        EntryPoint::Unknown => {
            // Emit a placeholder that will fail loudly at container start so
            // the developer gets an actionable error rather than a silent hang.
            "# TODO: no Python entry point detected — set CMD manually\n\
             CMD [\"python\", \"-c\", \"import sys; print('ERROR: no entry point configured'); sys.exit(1)\"]"
                .to_string()
        }
    }
}

// ── Dockerfile rendering ─────────────────────────────────────────────────────

fn render_dockerfile(pm: &PythonPackageManager, entry: &EntryPoint) -> String {
    let cmd = cmd_line(entry);

    match pm {
        PythonPackageManager::Pip => format!(
            "\
FROM {PYTHON_BASE_IMAGE}
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
EXPOSE 8080
{cmd}
"
        ),

        PythonPackageManager::Poetry => format!(
            "\
FROM {PYTHON_BASE_IMAGE}
WORKDIR /app
RUN pip install --no-cache-dir poetry
COPY pyproject.toml poetry.lock ./
RUN poetry config virtualenvs.create false \\
    && poetry install --only main --no-interaction --no-ansi
COPY . .
EXPOSE 8080
{cmd}
"
        ),

        PythonPackageManager::Uv => format!(
            "\
FROM {PYTHON_BASE_IMAGE}
WORKDIR /app
RUN pip install --no-cache-dir uv
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev
COPY . .
EXPOSE 8080
{cmd}
"
        ),

        PythonPackageManager::Pyproject => format!(
            "\
FROM {PYTHON_BASE_IMAGE}
WORKDIR /app
COPY pyproject.toml ./
COPY . .
RUN pip install --no-cache-dir .
EXPOSE 8080
{cmd}
"
        ),
    }
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
    fn detect_returns_true_for_requirements_or_pyproject_projects() {
        let requirements_repo = create_repo();
        fs::write(requirements_repo.path().join("requirements.txt"), "flask\n")
            .unwrap_or_else(|err| panic!("failed to write requirements.txt: {err}"));
        assert!(
            PythonProfile.detect(requirements_repo.path()),
            "requirements.txt should be enough to detect a Python project"
        );

        let pyproject_repo = create_repo();
        fs::write(
            pyproject_repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        assert!(
            PythonProfile.detect(pyproject_repo.path()),
            "pyproject.toml should be enough to detect a Python project"
        );
    }

    #[test]
    fn detect_returns_false_when_no_python_manifest_exists() {
        let repo = create_repo();

        let detected = PythonProfile.detect(repo.path());

        assert!(
            !detected,
            "PythonProfile should ignore repositories without Python dependency manifests"
        );
    }

    #[test]
    fn detect_package_manager_honors_lockfile_priority() {
        let poetry_repo = create_repo();
        fs::write(
            poetry_repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        fs::write(poetry_repo.path().join("poetry.lock"), "[[package]]\n")
            .unwrap_or_else(|err| panic!("failed to write poetry.lock: {err}"));
        assert_eq!(
            detect_package_manager(poetry_repo.path()).name(),
            "poetry",
            "poetry.lock should take precedence over other Python dependency files"
        );

        let uv_repo = create_repo();
        fs::write(
            uv_repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        fs::write(uv_repo.path().join("uv.lock"), "version = 1\n")
            .unwrap_or_else(|err| panic!("failed to write uv.lock: {err}"));
        assert_eq!(
            detect_package_manager(uv_repo.path()).name(),
            "uv",
            "uv.lock should select uv when poetry.lock is absent"
        );

        let pip_repo = create_repo();
        fs::write(pip_repo.path().join("requirements.txt"), "fastapi\n")
            .unwrap_or_else(|err| panic!("failed to write requirements.txt: {err}"));
        assert_eq!(
            detect_package_manager(pip_repo.path()).name(),
            "pip",
            "requirements.txt should select pip when no lockfile is present"
        );

        let pyproject_repo = create_repo();
        fs::write(
            pyproject_repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        assert_eq!(
            detect_package_manager(pyproject_repo.path()).name(),
            "pip (pyproject.toml)",
            "pyproject-only projects should use pip install . as a fallback"
        );
    }

    #[test]
    fn detect_entry_point_prefers_django_then_common_script_names() {
        let django_repo = create_repo();
        fs::write(django_repo.path().join("manage.py"), "print('django')\n")
            .unwrap_or_else(|err| panic!("failed to write manage.py: {err}"));
        fs::write(django_repo.path().join("app.py"), "print('app')\n")
            .unwrap_or_else(|err| panic!("failed to write app.py: {err}"));
        assert!(
            matches!(detect_entry_point(django_repo.path()), EntryPoint::Django),
            "manage.py should take precedence over generic script entry points"
        );

        let script_repo = create_repo();
        fs::write(script_repo.path().join("main.py"), "print('main')\n")
            .unwrap_or_else(|err| panic!("failed to write main.py: {err}"));
        fs::write(script_repo.path().join("server.py"), "print('server')\n")
            .unwrap_or_else(|err| panic!("failed to write server.py: {err}"));
        assert!(
            matches!(detect_entry_point(script_repo.path()), EntryPoint::Script(name) if name == "main.py"),
            "entry point detection should honor the documented priority order for generic script names"
        );

        let unknown_repo = create_repo();
        assert!(
            matches!(detect_entry_point(unknown_repo.path()), EntryPoint::Unknown),
            "repositories without a known entry point should return EntryPoint::Unknown"
        );
    }

    #[test]
    fn build_plan_prefers_uvicorn_for_fastapi_style_asgi_entrypoints() {
        let repo = create_repo();
        fs::write(repo.path().join("requirements.txt"), "fastapi\nuvicorn\n")
            .unwrap_or_else(|err| panic!("failed to write requirements.txt: {err}"));
        fs::write(
            repo.path().join("main.py"),
            "from fastapi import FastAPI\napp = FastAPI()\n",
        )
        .unwrap_or_else(|err| panic!("failed to write main.py: {err}"));

        let plan = PythonProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected BuildPlan generation to succeed: {err}"));

        assert_eq!(
            plan.run_command.as_deref(),
            Some("uvicorn main:app --host 0.0.0.0 --port 8080"),
            "FastAPI-style ASGI apps should prefer uvicorn with a stable app target"
        );
        assert!(
            plan.dockerfile.contains(
                "CMD [\"uvicorn\", \"main:app\", \"--host\", \"0.0.0.0\", \"--port\", \"8080\"]"
            ),
            "ASGI apps should render a uvicorn CMD in the Dockerfile, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn build_plan_prefers_gunicorn_for_wsgi_entrypoints() {
        let repo = create_repo();
        fs::write(repo.path().join("requirements.txt"), "flask\ngunicorn\n")
            .unwrap_or_else(|err| panic!("failed to write requirements.txt: {err}"));
        fs::write(repo.path().join("wsgi.py"), "from app import app\n")
            .unwrap_or_else(|err| panic!("failed to write wsgi.py: {err}"));

        let plan = PythonProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected BuildPlan generation to succeed: {err}"));

        assert_eq!(
            plan.run_command.as_deref(),
            Some("gunicorn wsgi:app --bind 0.0.0.0:8080"),
            "WSGI apps should prefer gunicorn with a stable app target"
        );
        assert!(
            plan.dockerfile
                .contains("CMD [\"gunicorn\", \"wsgi:app\", \"--bind\", \"0.0.0.0:8080\"]"),
            "WSGI apps should render a gunicorn CMD in the Dockerfile, got: {}",
            plan.dockerfile
        );
    }

    #[test]
    fn generate_dockerfile_for_pyproject_only_project_uses_pip_install_dot_and_placeholder_cmd() {
        let repo = create_repo();
        fs::write(
            repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));

        let dockerfile = PythonProfile
            .generate_dockerfile(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("RUN pip install --no-cache-dir ."),
            "pyproject-only projects should install via pip install ., got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("# TODO: no Python entry point detected — set CMD manually"),
            "projects without a detected entry point should contain an explicit TODO comment, got: {dockerfile}"
        );
    }

    #[test]
    fn generate_dockerfile_for_poetry_django_project_uses_poetry_install_and_runserver() {
        let repo = create_repo();
        fs::write(
            repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        fs::write(repo.path().join("poetry.lock"), "[[package]]\n")
            .unwrap_or_else(|err| panic!("failed to write poetry.lock: {err}"));
        fs::write(repo.path().join("manage.py"), "print('django')\n")
            .unwrap_or_else(|err| panic!("failed to write manage.py: {err}"));

        let dockerfile = PythonProfile
            .generate_dockerfile(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("RUN pip install --no-cache-dir poetry"),
            "Poetry projects should install Poetry inside the image, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("COPY pyproject.toml poetry.lock ./"),
            "Poetry projects should copy both pyproject.toml and poetry.lock, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("CMD [\"python\", \"manage.py\", \"runserver\", \"0.0.0.0:8080\"]"),
            "Django projects should run via manage.py runserver, got: {dockerfile}"
        );
    }

    #[test]
    fn generate_dockerfile_for_uv_project_uses_uv_sync_and_script_entrypoint() {
        let repo = create_repo();
        fs::write(
            repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        fs::write(repo.path().join("uv.lock"), "version = 1\n")
            .unwrap_or_else(|err| panic!("failed to write uv.lock: {err}"));
        fs::write(repo.path().join("app.py"), "print('hello')\n")
            .unwrap_or_else(|err| panic!("failed to write app.py: {err}"));

        let dockerfile = PythonProfile
            .generate_dockerfile(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected Dockerfile generation to succeed: {err}"));

        assert!(
            dockerfile.contains("RUN pip install --no-cache-dir uv"),
            "uv projects should install uv in the image, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("RUN uv sync --frozen --no-dev"),
            "uv projects should sync dependencies with uv sync, got: {dockerfile}"
        );
        assert!(
            dockerfile.contains("CMD [\"python\", \"app.py\"]"),
            "generic Python script projects should execute the detected script directly, got: {dockerfile}"
        );
    }

    #[test]
    fn build_plan_captures_python_detection_details() {
        let repo = create_repo();
        fs::write(
            repo.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap_or_else(|err| panic!("failed to write pyproject.toml: {err}"));
        fs::write(repo.path().join("uv.lock"), "version = 1\n")
            .unwrap_or_else(|err| panic!("failed to write uv.lock: {err}"));
        fs::write(repo.path().join("app.py"), "print('hello')\n")
            .unwrap_or_else(|err| panic!("failed to write app.py: {err}"));

        let plan = PythonProfile
            .build_plan(repo.path(), false)
            .unwrap_or_else(|err| panic!("expected BuildPlan generation to succeed: {err}"));

        assert_eq!(
            plan.stack, "python",
            "Python BuildPlan should record a stable stack identifier"
        );
        assert_eq!(
            plan.builder_image.as_deref(),
            Some(PYTHON_BASE_IMAGE),
            "Python BuildPlan should capture the selected base image"
        );
        assert_eq!(
            plan.run_command.as_deref(),
            Some("python app.py"),
            "Python BuildPlan should expose the inferred runtime command"
        );
        assert!(
            plan.dockerfile.contains("RUN uv sync --frozen --no-dev"),
            "BuildPlan should still carry the rendered Dockerfile contents, got: {}",
            plan.dockerfile
        );
    }
}
