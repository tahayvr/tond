// acme.rs — ACME HTTP-01 certificate provisioning via Let's Encrypt
//
// Handles the full ACME workflow so tond-proxy can obtain and renew real
// TLS certificates automatically, without any manual cert management.
//
// How it fits into the system:
//   1. When a user sets a custom domain (PUT /api/apps/{id}/domain),
//      tond-server writes a 'pending' cert row to the database.
//   2. The issuance loop (issuance.rs, every 30 s with per-domain backoff)
//      decides which domains need a cert or a renewal.
//   3. For each of those, provision_cert() runs the full ACME HTTP-01 flow:
//        a. Get/restore an ACME account (credentials persisted to disk as
//           acme_account.json so the same account is reused across restarts)
//        b. Create an ACME order for the domain
//        c. Place the HTTP-01 challenge token into ChallengeMap (shared
//           with run_http_server() which serves it at /.well-known/acme-challenge/)
//        d. Tell Let's Encrypt the challenge is ready and poll for validation
//        e. Finalize the order (auto-generates a CSR via rcgen)
//        f. Retrieve the signed cert and store it in CertStore (memory + disk)
//   4. After storage, TondTlsAccept immediately starts serving the real cert
//      for that domain on the next TLS handshake.
//
// ChallengeMap is an Arc<RwLock<HashMap<token, key_authorization>>> shared
// between this module and run_http_server(). Tokens are inserted before
// signaling readiness and remain for the duration of the ACME poll.
//
// TOND_ACME_STAGING=1 switches to the Let's Encrypt staging environment
// (for testing without hitting rate limits).
// TOND_ACME_DIRECTORY=<url> overrides the ACME directory entirely (takes
// precedence over TOND_ACME_STAGING). Used by the end-to-end tests to point
// the proxy at a local Pebble test CA.

//! ACME HTTP-01 certificate provisioning via Let's Encrypt.
//!
//! This module handles:
//! - ACME account creation and persistence
//! - HTTP-01 challenge token storage (shared with the proxy for serving)
//! - Certificate ordering, validation, and retrieval
//! - Automatic renewal of certificates nearing expiry

use anyhow::{Context, Result};
use instant_acme::{
    Account, AccountCredentials, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder,
    OrderStatus, RetryPolicy,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::certs::CertStore;

/// Shared state for pending ACME HTTP-01 challenges.
/// Maps token -> key_authorization for `/.well-known/acme-challenge/{token}`.
pub type ChallengeMap = Arc<RwLock<HashMap<String, String>>>;

/// Create a new empty challenge map.
pub fn new_challenge_map() -> ChallengeMap {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Path to the persisted ACME account credentials.
fn account_creds_path(data_dir: &Path) -> PathBuf {
    data_dir.join("acme_account.json")
}

pub(crate) fn should_renew(days_until_expiry: Option<i64>) -> bool {
    match days_until_expiry {
        Some(days) => days <= 30,
        None => true,
    }
}

/// The ACME directory to use: `TOND_ACME_DIRECTORY` if set, else Let's Encrypt
/// (staging or production).
fn acme_directory_url(staging: bool) -> String {
    directory_url_from(std::env::var("TOND_ACME_DIRECTORY").ok(), staging)
}

fn directory_url_from(override_url: Option<String>, staging: bool) -> String {
    match override_url.filter(|url| !url.trim().is_empty()) {
        Some(url) => url,
        None if staging => LetsEncrypt::Staging.url().to_string(),
        None => LetsEncrypt::Production.url().to_string(),
    }
}

/// Get or create an ACME account.
async fn get_or_create_account(data_dir: &Path, staging: bool) -> Result<Account> {
    let creds_path = account_creds_path(data_dir);
    let directory_url = acme_directory_url(staging);

    if creds_path.exists() {
        // Restore existing account
        let creds_json = tokio::fs::read_to_string(&creds_path)
            .await
            .context("reading ACME account credentials")?;
        let creds: AccountCredentials =
            serde_json::from_str(&creds_json).context("parsing ACME account credentials")?;

        let account = Account::builder()?
            .from_credentials(creds)
            .await
            .context("restoring ACME account")?;

        tracing::debug!("Restored ACME account from {}", creds_path.display());
        Ok(account)
    } else {
        // Create new account
        let new_account = NewAccount {
            contact: &[],
            terms_of_service_agreed: true,
            only_return_existing: false,
        };

        let (account, creds) = Account::builder()?
            .create(&new_account, directory_url, None)
            .await
            .context("creating ACME account")?;

        // Persist credentials
        let creds_json =
            serde_json::to_string_pretty(&creds).context("serializing ACME credentials")?;
        tokio::fs::write(&creds_path, creds_json)
            .await
            .context("writing ACME credentials")?;

        tracing::info!(
            "Created new ACME account, saved to {}",
            creds_path.display()
        );
        Ok(account)
    }
}

/// Provision a certificate for the given domain via ACME HTTP-01.
///
/// This function:
/// 1. Creates/restores an ACME account
/// 2. Creates a new order for the domain
/// 3. Sets up the HTTP-01 challenge token in the challenge map
/// 4. Waits for the challenge to be validated
/// 5. Finalizes the order and retrieves the certificate
/// 6. Stores the certificate in the cert store
pub async fn provision_cert(
    domain: &str,
    data_dir: &Path,
    cert_store: &CertStore,
    challenges: &ChallengeMap,
    staging: bool,
) -> Result<()> {
    let mut tokens = Vec::new();
    let result = provision_cert_inner(
        domain,
        data_dir,
        cert_store,
        challenges,
        staging,
        &mut tokens,
    )
    .await;
    // Challenge responses are only needed while the order is validated.
    let mut map = challenges.write().await;
    for token in tokens {
        map.remove(&token);
    }
    result
}

async fn provision_cert_inner(
    domain: &str,
    data_dir: &Path,
    cert_store: &CertStore,
    challenges: &ChallengeMap,
    staging: bool,
    tokens: &mut Vec<String>,
) -> Result<()> {
    tracing::debug!("Starting ACME cert provisioning for {}", domain);

    let account = get_or_create_account(data_dir, staging).await?;

    // Create order
    let identifiers = [Identifier::Dns(domain.to_string())];
    let new_order = NewOrder::new(&identifiers);
    let mut order = account
        .new_order(&new_order)
        .await
        .context("creating ACME order")?;

    tracing::debug!("Order created for {}: {}", domain, order.url());

    // Process authorizations
    let mut authz_stream = order.authorizations();
    while let Some(authz_result) = authz_stream.next().await {
        let mut authz = authz_result.context("fetching authorization")?;

        // Get the HTTP-01 challenge
        let mut challenge = authz
            .challenge(ChallengeType::Http01)
            .context("no HTTP-01 challenge available")?;

        let token = challenge.token.clone();
        let key_auth = challenge.key_authorization();
        let key_auth_str = key_auth.as_str().to_string();

        tracing::debug!(
            "HTTP-01 challenge for {}: token={}, serving key auth",
            domain,
            token
        );

        // Store the challenge response so the proxy can serve it
        {
            let mut map = challenges.write().await;
            map.insert(token.clone(), key_auth_str);
        }
        tokens.push(token.clone());

        // Tell the ACME server we're ready
        challenge
            .set_ready()
            .await
            .context("signaling challenge ready")?;

        tracing::debug!("Challenge marked ready for {}", domain);
    }

    // Poll until order is ready
    let status = order
        .poll_ready(&RetryPolicy::new())
        .await
        .context("polling order ready")?;

    if status != OrderStatus::Ready {
        anyhow::bail!(
            "Order for {} reached unexpected status: {:?}",
            domain,
            status
        );
    }

    tracing::debug!("Order ready for {}, finalizing...", domain);

    // Finalize with auto-generated CSR (uses rcgen internally)
    let private_key_pem = order.finalize().await.context("finalizing order")?;

    // Poll for certificate
    let cert_pem = order
        .poll_certificate(&RetryPolicy::new())
        .await
        .context("polling certificate")?;

    tracing::debug!("Certificate obtained for {}", domain);

    // Store cert in the cert store (memory + disk)
    cert_store
        .store(domain, cert_pem.as_bytes(), private_key_pem.as_bytes())
        .await
        .context("storing certificate")?;

    tracing::info!("ACME provisioning complete for {}", domain);
    Ok(())
}

pub(crate) fn certificate_timestamps(cert_pem: &[u8]) -> Result<(String, String)> {
    let cert = openssl::x509::X509::from_pem(cert_pem).context("parsing issued certificate PEM")?;
    let issued_at = cert.not_before().to_string();
    let expires_at = cert.not_after().to_string();
    Ok((issued_at, expires_at))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn acme_http_client_builds_with_both_rustls_backends_compiled_in() {
        // Workspace builds unify features, compiling both rustls backends in (lettre →
        // ring, instant-acme → aws-lc-rs). Without an explicit provider, building the
        // ACME client panics — this crashed tond-proxy on its first certificate order.
        tond_common::install_crypto_provider();

        assert!(Account::builder().is_ok());
    }

    #[test]
    fn directory_url_prefers_override_then_staging_then_production() {
        assert_eq!(
            directory_url_from(Some("https://localhost:14000/dir".to_string()), true),
            "https://localhost:14000/dir",
            "TOND_ACME_DIRECTORY should take precedence over staging"
        );
        assert_eq!(
            directory_url_from(Some("  ".to_string()), true),
            LetsEncrypt::Staging.url(),
            "a blank override should be ignored"
        );
        assert_eq!(
            directory_url_from(None, false),
            LetsEncrypt::Production.url()
        );
    }

    #[test]
    fn new_challenge_map_starts_empty_and_is_shareable() {
        let map = new_challenge_map();
        assert!(
            map.try_read()
                .unwrap_or_else(|_| panic!(
                    "challenge map read lock should be available in a synchronous test"
                ))
                .is_empty(),
            "new_challenge_map should initialize with no pending ACME challenge tokens"
        );
    }

    #[test]
    fn account_creds_path_points_to_acme_account_json() {
        let dir = TempDir::new().unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        assert_eq!(
            account_creds_path(dir.path()),
            dir.path().join("acme_account.json"),
            "account_creds_path should store ACME account credentials in acme_account.json under the proxy data directory"
        );
    }

    #[test]
    fn should_renew_marks_missing_and_soon_expiring_certs_for_renewal() {
        assert!(
            should_renew(None),
            "should_renew should re-provision certificates when expiry cannot be determined"
        );
        assert!(
            should_renew(Some(30)),
            "should_renew should renew certificates expiring within 30 days"
        );
        assert!(
            !should_renew(Some(31)),
            "should_renew should skip renewal for certificates with more than 30 days remaining"
        );
    }

    #[test]
    fn certificate_timestamps_extract_not_before_and_not_after() {
        let (_pair, cert_pem, _key_pem) =
            crate::certs::generate_self_signed_for_domain("dash.example.com")
                .unwrap_or_else(|err| panic!("failed to generate self-signed cert fixture: {err}"));

        let (issued_at, expires_at) = certificate_timestamps(&cert_pem)
            .unwrap_or_else(|err| panic!("failed to extract certificate timestamps: {err}"));

        let cert = openssl::x509::X509::from_pem(&cert_pem)
            .unwrap_or_else(|err| panic!("failed to parse generated certificate PEM: {err}"));

        assert!(
            cert.not_before().to_string() == issued_at,
            "issued_at should match the parsed certificate's not_before value"
        );
        assert!(
            cert.not_after().to_string() == expires_at,
            "expires_at should match the parsed certificate's not_after value"
        );
    }
}
