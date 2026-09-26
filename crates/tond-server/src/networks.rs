// networks.rs — per-tenant Docker networks (M9)
//
// Nothing Tond starts joins Docker's default bridge, where every container can
// reach every other one. Instead each workload has a *home network*:
//
//   app in a project       tond-<project_id>          shared by the project (icc on)
//   app without a project  tond-net-app-<app>         the app + databases linked to it (icc on)
//   database in a project  tond-<project_id>
//   database, no project   tond-net-db-<db>           the database alone (icc off)
//   cron job in a project  tond-<project_id>
//   cron job, no project   tond-net-cron-<job id>     the job alone (icc off)
//   preview                tond-net-preview-<slug>    the preview; a project preview's
//                                                     apps share the slug (icc on)
//
// Docker isolates bridge networks from each other, so a container can only
// reach containers on its own networks (by alias or IP). tond-proxy is
// unaffected: it reaches apps through their published 127.0.0.1 ports.
//
// Database links (`database_links`) make access outside a project explicit:
// linking database D to app A connects D's container to A's home network with
// D's name as its alias. D is multi-homed; A reaches D as `D:<port>`, and two
// apps linked to the same database still cannot reach each other (they are on
// different networks). An app in a project shares its project network, so a
// link to such an app makes the database reachable by the whole project.
//
// Compose apps run on their own compose network (`<project>_default`) and
// cannot be linked; compose_policy refuses external networks and the default
// bridge.
//
// Previews never join the project network: they run pull-request code and
// must not reach production databases.
//
//   *_network()/ *_home()    network names
//   ensure_*_home()          create the home network if missing
//   reconcile_database()     attach a database container to exactly the
//                            networks it should be on (home + linked apps)
//   reconcile_app()          move an app container to its current home
//                            network (after a project change)
//   linked_apps / links_for_app  read database_links

use std::collections::BTreeSet;

use anyhow::Result;
use sqlx::SqlitePool;
use tond_common::{App, CronJob, ManagedDatabase};

use crate::{docker, state::AppState};

/// Docker's default bridge: never a home network, always detached on reconcile.
const DEFAULT_BRIDGE: &str = "bridge";

pub fn project_network(project_id: &str) -> String {
    format!("tond-{project_id}")
}

pub fn app_network(app_name: &str) -> String {
    format!("tond-net-app-{app_name}")
}

pub fn database_network(db_name: &str) -> String {
    format!("tond-net-db-{db_name}")
}

pub fn cron_network(job_id: &str) -> String {
    format!("tond-net-cron-{job_id}")
}

pub fn preview_network(slug: &str) -> String {
    format!("tond-net-preview-{slug}")
}

/// The network an app's container runs on.
pub fn app_home(app: &App) -> String {
    match app.project_id.as_deref() {
        Some(pid) => project_network(pid),
        None => app_network(&app.name),
    }
}

/// The network a database container is created on (links add more).
pub fn database_home(db: &ManagedDatabase) -> String {
    match db.project_id.as_deref() {
        Some(pid) => project_network(pid),
        None => database_network(&db.name),
    }
}

/// The network a cron job's container runs on.
pub fn cron_home(job: &CronJob) -> String {
    match job.project_id.as_deref() {
        Some(pid) => project_network(pid),
        None => cron_network(&job.id),
    }
}

/// Networks Tond manages (and may attach or detach containers from).
fn is_managed(name: &str) -> bool {
    name.starts_with("tond-") || name == DEFAULT_BRIDGE
}

/// Create (if missing) and return an app's home network. Project and app
/// networks keep inter-container traffic on: project members talk to each
/// other, and an app talks to the databases linked to it.
pub async fn ensure_app_home(docker: &bollard::Docker, app: &App) -> Result<String> {
    let name = app_home(app);
    docker::ensure_network(docker, &name, true).await?;
    Ok(name)
}

/// Create (if missing) and return a database's home network. A database's own
/// network holds only the database, so inter-container traffic is off.
pub async fn ensure_database_home(
    docker: &bollard::Docker,
    db: &ManagedDatabase,
) -> Result<String> {
    let name = database_home(db);
    docker::ensure_network(docker, &name, db.project_id.is_some()).await?;
    Ok(name)
}

/// Create (if missing) and return a cron job's home network.
pub async fn ensure_cron_home(docker: &bollard::Docker, job: &CronJob) -> Result<String> {
    let name = cron_home(job);
    docker::ensure_network(docker, &name, job.project_id.is_some()).await?;
    Ok(name)
}

/// Create (if missing) and return a preview's own network. The apps of a
/// project preview share one slug and so one network, where they reach each
/// other by app name — like the project network, but without production.
pub async fn ensure_preview(docker: &bollard::Docker, slug: &str) -> Result<String> {
    let name = preview_network(slug);
    docker::ensure_network(docker, &name, true).await?;
    Ok(name)
}

// ── Links ────────────────────────────────────────────────────────────────────

/// Apps explicitly linked to a database (oldest link first).
pub async fn linked_apps(pool: &SqlitePool, database_id: &str) -> Result<Vec<App>> {
    Ok(sqlx::query_as::<_, App>(
        "SELECT a.* FROM apps a JOIN database_links l ON l.app_id = a.id
         WHERE l.database_id = ? ORDER BY l.created_at, a.name",
    )
    .bind(database_id)
    .fetch_all(pool)
    .await?)
}

/// Databases explicitly linked to an app.
pub async fn links_for_app(pool: &SqlitePool, app_id: &str) -> Result<Vec<ManagedDatabase>> {
    Ok(sqlx::query_as::<_, ManagedDatabase>(
        "SELECT d.* FROM databases d JOIN database_links l ON l.database_id = d.id
         WHERE l.app_id = ? ORDER BY d.name",
    )
    .bind(app_id)
    .fetch_all(pool)
    .await?)
}

/// Every network a database container should be attached to: its home plus
/// the home network of each linked app (compose apps can't be linked).
pub fn desired_database_networks(db: &ManagedDatabase, linked: &[App]) -> BTreeSet<String> {
    let mut desired = BTreeSet::from([database_home(db)]);
    desired.extend(
        linked
            .iter()
            .filter(|a| a.source_type != "compose")
            .map(app_home),
    );
    desired
}

/// Which managed networks to connect and disconnect so a container is on
/// exactly `desired`. Networks Tond doesn't manage are left alone; the
/// default bridge is always dropped.
pub fn plan_changes(current: &[String], desired: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    let connect = desired
        .iter()
        .filter(|n| !current.contains(n))
        .cloned()
        .collect();
    let disconnect = current
        .iter()
        .filter(|n| is_managed(n) && !desired.contains(*n))
        .cloned()
        .collect();
    (connect, disconnect)
}

/// Attach `container` to `desired` networks (with `alias`) and detach it from
/// other managed ones. Connects first, so the container always has a network.
async fn reconcile_container(
    docker: &bollard::Docker,
    container: &str,
    desired: &BTreeSet<String>,
    alias: &str,
) -> Result<()> {
    let current = docker::container_networks(docker, container).await?;
    let (connect, disconnect) = plan_changes(&current, desired);
    for network in &connect {
        docker::connect_network(docker, network, container, alias).await?;
    }
    for network in &disconnect {
        docker::disconnect_network(docker, network, container).await?;
    }
    if !connect.is_empty() || !disconnect.is_empty() {
        tracing::info!("Networks for {}: +{:?} -{:?}", alias, connect, disconnect);
    }
    Ok(())
}

/// Put a database container on exactly its home network plus the home
/// network of every linked app, creating networks as needed. A database
/// without a container is left alone (it is attached when it is created).
pub async fn reconcile_database(state: &AppState, db: &ManagedDatabase) -> Result<()> {
    let Some(container) = db.container_id.as_deref() else {
        return Ok(());
    };
    let linked = linked_apps(&state.db, &db.id).await?;
    ensure_database_home(&state.docker, db).await?;
    for app in linked.iter().filter(|a| a.source_type != "compose") {
        ensure_app_home(&state.docker, app).await?;
    }
    let desired = desired_database_networks(db, &linked);
    reconcile_container(&state.docker, container, &desired, &db.name).await
}

/// Re-attach every database linked to (or in the project of) an app — after
/// the app's home network changed or was recreated.
pub async fn reconcile_databases_for_app(state: &AppState, app: &App) -> Result<()> {
    for db in links_for_app(&state.db, &app.id).await? {
        reconcile_database(state, &db).await?;
    }
    Ok(())
}

/// Move an app's container to its current home network (after a project
/// change), then re-attach its linked databases. Compose apps and apps
/// without a container are left alone; their next deploy uses the new home.
pub async fn reconcile_app(state: &AppState, app: &App) -> Result<()> {
    if app.source_type != "compose"
        && let Some(container) = app.container_id.as_deref()
    {
        let home = ensure_app_home(&state.docker, app).await?;
        reconcile_container(&state.docker, container, &BTreeSet::from([home]), &app.name).await?;
    }
    reconcile_databases_for_app(state, app).await
}

/// At startup: make sure every database is on the networks its links say.
/// Best-effort per database (logged), so one broken container can't stop the
/// server from starting.
pub async fn reconcile_all_databases(state: &AppState) {
    let dbs: Vec<ManagedDatabase> = match sqlx::query_as("SELECT * FROM databases")
        .fetch_all(&state.db)
        .await
    {
        Ok(dbs) => dbs,
        Err(e) => {
            tracing::warn!("Could not list databases for network reconcile: {e}");
            return;
        }
    };
    for db in dbs {
        if let Err(e) = reconcile_database(state, &db).await {
            tracing::warn!("Could not reconcile networks for database {}: {e}", db.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, project: Option<&str>) -> App {
        let mut app = App::new(name.to_string(), "git".to_string(), None, None, 3000);
        app.project_id = project.map(str::to_string);
        app
    }

    fn db(name: &str, project: Option<&str>) -> ManagedDatabase {
        ManagedDatabase::new(
            name.to_string(),
            "postgres".to_string(),
            project.map(str::to_string),
        )
    }

    #[test]
    fn home_networks_follow_project_membership() {
        assert_eq!(app_home(&app("web", None)), "tond-net-app-web");
        assert_eq!(app_home(&app("web", Some("p1"))), "tond-p1");
        assert_eq!(database_home(&db("main", None)), "tond-net-db-main");
        assert_eq!(database_home(&db("main", Some("p1"))), "tond-p1");
        assert_eq!(preview_network("abc123"), "tond-net-preview-abc123");
        assert_eq!(cron_network("job-1"), "tond-net-cron-job-1");
    }

    #[test]
    fn desired_database_networks_add_linked_app_homes_but_not_compose_apps() {
        let mut compose = app("stack", None);
        compose.source_type = "compose".to_string();
        let desired = desired_database_networks(
            &db("main", None),
            &[app("web", None), app("api", Some("p1")), compose],
        );
        assert_eq!(
            desired,
            BTreeSet::from([
                "tond-net-db-main".to_string(),
                "tond-net-app-web".to_string(),
                "tond-p1".to_string(),
            ]),
            "a database joins its own network plus each linked app's home network"
        );
    }

    #[test]
    fn plan_changes_detaches_the_default_bridge_and_stale_tond_networks_only() {
        let current = vec![
            "bridge".to_string(),
            "tond-net-db-main".to_string(),
            "tond-net-app-old".to_string(),
            "someone-elses-net".to_string(),
        ];
        let desired = BTreeSet::from([
            "tond-net-db-main".to_string(),
            "tond-net-app-web".to_string(),
        ]);
        let (connect, disconnect) = plan_changes(&current, &desired);
        assert_eq!(connect, vec!["tond-net-app-web".to_string()]);
        assert_eq!(
            disconnect,
            vec!["bridge".to_string(), "tond-net-app-old".to_string()],
            "unmanaged networks are never touched"
        );
    }
}
