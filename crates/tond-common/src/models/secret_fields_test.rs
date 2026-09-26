// secret_fields_test.rs — guard: no API type serializes a secret by accident
//
// Scans the source of every model file for structs that derive `Serialize`
// and fails when one has a field whose name marks it as a secret (password,
// token, secret, private key, env values, connection strings, webhook URLs…)
// unless the field
//   - is `#[serde(skip_serializing)]` (or `skip`),
//   - has a type that cannot carry the value (`bool`, masked `EnvVarEntry`
//     lists), or
//   - is on the allowlist below with the reason it is deliberate (explicit,
//     audited reveal endpoints; values shown once at creation; inbound-only
//     payloads).
//
// Adding a secret to a response therefore needs a conscious allowlist entry.

const MODEL_SOURCES: &[(&str, &str)] = &[
    ("access.rs", include_str!("access.rs")),
    ("alerts.rs", include_str!("alerts.rs")),
    ("api.rs", include_str!("api.rs")),
    ("apps.rs", include_str!("apps.rs")),
    ("auth.rs", include_str!("auth.rs")),
    ("backup.rs", include_str!("backup.rs")),
    ("certificates.rs", include_str!("certificates.rs")),
    ("cron.rs", include_str!("cron.rs")),
    ("databases.rs", include_str!("databases.rs")),
    ("deployments.rs", include_str!("deployments.rs")),
    ("domains.rs", include_str!("domains.rs")),
    ("github.rs", include_str!("github.rs")),
    ("gitlab.rs", include_str!("gitlab.rs")),
    ("monitoring.rs", include_str!("monitoring.rs")),
    ("notifications.rs", include_str!("notifications.rs")),
    ("projects.rs", include_str!("projects.rs")),
    ("registry.rs", include_str!("registry.rs")),
    ("settings.rs", include_str!("settings.rs")),
    ("storage.rs", include_str!("storage.rs")),
    ("templates.rs", include_str!("templates.rs")),
];

/// (struct, field, why it may serialize a secret)
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "WebhookSecretResponse",
        "secret",
        "admin-only audited reveal, and the rotate reply",
    ),
    (
        "DatabaseCredentialsResponse",
        "db_password",
        "admin-only audited reveal",
    ),
    (
        "DatabaseCredentialsResponse",
        "root_password",
        "admin-only audited reveal",
    ),
    (
        "DatabaseCredentialsResponse",
        "connection_string",
        "admin-only audited reveal",
    ),
    (
        "DatabaseCredentialsResponse",
        "public_connection_string",
        "admin-only audited reveal",
    ),
    (
        "DatabaseCredentialsResponse",
        "internal_connection_string",
        "admin-only audited reveal",
    ),
    (
        "CreatedDatabaseResponse",
        "credentials",
        "generated credentials shown once to the creator",
    ),
    (
        "TemplateDeployResponse",
        "credentials",
        "a template's generated first-login credentials, shown once to the deployer",
    ),
    (
        "ManagedDatabaseResponse",
        "custom_external_url",
        "masked with mask_url_password",
    ),
    (
        "ManagedDatabaseResponse",
        "custom_internal_url",
        "masked with mask_url_password",
    ),
    (
        "ManagedDatabaseDetail",
        "connection_string",
        "masked with mask_url_password",
    ),
    (
        "ManagedDatabaseDetail",
        "public_connection_string",
        "masked with mask_url_password",
    ),
    (
        "ManagedDatabaseDetail",
        "internal_connection_string",
        "masked with mask_url_password",
    ),
    (
        "TotpSetupResponse",
        "secret",
        "shown once while the user enrols an authenticator",
    ),
    (
        "UpdateDatabaseCredentials",
        "db_password",
        "inbound payload (client → server)",
    ),
    (
        "UpdateDatabaseCredentials",
        "root_password",
        "inbound payload (client → server)",
    ),
    (
        "CreatedTokenResponse",
        "token",
        "token metadata (TokenResponse: name, scopes, expiry), not the secret",
    ),
    (
        "CreatedTokenResponse",
        "secret",
        "a new API token's secret, shown once to its creator",
    ),
    (
        "PasswordLinkResponse",
        "token",
        "a one-time set-password link, shown once to the admin who made it",
    ),
    (
        "TelegramConfig",
        "bot_token",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "DiscordConfig",
        "webhook_url",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "SlackConfig",
        "webhook_url",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "EmailConfig",
        "smtp_password",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "NtfyConfig",
        "token",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "PushoverConfig",
        "app_token",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "GotifyConfig",
        "app_token",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "WebhookConfig",
        "webhook_url",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "WebhookConfig",
        "signing_secret",
        "internal config; IntegrationResponse strips secret fields",
    ),
    (
        "S3Config",
        "secret_access_key_enc",
        "internal config; StorageProviderResponse strips it",
    ),
];

const SECRET_MARKERS: &[&str] = &[
    "password",
    "secret",
    "token",
    "private_key",
    "api_key",
    "pem",
    "webhook_url",
    "env_vars",
    "connection_string",
    "custom_external_url",
    "custom_internal_url",
    "credential",
];

fn looks_secret(field: &str) -> bool {
    let f = field.to_ascii_lowercase();
    if f.starts_with("has_") || f.ends_with("_id") || f == "saved_secrets" {
        return false;
    }
    f == "env" || SECRET_MARKERS.iter().any(|m| f.contains(m))
}

/// Types that cannot hold a plaintext secret value.
fn safe_type(ty: &str) -> bool {
    let ty = ty.replace(' ', "");
    let ty = ty.trim_end_matches(',');
    ty == "bool" || ty.contains("EnvVarEntry")
}

#[derive(Debug)]
struct Violation {
    file: &'static str,
    strukt: String,
    field: String,
}

fn scan(file: &'static str, source: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut attrs: Vec<String> = Vec::new();
    let mut current: Option<(String, bool)> = None; // (struct name, serializes)
    let mut field_attrs: Vec<String> = Vec::new();

    for raw in source.lines() {
        let line = raw.trim();
        if line.starts_with("#[cfg(test)]") {
            break; // model tests follow; they are not API types
        }
        if let Some((name, serializes)) = current.as_ref() {
            if raw.starts_with('}') {
                current = None;
                field_attrs.clear();
                continue;
            }
            if line.starts_with("#[") {
                field_attrs.push(line.to_string());
                continue;
            }
            if line.starts_with("//") || line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("pub ")
                && let Some((field, ty)) = rest.split_once(':')
            {
                let skipped = field_attrs
                    .iter()
                    .any(|a| a.contains("skip_serializing") || a.contains("serde(skip"));
                let field = field.trim();
                let allowed = ALLOWED.iter().any(|(s, f, _)| s == name && *f == field);
                if *serializes && !skipped && !allowed && looks_secret(field) && !safe_type(ty) {
                    violations.push(Violation {
                        file,
                        strukt: name.clone(),
                        field: field.to_string(),
                    });
                }
            }
            field_attrs.clear();
            continue;
        }

        if line.starts_with("#[") {
            attrs.push(line.to_string());
            continue;
        }
        if line.starts_with("//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("pub struct ")
            && line.ends_with('{')
        {
            let name = rest
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
                .unwrap_or_default()
                .to_string();
            let serializes = attrs
                .iter()
                .any(|a| a.starts_with("#[derive") && a.contains("Serialize"));
            // `*Request` types are inbound payloads (they derive Serialize only so
            // the CLI can send them); the server never returns them.
            let serializes = serializes && !name.ends_with("Request");
            current = Some((name, serializes));
        }
        attrs.clear();
    }
    violations
}

#[test]
fn no_serialized_model_carries_an_unmasked_secret() {
    let violations: Vec<Violation> = MODEL_SOURCES
        .iter()
        .flat_map(|(file, source)| scan(file, source))
        .collect();
    assert!(
        violations.is_empty(),
        "these serializable fields look like secrets; mask them, mark them \
         #[serde(skip_serializing)], or allowlist them with a reason in \
         secret_fields_test.rs:\n{}",
        violations
            .iter()
            .map(|v| format!("  {}: {}.{}", v.file, v.strukt, v.field))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_allowlist_entry_still_exists() {
    for (strukt, field, _) in ALLOWED {
        let found = MODEL_SOURCES.iter().any(|(_, source)| {
            source.contains(&format!("pub struct {strukt} "))
                && source.contains(&format!("pub {field}:"))
        });
        assert!(found, "stale allowlist entry {strukt}.{field}");
    }
}

#[test]
fn the_scanner_catches_a_secret_field() {
    let source = "#[derive(Debug, Serialize)]\npub struct LeakyResponse {\n    pub id: String,\n    pub db_password: Option<String>,\n    #[serde(skip_serializing)]\n    pub api_token: String,\n    pub has_secret: bool,\n    pub env: Vec<crate::EnvVarEntry>,\n}\n\n#[derive(Debug, Deserialize)]\npub struct InboundOnly {\n    pub password: String,\n}\n";
    let found = scan("synthetic.rs", source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].strukt, "LeakyResponse");
    assert_eq!(found[0].field, "db_password");
}
