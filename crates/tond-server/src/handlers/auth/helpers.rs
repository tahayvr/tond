// handlers/auth/helpers.rs — private helpers shared across auth sub-modules

use axum::http::{HeaderMap, header};
use chrono::{Duration, Utc};
use tond_common::generate_raw_key;

/// Hash a password with argon2.
pub(super) fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::{Argon2, PasswordHasher};

    // A random salt is generated internally from the OS RNG.
    let hash = Argon2::default().hash_password(password.as_bytes())?;
    Ok(hash.to_string())
}

/// Verify a password against a stored argon2 hash.
pub(super) fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::phc::PasswordHash;
    use argon2::{Argon2, PasswordVerifier};

    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Create a session for a user and return the session token.
/// Captures the optional User-Agent and IP address for session listing.
pub(super) async fn create_session(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    user_agent: Option<&str>,
    ip_address: Option<&str>,
) -> Result<String, sqlx::Error> {
    let session_id = generate_raw_key(); // reuse the random hex generator
    let now = Utc::now();
    let expires = now + Duration::hours(24);

    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at, created_at, user_agent, ip_address, last_used_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(expires.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(user_agent)
    .bind(ip_address)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(session_id)
}

/// Extract the User-Agent string from request headers (truncated to 512 chars).
pub(super) fn extract_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(512).collect())
}

/// Extract the client IP from X-Forwarded-For or the connection's remote addr.
/// Falls back to None if neither is present.
pub(super) fn extract_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
}

/// Build a Set-Cookie header value for the session token.
pub(super) fn session_cookie(token: &str, max_age_secs: i64) -> String {
    format!(
        "tond_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        token, max_age_secs
    )
}

/// Build a Set-Cookie header that clears the session cookie.
pub(super) fn clear_session_cookie() -> String {
    "tond_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0".to_string()
}

/// Encrypt a plaintext string using AES-256-GCM (same helper used for GitLab tokens).
pub(super) fn encrypt_secret(plaintext: &str, key: &[u8; 32]) -> String {
    use aes_gcm::aead::{Aead, Nonce};
    use aes_gcm::{Aes256Gcm, KeyInit};
    use rand::Rng;

    let cipher = Aes256Gcm::new(&(*key).into());
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::<Aes256Gcm>::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .expect("encryption failure");
    // Encode as nonce_hex:ciphertext_hex
    format!("{}:{}", hex::encode(nonce_bytes), hex::encode(ciphertext))
}

/// Decrypt a string encrypted with `encrypt_secret`.
pub(super) fn decrypt_secret(encoded: &str, key: &[u8; 32]) -> Option<String> {
    use aes_gcm::aead::{Aead, Nonce};
    use aes_gcm::{Aes256Gcm, KeyInit};

    let parts: Vec<&str> = encoded.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }
    let nonce_bytes = hex::decode(parts[0]).ok()?;
    let ciphertext = hex::decode(parts[1]).ok()?;
    let cipher = Aes256Gcm::new(&(*key).into());
    // A malformed nonce is rejected here instead of panicking.
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce_bytes.as_slice()).ok()?;
    let plaintext = cipher.decrypt(&nonce, ciphertext.as_ref()).ok()?;
    String::from_utf8(plaintext).ok()
}

/// Derive a 32-byte AES key from the TOND_SECRET_KEY env var (or a hard-coded
/// dev default). In production the operator sets TOND_SECRET_KEY to a random
/// 32-char hex string.
pub(super) fn aes_key() -> [u8; 32] {
    let raw = std::env::var("TOND_SECRET_KEY")
        .unwrap_or_else(|_| "00000000000000000000000000000000".to_string());
    let bytes = raw.as_bytes();
    let mut key = [0u8; 32];
    let len = bytes.len().min(32);
    key[..len].copy_from_slice(&bytes[..len]);
    key
}

/// Verify a 6-digit TOTP code against a base32 secret, allowing ±1 step skew.
pub(super) fn verify_totp(secret_b32: &str, code: &str) -> bool {
    use totp_rs::{Builder, Secret};
    let Ok(secret) = Secret::try_from_base32(secret_b32) else {
        return false;
    };
    // Builder defaults: SHA1, 6 digits, ±1 step skew, 30 s steps.
    let Ok(totp) = Builder::new().with_secret(secret).build() else {
        return false;
    };
    totp.check_current(code).is_some()
}

/// Simple base32 encoding (RFC 4648, no padding) for TOTP secrets.
pub(super) fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::new();
    let mut buffer: u32 = 0;
    let mut bits_left: u32 = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits_left += 8;
        while bits_left >= 5 {
            bits_left -= 5;
            output.push(ALPHABET[((buffer >> bits_left) & 0x1F) as usize] as char);
        }
    }
    if bits_left > 0 {
        output.push(ALPHABET[((buffer << (5 - bits_left)) & 0x1F) as usize] as char);
    }
    output
}

/// Check if the submitted string matches a stored backup code for the user.
/// If it matches, deletes the code so it cannot be reused.
pub(super) async fn use_backup_code(pool: &sqlx::SqlitePool, user_id: &str, code: &str) -> bool {
    use sha2::Digest;
    let hash = hex::encode(sha2::Sha256::digest(code.as_bytes()));

    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM totp_backup_codes WHERE user_id = ? AND code_hash = ?")
            .bind(user_id)
            .bind(&hash)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if let Some((code_id,)) = row {
        let _ = sqlx::query("DELETE FROM totp_backup_codes WHERE id = ?")
            .bind(&code_id)
            .execute(pool)
            .await;
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};
    use sha2::Digest;
    use sqlx::SqlitePool;
    use totp_rs::{Builder, Secret};

    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&pool)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for auth helper tests: {err}"));
        pool
    }

    #[test]
    fn hash_password_and_verify_password_round_trip() {
        let hash = hash_password("S3cureP@ssword!")
            .unwrap_or_else(|err| panic!("failed to hash password: {err}"));

        assert!(
            verify_password("S3cureP@ssword!", &hash),
            "verify_password should accept the original password against its argon2 hash"
        );
        assert!(
            !verify_password("wrong-password", &hash),
            "verify_password should reject an incorrect password"
        );
    }

    #[tokio::test]
    async fn create_session_persists_session_with_metadata() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("user-1")
        .bind("admin@example.com")
        .bind("hash")
        .bind("admin")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(0)
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user for session test: {err}"));

        let token = create_session(&pool, "user-1", Some("curl/8.0"), Some("127.0.0.1"))
            .await
            .unwrap_or_else(|err| panic!("failed to create session: {err}"));

        let row: (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT id, user_id, user_agent, ip_address, last_used_at FROM sessions WHERE id = ?",
        )
        .bind(&token)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to read persisted session: {err}"));

        assert_eq!(
            row.0, token,
            "create_session should persist the generated session token"
        );
        assert_eq!(
            row.1, "user-1",
            "create_session should persist the owning user_id"
        );
        assert_eq!(
            row.2.as_deref(),
            Some("curl/8.0"),
            "create_session should persist user_agent metadata"
        );
        assert_eq!(
            row.3.as_deref(),
            Some("127.0.0.1"),
            "create_session should persist ip_address metadata"
        );
        assert!(
            row.4.is_some(),
            "create_session should initialize last_used_at"
        );
    }

    #[test]
    fn extract_user_agent_returns_truncated_value() {
        let mut headers = HeaderMap::new();
        let long_user_agent = "a".repeat(600);
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_str(&long_user_agent)
                .unwrap_or_else(|err| panic!("failed to build test User-Agent header: {err}")),
        );

        let user_agent = extract_user_agent(&headers)
            .unwrap_or_else(|| panic!("expected extract_user_agent to return a value"));

        assert_eq!(
            user_agent.len(),
            512,
            "extract_user_agent should truncate overly long user agents to 512 characters"
        );
    }

    #[test]
    fn extract_ip_prefers_first_forwarded_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.1, 198.51.100.5"),
        );

        let ip = extract_ip(&headers);

        assert_eq!(
            ip.as_deref(),
            Some("203.0.113.1"),
            "extract_ip should return the first entry from X-Forwarded-For"
        );
    }

    #[test]
    fn session_cookie_helpers_generate_expected_cookie_values() {
        let set_cookie = session_cookie("token-123", 3600);
        assert!(
            set_cookie.contains("tond_session=token-123"),
            "session_cookie should include the session token"
        );
        assert!(
            set_cookie.contains("Max-Age=3600"),
            "session_cookie should include the configured Max-Age"
        );

        assert_eq!(
            clear_session_cookie(),
            "tond_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0",
            "clear_session_cookie should immediately expire the session cookie"
        );
    }

    #[test]
    fn secret_encryption_round_trips_and_rejects_invalid_payloads() {
        let key = aes_key();
        let encrypted = encrypt_secret("super-secret", &key);

        assert_eq!(
            decrypt_secret(&encrypted, &key).as_deref(),
            Some("super-secret"),
            "decrypt_secret should recover the original plaintext encrypted by encrypt_secret"
        );
        assert_eq!(
            decrypt_secret("not-a-valid-payload", &key),
            None,
            "decrypt_secret should return None for malformed encrypted payloads"
        );
    }

    #[test]
    fn base32_encode_matches_known_output() {
        assert_eq!(
            base32_encode(b"hello"),
            "NBSWY3DP",
            "base32_encode should follow RFC 4648 without padding"
        );
    }

    #[test]
    fn verify_password_accepts_hash_from_argon2_0_5() {
        // Generated with argon2 0.5.3, before the 0.6 upgrade: password hashes already
        // stored in the users table must keep verifying.
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$086ZikNPOgNLQ7MpJbhgnQ$Br9v0kEEm/Gs0SC69qDZILv4n/fBzd04aU6xnSGnM1g";

        assert!(verify_password("correct horse battery staple", hash));
        assert!(!verify_password("wrong password", hash));
    }

    #[test]
    fn decrypt_secret_reads_ciphertext_from_aes_gcm_0_10() {
        // Produced by `encrypt_secret` on aes-gcm 0.10.3, before the 0.11 upgrade: encrypted
        // TOTP secrets already stored in the database must keep decrypting.
        let key = [b'0'; 32];
        let encoded = "070707070707070707070707:81a84ff1a54a3069c8dc823c9f198cb131fa4ba901df6c841b79e5e7b45496b4";

        assert_eq!(
            decrypt_secret(encoded, &key).as_deref(),
            Some("JBSWY3DPEHPK3PXP")
        );
    }

    #[test]
    fn decrypt_secret_rejects_malformed_nonce_without_panicking() {
        let key = [b'0'; 32];

        assert_eq!(decrypt_secret("0707:81a84ff1", &key), None);
    }

    #[test]
    fn verify_totp_accepts_current_code_for_valid_secret() {
        let secret_b32 = base32_encode(b"12345678901234567890");
        let secret = Secret::try_from_base32(&secret_b32)
            .unwrap_or_else(|err| panic!("failed to decode base32 TOTP secret: {err}"));
        let totp = Builder::new()
            .with_secret(secret)
            .build()
            .unwrap_or_else(|err| panic!("failed to construct TOTP generator: {err}"));
        let code = totp.generate_current().to_string();

        assert!(
            verify_totp(&secret_b32, &code),
            "verify_totp should accept the current valid TOTP code"
        );
        assert!(
            !verify_totp(&secret_b32, "000000"),
            "verify_totp should reject an incorrect TOTP code"
        );
    }

    #[tokio::test]
    async fn use_backup_code_is_single_use() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("user-1")
        .bind("admin@example.com")
        .bind("hash")
        .bind("admin")
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(Option::<String>::None)
        .bind(1)
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user for backup code test: {err}"));

        let code = "deadbeefcafebabe";
        let code_hash = hex::encode(sha2::Sha256::digest(code.as_bytes()));
        sqlx::query(
            "INSERT INTO totp_backup_codes (id, user_id, code_hash, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind("code-1")
        .bind("user-1")
        .bind(&code_hash)
        .bind("2026-01-01T00:00:00Z")
        .execute(&pool)
        .await
        .unwrap_or_else(|err| panic!("failed to seed backup code: {err}"));

        assert!(
            use_backup_code(&pool, "user-1", code).await,
            "use_backup_code should accept a matching stored backup code"
        );
        assert!(
            !use_backup_code(&pool, "user-1", code).await,
            "use_backup_code should delete a backup code after first successful use"
        );
    }
}
