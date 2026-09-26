// handlers/domains.rs — live DNS check for the dashboard's domain card
//
// GET /api/domains/check?domain=app.example.com
//
//   Resolves the domain (A/AAAA), compares the answer with this server's
//   public addresses (tond_common::dns, the same logic tond-proxy uses before
//   ordering a certificate) and returns:
//     - the exact records to create (type, name relative to the zone, value)
//       and, for domains under the apps domain, the wildcard that covers
//       every app;
//     - the DNS status: ok | mismatch | missing | inconclusive, with the
//       resolved and expected addresses and a readable message;
//     - the certificate that serves the domain, if any: its own row, else an
//       active wildcard (or uploaded wildcard) one label up (`covered_by`);
//     - the DNS provider that publishes its DNS-01 records, if one covers it.
//
// The server's public addresses come from the routing table
// (dns::server_addresses); behind NAT, where those are private, a public-IP
// lookup is made once per hour and cached. The apps domain is the
// `apps_domain` setting (apps never default to the dashboard's domain; see
// domains.rs). `warning` is set when the domain is on the dashboard's site.

use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tond_common::dns::{DnsCheck, classify_dns, is_public_ip, join_ips, server_addresses};
use tond_common::{ApiResponse, Certificate, TondError};

use crate::{
    auth::{Authz, RateLimit, scope},
    domains::{
        is_valid_hostname, normalize_domain, registrable_domain, same_site, same_site_warning,
    },
    error::AppError,
    handlers::onboarding,
    state::AppState,
};

const DNS_TIMEOUT: Duration = Duration::from_secs(3);
const PUBLIC_IP_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Deserialize)]
pub struct DomainCheckQuery {
    pub domain: String,
}

/// A DNS record to create.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DnsRecord {
    /// "A" | "AAAA"
    #[serde(rename = "type")]
    pub record_type: &'static str,
    /// Name relative to the zone ("app", "@", "*.apps")
    pub name: String,
    /// Fully qualified host the record is for
    pub host: String,
    pub value: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DnsStatus {
    /// "ok" | "mismatch" | "missing" | "inconclusive"
    pub status: &'static str,
    pub resolved: Vec<String>,
    pub expected: Vec<String>,
    /// Public addresses the domain resolves to that are not this server
    /// (a stale record that breaks some visitors and certificate checks)
    pub stray: Vec<String>,
    pub message: String,
}

/// The certificate fields the domain card shows.
#[derive(Debug, Serialize)]
pub struct CertificateSummary {
    /// The certificate's own domain (`*.apps.example.com` for a wildcard).
    pub domain: String,
    /// "acme" | "custom"
    pub cert_type: String,
    /// "http-01" | "dns-01"
    pub challenge: Option<String>,
    pub issuer: Option<String>,
    pub status: String,
    pub issued_at: String,
    pub expires_at: String,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<String>,
    pub next_retry_at: Option<String>,
    pub failure_count: i64,
}

impl From<Certificate> for CertificateSummary {
    fn from(c: Certificate) -> Self {
        Self {
            domain: c.domain,
            cert_type: c.cert_type,
            challenge: c.challenge,
            issuer: c.issuer,
            status: c.status,
            issued_at: c.issued_at,
            expires_at: c.expires_at,
            last_error: c.last_error,
            last_attempt_at: c.last_attempt_at,
            next_retry_at: c.next_retry_at,
            failure_count: c.failure_count,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DomainCheckResponse {
    pub domain: String,
    /// Registrable domain (the DNS zone the records go in), if known
    pub zone: Option<String>,
    /// Domain apps get by default (`<app>.<apps_domain>`), if configured
    pub apps_domain: Option<String>,
    /// The domain is under the apps domain (covered by its wildcard record)
    pub under_apps_domain: bool,
    /// Records that make the domain point at this server
    pub records: Vec<DnsRecord>,
    /// The wildcard records for the apps domain, when the domain is under it
    pub wildcard_records: Vec<DnsRecord>,
    /// This server's public addresses (empty when unknown)
    pub server_ips: Vec<String>,
    pub dns: DnsStatus,
    pub certificate: Option<CertificateSummary>,
    /// Set when a wildcard certificate serves the domain (its name).
    pub covered_by: Option<String>,
    /// Name of the DNS provider that validates this domain with DNS-01.
    pub dns_provider: Option<String>,
    pub checked_at: String,
    /// Set when the domain is on the same site as the dashboard (allowed,
    /// but shown as a warning)
    pub warning: Option<String>,
}

/// Name of `host` relative to `zone`: "@" for the apex, else the prefix.
pub fn relative_name(host: &str, zone: Option<&str>) -> String {
    match zone {
        Some(z) if host == z => "@".to_string(),
        Some(z) => host
            .strip_suffix(&format!(".{z}"))
            .map(str::to_string)
            .unwrap_or_else(|| host.to_string()),
        None => host.to_string(),
    }
}

/// A and AAAA records pointing `host` at `ips`.
pub fn records_for(host: &str, zone: Option<&str>, ips: &[IpAddr]) -> Vec<DnsRecord> {
    let name = relative_name(host, zone);
    ips.iter()
        .map(|ip| DnsRecord {
            record_type: if ip.is_ipv4() { "A" } else { "AAAA" },
            name: name.clone(),
            host: host.to_string(),
            value: ip.to_string(),
        })
        .collect()
}

/// Turn a lookup result into the card's DNS status.
pub fn dns_status(domain: &str, resolved: Option<&[IpAddr]>, server_ips: &[IpAddr]) -> DnsStatus {
    let resolved_list: Vec<IpAddr> = resolved.map(<[IpAddr]>::to_vec).unwrap_or_default();
    let expected: Vec<IpAddr> = server_ips.iter().copied().filter(is_public_ip).collect();
    let to_strings = |ips: &[IpAddr]| ips.iter().map(ToString::to_string).collect::<Vec<_>>();
    let stray: Vec<IpAddr> = resolved_list
        .iter()
        .copied()
        .filter(|ip| is_public_ip(ip) && !server_ips.contains(ip))
        .collect();

    let (status, message) = if resolved_list.is_empty() {
        (
            "missing",
            format!(
                "{domain} has no A or AAAA record yet. Create the record below; DNS changes usually show up within a few minutes."
            ),
        )
    } else {
        match classify_dns(Some(&resolved_list), server_ips) {
            DnsCheck::Matches if stray.is_empty() => {
                ("ok", format!("{domain} points at this server."))
            }
            DnsCheck::Matches => (
                "ok",
                format!(
                    "{domain} points at this server, but also at {}. Remove that record: visitors and Let's Encrypt may reach the other address.",
                    join_ips(&stray)
                ),
            ),
            check @ DnsCheck::Mismatch { .. } => (
                "mismatch",
                check.mismatch_message(domain).unwrap_or_default(),
            ),
            DnsCheck::Inconclusive => (
                "inconclusive",
                format!(
                    "{domain} resolves to {}. Tond can't confirm that's this server (it may be behind a proxy or CDN, or the answer is a private address).",
                    join_ips(&resolved_list)
                ),
            ),
        }
    };
    DnsStatus {
        status,
        resolved: to_strings(&resolved_list),
        expected: to_strings(&expected),
        stray: to_strings(&stray),
        message,
    }
}

async fn resolve(domain: &str) -> Option<Vec<IpAddr>> {
    let lookup = tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((domain, 443)))
        .await
        .ok()?
        .ok()?;
    let mut ips: Vec<IpAddr> = lookup.map(|a| a.ip()).collect();
    ips.sort();
    ips.dedup();
    Some(ips)
}

/// When the public IP was looked up, and the answer.
type CachedIp = Option<(Instant, Option<IpAddr>)>;

static PUBLIC_IP: LazyLock<Mutex<CachedIp>> = LazyLock::new(|| Mutex::new(None));

/// This server's public addresses: the routing table's when public, else a
/// cached public-IP lookup (NAT).
async fn public_server_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = server_addresses()
        .into_iter()
        .filter(is_public_ip)
        .collect();
    if ips.iter().any(IpAddr::is_ipv4) {
        return ips;
    }
    let mut cached = PUBLIC_IP.lock().await;
    let fresh = matches!(&*cached, Some((at, _)) if at.elapsed() < PUBLIC_IP_TTL);
    if !fresh {
        *cached = Some((Instant::now(), onboarding::fetch_public_ip().await));
    }
    if let Some((_, Some(ip))) = &*cached
        && !ips.contains(ip)
    {
        ips.insert(0, *ip);
    }
    ips
}

async fn apps_domain(state: &AppState) -> Option<String> {
    let configured: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'apps_domain'")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let domain = normalize_domain(&configured.unwrap_or_default());
    (!domain.is_empty()).then_some(domain)
}

pub async fn check_domain(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
    Query(query): Query<DomainCheckQuery>,
) -> Result<impl IntoResponse, AppError> {
    let domain = normalize_domain(&query.domain);
    if !is_valid_hostname(&domain) {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{}' isn't a valid domain name",
            query.domain.trim()
        ))));
    }

    let (resolved, server_ips) = tokio::join!(resolve(&domain), public_server_ips());
    let zone = registrable_domain(&domain);
    let apps_domain = apps_domain(&state).await;
    let under_apps_domain = apps_domain
        .as_deref()
        .is_some_and(|apps| domain.ends_with(&format!(".{apps}")));

    let records = records_for(&domain, zone.as_deref(), &server_ips);
    let wildcard_records = match (&apps_domain, under_apps_domain) {
        (Some(apps), true) => {
            let apps_zone = registrable_domain(apps);
            records_for(&format!("*.{apps}"), apps_zone.as_deref(), &server_ips)
        }
        _ => Vec::new(),
    };

    let dashboard = state.settings.read().await.dashboard_domain.clone();
    let warning = same_site(&domain, &dashboard).then(|| same_site_warning(&domain, &dashboard));

    let own =
        sqlx::query_as::<_, Certificate>("SELECT * FROM certificates WHERE domain = ? LIMIT 1")
            .bind(&domain)
            .fetch_optional(&state.db)
            .await?;
    let wildcard = match (&own, tond_common::dns::wildcard_for(&domain)) {
        (None, Some(w)) => {
            sqlx::query_as::<_, Certificate>(
                "SELECT * FROM certificates WHERE domain = ? AND status = 'active' LIMIT 1",
            )
            .bind(&w)
            .fetch_optional(&state.db)
            .await?
        }
        _ => None,
    };
    let covered_by = wildcard.as_ref().map(|c| c.domain.clone());
    let certificate = own.or(wildcard).map(CertificateSummary::from);
    let providers = crate::dns_providers::list(&state.db).await?;
    let dns_provider =
        crate::dns_providers::provider_for(&providers, &domain).map(|(p, _)| p.name.clone());

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(DomainCheckResponse {
            dns: dns_status(&domain, resolved.as_deref(), &server_ips),
            domain,
            zone,
            apps_domain,
            under_apps_domain,
            records,
            wildcard_records,
            server_ips: server_ips.iter().map(ToString::to_string).collect(),
            certificate,
            covered_by,
            dns_provider,
            checked_at: chrono::Utc::now().to_rfc3339(),
            warning,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn record_names_are_relative_to_the_zone() {
        assert_eq!(relative_name("app.example.com", Some("example.com")), "app");
        assert_eq!(relative_name("example.com", Some("example.com")), "@");
        assert_eq!(
            relative_name("*.apps.example.co.uk", Some("example.co.uk")),
            "*.apps"
        );
        assert_eq!(relative_name("x.y", None), "x.y");
        let records = records_for(
            "app.example.com",
            Some("example.com"),
            &[ip("203.0.113.7"), ip("2001:db8::7")],
        );
        assert_eq!(records[0].record_type, "A");
        assert_eq!(records[0].name, "app");
        assert_eq!(records[0].value, "203.0.113.7");
        assert_eq!(records[1].record_type, "AAAA");
    }

    #[test]
    fn dns_status_covers_every_state() {
        let server = [ip("203.0.113.7")];
        let s = dns_status("a.example.com", None, &server);
        assert_eq!(s.status, "missing");
        let s = dns_status("a.example.com", Some(&[]), &server);
        assert_eq!(s.status, "missing");

        let s = dns_status("a.example.com", Some(&[ip("203.0.113.7")]), &server);
        assert_eq!(s.status, "ok");
        assert!(s.stray.is_empty());

        let s = dns_status(
            "a.example.com",
            Some(&[ip("203.0.113.7"), ip("198.51.100.1")]),
            &server,
        );
        assert_eq!(s.status, "ok");
        assert_eq!(s.stray, vec!["198.51.100.1"]);
        assert!(s.message.contains("Remove that record"));

        let s = dns_status("a.example.com", Some(&[ip("198.51.100.1")]), &server);
        assert_eq!(s.status, "mismatch");
        assert_eq!(s.expected, vec!["203.0.113.7"]);

        let s = dns_status("a.example.com", Some(&[ip("10.0.0.1")]), &server);
        assert_eq!(s.status, "inconclusive");
    }
}
