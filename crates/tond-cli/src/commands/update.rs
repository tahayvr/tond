// commands/update.rs — `tond update`: update Tond, or roll it back
//
// The normal path is the same as the dashboard's (System → Updates): ask
// tond-server (POST /api/v1/system/update, or …/rollback) to take a full snapshot
// and start the update in a transient systemd unit, then follow its progress
// (GET /api/v1/system/update) across the server's restart and exit with the
// outcome. `--check` only reports whether a newer release exists.
//
// `--direct` does the update itself, without the server: download the release
// tarball for this host's architecture (tond-linux-x86_64 or
// tond-linux-aarch64), verify it against the release's .sha256 and its
// minisign signature (signature.rs: fail closed), extract it and run its own
// update.sh with --bundle, so the update logic always matches the release
// being installed. The transient unit runs exactly this; people use it when
// tond-server is down (the CLI falls back to it, with a warning, when the
// local socket doesn't answer). A download that fails verification is never
// replaced by the installed script.

use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::client::TondClient;
use crate::output::{Output, note, outln};
use crate::signature::{self, Verdict};

const FALLBACK_SCRIPT: &str = "/opt/tond/bin/update.sh";
/// How long to follow a run before giving up (a snapshot of a big server
/// plus a slow download can take a while).
const FOLLOW_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// `tond update` options.
#[derive(Debug, Clone, Default)]
pub struct UpdateArgs {
    pub version: Option<String>,
    pub dry_run: bool,
    pub skip_snapshot: bool,
    pub force: bool,
    pub rollback: bool,
    pub check: bool,
    pub direct: bool,
    pub run_id: Option<String>,
    pub actor: Option<String>,
    pub snapshot_id: Option<String>,
}

/// Handle `tond update`. `connect` opens the API client (local socket unless
/// --server); `remote` is true for --server connections (no direct fallback).
pub async fn handle(
    connect: impl FnOnce() -> Result<TondClient>,
    remote: bool,
    out: &Output,
    mut args: UpdateArgs,
) -> Result<()> {
    args.version = args
        .version
        .map(|v| v.trim().trim_start_matches('v').to_string());

    if args.direct || args.dry_run {
        if remote {
            bail!(
                "--direct and --dry-run run on the server itself; drop --server and run them there as root."
            );
        }
        return direct(out, &args);
    }

    let client = match connect() {
        Ok(c) => c,
        Err(e) if !remote && !args.check => {
            note!(
                out,
                "Warning: tond-server isn't answering ({e:#}).\n\
                 Updating directly instead, without taking a snapshot first."
            );
            return direct(out, &args);
        }
        Err(e) => return Err(e),
    };

    if args.check {
        let overview = client.check_updates().await?;
        out.json(&overview)?;
        print_overview(out, &overview);
        return Ok(());
    }

    let status = if args.rollback {
        client.start_rollback(args.skip_snapshot).await?
    } else {
        client
            .start_update(args.version.as_deref(), args.skip_snapshot, args.force)
            .await?
    };
    let run_id = status["id"].as_str().unwrap_or_default().to_string();
    outln!(
        out,
        "{} Tond {} → {}{}",
        if args.rollback {
            "Rolling back"
        } else {
            "Updating"
        },
        status["from_version"].as_str().unwrap_or("?"),
        status["to_version"].as_str().unwrap_or("?"),
        if args.skip_snapshot {
            " (no snapshot, as asked)"
        } else {
            " (taking a full snapshot first)"
        }
    );
    follow(&client, out, &run_id).await
}

/// Print the version check.
fn print_overview(out: &Output, o: &Value) {
    let current = o["current_version"].as_str().unwrap_or("?");
    outln!(out, "Running: Tond {current}");
    match o["latest"]["version"].as_str() {
        Some(latest) if o["update_available"].as_bool() == Some(true) => {
            outln!(out, "Available: Tond {latest}");
            if let Some(changelog) = o["changelog"].as_array() {
                for release in changelog {
                    outln!(
                        out,
                        "\n── {} ({})",
                        release["name"].as_str().unwrap_or_default(),
                        release["published_at"]
                            .as_str()
                            .and_then(|d| d.get(..10))
                            .unwrap_or("unreleased")
                    );
                    for line in release["body"]
                        .as_str()
                        .unwrap_or_default()
                        .lines()
                        .take(40)
                    {
                        outln!(out, "  {line}");
                    }
                }
            }
            outln!(out, "\nUpdate with: sudo tond update");
        }
        Some(latest) => outln!(out, "Up to date (latest release: {latest})."),
        None => outln!(out, "No release information yet."),
    }
    if let Some(err) = o["check"]["error"].as_str() {
        outln!(out, "The last check failed: {err}");
    }
    if let Some(rb) = o["rollback"].as_object() {
        outln!(
            out,
            "Rollback to {} is available (sudo tond update --rollback).",
            rb.get("to_version").and_then(Value::as_str).unwrap_or("?")
        );
    }
}

/// The lines of `new` (a log tail) not yet printed, given the lines printed
/// so far (`seen`, also a tail). Finds where the two overlap; when they don't
/// (the log restarted or scrolled past), everything in `new` is new.
pub fn new_log_lines<'a>(seen: &[String], new: &'a [String]) -> &'a [String] {
    for k in (1..=seen.len().min(new.len())).rev() {
        if seen[seen.len() - k..] == new[..k] {
            return &new[k..];
        }
    }
    new
}

/// How a run ended, from its status.
#[derive(Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Running,
    Succeeded(String),
    Failed(String),
}

pub fn run_outcome(run: &Value, run_id: &str) -> RunOutcome {
    if run.is_null() || (!run_id.is_empty() && run["id"].as_str() != Some(run_id)) {
        return RunOutcome::Running;
    }
    let message = run["message"].as_str().unwrap_or_default().to_string();
    match run["effective_state"]
        .as_str()
        .or_else(|| run["state"].as_str())
    {
        Some("running") | None => RunOutcome::Running,
        Some("succeeded") => RunOutcome::Succeeded(message),
        Some("interrupted") => RunOutcome::Failed(format!(
            "The run was interrupted{}. Check: journalctl -u tond-update -u tond-server",
            if message.is_empty() {
                String::new()
            } else {
                format!(" ({message})")
            }
        )),
        Some(_) => RunOutcome::Failed(message),
    }
}

/// Follow a run until it finishes, printing its log. The server restarts in
/// the middle: failed requests are expected and retried.
async fn follow(client: &TondClient, out: &Output, run_id: &str) -> Result<()> {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut offline_since: Option<Instant> = None;
    loop {
        match client.get_update().await {
            Ok(overview) => {
                if let Some(since) = offline_since.take() {
                    outln!(
                        out,
                        "  (tond-server is back after {} s)",
                        since.elapsed().as_secs()
                    );
                }
                let run = &overview["run"];
                let log: Vec<String> = run["log"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if run["id"].as_str() == Some(run_id) || run_id.is_empty() {
                    for line in new_log_lines(&seen, &log) {
                        outln!(out, "  {line}");
                    }
                    seen = log;
                }
                match run_outcome(run, run_id) {
                    RunOutcome::Running => {}
                    RunOutcome::Succeeded(msg) => {
                        out.json(run)?;
                        outln!(out, "{msg}");
                        return Ok(());
                    }
                    RunOutcome::Failed(msg) => bail!("{msg}"),
                }
            }
            Err(_) => {
                if offline_since.is_none() {
                    outln!(out, "  (tond-server is restarting; waiting for it…)");
                    offline_since = Some(Instant::now());
                }
            }
        }
        if started.elapsed() > FOLLOW_TIMEOUT {
            bail!(
                "Still not finished after {} minutes. It may still be running: \
                 check `tond update --check`, System → Updates, or journalctl -u tond-update.",
                FOLLOW_TIMEOUT.as_secs() / 60
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ── Direct mode ──

/// Why a release could not be used.
enum DownloadError {
    /// Network trouble etc.: falling back to the installed script is fine.
    Unavailable(anyhow::Error),
    /// The download failed verification: stop, never fall back.
    Untrusted(anyhow::Error),
}

impl From<anyhow::Error> for DownloadError {
    fn from(e: anyhow::Error) -> Self {
        DownloadError::Unavailable(e)
    }
}

/// Release bundle name for this build's architecture, e.g. `tond-linux-aarch64`.
fn bundle_name() -> String {
    format!("tond-linux-{}", std::env::consts::ARCH)
}

/// Tarball URL of a release tag (e.g. `v0.2.0`).
fn tarball_url(tag: &str) -> String {
    let bundle = bundle_name();
    format!("https://github.com/tahayvr/tond/releases/download/{tag}/{bundle}.tar.gz")
}

/// The tag GitHub's `releases/latest` redirects to, from the final URL.
fn tag_from_latest_url(url: &str) -> Option<String> {
    let tag = url.trim().rsplit_once("/releases/tag/")?.1;
    let tag = tag.split(['?', '#', '/']).next()?;
    (tag.starts_with('v') && tag.len() > 1).then(|| tag.to_string())
}

/// Resolve the release tag to install: `v<version>`, or the latest release.
fn resolve_tag(version: Option<&str>) -> Result<String> {
    if let Some(v) = version {
        return Ok(format!("v{v}"));
    }
    let output = process::Command::new("curl")
        .args([
            "-fsSLI",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "https://github.com/tahayvr/tond/releases/latest",
        ])
        .output()
        .context("Failed to run curl — is it installed?")?;
    if !output.status.success() {
        bail!("could not look up the latest release");
    }
    tag_from_latest_url(&String::from_utf8_lossy(&output.stdout))
        .context("could not work out the latest release tag")
}

/// Download `url` to `dest`. `Ok(false)` when the server says 404.
fn download_optional(url: &str, dest: &Path) -> Result<bool> {
    let output = process::Command::new("curl")
        .args(["-sSL", "-o"])
        .arg(dest)
        .args(["-w", "%{http_code}"])
        .arg(url)
        .output()
        .context("Failed to run curl")?;
    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match code.as_str() {
        "200" if output.status.success() => Ok(true),
        "404" => Ok(false),
        _ => bail!("could not download {url} (HTTP {code})"),
    }
}

/// First field of a `sha256sum` line, if it is a SHA-256 hex digest.
fn parse_sha256_line(line: &str) -> Option<String> {
    let hash = line.split_whitespace().next()?.to_ascii_lowercase();
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

/// update.sh arguments for a direct run (after the mode-specific ones).
pub fn script_args(args: &UpdateArgs) -> Vec<String> {
    let mut out = Vec::new();
    if args.force {
        out.push("--force".to_string());
    }
    if args.dry_run {
        out.push("--dry-run".to_string());
    }
    for (flag, value) in [
        ("--run-id", &args.run_id),
        ("--actor", &args.actor),
        ("--snapshot-id", &args.snapshot_id),
    ] {
        if let Some(v) = value {
            out.push(flag.to_string());
            out.push(v.clone());
        }
    }
    out
}

/// Update (or roll back) without tond-server.
fn direct(out: &Output, args: &UpdateArgs) -> Result<()> {
    let is_root = is_running_as_root();
    if !is_root && !sudo_available() {
        bail!(
            "tond update --direct needs root. Run it as root or with sudo, or run \
             sudo {FALLBACK_SCRIPT} directly."
        );
    }

    if args.rollback {
        let mut cmd_args = vec!["--rollback".to_string()];
        cmd_args.extend(script_args_without_force(args));
        return run_and_report(out, Path::new(FALLBACK_SCRIPT), &cmd_args, is_root);
    }

    let tmp_dir = create_temp_dir().context("Failed to create temporary directory")?;
    let result = match download_bundle(out, &tmp_dir, args.version.as_deref()) {
        Ok(bundle) => {
            let mut cmd_args = vec![
                "--bundle".to_string(),
                bundle.to_string_lossy().into_owned(),
            ];
            cmd_args.extend(script_args(args));
            run_and_report(out, &bundle.join("update.sh"), &cmd_args, is_root)
        }
        Err(DownloadError::Untrusted(e)) => Err(e),
        Err(DownloadError::Unavailable(e)) => {
            let fallback = Path::new(FALLBACK_SCRIPT);
            if fallback.exists() {
                note!(
                    out,
                    "Warning: could not download the release ({e:#}).\n\
                     Falling back to the installed {FALLBACK_SCRIPT} (it downloads the release itself)."
                );
                let mut cmd_args = Vec::new();
                if let Some(v) = &args.version {
                    cmd_args.push("--version".to_string());
                    cmd_args.push(v.clone());
                }
                cmd_args.extend(script_args(args));
                run_and_report(out, fallback, &cmd_args, is_root)
            } else {
                Err(anyhow::anyhow!(
                    "Could not download the release ({e:#}), and {FALLBACK_SCRIPT} doesn't exist. \
                     Is Tond installed? Install with: curl -fsSL https://tond.io/install | sudo bash"
                ))
            }
        }
    };
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

fn script_args_without_force(args: &UpdateArgs) -> Vec<String> {
    script_args(args)
        .into_iter()
        .filter(|a| a != "--force")
        .collect()
}

fn run_and_report(out: &Output, script: &Path, args: &[String], is_root: bool) -> Result<()> {
    let mut cmd = if is_root {
        process::Command::new(script)
    } else {
        let mut sudo = process::Command::new("sudo");
        sudo.arg(script);
        sudo
    };
    cmd.args(args);
    // With --json the script's progress goes to stderr: stdout holds only
    // the JSON result.
    if out.is_json() {
        cmd.stdout(process::Stdio::from(std::io::stderr()));
    }
    let status = cmd
        .status()
        .with_context(|| format!("Failed to run {}", script.display()))?;
    if status.success() {
        out.json(&serde_json::json!({ "ok": true, "script": script.display().to_string() }))?;
        return Ok(());
    }
    match status.code() {
        Some(1) => bail!("The update did not complete (see the messages above)."),
        Some(2) => bail!("update.sh rejected the options (see above)."),
        Some(3) => bail!(
            "The update failed and so did the automatic rollback: Tond may be down. Follow the recovery steps above."
        ),
        Some(code) => bail!("update.sh exited with status {code}"),
        None => bail!("update.sh was killed"),
    }
}

/// Download, verify and extract the release bundle into `dir`.
fn download_bundle(
    out: &Output,
    dir: &Path,
    version: Option<&str>,
) -> Result<PathBuf, DownloadError> {
    let tag = resolve_tag(version)?;
    let url = tarball_url(&tag);
    let bundle = bundle_name();

    let tarball_path = dir.join("tond.tar.gz");
    let checksum_path = dir.join("tond.tar.gz.sha256");

    note!(out, "Downloading {url}");
    let status = process::Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&tarball_path)
        .arg(&url)
        .status()
        .context("Failed to run curl — is it installed?")?;
    if !status.success() {
        return Err(anyhow::anyhow!("curl failed to download {url}").into());
    }

    let status = process::Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&checksum_path)
        .arg(format!("{url}.sha256"))
        .status()
        .context("Failed to run curl")?;
    if !status.success() {
        return Err(anyhow::anyhow!("curl failed to download {url}.sha256").into());
    }
    let expected = std::fs::read_to_string(&checksum_path)
        .ok()
        .and_then(|s| parse_sha256_line(&s))
        .context("release checksum file is malformed")?;
    let output = process::Command::new("sha256sum")
        .arg(&tarball_path)
        .output()
        .context("Failed to run sha256sum")?;
    let actual = parse_sha256_line(&String::from_utf8_lossy(&output.stdout))
        .context("sha256sum produced no digest")?;
    if actual != expected {
        return Err(DownloadError::Untrusted(anyhow::anyhow!(
            "checksum mismatch for {url} (download corrupted or tampered with)"
        )));
    }
    note!(out, "Checksum verified.");

    let signature_path = dir.join("tond.tar.gz.minisig");
    let signature = if download_optional(&format!("{url}.minisig"), &signature_path)? {
        Some(std::fs::read_to_string(&signature_path).context("could not read the signature")?)
    } else {
        None
    };
    let tarball = std::fs::read(&tarball_path).context("could not read the download")?;
    match signature::check_release(
        &tarball,
        signature.as_deref(),
        &tag,
        signature::release_public_key(),
        signature::allow_unsigned_from_env(),
    ) {
        Ok(Verdict::Verified) => note!(out, "Signature of {tag} verified."),
        Ok(Verdict::Unverified(why)) => note!(out, "Warning: {why}."),
        Err(e) => return Err(DownloadError::Untrusted(e)),
    }

    let extract = process::Command::new("tar")
        .args(["-xzf"])
        .arg(&tarball_path)
        .args(["-C"])
        .arg(dir)
        .status()
        .context("Failed to run tar")?;
    if !extract.success() {
        return Err(anyhow::anyhow!("tar failed to extract {url}").into());
    }
    let bundle_dir = dir.join(&bundle);
    if !bundle_dir.join("update.sh").is_file() {
        return Err(anyhow::anyhow!("update.sh not found in the release bundle").into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = bundle_dir.join("update.sh");
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
    }
    Ok(bundle_dir)
}

/// Returns `true` when the effective UID is 0 (root).
fn is_running_as_root() -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tarball_url_names_the_bundle_for_this_architecture() {
        let arch = std::env::consts::ARCH;
        assert_eq!(
            tarball_url("v0.2.0"),
            format!(
                "https://github.com/tahayvr/tond/releases/download/v0.2.0/tond-linux-{arch}.tar.gz"
            )
        );
    }

    #[test]
    fn tag_from_latest_url_reads_the_redirect_target() {
        assert_eq!(
            tag_from_latest_url("https://github.com/tahayvr/tond/releases/tag/v0.2.0\n").as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(
            tag_from_latest_url("https://github.com/tahayvr/tond/releases"),
            None
        );
        assert_eq!(
            tag_from_latest_url("https://github.com/x/releases/tag/"),
            None
        );
    }

    #[test]
    fn parse_sha256_line_accepts_sha256sum_output_only() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_sha256_line(&format!("{hash}  tond-linux-x86_64.tar.gz\n")),
            Some(hash)
        );
        assert_eq!(parse_sha256_line("not-a-hash  file"), None);
        assert_eq!(parse_sha256_line(""), None);
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_unseen_log_lines_are_printed() {
        let seen = lines(&["a", "b", "c"]);
        assert_eq!(
            new_log_lines(&seen, &lines(&["b", "c", "d", "e"])),
            lines(&["d", "e"])
        );
        assert_eq!(
            new_log_lines(&seen, &lines(&["a", "b", "c"])),
            Vec::<String>::new()
        );
        // Nothing seen yet, or the log restarted: everything is new.
        assert_eq!(new_log_lines(&[], &lines(&["x"])), lines(&["x"]));
        assert_eq!(
            new_log_lines(&seen, &lines(&["x", "y"])),
            lines(&["x", "y"])
        );
        // Repeated lines: the longest overlap wins.
        let seen = lines(&["ok", "ok"]);
        assert_eq!(
            new_log_lines(&seen, &lines(&["ok", "ok", "done"])),
            lines(&["done"])
        );
    }

    #[test]
    fn a_run_is_finished_only_for_the_requested_id() {
        let running = json!({"id": "r1", "state": "running", "effective_state": "running"});
        assert_eq!(run_outcome(&running, "r1"), RunOutcome::Running);
        let other = json!({"id": "r0", "state": "succeeded", "effective_state": "succeeded"});
        assert_eq!(run_outcome(&other, "r1"), RunOutcome::Running);
        assert_eq!(run_outcome(&Value::Null, "r1"), RunOutcome::Running);
        let ok = json!({"id": "r1", "state": "succeeded", "effective_state": "succeeded", "message": "Updated"});
        assert_eq!(
            run_outcome(&ok, "r1"),
            RunOutcome::Succeeded("Updated".into())
        );
        let rolled = json!({"id": "r1", "state": "rolled_back", "effective_state": "rolled_back", "message": "back on 0.3.0"});
        assert_eq!(
            run_outcome(&rolled, "r1"),
            RunOutcome::Failed("back on 0.3.0".into())
        );
        let gone = json!({"id": "r1", "state": "running", "effective_state": "interrupted"});
        assert!(
            matches!(run_outcome(&gone, "r1"), RunOutcome::Failed(m) if m.contains("interrupted"))
        );
    }

    #[test]
    fn direct_runs_forward_the_run_details_to_update_sh() {
        let args = UpdateArgs {
            force: true,
            run_id: Some("r1".into()),
            actor: Some("admin@example.com".into()),
            snapshot_id: Some("s1".into()),
            ..UpdateArgs::default()
        };
        assert_eq!(
            script_args(&args),
            [
                "--force",
                "--run-id",
                "r1",
                "--actor",
                "admin@example.com",
                "--snapshot-id",
                "s1"
            ]
        );
        assert!(!script_args_without_force(&args).contains(&"--force".to_string()));
        assert!(script_args(&UpdateArgs::default()).is_empty());
    }
}
