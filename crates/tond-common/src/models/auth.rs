// auth.rs — User, Session, API keys, credentials, and all auth/TOTP request types

use chrono::Utc;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// ── API Key ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

impl ApiKey {
    /// Generate a new API key. Returns (ApiKey, raw_key) where raw_key is the
    /// plaintext key that must be shown to the user exactly once.
    pub fn generate(name: &str) -> (Self, String) {
        let raw_key = generate_raw_key();
        let key_hash = hash_key(&raw_key);
        let now = Utc::now().to_rfc3339();

        let api_key = Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            key_hash,
            created_at: now,
            last_used_at: None,
        };

        (api_key, raw_key)
    }
}

/// Generate a cryptographically random 32-byte API key, hex-encoded (64 chars).
pub fn generate_raw_key() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Hash an API key with SHA-256 for storage.
pub fn hash_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

// ── CLI credentials ──

#[derive(Debug, Serialize, Deserialize)]
pub struct Credentials {
    pub server: String,
    pub api_key: String,
}

// ── User (dashboard account) ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String, // "admin" | "member"
    pub created_at: String,
    pub updated_at: String,
    /// AES-GCM encrypted TOTP secret (None until 2FA is set up)
    #[serde(skip_serializing)]
    pub totp_secret_enc: Option<String>,
    /// Whether TOTP 2FA is currently active for this user (0/1 in SQLite)
    pub totp_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    /// Whether TOTP 2FA is active, so the dashboard can offer "disable"
    pub totp_enabled: bool,
    pub created_at: String,
}

impl From<User> for UserResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            role: u.role,
            totp_enabled: u.totp_enabled,
            created_at: u.created_at,
        }
    }
}

/// Request body for `POST /api/auth/setup` (first-run admin account).
#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    /// One-time setup token from the installer (`<data dir>/setup-token`).
    pub token: String,
    pub email: String,
    pub password: String,
}

/// Request body for `POST /api/auth/setup/check`.
#[derive(Debug, Deserialize)]
pub struct SetupCheckRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "member".to_string()
}

// ── Session (dashboard login session) ──

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub expires_at: String,
    pub created_at: String,
    /// HTTP User-Agent header captured at login (optional)
    pub user_agent: Option<String>,
    /// Client IP address captured at login (optional)
    pub ip_address: Option<String>,
    /// Timestamp of the most recent request that touched this session
    pub last_used_at: Option<String>,
}

/// Public API view of a session — never includes the raw token (the token
/// is the session ID, which is never returned after creation).
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: String,
    /// Whether this is the session that made the current request
    pub is_current: bool,
}

// ── Phase 7.1 — Password & 2FA request types ──

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Returned by `POST /api/auth/2fa/setup`. Contains the plain-text secret,
/// an `otpauth://` URI, and a base64-encoded PNG QR code ready for use as
/// `<img src="data:image/png;base64,{qr_code}">`. 2FA is **not** active
/// until `POST /api/auth/2fa/confirm` is called with a valid code.
#[derive(Debug, Serialize)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub otpauth_uri: String,
    /// Base64-encoded PNG QR code (data URI payload, no prefix).
    pub qr_code: String,
}

/// Request body for `POST /api/auth/2fa/setup`: the current password, so a
/// hijacked session can't silently enroll an attacker's authenticator.
#[derive(Debug, Deserialize)]
pub struct TotpSetupRequest {
    pub password: String,
}

/// Request body for `POST /api/auth/2fa/confirm`.
#[derive(Debug, Deserialize)]
pub struct TotpConfirmRequest {
    pub code: String,
}

/// Request body for `POST /api/auth/2fa/disable`.
#[derive(Debug, Deserialize)]
pub struct TotpDisableRequest {
    pub password: String,
    pub code: String,
}

/// Request body for `POST /api/auth/2fa/verify`.
#[derive(Debug, Deserialize)]
pub struct TotpVerifyRequest {
    /// Either a 6-digit TOTP code or one of the 16-char hex backup codes.
    pub code: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_raw_key_returns_64_character_hex_string() {
        let key = generate_raw_key();

        assert_eq!(
            key.len(),
            64,
            "generate_raw_key should return a 32-byte key encoded as 64 hex characters"
        );
        assert!(
            key.chars().all(|c| c.is_ascii_hexdigit()),
            "generate_raw_key should only contain hexadecimal characters"
        );
    }

    #[test]
    fn generate_raw_key_produces_distinct_values() {
        let first = generate_raw_key();
        let second = generate_raw_key();

        assert_ne!(
            first, second,
            "generate_raw_key should produce fresh random output on each call"
        );
    }

    #[test]
    fn hash_key_matches_known_sha256_value() {
        let digest = hash_key("tond-test-key");

        assert_eq!(
            digest, "046bb0105eb6bd0b0678a9e855099a6c36cdb7857071ab02411b1360c1c99e15",
            "hash_key should compute the SHA-256 digest of the provided raw key"
        );
    }

    #[test]
    fn api_key_generate_hashes_returned_raw_key() {
        let (api_key, raw_key) = ApiKey::generate("root");

        assert_eq!(
            api_key.name, "root",
            "ApiKey::generate should preserve the provided key name"
        );
        assert_eq!(
            api_key.key_hash,
            hash_key(&raw_key),
            "ApiKey::generate should store the hash of the returned raw key"
        );
        assert_eq!(
            api_key.last_used_at, None,
            "newly generated API keys should not have a last_used_at timestamp"
        );
    }

    #[test]
    fn user_serialization_omits_password_hash() {
        let user = User {
            id: "user-1".to_string(),
            email: "admin@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            role: "admin".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            totp_secret_enc: Some("encrypted".to_string()),
            totp_enabled: true,
        };

        let serialized = serde_json::to_value(&user)
            .unwrap_or_else(|err| panic!("failed to serialize User: {err}"));

        assert_eq!(
            serialized.get("password_hash"),
            None,
            "User serialization should omit password_hash"
        );
    }

    #[test]
    fn create_user_request_defaults_role_to_member() {
        let request: CreateUserRequest = serde_json::from_value(serde_json::json!({
            "email": "member@example.com",
            "password": "hunter2"
        }))
        .unwrap_or_else(|err| panic!("failed to deserialize CreateUserRequest: {err}"));

        assert_eq!(
            request.role, "member",
            "CreateUserRequest should default role to member"
        );
    }

    #[test]
    fn user_response_from_user_omits_sensitive_fields() {
        let user = User {
            id: "user-1".to_string(),
            email: "admin@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            role: "admin".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            totp_secret_enc: Some("encrypted".to_string()),
            totp_enabled: true,
        };

        let response = UserResponse::from(user);
        let serialized = serde_json::to_value(&response)
            .unwrap_or_else(|err| panic!("failed to serialize UserResponse: {err}"));

        assert_eq!(
            response.email, "admin@example.com",
            "UserResponse should preserve the user email"
        );
        assert_eq!(
            serialized.get("password_hash"),
            None,
            "UserResponse should not expose password_hash"
        );
        assert_eq!(
            serialized.get("totp_secret_enc"),
            None,
            "UserResponse should not expose the encrypted TOTP secret"
        );
        assert_eq!(
            serialized.get("totp_enabled"),
            Some(&serde_json::json!(true)),
            "UserResponse should say whether 2FA is enabled"
        );
    }

    #[test]
    fn session_response_round_trips_with_is_current_flag() {
        let response = SessionResponse {
            id: "session-1".to_string(),
            user_agent: Some("curl/8.0".to_string()),
            ip_address: Some("127.0.0.1".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used_at: Some("2026-01-01T01:00:00Z".to_string()),
            expires_at: "2026-01-08T00:00:00Z".to_string(),
            is_current: true,
        };

        let roundtrip: SessionResponse = serde_json::from_value(
            serde_json::to_value(&response)
                .unwrap_or_else(|err| panic!("failed to serialize SessionResponse: {err}")),
        )
        .unwrap_or_else(|err| panic!("failed to deserialize SessionResponse: {err}"));

        assert!(
            roundtrip.is_current,
            "SessionResponse should preserve the is_current flag through serde round-trips"
        );
        assert_eq!(
            roundtrip.user_agent.as_deref(),
            Some("curl/8.0"),
            "SessionResponse should preserve the user agent through serde round-trips"
        );
    }
}
