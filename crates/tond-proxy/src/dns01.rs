// dns01.rs — ask tond-server to publish ACME DNS-01 TXT records
//
// tond-proxy runs the ACME order but holds no DNS provider token (they are
// secrets tond-server keeps; this process faces the internet). For a DNS-01
// challenge the order sends `dns_txt` over the current control connection
// and waits for tond-server's `dns_result` — which arrives once the record
// is published and has had time to propagate.
//
// DnsBridge is shared by the certificate loop (callers) and the control
// socket (control.rs attaches each new connection and routes the results).
// Without a connected tond-server a DNS-01 order fails right away and backs
// off like any other failure.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::sync::{mpsc, oneshot};
use tond_common::proxy_control::{DnsTxtAction, ProxyMessage};

/// Longest wait for tond-server: provider API calls plus its propagation
/// wait (up to 60 s for Route 53, or TOND_DNS_PROPAGATION_SECS).
const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Default)]
pub struct DnsBridge {
    outgoing: Mutex<Option<mpsc::UnboundedSender<ProxyMessage>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_id: AtomicU64,
}

impl DnsBridge {
    /// A new control connection takes over sending (the latest wins).
    pub fn attach(&self) -> mpsc::UnboundedReceiver<ProxyMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.outgoing.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        rx
    }

    /// tond-server answered request `id`.
    pub fn resolve(&self, id: u64, error: Option<String>) {
        let tx = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if let Some(tx) = tx {
            let _ = tx.send(error);
        }
    }

    /// Publish or clear `name` = `value`; Ok once tond-server confirms.
    pub async fn txt(&self, action: DnsTxtAction, name: &str, value: &str) -> Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        let msg = ProxyMessage::DnsTxt {
            id,
            action,
            name: name.to_string(),
            value: value.to_string(),
        };
        let sent = self
            .outgoing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|out| out.send(msg).is_ok());
        let result = if sent {
            match tokio::time::timeout(TIMEOUT, rx).await {
                Ok(Ok(None)) => Ok(()),
                Ok(Ok(Some(error))) => Err(anyhow!("{error}")),
                Ok(Err(_)) => Err(anyhow!("the connection to tond-server closed")),
                Err(_) => Err(anyhow!(
                    "tond-server did not confirm the DNS record within {}s",
                    TIMEOUT.as_secs()
                )),
            }
        } else {
            Err(anyhow!(
                "tond-server is not connected, so the DNS-01 record can't be published"
            ))
        };
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn requests_go_to_the_attached_connection_and_wait_for_the_answer() {
        let bridge = std::sync::Arc::new(DnsBridge::default());
        let fails = bridge
            .txt(DnsTxtAction::Set, "_acme-challenge.a.test", "v")
            .await;
        assert!(fails.is_err(), "no connection: fail fast");

        let mut rx = bridge.attach();
        let b = bridge.clone();
        let call = tokio::spawn(async move {
            b.txt(DnsTxtAction::Set, "_acme-challenge.a.test", "v")
                .await
        });
        let Some(ProxyMessage::DnsTxt {
            id,
            action,
            name,
            value,
        }) = rx.recv().await
        else {
            panic!("expected a dns_txt message");
        };
        assert_eq!(
            (action, name.as_str(), value.as_str()),
            (DnsTxtAction::Set, "_acme-challenge.a.test", "v")
        );
        bridge.resolve(id, None);
        assert!(call.await.map(|r| r.is_ok()).unwrap_or(false));

        let b = bridge.clone();
        let call = tokio::spawn(async move {
            b.txt(DnsTxtAction::Clear, "_acme-challenge.a.test", "v")
                .await
        });
        let Some(ProxyMessage::DnsTxt { id, .. }) = rx.recv().await else {
            panic!("expected a dns_txt message");
        };
        bridge.resolve(id, Some("zone not found".into()));
        let err = call
            .await
            .ok()
            .and_then(|r| r.err())
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert_eq!(err, "zone not found");
    }
}
