// dns.rs — compare a domain's DNS with this server's addresses
//
// Shared by tond-proxy (the pre-check before ordering a certificate, see
// tond-proxy/src/issuance.rs) and tond-server (the dashboard's domain card,
// GET /api/domains/check). Pure functions only; the async lookup lives with
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

#[cfg(test)]
mod tests {
    use super::*;

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
