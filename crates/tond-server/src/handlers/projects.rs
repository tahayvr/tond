// handlers/projects.rs — Project (App Group) CRUD handlers
//
// Projects group related apps under a single logical unit and provide
// shared environment variables injected into every member app at deploy time.
//
// Routes (all require RequireAuth + RateLimit):
//   GET    /api/projects              — list all projects
//   POST   /api/projects              — create a project
//   GET    /api/projects/{id}         — get project details (with member apps)
//   PUT    /api/projects/{id}         — update name / description / env_vars
//   DELETE /api/projects/{id}         — delete project (apps are un-assigned, not deleted)
//   POST   /api/projects/{id}/deploy  — deploy all apps in the project
//   PUT    /api/apps/{id}/project     — assign or remove an app's project (in handlers.rs)

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::*;

use crate::{
    alerts,
    audit_log::{AuthActor, write_audit_log},
    auth::{RateLimit, RequireAuth},
    deploy, docker,
    error::AppError,
    state::AppState,
};

// ── Project detail response (includes member apps) ──

#[derive(serde::Serialize)]
struct ProjectDetailResponse {
    #[serde(flatten)]
    project: ProjectResponse,
    apps: Vec<AppResponse>,
}

// ── List projects ──

pub async fn list_projects(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    // Include a count of active (non-expired) project previews per project.
    // We use a subquery so we get 0 for projects with no previews.
    #[derive(sqlx::FromRow)]
    struct ProjectWithCount {
        id: String,
        name: String,
        description: Option<String>,
        env_vars: String,
        created_at: String,
        updated_at: String,
        active_preview_count: i64,
    }

    let rows: Vec<ProjectWithCount> = sqlx::query_as(
        r#"SELECT p.*,
              COALESCE((
                  SELECT COUNT(*)
                  FROM project_previews pp2
                  WHERE pp2.project_id = p.id
                    AND pp2.id IN (
                        SELECT DISTINCT pd.project_preview_id
                        FROM preview_deployments pd
                        WHERE pd.project_preview_id IS NOT NULL
                          AND pd.status != 'expired'
                    )
              ), 0) AS active_preview_count
           FROM projects p
           ORDER BY p.created_at DESC"#,
    )
    .fetch_all(&state.db)
    .await?;

    let responses: Vec<ProjectResponse> = rows
        .into_iter()
        .map(|r| {
            let env_vars: serde_json::Value = serde_json::from_str(&r.env_vars)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            ProjectResponse {
                id: r.id,
                name: r.name,
                description: r.description,
                env_vars,
                created_at: r.created_at,
                updated_at: r.updated_at,
                active_preview_count: r.active_preview_count,
            }
        })
        .collect();

    Ok(Json(ApiResponse::ok(responses)))
}

// ── Get single project (with apps) ──

pub async fn get_project(
    _rate: RateLimit,
    _auth: RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let project: Option<Project> =
        sqlx::query_as("SELECT * FROM projects WHERE id = ? OR name = ? LIMIT 1")
            .bind(&id)
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;

    let project =
        project.ok_or_else(|| AppError(TondError::NotFound(format!("project '{}'", id))))?;

    let apps: Vec<App> =
        sqlx::query_as("SELECT * FROM apps WHERE project_id = ? ORDER BY created_at ASC")
            .bind(&project.id)
            .fetch_all(&state.db)
            .await?;

    let app_responses: Vec<AppResponse> = apps.into_iter().map(AppResponse::from).collect();

    Ok(Json(ApiResponse::ok(ProjectDetailResponse {
        project: ProjectResponse::from(project),
        apps: app_responses,
    })))
}

// ── Create project ──

pub async fn create_project(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, AppError> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError(TondError::InvalidInput(
            "project name cannot be empty".into(),
        )));
    }

    // Check for duplicate name
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE name = ?)")
        .bind(&name)
        .fetch_one(&state.db)
        .await?;
    if exists {
        return Err(AppError(TondError::AppAlreadyExists(format!(
            "project '{}'",
            name
        ))));
    }

    let project = Project::new(name.clone(), req.description.clone());

    // Use caller-supplied env_vars if provided
    let env_vars_str = if let Some(ev) = req.env_vars {
        serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string())
    } else {
        "{}".to_string()
    };

    sqlx::query(
        "INSERT INTO projects (id, name, description, env_vars, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&project.id)
    .bind(&project.name)
    .bind(&project.description)
    .bind(&env_vars_str)
    .bind(&project.created_at)
    .bind(&project.updated_at)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "create_project",
        &actor.0,
        Some(&format!("name={}", name)),
    )
    .await;

    let mut project = project;
    project.env_vars = env_vars_str;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::ok(ProjectResponse::from(project))),
    ))
}

// ── Update project ──

pub async fn update_project(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<impl IntoResponse, AppError> {
    let project: Option<Project> =
        sqlx::query_as("SELECT * FROM projects WHERE id = ? OR name = ? LIMIT 1")
            .bind(&id)
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let project =
        project.ok_or_else(|| AppError(TondError::NotFound(format!("project '{}'", id))))?;

    let new_name = req.name.as_deref().unwrap_or(&project.name).to_string();
    let new_desc = req.description.or(project.description.clone());
    let new_env_str = if let Some(ev) = req.env_vars {
        serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string())
    } else {
        project.env_vars.clone()
    };

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE projects SET name = ?, description = ?, env_vars = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&new_name)
    .bind(&new_desc)
    .bind(&new_env_str)
    .bind(&now)
    .bind(&project.id)
    .execute(&state.db)
    .await?;

    write_audit_log(
        &state.db,
        None,
        None,
        "update_project",
        &actor.0,
        Some(&format!("id={}", project.id)),
    )
    .await;

    let updated = Project {
        name: new_name,
        description: new_desc,
        env_vars: new_env_str,
        updated_at: now,
        ..project
    };
    Ok(Json(ApiResponse::ok(ProjectResponse::from(updated))))
}

// ── Delete project ──

pub async fn delete_project(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let project: Option<Project> =
        sqlx::query_as("SELECT * FROM projects WHERE id = ? OR name = ? LIMIT 1")
            .bind(&id)
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let project =
        project.ok_or_else(|| AppError(TondError::NotFound(format!("project '{}'", id))))?;

    // Un-assign all member apps (do NOT delete them)
    sqlx::query("UPDATE apps SET project_id = NULL WHERE project_id = ?")
        .bind(&project.id)
        .execute(&state.db)
        .await?;

    sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(&project.id)
        .execute(&state.db)
        .await?;

    // Remove the Docker network for this project (best-effort)
    let network_name = format!("tond-{}", project.id);
    docker::remove_network(&state.docker, &network_name).await;

    write_audit_log(
        &state.db,
        None,
        None,
        "delete_project",
        &actor.0,
        Some(&format!("name={}", project.name)),
    )
    .await;

    Ok(Json(ApiResponse::ok(
        serde_json::json!({ "deleted": project.id }),
    )))
}

// ── Deploy all apps in a project ──
//
// Spawns a background deploy task for every app in the project that has a
// source configured. Returns 202 immediately; callers poll each app's status.

pub async fn deploy_project(
    _rate: RateLimit,
    _auth: RequireAuth,
    actor: AuthActor,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let project: Option<Project> =
        sqlx::query_as("SELECT * FROM projects WHERE id = ? OR name = ? LIMIT 1")
            .bind(&id)
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let project =
        project.ok_or_else(|| AppError(TondError::NotFound(format!("project '{}'", id))))?;

    let apps: Vec<App> = sqlx::query_as("SELECT * FROM apps WHERE project_id = ?")
        .bind(&project.id)
        .fetch_all(&state.db)
        .await?;

    if apps.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::ok(serde_json::json!({
                "queued": 0,
                "message": "no apps in project"
            }))),
        ));
    }

    let queued = apps.len();
    // Deploys are run sequentially in a background task to avoid the port
    // allocation race: find_free_port reads live container ports, so two
    // concurrent deploys that both need a new port will see the same snapshot
    // and pick the same port, causing one container to fail with "port already
    // allocated". Running them one after another ensures each deploy binds its
    // port before the next one calls find_free_port.
    let state_clone = state.clone();
    tokio::spawn(async move {
        for mut app in apps {
            if let Err(e) = deploy::deploy(&state_clone, &mut app, "manual").await {
                tracing::error!("Project deploy error for {}: {}", app.name, e);
                let _ = sqlx::query(
                    "UPDATE apps SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
                )
                .bind(&app.id)
                .execute(&state_clone.db)
                .await;
                alerts::fire_event_alerts(
                    &state_clone,
                    "deployment_failure",
                    Some(app.id.as_str()),
                    Some(app.name.as_str()),
                    Some(&e.to_string()),
                )
                .await;
            } else {
                alerts::fire_event_alerts(
                    &state_clone,
                    "deployment_success",
                    Some(app.id.as_str()),
                    Some(app.name.as_str()),
                    None,
                )
                .await;
            }
        }
    });

    write_audit_log(
        &state.db,
        None,
        None,
        "deploy_project",
        &actor.0,
        Some(&format!("project={} apps={}", project.name, queued)),
    )
    .await;

    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::ok(serde_json::json!({
            "queued": queued,
            "message": format!("deploying {} app(s)", queued)
        }))),
    ))
}
