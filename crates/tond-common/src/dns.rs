// dns.rs — compare a domain's DNS with this server's addresses
//
// Shared by tond-proxy (the pre-check before ordering a certificate, see
// tond-proxy/src/issuance.rs) and tond-server (the dashboard's domain card,
// GET /api/v1/domains/check). Pure functions only; the async lookup lives with
// each caller.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Outcome of comparing a domain's DNS with this server's addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsCheck {
    /// The domain resolves to one of our addresses.
    Matches,
    /// The domain clearly points somewhere else.
    Mismatch {
        resolved: Vec<IpAddr>,
        expected: Vec<IpAddr>,
    },
    /// Could not decide (lookup failure, NXDOMAIN, private answers, NAT).
    Inconclusive,
}

impl DnsCheck {
    /// The dashboard message for a mismatch.
    pub fn mismatch_message(&self, domain: &str) -> Option<String> {
        let DnsCheck::Mismatch { resolved, expected } = self else {
            return None;
        };
        Some(format!(
            "DNS for {domain} points to {}, expected {}. Update the domain's A/AAAA record to this server.",
            join_ips(resolved),
            join_ips(expected),
        ))
    }
}

pub fn join_ips(ips: &[IpAddr]) -> String {
    ips.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// True for addresses reachable from the public internet (no private,
/// loopback, link-local, CGNAT, unique-local, multicast or unspecified).
pub fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(&v4);
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (first & 0xffc0) == 0xfe80) // link local fe80::/10
        }
    }
}

fn is_public_v4(v4: &Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_unspecified()
        || v4.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))) // CGNAT 100.64.0.0/10
}

/// Decide from a lookup result and our addresses.
pub fn classify_dns(resolved: Option<&[IpAddr]>, server_ips: &[IpAddr]) -> DnsCheck {
    let server_public: Vec<IpAddr> = server_ips
        .iter()
        .filter(|ip| is_public_ip(ip))
        .copied()
        .collect();
    let Some(resolved) = resolved.filter(|r| !r.is_empty()) else {
        return DnsCheck::Inconclusive; // lookup failed / NXDOMAIN
    };
    if resolved.iter().any(|ip| server_ips.contains(ip)) {
        return DnsCheck::Matches;
    }
    if server_public.is_empty() || resolved.iter().any(|ip| !is_public_ip(ip)) {
        // Behind NAT (we don't know our public IP) or split-horizon/test DNS.
        return DnsCheck::Inconclusive;
    }
    let mut resolved = resolved.to_vec();
    resolved.sort();
    resolved.dedup();
    DnsCheck::Mismatch {
        resolved,
        expected: server_public,
    }
}

/// This host's primary outbound addresses. Uses the routing table only
/// (connecting a UDP socket sends no packets). On a typical VPS these are the
/// public addresses; behind NAT they are private.
pub fn server_addresses() -> Vec<IpAddr> {
    let probes: [(SocketAddr, SocketAddr); 2] = [
        (
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 53)),
        ),
        (
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
            SocketAddr::from((
                Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111),
                53,
            )),
        ),
    ];
    probes
        .iter()
        .filter_map(|(bind, target)| {
            let socket = std::net::UdpSocket::bind(bind).ok()?;
            socket.connect(target).ok()?;
            Some(socket.local_addr().ok()?.ip())
        })
        .collect()
}

/// `host` equals `zone` or is a subdomain of it (both normalized).
fn in_zone(host: &str, zone: &str) -> bool {
    host == zone
        || host
            .strip_suffix(zone)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// The longest of `zones` that contains `domain` (a wildcard `*.x` is looked
/// up as `x`). Zones are plain names like `example.com`.
pub fn zone_for<'a>(domain: &str, zones: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let host = domain.strip_prefix("*.").unwrap_or(domain);
    zones
        .into_iter()
        .filter(|z| !z.is_empty() && in_zone(host, z))
        .max_by_key(|z| z.len())
}

/// The TXT record name a DNS-01 challenge for `domain` is published under:
/// `_acme-challenge.<domain>`, where a wildcard `*.x` uses `x`.
pub fn acme_challenge_name(domain: &str) -> String {
    format!(
        "_acme-challenge.{}",
        domain.strip_prefix("*.").unwrap_or(domain)
    )
}

/// The domain a challenge record name belongs to (inverse of
/// `acme_challenge_name`, minus the wildcard).
pub fn challenge_domain(record_name: &str) -> Option<&str> {
    record_name
        .trim_end_matches('.')
        .strip_prefix("_acme-challenge.")
        .filter(|d| !d.is_empty())
}

/// `pattern` is a wildcard (`*.x`) that covers `host`: exactly one label in
/// front of `x`, as TLS wildcard matching works (RFC 6125).
pub fn wildcard_covers(pattern: &str, host: &str) -> bool {
    let Some(base) = pattern.strip_prefix("*.") else {
        return false;
    };
    host.strip_suffix(base)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

/// The wildcard name that would cover `host` (`a.b.c` -> `*.b.c`), if the
/// host has a parent with at least two labels.
pub fn wildcard_for(host: &str) -> Option<String> {
    let (_, parent) = host.split_once('.')?;
    parent.contains('.').then(|| format!("*.{parent}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_for_picks_the_longest_containing_zone() {
        let zones = ["example.com", "apps.example.com", "other.org", ""];
        assert_eq!(zone_for("a.example.com", zones), Some("example.com"));
        assert_eq!(
            zone_for("x.apps.example.com", zones),
            Some("apps.example.com")
        );
        assert_eq!(
            zone_for("*.apps.example.com", zones),
            Some("apps.example.com")
        );
        assert_eq!(zone_for("example.com", zones), Some("example.com"));
        assert_eq!(
            zone_for("badexample.com", zones),
            None,
            "suffix, not a subdomain"
        );
        assert_eq!(zone_for("example.net", zones), None);
    }

    #[test]
    fn challenge_names_strip_the_wildcard() {
        assert_eq!(
            acme_challenge_name("*.apps.test"),
            "_acme-challenge.apps.test"
        );
        assert_eq!(acme_challenge_name("a.b.test"), "_acme-challenge.a.b.test");
        assert_eq!(
            challenge_domain("_acme-challenge.apps.test."),
            Some("apps.test")
        );
        assert_eq!(challenge_domain("_acme-challenge."), None);
        assert_eq!(challenge_domain("apps.test"), None);
    }

    #[test]
    fn wildcards_cover_exactly_one_label() {
        assert!(wildcard_covers("*.apps.test", "web.apps.test"));
        assert!(!wildcard_covers("*.apps.test", "apps.test"), "not the base");
        assert!(
            !wildcard_covers("*.apps.test", "a.b.apps.test"),
            "one label only"
        );
        assert!(!wildcard_covers("*.apps.test", "webapps.test"));
        assert!(
            !wildcard_covers("apps.test", "web.apps.test"),
            "not a wildcard"
        );
        assert_eq!(
            wildcard_for("web.apps.test").as_deref(),
            Some("*.apps.test")
        );
        assert_eq!(wildcard_for("apps.test"), None, "no wildcard over a TLD");
        assert_eq!(wildcard_for("localhost"), None);
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap_or_else(|err| panic!("bad ip {s}: {err}"))
    }

    #[test]
    fn public_ip_filter_excludes_private_loopback_and_cgnat() {
        for private in [
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "127.0.0.1",
            "100.64.0.1",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(&ip(private)), "{private} is not public");
        }
        for public in ["203.0.113.7", "8.8.8.8", "2001:4860::1", "::ffff:8.8.8.8"] {
            assert!(is_public_ip(&ip(public)), "{public} is public");
        }
    }

    #[test]
    fn dns_check_only_blocks_on_a_clear_public_mismatch() {
        let server = vec![ip("203.0.113.10"), ip("10.0.0.5")];

        assert_eq!(
            classify_dns(Some(&[ip("203.0.113.10")]), &server),
            DnsCheck::Matches
        );
        assert_eq!(
            classify_dns(Some(&[ip("198.51.100.4"), ip("203.0.113.10")]), &server),
            DnsCheck::Matches,
            "any matching record is enough"
        );
        assert_eq!(
            classify_dns(Some(&[ip("198.51.100.4")]), &server),
            DnsCheck::Mismatch {
                resolved: vec![ip("198.51.100.4")],
                expected: vec![ip("203.0.113.10")],
            }
        );
        // NXDOMAIN / lookup failure: try anyway (e2e domains like whoami.tond.test).
        assert_eq!(classify_dns(None, &server), DnsCheck::Inconclusive);
        assert_eq!(classify_dns(Some(&[]), &server), DnsCheck::Inconclusive);
        // Fake or split-horizon DNS answering with private/loopback addresses.
        assert_eq!(
            classify_dns(Some(&[ip("127.0.0.1")]), &server),
            DnsCheck::Inconclusive
        );
        // Behind NAT: we only know a private address for ourselves.
        assert_eq!(
            classify_dns(Some(&[ip("198.51.100.4")]), &[ip("10.0.0.5")]),
            DnsCheck::Inconclusive
        );
    }

    #[test]
    fn mismatch_message_names_both_addresses() {
        let check = DnsCheck::Mismatch {
            resolved: vec![ip("1.2.3.4")],
            expected: vec![ip("5.6.7.8")],
        };
        let message = check
            .mismatch_message("x.com")
            .unwrap_or_else(|| panic!("mismatch should have a message"));
        assert!(message.starts_with("DNS for x.com points to 1.2.3.4, expected 5.6.7.8."));
        assert_eq!(DnsCheck::Inconclusive.mismatch_message("x.com"), None);
    }
}
