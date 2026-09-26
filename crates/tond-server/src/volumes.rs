// volumes.rs — host path confinement for every bind mount Tond creates
//
// A bind mount is a hole from a container into the host. If a caller can pick
// the host side freely (`host_path: "/"`), deploying the app hands them root on
// the VPS. So users never choose host paths directly any more: they name a
// volume (or let Tond derive the name from the container path) and Tond maps
// it into a directory it owns:
//
//   apps       <data>/volumes/<app-name>/<volume-name>
//   cron jobs  <data>/volumes/_cron/<job-id>/<volume-name>    (or an app's volume)
//   compose    <data>/volumes/<app-name>/<relative path>
//
// (`<data>` is TOND_DATA_DIR, default /opt/tond/data. App names are
// alphanumeric + hyphen, so `_cron` can never collide with an app.)
//
// Two layers of checks:
//   1. Lexical validation when the volume is configured (API time) so the
//      user gets an immediate, specific error.
//   2. `prepare_host_dir` right before the container starts: walk the path one
//      component at a time from the confinement root, refusing symlinks and
//      non-directories, creating what is missing, then canonicalize and check
//      containment. This catches a container that planted a symlink inside a
//      volume directory to escape on the next deploy.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use tond_common::{CronVolume, CronVolumeRequest, Volume, VolumeRequest};

/// Directory under the data dir that holds every volume.
const VOLUMES_DIR: &str = "volumes";
/// Directory under `volumes/` for cron jobs' own volumes.
const CRON_DIR: &str = "_cron";
/// Longest accepted volume name / container path.
const MAX_NAME_LEN: usize = 64;
const MAX_CONTAINER_PATH_LEN: usize = 255;

/// Tond's data directory (`TOND_DATA_DIR`, default `/opt/tond/data`).
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string()))
}

/// `<data>/volumes`
pub fn volumes_root(data_dir: &Path) -> PathBuf {
    data_dir.join(VOLUMES_DIR)
}

/// `<data>/volumes/<app>` — the only host directory an app's containers may mount from.
pub fn app_volume_dir(data_dir: &Path, app_name: &str) -> PathBuf {
    volumes_root(data_dir).join(app_name)
}

/// `<data>/volumes/_cron/<job-id>` — where a cron job's own volumes live.
pub fn cron_volume_dir(data_dir: &Path, job_id: &str) -> PathBuf {
    volumes_root(data_dir).join(CRON_DIR).join(job_id)
}

/// Validate the mount point inside the container.
///
/// It must be absolute and plain: no `..`, and none of the characters Docker's
/// `host:container[:options]` bind syntax treats specially, so a path can't
/// smuggle in mount options.
pub fn validate_container_path(path: &str) -> Result<(), String> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(format!(
            "Container path '{path}' must be an absolute path inside the container, e.g. /data"
        ));
    }
    if path == "/" {
        return Err(
            "Container path cannot be '/'; mount the volume at a directory such as /data"
                .to_string(),
        );
    }
    if path.len() > MAX_CONTAINER_PATH_LEN {
        return Err(format!(
            "Container path is too long (max {MAX_CONTAINER_PATH_LEN} characters)"
        ));
    }
    if path.chars().any(|c| c == ':' || c == ',' || c.is_control()) {
        return Err(format!(
            "Container path '{path}' contains ':' ',' or control characters, which Docker would read as mount options"
        ));
    }
    // Check raw segments: `Path::components` silently drops interior `.`.
    if path.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(format!(
            "Container path '{path}' must not contain '.' or '..' segments"
        ));
    }
    Ok(())
}

/// Validate a volume name: one directory name made of letters, digits, `.`,
/// `_` and `-`, not starting with `.` (so never `.` or `..`).
pub fn validate_volume_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Volume name cannot be empty".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "Volume name is too long (max {MAX_NAME_LEN} characters)"
        ));
    }
    if name.starts_with('.') {
        return Err(format!("Volume name '{name}' cannot start with '.'"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "Volume name '{name}' may only contain letters, digits, '.', '_' and '-' (no slashes)"
        ));
    }
    Ok(())
}

/// Derive a volume name from a container path: `/var/lib/data` → `var_lib_data`.
pub fn default_volume_name(container_path: &str) -> String {
    let name: String = container_path
        .trim_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_start_matches('.').to_string();
    let name: String = name.chars().take(MAX_NAME_LEN).collect();
    if name.is_empty() {
        "data".to_string()
    } else {
        name
    }
}

/// Resolve the volume name from a request: an explicit `name`, a legacy
/// `host_path` that already points at `<base>/<name>`, or a name derived from
/// the container path. Any other `host_path` is refused.
fn requested_name(
    base_dir: &Path,
    name: Option<&str>,
    host_path: Option<&str>,
    container_path: &str,
) -> Result<String, String> {
    if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
        validate_volume_name(name)?;
        return Ok(name.to_string());
    }

    if let Some(host_path) = host_path.map(str::trim).filter(|p| !p.is_empty()) {
        let path = Path::new(host_path);
        let plain = path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
        if plain
            && let Ok(rest) = path.strip_prefix(base_dir)
            && let Some(name) = rest.to_str()
            && validate_volume_name(name).is_ok()
        {
            return Ok(name.to_string());
        }
        return Err(format!(
            "Host path '{host_path}' is not allowed. Volumes are stored inside {}; \
             give the volume a name (e.g. \"data\") instead of a host path.",
            base_dir.display()
        ));
    }

    Ok(default_volume_name(container_path))
}

/// Turn an app volume request into a stored [`Volume`] under `<data>/volumes/<app>`.
pub fn app_volume(data_dir: &Path, app_name: &str, req: &VolumeRequest) -> Result<Volume, String> {
    let container_path = req.container_path.trim();
    validate_container_path(container_path)?;
    let base = app_volume_dir(data_dir, app_name);
    let name = requested_name(
        &base,
        req.name.as_deref(),
        req.host_path.as_deref(),
        container_path,
    )?;
    Ok(Volume {
        host_path: base.join(&name).to_string_lossy().into_owned(),
        name,
        container_path: container_path.to_string(),
    })
}

/// Validate a full list of app volume requests (no duplicate mount points or names).
pub fn app_volumes(
    data_dir: &Path,
    app_name: &str,
    reqs: &[VolumeRequest],
) -> Result<Vec<Volume>, String> {
    let mut out: Vec<Volume> = Vec::with_capacity(reqs.len());
    for req in reqs {
        let vol = app_volume(data_dir, app_name, req)?;
        ensure_unique(&out, &vol.name, &vol.container_path)?;
        out.push(vol);
    }
    Ok(out)
}

fn ensure_unique(existing: &[Volume], name: &str, container_path: &str) -> Result<(), String> {
    if existing.iter().any(|v| v.container_path == container_path) {
        return Err(format!("A volume is already mounted at {container_path}"));
    }
    if existing.iter().any(|v| v.name == name) {
        return Err(format!(
            "A volume named '{name}' already exists; choose a different name"
        ));
    }
    Ok(())
}

/// Add one volume to an app's existing list.
pub fn add_app_volume(
    data_dir: &Path,
    app_name: &str,
    existing: &[Volume],
    req: &VolumeRequest,
) -> Result<Volume, String> {
    let vol = app_volume(data_dir, app_name, req)?;
    ensure_unique(existing, &vol.name, &vol.container_path)?;
    Ok(vol)
}

/// Turn cron volume requests into stored [`CronVolume`]s. A volume either
/// belongs to the job (`<data>/volumes/_cron/<job-id>/<name>`) or reuses a
/// volume of an existing app (`app` set). `app_exists` checks app names.
pub fn cron_volumes(
    data_dir: &Path,
    job_id: &str,
    reqs: &[CronVolumeRequest],
    app_exists: impl Fn(&str) -> bool,
) -> Result<Vec<CronVolume>, String> {
    let mut out: Vec<CronVolume> = Vec::with_capacity(reqs.len());
    for req in reqs {
        let container_path = req.container_path.trim();
        validate_container_path(container_path)?;
        let app = req
            .app
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string);
        let base = match &app {
            Some(app) => {
                if !app_exists(app) {
                    return Err(format!("App '{app}' does not exist"));
                }
                app_volume_dir(data_dir, app)
            }
            None => cron_volume_dir(data_dir, job_id),
        };
        let name = requested_name(
            &base,
            req.name.as_deref(),
            req.host_path.as_deref(),
            container_path,
        )?;
        if out.iter().any(|v| v.container_path == container_path) {
            return Err(format!("A volume is already mounted at {container_path}"));
        }
        out.push(CronVolume {
            host_path: base.join(&name).to_string_lossy().into_owned(),
            app,
            name,
            container_path: container_path.to_string(),
        });
    }
    Ok(out)
}

/// Create (if needed) and verify the host directory `root/<relative>`.
///
/// `root` is the confinement root (e.g. `<data>/volumes/<app>`) and must end up
/// inside `<data>/volumes`. Every component of `relative` is checked with
/// `symlink_metadata`: symlinks are refused and missing directories created one
/// at a time, so nothing is ever created outside the root. If `allow_file` is
/// set the final component may be an existing regular file (compose binds a
/// single config file this way). Returns the canonical path.
pub fn prepare_host_dir(
    data_dir: &Path,
    root: &Path,
    relative: &Path,
    allow_file: bool,
) -> Result<PathBuf> {
    let volumes = volumes_root(data_dir);
    if !volumes.exists() {
        // Same mode install.sh gives it: root-only at the top level; the
        // per-volume directories below keep their own modes for containers.
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&volumes)?;
    }
    let volumes = volumes.canonicalize()?;

    // Create the root below <data>/volumes one component at a time too, so a
    // planted symlink at root level can't redirect it.
    let root_rel = root
        .strip_prefix(volumes_root(data_dir))
        .map_err(|_| anyhow::anyhow!("{} is not inside {}", root.display(), volumes.display()))?;
    let mut current = volumes.clone();
    let mut components: Vec<std::ffi::OsString> = Vec::new();
    for c in root_rel.components().chain(relative.components()) {
        match c {
            Component::Normal(part) => components.push(part.to_os_string()),
            Component::CurDir => {}
            _ => bail!(
                "Path '{}' escapes the volume directory {}",
                relative.display(),
                root.display()
            ),
        }
    }
    let last = components.len().saturating_sub(1);
    for (i, part) in components.iter().enumerate() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => bail!(
                "{} is a symbolic link; Tond refuses to mount it because it could point outside the volume directory",
                current.display()
            ),
            Ok(meta) if meta.is_dir() => {}
            Ok(meta) if allow_file && i == last && meta.is_file() => {}
            Ok(_) => bail!("{} exists but is not a directory", current.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(e) => return Err(e.into()),
        }
    }

    let canonical = current.canonicalize()?;
    let canonical_root = volumes.join(root_rel).canonicalize()?;
    if !canonical.starts_with(&canonical_root) || !canonical_root.starts_with(&volumes) {
        bail!(
            "{} resolves outside the volume directory {}",
            current.display(),
            root.display()
        );
    }
    Ok(canonical)
}

/// Build Docker bind strings (`host:container`) for an app's stored volumes,
/// re-deriving and re-verifying every host path from the volume name.
pub fn app_binds(data_dir: &Path, app_name: &str, volumes: &[Volume]) -> Result<Vec<String>> {
    let root = app_volume_dir(data_dir, app_name);
    volumes
        .iter()
        .map(|vol| {
            validate_container_path(&vol.container_path).map_err(anyhow::Error::msg)?;
            let name = if vol.name.is_empty() {
                default_volume_name(&vol.container_path)
            } else {
                vol.name.clone()
            };
            validate_volume_name(&name).map_err(anyhow::Error::msg)?;
            let host = prepare_host_dir(data_dir, &root, Path::new(&name), false)?;
            Ok(format!("{}:{}", host.display(), vol.container_path))
        })
        .collect()
}

/// Build Docker bind strings for a cron job's stored volumes.
pub fn cron_binds(data_dir: &Path, job_id: &str, volumes: &[CronVolume]) -> Result<Vec<String>> {
    volumes
        .iter()
        .map(|vol| {
            validate_container_path(&vol.container_path).map_err(anyhow::Error::msg)?;
            validate_volume_name(&vol.name).map_err(anyhow::Error::msg)?;
            let root = match vol.app.as_deref() {
                Some(app) => {
                    if !app.chars().all(|c| c.is_alphanumeric() || c == '-') || app.is_empty() {
                        bail!("Invalid app name '{app}' in cron volume");
                    }
                    app_volume_dir(data_dir, app)
                }
                None => cron_volume_dir(data_dir, job_id),
            };
            let host = prepare_host_dir(data_dir, &root, Path::new(&vol.name), false)?;
            Ok(format!("{}:{}", host.display(), vol.container_path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(container_path: &str, name: Option<&str>, host_path: Option<&str>) -> VolumeRequest {
        VolumeRequest {
            container_path: container_path.to_string(),
            name: name.map(str::to_string),
            host_path: host_path.map(str::to_string),
        }
    }

    #[test]
    fn container_paths_must_be_absolute_and_free_of_mount_syntax() {
        assert!(validate_container_path("/data").is_ok());
        assert!(validate_container_path("/var/lib/postgresql/data").is_ok());
        for bad in [
            "",
            "data",
            "/",
            "/data:/etc",
            "/data:ro",
            "/data,readonly",
            "/data/../etc",
            "/./data",
            "/da\nta",
        ] {
            assert!(
                validate_container_path(bad).is_err(),
                "container path {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn volume_names_are_single_safe_segments() {
        assert!(validate_volume_name("data").is_ok());
        assert!(validate_volume_name("pg_data-1.2").is_ok());
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "../x",
            "a b",
            "x:y",
            &"a".repeat(65),
        ] {
            assert!(
                validate_volume_name(bad).is_err(),
                "volume name {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn default_names_are_derived_from_the_container_path() {
        assert_eq!(default_volume_name("/data"), "data");
        assert_eq!(default_volume_name("/data/cache"), "data_cache");
        assert_eq!(default_volume_name("/.config"), "config");
        assert_eq!(default_volume_name("/a b"), "a_b");
    }

    #[test]
    fn app_volumes_are_mapped_under_the_app_directory() {
        let data = Path::new("/opt/tond/data");
        let vol = app_volume(data, "web", &req("/data/cache", None, None))
            .unwrap_or_else(|e| panic!("valid volume rejected: {e}"));
        assert_eq!(vol.name, "data_cache");
        assert_eq!(vol.host_path, "/opt/tond/data/volumes/web/data_cache");

        let named = app_volume(data, "web", &req("/srv", Some("uploads"), None))
            .unwrap_or_else(|e| panic!("named volume rejected: {e}"));
        assert_eq!(named.host_path, "/opt/tond/data/volumes/web/uploads");
    }

    #[test]
    fn host_paths_outside_the_app_directory_are_rejected() {
        let data = Path::new("/opt/tond/data");
        for bad in [
            "/",
            "/etc",
            "/var/run/docker.sock",
            "/opt/tond/data",
            "/opt/tond/data/volumes",
            "/opt/tond/data/volumes/web",
            "/opt/tond/data/volumes/other/data",
            "/opt/tond/data/volumes/web/../other",
            "/opt/tond/data/volumes/web/a/b",
            "relative/path",
        ] {
            let err = app_volume(data, "web", &req("/host", None, Some(bad)))
                .err()
                .unwrap_or_else(|| panic!("host_path {bad:?} must be rejected"));
            assert!(
                err.contains("not allowed"),
                "error for {bad:?} should explain the rule: {err}"
            );
        }
    }

    #[test]
    fn a_host_path_already_inside_the_app_directory_is_accepted() {
        let data = Path::new("/opt/tond/data");
        let vol = app_volume(
            data,
            "web",
            &req("/data", None, Some("/opt/tond/data/volumes/web/data")),
        )
        .unwrap_or_else(|e| panic!("in-directory host path rejected: {e}"));
        assert_eq!(vol.name, "data");
    }

    #[test]
    fn duplicate_mount_points_and_names_are_rejected() {
        let data = Path::new("/opt/tond/data");
        assert!(
            app_volumes(
                data,
                "web",
                &[req("/data", None, None), req("/data", Some("other"), None)]
            )
            .is_err()
        );
        assert!(
            app_volumes(
                data,
                "web",
                &[req("/a", Some("x"), None), req("/b", Some("x"), None)]
            )
            .is_err()
        );
    }

    #[test]
    fn cron_volumes_live_under_the_job_or_an_existing_app() {
        let data = Path::new("/opt/tond/data");
        let reqs = vec![
            CronVolumeRequest {
                app: None,
                name: Some("scratch".to_string()),
                host_path: None,
                container_path: "/scratch".to_string(),
            },
            CronVolumeRequest {
                app: Some("web".to_string()),
                name: Some("uploads".to_string()),
                host_path: None,
                container_path: "/uploads".to_string(),
            },
        ];
        let vols = cron_volumes(data, "job-1", &reqs, |a| a == "web")
            .unwrap_or_else(|e| panic!("valid cron volumes rejected: {e}"));
        assert_eq!(
            vols[0].host_path,
            "/opt/tond/data/volumes/_cron/job-1/scratch"
        );
        assert_eq!(vols[1].host_path, "/opt/tond/data/volumes/web/uploads");

        let missing_app = vec![CronVolumeRequest {
            app: Some("nope".to_string()),
            name: None,
            host_path: None,
            container_path: "/x".to_string(),
        }];
        assert!(cron_volumes(data, "job-1", &missing_app, |_| false).is_err());

        let escape = vec![CronVolumeRequest {
            app: None,
            name: None,
            host_path: Some("/".to_string()),
            container_path: "/host".to_string(),
        }];
        assert!(cron_volumes(data, "job-1", &escape, |_| true).is_err());
    }

    #[test]
    fn prepare_host_dir_creates_directories_inside_the_root() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let data = tmp.path();
        let root = app_volume_dir(data, "web");
        let path = prepare_host_dir(data, &root, Path::new("data"), false)
            .unwrap_or_else(|e| panic!("prepare_host_dir failed: {e}"));
        assert!(path.is_dir());
        assert!(path.ends_with("volumes/web/data"));
    }

    #[cfg(unix)]
    #[test]
    fn prepare_host_dir_refuses_symlinks_planted_inside_the_volume_tree() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let data = tmp.path();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let root = app_volume_dir(data, "web");
        std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::os::unix::fs::symlink(&outside, root.join("data"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));

        let err = prepare_host_dir(data, &root, Path::new("data"), false)
            .err()
            .unwrap_or_else(|| panic!("a symlinked volume directory must be refused"));
        assert!(err.to_string().contains("symbolic link"), "{err}");

        // Nothing may be created through the link either.
        assert!(prepare_host_dir(data, &root, Path::new("data/sub"), false).is_err());
        assert!(!outside.join("sub").exists());
    }

    #[cfg(unix)]
    #[test]
    fn prepare_host_dir_refuses_a_symlinked_app_root() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let data = tmp.path();
        std::fs::create_dir_all(volumes_root(data)).unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::os::unix::fs::symlink("/", app_volume_dir(data, "web"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));
        assert!(
            prepare_host_dir(data, &app_volume_dir(data, "web"), Path::new("etc"), false).is_err()
        );
    }

    #[test]
    fn prepare_host_dir_rejects_parent_components() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let data = tmp.path();
        let root = app_volume_dir(data, "web");
        assert!(prepare_host_dir(data, &root, Path::new("../other"), false).is_err());
        assert!(prepare_host_dir(data, &root, Path::new("/etc"), false).is_err());
    }

    #[test]
    fn app_binds_rederive_host_paths_from_names() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let data = tmp.path();
        // A tampered host_path in the stored JSON is ignored: the bind comes from the name.
        let stored = vec![Volume {
            name: "data".to_string(),
            host_path: "/".to_string(),
            container_path: "/data".to_string(),
        }];
        let binds = app_binds(data, "web", &stored).unwrap_or_else(|e| panic!("binds: {e}"));
        assert_eq!(binds.len(), 1);
        assert!(binds[0].ends_with("volumes/web/data:/data"), "{}", binds[0]);
    }
}
