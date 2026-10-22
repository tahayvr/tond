use std::sync::Arc;

use axum::body::to_bytes;
use bollard::{API_DEFAULT_VERSION, Docker};
use tokio::sync::RwLock;

use super::*;
use crate::auth::{Principal, RateLimiter};

const FILE: &str = "services:\n  web:\n    image: nginx\n    ports: [\"8080:80\"]\n  cache:\n    image: redis:7\n    expose: [6379]\n";

async fn test_state() -> AppState {
    let db = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    AppState {
        db,
        // Nothing listens here: Docker calls fail fast.
        docker: Docker::connect_with_http("http://127.0.0.1:9", 1, API_DEFAULT_VERSION)
            .unwrap_or_else(|e| panic!("{e}")),
        rate_limiter: RateLimiter::new(60, 10.0),
        settings: Arc::new(RwLock::new(ServerSettings::default())),
    }
}

fn admin<R: crate::auth::principal::ScopeReq>() -> Authz<R> {
    Authz::unchecked(Principal::test_user(Role::Admin))
}

async fn body(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{e}"))
}

async fn create(state: &AppState, name: &str, domain: Option<&str>) {
    let req: CreateAppRequest = serde_json::from_value(json!({
        "name": name,
        "source_type": "compose",
        "container_port": 80,
        "compose_content": FILE,
        "domain": domain,
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    super::super::create_app(RateLimit, admin(), State(state.clone()), Json(req))
        .await
        .unwrap_or_else(|e| panic!("create: {}", e.0));
}

async fn update(
    state: &AppState,
    app: &str,
    service: &str,
    port: Option<u16>,
    domains: &[&str],
) -> Result<serde_json::Value, AppError> {
    let response = update_service(
        RateLimit,
        admin(),
        State(state.clone()),
        Path((app.to_string(), service.to_string())),
        Json(UpdateComposeServiceRequest {
            published_port: port,
            domains: domains.iter().map(|d| d.to_string()).collect(),
        }),
    )
    .await?
    .into_response();
    Ok(body(response).await)
}

#[tokio::test]
async fn creating_a_compose_app_publishes_the_web_service_with_its_domain() {
    let state = test_state().await;
    create(&state, "shop", Some("shop.example.com")).await;
    let app = fetch_app(&state.db, "shop")
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(
        app.domain.as_deref(),
        Some("shop.example.com"),
        "apps.domain mirrors the primary domain"
    );
    let row: (Option<String>, Option<i64>, String) = sqlx::query_as(
        "SELECT service, port, kind FROM app_domains WHERE domain = 'shop.example.com'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        row,
        (Some("web".into()), Some(80), "primary".into()),
        "the seeded primary row belongs to the web service"
    );
    let services = services_response(&state, &app, true)
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(services.len(), 2);
    let web = &services[0];
    assert_eq!(web.name, "web");
    assert_eq!(web.published_port, Some(80));
    assert_eq!(web.domains, vec!["shop.example.com".to_string()]);
    assert!(web.redeploy_required, "not deployed yet");
    assert_eq!(services[1].published_port, None);
    assert_eq!(services[1].ports, vec![6379]);

    // The domain is taken now.
    create(&state, "other", None).await;
    let err = update(&state, "other", "web", Some(80), &["shop.example.com"])
        .await
        .err()
        .unwrap_or_else(|| panic!("a domain routes to one place"));
    assert!(
        err.0.to_string().contains("already used by the app 'shop'"),
        "{}",
        err.0
    );
}

#[tokio::test]
async fn service_domains_need_a_published_port_and_replace_the_list() {
    let state = test_state().await;
    create(&state, "shop", None).await;
    let err = update(&state, "shop", "cache", None, &["cache.example.com"])
        .await
        .err()
        .unwrap_or_else(|| panic!("domains need a port"));
    assert!(
        err.0.to_string().contains("Publish a container port"),
        "{}",
        err.0
    );
    let err = update(&state, "shop", "nope", Some(1), &[])
        .await
        .err()
        .unwrap_or_else(|| panic!("unknown service"));
    assert_eq!(err.0.status_code(), 404);

    let out = update(
        &state,
        "shop",
        "cache",
        Some(6379),
        &["A.Example.com", "b.example.com", "a.example.com"],
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(
        out["data"]["service"]["domains"],
        json!(["a.example.com", "b.example.com"])
    );
    assert_eq!(out["data"]["redeploy_required"], json!(true));

    let out = update(&state, "shop", "cache", Some(6379), &["b.example.com"])
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(out["data"]["service"]["domains"], json!(["b.example.com"]));

    // Unpublishing drops the host port and the domains.
    sqlx::query(
        "UPDATE compose_services SET host_port = 8300, routed_port = 6379 WHERE name = 'cache'",
    )
    .execute(&state.db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let out = update(&state, "shop", "cache", None, &[])
        .await
        .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(out["data"]["service"]["host_port"], json!(null));
    assert_eq!(out["data"]["service"]["domains"], json!([]));
}

#[tokio::test]
async fn service_domains_set_through_the_api_are_what_the_proxy_routes() {
    let state = test_state().await;
    create(&state, "shop", None).await;
    update(
        &state,
        "shop",
        "cache",
        Some(6379),
        &["cache.example.com", "redis.example.com"],
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    type Row = (String, String, String, String, i64, bool, bool);
    let routes = || async {
        sqlx::query_as::<_, Row>(tond_common::route_queries::COMPOSE_SERVICE_ROUTES)
            .fetch_all(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    };
    assert!(
        routes().await.is_empty(),
        "not deployed yet: no host port, nothing to route"
    );

    // What a deploy does: the service gets its loopback host port, the app runs.
    sqlx::query("UPDATE compose_services SET host_port = 8301 WHERE name = 'cache'")
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query("UPDATE apps SET status = 'running' WHERE name = 'shop'")
        .execute(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut routed: Vec<(String, String, i64)> = routes()
        .await
        .into_iter()
        .map(|(domain, _, _, service, port, _, _)| (domain, service, port))
        .collect();
    routed.sort();
    assert_eq!(
        routed,
        vec![
            ("cache.example.com".to_string(), "cache".to_string(), 8301),
            ("redis.example.com".to_string(), "cache".to_string(), 8301),
        ]
    );

    // A template app whose admin account Tond is still creating isn't routed,
    // and leaving 'pending' is a routing change the proxy is told about.
    let version = || async {
        sqlx::query_scalar::<_, i64>("SELECT version FROM route_state WHERE id = 1")
            .fetch_one(&state.db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let set_admin_setup = |value: &'static str| {
        let db = state.db.clone();
        async move {
            sqlx::query("UPDATE apps SET admin_setup = ? WHERE name = 'shop'")
                .bind(value)
                .execute(&db)
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        }
    };
    let before = version().await;
    set_admin_setup("pending").await;
    assert!(routes().await.is_empty(), "pending admin setup: not routed");
    assert!(version().await > before);
    let before = version().await;
    set_admin_setup("done").await;
    assert_eq!(routes().await.len(), 2);
    assert!(version().await > before);
    let before = version().await;
    set_admin_setup("dismissed").await;
    assert_eq!(version().await, before, "only 'pending' changes routing");
}

#[tokio::test]
async fn saving_a_compose_file_validates_and_resyncs_services() {
    let state = test_state().await;
    create(&state, "shop", None).await;
    let save = |content: &str| {
        update_compose(
            RateLimit,
            admin(),
            State(state.clone()),
            Path("shop".to_string()),
            Json(UpdateComposeRequest {
                compose_content: Some(content.to_string()),
                ..Default::default()
            }),
        )
    };
    let err = save("services:\n  web:\n    image: x\n    privileged: true\n")
        .await
        .err()
        .unwrap_or_else(|| panic!("policy applies to saves"));
    assert!(err.0.to_string().contains("privileged"), "{}", err.0);
    let out = body(
        save("services:\n  web:\n    image: nginx:2\n  worker:\n    image: busybox\n")
            .await
            .unwrap_or_else(|e| panic!("{}", e.0))
            .into_response(),
    )
    .await;
    let names: Vec<String> = out["data"]["services"]
        .as_array()
        .unwrap_or_else(|| panic!("services"))
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(names, ["web", "worker"]);
    assert_eq!(
        out["data"]["services"][0]["published_port"],
        json!(80),
        "kept"
    );

    // Members can't edit compose files.
    let err = update_compose(
        RateLimit,
        Authz::unchecked(Principal::test_user(Role::Deployer)),
        State(state.clone()),
        Path("shop".to_string()),
        Json(UpdateComposeRequest::default()),
    )
    .await
    .err()
    .unwrap_or_else(|| panic!("admin only"));
    assert_eq!(err.0.status_code(), 403);
}

#[tokio::test]
async fn compose_apps_refuse_the_single_domain_endpoint_and_others_have_no_services() {
    let state = test_state().await;
    create(&state, "shop", None).await;
    let err = super::super::update_domain(
        RateLimit,
        admin(),
        State(state.clone()),
        Path("shop".to_string()),
        Json(UpdateDomainRequest {
            domain: "x.example.com".into(),
        }),
    )
    .await
    .err()
    .unwrap_or_else(|| panic!("per-service domains only"));
    assert!(err.0.to_string().contains("per service"), "{}", err.0);

    sqlx::query(
        "INSERT INTO apps (id, name, source_type, created_at, updated_at)
         VALUES ('i1', 'img', 'image', 'now', 'now')",
    )
    .execute(&state.db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let err = list_services(RateLimit, admin(), State(state), Path("img".to_string()))
        .await
        .err()
        .unwrap_or_else(|| panic!("not a compose app"));
    assert!(
        err.0.to_string().contains("not a Docker Compose app"),
        "{}",
        err.0
    );
}
