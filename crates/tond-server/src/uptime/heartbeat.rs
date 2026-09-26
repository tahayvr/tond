// uptime/heartbeat.rs — outbound heartbeat (dead-man's switch)
//
// A server can't report its own outage, so Tond pings an outside monitor
// (healthchecks.io, Uptime Kuma push, Better Stack heartbeat, …) on a
// schedule; when the pings stop, that service alerts. The ping is sent only
// while Tond is healthy — otherwise it is skipped (recorded with the reason)
// and the monitor alerts after its grace period.
//
// Healthy means all of:
//   - Docker answers (docker_host::health, at most a minute old);
//   - tond-proxy is connected over its control socket (it is running and
//     serving the routes tond-server gave it);
//   - the metrics collector — the server's core background loop — ran in the
//     last HEALTHY_LOOP_AGE (the runtime is not wedged);
//   - the database answers.
//
// Configuration lives in heartbeat_config (one row). The URL is a bearer
// secret: stored encrypted (secret_key::Purpose::HeartbeatUrl), write-only
// in the API (only its host is shown), and never logged — request errors are
// rendered without it. The schedule runs in the uptime loop (uptime/mod.rs)
// on the same minute grid as the uptime checks, so it adds no wakeups.

use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::metrics::docker_host;
use crate::runtime::loops;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;

pub const DEFAULT_INTERVAL_SECS: i64 = 300;
pub const MIN_INTERVAL_SECS: i64 = 60;
pub const MAX_INTERVAL_SECS: i64 = 86_400;
const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// The metrics loop ticks every 30 s; older than this means it is stuck.
const HEALTHY_LOOP_AGE: Duration = Duration::from_secs(120);

/// The heartbeat configuration and its last outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Decrypted URL; None when not configured.
    pub url: Option<String>,
    /// "GET" | "POST"
    pub method: String,
    pub interval_secs: i64,
    pub enabled: bool,
    pub last_ok_at: Option<i64>,
    pub last_failure_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_skipped_at: Option<i64>,
    pub last_skip_reason: Option<String>,
}

impl Config {
    /// Configured and switched on.
    pub fn active(&self) -> bool {
        self.enabled && self.url.is_some()
    }

    /// The URL's host, safe to show.
    pub fn url_host(&self) -> Option<String> {
        self.url
            .as_deref()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_string))
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    url_enc: String,
    method: String,
    interval_secs: i64,
    enabled: bool,
    last_ok_at: Option<i64>,
    last_failure_at: Option<i64>,
    last_error: Option<String>,
    last_skipped_at: Option<i64>,
    last_skip_reason: Option<String>,
}

pub async fn load(db: &SqlitePool) -> anyhow::Result<Config> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT url_enc, method, interval_secs, enabled, last_ok_at, last_failure_at, last_error,
                last_skipped_at, last_skip_reason
         FROM heartbeat_config WHERE id = 'heartbeat'",
    )
    .fetch_optional(db)
    .await?;
    let Some(r) = row else {
        return Ok(Config {
            method: "GET".into(),
            interval_secs: DEFAULT_INTERVAL_SECS,
            enabled: true,
            ..Default::default()
        });
    };
    let url = if r.url_enc.is_empty() {
        None
    } else {
        Some(secret_key::decrypt(Purpose::HeartbeatUrl, &r.url_enc)?)
    };
    Ok(Config {
        url,
        method: r.method,
        interval_secs: r.interval_secs,
        enabled: r.enabled,
        last_ok_at: r.last_ok_at,
        last_failure_at: r.last_failure_at,
        last_error: r.last_error,
        last_skipped_at: r.last_skipped_at,
        last_skip_reason: r.last_skip_reason,
    })
}

/// A heartbeat URL: http(s), with a host, no credentials in it.
pub fn validate_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| "Enter the full heartbeat URL, starting with https://".to_string())?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err("The heartbeat URL must start with https:// (or http://)".into());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("The heartbeat URL has no host".into());
    }
    Ok(())
}

/// Changes to the configuration. `url`: None keeps the stored URL,
/// Some("") clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Update {
    pub url: Option<String>,
    pub method: Option<String>,
    pub interval_secs: Option<i64>,
    pub enabled: Option<bool>,
}

pub async fn save(db: &SqlitePool, update: &Update) -> anyhow::Result<()> {
    let mut tx = crate::db::begin_write(db).await?;
    if let Some(url) = &update.url {
        let url = url.trim();
        let enc = if url.is_empty() {
            String::new()
        } else {
            secret_key::encrypt(Purpose::HeartbeatUrl, url)?
        };
        // A new URL starts a new history.
        sqlx::query(
            "UPDATE heartbeat_config SET url_enc = ?, last_ok_at = NULL, last_failure_at = NULL,
                    last_error = NULL, last_skipped_at = NULL, last_skip_reason = NULL
             WHERE id = 'heartbeat'",
        )
        .bind(enc)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(method) = &update.method {
        sqlx::query("UPDATE heartbeat_config SET method = ? WHERE id = 'heartbeat'")
            .bind(method)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(interval) = update.interval_secs {
        sqlx::query("UPDATE heartbeat_config SET interval_secs = ? WHERE id = 'heartbeat'")
            .bind(interval)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(enabled) = update.enabled {
        sqlx::query("UPDATE heartbeat_config SET enabled = ? WHERE id = 'heartbeat'")
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// What one heartbeat did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// The monitor answered 2xx.
    Sent { status: u16 },
    /// The request failed or the monitor answered with an error.
    Failed { error: String },
    /// Tond is unhealthy, so no ping was sent (the monitor will alert).
    Skipped { reason: String },
}

/// Decide health from its parts (pure, for tests).
pub fn judge_health(
    docker: Result<(), String>,
    proxy_connected: bool,
    metrics_ran_ago: Option<Duration>,
) -> Result<(), String> {
    docker.map_err(|e| format!("Docker is not healthy: {e}"))?;
    if !proxy_connected {
        return Err("tond-proxy is not connected to tond-server".into());
    }
    match metrics_ran_ago {
        Some(age) if age <= HEALTHY_LOOP_AGE => Ok(()),
        Some(age) => Err(format!(
            "tond-server's background work is stuck (last ran {} s ago)",
            age.as_secs()
        )),
        None => Err("tond-server's background work has not run yet".into()),
    }
}

/// Whether Tond is healthy enough to send a heartbeat.
pub async fn healthy(state: &AppState) -> Result<(), String> {
    let metrics_age = loops::snapshot()
        .into_iter()
        .find(|l| l.name == "metrics")
        .and_then(|l| l.last_run_at)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .and_then(|t| {
            (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
                .to_std()
                .ok()
        });
    judge_health(
        docker_host::docker_reachable(&state.docker).await,
        crate::proxy_link::is_connected(),
        metrics_age,
    )?;
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.db)
        .await
        .map(|_| ())
        .map_err(|e| format!("the database is not answering: {e}"))
}

/// Send one ping (no health check).
pub async fn send(url: &str, method: &str) -> Outcome {
    let client = match reqwest::Client::builder()
        .timeout(PING_TIMEOUT)
        .user_agent(concat!("Tond-Heartbeat/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Outcome::Failed {
                error: e.without_url().to_string(),
            };
        }
    };
    let request = if method.eq_ignore_ascii_case("POST") {
        client.post(url).body("")
    } else {
        client.get(url)
    };
    match request.send().await {
        Ok(resp) if resp.status().is_success() => Outcome::Sent {
            status: resp.status().as_u16(),
        },
        Ok(resp) => Outcome::Failed {
            error: format!(
                "the monitor answered HTTP {}. Check that the URL is the one your monitor gave you.",
                resp.status().as_u16()
            ),
        },
        Err(e) if e.is_timeout() => Outcome::Failed {
            error: format!(
                "the monitor did not answer within {} s",
                PING_TIMEOUT.as_secs()
            ),
        },
        Err(e) => Outcome::Failed {
            error: describe_request_error(&e),
        },
    }
}

fn describe_request_error(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    // The innermost cause says the most ("connection refused", "dns error");
    // the URL is never included.
    let mut source: Option<&dyn std::error::Error> = e.source();
    let mut last = None;
    while let Some(s) = source {
        last = Some(s.to_string());
        source = s.source();
    }
    match last {
        Some(cause) => format!("couldn't reach the monitor: {cause}"),
        None => "couldn't reach the monitor".to_string(),
    }
}

/// Health check, then ping, then record the outcome.
pub async fn beat(state: &AppState, cfg: &Config) -> Outcome {
    let Some(url) = cfg.url.as_deref() else {
        return Outcome::Skipped {
            reason: "no heartbeat URL is configured".into(),
        };
    };
    let outcome = match healthy(state).await {
        Ok(()) => send(url, &cfg.method).await,
        Err(reason) => Outcome::Skipped { reason },
    };
    if let Err(e) = record(&state.db, &outcome, chrono::Utc::now().timestamp()).await {
        tracing::warn!("heartbeat: cannot record the outcome: {e}");
    }
    outcome
}

/// Store the last outcome.
pub async fn record(db: &SqlitePool, outcome: &Outcome, now: i64) -> sqlx::Result<()> {
    match outcome {
        Outcome::Sent { .. } => {
            sqlx::query("UPDATE heartbeat_config SET last_ok_at = ? WHERE id = 'heartbeat'")
                .bind(now)
                .execute(db)
                .await?;
        }
        Outcome::Failed { error } => {
            sqlx::query(
                "UPDATE heartbeat_config SET last_failure_at = ?, last_error = ? WHERE id = 'heartbeat'",
            )
            .bind(now)
            .bind(error)
            .execute(db)
            .await?;
        }
        Outcome::Skipped { reason } => {
            sqlx::query(
                "UPDATE heartbeat_config SET last_skipped_at = ?, last_skip_reason = ? WHERE id = 'heartbeat'",
            )
            .bind(now)
            .bind(reason)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        db
    }

    #[test]
    fn urls_are_validated() {
        assert!(validate_url("https://hc-ping.com/abc").is_ok());
        assert!(validate_url("http://127.0.0.1:9000/ping").is_ok());
        assert!(validate_url("hc-ping.com/abc").is_err());
        assert!(validate_url("ftp://example.com/x").is_err());
    }

    #[test]
    fn health_needs_docker_the_proxy_and_a_live_collector() {
        let fresh = Some(Duration::from_secs(20));
        assert!(judge_health(Ok(()), true, fresh).is_ok());
        assert!(
            judge_health(Err("down".into()), true, fresh)
                .unwrap_err()
                .contains("Docker")
        );
        assert!(
            judge_health(Ok(()), false, fresh)
                .unwrap_err()
                .contains("tond-proxy")
        );
        assert!(
            judge_health(Ok(()), true, Some(Duration::from_secs(600)))
                .unwrap_err()
                .contains("stuck")
        );
        assert!(judge_health(Ok(()), true, None).is_err());
    }

    #[tokio::test]
    async fn the_url_is_stored_encrypted_and_outcomes_are_recorded() {
        let db = db().await;
        let cfg = load(&db).await.unwrap_or_default();
        assert!(!cfg.active());
        assert_eq!((cfg.interval_secs, cfg.method.as_str()), (300, "GET"));

        save(
            &db,
            &Update {
                url: Some("https://hc-ping.com/secret-uuid".into()),
                method: Some("POST".into()),
                interval_secs: Some(120),
                enabled: Some(true),
            },
        )
        .await
        .unwrap_or_else(|e| panic!("save: {e}"));
        let stored: String = sqlx::query_scalar("SELECT url_enc FROM heartbeat_config")
            .fetch_one(&db)
            .await
            .unwrap_or_default();
        assert!(!stored.contains("secret-uuid"), "encrypted at rest");
        let cfg = load(&db).await.unwrap_or_default();
        assert!(cfg.active());
        assert_eq!(cfg.url_host().as_deref(), Some("hc-ping.com"));
        assert_eq!((cfg.interval_secs, cfg.method.as_str()), (120, "POST"));

        record(&db, &Outcome::Sent { status: 200 }, 1000)
            .await
            .unwrap_or_else(|e| panic!("record: {e}"));
        record(
            &db,
            &Outcome::Skipped {
                reason: "Docker is not healthy".into(),
            },
            1300,
        )
        .await
        .unwrap_or_else(|e| panic!("record: {e}"));
        let cfg = load(&db).await.unwrap_or_default();
        assert_eq!(cfg.last_ok_at, Some(1000));
        assert_eq!(cfg.last_skipped_at, Some(1300));

        // Clearing the URL clears the history too; other fields stay.
        save(
            &db,
            &Update {
                url: Some(String::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("save: {e}"));
        let cfg = load(&db).await.unwrap_or_default();
        assert_eq!(
            (cfg.url, cfg.last_ok_at, cfg.interval_secs),
            (None, None, 120)
        );
    }

    #[tokio::test]
    async fn pings_report_success_errors_and_never_the_url() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("bind: {e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        tokio::spawn(async move {
            for status in ["200 OK", "404 Not Found"] {
                if let Ok((mut s, _)) = listener.accept().await {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                }
            }
        });
        let url = format!("http://127.0.0.1:{port}/ping/secret-uuid");
        assert_eq!(send(&url, "POST").await, Outcome::Sent { status: 200 });
        match send(&url, "GET").await {
            Outcome::Failed { error } => {
                assert!(error.contains("404"));
                assert!(!error.contains("secret-uuid"));
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        // Nothing listens any more.
        match send(&url, "GET").await {
            Outcome::Failed { error } => assert!(!error.contains("secret-uuid"), "{error}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }
}
