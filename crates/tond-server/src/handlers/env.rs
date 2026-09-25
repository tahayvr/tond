// handlers/env.rs — app environment variables (masked reads, patch writes)
//
// Env var values are secrets, so reads never return them in bulk and writes
// never replace the whole map:
//
//   GET   /api/apps/{id}/env               — keys with masked values + preview config
//   GET   /api/apps/{id}/env/{key}         — reveal one value (audit-logged)
//   PATCH /api/apps/{id}/env               — {set: {K: V}, unset: [K]}
//   GET   /api/apps/{id}/preview-env/{key} — reveal one preview override (audit-logged)
//   PATCH /api/apps/{id}/preview-env       — {set, unset, exclude, include}
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
    audit_log::{AuthActor, write_audit_log},
    auth::{RateLimit, RequireAuth},
    error::AppError,
    state::AppState,
};

// ── Pure helpers ──

/// Parse a stored env JSON object. A missing or corrupt column reads as empty
/// only when it is blank; corrupt JSON is an error so a patch never silently
/// replaces data it could not read.
fn parse_env_map(raw: Option<&str>) -> Result<BTreeMap<String, String>, AppError> {
    let raw = raw.map(str::trim).unwrap_or_default();
    if raw.is_empty() {
        return Ok(BTreeMap::new());
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        AppError(TondError::Internal(format!(
            "stored environment is not valid JSON: {e}"
        )))
    })?;
    let object = value.as_object().ok_or_else(|| {
        AppError(TondError::Internal(
            "stored environment is not a JSON object".to_string(),
        ))
    })?;
    Ok(object
        .iter()
        .map(|(k, v)| {
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            (k.clone(), value)
        })
        .collect())
}

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

/// Apply exclude/include to the preview exclusion list, keeping it sorted and
/// free of duplicates.
fn apply_exclusions(
    current: Vec<String>,
    exclude: &[String],
    include: &[String],
) -> Result<Vec<String>, AppError> {
    validate_keys(exclude)?;
    validate_keys(include)?;
    if let Some(key) = include.iter().find(|k| exclude.contains(k)) {
        return Err(AppError(TondError::InvalidInput(format!(
            "'{key}' is both excluded and included in the same request"
        ))));
    }
    let mut keys: BTreeSet<String> = current.into_iter().collect();
    keys.extend(exclude.iter().cloned());
    for key in include {
        keys.remove(key);
    }
    Ok(keys.into_iter().collect())
}

fn env_response(app: &App) -> Result<AppEnvResponse, AppError> {
    let vars = parse_env_map(app.env_vars.as_deref())?;
    let overrides = parse_env_map(Some(&app.preview_env_vars))?;
    Ok(AppEnvResponse {
        vars: masked_env_entries(&vars),
        preview: PreviewEnvResponse {
            enabled: app.preview_enabled != 0,
            excluded_keys: parse_key_list(&app.preview_excluded_keys),
            overrides: masked_env_entries(&overrides),
        },
    })
}

fn to_json(map: &BTreeMap<String, String>) -> Result<String, AppError> {
    serde_json::to_string(map).map_err(|e| AppError(TondError::Internal(e.to_string())))
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
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(env_response(&app)?))))
}

pub async fn reveal_env_var(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let vars = parse_env_map(app.env_vars.as_deref())?;
    let value = vars
        .get(&key)
        .cloned()
        .ok_or_else(|| AppError(TondError::NotFound(format!("variable '{key}'"))))?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "env_reveal",
        &actor.0,
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
    _auth: RequireAuth,
    actor: AuthActor,
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
    let next = apply_patch(
        parse_env_map(app.env_vars.as_deref())?,
        &req.set,
        &req.unset,
    )?;
    sqlx::query("UPDATE apps SET env_vars = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(to_json(&next)?)
        .bind(&app.id)
        .execute(&mut *tx)
        .await?;
    let updated = fetch_app_in_tx(&mut tx, &app.id).await?;
    tx.commit().await?;

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
        &actor.0,
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
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let overrides = parse_env_map(Some(&app.preview_env_vars))?;
    let value = overrides
        .get(&key)
        .cloned()
        .ok_or_else(|| AppError(TondError::NotFound(format!("preview override '{key}'"))))?;

    write_audit_log(
        &state.db,
        Some(&app.id),
        Some(&app.name),
        "preview_env_reveal",
        &actor.0,
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
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchPreviewEnvRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.set.is_empty()
        && req.unset.is_empty()
        && req.exclude.is_empty()
        && req.include.is_empty()
    {
        return Err(AppError(TondError::InvalidInput(
            "nothing to change: send at least one override or exclusion change".to_string(),
        )));
    }
    let app_id = fetch_app(&state.db, &id).await?.id;

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let app = fetch_app_in_tx(&mut tx, &app_id).await?;
    let overrides = apply_patch(
        parse_env_map(Some(&app.preview_env_vars))?,
        &req.set,
        &req.unset,
    )?;
    let excluded = apply_exclusions(
        parse_key_list(&app.preview_excluded_keys),
        &req.exclude,
        &req.include,
    )?;
    let excluded_json = serde_json::to_string(&excluded)
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    sqlx::query(
        "UPDATE apps SET preview_env_vars = ?, preview_excluded_keys = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(to_json(&overrides)?)
    .bind(excluded_json)
    .bind(&app.id)
    .execute(&mut *tx)
    .await?;
    let updated = fetch_app_in_tx(&mut tx, &app.id).await?;
    tx.commit().await?;

    let detail = format!(
        "set={} unset={} exclude={} include={}",
        req.set.keys().cloned().collect::<Vec<_>>().join(","),
        req.unset.join(","),
        req.exclude.join(","),
        req.include.join(",")
    );
    write_audit_log(
        &state.db,
        Some(&updated.id),
        Some(&updated.name),
        "preview_env_update",
        &actor.0,
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

    use super::*;
    use crate::auth::RateLimiter;

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

    async fn seed_app(state: &AppState, env: &str, preview_env: &str, excluded: &str) -> App {
        let app = App::new("demo".to_string(), "image".to_string(), None, None, 3000);
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, container_port, status, env_vars, preview_env_vars, preview_excluded_keys, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.source_type)
        .bind(app.container_port)
        .bind(&app.status)
        .bind(env)
        .bind(preview_env)
        .bind(excluded)
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
        parse_env_map(raw.as_deref()).unwrap_or_else(|err| panic!("bad env JSON: {}", err.0))
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    fn actor() -> AuthActor {
        AuthActor("admin@example.com".to_string())
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
    fn parse_env_map_refuses_corrupt_json_instead_of_reading_it_as_empty() {
        assert!(parse_env_map(Some("not json")).is_err());
        assert!(parse_env_map(Some("[1,2]")).is_err());
        assert_eq!(
            parse_env_map(None).map_err(|e| e.0.to_string()),
            Ok(BTreeMap::new())
        );
    }

    #[test]
    fn apply_exclusions_dedupes_and_sorts() {
        let next = apply_exclusions(
            vec!["TOKEN".to_string(), "API_KEY".to_string()],
            &["SECRET".to_string(), "TOKEN".to_string()],
            &["API_KEY".to_string()],
        )
        .unwrap_or_else(|err| panic!("exclusions should apply: {}", err.0));
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

        let response = get_env(RateLimit, RequireAuth, State(state.clone()), Path(app.id))
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
        assert_eq!(json["data"]["preview"]["excluded_keys"][0], "DATABASE_URL");
        assert_eq!(json["data"]["preview"]["overrides"][0]["key"], "MODE");
    }

    #[tokio::test]
    async fn patch_env_changes_only_the_named_keys() {
        let state = test_state().await;
        let app = seed_app(&state, r#"{"A":"1","B":"2","C":"3"}"#, "{}", "[]").await;

        let response = patch_env(
            RateLimit,
            RequireAuth,
            actor(),
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
            "SELECT detail FROM audit_log WHERE action = 'env_update' AND app_id = ?",
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
            RequireAuth,
            actor(),
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
            RequireAuth,
            actor(),
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
            "SELECT COUNT(*) FROM audit_log WHERE action = 'env_reveal' AND app_id = ?",
        )
        .bind(&app.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|err| panic!("failed to count audit entries: {err}"));
        assert_eq!(reveals, 1);

        let missing = reveal_env_var(
            RateLimit,
            RequireAuth,
            actor(),
            State(state.clone()),
            Path((app.id.clone(), "NOPE".to_string())),
        )
        .await;
        assert!(missing.is_err(), "revealing an unknown key should 404");
    }

    #[tokio::test]
    async fn patch_preview_env_edits_overrides_and_exclusions_independently() {
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
            RequireAuth,
            actor(),
            State(state.clone()),
            Path(app.id.clone()),
            Json(PatchPreviewEnvRequest {
                set: BTreeMap::from([("MODE".to_string(), "staging".to_string())]),
                unset: vec![],
                exclude: vec!["SECRET".to_string()],
                include: vec!["A".to_string()],
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("patch_preview_env should succeed: {}", err.0))
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let (overrides, excluded): (String, String) =
            sqlx::query_as("SELECT preview_env_vars, preview_excluded_keys FROM apps WHERE id = ?")
                .bind(&app.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or_else(|err| panic!("failed to read preview env: {err}"));
        assert_eq!(
            parse_env_map(Some(&overrides)).map_err(|e| e.0.to_string()),
            Ok(BTreeMap::from([
                ("KEEP".to_string(), "k".to_string()),
                ("MODE".to_string(), "staging".to_string()),
            ]))
        );
        assert_eq!(parse_key_list(&excluded), vec!["SECRET".to_string()]);
        assert_eq!(
            stored_env(&state, &app.id).await,
            BTreeMap::from([("A".to_string(), "1".to_string())]),
            "preview edits must not touch the app's own variables"
        );
    }
}
