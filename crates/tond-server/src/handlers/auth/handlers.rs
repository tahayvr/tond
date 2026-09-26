// handlers/auth/handlers.rs — HTTP handler functions for the auth routes
//
// Brute-force protection (see auth/rate_limit.rs): every password or 2FA
// check first consults the per-client and per-account backoff, records
// failures, and runs argon2 through a small semaphore. Unknown emails are
// verified against a dummy hash so timing doesn't reveal which accounts exist.

use std::net::IpAddr;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde_json::json;
use tond_common::*;
use uuid::Uuid;

use crate::{
    audit_log::write_audit_log,
    auth::{
        ClientIp, RateLimit,
        rate_limit::lockout_message,
        setup_token::{self, setup_token_path},
    },
    state::AppState,
};

use super::helpers::{
    MFA_MAX_ATTEMPTS, MFA_PENDING_TTL_SECS, PasswordError, base32_encode, clear_mfa_pending_cookie,
    clear_session_cookie, create_session, decrypt_secret, encrypt_secret, extract_user_agent,
    hash_password_throttled, ip_label, mfa_pending_cookie, normalize_email, revoke_other_sessions,
    session_cookie, use_backup_code, verify_password_throttled, verify_totp_once,
};
use super::session::{MFA_PENDING_COOKIE, extract_cookie, extract_session_token, validate_session};

// ── Response helpers ──

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<()>::err(message.into()))).into_response()
}

fn internal_error(e: impl std::fmt::Display) -> Response {
    tracing::error!("auth handler error: {e}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

fn password_error_response(e: PasswordError) -> Response {
    match e {
        PasswordError::Busy => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "The server is busy verifying other sign-ins. Try again in a moment.",
        ),
        PasswordError::Failed => internal_error("argon2 task failed"),
    }
}

fn with_cookies(mut response: Response, cookies: &[String]) -> Response {
    for cookie in cookies {
        if let Ok(value) = cookie.parse() {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// An early-return response from an auth helper (boxed: `Response` is large).
pub type Rejection = Box<Response>;

fn reject(status: StatusCode, message: impl Into<String>) -> Rejection {
    Box::new(error_response(status, message))
}

/// Resolve the session user, or the 401/500 response to return.
async fn session_user(state: &AppState, headers: &HeaderMap) -> Result<(String, User), Rejection> {
    let token = extract_session_token(headers)
        .ok_or_else(|| reject(StatusCode::UNAUTHORIZED, "Not authenticated"))?;
    match validate_session(&state.db, &token).await {
        Ok(Some(user)) => Ok((token, user)),
        Ok(None) => Err(reject(
            StatusCode::UNAUTHORIZED,
            "Session expired or invalid",
        )),
        Err(e) => Err(Box::new(internal_error(e))),
    }
}

/// Refuse with 429 while this client or account is in login backoff.
async fn check_lockout(
    state: &AppState,
    ip: Option<IpAddr>,
    account: Option<&str>,
) -> Result<(), Rejection> {
    match state.rate_limiter.login_retry_after(ip, account).await {
        Some(wait) => Err(reject(StatusCode::TOO_MANY_REQUESTS, lockout_message(wait))),
        None => Ok(()),
    }
}

/// Check the password of an already-identified user (re-authentication),
/// with backoff. Returns the response to send when it doesn't match.
async fn reauthenticate_password(
    state: &AppState,
    ip: Option<IpAddr>,
    user: &User,
    password: &str,
    wrong_message: &str,
) -> Result<(), Rejection> {
    check_lockout(state, ip, Some(&user.id)).await?;
    match verify_password_throttled(password, Some(&user.password_hash)).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            state
                .rate_limiter
                .record_login_failure(ip, Some(&user.id))
                .await;
            Err(reject(StatusCode::UNAUTHORIZED, wrong_message))
        }
        Err(e) => Err(Box::new(password_error_response(e))),
    }
}

/// Check a TOTP code (or backup code) for a 2FA-enabled user, with backoff.
async fn verify_second_factor(
    state: &AppState,
    ip: Option<IpAddr>,
    user: &User,
    code: &str,
) -> Result<(), Rejection> {
    let secret_b32 = user
        .totp_secret_enc
        .as_deref()
        .and_then(decrypt_secret)
        .unwrap_or_default();
    let code = code.trim();
    let valid = match verify_totp_once(&state.db, &user.id, &secret_b32, code).await {
        Ok(true) => true,
        Ok(false) => use_backup_code(&state.db, &user.id, code).await,
        Err(e) => return Err(Box::new(internal_error(e))),
    };
    if valid {
        Ok(())
    } else {
        state
            .rate_limiter
            .record_login_failure(ip, Some(&user.id))
            .await;
        Err(reject(
            StatusCode::UNAUTHORIZED,
            "Invalid authentication code",
        ))
    }
}

/// Step-up re-authentication for sensitive actions (host terminal): the
/// password, plus a TOTP or backup code when the user has 2FA enabled.
pub async fn verify_step_up(
    state: &AppState,
    ip: Option<IpAddr>,
    user: &User,
    password: &str,
    code: Option<&str>,
) -> Result<(), Rejection> {
    reauthenticate_password(state, ip, user, password, "Password is incorrect").await?;
    if user.totp_enabled {
        let code = code
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| {
                reject(
                    StatusCode::UNAUTHORIZED,
                    "Enter the code from your authenticator app",
                )
            })?;
        verify_second_factor(state, ip, user, code).await?;
    }
    Ok(())
}

fn validate_admin_password(pwd: &str) -> Result<(), &'static str> {
    if pwd.chars().count() < 12 {
        return Err("Password must be at least 12 characters");
    }
    if !pwd.chars().any(|c| c.is_uppercase()) {
        return Err("Password must contain at least one uppercase letter");
    }
    if !pwd.chars().any(|c| c.is_lowercase()) {
        return Err("Password must contain at least one lowercase letter");
    }
    if !pwd.chars().any(|c| c.is_ascii_digit()) {
        return Err("Password must contain at least one number");
    }
    if !pwd.chars().any(|c| !c.is_alphanumeric()) {
        return Err("Password must contain at least one special character");
    }
    Ok(())
}

const INVALID_SETUP_TOKEN: &str = "This setup link is invalid or has already been used. \
     If the admin account already exists, sign in instead.";

// ── GET /api/auth/status ──
// Whether the caller is authenticated. No auth required. Deliberately says
// nothing about whether first-run setup is still open: setup needs the
// one-time token anyway, and the setup page checks that token itself.

pub async fn auth_status(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match extract_session_token(&headers) {
        Some(token) => validate_session(&state.db, &token).await.unwrap_or(None),
        None => None,
    };

    let response = json!({
        "authenticated": user.is_some(),
        "user": user.map(UserResponse::from),
    });

    (StatusCode::OK, Json(ApiResponse::ok(response))).into_response()
}

// ── POST /api/auth/setup/check ──
// Lets the setup page validate a setup link before showing the form.

pub async fn setup_check(
    _rate: RateLimit,
    State(state): State<AppState>,
    Json(req): Json<SetupCheckRequest>,
) -> Response {
    match setup_token::is_valid(&state.db, &req.token).await {
        Ok(true) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "valid": true }))),
        )
            .into_response(),
        Ok(false) => error_response(StatusCode::FORBIDDEN, INVALID_SETUP_TOKEN),
        Err(e) => internal_error(e),
    }
}

// ── POST /api/auth/setup ──
// First-run admin account creation. Requires the one-time setup token; the
// token is consumed and the user inserted in one transaction, so of two
// concurrent requests only one can succeed.

pub async fn setup(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetupRequest>,
) -> Response {
    let ua = extract_user_agent(&headers);
    let email = req.email.trim().to_string();

    // Cheap rejection before any argon2 work.
    match setup_token::is_valid(&state.db, &req.token).await {
        Ok(true) => {}
        Ok(false) => return error_response(StatusCode::FORBIDDEN, INVALID_SETUP_TOKEN),
        Err(e) => return internal_error(e),
    }

    if email.is_empty() || !email.contains('@') {
        return error_response(StatusCode::BAD_REQUEST, "Invalid email address");
    }
    if let Err(message) = validate_admin_password(&req.password) {
        return error_response(StatusCode::BAD_REQUEST, message);
    }

    let password_hash = match hash_password_throttled(&req.password).await {
        Ok(h) => h,
        Err(e) => return password_error_response(e),
    };

    let now = Utc::now().to_rfc3339();
    let user_id = Uuid::new_v4().to_string();

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => return internal_error(e),
    };
    match setup_token::consume(&mut tx, &req.token).await {
        Ok(true) => {}
        Ok(false) => return error_response(StatusCode::FORBIDDEN, INVALID_SETUP_TOKEN),
        Err(e) => return internal_error(e),
    }
    let inserted = sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, created_at, updated_at)
         SELECT ?, ?, ?, 'admin', ?, ?
         WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(&user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await;
    match inserted {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "Setup already completed. Use login instead.",
            );
        }
        Err(e) => return internal_error(e),
    }
    let ip_str = ip_label(ip);
    let session_token =
        match create_session(&mut *tx, &user_id, ua.as_deref(), ip_str.as_deref()).await {
            Ok(t) => t,
            Err(e) => return internal_error(e),
        };
    if let Err(e) = tx.commit().await {
        return internal_error(e);
    }

    setup_token::remove_token_file(&setup_token_path());
    write_audit_log(
        &state.db,
        None,
        None,
        "admin_setup",
        &email,
        Some("first-run admin account created"),
    )
    .await;

    let user_resp = UserResponse {
        id: user_id,
        email,
        role: "admin".to_string(),
        totp_enabled: false,
        created_at: now,
    };
    let response = (
        StatusCode::CREATED,
        Json(ApiResponse::ok(json!({
            "user": user_resp,
            "message": "Admin account created",
        }))),
    )
        .into_response();
    with_cookies(response, &[session_cookie(&session_token)])
}

// ── POST /api/auth/login ──
// When the user has 2FA enabled, this endpoint responds with a 200 and a
// short-lived pending-2FA cookie instead of the full session. The client must
// then POST to /api/auth/2fa/verify with the TOTP code.

pub async fn login(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    use tond_common::generate_raw_key;

    let ua = extract_user_agent(&headers);

    let user = match sqlx::query_as::<_, User>("SELECT * FROM users WHERE lower(email) = ?")
        .bind(normalize_email(&req.email))
        .fetch_optional(&state.db)
        .await
    {
        Ok(u) => u,
        Err(e) => return internal_error(e),
    };
    let account = user.as_ref().map(|u| u.id.as_str());

    if let Err(response) = check_lockout(&state, ip, account).await {
        return *response;
    }

    let verified = match verify_password_throttled(
        &req.password,
        user.as_ref().map(|u| u.password_hash.as_str()),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return password_error_response(e),
    };
    let user = match (verified, user) {
        (true, Some(user)) => user,
        (_, user) => {
            state
                .rate_limiter
                .record_login_failure(ip, user.as_ref().map(|u| u.id.as_str()))
                .await;
            return error_response(StatusCode::UNAUTHORIZED, "Invalid email or password");
        }
    };

    if user.totp_enabled {
        let pending = generate_raw_key();
        let now = Utc::now();
        let expires = now + chrono::Duration::seconds(MFA_PENDING_TTL_SECS);

        if let Err(e) = sqlx::query(
            "INSERT INTO mfa_pending (id, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(hash_key(&pending))
        .bind(&user.id)
        .bind(expires.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        {
            return internal_error(e);
        }

        let response = (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "mfa_required": true }))),
        )
            .into_response();
        return with_cookies(response, &[mfa_pending_cookie(&pending)]);
    }

    let ip_str = ip_label(ip);
    let session_token =
        match create_session(&state.db, &user.id, ua.as_deref(), ip_str.as_deref()).await {
            Ok(t) => t,
            Err(e) => return internal_error(e),
        };
    state.rate_limiter.record_login_success(&user.id).await;

    let response = (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "user": UserResponse::from(user) }))),
    )
        .into_response();
    with_cookies(response, &[session_cookie(&session_token)])
}

// ── POST /api/auth/logout ──

pub async fn logout(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Some(token) = extract_session_token(&headers) {
        let _ = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(hash_key(&token))
            .execute(&state.db)
            .await;
    }

    let response = (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "Logged out" }))),
    )
        .into_response();
    with_cookies(
        response,
        &[clear_session_cookie(), clear_mfa_pending_cookie()],
    )
}

// ── GET /api/auth/me ──
// Returns the current user. Requires a valid session cookie.

pub async fn me(_rate: RateLimit, State(state): State<AppState>, headers: HeaderMap) -> Response {
    match session_user(&state, &headers).await {
        Ok((_, user)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(UserResponse::from(user))),
        )
            .into_response(),
        Err(response) => *response,
    }
}

// ── POST /api/auth/users ──
// Admin-only: create a new user account.

pub async fn create_user(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> Response {
    let caller = match session_user(&state, &headers).await {
        Ok((_, user)) => user,
        Err(response) => return *response,
    };
    if caller.role != "admin" {
        return error_response(StatusCode::FORBIDDEN, "Only admins can create users");
    }

    let email = req.email.trim().to_string();
    if email.is_empty() || !email.contains('@') {
        return error_response(StatusCode::BAD_REQUEST, "Invalid email address");
    }
    if req.password.chars().count() < 8 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters",
        );
    }
    if !["admin", "member"].contains(&req.role.as_str()) {
        return error_response(StatusCode::BAD_REQUEST, "Role must be 'admin' or 'member'");
    }

    let password_hash = match hash_password_throttled(&req.password).await {
        Ok(h) => h,
        Err(e) => return password_error_response(e),
    };

    let now = Utc::now().to_rfc3339();
    let user_id = Uuid::new_v4().to_string();

    match sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(&req.role)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    {
        Ok(_) => {
            let user_resp = UserResponse {
                id: user_id,
                email,
                role: req.role,
                totp_enabled: false,
                created_at: now,
            };
            (StatusCode::CREATED, Json(ApiResponse::ok(user_resp))).into_response()
        }
        Err(e) if e.to_string().contains("UNIQUE") => error_response(
            StatusCode::CONFLICT,
            format!("User with email '{email}' already exists"),
        ),
        Err(e) => internal_error(e),
    }
}

// ── GET /api/auth/users ──
// Admin-only: list all users.

pub async fn list_users(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let caller = match session_user(&state, &headers).await {
        Ok((_, user)) => user,
        Err(response) => return *response,
    };
    if caller.role != "admin" {
        return error_response(StatusCode::FORBIDDEN, "Only admins can list users");
    }

    match sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at ASC")
        .fetch_all(&state.db)
        .await
    {
        Ok(users) => {
            let responses: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
            (StatusCode::OK, Json(ApiResponse::ok(responses))).into_response()
        }
        Err(e) => internal_error(e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Password Management
// ─────────────────────────────────────────────────────────────────────────────

// ── POST /api/auth/change-password ──
// Session cookie only (only human users have passwords). Verifies the current
// password, re-hashes the new one, and signs out every other session.

pub async fn change_password(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    let (token, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };

    if let Err(response) = reauthenticate_password(
        &state,
        ip,
        &user,
        &req.current_password,
        "Current password is incorrect",
    )
    .await
    {
        return *response;
    }

    if req.new_password.chars().count() < 8 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "New password must be at least 8 characters",
        );
    }

    let new_hash = match hash_password_throttled(&req.new_password).await {
        Ok(h) => h,
        Err(e) => return password_error_response(e),
    };

    let now = Utc::now().to_rfc3339();
    if let Err(e) = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(&new_hash)
        .bind(&now)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        return internal_error(e);
    }

    let _ = revoke_other_sessions(&state.db, &user.id, Some(&token)).await;

    (
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Password changed successfully" }),
        )),
    )
        .into_response()
}

// ── GET /api/auth/sessions ──
// All active sessions for the current user (dashboard "Security" tab). The
// `id` is the stored hash, which identifies a session without being usable
// as a cookie.

pub async fn list_sessions(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let (token, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };
    let current = hash_key(&token);
    let now = Utc::now().to_rfc3339();

    let sessions = match sqlx::query_as::<_, Session>(
        "SELECT * FROM sessions WHERE user_id = ? AND expires_at > ? ORDER BY created_at DESC",
    )
    .bind(&user.id)
    .bind(&now)
    .fetch_all(&state.db)
    .await
    {
        Ok(s) => s,
        Err(e) => return internal_error(e),
    };

    let responses: Vec<SessionResponse> = sessions
        .into_iter()
        .map(|s| SessionResponse {
            is_current: s.id == current,
            id: s.id,
            user_agent: s.user_agent,
            ip_address: s.ip_address,
            created_at: s.created_at,
            last_used_at: s.last_used_at,
            expires_at: s.expires_at,
        })
        .collect();

    (StatusCode::OK, Json(ApiResponse::ok(responses))).into_response()
}

// ── DELETE /api/auth/sessions/{id} ──
// Revoke a specific session (by the id from list_sessions). Users can only
// revoke their own sessions.

pub async fn revoke_session(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let (_, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };

    match sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(&session_id)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => {
            error_response(StatusCode::NOT_FOUND, "Session not found")
        }
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "message": "Session revoked" }))),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

// ── DELETE /api/auth/sessions ──
// Sign out everywhere: revoke all sessions except the current one.

pub async fn revoke_all_sessions(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let (token, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };

    match revoke_other_sessions(&state.db, &user.id, Some(&token)).await {
        Ok(n) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                json!({ "message": format!("Revoked {n} session(s)") }),
            )),
        )
            .into_response(),
        Err(e) => internal_error(e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TOTP Two-Factor Authentication
// ─────────────────────────────────────────────────────────────────────────────

// ── POST /api/auth/2fa/setup ──
// Requires the current password and is refused while 2FA is enabled (disable
// it first), so a stolen session can't swap in the attacker's authenticator.
// Generates a fresh secret, stores it encrypted (not yet enabled), and returns
// the secret + otpauth:// URI + QR code. 2FA is active only after /confirm.

pub async fn totp_setup(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpSetupRequest>,
) -> Response {
    let (_, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };

    if user.totp_enabled {
        return error_response(
            StatusCode::CONFLICT,
            "2FA is already enabled. Disable it first to set up a new authenticator.",
        );
    }

    if let Err(response) =
        reauthenticate_password(&state, ip, &user, &req.password, "Password is incorrect").await
    {
        return *response;
    }

    use rand::Rng;
    use totp_rs::Builder;
    let mut secret_bytes = [0u8; 20];
    rand::rng().fill_bytes(&mut secret_bytes);
    let secret_b32 = base32_encode(&secret_bytes);

    let totp = match Builder::new()
        .with_secret(secret_bytes)
        .with_issuer(Some("Tond"))
        .with_account_name(user.email.as_str())
        .build()
    {
        Ok(t) => t,
        Err(e) => return internal_error(e),
    };
    let otpauth_uri = match totp.to_url() {
        Ok(url) => url,
        Err(e) => return internal_error(e),
    };
    let qr_code = match totp.to_qr_base64() {
        Ok(b64) => b64,
        Err(e) => return internal_error(e),
    };

    let encrypted = encrypt_secret(&secret_b32);
    let now = Utc::now().to_rfc3339();
    // A new secret starts with no accepted time step (replay tracking reset).
    // The WHERE clause re-checks totp_enabled in case of a concurrent confirm.
    match sqlx::query(
        "UPDATE users SET totp_secret_enc = ?, totp_last_step = NULL, updated_at = ?
         WHERE id = ? AND totp_enabled = 0",
    )
    .bind(&encrypted)
    .bind(&now)
    .bind(&user.id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => return error_response(StatusCode::CONFLICT, "2FA is already enabled."),
        Err(e) => return internal_error(e),
    }

    (
        StatusCode::OK,
        Json(ApiResponse::ok(TotpSetupResponse {
            secret: secret_b32,
            otpauth_uri,
            qr_code,
        })),
    )
        .into_response()
}

// ── POST /api/auth/2fa/confirm ──
// Verify the first TOTP code to confirm the user has scanned the QR correctly.
// On success: enable 2FA, generate 10 one-time backup codes, and sign out all
// other sessions.

pub async fn totp_confirm(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpConfirmRequest>,
) -> Response {
    let (token, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };

    if user.totp_enabled {
        return error_response(StatusCode::CONFLICT, "2FA is already enabled.");
    }

    let Some(encrypted) = user.totp_secret_enc.clone() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "2FA setup not started. Call POST /api/auth/2fa/setup first.",
        );
    };
    let Some(secret_b32) = decrypt_secret(&encrypted) else {
        return internal_error("failed to decrypt TOTP secret");
    };

    if let Err(response) = check_lockout(&state, ip, Some(&user.id)).await {
        return *response;
    }
    match verify_totp_once(&state.db, &user.id, &secret_b32, req.code.trim()).await {
        Ok(true) => {}
        Ok(false) => {
            state
                .rate_limiter
                .record_login_failure(ip, Some(&user.id))
                .await;
            return error_response(StatusCode::UNAUTHORIZED, "Invalid TOTP code");
        }
        Err(e) => return internal_error(e),
    }

    let now = Utc::now().to_rfc3339();
    if let Err(e) = sqlx::query("UPDATE users SET totp_enabled = 1, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        return internal_error(e);
    }

    let _ = sqlx::query("DELETE FROM totp_backup_codes WHERE user_id = ?")
        .bind(&user.id)
        .execute(&state.db)
        .await;

    let mut backup_codes: Vec<String> = Vec::with_capacity(10);
    use rand::Rng;
    for _ in 0..10 {
        let mut raw = [0u8; 8];
        rand::rng().fill_bytes(&mut raw);
        let code = hex::encode(raw); // 16-char hex
        backup_codes.push(code.clone());

        use sha2::Digest;
        let hash = hex::encode(sha2::Sha256::digest(code.as_bytes()));
        let _ = sqlx::query(
            "INSERT INTO totp_backup_codes (id, user_id, code_hash, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&user.id)
        .bind(&hash)
        .bind(&now)
        .execute(&state.db)
        .await;
    }

    // Anyone else holding a session got in without the new second factor.
    let _ = revoke_other_sessions(&state.db, &user.id, Some(&token)).await;
    write_audit_log(&state.db, None, None, "2fa_enabled", &user.email, None).await;

    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({
            "message": "2FA enabled successfully",
            "backup_codes": backup_codes
        }))),
    )
        .into_response()
}

// ── POST /api/auth/2fa/disable ──
// Requires the current password AND a valid TOTP code (or backup code).
// Clears the secret, deletes backup codes, and signs out all other sessions.

pub async fn totp_disable(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpDisableRequest>,
) -> Response {
    let (token, user) = match session_user(&state, &headers).await {
        Ok(v) => v,
        Err(response) => return *response,
    };

    if let Err(response) = reauthenticate_password(
        &state,
        ip,
        &user,
        &req.password,
        "Current password is incorrect",
    )
    .await
    {
        return *response;
    }

    if !user.totp_enabled {
        return error_response(StatusCode::BAD_REQUEST, "2FA is not enabled");
    }

    if let Err(response) = verify_second_factor(&state, ip, &user, &req.code).await {
        return *response;
    }

    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        "UPDATE users SET totp_enabled = 0, totp_secret_enc = NULL, totp_last_step = NULL,
         updated_at = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(&user.id)
    .execute(&state.db)
    .await;

    let _ = sqlx::query("DELETE FROM totp_backup_codes WHERE user_id = ?")
        .bind(&user.id)
        .execute(&state.db)
        .await;

    let _ = revoke_other_sessions(&state.db, &user.id, Some(&token)).await;
    write_audit_log(&state.db, None, None, "2fa_disabled", &user.email, None).await;

    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "2FA disabled" }))),
    )
        .into_response()
}

// ── POST /api/auth/2fa/verify ──
// Second factor after login when 2FA is enabled. Validates the pending-2FA
// cookie, checks the TOTP code (or backup code), and on success issues a full
// session cookie. Each pending login allows MFA_MAX_ATTEMPTS wrong codes;
// after that the password has to be entered again.

pub async fn totp_verify(
    _rate: RateLimit,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpVerifyRequest>,
) -> Response {
    let ua = extract_user_agent(&headers);

    let Some(pending) = extract_cookie(&headers, MFA_PENDING_COOKIE) else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "No pending sign-in. Please log in first.",
        );
    };
    let pending_hash = hash_key(&pending);
    let now = Utc::now().to_rfc3339();

    // Count this attempt up front (atomically), so parallel guesses can't
    // exceed the cap.
    let row: Option<(String, i64)> = match sqlx::query_as(
        "UPDATE mfa_pending SET attempts = attempts + 1
         WHERE id = ? AND expires_at > ? AND attempts < ?
         RETURNING user_id, attempts",
    )
    .bind(&pending_hash)
    .bind(&now)
    .bind(MFA_MAX_ATTEMPTS)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(e) => return internal_error(e),
    };

    let Some((user_id, attempts)) = row else {
        let _ = sqlx::query("DELETE FROM mfa_pending WHERE id = ?")
            .bind(&pending_hash)
            .execute(&state.db)
            .await;
        return with_cookies(
            error_response(
                StatusCode::UNAUTHORIZED,
                "Your sign-in expired or had too many wrong codes. Please log in again.",
            ),
            &[clear_mfa_pending_cookie()],
        );
    };

    if let Err(response) = check_lockout(&state, ip, Some(&user_id)).await {
        return *response;
    }

    let user = match sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(u)) => u,
        Ok(None) => return error_response(StatusCode::UNAUTHORIZED, "User not found"),
        Err(e) => return internal_error(e),
    };

    if let Err(response) = verify_second_factor(&state, ip, &user, &req.code).await {
        let response = *response;
        if attempts >= MFA_MAX_ATTEMPTS {
            let _ = sqlx::query("DELETE FROM mfa_pending WHERE id = ?")
                .bind(&pending_hash)
                .execute(&state.db)
                .await;
            return with_cookies(
                error_response(
                    StatusCode::UNAUTHORIZED,
                    "Too many wrong codes. Please log in again.",
                ),
                &[clear_mfa_pending_cookie()],
            );
        }
        return response;
    }

    let _ = sqlx::query("DELETE FROM mfa_pending WHERE id = ?")
        .bind(&pending_hash)
        .execute(&state.db)
        .await;

    let ip_str = ip_label(ip);
    let session_token =
        match create_session(&state.db, &user.id, ua.as_deref(), ip_str.as_deref()).await {
            Ok(t) => t,
            Err(e) => return internal_error(e),
        };
    state.rate_limiter.record_login_success(&user.id).await;

    let response = (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "user": UserResponse::from(user) }))),
    )
        .into_response();
    with_cookies(
        response,
        &[session_cookie(&session_token), clear_mfa_pending_cookie()],
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Json,
        body::to_bytes,
        extract::State,
        http::{HeaderMap, HeaderValue, StatusCode, header},
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{ServerSettings, User};
    use totp_rs::{Builder, Secret};

    use super::*;
    use crate::{
        auth::{RateLimiter, rate_limit::RateLimit},
        state::AppState,
    };

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for auth handler tests: {err}"));

        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    const IP: ClientIp = ClientIp(None);

    fn client(ip: &str) -> ClientIp {
        ClientIp(Some(
            ip.parse().unwrap_or_else(|err| panic!("bad ip: {err}")),
        ))
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    fn cookie_headers(response: &Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
    }

    /// Value of the named cookie in the response's Set-Cookie headers.
    fn set_cookie_value(response: &Response, name: &str) -> Option<String> {
        cookie_headers(response).iter().find_map(|c| {
            let first = c.split(';').next()?;
            let (k, v) = first.split_once('=')?;
            (k == name && !v.is_empty()).then(|| v.to_string())
        })
    }

    async fn seed_user(
        state: &AppState,
        user_id: &str,
        email: &str,
        password: &str,
        role: &str,
        totp_enabled: bool,
        totp_secret_enc: Option<String>,
    ) {
        let password_hash = super::super::helpers::hash_password(password)
            .unwrap_or_else(|err| panic!("failed to hash seeded password: {err}"));
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_secret_enc, totp_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(email)
        .bind(&password_hash)
        .bind(role)
        .bind("2026-01-01T00:00:00Z")
        .bind("2026-01-01T00:00:00Z")
        .bind(totp_secret_enc)
        .bind(if totp_enabled { 1 } else { 0 })
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user: {err}"));
    }

    async fn seed_session(state: &AppState, user_id: &str, token_hint: &str) -> String {
        super::super::helpers::create_session(
            &state.db,
            user_id,
            Some("curl/8.0"),
            Some("127.0.0.1"),
        )
        .await
        .unwrap_or_else(|err| panic!("failed to seed session ({token_hint}): {err}"))
    }

    fn session_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("__Host-tond_session={token}"))
                .unwrap_or_else(|err| panic!("failed to build session cookie header: {err}")),
        );
        headers
    }

    fn mfa_headers(pending: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("__Host-tond_mfa={pending}"))
                .unwrap_or_else(|err| panic!("failed to build mfa cookie header: {err}")),
        );
        headers
    }

    /// Start first-run setup: issue a setup token into a temp file and return it.
    async fn issue_setup_token(state: &AppState) -> String {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = dir.path().join("setup-token");
        setup_token::ensure_setup_token(&state.db, &path)
            .await
            .unwrap_or_else(|err| panic!("ensure_setup_token failed: {err}"));
        crate::auth::secret_file::read_secret_file(&path)
            .unwrap_or_else(|| panic!("setup token file missing"))
    }

    fn setup_request(token: &str, email: &str) -> Json<SetupRequest> {
        Json(SetupRequest {
            token: token.to_string(),
            email: email.to_string(),
            password: "ValidPass123!".to_string(),
        })
    }

    fn totp_secret() -> (String, String) {
        let secret = super::super::helpers::base32_encode(b"12345678901234567890");
        let encrypted = super::super::helpers::encrypt_secret(&secret);
        (secret, encrypted)
    }

    fn current_code(secret_b32: &str) -> String {
        let secret = Secret::try_from_base32(secret_b32)
            .unwrap_or_else(|err| panic!("failed to decode base32 TOTP secret: {err}"));
        Builder::new()
            .with_secret(secret)
            .build()
            .unwrap_or_else(|err| panic!("failed to construct TOTP generator: {err}"))
            .generate_current()
            .to_string()
    }

    async fn user_count(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to count users: {err}"))
    }

    async fn session_count(state: &AppState, user_id: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to count sessions: {err}"))
    }

    // ── Setup ──

    #[tokio::test]
    async fn auth_status_does_not_reveal_whether_setup_is_open() {
        let state = test_state().await;
        let response = auth_status(RateLimit, State(state), HeaderMap::new())
            .await
            .into_response();
        let body = response_json(response).await;
        assert!(
            body["data"].get("needs_setup").is_none(),
            "unauthenticated callers must not learn that setup is open"
        );
        assert_eq!(body["data"]["authenticated"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn setup_requires_the_setup_token() {
        let state = test_state().await;
        let _token = issue_setup_token(&state).await;

        for token in ["", "wrong-token"] {
            let response = setup(
                RateLimit,
                IP,
                State(state.clone()),
                HeaderMap::new(),
                setup_request(token, "admin@example.com"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        assert_eq!(
            user_count(&state).await,
            0,
            "no user may be created without the token"
        );
    }

    #[tokio::test]
    async fn setup_rejects_invalid_email() {
        let state = test_state().await;
        let token = issue_setup_token(&state).await;

        let response = setup(
            RateLimit,
            IP,
            State(state),
            HeaderMap::new(),
            setup_request(&token, "not-an-email"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"], serde_json::json!("Invalid email address"));
    }

    #[tokio::test]
    async fn setup_creates_admin_consumes_token_and_sets_host_cookie() {
        let state = test_state().await;
        let token = issue_setup_token(&state).await;
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, HeaderValue::from_static("curl/8.0"));

        let response = setup(
            RateLimit,
            client("203.0.113.1"),
            State(state.clone()),
            headers,
            setup_request(&token, "admin@example.com"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        let cookies = cookie_headers(&response);
        assert!(
            cookies.iter().any(|c| c.starts_with("__Host-tond_session=")
                && c.contains("Secure")
                && c.contains("HttpOnly")
                && c.contains("SameSite=Strict")
                && c.contains("Path=/")),
            "setup must issue a __Host- Secure HttpOnly SameSite=Strict cookie, got {cookies:?}"
        );
        let body = response_json(response).await;
        assert_eq!(body["data"]["user"]["role"], serde_json::json!("admin"));
        assert_eq!(user_count(&state).await, 1);

        let ip: Option<String> = sqlx::query_scalar("SELECT ip_address FROM sessions")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(ip.as_deref(), Some("203.0.113.1"));

        let again = setup(
            RateLimit,
            IP,
            State(state.clone()),
            HeaderMap::new(),
            setup_request(&token, "second@example.com"),
        )
        .await;
        assert_eq!(
            again.status(),
            StatusCode::FORBIDDEN,
            "a used setup token must not work twice"
        );
        assert_eq!(user_count(&state).await, 1);
    }

    #[tokio::test]
    async fn concurrent_setups_create_exactly_one_admin() {
        // A file-backed DB so the requests really run on separate connections.
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let db = SqlitePool::connect(&format!(
            "sqlite:{}?mode=rwc",
            dir.path().join("tond.db").display()
        ))
        .await
        .unwrap_or_else(|err| panic!("{err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        let mut state = test_state().await;
        state.db = db;
        let token = issue_setup_token(&state).await;

        let mut handles = Vec::new();
        for i in 0..6 {
            let state = state.clone();
            let token = token.clone();
            handles.push(tokio::spawn(async move {
                setup(
                    RateLimit,
                    IP,
                    State(state),
                    HeaderMap::new(),
                    setup_request(&token, &format!("admin{i}@example.com")),
                )
                .await
                .status()
            }));
        }
        let mut created = 0;
        for handle in handles {
            let status = handle.await.unwrap_or_else(|err| panic!("{err}"));
            if status == StatusCode::CREATED {
                created += 1;
            } else {
                assert!(
                    status == StatusCode::FORBIDDEN || status == StatusCode::INTERNAL_SERVER_ERROR,
                    "losing setups must fail, got {status}"
                );
            }
        }
        assert_eq!(created, 1, "exactly one concurrent setup may succeed");
        assert_eq!(user_count(&state).await, 1);
    }

    #[tokio::test]
    async fn setup_check_validates_tokens_without_revealing_state() {
        let state = test_state().await;
        let token = issue_setup_token(&state).await;
        let ok = setup_check(
            RateLimit,
            State(state.clone()),
            Json(SetupCheckRequest { token }),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
        let bad = setup_check(
            RateLimit,
            State(state),
            Json(SetupCheckRequest {
                token: "nope".to_string(),
            }),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::FORBIDDEN);
    }

    // ── Login ──

    #[tokio::test]
    async fn login_rejects_invalid_password() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;

        let response = login(
            RateLimit,
            IP,
            State(state),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "wrong".to_string(),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_sets_hashed_session_when_2fa_is_disabled() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;

        let response = login(
            RateLimit,
            IP,
            State(state.clone()),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "Admin@Example.com ".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let token = set_cookie_value(&response, "__Host-tond_session")
            .unwrap_or_else(|| panic!("login should issue a session cookie"));
        let stored: String = sqlx::query_scalar("SELECT id FROM sessions")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(
            stored,
            hash_key(&token),
            "only the token hash may be stored"
        );
    }

    #[tokio::test]
    async fn login_backoff_ignores_rotating_forwarded_addresses() {
        // Rotating X-Forwarded-For used to give unlimited guesses; now the
        // account itself backs off whatever address the guesses come from.
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;

        let mut last = StatusCode::OK;
        for i in 0..12 {
            last = login(
                RateLimit,
                client(&format!("198.51.100.{i}")),
                State(state.clone()),
                HeaderMap::new(),
                Json(LoginRequest {
                    email: "admin@example.com".to_string(),
                    password: format!("guess-{i}"),
                }),
            )
            .await
            .status();
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);

        // Even the right password is refused while locked out.
        let locked = login(
            RateLimit,
            client("192.0.2.1"),
            State(state),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;
        assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn login_backoff_locks_a_client_guessing_unknown_emails() {
        let state = test_state().await;
        let mut statuses = Vec::new();
        for i in 0..6 {
            statuses.push(
                login(
                    RateLimit,
                    client("203.0.113.50"),
                    State(state.clone()),
                    HeaderMap::new(),
                    Json(LoginRequest {
                        email: format!("nobody{i}@example.com"),
                        password: "x".to_string(),
                    }),
                )
                .await
                .status(),
            );
        }
        assert_eq!(statuses[0], StatusCode::UNAUTHORIZED);
        assert_eq!(statuses[5], StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn login_sets_mfa_pending_cookie_when_2fa_is_enabled() {
        let state = test_state().await;
        let (_, encrypted) = totp_secret();
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            true,
            Some(encrypted),
        )
        .await;

        let response = login(
            RateLimit,
            IP,
            State(state.clone()),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let pending = set_cookie_value(&response, "__Host-tond_mfa")
            .unwrap_or_else(|| panic!("login should issue a pending-2FA cookie"));
        assert_eq!(session_count(&state, "user-1").await, 0);
        let stored: String = sqlx::query_scalar("SELECT id FROM mfa_pending")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(stored, hash_key(&pending));
    }

    async fn pending_login(state: &AppState) -> String {
        let response = login(
            RateLimit,
            IP,
            State(state.clone()),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;
        set_cookie_value(&response, "__Host-tond_mfa")
            .unwrap_or_else(|| panic!("expected a pending-2FA cookie"))
    }

    #[tokio::test]
    async fn totp_verify_caps_wrong_codes_per_pending_login() {
        let state = test_state().await;
        let (secret, encrypted) = totp_secret();
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            true,
            Some(encrypted),
        )
        .await;
        let pending = pending_login(&state).await;

        for _ in 0..MFA_MAX_ATTEMPTS {
            let wrong = totp_verify(
                RateLimit,
                IP,
                State(state.clone()),
                mfa_headers(&pending),
                Json(TotpVerifyRequest {
                    code: "000000".to_string(),
                }),
            )
            .await;
            assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        }

        // The right code no longer helps: this pending login is spent.
        let right = totp_verify(
            RateLimit,
            IP,
            State(state.clone()),
            mfa_headers(&pending),
            Json(TotpVerifyRequest {
                code: current_code(&secret),
            }),
        )
        .await;
        assert_eq!(right.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(session_count(&state, "user-1").await, 0);
    }

    #[tokio::test]
    async fn totp_verify_issues_session_for_valid_code() {
        let state = test_state().await;
        let (secret, encrypted) = totp_secret();
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            true,
            Some(encrypted),
        )
        .await;
        let pending = pending_login(&state).await;

        let response = totp_verify(
            RateLimit,
            IP,
            State(state.clone()),
            mfa_headers(&pending),
            Json(TotpVerifyRequest {
                code: current_code(&secret),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(set_cookie_value(&response, "__Host-tond_session").is_some());
        assert_eq!(session_count(&state, "user-1").await, 1);
    }

    // ── Session endpoints ──

    #[tokio::test]
    async fn me_requires_session_cookie() {
        let state = test_state().await;
        let response = me(RateLimit, State(state), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_returns_current_user_for_valid_session() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;
        let token = seed_session(&state, "user-1", "current").await;

        let response = me(RateLimit, State(state), session_headers(&token)).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["email"],
            serde_json::json!("admin@example.com")
        );
        assert_eq!(
            body["data"]["totp_enabled"],
            serde_json::json!(false),
            "me should report whether 2FA is enabled"
        );
    }

    #[tokio::test]
    async fn me_reports_enabled_two_factor_auth() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-2fa",
            "mfa@example.com",
            "ValidPass123!",
            "admin",
            true,
            None,
        )
        .await;
        let token = seed_session(&state, "user-2fa", "current").await;

        let response = me(RateLimit, State(state), session_headers(&token))
            .await
            .into_response();

        let body = response_json(response).await;
        assert_eq!(
            body["data"]["totp_enabled"],
            serde_json::json!(true),
            "me should report 2FA as enabled so the profile can offer to disable it"
        );
    }

    #[tokio::test]
    async fn legacy_cookie_name_is_not_accepted() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;
        let token = seed_session(&state, "user-1", "current").await;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("tond_session={token}"))
                .unwrap_or_else(|err| panic!("{err}")),
        );
        let response = me(RateLimit, State(state), headers).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_sessions_marks_current_and_hides_tokens() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;
        let token = seed_session(&state, "user-1", "current").await;
        let _other = seed_session(&state, "user-1", "other").await;

        let response = list_sessions(RateLimit, State(state), session_headers(&token)).await;
        let body = response_json(response).await;
        let sessions = body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("expected a session list"));
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions
                .iter()
                .filter(|s| s["is_current"] == serde_json::json!(true))
                .count(),
            1
        );
        assert!(sessions.iter().all(|s| s["id"] != serde_json::json!(token)));
    }

    #[tokio::test]
    async fn create_user_requires_admin_role() {
        let state = test_state().await;
        seed_user(
            &state,
            "member-1",
            "member@example.com",
            "ValidPass123!",
            "member",
            false,
            None,
        )
        .await;
        let token = seed_session(&state, "member-1", "member-session").await;

        let response = create_user(
            RateLimit,
            State(state),
            session_headers(&token),
            Json(CreateUserRequest {
                email: "new@example.com".to_string(),
                password: "StrongPass1!".to_string(),
                role: "member".to_string(),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn change_password_updates_hash_and_revokes_other_sessions() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;
        let current_token = seed_session(&state, "user-1", "current").await;
        let _other_token = seed_session(&state, "user-1", "other").await;

        let response = change_password(
            RateLimit,
            IP,
            State(state.clone()),
            session_headers(&current_token),
            Json(ChangePasswordRequest {
                current_password: "ValidPass123!".to_string(),
                new_password: "ChangedPass123!".to_string(),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);

        let user: User = sqlx::query_as("SELECT * FROM users WHERE id = ?")
            .bind("user-1")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to load user after password change: {err}"));
        assert!(super::super::helpers::verify_password(
            "ChangedPass123!",
            &user.password_hash
        ));

        let remaining: Vec<(String,)> = sqlx::query_as("SELECT id FROM sessions WHERE user_id = ?")
            .bind("user-1")
            .fetch_all(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to load sessions: {err}"));
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, hash_key(&current_token));
    }

    // ── 2FA management (M6) ──

    #[tokio::test]
    async fn totp_setup_requires_the_password() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;
        let token = seed_session(&state, "user-1", "current").await;

        let wrong = totp_setup(
            RateLimit,
            IP,
            State(state.clone()),
            session_headers(&token),
            Json(TotpSetupRequest {
                password: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let secret: Option<String> =
            sqlx::query_scalar("SELECT totp_secret_enc FROM users WHERE id = 'user-1'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"));
        assert!(
            secret.is_none(),
            "no secret may be stored without the password"
        );

        let ok = totp_setup(
            RateLimit,
            IP,
            State(state),
            session_headers(&token),
            Json(TotpSetupRequest {
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn totp_setup_is_refused_while_2fa_is_enabled() {
        let state = test_state().await;
        let (_, encrypted) = totp_secret();
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            true,
            Some(encrypted.clone()),
        )
        .await;
        let token = seed_session(&state, "user-1", "current").await;

        let response = totp_setup(
            RateLimit,
            IP,
            State(state.clone()),
            session_headers(&token),
            Json(TotpSetupRequest {
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let stored: Option<String> =
            sqlx::query_scalar("SELECT totp_secret_enc FROM users WHERE id = 'user-1'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(
            stored,
            Some(encrypted),
            "the active secret must not be replaced"
        );
    }

    #[tokio::test]
    async fn enabling_and_disabling_2fa_revoke_other_sessions() {
        let state = test_state().await;
        seed_user(
            &state,
            "user-1",
            "admin@example.com",
            "ValidPass123!",
            "admin",
            false,
            None,
        )
        .await;
        let token = seed_session(&state, "user-1", "current").await;
        let _other = seed_session(&state, "user-1", "other").await;

        let setup_response = totp_setup(
            RateLimit,
            IP,
            State(state.clone()),
            session_headers(&token),
            Json(TotpSetupRequest {
                password: "ValidPass123!".to_string(),
            }),
        )
        .await;
        let secret = response_json(setup_response).await["data"]["secret"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let confirm = totp_confirm(
            RateLimit,
            IP,
            State(state.clone()),
            session_headers(&token),
            Json(TotpConfirmRequest {
                code: current_code(&secret),
            }),
        )
        .await;
        assert_eq!(confirm.status(), StatusCode::OK);
        assert_eq!(
            session_count(&state, "user-1").await,
            1,
            "enabling 2FA must sign out the other sessions"
        );

        let _another = seed_session(&state, "user-1", "another").await;
        // The code for the current step was consumed by confirm; use a backup code.
        let backup: String = {
            use sha2::Digest;
            let code = "0123456789abcdef";
            sqlx::query("DELETE FROM totp_backup_codes WHERE user_id = 'user-1'")
                .execute(&state.db)
                .await
                .unwrap_or_else(|err| panic!("{err}"));
            sqlx::query(
                "INSERT INTO totp_backup_codes (id, user_id, code_hash, created_at) VALUES ('b1', 'user-1', ?, 'now')",
            )
            .bind(hex::encode(sha2::Sha256::digest(code.as_bytes())))
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("{err}"));
            code.to_string()
        };
        let disable = totp_disable(
            RateLimit,
            IP,
            State(state.clone()),
            session_headers(&token),
            Json(TotpDisableRequest {
                password: "ValidPass123!".to_string(),
                code: backup,
            }),
        )
        .await;
        assert_eq!(disable.status(), StatusCode::OK);
        assert_eq!(
            session_count(&state, "user-1").await,
            1,
            "disabling 2FA must sign out the other sessions"
        );
    }
}
