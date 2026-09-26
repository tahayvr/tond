// deploy/failure.rs — turn a failed deployment into a likely cause
//
// classify() looks at what failed (phase, error text, the last deploy log
// lines, and for start/health failures what the container did) and returns a
// DeployFailure: a stable code, a one-line title, a hint that says what to
// do, and the few log lines that show the problem. It is a pure function so
// every rule is unit-tested; the deploy pipeline gathers the facts.

use tond_common::{DeployFailure, DeployPhase};

/// Lines quoted in a failure excerpt.
const EXCERPT_LINES: usize = 12;

/// What the health phase observed last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    Healthy,
    /// Nothing accepted the connection (or it was reset straight away).
    NotListening,
    /// The app answered with this non-2xx status.
    HttpStatus(u16),
    /// Requests timed out.
    Timeout,
}

/// How the new container ended, if it stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerExit {
    pub running: bool,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
}

/// Everything known about a failure.
#[derive(Debug, Default)]
pub struct FailureContext<'a> {
    pub phase: Option<DeployPhase>,
    /// The error chain, or a description for failures that are not errors.
    pub error: &'a str,
    /// The deployment's latest log lines.
    pub recent_lines: &'a [String],
    pub container_port: i64,
    pub health_check_path: Option<&'a str>,
    pub health_timeout_secs: u64,
    pub probe: Option<ProbeResult>,
    pub container: Option<ContainerExit>,
    /// The new container's own output (tail).
    pub container_logs: &'a [String],
    pub memory_limit: Option<i64>,
    pub has_registry_credential: bool,
}

fn failure(
    phase: Option<DeployPhase>,
    code: &str,
    title: impl Into<String>,
    hint: impl Into<String>,
) -> DeployFailure {
    DeployFailure {
        phase,
        code: code.to_string(),
        title: title.into(),
        hint: hint.into(),
        detail: None,
        excerpt: Vec::new(),
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    let lower = haystack.to_ascii_lowercase();
    needles.iter().any(|n| lower.contains(n))
}

/// Strip ANSI escape sequences so log excerpts match and read cleanly.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // CSI: parameters/intermediates until a final byte @..~
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The exit code in "exit code: exit status: 1" / "exit code: 137" / "exit status 2".
pub fn exit_code_in(text: &str) -> Option<i64> {
    for marker in ["exit status: ", "exit status ", "exit code: ", "exit code "] {
        let mut rest = text;
        while let Some(pos) = rest.find(marker) {
            let after = &rest[pos + marker.len()..];
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(code) = digits.parse() {
                return Some(code);
            }
            rest = after;
        }
    }
    None
}

/// The lines that most likely show an error: those mentioning one, or the
/// tail when none do.
pub fn error_excerpt(lines: &[String]) -> Vec<String> {
    let clean: Vec<String> = lines
        .iter()
        .map(|l| strip_ansi(l))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let is_error = |l: &str| {
        contains_any(
            l,
            &[
                "error",
                "err!",
                "failed",
                "fatal",
                "cannot",
                "can't",
                "not found",
                "no such",
                "exception",
                "panic",
                "killed",
                "denied",
            ],
        )
    };
    // Skip the builder's own closing lines when looking for the error.
    let last_error = clean.iter().rposition(|l| is_error(l));
    match last_error {
        Some(idx) => {
            let start = idx.saturating_sub(EXCERPT_LINES - 1);
            let first_error = clean[start..=idx]
                .iter()
                .position(|l| is_error(l))
                .map(|p| start + p)
                .unwrap_or(start);
            // Keep a couple of lines of context before the first error.
            let from = first_error.saturating_sub(2).max(start);
            clean[from..=idx].to_vec()
        }
        None => {
            let from = clean.len().saturating_sub(EXCERPT_LINES);
            clean[from..].to_vec()
        }
    }
}

fn tail(lines: &[String]) -> Vec<String> {
    let clean: Vec<String> = lines
        .iter()
        .map(|l| strip_ansi(l))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let from = clean.len().saturating_sub(EXCERPT_LINES);
    clean[from..].to_vec()
}

fn format_bytes(bytes: i64) -> String {
    let mb = bytes / (1024 * 1024);
    if mb >= 1024 && mb % 1024 == 0 {
        format!("{} GB", mb / 1024)
    } else {
        format!("{mb} MB")
    }
}

const OOM_MARKERS: &[&str] = &[
    "exit code: 137",
    "exit status: 137",
    "exit code 137",
    "signal: killed",
    "out of memory",
    "heap out of memory",
    "cannot allocate memory",
    "enomem",
    "oomkilled",
];

/// Classify a failed deployment.
pub fn classify(ctx: &FailureContext<'_>) -> DeployFailure {
    let err = ctx.error;
    let phase = ctx.phase;
    let mut f = classify_inner(ctx);
    if f.detail.is_none() && !err.is_empty() {
        f.detail = Some(strip_ansi(err));
    }
    if f.phase.is_none() {
        f.phase = phase;
    }
    f
}

fn classify_inner(ctx: &FailureContext<'_>) -> DeployFailure {
    use DeployPhase::*;
    let err = ctx.error;
    let recent = ctx.recent_lines;
    let joined_recent = recent.join("\n");
    let port = ctx.container_port;

    // ── Source problems ──
    if contains_any(err, &["no dockerfile was found", "dockerfile not found"]) {
        return failure(
            Some(Build),
            "no_dockerfile",
            "No Dockerfile found",
            "The build method is Dockerfile but the repository has no Dockerfile in its root (or base directory). Add one, set the base directory to the folder that has it, or switch the build method to TBuild so Tond generates one.",
        );
    }
    if contains_any(err, &["tbuild failed"]) {
        let mut f = failure(
            Some(Build),
            "build_detection_failed",
            "Tond couldn't work out how to build this repository",
            "TBuild didn't recognise the project. Add a Dockerfile to the repository and set the build method to Dockerfile, or set the base directory if the app lives in a subfolder.",
        );
        f.excerpt = error_excerpt(recent);
        return f;
    }
    if contains_any(err, &["base_dir"]) {
        return failure(
            Some(Build),
            "bad_base_dir",
            "The base directory isn't in the repository",
            "Check the base directory in the app's build settings; it must be a folder inside the repository.",
        );
    }
    if contains_any(err, &["git clone failed"]) {
        if contains_any(
            err,
            &[
                "remote branch",
                "couldn't find remote ref",
                "not found in upstream",
            ],
        ) {
            return failure(
                Some(Clone),
                "branch_not_found",
                "The branch doesn't exist in the repository",
                "Check the branch name in the Git tab; it must match a branch on the remote exactly.",
            );
        }
        if contains_any(
            err,
            &[
                "authentication failed",
                "could not read username",
                "permission denied",
                "repository not found",
                "403",
                "401",
                "terminal prompts disabled",
                "host key verification failed",
            ],
        ) {
            return failure(
                Some(Clone),
                "repo_access_denied",
                "Tond couldn't access the repository",
                "The repository is private or the URL is wrong. Link a GitHub App or GitLab token in the Git tab, or add the app's deploy key to the repository, then deploy again.",
            );
        }
        return failure(
            Some(Clone),
            "clone_failed",
            "The repository couldn't be cloned",
            "Check the Git URL and that this server can reach the Git host.",
        );
    }
    if contains_any(err, &["failed to pull image"]) {
        let denied = contains_any(
            err,
            &[
                "unauthorized",
                "authentication required",
                "denied",
                "401",
                "403",
                "may require 'docker login'",
            ],
        );
        let not_found = contains_any(
            err,
            &["not found", "manifest unknown", "404", "does not exist"],
        );
        if denied || (not_found && !ctx.has_registry_credential) {
            return failure(
                Some(Clone),
                "image_pull_denied",
                "The registry refused to hand out the image",
                if ctx.has_registry_credential {
                    "The attached registry credentials were rejected, or the image name is wrong. Check the image name and tag, and update the credentials in the Environment tab."
                } else {
                    "Check the image name and tag. If the image is private, add registry credentials in the Environment tab and deploy again."
                },
            );
        }
        if not_found {
            return failure(
                Some(Clone),
                "image_not_found",
                "The image or tag doesn't exist",
                "Check the image name and tag for typos; the registry has no such image.",
            );
        }
        return failure(
            Some(Clone),
            "image_pull_failed",
            "The image couldn't be pulled",
            "Check that the image name is right and that this server can reach the registry.",
        );
    }
    if contains_any(err, &["no uploaded image"]) {
        return failure(
            Some(Build),
            "upload_missing",
            "The uploaded source is gone",
            "Upload the source again (tond apps push) to deploy it.",
        );
    }

    // ── Build problems ──
    if contains_any(err, &["docker build failed", "docker compose up failed"]) {
        let compose = contains_any(err, &["compose"]);
        if contains_any(err, OOM_MARKERS) || contains_any(&joined_recent, OOM_MARKERS) {
            let mut f = failure(
                Some(Build),
                "build_oom",
                "The build ran out of memory",
                "The build was killed for using too much memory. Add swap to the server, lower the build's memory use (for Node, NODE_OPTIONS=--max-old-space-size=512), or build on a bigger server.",
            );
            f.excerpt = error_excerpt(recent);
            return f;
        }
        let code = exit_code_in(err);
        let title = match (compose, code) {
            (true, _) => "docker compose up failed".to_string(),
            (false, Some(code)) => format!("The build failed (exit code {code})"),
            (false, None) => "The build failed".to_string(),
        };
        let mut f = failure(
            Some(Build),
            if compose {
                "compose_failed"
            } else {
                "build_failed"
            },
            title,
            "The lines below are where the build stopped. Fix the error in your code or Dockerfile and deploy again.",
        );
        f.excerpt = error_excerpt(recent);
        return f;
    }

    // ── Start problems ──
    if contains_any(
        err,
        &["port is already allocated", "address already in use"],
    ) {
        return failure(
            Some(Start),
            "port_conflict",
            "The container's port is already in use on this server",
            "Another process holds the port Tond picked. Deploy again; if it keeps happening, check for containers started outside Tond.",
        );
    }
    if ctx.phase == Some(Start) && ctx.probe.is_none() {
        return failure(
            Some(Start),
            "start_failed",
            "The container couldn't be started",
            "Docker refused to start the container; the error below says why.",
        );
    }

    // ── Health problems ──
    if let Some(probe) = &ctx.probe {
        if let Some(exit) = ctx.container.as_ref().filter(|c| !c.running) {
            if exit.oom_killed {
                let limit = ctx
                    .memory_limit
                    .map(format_bytes)
                    .map(|l| format!(" of {l}"))
                    .unwrap_or_default();
                let mut f = failure(
                    Some(Health),
                    "app_oom",
                    "The app was killed for running out of memory",
                    format!(
                        "It hit its memory limit{limit}. Raise the memory limit in the Resources tab, or reduce the app's memory use."
                    ),
                );
                f.excerpt = tail(ctx.container_logs);
                return f;
            }
            let code = exit
                .exit_code
                .map(|c| format!(" with code {c}"))
                .unwrap_or_default();
            let mut f = failure(
                Some(Health),
                "app_crashed",
                format!("The app exited{code} right after starting"),
                "The app's own output is below. Common causes: a missing environment variable, a database it can't reach, or a start command that exits instead of running a server.",
            );
            f.excerpt = tail(ctx.container_logs);
            return f;
        }
        let timeout = ctx.health_timeout_secs;
        return match probe {
            ProbeResult::NotListening | ProbeResult::Healthy => {
                let mut f = failure(
                    Some(Health),
                    "port_not_listening",
                    format!("Nothing answered on port {port} within {timeout}s"),
                    format!(
                        "Tond expected the app to listen on port {port} inside the container. If it uses another port (nginx listens on 80, many Node apps on 3000), change the app's port; make sure the app binds to 0.0.0.0, not localhost."
                    ),
                );
                f.excerpt = tail(ctx.container_logs);
                f
            }
            ProbeResult::HttpStatus(status) => {
                let path = ctx.health_check_path.unwrap_or("/");
                let mut f = failure(
                    Some(Health),
                    "health_check_failed",
                    format!("The health check at {path} returned HTTP {status}"),
                    format!(
                        "Tond polls port {port}{path} inside the container and needs a 2xx answer. Fix the endpoint, or change or clear the health check path in the Health tab."
                    ),
                );
                f.excerpt = tail(ctx.container_logs);
                f
            }
            ProbeResult::Timeout => {
                let path = ctx.health_check_path.unwrap_or("/");
                let mut f = failure(
                    Some(Health),
                    "health_check_timeout",
                    format!("The health check at {path} didn't answer in time"),
                    format!(
                        "Requests to port {port}{path} timed out for {timeout}s. If the app needs longer to boot, raise the drain timeout in the Health tab."
                    ),
                );
                f.excerpt = tail(ctx.container_logs);
                f
            }
        };
    }

    let mut f = failure(
        ctx.phase,
        "deploy_failed",
        "The deployment failed",
        "The error below says what went wrong; the full log is above.",
    );
    f.excerpt = error_excerpt(recent);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn ctx<'a>(error: &'a str, recent: &'a [String]) -> FailureContext<'a> {
        FailureContext {
            error,
            recent_lines: recent,
            container_port: 8080,
            health_timeout_secs: 30,
            ..Default::default()
        }
    }

    #[test]
    fn strips_ansi_and_finds_exit_codes() {
        assert_eq!(strip_ansi("\u{1b}[31merror\u{1b}[0m: x"), "error: x");
        assert_eq!(
            exit_code_in("Docker build failed (exit code: exit status: 1)"),
            Some(1)
        );
        assert_eq!(exit_code_in("process exit code: 137"), Some(137));
        assert_eq!(exit_code_in("no code here"), None);
    }

    #[test]
    fn build_failure_quotes_the_error_lines() {
        let recent = lines(&[
            "#5 [2/4] RUN npm ci",
            "#5 1.2 added 10 packages",
            "#6 [3/4] RUN npm run build",
            "#6 2.0 src/app.ts(3,1): error TS2304: Cannot find name 'foo'.",
            "#6 ERROR: process \"/bin/sh -c npm run build\" did not complete successfully: exit code: 2",
            "------",
        ]);
        let f = classify(&ctx(
            "Docker build failed (exit code: exit status: 1)",
            &recent,
        ));
        assert_eq!(f.code, "build_failed");
        assert_eq!(f.title, "The build failed (exit code 1)");
        assert_eq!(f.phase, Some(DeployPhase::Build));
        assert!(
            f.excerpt.iter().any(|l| l.contains("TS2304")),
            "{:?}",
            f.excerpt
        );
        assert!(!f.excerpt.iter().any(|l| l == "------"));
    }

    #[test]
    fn oom_during_build_points_at_memory() {
        let recent = lines(&[
            "#8 RUN npm run build",
            "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory",
        ]);
        let f = classify(&ctx(
            "Docker build failed (exit code: exit status: 1)",
            &recent,
        ));
        assert_eq!(f.code, "build_oom");
        let f = classify(&ctx(
            "Docker build failed (exit code: signal: 9 (SIGKILL))",
            &lines(&["#9 ERROR: process did not complete successfully: exit code: 137"]),
        ));
        assert_eq!(f.code, "build_oom");
    }

    #[test]
    fn pull_failures_mention_registry_credentials() {
        let f = classify(&ctx(
            "Failed to pull image: Docker responded with status code 404: pull access denied for acme/private, repository does not exist or may require 'docker login'",
            &[],
        ));
        assert_eq!(f.code, "image_pull_denied");
        assert!(f.hint.contains("registry credentials"));
        assert_eq!(f.phase, Some(DeployPhase::Clone));

        let mut c = ctx(
            "Failed to pull image: Docker responded with status code 404: manifest unknown",
            &[],
        );
        c.has_registry_credential = true;
        assert_eq!(classify(&c).code, "image_not_found");
    }

    #[test]
    fn clone_failures_are_split_by_cause() {
        assert_eq!(
            classify(&ctx(
                "Git clone failed: fatal: Remote branch mian not found in upstream origin",
                &[]
            ))
            .code,
            "branch_not_found"
        );
        assert_eq!(
            classify(&ctx(
                "Git clone failed: fatal: could not read Username for 'https://github.com': terminal prompts disabled",
                &[]
            ))
            .code,
            "repo_access_denied"
        );
        assert_eq!(
            classify(&ctx("Git clone failed: fatal: unable to access", &[])).code,
            "clone_failed"
        );
    }

    #[test]
    fn missing_dockerfile_and_detection_failures() {
        let f = classify(&ctx(
            "build_method is 'dockerfile' but no Dockerfile was found in the repo root",
            &[],
        ));
        assert_eq!(f.code, "no_dockerfile");
        assert_eq!(
            classify(&ctx("TBuild failed: no supported stack detected", &[])).code,
            "build_detection_failed"
        );
    }

    #[test]
    fn health_failures_explain_the_expected_port_and_path() {
        let logs = lines(&["listening on 127.0.0.1:3000"]);
        let mut c = ctx("new container did not become healthy", &[]);
        c.phase = Some(DeployPhase::Health);
        c.probe = Some(ProbeResult::NotListening);
        c.container = Some(ContainerExit {
            running: true,
            ..Default::default()
        });
        c.container_logs = &logs;
        let f = classify(&c);
        assert_eq!(f.code, "port_not_listening");
        assert!(f.title.contains("port 8080"));
        assert!(f.hint.contains("0.0.0.0"));
        assert_eq!(f.excerpt, logs);

        c.probe = Some(ProbeResult::HttpStatus(503));
        c.health_check_path = Some("/healthz");
        let f = classify(&c);
        assert_eq!(f.code, "health_check_failed");
        assert!(f.title.contains("/healthz") && f.title.contains("503"));

        c.probe = Some(ProbeResult::Timeout);
        assert_eq!(classify(&c).code, "health_check_timeout");
    }

    #[test]
    fn a_container_that_exits_is_a_crash_or_an_oom() {
        let logs = lines(&["Error: DATABASE_URL is not set"]);
        let mut c = ctx("new container did not become healthy", &[]);
        c.probe = Some(ProbeResult::NotListening);
        c.container = Some(ContainerExit {
            running: false,
            exit_code: Some(1),
            oom_killed: false,
        });
        c.container_logs = &logs;
        let f = classify(&c);
        assert_eq!(f.code, "app_crashed");
        assert_eq!(f.title, "The app exited with code 1 right after starting");
        assert_eq!(f.excerpt, logs);

        c.container = Some(ContainerExit {
            running: false,
            exit_code: Some(137),
            oom_killed: true,
        });
        c.memory_limit = Some(512 * 1024 * 1024);
        let f = classify(&c);
        assert_eq!(f.code, "app_oom");
        assert!(f.hint.contains("512 MB"));
    }

    #[test]
    fn unknown_errors_fall_back_with_the_detail() {
        let f = classify(&ctx("something odd", &lines(&["a", "b"])));
        assert_eq!(f.code, "deploy_failed");
        assert_eq!(f.detail.as_deref(), Some("something odd"));
        assert_eq!(f.excerpt, lines(&["a", "b"]));
    }

    #[test]
    fn excerpt_is_the_tail_without_errors_and_bounded_with_them() {
        let many: Vec<String> = (0..40).map(|i| format!("step {i}")).collect();
        assert_eq!(error_excerpt(&many).len(), EXCERPT_LINES);
        assert_eq!(
            error_excerpt(&many).last().map(String::as_str),
            Some("step 39")
        );
        let mut with_error = many.clone();
        with_error.push("npm ERR! missing script: build".into());
        with_error.push("done".into());
        let ex = error_excerpt(&with_error);
        assert_eq!(
            ex.last().map(String::as_str),
            Some("npm ERR! missing script: build")
        );
        assert!(ex.len() <= EXCERPT_LINES);
    }
}
