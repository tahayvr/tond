// compose.rs — Docker Compose apps: services, per-service routing, per-deployment images

use serde::{Deserialize, Serialize};

/// A service of a compose app as the file declares it (from
/// `POST /api/compose/validate` and compose saves), before anything runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ComposeServiceSpec {
    pub name: String,
    /// Image ref from the file (None for build-only services)
    #[serde(default)]
    pub image: Option<String>,
    /// The service has a `build:` section (built by Tond on deploy)
    #[serde(default)]
    pub builds: bool,
    /// Container ports declared under `ports:` and `expose:`
    #[serde(default)]
    pub ports: Vec<u16>,
    /// The service defines a healthcheck
    #[serde(default)]
    pub has_healthcheck: bool,
    /// Replicas the file asks for (`deploy.replicas` or `scale`; 1 when unset)
    #[serde(default = "one_replica")]
    pub replicas: u32,
}

fn one_replica() -> u32 {
    1
}

/// `GET /api/apps/{id}/services` item: one compose service with what Docker
/// reports about it now.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ComposeServiceResponse {
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub builds: bool,
    /// Container ports declared in the file
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub has_healthcheck: bool,
    /// Container port Tond publishes for routing (None = not published)
    #[serde(default)]
    pub published_port: Option<u16>,
    /// Loopback host port the published port is bound to
    #[serde(default)]
    pub host_port: Option<u16>,
    /// The published port changed since the last deploy: redeploy to apply
    #[serde(default)]
    pub redeploy_required: bool,
    /// Domains routed to this service
    #[serde(default)]
    pub domains: Vec<String>,
    /// "pending" | "running" | "restarting" | "paused" | "created" | "exited"
    /// | "dead" | "missing" | "parked"
    pub status: String,
    /// Replicas the file asks for (1 when unset)
    #[serde(default = "one_replica")]
    pub replicas: u32,
    /// The service is no longer in the compose file (for example after a
    /// rollback to an older file): it does not run and its domains are not
    /// routed, but they are kept and come back when the file declares the
    /// service again.
    #[serde(default)]
    pub parked: bool,
    /// "healthy" | "unhealthy" | "starting"; None without a healthcheck
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub container_id: Option<String>,
    /// Pinned local image the container runs (`tond-<app>/<service>:<image id prefix>`)
    #[serde(default)]
    pub running_image: Option<String>,
    /// Image id of the running image (sha256:...)
    #[serde(default)]
    pub image_digest: Option<String>,
    /// Latest CPU use in 1/100 % of one core (None before the first sample)
    #[serde(default)]
    pub cpu: Option<i64>,
    /// Latest memory use in bytes
    #[serde(default)]
    pub mem: Option<i64>,
}

/// `PUT /api/apps/{id}/services/{service}`: publish a container port and
/// route domains to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateComposeServiceRequest {
    /// Container port to publish on a loopback host port (None = none)
    #[serde(default)]
    pub published_port: Option<u16>,
    /// Domains to route to the published port (replaces the current list)
    #[serde(default)]
    pub domains: Vec<String>,
}

/// `PUT /api/apps/{id}/compose`: replace the compose file (inline) or the
/// repository location of a git-sourced compose app.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateComposeRequest {
    #[serde(default)]
    pub compose_content: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub compose_path: Option<String>,
}

/// One service of a compose deployment: how its image was produced and the
/// pinned tag rollback restores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeploymentService {
    pub service: String,
    /// "build" | "pull" | "reuse"
    pub source: String,
    #[serde(default)]
    pub source_image: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    /// "pending" | "building" | "pulling" | "ready" | "starting" | "healthy"
    /// | "running" | "failed"
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_update_defaults_to_nothing_published() {
        let req: UpdateComposeServiceRequest =
            serde_json::from_str("{}").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(req.published_port, None);
        assert!(req.domains.is_empty());
    }
}

// ── Backup hooks (compose labels) ────────────────────────────────────────────

/// Compose label: a command run with `sh -c` inside the service before its
/// volumes are copied into a snapshot. Its stdout is stored in the snapshot as
/// the service's dump (a command may instead write into its own volume, e.g.
/// `redis-cli SAVE`).
pub const BACKUP_DUMP_LABEL: &str = "tond.backup.dump";
/// Compose label: a command run with `sh -c` inside the service after a
/// restore, with the service's dump on stdin.
pub const BACKUP_RESTORE_LABEL: &str = "tond.backup.restore";
/// Longest accepted backup hook command.
pub const BACKUP_HOOK_MAX_LEN: usize = 2048;

/// Check the value of a backup hook label: a non-empty, single-line command
/// of at most `BACKUP_HOOK_MAX_LEN` bytes.
pub fn check_backup_hook(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("label {label} must not be empty"));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(format!("label {label} must be a single-line command"));
    }
    if value.len() > BACKUP_HOOK_MAX_LEN {
        return Err(format!(
            "label {label} is longer than {BACKUP_HOOK_MAX_LEN} characters"
        ));
    }
    Ok(())
}

/// The database engine an image runs, for images whose files can't be copied
/// consistently while running (postgres and its variants, mysql, mariadb,
/// mongo, redis and its forks). Matches on the image name without registry,
/// namespace and tag: `postgres`, `pgvector/pgvector`,
/// `ghcr.io/immich-app/postgres`, `timescale/timescaledb`, `postgis/postgis`,
/// `bitnami/mysql`, `valkey/valkey`, …
pub fn database_engine_of_image(image: &str) -> Option<&'static str> {
    let without_digest = image.split('@').next().unwrap_or(image);
    let last = without_digest.rsplit('/').next().unwrap_or(without_digest);
    let name = last.split(':').next().unwrap_or(last).to_ascii_lowercase();
    match name.as_str() {
        "postgres" | "postgresql" | "pgvector" | "timescaledb" | "timescaledb-ha" | "postgis" => {
            Some("postgres")
        }
        "mysql" | "percona-server" => Some("mysql"),
        "mariadb" => Some("mariadb"),
        "mongo" | "mongodb" | "mongodb-community-server" => Some("mongo"),
        "redis" | "redis-stack-server" | "valkey" | "keydb" => Some("redis"),
        _ => None,
    }
}

/// The value of `key` in a compose `labels:` section (a mapping, or a list of
/// `key=value` strings).
pub fn compose_label(labels: &serde_yaml::Value, key: &str) -> Option<String> {
    use serde_yaml::Value;
    let scalar = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    match labels {
        Value::Mapping(m) => m
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .and_then(|(_, v)| scalar(v)),
        Value::Sequence(items) => items.iter().find_map(|item| {
            let (k, v) = item.as_str()?.split_once('=')?;
            (k.trim() == key).then(|| v.to_string())
        }),
        Value::Tagged(t) => compose_label(&t.value, key),
        _ => None,
    }
}

#[cfg(test)]
mod backup_hook_tests {
    use super::*;

    #[test]
    fn recognises_database_images() {
        for (image, engine) in [
            ("postgres:17-alpine", Some("postgres")),
            (
                "ghcr.io/immich-app/postgres:14-vectorchord0.4.3",
                Some("postgres"),
            ),
            ("pgvector/pgvector:pg17", Some("postgres")),
            ("timescale/timescaledb:2.17.0-pg16", Some("postgres")),
            ("mysql:8.4", Some("mysql")),
            ("mariadb:11@sha256:abc", Some("mariadb")),
            ("mongo:8", Some("mongo")),
            ("valkey/valkey:8-alpine", Some("redis")),
            ("redis:7", Some("redis")),
            ("ghost:6-alpine", None),
            ("clickhouse/clickhouse-server:24.12", None),
            ("registry.example.com:5000/postgres-exporter:1", None),
        ] {
            assert_eq!(database_engine_of_image(image), engine, "{image}");
        }
    }

    #[test]
    fn reads_labels_in_both_forms() {
        let map: serde_yaml::Value =
            serde_yaml::from_str("tond.backup.dump: pg_dumpall -U x\nother: 1")
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            compose_label(&map, BACKUP_DUMP_LABEL).as_deref(),
            Some("pg_dumpall -U x")
        );
        let list: serde_yaml::Value =
            serde_yaml::from_str("- a=b\n- tond.backup.restore=psql -c 'x=1'")
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            compose_label(&list, BACKUP_RESTORE_LABEL).as_deref(),
            Some("psql -c 'x=1'")
        );
        assert_eq!(compose_label(&list, BACKUP_DUMP_LABEL), None);
    }

    #[test]
    fn hook_commands_are_single_non_empty_lines() {
        assert!(check_backup_hook(BACKUP_DUMP_LABEL, "pg_dumpall").is_ok());
        assert!(check_backup_hook(BACKUP_DUMP_LABEL, "  ").is_err());
        assert!(check_backup_hook(BACKUP_DUMP_LABEL, "a\nb").is_err());
        assert!(
            check_backup_hook(BACKUP_DUMP_LABEL, &"x".repeat(BACKUP_HOOK_MAX_LEN + 1)).is_err()
        );
    }
}
