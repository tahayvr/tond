// certificates.rs — TLS certificate tracking types

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Certificate (TLS cert tracking) ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Why the last issuance attempt failed (or was skipped), written by
    /// tond-proxy; NULL after a success.
    pub last_error: Option<String>,
    /// RFC 3339 time of the last issuance attempt.
    pub last_attempt_at: Option<String>,
    /// RFC 3339 time before which tond-proxy will not try again (backoff).
    pub next_retry_at: Option<String>,
    /// Consecutive failed attempts.
    pub failure_count: i64,
}

impl Certificate {
    pub fn new(
        domain: String,
        cert_type: &str,
        cert_path: String,
        key_path: String,
        issued_at: String,
        expires_at: String,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            domain,
            cert_type: cert_type.to_string(),
            cert_path,
            key_path,
            issued_at,
            expires_at,
            status: "active".to_string(),
            created_at: now.clone(),
            updated_at: now,
            last_error: None,
            last_attempt_at: None,
            next_retry_at: None,
            failure_count: 0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertificateResponse {
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
}

impl From<Certificate> for CertificateResponse {
    fn from(c: Certificate) -> Self {
        Self {
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
        }
    }
}
