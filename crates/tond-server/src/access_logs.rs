// access_logs.rs — reading and removing the opt-in per-app access logs
//
// tond-proxy writes `<data>/access/<app id>.log` (+ `.log.1`, the rotated
// half) — see crates/tond-proxy/src/access_log.rs for the format and the
// permission model. tond-server runs as root and reads files in a directory
// the unprivileged proxy controls, so every open is defensive: the directory
// must be a real directory (not a symlink), and a log file is opened with
// O_NOFOLLOW and used only if it is a regular file with a single link owned
// by the directory's owner. A compromised proxy therefore cannot make
// tond-server read (and serve to the dashboard) a root-only file through a
// symlink or hard link.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tond_common::traffic::{ACCESS_LOG_DIR, access_log_names, is_safe_log_id};

/// `<data>/access`
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(ACCESS_LOG_DIR)
}

/// Owner uid of the access log directory, if it is a real directory.
fn dir_owner(dir: &Path) -> Option<u32> {
    let meta = std::fs::symlink_metadata(dir).ok()?;
    meta.is_dir().then(|| meta.uid())
}

/// Open one log file safely (None when absent or refused).
fn open_checked(dir: &Path, name: &str) -> Option<File> {
    let owner = dir_owner(dir)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(dir.join(name))
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != owner {
        tracing::warn!(
            "Access log {name} refused: not a regular single-link file owned by the proxy"
        );
        return None;
    }
    Some(file)
}

/// The app's log files that exist, oldest first (rotated, then current).
fn files(data_dir: &Path, app_id: &str) -> Vec<File> {
    if !is_safe_log_id(app_id) {
        return Vec::new();
    }
    let dir = dir(data_dir);
    let (current, rotated) = access_log_names(app_id);
    [rotated, current]
        .iter()
        .filter_map(|name| open_checked(&dir, name))
        .collect()
}

/// Bytes on disk for an app's log.
pub fn size(data_dir: &Path, app_id: &str) -> u64 {
    files(data_dir, app_id)
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum()
}

const CHUNK: usize = 64 * 1024;

/// Lines of `file` from the end backwards, newest first, feeding `keep`
/// until it returns false.
fn scan_backwards(file: &mut File, keep: &mut dyn FnMut(&str) -> bool) -> Result<bool> {
    let mut pos = file.seek(SeekFrom::End(0))?;
    let mut carry: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    while pos > 0 {
        let n = CHUNK.min(usize::try_from(pos).unwrap_or(CHUNK));
        pos -= n as u64;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..n])?;
        let mut block = buf[..n].to_vec();
        block.extend_from_slice(&carry);
        // Everything before the first newline may continue in the previous
        // chunk: carry it over unless this is the start of the file.
        let mut lines: Vec<&[u8]> = block.split(|b| *b == b'\n').collect();
        let head = if pos > 0 {
            lines.remove(0).to_vec()
        } else {
            Vec::new()
        };
        for line in lines.iter().rev() {
            if line.is_empty() {
                continue;
            }
            if !keep(&String::from_utf8_lossy(line)) {
                return Ok(false);
            }
        }
        carry = head;
    }
    Ok(true)
}

/// The last `limit` lines (oldest first) containing `filter`
/// (case-insensitive), and whether more matched.
pub fn tail(
    data_dir: &Path,
    app_id: &str,
    limit: usize,
    filter: Option<&str>,
) -> Result<(Vec<String>, bool)> {
    let needle = filter.map(str::to_lowercase).filter(|f| !f.is_empty());
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut keep = |line: &str| {
        if needle
            .as_deref()
            .is_some_and(|n| !line.to_lowercase().contains(n))
        {
            return true;
        }
        if out.len() == limit {
            truncated = true;
            return false;
        }
        out.push(line.to_string());
        true
    };
    // Newest file first.
    for mut file in files(data_dir, app_id).into_iter().rev() {
        if !scan_backwards(&mut file, &mut keep).context("reading the access log")? {
            break;
        }
    }
    out.reverse();
    Ok((out, truncated))
}

/// The whole log (rotated then current file) for a download.
pub fn open_for_download(data_dir: &Path, app_id: &str) -> Vec<File> {
    files(data_dir, app_id)
}

/// Delete an app's log files. Returns how many were removed.
pub fn remove(data_dir: &Path, app_id: &str) -> u64 {
    if !is_safe_log_id(app_id) {
        return 0;
    }
    let dir = dir(data_dir);
    if dir_owner(&dir).is_none() {
        return 0;
    }
    let (current, rotated) = access_log_names(app_id);
    [current, rotated]
        .iter()
        .filter(|name| std::fs::remove_file(dir.join(name)).is_ok())
        .count() as u64
}

/// Remove logs of apps that no longer exist (janitor).
pub async fn prune_orphans(db: &SqlitePool, data_dir: &Path) -> Result<u64> {
    let dir = dir(data_dir);
    if dir_owner(&dir).is_none() {
        return Ok(0);
    }
    let apps: std::collections::HashSet<String> = sqlx::query_scalar("SELECT id FROM apps")
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
    let mut removed = 0;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let id = name
            .strip_suffix(".log.1")
            .or_else(|| name.strip_suffix(".log"));
        if let Some(id) = id
            && is_safe_log_id(id)
            && !apps.contains(id)
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let data = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let dir = dir(data.path());
        std::fs::create_dir(&dir).unwrap_or_else(|e| panic!("{e}"));
        (data, dir)
    }

    fn write_lines(path: &Path, from: usize, to: usize) {
        let text: String = (from..to)
            .map(|i| format!("line {i} GET /p{i} 200\n"))
            .collect();
        std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn tail_reads_across_the_rotated_file_and_filters() {
        let (data, dir) = setup();
        write_lines(&dir.join("app-1.log.1"), 0, 5000);
        write_lines(&dir.join("app-1.log"), 5000, 5010);
        let (lines, truncated) = tail(data.path(), "app-1", 20, None).unwrap_or_default();
        assert_eq!(lines.len(), 20);
        assert!(truncated);
        assert_eq!(lines[0], "line 4990 GET /p4990 200");
        assert_eq!(lines[19], "line 5009 GET /p5009 200");

        let (lines, truncated) = tail(data.path(), "app-1", 100, Some("/P12 ")).unwrap_or_default();
        assert!(!truncated);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("line 12 GET /p12 200")
        );
        assert_eq!(lines.len(), 1);
        assert!(size(data.path(), "app-1") > 0);

        let (none, _) = tail(data.path(), "missing", 10, None).unwrap_or_default();
        assert!(none.is_empty());
        let (none, _) = tail(data.path(), "../etc", 10, None).unwrap_or_default();
        assert!(none.is_empty());
    }

    #[test]
    fn symlinks_and_hard_links_are_refused() {
        let (data, dir) = setup();
        let secret = data.path().join("secret");
        std::fs::write(&secret, "root only\n").unwrap_or_else(|e| panic!("{e}"));
        std::os::unix::fs::symlink(&secret, dir.join("a.log")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::hard_link(&secret, dir.join("b.log")).unwrap_or_else(|e| panic!("{e}"));
        for app in ["a", "b"] {
            let (lines, _) = tail(data.path(), app, 10, None).unwrap_or_default();
            assert!(lines.is_empty(), "{app}");
        }

        // A symlinked directory is refused too.
        let other = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let data2 = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        write_lines(&other.path().join("c.log"), 0, 3);
        std::os::unix::fs::symlink(other.path(), dir_path(data2.path()))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            tail(data2.path(), "c", 10, None)
                .unwrap_or_default()
                .0
                .is_empty()
        );
    }

    fn dir_path(data: &Path) -> PathBuf {
        dir(data)
    }

    #[tokio::test]
    async fn remove_and_prune_orphans() {
        let (data, dir) = setup();
        write_lines(&dir.join("keep.log"), 0, 1);
        write_lines(&dir.join("gone.log"), 0, 1);
        write_lines(&dir.join("gone.log.1"), 0, 1);
        std::fs::write(dir.join("notes.txt"), "x").unwrap_or_else(|e| panic!("{e}"));
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, created_at, updated_at)
             VALUES ('keep', 'keep', 'git', 3000, 'running', datetime('now'), datetime('now'))",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(prune_orphans(&db, data.path()).await.ok(), Some(2));
        assert!(dir.join("keep.log").exists() && dir.join("notes.txt").exists());
        assert_eq!(remove(data.path(), "keep"), 1);
        assert_eq!(remove(data.path(), "keep"), 0);
    }
}
