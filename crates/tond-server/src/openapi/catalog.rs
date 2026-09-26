// openapi/catalog.rs — the human-written half of the spec
//
// One entry per route in routes.rs, keyed "METHOD /path": a summary, the
// success status and what `data` holds. Auth, scopes, parameters and the
// request body come from the handler's types (describe.rs), so they can't
// drift. generate.rs fails when a route has no entry here or an entry has no
// route.
//
// Response kinds:
//   returns::<T>()   `data` is a T (a JSON Schema derived from the type)
//   message()        `data` is {"message": "…"}
//   object(d)        `data` is an object described in words (ad-hoc JSON)
//   null()           `data` is null
//   raw(ct, d)       not enveloped (probes, Prometheus, downloads, callbacks)
//   websocket(d)     a WebSocket; frames described in words

use schemars::{JsonSchema, Schema, SchemaGenerator};
use tond_common::*;

use crate::handlers::{
    self, domains::DomainCheckResponse, git::RemoteBranchesResponse, host, images,
    onboarding::OnboardingStatusResponse, terminal, tokens::WhoAmIResponse, updates, uptime,
    users::AdminUserResponse,
};
use crate::metrics::{docker_host, host_history};

pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<T>()
}

#[derive(Clone)]
pub enum Resp {
    Data(SchemaFn),
    Message,
    Object(&'static str),
    Null,
    Raw(&'static str, &'static str),
    WebSocket(&'static str),
}

#[derive(Clone)]
pub struct Doc {
    pub key: &'static str,
    pub summary: &'static str,
    pub description: Option<&'static str>,
    pub status: u16,
    pub response: Resp,
    /// Scopes a handler checks itself (on top of its extractor's).
    pub extra_scopes: &'static [&'static str],
}

fn e(key: &'static str, summary: &'static str) -> Doc {
    Doc {
        key,
        summary,
        description: None,
        status: 200,
        response: Resp::Object("See the summary."),
        extra_scopes: &[],
    }
}

impl Doc {
    fn returns<T: JsonSchema>(mut self) -> Self {
        self.response = Resp::Data(schema_of::<T>);
        self
    }
    fn message(mut self) -> Self {
        self.response = Resp::Message;
        self
    }
    fn object(mut self, what: &'static str) -> Self {
        self.response = Resp::Object(what);
        self
    }
    fn null(mut self) -> Self {
        self.response = Resp::Null;
        self
    }
    fn raw(mut self, content_type: &'static str, what: &'static str) -> Self {
        self.response = Resp::Raw(content_type, what);
        self
    }
    fn websocket(mut self, frames: &'static str) -> Self {
        self.response = Resp::WebSocket(frames);
        self
    }
    fn created(mut self) -> Self {
        self.status = 201;
        self
    }
    fn accepted(mut self) -> Self {
        self.status = 202;
        self
    }
    fn about(mut self, text: &'static str) -> Self {
        self.description = Some(text);
        self
    }
    fn scopes(mut self, scopes: &'static [&'static str]) -> Self {
        self.extra_scopes = scopes;
        self
    }
}

const DELETED: &str = "`{\"deleted\": \"<id>\"}`";

pub fn entries() -> Vec<Doc> {
    vec![
        // ── Probes and the spec ──
        e("GET /health", "Liveness probe").raw("application/json", "`{\"status\":\"ok\",\"service\":\"tond\"}` (not enveloped)."),
        e("GET /api/v1/openapi.json", "This OpenAPI document").raw("application/json", "The OpenAPI 3.1 document (not enveloped)."),
        e("GET /metrics", "Prometheus metrics")
            .raw("text/plain; version=0.0.4", "Prometheus text exposition of Tond's own metrics.")
            .about("Needs a token with `metrics:read` over TCP; the root socket needs none."),
        // ── Auth (dashboard sessions) ──
        e("GET /api/v1/auth/status", "Whether the caller is signed in").object("`{authenticated, user}`; never reveals whether setup is open."),
        e("POST /api/v1/auth/setup", "Create the first admin account")
            .created()
            .object("`{user, message}`; sets the session cookie.")
            .about("Needs the one-time setup token printed by the installer (`tond setup-link`)."),
        e("POST /api/v1/auth/setup/check", "Check a setup token").object("`{valid: true}`"),
        e("POST /api/v1/auth/login", "Sign in with email and password")
            .object("`{user}` and a session cookie, or `{mfa_required: true}` when a TOTP code must follow (POST /api/v1/auth/2fa/verify)."),
        e("POST /api/v1/auth/logout", "Sign out").message(),
        e("GET /api/v1/auth/me", "The signed-in user").returns::<UserResponse>(),
        e("GET /api/v1/auth/whoami", "Who the credentials belong to (user, token or local socket)").returns::<WhoAmIResponse>(),
        e("POST /api/v1/auth/password-link/check", "Check a set-password link").object("`{email, purpose}`"),
        e("POST /api/v1/auth/password-link/complete", "Set a password from a one-time link")
            .object("`{signed_in, user?, message?}`"),
        e("POST /api/v1/auth/change-password", "Change your password").message(),
        e("GET /api/v1/auth/sessions", "Your active sessions").returns::<Vec<SessionResponse>>(),
        e("DELETE /api/v1/auth/sessions", "Sign out every other session").message(),
        e("DELETE /api/v1/auth/sessions/{id}", "Revoke one of your sessions").message(),
        e("POST /api/v1/auth/2fa/setup", "Start TOTP two-factor setup").returns::<TotpSetupResponse>(),
        e("POST /api/v1/auth/2fa/confirm", "Confirm TOTP setup").object("`{message, backup_codes}`"),
        e("POST /api/v1/auth/2fa/disable", "Turn off two-factor auth").message(),
        e("POST /api/v1/auth/2fa/verify", "Finish a sign-in with a TOTP or backup code").object("`{user}` and a session cookie."),
        e("GET /api/v1/onboarding/status", "First-run checklist").returns::<OnboardingStatusResponse>(),
        // ── Users ──
        e("GET /api/v1/users", "List users").returns::<Vec<AdminUserResponse>>(),
        e("POST /api/v1/users", "Invite a user").created().returns::<PasswordLinkResponse>(),
        e("PATCH /api/v1/users/{id}", "Change a user's role or disable them").returns::<UserResponse>(),
        e("DELETE /api/v1/users/{id}", "Delete a user").object(DELETED),
        e("POST /api/v1/users/{id}/password-link", "Issue a one-time set-password link").created().returns::<PasswordLinkResponse>(),
        e("GET /api/v1/users/{id}/sessions", "A user's sessions").returns::<Vec<SessionResponse>>(),
        e("DELETE /api/v1/users/{id}/sessions", "Sign a user out everywhere").object("`{revoked: <count>}`"),
        e("DELETE /api/v1/users/{id}/sessions/{session_id}", "Revoke one session of a user").object("`{revoked: 1}`"),
        // ── API tokens ──
        e("GET /api/v1/tokens/catalog", "Scopes, groups, presets and roles for the token form").returns::<ScopeCatalog>(),
        e("GET /api/v1/tokens", "List API tokens")
            .returns::<Vec<TokenResponse>>()
            .about("Your own tokens; `all=true` lists everyone's and needs `tokens:admin`."),
        e("POST /api/v1/tokens", "Create an API token")
            .created()
            .returns::<CreatedTokenResponse>()
            .about("The secret is in the response once and never again. Scopes are capped by your role; `tokens:admin` may create tokens for others."),
        e("DELETE /api/v1/tokens/{id}", "Revoke an API token").returns::<TokenResponse>(),
        // ── Apps ──
        e("GET /api/v1/apps", "List apps").returns::<Vec<AppResponse>>(),
        e("POST /api/v1/apps", "Create an app")
            .created()
            .returns::<AppResponse>()
            .about("Volumes and Docker Compose sources also need `settings:write`, because they reach the host."),
        e("GET /api/v1/apps/{id}", "Get an app (by id or name)").returns::<AppResponse>(),
        e("DELETE /api/v1/apps/{id}", "Delete an app, its containers, images and volumes").object(DELETED),
        e("POST /api/v1/apps/{id}/deploy", "Deploy an app from its source")
            .accepted()
            .returns::<handlers::QueuedDeploymentResponse>()
            .about("Queues a deployment and returns at once. Follow it with GET /api/v1/apps/{id}/deployments/{deployment_id} or its /stream WebSocket (`tond apps deploy --wait`)."),
        e("POST /api/v1/apps/{id}/stop", "Stop an app").message(),
        e("POST /api/v1/apps/{id}/start", "Start a stopped app").message(),
        e("POST /api/v1/apps/{id}/restart", "Restart an app").message(),
        e("GET /api/v1/apps/{id}/logs", "Recent deploy and container logs").object("`{deploy_logs, container_logs}`"),
        e("PUT /api/v1/apps/{id}/domain", "Make a domain the app's primary domain").object("`{message, domain, warning?}`; the domain is added if needed, the previous primary stays as an alias, and certificate provisioning is queued."),
        e("GET /api/v1/apps/{id}/domains", "List an app's domains (primary, aliases, redirects)").returns::<Vec<AppDomainResponse>>(),
        e("POST /api/v1/apps/{id}/domains", "Add a domain to an app").created().returns::<Vec<AppDomainResponse>>(),
        e("POST /api/v1/apps/{id}/domains/www-pair", "Add a www/apex pair, one redirecting to the other").created().returns::<Vec<AppDomainResponse>>(),
        e("PATCH /api/v1/apps/{id}/domains/{domain}", "Change a domain's kind, redirect target or code").returns::<Vec<AppDomainResponse>>(),
        e("DELETE /api/v1/apps/{id}/domains/{domain}", "Remove a domain from an app").returns::<Vec<AppDomainResponse>>(),
        e("PUT /api/v1/apps/{id}/health", "Configure health checks").message(),
        e("PUT /api/v1/apps/{id}/port", "Change the container port").returns::<AppResponse>(),
        e("PUT /api/v1/apps/{id}/readiness", "Configure readiness (when a deploy counts as up)").returns::<AppResponse>(),
        e("POST /api/v1/compose/validate", "Validate a Docker Compose file against Tond's policy").object("`{valid, errors}`"),
        e("POST /api/v1/apps/{id}/volumes", "Add a volume (applies on the next deploy)")
            .object("`{message, volumes}`")
            .scopes(&["settings:write"]),
        e("DELETE /api/v1/apps/{id}/volumes", "Remove a volume (applies on the next deploy)")
            .object("`{message, volumes}`")
            .scopes(&["settings:write"]),
        e("PUT /api/v1/apps/{id}/drain-timeout", "Set the graceful-deploy drain timeout").object("`{message, drain_timeout}`"),
        e("PATCH /api/v1/apps/{id}/resources", "Change memory and CPU limits").returns::<AppResponse>(),
        e("POST /api/v1/apps/{id}/upload", "Deploy an uploaded source archive")
            .accepted()
            .returns::<handlers::QueuedDeploymentResponse>()
            .about("multipart/form-data with a `file` part holding a .tar.gz of the source (what `tond apps push` sends). The size cap is the `upload_max_bytes` setting."),
        e("PUT /api/v1/apps/{id}/project", "Move an app into a project (or out)").message(),
        // ── Environment ──
        e("GET /api/v1/apps/{id}/env", "Environment variable names (values masked)").returns::<AppEnvResponse>(),
        e("PATCH /api/v1/apps/{id}/env", "Set and unset environment variables").returns::<AppEnvResponse>(),
        e("GET /api/v1/apps/{id}/env/{key}", "Reveal one environment variable").returns::<EnvVarValueResponse>(),
        e("PATCH /api/v1/apps/{id}/preview-env", "Set and unset preview-only variables").returns::<AppEnvResponse>(),
        e("GET /api/v1/apps/{id}/preview-env/{key}", "Reveal one preview-only variable").returns::<EnvVarValueResponse>(),
        // ── Logs, terminal ──
        e("GET /api/v1/apps/{id}/logs/stream", "Live app logs")
            .websocket("A `snapshot` frame (recent deploy and container logs), then `deploy` and `container` frames as they happen."),
        e("GET /api/v1/system/logs/stream", "Live Tond server logs").websocket("One JSON frame per log line."),
        e("GET /api/v1/databases/logs/stream", "Live logs of every database").websocket("One JSON frame per log line."),
        e("GET /api/v1/databases/{id}/logs/stream", "Live logs of one database").websocket("One JSON frame per log line."),
        e("GET /api/v1/cron/{id}/runs/stream", "Live cron job runs and their output").websocket("Run updates and output lines as JSON frames."),
        e("GET /api/v1/snapshots/{id}/logs/stream", "Live snapshot progress").websocket("Progress and log frames until the snapshot finishes."),
        e("GET /api/v1/terminal/targets", "Containers (and the host) a terminal can open in").returns::<Vec<terminal::TerminalTargetResponse>>(),
        e("POST /api/v1/terminal/tickets", "Get a one-time ticket to open a terminal")
            .created()
            .returns::<terminal::TerminalTicketResponse>()
            .about("A host terminal also needs `terminal:host`, the setting turned on and your password (step-up)."),
        e("GET /api/v1/terminal/ws", "Terminal session").websocket("Binary frames carry the PTY; JSON control frames resize and report exit."),
        // ── Deployments ──
        e("GET /api/v1/apps/{id}/deployments", "Deployment history").returns::<Vec<DeploymentResponse>>(),
        e("GET /api/v1/apps/{id}/deployments/stats", "Deploy success rate and durations").returns::<DeployStatsResponse>(),
        e("GET /api/v1/apps/{id}/deployments/{deployment_id}", "A deployment with its phases and log").returns::<DeploymentDetailResponse>(),
        e("GET /api/v1/apps/{id}/deployments/{deployment_id}/stream", "Follow a deployment live")
            .websocket("A `snapshot` frame (row and log so far), then `log` lines and `deployment` updates; the server closes shortly after the deployment finishes."),
        e("GET /api/v1/apps/{id}/deployments/{deployment_id}/generated-dockerfile", "The Dockerfile TBuild generated for a deployment")
            .object("`{deployment_id, generated_dockerfile}`"),
        e("POST /api/v1/apps/{id}/rollback", "Roll back to the previous successful deployment's image")
            .accepted()
            .returns::<handlers::QueuedDeploymentResponse>(),
        // ── Sources, images, domains ──
        e("POST /api/v1/git/branches", "List a remote repository's branches").returns::<RemoteBranchesResponse>(),
        e("POST /api/v1/images/inspect", "Inspect a container image (ports, size)").returns::<images::InspectImageResponse>(),
        e("GET /api/v1/domains/check", "Check a domain's DNS against this server").returns::<DomainCheckResponse>(),
        e("PUT /api/v1/apps/{id}/auto-deploy", "Turn deploy-on-push on or off").returns::<AppResponse>(),
        e("GET /api/v1/apps/{id}/webhook", "Webhook URLs for push deploys").returns::<WebhookConfigResponse>(),
        e("PUT /api/v1/apps/{id}/webhook-secret", "Set the webhook secret").returns::<WebhookConfigResponse>(),
        e("GET /api/v1/apps/{id}/webhook-secret", "Reveal the webhook secret").returns::<WebhookSecretResponse>(),
        e("POST /api/v1/apps/{id}/webhook-secret/rotate", "Rotate the webhook secret").returns::<WebhookSecretResponse>(),
        e("POST /api/v1/apps/{id}/deploy-key", "Generate an SSH deploy key").created().returns::<DeployKeyResponse>(),
        e("GET /api/v1/apps/{id}/deploy-key", "The app's SSH deploy key (public part)").returns::<DeployKeyResponse>(),
        e("PUT /api/v1/apps/{id}/source", "Change an app's source (repo, branch, image…)").returns::<AppResponse>(),
        // ── Certificates, settings, templates ──
        e("GET /api/v1/certs", "TLS certificates and their status").returns::<Vec<CertificateResponse>>(),
        e("POST /api/v1/certs/{domain}/retry", "Retry issuing a certificate").object("`{message, domain}`"),
        e("POST /api/v1/certs/custom", "Upload a certificate for a domain").created().returns::<Option<CertificateResponse>>(),
        e("DELETE /api/v1/certs/custom/{domain}", "Remove an uploaded certificate").object("`{\"deleted\": \"<domain>\"}`"),
        e("GET /api/v1/dns-providers", "List DNS providers (ACME DNS-01)").returns::<Vec<DnsProviderResponse>>(),
        e("POST /api/v1/dns-providers", "Add a DNS provider").created().returns::<DnsProviderResponse>(),
        e("PATCH /api/v1/dns-providers/{id}", "Update a DNS provider").returns::<DnsProviderResponse>(),
        e("DELETE /api/v1/dns-providers/{id}", "Delete a DNS provider").object(DELETED),
        e("POST /api/v1/dns-providers/{id}/test", "Check a DNS provider can manage its zones").returns::<DnsProviderTestResult>(),
        e("GET /api/v1/settings", "Server settings").returns::<ServerSettings>(),
        e("PUT /api/v1/settings", "Update server settings").returns::<ServerSettings>(),
        e("GET /api/v1/settings/retention", "Data retention rules and table sizes").returns::<RetentionResponse>(),
        e("PUT /api/v1/settings/retention", "Update data retention").returns::<RetentionResponse>(),
        e("GET /api/v1/settings/host-terminal", "Whether the host terminal is enabled").returns::<terminal::HostTerminalSetting>(),
        e("PUT /api/v1/settings/host-terminal", "Enable or disable the host terminal")
            .returns::<terminal::HostTerminalSetting>()
            .about("Only over the root-only Unix socket (`tond settings host-terminal on|off`)."),
        e("GET /api/v1/templates", "The app template catalog").returns::<Vec<TemplateCatalogEntry>>(),
        // ── Registry credentials ──
        e("GET /api/v1/registry-credentials", "List container registry credentials").returns::<Vec<RegistryCredentialResponse>>(),
        e("POST /api/v1/registry-credentials", "Add a registry credential").created().returns::<RegistryCredentialResponse>(),
        e("DELETE /api/v1/registry-credentials/{id}", "Delete a registry credential").null(),
        e("PUT /api/v1/apps/{id}/registry-credential", "Use a registry credential for an app's image").returns::<AppResponse>(),
        // ── GitHub Apps ──
        e("GET /api/v1/github-apps", "List connected GitHub Apps").returns::<Vec<GitHubAppResponse>>(),
        e("POST /api/v1/github-apps/manifest", "Start creating a GitHub App (manifest flow)").object("`{github_url, manifest}`"),
        e("POST /api/v1/github-apps/{id}/install", "Install URL for a GitHub App").object("`{url}`"),
        e("PATCH /api/v1/github-apps/{id}", "Update a GitHub App").returns::<GitHubAppResponse>(),
        e("DELETE /api/v1/github-apps/{id}", "Disconnect a GitHub App").object("`{deleted, name}`"),
        e("GET /api/v1/github-apps/{id}/repos", "Repositories a GitHub App can access").returns::<Vec<GitHubRepoResponse>>(),
        e("GET /api/v1/github-apps/{id}/repos/{owner}/{repo}/branches", "Branches of a repository").returns::<Vec<GitHubBranchResponse>>(),
        e("PUT /api/v1/apps/{id}/github-app", "Link an app to a GitHub App").returns::<AppResponse>(),
        e("GET /api/v1/apps/{id}/github-prs", "Open pull requests of an app's repository").object("A list of `{number, title, head_ref, url, …}`."),
        e("GET /api/v1/github-apps/callback", "GitHub App manifest callback (called by GitHub)").raw("text/html", "Redirects to the dashboard."),
        e("POST /api/v1/github-apps/complete", "Finish the GitHub App manifest flow").object("`{completed: true}`"),
        e("GET /webhooks/source/github/install", "GitHub App installation callback (called by GitHub)").raw("text/html", "Redirects to the dashboard."),
        // ── GitLab tokens ──
        e("GET /api/v1/gitlab-tokens", "List GitLab access tokens").returns::<Vec<GitLabTokenResponse>>(),
        e("POST /api/v1/gitlab-tokens", "Add a GitLab access token").created().returns::<GitLabTokenResponse>(),
        e("DELETE /api/v1/gitlab-tokens/{id}", "Delete a GitLab access token").null(),
        e("GET /api/v1/gitlab-tokens/{id}/repos", "Projects a GitLab token can access").returns::<Vec<GitLabRepoResponse>>(),
        e("GET /api/v1/gitlab-tokens/{id}/repos/{project_id}/branches", "Branches of a GitLab project").returns::<Vec<GitLabBranchResponse>>(),
        e("GET /api/v1/gitea-tokens", "List Gitea/Forgejo access tokens").returns::<Vec<GiteaTokenResponse>>(),
        e("POST /api/v1/gitea-tokens", "Add a Gitea/Forgejo access token").created().returns::<GiteaTokenResponse>()
            .about("The token is checked against <base_url>/api/v1/user first. Linked to an app it clones private repositories and reports commit statuses and pull request previews."),
        e("DELETE /api/v1/gitea-tokens/{id}", "Delete a Gitea/Forgejo access token").null(),
        e("PUT /api/v1/apps/{id}/gitea-token", "Link or unlink a Gitea/Forgejo token and an app").returns::<AppResponse>(),
        e("PUT /api/v1/apps/{id}/gitlab-token", "Use a GitLab token for an app's repository").returns::<AppResponse>(),
        // ── Push webhooks (called by the forge) ──
        e("POST /api/v1/webhooks/github/{app_id}", "GitHub push/PR webhook")
            .object("Enveloped; `message` says why nothing was deployed (branch not watched, auto-deploy off).")
            .about("Authenticated by the HMAC signature (X-Hub-Signature-256) with the app's webhook secret."),
        e("POST /api/v1/webhooks/gitlab/{app_id}", "GitLab push/MR webhook")
            .object("Enveloped; `message` says why nothing was deployed.")
            .about("Authenticated by X-Gitlab-Token."),
        e("POST /api/v1/webhooks/gitea/{app_id}", "Gitea/Forgejo push webhook")
            .object("Enveloped; `message` says why nothing was deployed.")
            .about("Authenticated by the HMAC signature (X-Gitea-Signature)."),
        e("POST /api/v1/webhooks/generic/{app_id}", "Generic deploy webhook")
            .object("Enveloped; queues a deploy.")
            .about("Authenticated by the app's webhook secret."),
        e("POST /webhooks/source/github/events", "GitHub App events (called by GitHub)")
            .raw("application/json", "Acknowledgement.")
            .about("Authenticated by the GitHub App's webhook signature."),
        // ── Previews ──
        e("POST /api/v1/apps/{id}/preview", "Deploy a preview of a branch or pull request").accepted().returns::<PreviewDeploymentResponse>(),
        e("GET /api/v1/apps/{id}/previews", "List an app's previews").returns::<Vec<PreviewDeploymentResponse>>(),
        e("DELETE /api/v1/apps/{id}/previews/{preview_id}", "Delete a preview").object("`{deleted, slug}`"),
        e("PUT /api/v1/apps/{id}/preview-config", "Configure previews for an app").object("`{preview_enabled, …}`"),
        // ── Observability ──
        e("GET /api/v1/apps/{id}/events", "App events (deploys, restarts, crashes, OOM kills)").returns::<AppEventsResponse>(),
        e("GET /api/v1/apps/{id}/metrics", "App CPU, memory and network over time").returns::<MetricsResponse>(),
        e("GET /api/v1/databases/{id}/metrics", "Database CPU, memory and network over time").returns::<MetricsResponse>(),
        e("GET /api/v1/cron/{id}/metrics", "Cron job resource use over time").returns::<MetricsResponse>(),
        e("GET /api/v1/system/metrics", "Current host and container metrics").returns::<SystemMetrics>(),
        e("GET /api/v1/apps/{id}/requests", "App HTTP traffic (requests, status mix, latency)").returns::<RequestMetricsResponse>(),
        e("GET /api/v1/previews/{id}/requests", "Preview HTTP traffic").returns::<RequestMetricsResponse>(),
        e("GET /api/v1/previews/{id}/metrics", "Preview resource use over time").returns::<MetricsResponse>(),
        e("GET /api/v1/system/requests", "Traffic overview across apps").returns::<TrafficOverviewResponse>(),
        e("GET /api/v1/apps/{id}/access-log", "Access log settings and recent lines").returns::<AccessLogResponse>(),
        e("PUT /api/v1/apps/{id}/access-log", "Turn the access log on or off").returns::<AccessLogSettings>(),
        e("DELETE /api/v1/apps/{id}/access-log", "Delete the stored access log").null(),
        e("GET /api/v1/apps/{id}/access-log/download", "Download the access log").raw("text/plain", "The access log as JSON lines."),
        e("GET /api/v1/system/self", "Tond's own resource use, loops, DB and HTTP latency").returns::<SystemSelfResponse>(),
        e("GET /api/v1/system/host", "Host CPU, memory, disk and Docker").returns::<host::HostResponse>(),
        e("GET /api/v1/system/host/history", "Host metrics over time").returns::<host::HistoryResponse>(),
        e("GET /api/v1/system/host/forecast", "When the disk will be full at the current rate").returns::<host_history::DiskEta>(),
        e("POST /api/v1/system/disk/refresh", "Recompute Docker disk usage").returns::<docker_host::DiskUsage>(),
        e("GET /api/v1/system/disk/cleanup", "What a Docker cleanup would free").returns::<docker_host::CleanupPreview>(),
        e("POST /api/v1/system/disk/cleanup", "Free disk: unused images and build cache").returns::<docker_host::CleanupResult>(),
        e("GET /api/v1/system/heartbeat", "Outbound heartbeat settings and status").returns::<host::HeartbeatResponse>(),
        e("PUT /api/v1/system/heartbeat", "Configure the outbound heartbeat").returns::<host::HeartbeatResponse>(),
        e("POST /api/v1/system/heartbeat/test", "Send a test heartbeat").returns::<host::HeartbeatTestResponse>(),
        e("GET /api/v1/system/update", "Running version, latest release, changelog, update run and rollback offer").returns::<updates::UpdateOverview>(),
        e("POST /api/v1/system/update", "Update Tond: a full snapshot, then the update in tond-update.service (automatic rollback on failure)").accepted().returns::<crate::updates::run::RunStatus>(),
        e("POST /api/v1/system/update/rollback", "Roll back to the version before the last update (within 7 days)").accepted().returns::<crate::updates::run::RunStatus>(),
        e("POST /api/v1/system/update/check", "Look for new Tond releases now (at most once a minute)").returns::<updates::UpdateOverview>(),
        e("PUT /api/v1/system/update/settings", "Turn the daily update check on or off").returns::<updates::UpdateOverview>(),
        e("GET /api/v1/uptime", "Uptime of every check").returns::<Vec<uptime::UptimeSummaryResponse>>(),
        e("GET /api/v1/apps/{id}/uptime", "An app's uptime check").returns::<uptime::UptimeCheckResponse>(),
        e("PUT /api/v1/apps/{id}/uptime", "Configure an app's uptime check").returns::<uptime::UptimeCheckResponse>(),
        e("POST /api/v1/apps/{id}/uptime/check", "Probe an app now").returns::<uptime::UptimeCheckResponse>(),
        e("GET /api/v1/system/uptime", "The dashboard's uptime check").returns::<uptime::UptimeCheckResponse>(),
        e("PUT /api/v1/system/uptime", "Configure the dashboard's uptime check").returns::<uptime::UptimeCheckResponse>(),
        e("POST /api/v1/system/uptime/check", "Probe the dashboard now").returns::<uptime::UptimeCheckResponse>(),
        // ── Alerts ──
        e("GET /api/v1/alerts", "List alert rules").returns::<Vec<AlertRuleResponse>>(),
        e("POST /api/v1/alerts", "Create an alert rule").created().returns::<AlertRuleResponse>(),
        e("GET /api/v1/alerts/kinds", "Alert kinds and their parameters").returns::<Vec<AlertKindResponse>>(),
        e("GET /api/v1/alerts/active", "Alerts firing now").returns::<Vec<ActiveAlert>>(),
        e("GET /api/v1/alerts/history", "Alert history").returns::<AlertHistoryPage>(),
        e("GET /api/v1/alerts/history/{id}", "One alert episode with its deliveries").returns::<AlertEpisodeDetail>(),
        e("POST /api/v1/alerts/history/{id}/acknowledge", "Acknowledge an alert").object("`{acknowledged: <id>}`"),
        e("GET /api/v1/alerts/settings", "Global alert settings (quiet hours)").returns::<AlertSettings>(),
        e("PUT /api/v1/alerts/settings", "Update global alert settings").returns::<AlertSettings>(),
        e("GET /api/v1/alerts/{id}", "Get an alert rule").returns::<AlertRuleResponse>(),
        e("PATCH /api/v1/alerts/{id}", "Update an alert rule").returns::<AlertRuleResponse>(),
        e("DELETE /api/v1/alerts/{id}", "Delete an alert rule").object(DELETED),
        e("POST /api/v1/alerts/{id}/test", "Send a test alert to the rule's channels").object("A result per channel."),
        e("POST /api/v1/alerts/{id}/snooze", "Snooze an alert rule").object("`{snoozed_until}`"),
        e("POST /api/v1/alerts/{id}/unsnooze", "Unsnooze an alert rule").object("`{unsnoozed: true}`"),
        // ── Notification channels ──
        e("GET /api/v1/integrations", "List notification channels").returns::<Vec<IntegrationResponse>>(),
        e("POST /api/v1/integrations", "Add a notification channel").created().returns::<IntegrationResponse>(),
        e("GET /api/v1/integrations/{id}", "Get a notification channel").returns::<IntegrationResponse>(),
        e("PATCH /api/v1/integrations/{id}", "Update a notification channel").returns::<IntegrationResponse>(),
        e("DELETE /api/v1/integrations/{id}", "Delete a notification channel").object(DELETED),
        e("POST /api/v1/integrations/{id}/test", "Send a test notification").message(),
        // ── Audit log ──
        e("GET /api/v1/audit-log", "Audit log, newest first").returns::<AuditLogPage>(),
        e("GET /api/v1/audit-log/shipping", "Audit log shipping settings and status").returns::<AuditShippingResponse>(),
        e("PUT /api/v1/audit-log/shipping", "Configure audit log shipping").returns::<AuditShippingResponse>(),
        e("POST /api/v1/audit-log/shipping/test", "Send a test audit event").returns::<AuditShippingTestResult>(),
        e("GET /api/v1/audit-log/verify", "Verify the audit log's hash chain").returns::<AuditVerifyReport>(),
        // ── Projects ──
        e("GET /api/v1/projects", "List projects").returns::<Vec<ProjectResponse>>(),
        e("POST /api/v1/projects", "Create a project").created().returns::<ProjectResponse>(),
        e("GET /api/v1/projects/{id}", "A project with its apps, databases and cron jobs").object("`{project, apps, databases, cron_jobs}`"),
        e("PUT /api/v1/projects/{id}", "Update a project").returns::<ProjectResponse>(),
        e("DELETE /api/v1/projects/{id}", "Delete a project (its apps are kept)").object(DELETED),
        e("POST /api/v1/projects/{id}/deploy", "Deploy every app in a project")
            .accepted()
            .object("`{queued, deployment_ids, message}`"),
        e("GET /api/v1/projects/{id}/previews", "List a project's previews").returns::<Vec<ProjectPreviewResponse>>(),
        e("POST /api/v1/projects/{id}/previews", "Deploy a preview of the whole project").accepted().returns::<ProjectPreviewResponse>(),
        e("DELETE /api/v1/projects/{id}/previews/{preview_id}", "Delete a project preview").object("`{deleted, slug}`"),
        // ── Databases ──
        e("GET /api/v1/databases", "List databases").returns::<Vec<ManagedDatabaseResponse>>(),
        e("POST /api/v1/databases", "Create a database")
            .created()
            .returns::<CreatedDatabaseResponse>()
            .about("The credentials are in the response; reveal them later with GET /api/v1/databases/{id}/credentials (`db:credentials`)."),
        e("GET /api/v1/databases/{id}", "Get a database").returns::<ManagedDatabaseDetail>(),
        e("DELETE /api/v1/databases/{id}", "Delete a database and its data").object(DELETED),
        e("PATCH /api/v1/databases/{id}", "Update a database's settings").returns::<ManagedDatabaseDetail>(),
        e("POST /api/v1/databases/{id}/stop", "Stop a database").message(),
        e("POST /api/v1/databases/{id}/start", "Start a database").message(),
        e("POST /api/v1/databases/{id}/redeploy", "Recreate a database's container (data kept)").returns::<ManagedDatabaseDetail>(),
        e("GET /api/v1/databases/{id}/backups", "A database's backups").returns::<Vec<SnapshotResponse>>(),
        e("GET /api/v1/databases/catalog", "Engines, versions and image variants a database can run").returns::<Vec<DbCatalogEntry>>(),
        e("GET /api/v1/databases/{id}/backup-schedule", "A database's backup schedule and backup health").returns::<DatabaseBackupStatus>(),
        e("PUT /api/v1/databases/{id}/backup-schedule", "Set or turn off a database's backup schedule").returns::<DatabaseBackupStatus>()
            .about("Scheduled backups run in the backup scheduler (UTC cron) and keep `retention` (1–365) backups; a failure fires the \"backup failed\" alert for that database."),
        e("GET /api/v1/databases/{id}/extensions", "PostgreSQL extensions offered for a database").returns::<Vec<DatabaseExtension>>(),
        e("POST /api/v1/databases/{id}/extensions", "Enable a PostgreSQL extension").message(),
        e("POST /api/v1/databases/{id}/upgrade", "Start a major-version upgrade").accepted().returns::<DbUpgradeState>()
            .about("Admin only. Backs up, loads the data into a fresh data directory of the new version, switches, and keeps the old data until confirmed. Downgrades are refused."),
        e("POST /api/v1/databases/{id}/upgrade/confirm", "Keep an upgraded version (deletes the old data)").message(),
        e("POST /api/v1/databases/{id}/upgrade/rollback", "Roll an upgrade back to the old version and data").message(),
        e("POST /api/v1/databases/{id}/upgrade/dismiss", "Clear a failed upgrade's state").null(),
        e("POST /api/v1/databases/{id}/backups", "Back up a database now").created().returns::<SnapshotResponse>(),
        e("PATCH /api/v1/databases/{id}/credentials", "Change a database's credentials").object("`{database, changed}`"),
        e("GET /api/v1/databases/{id}/credentials", "Reveal a database's credentials").returns::<DatabaseCredentialsResponse>(),
        e("GET /api/v1/databases/{id}/access", "Which apps can reach a database").returns::<DatabaseAccessResponse>(),
        e("PUT /api/v1/databases/{id}/links/{app}", "Let an app reach a database").returns::<DatabaseAccessResponse>(),
        e("DELETE /api/v1/databases/{id}/links/{app}", "Stop an app reaching a database").returns::<DatabaseAccessResponse>(),
        e("GET /api/v1/apps/{id}/databases", "Databases an app can reach").returns::<Vec<AppDatabaseAccess>>(),
        // ── Cron ──
        e("GET /api/v1/cron", "List cron jobs").returns::<Vec<CronJobResponse>>(),
        e("POST /api/v1/cron", "Create a cron job").created().returns::<CronJobResponse>(),
        e("GET /api/v1/cron/schedule-preview", "Next run times of a schedule").object("`{next_runs, timezone}`"),
        e("GET /api/v1/cron/{id}", "Get a cron job").returns::<CronJobResponse>(),
        e("PATCH /api/v1/cron/{id}", "Update a cron job").returns::<CronJobResponse>(),
        e("DELETE /api/v1/cron/{id}", "Delete a cron job").object(DELETED),
        e("POST /api/v1/cron/{id}/trigger", "Run a cron job now").accepted().object("`{message, cron_job_id}`"),
        e("GET /api/v1/cron/{id}/runs", "A cron job's runs").returns::<Vec<CronJobRun>>(),
        // ── Storage providers ──
        e("GET /api/v1/storage-providers", "List S3-compatible storage providers").returns::<Vec<StorageProviderResponse>>(),
        e("POST /api/v1/storage-providers", "Add a storage provider").created().returns::<StorageProviderResponse>(),
        e("GET /api/v1/storage-providers/{id}", "Get a storage provider").returns::<StorageProviderResponse>(),
        e("PATCH /api/v1/storage-providers/{id}", "Update a storage provider").returns::<StorageProviderResponse>(),
        e("DELETE /api/v1/storage-providers/{id}", "Delete a storage provider").object(DELETED),
        e("POST /api/v1/storage-providers/{id}/test", "Test the connection to a storage provider").object("`{ok, message}`"),
        // ── Snapshots ──
        e("GET /api/v1/snapshots", "List snapshots").returns::<Vec<SnapshotResponse>>(),
        e("POST /api/v1/snapshots", "Take a snapshot now").created().returns::<SnapshotResponse>(),
        e("GET /api/v1/snapshots/config", "Snapshot schedule and retention").returns::<BackupConfigResponse>(),
        e("PUT /api/v1/snapshots/config", "Update the snapshot schedule and retention").returns::<BackupConfigResponse>(),
        e("DELETE /api/v1/snapshots/{id}", "Delete a snapshot").object(DELETED),
        e("POST /api/v1/snapshots/{id}/restore", "Restore the whole server from a snapshot")
            .accepted()
            .object("`{pre_restore_snapshot_id, message}`; a snapshot of the current state is taken first."),
        e("POST /api/v1/snapshots/{id}/restore-app", "Restore one app from a snapshot").object("`{message, …}`"),
        e("POST /api/v1/snapshots/{id}/restore-database", "Restore one database from a snapshot").returns::<RestoreDatabaseResponse>(),
        e("POST /api/v1/snapshots/{id}/restore-new-database", "Restore one database of a snapshot into a new database")
            .created()
            .object("`{message, database}`; the original database is not touched."),
    ]
}
