// commands/audit.rs — `tond audit` subcommands
//
//   tond audit verify         — walk the hash chain; exits non-zero if broken
//   tond audit log [filters]  — show recent entries

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::client::TondClient;
use crate::output::{Output, outln};

#[derive(Subcommand)]
pub enum AuditAction {
    /// Verify the audit log's hash chain (exit status 1 if it was tampered with)
    Verify,

    /// Show recent audit log entries, newest first
    Log {
        /// Only this actor (email, token name or "local-socket")
        #[arg(long)]
        actor: Option<String>,

        /// Only this action (a trailing * matches a prefix, e.g. "login*")
        #[arg(long)]
        action: Option<String>,

        /// How many entries (1-200)
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

pub async fn handle(client: &TondClient, out: &Output, action: AuditAction) -> Result<()> {
    match action {
        AuditAction::Verify => {
            let report = client.verify_audit_log().await?;
            out.json(&report)?;
            if report.ok {
                let range = match (report.first_seq, report.last_seq) {
                    (Some(a), Some(b)) => format!("entries #{a}–#{b}"),
                    _ => "no entries".to_string(),
                };
                outln!(out, "Audit log OK: {} checked ({range}).", report.checked);
                if report.checkpoint_seq > 0 {
                    outln!(
                        out,
                        "Entries up to #{} were removed by retention; the chain continues from that checkpoint.",
                        report.checkpoint_seq
                    );
                }
            } else {
                bail!(
                    "Audit log verification FAILED at entry #{}: {}",
                    report
                        .broken_at
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "?".into()),
                    report.problem.unwrap_or_default()
                );
            }
        }
        AuditAction::Log {
            actor,
            action,
            limit,
        } => {
            let mut query = vec![("limit", limit.to_string())];
            if let Some(a) = actor {
                query.push(("actor", a));
            }
            if let Some(a) = action {
                query.push(("action", a));
            }
            let page = client.list_audit_log(&query).await?;
            out.json(&page)?;
            for e in page.entries {
                let resource = match (&e.resource_type, &e.resource_name) {
                    (Some(t), Some(n)) => format!(" {t}:{n}"),
                    (Some(t), None) => format!(" {t}"),
                    _ => String::new(),
                };
                outln!(
                    out,
                    "#{:<6} {}  {:<24} {}{}{}{}",
                    e.seq,
                    e.created_at,
                    format!("{} ({})", e.actor_label, e.actor_kind),
                    e.action,
                    resource,
                    if e.outcome == "failure" {
                        " [failed]"
                    } else {
                        ""
                    },
                    e.detail.map(|d| format!(" — {d}")).unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}
