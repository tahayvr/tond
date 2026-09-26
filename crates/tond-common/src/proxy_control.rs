// proxy_control.rs — tond-server <-> tond-proxy control protocol
//
// tond-proxy listens on a Unix socket (`<data dir>/proxy.sock`, owned by the
// tond-proxy user, mode 0600, and it only accepts peers whose uid is 0 or its
// own). tond-server, running as root, keeps one persistent connection to it.
//
// Framing: one JSON object per line, in both directions.
//
//   server -> proxy   ProxyRequest
//     sync  { id, min_version }  reload the routing table if route_state
//                                changed and reply once the loaded version is
//                                at least `min_version` (or the reload failed)
//     certs { id, domain? }      run a certificate pass now; with `domain`,
//                                forget that domain's in-memory backoff first
//     stats { id }               report process / loop stats
//     traffic { id }             report cumulative per-route request counters
//                                (tond_common::traffic)
//     ack   { seq }              a cert event was stored; the proxy drops it
//     install_cert { id, domain, cert_pem, key_pem }
//                                validate an uploaded certificate and serve it
//                                (replies cert_installed or error)
//     remove_cert { id, domain } stop serving a certificate and delete its files
//     dns_result { id, error? }  answer to a dns_txt message
//
//   proxy -> server   ProxyMessage
//     hello      { pid, route_version }         first line on every connection
//     reply      { id, body }                   answer to a request with that id
//     cert_event { seq, report }                result of an issuance attempt; resent
//                                               on every new connection until acked
//     dns_txt    { id, action, name, value }    publish (set) or remove (clear) an ACME
//                                               DNS-01 TXT record; tond-server answers
//                                               with dns_result
//
// DNS-01: tond-proxy runs every ACME order, but it never sees a DNS provider
// token. When an order needs a DNS-01 challenge it asks tond-server (root,
// holds the encrypted tokens) to publish the TXT record, waits for the
// answer, and asks it to clear the record afterwards. tond-server only
// publishes `_acme-challenge.<d>` records for domains Tond serves.

// tond-proxy opens tond.db read-only: everything it used to write (the
// certificate retry state) travels as cert events, and tond-server writes it.

use serde::{Deserialize, Serialize};

use crate::models::runtime::ProxyStats;
use crate::traffic::TrafficSnapshot;

/// File name of the control socket inside the data directory.
pub const PROXY_SOCKET_NAME: &str = "proxy.sock";

/// Default path of the control socket.
pub const DEFAULT_PROXY_SOCKET_PATH: &str = "/opt/tond/data/proxy.sock";

/// Longest line either side accepts.
pub const MAX_LINE_BYTES: usize = 256 * 1024;

/// How often tond-proxy re-checks `route_state.version` on its own, as a
/// safety net for a missed nudge. A cutover that the proxy did not
/// acknowledge waits this long (plus a margin) before stopping the old
/// container.
pub const FALLBACK_POLL_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProxyRequest {
    Sync {
        id: u64,
        min_version: i64,
    },
    Certs {
        id: u64,
        domain: Option<String>,
    },
    Stats {
        id: u64,
    },
    Traffic {
        id: u64,
    },
    Ack {
        seq: u64,
    },
    InstallCert {
        id: u64,
        domain: String,
        cert_pem: String,
        key_pem: String,
    },
    RemoveCert {
        id: u64,
        domain: String,
    },
    DnsResult {
        id: u64,
        error: Option<String>,
    },
}

/// What a dns_txt message asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsTxtAction {
    Set,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyMessage {
    Hello {
        pid: u32,
        route_version: i64,
    },
    Reply {
        id: u64,
        body: ReplyBody,
    },
    CertEvent {
        seq: u64,
        report: CertReport,
    },
    DnsTxt {
        id: u64,
        action: DnsTxtAction,
        /// `_acme-challenge.<domain>`, without a trailing dot.
        name: String,
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplyBody {
    Synced { route_version: i64, routes: u64 },
    Stats { stats: ProxyStats },
    Traffic { traffic: TrafficSnapshot },
    CertInstalled { cert: InstalledCert },
    Ok,
    Error { message: String },
}

/// An uploaded certificate the proxy validated and now serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledCert {
    pub domain: String,
    pub cert_path: String,
    pub key_path: String,
    /// RFC 3339 `notBefore` / `notAfter` of the leaf.
    pub issued_at: String,
    pub expires_at: String,
    pub issuer: Option<String>,
    /// DNS names in the leaf's subjectAltName.
    pub names: Vec<String>,
}

/// Outcome of one certificate issuance attempt, as tond-server stores it in
/// the `certificates` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertReport {
    pub domain: String,
    pub cert_path: String,
    pub key_path: String,
    /// RFC 3339 time of the attempt.
    pub attempted_at: String,
    /// "http-01" | "dns-01" — how the CA validated the domain.
    #[serde(default)]
    pub challenge: Option<String>,
    pub outcome: CertOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CertOutcome {
    /// A certificate was issued; retry state is cleared.
    Issued {
        issued_at: String,
        expires_at: String,
        /// The issuing CA as named in the certificate (O and CN).
        #[serde(default)]
        issuer: Option<String>,
    },
    /// The attempt failed or was skipped (DNS pre-check). `status` is what
    /// the dashboard shows: 'active' while an older certificate still serves,
    /// 'pending' for a DNS skip, 'failed' otherwise.
    Failed {
        status: String,
        error: String,
        failure_count: u32,
        next_retry_at: String,
    },
    /// The domain's own certificate is no longer used: a wildcard (`by`)
    /// serves it now. tond-server drops the row (no expiry alerts for it).
    Superseded { by: String },
}

/// Serialize one message as a line (with the trailing newline).
pub fn encode_line<T: Serialize>(msg: &T) -> serde_json::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_with_an_op_tag() {
        let req = ProxyRequest::Sync {
            id: 7,
            min_version: 42,
        };
        let line = encode_line(&req).unwrap_or_default();
        assert_eq!(
            String::from_utf8_lossy(&line),
            "{\"op\":\"sync\",\"id\":7,\"min_version\":42}\n"
        );
        let back: ProxyRequest =
            serde_json::from_slice(&line).unwrap_or(ProxyRequest::Ack { seq: 0 });
        assert_eq!(back, req);
    }

    #[test]
    fn certificate_and_dns_requests_round_trip() {
        for req in [
            ProxyRequest::InstallCert {
                id: 1,
                domain: "*.example.com".into(),
                cert_pem: "-----BEGIN CERTIFICATE-----".into(),
                key_pem: "-----BEGIN PRIVATE KEY-----".into(),
            },
            ProxyRequest::RemoveCert {
                id: 2,
                domain: "a.example.com".into(),
            },
            ProxyRequest::DnsResult {
                id: 3,
                error: Some("zone not found".into()),
            },
        ] {
            let line = encode_line(&req).unwrap_or_default();
            let back: Result<ProxyRequest, _> = serde_json::from_slice(&line);
            assert_eq!(back.ok(), Some(req));
        }
        // Reports from older proxies (no `challenge`) still parse.
        let old = r#"{"domain":"a","cert_path":"c","key_path":"k","attempted_at":"t","outcome":{"result":"issued","issued_at":"i","expires_at":"e"}}"#;
        let report: Result<CertReport, _> = serde_json::from_str(old);
        assert_eq!(report.ok().and_then(|r| r.challenge), None);
    }

    #[test]
    fn messages_round_trip() {
        let msgs = vec![
            ProxyMessage::Hello {
                pid: 1,
                route_version: 3,
            },
            ProxyMessage::Reply {
                id: 2,
                body: ReplyBody::Synced {
                    route_version: 3,
                    routes: 4,
                },
            },
            ProxyMessage::Reply {
                id: 3,
                body: ReplyBody::Error {
                    message: "db locked".into(),
                },
            },
            ProxyMessage::CertEvent {
                seq: 9,
                report: CertReport {
                    domain: "a.example.com".into(),
                    cert_path: "/c".into(),
                    key_path: "/k".into(),
                    attempted_at: "2026-01-01T00:00:00Z".into(),
                    challenge: Some("dns-01".into()),
                    outcome: CertOutcome::Failed {
                        status: "failed".into(),
                        error: "boom".into(),
                        failure_count: 2,
                        next_retry_at: "2026-01-01T00:02:00Z".into(),
                    },
                },
            },
            ProxyMessage::DnsTxt {
                id: 4,
                action: DnsTxtAction::Set,
                name: "_acme-challenge.apps.test".into(),
                value: "abc".into(),
            },
        ];
        for msg in msgs {
            let line = encode_line(&msg).unwrap_or_default();
            let back: Result<ProxyMessage, _> = serde_json::from_slice(&line);
            assert_eq!(back.ok(), Some(msg));
        }
    }
}
