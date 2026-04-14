// databases.rs — managed database engine config and all database types

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::generate_raw_key;

// ── DbEngineConfig ──

/// Compile-time configuration for a single supported database engine.
///
/// Adding a new engine (e.g. MongoDB, KeyDB) is a **one-place change**:
/// add an entry to `ALL_ENGINES` below. Every caller in the handler that
/// previously used a scattered `match db_type.as_str()` now drives from
/// this table instead.
#[derive(Debug, Clone)]
pub struct DbEngineConfig {
    /// Canonical type string sent from the API (e.g. "postgres")
    pub db_type: &'static str,
    /// Docker image used to run this engine
    pub image: &'static str,
    /// Container-internal TCP port the engine listens on
    pub container_port: u16,
    /// URL scheme prefix for connection strings (e.g. "postgres", "redis")
    pub url_scheme: &'static str,
    /// Whether this engine uses credentials (db_name / db_user / db_password).
    /// Credential-less engines (Redis, KeyDB, Valkey) set this to false.
    pub uses_credentials: bool,
    /// Host path inside the container where data should be volume-mounted
    pub data_dir: &'static str,
}

impl DbEngineConfig {
    /// Build the environment variable map for `docker run`.
    /// Returns an empty map for credential-less engines.
    pub fn build_env(
        &self,
        db_name: Option<&str>,
        db_user: Option<&str>,
        db_password: Option<&str>,
        root_password: Option<&str>,
    ) -> std::collections::HashMap<String, String> {
        let mut env = std::collections::HashMap::new();
        if !self.uses_credentials {
            return env;
        }
        let name = db_name.unwrap_or_default();
        let user = db_user.unwrap_or_default();
        let pass = db_password.unwrap_or_default();
        let root = root_password.unwrap_or(pass); // fallback: mirror app password
        match self.db_type {
            "postgres" => {
                env.insert("POSTGRES_DB".into(), name.into());
                env.insert("POSTGRES_USER".into(), user.into());
                env.insert("POSTGRES_PASSWORD".into(), pass.into());
            }
            "mysql" => {
                env.insert("MYSQL_DATABASE".into(), name.into());
                env.insert("MYSQL_USER".into(), user.into());
                env.insert("MYSQL_PASSWORD".into(), pass.into());
                env.insert("MYSQL_ROOT_PASSWORD".into(), root.into());
            }
            "mariadb" => {
                env.insert("MARIADB_DATABASE".into(), name.into());
                env.insert("MARIADB_USER".into(), user.into());
                env.insert("MARIADB_PASSWORD".into(), pass.into());
                env.insert("MARIADB_ROOT_PASSWORD".into(), root.into());
            }
            "mongodb" => {
                env.insert("MONGO_INITDB_DATABASE".into(), name.into());
                env.insert("MONGO_INITDB_ROOT_USERNAME".into(), user.into());
                env.insert("MONGO_INITDB_ROOT_PASSWORD".into(), pass.into());
            }
            // Redis / KeyDB: use requirepass for the default ACL user.
            // REDIS_USERNAME is informational only — Redis doesn't have a
            // dedicated env for username, but we expose it to match the
            // REDIS_USERNAME / REDIS_PASSWORD pattern expected by clients.
            "redis" | "keydb" => {
                env.insert("REDIS_PASSWORD".into(), pass.into());
                env.insert("REDIS_USERNAME".into(), user.into());
            }
            _ => {}
        }
        env
    }

    /// Build a connection string for external access via the host port.
    pub fn external_connection_string(
        &self,
        host_port: i64,
        db_name: Option<&str>,
        db_user: Option<&str>,
        db_password: Option<&str>,
    ) -> String {
        if self.uses_credentials {
            if db_name.is_some_and(|n| !n.is_empty()) {
                format!(
                    "{}://{}:{}@localhost:{}/{}",
                    self.url_scheme,
                    db_user.unwrap_or_default(),
                    db_password.unwrap_or_default(),
                    host_port,
                    db_name.unwrap_or_default(),
                )
            } else {
                // Engines that use credentials but have no named database (e.g. Redis)
                format!(
                    "{}://{}:{}@localhost:{}",
                    self.url_scheme,
                    db_user.unwrap_or_default(),
                    db_password.unwrap_or_default(),
                    host_port,
                )
            }
        } else {
            format!("{}://localhost:{}", self.url_scheme, host_port)
        }
    }

    /// Build a connection string for internal access within the project network,
    /// using the database's Docker network alias as the hostname.
    pub fn internal_connection_string(
        &self,
        alias: &str,
        db_name: Option<&str>,
        db_user: Option<&str>,
        db_password: Option<&str>,
    ) -> String {
        if self.uses_credentials {
            if db_name.is_some_and(|n| !n.is_empty()) {
                format!(
                    "{}://{}:{}@{}:{}/{}",
                    self.url_scheme,
                    db_user.unwrap_or_default(),
                    db_password.unwrap_or_default(),
                    alias,
                    self.container_port,
                    db_name.unwrap_or_default(),
                )
            } else {
                // Engines with credentials but no named database (e.g. Redis)
                format!(
                    "{}://{}:{}@{}:{}",
                    self.url_scheme,
                    db_user.unwrap_or_default(),
                    db_password.unwrap_or_default(),
                    alias,
                    self.container_port,
                )
            }
        } else {
            format!("{}://{}:{}", self.url_scheme, alias, self.container_port)
        }
    }
}

/// All supported database engines.
///
/// To add a new engine:
///   1. Add an entry here.
///   2. Update the dashboard `DbType` union in `db-wizard-config.svelte`.
///   3. That is all — the handler, connection strings, and credential
///      generation are all driven from this table.
pub const ALL_ENGINES: &[DbEngineConfig] = &[
    DbEngineConfig {
        db_type: "postgres",
        image: "postgres:16-alpine",
        container_port: 5432,
        url_scheme: "postgres",
        uses_credentials: true,
        data_dir: "/var/lib/postgresql/data",
    },
    DbEngineConfig {
        db_type: "mysql",
        image: "mysql:8",
        container_port: 3306,
        url_scheme: "mysql",
        uses_credentials: true,
        data_dir: "/var/lib/mysql",
    },
    DbEngineConfig {
        db_type: "redis",
        image: "redis:7-alpine",
        container_port: 6379,
        url_scheme: "redis",
        uses_credentials: true,
        data_dir: "/data",
    },
    DbEngineConfig {
        db_type: "keydb",
        image: "eqalpha/keydb:latest",
        container_port: 6379,
        url_scheme: "redis", // KeyDB is wire-compatible with Redis
        uses_credentials: true,
        data_dir: "/data",
    },
    DbEngineConfig {
        db_type: "mariadb",
        image: "mariadb:11",
        container_port: 3306,
        url_scheme: "mysql", // MariaDB is wire-compatible with MySQL
        uses_credentials: true,
        data_dir: "/var/lib/mysql",
    },
    DbEngineConfig {
        db_type: "mongodb",
        image: "mongo:7",
        container_port: 27017,
        url_scheme: "mongodb",
        uses_credentials: true,
        data_dir: "/data/db",
    },
];

/// Look up the engine configuration for a given `db_type` string.
/// Returns `None` if the type is not in `ALL_ENGINES`.
pub fn engine_for(db_type: &str) -> Option<&'static DbEngineConfig> {
    ALL_ENGINES.iter().find(|e| e.db_type == db_type)
}

/// A managed database container created and supervised by Tond.
/// One row per database instance.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ManagedDatabase {
    pub id: String,
    /// User-chosen name; also used as the Docker container name suffix
    /// (`tond-db-{name}`) and as the network alias within a project.
    pub name: String,
    /// "postgres" | "mysql" | "redis"
    pub db_type: String,
    /// Optional project association — when set the container is also
    /// connected to the `tond-{project_id}` bridge network.
    pub project_id: Option<String>,
    /// Docker container ID (set after the container starts)
    pub container_id: Option<String>,
    /// Host port bound for external access (set after the container starts)
    pub host_port: Option<i64>,
    /// "pending" | "running" | "stopped" | "failed"
    pub status: String,
    /// Database name inside the engine (postgres / mysql only)
    pub db_name: Option<String>,
    /// Database user (postgres / mysql only)
    pub db_user: Option<String>,
    /// Database password (stored in plain text so apps can inject it)
    pub db_password: Option<String>,
    /// Separate root/admin password for MySQL and MariaDB.
    /// These engines require a distinct `MYSQL_ROOT_PASSWORD` /
    /// `MARIADB_ROOT_PASSWORD` in addition to the application-user password.
    /// `None` for engines that do not use a separate root credential
    /// (postgres, redis, keydb, mongodb).
    pub root_password: Option<String>,
    /// Absolute host path for the persistent data volume
    pub volume_path: Option<String>,
    /// Optional memory cap in bytes (None = no limit)
    pub memory_limit: Option<i64>,
    /// User-supplied override for the external connection string.
    /// When set, supersedes the auto-computed `host_port`-based string.
    pub custom_external_url: Option<String>,
    /// User-supplied override for the internal (project-network) connection string.
    /// When set, supersedes the auto-computed container-alias-based string.
    pub custom_internal_url: Option<String>,
    /// The Docker image actually used when the container was last created or redeployed.
    /// Populated on create and on every redeploy; `None` for rows created before this
    /// field was added.
    pub image_used: Option<String>,
    /// User-supplied Docker image override. When non-`None`, this image is used instead
    /// of the engine default on the next redeploy (e.g. `"postgres:15-alpine"`).
    pub custom_image: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl ManagedDatabase {
    /// Construct a new `ManagedDatabase` with auto-generated credentials.
    /// The container is not started yet — `status` starts as `"pending"`.
    pub fn new(name: String, db_type: String, project_id: Option<String>) -> Self {
        let now = Utc::now().to_rfc3339();

        // Look up whether this engine uses credentials.
        // We prefix with "tond_" to:
        //   1. Avoid PostgreSQL's reserved "pg_" role-name prefix (which causes
        //      initdb to reject any username that begins with "pg_").
        //   2. Make it obvious in pg_roles/pg_databases that the object is
        //      managed by Tond.
        let safe_name = name.replace('-', "_");
        let uses_credentials = engine_for(&db_type).is_some_and(|e| e.uses_credentials);
        let has_db_name = engine_for(&db_type)
            .is_some_and(|e| e.uses_credentials && e.db_type != "redis" && e.db_type != "keydb");
        let db_name = if has_db_name {
            Some(format!("tond_{}", safe_name))
        } else {
            None
        };
        let db_user = if uses_credentials {
            Some(format!("tond_{}", safe_name))
        } else {
            None
        };
        let db_password = if uses_credentials {
            Some(generate_raw_key()[..24].to_string()) // 24-char random hex
        } else {
            None
        };
        // MySQL and MariaDB require a separate root password distinct from the
        // application-user password.  Other engines set this to None.
        let needs_root_password =
            engine_for(&db_type).is_some_and(|e| e.db_type == "mysql" || e.db_type == "mariadb");
        let root_password = if needs_root_password {
            Some(generate_raw_key()[..24].to_string())
        } else {
            None
        };

        Self {
            id: Uuid::new_v4().to_string(),
            name,
            db_type,
            project_id,
            container_id: None,
            host_port: None,
            status: "pending".to_string(),
            db_name,
            db_user,
            db_password,
            root_password,
            volume_path: None,
            memory_limit: None,
            custom_external_url: None,
            custom_internal_url: None,
            image_used: None,
            custom_image: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// Full API view for a managed database.
/// Password is included so callers can build connection strings.
#[derive(Debug, Serialize, Deserialize)]
pub struct ManagedDatabaseResponse {
    pub id: String,
    pub name: String,
    pub db_type: String,
    pub project_id: Option<String>,
    pub host_port: Option<i64>,
    pub status: String,
    pub db_name: Option<String>,
    pub db_user: Option<String>,
    pub db_password: Option<String>,
    /// Separate root/admin password for MySQL and MariaDB.
    /// `None` for engines that do not use a separate root credential.
    pub root_password: Option<String>,
    pub memory_limit: Option<i64>,
    pub custom_external_url: Option<String>,
    pub custom_internal_url: Option<String>,
    /// Docker image used for the running container (populated on create/redeploy)
    pub image_used: Option<String>,
    /// User-supplied image override for the next redeploy
    pub custom_image: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ManagedDatabase> for ManagedDatabaseResponse {
    fn from(d: ManagedDatabase) -> Self {
        Self {
            id: d.id,
            name: d.name,
            db_type: d.db_type,
            project_id: d.project_id,
            host_port: d.host_port,
            status: d.status,
            db_name: d.db_name,
            db_user: d.db_user,
            db_password: d.db_password,
            root_password: d.root_password,
            memory_limit: d.memory_limit,
            custom_external_url: d.custom_external_url,
            custom_internal_url: d.custom_internal_url,
            image_used: d.image_used,
            custom_image: d.custom_image,
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

/// GET /api/databases/{id} response — includes generated connection strings.
#[derive(Debug, Serialize, Deserialize)]
pub struct ManagedDatabaseDetail {
    #[serde(flatten)]
    pub database: ManagedDatabaseResponse,
    /// External connection string (using host port on localhost)
    pub connection_string: Option<String>,
    /// Internal connection string (using container alias within the project network)
    pub internal_connection_string: Option<String>,
}

/// POST /api/databases request body
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
    /// "postgres" | "mysql" | "redis"
    pub db_type: String,
    /// Optional project to associate with (also connects to project network)
    pub project_id: Option<String>,
    /// Optional memory cap in bytes
    pub memory_limit: Option<i64>,
}

/// PATCH /api/databases/{id} request body
///
/// Only the fields present in the request body are updated (partial update).
/// Setting a field to an empty string clears the override and reverts to the
/// auto-computed value.
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateDatabaseRequest {
    /// Override for the external connection string (replaces the auto-computed one).
    /// Send `null` or omit to leave unchanged; send `""` to clear the override.
    pub custom_external_url: Option<String>,
    /// Override for the internal connection string (replaces the auto-computed one).
    /// Send `null` or omit to leave unchanged; send `""` to clear the override.
    pub custom_internal_url: Option<String>,
    /// Custom Docker image to use on the next redeploy (e.g. `"postgres:15-alpine"`).
    /// Send `null` or omit to leave unchanged; send `""` to clear and use the engine default.
    pub custom_image: Option<String>,
    /// Optional memory cap in bytes (None = no limit).
    /// Send `null` or omit to leave unchanged; send `0` to clear the limit.
    pub memory_limit: Option<i64>,
}

/// PATCH /api/databases/{id}/credentials request body
///
/// Updates the stored credentials (username, password, database name).
/// The changes take effect on the next redeploy — the running container is not
/// restarted automatically.  Send `null` or omit a field to leave it unchanged.
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateDatabaseCredentials {
    /// New database name (postgres / mysql / mongodb only).
    pub db_name: Option<String>,
    /// New database username.
    pub db_user: Option<String>,
    /// New database password.  Send `""` to leave the current value unchanged.
    pub db_password: Option<String>,
    /// New root/admin password (MySQL and MariaDB only).
    /// Send `""` to leave the current value unchanged.
    pub root_password: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn engine_for_returns_expected_engines_and_unknown_types_are_rejected() {
        assert_eq!(
            engine_for("postgres").map(|e| e.image),
            Some("postgres:16-alpine"),
            "engine_for should return the postgres engine config"
        );
        assert_eq!(
            engine_for("mongodb").map(|e| e.container_port),
            Some(27017),
            "engine_for should return the mongodb engine config"
        );
        assert!(
            engine_for("sqlite").is_none(),
            "engine_for should return None for unsupported engine types"
        );
    }

    #[test]
    fn all_engines_have_unique_type_names() {
        let unique_types: HashSet<&str> = ALL_ENGINES.iter().map(|engine| engine.db_type).collect();

        assert_eq!(
            unique_types.len(),
            ALL_ENGINES.len(),
            "ALL_ENGINES should not contain duplicate db_type entries"
        );
    }

    #[test]
    fn build_env_sets_expected_engine_specific_variables() {
        let postgres =
            engine_for("postgres").unwrap_or_else(|| panic!("expected postgres engine to exist"));
        let postgres_env = postgres.build_env(Some("tond_demo"), Some("user"), Some("pass"), None);
        assert_eq!(
            postgres_env.get("POSTGRES_DB"),
            Some(&"tond_demo".to_string()),
            "postgres env should include POSTGRES_DB"
        );
        assert_eq!(
            postgres_env.get("POSTGRES_USER"),
            Some(&"user".to_string()),
            "postgres env should include POSTGRES_USER"
        );
        assert_eq!(
            postgres_env.get("POSTGRES_PASSWORD"),
            Some(&"pass".to_string()),
            "postgres env should include POSTGRES_PASSWORD"
        );

        let mysql = engine_for("mysql").unwrap_or_else(|| panic!("expected mysql engine to exist"));
        let mysql_env = mysql.build_env(Some("tond_demo"), Some("user"), Some("pass"), None);
        assert_eq!(
            mysql_env.get("MYSQL_ROOT_PASSWORD"),
            Some(&"pass".to_string()),
            "mysql should fall back to the app password when no root password is provided"
        );

        let mongodb =
            engine_for("mongodb").unwrap_or_else(|| panic!("expected mongodb engine to exist"));
        let mongodb_env = mongodb.build_env(Some("tond_demo"), Some("root"), Some("secret"), None);
        assert_eq!(
            mongodb_env.get("MONGO_INITDB_DATABASE"),
            Some(&"tond_demo".to_string()),
            "mongodb env should include the initial database name"
        );
        assert_eq!(
            mongodb_env.get("MONGO_INITDB_ROOT_USERNAME"),
            Some(&"root".to_string()),
            "mongodb env should include the root username"
        );

        let redis = engine_for("redis").unwrap_or_else(|| panic!("expected redis engine to exist"));
        let redis_env = redis.build_env(None, Some("default"), Some("secret"), None);
        assert_eq!(
            redis_env.get("REDIS_USERNAME"),
            Some(&"default".to_string()),
            "redis env should expose the username for clients that expect it"
        );
        assert_eq!(
            redis_env.get("REDIS_PASSWORD"),
            Some(&"secret".to_string()),
            "redis env should expose the password for requirepass-style setup"
        );
    }

    #[test]
    fn connection_string_helpers_format_named_and_nameless_engines_correctly() {
        let postgres =
            engine_for("postgres").unwrap_or_else(|| panic!("expected postgres engine to exist"));
        assert_eq!(
            postgres.external_connection_string(
                54321,
                Some("tond_demo"),
                Some("user"),
                Some("pass")
            ),
            "postgres://user:pass@localhost:54321/tond_demo",
            "postgres external connection strings should include the database name"
        );
        assert_eq!(
            postgres.internal_connection_string(
                "db-alias",
                Some("tond_demo"),
                Some("user"),
                Some("pass")
            ),
            "postgres://user:pass@db-alias:5432/tond_demo",
            "postgres internal connection strings should use the network alias and container port"
        );

        let redis = engine_for("redis").unwrap_or_else(|| panic!("expected redis engine to exist"));
        assert_eq!(
            redis.external_connection_string(6380, None, Some("default"), Some("secret")),
            "redis://default:secret@localhost:6380",
            "redis external connection strings should omit the database path"
        );
        assert_eq!(
            redis.internal_connection_string("redis-cache", None, Some("default"), Some("secret")),
            "redis://default:secret@redis-cache:6379",
            "redis internal connection strings should omit the database path"
        );
    }

    #[test]
    fn managed_database_new_generates_engine_specific_defaults() {
        let postgres = ManagedDatabase::new(
            "my-app".to_string(),
            "postgres".to_string(),
            Some("project-1".to_string()),
        );
        assert_eq!(
            postgres.status, "pending",
            "new managed databases should start in pending status"
        );
        assert_eq!(
            postgres.project_id.as_deref(),
            Some("project-1"),
            "new managed databases should preserve the supplied project association"
        );
        assert_eq!(
            postgres.db_name.as_deref(),
            Some("tond_my_app"),
            "database names should replace dashes with underscores and be prefixed with tond_"
        );
        assert_eq!(
            postgres.db_user.as_deref(),
            Some("tond_my_app"),
            "database usernames should replace dashes with underscores and be prefixed with tond_"
        );
        assert!(
            postgres
                .db_password
                .as_ref()
                .is_some_and(|password| password.len() == 24),
            "credentialed engines should receive a 24-character generated password"
        );
        assert_eq!(
            postgres.root_password, None,
            "postgres should not allocate a separate root password"
        );

        let mysql = ManagedDatabase::new("mysql-db".to_string(), "mysql".to_string(), None);
        assert!(
            mysql
                .root_password
                .as_ref()
                .is_some_and(|password| password.len() == 24),
            "mysql should allocate a separate 24-character root password"
        );

        let redis = ManagedDatabase::new("cache".to_string(), "redis".to_string(), None);
        assert_eq!(
            redis.db_name, None,
            "redis should not allocate a named database"
        );
        assert!(
            redis.db_user.is_some(),
            "redis should still allocate a username for connection string generation"
        );
        assert!(
            redis.db_password.is_some(),
            "redis should still allocate a password"
        );
        assert_eq!(
            redis.root_password, None,
            "redis should not allocate a separate root password"
        );
    }
}
