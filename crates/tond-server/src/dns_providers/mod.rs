// dns_providers — DNS APIs for ACME DNS-01 challenges
//
// Why tond-server and not tond-proxy: DNS API tokens can change any record in
// a zone, so they must never be readable by the unprivileged, internet-facing
// proxy. tond-proxy keeps running every ACME order (one account, one backoff,
// one certificate store); when an order needs DNS-01 it sends `dns_txt` over
// the control socket, tond-server publishes the record with the provider's
// API and answers once the record has had time to propagate, and the proxy
// asks for the record to be cleared when the order is done
// (tond_common::proxy_control). Only a key-authorization digest crosses the
// socket — a value that is published in public DNS anyway.
//
// A compromised proxy could ask for challenge records to get certificates
// for other names in a configured zone, so tond-server only publishes
// `_acme-challenge.<d>` where <d> is a domain Tond serves: the apps, preview
// or dashboard domain (wildcards use the base name) or an app domain.
//
// Tokens are stored encrypted (secret_key::Purpose::DnsProviderToken) and
// are write-only in the API. The test-only `challtestsrv` kind (Pebble's
// challenge server, used by the e2e test as a fake provider) is refused
// unless TOND_DNS_TEST_PROVIDER=1 or in debug builds; the installer never
// sets it.
//
// Propagation: after publishing, tond-server waits before answering so the
// CA's resolvers see the record — 30 s (Route 53: 60 s; challtestsrv: none),
// or TOND_DNS_PROPAGATION_SECS.

pub mod clients;
pub mod sigv4;

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use sqlx::SqlitePool;
use tond_common::dns::{challenge_domain, zone_for};
use tond_common::proxy_control::DnsTxtAction;
use tond_common::{DnsProvider, DnsProviderConfig, DnsProviderKind, DnsProviderResponse};

use crate::domains::normalize_domain;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;
use clients::{Client, Reqwest};

/// The test-only provider kind may be used.
pub fn test_provider_enabled() -> bool {
    cfg!(debug_assertions)
        || std::env::var("TOND_DNS_TEST_PROVIDER").is_ok_and(|v| v == "1" || v == "true")
}

/// Kinds that may be configured on this server.
pub fn kind_allowed(kind: DnsProviderKind) -> bool {
    kind != DnsProviderKind::Challtestsrv || test_provider_enabled()
}

pub fn parse_zones(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Normalize and validate a zone list.
pub fn clean_zones(zones: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for z in zones {
        let z = normalize_domain(z.trim_start_matches("*."));
        if z.is_empty() {
            continue;
        }
        if !crate::domains::is_valid_hostname(&z) {
            return Err(format!(
                "'{z}' isn't a valid zone name (expected something like example.com)"
            ));
        }
        if !out.contains(&z) {
            out.push(z);
        }
    }
    if out.is_empty() {
        return Err(
            "List at least one zone (the domain the provider hosts, e.g. example.com)".into(),
        );
    }
    Ok(out)
}

pub fn to_response(p: DnsProvider) -> DnsProviderResponse {
    DnsProviderResponse {
        kind: DnsProviderKind::parse(&p.kind).unwrap_or(DnsProviderKind::Cloudflare),
        zones: parse_zones(&p.zones),
        config: serde_json::from_str(&p.config).unwrap_or_default(),
        token_set: !p.token_enc.is_empty(),
        id: p.id,
        name: p.name,
        created_at: p.created_at,
        updated_at: p.updated_at,
        last_used_at: p.last_used_at,
        last_error: p.last_error,
    }
}

pub async fn list(db: &SqlitePool) -> Result<Vec<DnsProvider>, sqlx::Error> {
    sqlx::query_as::<_, DnsProvider>("SELECT * FROM dns_providers ORDER BY name, created_at")
        .fetch_all(db)
        .await
}

/// The provider (and zone) responsible for `domain`: the longest zone match.
pub fn provider_for<'a>(
    providers: &'a [DnsProvider],
    domain: &str,
) -> Option<(&'a DnsProvider, String)> {
    providers
        .iter()
        .filter(|p| DnsProviderKind::parse(&p.kind).is_some_and(kind_allowed))
        .filter_map(|p| {
            let zones = parse_zones(&p.zones);
            zone_for(domain, zones.iter().map(String::as_str)).map(|z| (p, z.to_string()))
        })
        .max_by_key(|(_, z)| z.len())
}

/// A client with the decrypted token.
pub fn client(p: &DnsProvider) -> Result<Client> {
    let kind =
        DnsProviderKind::parse(&p.kind).with_context(|| format!("unknown kind {}", p.kind))?;
    if !kind_allowed(kind) {
        anyhow::bail!(
            "the test-only {} provider is disabled on this server",
            kind.as_str()
        );
    }
    let token = secret_key::decrypt(Purpose::DnsProviderToken, &p.token_enc)
        .context("decrypting the DNS provider token")?;
    let config: DnsProviderConfig = serde_json::from_str(&p.config).unwrap_or_default();
    Ok(Client {
        kind,
        token,
        config,
    })
}

fn propagation_delay(kind: DnsProviderKind) -> Duration {
    if let Some(secs) = std::env::var("TOND_DNS_PROPAGATION_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return Duration::from_secs(secs.min(600));
    }
    Duration::from_secs(match kind {
        DnsProviderKind::Challtestsrv => 0,
        DnsProviderKind::Route53 => 60,
        _ => 30,
    })
}

/// Tond serves `domain` (or it is the base of a wildcard Tond orders), so a
/// challenge record for it may be published.
pub async fn serves(state: &AppState, domain: &str) -> Result<bool, sqlx::Error> {
    {
        let s = state.settings.read().await;
        let bases = [&s.apps_domain, &s.preview_domain, &s.dashboard_domain];
        if bases.iter().any(|b| !b.is_empty() && b.as_str() == domain) {
            return Ok(true);
        }
    }
    let n: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM app_domains WHERE domain = ?1)
              + (SELECT COUNT(*) FROM preview_deployments WHERE subdomain = ?1)",
    )
    .bind(domain)
    .fetch_one(&state.db)
    .await?;
    Ok(n > 0)
}

async fn record_use(db: &SqlitePool, id: &str, error: Option<&str>) {
    let _ = sqlx::query("UPDATE dns_providers SET last_used_at = ?, last_error = ? WHERE id = ?")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(error)
        .bind(id)
        .execute(db)
        .await;
}

/// Handle a `dns_txt` message from tond-proxy: publish or clear the record.
pub async fn handle_txt(
    state: &AppState,
    action: DnsTxtAction,
    name: &str,
    value: &str,
) -> Result<()> {
    let name = normalize_domain(name);
    let domain = challenge_domain(&name)
        .ok_or_else(|| anyhow!("{name} is not an ACME challenge record"))?
        .to_string();
    if !serves(state, &domain).await? {
        anyhow::bail!("refusing to publish a challenge for {domain}: Tond does not serve it");
    }
    let providers = list(&state.db).await?;
    let (provider, zone) = provider_for(&providers, &domain)
        .ok_or_else(|| anyhow!("no DNS provider is configured for {domain}"))?;
    let client = client(provider)?;
    let result = match action {
        DnsTxtAction::Set => client.set_txt(&Reqwest, &zone, &name, value).await,
        DnsTxtAction::Clear => client.clear_txt(&Reqwest, &zone, &name, value).await,
    };
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    record_use(&state.db, &provider.id, error.as_deref()).await;
    result.with_context(|| format!("{} ({})", provider.name, client.kind.label()))?;
    if action == DnsTxtAction::Set {
        let delay = propagation_delay(client.kind);
        tracing::info!(
            "[dns] published {name} with {}; waiting {}s for it to propagate",
            provider.name,
            delay.as_secs()
        );
        tokio::time::sleep(delay).await;
    } else {
        tracing::debug!("[dns] cleared {name} with {}", provider.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, kind: &str, zones: &[&str]) -> DnsProvider {
        DnsProvider {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            zones: serde_json::to_string(zones).unwrap_or_default(),
            config: "{}".into(),
            token_enc: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            last_used_at: None,
            last_error: None,
        }
    }

    #[test]
    fn the_most_specific_zone_wins() {
        let providers = vec![
            provider("cf", "cloudflare", &["example.com"]),
            provider("r53", "route53", &["apps.example.com", "other.org"]),
        ];
        let pick = |d: &str| provider_for(&providers, d).map(|(p, z)| (p.id.clone(), z));
        assert_eq!(
            pick("a.example.com"),
            Some(("cf".into(), "example.com".into()))
        );
        assert_eq!(
            pick("*.apps.example.com"),
            Some(("r53".into(), "apps.example.com".into()))
        );
        assert_eq!(
            pick("x.other.org"),
            Some(("r53".into(), "other.org".into()))
        );
        assert_eq!(pick("example.net"), None);
    }

    #[test]
    fn zones_are_normalized_and_required() {
        assert_eq!(
            clean_zones(&[
                "Example.COM.".into(),
                "*.apps.test".into(),
                "example.com".into()
            ]),
            Ok(vec!["example.com".to_string(), "apps.test".to_string()])
        );
        assert!(clean_zones(&[]).is_err());
        assert!(clean_zones(&["not a zone".into()]).is_err());
    }

    #[test]
    fn the_test_provider_is_available_in_debug_builds_only_by_default() {
        assert!(kind_allowed(DnsProviderKind::Cloudflare));
        assert_eq!(
            kind_allowed(DnsProviderKind::Challtestsrv),
            test_provider_enabled()
        );
        assert!(cfg!(debug_assertions) == test_provider_enabled() || test_provider_enabled());
    }
}
