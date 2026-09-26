// updates/run.rs — running an update (or a rollback) from the server
//
// tond-server can't replace its own binaries mid-request (and its systemd
// sandbox makes /opt/tond/bin read-only anyway), so the work runs outside it,
// in a transient systemd unit (`tond-update.service`, started with
// systemd-run) that outlives the server's restart:
//
//   update:    tond update --direct --version X …   (the installed CLI fetches
//              update.sh from release X, verifies checksum + signature, runs it)
//   rollback:  /opt/tond/bin/update.sh --rollback --backup <dir> …
//
// Before starting the unit the server takes a full snapshot (step
// "snapshot"). update.sh then stops tond-server, copies the database and the
// current binaries into <data>/backups/tond_backup_<time>/, swaps in the new
// release, starts it (migrations run), health-checks it, restarts tond-proxy,
// and on any failure puts the old binaries back — and the old database when
// migrations may have run — and starts that again (release/update.sh).
//
// Progress is a small JSON file, <data>/update/status.json, written by the
// server for the snapshot step and by update.sh for the rest; the run's
// output goes to <data>/update/update.log. Both are files, not rows, because
// a failed update may restore an older copy of the database. The dashboard
// polls GET /api/system/update, which keeps answering across the restart.
//
// A rollback to the previous version is offered for ROLLBACK_WINDOW_SECS after
// an update: the backup directory update.sh left behind holds the previous
// binaries, dashboard and database (info.env describes it).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Name of the transient unit. One at a time: systemd refuses a second unit
/// with the same name while the first is active.
pub const UNIT: &str = "tond-update";
/// How long "Roll back to the previous version" is offered after an update.
pub const ROLLBACK_WINDOW_SECS: i64 = 7 * 24 * 3600;
/// A running run whose process is gone and that hasn't written its status for
/// this long was interrupted (killed, or the machine rebooted).
pub const STALE_SECS: i64 = 30;
/// Lines of update.log returned with the status.
pub const LOG_TAIL_LINES: usize = 60;

/// Set while this process is starting a run (between the request and the
/// status file saying so), so two clicks can't start two runs.
static STARTING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Update,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// The new version (or, for a rollback, the previous one) is running.
    Succeeded,
    /// Nothing was changed, or the rollback itself failed (the message says which).
    Failed,
    /// The update failed and the previous version is running again.
    RolledBack,
    /// The process doing the work disappeared (never written to the file).
    Interrupted,
}

impl RunState {
    pub fn is_finished(self) -> bool {
        !matches!(self, RunState::Running)
    }
}

/// <data>/update/status.json. update.sh writes the same fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatus {
    pub id: String,
    pub kind: RunKind,
    pub state: RunState,
    /// snapshot, download, backup, install, migrate, health, proxy, rollback, done
    pub step: String,
    pub from_version: String,
    pub to_version: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub finished_at: Option<i64>,
    #[serde(default)]
    pub message: String,
    /// The process doing the current step (tond-server during the snapshot,
    /// then update.sh).
    #[serde(default)]
    pub pid: Option<u32>,
    /// The automatic rollback put the pre-update database back.
    #[serde(default)]
    pub database_restored: bool,
}

/// The state to report: a "running" run whose process is gone and whose file
/// went quiet was interrupted.
pub fn effective_state(status: &RunStatus, pid_alive: impl Fn(u32) -> bool, now: i64) -> RunState {
    if status.state != RunState::Running {
        return status.state;
    }
    let alive = status.pid.is_some_and(&pid_alive);
    if !alive && now - status.updated_at > STALE_SECS {
        RunState::Interrupted
    } else {
        RunState::Running
    }
}

pub fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

// ── Files ──

#[derive(Debug, Clone)]
pub struct Paths {
    /// <data>/update: status.json and update.log.
    pub update_dir: PathBuf,
    /// <data>/backups: update.sh's tond_backup_<time> directories.
    pub backups_dir: PathBuf,
    /// Where tond-server, tond and update.sh are installed.
    pub bin_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let data = crate::auth::secret_file::data_dir();
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("/opt/tond/bin"));
        Self {
            update_dir: data.join("update"),
            backups_dir: data.join("backups"),
            bin_dir,
        }
    }

    pub fn status_file(&self) -> PathBuf {
        self.update_dir.join("status.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.update_dir.join("update.log")
    }
}

pub fn read_status(paths: &Paths) -> Option<RunStatus> {
    let text = std::fs::read_to_string(paths.status_file()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write status.json atomically (temp file + rename), root-only.
pub fn write_status(paths: &Paths, status: &RunStatus) -> Result<()> {
    create_private_dir(&paths.update_dir)?;
    let tmp = paths.update_dir.join(".status.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(status)?)?;
    std::fs::rename(&tmp, paths.status_file())?;
    Ok(())
}

fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Start a fresh update.log with a first line.
fn reset_log(paths: &Paths, first_line: &str) -> Result<()> {
    create_private_dir(&paths.update_dir)?;
    std::fs::write(paths.log_file(), format!("{first_line}\n"))?;
    Ok(())
}

fn append_log(paths: &Paths, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(paths.log_file())
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The last `n` lines of update.log, without terminal colour codes.
pub fn log_tail(paths: &Paths, n: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(paths.log_file()) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| strip_ansi(l))
        .collect()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

// ── Backups left by update.sh (rollback targets) ──

/// Whether the update that made a backup changed the database schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Migrated {
    Yes,
    No,
    /// Couldn't tell (e.g. the old server didn't report its schema).
    Unknown,
}

/// A `tond_backup_<time>` directory and its info.env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    /// "update" (made before an update) or "rollback" (made before a rollback).
    pub kind: String,
    pub from_version: String,
    pub to_version: String,
    pub created_at: i64,
    pub migrated: Migrated,
    pub has_binaries: bool,
}

/// Parse info.env (KEY=VALUE lines, as update.sh writes them).
pub fn parse_backup_info(path: &Path, text: &str, has_binaries: bool) -> Option<BackupInfo> {
    let mut kind = None;
    let mut from = None;
    let mut to = None;
    let mut created = None;
    let mut migrated = Migrated::Unknown;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "KIND" => kind = Some(value),
            "FROM_VERSION" => from = Some(value.trim_start_matches('v').to_string()),
            "TO_VERSION" => to = Some(value.trim_start_matches('v').to_string()),
            "CREATED_AT" => created = value.parse().ok(),
            "MIGRATED" => {
                migrated = match value.as_str() {
                    "1" => Migrated::Yes,
                    "0" => Migrated::No,
                    _ => Migrated::Unknown,
                }
            }
            _ => {}
        }
    }
    Some(BackupInfo {
        path: path.to_path_buf(),
        kind: kind?,
        from_version: from?,
        to_version: to?,
        created_at: created?,
        migrated,
        has_binaries,
    })
}

pub fn list_backups(backups_dir: &Path) -> Vec<BackupInfo> {
    let Ok(entries) = std::fs::read_dir(backups_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("tond_backup_"))
        .filter_map(|e| {
            let path = e.path();
            let text = std::fs::read_to_string(path.join("info.env")).ok()?;
            let has_binaries = path.join("bin").join("tond-server").is_file();
            parse_backup_info(&path, &text, has_binaries)
        })
        .collect()
}

/// What "Roll back" would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollbackOption {
    /// The version it goes back to.
    pub to_version: String,
    /// When the update being undone started (the database copy's age).
    pub backup_created_at: i64,
    /// The offer ends then (Unix seconds).
    pub available_until: i64,
    /// Rolling back also puts back the database from before the update
    /// (changes made since are lost; a snapshot is taken first).
    pub restores_database: bool,
    #[serde(skip)]
    pub backup: PathBuf,
}

/// The backup to roll back to: made by the update to the version running now,
/// within the rollback window, with its binaries present. The newest wins.
pub fn rollback_candidate(
    backups: &[BackupInfo],
    current_version: &str,
    now: i64,
) -> Option<RollbackOption> {
    backups
        .iter()
        .filter(|b| b.kind == "update" && b.has_binaries)
        .filter(|b| b.to_version == current_version)
        .filter(|b| b.from_version != current_version)
        .filter(|b| now >= b.created_at && now - b.created_at <= ROLLBACK_WINDOW_SECS)
        .max_by_key(|b| b.created_at)
        .map(|b| RollbackOption {
            to_version: b.from_version.clone(),
            backup_created_at: b.created_at,
            available_until: b.created_at + ROLLBACK_WINDOW_SECS,
            restores_database: restores_database(b.migrated),
            backup: b.path.clone(),
        })
}

/// A rollback puts the old database back unless the update is known not to
/// have migrated it: old binaries refuse a schema they don't know.
pub fn restores_database(migrated: Migrated) -> bool {
    migrated != Migrated::No
}

// ── Launching ──

/// Why updates can't run from the server here, if they can't.
pub fn unsupported_reason(paths: &Paths, kind: RunKind) -> Option<String> {
    if !Path::new("/run/systemd/system").is_dir() || systemd_run().is_none() {
        return Some(
            "This server isn't running systemd, so Tond can't update itself from the dashboard. \
             Run update.sh on the server instead."
                .to_string(),
        );
    }
    let needed = match kind {
        RunKind::Update => "tond",
        RunKind::Rollback => "update.sh",
    };
    if !paths.bin_dir.join(needed).is_file() {
        return Some(format!(
            "{} is missing, so Tond can't run the {}. Reinstall Tond or run update.sh on the server.",
            paths.bin_dir.join(needed).display(),
            match kind {
                RunKind::Update => "update",
                RunKind::Rollback => "rollback",
            }
        ));
    }
    None
}

fn systemd_run() -> Option<&'static str> {
    ["/usr/bin/systemd-run", "/bin/systemd-run"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
}

/// Arguments common to both kinds: who asked, the run id, the snapshot.
fn run_args(run_id: &str, actor: &str, snapshot_id: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--run-id".to_string(),
        run_id.to_string(),
        "--actor".to_string(),
        actor.to_string(),
    ];
    if let Some(id) = snapshot_id {
        args.push("--snapshot-id".to_string());
        args.push(id.to_string());
    }
    args
}

/// The command the unit runs for an update to `version`.
pub fn update_command(
    bin_dir: &Path,
    version: &str,
    run_id: &str,
    actor: &str,
    snapshot_id: Option<&str>,
) -> Vec<String> {
    let mut cmd = vec![
        bin_dir.join("tond").to_string_lossy().into_owned(),
        "update".to_string(),
        "--direct".to_string(),
        "--version".to_string(),
        version.to_string(),
    ];
    cmd.extend(run_args(run_id, actor, snapshot_id));
    cmd
}

/// The command the unit runs to roll back to `backup`.
pub fn rollback_command(
    bin_dir: &Path,
    backup: &Path,
    run_id: &str,
    actor: &str,
    snapshot_id: Option<&str>,
) -> Vec<String> {
    let mut cmd = vec![
        bin_dir.join("update.sh").to_string_lossy().into_owned(),
        "--rollback".to_string(),
        "--backup".to_string(),
        backup.to_string_lossy().into_owned(),
    ];
    cmd.extend(run_args(run_id, actor, snapshot_id));
    cmd
}

/// systemd-run arguments that start `cmd` as the transient unit, its output
/// appended to update.log.
pub fn systemd_run_args(paths: &Paths, cmd: &[String]) -> Vec<String> {
    let log = paths.log_file().to_string_lossy().into_owned();
    let mut args = vec![
        format!("--unit={UNIT}"),
        "--description=Tond update".to_string(),
        "--collect".to_string(),
        "--quiet".to_string(),
        "--property=Type=exec".to_string(),
        format!("--property=StandardOutput=append:{log}"),
        format!("--property=StandardError=append:{log}"),
        // Never killed by tond-server's own stop: it's not in its cgroup.
        "--property=TimeoutStopSec=15min".to_string(),
        "--setenv=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "--setenv=NO_COLOR=1".to_string(),
        "--".to_string(),
    ];
    args.extend(cmd.iter().cloned());
    args
}

/// What to start.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub kind: RunKind,
    /// update: the version to install. rollback: the version it goes back to.
    pub to_version: String,
    /// rollback: the backup directory.
    pub backup: Option<PathBuf>,
    /// Who asked (shown in the dashboard and the audit log).
    pub actor: String,
    /// Skip the pre-update snapshot (CLI --skip-backup).
    pub skip_snapshot: bool,
}

pub fn new_run_id(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "{}-{}",
        now.format("%Y%m%dT%H%M%SZ"),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Validate, write the first status and start the work in the background.
/// Returns the status as it stands when the request returns.
pub async fn start(
    state: &crate::state::AppState,
    paths: &Paths,
    req: StartRequest,
) -> Result<RunStatus> {
    if let Some(reason) = unsupported_reason(paths, req.kind) {
        bail!(StartError::Unsupported(reason));
    }
    if STARTING.swap(true, Ordering::SeqCst) {
        bail!(StartError::Busy);
    }
    let result = begin(state, paths, req).await;
    if result.is_err() {
        STARTING.store(false, Ordering::SeqCst);
    }
    result
}

async fn begin(
    state: &crate::state::AppState,
    paths: &Paths,
    req: StartRequest,
) -> Result<RunStatus> {
    let now = chrono::Utc::now();
    if let Some(current) = read_status(paths)
        && effective_state(&current, pid_alive, now.timestamp()) == RunState::Running
    {
        bail!(StartError::Busy);
    }
    let status = RunStatus {
        id: new_run_id(now),
        kind: req.kind,
        state: RunState::Running,
        step: if req.skip_snapshot {
            "download".to_string()
        } else {
            "snapshot".to_string()
        },
        from_version: super::version::current().to_string(),
        to_version: req.to_version.clone(),
        actor: req.actor.clone(),
        snapshot_id: None,
        started_at: now.timestamp(),
        updated_at: now.timestamp(),
        finished_at: None,
        message: if req.skip_snapshot {
            "Starting (no snapshot, as requested)".to_string()
        } else {
            "Taking a full snapshot".to_string()
        },
        pid: Some(std::process::id()),
        database_restored: false,
    };
    reset_log(
        paths,
        &format!(
            "[update] {} {} → {} requested by {}",
            match req.kind {
                RunKind::Update => "Update",
                RunKind::Rollback => "Rollback",
            },
            status.from_version,
            status.to_version,
            req.actor
        ),
    )?;
    write_status(paths, &status)?;

    let state = state.clone();
    let paths = paths.clone();
    let first = status.clone();
    crate::runtime::shutdown::spawn_tracked(async move {
        let outcome = run_in_background(&state, &paths, first.clone(), &req).await;
        if let Err(e) = outcome {
            let msg = format!("{e:#}");
            tracing::warn!("Tond {:?} did not start: {msg}", req.kind);
            append_log(&paths, &format!("[update] {msg}"));
            let mut failed = read_status(&paths).unwrap_or(first);
            failed.state = RunState::Failed;
            failed.message = msg;
            let now = chrono::Utc::now().timestamp();
            failed.updated_at = now;
            failed.finished_at = Some(now);
            let _ = write_status(&paths, &failed);
        }
        STARTING.store(false, Ordering::SeqCst);
    });
    Ok(status)
}

async fn run_in_background(
    state: &crate::state::AppState,
    paths: &Paths,
    mut status: RunStatus,
    req: &StartRequest,
) -> Result<()> {
    if !req.skip_snapshot {
        append_log(paths, "[update] Taking a full snapshot first…");
        let id = crate::backup::create_snapshot(
            state,
            crate::backup::SnapshotTrigger::PreUpdate {
                to_version: req.to_version.clone(),
            },
            None,
        )
        .await
        .context("Couldn't take the snapshot, so nothing was changed")?;
        append_log(paths, &format!("[update] Snapshot {id} taken."));
        status.snapshot_id = Some(id);
        status.message = "Snapshot taken; starting the update".to_string();
        status.updated_at = chrono::Utc::now().timestamp();
        write_status(paths, &status)?;
    }

    let cmd = match req.kind {
        RunKind::Update => update_command(
            &paths.bin_dir,
            &req.to_version,
            &status.id,
            &req.actor,
            status.snapshot_id.as_deref(),
        ),
        RunKind::Rollback => rollback_command(
            &paths.bin_dir,
            req.backup.as_deref().context("no backup to roll back to")?,
            &status.id,
            &req.actor,
            status.snapshot_id.as_deref(),
        ),
    };
    let systemd_run = systemd_run().context("systemd-run is missing")?;
    let output = crate::process::command(systemd_run)
        .args(systemd_run_args(paths, &cmd))
        .output()
        .await
        .context("could not run systemd-run")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("systemd couldn't start the update unit: {stderr}");
    }
    append_log(paths, &format!("[update] Started {UNIT}.service"));
    Ok(())
}

/// Why a run couldn't start (mapped to HTTP statuses by the handler).
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("An update or rollback is already running.")]
    Busy,
    #[error("{0}")]
    Unsupported(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: RunState, pid: Option<u32>, updated_at: i64) -> RunStatus {
        RunStatus {
            id: "r1".into(),
            kind: RunKind::Update,
            state,
            step: "install".into(),
            from_version: "0.3.0".into(),
            to_version: "0.4.0".into(),
            actor: "admin@example.com".into(),
            snapshot_id: None,
            started_at: 0,
            updated_at,
            finished_at: None,
            message: String::new(),
            pid,
            database_restored: false,
        }
    }

    #[test]
    fn a_running_run_with_a_live_process_is_running() {
        let s = status(RunState::Running, Some(42), 0);
        assert_eq!(effective_state(&s, |_| true, 10_000), RunState::Running);
    }

    #[test]
    fn a_running_run_whose_process_died_is_interrupted_once_it_goes_quiet() {
        let s = status(RunState::Running, Some(42), 1_000);
        // Just written: the next process may be taking over (server → unit).
        assert_eq!(effective_state(&s, |_| false, 1_010), RunState::Running);
        assert_eq!(
            effective_state(&s, |_| false, 1_000 + STALE_SECS + 1),
            RunState::Interrupted
        );
        let no_pid = status(RunState::Running, None, 1_000);
        assert_eq!(
            effective_state(&no_pid, |_| true, 1_000 + STALE_SECS + 1),
            RunState::Interrupted
        );
    }

    #[test]
    fn finished_states_are_reported_as_written() {
        for st in [RunState::Succeeded, RunState::Failed, RunState::RolledBack] {
            assert_eq!(
                effective_state(&status(st, None, 0), |_| false, 1 << 40),
                st
            );
            assert!(st.is_finished());
        }
        assert!(!RunState::Running.is_finished());
    }

    #[test]
    fn status_json_round_trips_and_tolerates_missing_optional_fields() {
        let s = status(RunState::RolledBack, Some(7), 5);
        let json = serde_json::to_string(&s).unwrap_or_else(|e| panic!("{e}"));
        assert!(json.contains("\"state\":\"rolled_back\""));
        let back: RunStatus = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, s);
        // What update.sh writes at minimum.
        let minimal = r#"{"id":"x","kind":"rollback","state":"running","step":"backup",
            "from_version":"0.4.0","to_version":"0.3.0","started_at":1,"updated_at":2}"#;
        let parsed: RunStatus = serde_json::from_str(minimal).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.kind, RunKind::Rollback);
        assert_eq!(parsed.pid, None);
        assert!(!parsed.database_restored);
    }

    fn backup(kind: &str, from: &str, to: &str, at: i64, migrated: Migrated) -> BackupInfo {
        BackupInfo {
            path: PathBuf::from(format!("/b/{at}")),
            kind: kind.into(),
            from_version: from.into(),
            to_version: to.into(),
            created_at: at,
            migrated,
            has_binaries: true,
        }
    }

    #[test]
    fn rollback_goes_to_the_version_before_the_update_to_the_running_one() {
        let now = 10 * 24 * 3600;
        let backups = vec![
            backup(
                "update",
                "0.2.0",
                "0.3.0",
                now - 9 * 24 * 3600,
                Migrated::Yes,
            ),
            backup("update", "0.3.0", "0.4.0", now - 3600, Migrated::No),
        ];
        let option = rollback_candidate(&backups, "0.4.0", now).unwrap_or_else(|| panic!());
        assert_eq!(option.to_version, "0.3.0");
        assert_eq!(option.backup, PathBuf::from(format!("/b/{}", now - 3600)));
        assert_eq!(option.available_until, now - 3600 + ROLLBACK_WINDOW_SECS);
        assert!(!option.restores_database);
    }

    #[test]
    fn no_rollback_after_seven_days_or_for_another_version() {
        let now = 30 * 24 * 3600;
        let old = vec![backup(
            "update",
            "0.3.0",
            "0.4.0",
            now - ROLLBACK_WINDOW_SECS - 1,
            Migrated::No,
        )];
        assert!(rollback_candidate(&old, "0.4.0", now).is_none());
        let other = vec![backup("update", "0.3.0", "0.4.0", now - 60, Migrated::No)];
        assert!(rollback_candidate(&other, "0.5.0", now).is_none());
        // Backups taken before a rollback are not themselves rollback targets.
        let rb = vec![backup("rollback", "0.4.0", "0.3.0", now - 60, Migrated::No)];
        assert!(rollback_candidate(&rb, "0.3.0", now).is_none());
        // A backup without binaries can't be rolled back to.
        let mut broken = backup("update", "0.3.0", "0.4.0", now - 60, Migrated::No);
        broken.has_binaries = false;
        assert!(rollback_candidate(&[broken], "0.4.0", now).is_none());
    }

    #[test]
    fn the_database_is_restored_unless_the_update_is_known_not_to_have_migrated() {
        assert!(restores_database(Migrated::Yes));
        assert!(restores_database(Migrated::Unknown));
        assert!(!restores_database(Migrated::No));
    }

    #[test]
    fn parses_info_env_as_update_sh_writes_it() {
        let text = "KIND=update\nFROM_VERSION=v0.3.0\nTO_VERSION=0.4.0\nCREATED_AT=1790000000\nMIGRATED=1\nSCHEMA_BEFORE=60\n";
        let info = parse_backup_info(Path::new("/b/x"), text, true).unwrap_or_else(|| panic!());
        assert_eq!(info.from_version, "0.3.0");
        assert_eq!(info.to_version, "0.4.0");
        assert_eq!(info.created_at, 1_790_000_000);
        assert_eq!(info.migrated, Migrated::Yes);
        let unknown = parse_backup_info(
            Path::new("/b/x"),
            "KIND=update\nFROM_VERSION=a\nTO_VERSION=b\nCREATED_AT=1\nMIGRATED=unknown\n",
            true,
        )
        .unwrap_or_else(|| panic!());
        assert_eq!(unknown.migrated, Migrated::Unknown);
        assert!(parse_backup_info(Path::new("/b/x"), "KIND=update\n", true).is_none());
    }

    #[test]
    fn lists_backup_directories_with_their_info() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let b = dir.path().join("tond_backup_20260920_101500");
        std::fs::create_dir_all(b.join("bin")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(b.join("bin/tond-server"), "x").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            b.join("info.env"),
            "KIND=update\nFROM_VERSION=0.3.0\nTO_VERSION=0.4.0\nCREATED_AT=5\nMIGRATED=0\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        // Not an update backup directory, and one without info.env.
        std::fs::create_dir_all(dir.path().join("snapshots")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(dir.path().join("tond_backup_broken"))
            .unwrap_or_else(|e| panic!("{e}"));
        let list = list_backups(dir.path());
        assert_eq!(list.len(), 1);
        assert!(list[0].has_binaries);
        assert_eq!(list[0].migrated, Migrated::No);
    }

    #[test]
    fn the_unit_runs_the_cli_for_updates_and_update_sh_for_rollbacks() {
        let bin = Path::new("/opt/tond/bin");
        let update = update_command(bin, "0.4.0", "r1", "admin@example.com", Some("snap"));
        assert_eq!(
            update,
            [
                "/opt/tond/bin/tond",
                "update",
                "--direct",
                "--version",
                "0.4.0",
                "--run-id",
                "r1",
                "--actor",
                "admin@example.com",
                "--snapshot-id",
                "snap"
            ]
        );
        let rollback = rollback_command(bin, Path::new("/b/x"), "r2", "root", None);
        assert_eq!(
            rollback,
            [
                "/opt/tond/bin/update.sh",
                "--rollback",
                "--backup",
                "/b/x",
                "--run-id",
                "r2",
                "--actor",
                "root"
            ]
        );
        let paths = Paths {
            update_dir: PathBuf::from("/opt/tond/data/update"),
            backups_dir: PathBuf::from("/opt/tond/data/backups"),
            bin_dir: bin.to_path_buf(),
        };
        let args = systemd_run_args(&paths, &rollback);
        assert_eq!(args[0], "--unit=tond-update");
        assert!(args.contains(&"--collect".to_string()));
        assert!(
            args.contains(
                &"--property=StandardOutput=append:/opt/tond/data/update/update.log".into()
            )
        );
        let sep = args
            .iter()
            .position(|a| a == "--")
            .unwrap_or_else(|| panic!());
        assert_eq!(&args[sep + 1..], rollback.as_slice());
    }

    #[test]
    fn status_and_log_files_round_trip() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let paths = Paths {
            update_dir: dir.path().join("update"),
            backups_dir: dir.path().join("backups"),
            bin_dir: dir.path().join("bin"),
        };
        assert!(read_status(&paths).is_none());
        let s = status(RunState::Running, Some(1), 2);
        write_status(&paths, &s).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read_status(&paths), Some(s));
        reset_log(&paths, "first").unwrap_or_else(|e| panic!("{e}"));
        append_log(&paths, "\u{1b}[0;32m[update]\u{1b}[0m green");
        assert_eq!(log_tail(&paths, 10), ["first", "[update] green"]);
        assert_eq!(log_tail(&paths, 1), ["[update] green"]);
    }

    #[test]
    fn run_ids_sort_by_time() {
        let a = new_run_id(
            chrono::DateTime::from_timestamp(1_790_000_000, 0).unwrap_or_else(|| panic!()),
        );
        let b = new_run_id(
            chrono::DateTime::from_timestamp(1_790_000_060, 0).unwrap_or_else(|| panic!()),
        );
        assert!(a < b);
        assert!(a.starts_with("20260921T"));
    }
}
