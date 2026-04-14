// commands/update.rs — `tond update` subcommand handler
//
// Strategy:
//   1. Download the update.sh bundled in the latest (or specified) release
//      tarball from GitHub.  This ensures the update logic always matches the
//      release being installed, even if the locally-installed script is stale.
//   2. Run it under sudo (or directly, if already root).
//
// The script is extracted from the release tarball rather than pulled from the
// raw main branch, so the logic is stable and matches the target artefact.

use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result, bail};

const LATEST_TARBALL_URL: &str =
    "https://github.com/tahayvr/tond/releases/latest/download/tond-linux-x86_64.tar.gz";
const FALLBACK_SCRIPT: &str = "/opt/tond/bin/update.sh";

/// Build the tarball URL for a specific version (without leading `v`).
fn versioned_tarball_url(version: &str) -> String {
    format!("https://github.com/tahayvr/tond/releases/download/v{version}/tond-linux-x86_64.tar.gz")
}

/// Handle `tond update [OPTIONS]`.
///
/// Downloads update.sh from the appropriate GitHub release tarball and
/// executes it.  Falls back to the locally-installed script at
/// `/opt/tond/bin/update.sh` only when the download cannot be completed (e.g.
/// the host has no internet access).
pub fn handle(
    version: Option<String>,
    dry_run: bool,
    skip_backup: bool,
    force: bool,
) -> Result<()> {
    // Normalize version: strip any leading "v" so it matches what update.sh expects.
    let version = version.map(|v| v.trim_start_matches('v').to_string());

    // Build the argument list that will be forwarded to update.sh.
    let mut script_args: Vec<String> = vec![];
    if let Some(ref v) = version {
        script_args.push("--version".to_string());
        script_args.push(v.clone());
    }
    if dry_run {
        script_args.push("--dry-run".to_string());
    }
    if skip_backup {
        script_args.push("--skip-backup".to_string());
    }
    if force {
        script_args.push("--force".to_string());
    }

    // --- Pre-flight: must run as root (directly or via sudo) ---
    let is_root = is_running_as_root();
    if !is_root && !sudo_available() {
        bail!(
            "tond update requires root privileges.\n\
             Run as root, or ensure `sudo` is in PATH and you have sudo access.\n\
             Alternatively, run the update script directly:\n\
             \n  sudo {FALLBACK_SCRIPT}"
        );
    }

    // --- Create a temp directory manually (no external crates needed) ---
    let tmp_dir = create_temp_dir().context("Failed to create temporary directory")?;

    // --- Try to obtain a fresh update.sh from the release tarball ---
    let script_path = match download_update_script(&tmp_dir, version.as_deref()) {
        Ok(p) => {
            println!("Using update script from release tarball.");
            p
        }
        Err(e) => {
            // Fall back to the locally-installed script.
            let fallback = Path::new(FALLBACK_SCRIPT);
            if !fallback.exists() {
                // Best-effort cleanup before bailing.
                let _ = std::fs::remove_dir_all(&tmp_dir);
                bail!(
                    "Could not download update script ({e:#}), and the local fallback \
                     does not exist at {FALLBACK_SCRIPT}.\n\
                     Is Tond installed?  Try: curl -fsSL https://tond.io/install | sudo bash"
                );
            }
            eprintln!(
                "Warning: could not download update script ({e:#}).\n\
                 Falling back to installed script at {FALLBACK_SCRIPT} (may be outdated)."
            );
            fallback.to_path_buf()
        }
    };

    println!("Updating Tond...");

    let status = run_script(&script_path, &script_args, is_root)
        .with_context(|| format!("Failed to execute {}", script_path.display()))?;

    // Cleanup temp dir — best effort; it will be cleaned by the OS on reboot otherwise.
    let _ = std::fs::remove_dir_all(&tmp_dir);

    if status.success() {
        println!("Update completed successfully.");
    } else {
        let code = status.code().unwrap_or(1);
        bail!("Update script exited with status {code}");
    }

    Ok(())
}

/// Run the script, prepending `sudo` if we are not already root.
fn run_script(
    script: &Path,
    args: &[String],
    is_root: bool,
) -> std::io::Result<process::ExitStatus> {
    if is_root {
        process::Command::new(script).args(args).status()
    } else {
        process::Command::new("sudo")
            .arg(script)
            .args(args)
            .status()
    }
}

/// Download `update.sh` from the appropriate release tarball into `dir`.
///
/// Returns the path to the extracted script.
fn download_update_script(dir: &Path, version: Option<&str>) -> Result<PathBuf> {
    let url = match version {
        Some(v) => versioned_tarball_url(v),
        None => LATEST_TARBALL_URL.to_string(),
    };

    let tarball_path = dir.join("tond.tar.gz");

    // Download
    let status = process::Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&tarball_path)
        .arg(&url)
        .status()
        .context("Failed to run curl — is it installed?")?;

    if !status.success() {
        bail!("curl failed to download {url}");
    }

    // Extract only the update.sh file from the tarball.
    let extract_status = process::Command::new("tar")
        .args(["-xzf"])
        .arg(&tarball_path)
        .args(["-C"])
        .arg(dir)
        .arg("tond-linux-x86_64/update.sh")
        .status()
        .context("Failed to run tar")?;

    if !extract_status.success() {
        bail!("tar failed to extract update.sh from {url}");
    }

    let script = dir.join("tond-linux-x86_64/update.sh");
    if !script.exists() {
        bail!("update.sh not found in tarball at expected path");
    }

    // Make it executable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script)
            .context("Failed to stat extracted update.sh")?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).context("Failed to chmod extracted update.sh")?;
    }

    Ok(script)
}

/// Returns `true` when the effective UID is 0 (root).
fn is_running_as_root() -> bool {
    // `id -u` is available on all POSIX systems and avoids a libc dependency.
    process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

/// Returns `true` if `sudo` is found on PATH.
fn sudo_available() -> bool {
    process::Command::new("which")
        .arg("sudo")
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Create a temporary directory using `mktemp -d` (POSIX, no external crates).
fn create_temp_dir() -> Result<PathBuf> {
    let output = process::Command::new("mktemp")
        .arg("-d")
        .output()
        .context("Failed to run mktemp")?;

    if !output.status.success() {
        bail!("mktemp failed");
    }

    let path = String::from_utf8(output.stdout).context("mktemp output is not valid UTF-8")?;

    Ok(PathBuf::from(path.trim()))
}
