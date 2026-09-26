// commands/setup.rs — `tond setup-link`: print the first-run setup link again
//
// On first start tond-server writes a one-time setup token to
// <data dir>/setup-token (root only) and deletes it once the admin account
// exists. install.sh prints the link at the end of the install; this command
// prints it again. It reads the file directly, so run it as root on the server.

use std::path::Path;

use anyhow::Result;

use crate::client::TondClient;

/// Name of the token file inside the data directory (next to the socket).
const SETUP_TOKEN_FILE: &str = "setup-token";

/// Build the setup URL for a dashboard domain.
pub fn setup_url(dashboard_domain: &str, token: &str) -> String {
    format!("https://{dashboard_domain}/setup?token={token}")
}

/// Handle `tond setup-link`. `data_dir` is the directory holding the socket.
pub async fn handle_setup_link(client: Result<TondClient>, data_dir: &Path) -> Result<()> {
    let token_path = data_dir.join(SETUP_TOKEN_FILE);
    let token = match std::fs::read_to_string(&token_path) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        Ok(_) => anyhow::bail!("{} is empty", token_path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "No setup is pending: the admin account already exists (or tond-server hasn't \
                 started yet). Sign in to the dashboard instead."
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            anyhow::bail!(
                "Permission denied reading {}. Run this as root: sudo tond setup-link",
                token_path.display()
            );
        }
        Err(e) => return Err(e.into()),
    };

    let domain = match client {
        Ok(client) => client
            .get_settings()
            .await
            .map(|s| s.dashboard_domain)
            .unwrap_or_default(),
        Err(_) => String::new(),
    };

    if domain.trim().is_empty() {
        println!("No dashboard domain is set yet. Point a DNS A record at this server, then run:");
        println!();
        println!("  sudo tond settings set-dashboard-domain dash.example.com");
        println!();
        println!("and open (valid until used):");
        println!();
        println!("  {}", setup_url("dash.example.com", &token));
        println!();
        println!("No domain yet? Use an SSH tunnel from your computer instead:");
        println!();
        println!("  ssh -L 3001:127.0.0.1:3001 root@<this-server>");
        println!("  then open http://localhost:3001/setup?token={token}");
    } else {
        println!("Open this link to create your admin account (valid until used):");
        println!();
        println!("  {}", setup_url(domain.trim(), &token));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_url_points_at_the_https_setup_page() {
        assert_eq!(
            setup_url("dash.example.com", "abc"),
            "https://dash.example.com/setup?token=abc"
        );
    }

    #[tokio::test]
    async fn missing_token_file_explains_that_setup_is_done() {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let err = handle_setup_link(Err(anyhow::anyhow!("no server")), dir.path())
            .await
            .err()
            .unwrap_or_else(|| panic!("expected an error without a token file"));
        assert!(err.to_string().contains("No setup is pending"));
    }

    #[tokio::test]
    async fn token_file_is_printed_without_a_server() {
        let dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        std::fs::write(dir.path().join(SETUP_TOKEN_FILE), "tok\n")
            .unwrap_or_else(|err| panic!("write: {err}"));
        assert!(
            handle_setup_link(Err(anyhow::anyhow!("no server")), dir.path())
                .await
                .is_ok()
        );
    }
}
