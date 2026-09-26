// log_scrub.rs — remove secrets from log lines before anyone sees them
//
// Every line written with `deploy::log_deploy` passes through `scrub_for_app`
// before it is stored, and so does every container log line the API returns
// or streams (handlers/logs.rs, GET /api/v1/apps/{id}/logs); database container
// logs get the same treatment without app secrets (`scrub(line, &[])`):
//
//   1. credentials in URLs (`https://user:token@host` → `https://***@host`);
//   2. the app's own secret values — env vars and preview overrides of at
//      least `MIN_SECRET_LEN` characters (and each line of a multi-line value,
//      e.g. a PEM key), plus its webhook secret;
//   3. common key patterns: AWS access key ids, GitHub/GitLab/Slack/Tond
//      tokens, and private key blocks.
//
// Decrypting the env for every build line would be wasteful, so an app's
// secret values are cached for `CACHE_TTL`; env edits call `forget_app`.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use crate::deploy::redact_url_credentials;
use crate::secret_key::{self, Purpose};

/// Values shorter than this are not scrubbed (too likely to match normal text).
pub const MIN_SECRET_LEN: usize = 8;
const CACHE_TTL: Duration = Duration::from_secs(60);
const REDACTED: &str = "[redacted]";

type SecretCache = HashMap<String, (Instant, Arc<Vec<String>>)>;

static CACHE: LazyLock<Mutex<SecretCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Drop the cached secret values of an app (call after its env changes).
pub fn forget_app(app_id: &str) {
    if let Ok(mut cache) = CACHE.lock() {
        cache.remove(app_id);
    }
}

/// Scrub `message` for the app `app_id`.
pub async fn scrub_for_app(db: &SqlitePool, app_id: &str, message: &str) -> String {
    let secrets = app_secrets(db, app_id).await;
    scrub(message, &secrets)
}

/// Scrub many lines of one app with a single secret lookup.
pub async fn scrub_lines_for_app(db: &SqlitePool, app_id: &str, lines: Vec<String>) -> Vec<String> {
    let secrets = app_secrets(db, app_id).await;
    lines.into_iter().map(|l| scrub(&l, &secrets)).collect()
}

/// Scrub `message`: URL credentials, the given secret values, key patterns.
pub fn scrub(message: &str, secrets: &[String]) -> String {
    let mut out = redact_url_credentials(message);
    // `secrets` is sorted longest first, so a value that contains another is
    // replaced whole.
    for secret in secrets {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    scrub_patterns(&out)
}

async fn app_secrets(db: &SqlitePool, app_id: &str) -> Arc<Vec<String>> {
    if let Ok(cache) = CACHE.lock()
        && let Some((loaded, secrets)) = cache.get(app_id)
        && loaded.elapsed() < CACHE_TTL
    {
        return secrets.clone();
    }

    let row: Option<(Option<String>, String, Option<String>)> =
        sqlx::query_as("SELECT env_vars, preview_env_vars, webhook_secret FROM apps WHERE id = ?")
            .bind(app_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let mut values = Vec::new();
    if let Some((env, preview, webhook)) = row {
        for map in [
            secret_key::open_map(Purpose::AppEnv, env.as_deref()),
            secret_key::open_map(Purpose::AppPreviewEnv, Some(&preview)),
        ]
        .into_iter()
        .flatten()
        {
            values.extend(map.into_values());
        }
        if let Ok(Some(secret)) =
            secret_key::decrypt_opt(Purpose::AppWebhookSecret, webhook.as_deref())
        {
            values.push(secret);
        }
    }
    let secrets = Arc::new(secret_needles(values));
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(app_id.to_string(), (Instant::now(), secrets.clone()));
    }
    secrets
}

/// The strings to search for: each value of at least `MIN_SECRET_LEN`
/// characters and each long-enough line of a multi-line value, longest first.
pub fn secret_needles(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut needles = Vec::new();
    for value in values {
        if value.contains('\n') {
            for line in value.lines() {
                let line = line.trim();
                if line.len() >= MIN_SECRET_LEN {
                    needles.push(line.to_string());
                }
            }
        }
        let trimmed = value.trim();
        if trimmed.len() >= MIN_SECRET_LEN {
            needles.push(trimmed.to_string());
        }
    }
    needles.sort_by_key(|s| std::cmp::Reverse(s.len()));
    needles.dedup();
    needles
}

// ── Patterns ─────────────────────────────────────────────────────────────────

/// Token prefixes and the minimum length of the random part after them.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("github_pat_", 20),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("glpat-", 16),
    ("tond_pat_", 16),
    ("xoxb-", 16),
    ("xoxp-", 16),
    ("sk_live_", 16),
];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Replace key-shaped substrings: private key blocks, known token prefixes and
/// AWS access key ids.
pub fn scrub_patterns(input: &str) -> String {
    let mut out = scrub_private_keys(input);
    for (prefix, min_len) in TOKEN_PREFIXES {
        out = scrub_prefixed(&out, prefix, *min_len);
    }
    scrub_aws_access_keys(&out)
}

fn scrub_private_keys(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("-----BEGIN ") {
        let header_end = rest[start..].find("-----\n").map(|i| start + i);
        let header = &rest[start..header_end.unwrap_or(rest.len()).min(start + 80)];
        if !header.contains("PRIVATE KEY") {
            out.push_str(&rest[..start + 11]);
            rest = &rest[start + 11..];
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str("[redacted private key]");
        // Skip to the end of the matching END line, or the end of the text
        // (a log line may hold only the start of a key).
        let after = &rest[start..];
        match after.find("-----END ") {
            Some(end) => {
                let tail = &after[end + 9..];
                let close = tail.find("-----").map(|i| i + 5).unwrap_or(tail.len());
                rest = &tail[close..];
            }
            None => rest = "",
        }
    }
    out.push_str(rest);
    out
}

fn scrub_prefixed(input: &str, prefix: &str, min_len: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find(prefix) {
        let at_boundary = rest[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !is_token_char(c));
        let tail = &rest[pos + prefix.len()..];
        let len = tail.find(|c: char| !is_token_char(c)).unwrap_or(tail.len());
        out.push_str(&rest[..pos]);
        out.push_str(prefix);
        if at_boundary && len >= min_len {
            out.push_str("***");
        } else {
            out.push_str(&tail[..len]);
        }
        rest = &tail[len..];
    }
    out.push_str(rest);
    out
}

fn scrub_aws_access_keys(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut copied = 0;
    while i + 20 <= bytes.len() {
        let window = &bytes[i..i + 20];
        let prefixed = window.starts_with(b"AKIA") || window.starts_with(b"ASIA");
        let body_ok = window[4..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let after_ok = bytes.get(i + 20).is_none_or(|b| !b.is_ascii_alphanumeric());
        if prefixed && body_ok && before_ok && after_ok {
            out.push_str(&input[copied..i]);
            out.push_str(&input[i..i + 4]);
            out.push_str("***");
            i += 20;
            copied = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&input[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn container_lines_lose_the_apps_secret_values() {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|e| panic!("pool: {e}"));
        crate::db::migrate(&db)
            .await
            .unwrap_or_else(|e| panic!("migrate: {e}"));
        let mut env = std::collections::BTreeMap::new();
        env.insert("DB_PASSWORD".to_string(), "s3cret-value-123".to_string());
        let sealed = crate::secrets::seal_app_env(&env).unwrap_or_else(|e| panic!("seal: {e}"));
        sqlx::query(
            "INSERT INTO apps (id, name, env_vars, created_at, updated_at)
             VALUES ('scrub-app', 'web', ?, 'x', 'x')",
        )
        .bind(sealed)
        .execute(&db)
        .await
        .unwrap_or_else(|e| panic!("app: {e}"));
        forget_app("scrub-app");
        let out = scrub_lines_for_app(
            &db,
            "scrub-app",
            vec![
                "connecting with s3cret-value-123".to_string(),
                "GET https://admin:pw@api.internal/x".to_string(),
            ],
        )
        .await;
        assert_eq!(
            out,
            vec![
                "connecting with [redacted]".to_string(),
                "GET https://***@api.internal/x".to_string()
            ]
        );
    }

    #[test]
    fn scrubs_env_values_and_lines_of_multiline_values() {
        let secrets = secret_needles([
            "hunter2hunter2".to_string(),
            "short".to_string(),
            "line-one-secret\nline-two-secret".to_string(),
        ]);
        let out = scrub(
            "password=hunter2hunter2 short line-two-secret done",
            &secrets,
        );
        assert_eq!(out, "password=[redacted] short [redacted] done");
    }

    #[test]
    fn scrubs_key_patterns() {
        let gh = format!("ghp_{}", "a".repeat(36));
        let pat = format!("github_pat_{}", "B1_".repeat(10));
        let out = scrub(
            &format!("token {gh} and {pat} and tond_pat_0123456789abcdef0123 AKIAIOSFODNN7EXAMPLE"),
            &[],
        );
        assert_eq!(
            out,
            "token ghp_*** and github_pat_*** and tond_pat_*** AKIA***"
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let line = "Step 3/7 : RUN npm ci --prefer-offline (ghp_ docs) AKIA-short";
        assert_eq!(scrub(line, &[]), line);
    }

    #[test]
    fn scrubs_private_key_blocks_and_their_first_line() {
        let block = "key: -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY----- ok";
        assert_eq!(scrub(block, &[]), "key: [redacted private key] ok");
        assert_eq!(
            scrub("-----BEGIN RSA PRIVATE KEY-----", &[]),
            "[redacted private key]"
        );
        let public = "-----BEGIN PUBLIC KEY-----";
        assert_eq!(scrub(public, &[]), public);
    }

    #[test]
    fn keeps_masking_url_credentials() {
        assert_eq!(
            scrub("clone https://oauth2:glpat-xyz@gitlab.com/a/b", &[]),
            "clone https://***@gitlab.com/a/b"
        );
    }
}
