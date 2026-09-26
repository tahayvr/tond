// db_catalog.rs — which image runs a managed database
//
// A managed database is an engine (`db_type`), a major version and an
// optional variant (an image with extensions built in). This table is the
// one place that maps the three to a pinned image. Versions are listed
// oldest first; `DbEngineConfig::image` is the default version's image.
//
// Upgrades only ever move to a newer version of the same engine and variant
// (see `upgrade_targets`); downgrades are refused because no engine reads
// data files written by a newer release.

use serde::{Deserialize, Serialize};

/// One runnable image of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineImage {
    pub db_type: &'static str,
    /// "" = the engine's own image
    pub variant: &'static str,
    pub version: &'static str,
    pub image: &'static str,
}

/// An image variant: the engine plus extensions that need their own build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbVariant {
    pub db_type: &'static str,
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Extensions this variant adds (enable them with CREATE EXTENSION)
    pub extensions: &'static [&'static str],
    /// Published for x86_64 only (no ARM64 image upstream)
    pub amd64_only: bool,
}

const fn img(
    db_type: &'static str,
    variant: &'static str,
    version: &'static str,
    image: &'static str,
) -> EngineImage {
    EngineImage {
        db_type,
        variant,
        version,
        image,
    }
}

/// Every image Tond runs for managed databases, oldest version first.
pub const ENGINE_IMAGES: &[EngineImage] = &[
    img("postgres", "", "14", "postgres:14-alpine"),
    img("postgres", "", "15", "postgres:15-alpine"),
    img("postgres", "", "16", "postgres:16-alpine"),
    img("postgres", "", "17", "postgres:17-alpine"),
    img("postgres", "", "18", "postgres:18-alpine"),
    img("postgres", "pgvector", "14", "pgvector/pgvector:0.8.1-pg14"),
    img("postgres", "pgvector", "15", "pgvector/pgvector:0.8.1-pg15"),
    img("postgres", "pgvector", "16", "pgvector/pgvector:0.8.1-pg16"),
    img("postgres", "pgvector", "17", "pgvector/pgvector:0.8.1-pg17"),
    img("postgres", "pgvector", "18", "pgvector/pgvector:0.8.1-pg18"),
    img("postgres", "postgis", "14", "postgis/postgis:14-3.5-alpine"),
    img("postgres", "postgis", "15", "postgis/postgis:15-3.5-alpine"),
    img("postgres", "postgis", "16", "postgis/postgis:16-3.5-alpine"),
    img("postgres", "postgis", "17", "postgis/postgis:17-3.6-alpine"),
    img("postgres", "postgis", "18", "postgis/postgis:18-3.6-alpine"),
    img("mysql", "", "8.0", "mysql:8.0"),
    img("mysql", "", "8.4", "mysql:8.4"),
    img("mariadb", "", "10.11", "mariadb:10.11"),
    img("mariadb", "", "11.4", "mariadb:11.4"),
    img("mariadb", "", "11.8", "mariadb:11.8"),
    img("redis", "", "6", "redis:6-alpine"),
    img("redis", "", "7", "redis:7-alpine"),
    img("redis", "", "8", "redis:8-alpine"),
    img("keydb", "", "latest", "eqalpha/keydb:latest"),
    img("mongodb", "", "6", "mongo:6"),
    img("mongodb", "", "7", "mongo:7"),
    img("mongodb", "", "8", "mongo:8"),
];

/// Image variants (besides the engine's own image).
pub const DB_VARIANTS: &[DbVariant] = &[
    DbVariant {
        db_type: "postgres",
        id: "pgvector",
        label: "PostgreSQL + pgvector",
        description: "Vector similarity search for embeddings (the pgvector image).",
        extensions: &["vector"],
        amd64_only: false,
    },
    DbVariant {
        db_type: "postgres",
        id: "postgis",
        label: "PostgreSQL + PostGIS",
        description: "Geographic objects and spatial queries (the PostGIS image). x86_64 servers only.",
        extensions: &[
            "postgis",
            "postgis_topology",
            "fuzzystrmatch",
            "postgis_tiger_geocoder",
        ],
        amd64_only: true,
    },
];

/// Extensions the `CREATE EXTENSION` helper offers for a postgres image:
/// the contrib ones every image ships, then the variant's own.
pub const POSTGRES_CONTRIB_EXTENSIONS: &[&str] = &[
    "pg_stat_statements",
    "pg_trgm",
    "pgcrypto",
    "uuid-ossp",
    "citext",
    "hstore",
    "btree_gin",
    "btree_gist",
    "unaccent",
    "ltree",
];

/// The default major version of an engine.
pub fn default_version(db_type: &str) -> Option<&'static str> {
    Some(match db_type {
        "postgres" => "16",
        "mysql" => "8.4",
        "mariadb" => "11.8",
        "redis" => "7",
        "keydb" => "latest",
        "mongodb" => "7",
        _ => return None,
    })
}

/// Look up a variant ("" is the engine's own image and has no entry).
pub fn variant_for(db_type: &str, variant: &str) -> Option<&'static DbVariant> {
    DB_VARIANTS
        .iter()
        .find(|v| v.db_type == db_type && v.id == variant)
}

/// The versions available for an engine and variant, oldest first.
pub fn versions_for(db_type: &str, variant: &str) -> Vec<&'static str> {
    ENGINE_IMAGES
        .iter()
        .filter(|i| i.db_type == db_type && i.variant == variant)
        .map(|i| i.version)
        .collect()
}

/// The image for an engine, variant and version (`None` = default version).
pub fn image_for(db_type: &str, variant: &str, version: Option<&str>) -> Option<&'static str> {
    let version = match version {
        Some(v) => v,
        None => default_version(db_type)?,
    };
    ENGINE_IMAGES
        .iter()
        .find(|i| i.db_type == db_type && i.variant == variant && i.version == version)
        .map(|i| i.image)
}

/// Numeric parts of a version ("11.4" → [11, 4]); "latest" sorts last.
fn version_key(version: &str) -> Vec<u64> {
    if version == "latest" {
        return vec![u64::MAX];
    }
    version
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Compare two versions of the same engine.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    version_key(a).cmp(&version_key(b))
}

/// Why an upgrade from `from` to `to` isn't possible, if it isn't.
pub fn check_upgrade(db_type: &str, variant: &str, from: &str, to: &str) -> Result<(), String> {
    if !supports_upgrade(db_type) {
        return Err(format!(
            "Tond can't upgrade {db_type} databases between versions."
        ));
    }
    if image_for(db_type, variant, Some(to)).is_none() {
        let list = versions_for(db_type, variant).join(", ");
        return Err(format!(
            "Version {to} isn't available for this database. Choose one of: {list}."
        ));
    }
    match compare_versions(to, from) {
        std::cmp::Ordering::Greater => Ok(()),
        std::cmp::Ordering::Equal => Err(format!("The database already runs version {from}.")),
        std::cmp::Ordering::Less => Err(format!(
            "Downgrades aren't possible: version {to} can't read data written by {from}. \
             Restore a backup taken before the upgrade into a new database instead."
        )),
    }
}

/// Newer versions an existing database can move to.
pub fn upgrade_targets(db_type: &str, variant: &str, from: &str) -> Vec<&'static str> {
    if !supports_upgrade(db_type) {
        return Vec::new();
    }
    versions_for(db_type, variant)
        .into_iter()
        .filter(|v| compare_versions(v, from) == std::cmp::Ordering::Greater)
        .collect()
}

/// Engines with a guarded major-version upgrade (db_upgrade.rs).
pub fn supports_upgrade(db_type: &str) -> bool {
    matches!(
        db_type,
        "postgres" | "mysql" | "mariadb" | "redis" | "mongodb"
    )
}

/// Engines that can serve TLS with a mounted certificate.
pub fn supports_tls(db_type: &str) -> bool {
    matches!(db_type, "postgres" | "mysql" | "mariadb")
}

/// Whether `name` is a plain extension name (safe to quote in SQL).
pub fn valid_extension_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Extensions offered for a postgres database of `variant`.
pub fn offered_extensions(variant: &str) -> Vec<&'static str> {
    let mut list: Vec<&'static str> = variant_for("postgres", variant)
        .map(|v| v.extensions.to_vec())
        .unwrap_or_default();
    list.extend_from_slice(POSTGRES_CONTRIB_EXTENSIONS);
    list
}

// ── API shapes ──────────────────────────────────────────────────────────────

/// GET /api/v1/databases/catalog — what can be created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DbCatalogEntry {
    pub db_type: String,
    /// "" = the engine's own image
    pub variant: String,
    pub label: String,
    pub description: String,
    pub versions: Vec<String>,
    pub default_version: String,
    pub extensions: Vec<String>,
    /// Available on this server (false for x86_64-only images on ARM64)
    pub available: bool,
    pub supports_tls: bool,
    pub supports_upgrade: bool,
}

/// The whole catalog, for a server of CPU architecture `arch`
/// (`std::env::consts::ARCH`).
pub fn catalog(arch: &str) -> Vec<DbCatalogEntry> {
    let mut out = Vec::new();
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for image in ENGINE_IMAGES {
        let key = (image.db_type, image.variant);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let variant = variant_for(image.db_type, image.variant);
        let versions: Vec<String> = versions_for(image.db_type, image.variant)
            .into_iter()
            .map(str::to_string)
            .collect();
        let default = default_version(image.db_type)
            .filter(|d| versions.iter().any(|v| v == d))
            .map(str::to_string)
            .or_else(|| versions.last().cloned())
            .unwrap_or_default();
        out.push(DbCatalogEntry {
            db_type: image.db_type.to_string(),
            variant: image.variant.to_string(),
            label: variant.map(|v| v.label.to_string()).unwrap_or_default(),
            description: variant
                .map(|v| v.description.to_string())
                .unwrap_or_default(),
            versions,
            default_version: default,
            extensions: if image.db_type == "postgres" {
                offered_extensions(image.variant)
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            } else {
                Vec::new()
            },
            available: !variant.is_some_and(|v| v.amd64_only) || arch == "x86_64",
            supports_tls: supports_tls(image.db_type),
            supports_upgrade: supports_upgrade(image.db_type),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_its_default_version_in_the_catalog() {
        for engine in super::super::databases::ALL_ENGINES {
            let default = default_version(engine.db_type)
                .unwrap_or_else(|| panic!("{} has no default version", engine.db_type));
            assert_eq!(
                image_for(engine.db_type, "", Some(default)),
                Some(engine.image),
                "the engine's default image must be its default version's image"
            );
            assert_eq!(image_for(engine.db_type, "", None), Some(engine.image));
        }
    }

    #[test]
    fn images_are_unique_and_pinned_to_a_major_version() {
        let mut seen = std::collections::HashSet::new();
        for i in ENGINE_IMAGES {
            assert!(seen.insert((i.db_type, i.variant, i.version)), "{i:?}");
            if i.db_type != "keydb" {
                assert!(
                    i.image.contains(i.version) && !i.image.ends_with(":latest"),
                    "{} must pin its version",
                    i.image
                );
            }
        }
    }

    #[test]
    fn variants_resolve_to_their_own_images() {
        assert_eq!(
            image_for("postgres", "pgvector", Some("17")),
            Some("pgvector/pgvector:0.8.1-pg17")
        );
        assert_eq!(
            image_for("postgres", "postgis", Some("16")),
            Some("postgis/postgis:16-3.5-alpine")
        );
        assert_eq!(image_for("postgres", "postgis", Some("13")), None);
        assert_eq!(image_for("mysql", "pgvector", None), None);
    }

    #[test]
    fn versions_compare_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("10.11", "11.4"), Less);
        assert_eq!(compare_versions("8.4", "8.0"), Greater);
        assert_eq!(compare_versions("9", "10"), Less);
        assert_eq!(compare_versions("16", "16"), Equal);
    }

    #[test]
    fn upgrades_only_go_forward_within_the_catalog() {
        assert!(check_upgrade("postgres", "", "16", "17").is_ok());
        assert!(check_upgrade("postgres", "pgvector", "14", "18").is_ok());
        let down = check_upgrade("postgres", "", "17", "16").unwrap_err();
        assert!(down.contains("Downgrades"), "{down}");
        assert!(check_upgrade("postgres", "", "16", "16").is_err());
        assert!(check_upgrade("postgres", "", "16", "99").is_err());
        assert!(check_upgrade("keydb", "", "latest", "latest").is_err());
        assert_eq!(
            upgrade_targets("mariadb", "", "10.11"),
            vec!["11.4", "11.8"]
        );
        assert_eq!(upgrade_targets("mysql", "", "8.4"), Vec::<&str>::new());
        assert!(upgrade_targets("keydb", "", "latest").is_empty());
    }

    #[test]
    fn postgis_is_unavailable_on_arm64() {
        let arm = catalog("aarch64");
        let postgis = arm
            .iter()
            .find(|e| e.variant == "postgis")
            .unwrap_or_else(|| panic!("postgis is in the catalog"));
        assert!(!postgis.available);
        assert!(
            arm.iter()
                .find(|e| e.variant == "pgvector")
                .is_some_and(|e| e.available)
        );
        assert!(catalog("x86_64").iter().all(|e| e.available));
        let pg = arm
            .iter()
            .find(|e| e.db_type == "postgres" && e.variant.is_empty())
            .unwrap_or_else(|| panic!("postgres is in the catalog"));
        assert_eq!(pg.default_version, "16");
        assert!(pg.extensions.contains(&"pg_trgm".to_string()));
    }

    #[test]
    fn extension_names_are_validated() {
        assert!(valid_extension_name("uuid-ossp"));
        assert!(valid_extension_name("vector"));
        assert!(!valid_extension_name("x\"; DROP TABLE t; --"));
        assert!(!valid_extension_name(""));
        assert!(!valid_extension_name("Vector"));
    }
}
