// deploy/template_setup.rs — pre-seed a template app's admin account
//
// Many self-hosted apps hand the admin account to whoever opens them first
// (an install page, "the first sign-up becomes admin") or ship a fixed
// default login. A template that can avoid that declares `[[post_deploy]]`
// commands (tond_common::TemplatePostDeploy): Tond runs them with
// `docker exec` in the named service's container during the app's first
// successful deploy, after every service is ready and BEFORE the release
// step routes the app, so nobody on the internet can reach it first.
//
//   apps.admin_setup  'pending' (set by the template deploy) → 'done', or
//                     'failed' with admin_setup_error. 'open' marks templates
//                     where the first visitor creates the admin
//                     (first_visitor_admin); the dashboard warns on 'open'
//                     and 'failed' until the warning is dismissed.
//
// Each command gets the template inputs it lists in `env` (from the app's
// decrypted environment) as env vars, never on its command line. It is
// retried every few seconds until it exits 0 or its `timeout_secs` budget
// runs out (the app may still be running migrations). Output is bounded and
// the input values are redacted from it (db_exec::run), and the deploy log
// scrubs the app's secrets too. A failure never fails the deploy: the app
// is released and its page warns that the admin account is unclaimed.
//
// Each step that succeeded is remembered by fingerprint
// (apps.template_hooks_done). A template upgrade (handlers/templates.rs)
// whose new version adds steps sets admin_setup to 'upgrade' when Tond
// seeded the admin: the next deploy runs only the steps not done yet, and
// the app stays routed. When the admin came from a first visitor (or there
// was none), the new steps are recorded as done without running, so an
// account that exists is never seeded a second time.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use tond_common::{AdminSetup, App, TemplatePostDeploy, builtin_template};

use super::core::log_deploy;
use crate::state::AppState;
use crate::{compose, db_exec, secrets, template_apps};

/// Pause between attempts of a failing command.
const RETRY: Duration = Duration::from_secs(3);
/// Longest single attempt.
const ATTEMPT: Duration = Duration::from_secs(60);

/// The `docker exec` command for one post-deploy step: its argv and user, and
/// the listed inputs as env vars. An input missing from the app's environment
/// (deleted on the Environment tab) is an error rather than an empty value.
pub fn hook_cmd(hook: &TemplatePostDeploy, env: &BTreeMap<String, String>) -> Result<db_exec::Cmd> {
    let mut cmd = db_exec::Cmd {
        argv: hook.command.clone(),
        env: Vec::new(),
        user: hook.user.clone(),
    };
    for key in &hook.env {
        let value = env
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("the app has no {key} environment variable any more"))?;
        cmd = cmd.env(key, value);
    }
    Ok(cmd)
}

/// A step's identity: SHA-256 (hex, like `template_apps::compose_sha256`) of
/// what it runs. Its description and timeout don't count, so rewording a
/// step never makes it run again.
pub fn hook_fingerprint(hook: &TemplatePostDeploy) -> String {
    let canonical = format!(
        "{}\0{}\0{}\0{}",
        hook.service,
        hook.user.as_deref().unwrap_or(""),
        hook.command.join("\u{1f}"),
        hook.env.join(",")
    );
    template_apps::compose_sha256(&canonical)
}

/// Fingerprints of the steps an app has run (`apps.template_hooks_done`).
pub async fn hooks_done<'e, E>(executor: E, app_id: &str) -> sqlx::Result<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let raw: Option<String> =
        sqlx::query_scalar("SELECT template_hooks_done FROM apps WHERE id = ?")
            .bind(app_id)
            .fetch_optional(executor)
            .await?;
    Ok(raw
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default())
}

/// Store the fingerprints of the steps an app has run.
pub async fn save_hooks_done<'e, E>(executor: E, app_id: &str, done: &[String]) -> sqlx::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("UPDATE apps SET template_hooks_done = ? WHERE id = ?")
        .bind(serde_json::to_string(done).unwrap_or_else(|_| "[]".into()))
        .bind(app_id)
        .execute(executor)
        .await?;
    Ok(())
}

/// The steps of `hooks` not in `done`, in order.
pub fn outstanding<'a>(
    hooks: &'a [TemplatePostDeploy],
    done: &[String],
) -> Vec<&'a TemplatePostDeploy> {
    hooks
        .iter()
        .filter(|h| !done.contains(&hook_fingerprint(h)))
        .collect()
}

/// What a template upgrade does about the new version's steps, given the
/// app's admin_setup: `Some(status)` to run the outstanding ones on the next
/// deploy, `None` to record them as done without running them (the admin
/// came from a first visitor, or the app had none: it may exist already).
pub fn upgrade_admin_setup(current: Option<&str>) -> Option<&'static str> {
    match current {
        // Not yet seeded: the first deploy still withholds the routes.
        Some(AdminSetup::PENDING) => Some(AdminSetup::PENDING),
        Some(AdminSetup::DONE | AdminSetup::FAILED | AdminSetup::UPGRADE) => {
            Some(AdminSetup::UPGRADE)
        }
        _ => None,
    }
}

/// The input values a step receives, to redact from its output.
fn secrets_of<'a>(hook: &TemplatePostDeploy, env: &'a BTreeMap<String, String>) -> Vec<&'a str> {
    hook.env
        .iter()
        .filter_map(|k| env.get(k).map(String::as_str))
        .collect()
}

async fn set_status(state: &AppState, app_id: &str, status: &str, error: Option<&str>) {
    let _ = sqlx::query(
        "UPDATE apps SET admin_setup = ?, admin_setup_error = ?, updated_at = datetime('now')
         WHERE id = ?",
    )
    .bind(status)
    .bind(error)
    .bind(app_id)
    .execute(&state.db)
    .await;
}

/// Run the app's pending post-deploy steps (if its admin_setup is
/// 'pending'). Never fails: the outcome is recorded on the app and logged.
pub(super) async fn run_pending(state: &AppState, app: &App, project: &str, trigger: &str) {
    let status: Option<String> = sqlx::query_scalar("SELECT admin_setup FROM apps WHERE id = ?")
        .bind(&app.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .flatten();
    if !matches!(
        status.as_deref(),
        Some(AdminSetup::PENDING | AdminSetup::UPGRADE)
    ) {
        return;
    }
    let log = |level: &'static str, message: String| async move {
        let _ = log_deploy(state, &app.id, level, trigger, &message).await;
    };
    let hooks = match app.template_id.as_deref().map(builtin_template) {
        Some(Ok(Some(t))) => t.manifest.post_deploy,
        _ => Vec::new(),
    };
    let mut done = hooks_done(&state.db, &app.id).await.unwrap_or_default();
    let todo: Vec<TemplatePostDeploy> = outstanding(&hooks, &done).into_iter().cloned().collect();
    if todo.is_empty() {
        set_status(state, &app.id, AdminSetup::DONE, None).await;
        return;
    }
    let env = match secrets::effective_app_env(&state.db, app).await {
        Ok(env) => env,
        Err(e) => {
            let error = format!("couldn't read the app's environment: {e:#}");
            log("warn", format!("Admin account setup failed: {error}")).await;
            set_status(state, &app.id, AdminSetup::FAILED, Some(&error)).await;
            return;
        }
    };
    for hook in &todo {
        log("info", format!("[{}] {}…", hook.service, hook.description)).await;
        match run_hook(state, project, hook, &env).await {
            Ok(()) => {
                done.push(hook_fingerprint(hook));
                let _ = save_hooks_done(&state.db, &app.id, &done).await;
                log(
                    "info",
                    format!("[{}] {}: done", hook.service, hook.description),
                )
                .await
            }
            Err(e) => {
                let error = format!("{}: {e:#}", hook.description);
                log(
                    "warn",
                    format!(
                        "[{}] {error}. The app is released anyway: open it now and secure the admin account.",
                        hook.service
                    ),
                )
                .await;
                set_status(state, &app.id, AdminSetup::FAILED, Some(&error)).await;
                return;
            }
        }
    }
    set_status(state, &app.id, AdminSetup::DONE, None).await;
}

/// One step, retried until it succeeds or its budget runs out.
async fn run_hook(
    state: &AppState,
    project: &str,
    hook: &TemplatePostDeploy,
    env: &BTreeMap<String, String>,
) -> Result<()> {
    let cmd = hook_cmd(hook, env)?;
    let redact = secrets_of(hook, env);
    let deadline = Instant::now() + Duration::from_secs(u64::from(hook.timeout_secs()));
    loop {
        let attempt = match compose::service_containers(&state.docker, project, &hook.service)
            .await
            .first()
        {
            Some(container) => {
                let budget = deadline
                    .saturating_duration_since(Instant::now())
                    .max(RETRY);
                db_exec::run(&state.docker, container, &cmd, ATTEMPT.min(budget), &redact)
                    .await
                    .map(|_| ())
            }
            None => Err(anyhow::anyhow!("service {} has no container", hook.service)),
        };
        match attempt {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() + RETRY >= deadline => {
                return Err(e.context(format!("gave up after {} seconds", hook.timeout_secs())));
            }
            Err(e) => {
                tracing::debug!("post-deploy step for {project} not done yet: {e:#}");
                tokio::time::sleep(RETRY).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_ignore_wording_and_find_only_new_steps() {
        let a = hook();
        let mut reworded = hook();
        reworded.description = "Something else".into();
        reworded.timeout_secs = Some(30);
        assert_eq!(hook_fingerprint(&a), hook_fingerprint(&reworded));
        assert_eq!(hook_fingerprint(&a).len(), 64);
        let mut other = hook();
        other.command.push("--force".into());
        assert_ne!(hook_fingerprint(&a), hook_fingerprint(&other));

        let hooks = vec![a.clone(), other.clone()];
        let done = vec![hook_fingerprint(&a)];
        assert_eq!(outstanding(&hooks, &done), vec![&other]);
        assert!(outstanding(&hooks, &[hook_fingerprint(&a), hook_fingerprint(&other)]).is_empty());
    }

    #[test]
    fn upgrades_run_new_steps_only_where_tond_seeded_the_admin() {
        assert_eq!(upgrade_admin_setup(Some("done")), Some("upgrade"));
        assert_eq!(upgrade_admin_setup(Some("failed")), Some("upgrade"));
        assert_eq!(upgrade_admin_setup(Some("upgrade")), Some("upgrade"));
        assert_eq!(upgrade_admin_setup(Some("pending")), Some("pending"));
        for first_visitor in [Some("open"), Some("dismissed"), None] {
            assert_eq!(
                upgrade_admin_setup(first_visitor),
                None,
                "{first_visitor:?}: the admin may exist already"
            );
        }
    }

    fn hook() -> TemplatePostDeploy {
        TemplatePostDeploy {
            service: "web".into(),
            user: Some("git".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "app admin create --password \"$ADMIN_PASSWORD\"".into(),
            ],
            env: vec!["ADMIN_PASSWORD".into(), "ADMIN_EMAIL".into()],
            description: "Create the administrator account".into(),
            timeout_secs: None,
        }
    }

    fn env() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("ADMIN_PASSWORD".to_string(), "s3cret-value".to_string()),
            ("ADMIN_EMAIL".to_string(), "me@example.com".to_string()),
            ("OTHER".to_string(), "not passed".to_string()),
        ])
    }

    #[test]
    fn hook_commands_pass_inputs_as_env_never_in_argv() {
        let cmd = hook_cmd(&hook(), &env()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cmd.argv, hook().command, "argv is used as declared");
        assert!(
            cmd.argv.iter().all(|a| !a.contains("s3cret-value")),
            "secrets never appear on the command line"
        );
        assert_eq!(
            cmd.env,
            vec![
                "ADMIN_PASSWORD=s3cret-value".to_string(),
                "ADMIN_EMAIL=me@example.com".to_string()
            ],
            "only the listed inputs are passed"
        );
        assert_eq!(cmd.user.as_deref(), Some("git"));
        assert_eq!(
            secrets_of(&hook(), &env()),
            ["s3cret-value", "me@example.com"]
        );
    }

    #[test]
    fn a_missing_input_is_an_error_not_an_empty_value() {
        let mut env = env();
        env.remove("ADMIN_PASSWORD");
        let err = hook_cmd(&hook(), &env)
            .err()
            .unwrap_or_else(|| panic!("a missing input must fail"));
        assert!(err.to_string().contains("ADMIN_PASSWORD"), "{err}");
    }

    #[test]
    fn every_shipped_hook_builds_from_its_template_inputs() {
        for t in tond_common::builtin_templates().unwrap_or_else(|e| panic!("{e}")) {
            let env: BTreeMap<String, String> = t
                .manifest
                .inputs
                .iter()
                .map(|i| (i.key.clone(), "value".to_string()))
                .collect();
            for hook in &t.manifest.post_deploy {
                let cmd = hook_cmd(hook, &env).unwrap_or_else(|e| panic!("{}: {e}", t.manifest.id));
                assert_eq!(cmd.env.len(), hook.env.len(), "{}", t.manifest.id);
            }
        }
    }
}
