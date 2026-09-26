// certs.rs — TLS certificate store for tond-proxy
//
// Manages the full cert lifecycle: loading from disk on startup, serving
// during TLS handshakes, and persisting new certs after ACME provisioning.
//
// How it fits into the system:
//   - CertStore is created in main() and shared (Arc clone) between:
//       TondTlsAccept — calls get() on every TLS handshake for SNI lookup
//       acme module   — calls store() after a successful ACME order
//   - Certs live on disk at: <TOND_DATA_DIR>/certs/<domain>/cert.pem|key.pem
//   - On startup, load_all_from_disk() populates the in-memory cache from
//     whatever was previously provisioned, so certs survive restarts.
//   - For domains with no cert yet, get() returns the self-signed fallback
//     (tond.local) so TLS handshakes still complete (with a browser warning)
//     while ACME provisioning is in progress.
//
// CertPair wraps the parsed OpenSSL objects (X509 + PKey<Private>) so the
// TLS layer can inject them directly without re-parsing on every handshake.

//! Certificate manager: in-memory cache, disk persistence, self-signed fallback.
//!
//! Certificates are stored on disk at `<certs_dir>/<domain>/cert.pem` and
//! `<certs_dir>/<domain>/key.pem`, and cached in memory as parsed OpenSSL
//! objects for fast SNI lookups during TLS handshakes.

use anyhow::{Context, Result};
use openssl::asn1::Asn1Time;
use openssl::pkey::{PKey, Private};
use openssl::x509::X509;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// A parsed certificate + private key pair ready for TLS handshakes.
#[derive(Clone)]
pub struct CertPair {
    pub cert: X509,
    /// Intermediates sent after the leaf. Without them clients that don't
    /// fetch missing intermediates themselves (curl, most HTTP libraries,
    /// Android) reject the certificate.
    pub chain: Vec<X509>,
    pub key: PKey<Private>,
}

impl CertPair {
    /// Parse a PEM bundle (leaf first, then intermediates) and its key.
    fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self> {
        let mut certs = X509::stack_from_pem(cert_pem)
            .context("parsing cert PEM")?
            .into_iter();
        let cert = certs.next().context("cert PEM contains no certificate")?;
        let key = PKey::private_key_from_pem(key_pem).context("parsing key PEM")?;
        Ok(Self {
            cert,
            chain: certs.collect(),
            key,
        })
    }
}

/// Thread-safe certificate store with in-memory cache and disk backing.
#[derive(Clone)]
pub struct CertStore {
    /// domain -> CertPair
    cache: Arc<RwLock<HashMap<String, CertPair>>>,
    /// Base directory for cert storage (e.g. /opt/tond/data/certs)
    certs_dir: PathBuf,
    /// Fallback self-signed cert for unknown domains
    fallback: Arc<CertPair>,
}

impl CertStore {
    /// Create a new CertStore, generating a self-signed fallback cert and
    /// loading any existing certs from disk.
    pub async fn new(certs_dir: &Path) -> Result<Self> {
        tokio::fs::create_dir_all(certs_dir)
            .await
            .with_context(|| format!("creating certs dir: {}", certs_dir.display()))?;

        let fallback = generate_self_signed("tond.local")?;
        tracing::info!("Generated self-signed fallback certificate for unknown domains");

        let store = Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            certs_dir: certs_dir.to_path_buf(),
            fallback: Arc::new(fallback),
        };

        // Load any existing certs from disk
        store.load_all_from_disk().await?;

        Ok(store)
    }

    /// Get the cert pair for a domain, falling back to the self-signed cert.
    pub async fn get(&self, domain: &str) -> CertPair {
        let cache = self.cache.read().await;
        cache
            .get(domain)
            .cloned()
            .unwrap_or_else(|| (*self.fallback).clone())
    }

    /// Check if we have a cert (non-fallback) for this domain.
    pub async fn has_cert(&self, domain: &str) -> bool {
        let cache = self.cache.read().await;
        cache.contains_key(domain)
    }

    /// Return the number of days until the cert for `domain` expires.
    /// Returns `None` if no cert is cached for the domain.
    /// Returns `0` if the cert has already expired.
    pub async fn days_until_expiry(&self, domain: &str) -> Option<i32> {
        let cache = self.cache.read().await;
        let pair = cache.get(domain)?;

        let now = Asn1Time::days_from_now(0).ok()?;
        let diff = pair.cert.not_after().diff(&now).ok()?;

        // diff.days is negative when not_after is in the future (now < not_after),
        // positive when the cert has already expired.
        let days_remaining = -diff.days;
        Some(days_remaining.max(0))
    }

    /// Store a cert pair both in memory and on disk.
    pub async fn store(&self, domain: &str, cert_pem: &[u8], key_pem: &[u8]) -> Result<()> {
        // Parse to validate
        let pair = CertPair::from_pem(cert_pem, key_pem)?;

        // Write to disk
        let domain_dir = self.certs_dir.join(domain);
        tokio::fs::create_dir_all(&domain_dir).await?;

        let cert_path = domain_dir.join("cert.pem");
        let key_path = domain_dir.join("key.pem");

        tokio::fs::write(&cert_path, cert_pem).await?;
        write_private(&key_path, key_pem).await?;

        tracing::info!("Stored certificate for {} on disk", domain);

        // Update in-memory cache
        let mut cache = self.cache.write().await;
        cache.insert(domain.to_string(), pair);

        Ok(())
    }

    /// Get the filesystem paths where a domain's cert would be stored.
    /// Used by tond-server when recording cert paths in the database.
    #[allow(dead_code)]
    pub fn cert_paths(&self, domain: &str) -> (PathBuf, PathBuf) {
        let domain_dir = self.certs_dir.join(domain);
        (domain_dir.join("cert.pem"), domain_dir.join("key.pem"))
    }

    /// Load all certificates from the certs directory into memory.
    async fn load_all_from_disk(&self) -> Result<()> {
        let mut entries = match tokio::fs::read_dir(&self.certs_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        let mut count = 0u32;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let domain = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            let cert_path = path.join("cert.pem");
            let key_path = path.join("key.pem");

            if !cert_path.exists() || !key_path.exists() {
                tracing::warn!("Incomplete cert for domain {}, skipping", domain);
                continue;
            }

            match load_cert_from_disk(&cert_path, &key_path) {
                Ok(pair) => {
                    let mut cache = self.cache.write().await;
                    cache.insert(domain.clone(), pair);
                    count += 1;
                    tracing::debug!("Loaded cert for {} from disk", domain);
                }
                Err(e) => {
                    tracing::warn!("Failed to load cert for {}: {}", domain, e);
                }
            }
        }

        if count > 0 {
            tracing::info!("Loaded {} certificates from disk", count);
        }

        Ok(())
    }

    /// List all cached domains.
    #[cfg(test)]
    pub async fn domains(&self) -> Vec<String> {
        let cache = self.cache.read().await;
        cache.keys().cloned().collect()
    }
}

/// Load a cert + key pair from PEM files on disk.
fn load_cert_from_disk(cert_path: &Path, key_path: &Path) -> Result<CertPair> {
    let cert_pem =
        std::fs::read(cert_path).with_context(|| format!("reading {}", cert_path.display()))?;
    let key_pem =
        std::fs::read(key_path).with_context(|| format!("reading {}", key_path.display()))?;

    CertPair::from_pem(&cert_pem, &key_pem)
}

/// Write a private key readable by root only.
async fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(contents).await?;
    // Tighten keys written by older versions too; mode() only applies on create.
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(())
}

/// Generate a self-signed certificate for the given domain using rcgen.
fn generate_self_signed(domain: &str) -> Result<CertPair> {
    let certified_key = rcgen::generate_simple_self_signed(vec![domain.to_string()])
        .context("generating self-signed cert")?;

    let cert_pem = certified_key.cert.pem();
    let key_pem = certified_key.signing_key.serialize_pem();

    let cert = X509::from_pem(cert_pem.as_bytes()).context("parsing generated cert")?;
    let key = PKey::private_key_from_pem(key_pem.as_bytes()).context("parsing generated key")?;

    Ok(CertPair {
        cert,
        chain: Vec::new(),
        key,
    })
}

/// Generate a self-signed certificate and save it to disk.
/// Returns the CertPair and the PEM bytes (cert, key).
/// Used by tond-server for initial domain setup before ACME provisioning.
#[allow(dead_code)]
pub fn generate_self_signed_for_domain(domain: &str) -> Result<(CertPair, Vec<u8>, Vec<u8>)> {
    let certified_key = rcgen::generate_simple_self_signed(vec![domain.to_string()])
        .context("generating self-signed cert")?;

    let cert_pem = certified_key.cert.pem().into_bytes();
    let key_pem = certified_key.signing_key.serialize_pem().into_bytes();

    let cert = X509::from_pem(&cert_pem).context("parsing generated cert")?;
    let key = PKey::private_key_from_pem(&key_pem).context("parsing generated key")?;

    Ok((
        CertPair {
            cert,
            chain: Vec::new(),
            key,
        },
        cert_pem,
        key_pem,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tempfile::TempDir;

    use super::*;

    fn temp_dir() -> TempDir {
        TempDir::new().unwrap_or_else(|err| panic!("failed to create temporary directory: {err}"))
    }

    #[tokio::test]
    async fn new_creates_directory_and_returns_fallback_for_unknown_domains() {
        let dir = temp_dir();
        let certs_dir = dir.path().join("certs");

        let store = CertStore::new(&certs_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create CertStore: {err}"));

        assert!(
            certs_dir.exists(),
            "CertStore::new should create the certificate directory if it does not exist"
        );
        assert!(
            !store.has_cert("unknown.example.com").await,
            "unknown domains should not appear as cached certificates"
        );

        let pair = store.get("unknown.example.com").await;
        assert!(
            pair.cert.to_der().ok() == store.fallback.cert.to_der().ok(),
            "unknown domains should receive the self-signed fallback certificate"
        );
    }

    #[tokio::test]
    async fn store_persists_cert_to_disk_and_marks_domain_as_cached() {
        let dir = temp_dir();
        let certs_dir = dir.path().join("certs");
        let store = CertStore::new(&certs_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create CertStore: {err}"));
        let (_, cert_pem, key_pem) = generate_self_signed_for_domain("demo.example.com")
            .unwrap_or_else(|err| panic!("failed to generate self-signed cert fixture: {err}"));

        store
            .store("demo.example.com", &cert_pem, &key_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to store certificate: {err}"));

        assert!(
            store.has_cert("demo.example.com").await,
            "stored domains should be present in the in-memory cache"
        );

        let (cert_path, key_path) = store.cert_paths("demo.example.com");
        assert!(cert_path.exists(), "store should write cert.pem to disk");
        assert!(key_path.exists(), "store should write key.pem to disk");
        assert_eq!(
            tokio::fs::read(&cert_path)
                .await
                .unwrap_or_else(|err| panic!("failed to read stored cert.pem: {err}")),
            cert_pem,
            "stored cert.pem contents should match the PEM bytes that were provided"
        );
        assert_eq!(
            tokio::fs::read(&key_path)
                .await
                .unwrap_or_else(|err| panic!("failed to read stored key.pem: {err}")),
            key_pem,
            "stored key.pem contents should match the PEM bytes that were provided"
        );
    }

    #[tokio::test]
    async fn store_keeps_intermediates_and_writes_key_root_only() {
        let dir = temp_dir();
        let store = CertStore::new(&dir.path().join("certs"))
            .await
            .unwrap_or_else(|err| panic!("failed to create CertStore: {err}"));
        // ACME returns the leaf followed by its intermediates in one PEM.
        let (_, leaf_pem, key_pem) = generate_self_signed_for_domain("demo.example.com")
            .unwrap_or_else(|err| panic!("failed to generate leaf fixture: {err}"));
        let (_, intermediate_pem, _) = generate_self_signed_for_domain("Intermediate CA")
            .unwrap_or_else(|err| panic!("failed to generate intermediate fixture: {err}"));
        let bundle = [leaf_pem.as_slice(), intermediate_pem.as_slice()].concat();

        store
            .store("demo.example.com", &bundle, &key_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to store certificate: {err}"));

        let pair = store.get("demo.example.com").await;
        assert_eq!(
            pair.chain.len(),
            1,
            "the intermediate must be served after the leaf"
        );
        let expected = X509::from_pem(&intermediate_pem)
            .unwrap_or_else(|err| panic!("failed to parse intermediate: {err}"));
        assert!(pair.chain[0].to_der().ok() == expected.to_der().ok());

        let (_, key_path) = store.cert_paths("demo.example.com");
        let mode = std::fs::metadata(&key_path)
            .unwrap_or_else(|err| panic!("failed to stat key.pem: {err}"))
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "private keys must be readable by root only"
        );

        // The chain survives a restart.
        let reloaded = CertStore::new(&dir.path().join("certs"))
            .await
            .unwrap_or_else(|err| panic!("failed to reload CertStore: {err}"));
        assert_eq!(reloaded.get("demo.example.com").await.chain.len(), 1);
    }

    #[tokio::test]
    async fn store_rejects_invalid_cert_or_key_pem() {
        let dir = temp_dir();
        let certs_dir = dir.path().join("certs");
        let store = CertStore::new(&certs_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create CertStore: {err}"));
        let (_, cert_pem, key_pem) = generate_self_signed_for_domain("demo.example.com")
            .unwrap_or_else(|err| panic!("failed to generate self-signed cert fixture: {err}"));

        let bad_cert = store.store("demo.example.com", b"not pem", &key_pem).await;
        assert!(
            bad_cert.is_err(),
            "invalid certificate PEM should be rejected"
        );

        let bad_key = store.store("demo.example.com", &cert_pem, b"not pem").await;
        assert!(
            bad_key.is_err(),
            "invalid private key PEM should be rejected"
        );
    }

    #[tokio::test]
    async fn new_loads_valid_certs_and_skips_incomplete_or_invalid_entries() {
        let dir = temp_dir();
        let certs_dir = dir.path().join("certs");
        tokio::fs::create_dir_all(&certs_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create certs directory: {err}"));

        let valid_dir = certs_dir.join("valid.example.com");
        tokio::fs::create_dir_all(&valid_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create valid domain dir: {err}"));
        let (_, cert_pem, key_pem) = generate_self_signed_for_domain("valid.example.com")
            .unwrap_or_else(|err| panic!("failed to generate valid cert fixture: {err}"));
        tokio::fs::write(valid_dir.join("cert.pem"), &cert_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to write valid cert.pem: {err}"));
        tokio::fs::write(valid_dir.join("key.pem"), &key_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to write valid key.pem: {err}"));

        let incomplete_dir = certs_dir.join("incomplete.example.com");
        tokio::fs::create_dir_all(&incomplete_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create incomplete domain dir: {err}"));
        tokio::fs::write(incomplete_dir.join("cert.pem"), &cert_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to write incomplete cert.pem: {err}"));

        let invalid_dir = certs_dir.join("invalid.example.com");
        tokio::fs::create_dir_all(&invalid_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create invalid domain dir: {err}"));
        tokio::fs::write(invalid_dir.join("cert.pem"), b"not pem")
            .await
            .unwrap_or_else(|err| panic!("failed to write invalid cert.pem: {err}"));
        tokio::fs::write(invalid_dir.join("key.pem"), b"not pem")
            .await
            .unwrap_or_else(|err| panic!("failed to write invalid key.pem: {err}"));

        let store = CertStore::new(&certs_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create CertStore: {err}"));

        assert!(
            store.has_cert("valid.example.com").await,
            "CertStore::new should load valid on-disk certificates into the cache"
        );
        assert!(
            !store.has_cert("incomplete.example.com").await,
            "CertStore::new should skip incomplete certificate directories"
        );
        assert!(
            !store.has_cert("invalid.example.com").await,
            "CertStore::new should skip invalid certificate directories without failing startup"
        );
    }

    #[tokio::test]
    async fn domains_lists_cached_domains_and_days_until_expiry_is_available_for_stored_cert() {
        let dir = temp_dir();
        let certs_dir = dir.path().join("certs");
        let store = CertStore::new(&certs_dir)
            .await
            .unwrap_or_else(|err| panic!("failed to create CertStore: {err}"));
        let (_, cert_pem, key_pem) = generate_self_signed_for_domain("one.example.com")
            .unwrap_or_else(|err| panic!("failed to generate cert fixture: {err}"));
        store
            .store("one.example.com", &cert_pem, &key_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to store first certificate: {err}"));
        let (_, cert_pem, key_pem) = generate_self_signed_for_domain("two.example.com")
            .unwrap_or_else(|err| panic!("failed to generate cert fixture: {err}"));
        store
            .store("two.example.com", &cert_pem, &key_pem)
            .await
            .unwrap_or_else(|err| panic!("failed to store second certificate: {err}"));

        let domains: HashSet<String> = store.domains().await.into_iter().collect();

        assert_eq!(
            domains,
            HashSet::from(["one.example.com".to_string(), "two.example.com".to_string()]),
            "domains should return the set of cached domain names"
        );

        let days = store.days_until_expiry("one.example.com").await;
        assert!(
            days.is_some_and(|remaining| remaining >= 0),
            "days_until_expiry should return a non-negative remaining lifetime for stored certs"
        );
        assert_eq!(
            store.days_until_expiry("missing.example.com").await,
            None,
            "days_until_expiry should return None for domains without a cached certificate"
        );
    }

    #[test]
    fn generate_self_signed_for_domain_returns_valid_pem_material() {
        let (pair, cert_pem, key_pem) = generate_self_signed_for_domain("demo.example.com")
            .unwrap_or_else(|err| panic!("failed to generate self-signed domain cert: {err}"));

        assert!(
            String::from_utf8(cert_pem.clone())
                .unwrap_or_else(|err| panic!("generated cert PEM was not UTF-8: {err}"))
                .contains("BEGIN CERTIFICATE"),
            "generate_self_signed_for_domain should return certificate PEM bytes"
        );
        assert!(
            String::from_utf8(key_pem.clone())
                .unwrap_or_else(|err| panic!("generated key PEM was not UTF-8: {err}"))
                .contains("BEGIN PRIVATE KEY"),
            "generate_self_signed_for_domain should return private key PEM bytes"
        );
        assert!(
            pair.cert
                .subject_alt_names()
                .into_iter()
                .flatten()
                .any(|name| name.dnsname() == Some("demo.example.com")),
            "generate_self_signed_for_domain should include the requested domain in the certificate SAN"
        );
        assert!(
            X509::from_pem(&cert_pem)
                .ok()
                .and_then(|cert| cert.subject_alt_names())
                .into_iter()
                .flatten()
                .any(|name| name.dnsname() == Some("demo.example.com")),
            "generated certificate PEM should parse back to a certificate containing the requested SAN"
        );
        assert!(
            PKey::private_key_from_pem(&key_pem).is_ok(),
            "generated private key PEM should parse successfully"
        );
    }
}
