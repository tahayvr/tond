// handlers/audit_shipping.rs — where the audit log is copied to (admin only)
//
//   get_shipping()     GET  /api/audit-log/shipping       — destinations + delivery status
//   update_shipping()  PUT  /api/audit-log/shipping       — change syslog and/or webhook
//   test_shipping()    POST /api/audit-log/shipping/test  — send a test entry now
//
// Delivery itself is audit_ship.rs. Re-pointing a destination (other host,
// port, TLS or URL) starts its copy over from the oldest retained entry.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use tond_common::*;

use crate::audit_log::{AuditEntry, record};
use crate::audit_ship::{self, SYSLOG, WEBHOOK};
use crate::auth::{Authz, RateLimit, scope};
use crate::error::AppError;
use crate::secret_key::{self, Purpose};
use crate::state::AppState;

/// Shortest accepted webhook signing secret.
pub const MIN_SECRET_LEN: usize = 16;

async fn delivery_status(
    db: &sqlx::SqlitePool,
    destination: &str,
) -> Result<DeliveryStatus, AppError> {
    let s = audit_ship::load_state(db, destination)
        .await
        .map_err(|e| TondError::Internal(e.to_string()))?;
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE seq > ?")
        .bind(s.shipped_seq)
        .fetch_one(db)
        .await?;
    Ok(DeliveryStatus {
        shipped_seq: s.shipped_seq,
        pending,
        failures: s.failures,
        last_attempt_at: s.last_attempt_at,
        last_success_at: s.last_success_at,
        last_error: s.last_error,
        next_attempt_at: s.next_attempt_at,
        dropped: s.dropped,
    })
}

async fn shipping_response(state: &AppState) -> Result<AuditShippingResponse, AppError> {
    let row = audit_ship::load_row(&state.db)
        .await
        .map_err(|e| TondError::Internal(e.to_string()))?;
    let latest_seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM audit_log")
        .fetch_one(&state.db)
        .await?;
    Ok(AuditShippingResponse {
        syslog: SyslogDestination {
            enabled: row.syslog_enabled,
            host: row.syslog_host.clone(),
            port: u16::try_from(row.syslog_port).unwrap_or(6514),
            tls: row.syslog_tls,
            status: delivery_status(&state.db, SYSLOG).await?,
        },
        webhook: WebhookDestination {
            enabled: row.webhook_enabled,
            url: row.webhook_url.clone(),
            has_secret: !row.webhook_secret.is_empty(),
            status: delivery_status(&state.db, WEBHOOK).await?,
        },
        latest_seq,
    })
}

pub async fn get_shipping(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(shipping_response(&state).await?)),
    ))
}

pub async fn update_shipping(
    _rate: RateLimit,
    auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<UpdateAuditShippingRequest>,
) -> Result<impl IntoResponse, AppError> {
    let current = audit_ship::load_row(&state.db)
        .await
        .map_err(|e| TondError::Internal(e.to_string()))?;
    let mut changes = Vec::new();

    if let Some(syslog) = &req.syslog {
        let host = syslog.host.trim().to_string();
        if (syslog.enabled || !host.is_empty())
            && let Some(problem) = audit_ship::syslog_problem(&host, syslog.port)
        {
            return Err(TondError::InvalidInput(problem).into());
        }
        let repointed = host != current.syslog_host
            || i64::from(syslog.port) != current.syslog_port
            || syslog.tls != current.syslog_tls;
        sqlx::query(
            "UPDATE audit_shipping SET syslog_enabled = ?, syslog_host = ?, syslog_port = ?,
                                       syslog_tls = ?, updated_at = datetime('now')
             WHERE id = 'default'",
        )
        .bind(syslog.enabled)
        .bind(&host)
        .bind(i64::from(syslog.port))
        .bind(syslog.tls)
        .execute(&state.db)
        .await?;
        if repointed {
            audit_ship::reset_state(&state.db, SYSLOG)
                .await
                .map_err(|e| TondError::Internal(e.to_string()))?;
        }
        changes.push(format!(
            "syslog={}{}",
            if syslog.enabled { "on" } else { "off" },
            if host.is_empty() {
                String::new()
            } else {
                format!(" syslog_target={host}:{}", syslog.port)
            }
        ));
    }

    if let Some(webhook) = &req.webhook {
        let url = webhook.url.trim().to_string();
        if (webhook.enabled || !url.is_empty())
            && let Some(problem) = audit_ship::webhook_problem(&url)
        {
            return Err(TondError::InvalidInput(problem).into());
        }
        let new_secret = webhook
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(secret) = new_secret
            && secret.len() < MIN_SECRET_LEN
        {
            return Err(TondError::InvalidInput(format!(
                "The signing secret must be at least {MIN_SECRET_LEN} characters."
            ))
            .into());
        }
        if webhook.enabled && new_secret.is_none() && current.webhook_secret.is_empty() {
            return Err(TondError::InvalidInput(
                "Set a signing secret so the receiver can check each delivery came from this server."
                    .into(),
            )
            .into());
        }
        let sealed = match new_secret {
            Some(secret) => secret_key::encrypt(Purpose::AuditWebhookSecret, secret)
                .map_err(|e| TondError::Internal(e.to_string()))?,
            None => current.webhook_secret.clone(),
        };
        sqlx::query(
            "UPDATE audit_shipping SET webhook_enabled = ?, webhook_url = ?, webhook_secret = ?,
                                       updated_at = datetime('now')
             WHERE id = 'default'",
        )
        .bind(webhook.enabled)
        .bind(&url)
        .bind(&sealed)
        .execute(&state.db)
        .await?;
        if url != current.webhook_url {
            audit_ship::reset_state(&state.db, WEBHOOK)
                .await
                .map_err(|e| TondError::Internal(e.to_string()))?;
        }
        changes.push(format!(
            "webhook={}{}{}",
            if webhook.enabled { "on" } else { "off" },
            reqwest::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(|h| format!(" webhook_host={h}")))
                .unwrap_or_default(),
            if new_secret.is_some() {
                " secret=changed"
            } else {
                ""
            }
        ));
    }

    if !changes.is_empty() {
        record(
            &state.db,
            &auth.audit_actor(),
            AuditEntry::new("audit_shipping_update")
                .resource("settings", "audit_shipping", "audit shipping")
                .detail(changes.join(" ")),
        )
        .await;
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::ok(shipping_response(&state).await?)),
    ))
}

pub async fn test_shipping(
    _rate: RateLimit,
    _auth: Authz<scope::SettingsWrite>,
    State(state): State<AppState>,
    Json(req): Json<AuditShippingTestRequest>,
) -> Result<impl IntoResponse, AppError> {
    if req.destination != SYSLOG && req.destination != WEBHOOK {
        return Err(
            TondError::InvalidInput(format!("Unknown destination '{}'", req.destination)).into(),
        );
    }
    let result = match audit_ship::send_test(&state, &req.destination).await {
        Ok(()) => AuditShippingTestResult {
            ok: true,
            message: match req.destination.as_str() {
                SYSLOG => {
                    "Test entry sent. Look for action audit_shipping_test on the syslog server."
                        .into()
                }
                _ => "The webhook accepted the test entry.".into(),
            },
        },
        Err(e) => AuditShippingTestResult {
            ok: false,
            message: format!("{e:#}"),
        },
    };
    Ok((StatusCode::OK, Json(ApiResponse::ok(result))))
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::response::IntoResponse;

    use super::*;
    use crate::auth::Principal;

    fn admin() -> Authz<scope::SettingsWrite> {
        Authz::unchecked(Principal::test_user(Role::Admin))
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    #[tokio::test]
    async fn webhook_needs_https_and_a_secret_which_is_stored_encrypted() {
        let state = AppState::for_tests().await;
        let enable = |url: &str, secret: Option<&str>| UpdateAuditShippingRequest {
            syslog: None,
            webhook: Some(UpdateWebhookDestination {
                enabled: true,
                url: url.into(),
                secret: secret.map(str::to_string),
            }),
        };

        let plain = update_shipping(
            RateLimit,
            admin(),
            State(state.clone()),
            Json(enable(
                "http://logs.example.com/x",
                Some("0123456789abcdef"),
            )),
        )
        .await;
        assert!(plain.is_err(), "plain HTTP to another host is refused");

        let no_secret = update_shipping(
            RateLimit,
            admin(),
            State(state.clone()),
            Json(enable("https://logs.example.com/x", None)),
        )
        .await;
        assert!(no_secret.is_err(), "a signing secret is required");

        let ok = update_shipping(
            RateLimit,
            admin(),
            State(state.clone()),
            Json(enable(
                "https://logs.example.com/x",
                Some("0123456789abcdef"),
            )),
        )
        .await
        .unwrap_or_else(|_| panic!("valid config"))
        .into_response();
        let body = json(ok).await;
        assert_eq!(body["data"]["webhook"]["enabled"], true);
        assert_eq!(body["data"]["webhook"]["has_secret"], true);
        assert!(
            !body.to_string().contains("0123456789abcdef"),
            "the secret is never returned"
        );

        let stored: String =
            sqlx::query_scalar("SELECT webhook_secret FROM audit_shipping WHERE id = 'default'")
                .fetch_one(&state.db)
                .await
                .unwrap_or_default();
        assert!(stored.starts_with("v1."), "encrypted at rest");
        assert_eq!(
            secret_key::decrypt(Purpose::AuditWebhookSecret, &stored).ok(),
            Some("0123456789abcdef".to_string())
        );

        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'audit_shipping_update'
               AND detail LIKE 'webhook=on webhook_host=logs.example.com secret=changed'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or_default();
        assert_eq!(audited, 1);
    }

    #[tokio::test]
    async fn repointing_syslog_starts_the_copy_over() {
        let state = AppState::for_tests().await;
        sqlx::query("INSERT INTO audit_ship_state (destination, shipped_seq) VALUES ('syslog', 9)")
            .execute(&state.db)
            .await
            .unwrap_or_else(|e| panic!("seed: {e}"));
        let set = |host: &str| UpdateAuditShippingRequest {
            syslog: Some(UpdateSyslogDestination {
                enabled: true,
                host: host.into(),
                port: 6514,
                tls: true,
            }),
            webhook: None,
        };
        let response = update_shipping(
            RateLimit,
            admin(),
            State(state.clone()),
            Json(set("logs.example.com")),
        )
        .await
        .unwrap_or_else(|_| panic!("valid config"))
        .into_response();
        let body = json(response).await;
        assert_eq!(body["data"]["syslog"]["status"]["shipped_seq"], 0);
        assert_eq!(body["data"]["syslog"]["host"], "logs.example.com");

        let bad = update_shipping(RateLimit, admin(), State(state), Json(set(""))).await;
        assert!(bad.is_err(), "enabling needs a host");
    }
}
