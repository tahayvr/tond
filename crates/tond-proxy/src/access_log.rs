// access_log.rs — opt-in per-app access logs
//
// Apps with `access_log = 1` get one line per request:
//
//   2026-09-25T12:00:00.123Z GET /path 200 12.3ms 1534 203.0.113.9
//   (time, method, path, status, total duration, response bytes, client IP)
//
// The query string is left out unless the app also sets
// `access_log_query = 1` (query strings often carry tokens). Paths are
// escaped (no spaces or control characters, so no forged lines) and capped
// at MAX_PATH bytes.
//
// Where the proxy writes (see release/install.sh secure_data_dir): the data
// directory is root:tond-proxy 3770, so the proxy may create its own entries
// there but not touch root's. It creates `<data>/access/` (0700, owned by
// tond-proxy) and appends to `<app id>.log` (0600, opened with O_NOFOLLOW).
// At ACCESS_LOG_ROTATE_BYTES the file becomes `<app id>.log.1`, replacing the
// previous one: at most ACCESS_LOG_MAX_BYTES (60 MB) per app. tond-server
// reads the files as root, refusing anything that is not a regular,
// single-link file owned by the proxy (handlers/access_log.rs), and deletes
// them when the app is deleted (janitor). The proxy never writes tond.db.
//
// Requests never wait for the disk: the `logging()` hook formats the line and
// `try_send`s it to a bounded queue; the writer task on the background thread
// appends batches. When the queue is full the line is dropped and counted.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::net::IpAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tond_common::traffic::{
    ACCESS_LOG_DIR, ACCESS_LOG_ROTATE_BYTES, access_log_names, is_safe_log_id,
};

/// Longest path written (bytes, before escaping).
const MAX_PATH: usize = 1024;
/// Lines queued for the writer at most.
const QUEUE: usize = 4096;
/// Lines written per batch at most.
const BATCH: usize = 256;

/// Per-route access log settings (None on the route: logging off).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessConfig {
    pub app_id: Arc<str>,
    pub include_query: bool,
}

struct Entry {
    app: Arc<str>,
    line: String,
}

/// The sending side, shared by the proxy workers.
pub struct AccessLog {
    tx: mpsc::Sender<Entry>,
    dropped: AtomicU64,
    /// Bumped on route reloads: the writer closes its files so a disabled
    /// or deleted app's file is not held open.
    reload: watch::Sender<u64>,
}

/// Everything the writer task needs.
pub struct Writer {
    dir: PathBuf,
    rx: mpsc::Receiver<Entry>,
    reload: watch::Receiver<u64>,
}

pub fn channel(data_dir: &Path) -> (AccessLog, Writer) {
    let (tx, rx) = mpsc::channel(QUEUE);
    let (reload, reload_rx) = watch::channel(0);
    (
        AccessLog {
            tx,
            dropped: AtomicU64::new(0),
            reload,
        },
        Writer {
            dir: data_dir.join(ACCESS_LOG_DIR),
            rx,
            reload: reload_rx,
        },
    )
}

/// What one request looked like, for its access line.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub status: u16,
    pub duration: Duration,
    pub bytes_out: u64,
    pub client: Option<IpAddr>,
}

impl AccessLog {
    /// Queue one line (never blocks; drops when the writer is behind).
    pub fn log(&self, cfg: &AccessConfig, req: &Request<'_>) {
        let line = format_line(chrono::Utc::now(), cfg.include_query, req);
        if self
            .tx
            .try_send(Entry {
                app: cfg.app_id.clone(),
                line,
            })
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Lines dropped because the writer could not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Routes were reloaded: close open files.
    pub fn routes_reloaded(&self) {
        self.reload.send_modify(|v| *v += 1);
    }
}

fn push_escaped(out: &mut String, text: &str) {
    for b in text.bytes().take(MAX_PATH) {
        if b.is_ascii_graphic() {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    if text.len() > MAX_PATH {
        out.push('…');
    }
}

/// One access line (with the trailing newline).
pub fn format_line(
    now: chrono::DateTime<chrono::Utc>,
    include_query: bool,
    req: &Request<'_>,
) -> String {
    let mut line = String::with_capacity(96 + req.path.len().min(MAX_PATH));
    let _ = write!(line, "{} ", now.format("%Y-%m-%dT%H:%M:%S%.3fZ"));
    push_escaped(
        &mut line,
        if req.method.is_empty() {
            "-"
        } else {
            req.method
        },
    );
    line.push(' ');
    push_escaped(&mut line, if req.path.is_empty() { "/" } else { req.path });
    if include_query && let Some(q) = req.query.filter(|q| !q.is_empty()) {
        line.push('?');
        push_escaped(&mut line, q);
    }
    let ms = req.duration.as_secs_f64() * 1000.0;
    let status = if req.status == 0 {
        "-".to_string()
    } else {
        req.status.to_string()
    };
    let _ = write!(line, " {status} {ms:.1}ms {}", req.bytes_out);
    match req.client {
        Some(ip) => {
            let _ = write!(line, " {ip}");
        }
        None => line.push_str(" -"),
    }
    line.push('\n');
    line
}

/// One app's open file.
struct Open {
    file: File,
    size: u64,
}

fn open_append(path: &Path) -> std::io::Result<Open> {
    use std::os::unix::fs::PermissionsExt;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    // Keep it owner-only even if something loosened it.
    if meta.permissions().mode() & 0o077 != 0 {
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    Ok(Open {
        file,
        size: meta.len(),
    })
}

impl Writer {
    fn ensure_dir(&self) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::symlink_metadata(&self.dir) {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(std::io::Error::other(format!(
                "{} is not a directory",
                self.dir.display()
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new().mode(0o700).create(&self.dir)
            }
            Err(e) => Err(e),
        }
    }

    fn write_one(&self, files: &mut HashMap<Arc<str>, Open>, entry: &Entry) -> std::io::Result<()> {
        if !is_safe_log_id(&entry.app) {
            return Ok(());
        }
        let (current, rotated) = access_log_names(&entry.app);
        let path = self.dir.join(&current);
        if !files.contains_key(&entry.app) {
            self.ensure_dir()?;
            files.insert(entry.app.clone(), open_append(&path)?);
        }
        let needs_rotation = files.get(&entry.app).is_some_and(|o| {
            o.size > 0 && o.size + entry.line.len() as u64 > ACCESS_LOG_ROTATE_BYTES
        });
        if needs_rotation {
            files.remove(&entry.app);
            std::fs::rename(&path, self.dir.join(&rotated))?;
            files.insert(entry.app.clone(), open_append(&path)?);
        }
        if let Some(open) = files.get_mut(&entry.app) {
            open.file.write_all(entry.line.as_bytes())?;
            open.size += entry.line.len() as u64;
        }
        Ok(())
    }

    /// Append queued lines until every sender is gone.
    pub async fn run(mut self) {
        let mut files: HashMap<Arc<str>, Open> = HashMap::new();
        let mut batch = Vec::with_capacity(BATCH);
        let mut last_error: Option<std::time::Instant> = None;
        let mut reload_open = true;
        loop {
            tokio::select! {
                biased;
                n = self.rx.recv_many(&mut batch, BATCH) => {
                    if n == 0 {
                        return;
                    }
                    for entry in batch.drain(..) {
                        if let Err(e) = self.write_one(&mut files, &entry) {
                            files.remove(&entry.app);
                            if last_error.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                                tracing::warn!("Access log for app {} not written: {e}", entry.app);
                                last_error = Some(std::time::Instant::now());
                            }
                        }
                    }
                }
                changed = self.reload.changed(), if reload_open => {
                    reload_open = changed.is_ok();
                    files.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn req<'a>(path: &'a str, query: Option<&'a str>) -> Request<'a> {
        Request {
            method: "GET",
            path,
            query,
            status: 200,
            duration: Duration::from_micros(12_345),
            bytes_out: 1534,
            client: "203.0.113.9".parse().ok(),
        }
    }

    fn at() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 9, 25, 12, 0, 0)
            .single()
            .unwrap_or_default()
    }

    #[test]
    fn lines_leave_out_the_query_string_by_default() {
        let r = req("/login", Some("token=secret"));
        assert_eq!(
            format_line(at(), false, &r),
            "2026-09-25T12:00:00.000Z GET /login 200 12.3ms 1534 203.0.113.9\n"
        );
        assert!(format_line(at(), true, &r).contains("GET /login?token=secret 200"));
    }

    #[test]
    fn paths_cannot_forge_lines() {
        let r = req("/a b\nFAKE 200", None);
        let line = format_line(at(), false, &r);
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.contains("/a%20b%0AFAKE%20200"));
        let long = "/".to_string() + &"x".repeat(5000);
        assert!(format_line(at(), false, &req(&long, None)).len() < 1200);
        let mut gone = req("/", None);
        gone.status = 0;
        gone.client = None;
        assert!(format_line(at(), false, &gone).ends_with(" - 12.3ms 1534 -\n"));
    }

    #[tokio::test]
    async fn the_writer_appends_rotates_and_refuses_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let (log, writer) = channel(data.path());
        let cfg = AccessConfig {
            app_id: Arc::from("app-1"),
            include_query: false,
        };
        let task = tokio::spawn(writer.run());
        for _ in 0..3 {
            log.log(&cfg, &req("/", None));
        }
        // Unsafe ids are ignored.
        log.log(
            &AccessConfig {
                app_id: Arc::from("../evil"),
                include_query: false,
            },
            &req("/", None),
        );
        drop(log);
        let _ = task.await;
        let dir = data.path().join(ACCESS_LOG_DIR);
        let mode = std::fs::metadata(&dir)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o700);
        let text = std::fs::read_to_string(dir.join("app-1.log")).unwrap_or_default();
        assert_eq!(text.lines().count(), 3);
        assert!(!data.path().join("evil.log").exists());

        // Rotation: a file already at the limit moves to .log.1.
        let (log, writer) = channel(data.path());
        let big = std::fs::File::options()
            .append(true)
            .open(dir.join("app-1.log"))
            .unwrap_or_else(|e| panic!("{e}"));
        big.set_len(ACCESS_LOG_ROTATE_BYTES)
            .unwrap_or_else(|e| panic!("{e}"));
        let task = tokio::spawn(writer.run());
        log.log(&cfg, &req("/after", None));
        drop(log);
        let _ = task.await;
        let rotated = std::fs::metadata(dir.join("app-1.log.1")).map(|m| m.len());
        assert_eq!(rotated.ok(), Some(ACCESS_LOG_ROTATE_BYTES));
        let text = std::fs::read_to_string(dir.join("app-1.log")).unwrap_or_default();
        assert!(text.contains("/after") && text.lines().count() == 1);

        // A symlink planted in place of the log is not followed.
        let target = data.path().join("elsewhere");
        std::fs::write(&target, "").unwrap_or_else(|e| panic!("{e}"));
        std::os::unix::fs::symlink(&target, dir.join("app-2.log"))
            .unwrap_or_else(|e| panic!("{e}"));
        let (log, writer) = channel(data.path());
        let task = tokio::spawn(writer.run());
        log.log(
            &AccessConfig {
                app_id: Arc::from("app-2"),
                include_query: false,
            },
            &req("/", None),
        );
        drop(log);
        let _ = task.await;
        assert_eq!(std::fs::read_to_string(&target).unwrap_or_default(), "");
    }

    #[test]
    fn a_full_queue_drops_instead_of_blocking() {
        let data = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let (log, _writer) = channel(data.path());
        let cfg = AccessConfig {
            app_id: Arc::from("a"),
            include_query: false,
        };
        for _ in 0..QUEUE + 10 {
            log.log(&cfg, &req("/", None));
        }
        assert_eq!(log.dropped(), 10);
    }
}
