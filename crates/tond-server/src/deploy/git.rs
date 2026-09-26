// deploy/git.rs — git checkout hardening for builds
//
// Credentials never appear in a URL, argv or .git/config:
//   - HTTPS tokens (GitHub App installation tokens, GitLab PATs, userinfo the
//     user put in source_url) are sent as an `Authorization: Basic` header set
//     through GIT_CONFIG_COUNT/GIT_CONFIG_KEY_n/GIT_CONFIG_VALUE_n. The header
//     is scoped to the repository's origin (`http.<origin>/.extraHeader`), so
//     it is not sent anywhere else. Env vars are readable only by the same uid
//     (root), unlike argv which every user sees in `ps`. Requires git >= 2.31.
//   - `redact_url_credentials` masks `scheme://user:pass@` in anything written
//     to deploy_logs (applied centrally in `log_deploy`) and in error messages.
//   - The checkout's `.git` directory is deleted before the build, so neither
//     history nor config can end up in an image.
//
// Clone invocation: `--` separates options from the URL and target directory,
// GIT_ALLOW_PROTOCOL limits transports to https/http/ssh/git (no `file://` or
// local paths that would copy host files into the build context, no `ext::`),
// and GIT_TERMINAL_PROMPT=0 makes auth failures fail instead of hanging.
//
// SSH deploy keys: host keys are verified against a per-app known_hosts file
// ($TOND_DATA_DIR/keys/<app_id>/known_hosts). It is seeded with the published
// Ed25519 host keys of github.com, gitlab.com and bitbucket.org, so those hosts
// are pinned from the first clone; any other host (self-hosted Gitea/GitLab)
// is trusted on first use (`StrictHostKeyChecking=accept-new`) and pinned from
// then on — a changed key makes the clone fail. This replaces
// `StrictHostKeyChecking=no`, which accepted any key on every clone.

use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

/// Username/password (token) for HTTPS git access.
#[derive(Clone)]
pub struct GitHttpAuth {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for GitHttpAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHttpAuth")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

/// Transports git may use for app sources.
pub const GIT_ALLOW_PROTOCOL: &str = "https:http:ssh:git";

/// Published Ed25519 host keys of the big hosted providers (verified against
/// the providers' documentation and `ssh-keyscan`).
pub const PINNED_KNOWN_HOSTS: &[&str] = &[
    "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
    "gitlab.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAfuCHKVTjquxvt6CM6tdG4SLp1Btn/nOeHHE5UOzRdf",
    "bitbucket.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIazEu89wgQZ4bqs3d63QSMzYVa0MuJ2e2gKTKqu+UUO",
];

/// Mask credentials in every `scheme://user[:pass]@host` occurrence:
/// `https://oauth2:tok@gitlab.com/x` → `https://***@gitlab.com/x`.
pub fn redact_url_credentials(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find("://") {
        let (before, after) = rest.split_at(pos + 3);
        out.push_str(before);
        // The authority ends at the first path/query/fragment delimiter,
        // whitespace or quote.
        let end = after
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | '"' | '\'' | '<' | '>') || c.is_whitespace()
            })
            .unwrap_or(after.len());
        let authority = &after[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Split userinfo off an http(s) URL: `https://u:p@host/x` → (`https://host/x`,
/// Some(u, p)). Other URLs (ssh, scp-style) are returned unchanged.
pub fn split_url_credentials(url: &str) -> (String, Option<GitHttpAuth>) {
    let url = url.trim();
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return (url.to_string(), None);
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return (url.to_string(), None);
    }
    if parsed.username().is_empty() && parsed.password().is_none() {
        return (url.to_string(), None);
    }
    let decode = |s: &str| {
        urlencoding::decode(s)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| s.to_string())
    };
    let auth = GitHttpAuth {
        username: decode(parsed.username()),
        password: decode(parsed.password().unwrap_or("")),
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    (parsed.to_string(), Some(auth))
}

/// `scheme://host[:port]/` of an http(s) URL, used to scope the auth header.
fn http_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{}://{}:{}/", parsed.scheme(), host, port),
        None => format!("{}://{}/", parsed.scheme(), host),
    })
}

/// Environment that makes git send `auth` as a Basic Authorization header to
/// the origin of `url` only. Empty for non-http(s) URLs.
pub fn git_auth_env(url: &str, auth: &GitHttpAuth) -> Vec<(String, String)> {
    let Some(origin) = http_origin(url) else {
        return Vec::new();
    };
    let basic = B64.encode(format!("{}:{}", auth.username, auth.password));
    vec![
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        (
            "GIT_CONFIG_KEY_0".to_string(),
            format!("http.{origin}.extraHeader"),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_string(),
            format!("Authorization: Basic {basic}"),
        ),
    ]
}

/// Arguments for a shallow single-branch clone. `--` keeps a URL or path that
/// starts with `-` from being parsed as an option.
pub fn clone_args(branch: &str, url: &str, target_dir: &str) -> Vec<String> {
    vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--single-branch".to_string(),
        format!("--branch={branch}"),
        "--".to_string(),
        url.to_string(),
        target_dir.to_string(),
    ]
}

/// Per-app known_hosts path, next to the deploy key.
pub fn known_hosts_path(data_dir: &str, app_id: &str) -> PathBuf {
    Path::new(data_dir)
        .join("keys")
        .join(app_id)
        .join("known_hosts")
}

/// Make sure the per-app known_hosts file exists and contains the pinned
/// provider keys. Entries learned by accept-new are kept.
pub async fn ensure_known_hosts(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let existing = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let missing: Vec<&str> = PINNED_KNOWN_HOSTS
        .iter()
        .copied()
        .filter(|line| !existing.lines().any(|l| l.trim() == *line))
        .collect();
    if missing.is_empty() && !existing.is_empty() {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    for line in missing {
        content.push_str(line);
        content.push('\n');
    }
    tokio::fs::write(path, content).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

/// GIT_SSH_COMMAND for cloning with a deploy key, verifying host keys against
/// the per-app known_hosts file.
pub fn ssh_command(key_path: &str, known_hosts: &Path) -> String {
    format!(
        "ssh -i '{}' -o IdentitiesOnly=yes -o BatchMode=yes \
         -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile='{}' \
         -o GlobalKnownHostsFile=/dev/null",
        key_path,
        known_hosts.display()
    )
}

/// The commit a checkout is at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
}

/// `git log` arguments whose output [`parse_commit_log`] reads.
pub const COMMIT_LOG_ARGS: [&str; 4] = ["log", "-1", "--no-color", "--format=%H%x1f%an%x1f%s"];

/// Parse `git log -1 --format=%H%x1f%an%x1f%s`.
pub fn parse_commit_log(output: &str) -> Option<CommitInfo> {
    let line = output.lines().next()?.trim_end();
    let mut parts = line.splitn(3, '\u{1f}');
    let sha = parts.next()?.trim().to_string();
    if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let author = parts.next().unwrap_or_default().trim().to_string();
    let message: String = parts
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(200)
        .collect();
    Some(CommitInfo {
        sha,
        message: redact_url_credentials(&message),
        author,
    })
}

/// `git ls-remote` arguments listing the default branch and all branches.
pub fn ls_remote_args(url: &str) -> Vec<String> {
    vec![
        "ls-remote".to_string(),
        "--symref".to_string(),
        "--".to_string(),
        url.to_string(),
        "HEAD".to_string(),
        "refs/heads/*".to_string(),
    ]
}

/// Branches of a remote: the default branch (from `HEAD`) and every branch,
/// default first, the rest sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranches {
    pub default_branch: Option<String>,
    pub branches: Vec<String>,
}

/// Parse `git ls-remote --symref <url> HEAD refs/heads/*`.
pub fn parse_ls_remote(output: &str) -> RemoteBranches {
    let mut default_branch = None;
    let mut branches = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("ref: ") {
            // "ref: refs/heads/main\tHEAD"
            let mut fields = rest.split('\t');
            if let (Some(target), Some("HEAD")) = (fields.next(), fields.next()) {
                default_branch = target.strip_prefix("refs/heads/").map(str::to_string);
            }
            continue;
        }
        if let Some(name) = line
            .split('\t')
            .nth(1)
            .and_then(|r| r.strip_prefix("refs/heads/"))
        {
            branches.push(name.to_string());
        }
    }
    branches.sort();
    branches.dedup();
    if let Some(default) = &default_branch
        && let Some(pos) = branches.iter().position(|b| b == default)
    {
        let d = branches.remove(pos);
        branches.insert(0, d);
    }
    RemoteBranches {
        default_branch,
        branches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commit_log_output() {
        let c = parse_commit_log(
            "0123456789abcdef0123456789abcdef01234567\u{1f}Ada Lovelace\u{1f}Fix login redirect\n",
        )
        .unwrap_or_else(|| panic!("commit should parse"));
        assert_eq!(c.sha, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(c.author, "Ada Lovelace");
        assert_eq!(c.message, "Fix login redirect");
        assert!(parse_commit_log("").is_none());
        assert!(parse_commit_log("fatal: not a git repository").is_none());
    }

    #[test]
    fn parses_ls_remote_with_default_branch_first() {
        let out = "ref: refs/heads/trunk\tHEAD\n\
                   aaa\tHEAD\n\
                   bbb\trefs/heads/feature/x\n\
                   ccc\trefs/heads/trunk\n\
                   ddd\trefs/heads/alpha\n";
        let r = parse_ls_remote(out);
        assert_eq!(r.default_branch.as_deref(), Some("trunk"));
        assert_eq!(r.branches, vec!["trunk", "alpha", "feature/x"]);
        let r = parse_ls_remote("bbb\trefs/heads/main\n");
        assert_eq!(r.default_branch, None);
        assert_eq!(r.branches, vec!["main"]);
        let args = ls_remote_args("--upload-pack=x");
        let dd = args.iter().position(|a| a == "--").unwrap_or(usize::MAX);
        assert_eq!(
            args.get(dd + 1).map(String::as_str),
            Some("--upload-pack=x")
        );
    }

    #[test]
    fn redact_url_credentials_masks_userinfo_everywhere() {
        assert_eq!(
            redact_url_credentials("Cloning https://oauth2:glpat-SECRET@gitlab.com/g/r.git (main)"),
            "Cloning https://***@gitlab.com/g/r.git (main)"
        );
        assert_eq!(
            redact_url_credentials(
                "fatal: https://x-access-token:ghs_abc@github.com/o/r and http://u@h:8080/x"
            ),
            "fatal: https://***@github.com/o/r and http://***@h:8080/x"
        );
        assert_eq!(
            redact_url_credentials("'https://a:b@host'"),
            "'https://***@host'"
        );
        // A password containing '@' is fully masked (the last '@' ends userinfo).
        assert_eq!(
            redact_url_credentials("https://u:p@ss@host/x"),
            "https://***@host/x"
        );
        // Nothing to redact.
        for s in [
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "Building with Dockerfile",
            "email me: a@b.c",
            "trailing ://",
        ] {
            assert_eq!(redact_url_credentials(s), s);
        }
    }

    #[test]
    fn split_url_credentials_moves_userinfo_out_of_http_urls() {
        let (url, auth) = split_url_credentials("https://user:p%40ss@gitlab.example.com/g/r.git");
        assert_eq!(url, "https://gitlab.example.com/g/r.git");
        let auth = auth.unwrap_or_else(|| panic!("credentials should be extracted"));
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "p@ss");
        assert!(
            !format!("{auth:?}").contains("p@ss"),
            "Debug must not print the secret"
        );

        assert!(
            split_url_credentials("https://github.com/o/r.git")
                .1
                .is_none()
        );
        let (ssh, none) = split_url_credentials("git@github.com:o/r.git");
        assert_eq!(ssh, "git@github.com:o/r.git");
        assert!(none.is_none());
    }

    #[test]
    fn git_auth_env_scopes_a_basic_header_to_the_origin() {
        let auth = GitHttpAuth {
            username: "x-access-token".to_string(),
            password: "ghs_token".to_string(),
        };
        let env = git_auth_env("https://github.com/o/r.git", &auth);
        assert_eq!(
            env,
            vec![
                ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
                (
                    "GIT_CONFIG_KEY_0".to_string(),
                    "http.https://github.com/.extraHeader".to_string()
                ),
                (
                    "GIT_CONFIG_VALUE_0".to_string(),
                    format!(
                        "Authorization: Basic {}",
                        B64.encode("x-access-token:ghs_token")
                    )
                ),
            ]
        );
        assert_eq!(
            git_auth_env("https://git.example.com:8443/o/r.git", &auth)[1].1,
            "http.https://git.example.com:8443/.extraHeader"
        );
        assert!(git_auth_env("git@github.com:o/r.git", &auth).is_empty());
    }

    #[test]
    fn clone_args_end_options_before_url_and_target() {
        let args = clone_args("main", "--upload-pack=touch /tmp/pwned", "/work");
        let dashdash = args
            .iter()
            .position(|a| a == "--")
            .unwrap_or_else(|| panic!("clone args must contain --"));
        assert_eq!(args[dashdash + 1], "--upload-pack=touch /tmp/pwned");
        assert_eq!(args[dashdash + 2], "/work");
        assert!(args.contains(&"--branch=main".to_string()));
    }

    #[test]
    fn ssh_command_verifies_host_keys() {
        let cmd = ssh_command("/d/keys/a/id_ed25519", Path::new("/d/keys/a/known_hosts"));
        assert!(cmd.contains("StrictHostKeyChecking=accept-new"));
        assert!(cmd.contains("UserKnownHostsFile='/d/keys/a/known_hosts'"));
        assert!(!cmd.contains("StrictHostKeyChecking=no"));
    }

    #[tokio::test]
    async fn ensure_known_hosts_seeds_pins_and_keeps_learned_hosts() {
        let dir = tempfile::TempDir::new()
            .unwrap_or_else(|err| panic!("failed to create temp dir: {err}"));
        let path = known_hosts_path(&dir.path().to_string_lossy(), "app-1");

        ensure_known_hosts(&path)
            .await
            .unwrap_or_else(|err| panic!("seeding known_hosts failed: {err}"));
        let seeded = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("known_hosts should exist: {err}"));
        for line in PINNED_KNOWN_HOSTS {
            assert!(seeded.lines().any(|l| l == *line));
        }

        // A host learned via accept-new survives; pins are not duplicated.
        std::fs::write(
            &path,
            format!("{seeded}git.example.com ssh-ed25519 AAAAlearned\n"),
        )
        .unwrap_or_else(|err| panic!("failed to append host: {err}"));
        ensure_known_hosts(&path)
            .await
            .unwrap_or_else(|err| panic!("re-seeding known_hosts failed: {err}"));
        let after = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("known_hosts should exist: {err}"));
        assert!(after.contains("git.example.com ssh-ed25519 AAAAlearned"));
        assert_eq!(after.lines().count(), PINNED_KNOWN_HOSTS.len() + 1);
    }
}
