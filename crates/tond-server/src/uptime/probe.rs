// uptime/probe.rs — one HTTPS probe through the public path
//
//   1. DNS      resolve the host (A/AAAA) like a visitor would, and compare
//               the answer with this server's addresses (tond_common::dns).
//               A name that points elsewhere (a CDN, a stale record) is noted
//               but still probed: visitors go there too.
//   2. TCP      connect to the resolved public address on 443 — never
//               loopback — so the request crosses tond-proxy exactly like
//               internet traffic.
//   3. TLS      handshake with SNI, verified against Mozilla's root store
//               (webpki-roots, compiled in: no files, no allocations for the
//               anchors). The leaf certificate's expiry is read from its DER.
//   4. HTTP     `GET <path> HTTP/1.1` with `Connection: close`; only the
//               status line is read.
//
// Each step's failure maps to a cause (dns, refused, connect, timeout, tls,
// http_5xx, http_status) with a sentence that says what to do.
//
// Tests only (never set in production):
//   TOND_UPTIME_RESOLVE=<ip>     connect to this address instead of resolving
//                                (the e2e hosts have no public DNS)
//   TOND_UPTIME_CA_FILE=<pem>    extra trusted roots (the e2e ACME server's
//                                issuing root, which no browser trusts)

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, ServerName};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tond_common::dns::{DnsCheck, classify_dns};

const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEAD: usize = 8 * 1024;
const USER_AGENT: &str = "Tond-Uptime/1 (+https://github.com/tahayvr/tond)";

/// Why a probe failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    Dns,
    Refused,
    Connect,
    Timeout,
    Tls,
    Http5xx,
    HttpStatus,
}

impl Cause {
    /// Stored in uptime_1m.result (0 is "up").
    pub fn code(self) -> i64 {
        match self {
            Cause::Dns => 1,
            Cause::Refused => 2,
            Cause::Connect => 3,
            Cause::Timeout => 4,
            Cause::Tls => 5,
            Cause::Http5xx => 6,
            Cause::HttpStatus => 7,
        }
    }

    /// Decode uptime_1m.result (for readers of the table, e.g. alerts).
    #[allow(dead_code)]
    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            1 => Cause::Dns,
            2 => Cause::Refused,
            3 => Cause::Connect,
            4 => Cause::Timeout,
            5 => Cause::Tls,
            6 => Cause::Http5xx,
            7 => Cause::HttpStatus,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Cause::Dns => "dns",
            Cause::Refused => "refused",
            Cause::Connect => "connect",
            Cause::Timeout => "timeout",
            Cause::Tls => "tls",
            Cause::Http5xx => "http_5xx",
            Cause::HttpStatus => "http_status",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Cause::Dns,
            Cause::Refused,
            Cause::Connect,
            Cause::Timeout,
            Cause::Tls,
            Cause::Http5xx,
            Cause::HttpStatus,
        ]
        .into_iter()
        .find(|c| c.as_str() == s)
    }
}

/// How the host's DNS compares with this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DnsState {
    /// Resolves to this server.
    Matches,
    /// Resolves to public addresses that are not this server (a CDN or a
    /// stale record).
    Elsewhere,
    /// Can't tell (private answers, NAT).
    Unknown,
    /// Did not resolve.
    Failed,
    /// TOND_UPTIME_RESOLVE is set (tests).
    Overridden,
}

/// What to probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSpec {
    pub host: String,
    pub path: String,
    /// None = any 2xx or 3xx.
    pub expected_status: Option<u16>,
    pub timeout: Duration,
}

/// The outcome of one probe.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ProbeResult {
    pub ok: bool,
    pub cause: Option<Cause>,
    /// What happened, in a sentence.
    pub detail: String,
    pub status: Option<u16>,
    /// Time to the response's status line (or to the failure).
    pub latency_ms: u32,
    pub ip: Option<IpAddr>,
    pub dns: DnsState,
    /// Leaf certificate expiry, unix seconds.
    pub cert_expires_at: Option<i64>,
}

impl ProbeResult {
    fn fail(cause: Cause, detail: String, started: Instant, ctx: &Ctx) -> Self {
        Self {
            ok: false,
            cause: Some(cause),
            detail,
            status: None,
            latency_ms: elapsed_ms(started),
            ip: ctx.ip,
            dns: ctx.dns,
            cert_expires_at: ctx.cert_expires_at,
        }
    }

    /// Stored result code: 0 up, else the cause's code.
    pub fn code(&self) -> i64 {
        self.cause.map_or(0, Cause::code)
    }
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Where a probe is, so a timeout can say where it got stuck.
const PHASE_DNS: u8 = 0;
const PHASE_CONNECT: u8 = 1;
const PHASE_TLS: u8 = 2;
const PHASE_HTTP: u8 = 3;

#[derive(Debug, Clone, Copy)]
struct Ctx {
    ip: Option<IpAddr>,
    dns: DnsState,
    cert_expires_at: Option<i64>,
}

/// The TLS client configuration every round reuses, built once (the root
/// store copies ~150 anchors).
static TLS: LazyLock<tokio_rustls::TlsConnector> = LazyLock::new(|| tls_connector(Vec::new()));

/// Mozilla roots + `extra` + TOND_UPTIME_CA_FILE.
fn tls_connector(extra: Vec<CertificateDer<'static>>) -> tokio_rustls::TlsConnector {
    tond_common::install_crypto_provider();
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    for cert in extra {
        let _ = roots.add(cert);
    }
    if let Ok(path) = std::env::var("TOND_UPTIME_CA_FILE")
        && !path.is_empty()
    {
        use rustls::pki_types::pem::PemObject;
        match CertificateDer::pem_file_iter(&path) {
            Ok(certs) => {
                for cert in certs.flatten() {
                    let _ = roots.add(cert);
                }
            }
            Err(e) => tracing::debug!("uptime: cannot read TOND_UPTIME_CA_FILE {path}: {e}"),
        }
    }
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // Every probe does a full handshake: a resumed session carries no
    // certificate, so its expiry could not be read or a replaced (or
    // expired) certificate re-verified.
    config.resumption = rustls::client::Resumption::disabled();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

/// One round of probes: the shared TLS configuration and this server's
/// addresses as of the round.
pub struct Prober {
    tls: tokio_rustls::TlsConnector,
    resolve_override: Option<IpAddr>,
    server_ips: Vec<IpAddr>,
    /// 443, except in tests.
    port: u16,
}

impl Prober {
    /// Reuse the shared TLS configuration and read this server's addresses.
    pub fn new() -> Self {
        Self::with_tls(TLS.clone())
    }

    fn with_tls(tls: tokio_rustls::TlsConnector) -> Self {
        let resolve_override = std::env::var("TOND_UPTIME_RESOLVE")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        Self {
            tls,
            resolve_override,
            server_ips: tond_common::dns::server_addresses(),
            port: 443,
        }
    }

    #[cfg(test)]
    fn with_extra_roots(extra: Vec<CertificateDer<'static>>) -> Self {
        Self::with_tls(tls_connector(extra))
    }

    /// Probe once, within `spec.timeout`.
    pub async fn probe(&self, spec: &ProbeSpec) -> ProbeResult {
        let started = Instant::now();
        let phase = AtomicU8::new(PHASE_DNS);
        let ctx = std::sync::Mutex::new(Ctx {
            ip: None,
            dns: DnsState::Unknown,
            cert_expires_at: None,
        });
        match tokio::time::timeout(spec.timeout, self.run(spec, started, &phase, &ctx)).await {
            Ok(result) => result,
            Err(_) => {
                let ctx = *ctx.lock().unwrap_or_else(|e| e.into_inner());
                let secs = spec.timeout.as_secs();
                let detail = match phase.load(Ordering::Relaxed) {
                    PHASE_DNS => format!("Looking up {} took longer than {secs} s.", spec.host),
                    PHASE_CONNECT => format!(
                        "No answer from {} on port 443 within {secs} s. Is a firewall blocking HTTPS?",
                        ctx.ip.map(|i| i.to_string()).unwrap_or_default()
                    ),
                    PHASE_TLS => format!("The TLS handshake did not finish within {secs} s."),
                    _ => format!(
                        "The app did not answer within {secs} s (the connection and TLS were fine)."
                    ),
                };
                ProbeResult::fail(Cause::Timeout, detail, started, &ctx)
            }
        }
    }

    async fn run(
        &self,
        spec: &ProbeSpec,
        started: Instant,
        phase: &AtomicU8,
        shared: &std::sync::Mutex<Ctx>,
    ) -> ProbeResult {
        let mut ctx = Ctx {
            ip: None,
            dns: DnsState::Unknown,
            cert_expires_at: None,
        };
        let publish = |ctx: &Ctx| {
            if let Ok(mut c) = shared.lock() {
                *c = *ctx;
            }
        };

        // 1. DNS
        let ip = match self.resolve_override {
            Some(ip) => {
                ctx.dns = DnsState::Overridden;
                ip
            }
            None => match resolve(&spec.host).await {
                Ok(ips) => {
                    ctx.dns = dns_state(&ips, &self.server_ips);
                    pick_address(&ips, &self.server_ips)
                }
                Err(detail) => {
                    ctx.dns = DnsState::Failed;
                    return ProbeResult::fail(Cause::Dns, detail, started, &ctx);
                }
            },
        };
        ctx.ip = Some(ip);
        publish(&ctx);

        // 2. TCP
        phase.store(PHASE_CONNECT, Ordering::Relaxed);
        let port = self.port;
        let tcp = match TcpStream::connect(SocketAddr::new(ip, port)).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                return ProbeResult::fail(
                    Cause::Refused,
                    format!(
                        "{ip} refused the connection on port {port}: nothing is listening. Is tond-proxy running?"
                    ),
                    started,
                    &ctx,
                );
            }
            Err(e) => {
                return ProbeResult::fail(
                    Cause::Connect,
                    format!("Couldn't connect to {ip}:{port}: {e}."),
                    started,
                    &ctx,
                );
            }
        };
        let _ = tcp.set_nodelay(true);

        // 3. TLS
        phase.store(PHASE_TLS, Ordering::Relaxed);
        let Ok(name) = ServerName::try_from(spec.host.clone()) else {
            return ProbeResult::fail(
                Cause::Tls,
                format!("{} is not a valid TLS server name.", spec.host),
                started,
                &ctx,
            );
        };
        let mut tls = match self.tls.connect(name, tcp).await {
            Ok(s) => s,
            Err(e) => {
                return ProbeResult::fail(
                    Cause::Tls,
                    describe_tls_error(&e, &spec.host),
                    started,
                    &ctx,
                );
            }
        };
        ctx.cert_expires_at = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|certs| certs.first())
            .and_then(|leaf| not_after(leaf.as_ref()));
        publish(&ctx);

        // 4. HTTP
        phase.store(PHASE_HTTP, Ordering::Relaxed);
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
            spec.path, spec.host
        );
        if let Err(e) = tls.write_all(request.as_bytes()).await {
            return ProbeResult::fail(
                Cause::Connect,
                format!("The connection broke while sending the request: {e}."),
                started,
                &ctx,
            );
        }
        let status = match read_status(&mut tls).await {
            Ok(s) => s,
            Err(detail) => return ProbeResult::fail(Cause::Connect, detail, started, &ctx),
        };
        let latency_ms = elapsed_ms(started);
        let (ok, cause, detail) = judge(status, spec.expected_status);
        ProbeResult {
            ok,
            cause,
            detail,
            status: Some(status),
            latency_ms,
            ip: ctx.ip,
            dns: ctx.dns,
            cert_expires_at: ctx.cert_expires_at,
        }
    }
}

async fn resolve(host: &str) -> Result<Vec<IpAddr>, String> {
    match tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((host, 443))).await {
        Ok(Ok(addrs)) => {
            let mut ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
            ips.dedup();
            if ips.is_empty() {
                Err(format!(
                    "{host} has no A or AAAA record. Point it at this server."
                ))
            } else {
                Ok(ips)
            }
        }
        Ok(Err(_)) => Err(format!(
            "{host} doesn't resolve (no A or AAAA record, or the name doesn't exist). Check the DNS record."
        )),
        Err(_) => Err(format!(
            "Looking up {host} took longer than {} s. The DNS provider may be having trouble.",
            DNS_TIMEOUT.as_secs()
        )),
    }
}

fn dns_state(resolved: &[IpAddr], server_ips: &[IpAddr]) -> DnsState {
    match classify_dns(Some(resolved), server_ips) {
        DnsCheck::Matches => DnsState::Matches,
        DnsCheck::Mismatch { .. } => DnsState::Elsewhere,
        DnsCheck::Inconclusive => DnsState::Unknown,
    }
}

/// This server's address when the name has it, else the first IPv4, else
/// the first address.
fn pick_address(resolved: &[IpAddr], server_ips: &[IpAddr]) -> IpAddr {
    resolved
        .iter()
        .find(|ip| server_ips.contains(ip))
        .or_else(|| resolved.iter().find(|ip| ip.is_ipv4()))
        .or(resolved.first())
        .copied()
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}

/// Read up to the end of the status line and parse the status code.
async fn read_status<S: AsyncReadExt + Unpin>(stream: &mut S) -> Result<u16, String> {
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    loop {
        if let Some(end) = buf.windows(2).position(|w| w == b"\r\n") {
            return parse_status_line(&buf[..end]).ok_or_else(|| {
                "The server answered, but not with HTTP. Is the domain routed to the right service?"
                    .to_string()
            });
        }
        if buf.len() > MAX_HEAD {
            return Err("The server's response started with an overlong line.".into());
        }
        match stream.read(&mut chunk).await {
            Ok(0) => {
                return Err(
                    "The connection closed before any response arrived. The proxy may have dropped it."
                        .into(),
                );
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                return Err(format!(
                    "The connection broke while waiting for the response: {e}."
                ));
            }
        }
    }
}

fn parse_status_line(line: &[u8]) -> Option<u16> {
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts
        .next()?
        .parse()
        .ok()
        .filter(|s| (100..600).contains(s))
}

/// Up or down, given the status and what was expected.
fn judge(status: u16, expected: Option<u16>) -> (bool, Option<Cause>, String) {
    match expected {
        Some(want) if status == want => (true, None, format!("HTTP {status}")),
        None if (200..400).contains(&status) => (true, None, format!("HTTP {status}")),
        _ if status >= 500 => (
            false,
            Some(Cause::Http5xx),
            match status {
                502 | 503 => format!(
                    "HTTP {status}: the proxy couldn't reach the app. Is it running and listening on its port?"
                ),
                504 => "HTTP 504: the app took too long to answer the proxy.".to_string(),
                _ => format!("HTTP {status}: the app returned a server error. Check its logs."),
            },
        ),
        Some(want) => (
            false,
            Some(Cause::HttpStatus),
            format!("HTTP {status}, expected {want}."),
        ),
        None => (
            false,
            Some(Cause::HttpStatus),
            format!("HTTP {status}, expected a 2xx or 3xx status. Check the check's path."),
        ),
    }
}

fn describe_tls_error(e: &std::io::Error, host: &str) -> String {
    use rustls::{AlertDescription, CertificateError, Error as TlsError};
    let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<TlsError>()) else {
        return format!("The TLS handshake failed: {e}.");
    };
    match inner {
        TlsError::InvalidCertificate(CertificateError::Expired)
        | TlsError::InvalidCertificate(CertificateError::ExpiredContext { .. }) => format!(
            "The certificate for {host} has expired. Check Settings → Domain for renewal errors."
        ),
        TlsError::InvalidCertificate(CertificateError::NotValidForName)
        | TlsError::InvalidCertificate(CertificateError::NotValidForNameContext { .. }) => {
            format!("The certificate served is not valid for {host}.")
        }
        TlsError::InvalidCertificate(CertificateError::UnknownIssuer) => format!(
            "The certificate for {host} isn't from a publicly trusted authority (self-signed or a staging certificate)."
        ),
        TlsError::InvalidCertificate(other) => {
            format!("The certificate for {host} was rejected: {other:?}.")
        }
        TlsError::AlertReceived(AlertDescription::HandshakeFailure)
        | TlsError::AlertReceived(AlertDescription::UnrecognisedName)
        | TlsError::AlertReceived(AlertDescription::InternalError) => format!(
            "The server has no certificate for {host} yet. Certificates are issued once DNS points here."
        ),
        other => format!("The TLS handshake failed: {other}."),
    }
}

// ── DER: the leaf certificate's notAfter ──

/// One DER TLV: (tag, content, rest).
fn tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first & 0x80 == 0 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    Some((tag, &rest[..len], &rest[len..]))
}

/// The `notAfter` of an X.509 certificate, as unix seconds.
pub fn not_after(cert: &[u8]) -> Option<i64> {
    let (_, certificate, _) = tlv(cert)?;
    let (_, tbs, _) = tlv(certificate)?;
    let mut rest = tbs;
    // Optional [0] EXPLICIT version.
    if rest.first() == Some(&0xa0) {
        rest = tlv(rest)?.2;
    }
    rest = tlv(rest)?.2; // serialNumber
    rest = tlv(rest)?.2; // signature AlgorithmIdentifier
    rest = tlv(rest)?.2; // issuer
    let (_, validity, _) = tlv(rest)?;
    let (_, _, after_not_before) = tlv(validity)?;
    let (tag, time, _) = tlv(after_not_before)?;
    parse_time(tag, std::str::from_utf8(time).ok()?)
}

fn parse_time(tag: u8, s: &str) -> Option<i64> {
    use chrono::NaiveDateTime;
    let s = s.strip_suffix('Z')?;
    let full = match tag {
        // UTCTime: YYMMDDHHMMSS; 50–99 → 19xx.
        0x17 => {
            let yy: i32 = s.get(..2)?.parse().ok()?;
            let century = if yy >= 50 { "19" } else { "20" };
            format!("{century}{s}")
        }
        // GeneralizedTime: YYYYMMDDHHMMSS
        0x18 => s.to_string(),
        _ => return None,
    };
    NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%S")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes_round_trip_through_codes_and_names() {
        for c in [
            Cause::Dns,
            Cause::Refused,
            Cause::Connect,
            Cause::Timeout,
            Cause::Tls,
            Cause::Http5xx,
            Cause::HttpStatus,
        ] {
            assert_eq!(Cause::from_code(c.code()), Some(c));
            assert_eq!(Cause::parse(c.as_str()), Some(c));
        }
        assert_eq!(Cause::from_code(0), None);
    }

    #[test]
    fn statuses_are_judged_against_the_expectation() {
        assert!(judge(200, None).0);
        assert!(judge(301, None).0);
        assert_eq!(judge(404, None).1, Some(Cause::HttpStatus));
        assert_eq!(judge(502, None).1, Some(Cause::Http5xx));
        assert!(judge(502, None).2.contains("couldn't reach the app"));
        assert!(judge(401, Some(401)).0);
        assert_eq!(judge(200, Some(204)).1, Some(Cause::HttpStatus));
        assert_eq!(judge(503, Some(200)).1, Some(Cause::Http5xx));
    }

    #[test]
    fn status_lines_parse() {
        assert_eq!(parse_status_line(b"HTTP/1.1 200 OK"), Some(200));
        assert_eq!(
            parse_status_line(b"HTTP/1.0 503 Service Unavailable"),
            Some(503)
        );
        assert_eq!(parse_status_line(b"SSH-2.0-OpenSSH"), None);
        assert_eq!(parse_status_line(b"HTTP/1.1 999 Nope"), None);
    }

    #[test]
    fn prefers_this_servers_address_then_ipv4() {
        let ours: IpAddr = "203.0.113.5".parse().unwrap_or_else(|e| panic!("{e}"));
        let other: IpAddr = "198.51.100.7".parse().unwrap_or_else(|e| panic!("{e}"));
        let v6: IpAddr = "2001:db8::1".parse().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(pick_address(&[v6, other, ours], &[ours]), ours);
        assert_eq!(pick_address(&[v6, other], &[ours]), other);
        assert_eq!(pick_address(&[v6], &[ours]), v6);
        assert_eq!(dns_state(&[ours], &[ours]), DnsState::Matches);
        assert_eq!(dns_state(&[other], &[ours]), DnsState::Elsewhere);
    }

    #[test]
    fn reads_not_after_from_a_generated_certificate() {
        let mut params = rcgen::CertificateParams::new(vec!["example.test".to_string()])
            .unwrap_or_else(|e| panic!("params: {e}"));
        params.not_after = rcgen::date_time_ymd(2031, 5, 17);
        let key = rcgen::KeyPair::generate().unwrap_or_else(|e| panic!("key: {e}"));
        let cert = params
            .self_signed(&key)
            .unwrap_or_else(|e| panic!("cert: {e}"));
        let expected = chrono::NaiveDate::from_ymd_opt(2031, 5, 17)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|t| t.and_utc().timestamp());
        assert_eq!(not_after(cert.der()), expected);
        assert_eq!(not_after(b"\x30\x03\x02\x01"), None);
        assert_eq!(parse_time(0x17, "491231235959Z").map(|t| t > 0), Some(true));
        assert_eq!(
            parse_time(0x17, "991231235959Z").map(|t| t < 946_684_800),
            Some(true)
        );
    }

    async fn serve_once(response: &'static [u8]) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(response).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn read_status_parses_a_real_response_and_reports_garbage() {
        let port = serve_once(b"HTTP/1.1 204 No Content\r\nServer: x\r\n\r\n").await;
        let mut s = TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| panic!("connect: {e}"));
        s.write_all(b"GET / HTTP/1.1\r\n\r\n")
            .await
            .unwrap_or_default();
        assert_eq!(read_status(&mut s).await, Ok(204));

        let port = serve_once(b"").await;
        let mut s = TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| panic!("connect: {e}"));
        s.write_all(b"GET / HTTP/1.1\r\n\r\n")
            .await
            .unwrap_or_default();
        assert!(read_status(&mut s).await.is_err());
    }

    #[tokio::test]
    async fn unresolvable_names_fail_with_a_dns_cause() {
        let prober = Prober {
            resolve_override: None,
            ..Prober::new()
        };
        let r = prober
            .probe(&ProbeSpec {
                host: "definitely-not-a-real-name.invalid".into(),
                path: "/".into(),
                expected_status: None,
                timeout: Duration::from_secs(8),
            })
            .await;
        assert!(!r.ok);
        assert!(matches!(r.cause, Some(Cause::Dns) | Some(Cause::Timeout)));
        assert_eq!(r.code(), r.cause.map_or(0, Cause::code));
    }

    fn local_prober(port: u16, extra: Vec<CertificateDer<'static>>) -> Prober {
        Prober {
            resolve_override: Some(IpAddr::from([127, 0, 0, 1])),
            port,
            ..Prober::with_extra_roots(extra)
        }
    }

    fn spec(host: &str) -> ProbeSpec {
        ProbeSpec {
            host: host.into(),
            path: "/health".into(),
            expected_status: None,
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn refused_connections_are_classified() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        drop(listener);
        let r = local_prober(port, Vec::new())
            .probe(&spec("app.test"))
            .await;
        assert_eq!(r.cause, Some(Cause::Refused));
        assert_eq!(r.dns, DnsState::Overridden);
        assert!(r.detail.contains("refused"));
    }

    /// A TLS server for `host` with a fresh self-signed certificate; answers
    /// every request with `response`.
    async fn tls_server(host: &str, response: &'static [u8]) -> (u16, CertificateDer<'static>) {
        let key = rcgen::KeyPair::generate().unwrap_or_else(|e| panic!("key: {e}"));
        let mut params = rcgen::CertificateParams::new(vec![host.to_string()])
            .unwrap_or_else(|e| panic!("params: {e}"));
        params.not_after = rcgen::date_time_ymd(2040, 1, 1);
        let cert = params
            .self_signed(&key)
            .unwrap_or_else(|e| panic!("cert: {e}"));
        let der = cert.der().clone();
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der())
            .unwrap_or_else(|e| panic!("key der: {e}"));
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![der.clone()], key_der)
            .unwrap_or_else(|e| panic!("server config: {e}"));
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(tcp).await {
                        let mut buf = [0u8; 1024];
                        let _ = tls.read(&mut buf).await;
                        let _ = tls.write_all(response).await;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });
        (port, der)
    }

    #[tokio::test]
    async fn a_trusted_certificate_and_a_2xx_answer_is_up_with_the_expiry_read() {
        tond_common::install_crypto_provider();
        let (port, cert) =
            tls_server("app.test", b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let r = local_prober(port, vec![cert])
            .probe(&spec("app.test"))
            .await;
        assert!(r.ok, "{r:?}");
        assert_eq!(r.status, Some(200));
        let expected = chrono::NaiveDate::from_ymd_opt(2040, 1, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|t| t.and_utc().timestamp());
        assert_eq!(r.cert_expires_at, expected);
    }

    #[tokio::test]
    async fn untrusted_or_misnamed_certificates_and_5xx_are_down() {
        tond_common::install_crypto_provider();
        let (port, cert) = tls_server("app.test", b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
        let untrusted = local_prober(port, Vec::new())
            .probe(&spec("app.test"))
            .await;
        assert_eq!(untrusted.cause, Some(Cause::Tls));
        assert!(
            untrusted.detail.contains("publicly trusted"),
            "{}",
            untrusted.detail
        );

        let misnamed = local_prober(port, vec![cert.clone()])
            .probe(&spec("other.test"))
            .await;
        assert_eq!(misnamed.cause, Some(Cause::Tls));
        assert!(
            misnamed.detail.contains("not valid for other.test"),
            "{}",
            misnamed.detail
        );

        let bad_gateway = local_prober(port, vec![cert])
            .probe(&spec("app.test"))
            .await;
        assert_eq!(bad_gateway.cause, Some(Cause::Http5xx));
        assert_eq!(bad_gateway.status, Some(502));
        assert!(bad_gateway.cert_expires_at.is_some());
    }
}
