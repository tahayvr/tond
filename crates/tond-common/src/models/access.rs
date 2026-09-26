// access.rs — roles, permission scopes, token presets and API token DTOs
//
// This is the ONE place that decides what each role may do. The server checks
// every request against a set of `Scope`s; a user's scopes come from their
// `Role`, a token's scopes are what it was created with, capped by its
// creator's role at every request. Keeping the role → scope mapping here makes
// it easy to gate roles later (roles beyond admin are planned as a paid option).
//
// The dashboard renders the scope picker from GET /api/v1/tokens/catalog, which
// serialises `scope_groups()` and `presets()` below, so descriptions live here
// too.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ── Scopes ──

macro_rules! scopes {
    ($( $variant:ident = $name:literal, $group:ident, $desc:literal; )+) => {
        /// A single permission. Serialised as its `name` (e.g. `apps:deploy`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Scope { $( $variant ),+ }

        impl Scope {
            /// Every scope, in display order.
            pub const ALL: &'static [Scope] = &[ $( Scope::$variant ),+ ];

            /// Stable wire name, e.g. `apps:deploy`.
            pub fn as_str(self) -> &'static str {
                match self { $( Scope::$variant => $name ),+ }
            }

            /// The group this scope is listed under.
            pub fn group(self) -> ScopeGroup {
                match self { $( Scope::$variant => ScopeGroup::$group ),+ }
            }

            /// Plain-language description for the token form.
            pub fn description(self) -> &'static str {
                match self { $( Scope::$variant => $desc ),+ }
            }
        }

        impl FromStr for Scope {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $name => Ok(Scope::$variant), )+
                    other => Err(format!("unknown scope '{other}'")),
                }
            }
        }
    };
}

scopes! {
    AppsRead = "apps:read", Apps, "See apps, their settings, domains and deployment history";
    AppsDeploy = "apps:deploy", Apps, "Deploy apps (build and release a new version)";
    AppsWrite = "apps:write", Apps, "Create apps and change their configuration, domains and volumes";
    AppsControl = "apps:control", Apps, "Start, stop and restart apps";
    AppsRollback = "apps:rollback", Apps, "Roll an app back to an earlier deployment";
    AppsDelete = "apps:delete", Apps, "Delete apps and projects";
    EnvReadNames = "env:read-names", Secrets, "See the names of environment variables (not their values)";
    EnvReadValues = "env:read-values", Secrets, "Reveal environment variable values";
    EnvWrite = "env:write", Secrets, "Add, change and remove environment variables";
    DbCredentials = "db:credentials", Secrets, "Reveal and change database passwords and connection strings";
    DbRead = "db:read", Data, "See databases and their status";
    DbWrite = "db:write", Data, "Create, start, stop and reconfigure databases";
    DbDelete = "db:delete", Data, "Delete databases";
    SnapshotsRead = "snapshots:read", Data, "See snapshots and backup status";
    SnapshotsCreate = "snapshots:create", Data, "Take snapshots";
    SnapshotsRestore = "snapshots:restore", Data, "Restore and delete snapshots (overwrites live data)";
    LogsRead = "logs:read", Ops, "Read build, app, database and cron logs";
    MetricsRead = "metrics:read", Ops, "See CPU, memory and network metrics and alert rules";
    EventsRead = "events:read", Ops, "See the activity (audit) log";
    CronRead = "cron:read", Ops, "See cron jobs and their runs";
    CronWrite = "cron:write", Ops, "Create, change and delete cron jobs";
    CronTrigger = "cron:trigger", Ops, "Run a cron job now";
    PreviewsWrite = "previews:write", Ops, "Create and delete preview deployments";
    SettingsWrite = "settings:write", Admin, "Change server settings, integrations, alerts and backups";
    UsersAdmin = "users:admin", Admin, "Invite, change and remove users and their sessions";
    TokensAdmin = "tokens:admin", Admin, "See and revoke every user's API tokens";
    TerminalHost = "terminal:host", Admin, "Open terminal sessions (host shell and containers)";
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Scope {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Scope".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let names: Vec<&str> = Scope::ALL.iter().map(|s| s.as_str()).collect();
        schemars::json_schema!({
            "type": "string",
            "enum": names,
            "description": "A permission scope, e.g. `apps:deploy` (GET /api/v1/tokens/catalog describes each)."
        })
    }
}

/// How scopes are grouped in the token form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScopeGroup {
    Apps,
    Secrets,
    Data,
    Ops,
    /// Never granted to agent tokens.
    Admin,
}

impl ScopeGroup {
    pub const ALL: &'static [ScopeGroup] = &[
        ScopeGroup::Apps,
        ScopeGroup::Secrets,
        ScopeGroup::Data,
        ScopeGroup::Ops,
        ScopeGroup::Admin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScopeGroup::Apps => "Apps",
            ScopeGroup::Secrets => "Secrets",
            ScopeGroup::Data => "Data",
            ScopeGroup::Ops => "Ops",
            ScopeGroup::Admin => "Admin",
        }
    }
}

/// A set of scopes (a bitmask: there are fewer than 64 scopes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ScopeSet(u64);

impl ScopeSet {
    pub const fn empty() -> Self {
        ScopeSet(0)
    }

    pub fn all() -> Self {
        Scope::ALL.iter().copied().collect()
    }

    fn bit(scope: Scope) -> u64 {
        1u64 << (scope as u32)
    }

    pub fn contains(&self, scope: Scope) -> bool {
        self.0 & Self::bit(scope) != 0
    }

    pub fn insert(&mut self, scope: Scope) {
        self.0 |= Self::bit(scope);
    }

    pub fn intersection(&self, other: ScopeSet) -> ScopeSet {
        ScopeSet(self.0 & other.0)
    }

    /// Whether every scope in `self` is also in `other`.
    pub fn is_subset(&self, other: ScopeSet) -> bool {
        self.0 & !other.0 == 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Scopes in display order.
    pub fn to_vec(&self) -> Vec<Scope> {
        Scope::ALL
            .iter()
            .copied()
            .filter(|s| self.contains(*s))
            .collect()
    }

    /// Scopes in `self` that are not in `other`.
    pub fn difference(&self, other: ScopeSet) -> ScopeSet {
        ScopeSet(self.0 & !other.0)
    }

    /// Parse a list of scope names, rejecting unknown ones.
    pub fn parse_names<I, S>(names: I) -> Result<ScopeSet, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = ScopeSet::empty();
        for name in names {
            set.insert(name.as_ref().trim().parse()?);
        }
        Ok(set)
    }
}

impl FromIterator<Scope> for ScopeSet {
    fn from_iter<T: IntoIterator<Item = Scope>>(iter: T) -> Self {
        let mut set = ScopeSet::empty();
        for s in iter {
            set.insert(s);
        }
        set
    }
}

impl Serialize for ScopeSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_vec().serialize(s)
    }
}

impl<'de> Deserialize<'de> for ScopeSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Vec::<Scope>::deserialize(d)?;
        Ok(v.into_iter().collect())
    }
}

// ── Roles ──

/// A user's role. The first (setup) user is an admin, who is the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Everything, including users, tokens and server settings.
    Admin,
    /// Ships code: deploy, restart, roll back, previews, env vars (write, names only).
    Deployer,
    /// Read-only: every `*:read` scope except secret values.
    Viewer,
}

impl Role {
    pub const ALL: &'static [Role] = &[Role::Admin, Role::Deployer, Role::Viewer];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Deployer => "deployer",
            Role::Viewer => "viewer",
        }
    }

    /// Parse the `users.role` column. Anything unrecognised is a viewer, so a
    /// bad value in the database can never grant more rights.
    pub fn from_db(value: &str) -> Role {
        value.parse().unwrap_or(Role::Viewer)
    }

    /// THE role → scope mapping.
    pub fn scopes(self) -> ScopeSet {
        use Scope::*;
        match self {
            Role::Admin => ScopeSet::all(),
            Role::Deployer => [
                AppsRead,
                AppsDeploy,
                AppsControl,
                AppsRollback,
                EnvReadNames,
                EnvWrite,
                LogsRead,
                MetricsRead,
                EventsRead,
                CronRead,
                CronTrigger,
                PreviewsWrite,
                DbRead,
                SnapshotsRead,
                SnapshotsCreate,
            ]
            .into_iter()
            .collect(),
            Role::Viewer => READ_ONLY.iter().copied().collect(),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Role::Admin => "Full control, including users, API tokens and server settings",
            Role::Deployer => {
                "Deploy, restart and roll back apps, manage previews and env vars (without reading values)"
            }
            Role::Viewer => {
                "Read-only access to apps, logs, metrics and activity; no secret values"
            }
        }
    }
}

impl FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "deployer" => Ok(Role::Deployer),
            "viewer" => Ok(Role::Viewer),
            other => Err(format!(
                "unknown role '{other}' (expected admin, deployer or viewer)"
            )),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every `*:read` scope except secret values (env values, db credentials).
const READ_ONLY: &[Scope] = &[
    Scope::AppsRead,
    Scope::EnvReadNames,
    Scope::DbRead,
    Scope::SnapshotsRead,
    Scope::LogsRead,
    Scope::MetricsRead,
    Scope::EventsRead,
    Scope::CronRead,
];

// ── Token kinds and presets ──

/// What a token is for. Agent tokens (AI agents, bots) must expire and can
/// never hold admin scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Personal,
    Service,
    Agent,
    Ci,
}

/// Default lifetime of an agent token.
pub const AGENT_TOKEN_DEFAULT_HOURS: i64 = 24;
/// Longest lifetime of an agent token.
pub const AGENT_TOKEN_MAX_DAYS: i64 = 30;
/// Default lifetime offered for other kinds (they may also never expire).
pub const TOKEN_DEFAULT_DAYS: i64 = 90;
/// Visible prefix of every token, so secret scanners can match it.
pub const TOKEN_PREFIX: &str = "tond_pat_";

impl TokenKind {
    pub const ALL: &'static [TokenKind] = &[
        TokenKind::Personal,
        TokenKind::Service,
        TokenKind::Agent,
        TokenKind::Ci,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Personal => "personal",
            TokenKind::Service => "service",
            TokenKind::Agent => "agent",
            TokenKind::Ci => "ci",
        }
    }

    pub fn from_db(value: &str) -> TokenKind {
        value.parse().unwrap_or(TokenKind::Personal)
    }
}

impl FromStr for TokenKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "personal" => Ok(TokenKind::Personal),
            "service" => Ok(TokenKind::Service),
            "agent" => Ok(TokenKind::Agent),
            "ci" => Ok(TokenKind::Ci),
            other => Err(format!(
                "unknown token kind '{other}' (expected personal, service, agent or ci)"
            )),
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A named starting point for a token's scopes.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TokenPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub scopes: Vec<Scope>,
}

/// The token presets, in display order.
pub fn presets() -> Vec<TokenPreset> {
    use Scope::*;
    vec![
        TokenPreset {
            id: "readonly",
            label: "Read-only",
            description: "Look but don't touch: apps, logs, metrics, activity. No secret values.",
            scopes: READ_ONLY.to_vec(),
        },
        TokenPreset {
            id: "deployer",
            label: "Deployer (CI)",
            description: "Deploy and read one app from CI. Pair it with an app filter.",
            scopes: vec![AppsRead, AppsDeploy, LogsRead],
        },
        TokenPreset {
            id: "agent-operator",
            label: "Agent operator",
            description: "For AI agents: read, deploy, restart, roll back, previews and env var names.",
            scopes: vec![
                AppsRead,
                AppsDeploy,
                AppsControl,
                AppsRollback,
                PreviewsWrite,
                EnvReadNames,
                LogsRead,
                MetricsRead,
                EventsRead,
            ],
        },
    ]
}

/// Look up a preset's scopes by id.
pub fn preset_scopes(id: &str) -> Option<ScopeSet> {
    presets()
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.scopes.into_iter().collect())
}

/// Scope catalog served to the dashboard (GET /api/v1/tokens/catalog).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScopeCatalog {
    pub groups: Vec<ScopeGroupInfo>,
    pub presets: Vec<TokenPreset>,
    pub roles: Vec<RoleInfo>,
    /// The scopes the caller may grant (their own ceiling).
    pub grantable: Vec<Scope>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScopeGroupInfo {
    pub id: ScopeGroup,
    pub label: &'static str,
    pub scopes: Vec<ScopeInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScopeInfo {
    pub id: Scope,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RoleInfo {
    pub id: Role,
    pub description: &'static str,
    pub scopes: Vec<Scope>,
}

impl ScopeCatalog {
    pub fn new(grantable: ScopeSet) -> Self {
        let groups = ScopeGroup::ALL
            .iter()
            .map(|g| ScopeGroupInfo {
                id: *g,
                label: g.label(),
                scopes: Scope::ALL
                    .iter()
                    .filter(|s| s.group() == *g)
                    .map(|s| ScopeInfo {
                        id: *s,
                        description: s.description(),
                    })
                    .collect(),
            })
            .collect();
        let roles = Role::ALL
            .iter()
            .map(|r| RoleInfo {
                id: *r,
                description: r.description(),
                scopes: r.scopes().to_vec(),
            })
            .collect();
        Self {
            groups,
            presets: presets(),
            roles,
            grantable: grantable.to_vec(),
        }
    }
}

// ── API token DTOs ──

/// Body of POST /api/v1/tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateTokenRequest {
    pub name: String,
    pub kind: Option<TokenKind>,
    /// A preset id (`readonly`, `deployer`, `agent-operator`). Combined with `scopes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Limit the token to these apps (ids or names).
    #[serde(default)]
    pub app_ids: Vec<String>,
    /// Limit the token to the apps in these projects (ids or names).
    #[serde(default)]
    pub project_ids: Vec<String>,
    /// IP addresses or CIDR ranges the token may be used from.
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    /// Lifetime in hours. `None` means the default for the kind; 0 means
    /// never expires (not allowed for agent tokens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_hours: Option<i64>,
}

/// An API token as listed (never includes the secret).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TokenResponse {
    pub id: String,
    pub name: String,
    /// Visible start of the token, e.g. `tond_pat_1a2b3c4d`.
    pub prefix: String,
    pub kind: TokenKind,
    pub scopes: Vec<Scope>,
    pub app_ids: Vec<String>,
    pub project_ids: Vec<String>,
    pub allowed_ips: Vec<String>,
    pub owner_id: Option<String>,
    /// Owner email, or "root (host)" for tokens created on the server.
    pub owner_label: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub last_used_ip: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
}

/// Response of POST /api/v1/tokens: the token plus its secret, shown once.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreatedTokenResponse {
    pub token: TokenResponse,
    pub secret: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scope_round_trips_through_its_name() {
        for scope in Scope::ALL {
            assert_eq!(scope.as_str().parse::<Scope>(), Ok(*scope));
        }
        assert!(Scope::ALL.len() < 64, "ScopeSet is a u64 bitmask");
        assert!("apps:everything".parse::<Scope>().is_err());
    }

    #[test]
    fn admin_has_everything_and_other_roles_are_strict_subsets() {
        assert_eq!(Role::Admin.scopes(), ScopeSet::all());
        assert!(Role::Deployer.scopes().is_subset(Role::Admin.scopes()));
        assert!(Role::Viewer.scopes().is_subset(Role::Deployer.scopes()));
        assert_ne!(Role::Viewer.scopes(), Role::Deployer.scopes());
    }

    #[test]
    fn deployer_matches_the_documented_mapping() {
        let d = Role::Deployer.scopes();
        for s in [
            Scope::AppsRead,
            Scope::AppsDeploy,
            Scope::AppsControl,
            Scope::AppsRollback,
            Scope::EnvReadNames,
            Scope::EnvWrite,
            Scope::LogsRead,
            Scope::MetricsRead,
            Scope::EventsRead,
            Scope::CronRead,
            Scope::CronTrigger,
            Scope::PreviewsWrite,
            Scope::DbRead,
            Scope::SnapshotsRead,
            Scope::SnapshotsCreate,
        ] {
            assert!(d.contains(s), "deployer should have {s}");
        }
        for s in [
            Scope::AppsWrite,
            Scope::AppsDelete,
            Scope::EnvReadValues,
            Scope::DbCredentials,
            Scope::SettingsWrite,
            Scope::UsersAdmin,
            Scope::TokensAdmin,
            Scope::TerminalHost,
        ] {
            assert!(!d.contains(s), "deployer must not have {s}");
        }
    }

    #[test]
    fn viewer_has_reads_but_no_secret_values() {
        let v = Role::Viewer.scopes();
        for s in Scope::ALL {
            let is_read = s.as_str().ends_with(":read") || *s == Scope::EnvReadNames;
            assert_eq!(v.contains(*s), is_read, "viewer and {s}");
        }
        assert!(!v.contains(Scope::EnvReadValues));
        assert!(!v.contains(Scope::DbCredentials));
    }

    #[test]
    fn unknown_roles_are_viewers() {
        assert_eq!(Role::from_db("admin"), Role::Admin);
        assert_eq!(Role::from_db("member"), Role::Viewer);
        assert_eq!(Role::from_db("Admin"), Role::Viewer);
        assert_eq!(Role::from_db(""), Role::Viewer);
    }

    #[test]
    fn presets_never_include_admin_scopes() {
        for preset in presets() {
            assert!(
                preset.scopes.iter().all(|s| s.group() != ScopeGroup::Admin),
                "preset {} must not include admin scopes",
                preset.id
            );
        }
        assert_eq!(
            preset_scopes("deployer").map(|s| s.to_vec()),
            Some(vec![Scope::AppsRead, Scope::AppsDeploy, Scope::LogsRead])
        );
        assert!(preset_scopes("nope").is_none());
    }

    #[test]
    fn scope_set_operations() {
        let a: ScopeSet = [Scope::AppsRead, Scope::AppsDeploy].into_iter().collect();
        let b: ScopeSet = [Scope::AppsRead].into_iter().collect();
        assert!(b.is_subset(a));
        assert!(!a.is_subset(b));
        assert_eq!(a.intersection(b), b);
        assert_eq!(a.difference(b).to_vec(), vec![Scope::AppsDeploy]);
        assert_eq!(ScopeSet::parse_names(["apps:read", " apps:deploy "]), Ok(a));
        assert!(ScopeSet::parse_names(["apps:read", "bogus"]).is_err());
        let json = serde_json::to_string(&a).unwrap_or_default();
        assert_eq!(json, r#"["apps:read","apps:deploy"]"#);
    }

    #[test]
    fn catalog_groups_every_scope_once() {
        let catalog = ScopeCatalog::new(ScopeSet::all());
        let total: usize = catalog.groups.iter().map(|g| g.scopes.len()).sum();
        assert_eq!(total, Scope::ALL.len());
        assert_eq!(catalog.roles.len(), 3);
    }
}
