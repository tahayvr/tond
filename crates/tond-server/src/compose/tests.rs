use super::*;
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
    let specs = build_specs(TWO_SERVICES, Path::new("/srv/repo"));
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
async fn syncing_keeps_user_choices_and_drops_services_that_left_the_file() {
    let db = db_with_app().await;
    let specs = service_specs(TWO_SERVICES).unwrap_or_else(|e| panic!("{e}"));
    sync_specs(&db, "a1", &specs)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query(
        "UPDATE compose_services SET published_port = 80, host_port = 8100 WHERE name = 'web'",
    )
    .execute(&db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    sqlx::query(
        "INSERT INTO app_domains (domain, app_id, service, port) VALUES
         ('shop.example.com', 'a1', 'web', 80), ('cache.example.com', 'a1', 'cache', 6379)",
    )
    .execute(&db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    // The cache leaves the file; web changes image.
    let next = service_specs(
        "services:\n  web:\n    image: nginx:2\n    ports: ['80']\n  api:\n    build: .\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    sync_specs(&db, "a1", &next)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let rows = rows(&db, "a1").await.unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["web", "api"]);
    assert_eq!(rows[0].image.as_deref(), Some("nginx:2"));
    assert_eq!(rows[0].published_port, Some(80), "user choice kept");
    assert_eq!(rows[0].host_port, Some(8100));
    let domains = domains_by_service(&db, "a1")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(domains.get("web").map(Vec::len), Some(1));
    assert!(
        !domains.contains_key("cache"),
        "domains of a removed service go too"
    );
    let primary = primary_domains(&db).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        primary.get("a1").map(String::as_str),
        Some("shop.example.com")
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
