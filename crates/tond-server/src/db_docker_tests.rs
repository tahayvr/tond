// db_docker_tests.rs — managed database flows against a real Docker engine
//
// Ignored by default (they pull images and start containers). Run them on a
// machine with Docker:
//
//   cargo nextest run -p tond-server --run-ignored only -E 'test(docker_)'
//
// Each test uses its own data directory and database file, and removes its
// containers and networks at the end.

use std::time::Duration;

use bollard::Docker;
use sqlx::SqlitePool;
use tond_common::{ManagedDatabase, ServerSettings};

use crate::auth::RateLimiter;
use crate::backup::{self, SnapshotTrigger};
use crate::handlers::databases as dbh;
use crate::state::AppState;
use crate::{db_exec, db_upgrade, secrets};

/// Serialises the tests: they share TOND_DATA_DIR.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn docker_state(dir: &std::path::Path) -> AppState {
    // SAFETY: the tests holding LOCK are the only ones reading these.
    unsafe {
        std::env::set_var("TOND_DATA_DIR", dir);
        std::env::set_var("TOND_DB_PATH", dir.join("tond.db"));
    }
    let db: SqlitePool = crate::db::open_pool(&dir.join("tond.db").to_string_lossy())
        .await
        .unwrap_or_else(|e| panic!("db: {e}"));
    crate::db::migrate(&db)
        .await
        .unwrap_or_else(|e| panic!("migrate: {e}"));
    AppState {
        db,
        docker: Docker::connect_with_local_defaults().unwrap_or_else(|e| panic!("docker: {e}")),
        rate_limiter: RateLimiter::new(60, 10.0),
        settings: std::sync::Arc::new(tokio::sync::RwLock::new(ServerSettings::default())),
    }
}

async fn create(state: &AppState, name: &str, db_type: &str, version: &str) -> ManagedDatabase {
    let mut db = ManagedDatabase::new(name.to_string(), db_type.to_string(), None);
    db.version = Some(version.to_string());
    dbh::provision_database(state, &mut db)
        .await
        .unwrap_or_else(|e| panic!("provision {name}: {}", e.0));
    let cid = db.container_id.clone().unwrap_or_default();
    db_exec::wait_ready(&state.docker, &cid, &db, Duration::from_secs(180))
        .await
        .unwrap_or_else(|e| panic!("{name} ready: {e:#}"));
    db
}

async fn reload(state: &AppState, name: &str) -> ManagedDatabase {
    let db: ManagedDatabase = sqlx::query_as("SELECT * FROM databases WHERE name = ?")
        .bind(name)
        .fetch_one(&state.db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    secrets::open_database(db).unwrap_or_else(|e| panic!("{e}"))
}

async fn psql(state: &AppState, db: &ManagedDatabase, sql: &str) -> String {
    let cmd = db_exec::Cmd::new(&[
        "psql",
        "-X",
        "-A",
        "-t",
        "-v",
        "ON_ERROR_STOP=1",
        "-U",
        db.db_user.as_deref().unwrap_or_default(),
        "-d",
        db.db_name.as_deref().unwrap_or_default(),
        "-c",
        sql,
    ]);
    db_exec::run(
        &state.docker,
        db.container_id.as_deref().unwrap_or_default(),
        &cmd,
        Duration::from_secs(30),
        &[],
    )
    .await
    .unwrap_or_else(|e| panic!("psql {sql}: {e:#}"))
    .trim()
    .to_string()
}

async fn redis(state: &AppState, db: &ManagedDatabase, args: &[&str]) -> String {
    let mut argv = vec![
        "redis-cli",
        "--user",
        db.db_user.as_deref().unwrap_or_default(),
        "--no-auth-warning",
    ];
    argv.extend_from_slice(args);
    let cmd = db_exec::Cmd::new(&argv).env(
        "REDISCLI_AUTH",
        db.db_password.as_deref().unwrap_or_default(),
    );
    db_exec::run(
        &state.docker,
        db.container_id.as_deref().unwrap_or_default(),
        &cmd,
        Duration::from_secs(30),
        &[],
    )
    .await
    .unwrap_or_else(|e| panic!("redis {args:?}: {e:#}"))
    .trim()
    .to_string()
}

async fn cleanup(state: &AppState, names: &[&str]) {
    for name in names {
        if let Ok(Some(db)) =
            sqlx::query_as::<_, ManagedDatabase>("SELECT * FROM databases WHERE name = ?")
                .bind(name)
                .fetch_optional(&state.db)
                .await
        {
            let _ = dbh::destroy_database(state, &db).await;
        }
        let _ = crate::docker::remove_container(&state.docker, &format!("tond-db-{name}-upgrade"))
            .await;
    }
}

async fn wait_upgrade(state: &AppState, name: &str) -> tond_common::DbUpgradeState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let db = reload(state, name).await;
        let u = db
            .upgrade_state()
            .unwrap_or_else(|| panic!("no upgrade state"));
        if u.status != "running" {
            return u;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "upgrade timed out at: {}",
            u.step
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn docker_postgres_restores_into_a_new_database_and_upgrades_with_rollback() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let state = docker_state(dir.path()).await;
    let names = ["tpg-src", "tpg-copy"];
    cleanup(&state, &names).await;

    let src = create(&state, "tpg-src", "postgres", "16").await;
    psql(&state, &src, "CREATE EXTENSION IF NOT EXISTS pg_trgm; CREATE TABLE t(id int primary key, v text); INSERT INTO t VALUES (1, 'kept')").await;

    let snap = backup::create_snapshot(
        &state,
        SnapshotTrigger::ManualDatabase {
            db_name: "tpg-src".to_string(),
        },
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("backup: {e:#}"));

    let copy = backup::restore_to_new_database(&state, &snap, "tpg-src", "tpg-copy", None)
        .await
        .unwrap_or_else(|e| panic!("restore into new: {e:#}"));
    assert_eq!(copy.effective_version(), "16");
    assert_ne!(
        copy.db_user, src.db_user,
        "the copy has its own credentials"
    );
    assert_eq!(
        psql(&state, &copy, "SELECT v FROM t WHERE id = 1").await,
        "kept"
    );
    // The original is untouched and still writable.
    psql(&state, &src, "INSERT INTO t VALUES (2, 'after')").await;
    assert_eq!(psql(&state, &copy, "SELECT count(*) FROM t").await, "1");

    // A name that's taken is refused without touching anything.
    let err = backup::restore_to_new_database(&state, &snap, "tpg-src", "tpg-src", None)
        .await
        .err()
        .unwrap_or_else(|| panic!("a taken name must be refused"));
    assert!(format!("{err:#}").contains("already exists"));

    // Upgrade 16 → 17, roll back, upgrade again and confirm.
    let src = reload(&state, "tpg-src").await;
    db_upgrade::start(&state, src, "17")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let u = wait_upgrade(&state, "tpg-src").await;
    assert_eq!(u.status, "awaiting_confirmation", "{:?}", u.error);
    let upgraded = reload(&state, "tpg-src").await;
    assert_eq!(upgraded.effective_version(), "17");
    assert!(
        psql(&state, &upgraded, "SHOW server_version")
            .await
            .starts_with("17")
    );
    assert_eq!(
        psql(
            &state,
            &upgraded,
            "SELECT string_agg(v, ',' ORDER BY id) FROM t"
        )
        .await,
        "kept,after"
    );
    psql(&state, &upgraded, "INSERT INTO t VALUES (3, 'on 17')").await;

    db_upgrade::rollback(&state, &upgraded)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let back = reload(&state, "tpg-src").await;
    assert_eq!(back.effective_version(), "16");
    assert!(back.upgrade.is_none());
    db_exec::wait_ready(
        &state.docker,
        back.container_id.as_deref().unwrap_or_default(),
        &back,
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|e| panic!("{e:#}"));
    assert!(
        psql(&state, &back, "SHOW server_version")
            .await
            .starts_with("16")
    );
    assert_eq!(
        psql(&state, &back, "SELECT count(*) FROM t").await,
        "2",
        "writes on 17 are gone after a rollback"
    );

    db_upgrade::start(&state, back, "17")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        wait_upgrade(&state, "tpg-src").await.status,
        "awaiting_confirmation"
    );
    let upgraded = reload(&state, "tpg-src").await;
    let rollback_dir = upgraded
        .upgrade_state()
        .and_then(|u| u.rollback_dir)
        .unwrap_or_default();
    db_upgrade::confirm(&state, &upgraded)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !std::path::Path::new(&rollback_dir).exists(),
        "confirm deletes the old data"
    );
    assert!(reload(&state, "tpg-src").await.upgrade.is_none());

    cleanup(&state, &names).await;
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn docker_redis_restores_into_a_new_database_and_upgrades() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let state = docker_state(dir.path()).await;
    let names = ["trd-src", "trd-copy"];
    cleanup(&state, &names).await;

    let src = create(&state, "trd-src", "redis", "7").await;
    assert_eq!(
        redis(&state, &src, &["SET", "greeting", "hello"]).await,
        "OK"
    );
    let snap = backup::create_snapshot(
        &state,
        SnapshotTrigger::ManualDatabase {
            db_name: "trd-src".to_string(),
        },
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("backup: {e:#}"));
    let copy = backup::restore_to_new_database(&state, &snap, "trd-src", "trd-copy", None)
        .await
        .unwrap_or_else(|e| panic!("restore into new: {e:#}"));
    assert_eq!(redis(&state, &copy, &["GET", "greeting"]).await, "hello");

    db_upgrade::start(&state, reload(&state, "trd-src").await, "8")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let u = wait_upgrade(&state, "trd-src").await;
    assert_eq!(u.status, "awaiting_confirmation", "{:?}", u.error);
    let upgraded = reload(&state, "trd-src").await;
    assert!(
        redis(&state, &upgraded, &["INFO", "server"])
            .await
            .contains("redis_version:8.")
    );
    assert_eq!(
        redis(&state, &upgraded, &["GET", "greeting"]).await,
        "hello"
    );
    db_upgrade::confirm(&state, &upgraded)
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    cleanup(&state, &names).await;
}
