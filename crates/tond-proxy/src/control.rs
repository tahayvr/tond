// control.rs — tond-proxy's control socket (tond-server connects here)
//
// Socket: TOND_PROXY_SOCKET, default <TOND_DATA_DIR>/proxy.sock. Created by
// the tond-proxy user with mode 0600 and checked with SO_PEERCRED: only root
// (tond-server; root passes the file mode through CAP_DAC_OVERRIDE) or the
// proxy's own uid may talk to it. Protocol: tond_common::proxy_control.
//
// Per connection: send `hello`, then answer requests (`sync` reloads routes
// when route_state.version moved and replies with the loaded version,
// `certs` pokes the certificate loop, `stats` reports process/loop stats, `traffic` the per-route request
// counters,
// `ack` drops a delivered cert event) while forwarding every certificate
// result from the Outbox until tond-server acknowledges it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tond_common::ProxyStats;
use tond_common::procstat::ProcessSample;
use tond_common::proxy_control::{
    MAX_LINE_BYTES, PROXY_SOCKET_NAME, ProxyMessage, ProxyRequest, ReplyBody, encode_line,
};

use crate::TondProxy;
use crate::issuance::Outbox;
use crate::meters;

pub struct Control {
    pub proxy: TondProxy,
    pub outbox: Arc<Outbox>,
    pub cert_wake: Arc<Notify>,
    pub started: Instant,
}

/// TOND_PROXY_SOCKET, else proxy.sock in the data directory.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    std::env::var("TOND_PROXY_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join(PROXY_SOCKET_NAME))
}

/// Bind the socket owner-only, replacing a stale socket (never another file).
pub fn bind(path: &Path) -> Result<UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)
            .with_context(|| format!("removing stale socket {}", path.display()))?,
        Ok(_) => anyhow::bail!(
            "{} exists and is not a socket; refusing to replace it",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", path.display())),
    }
    // Created under UMask=0077 (tond-proxy.service): already owner-only.
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))?;
    Ok(listener)
}

/// Peers allowed to connect: root (tond-server) and ourselves.
fn peer_allowed(peer_uid: u32, own_uid: u32) -> bool {
    peer_uid == 0 || peer_uid == own_uid
}

/// Accept connections forever.
pub async fn serve(listener: UnixListener, own_uid: u32, ctl: Arc<Control>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                tracing::warn!("Control socket accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }
        };
        match stream.peer_cred() {
            Ok(cred) if peer_allowed(cred.uid(), own_uid) => {}
            Ok(cred) => {
                tracing::warn!(
                    "Control socket: refused a connection from uid {}",
                    cred.uid()
                );
                continue;
            }
            Err(e) => {
                tracing::warn!("Control socket: cannot identify peer ({e}); refused");
                continue;
            }
        }
        let ctl = ctl.clone();
        tokio::spawn(async move {
            if let Err(e) = connection(stream, &ctl).await {
                tracing::debug!("Control connection ended: {e:#}");
            }
        });
    }
}

async fn write(stream: &mut tokio::net::unix::OwnedWriteHalf, msg: &ProxyMessage) -> Result<()> {
    stream.write_all(&encode_line(msg)?).await?;
    Ok(())
}

async fn stats(ctl: &Control) -> ProxyStats {
    let db = ctl.proxy.route_db();
    ProxyStats {
        pid: std::process::id(),
        process: ProcessSample::current(),
        uptime_secs: ctl.started.elapsed().as_secs(),
        route_version: db.loaded_version(),
        routes: ctl.proxy.route_count().await as u64,
        reloads: db.reloads(),
        access_log_dropped: ctl.proxy.access.dropped(),
        loops: meters::snapshot(),
    }
}

async fn answer(ctl: &Control, request: ProxyRequest) -> Option<(u64, ReplyBody)> {
    match request {
        ProxyRequest::Sync { id, min_version } => {
            let body = match ctl.proxy.sync().await {
                Ok((route_version, routes)) => {
                    if route_version < min_version {
                        tracing::warn!(
                            "Route sync: loaded version {route_version}, tond-server expected {min_version}"
                        );
                    }
                    ReplyBody::Synced {
                        route_version,
                        routes,
                    }
                }
                Err(e) => ReplyBody::Error {
                    message: format!("{e:#}"),
                },
            };
            Some((id, body))
        }
        ProxyRequest::Certs { id, domain } => {
            if let Some(domain) = domain {
                tracing::info!("[cert] Retry requested for {domain}");
            }
            ctl.cert_wake.notify_one();
            Some((id, ReplyBody::Ok))
        }
        ProxyRequest::Stats { id } => Some((
            id,
            ReplyBody::Stats {
                stats: stats(ctl).await,
            },
        )),
        ProxyRequest::Traffic { id } => Some((
            id,
            ReplyBody::Traffic {
                traffic: ctl.proxy.traffic.snapshot(),
            },
        )),
        ProxyRequest::Ack { seq } => {
            ctl.outbox.ack(seq);
            None
        }
    }
}

async fn connection(stream: UnixStream, ctl: &Control) -> Result<()> {
    let (read, mut write_half) = stream.into_split();
    write(
        &mut write_half,
        &ProxyMessage::Hello {
            pid: std::process::id(),
            route_version: ctl.proxy.route_db().loaded_version(),
        },
    )
    .await?;

    let mut events = ctl.outbox.subscribe();
    let mut sent_up_to = 0;
    // Everything still unacknowledged from earlier connections.
    for (seq, report) in ctl.outbox.pending_after(0) {
        write(&mut write_half, &ProxyMessage::CertEvent { seq, report }).await?;
        sent_up_to = seq;
    }

    let mut lines = BufReader::new(read).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                if line.len() > MAX_LINE_BYTES {
                    anyhow::bail!("oversized request");
                }
                let request: ProxyRequest = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("Unreadable control request: {e}");
                        continue;
                    }
                };
                if let Some((id, body)) = answer(ctl, request).await {
                    write(&mut write_half, &ProxyMessage::Reply { id, body }).await?;
                }
            }
            changed = events.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                for (seq, report) in ctl.outbox.pending_after(sent_up_to) {
                    write(&mut write_half, &ProxyMessage::CertEvent { seq, report }).await?;
                    sent_up_to = seq;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_root_and_the_proxy_itself_may_connect() {
        assert!(peer_allowed(0, 998));
        assert!(peer_allowed(998, 998));
        assert!(!peer_allowed(1000, 998));
    }

    #[tokio::test]
    async fn bind_creates_an_owner_only_socket_and_refuses_regular_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("proxy.sock");
        drop(bind(&path).unwrap_or_else(|e| panic!("bind: {e}")));
        // Stale socket from a previous run is replaced.
        let _listener = bind(&path).unwrap_or_else(|e| panic!("rebind: {e}"));
        let mode = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("stat: {e}"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let file = dir.path().join("not-a-socket");
        std::fs::write(&file, "x").unwrap_or_else(|e| panic!("write: {e}"));
        assert!(bind(&file).is_err());
    }
}
