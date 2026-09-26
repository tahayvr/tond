// auth/principal.rs — who is calling, and what they may do
//
// Every authenticated request resolves to a `Principal`: the actor (a user, an
// API token or the local socket), the set of scopes it holds, an optional
// resource filter (tokens limited to some apps/projects) and the client IP.
//
// Authentication, tried in order:
//   1. The root-only Unix socket (`LocalSocket` extension, added only by the
//      socket listener in main.rs): break-glass root, every scope, audited as
//      `local-socket`. Loopback TCP is never trusted — tond-proxy forwards
//      internet traffic over it.
//   2. `Authorization: Bearer tond_pat_…`: an API token (auth/tokens.rs).
//      Refused with 401 when unknown (TOKEN_INVALID), revoked
//      (TOKEN_REVOKED), expired (TOKEN_EXPIRED) or when its owner is disabled
//      (ACCOUNT_DISABLED); 403 IP_NOT_ALLOWED outside its allowed IPs. Its
//      scopes are capped by the owner's current role.
//   3. The `__Host-tond_session` cookie: a dashboard session, with the scopes
//      of the user's role (idle and absolute timeouts in session.rs).
//
// Authorization: handlers declare what they need in their signature:
//
//     async fn deploy_app(auth: Authz<scope::AppsDeploy>, …)
//
// `Authz<R>` authenticates, checks that the principal holds every scope in
// `R` (403 SCOPE_DENIED otherwise) and, for resource-limited tokens, that the
// route targets an allowed app or project (403 RESOURCE_DENIED). A limited
// token may only use routes under /api/apps/{id} and /api/projects/{id} for
// its own resources plus the app and project lists (which the handlers
// filter); every other route is refused. `Principal` itself is also an
// extractor, for self-service routes that only need *someone* signed in.
// Checks that depend on the request body use `principal.require(scope)`.

use std::marker::PhantomData;
use std::net::IpAddr;
use std::ops::Deref;

use axum::{
    Json,
    extract::{FromRequestParts, MatchedPath, RawPathParams},
    http::{Method, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use sqlx::SqlitePool;
use tond_common::{ApiResponse, Role, Scope, ScopeGroup, ScopeSet, TokenKind, TondError, User};

use super::LocalSocket;
use super::rate_limit::client_ip;
use super::tokens::{self, TokenRow};
use crate::audit_log::AuditActor;
use crate::error::AppError;
use crate::handlers::auth::{extract_session_token, validate_session_with_id};
use crate::state::AppState;

// ── Principal ──

/// The authenticated identity behind a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    User {
        id: String,
        email: String,
        role: Role,
    },
    Token {
        id: String,
        name: String,
        kind: TokenKind,
        owner_id: Option<String>,
    },
    LocalSocket,
}

/// Apps and projects a token is limited to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceFilter {
    pub app_ids: Vec<String>,
    pub project_ids: Vec<String>,
}

impl ResourceFilter {
    /// Whether an app (with its project) is inside the filter.
    pub fn allows_app(&self, app_id: &str, project_id: Option<&str>) -> bool {
        self.app_ids.iter().any(|a| a == app_id)
            || project_id.is_some_and(|p| self.project_ids.iter().any(|x| x == p))
    }

    pub fn allows_project(&self, project_id: &str) -> bool {
        self.project_ids.iter().any(|p| p == project_id)
    }
}

/// Who is calling and what they may do.
#[derive(Debug, Clone)]
pub struct Principal {
    actor: Actor,
    scopes: ScopeSet,
    filter: Option<ResourceFilter>,
    ip: Option<IpAddr>,
    /// Stored id (hash) of the session, for session-authenticated requests.
    session_id: Option<String>,
}

impl Principal {
    /// The root-only local socket: every scope.
    pub fn local_socket() -> Self {
        Self {
            actor: Actor::LocalSocket,
            scopes: ScopeSet::all(),
            filter: None,
            ip: None,
            session_id: None,
        }
    }

    /// A signed-in user with their role's scopes.
    pub fn for_user(user: &User, ip: Option<IpAddr>, session_id: Option<String>) -> Self {
        let role = user.role();
        Self {
            actor: Actor::User {
                id: user.id.clone(),
                email: user.email.clone(),
                role,
            },
            scopes: role.scopes(),
            filter: None,
            ip,
            session_id,
        }
    }

    /// A user principal for tests (no database row needed).
    #[cfg(test)]
    pub fn test_user(role: Role) -> Self {
        Self {
            actor: Actor::User {
                id: format!("user-{role}"),
                email: format!("{role}@example.com"),
                role,
            },
            scopes: role.scopes(),
            filter: None,
            ip: None,
            session_id: None,
        }
    }

    /// A token principal for tests.
    #[cfg(test)]
    pub fn test_token(scopes: ScopeSet, filter: Option<ResourceFilter>) -> Self {
        Self {
            actor: Actor::Token {
                id: "token-1".to_string(),
                name: "test-token".to_string(),
                kind: TokenKind::Service,
                owner_id: None,
            },
            scopes,
            filter,
            ip: None,
            session_id: None,
        }
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    pub fn scopes(&self) -> ScopeSet {
        self.scopes
    }

    pub fn filter(&self) -> Option<&ResourceFilter> {
        self.filter.as_ref()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn has(&self, scope: Scope) -> bool {
        self.scopes.contains(scope)
    }

    pub fn is_token(&self) -> bool {
        matches!(self.actor, Actor::Token { .. })
    }

    pub fn is_local_socket(&self) -> bool {
        matches!(self.actor, Actor::LocalSocket)
    }

    /// The user behind the request (the token owner for tokens).
    pub fn user_id(&self) -> Option<&str> {
        match &self.actor {
            Actor::User { id, .. } => Some(id),
            Actor::Token { owner_id, .. } => owner_id.as_deref(),
            Actor::LocalSocket => None,
        }
    }

    /// The signed-in user for session-only routes (not tokens, not the socket).
    pub fn session_user_id(&self) -> Option<&str> {
        match &self.actor {
            Actor::User { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The audit-log actor for this request.
    pub fn audit_actor(&self) -> AuditActor {
        let ip = self.ip.map(|i| i.to_string());
        match &self.actor {
            Actor::User { id, email, .. } => AuditActor::user(id, email, ip),
            Actor::Token { id, name, .. } => AuditActor {
                kind: "token",
                id: Some(id.clone()),
                label: name.clone(),
                ip,
            },
            Actor::LocalSocket => AuditActor::local_socket(),
        }
    }

    /// Fail with 403 SCOPE_DENIED unless the principal holds `scope`.
    pub fn require(&self, scope: Scope) -> Result<(), AppError> {
        if self.has(scope) {
            Ok(())
        } else {
            Err(AppError(self.scope_denied(scope)))
        }
    }

    /// Like `require`, naming the action in the message ("… to configure volumes").
    /// Used for inputs that reach the host (volumes, compose files, custom
    /// images, public ports), which additionally need an admin scope.
    pub fn require_to(&self, scope: Scope, action: &str) -> Result<(), AppError> {
        if self.has(scope) {
            return Ok(());
        }
        let who = match &self.actor {
            Actor::User { role, .. } => format!("Your role ({role})"),
            Actor::Token { .. } => "This API token".to_string(),
            Actor::LocalSocket => "This caller".to_string(),
        };
        Err(AppError(TondError::ScopeDenied {
            code: "SCOPE_DENIED",
            message: format!(
                "{who} can't {action}: that needs the {scope} permission, because it reaches the host."
            ),
        }))
    }

    fn scope_denied(&self, scope: Scope) -> TondError {
        let message = match &self.actor {
            Actor::User { role, .. } => format!(
                "Your role ({role}) doesn't allow this: it needs the {scope} permission. Ask an admin of this server."
            ),
            Actor::Token { .. } => format!(
                "This API token doesn't have the {scope} scope. Create a token that includes it."
            ),
            Actor::LocalSocket => format!("Missing the {scope} scope."),
        };
        TondError::ScopeDenied {
            code: "SCOPE_DENIED",
            message,
        }
    }

    /// Whether an app is visible to this principal (for filtering lists).
    pub fn can_see_app(&self, app_id: &str, project_id: Option<&str>) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|f| f.allows_app(app_id, project_id))
    }

    /// Whether a project is visible to this principal (for filtering lists).
    pub fn can_see_project(&self, project_id: &str) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|f| f.allows_project(project_id))
    }

    /// Fail with 403 RESOURCE_DENIED unless the app (id or name) is inside the
    /// token's filter. Unknown apps are refused too, so a limited token can't
    /// probe which apps exist.
    pub async fn ensure_app_access(&self, db: &SqlitePool, key: &str) -> Result<(), AppError> {
        let Some(filter) = &self.filter else {
            return Ok(());
        };
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT id, project_id FROM apps WHERE id = ? OR name = ? LIMIT 1")
                .bind(key)
                .bind(key)
                .fetch_optional(db)
                .await?;
        match row {
            Some((id, project)) if filter.allows_app(&id, project.as_deref()) => Ok(()),
            _ => Err(resource_denied()),
        }
    }

    /// Like `ensure_app_access` for a project (id or name).
    pub async fn ensure_project_access(&self, db: &SqlitePool, key: &str) -> Result<(), AppError> {
        let Some(filter) = &self.filter else {
            return Ok(());
        };
        let id: Option<String> =
            sqlx::query_scalar("SELECT id FROM projects WHERE id = ? OR name = ? LIMIT 1")
                .bind(key)
                .bind(key)
                .fetch_optional(db)
                .await?;
        match id {
            Some(id) if filter.allows_project(&id) => Ok(()),
            _ => Err(resource_denied()),
        }
    }
}

fn resource_denied() -> AppError {
    AppError(TondError::ScopeDenied {
        code: "RESOURCE_DENIED",
        message: "This API token is limited to other apps or projects.".to_string(),
    })
}

// ── Authentication ──

/// A failed authentication: status, stable code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFailure {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl AuthFailure {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL",
            "Authentication check failed",
        )
    }
}

impl IntoResponse for AuthFailure {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiResponse::<()>::err_code(self.code, self.message)),
        )
            .into_response()
    }
}

/// Resolve a presented token secret into a principal.
pub async fn authenticate_token(
    state: &AppState,
    secret: &str,
    ip: Option<IpAddr>,
) -> Result<Principal, AuthFailure> {
    let invalid = || {
        AuthFailure::new(
            StatusCode::UNAUTHORIZED,
            "TOKEN_INVALID",
            "Invalid API token. Check that you copied the whole token (it starts with tond_pat_).",
        )
    };
    if !tokens::looks_like_token(secret) {
        return Err(invalid());
    }
    let row = tokens::find_by_secret(&state.db, secret)
        .await
        .map_err(|_| AuthFailure::internal())?
        .ok_or_else(invalid)?;
    let principal = principal_for_token(&row, ip, Utc::now())?;
    tokens::touch(&state.db, &row, ip).await;
    Ok(principal)
}

/// Check a token row's status and build its principal (pure, for testing).
pub fn principal_for_token(
    row: &TokenRow,
    ip: Option<IpAddr>,
    now: chrono::DateTime<Utc>,
) -> Result<Principal, AuthFailure> {
    if row.revoked_at.is_some() {
        return Err(AuthFailure::new(
            StatusCode::UNAUTHORIZED,
            "TOKEN_REVOKED",
            "This API token was revoked. Create a new one.",
        ));
    }
    if let Some(expires) = row.expires_at.as_deref() {
        let expired = chrono::DateTime::parse_from_rfc3339(expires)
            .map(|t| t <= now)
            .unwrap_or(true);
        if expired {
            return Err(AuthFailure::new(
                StatusCode::UNAUTHORIZED,
                "TOKEN_EXPIRED",
                "This API token has expired. Create a new one.",
            ));
        }
    }
    // Ceiling: the owner's current role (root tokens have no owner).
    let ceiling = match (&row.owner_user_id, &row.owner_role) {
        (None, _) => ScopeSet::all(),
        (Some(_), Some(role)) => Role::from_db(role).scopes(),
        (Some(_), None) => ScopeSet::empty(),
    };
    if row.owner_user_id.is_some() && (row.owner_disabled_at.is_some() || row.owner_role.is_none())
    {
        return Err(AuthFailure::new(
            StatusCode::UNAUTHORIZED,
            "ACCOUNT_DISABLED",
            "The account that owns this API token is disabled.",
        ));
    }
    if !tokens::ip_allowed(&row.allowed_ips(), ip) {
        return Err(AuthFailure::new(
            StatusCode::FORBIDDEN,
            "IP_NOT_ALLOWED",
            format!(
                "This API token can't be used from {}.",
                ip.map(|i| i.to_string())
                    .unwrap_or_else(|| "this address".to_string())
            ),
        ));
    }
    let kind = row.kind();
    let mut scopes = row.stored_scopes().intersection(ceiling);
    if kind == TokenKind::Agent {
        scopes = scopes
            .to_vec()
            .into_iter()
            .filter(|s| s.group() != ScopeGroup::Admin)
            .collect();
    }
    let (app_ids, project_ids) = (row.app_ids(), row.project_ids());
    let filter = (!app_ids.is_empty() || !project_ids.is_empty()).then_some(ResourceFilter {
        app_ids,
        project_ids,
    });
    Ok(Principal {
        actor: Actor::Token {
            id: row.id.clone(),
            name: row.name.clone(),
            kind,
            owner_id: row.owner_user_id.clone(),
        },
        scopes,
        filter,
        ip,
        session_id: None,
    })
}

/// Authenticate a request. `query_token` is a token passed as `?token=` by
/// WebSocket clients that can't set headers.
pub async fn authenticate(
    parts: &Parts,
    state: &AppState,
    query_token: Option<&str>,
) -> Result<Principal, AuthFailure> {
    if parts.extensions.get::<LocalSocket>().is_some() {
        return Ok(Principal::local_socket());
    }
    let ip = client_ip(parts);

    let bearer = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or(query_token);
    if let Some(secret) = bearer {
        return authenticate_token(state, secret.trim(), ip).await;
    }

    if let Some(token) = extract_session_token(&parts.headers) {
        return match validate_session_with_id(state, &token).await {
            Ok(Some((user, session_id))) => Ok(Principal::for_user(&user, ip, Some(session_id))),
            Ok(None) => Err(AuthFailure::new(
                StatusCode::UNAUTHORIZED,
                "SESSION_EXPIRED",
                "Your session has ended. Sign in again.",
            )),
            Err(_) => Err(AuthFailure::internal()),
        };
    }

    Err(AuthFailure::new(
        StatusCode::UNAUTHORIZED,
        "UNAUTHENTICATED",
        "Authentication required. Use Authorization: Bearer <token> or sign in to the dashboard.",
    ))
}

/// `?token=` from the query string, if present.
fn query_token(parts: &Parts) -> Option<String> {
    parts.uri.query().and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == "token" && !v.is_empty())
                .then(|| urlencoding::decode(v).map(|s| s.into_owned()).ok())
                .flatten()
        })
    })
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = AuthFailure;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(parts, state, None).await
    }
}

/// Non-rejecting authentication for WebSocket upgrades, which report failures
/// inside the socket (close codes) rather than as HTTP errors. Accepts a token
/// in `?token=` as well as the header and cookie.
pub struct MaybePrincipal(pub Result<Principal, AuthFailure>);

impl FromRequestParts<AppState> for MaybePrincipal {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = query_token(parts);
        Ok(MaybePrincipal(
            authenticate(parts, state, token.as_deref()).await,
        ))
    }
}

// ── Scope requirements ──

/// A compile-time scope requirement for [`Authz`].
pub trait ScopeReq: Send + Sync + 'static {
    /// The first required scope the set lacks, if any.
    fn missing(scopes: ScopeSet) -> Option<Scope>;
}

/// Marker types, one per scope, named after `tond_common::Scope` variants.
/// Tuples require every scope: `Authz<(scope::AppsWrite, scope::EnvReadValues)>`.
pub mod scope {
    use super::{Scope, ScopeReq, ScopeSet};

    macro_rules! markers {
        ($($name:ident),+ $(,)?) => {
            $(
                #[doc = concat!("Requires `Scope::", stringify!($name), "`.")]
                #[allow(dead_code)] // one per scope, even if no route uses it yet
                pub struct $name;
                impl ScopeReq for $name {
                    fn missing(scopes: ScopeSet) -> Option<Scope> {
                        (!scopes.contains(Scope::$name)).then_some(Scope::$name)
                    }
                }
            )+
        };
    }

    markers!(
        AppsRead,
        AppsDeploy,
        AppsWrite,
        AppsControl,
        AppsRollback,
        AppsDelete,
        EnvReadNames,
        EnvReadValues,
        EnvWrite,
        DbCredentials,
        DbRead,
        DbWrite,
        DbDelete,
        SnapshotsRead,
        SnapshotsCreate,
        SnapshotsRestore,
        LogsRead,
        MetricsRead,
        EventsRead,
        CronRead,
        CronWrite,
        CronTrigger,
        PreviewsWrite,
        SettingsWrite,
        UsersAdmin,
        TokensAdmin,
        TerminalHost,
    );
}

impl<A: ScopeReq, B: ScopeReq> ScopeReq for (A, B) {
    fn missing(scopes: ScopeSet) -> Option<Scope> {
        A::missing(scopes).or_else(|| B::missing(scopes))
    }
}

/// Extractor: an authenticated principal holding every scope in `R`, allowed
/// to reach the targeted resource. Derefs to [`Principal`].
pub struct Authz<R: ScopeReq> {
    principal: Principal,
    _req: PhantomData<fn() -> R>,
}

impl<R: ScopeReq> Authz<R> {
    /// Wrap a principal without checks (for calling handlers in tests).
    pub fn unchecked(principal: Principal) -> Self {
        Self {
            principal,
            _req: PhantomData,
        }
    }

    /// Check a principal against `R` (scope part only).
    pub fn check(principal: Principal) -> Result<Self, AppError> {
        if let Some(scope) = R::missing(principal.scopes) {
            return Err(AppError(principal.scope_denied(scope)));
        }
        Ok(Self::unchecked(principal))
    }
}

impl<R: ScopeReq> Deref for Authz<R> {
    type Target = Principal;
    fn deref(&self) -> &Principal {
        &self.principal
    }
}

/// What a route acts on, for resource-limited tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceTarget {
    /// GET /api/apps or /api/projects: allowed, the handler filters.
    List,
    App(String),
    Project(String),
    /// Anything else: refused for limited tokens.
    Other,
}

/// Classify a route by its pattern (e.g. `/api/apps/{id}/deploy`).
pub fn resource_target(method: &Method, pattern: &str, id_param: Option<&str>) -> ResourceTarget {
    let id = || id_param.map(str::to_string);
    if (pattern == "/api/apps" || pattern == "/api/projects") && method == Method::GET {
        return ResourceTarget::List;
    }
    if pattern.starts_with("/api/apps/{id}")
        && let Some(id) = id()
    {
        return ResourceTarget::App(id);
    }
    if pattern.starts_with("/api/projects/{id}")
        && let Some(id) = id()
    {
        return ResourceTarget::Project(id);
    }
    ResourceTarget::Other
}

impl<R: ScopeReq> FromRequestParts<AppState> for Authz<R> {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(parts, state, None)
            .await
            .map_err(IntoResponse::into_response)?;
        let authz = Self::check(principal).map_err(IntoResponse::into_response)?;
        if authz.principal.filter.is_some() {
            let pattern = parts
                .extensions
                .get::<MatchedPath>()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let params = RawPathParams::from_request_parts(parts, state).await.ok();
            let id = params.as_ref().and_then(|p| {
                p.iter()
                    .find(|(k, _)| *k == "id")
                    .map(|(_, v)| v.to_string())
            });
            let result = match resource_target(&parts.method, &pattern, id.as_deref()) {
                ResourceTarget::List => Ok(()),
                ResourceTarget::App(key) => authz.ensure_app_access(&state.db, &key).await,
                ResourceTarget::Project(key) => authz.ensure_project_access(&state.db, &key).await,
                ResourceTarget::Other => Err(resource_denied()),
            };
            result.map_err(IntoResponse::into_response)?;
        }
        Ok(authz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> TokenRow {
        TokenRow {
            id: "tok-1".into(),
            name: "ci".into(),
            prefix: "tond_pat_12345678".into(),
            owner_user_id: Some("user-1".into()),
            kind: "ci".into(),
            scopes: r#"["apps:read","apps:deploy","settings:write"]"#.into(),
            app_ids: Some(r#"["app-1"]"#.into()),
            project_ids: None,
            allowed_ips: None,
            expires_at: None,
            last_used_at: None,
            last_used_ip: None,
            revoked_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            owner_email: Some("dev@example.com".into()),
            owner_role: Some("deployer".into()),
            owner_disabled_at: None,
        }
    }

    #[test]
    fn token_scopes_are_capped_by_the_owners_current_role() {
        let p = principal_for_token(&row(), None, Utc::now()).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(p.has(Scope::AppsDeploy));
        assert!(
            !p.has(Scope::SettingsWrite),
            "a deployer's token can't use settings:write even if it was stored"
        );
        let mut demoted = row();
        demoted.owner_role = Some("viewer".into());
        let p = principal_for_token(&demoted, None, Utc::now()).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(p.has(Scope::AppsRead));
        assert!(
            !p.has(Scope::AppsDeploy),
            "demotion narrows tokens immediately"
        );
    }

    #[test]
    fn root_tokens_have_no_ceiling_but_agent_tokens_never_get_admin_scopes() {
        let mut root = row();
        root.owner_user_id = None;
        root.owner_role = None;
        root.owner_email = None;
        let p = principal_for_token(&root, None, Utc::now()).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(p.has(Scope::SettingsWrite));
        root.kind = "agent".into();
        let p = principal_for_token(&root, None, Utc::now()).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!p.has(Scope::SettingsWrite));
        assert!(p.has(Scope::AppsDeploy));
    }

    #[test]
    fn revoked_expired_disabled_and_foreign_ip_tokens_are_refused() {
        let now = Utc::now();
        let mut r = row();
        r.revoked_at = Some("2026-01-02T00:00:00Z".into());
        assert_eq!(
            principal_for_token(&r, None, now)
                .err()
                .map(|e| (e.status, e.code)),
            Some((StatusCode::UNAUTHORIZED, "TOKEN_REVOKED"))
        );
        let mut r = row();
        r.expires_at = Some((now - chrono::Duration::seconds(1)).to_rfc3339());
        assert_eq!(
            principal_for_token(&r, None, now)
                .err()
                .map(|e| (e.status, e.code)),
            Some((StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED"))
        );
        let mut r = row();
        r.owner_disabled_at = Some("2026-01-02T00:00:00Z".into());
        assert_eq!(
            principal_for_token(&r, None, now).err().map(|e| e.code),
            Some("ACCOUNT_DISABLED")
        );
        let mut r = row();
        r.allowed_ips = Some(r#"["10.0.0.0/8"]"#.into());
        assert_eq!(
            principal_for_token(&r, "203.0.113.1".parse().ok(), now)
                .err()
                .map(|e| (e.status, e.code)),
            Some((StatusCode::FORBIDDEN, "IP_NOT_ALLOWED"))
        );
        assert!(principal_for_token(&r, "10.2.3.4".parse().ok(), now).is_ok());
    }

    #[test]
    fn token_principals_audit_as_the_token_not_a_generic_label() {
        let p = principal_for_token(&row(), "203.0.113.7".parse().ok(), Utc::now())
            .unwrap_or_else(|e| panic!("{e:?}"));
        let actor = p.audit_actor();
        assert_eq!(actor.kind, "token");
        assert_eq!(actor.id.as_deref(), Some("tok-1"));
        assert_eq!(actor.label, "ci");
        assert_eq!(actor.ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(
            Principal::local_socket().audit_actor().label,
            "local-socket"
        );
    }

    #[test]
    fn authz_checks_every_required_scope() {
        let viewer = Principal::test_user(Role::Viewer);
        assert!(Authz::<scope::AppsRead>::check(viewer.clone()).is_ok());
        let err = Authz::<scope::AppsDeploy>::check(viewer.clone())
            .err()
            .unwrap_or_else(|| panic!("viewers can't deploy"));
        assert_eq!(err.0.status_code(), 403);
        assert_eq!(err.0.code(), Some("SCOPE_DENIED"));
        assert!(err.0.to_string().contains("apps:deploy"));
        assert!(
            Authz::<(scope::AppsRead, scope::EnvReadValues)>::check(viewer).is_err(),
            "tuples need both scopes"
        );
        assert!(
            Authz::<(scope::AppsRead, scope::EnvReadValues)>::check(Principal::local_socket())
                .is_ok()
        );
    }

    #[test]
    fn resource_targets_are_derived_from_the_route_pattern() {
        let get = Method::GET;
        let post = Method::POST;
        assert_eq!(
            resource_target(&get, "/api/apps", None),
            ResourceTarget::List
        );
        assert_eq!(
            resource_target(&post, "/api/apps", None),
            ResourceTarget::Other
        );
        assert_eq!(
            resource_target(&post, "/api/apps/{id}/deploy", Some("web")),
            ResourceTarget::App("web".into())
        );
        assert_eq!(
            resource_target(&get, "/api/projects/{id}", Some("p1")),
            ResourceTarget::Project("p1".into())
        );
        assert_eq!(
            resource_target(&get, "/api/databases", None),
            ResourceTarget::Other
        );
        assert_eq!(
            resource_target(&get, "/api/settings", None),
            ResourceTarget::Other
        );
    }

    #[test]
    fn resource_filters_cover_apps_directly_or_through_their_project() {
        let f = ResourceFilter {
            app_ids: vec!["a1".into()],
            project_ids: vec!["p1".into()],
        };
        assert!(f.allows_app("a1", None));
        assert!(f.allows_app("a2", Some("p1")));
        assert!(!f.allows_app("a2", Some("p2")));
        assert!(!f.allows_app("a3", None));
        assert!(f.allows_project("p1"));
        assert!(!f.allows_project("p2"));
    }

    #[test]
    fn query_tokens_are_read_and_decoded() {
        let parts = axum::http::Request::builder()
            .uri("/api/apps/x/logs/stream?foo=1&token=tond_pat_abc%3D")
            .body(())
            .unwrap_or_else(|e| panic!("{e}"))
            .into_parts()
            .0;
        assert_eq!(query_token(&parts).as_deref(), Some("tond_pat_abc="));
    }
}
