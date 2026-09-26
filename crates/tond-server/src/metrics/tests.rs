use std::sync::Arc;

use bollard::{API_DEFAULT_VERSION, Docker};
use tokio::sync::RwLock as TokioRwLock;
use tond_common::ServerSettings;

use super::cgroup::tests::{NET_DEV, write_fake_cgroup};
use super::*;
use crate::auth::RateLimiter;

fn cid(c: char) -> String {
    c.to_string().repeat(64)
}

fn app(id: &str) -> Owner {
    Owner::new(TargetKind::App, id)
}

fn resolver() -> Resolver {
    let mut r = Resolver::default();
    r.by_container.insert(cid('a'), app("app-1"));
    r.by_container
        .insert(cid('d'), Owner::new(TargetKind::Database, "db-1"));
    r.app_by_name.insert("web".into(), "app-1".into());
    r.app_by_name.insert("shop".into(), "app-2".into());
    r.db_by_name.insert("pg".into(), "db-1".into());
    r.preview_by_slug.insert("a1b2c3".into(), "pv-1".into());
    r.cron_ids
        .push("0123abcd-0000-0000-0000-000000000000".into());
    r.names.insert(app("app-1"), "web".into());
    r
}

fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn resolver_maps_every_kind_of_container() {
    let r = resolver();
    let none = HashMap::new();
    assert_eq!(r.resolve(&cid('a'), "/whatever", &none), Some(app("app-1")));
    assert_eq!(
        r.resolve(
            &cid('x'),
            "/shop-db-1",
            &labels(&[("com.docker.compose.project", "shop")])
        ),
        Some(app("app-2")),
        "compose services belong to their app"
    );
    assert_eq!(
        r.resolve(
            &cid('x'),
            "/tond-cron-x",
            &labels(&[(CRON_JOB_LABEL, "job-9")])
        ),
        Some(Owner::new(TargetKind::Cron, "job-9"))
    );
    assert_eq!(
        r.resolve(&cid('x'), "/tond-cron-0123abcd-deadbeef", &none),
        Some(Owner::new(
            TargetKind::Cron,
            "0123abcd-0000-0000-0000-000000000000"
        ))
    );
    assert_eq!(
        r.resolve(&cid('x'), "/tond-db-pg", &none),
        Some(Owner::new(TargetKind::Database, "db-1"))
    );
    assert_eq!(
        r.resolve(&cid('x'), "/tond-preview-a1b2c3", &none),
        Some(Owner::new(TargetKind::Preview, "pv-1"))
    );
    assert_eq!(
        r.resolve(&cid('x'), "/tond-web-next", &none),
        Some(app("app-1"))
    );
    assert_eq!(r.resolve(&cid('x'), "/tond-WEB", &none), Some(app("app-1")));
    assert_eq!(r.resolve(&cid('x'), "/postgres", &none), None);
    assert_eq!(r.resolve(&cid('x'), "/tond-unknown", &none), None);
    assert_eq!(r.name(&app("app-1")), "web");
    assert_eq!(r.name(&app("zzz")), "zzz");
}

fn collector(root: &Path) -> Collector {
    let mut c = Collector::new(root.join("cg"), root.join("proc"));
    c.resolver = resolver();
    c
}

fn start(c: &mut Collector, id: &str, name: &str, now: i64) -> Option<Happening> {
    c.apply_event("start", id, &labels(&[("name", name)]), now)
}

#[test]
fn crash_then_restart_records_one_restart_with_the_exit_code() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let mut c = collector(dir.path());
    let id = cid('a');
    assert_eq!(
        start(&mut c, &id, "tond-web", 100),
        None,
        "first start is not a restart"
    );
    assert_eq!(containers_running(), 1);
    assert_eq!(
        c.apply_event("die", &id, &labels(&[("exitCode", "137")]), 200),
        None
    );
    let h = start(&mut c, &id, "tond-web", 205).unwrap_or_else(|| panic!("restart expected"));
    assert_eq!(h.kind, EventKind::Restart);
    assert_eq!(h.owner, app("app-1"));
    assert_eq!(h.detail.as_deref(), Some("exit code 137"));
}

#[test]
fn requested_stops_and_restarts_are_not_crashes() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let mut c = collector(dir.path());
    let id = cid('a');
    start(&mut c, &id, "tond-web", 100);
    c.apply_event("kill", &id, &HashMap::new(), 200);
    c.apply_event("die", &id, &labels(&[("exitCode", "143")]), 201);
    assert_eq!(start(&mut c, &id, "tond-web", 202), None);
    // Stop + remove (deploy replacing the container): nothing recorded.
    c.apply_event("kill", &id, &HashMap::new(), 300);
    c.apply_event("die", &id, &labels(&[("exitCode", "0")]), 301);
    assert_eq!(c.apply_event("destroy", &id, &HashMap::new(), 302), None);
    assert!(c.overdue_crashes(1000).is_empty());
}

#[test]
fn exits_that_stay_down_become_crashes() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let mut c = collector(dir.path());
    let id = cid('a');
    start(&mut c, &id, "tond-web", 100);
    c.apply_event("oom", &id, &HashMap::new(), 199);
    c.apply_event("die", &id, &labels(&[("exitCode", "137")]), 200);
    assert!(
        c.overdue_crashes(230).is_empty(),
        "wait for a possible restart"
    );
    let crashes = c.overdue_crashes(260);
    assert_eq!(crashes.len(), 1);
    assert_eq!(crashes[0].kind, EventKind::Crash);
    assert_eq!(crashes[0].ts, 200);
    assert_eq!(
        crashes[0].detail.as_deref(),
        Some("exit code 137, out of memory")
    );
    assert!(c.overdue_crashes(400).is_empty(), "reported once");

    // Removed while still down: the crash is reported on destroy.
    let id2 = cid('d');
    start(&mut c, &id2, "tond-db-pg", 100);
    c.apply_event("die", &id2, &labels(&[("exitCode", "1")]), 200);
    let h = c
        .apply_event("destroy", &id2, &HashMap::new(), 210)
        .unwrap_or_else(|| panic!("crash expected"));
    assert_eq!(h.kind, EventKind::Crash);
    assert_eq!(h.owner, Owner::new(TargetKind::Database, "db-1"));
}

#[test]
fn oom_events_are_recorded_for_owned_containers_only() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let mut c = collector(dir.path());
    start(&mut c, &cid('a'), "tond-web", 100);
    start(&mut c, &cid('e'), "someone-elses", 100);
    let h = c
        .apply_event("oom", &cid('a'), &HashMap::new(), 150)
        .unwrap_or_else(|| panic!("oom expected"));
    assert_eq!((h.kind, h.ts), (EventKind::Oom, 150));
    assert_eq!(c.apply_event("oom", &cid('e'), &HashMap::new(), 150), None);
    assert_eq!(c.apply_event("oom", &cid('f'), &HashMap::new(), 150), None);
    assert_eq!(containers_running(), 1, "only Tond containers are counted");
}

fn write_net(root: &Path, pid: u32, rx: u64, tx: u64) {
    let dir = root.join("proc").join(pid.to_string()).join("net");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let body = NET_DEV
        .replace("    410", &format!("{:>7}", rx - 100))
        .replace("       42 ", &format!("{:>9} ", tx - 58));
    std::fs::write(dir.join("dev"), body).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn samples_compute_rates_and_sum_compose_services() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    let mut c = collector(root);
    let (a, b) = (cid('a'), cid('b'));
    // Two containers of app-2 (compose) and nothing else.
    c.resolver.by_container.clear();
    for id in [&a, &b] {
        c.apply_event(
            "start",
            id,
            &labels(&[("name", "svc"), ("com.docker.compose.project", "shop")]),
            100,
        );
    }
    let cg = |id: &str| root.join("cg/docker").join(id);
    write_fake_cgroup(&cg(&a), 1_000_000, 11_000, 0);
    write_fake_cgroup(&cg(&b), 2_000_000, 21_000, 0);
    write_net(root, 563, 1_000, 500);

    let t0 = Instant::now();
    let (aggs, events) = c.sample_containers(1000, t0, &HashMap::new());
    assert!(events.is_empty());
    let first = aggs.get(&app("app-2")).cloned().unwrap_or_default();
    assert!(!first.has_rates, "no previous reading yet");
    assert_eq!(first.sample.mem, 10_000 + 20_000);
    assert_eq!(first.sample.pids, 6);
    assert_eq!(first.mem_limit, None, "memory.max is unlimited");

    // 10 s later: a used 0.5 s of CPU, b 1 s → 15% of one core in total.
    write_fake_cgroup(&cg(&a), 1_500_000, 11_000, 0);
    write_fake_cgroup(&cg(&b), 3_000_000, 21_000, 1);
    write_net(root, 563, 11_000, 1_500);
    let (aggs, events) = c.sample_containers(1010, t0 + Duration::from_secs(10), &HashMap::new());
    let second = aggs.get(&app("app-2")).cloned().unwrap_or_default();
    assert!(second.has_rates);
    assert_eq!(second.sample.cpu, 1_500, "hundredths of a percent");
    // Both containers read the same (shared) fake netns: 1000 B/s each.
    assert_eq!(second.sample.rx, 2_000);
    assert_eq!(second.sample.tx, 200);
    assert_eq!(events.len(), 1, "oom_kill went up in b");
    assert_eq!(events[0].kind, EventKind::Oom);
    assert_eq!(events[0].detail.as_deref(), Some("1 process killed"));

    // Counters went backwards: a restart the event stream missed.
    write_fake_cgroup(&cg(&a), 10, 11_000, 0);
    let (_, events) = c.sample_containers(1020, t0 + Duration::from_secs(20), &HashMap::new());
    assert!(events.iter().any(|h| h.kind == EventKind::Restart));

    // Cgroup vanished: skipped, not an error.
    std::fs::remove_dir_all(cg(&b)).unwrap_or_else(|e| panic!("{e}"));
    let (aggs, _) = c.sample_containers(1030, t0 + Duration::from_secs(30), &HashMap::new());
    assert_eq!(aggs.get(&app("app-2")).map(|a| a.sample.pids), Some(3));
}

#[test]
fn describe_reads_well() {
    let h = Happening {
        owner: app("app-1"),
        ts: 0,
        kind: EventKind::Restart,
        detail: Some("exit code 1".into()),
    };
    assert_eq!(
        describe(&h),
        "restarted after exiting unexpectedly (exit code 1)"
    );
}

async fn test_state() -> AppState {
    let db = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap_or_else(|e| panic!("pool: {e}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|e| panic!("migrate: {e}"));
    AppState {
        db,
        docker: Docker::connect_with_http("http://127.0.0.1:1", 1, API_DEFAULT_VERSION)
            .unwrap_or_else(|e| panic!("docker: {e}")),
        rate_limiter: RateLimiter::new(60, 10.0),
        settings: Arc::new(TokioRwLock::new(ServerSettings::default())),
    }
}

#[tokio::test]
async fn tick_writes_one_row_per_target_and_records_events() {
    let state = test_state().await;
    sqlx::query(
        "INSERT INTO apps (id, name, source_type, container_port, container_id, status, created_at, updated_at)
         VALUES ('app-1', 'web', 'git', 3000, ?, 'running', datetime('now'), datetime('now'))",
    )
    .bind(cid('a'))
    .execute(&state.db)
    .await
    .unwrap_or_else(|e| panic!("seed: {e}"));

    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let root = dir.path();
    let mut c = Collector::new(root.join("cg"), root.join("proc"));
    let mut host = host::HostSampler::new(root.to_path_buf());
    c.resolver = Resolver::load(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    start(&mut c, &cid('a'), "tond-web", 100);
    write_fake_cgroup(&root.join("cg/docker").join(cid('a')), 1_000, 5_000, 0);
    write_net(root, 563, 1_000, 1_000);

    c.tick(&state, &mut host).await;
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics_raw")
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows, 0, "first reading only primes the counters");
    assert_eq!(current(&app("app-1")).map(|c| c.sample.mem), Some(4_000));
    assert!(host::snapshot().is_some());

    // Make the second tick land in a different second.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    c.tick(&state, &mut host).await;
    let (target, mem, limit): (i64, i64, i64) = sqlx::query_as(
        "SELECT r.target, r.mem, t.mem_limit FROM metrics_raw r JOIN metric_targets t ON t.id = r.target",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((mem, limit), (4_000, 0));

    // A crash-restart is persisted as an event.
    c.apply_event("die", &cid('a'), &labels(&[("exitCode", "1")]), 500);
    let h = start(&mut c, &cid('a'), "tond-web", 501).unwrap_or_else(|| panic!("restart"));
    c.record(&state, h).await;
    let events = store::query_events(&state.db, target, 0)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "restart");
}
