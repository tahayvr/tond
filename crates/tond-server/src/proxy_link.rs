// proxy_link.rs — tond-server's end of the tond-proxy control socket
//
// tond-server keeps one persistent connection to tond-proxy's control socket
// (TOND_PROXY_SOCKET, default <data dir>/proxy.sock; protocol in
// tond_common::proxy_control). Over it tond-server:
//
//   - nudges the proxy whenever route_state.version changes (the SQLite
//     update hook reports writes to route_state, see runtime/changes.rs), so
//     routing follows the database within milliseconds and the proxy never
//     has to poll;
//   - asks for an acknowledged reload during a deploy cutover
//     (`sync_routes`): the old container is stopped only once the proxy
//     confirms it routes the new port. This replaced a fixed 3 s sleep;
//   - receives certificate results (cert events) and stores them — the proxy
//     opens tond.db read-only — then acks them;
//   - asks for the proxy's own stats for GET /api/system/self.
//
// When the proxy is not running there is nothing to wait for: it loads the
// current routes from the database when it starts. When it runs but does not
// answer, a cutover waits out the proxy's fallback poll instead
// (FALLBACK_POLL_SECS) so traffic never hits a stopped container.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{Notify, oneshot};
use tond_common::ProxyStats;
use tond_common::proxy_control::{
    CertOutcome, CertReport, FALLBACK_POLL_SECS, MAX_LINE_BYTES, PROXY_SOCKET_NAME, ProxyMessage,
    ProxyRequest, ReplyBody, encode_line,
};

use crate::runtime::changes::{Table, Watch};
use crate::runtime::{loops, shutdown};
use crate::state::AppState;

/// How long a cutover waits for the proxy's acknowledgement.
const SYNC_TIMEOUT: Duration = Duration::from_secs(5);
/// Reconnect backoff bounds while the proxy is unreachable.
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
/// Longest error text stored for the dashboard.
const MAX_ERROR_LEN: usize = 500;

struct Link {
    writer: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<ReplyBody>>>,
    next_id: AtomicU64,
    connected: AtomicBool,
    /// Wakes the connection loop early (a cutover found the link down).
    reconnect_now: Notify,
    /// Signalled on every successful connect.
    connected_notify: Notify,
}

static LINK: LazyLock<Link> = LazyLock::new(|| Link {
    writer: tokio::sync::Mutex::new(None),
    pending: Mutex::new(HashMap::new()),
    next_id: AtomicU64::new(1),
    connected: AtomicBool::new(false),
    reconnect_now: Notify::new(),
    connected_notify: Notify::new(),
});

/// Path of the proxy control socket: TOND_PROXY_SOCKET, else proxy.sock next
/// to the database.
pub fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("TOND_PROXY_SOCKET") {
        return PathBuf::from(path);
    }
    let db_path =
        std::env::var("TOND_DB_PATH").unwrap_or_else(|_| "/opt/tond/data/tond.db".to_string());
    Path::new(&db_path).with_file_name(PROXY_SOCKET_NAME)
}

pub fn is_connected() -> bool {
    LINK.connected.load(Ordering::Relaxed)
}

/// How a cutover was confirmed.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncOutcome {
    /// The proxy reloaded (or already had) the routes.
    Acked {
        route_version: i64,
        elapsed: Duration,
    },
    /// No proxy is running; it reads the current routes when it starts.
    NotRunning,
    /// The proxy exists but did not confirm.
    NoAck { reason: String },
}

impl SyncOutcome {
    /// Line for the deploy log.
    pub fn describe(&self) -> String {
        match self {
            SyncOutcome::Acked {
                route_version,
                elapsed,
            } => format!(
                "Proxy confirmed the new route in {} ms (route version {route_version}); stopping the old container",
                elapsed.as_millis()
            ),
            SyncOutcome::NotRunning => {
                "tond-proxy is not running; it will route to the new container when it starts"
                    .to_string()
            }
            SyncOutcome::NoAck { reason } => format!(
                "tond-proxy did not confirm the new route ({reason}); keeping the old container for {}s, until the proxy's fallback check has certainly picked it up",
                FALLBACK_POLL_SECS + 2
            ),
        }
    }

    /// How long to keep the old container before stopping it.
    pub fn fallback_wait(&self) -> Option<Duration> {
        match self {
            SyncOutcome::NoAck { .. } => Some(Duration::from_secs(FALLBACK_POLL_SECS + 2)),
            _ => None,
        }
    }
}

async fn current_route_version(state: &AppState) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT version FROM route_state WHERE id = 1")
            .fetch_one(&state.db)
            .await?,
    )
}

/// Send `request` (built with a fresh id) and wait up to `timeout` for the reply.
async fn request(build: impl FnOnce(u64) -> ProxyRequest, timeout: Duration) -> Result<ReplyBody> {
    let id = LINK.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    LINK.pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, tx);
    let sent = send(&build(id)).await;
    if let Err(e) = sent {
        LINK.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        return Err(e);
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(body)) => Ok(body),
        Ok(Err(_)) => anyhow::bail!("the connection to tond-proxy closed"),
        Err(_) => {
            LINK.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            anyhow::bail!("no answer within {}s", timeout.as_secs())
        }
    }
}

async fn send(msg: &ProxyRequest) -> Result<()> {
    let line = encode_line(msg)?;
    let mut writer = LINK.writer.lock().await;
    let w = writer.as_mut().context("not connected to tond-proxy")?;
    w.write_all(&line).await.context("writing to tond-proxy")?;
    Ok(())
}

/// Classify a failed connect: is there simply no proxy?
fn proxy_absent(path: &Path) -> bool {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => false,
        Err(e) => matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused),
    }
}

/// Ask tond-proxy to load the current routes and wait for its acknowledgement.
pub async fn sync_routes(state: &AppState) -> SyncOutcome {
    let started = Instant::now();
    let version = match current_route_version(state).await {
        Ok(v) => v,
        Err(e) => {
            return SyncOutcome::NoAck {
                reason: format!("could not read the route version: {e}"),
            };
        }
    };

    if !is_connected() {
        if proxy_absent(&socket_path()) {
            return SyncOutcome::NotRunning;
        }
        // The proxy is up but the link is down (e.g. it just restarted):
        // reconnect now instead of waiting out the backoff.
        LINK.reconnect_now.notify_one();
        let _ =
            tokio::time::timeout(Duration::from_secs(2), LINK.connected_notify.notified()).await;
    }

    match request(
        |id| ProxyRequest::Sync {
            id,
            min_version: version,
        },
        SYNC_TIMEOUT,
    )
    .await
    {
        Ok(ReplyBody::Synced { route_version, .. }) if route_version >= version => {
            SyncOutcome::Acked {
                route_version,
                elapsed: started.elapsed(),
            }
        }
        Ok(ReplyBody::Synced { route_version, .. }) => SyncOutcome::NoAck {
            reason: format!("it loaded route version {route_version}, expected {version}"),
        },
        Ok(ReplyBody::Error { message }) => SyncOutcome::NoAck { reason: message },
        Ok(other) => SyncOutcome::NoAck {
            reason: format!("unexpected answer {other:?}"),
        },
        Err(e) if !is_connected() && proxy_absent(&socket_path()) => {
            tracing::debug!("proxy went away during sync: {e}");
            SyncOutcome::NotRunning
        }
        Err(e) => SyncOutcome::NoAck {
            reason: e.to_string(),
        },
    }
}

/// Ask the proxy to run a certificate pass now (after "Retry now" or a
/// domain change). With `domain`, the proxy forgets its in-memory backoff for
/// it first. Best-effort: a proxy that is down runs a pass when it starts.
pub async fn request_certs(domain: Option<String>) {
    if !is_connected() {
        return;
    }
    if let Err(e) = request(|id| ProxyRequest::Certs { id, domain }, SYNC_TIMEOUT).await {
        tracing::debug!("proxy certs request failed: {e}");
    }
}

/// The proxy's own stats, if it is connected and answers quickly.
pub async fn proxy_stats() -> Option<ProxyStats> {
    if !is_connected() {
        return None;
    }
    match request(|id| ProxyRequest::Stats { id }, Duration::from_secs(2)).await {
        Ok(ReplyBody::Stats { stats }) => Some(stats),
        _ => None,
    }
}

/// Store a certificate result reported by the proxy.
pub async fn apply_cert_report(db: &sqlx::SqlitePool, report: &CertReport) -> Result<()> {
    match &report.outcome {
        CertOutcome::Issued {
            issued_at,
            expires_at,
            issuer,
        } => {
            sqlx::query(
                "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at,
                                           status, created_at, updated_at,
                                           last_error, last_attempt_at, next_retry_at, failure_count,
                                           issuer, last_renewed_at)
                 VALUES (?1, ?2, 'acme', ?3, ?4, ?5, ?6, 'active', ?7, ?7, NULL, ?7, NULL, 0, ?8, ?7)
                 ON CONFLICT(domain) DO UPDATE SET
                    cert_path = excluded.cert_path,
                    key_path = excluded.key_path,
                    issued_at = excluded.issued_at,
                    expires_at = excluded.expires_at,
                    status = 'active',
                    updated_at = excluded.updated_at,
                    last_error = NULL,
                    last_attempt_at = excluded.last_attempt_at,
                    next_retry_at = NULL,
                    failure_count = 0,
                    issuer = excluded.issuer,
                    last_renewed_at = excluded.last_renewed_at",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&report.domain)
            .bind(&report.cert_path)
            .bind(&report.key_path)
            .bind(issued_at)
            .bind(expires_at)
            .bind(&report.attempted_at)
            .bind(issuer)
            .execute(db)
            .await
            .context("storing issued certificate")?;
        }
        CertOutcome::Failed {
            status,
            error,
            failure_count,
            next_retry_at,
        } => {
            sqlx::query(
                "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at,
                                           status, created_at, updated_at,
                                           last_error, last_attempt_at, next_retry_at, failure_count)
                 VALUES (?1, ?2, 'acme', ?3, ?4, ?5, ?5, ?6, ?5, ?5, ?7, ?5, ?8, ?9)
                 ON CONFLICT(domain) DO UPDATE SET
                    status = excluded.status,
                    updated_at = excluded.updated_at,
                    last_error = excluded.last_error,
                    last_attempt_at = excluded.last_attempt_at,
                    next_retry_at = excluded.next_retry_at,
                    failure_count = excluded.failure_count",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&report.domain)
            .bind(&report.cert_path)
            .bind(&report.key_path)
            .bind(&report.attempted_at)
            .bind(status)
            .bind(truncate(error, MAX_ERROR_LEN))
            .bind(next_retry_at)
            .bind(i64::from(*failure_count))
            .execute(db)
            .await
            .context("storing certificate failure")?;
        }
    }
    Ok(())
}

fn truncate(message: &str, max: usize) -> String {
    if message.len() <= max {
        return message.to_string();
    }
    let mut end = max;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

async fn handle_message(state: &AppState, msg: ProxyMessage) {
    match msg {
        ProxyMessage::Hello { pid, route_version } => {
            tracing::info!("Connected to tond-proxy (pid {pid}, route version {route_version})");
        }
        ProxyMessage::Reply { id, body } => {
            let tx = LINK
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            if let Some(tx) = tx {
                let _ = tx.send(body);
            }
        }
        ProxyMessage::CertEvent { seq, report } => {
            match apply_cert_report(&state.db, &report).await {
                Ok(()) => {
                    let _ = send(&ProxyRequest::Ack { seq }).await;
                }
                // Not acked: the proxy resends it on the next connection.
                Err(e) => tracing::warn!(
                    "Could not store certificate result for {}: {e:#}",
                    report.domain
                ),
            }
        }
    }
}

/// Serve one connection until it closes.
async fn serve_connection(state: &AppState, stream: UnixStream) {
    let (read, write) = stream.into_split();
    *LINK.writer.lock().await = Some(write);
    LINK.connected.store(true, Ordering::Relaxed);
    LINK.connected_notify.notify_waiters();

    // Catch the proxy up with whatever changed while it was disconnected.
    let resync = state.clone();
    tokio::spawn(async move {
        let _ = sync_routes(&resync).await;
    });

    let mut lines = BufReader::new(read).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.len() <= MAX_LINE_BYTES => {
                match serde_json::from_str::<ProxyMessage>(&line) {
                    Ok(msg) => handle_message(state, msg).await,
                    Err(e) => tracing::warn!("Unreadable message from tond-proxy: {e}"),
                }
            }
            Ok(Some(_)) => tracing::warn!("Oversized message from tond-proxy ignored"),
            Ok(None) | Err(_) => break,
        }
    }

    LINK.connected.store(false, Ordering::Relaxed);
    *LINK.writer.lock().await = None;
    // Fail outstanding requests now instead of letting them time out.
    LINK.pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    tracing::info!("Connection to tond-proxy closed");
}

/// Keep the link up for the life of the process, and forward route changes.
pub async fn run(state: AppState) {
    tokio::spawn(forward_route_changes(state.clone()));

    let lp = loops::register("proxy-link");
    let path = socket_path();
    let mut backoff = RECONNECT_MIN;
    let mut never = Watch::new(&[]);
    loop {
        if shutdown::is_shutting_down() {
            return;
        }
        match UnixStream::connect(&path).await {
            Ok(stream) => {
                backoff = RECONNECT_MIN;
                drop(lp.run());
                serve_connection(&state, stream).await;
            }
            Err(e) => {
                if backoff == RECONNECT_MIN {
                    tracing::debug!(
                        "tond-proxy control socket {} unavailable: {e}",
                        path.display()
                    );
                }
                tokio::select! {
                    _ = lp.wait(Some(backoff), &mut never) => {}
                    _ = LINK.reconnect_now.notified() => {}
                    _ = shutdown::cancelled() => return,
                }
                backoff = (backoff * 2).min(RECONNECT_MAX);
            }
        }
    }
}

/// Nudge the proxy on every route_state change (event-driven, no polling).
async fn forward_route_changes(state: AppState) {
    let lp = loops::register("route-nudge");
    let mut watch = Watch::new(&[Table::RouteState]);
    loop {
        lp.wait(None, &mut watch).await;
        if !is_connected() {
            continue;
        }
        let run = lp.run();
        match sync_routes(&state).await {
            SyncOutcome::Acked { .. } | SyncOutcome::NotRunning => drop(run),
            SyncOutcome::NoAck { reason } => run.fail(&reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> sqlx::SqlitePool {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    fn report(outcome: CertOutcome) -> CertReport {
        CertReport {
            domain: "app.example.com".into(),
            cert_path: "/data/certs/app.example.com/cert.pem".into(),
            key_path: "/data/certs/app.example.com/key.pem".into(),
            attempted_at: "2026-05-01T10:00:00+00:00".into(),
            outcome,
        }
    }

    #[tokio::test]
    async fn cert_reports_record_failures_then_clear_them_on_success() {
        let db = db().await;
        apply_cert_report(
            &db,
            &report(CertOutcome::Failed {
                status: "failed".into(),
                error: "x".repeat(900),
                failure_count: 2,
                next_retry_at: "2026-05-01T10:02:00+00:00".into(),
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("apply failure: {e}"));
        let row: (String, Option<String>, Option<String>, i64) = sqlx::query_as(
            "SELECT status, last_error, next_retry_at, failure_count FROM certificates WHERE domain = 'app.example.com'",
        )
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("row: {e}"));
        assert_eq!(row.0, "failed");
        assert!(
            row.1
                .as_deref()
                .is_some_and(|e| e.chars().count() <= MAX_ERROR_LEN + 1)
        );
        assert_eq!(row.2.as_deref(), Some("2026-05-01T10:02:00+00:00"));
        assert_eq!(row.3, 2);

        apply_cert_report(
            &db,
            &report(CertOutcome::Issued {
                issued_at: "2026-05-01T10:00:00Z".into(),
                expires_at: "2026-07-30T10:00:00Z".into(),
                issuer: Some("Let's Encrypt R11".into()),
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("apply success: {e}"));
        let row: (String, Option<String>, Option<String>, i64, String) = sqlx::query_as(
            "SELECT status, last_error, next_retry_at, failure_count, expires_at FROM certificates WHERE domain = 'app.example.com'",
        )
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("row: {e}"));
        assert_eq!(
            row,
            (
                "active".into(),
                None,
                None,
                0,
                "2026-07-30T10:00:00Z".into()
            )
        );
        let (issuer, renewed): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT issuer, last_renewed_at FROM certificates WHERE domain = 'app.example.com'",
        )
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("row: {e}"));
        assert_eq!(issuer.as_deref(), Some("Let's Encrypt R11"));
        assert_eq!(renewed.as_deref(), Some("2026-05-01T10:00:00+00:00"));

        // A later failed renewal keeps the issuer and the last renewal time.
        apply_cert_report(
            &db,
            &report(CertOutcome::Failed {
                status: "active".into(),
                error: "timeout".into(),
                failure_count: 1,
                next_retry_at: "2026-06-01T10:01:00+00:00".into(),
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("apply failure: {e}"));
        let (issuer, renewed): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT issuer, last_renewed_at FROM certificates WHERE domain = 'app.example.com'",
        )
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("row: {e}"));
        assert!(issuer.is_some() && renewed.is_some());
    }

    #[test]
    fn outcomes_describe_themselves_and_only_a_missing_ack_waits() {
        let acked = SyncOutcome::Acked {
            route_version: 4,
            elapsed: Duration::from_millis(12),
        };
        assert!(acked.describe().contains("12 ms"));
        assert_eq!(acked.fallback_wait(), None);
        assert_eq!(SyncOutcome::NotRunning.fallback_wait(), None);
        let no_ack = SyncOutcome::NoAck {
            reason: "timeout".into(),
        };
        assert_eq!(
            no_ack.fallback_wait(),
            Some(Duration::from_secs(FALLBACK_POLL_SECS + 2))
        );
        assert!(no_ack.describe().contains("timeout"));
    }

    #[test]
    fn a_missing_socket_means_no_proxy() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        assert!(proxy_absent(&dir.path().join("proxy.sock")));
        // A bound socket nobody accepts on still counts as present.
        let path = dir.path().join("live.sock");
        let _listener =
            std::os::unix::net::UnixListener::bind(&path).unwrap_or_else(|e| panic!("bind: {e}"));
        assert!(!proxy_absent(&path));
    }

    /// End to end over a real socket, against a fake proxy: the link
    /// connects, stores and acks a cert event, and a cutover sync is acked.
    #[tokio::test]
    async fn link_round_trip_with_a_fake_proxy() {
        use bollard::{API_DEFAULT_VERSION, Docker};
        use std::sync::Arc;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::sync::RwLock;

        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("proxy.sock");
        // SAFETY: only this test reads TOND_PROXY_SOCKET (nextest runs each
        // test in its own process).
        unsafe { std::env::set_var("TOND_PROXY_SOCKET", &path) };
        let listener =
            tokio::net::UnixListener::bind(&path).unwrap_or_else(|e| panic!("bind: {e}"));

        let state = AppState {
            db: db().await,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 1, API_DEFAULT_VERSION)
                .unwrap_or_else(|e| panic!("docker: {e}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(tond_common::ServerSettings::default())),
        };
        tokio::spawn(run(state.clone()));

        let (acked_tx, acked_rx) = tokio::sync::oneshot::channel::<u64>();
        tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .unwrap_or_else(|e| panic!("accept: {e}"));
            let (read, mut write) = stream.into_split();
            let send = |msg: ProxyMessage| encode_line(&msg).unwrap_or_default();
            let _ = write
                .write_all(&send(ProxyMessage::Hello {
                    pid: 7,
                    route_version: 0,
                }))
                .await;
            let _ = write
                .write_all(&send(ProxyMessage::CertEvent {
                    seq: 41,
                    report: report(CertOutcome::Failed {
                        status: "failed".into(),
                        error: "dns".into(),
                        failure_count: 1,
                        next_retry_at: "2999-01-01T00:00:00+00:00".into(),
                    }),
                }))
                .await;
            let mut acked_tx = Some(acked_tx);
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ProxyRequest>(&line) {
                    Ok(ProxyRequest::Sync { id, min_version }) => {
                        let _ = write
                            .write_all(&send(ProxyMessage::Reply {
                                id,
                                body: ReplyBody::Synced {
                                    route_version: min_version,
                                    routes: 3,
                                },
                            }))
                            .await;
                    }
                    Ok(ProxyRequest::Ack { seq }) => {
                        if let Some(tx) = acked_tx.take() {
                            let _ = tx.send(seq);
                        }
                    }
                    _ => {}
                }
            }
        });

        let acked = tokio::time::timeout(Duration::from_secs(5), acked_rx)
            .await
            .unwrap_or_else(|_| panic!("the cert event was never acked"))
            .unwrap_or_default();
        assert_eq!(acked, 41);
        let failures: i64 = sqlx::query_scalar(
            "SELECT failure_count FROM certificates WHERE domain = 'app.example.com'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("stored report: {e}"));
        assert_eq!(failures, 1, "the report was stored before the ack");

        assert!(is_connected());
        match sync_routes(&state).await {
            SyncOutcome::Acked { route_version, .. } => assert_eq!(route_version, 0),
            other => panic!("expected an ack, got {other:?}"),
        }
    }
}
