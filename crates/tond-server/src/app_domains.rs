// app_domains.rs — the domains an app answers on (model and rules)
//
// `app_domains` is the source of truth; `apps.domain` is a read-only mirror
// of the primary domain kept by SQL triggers (001_baseline.sql), so code that
// only needs "the app's address" keeps reading `apps.domain`. Never write
// `apps.domain` directly: go through the functions here.
//
// Rules:
//   - a domain belongs to at most one app (primary key);
//   - an app has at most one primary; the first non-redirect domain added
//     becomes primary, and removing the primary promotes the oldest alias;
//   - a redirect points at another non-redirect domain of the same app (no
//     chains, no open redirects); a domain that redirects are aimed at can't
//     be removed or turned into a redirect until they point elsewhere;
//   - custom domains follow domains::check_custom_app_domain (never the
//     dashboard domain or under the preview domain);
//   - every new domain gets a pending certificate row with a fresh backoff,
//     so tond-proxy orders its certificate on the next pass.
//
//   list()              domains of an app, primary first
//   add()               add one domain (alias/primary/redirect)
//   update()            change kind / redirect target / code
//   remove()            remove one domain
//   www_pair()          add example.com + www.example.com, one redirecting
//   set_primary()       add-if-missing and make primary (PUT /domain)
//   assign_automatic()  the apps-domain address on first deploy
//   clear()             drop every domain (background workers)

use sqlx::{Sqlite, SqlitePool, Transaction};
use tond_common::{
    AddAppDomainRequest, AppDomain, AppDomainResponse, CanonicalHost, DomainKind, REDIRECT_CODES,
    TondError, UpdateAppDomainRequest, WwwPairRequest, www_pair as split_www_pair,
};

use crate::domains::{check_custom_app_domain, normalize_domain, same_site, same_site_warning};

/// The domain settings the rules depend on.
#[derive(Debug, Clone, Default)]
pub struct DomainContext {
    pub dashboard: String,
    pub preview: String,
    pub apps: String,
}

impl DomainContext {
    pub async fn load(state: &crate::state::AppState) -> Self {
        let s = state.settings.read().await;
        Self {
            dashboard: s.dashboard_domain.clone(),
            preview: s.preview_domain.clone(),
            apps: s.apps_domain.clone(),
        }
    }

    fn is_automatic(&self, domain: &str) -> bool {
        !self.apps.is_empty()
            && tond_common::dns::wildcard_covers(&format!("*.{}", self.apps), domain)
    }

    pub fn response(&self, d: AppDomain) -> AppDomainResponse {
        let warning = same_site(&d.domain, &self.dashboard)
            .then(|| same_site_warning(&d.domain, &self.dashboard));
        AppDomainResponse {
            automatic: self.is_automatic(&d.domain),
            kind: d.kind(),
            redirect_code: u16::try_from(d.redirect_code).unwrap_or(308),
            domain: d.domain,
            redirect_to: d.redirect_to,
            service: d.service,
            port: d.port,
            created_at: d.created_at,
            warning,
        }
    }
}

fn invalid_domain(msg: impl Into<String>) -> TondError {
    TondError::invalid_field("domain", msg)
}

/// Domains of an app: primary, then aliases, then redirects, oldest first.
pub async fn list(db: &SqlitePool, app_id: &str) -> Result<Vec<AppDomain>, sqlx::Error> {
    sqlx::query_as::<_, AppDomain>(
        "SELECT * FROM app_domains WHERE app_id = ?
         ORDER BY CASE kind WHEN 'primary' THEN 0 WHEN 'alias' THEN 1 ELSE 2 END, created_at, domain",
    )
    .bind(app_id)
    .fetch_all(db)
    .await
}

async fn get_in(
    tx: &mut Transaction<'_, Sqlite>,
    domain: &str,
) -> Result<Option<AppDomain>, sqlx::Error> {
    sqlx::query_as::<_, AppDomain>("SELECT * FROM app_domains WHERE domain = ?")
        .bind(domain)
        .fetch_optional(&mut **tx)
        .await
}

/// Normalize and validate a domain for `app_id`: syntax, the dashboard and
/// preview domain rules, and not used by another app.
async fn check_new_domain(
    tx: &mut Transaction<'_, Sqlite>,
    ctx: &DomainContext,
    app_id: &str,
    raw: &str,
) -> Result<(String, Option<AppDomain>), TondError> {
    let domain = normalize_domain(raw);
    check_custom_app_domain(&domain, &ctx.dashboard, &ctx.preview).map_err(invalid_domain)?;
    let existing = get_in(tx, &domain).await?;
    if let Some(row) = &existing
        && row.app_id != app_id
    {
        let other: Option<String> = sqlx::query_scalar("SELECT name FROM apps WHERE id = ?")
            .bind(&row.app_id)
            .fetch_optional(&mut **tx)
            .await?;
        return Err(TondError::Conflict(format!(
            "{domain} is already used by the app '{}'",
            other.unwrap_or_else(|| row.app_id.clone())
        ))
        .with_hint("Remove it from that app first, then add it here."));
    }
    Ok((domain, existing))
}

fn check_code(code: Option<u16>) -> Result<i64, TondError> {
    let code = code.unwrap_or(308);
    if !REDIRECT_CODES.contains(&code) {
        return Err(TondError::invalid_field(
            "redirect_code",
            format!("Redirects answer with 301 or 308, not {code}"),
        ));
    }
    Ok(i64::from(code))
}

/// The redirect target must be a non-redirect domain of the same app.
async fn check_redirect_target(
    tx: &mut Transaction<'_, Sqlite>,
    app_id: &str,
    domain: &str,
    target: Option<&str>,
) -> Result<String, TondError> {
    let target = normalize_domain(target.unwrap_or_default());
    if target.is_empty() {
        return Err(TondError::invalid_field(
            "redirect_to",
            format!("Choose the domain {domain} should redirect to"),
        ));
    }
    if target == domain {
        return Err(TondError::invalid_field(
            "redirect_to",
            format!("{domain} can't redirect to itself"),
        ));
    }
    match get_in(tx, &target).await? {
        Some(t) if t.app_id == app_id && t.kind() != DomainKind::Redirect => Ok(target),
        Some(t) if t.app_id == app_id => Err(TondError::invalid_field(
            "redirect_to",
            format!("{target} is itself a redirect; point {domain} at the domain it redirects to"),
        )),
        _ => Err(TondError::invalid_field(
            "redirect_to",
            format!(
                "{target} isn't a domain of this app. Redirects can only point at the app's own \
                 domains; add {target} first."
            ),
        )),
    }
}

/// Domains that redirect to `domain`.
async fn redirects_to(
    tx: &mut Transaction<'_, Sqlite>,
    domain: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT domain FROM app_domains WHERE redirect_to = ? ORDER BY domain")
        .bind(domain)
        .fetch_all(&mut **tx)
        .await
}

async fn demote_primary(
    tx: &mut Transaction<'_, Sqlite>,
    app_id: &str,
    except: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE app_domains SET kind = 'alias' WHERE app_id = ? AND kind = 'primary' AND domain != ?",
    )
    .bind(app_id)
    .bind(except)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn has_primary(tx: &mut Transaction<'_, Sqlite>, app_id: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM app_domains WHERE app_id = ? AND kind = 'primary'",
    )
    .bind(app_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(n > 0)
}

/// Make the oldest alias primary when the app has none.
async fn promote_if_needed(
    tx: &mut Transaction<'_, Sqlite>,
    app_id: &str,
) -> Result<(), sqlx::Error> {
    if has_primary(tx, app_id).await? {
        return Ok(());
    }
    sqlx::query(
        "UPDATE app_domains SET kind = 'primary' WHERE domain = (
            SELECT domain FROM app_domains WHERE app_id = ? AND kind = 'alias'
            ORDER BY created_at, domain LIMIT 1)",
    )
    .bind(app_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert or overwrite one row.
#[allow(clippy::too_many_arguments)]
async fn upsert(
    tx: &mut Transaction<'_, Sqlite>,
    app_id: &str,
    domain: &str,
    kind: DomainKind,
    redirect_to: Option<&str>,
    redirect_code: i64,
    service: Option<&str>,
    port: Option<i64>,
) -> Result<(), sqlx::Error> {
    if kind == DomainKind::Primary {
        demote_primary(tx, app_id, domain).await?;
    }
    sqlx::query(
        "INSERT INTO app_domains (domain, app_id, service, port, kind, redirect_to, redirect_code)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(domain) DO UPDATE SET
            kind = excluded.kind,
            redirect_to = excluded.redirect_to,
            redirect_code = excluded.redirect_code,
            service = excluded.service,
            port = excluded.port",
    )
    .bind(domain)
    .bind(app_id)
    .bind(service)
    .bind(port)
    .bind(kind.as_str())
    .bind(redirect_to)
    .bind(redirect_code)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Queue a certificate for a newly added domain: a pending row with a fresh
/// backoff (an existing row keeps its certificate but retries now).
pub async fn queue_certificate(
    tx: &mut Transaction<'_, Sqlite>,
    domain: &str,
) -> Result<(), sqlx::Error> {
    let data_dir = std::env::var("TOND_DATA_DIR").unwrap_or_else(|_| "/opt/tond/data".to_string());
    let dir = format!("{data_dir}/certs/{domain}");
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO certificates (id, domain, cert_type, cert_path, key_path, issued_at, expires_at,
                                   status, created_at, updated_at)
         VALUES (?, ?, 'acme', ?, ?, ?, ?, 'pending', ?, ?)
         ON CONFLICT(domain) DO UPDATE SET updated_at = excluded.updated_at,
            last_error = NULL, next_retry_at = NULL, failure_count = 0
         WHERE certificates.cert_type != 'custom'",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(domain)
    .bind(format!("{dir}/cert.pem"))
    .bind(format!("{dir}/key.pem"))
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn fetch(db: &SqlitePool, domain: &str) -> Result<AppDomain, TondError> {
    sqlx::query_as::<_, AppDomain>("SELECT * FROM app_domains WHERE domain = ?")
        .bind(domain)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| TondError::NotFound(format!("Domain {domain}")))
}

/// Add a domain to an app. Without a kind it becomes the primary domain when
/// the app has none, else an alias.
pub async fn add(
    db: &SqlitePool,
    ctx: &DomainContext,
    app_id: &str,
    req: &AddAppDomainRequest,
) -> Result<AppDomain, TondError> {
    let mut tx = crate::db::begin_write(db).await?;
    let (domain, existing) = check_new_domain(&mut tx, ctx, app_id, &req.domain).await?;
    if existing.is_some() {
        return Err(TondError::Conflict(format!(
            "{domain} is already a domain of this app"
        )));
    }
    let service = req
        .service
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(port) = req.port
        && !(1..=65535).contains(&port)
    {
        return Err(TondError::invalid_field(
            "port",
            "port must be between 1 and 65535",
        ));
    }
    let kind = match req.kind {
        Some(kind) => kind,
        None if has_primary(&mut tx, app_id).await? => DomainKind::Alias,
        None => DomainKind::Primary,
    };
    let (redirect_to, code) = if kind == DomainKind::Redirect {
        let target =
            check_redirect_target(&mut tx, app_id, &domain, req.redirect_to.as_deref()).await?;
        (Some(target), check_code(req.redirect_code)?)
    } else {
        (None, 308)
    };
    upsert(
        &mut tx,
        app_id,
        &domain,
        kind,
        redirect_to.as_deref(),
        code,
        service,
        req.port,
    )
    .await?;
    promote_if_needed(&mut tx, app_id).await?;
    queue_certificate(&mut tx, &domain).await?;
    tx.commit().await?;
    fetch(db, &domain).await
}

/// Change a domain's kind, redirect target or code.
pub async fn update(
    db: &SqlitePool,
    app_id: &str,
    domain: &str,
    req: &UpdateAppDomainRequest,
) -> Result<AppDomain, TondError> {
    let domain = normalize_domain(domain);
    let mut tx = crate::db::begin_write(db).await?;
    let current = get_in(&mut tx, &domain)
        .await?
        .filter(|d| d.app_id == app_id)
        .ok_or_else(|| TondError::NotFound(format!("{domain} is not a domain of this app")))?;
    let kind = req.kind.unwrap_or(current.kind());
    if current.kind() == DomainKind::Primary && kind != DomainKind::Primary {
        return Err(
            TondError::Conflict(format!("{domain} is the primary domain"))
                .with_hint("Make another domain primary first."),
        );
    }
    let (redirect_to, code) = if kind == DomainKind::Redirect {
        let pointing = redirects_to(&mut tx, &domain).await?;
        if !pointing.is_empty() {
            return Err(TondError::Conflict(format!(
                "{} redirect{} to {domain}",
                pointing.join(", "),
                if pointing.len() == 1 { "s" } else { "" },
            ))
            .with_hint("Point those redirects at another domain first."));
        }
        let target = req
            .redirect_to
            .as_deref()
            .or(current.redirect_to.as_deref());
        let target = check_redirect_target(&mut tx, app_id, &domain, target).await?;
        let code = req
            .redirect_code
            .or(u16::try_from(current.redirect_code).ok());
        (Some(target), check_code(code)?)
    } else {
        (None, 308)
    };
    upsert(
        &mut tx,
        app_id,
        &domain,
        kind,
        redirect_to.as_deref(),
        code,
        current.service.as_deref(),
        current.port,
    )
    .await?;
    promote_if_needed(&mut tx, app_id).await?;
    tx.commit().await?;
    fetch(db, &domain).await
}

/// Remove a domain. The oldest alias becomes primary when the primary goes.
pub async fn remove(db: &SqlitePool, app_id: &str, domain: &str) -> Result<AppDomain, TondError> {
    let domain = normalize_domain(domain);
    let mut tx = crate::db::begin_write(db).await?;
    let current = get_in(&mut tx, &domain)
        .await?
        .filter(|d| d.app_id == app_id)
        .ok_or_else(|| TondError::NotFound(format!("{domain} is not a domain of this app")))?;
    let pointing = redirects_to(&mut tx, &domain).await?;
    if !pointing.is_empty() {
        return Err(TondError::Conflict(format!(
            "{} redirect{} to {domain}",
            pointing.join(", "),
            if pointing.len() == 1 { "s" } else { "" },
        ))
        .with_hint("Remove those redirects or point them at another domain first."));
    }
    sqlx::query("DELETE FROM app_domains WHERE domain = ?")
        .bind(&domain)
        .execute(&mut *tx)
        .await?;
    promote_if_needed(&mut tx, app_id).await?;
    tx.commit().await?;
    Ok(current)
}

/// Add both `example.com` and `www.example.com`: the canonical one serves
/// (primary unless `primary: false`), the other redirects to it with a 308.
/// Existing domains of the same app are converted; redirects that pointed at
/// the redirecting name follow it to the canonical one.
pub async fn www_pair(
    db: &SqlitePool,
    ctx: &DomainContext,
    app_id: &str,
    req: &WwwPairRequest,
) -> Result<Vec<AppDomain>, TondError> {
    let (apex, www) = split_www_pair(&normalize_domain(&req.domain));
    let (canonical, other) = match req.canonical {
        CanonicalHost::Apex => (apex, www),
        CanonicalHost::Www => (www, apex),
    };
    let mut tx = crate::db::begin_write(db).await?;
    let (canonical, canonical_row) = check_new_domain(&mut tx, ctx, app_id, &canonical).await?;
    let (other, other_row) = check_new_domain(&mut tx, ctx, app_id, &other).await?;

    let make_primary = req.primary.unwrap_or(true);
    let canonical_kind = match &canonical_row {
        Some(row) if row.kind() == DomainKind::Primary => DomainKind::Primary,
        _ if make_primary => DomainKind::Primary,
        _ => DomainKind::Alias,
    };
    // The redirecting name may be the current primary: demote it first.
    if other_row
        .as_ref()
        .is_some_and(|r| r.kind() == DomainKind::Primary)
        && canonical_kind != DomainKind::Primary
    {
        return Err(
            TondError::Conflict(format!("{other} is the primary domain")).with_hint(format!(
                "Make {canonical} primary to redirect {other} to it."
            )),
        );
    }
    let service = canonical_row.as_ref().and_then(|r| r.service.clone());
    let port = canonical_row.as_ref().and_then(|r| r.port);
    upsert(
        &mut tx,
        app_id,
        &canonical,
        canonical_kind,
        None,
        308,
        service.as_deref(),
        port,
    )
    .await?;
    sqlx::query("UPDATE app_domains SET redirect_to = ? WHERE redirect_to = ? AND app_id = ?")
        .bind(&canonical)
        .bind(&other)
        .bind(app_id)
        .execute(&mut *tx)
        .await?;
    upsert(
        &mut tx,
        app_id,
        &other,
        DomainKind::Redirect,
        Some(&canonical),
        308,
        other_row.as_ref().and_then(|r| r.service.as_deref()),
        other_row.as_ref().and_then(|r| r.port),
    )
    .await?;
    promote_if_needed(&mut tx, app_id).await?;
    if canonical_row.is_none() {
        queue_certificate(&mut tx, &canonical).await?;
    }
    if other_row.is_none() {
        queue_certificate(&mut tx, &other).await?;
    }
    tx.commit().await?;
    Ok(vec![fetch(db, &canonical).await?, fetch(db, &other).await?])
}

/// PUT /api/apps/{id}/domain: make `domain` the primary domain, adding it if
/// needed. The previous primary stays as an alias.
pub async fn set_primary(
    db: &SqlitePool,
    ctx: &DomainContext,
    app_id: &str,
    raw: &str,
) -> Result<AppDomain, TondError> {
    let mut tx = crate::db::begin_write(db).await?;
    // A redirect made primary serves again (nothing can point at a redirect).
    let (domain, existing) = check_new_domain(&mut tx, ctx, app_id, raw).await?;
    upsert(
        &mut tx,
        app_id,
        &domain,
        DomainKind::Primary,
        None,
        308,
        existing.as_ref().and_then(|r| r.service.as_deref()),
        existing.as_ref().and_then(|r| r.port),
    )
    .await?;
    if existing.is_none() {
        queue_certificate(&mut tx, &domain).await?;
    }
    tx.commit().await?;
    fetch(db, &domain).await
}

/// Give an app without any domain its automatic address as primary. Returns
/// false when the name is taken (another app already uses it).
pub async fn assign_automatic(
    db: &SqlitePool,
    app_id: &str,
    domain: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = crate::db::begin_write(db).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_domains WHERE app_id = ?")
        .bind(app_id)
        .fetch_one(&mut *tx)
        .await?;
    if count > 0 {
        return Ok(true);
    }
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO app_domains (domain, app_id, kind) VALUES (?, ?, 'primary')",
    )
    .bind(domain)
    .bind(app_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(inserted > 0)
}

// ── Compose services ──
//
// A compose app's domains are rows with `service` (and `port`, the container
// port Tond publishes for that service) set. The same rules apply: one app
// per domain, one primary per app, redirects only to the app's own domains.

/// Point the app's domains that name no service (the address given at
/// creation, seeded as primary by the apps_domain_seed trigger) at a compose
/// service and its published port.
pub async fn attach_to_service(
    db: &SqlitePool,
    app_id: &str,
    service: &str,
    port: i64,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE app_domains SET service = ?, port = ?
         WHERE app_id = ? AND service IS NULL AND kind != 'redirect'",
    )
    .bind(service)
    .bind(port)
    .bind(app_id)
    .execute(db)
    .await?
    .rows_affected())
}

/// Replace the serving domains of one compose service with `domains` (on
/// `port`). New domains are checked like [`add`] and get a certificate; the
/// first becomes primary when the app has none. A removed domain that
/// redirects point at is refused. Returns the domains added.
pub async fn set_service_domains(
    db: &SqlitePool,
    ctx: &DomainContext,
    app_id: &str,
    service: &str,
    port: Option<i64>,
    domains: &[String],
) -> Result<Vec<String>, TondError> {
    let mut tx = crate::db::begin_write(db).await?;
    let mut wanted: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for raw in domains {
        let (domain, existing) = check_new_domain(&mut tx, ctx, app_id, raw).await?;
        if wanted.contains(&domain) {
            continue;
        }
        match existing {
            Some(row) if row.service.as_deref() != Some(service) => {
                return Err(TondError::Conflict(format!(
                    "{domain} is already a domain of this app{}",
                    row.service
                        .map(|s| format!(" (service {s})"))
                        .unwrap_or_default()
                ))
                .with_hint("Remove it there first."));
            }
            Some(row) if row.kind() == DomainKind::Redirect => {
                return Err(TondError::invalid_field(
                    "domains",
                    format!("{domain} is a redirect; change it in the app's domains first"),
                ));
            }
            Some(_) => {}
            None => added.push(domain.clone()),
        }
        wanted.push(domain);
    }
    let current: Vec<String> = sqlx::query_scalar(
        "SELECT domain FROM app_domains WHERE app_id = ? AND service = ? AND kind != 'redirect'",
    )
    .bind(app_id)
    .bind(service)
    .fetch_all(&mut *tx)
    .await?;
    for gone in current.iter().filter(|d| !wanted.contains(d)) {
        let pointing = redirects_to(&mut tx, gone).await?;
        if !pointing.is_empty() {
            return Err(TondError::Conflict(format!(
                "{} redirect{} to {gone}",
                pointing.join(", "),
                if pointing.len() == 1 { "s" } else { "" },
            ))
            .with_hint("Remove those redirects or point them at another domain first."));
        }
        sqlx::query("DELETE FROM app_domains WHERE domain = ?")
            .bind(gone)
            .execute(&mut *tx)
            .await?;
    }
    // Existing rows follow the published port.
    sqlx::query("UPDATE app_domains SET port = ? WHERE app_id = ? AND service = ?")
        .bind(port)
        .bind(app_id)
        .bind(service)
        .execute(&mut *tx)
        .await?;
    for domain in &added {
        let kind = if has_primary(&mut tx, app_id).await? {
            DomainKind::Alias
        } else {
            DomainKind::Primary
        };
        upsert(
            &mut tx,
            app_id,
            domain,
            kind,
            None,
            308,
            Some(service),
            port,
        )
        .await?;
        queue_certificate(&mut tx, domain).await?;
    }
    promote_if_needed(&mut tx, app_id).await?;
    tx.commit().await?;
    Ok(added)
}

/// Drop the domains of a compose service that left the file (redirects to
/// them go too), promoting another domain to primary if needed.
pub async fn remove_service(
    tx: &mut Transaction<'_, Sqlite>,
    app_id: &str,
    service: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM app_domains WHERE app_id = ? AND redirect_to IN (
            SELECT domain FROM app_domains WHERE app_id = ? AND service = ?)",
    )
    .bind(app_id)
    .bind(app_id)
    .bind(service)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM app_domains WHERE app_id = ? AND service = ?")
        .bind(app_id)
        .bind(service)
        .execute(&mut **tx)
        .await?;
    promote_if_needed(tx, app_id).await
}

/// Give a compose app without any domain its automatic address, on `service`.
pub async fn assign_automatic_to_service(
    db: &SqlitePool,
    app_id: &str,
    domain: &str,
    service: &str,
    port: i64,
) -> Result<bool, sqlx::Error> {
    if !assign_automatic(db, app_id, domain).await? {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE app_domains SET service = ?, port = ? WHERE domain = ? AND app_id = ?
           AND service IS NULL",
    )
    .bind(service)
    .bind(port)
    .bind(domain)
    .bind(app_id)
    .execute(db)
    .await?;
    Ok(true)
}

/// Remove every domain of an app (it became a background worker).
pub async fn clear(db: &SqlitePool, app_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let removed: Vec<String> =
        sqlx::query_scalar("DELETE FROM app_domains WHERE app_id = ? RETURNING domain")
            .bind(app_id)
            .fetch_all(db)
            .await?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        for (id, name) in [("a1", "web"), ("a2", "api")] {
            sqlx::query(
                "INSERT INTO apps (id, name, created_at, updated_at) VALUES (?, ?, 'now', 'now')",
            )
            .bind(id)
            .bind(name)
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("insert app: {e}"));
        }
        db
    }

    fn ctx() -> DomainContext {
        DomainContext {
            dashboard: "dash.example.com".into(),
            preview: "example-previews.com".into(),
            apps: "example-apps.com".into(),
        }
    }

    #[tokio::test]
    async fn compose_service_domains_follow_the_same_rules() {
        let db = db().await;
        let ctx = ctx();
        let set = |service: &'static str, port: i64, domains: &'static [&'static str]| {
            let db = db.clone();
            let ctx = ctx.clone();
            async move {
                let domains: Vec<String> = domains.iter().map(|d| d.to_string()).collect();
                set_service_domains(&db, &ctx, "a1", service, Some(port), &domains).await
            }
        };
        // The first domain of the app becomes primary; apps.domain mirrors it.
        let added = set("web", 80, &["Shop.Example.com", "www.shop.example.com"])
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(added, ["shop.example.com", "www.shop.example.com"]);
        set("admin", 8080, &["admin.example.com"])
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let rows = list(&db, "a1").await.unwrap_or_else(|e| panic!("{e}"));
        let mut view: Vec<(&str, &str, Option<&str>, Option<i64>)> = rows
            .iter()
            .map(|r| {
                (
                    r.domain.as_str(),
                    r.kind.as_str(),
                    r.service.as_deref(),
                    r.port,
                )
            })
            .collect();
        // Aliases list oldest first (created_at has whole seconds): the two
        // calls may straddle a second, so compare the aliases by name.
        view[1..].sort_by_key(|r| r.0);
        assert_eq!(
            view,
            [
                ("shop.example.com", "primary", Some("web"), Some(80)),
                ("admin.example.com", "alias", Some("admin"), Some(8080)),
                ("www.shop.example.com", "alias", Some("web"), Some(80)),
            ]
        );
        // A domain of another service or app is refused.
        assert!(set("admin", 8080, &["shop.example.com"]).await.is_err());
        sqlx::query("INSERT INTO app_domains (domain, app_id) VALUES ('api.example.com', 'a2')")
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(set("admin", 8080, &["api.example.com"]).await.is_err());
        // Replacing the list follows a new port and promotes a new primary.
        set("web", 3000, &["www.shop.example.com"])
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let primary: Option<String> = sqlx::query_scalar("SELECT domain FROM apps WHERE id = 'a1'")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(primary.as_deref(), Some("admin.example.com"));
        let port: Option<i64> = sqlx::query_scalar(
            "SELECT port FROM app_domains WHERE domain = 'www.shop.example.com'",
        )
        .fetch_one(&db)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(port, Some(3000));
        // A service leaving the file takes its domains (and redirects to them).
        let mut tx = crate::db::begin_write(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        remove_service(&mut tx, "a1", "admin")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        tx.commit().await.unwrap_or_else(|e| panic!("{e}"));
        let primary: Option<String> = sqlx::query_scalar("SELECT domain FROM apps WHERE id = 'a1'")
            .fetch_one(&db)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(primary.as_deref(), Some("www.shop.example.com"));
    }

    fn req(domain: &str, kind: Option<DomainKind>, to: Option<&str>) -> AddAppDomainRequest {
        AddAppDomainRequest {
            domain: domain.into(),
            kind,
            redirect_to: to.map(Into::into),
            ..Default::default()
        }
    }

    async fn apps_domain(db: &SqlitePool, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT domain FROM apps WHERE id = ?")
            .bind(id)
            .fetch_one(db)
            .await
            .unwrap_or_else(|e| panic!("apps.domain: {e}"))
    }

    async fn kinds(db: &SqlitePool, id: &str) -> Vec<(String, String)> {
        list(db, id)
            .await
            .unwrap_or_else(|e| panic!("list: {e}"))
            .into_iter()
            .map(|d| (d.domain, d.kind))
            .collect()
    }

    fn ok<T>(r: Result<T, TondError>) -> T {
        r.unwrap_or_else(|e| panic!("unexpected error: {e}"))
    }

    #[tokio::test]
    async fn the_first_domain_is_primary_and_mirrored_into_apps_domain() {
        let db = db().await;
        ok(add(&db, &ctx(), "a1", &req("Shop.Example.org.", None, None)).await);
        ok(add(&db, &ctx(), "a1", &req("store.example.org", None, None)).await);
        assert_eq!(
            kinds(&db, "a1").await,
            vec![
                ("shop.example.org".into(), "primary".into()),
                ("store.example.org".into(), "alias".into())
            ]
        );
        assert_eq!(
            apps_domain(&db, "a1").await.as_deref(),
            Some("shop.example.org")
        );

        // Making the alias primary demotes the old one; apps.domain follows.
        let patch = UpdateAppDomainRequest {
            kind: Some(DomainKind::Primary),
            ..Default::default()
        };
        ok(update(&db, "a1", "store.example.org", &patch).await);
        assert_eq!(
            apps_domain(&db, "a1").await.as_deref(),
            Some("store.example.org")
        );

        // Removing the primary promotes the remaining alias.
        ok(remove(&db, "a1", "store.example.org").await);
        assert_eq!(
            apps_domain(&db, "a1").await.as_deref(),
            Some("shop.example.org")
        );
        ok(remove(&db, "a1", "shop.example.org").await);
        assert_eq!(apps_domain(&db, "a1").await, None);
    }

    #[tokio::test]
    async fn a_domain_belongs_to_one_app_and_follows_the_site_rules() {
        let db = db().await;
        ok(add(&db, &ctx(), "a1", &req("shop.example.org", None, None)).await);
        let taken = add(&db, &ctx(), "a2", &req("shop.example.org", None, None)).await;
        assert!(
            matches!(&taken, Err(e) if e.code() == "CONFLICT" && e.message().contains("already used by the app 'web'")),
            "got {taken:?}"
        );
        for bad in ["dash.example.com", "x.example-previews.com", "not a domain"] {
            assert!(
                add(&db, &ctx(), "a2", &req(bad, None, None)).await.is_err(),
                "{bad} must be refused"
            );
        }
        let dup = add(&db, &ctx(), "a1", &req("shop.example.org", None, None)).await;
        assert!(dup.is_err(), "adding the same domain twice is refused");
    }

    #[tokio::test]
    async fn redirects_point_at_a_serving_domain_of_the_same_app() {
        let db = db().await;
        ok(add(&db, &ctx(), "a1", &req("shop.example.org", None, None)).await);
        ok(add(&db, &ctx(), "a2", &req("api.example.org", None, None)).await);

        let other_app = add(
            &db,
            &ctx(),
            "a1",
            &req(
                "old.example.org",
                Some(DomainKind::Redirect),
                Some("api.example.org"),
            ),
        )
        .await;
        assert!(other_app.is_err(), "no redirects to another app's domain");
        let missing = add(
            &db,
            &ctx(),
            "a1",
            &req("old.example.org", Some(DomainKind::Redirect), None),
        )
        .await;
        assert!(missing.is_err(), "a redirect needs a target");

        let r = ok(add(
            &db,
            &ctx(),
            "a1",
            &req(
                "old.example.org",
                Some(DomainKind::Redirect),
                Some("shop.example.org"),
            ),
        )
        .await);
        assert_eq!(r.redirect_to.as_deref(), Some("shop.example.org"));
        assert_eq!(r.redirect_code, 308);

        let chain = add(
            &db,
            &ctx(),
            "a1",
            &req(
                "older.example.org",
                Some(DomainKind::Redirect),
                Some("old.example.org"),
            ),
        )
        .await;
        assert!(chain.is_err(), "no redirect chains");

        let target_removed = remove(&db, "a1", "shop.example.org").await;
        assert!(
            matches!(&target_removed, Err(e) if e.code() == "CONFLICT" && e.message().contains("old.example.org redirects") && e.hint().is_some()),
            "got {target_removed:?}"
        );
        let bad_code = UpdateAppDomainRequest {
            redirect_code: Some(302),
            ..Default::default()
        };
        assert!(
            update(&db, "a1", "old.example.org", &bad_code)
                .await
                .is_err()
        );
        let code = UpdateAppDomainRequest {
            redirect_code: Some(301),
            ..Default::default()
        };
        let r = ok(update(&db, "a1", "old.example.org", &code).await);
        assert_eq!(
            (r.redirect_code, r.redirect_to.as_deref()),
            (301, Some("shop.example.org"))
        );
    }

    #[tokio::test]
    async fn www_pair_adds_both_names_with_one_redirecting() {
        let db = db().await;
        let pair = WwwPairRequest {
            domain: "www.example.org".into(),
            canonical: CanonicalHost::Apex,
            primary: None,
        };
        let rows = ok(www_pair(&db, &ctx(), "a1", &pair).await);
        assert_eq!(rows[0].domain, "example.org");
        assert_eq!(rows[0].kind(), DomainKind::Primary);
        assert_eq!(rows[1].domain, "www.example.org");
        assert_eq!(rows[1].redirect_to.as_deref(), Some("example.org"));

        // Flip it: www becomes canonical, the apex redirects.
        let flip = WwwPairRequest {
            domain: "example.org".into(),
            canonical: CanonicalHost::Www,
            primary: None,
        };
        let rows = ok(www_pair(&db, &ctx(), "a1", &flip).await);
        assert_eq!(rows[0].domain, "www.example.org");
        assert_eq!(rows[0].kind(), DomainKind::Primary);
        assert_eq!(rows[1].domain, "example.org");
        assert_eq!(rows[1].redirect_to.as_deref(), Some("www.example.org"));
        assert_eq!(
            apps_domain(&db, "a1").await.as_deref(),
            Some("www.example.org")
        );
        let certs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM certificates WHERE domain IN ('example.org', 'www.example.org')",
        )
        .fetch_one(&db)
        .await
        .unwrap_or_default();
        assert_eq!(certs, 2, "both names get a certificate");
    }

    #[tokio::test]
    async fn set_primary_keeps_the_old_primary_as_an_alias() {
        let db = db().await;
        assert!(
            assign_automatic(&db, "a1", "web.example-apps.com")
                .await
                .unwrap_or(false)
        );
        ok(set_primary(&db, &ctx(), "a1", "shop.example.org").await);
        assert_eq!(
            kinds(&db, "a1").await,
            vec![
                ("shop.example.org".into(), "primary".into()),
                ("web.example-apps.com".into(), "alias".into())
            ]
        );
        let responses: Vec<bool> = list(&db, "a1")
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|d| ctx().response(d).automatic)
            .collect();
        assert_eq!(responses, vec![false, true]);
        assert_eq!(clear(&db, "a1").await.unwrap_or_default().len(), 2);
        assert_eq!(apps_domain(&db, "a1").await, None);
    }

    #[tokio::test]
    async fn domain_changes_bump_the_route_version() {
        let db = db().await;
        let version = || async {
            sqlx::query_scalar::<_, i64>("SELECT version FROM route_state WHERE id = 1")
                .fetch_one(&db)
                .await
                .unwrap_or_default()
        };
        let before = version().await;
        ok(add(&db, &ctx(), "a1", &req("shop.example.org", None, None)).await);
        let after_add = version().await;
        assert!(after_add > before);
        ok(remove(&db, "a1", "shop.example.org").await);
        assert!(version().await > after_add);
    }

    #[tokio::test]
    async fn apps_inserted_with_a_domain_seed_their_primary() {
        let db = db().await;
        sqlx::query(
            "INSERT INTO apps (id, name, domain, created_at, updated_at)
             VALUES ('a3', 'seeded', 'seeded.example.org', 'now', 'now')",
        )
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("insert: {e}"));
        assert_eq!(
            kinds(&db, "a3").await,
            vec![("seeded.example.org".into(), "primary".into())]
        );
        sqlx::query("DELETE FROM apps WHERE id = 'a3'")
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("delete: {e}"));
        assert!(kinds(&db, "a3").await.is_empty(), "domains go with the app");
    }
}
