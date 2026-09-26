// auth/tokens.rs — scoped API tokens (`tond_pat_…`)
//
// A token is `tond_pat_` + 64 hex characters. Only its SHA-256 is stored;
// the visible prefix (`tond_pat_` + 8 characters) identifies it in lists and
// lets secret scanners match leaked tokens.
//
// Each token carries: a kind (personal, service, agent, ci), scopes, an
// optional resource filter (app and/or project ids), optional allowed IPs /
// CIDRs, an expiry, last use (time and IP) and a revoked flag.
//
// Ceiling: a token can never do more than its creator. The scopes are checked
// against the creator's role when the token is created AND capped by the
// creator's current role on every request (principal.rs), so demoting a user
// immediately narrows their tokens, and disabling a user suspends them. Tokens
// created as root on the host (`tond token create` over the Unix socket) have
// no owner and no ceiling beyond their scopes.
//
// Agent tokens must expire (default 24 h, at most 30 days) and never hold
// admin scopes. Other kinds may be non-expiring; the default is 90 days.
// Token-authenticated callers can't create tokens: a leaked token must not be
// able to mint more.

use std::net::IpAddr;

use chrono::{Duration, SecondsFormat, Utc};
use rand::RngExt;
use sqlx::{AssertSqlSafe, SqlitePool};
use tond_common::{
    AGENT_TOKEN_DEFAULT_HOURS, AGENT_TOKEN_MAX_DAYS, CreateTokenRequest, CreatedTokenResponse,
    Scope, ScopeGroup, ScopeSet, TOKEN_DEFAULT_DAYS, TOKEN_PREFIX, TokenKind, TokenResponse,
    TondError, hash_key, preset_scopes,
};
use uuid::Uuid;

use super::principal::Principal;

/// Longest token name.
const MAX_NAME_CHARS: usize = 64;
/// Most entries in a resource filter or IP allow-list.
const MAX_LIST_ENTRIES: usize = 50;

/// A row of `api_tokens` joined with its owner.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub owner_user_id: Option<String>,
    pub kind: String,
    pub scopes: String,
    pub app_ids: Option<String>,
    pub project_ids: Option<String>,
    pub allowed_ips: Option<String>,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub last_used_ip: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
    /// users.email (NULL for root tokens or a deleted owner)
    pub owner_email: Option<String>,
    /// users.role
    pub owner_role: Option<String>,
    /// users.disabled_at
    pub owner_disabled_at: Option<String>,
}

/// SELECT list for `TokenRow`.
pub const TOKEN_SELECT: &str = "SELECT t.id, t.name, t.prefix, t.owner_user_id, t.kind, t.scopes,
        t.app_ids, t.project_ids, t.allowed_ips, t.expires_at, t.last_used_at, t.last_used_ip,
        t.revoked_at, t.created_at,
        u.email AS owner_email, u.role AS owner_role, u.disabled_at AS owner_disabled_at
     FROM api_tokens t LEFT JOIN users u ON u.id = t.owner_user_id";

impl TokenRow {
    pub fn kind(&self) -> TokenKind {
        TokenKind::from_db(&self.kind)
    }

    /// The scopes stored on the token (unknown names are dropped).
    pub fn stored_scopes(&self) -> ScopeSet {
        json_list(Some(&self.scopes))
            .iter()
            .filter_map(|s| s.parse::<Scope>().ok())
            .collect()
    }

    pub fn app_ids(&self) -> Vec<String> {
        json_list(self.app_ids.as_deref())
    }

    pub fn project_ids(&self) -> Vec<String> {
        json_list(self.project_ids.as_deref())
    }

    pub fn allowed_ips(&self) -> Vec<String> {
        json_list(self.allowed_ips.as_deref())
    }

    pub fn to_response(&self) -> TokenResponse {
        TokenResponse {
            id: self.id.clone(),
            name: self.name.clone(),
            prefix: self.prefix.clone(),
            kind: self.kind(),
            scopes: self.stored_scopes().to_vec(),
            app_ids: self.app_ids(),
            project_ids: self.project_ids(),
            allowed_ips: self.allowed_ips(),
            owner_id: self.owner_user_id.clone(),
            owner_label: match (&self.owner_user_id, &self.owner_email) {
                (None, _) => "root (host)".to_string(),
                (Some(_), Some(email)) => email.clone(),
                (Some(id), None) => id.clone(),
            },
            expires_at: self.expires_at.clone(),
            last_used_at: self.last_used_at.clone(),
            last_used_ip: self.last_used_ip.clone(),
            revoked_at: self.revoked_at.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

/// Parse a JSON array column (NULL or malformed → empty).
pub fn json_list(value: Option<&str>) -> Vec<String> {
    value
        .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

fn to_json_list(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| serde_json::to_string(values).unwrap_or_else(|_| "[]".into()))
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ── Secrets ──

/// Generate a new token secret. Returns (secret, visible prefix, hash).
pub fn generate_secret() -> (String, String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let secret = format!("{TOKEN_PREFIX}{}", hex::encode(bytes));
    let prefix = secret[..TOKEN_PREFIX.len() + 8].to_string();
    let hash = hash_key(&secret);
    (secret, prefix, hash)
}

/// Whether a bearer value is shaped like a Tond token.
pub fn looks_like_token(raw: &str) -> bool {
    raw.strip_prefix(TOKEN_PREFIX)
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

// ── IP allow-lists ──

/// An allowed address or CIDR range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRule {
    network: IpAddr,
    prefix: u8,
}

impl IpRule {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let (addr, prefix) = match value.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (value, None),
        };
        let network: IpAddr = addr
            .parse()
            .map_err(|_| format!("'{value}' is not an IP address or CIDR range"))?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("'{value}' has an invalid prefix length"))?,
            None => max,
        };
        Ok(Self { network, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.network, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// Whether `ip` may use a token with this allow-list (empty = anywhere).
/// A request without a client address (none can be determined) only passes
/// an empty list.
pub fn ip_allowed(rules: &[String], ip: Option<IpAddr>) -> bool {
    if rules.is_empty() {
        return true;
    }
    let Some(ip) = ip else {
        return false;
    };
    rules
        .iter()
        .filter_map(|r| IpRule::parse(r).ok())
        .any(|rule| rule.contains(ip))
}

// ── Creation ──

fn invalid(msg: impl Into<String>) -> TondError {
    TondError::InvalidInput(msg.into())
}

fn scope_denied(msg: String) -> TondError {
    TondError::ScopeDenied {
        code: "SCOPE_DENIED",
        message: msg,
    }
}

/// Resolve app ids or names to ids.
async fn resolve_ids(
    db: &SqlitePool,
    table: &str,
    what: &str,
    values: &[String],
) -> Result<Vec<String>, TondError> {
    if values.len() > MAX_LIST_ENTRIES {
        return Err(invalid(format!(
            "At most {MAX_LIST_ENTRIES} {what}s per token"
        )));
    }
    let mut ids = Vec::new();
    for value in values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
        let sql = format!("SELECT id FROM {table} WHERE id = ? OR name = ? LIMIT 1");
        let id: Option<String> = sqlx::query_scalar(AssertSqlSafe(sql))
            .bind(value)
            .bind(value)
            .fetch_optional(db)
            .await?;
        let id = id.ok_or_else(|| invalid(format!("No {what} named '{value}'")))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Resolve the expiry for a token of `kind` from the requested lifetime.
pub fn resolve_expiry(
    kind: TokenKind,
    expires_in_hours: Option<i64>,
    now: chrono::DateTime<Utc>,
) -> Result<Option<String>, TondError> {
    let hours = match (kind, expires_in_hours) {
        (TokenKind::Agent, None) => AGENT_TOKEN_DEFAULT_HOURS,
        (TokenKind::Agent, Some(h)) if h <= 0 => {
            return Err(invalid(
                "Agent tokens must expire. Choose a lifetime of up to 30 days.",
            ));
        }
        (TokenKind::Agent, Some(h)) if h > AGENT_TOKEN_MAX_DAYS * 24 => {
            return Err(invalid(format!(
                "Agent tokens can live at most {AGENT_TOKEN_MAX_DAYS} days."
            )));
        }
        (_, None) => TOKEN_DEFAULT_DAYS * 24,
        (_, Some(0)) => return Ok(None),
        (_, Some(h)) if h < 0 => return Err(invalid("The lifetime can't be negative.")),
        (_, Some(h)) if h > 24 * 365 * 5 => {
            return Err(invalid(
                "Choose a lifetime of at most 5 years, or no expiry.",
            ));
        }
        (_, Some(h)) => h,
    };
    Ok(Some(
        (now + Duration::hours(hours)).to_rfc3339_opts(SecondsFormat::Secs, true),
    ))
}

/// Create a token on behalf of `creator`. Returns the token and its secret.
pub async fn create_token(
    db: &SqlitePool,
    creator: &Principal,
    req: &CreateTokenRequest,
) -> Result<CreatedTokenResponse, TondError> {
    if creator.is_token() {
        return Err(scope_denied(
            "API tokens can't create other tokens. Sign in to the dashboard or use `tond token create` on the server."
                .to_string(),
        ));
    }

    let name = req.name.trim();
    if name.is_empty() {
        return Err(invalid(
            "Give the token a name, e.g. the machine or workflow that uses it.",
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid(format!(
            "Token names can be at most {MAX_NAME_CHARS} characters."
        )));
    }
    let kind = req.kind.unwrap_or(TokenKind::Personal);

    let mut scopes = ScopeSet::parse_names(&req.scopes).map_err(invalid)?;
    if let Some(preset) = req.preset.as_deref().filter(|p| !p.is_empty()) {
        let preset_set = preset_scopes(preset).ok_or_else(|| {
            invalid(format!(
                "Unknown preset '{preset}' (expected readonly, deployer or agent-operator)"
            ))
        })?;
        for s in preset_set.to_vec() {
            scopes.insert(s);
        }
    }
    if scopes.is_empty() {
        return Err(invalid("Choose at least one scope or a preset."));
    }
    if kind == TokenKind::Agent {
        let admin: Vec<String> = scopes
            .to_vec()
            .into_iter()
            .filter(|s| s.group() == ScopeGroup::Admin)
            .map(|s| s.to_string())
            .collect();
        if !admin.is_empty() {
            return Err(invalid(format!(
                "Agent tokens can't hold admin scopes ({}).",
                admin.join(", ")
            )));
        }
    }
    let beyond = scopes.difference(creator.scopes());
    if !beyond.is_empty() {
        let names: Vec<String> = beyond.to_vec().iter().map(|s| s.to_string()).collect();
        return Err(scope_denied(format!(
            "A token can't do more than you can. Your access doesn't include: {}.",
            names.join(", ")
        )));
    }

    let app_ids = resolve_ids(db, "apps", "app", &req.app_ids).await?;
    let project_ids = resolve_ids(db, "projects", "project", &req.project_ids).await?;
    // A resource-limited creator can only mint tokens within their own filter.
    if let Some(filter) = creator.filter()
        && (app_ids.is_empty() && project_ids.is_empty()
            || !app_ids.iter().all(|a| filter.app_ids.contains(a))
            || !project_ids.iter().all(|p| filter.project_ids.contains(p)))
    {
        return Err(scope_denied(
            "A token can't reach apps you can't reach.".to_string(),
        ));
    }

    if req.allowed_ips.len() > MAX_LIST_ENTRIES {
        return Err(invalid(format!(
            "At most {MAX_LIST_ENTRIES} allowed IPs per token"
        )));
    }
    let mut allowed_ips = Vec::new();
    for ip in req
        .allowed_ips
        .iter()
        .map(|i| i.trim())
        .filter(|i| !i.is_empty())
    {
        IpRule::parse(ip).map_err(invalid)?;
        allowed_ips.push(ip.to_string());
    }

    let now = Utc::now();
    let expires_at = resolve_expiry(kind, req.expires_in_hours, now)?;
    let (secret, prefix, hash) = generate_secret();
    let id = Uuid::new_v4().to_string();
    let scope_names: Vec<String> = scopes.to_vec().iter().map(|s| s.to_string()).collect();

    sqlx::query(
        "INSERT INTO api_tokens (id, name, prefix, token_hash, owner_user_id, kind, scopes,
             app_ids, project_ids, allowed_ips, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(&prefix)
    .bind(&hash)
    .bind(creator.user_id())
    .bind(kind.as_str())
    .bind(serde_json::to_string(&scope_names).unwrap_or_else(|_| "[]".into()))
    .bind(to_json_list(&app_ids))
    .bind(to_json_list(&project_ids))
    .bind(to_json_list(&allowed_ips))
    .bind(&expires_at)
    .bind(now.to_rfc3339_opts(SecondsFormat::Secs, true))
    .execute(db)
    .await?;

    let row = find_token(db, &id)
        .await?
        .ok_or_else(|| TondError::Internal("token vanished after insert".into()))?;
    Ok(CreatedTokenResponse {
        token: row.to_response(),
        secret,
    })
}

/// Load a token by id.
pub async fn find_token(db: &SqlitePool, id: &str) -> Result<Option<TokenRow>, sqlx::Error> {
    sqlx::query_as::<_, TokenRow>(AssertSqlSafe(format!("{TOKEN_SELECT} WHERE t.id = ?")))
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Load a token by the hash of its secret.
pub async fn find_by_secret(
    db: &SqlitePool,
    secret: &str,
) -> Result<Option<TokenRow>, sqlx::Error> {
    sqlx::query_as::<_, TokenRow>(AssertSqlSafe(format!(
        "{TOKEN_SELECT} WHERE t.token_hash = ?"
    )))
    .bind(hash_key(secret))
    .fetch_optional(db)
    .await
}

/// Tokens, newest first; only `owner`'s when given.
pub async fn list_tokens(
    db: &SqlitePool,
    owner: Option<&str>,
) -> Result<Vec<TokenRow>, sqlx::Error> {
    match owner {
        Some(owner) => {
            sqlx::query_as::<_, TokenRow>(AssertSqlSafe(format!(
                "{TOKEN_SELECT} WHERE t.owner_user_id = ? ORDER BY t.created_at DESC"
            )))
            .bind(owner)
            .fetch_all(db)
            .await
        }
        None => {
            sqlx::query_as::<_, TokenRow>(AssertSqlSafe(format!(
                "{TOKEN_SELECT} ORDER BY t.created_at DESC"
            )))
            .fetch_all(db)
            .await
        }
    }
}

/// Mark a token revoked. Returns false when it was already revoked.
pub async fn revoke_token(db: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(now_rfc3339())
            .bind(id)
            .execute(db)
            .await?;
    Ok(result.rows_affected() == 1)
}

/// Record a token's use. Throttled to one write a minute per token (or when
/// the address changes) so busy automation doesn't write on every request.
pub async fn touch(db: &SqlitePool, row: &TokenRow, ip: Option<IpAddr>) {
    let ip = ip.map(|i| i.to_string());
    let now = Utc::now();
    let recent = row
        .last_used_at
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| now.signed_duration_since(t) < Duration::seconds(60));
    if recent && row.last_used_ip == ip {
        return;
    }
    let _ = sqlx::query("UPDATE api_tokens SET last_used_at = ?, last_used_ip = ? WHERE id = ?")
        .bind(now.to_rfc3339_opts(SecondsFormat::Secs, true))
        .bind(&ip)
        .bind(&row.id)
        .execute(db)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_have_the_scannable_prefix_and_hash() {
        let (secret, prefix, hash) = generate_secret();
        assert!(secret.starts_with("tond_pat_"));
        assert_eq!(secret.len(), "tond_pat_".len() + 64);
        assert!(looks_like_token(&secret));
        assert_eq!(prefix.len(), "tond_pat_".len() + 8);
        assert!(secret.starts_with(&prefix));
        assert_eq!(hash, hash_key(&secret));
        assert!(!looks_like_token("tond_pat_short"));
        assert!(!looks_like_token(&"0".repeat(64)));
    }

    #[test]
    fn ip_rules_match_addresses_and_ranges() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap_or_else(|e| panic!("{s}: {e}"));
        let single = IpRule::parse("127.0.0.1").unwrap_or_else(|e| panic!("{e}"));
        assert!(single.contains(ip("127.0.0.1")));
        assert!(!single.contains(ip("127.0.0.2")));
        let range = IpRule::parse("10.1.0.0/16").unwrap_or_else(|e| panic!("{e}"));
        assert!(range.contains(ip("10.1.200.3")));
        assert!(!range.contains(ip("10.2.0.1")));
        assert!(
            range.contains(ip("::ffff:10.1.2.3")),
            "v4-mapped v6 matches v4 rules"
        );
        let v6 = IpRule::parse("2001:db8::/32").unwrap_or_else(|e| panic!("{e}"));
        assert!(v6.contains(ip("2001:db8:1::5")));
        assert!(!v6.contains(ip("10.1.2.3")));
        assert!(IpRule::parse("0.0.0.0/0").is_ok_and(|r| r.contains(ip("8.8.8.8"))));
        assert!(IpRule::parse("10.0.0.0/33").is_err());
        assert!(IpRule::parse("not-an-ip").is_err());
    }

    #[test]
    fn empty_allow_lists_allow_everyone_but_lists_need_an_address() {
        let ip = "203.0.113.4".parse().ok();
        assert!(ip_allowed(&[], ip));
        assert!(ip_allowed(&[], None));
        assert!(ip_allowed(&["203.0.113.0/24".into()], ip));
        assert!(!ip_allowed(&["198.51.100.1".into()], ip));
        assert!(!ip_allowed(&["203.0.113.0/24".into()], None));
    }

    #[test]
    fn agent_tokens_must_expire_within_thirty_days() {
        let now = Utc::now();
        let default = resolve_expiry(TokenKind::Agent, None, now)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_default();
        assert_eq!(
            default,
            (now + Duration::hours(24)).to_rfc3339_opts(SecondsFormat::Secs, true)
        );
        assert!(resolve_expiry(TokenKind::Agent, Some(0), now).is_err());
        assert!(resolve_expiry(TokenKind::Agent, Some(30 * 24), now).is_ok());
        assert!(resolve_expiry(TokenKind::Agent, Some(30 * 24 + 1), now).is_err());
    }

    #[test]
    fn other_tokens_default_to_ninety_days_and_may_never_expire() {
        let now = Utc::now();
        assert_eq!(
            resolve_expiry(TokenKind::Ci, None, now).unwrap_or_else(|e| panic!("{e}")),
            Some((now + Duration::days(90)).to_rfc3339_opts(SecondsFormat::Secs, true))
        );
        assert_eq!(
            resolve_expiry(TokenKind::Service, Some(0), now).unwrap_or_else(|e| panic!("{e}")),
            None
        );
        assert!(resolve_expiry(TokenKind::Personal, Some(-1), now).is_err());
    }
}
