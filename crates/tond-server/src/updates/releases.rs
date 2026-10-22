// updates/releases.rs — which Tond releases exist (the "update available" check)
//
// The published releases come from GitHub's releases API. The answer is
// cached in the `update_check` row and refreshed at most once a day, from the
// janitor's hourly pass (no loop of its own), or when an admin presses "Check
// now" (throttled). The request carries nothing about the server beyond the
// User-Agent `tond/<version>`; admins can turn the check off.
//
// The list comes from the release base (tond_common::release: TOND_RELEASE_BASE,
// <install dir>/RELEASE_BASE, else github.com/tahayvr/tond/releases): GitHub's
// releases API for a GitHub base, `<base>/releases.json` (same format) for a
// mirror. TOND_UPDATE_RELEASES_URL overrides the URL outright (the end-to-end
// test points it at a local file).

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use tond_common::release;

use super::run::Paths;
use super::version::{self, Version};

/// Automatic checks: at most this often.
pub const CHECK_INTERVAL_SECS: i64 = 24 * 3600;
/// "Check now": at most this often.
pub const MANUAL_THROTTLE_SECS: i64 = 60;
/// Release notes longer than this are cut (the dashboard links to GitHub).
const MAX_BODY_BYTES: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("tond/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
});

/// A published release, as cached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// `1.2.3` (no `v`).
    pub version: String,
    /// The git tag, e.g. `v1.2.3`.
    pub tag: String,
    pub name: String,
    /// Release notes (Markdown), possibly cut at MAX_BODY_BYTES.
    pub body: String,
    pub url: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("\n\n…");
    s
}

/// Published, stable releases from a GitHub releases API response, newest
/// version first. Drafts, pre-releases and tags that aren't versions are left out.
pub fn parse_github(json: &str) -> Result<Vec<Release>> {
    let raw: Vec<GithubRelease> =
        serde_json::from_str(json).context("unexpected response from the releases API")?;
    let mut releases: Vec<(Version, Release)> = raw
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let v = Version::parse(&r.tag_name)?;
            if v.is_prerelease() {
                return None;
            }
            Some((
                v.clone(),
                Release {
                    version: v.to_string(),
                    name: r
                        .name
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| r.tag_name.clone()),
                    tag: r.tag_name,
                    body: truncate(
                        r.body.unwrap_or_default().replace("\r\n", "\n"),
                        MAX_BODY_BYTES,
                    ),
                    url: r.html_url,
                    published_at: r.published_at,
                },
            ))
        })
        .collect();
    releases.sort_by(|a, b| b.0.cmp(&a.0));
    releases.dedup_by(|a, b| a.0 == b.0);
    Ok(releases.into_iter().map(|(_, r)| r).collect())
}

/// Releases newer than `current`, newest first (the changelog to show).
pub fn newer_than<'a>(releases: &'a [Release], current: &Version) -> Vec<&'a Release> {
    releases
        .iter()
        .filter(|r| Version::parse(&r.version).is_some_and(|v| &v > current))
        .collect()
}

/// Whether an automatic check is due.
pub fn check_due(enabled: bool, checked_at: Option<i64>, now: i64) -> bool {
    enabled && checked_at.is_none_or(|t| now - t >= CHECK_INTERVAL_SECS || t > now)
}

/// The release base of this install (the parent of bin/ is the install dir).
pub fn release_base() -> String {
    let install_dir = Paths::from_env()
        .bin_dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("/opt/tond"));
    release::release_base(&install_dir)
}

async fn fetch() -> Result<Vec<Release>> {
    let url = std::env::var("TOND_UPDATE_RELEASES_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| release::releases_api_url(&release_base()));
    let body = if let Some(path) = url.strip_prefix("file://") {
        tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("could not read {path}"))?
    } else {
        let response = CLIENT
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .context("could not reach GitHub")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("GitHub answered HTTP {}", status.as_u16());
        }
        response
            .text()
            .await
            .context("could not read GitHub's answer")?
    };
    parse_github(&body)
}

/// The cached check.
#[derive(Debug, Clone, Default)]
pub struct Cached {
    pub enabled: bool,
    pub checked_at: Option<i64>,
    pub error: Option<String>,
    pub releases: Vec<Release>,
}

pub async fn load(db: &SqlitePool) -> Result<Cached> {
    let row: Option<(i64, Option<i64>, Option<String>, String)> = sqlx::query_as(
        "SELECT enabled, checked_at, error, releases_json FROM update_check WHERE id = 1",
    )
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some((enabled, checked_at, error, json)) => Cached {
            enabled: enabled != 0,
            checked_at,
            error,
            releases: serde_json::from_str(&json).unwrap_or_default(),
        },
        None => Cached {
            enabled: true,
            ..Cached::default()
        },
    })
}

pub async fn set_enabled(db: &SqlitePool, enabled: bool) -> Result<()> {
    sqlx::query("UPDATE update_check SET enabled = ? WHERE id = 1")
        .bind(enabled as i64)
        .execute(db)
        .await?;
    Ok(())
}

/// Store the outcome of a check. A failure keeps the releases already known.
pub async fn store(db: &SqlitePool, now: i64, outcome: &Result<Vec<Release>>) -> Result<()> {
    match outcome {
        Ok(releases) => {
            sqlx::query(
                "UPDATE update_check SET checked_at = ?, error = NULL, releases_json = ? WHERE id = 1",
            )
            .bind(now)
            .bind(serde_json::to_string(releases)?)
            .execute(db)
            .await?;
        }
        Err(e) => {
            sqlx::query("UPDATE update_check SET checked_at = ?, error = ? WHERE id = 1")
                .bind(now)
                .bind(format!("{e:#}"))
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// Ask GitHub now and cache the answer.
pub async fn check_now(db: &SqlitePool) -> Result<Cached> {
    let now = chrono::Utc::now().timestamp();
    let outcome = fetch().await;
    if let Err(e) = &outcome {
        tracing::info!("Update check failed: {e:#}");
    }
    store(db, now, &outcome).await?;
    load(db).await
}

/// The janitor's hook: check when a day has passed since the last attempt.
pub async fn check_if_due(db: &SqlitePool) {
    let now = chrono::Utc::now().timestamp();
    let cached = match load(db).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Update check: could not read the cache: {e}");
            return;
        }
    };
    if !check_due(cached.enabled, cached.checked_at, now) {
        return;
    }
    match check_now(db).await {
        Ok(c) => {
            let current = version::current();
            if let Some(latest) = newer_than(&c.releases, &current).first() {
                tracing::info!("Tond {} is available (running {current})", latest.version);
            }
        }
        Err(e) => tracing::warn!("Update check: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r###"[
      {"tag_name":"v0.4.0","name":"Tond 0.4.0","body":"## Changes\r\n- faster","html_url":"https://github.com/tahayvr/tond/releases/tag/v0.4.0","published_at":"2026-09-20T10:00:00Z","draft":false,"prerelease":false},
      {"tag_name":"v0.5.0-rc.1","name":"rc","body":"","html_url":"u","published_at":null,"draft":false,"prerelease":true},
      {"tag_name":"v0.6.0","name":"draft","body":"","html_url":"u","published_at":null,"draft":true,"prerelease":false},
      {"tag_name":"nightly","name":"n","body":"","html_url":"u","published_at":null,"draft":false,"prerelease":false},
      {"tag_name":"v0.3.1","name":"","body":null,"html_url":"https://github.com/tahayvr/tond/releases/tag/v0.3.1","published_at":"2026-09-01T10:00:00Z","draft":false,"prerelease":false},
      {"tag_name":"v0.10.0","name":null,"body":"big","html_url":"x","published_at":"2026-09-25T10:00:00Z","draft":false,"prerelease":false}
    ]"###;

    #[test]
    fn keeps_published_stable_releases_newest_version_first() {
        let releases = parse_github(SAMPLE).unwrap_or_else(|e| panic!("{e}"));
        let versions: Vec<&str> = releases.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["0.10.0", "0.4.0", "0.3.1"]);
        assert_eq!(releases[1].body, "## Changes\n- faster");
        // A missing or empty name falls back to the tag.
        assert_eq!(releases[0].name, "v0.10.0");
        assert_eq!(releases[2].name, "v0.3.1");
        assert_eq!(releases[2].body, "");
    }

    #[test]
    fn a_garbage_answer_is_an_error() {
        assert!(parse_github("{\"message\":\"rate limited\"}").is_err());
    }

    #[test]
    fn changelog_lists_only_newer_releases() {
        let releases = parse_github(SAMPLE).unwrap_or_else(|e| panic!("{e}"));
        let current = Version::parse("0.4.0").unwrap_or_else(|| panic!());
        let newer: Vec<&str> = newer_than(&releases, &current)
            .iter()
            .map(|r| r.version.as_str())
            .collect();
        assert_eq!(newer, ["0.10.0"]);
        let latest = Version::parse("0.10.0").unwrap_or_else(|| panic!());
        assert!(newer_than(&releases, &latest).is_empty());
    }

    #[test]
    fn long_release_notes_are_cut_on_a_character_boundary() {
        let s = truncate("é".repeat(20), 7);
        assert!(s.starts_with("ééé"));
        assert!(s.ends_with('…'));
    }

    #[test]
    fn automatic_checks_run_at_most_daily_and_only_when_enabled() {
        let now = 1_000_000;
        assert!(check_due(true, None, now));
        assert!(!check_due(false, None, now));
        assert!(!check_due(true, Some(now - 3600), now));
        assert!(check_due(true, Some(now - CHECK_INTERVAL_SECS), now));
        // A clock that went backwards doesn't block checks forever.
        assert!(check_due(true, Some(now + 10_000), now));
    }

    #[tokio::test]
    async fn the_cache_round_trips_and_a_failure_keeps_known_releases() {
        let state = crate::state::AppState::for_tests().await;
        let releases = parse_github(SAMPLE).unwrap_or_else(|e| panic!("{e}"));
        store(&state.db, 100, &Ok(releases.clone()))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        store(&state.db, 200, &Err(anyhow::anyhow!("offline")))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let cached = load(&state.db).await.unwrap_or_else(|e| panic!("{e}"));
        assert!(cached.enabled);
        assert_eq!(cached.checked_at, Some(200));
        assert_eq!(cached.error.as_deref(), Some("offline"));
        assert_eq!(cached.releases, releases);

        set_enabled(&state.db, false)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !load(&state.db)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .enabled
        );
    }
}
