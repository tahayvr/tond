// handlers/env.rs — app environment variables (masked reads, patch writes)
//
// Env var values are secrets, so reads never return them in bulk and writes
// never replace the whole map:
//
//   GET   /api/apps/{id}/env               — keys with masked values + preview config
//   GET   /api/apps/{id}/env/{key}         — reveal one value (admin only, audit-logged)
//   PATCH /api/apps/{id}/env               — {set: {K: V}, unset: [K]}
//   GET   /api/apps/{id}/preview-env/{key} — reveal one preview override (admin only, audit-logged)
//   PATCH /api/apps/{id}/preview-env       — {set, unset, share, unshare}
//
// Previews never inherit the app's environment: they get their own overrides
// plus only the app variables an admin explicitly shares (shared_keys).
//
// Changes are stored immediately and reach the container on the next deploy.
// Each patch is applied inside one IMMEDIATE transaction so two concurrent
// edits of different keys can't overwrite each other.

use std::collections::{BTreeMap, BTreeSet};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use super::fetch_app;
use crate::{
    audit_log::write_audit_log,
    auth::{Authz, RateLimit, scope},
    error::AppError,
    log_scrub, secrets,
    state::AppState,
};

// ── Pure helpers ──

fn parse_key_list(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn validate_keys<'a>(keys: impl IntoIterator<Item = &'a String>) -> Result<(), AppError> {
    for key in keys {
        if !is_valid_env_key(key) {
            return Err(AppError(TondError::InvalidInput(format!(
                "'{key}' is not a valid variable name: use letters, digits and underscores, \
                 not starting with a digit"
            ))));
        }
    }
    Ok(())
}

/// Apply `set` then `unset` to `current`, rejecting invalid keys and keys that
/// appear in both lists.
fn apply_patch(
    mut current: BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
    unset: &[String],
) -> Result<BTreeMap<String, String>, AppError> {
    validate_keys(set.keys())?;
    validate_keys(unset)?;
    if let Some(key) = unset.iter().find(|k| set.contains_key(*k)) {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{key}' is both set and unset in the same request"
        ))));
    }
    for (key, value) in set {
        current.insert(key.clone(), value.clone());
    }
    for key in unset {
        current.remove(key);
    }
    Ok(current)
}

/// Apply share/unshare to the list of app variables previews receive, keeping
/// it sorted and free of duplicates.
fn apply_sharing(
    current: Vec<String>,
    share: &[String],
    unshare: &[String],
) -> Result<Vec<String>, AppError> {
    validate_keys(share)?;
    validate_keys(unshare)?;
    if let Some(key) = unshare.iter().find(|k| share.contains(k)) {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{key}' is both shared and unshared in the same request"
        ))));
    }
    let mut keys: BTreeSet<String> = current.into_iter().collect();
    keys.extend(share.iter().cloned());
    for key in unshare {
        keys.remove(key);
    }
    Ok(keys.into_iter().collect())
}

fn env_response(app: &App) -> Result<AppEnvResponse, AppError> {
    let vars = secrets::app_env(app)?;
    let overrides = secrets::app_preview_env(app)?;
    Ok(AppEnvResponse {
        vars: masked_env_entries(&vars),
        preview: PreviewEnvResponse {
            enabled: app.preview_enabled != 0,
            shared_keys: parse_key_list(&app.preview_shared_keys),
            overrides: masked_env_entries(&overrides),
        },
    })
}

async fn fetch_app_in_tx(tx: &mut sqlx::SqliteConnection, app_id: &str) -> Result<App, AppError> {
    let app = sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id = ?")
        .bind(app_id)
        .fetch_optional(&mut *tx)
        .await?;
    app.ok_or_else(|| AppError(TondError::AppNotFound(app_id.to_string())))
}

// ── Handlers ──

pub async fn get_env(
    _rate: RateLimit,
    _auth: Authz<scope::EnvReadNames>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(env_response(&app)?))))
}

pub async fn reveal_env_var(
    _rate: RateLimit,
    auth: Authz<scope::EnvReadValues>,
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let vars = secrets::app_env(&app)?;
    let value = vars
        .get(&key)
        .cloned()
        .ok_or_else(|| AppError(TondError::NotFound(format!("variable '{key}'"))))?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "env_reveal",
        &auth.audit_actor(),
        Some(&format!("key={key}")),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(EnvVarValueResponse { key, value })),
    ))
}

pub async fn patch_env(
    _rate: RateLimit,
    auth: Authz<scope::EnvWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchEnvRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.set.is_empty() && req.unset.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "nothing to change: send at least one variable to set or unset".to_string(),
        )));
    }
    let app_id = fetch_app(&state.db, &id).await?.id;

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let app = fetch_app_in_tx(&mut tx, &app_id).await?;
    let next = apply_patch(secrets::app_env(&app)?, &req.set, &req.unset)?;
    sqlx::query("UPDATE apps SET env_vars = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(secrets::seal_app_env(&next)?)
        .bind(&app.id)
        .execute(&mut *tx)
        .await?;
    let updated = fetch_app_in_tx(&mut tx, &app.id).await?;
    tx.commit().await?;
    log_scrub::forget_app(&updated.id);

    // Record which keys changed, never their values.
    let detail = format!(
        "set={} unset={}",
        req.set.keys().cloned().collect::<Vec<_>>().join(","),
        req.unset.join(",")
    );
    write_audit_log(
        &state.db,
        Some(&updated.id),
        Some(&updated.name),
        "env_update",
        &auth.audit_actor(),
        Some(&detail),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(env_response(&updated)?)),
    ))
}

pub async fn reveal_preview_env_var(
    _rate: RateLimit,
    auth: Authz<scope::EnvReadValues>,
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let overrides = secrets::app_preview_env(&app)?;
    let value = overrides
        .get(&key)
        .cloned()
        .ok_or_else(|| AppError(TondError::NotFound(format!("preview override '{key}'"))))?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "preview_env_reveal",
        &auth.audit_actor(),
        Some(&format!("key={key}")),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(EnvVarValueResponse { key, value })),
    ))
}

pub async fn patch_preview_env(
    _rate: RateLimit,
    auth: Authz<scope::EnvWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchPreviewEnvRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.set.is_empty() && req.unset.is_empty() && req.share.is_empty() && req.unshare.is_empty()
    {
        return Err(AppError(TondError::InvalidInput(
            "nothing to change: send at least one override or sharing change".to_string(),
        )));
    }
    let app_id = fetch_app(&state.db, &id).await?.id;

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let app = fetch_app_in_tx(&mut tx, &app_id).await?;
    let overrides = apply_patch(secrets::app_preview_env(&app)?, &req.set, &req.unset)?;
    let shared = apply_sharing(
        parse_key_list(&app.preview_shared_keys),
        &req.share,
        &req.unshare,
    )?;
    let shared_json =
        serde_json::to_string(&shared).map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    sqlx::query(
        "UPDATE apps SET preview_env_vars = ?, preview_shared_keys = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(secrets::seal_app_preview_env(&overrides)?)
    .bind(shared_json)
    .bind(&app.id)
    .execute(&mut *tx)
    .await?;
    let updated = fetch_app_in_tx(&mut tx, &app.id).await?;
    tx.commit().await?;
    log_scrub::forget_app(&updated.id);

    let detail = format!(
        "set={} unset={} share={} unshare={}",
        req.set.keys().cloned().collect::<Vec<_>>().join(","),
        req.unset.join(","),
        req.share.join(","),
        req.unshare.join(",")
    );
    write_audit_log(
        &state.db,
        Some(&updated.id),
        Some(&updated.name),
        "preview_env_update",
        &auth.audit_actor(),
        Some(&detail),
    )
    .await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(env_response(&updated)?)),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::to_bytes;
    use bollard::{API_DEFAULT_VERSION, Docker};
    use sqlx::SqlitePool;
    use tokio::sync::RwLock;

    use axum::{extract::FromRequestParts, http::header};

    use super::*;
    use crate::auth::Principal;
    use crate::auth::{LocalSocket, RateLimiter};
    use crate::handlers::auth::SESSION_COOKIE;

    async fn test_state() -> AppState {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations for env handler tests: {err}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:2375", 5, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    fn seal(purpose: crate::secret_key::Purpose, json: &str) -> String {
        let map: BTreeMap<String, String> =
            serde_json::from_str(json).unwrap_or_else(|err| panic!("bad fixture JSON: {err}"));
        crate::secret_key::seal_map(purpose, &map).unwrap_or_else(|err| panic!("seal: {err}"))
    }

    async fn seed_app(state: &AppState, env: &str, preview_env: &str, shared: &str) -> App {
        let env = seal(crate::secret_key::Purpose::AppEnv, env);
        let preview_env = seal(crate::secret_key::Purpose::AppPreviewEnv, preview_env);
        let app = App::new("demo".to_string(), "image".to_string(), None, None, 3000);
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, preview_env_vars, preview_shared_keys, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.source_type)
        .bind(app.container_port)
        .bind(&app.status)
        .bind(&env)
        .bind(&preview_env)
        .bind(shared)
        .bind(&app.created_at)
        .bind(&app.updated_at)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed app: {err}"));
        app
    }

    async fn stored_env(state: &AppState, id: &str) -> BTreeMap<String, String> {
        let raw: Option<String> = sqlx::query_scalar("SELECT env_vars FROM apps WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to read env_vars: {err}"));
        crate::secret_key::open_map(crate::secret_key::Purpose::AppEnv, raw.as_deref())
            .unwrap_or_else(|err| panic!("bad stored env: {err}"))
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    /// Seed a user with `role` and a live session; returns the raw session token.
    async fn seed_session_for(state: &AppState, role: &str) -> String {
        let user_id = format!("user-{role}");
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, created_at, updated_at, totp_enabled) VALUES (?, ?, 'x', ?, ?, ?, 0)",
        )
        .bind(&user_id)
        .bind(format!("{role}@example.com"))
        .bind(role)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed user: {err}"));
        let token = format!("token-{role}");
        sqlx::query(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(hash_key(&token))
        .bind(&user_id)
        .bind((now + chrono::Duration::hours(1)).to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to seed session: {err}"));
        token
    }

    /// HTTP status the reveal route's `Authz<scope::EnvReadValues>` gives these headers (200 = passes).
    async fn reveal_gate(state: &AppState, headers: &[(header::HeaderName, String)]) -> u16 {
        let mut builder = axum::http::Request::builder().uri("/api/apps/x/env/K");
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        let (mut parts, _) = builder
            .body(())
            .unwrap_or_else(|err| panic!("failed to build request: {err}"))
            .into_parts();
        match Authz::<scope::EnvReadValues>::from_request_parts(&mut parts, state).await {
            Ok(_) => 200,
            Err(response) => response.status().as_u16(),
        }
    }

    async fn token_with(state: &AppState, scopes: &[&str]) -> String {
        crate::auth::tokens::create_token(
            &state.db,
            &Principal::local_socket(),
            &CreateTokenRequest {
                name: "e2e".into(),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                ..CreateTokenRequest::default()
            },
        )
        .await
        .unwrap_or_else(|err| panic!("failed to create token: {err}"))
        .secret
    }

    #[tokio::test]
    async fn only_callers_with_env_read_values_pass_the_reveal_gate() {
        let state = test_state().await;
        let admin = seed_session_for(&state, "admin").await;
        let deployer = seed_session_for(&state, "deployer").await;
        let cookie = |t: &str| (header::COOKIE, format!("{SESSION_COOKIE}={t}"));
        let bearer = |t: &str| (header::AUTHORIZATION, format!("Bearer {t}"));

        assert_eq!(
            reveal_gate(&state, &[cookie(&admin)]).await,
            200,
            "admin session"
        );
        assert_eq!(
            reveal_gate(&state, &[cookie(&deployer)]).await,
            403,
            "deployers can write env vars but not read values"
        );
        let reader = token_with(&state, &["env:read-values"]).await;
        assert_eq!(reveal_gate(&state, &[bearer(&reader)]).await, 200);
        let names_only = token_with(&state, &["env:read-names"]).await;
        assert_eq!(reveal_gate(&state, &[bearer(&names_only)]).await, 403);
        assert_eq!(reveal_gate(&state, &[bearer("bogus")]).await, 401);
        assert_eq!(reveal_gate(&state, &[]).await, 401, "no credential");

        let (mut parts, _) = axum::http::Request::builder()
            .extension(LocalSocket)
            .body(())
            .unwrap_or_else(|err| panic!("failed to build request: {err}"))
            .into_parts();
        assert!(
            Authz::<scope::EnvReadValues>::from_request_parts(&mut parts, &state)
                .await
                .is_ok(),
            "the root-only Unix socket holds every scope"
        );
    }

    #[test]
    fn apply_patch_sets_and_unsets_without_touching_other_keys() {
        let current = BTreeMap::from([
            ("KEEP".to_string(), "1".to_string()),
            ("CHANGE".to_string(), "old".to_string()),
            ("DROP".to_string(), "x".to_string()),
        ]);
        let set = BTreeMap::from([
            ("CHANGE".to_string(), "new".to_string()),
            ("ADD".to_string(), "added".to_string()),
        ]);

        let next = apply_patch(current, &set, &["DROP".to_string(), "MISSING".to_string()])
            .unwrap_or_else(|err| panic!("patch should apply: {}", err.0));

        assert_eq!(
            next,
            BTreeMap::from([
                ("ADD".to_string(), "added".to_string()),
                ("CHANGE".to_string(), "new".to_string()),
                ("KEEP".to_string(), "1".to_string()),
            ]),
            "set should overwrite/add, unset should remove, unknown unsets are ignored"
        );
    }

    #[test]
    fn apply_patch_rejects_invalid_and_conflicting_keys() {
        let bad_key = BTreeMap::from([("1BAD".to_string(), "v".to_string())]);
        assert!(apply_patch(BTreeMap::new(), &bad_key, &[]).is_err());

        let set = BTreeMap::from([("A".to_string(), "v".to_string())]);
        assert!(
            apply_patch(BTreeMap::new(), &set, &["A".to_string()]).is_err(),
            "a key both set and unset should be rejected"
        );
    }

    #[test]
    fn stored_env_that_does_not_decrypt_is_an_error_not_an_empty_map() {
        use crate::secret_key::{Purpose, open_map};
        assert!(open_map(Purpose::AppEnv, Some("not json")).is_err());
        assert!(open_map(Purpose::AppEnv, Some(r#"{"A":"plaintext"}"#)).is_err());
        assert_eq!(open_map(Purpose::AppEnv, None).ok(), Some(BTreeMap::new()));
    }

    #[tokio::test]
    async fn patch_env_stores_values_encrypted() {
        let state = test_state().await;
        let app = seed_app(&state, "{}", "{}", "[]").await;
        patch_env(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(PatchEnvRequest {
                set: BTreeMap::from([("API_KEY".to_string(), "sk-very-secret".to_string())]),
                unset: vec![],
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("patch_env should succeed: {}", err.0));
        let raw: String = sqlx::query_scalar("SELECT env_vars FROM apps WHERE id = ?")
            .bind(&app.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("failed to read env_vars: {err}"));
        assert!(
            raw.starts_with("v1."),
            "env must be stored encrypted: {raw}"
        );
        assert!(!raw.contains("sk-very-secret") && !raw.contains("API_KEY"));
    }

    #[test]
    fn apply_sharing_dedupes_and_sorts() {
        let next = apply_sharing(
            vec!["TOKEN".to_string(), "API_KEY".to_string()],
            &["SECRET".to_string(), "TOKEN".to_string()],
            &["API_KEY".to_string()],
        )
        .unwrap_or_else(|err| panic!("sharing should apply: {}", err.0));
        assert_eq!(next, vec!["SECRET".to_string(), "TOKEN".to_string()]);
    }

    #[tokio::test]
    async fn get_env_masks_values_and_includes_preview_config() {
        let state = test_state().await;
        let app = seed_app(
            &state,
            r#"{"DATABASE_URL":"postgres://secret","EMPTY":""}"#,
            r#"{"MODE":"preview"}"#,
            r#"["DATABASE_URL"]"#,
        )
        .await;

        let response = get_env(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id),
        )
        .await
        .unwrap_or_else(|err| panic!("get_env should succeed: {}", err.0))
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        let text = json.to_string();

        assert!(
            !text.contains("postgres://secret") && !text.contains("\"preview\"\""),
            "values must never appear in the list response: {text}"
        );
        assert_eq!(json["data"]["vars"][0]["key"], "DATABASE_URL");
        assert_eq!(json["data"]["vars"][0]["masked"], ENV_VALUE_MASK);
        assert_eq!(json["data"]["vars"][1]["masked"], "");
        assert_eq!(json["data"]["preview"]["shared_keys"][0], "DATABASE_URL");
        assert_eq!(json["data"]["preview"]["overrides"][0]["key"], "MODE");
    }

    #[tokio::test]
    async fn patch_env_changes_only_the_named_keys() {
        let state = test_state().await;
        let app = seed_app(&state, r#"{"A":"1","B":"2","C":"3"}"#, "{}", "[]").await;

        let response = patch_env(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.name.clone()),
            Json(PatchEnvRequest {
                set: BTreeMap::from([("B".to_string(), "two".to_string())]),
                unset: vec!["C".to_string()],
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("patch_env should succeed: {}", err.0))
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            stored_env(&state, &app.id).await,
            BTreeMap::from([
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "two".to_string()),
            ]),
            "untouched keys must survive a patch"
        );

        let audit: String = sqlx::query_scalar(
            "SELECT detail FROM audit_log WHERE action = 'env_update' AND resource_id = ?",
        )
        .bind(&app.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("audit log entry should exist: {err}"));
        assert!(
            !audit.contains("two"),
            "the audit log must record keys, never values: {audit}"
        );
    }

    #[tokio::test]
    async fn patch_env_rejects_empty_patches() {
        let state = test_state().await;
        let app = seed_app(&state, r#"{"A":"1"}"#, "{}", "[]").await;

        let result = patch_env(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(PatchEnvRequest::default()),
        )
        .await;

        assert!(result.is_err(), "an empty patch should be rejected");
        assert_eq!(
            stored_env(&state, &app.id).await,
            BTreeMap::from([("A".to_string(), "1".to_string())])
        );
    }

    #[tokio::test]
    async fn reveal_env_var_returns_one_plaintext_value_and_audits_it() {
        let state = test_state().await;
        let app = seed_app(&state, r#"{"TOKEN":"abc123","OTHER":"zzz"}"#, "{}", "[]").await;

        let response = reveal_env_var(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path((app.id.clone(), "TOKEN".to_string())),
        )
        .await
        .unwrap_or_else(|err| panic!("reveal should succeed: {}", err.0))
        .into_response();
        let json = body_json(response).await;
        assert_eq!(json["data"]["value"], "abc123");
        assert!(!json.to_string().contains("zzz"));

        let reveals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'env_reveal' AND resource_id = ?",
        )
        .bind(&app.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to count audit entries: {err}"));
        assert_eq!(reveals, 1);

        let missing = reveal_env_var(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path((app.id.clone(), "NOPE".to_string())),
        )
        .await;
        assert!(missing.is_err(), "revealing an unknown key should 404");
    }

    #[tokio::test]
    async fn patch_preview_env_edits_overrides_and_sharing_independently() {
        let state = test_state().await;
        let app = seed_app(
            &state,
            r#"{"A":"1"}"#,
            r#"{"MODE":"preview","KEEP":"k"}"#,
            r#"["A"]"#,
        )
        .await;

        let response = patch_preview_env(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(state.clone()),
            Path(app.id.clone()),
            Json(PatchPreviewEnvRequest {
                set: BTreeMap::from([("MODE".to_string(), "staging".to_string())]),
                unset: vec![],
                share: vec!["NODE_ENV".to_string()],
                unshare: vec!["A".to_string()],
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("patch_preview_env should succeed: {}", err.0))
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let (overrides, shared): (String, String) =
            sqlx::query_as("SELECT preview_env_vars, preview_shared_keys FROM apps WHERE id = ?")
                .bind(&app.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read preview env: {err}"));
        assert_eq!(
            crate::secret_key::open_map(
                crate::secret_key::Purpose::AppPreviewEnv,
                Some(&overrides)
            )
            .ok(),
            Some(BTreeMap::from([
                ("KEEP".to_string(), "k".to_string()),
                ("MODE".to_string(), "staging".to_string()),
            ]))
        );
        assert_eq!(parse_key_list(&shared), vec!["NODE_ENV".to_string()]);
        assert_eq!(
            stored_env(&state, &app.id).await,
            BTreeMap::from([("A".to_string(), "1".to_string())]),
            "preview edits must not touch the app's own variables"
        );
    }
}
