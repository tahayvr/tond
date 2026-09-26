// certificates.rs — TLS certificate tracking types

use serde::{Deserialize, Serialize};

// ── Certificate (TLS cert tracking) ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Certificate {
    pub id: String,
    pub domain: String,
    /// "acme" | "self_signed"
    pub cert_type: String,
    pub cert_path: String,
    pub key_path: String,
    pub issued_at: String,
    pub expires_at: String,
    /// "active" | "pending" | "expired" | "failed"
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// Why the last issuance attempt failed (or was skipped), reported by
    /// tond-proxy; NULL after a success.
    pub last_error: Option<String>,
    /// RFC 3339 time of the last issuance attempt.
    pub last_attempt_at: Option<String>,
    /// RFC 3339 time before which tond-proxy will not try again (backoff).
    pub next_retry_at: Option<String>,
    /// Consecutive failed attempts.
    pub failure_count: i64,
    /// Issuing CA as named in the certificate ("Let's Encrypt R11").
    pub issuer: Option<String>,
    /// RFC 3339 time of the last successful issuance or renewal.
    pub last_renewed_at: Option<String>,
}

/// Which CA family issued a certificate, from its issuer name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum IssuerKind {
    /// Let's Encrypt production: trusted by browsers.
    Letsencrypt,
    /// Let's Encrypt staging (`TOND_ACME_STAGING=1`): NOT trusted by browsers.
    LetsencryptStaging,
    /// Any other CA (a custom `TOND_ACME_DIRECTORY`, e.g. a private ACME CA).
    Custom,
    /// No certificate issued yet.
    Unknown,
}

impl IssuerKind {
    pub fn from_issuer(issuer: Option<&str>) -> Self {
        let Some(name) = issuer.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Unknown;
        };
        let lower = name.to_ascii_lowercase();
        // Staging intermediates are "(STAGING) …" (historically "Fake LE …").
        if lower.contains("staging") || lower.contains("fake le") {
            Self::LetsencryptStaging
        } else if lower.contains("let's encrypt") || lower.contains("lets encrypt") {
            Self::Letsencrypt
        } else {
            Self::Custom
        }
    }
}

/// What a certificate's domain is used for right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CertificateTarget {
    /// The Tond dashboard (`dashboard_domain`).
    Dashboard,
    /// An app's domain.
    App { app_id: String, app_name: String },
    /// A preview deployment of an app.
    Preview {
        app_id: String,
        app_name: String,
        preview_id: String,
        pr_number: Option<i64>,
    },
    /// Nothing is served on this domain any more (a leftover certificate).
    Unused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CertificateResponse {
    /// Empty for a served domain that has no certificate row yet.
    pub id: String,
    pub domain: String,
    pub cert_type: String,
    pub status: String,
    pub issued_at: String,
    pub expires_at: String,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<String>,
    pub next_retry_at: Option<String>,
    pub failure_count: i64,
    pub issuer: Option<String>,
    pub issuer_kind: IssuerKind,
    pub last_renewed_at: Option<String>,
    pub serves: CertificateTarget,
}

impl CertificateResponse {
    pub fn from_row(c: Certificate, serves: CertificateTarget) -> Self {
        Self {
            issuer_kind: IssuerKind::from_issuer(c.issuer.as_deref()),
            id: c.id,
            domain: c.domain,
            cert_type: c.cert_type,
            status: c.status,
            issued_at: c.issued_at,
            expires_at: c.expires_at,
            last_error: c.last_error,
            last_attempt_at: c.last_attempt_at,
            next_retry_at: c.next_retry_at,
            failure_count: c.failure_count,
            issuer: c.issuer,
            last_renewed_at: c.last_renewed_at,
            serves,
        }
    }

    /// A served domain tond-proxy has not reported on yet.
    pub fn waiting(domain: String, serves: CertificateTarget) -> Self {
        Self {
            id: String::new(),
            domain,
            cert_type: "acme".into(),
            status: "pending".into(),
            issued_at: String::new(),
            expires_at: String::new(),
            last_error: None,
            last_attempt_at: None,
            next_retry_at: None,
            failure_count: 0,
            issuer: None,
            issuer_kind: IssuerKind::Unknown,
            last_renewed_at: None,
            serves,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_kind_recognises_production_staging_and_other_cas() {
        assert_eq!(
            IssuerKind::from_issuer(Some("Let's Encrypt R11")),
            IssuerKind::Letsencrypt
        );
        assert_eq!(
            IssuerKind::from_issuer(Some(
                "(STAGING) Let's Encrypt (STAGING) Riddling Rhubarb R12"
            )),
            IssuerKind::LetsencryptStaging
        );
        assert_eq!(
            IssuerKind::from_issuer(Some("Fake LE Intermediate X1")),
            IssuerKind::LetsencryptStaging
        );
        assert_eq!(
            IssuerKind::from_issuer(Some("Pebble Intermediate CA 4b1b52")),
            IssuerKind::Custom
        );
        assert_eq!(IssuerKind::from_issuer(None), IssuerKind::Unknown);
        assert_eq!(IssuerKind::from_issuer(Some("  ")), IssuerKind::Unknown);
    }

    #[test]
    fn targets_serialise_with_a_kind_tag() {
        let v = serde_json::to_value(CertificateTarget::App {
            app_id: "a1".into(),
            app_name: "web".into(),
        })
        .unwrap_or_default();
        assert_eq!(v["kind"], "app");
        assert_eq!(v["app_name"], "web");
        let v = serde_json::to_value(CertificateTarget::Unused).unwrap_or_default();
        assert_eq!(v["kind"], "unused");
    }
}
