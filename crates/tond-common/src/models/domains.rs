// domains.rs — app domains (several per app, redirects) and DNS providers
//
// An app answers on any number of domains (`app_domains` table):
//
//   primary   the app's main address (at most one per app; mirrored into
//             `apps.domain` by SQL triggers so older readers keep working)
//   alias     also served, same content
//   redirect  answered by tond-proxy itself with a 301/308 to `redirect_to`
//             (another domain of the same app), path and query preserved;
//             the app's container never sees these requests
//
// `service` is NULL for the app's main container; compose apps may route a
// domain to one of their services (with `port` the service's container port).
//
// DNS providers let Tond publish ACME DNS-01 challenges: wildcard
// certificates for the apps and preview domains (one order instead of one per
// host) and certificates for domains that can't be reached over HTTP. Their
// API tokens are write-only (encrypted at rest, never returned).

use serde::{Deserialize, Serialize};

// ── App domains ───────────────────────────────────────────────────────────────

/// What a domain does for its app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DomainKind {
    Primary,
    Alias,
    Redirect,
}

impl DomainKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DomainKind::Primary => "primary",
            DomainKind::Alias => "alias",
            DomainKind::Redirect => "redirect",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "primary" => Some(DomainKind::Primary),
            "alias" => Some(DomainKind::Alias),
            "redirect" => Some(DomainKind::Redirect),
            _ => None,
        }
    }
}

/// Status codes a redirect domain may answer with. 308 (the default) keeps
/// the method and body; 301 is the classic permanent redirect.
pub const REDIRECT_CODES: [u16; 2] = [301, 308];

/// Raw row from `app_domains`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppDomain {
    pub domain: String,
    pub app_id: String,
    /// NULL: the app's main container; else a compose service.
    pub service: Option<String>,
    /// Container port of `service` (NULL: the app's port).
    pub port: Option<i64>,
    /// "primary" | "alias" | "redirect"
    pub kind: String,
    /// For redirects: the domain to send visitors to.
    pub redirect_to: Option<String>,
    /// For redirects: 301 or 308.
    pub redirect_code: i64,
    pub created_at: String,
}

impl AppDomain {
    pub fn kind(&self) -> DomainKind {
        DomainKind::parse(&self.kind).unwrap_or(DomainKind::Alias)
    }
}

/// One domain of an app, as the API returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppDomainResponse {
    pub domain: String,
    pub kind: DomainKind,
    pub redirect_to: Option<String>,
    pub redirect_code: u16,
    pub service: Option<String>,
    pub port: Option<i64>,
    pub created_at: String,
    /// The domain is under the apps domain (the automatic address).
    pub automatic: bool,
    /// Set when the domain is on the same site as the dashboard.
    pub warning: Option<String>,
}

/// POST /api/apps/{id}/domains
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AddAppDomainRequest {
    pub domain: String,
    /// "primary" | "alias" (default; primary when the app has none) | "redirect"
    #[serde(default)]
    pub kind: Option<DomainKind>,
    /// Required for redirects: another domain of this app.
    #[serde(default)]
    pub redirect_to: Option<String>,
    /// 301 or 308 (default 308).
    #[serde(default)]
    pub redirect_code: Option<u16>,
    /// Compose apps: the service this domain routes to.
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub port: Option<i64>,
}

/// PATCH /api/apps/{id}/domains/{domain}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateAppDomainRequest {
    #[serde(default)]
    pub kind: Option<DomainKind>,
    #[serde(default)]
    pub redirect_to: Option<String>,
    #[serde(default)]
    pub redirect_code: Option<u16>,
}

/// Which of the two names a www/apex pair serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CanonicalHost {
    /// example.com serves, www.example.com redirects to it.
    Apex,
    /// www.example.com serves, example.com redirects to it.
    Www,
}

/// POST /api/apps/{id}/domains/www-pair: add `example.com` and
/// `www.example.com` in one go, one redirecting to the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WwwPairRequest {
    /// Either name of the pair (with or without `www.`).
    pub domain: String,
    pub canonical: CanonicalHost,
    /// Make the canonical name the app's primary domain (default true).
    #[serde(default)]
    pub primary: Option<bool>,
}

/// Split a www/apex pair: (apex, www) for either input.
pub fn www_pair(domain: &str) -> (String, String) {
    match domain.strip_prefix("www.") {
        Some(apex) => (apex.to_string(), domain.to_string()),
        None => (domain.to_string(), format!("www.{domain}")),
    }
}

// ── DNS providers ─────────────────────────────────────────────────────────────

/// A DNS hosting API Tond can publish `_acme-challenge` TXT records with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DnsProviderKind {
    Cloudflare,
    Hetzner,
    Digitalocean,
    Route53,
    /// Test only: Pebble's challtestsrv management API. Refused unless
    /// tond-server runs with TOND_DNS_TEST_PROVIDER=1 (the installer never
    /// sets it) or is a debug build.
    Challtestsrv,
}

impl DnsProviderKind {
    pub const PUBLIC: [DnsProviderKind; 4] = [
        DnsProviderKind::Cloudflare,
        DnsProviderKind::Hetzner,
        DnsProviderKind::Digitalocean,
        DnsProviderKind::Route53,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DnsProviderKind::Cloudflare => "cloudflare",
            DnsProviderKind::Hetzner => "hetzner",
            DnsProviderKind::Digitalocean => "digitalocean",
            DnsProviderKind::Route53 => "route53",
            DnsProviderKind::Challtestsrv => "challtestsrv",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cloudflare" => Some(DnsProviderKind::Cloudflare),
            "hetzner" => Some(DnsProviderKind::Hetzner),
            "digitalocean" => Some(DnsProviderKind::Digitalocean),
            "route53" => Some(DnsProviderKind::Route53),
            "challtestsrv" => Some(DnsProviderKind::Challtestsrv),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DnsProviderKind::Cloudflare => "Cloudflare",
            DnsProviderKind::Hetzner => "Hetzner DNS",
            DnsProviderKind::Digitalocean => "DigitalOcean",
            DnsProviderKind::Route53 => "Amazon Route 53",
            DnsProviderKind::Challtestsrv => "challtestsrv (test)",
        }
    }
}

/// Raw row from `dns_providers`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DnsProvider {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// JSON array of zone names, e.g. `["example.com"]`.
    pub zones: String,
    /// Non-secret settings (Route 53 access key id, challtestsrv URL), JSON.
    pub config: String,
    /// The API token / secret key, encrypted (secret_key.rs).
    pub token_enc: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
    pub last_error: Option<String>,
}

/// Non-secret provider settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DnsProviderConfig {
    /// Route 53: the IAM access key id (the secret key is the token).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    /// challtestsrv: management API URL (default http://127.0.0.1:8055).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A provider as the API returns it: never the token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DnsProviderResponse {
    pub id: String,
    pub name: String,
    pub kind: DnsProviderKind,
    pub zones: Vec<String>,
    pub config: DnsProviderConfig,
    /// A token is stored (always true; it is required).
    pub token_set: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
    pub last_error: Option<String>,
}

/// POST /api/dns-providers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateDnsProviderRequest {
    pub name: String,
    pub kind: DnsProviderKind,
    pub zones: Vec<String>,
    /// API token (Route 53: the secret access key).
    pub token: String,
    #[serde(default)]
    pub config: DnsProviderConfig,
}

/// PATCH /api/dns-providers/{id}: omitted fields are unchanged; `token` is
/// replaced only when sent (it is never returned).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateDnsProviderRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub zones: Option<Vec<String>>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub config: Option<DnsProviderConfig>,
}

/// Result of POST /api/dns-providers/{id}/test.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DnsProviderTestResult {
    pub ok: bool,
    /// One line per zone: found, or why not.
    pub zones: Vec<DnsZoneCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DnsZoneCheck {
    pub zone: String,
    pub ok: bool,
    pub message: String,
}

// ── Custom certificates ──────────────────────────────────────────────────────

/// POST /api/certs/custom
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UploadCertificateRequest {
    /// The domain the certificate serves (may be a wildcard `*.example.com`).
    pub domain: String,
    /// PEM: the leaf certificate first, then its intermediates.
    pub cert_pem: String,
    /// PEM private key of the leaf.
    pub key_pem: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_pair_works_from_either_name() {
        assert_eq!(
            www_pair("example.com"),
            ("example.com".into(), "www.example.com".into())
        );
        assert_eq!(
            www_pair("www.example.com"),
            ("example.com".into(), "www.example.com".into())
        );
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [DomainKind::Primary, DomainKind::Alias, DomainKind::Redirect] {
            assert_eq!(DomainKind::parse(kind.as_str()), Some(kind));
        }
        for kind in DnsProviderKind::PUBLIC {
            assert_eq!(DnsProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(DomainKind::parse("nope"), None);
    }
}
