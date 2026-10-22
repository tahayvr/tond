// template_apps.rs — what an app deployed from a template remembers about it
//
// A template deploy (handlers/templates.rs) records, next to template_id and
// template_version (migration 031):
//
//   - the SHA-256 of the compose file it wrote. A one-click upgrade replaces
//     the file with the catalog's newer one, which is only safe when the user
//     hasn't edited it; a different hash means they did.
//   - every template compose file the app has run, by hash, with its version.
//     A compose rollback restores an older file (deploy/compose.rs), and this
//     map tells which template version that file was, so "Update available"
//     shows again.
//   - the env values derived from the app's address: inputs whose value
//     mentions {app_url} or {app_host}, with that text and the host they
//     were expanded for. When the primary domain changes, `domain_env_changes`
//     lists the values to update — only those still equal to what Tond wrote,
//     so a hand-edited value is never overwritten.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tond_common::{App, TemplateContext, TemplateDomainEnvChange};

use crate::state::AppState;

/// SHA-256 (hex) of a compose file.
pub fn compose_sha256(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// Env values derived from the app's address (`apps.template_domain_env`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomainEnv {
    /// The host the values were expanded for.
    pub host: String,
    /// env key → text with `{app_url}` / `{app_host}`
    pub keys: BTreeMap<String, String>,
}

/// The template columns of an app.
#[derive(Debug, Clone, Default)]
pub struct Tracking {
    pub compose_sha256: Option<String>,
    pub releases: BTreeMap<String, String>,
    pub domain_env: Option<DomainEnv>,
}

pub async fn load(db: &SqlitePool, app_id: &str) -> sqlx::Result<Tracking> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT template_compose_sha256, template_releases, template_domain_env
         FROM apps WHERE id = ?",
    )
    .bind(app_id)
    .fetch_optional(db)
    .await?;
    let Some((sha, releases, domain_env)) = row else {
        return Ok(Tracking::default());
    };
    Ok(Tracking {
        compose_sha256: sha,
        releases: releases
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or_default(),
        domain_env: domain_env.and_then(|d| serde_json::from_str(&d).ok()),
    })
}

/// Record that the app now runs `compose` from template `version`, with
/// `domain_env` derived from its address.
pub async fn record<'e, E>(
    executor: E,
    app_id: &str,
    mut releases: BTreeMap<String, String>,
    compose: &str,
    version: &str,
    domain_env: &DomainEnv,
) -> sqlx::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let sha = compose_sha256(compose);
    releases.insert(sha.clone(), version.to_string());
    sqlx::query(
        "UPDATE apps SET template_version = ?, template_compose_sha256 = ?,
           template_releases = ?, template_domain_env = ?
         WHERE id = ?",
    )
    .bind(version)
    .bind(&sha)
    .bind(serde_json::to_string(&releases).unwrap_or_else(|_| "{}".into()))
    .bind(serde_json::to_string(domain_env).unwrap_or_else(|_| "{}".into()))
    .bind(app_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// After a compose rollback restored `content`: when it is a template file
/// the app ran before, the app is on that template version again.
pub async fn follow_restored_compose(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    app_id: &str,
    content: &str,
) -> sqlx::Result<()> {
    let releases: Option<Option<String>> = sqlx::query_scalar(
        "SELECT template_releases FROM apps WHERE id = ? AND template_id IS NOT NULL",
    )
    .bind(app_id)
    .fetch_optional(&mut **tx)
    .await?;
    let releases: BTreeMap<String, String> = releases
        .flatten()
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default();
    let sha = compose_sha256(content);
    if let Some(version) = releases.get(&sha) {
        sqlx::query(
            "UPDATE apps SET template_version = ?, template_compose_sha256 = ? WHERE id = ?",
        )
        .bind(version)
        .bind(&sha)
        .bind(app_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// The host the app answers on now: its primary domain, else the automatic
/// `<name>.<apps domain>`. None when it has neither.
pub async fn current_host(state: &AppState, app: &App) -> Option<String> {
    if let Some(d) = app.domain.as_deref().filter(|d| !d.is_empty()) {
        return Some(d.to_string());
    }
    let apps_domain = state.settings.read().await.apps_domain.clone();
    (!apps_domain.is_empty()).then(|| crate::domains::app_host(&app.name, &apps_domain))
}

/// Domain-derived env values that don't match `host`: the changes to make
/// (values still as Tond wrote them for the recorded host) and the keys left
/// alone because they were edited by hand.
pub fn domain_env_changes(
    recorded: &DomainEnv,
    env: &BTreeMap<String, String>,
    app_name: &str,
    host: &str,
) -> (Vec<TemplateDomainEnvChange>, Vec<String>) {
    let then = TemplateContext::new(app_name, &recorded.host);
    let now = TemplateContext::new(app_name, host);
    let mut changes = Vec::new();
    let mut edited = Vec::new();
    for (key, template) in &recorded.keys {
        let current = env.get(key).cloned().unwrap_or_default();
        let proposed = now.expand(template);
        if current == proposed {
            continue;
        }
        if current == then.expand(template) {
            changes.push(TemplateDomainEnvChange {
                key: key.clone(),
                current,
                proposed,
            });
        } else {
            edited.push(key.clone());
        }
    }
    (changes, edited)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded() -> DomainEnv {
        DomainEnv {
            host: "stats.apps.example.com".into(),
            keys: BTreeMap::from([
                ("BASE_URL".to_string(), "{app_url}".to_string()),
                ("HOSTNAME".to_string(), "{app_host}".to_string()),
                (
                    "CALLBACK".to_string(),
                    "{app_url}/auth/callback".to_string(),
                ),
            ]),
        }
    }

    #[test]
    fn domain_changes_cover_untouched_values_and_skip_edited_ones() {
        let env = BTreeMap::from([
            (
                "BASE_URL".to_string(),
                "https://stats.apps.example.com".to_string(),
            ),
            ("HOSTNAME".to_string(), "custom.internal".to_string()),
            (
                "CALLBACK".to_string(),
                "https://stats.apps.example.com/auth/callback".to_string(),
            ),
        ]);
        let (changes, edited) = domain_env_changes(&recorded(), &env, "stats", "stats.example.org");
        assert_eq!(
            changes,
            [
                TemplateDomainEnvChange {
                    key: "BASE_URL".into(),
                    current: "https://stats.apps.example.com".into(),
                    proposed: "https://stats.example.org".into(),
                },
                TemplateDomainEnvChange {
                    key: "CALLBACK".into(),
                    current: "https://stats.apps.example.com/auth/callback".into(),
                    proposed: "https://stats.example.org/auth/callback".into(),
                },
            ]
        );
        assert_eq!(edited, ["HOSTNAME"]);
        // Nothing to do while the domain is the recorded one.
        let (changes, _) = domain_env_changes(&recorded(), &env, "stats", "stats.apps.example.com");
        assert!(changes.is_empty());
    }

    #[test]
    fn compose_hashes_are_stable_hex() {
        assert_eq!(compose_sha256("a"), compose_sha256("a"));
        assert_ne!(compose_sha256("a"), compose_sha256("b"));
        assert_eq!(compose_sha256("").len(), 64);
    }
}
