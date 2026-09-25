// handlers/auth/handlers.rs — HTTP handler functions for the auth routes

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use chrono::Utc;
use serde_json::json;
use tond_common::*;
use uuid::Uuid;

use crate::{auth::RateLimit, state::AppState};

use super::helpers::{
    aes_key, base32_encode, clear_session_cookie, create_session, decrypt_secret, encrypt_secret,
    extract_ip, extract_user_agent, hash_password, session_cookie, use_backup_code,
    verify_password, verify_totp,
};
use super::session::{extract_session_token, validate_session};

// ── GET /api/auth/status ──
// Returns whether setup is needed (no users exist) and whether the caller
// is authenticated. No auth required.

pub async fn auth_status(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    let needs_setup = user_count.0 == 0;

    // Check if caller has a valid session
    let user = if let Some(token) = extract_session_token(&headers) {
        validate_session(&state.db, &token).await.unwrap_or(None)
    } else {
        None
    };

    let response = json!({
        "needs_setup": needs_setup,
        "authenticated": user.is_some(),
        "user": user.map(UserResponse::from),
    });

    (StatusCode::OK, Json(ApiResponse::ok(response))).into_response()
}

// ── POST /api/auth/setup ──
// First-run admin account creation. Only works when no users exist.

pub async fn setup(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetupRequest>,
) -> impl IntoResponse {
    let ua = extract_user_agent(&headers);
    let ip = extract_ip(&headers);
    // Check if any users exist
    let count: (i64,) = match sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    if count.0 > 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::err(
                "Setup already completed. Use login instead.",
            )),
        )
            .into_response();
    }

    // Validate input
    if req.email.is_empty() || !req.email.contains('@') {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid email address")),
        )
            .into_response();
    }

    let pwd = &req.password;
    if pwd.len() < 12 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "Password must be at least 12 characters",
            )),
        )
            .into_response();
    }
    if !pwd.chars().any(|c| c.is_uppercase()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "Password must contain at least one uppercase letter",
            )),
        )
            .into_response();
    }
    if !pwd.chars().any(|c| c.is_lowercase()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "Password must contain at least one lowercase letter",
            )),
        )
            .into_response();
    }
    if !pwd.chars().any(|c| c.is_ascii_digit()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "Password must contain at least one number",
            )),
        )
            .into_response();
    }
    if !pwd.chars().any(|c| !c.is_alphanumeric()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "Password must contain at least one special character",
            )),
        )
            .into_response();
    }

    // Hash password
    let password_hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to hash password")),
            )
                .into_response();
        }
    };

    let now = Utc::now().to_rfc3339();
    let user_id = Uuid::new_v4().to_string();

    // Insert admin user
    if let Err(e) = sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, 'admin', ?, ?)",
    )
    .bind(&user_id)
    .bind(&req.email)
    .bind(&password_hash)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response();
    }

    // Create session
    let session_token =
        match create_session(&state.db, &user_id, ua.as_deref(), ip.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::err(e.to_string())),
                )
                    .into_response();
            }
        };

    let user_resp = UserResponse {
        id: user_id,
        email: req.email,
        role: "admin".to_string(),
        created_at: now,
    };

    let mut response = (
        StatusCode::CREATED,
        Json(ApiResponse::ok(json!({
            "user": user_resp,
            "message": "Admin account created",
        }))),
    )
        .into_response();

    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&session_token, 86400).parse().unwrap(),
    );

    response
}

// ── POST /api/auth/login ──
// When the user has 2FA enabled, this endpoint responds with a 200 and
// a short-lived `tond_mfa_pending` cookie instead of the full session.
// The client must then POST to /api/auth/2fa/verify with the TOTP code
// to receive the real `tond_session` cookie.

pub async fn login(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    use tond_common::generate_raw_key;

    let ua = extract_user_agent(&headers);
    let ip = extract_ip(&headers);

    // Look up user by email
    let user = match sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = ?")
        .bind(&req.email)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Invalid email or password")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    // Verify password
    if !verify_password(&req.password, &user.password_hash) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Invalid email or password")),
        )
            .into_response();
    }

    // If 2FA is enabled, issue a short-lived MFA-pending token instead of a
    // full session cookie. The client must complete the TOTP second factor.
    if user.totp_enabled {
        let pending_id = generate_raw_key();
        let now = Utc::now();
        let expires = now + chrono::Duration::minutes(10);

        if let Err(e) = sqlx::query(
            "INSERT INTO mfa_pending (id, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&pending_id)
        .bind(&user.id)
        .bind(expires.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }

        // Store ua/ip for use by the verify endpoint (we put them in a temp
        // field stored alongside the pending token).
        // For now we store them in the pending table but we don't need an
        // extra column — the verify endpoint will receive the same headers.

        let mfa_cookie = format!(
            "tond_mfa_pending={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=600",
            pending_id
        );

        let mut response = (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "mfa_required": true }))),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::SET_COOKIE, mfa_cookie.parse().unwrap());
        return response;
    }

    // Create a full session
    let session_token =
        match create_session(&state.db, &user.id, ua.as_deref(), ip.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::err(e.to_string())),
                )
                    .into_response();
            }
        };

    let user_resp = UserResponse::from(user);

    let mut response = (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "user": user_resp }))),
    )
        .into_response();

    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&session_token, 86400).parse().unwrap(),
    );

    response
}

// ── POST /api/auth/logout ──

pub async fn logout(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Delete session from DB if present
    if let Some(token) = extract_session_token(&headers) {
        let _ = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&token)
            .execute(&state.db)
            .await;
    }

    let mut response = (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "Logged out" }))),
    )
        .into_response();

    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_session_cookie().parse().unwrap());

    response
}

// ── GET /api/auth/me ──
// Returns the current user. Requires a valid session cookie.

pub async fn me(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    match validate_session(&state.db, &token).await {
        Ok(Some(user)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(UserResponse::from(user))),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Session expired or invalid")),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response(),
    }
}

// ── POST /api/auth/users ──
// Admin-only: create a new user account.

pub async fn create_user(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    // Verify caller is an authenticated admin
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let caller = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    if caller.role != "admin" {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::err("Only admins can create users")),
        )
            .into_response();
    }

    // Validate input
    if req.email.is_empty() || !req.email.contains('@') {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Invalid email address")),
        )
            .into_response();
    }

    if req.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "Password must be at least 8 characters",
            )),
        )
            .into_response();
    }

    let valid_roles = ["admin", "member"];
    if !valid_roles.contains(&req.role.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("Role must be 'admin' or 'member'")),
        )
            .into_response();
    }

    // Hash password
    let password_hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to hash password")),
            )
                .into_response();
        }
    };

    let now = Utc::now().to_rfc3339();
    let user_id = Uuid::new_v4().to_string();

    match sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(&req.email)
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
                email: req.email,
                role: req.role,
                created_at: now,
            };
            (
                StatusCode::CREATED,
                Json(ApiResponse::ok(user_resp)),
            )
                .into_response()
        }
        Err(e) => {
            let msg = if e.to_string().contains("UNIQUE") {
                format!("User with email '{}' already exists", req.email)
            } else {
                e.to_string()
            };
            (StatusCode::CONFLICT, Json(ApiResponse::<()>::err(msg))).into_response()
        }
    }
}

// ── GET /api/auth/users ──
// Admin-only: list all users.

pub async fn list_users(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Verify caller is an authenticated admin
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let caller = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    if caller.role != "admin" {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::err("Only admins can list users")),
        )
            .into_response();
    }

    match sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at ASC")
        .fetch_all(&state.db)
        .await
    {
        Ok(users) => {
            let responses: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
            (StatusCode::OK, Json(ApiResponse::ok(responses))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 7.1 — Password Management
// ─────────────────────────────────────────────────────────────────────────────

// ── POST /api/auth/change-password ──
// Authenticated via session cookie only (not API key — only human users have
// passwords). Verifies the current password, re-hashes the new one, updates
// the DB, and invalidates all sessions except the current one.

pub async fn change_password(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    // Verify current password
    if !verify_password(&req.current_password, &user.password_hash) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Current password is incorrect")),
        )
            .into_response();
    }

    if req.new_password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err(
                "New password must be at least 8 characters",
            )),
        )
            .into_response();
    }

    let new_hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to hash password")),
            )
                .into_response();
        }
    };

    let now = Utc::now().to_rfc3339();

    // Update password hash
    if let Err(e) = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(&new_hash)
        .bind(&now)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response();
    }

    // Invalidate all sessions except the current one
    let _ = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id != ?")
        .bind(&user.id)
        .bind(&token)
        .execute(&state.db)
        .await;

    (
        StatusCode::OK,
        Json(ApiResponse::ok(
            json!({ "message": "Password changed successfully" }),
        )),
    )
        .into_response()
}

// ── GET /api/auth/sessions ──
// Returns all active sessions for the current user so they can manage
// (revoke) them from the dashboard "Security" tab.

pub async fn list_sessions(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

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
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    let responses: Vec<SessionResponse> = sessions
        .into_iter()
        .map(|s| SessionResponse {
            is_current: s.id == token,
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
// Revoke a specific session. Users can only revoke their own sessions.

pub async fn revoke_session(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    // Delete the session only if it belongs to the current user
    match sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
        .bind(&session_id)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::err("Session not found")),
        )
            .into_response(),
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::ok(json!({ "message": "Session revoked" }))),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response(),
    }
}

// ── DELETE /api/auth/sessions ──
// Sign out everywhere: revoke all sessions except the current one.

pub async fn revoke_all_sessions(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    match sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id != ?")
        .bind(&user.id)
        .bind(&token)
        .execute(&state.db)
        .await
    {
        Ok(r) => (
            StatusCode::OK,
            Json(ApiResponse::ok(
                json!({ "message": format!("Revoked {} session(s)", r.rows_affected()) }),
            )),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 7.1 — TOTP Two-Factor Authentication
// ─────────────────────────────────────────────────────────────────────────────

// ── POST /api/auth/2fa/setup ──
// Generate a fresh TOTP secret, store it encrypted (but NOT yet enabled),
// and return the secret + otpauth:// URI so the client can display a QR code.
// 2FA is NOT active until /api/auth/2fa/confirm is called successfully.

pub async fn totp_setup(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    // Generate a random base32 TOTP secret (20 bytes = 160 bits)
    use rand::Rng;
    use totp_rs::Builder;
    let mut secret_bytes = [0u8; 20];
    rand::rng().fill_bytes(&mut secret_bytes);
    let secret_b32 = base32_encode(&secret_bytes);

    // Build a TOTP object via totp-rs so we can use its otpauth URL generator
    // and its built-in QR code renderer (no external HTTP call needed).
    let totp = match Builder::new()
        .with_secret(secret_bytes)
        .with_issuer(Some("Tond"))
        .with_account_name(user.email.as_str())
        .build()
    {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to build TOTP object")),
            )
                .into_response();
        }
    };

    let otpauth_uri = match totp.to_url() {
        Ok(url) => url,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to build TOTP URL")),
            )
                .into_response();
        }
    };

    // Generate the QR code as a base64-encoded PNG (server-side, no external request).
    let qr_code = match totp.to_qr_base64() {
        Ok(b64) => b64,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to generate QR code")),
            )
                .into_response();
        }
    };

    // Encrypt and persist the secret (2FA not yet enabled — pending confirmation)
    let key = aes_key();
    let encrypted = encrypt_secret(&secret_b32, &key);

    let now = Utc::now().to_rfc3339();
    if let Err(e) = sqlx::query("UPDATE users SET totp_secret_enc = ?, updated_at = ? WHERE id = ?")
        .bind(&encrypted)
        .bind(&now)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response();
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
// On success: set totp_enabled = true and generate 10 one-time backup codes.

pub async fn totp_confirm(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpConfirmRequest>,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    // Fetch and decrypt the pending TOTP secret
    let encrypted = match &user.totp_secret_enc {
        Some(e) => e.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::err(
                    "2FA setup not started. Call POST /api/auth/2fa/setup first.",
                )),
            )
                .into_response();
        }
    };

    let key = aes_key();
    let secret_b32 = match decrypt_secret(&encrypted, &key) {
        Some(s) => s,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("Failed to decrypt TOTP secret")),
            )
                .into_response();
        }
    };

    // Verify the submitted code
    if !verify_totp(&secret_b32, &req.code) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Invalid TOTP code")),
        )
            .into_response();
    }

    // Activate 2FA
    let now = Utc::now().to_rfc3339();
    if let Err(e) = sqlx::query("UPDATE users SET totp_enabled = 1, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&user.id)
        .execute(&state.db)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::err(e.to_string())),
        )
            .into_response();
    }

    // Delete any old backup codes and generate 10 new ones
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

        // Hash for storage
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
// Clears totp_secret_enc, sets totp_enabled = false, deletes backup codes.

pub async fn totp_disable(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpDisableRequest>,
) -> impl IntoResponse {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Not authenticated")),
            )
                .into_response();
        }
    };

    let user = match validate_session(&state.db, &token).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err("Session expired or invalid")),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err(e.to_string())),
            )
                .into_response();
        }
    };

    if !verify_password(&req.password, &user.password_hash) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Current password is incorrect")),
        )
            .into_response();
    }

    if !user.totp_enabled {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::err("2FA is not enabled")),
        )
            .into_response();
    }

    // Verify TOTP code (or backup code)
    let encrypted = user.totp_secret_enc.as_deref().unwrap_or("");
    let key = aes_key();
    let secret_b32 = decrypt_secret(encrypted, &key).unwrap_or_default();
    let code_valid = verify_totp(&secret_b32, &req.code)
        || use_backup_code(&state.db, &user.id, &req.code).await;

    if !code_valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Invalid TOTP or backup code")),
        )
            .into_response();
    }

    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        "UPDATE users SET totp_enabled = 0, totp_secret_enc = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(&user.id)
    .execute(&state.db)
    .await;

    let _ = sqlx::query("DELETE FROM totp_backup_codes WHERE user_id = ?")
        .bind(&user.id)
        .execute(&state.db)
        .await;

    (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "message": "2FA disabled" }))),
    )
        .into_response()
}

// ── POST /api/auth/2fa/verify ──
// Second factor after login when 2FA is enabled.
// Validates the `tond_mfa_pending` cookie, checks the TOTP code (or backup code),
// and on success issues a full `tond_session` cookie.

pub async fn totp_verify(
    _rate: RateLimit,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<TotpVerifyRequest>,
) -> impl IntoResponse {
    let ua = extract_user_agent(&headers);
    let ip = extract_ip(&headers);

    // Extract the MFA pending token from the cookie jar
    let pending_id = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                let c = c.trim();
                c.strip_prefix("tond_mfa_pending=")
                    .filter(|v| !v.is_empty())
                    .map(|v| v.to_string())
            })
        });

    let pending_id = match pending_id {
        Some(p) => p,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err(
                    "No MFA pending token. Please log in first.",
                )),
            )
                .into_response();
        }
    };

    let now = Utc::now().to_rfc3339();

    // Look up the pending MFA token and get the user
    let row: Option<(String,)> =
        sqlx::query_as("SELECT user_id FROM mfa_pending WHERE id = ? AND expires_at > ?")
            .bind(&pending_id)
            .bind(&now)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    let user_id = match row {
        Some((uid,)) => uid,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiResponse::<()>::err(
                    "MFA pending token expired or invalid",
                )),
            )
                .into_response();
        }
    };

    let user = match sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(u)) => u,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::err("User not found")),
            )
                .into_response();
        }
    };

    // Verify TOTP code or backup code
    let encrypted = user.totp_secret_enc.as_deref().unwrap_or("");
    let key = aes_key();
    let secret_b32 = decrypt_secret(encrypted, &key).unwrap_or_default();
    let code_valid = verify_totp(&secret_b32, &req.code)
        || use_backup_code(&state.db, &user.id, &req.code).await;

    if !code_valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::err("Invalid TOTP or backup code")),
        )
            .into_response();
    }

    // Delete the pending token (consume it)
    let _ = sqlx::query("DELETE FROM mfa_pending WHERE id = ?")
        .bind(&pending_id)
        .execute(&state.db)
        .await;

    // Issue a full session
    let session_token =
        match create_session(&state.db, &user.id, ua.as_deref(), ip.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::err(e.to_string())),
                )
                    .into_response();
            }
        };

    let user_resp = UserResponse::from(user);

    let clear_mfa = "tond_mfa_pending=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0".to_string();

    let mut response = (
        StatusCode::OK,
        Json(ApiResponse::ok(json!({ "user": user_resp }))),
    )
        .into_response();

    let headers_mut = response.headers_mut();
    headers_mut.insert(
        header::SET_COOKIE,
        session_cookie(&session_token, 86400).parse().unwrap(),
    );
    headers_mut.append(header::SET_COOKIE, clear_mfa.parse().unwrap());

    response
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Json,
        body::to_bytes,
        extract::State,
        http::{HeaderMap, HeaderValue, StatusCode, header},
        response::IntoResponse,
    };
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;
    use tond_common::{ServerSettings, User};

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

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    fn cookie_headers(response: &axum::response::Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
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
            HeaderValue::from_str(&format!("tond_session={token}"))
                .unwrap_or_else(|err| panic!("failed to build session cookie header: {err}")),
        );
        headers
    }

    #[tokio::test]
    async fn setup_rejects_invalid_email() {
        let state = test_state().await;

        let response = setup(
            RateLimit,
            State(state),
            HeaderMap::new(),
            Json(SetupRequest {
                email: "not-an-email".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "setup should reject malformed email addresses"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            serde_json::json!("Invalid email address"),
            "setup should return the documented invalid email error"
        );
    }

    #[tokio::test]
    async fn setup_creates_admin_user_and_sets_session_cookie() {
        let state = test_state().await;
        let mut headers = HeaderMap::new();
        headers.insert(header::USER_AGENT, HeaderValue::from_static("curl/8.0"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.1"));

        let response = setup(
            RateLimit,
            State(state.clone()),
            headers,
            Json(SetupRequest {
                email: "admin@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "setup should create the initial admin user"
        );
        let cookies = cookie_headers(&response);
        assert!(
            cookies
                .iter()
                .any(|cookie| cookie.contains("tond_session=")),
            "successful setup should issue a session cookie"
        );

        let body = response_json(response).await;
        assert_eq!(
            body["data"]["user"]["role"],
            serde_json::json!("admin"),
            "setup should create the initial account with the admin role"
        );

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to count users after setup: {err}"));
        assert_eq!(
            count.0, 1,
            "setup should create exactly one user on first run"
        );
    }

    #[tokio::test]
    async fn setup_rejects_second_run_after_user_exists() {
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

        let response = setup(
            RateLimit,
            State(state),
            HeaderMap::new(),
            Json(SetupRequest {
                email: "new@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "setup should be disabled once at least one user exists"
        );
    }

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
            State(state),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "wrong".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "login should reject incorrect passwords"
        );
    }

    #[tokio::test]
    async fn login_sets_session_cookie_when_2fa_is_disabled() {
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
            State(state.clone()),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "login should succeed for valid credentials"
        );
        let cookies = cookie_headers(&response);
        assert!(
            cookies
                .iter()
                .any(|cookie| cookie.contains("tond_session=")),
            "login should issue a full session cookie when 2FA is disabled"
        );

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to count sessions after login: {err}"));
        assert_eq!(
            count.0, 1,
            "login should create one session record for a successful login"
        );
    }

    #[tokio::test]
    async fn login_sets_mfa_pending_cookie_when_2fa_is_enabled() {
        let state = test_state().await;
        let secret = super::super::helpers::base32_encode(b"12345678901234567890");
        let encrypted =
            super::super::helpers::encrypt_secret(&secret, &super::super::helpers::aes_key());
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
            State(state.clone()),
            HeaderMap::new(),
            Json(LoginRequest {
                email: "admin@example.com".to_string(),
                password: "ValidPass123!".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "login should return 200 when first-factor authentication succeeds for a 2FA-enabled user"
        );
        let cookies = cookie_headers(&response);
        assert!(
            cookies
                .iter()
                .any(|cookie| cookie.contains("tond_mfa_pending=")),
            "login should issue an MFA-pending cookie instead of a full session when 2FA is enabled"
        );

        let session_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to count sessions after 2FA login: {err}"));
        assert_eq!(
            session_count.0, 0,
            "login should not create a full session before the TOTP second factor is verified"
        );
    }

    #[tokio::test]
    async fn me_requires_session_cookie() {
        let state = test_state().await;

        let response = me(RateLimit, State(state), HeaderMap::new())
            .await
            .into_response();

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "me should require a valid session cookie"
        );
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

        let response = me(RateLimit, State(state), session_headers(&token))
            .await
            .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "me should return the authenticated user for a valid session"
        );
        let body = response_json(response).await;
        assert_eq!(
            body["data"]["email"],
            serde_json::json!("admin@example.com"),
            "me should return the current authenticated user's email"
        );
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
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "create_user should only allow authenticated admins to create accounts"
        );
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
            State(state.clone()),
            session_headers(&current_token),
            Json(ChangePasswordRequest {
                current_password: "ValidPass123!".to_string(),
                new_password: "ChangedPass123!".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "change_password should succeed when the current password is correct"
        );

        let user: User = sqlx::query_as("SELECT * FROM users WHERE id = ?")
            .bind("user-1")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to load user after password change: {err}"));
        assert!(
            super::super::helpers::verify_password("ChangedPass123!", &user.password_hash),
            "change_password should persist the new password hash"
        );

        let remaining_sessions: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM sessions WHERE user_id = ?")
                .bind("user-1")
                .fetch_all(&state.db)
                .await
                .unwrap_or_else(|err| {
                    panic!("failed to load sessions after password change: {err}")
                });
        assert_eq!(
            remaining_sessions.len(),
            1,
            "change_password should revoke all other active sessions"
        );
        assert_eq!(
            remaining_sessions[0].0, current_token,
            "change_password should preserve the current session while revoking the others"
        );
    }
}
