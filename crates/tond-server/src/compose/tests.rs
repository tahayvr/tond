use super::*;
use std::collections::BTreeMap;
use std::path::Path;

const TWO_SERVICES: &str = r#"
services:
  web:
    image: nginx:${TAG:-1.27}
    ports:
      - "8080:80"
    healthcheck:
      test: ["CMD", "true"]
  cache:
    image: redis:7
    expose: [6379]
    volumes:
      - cache-data:/data
      - ./conf:/etc/redis
  api:
    build:
      context: ./api
      dockerfile: Dockerfile.prod
      args:
        MODE: ${MODE:-prod}
      target: runtime
    ports:
      - target: 3000
        protocol: tcp
      - "53:53/udp"
volumes:
  cache-data: {}
"#;

#[test]
fn interpolation_uses_an_empty_environment() {
    assert_eq!(interpolate("nginx:${TAG:-1.27}"), "nginx:1.27");
    assert_eq!(interpolate("a${X-b}c"), "abc");
    assert_eq!(interpolate("${UNSET}x"), "x");
    assert_eq!(interpolate("$UNSET/x"), "/x");
    assert_eq!(interpolate("cost: $$5"), "cost: $5");
    assert_eq!(interpolate("${A:-${B:-deep}}"), "deep");
    assert_eq!(interpolate("héllo"), "héllo");
}

#[test]
fn interpolation_reads_the_apps_environment() {
    let env = BTreeMap::from([
        ("TAG".to_string(), "2.0".to_string()),
        ("EMPTY".to_string(), String::new()),
    ]);
    assert_eq!(interpolate_env("nginx:${TAG:-1}", &env), "nginx:2.0");
    assert_eq!(interpolate_env("nginx:$TAG", &env), "nginx:2.0");
    assert_eq!(
        interpolate_env("${EMPTY:-d}", &env),
        "d",
        ":- also replaces empty"
    );
    assert_eq!(
        interpolate_env("${EMPTY-d}", &env),
        "",
        "- only replaces unset"
    );
    assert_eq!(interpolate_env("${MISSING?need it}", &env), "");
    let specs = service_specs_env("services:\n  a:\n    image: app:${TAG}\n", &env)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(specs[0].image.as_deref(), Some("app:2.0"));
    let builds = build_specs(
        "services:\n  a:\n    build:\n      context: .\n      args: [\"V=${TAG}\"]\n",
        Path::new("/c"),
        &env,
    );
    assert_eq!(builds["a"].args, vec![("V".to_string(), "2.0".to_string())]);
}

#[test]
fn specs_list_services_in_file_order_with_ports_and_healthchecks() {
    let specs = service_specs(TWO_SERVICES).unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["web", "cache", "api"]);
    assert_eq!(specs[0].image.as_deref(), Some("nginx:1.27"));
    assert_eq!(specs[0].ports, vec![80]);
    assert!(specs[0].has_healthcheck);
    assert_eq!(specs[1].ports, vec![6379]);
    assert!(!specs[1].has_healthcheck);
    assert!(specs[2].builds);
    assert_eq!(specs[2].image, None);
    assert_eq!(specs[2].ports, vec![3000], "udp ports are not routable");
}

#[test]
fn profiled_and_zero_replica_services_are_inactive() {
    let content = "services:\n  web:\n    image: a\n  debug:\n    image: b\n    profiles: [debug]\n  off:\n    image: c\n    deploy:\n      replicas: 0\n  on:\n    image: d\n    profiles: []\n";
    assert_eq!(inactive_services(content), vec!["debug", "off"]);
    assert!(inactive_services("not: [valid").is_empty());
}

#[test]
fn container_ports_parse_every_short_syntax() {
    let p = |s: &str| container_port(&Value::String(s.into()));
    assert_eq!(p("80"), Some(80));
    assert_eq!(p("8080:80"), Some(80));
    assert_eq!(p("127.0.0.1:8080:80"), Some(80));
    assert_eq!(p("127.0.0.1::80/tcp"), Some(80));
    assert_eq!(p("[::1]:8080:80"), Some(80));
    assert_eq!(p("53:53/udp"), None);
    assert_eq!(p("3000-3005"), None);
    assert_eq!(container_port(&Value::Number(9000.into())), Some(9000));
}

#[test]
fn volume_usage_finds_named_volumes_and_binds() {
    let usage = volume_usage(TWO_SERVICES);
    assert_eq!(usage.named, vec!["cache-data".to_string()]);
    assert!(usage.binds);
    assert_eq!(
        volume_usage("services:\n  a:\n    image: x\n    volumes: [/tmp]\n"),
        VolumeUsage::default(),
        "anonymous volumes are not backed up"
    );
    let long = volume_usage(
        "services:\n  a:\n    image: x\n    volumes:\n      - type: volume\n        source: db\n        target: /d\n",
    );
    assert_eq!(long.named, vec!["db".to_string()]);
}

#[test]
fn web_service_is_the_preferred_one_or_the_one_with_the_port() {
    let specs = service_specs(TWO_SERVICES).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        pick_web_service(&specs, None, 3000),
        Some(("api".into(), 3000))
    );
    assert_eq!(
        pick_web_service(&specs, None, 1234),
        Some(("web".into(), 80))
    );
    assert_eq!(
        pick_web_service(&specs, Some("cache"), 80),
        Some(("cache".into(), 6379)),
        "a preferred service publishes one of its own ports"
    );
    assert_eq!(pick_web_service(&specs, Some("nope"), 80), None);
    let bare = service_specs("services:\n  worker:\n    image: x\n").unwrap_or_default();
    assert_eq!(pick_web_service(&bare, None, 80), None);
    assert_eq!(
        pick_web_service(&bare, Some("worker"), 80),
        Some(("worker".into(), 80))
    );
}

#[test]
fn build_specs_resolve_contexts_inside_the_compose_directory() {
    let specs = build_specs(TWO_SERVICES, Path::new("/srv/repo"), &BTreeMap::new());
    let api = specs
        .get("api")
        .unwrap_or_else(|| panic!("api builds: {specs:?}"));
    assert_eq!(api.context, "/srv/repo/./api");
    assert_eq!(api.dockerfile.as_deref(), Some("Dockerfile.prod"));
    assert_eq!(api.args, vec![("MODE".to_string(), "prod".to_string())]);
    assert_eq!(api.target.as_deref(), Some("runtime"));
    assert_eq!(specs.len(), 1);

    let short = build_specs(
        "services:\n  a:\n    build: https://github.com/x/y.git\n  b:\n    build: .\n    image: b\n",
        Path::new("/c"),
        &BTreeMap::new(),
    );
    assert_eq!(short["a"].context, "https://github.com/x/y.git");
    assert_eq!(short["b"].context, "/c/.");
}

#[test]
fn pinned_tags_are_per_service_and_per_deployment() {
    assert_eq!(service_repo("Shop", "Web"), "tond-shop/web");
    assert_eq!(service_repo("shop", "_my svc."), "tond-shop/my-svc");
    assert_eq!(service_repo("shop", "..."), "tond-shop/svc");
    assert_eq!(
        pinned_tag("shop", "web", "3F2A1B9C-1234-5678-9abc-def012345678"),
        "tond-shop/web:3f2a1b9c1234"
    );
}

#[test]
fn content_tags_follow_the_image_not_the_deployment() {
    assert_eq!(
        content_tag("Shop", "web", "sha256:4F1C2A3B9E01D7C55A0021C1"),
        "tond-shop/web:4f1c2a3b9e01"
    );
    assert_eq!(
        content_tag("shop", "web", "sha256:4f1c2a3b9e01ffff"),
        content_tag("shop", "web", "sha256:4f1c2a3b9e01eeee"),
        "same prefix, same tag"
    );
}

#[test]
fn pin_file_swaps_builds_for_pinned_images_and_publishes_routed_ports() {
    let pinned = pin_file(
        TWO_SERVICES,
        &[
            Pin {
                service: "api".into(),
                image: Some("tond-shop/api:abc".into()),
                publish: Some((8101, 3000)),
            },
            Pin {
                service: "cache".into(),
                image: Some("tond-shop/cache:abc".into()),
                publish: None,
            },
            Pin {
                service: "gone".into(),
                image: Some("x".into()),
                publish: None,
            },
        ],
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let doc: Value = serde_yaml::from_str(&pinned).unwrap_or_else(|e| panic!("{e}"));
    let api = &doc["services"]["api"];
    assert_eq!(api["image"].as_str(), Some("tond-shop/api:abc"));
    assert!(api.get("build").is_none(), "the build ran already");
    assert_eq!(api["pull_policy"].as_str(), Some("never"));
    let ports: Vec<String> = api["ports"]
        .as_sequence()
        .unwrap_or_else(|| panic!("ports"))
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    assert!(ports.contains(&"127.0.0.1:8101:3000".to_string()));
    assert_eq!(
        doc["services"]["cache"]["image"].as_str(),
        Some("tond-shop/cache:abc")
    );
    assert!(doc["services"]["cache"]["ports"].is_null());
    assert_eq!(
        doc["services"]["web"]["image"].as_str(),
        Some("nginx:${TAG:-1.27}"),
        "unpinned services are left to compose"
    );
}

#[test]
fn status_strings_yield_health_and_exit_codes() {
    assert_eq!(
        health_from_status("Up 3 minutes (healthy)").as_deref(),
        Some("healthy")
    );
    assert_eq!(
        health_from_status("Up 3 minutes (unhealthy)").as_deref(),
        Some("unhealthy")
    );
    assert_eq!(
        health_from_status("Up 2 seconds (health: starting)").as_deref(),
        Some("starting")
    );
    assert_eq!(health_from_status("Up 3 minutes"), None);
    assert_eq!(exit_code_from_status("Exited (137) 2 hours ago"), Some(137));
    assert_eq!(exit_code_from_status("Up 1 second"), None);
    assert!(is_completed("exited", Some(0)));
    assert!(!is_completed("exited", Some(1)));
}

async fn db_with_app() -> sqlx::SqlitePool {
    let db = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query(
        "INSERT INTO apps (id, name, source_type, created_at, updated_at)
         VALUES ('a1', 'shop', 'compose', 'now', 'now')",
    )
    .execute(&db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    db
}

#[tokio::test]
async fn syncing_keeps_user_choices_and_parks_services_that_left_the_file() {
    let db = db_with_app().await;
    let specs = service_specs(TWO_SERVICES).unwrap_or_else(|e| panic!("{e}"));
    sync_specs(&db, "a1", &specs, Gone::Delete)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query(
        "UPDATE compose_services SET published_port = 80, host_port = 8100 WHERE name = 'web';
         UPDATE compose_services SET published_port = 6379, host_port = 8101 WHERE name = 'cache';
         UPDATE apps SET status = 'running' WHERE id = 'a1'",
    )
    .execute(&db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query(
        "INSERT INTO app_domains (domain, app_id, service, port, kind) VALUES
         ('shop.example.com', 'a1', 'web', 80, 'primary'),
         ('cache.example.com', 'a1', 'cache', 6379, 'alias')",
    )
    .execute(&db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    let routed = || async {
        sqlx::query_as::<_, (String, String, String, String, i64, i64, i64)>(
            tond_common::route_queries::COMPOSE_SERVICE_ROUTES,
        )
        .fetch_all(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|r| r.0)
        .collect::<Vec<_>>()
    };
    assert_eq!(routed().await, ["shop.example.com", "cache.example.com"]);

    // The cache leaves the file (a rollback to an older one); web changes image.
    const NEXT: &str =
        "services:\n  web:\n    image: nginx:2\n    ports: ['80']\n  api:\n    build: .\n";
    let leaving = services_leaving(&db, "a1", NEXT)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        leaving,
        [("cache".to_string(), vec!["cache.example.com".to_string()])]
    );
    let next = service_specs(NEXT).unwrap_or_else(|e| panic!("{e}"));
    sync_specs(&db, "a1", &next, Gone::Park)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let rows = rows(&db, "a1").await.unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["web", "api", "cache"]);
    assert_eq!(rows[0].image.as_deref(), Some("nginx:2"));
    assert_eq!(rows[0].published_port, Some(80), "user choice kept");
    assert_eq!(rows[0].host_port, Some(8100));
    let cache = &rows[2];
    assert_eq!(cache.parked, 1);
    assert_eq!(cache.status, "parked");
    assert_eq!(cache.published_port, Some(6379), "kept for when it returns");
    assert_eq!(cache.host_port, None, "unrouted");
    let domains = domains_by_service(&db, "a1")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(domains.get("web").map(Vec::len), Some(1));
    assert_eq!(
        domains.get("cache").map(Vec::len),
        Some(1),
        "a parked service keeps its domains"
    );
    assert_eq!(
        routed().await,
        ["shop.example.com"],
        "but they aren't routed"
    );
    assert!(
        services_leaving(&db, "a1", NEXT)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
            .is_empty(),
        "already parked"
    );
    let primary: Option<String> = sqlx::query_scalar("SELECT domain FROM apps WHERE id = 'a1'")
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        primary.as_deref(),
        Some("shop.example.com"),
        "apps.domain mirrors the primary domain, whichever service it is on"
    );

    // Docker reports web running and api missing.
    store_states(
        &db,
        "a1",
        &[ServiceState {
            service: "web".into(),
            container_id: "c1".into(),
            state: "running".into(),
            health: Some("healthy".into()),
            exit_code: None,
            image: None,
            image_id: None,
        }],
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let rows = super::rows(&db, "a1")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows[0].status, "running");
    assert_eq!(rows[0].health.as_deref(), Some("healthy"));
    assert_eq!(rows[1].status, "missing");
    assert_eq!(rows[1].container_id, None);
    assert_eq!(
        rows[2].status, "parked",
        "Docker's view leaves parked rows alone"
    );

    // The cache comes back (rolling forward): unparked with its choices.
    sync_specs(
        &db,
        "a1",
        &service_specs(TWO_SERVICES).unwrap_or_else(|e| panic!("{e}")),
        Gone::Park,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let rows = super::rows(&db, "a1")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let cache = rows
        .iter()
        .find(|r| r.name == "cache")
        .unwrap_or_else(|| panic!("cache row"));
    assert_eq!(cache.parked, 0);
    assert_eq!(cache.status, "pending");
    assert_eq!(cache.published_port, Some(6379));
    // Routed again once the next deploy allocates its host port.
    sqlx::query("UPDATE compose_services SET host_port = 8101 WHERE name = 'cache'")
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(routed().await, ["shop.example.com", "cache.example.com"]);
}

#[tokio::test]
async fn a_parked_service_can_be_forgotten_with_its_domains() {
    let db = db_with_app().await;
    sync_specs(
        &db,
        "a1",
        &service_specs(TWO_SERVICES).unwrap_or_else(|e| panic!("{e}")),
        Gone::Park,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query(
        "INSERT INTO app_domains (domain, app_id, service, port, kind) VALUES
         ('cache.example.com', 'a1', 'cache', 6379, 'primary')",
    )
    .execute(&db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !forget_parked(&db, "a1", "cache")
            .await
            .unwrap_or_else(|e| panic!("{e}")),
        "a service in the file isn't forgotten"
    );
    sync_specs(
        &db,
        "a1",
        &service_specs("services:\n  web:\n    image: nginx\n").unwrap_or_else(|e| panic!("{e}")),
        Gone::Park,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        forget_parked(&db, "a1", "cache")
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    );
    let names: Vec<String> = rows(&db, "a1")
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["web", "api"], "api is parked, cache is gone");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_domains WHERE app_id = 'a1'")
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(left, 0);

    // A changed file (not a rollback) removes running services outright, but
    // a parked one stays parked: after a rollback of a pasted file the stored
    // file lacks it too, and the next ordinary deploy must not drop it.
    sync_specs(
        &db,
        "a1",
        &service_specs("services:\n  other:\n    image: x\n").unwrap_or_else(|e| panic!("{e}")),
        Gone::Delete,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<(String, i64)> = rows(&db, "a1")
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|r| (r.name, r.parked))
        .collect();
    assert_eq!(names, [("other".to_string(), 0), ("api".to_string(), 1)]);
}

#[test]
fn replicas_come_from_deploy_or_scale() {
    let specs = service_specs(
        "services:\n  a:\n    image: x\n    deploy:\n      replicas: 3\n  b:\n    image: x\n    scale: 2\n  c:\n    image: x\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let replicas: Vec<u32> = specs.iter().map(|s| s.replicas).collect();
    assert_eq!(replicas, [3, 2, 1]);
    assert!(replicas_publish_error("a", 3).is_some_and(|m| m.contains("3 replicas")));
    assert_eq!(replicas_publish_error("c", 1), None);
    assert_eq!(replicas_publish_error("z", 0), None);
}

#[tokio::test]
async fn route_version_moves_with_service_domains_and_host_ports() {
    let db = db_with_app().await;
    let version = || async {
        sqlx::query_scalar::<_, i64>("SELECT version FROM route_state WHERE id = 1")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    };
    sync_specs(
        &db,
        "a1",
        &service_specs("services:\n  web:\n    image: x\n").unwrap_or_default(),
        Gone::Delete,
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let v0 = version().await;
    sqlx::query("UPDATE compose_services SET host_port = 8100 WHERE name = 'web'")
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let v1 = version().await;
    assert!(v1 > v0, "a new host port is a route change");
    sqlx::query("UPDATE compose_services SET status = 'running' WHERE name = 'web'")
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(version().await, v1, "status alone does not reload routes");
    sqlx::query("INSERT INTO app_domains (domain, app_id, service, port) VALUES ('w.example.com', 'a1', 'web', 80)")
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(version().await > v1);
}
