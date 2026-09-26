// handlers/database_links.rs — which apps can reach a managed database
//
// Every app and database runs on its own Docker network unless they share a
// project (see networks.rs). Access from an app outside the database's
// project is explicit: a link attaches the database container to the app's
// network, where the app reaches it by the database's name.
//
//   GET    /api/databases/{id}/access            — project apps + linked apps
//   PUT    /api/databases/{id}/links/{app}       — link an app (idempotent)
//   DELETE /api/databases/{id}/links/{app}       — unlink an app (idempotent)
//   GET    /api/apps/{id}/databases              — databases an app can reach
//
// Link and unlink return the refreshed access list. Both are audit-logged.

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
    networks,
    state::AppState,
};

async fn fetch_database(
    pool: &sqlx::SqlitePool,
    id_or_name: &str,
) -> Result<ManagedDatabase, AppError> {
    sqlx::query_as::<_, ManagedDatabase>("SELECT * FROM databases WHERE id = ? OR name = ? LIMIT 1")
        .bind(id_or_name)
        .bind(id_or_name)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError(TondError::NotFound(format!("database '{id_or_name}'"))))
}

fn app_ref(app: &App) -> DatabaseAppRef {
    DatabaseAppRef {
        id: app.id.clone(),
        name: app.name.clone(),
        status: app.status.clone(),
        project_id: app.project_id.clone(),
    }
}

fn internal_port(db: &ManagedDatabase) -> u16 {
    engine_for(&db.db_type).map_or(0, |e| e.container_port)
}

/// Build the access list for a database.
async fn access_for(
    state: &AppState,
    db: &ManagedDatabase,
) -> Result<DatabaseAccessResponse, AppError> {
    let (project, project_apps) = match db.project_id.as_deref() {
        Some(pid) => {
            let project =
                sqlx::query_as::<_, (String, String)>("SELECT id, name FROM projects WHERE id = ?")
                    .bind(pid)
                    .fetch_optional(&state.db)
                    .await?
                    .map(|(id, name)| DatabaseProjectRef { id, name });
            let apps: Vec<App> =
                sqlx::query_as("SELECT * FROM apps WHERE project_id = ? ORDER BY name")
                    .bind(pid)
                    .fetch_all(&state.db)
                    .await?;
            (project, apps.iter().map(app_ref).collect())
        }
        None => (None, Vec::new()),
    };
    let linked = networks::linked_apps(&state.db, &db.id)
        .await
        .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
    Ok(DatabaseAccessResponse {
        database_id: db.id.clone(),
        project,
        project_apps,
        linked_apps: linked.iter().map(app_ref).collect(),
        internal_host: db.name.clone(),
        internal_port: internal_port(db),
    })
}

/// Why an app can't be linked to a database, if it can't.
fn link_refusal(db: &ManagedDatabase, app: &App) -> Option<String> {
    if app.source_type == "compose" {
        return Some(format!(
            "'{}' is a compose app: compose apps run on their own compose network and can't be \
             linked. Add the database as a service in the compose file, or put the app's \
             services behind a regular app.",
            app.name
        ));
    }
    if db.project_id.is_some() && db.project_id == app.project_id {
        return Some(format!(
            "'{}' is in the same project as '{}' and can already reach it at {}:{}.",
            app.name,
            db.name,
            db.name,
            internal_port(db)
        ));
    }
    None
}

// ── GET /api/databases/{id}/access ──

pub async fn get_database_access(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_database(&state.db, &id).await?;
    let access = access_for(&state, &db).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(access))))
}

// ── PUT /api/databases/{id}/links/{app} ──

pub async fn link_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path((id, app_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_database(&state.db, &id).await?;
    let app = fetch_app(&state.db, &app_id).await?;
    if let Some(reason) = link_refusal(&db, &app) {
        return Err(AppError(TondError::InvalidInput(reason)));
    }

    let inserted = sqlx::query(
        "INSERT INTO database_links (database_id, app_id) VALUES (?, ?)
         ON CONFLICT(database_id, app_id) DO NOTHING",
    )
    .bind(&db.id)
    .bind(&app.id)
    .execute(&state.db)
    .await?
    .rows_affected()
        > 0;

    // Attach the database to the app's network now. If Docker refuses, undo
    // the link so the list never shows access that doesn't exist.
    if let Err(e) = networks::reconcile_database(&state, &db).await {
        if inserted {
            let _ = sqlx::query("DELETE FROM database_links WHERE database_id = ? AND app_id = ?")
                .bind(&db.id)
                .bind(&app.id)
                .execute(&state.db)
                .await;
        }
        return Err(AppError(TondError::DockerError(format!(
            "Couldn't connect '{}' to '{}': {e}. Nothing was changed.",
            db.name, app.name
        ))));
    }

    if inserted {
        write_audit_log(
            &state.db,
            Some(&app.id),
            Some(&app.name),
            "db_link",
            &actor.0,
            Some(&format!("database={}", db.name)),
        )
        .await;
    }

    let access = access_for(&state, &db).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(access))))
}

// ── DELETE /api/databases/{id}/links/{app} ──

pub async fn unlink_app(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path((id, app_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let db = fetch_database(&state.db, &id).await?;
    let app = fetch_app(&state.db, &app_id).await?;

    let removed = sqlx::query("DELETE FROM database_links WHERE database_id = ? AND app_id = ?")
        .bind(&db.id)
        .bind(&app.id)
        .execute(&state.db)
        .await?
        .rows_affected()
        > 0;

    if removed {
        // Detach now; if Docker fails the link is gone from the database and
        // the next reconcile (restart, redeploy) detaches the network.
        if let Err(e) = networks::reconcile_database(&state, &db).await {
            tracing::warn!(
                "Unlinked {} from {} but could not detach the network yet: {e}",
                app.name,
                db.name
            );
        }
        write_audit_log(
            &state.db,
            Some(&app.id),
            Some(&app.name),
            "db_unlink",
            &actor.0,
            Some(&format!("database={}", db.name)),
        )
        .await;
    }

    let access = access_for(&state, &db).await?;
    Ok((StatusCode::OK, Json(ApiResponse::ok(access))))
}

// ── GET /api/apps/{id}/databases ──

pub async fn list_app_databases(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let app = fetch_app(&state.db, &id).await?;
    let mut out: Vec<AppDatabaseAccess> = Vec::new();

    let to_access = |db: &ManagedDatabase, via: &str| AppDatabaseAccess {
        id: db.id.clone(),
        name: db.name.clone(),
        db_type: db.db_type.clone(),
        status: db.status.clone(),
        via: via.to_string(),
        internal_host: db.name.clone(),
        internal_port: internal_port(db),
    };

    // Compose apps run on their own compose network: nothing reaches them.
    if app.source_type != "compose" {
        if let Some(pid) = app.project_id.as_deref() {
            let dbs: Vec<ManagedDatabase> =
                sqlx::query_as("SELECT * FROM databases WHERE project_id = ? ORDER BY name")
                    .bind(pid)
                    .fetch_all(&state.db)
                    .await?;
            out.extend(dbs.iter().map(|db| to_access(db, "project")));
        }
        let linked = networks::links_for_app(&state.db, &app.id)
            .await
            .map_err(|e| AppError(TondError::Internal(e.to_string())))?;
        for db in &linked {
            if !out.iter().any(|a| a.id == db.id) {
                out.push(to_access(db, "link"));
            }
        }
    }

    Ok((StatusCode::OK, Json(ApiResponse::ok(out))))
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
            .unwrap_or_else(|err| panic!("sqlite: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("migrate: {err}"));
        AppState {
            db,
            docker: Docker::connect_with_http("http://127.0.0.1:1", 1, API_DEFAULT_VERSION)
                .unwrap_or_else(|err| panic!("docker client: {err}")),
            rate_limiter: RateLimiter::new(60, 10.0),
            settings: Arc::new(RwLock::new(ServerSettings::default())),
        }
    }

    async fn seed_app(
        state: &AppState,
        name: &str,
        project: Option<&str>,
        source_type: &str,
    ) -> App {
        let id = format!("app-{name}");
        sqlx::query(
            "INSERT INTO apps (id, name, source_type, status, project_id, created_at, updated_at)
             VALUES (?, ?, ?, 'running', ?, datetime('now'), datetime('now'))",
        )
        .bind(&id)
        .bind(name)
        .bind(source_type)
        .bind(project)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("seed app: {err}"));
        sqlx::query_as("SELECT * FROM apps WHERE id = ?")
            .bind(&id)
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|err| panic!("load app: {err}"))
    }

    async fn seed_project(state: &AppState, id: &str) {
        sqlx::query(
            "INSERT INTO projects (id, name, env_vars, created_at, updated_at)
             VALUES (?, ?, '{}', datetime('now'), datetime('now'))",
        )
        .bind(id)
        .bind(format!("proj-{id}"))
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("seed project: {err}"));
    }

    /// A database row without a container: links are stored and listed, and
    /// no Docker call is made (it is attached when its container starts).
    async fn seed_db(state: &AppState, name: &str, project: Option<&str>) -> ManagedDatabase {
        let db = ManagedDatabase::new(
            name.to_string(),
            "postgres".to_string(),
            project.map(str::to_string),
        );
        sqlx::query(
            "INSERT INTO databases (id, name, db_type, project_id, status, created_at, updated_at)
             VALUES (?, ?, 'postgres', ?, 'stopped', datetime('now'), datetime('now'))",
        )
        .bind(&db.id)
        .bind(&db.name)
        .bind(&db.project_id)
        .execute(&state.db)
        .await
        .unwrap_or_else(|err| panic!("seed db: {err}"));
        db
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("body: {err}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("json: {err}"))
    }

    fn actor() -> AuthActor {
        AuthActor("tester".to_string())
    }

    #[tokio::test]
    async fn link_and_unlink_update_the_access_list_and_audit_log() {
        let state = test_state().await;
        let db = seed_db(&state, "main", None).await;
        let web = seed_app(&state, "web", None, "git").await;

        let response = link_app(
            RateLimit,
            RequireAuth,
            actor(),
            State(state.clone()),
            Path((db.name.clone(), web.name.clone())),
        )
        .await
        .unwrap_or_else(|e| panic!("link should succeed: {}", e.0))
        .into_response();
        let body = json(response).await;
        assert_eq!(body["data"]["linked_apps"][0]["name"], "web");
        assert_eq!(body["data"]["internal_host"], "main");
        assert_eq!(body["data"]["internal_port"], 5432);

        // Linking twice is a no-op.
        let again = link_app(
            RateLimit,
            RequireAuth,
            actor(),
            State(state.clone()),
            Path((db.id.clone(), web.id.clone())),
        )
        .await
        .unwrap_or_else(|e| panic!("second link should succeed: {}", e.0))
        .into_response();
        assert_eq!(
            json(again).await["data"]["linked_apps"]
                .as_array()
                .map_or(0, |a| a.len()),
            1
        );

        let apps = list_app_databases(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path(web.id.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("list: {}", e.0))
        .into_response();
        let apps = json(apps).await;
        assert_eq!(apps["data"][0]["name"], "main");
        assert_eq!(apps["data"][0]["via"], "link");

        let response = unlink_app(
            RateLimit,
            RequireAuth,
            actor(),
            State(state.clone()),
            Path((db.id.clone(), web.id.clone())),
        )
        .await
        .unwrap_or_else(|e| panic!("unlink should succeed: {}", e.0))
        .into_response();
        assert_eq!(
            json(response).await["data"]["linked_apps"]
                .as_array()
                .map_or(1, |a| a.len()),
            0
        );

        let actions: Vec<String> =
            sqlx::query_scalar("SELECT action FROM audit_log ORDER BY created_at, rowid")
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();
        assert_eq!(
            actions,
            vec!["db_link".to_string(), "db_unlink".to_string()]
        );
    }

    #[tokio::test]
    async fn project_members_are_listed_and_cannot_be_linked_redundantly() {
        let state = test_state().await;
        seed_project(&state, "p1").await;
        let db = seed_db(&state, "main", Some("p1")).await;
        let api = seed_app(&state, "api", Some("p1"), "git").await;
        seed_app(&state, "other", None, "git").await;

        let access = get_database_access(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path(db.id.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("access: {}", e.0))
        .into_response();
        let access = json(access).await;
        assert_eq!(access["data"]["project"]["name"], "proj-p1");
        assert_eq!(access["data"]["project_apps"][0]["name"], "api");
        assert_eq!(
            access["data"]["linked_apps"]
                .as_array()
                .map_or(1, |a| a.len()),
            0
        );

        let err = link_app(
            RateLimit,
            RequireAuth,
            actor(),
            State(state.clone()),
            Path((db.id.clone(), api.id.clone())),
        )
        .await
        .err()
        .map(|e| e.0.to_string())
        .unwrap_or_default();
        assert!(err.contains("same project"), "got: {err}");

        let apps = list_app_databases(
            RateLimit,
            RequireAuth,
            State(state.clone()),
            Path(api.id.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("list: {}", e.0))
        .into_response();
        assert_eq!(json(apps).await["data"][0]["via"], "project");
    }

    #[tokio::test]
    async fn compose_apps_cannot_be_linked() {
        let state = test_state().await;
        let db = seed_db(&state, "main", None).await;
        let stack = seed_app(&state, "stack", None, "compose").await;
        let err = link_app(
            RateLimit,
            RequireAuth,
            actor(),
            State(state.clone()),
            Path((db.id.clone(), stack.id.clone())),
        )
        .await
        .err()
        .map(|e| e.0.to_string())
        .unwrap_or_default();
        assert!(err.contains("compose app"), "got: {err}");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM database_links")
            .fetch_one(&state.db)
            .await
            .unwrap_or(-1);
        assert_eq!(count, 0, "a refused link is not stored");
    }

    #[tokio::test]
    async fn deleting_an_app_or_database_removes_its_links() {
        let state = test_state().await;
        let db = seed_db(&state, "main", None).await;
        let web = seed_app(&state, "web", None, "git").await;
        sqlx::query("INSERT INTO database_links (database_id, app_id) VALUES (?, ?)")
            .bind(&db.id)
            .bind(&web.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("link: {err}"));
        sqlx::query("DELETE FROM apps WHERE id = ?")
            .bind(&web.id)
            .execute(&state.db)
            .await
            .unwrap_or_else(|err| panic!("delete: {err}"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM database_links")
            .fetch_one(&state.db)
            .await
            .unwrap_or(-1);
        assert_eq!(count, 0, "links cascade with the app");
    }
}
