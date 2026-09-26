// handlers/templates.rs — the one-click template catalog
//
//   GET  /api/templates              the catalog (tond_common::builtin_templates)
//   POST /api/templates/{id}/deploy  create the app and start its first deploy
//
// A template deploy is one server-side step so every client gets the same
// behaviour: inputs are checked and generated, the app is created as an inline
// compose app whose env holds the (encrypted) inputs, its volume directories
// are created with the owners the images expect, and the first deployment is
// queued. If anything fails after the app row exists, the app is removed again
// so a retry starts clean. Generated credentials are returned once.

use std::collections::BTreeMap;
use std::path::Path as FsPath;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use tond_common::{
    ApiResponse, App, CreateAppRequest, Scope, Template, TemplateCatalogResponse, TemplateContext,
    TemplateCredential, TemplateDeployRequest, TemplateDeployResponse, TemplateHostMemory,
    TemplateResponse, TondError, builtin_template, builtin_templates, resolve_inputs,
};

use crate::{
    audit_log::write_audit_log,
    auth::{Authz, Principal, RateLimit, scope},
    deploy, domains,
    error::AppError,
    handlers::{deploy_trigger, insert_app},
    metrics::host,
    state::AppState,
    volumes,
};

const MIB: u64 = 1024 * 1024;

fn catalog_error(err: impl std::fmt::Display) -> AppError {
    AppError(TondError::Internal(format!(
        "the template catalog is invalid: {err}"
    )))
}

pub async fn list_templates(
    _rate: RateLimit,
    _auth: Authz<scope::AppsRead>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let templates: Vec<TemplateResponse> = builtin_templates()
        .map_err(catalog_error)?
        .into_iter()
        .map(TemplateResponse::from)
        .collect();
    let memory = host::snapshot().map(|s| TemplateHostMemory {
        total_mb: s.mem_total / MIB,
        available_mb: s.mem_total.saturating_sub(s.mem_used) / MIB,
    });
    let apps_domain = state.settings.read().await.apps_domain.clone();
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(TemplateCatalogResponse {
            templates,
            memory,
            apps_domain,
        })),
    ))
}

/// Free memory in bytes (total minus used, i.e. what the kernel reports as
/// available), from the host collector's last sample.
fn available_memory() -> Option<u64> {
    host::snapshot().map(|s| s.mem_total.saturating_sub(s.mem_used))
}

/// The 409 for a template that needs more memory than is free, unless the
/// caller chose to deploy anyway.
fn check_memory(
    template: &Template,
    available: Option<u64>,
    allow_low_memory: bool,
) -> Result<(), AppError> {
    let Some(available) = available else {
        return Ok(());
    };
    let needed = u64::from(template.manifest.ram_mb) * MIB;
    if allow_low_memory || needed <= available {
        return Ok(());
    }
    Err(AppError(TondError::Conflict {
        code: "insufficient_memory",
        message: format!(
            "{} needs about {} MB of memory and this server has {} MB free. Stop or remove an app to make room, or deploy anyway and let it swap.",
            template.manifest.name,
            template.manifest.ram_mb,
            available / MIB
        ),
    }))
}

/// Refuse an app name whose volume directory already holds data: data left
/// by an earlier app of the same name would not match the new passwords.
fn check_no_leftover_data(data_dir: &FsPath, app_name: &str) -> Result<(), AppError> {
    let volume_dir = volumes::app_volume_dir(data_dir, app_name);
    if std::fs::read_dir(&volume_dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(AppError(TondError::InvalidInput(format!(
            "{} still holds data from an earlier app named {app_name}. Pick another name, or delete that directory on the server first.",
            volume_dir.display()
        ))));
    }
    Ok(())
}

/// Create the declared volume directories, with their owners, inside the
/// app's volume directory.
fn prepare_volumes(template: &Template, data_dir: &FsPath, app_name: &str) -> Result<(), AppError> {
    let volume_dir = volumes::app_volume_dir(data_dir, app_name);
    for volume in &template.manifest.volumes {
        let dir = volumes::prepare_host_dir(
            data_dir,
            &volume_dir,
            FsPath::new(volume.relative_path()),
            false,
        )
        .map_err(|e| {
            AppError(TondError::Internal(format!(
                "could not create the volume directory {}: {e:#}",
                volume.path
            )))
        })?;
        #[cfg(unix)]
        if let Some((uid, gid)) = volume.owner_ids()
            && let Err(e) = std::os::unix::fs::chown(&dir, Some(uid), Some(gid))
        {
            // Only root can chown; outside production (dev, tests) the
            // image may then fail to write, which its logs will say.
            tracing::warn!("could not give {} to {uid}:{gid}: {e}", dir.display());
        }
    }
    Ok(())
}

/// Undo a template deploy that failed after the app was created. Its volume
/// directory was new or empty (check_no_leftover_data), so removing it only
/// removes what this deploy created.
async fn remove_app(state: &AppState, app: &App, data_dir: &FsPath) {
    let _ = sqlx::query("DELETE FROM apps WHERE id = ?")
        .bind(&app.id)
        .execute(&state.db)
        .await;
    let _ = std::fs::remove_dir_all(volumes::app_volume_dir(data_dir, &app.name));
}

pub async fn deploy_template(
    _rate: RateLimit,
    auth: Authz<scope::AppsWrite>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TemplateDeployRequest>,
) -> Result<impl IntoResponse, AppError> {
    let response = deploy_from_template(&state, &auth, &id, req, &volumes::data_dir()).await?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::ok(response))))
}

/// The deploy itself, with the data directory passed in (tests use a temp dir).
async fn deploy_from_template(
    state: &AppState,
    auth: &Principal,
    id: &str,
    req: TemplateDeployRequest,
    data_dir: &FsPath,
) -> Result<TemplateDeployResponse, AppError> {
    // Templates are compose apps (admin scope, checked by insert_app) that
    // are deployed right away.
    auth.require_to(Scope::AppsDeploy, "deploy apps")?;
    let template = builtin_template(id)
        .map_err(catalog_error)?
        .ok_or_else(|| AppError(TondError::NotFound(format!("No template named {id}"))))?;

    let name = req.name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 40
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || name.starts_with('-')
        || name.ends_with('-')
    {
        return Err(AppError(TondError::InvalidInput(
            "App name must be 1–40 lowercase letters, digits and hyphens".into(),
        )));
    }
    check_memory(&template, available_memory(), req.allow_low_memory)?;

    // The address the app will answer on, for inputs like {app_url}: the
    // custom domain, else the automatic one (<name>.<apps domain>).
    let domain = req
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(domains::normalize_domain);
    let apps_domain = state.settings.read().await.apps_domain.clone();
    let host = match &domain {
        Some(d) => d.clone(),
        None if !apps_domain.is_empty() => domains::app_host(&name, &apps_domain),
        None => format!("{name}.localhost"),
    };
    let ctx = TemplateContext::new(&name, &host);
    let resolved = resolve_inputs(&template.manifest, &req.inputs, &ctx)
        .map_err(|e| AppError(TondError::InvalidInput(e.to_string())))?;

    check_no_leftover_data(data_dir, &name)?;

    let public = &template.manifest.public;
    let env: BTreeMap<&str, &str> = resolved
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let create = CreateAppRequest {
        name: name.clone(),
        source_type: "compose".into(),
        source_url: None,
        branch: None,
        container_port: i64::from(public.port),
        env_vars: Some(serde_json::json!(env)),
        domain: domain.clone(),
        project_id: req.project_id.clone().filter(|p| !p.is_empty()),
        memory_limit: None,
        cpu_limit: None,
        health_check_path: public.health_path.clone(),
        health_check_interval: None,
        health_failure_threshold: None,
        drain_timeout: None,
        build_method: None,
        auto_deploy: false,
        watch_branch: None,
        volumes: None,
        registry_credential_id: None,
        github_app_id: None,
        dockerfile_content: None,
        compose_content: Some(template.compose.clone()),
        base_dir: None,
        static_build: false,
        readiness: Some(
            if public.health_path.is_some() {
                "http"
            } else {
                "tcp"
            }
            .into(),
        ),
        ready_after_secs: None,
    };
    let app = insert_app(state, auth, create).await?;

    if let Err(e) = prepare_volumes(&template, data_dir, &name) {
        remove_app(state, &app, data_dir).await;
        return Err(e);
    }

    let app_id = app.id.clone();
    let app_domain = app.domain.clone();
    let (trigger, detail) = deploy_trigger(auth);
    let queued = match deploy::start(
        state,
        app.clone(),
        deploy::DeployRequest::source(trigger, detail),
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            remove_app(state, &app, data_dir).await;
            return Err(AppError(TondError::Internal(format!(
                "{} was not created: the first deploy couldn't be queued ({e:#})",
                template.manifest.name
            ))));
        }
    };

    write_audit_log(
        &state.db,
        Some(&app_id),
        Some(&name),
        "template.deploy",
        &auth.audit_actor(),
        Some(&format!(
            "template={} version={} deployment={}",
            template.manifest.id, template.manifest.version, queued.deployment_id
        )),
    )
    .await;

    let response = TemplateDeployResponse {
        app_id,
        app_name: name,
        domain: app_domain.or_else(|| (!apps_domain.is_empty()).then_some(host)),
        deployment_id: queued.deployment_id.clone(),
        credentials: resolved
            .credentials
            .into_iter()
            .map(|(key, label, value)| TemplateCredential { key, label, value })
            .collect(),
        setup_notes: template.manifest.setup_notes.clone(),
    };
    Ok(response)
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, response::IntoResponse};

    use super::*;
    use crate::compose_policy;
    use tond_common::Role;

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|err| panic!("failed to read response body: {err}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("failed to decode JSON response body: {err}"))
    }

    async fn test_state() -> AppState {
        let db = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|err| panic!("failed to create in-memory SQLite pool: {err}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|err| panic!("failed to run migrations: {err}"));
        AppState {
            db,
            docker: bollard::Docker::connect_with_http(
                "http://127.0.0.1:2375",
                5,
                bollard::API_DEFAULT_VERSION,
            )
            .unwrap_or_else(|err| panic!("failed to construct Docker client: {err}")),
            rate_limiter: crate::auth::RateLimiter::new(60, 10.0),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(tond_common::ServerSettings {
                apps_domain: "apps.example.com".into(),
                ..Default::default()
            })),
        }
    }

    fn deploy_request(name: &str, inputs: &[(&str, &str)]) -> TemplateDeployRequest {
        TemplateDeployRequest {
            name: name.into(),
            domain: None,
            project_id: None,
            inputs: inputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            allow_low_memory: true,
        }
    }

    async fn deploy(
        state: &AppState,
        data_dir: &FsPath,
        id: &str,
        req: TemplateDeployRequest,
    ) -> Result<serde_json::Value, AppError> {
        let principal = Principal::test_user(Role::Admin);
        let response = deploy_from_template(state, &principal, id, req, data_dir).await?;
        Ok(serde_json::json!({ "data": response }))
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"))
    }

    async fn app_count(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM apps")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[tokio::test]
    async fn deploy_creates_a_compose_app_with_its_inputs_and_queues_a_deploy() {
        let state = test_state().await;
        let dir = tmp();
        let body = deploy(&state, dir.path(), "it-tools", deploy_request("tools", &[]))
            .await
            .unwrap_or_else(|e| panic!("{}", e.0));
        assert_eq!(body["data"]["app_name"], "tools");
        assert_eq!(body["data"]["domain"], "tools.apps.example.com");
        assert!(
            body["data"]["deployment_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );

        let app: App = sqlx::query_as("SELECT * FROM apps WHERE name = 'tools'")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(app.source_type, "compose");
        assert_eq!(app.container_port, 80);
        assert!(
            app.compose_content
                .as_deref()
                .is_some_and(|c| c.contains("it-tools"))
        );
        assert_eq!(app.health_check_path.as_deref(), Some("/"));
    }

    #[tokio::test]
    async fn deploy_returns_generated_credentials_once_and_stores_them_encrypted() {
        let state = test_state().await;
        let dir = tmp();
        let body = deploy(
            &state,
            dir.path(),
            "vaultwarden",
            deploy_request("vault", &[]),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
        let creds = body["data"]["credentials"]
            .as_array()
            .unwrap_or_else(|| panic!("credentials"));
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0]["key"], "ADMIN_TOKEN");
        let token = creds[0]["value"].as_str().unwrap_or_default().to_string();
        assert_eq!(token.len(), 24);
        assert!(
            volumes::app_volume_dir(dir.path(), "vault")
                .join("data")
                .is_dir(),
            "declared volumes are created"
        );
        let app: App = sqlx::query_as("SELECT * FROM apps WHERE name = 'vault'")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let env = crate::secrets::app_env(&app).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(env["DOMAIN"], "https://vault.apps.example.com");
        assert_eq!(env["ADMIN_TOKEN"], token);
        assert!(
            !app.env_vars.as_deref().unwrap_or("").contains(&token),
            "env vars are stored encrypted"
        );

        // A second app of the same name would reuse the first one's data.
        sqlx::query("DELETE FROM apps")
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            volumes::app_volume_dir(dir.path(), "vault").join("data/db.sqlite3"),
            "x",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let err = deploy(
            &state,
            dir.path(),
            "vaultwarden",
            deploy_request("vault", &[]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("leftover data is refused"));
        assert!(
            err.0.to_string().contains("earlier app named vault"),
            "{}",
            err.0
        );
        assert_eq!(app_count(&state).await, 0);
    }

    #[tokio::test]
    async fn deploy_refuses_bad_names_unknown_templates_and_missing_inputs() {
        let state = test_state().await;
        let dir = tmp();
        let err = deploy(
            &state,
            dir.path(),
            "it-tools",
            deploy_request("Bad Name", &[]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("bad names are refused"));
        assert_eq!(err.0.status_code(), 400);
        let err = deploy(&state, dir.path(), "nope", deploy_request("x", &[]))
            .await
            .err()
            .unwrap_or_else(|| panic!("unknown templates are refused"));
        assert_eq!(err.0.status_code(), 404);
        let err = deploy(&state, dir.path(), "pocketbase", deploy_request("pb", &[]))
            .await
            .err()
            .unwrap_or_else(|| panic!("a missing required input is refused"));
        assert!(err.0.to_string().contains("Superuser email"), "{}", err.0);
        let err = deploy(
            &state,
            dir.path(),
            "pocketbase",
            deploy_request("pb", &[("SUPERUSER_EMAIL", "nope")]),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("an invalid email is refused"));
        assert!(err.0.to_string().contains("email address"), "{}", err.0);
        assert_eq!(app_count(&state).await, 0, "nothing is created on refusal");
    }

    #[tokio::test]
    async fn deploy_needs_the_compose_and_deploy_scopes() {
        let state = test_state().await;
        let result = deploy_template(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Viewer)),
            State(state.clone()),
            Path("it-tools".to_string()),
            Json(deploy_request("tools", &[])),
        )
        .await
        .map(|_| ());
        let Err(err) = result else {
            panic!("a viewer can't deploy templates");
        };
        assert_eq!(err.0.status_code(), 403);
        assert_eq!(app_count(&state).await, 0);
    }

    #[tokio::test]
    async fn list_templates_returns_the_catalog() {
        let response = list_templates(
            RateLimit,
            Authz::unchecked(Principal::test_user(Role::Admin)),
            State(test_state().await),
        )
        .await
        .unwrap_or_else(|_| panic!("expected list_templates to succeed"))
        .into_response();
        let body = response_json(response).await;
        assert_eq!(body["data"]["apps_domain"], "apps.example.com");
        let templates = body["data"]["templates"]
            .as_array()
            .unwrap_or_else(|| panic!("expected templates payload to be an array"));
        assert!(templates.len() >= 25);
        let umami = templates
            .iter()
            .find(|t| t["id"] == "umami")
            .unwrap_or_else(|| panic!("umami should be in the catalog"));
        assert_eq!(umami["public"]["service"], "umami");
        assert_eq!(umami["fits_2gb"], true);
        assert!(umami["services"].as_array().is_some_and(|s| s.len() == 2));
        assert!(
            umami["inputs"]
                .as_array()
                .is_some_and(|i| i.iter().all(|i| i["key"] != "DATABASE_URL")),
            "computed values are not form inputs"
        );
    }

    /// Every shipped compose file passes the same policy user compose files do.
    #[test]
    fn every_template_passes_compose_policy() {
        let volume_dir = FsPath::new("/opt/tond/data/volumes/app");
        for t in builtin_templates().unwrap_or_else(|e| panic!("{e}")) {
            if let Err(errors) = compose_policy::validate(&t.compose, volume_dir) {
                panic!(
                    "{}: {}",
                    t.manifest.id,
                    compose_policy::format_errors(&errors)
                );
            }
            // Every input reaches compose (no reserved names are dropped).
            let env: BTreeMap<String, String> = t
                .manifest
                .inputs
                .iter()
                .map(|i| (i.key.clone(), "x".to_string()))
                .collect();
            assert_eq!(
                compose_policy::interpolation_env(&t.compose, &env).len(),
                env.len(),
                "{}: every input should be passed to docker compose",
                t.manifest.id
            );
        }
    }

    #[test]
    fn memory_check_warns_unless_overridden() {
        let t = builtin_template("umami")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("umami"));
        let need = u64::from(t.manifest.ram_mb) * MIB;
        assert!(check_memory(&t, Some(need), false).is_ok());
        assert!(
            check_memory(&t, None, false).is_ok(),
            "no sample yet: don't block"
        );
        let err = check_memory(&t, Some(need / 2), false)
            .err()
            .unwrap_or_else(|| panic!("expected insufficient_memory"));
        assert_eq!(err.0.code(), Some("insufficient_memory"));
        assert_eq!(err.0.status_code(), 409);
        assert!(check_memory(&t, Some(need / 2), true).is_ok());
    }

    #[test]
    fn prepare_volumes_refuses_leftover_data() {
        let tmp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let t = builtin_template("umami")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("umami"));
        check_no_leftover_data(tmp.path(), "stats").unwrap_or_else(|e| panic!("{}", e.0));
        prepare_volumes(&t, tmp.path(), "stats").unwrap_or_else(|e| panic!("{}", e.0));
        let dir = volumes::app_volume_dir(tmp.path(), "stats");
        for v in &t.manifest.volumes {
            assert!(dir.join(v.relative_path()).is_dir());
        }
        let err = check_no_leftover_data(tmp.path(), "stats")
            .err()
            .unwrap_or_else(|| panic!("an app's leftover data should be refused"));
        assert!(err.0.to_string().contains("earlier app named stats"));
    }
}
