// db_access.rs — who may reach a public database, and over what
//
// ## Allowlist (kernel-enforced)
//
// A public database publishes its port on 0.0.0.0 (IPv4 only). Docker
// forwards that port with a DNAT rule, so the traffic never reaches a host
// socket that could check the client, and UFW's INPUT rules don't see it.
// Docker leaves one hook for this: the DOCKER-USER chain of the filter
// table, which every forwarded packet passes before Docker's own rules.
//
// tond-server owns a chain TOND-DB and a jump to it from DOCKER-USER. For
// every public database with an allowlist it holds:
//
//   -p tcp -m conntrack --ctstate DNAT --ctdir ORIGINAL --ctorigdstport <port> -s <cidr> -j RETURN
//   ...one per allowed address...
//   -p tcp -m conntrack --ctstate DNAT --ctdir ORIGINAL --ctorigdstport <port> -j DROP
//
// Matching on conntrack's *original* destination port (the published host
// port, before DNAT) and state DNAT means only traffic that arrived through
// the published port is filtered: apps on the database's Docker networks
// talk to the container directly and are never matched, nor are replies
// (ORIGINAL direction only). The chain is replaced atomically with
// `iptables-restore --noflush`, so there is no window where it is empty.
// Rules live in the kernel and vanish on reboot; the "db-access" loop puts
// them back at startup, whenever a database changes, and re-checks them
// hourly while any exist (Docker or a firewall reload may drop the jump).
//
// While a public database runs a major upgrade (`upgrade.status` is
// `running`, db_upgrade.rs) its port gets the DROP rule alone, allowlist or
// not: nothing reaches it while the dump runs. The rule follows the stored
// state, so it lifts when the upgrade waits for confirmation or fails, and
// after a crash once `recover_interrupted` marks it failed.
//
// Where iptables or the DOCKER-USER chain is missing (nftables-only hosts,
// rootless Docker, a dev machine) an allowlist can't be enforced: saving one
// is refused with an explanation, and a database that has one says so.
//
// Why not a TCP proxy in tond-server? It would see the real client address
// too, but every byte of database traffic would pass through this process,
// a restart of tond-server would cut every connection, and its memory
// budget is small. The kernel does this for free.
//
// ## TLS
//
// PostgreSQL, MySQL and MariaDB can serve TLS with a certificate mounted
// read-only at /etc/tond/tls: the Let's Encrypt certificate of the
// dashboard domain when there is one (the public connection string uses
// that name, so clients can verify it), otherwise a self-signed one. The
// container's command copies the key to a private file owned by the engine
// user (the engines refuse group/world-readable keys) and starts the server
// with TLS on. Connection strings ask for `sslmode=require`. TLS is
// offered, not forced: apps on the internal network keep connecting without
// it. When the dashboard certificate is renewed the copy is refreshed and
// the database restarted.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use tond_common::{ManagedDatabase, supports_tls};

use crate::runtime::changes::{Table, Watch};
use crate::runtime::loops;
use crate::state::AppState;

/// Chain holding Tond's database allowlist rules.
pub const CHAIN: &str = "TOND-DB";
/// Where the TLS certificate is mounted in a database container.
pub const TLS_MOUNT: &str = "/etc/tond/tls";
/// Most addresses one database may allow.
pub const MAX_ALLOWLIST: usize = 64;

// ── Allowlist ──────────────────────────────────────────────────────────────

/// Parse, validate and canonicalise allowlist entries: IPv4 addresses or
/// CIDR ranges ("203.0.113.7", "198.51.100.0/24"). Duplicates are dropped.
pub fn normalize_allowlist(entries: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in entries {
        let entry = raw.trim();
        if entry.is_empty() {
            continue;
        }
        let (addr, prefix) = match entry.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (entry, None),
        };
        let ip: IpAddr = addr.parse().map_err(|_| {
            format!("'{entry}' isn't an IP address or a CIDR range like 203.0.113.0/24")
        })?;
        let IpAddr::V4(v4) = ip else {
            return Err(format!(
                "'{entry}' is IPv6. Public databases are published on IPv4 only; allow IPv4 addresses."
            ));
        };
        let prefix: u8 = match prefix {
            Some(p) => p
                .parse()
                .ok()
                .filter(|p| *p <= 32)
                .ok_or_else(|| format!("'{entry}' has an invalid prefix length (0–32)"))?,
            None => 32,
        };
        if prefix == 0 {
            return Err(
                "0.0.0.0/0 allows every address; leave the allowlist empty instead".to_string(),
            );
        }
        // Canonical network address: 198.51.100.7/24 → 198.51.100.0/24.
        let mask = u32::MAX << (32 - u32::from(prefix));
        let net = std::net::Ipv4Addr::from(u32::from(v4) & mask);
        let canonical = if prefix == 32 {
            net.to_string()
        } else {
            format!("{net}/{prefix}")
        };
        if !out.contains(&canonical) {
            out.push(canonical);
        }
    }
    if out.len() > MAX_ALLOWLIST {
        return Err(format!(
            "At most {MAX_ALLOWLIST} addresses per database; use CIDR ranges to group them"
        ));
    }
    Ok(out)
}

/// The iptables-restore input that replaces the TOND-DB chain with rules for
/// `(host_port, allowed)` pairs. Declaring the chain flushes it atomically.
pub fn ruleset(rules: &BTreeMap<i64, Vec<String>>) -> String {
    let mut out = String::from("*filter\n");
    out.push_str(&format!(":{CHAIN} - [0:0]\n"));
    for (port, allowed) in rules {
        let m =
            format!("-p tcp -m conntrack --ctstate DNAT --ctdir ORIGINAL --ctorigdstport {port}");
        for cidr in allowed {
            out.push_str(&format!("-A {CHAIN} {m} -s {cidr} -j RETURN\n"));
        }
        out.push_str(&format!("-A {CHAIN} {m} -j DROP\n"));
    }
    out.push_str("COMMIT\n");
    out
}

/// Whether a database's public port is closed for a running major upgrade.
pub fn is_sealed(db: &ManagedDatabase) -> bool {
    db.upgrade_state().is_some_and(|u| u.status == "running")
}

/// The rules the databases call for, by host port: every public database
/// with an allowlist, and every public database in the middle of a major
/// upgrade with no allowed address at all (the port is closed while the
/// dump runs, so nothing is written that the copy would miss).
pub fn desired_rules(dbs: &[ManagedDatabase]) -> BTreeMap<i64, Vec<String>> {
    dbs.iter()
        .filter(|d| d.public_access)
        .filter_map(|d| {
            if is_sealed(d) {
                return d.host_port.map(|p| (p, Vec::new()));
            }
            let allowed = d.allowlist_entries();
            (!allowed.is_empty())
                .then_some(d.host_port)
                .flatten()
                .map(|p| (p, allowed))
        })
        .collect()
}

/// What the server firewall does for databases right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FirewallStatus {
    /// iptables with a DOCKER-USER chain is usable
    pub available: bool,
    /// Host ports whose allowlist is currently loaded
    pub applied_ports: Vec<i64>,
    /// Why the firewall can't be used, or the last apply failed
    pub error: Option<String>,
}

static STATUS: LazyLock<RwLock<FirewallStatus>> =
    LazyLock::new(|| RwLock::new(FirewallStatus::default()));
/// The rules last loaded (skip reloading identical rules).
/// Allowlist rules by host port.
type Rules = BTreeMap<i64, Vec<String>>;

static LOADED: LazyLock<tokio::sync::Mutex<Option<Rules>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

pub fn firewall_status() -> FirewallStatus {
    STATUS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_status(status: FirewallStatus) {
    *STATUS.write().unwrap_or_else(|e| e.into_inner()) = status;
}

/// Why a database's network access deserves attention, if it does.
pub fn access_warning(db: &ManagedDatabase, fw: &FirewallStatus) -> Option<String> {
    if !db.public_access {
        return None;
    }
    if is_sealed(db) && fw.applied_ports.contains(&db.host_port.unwrap_or(-1)) {
        // Closed to everyone until the upgrade finishes.
        return None;
    }
    let allowlist = db.allowlist_entries();
    if allowlist.is_empty() {
        return Some(
            "Open to every address on the internet. Add the addresses that need to connect \
             to the allowlist, or make the database private and use an SSH tunnel."
                .to_string(),
        );
    }
    if !fw.available {
        return Some(format!(
            "The allowlist isn't enforced: {}. Until it is, every address can connect.",
            fw.error
                .as_deref()
                .unwrap_or("the server firewall (iptables DOCKER-USER chain) isn't available")
        ));
    }
    if !fw.applied_ports.contains(&db.host_port.unwrap_or(-1)) {
        return Some(
            "The allowlist is saved but not loaded into the firewall yet; check the server logs."
                .to_string(),
        );
    }
    None
}

#[cfg(test)]
static TEST_FIREWALL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Make `require_firewall` succeed in tests.
#[cfg(test)]
pub fn set_test_firewall(available: bool) {
    TEST_FIREWALL.store(available, std::sync::atomic::Ordering::Relaxed);
}

/// Refuse to store an allowlist this server can't enforce.
pub async fn require_firewall() -> Result<(), String> {
    firewall_usable().await.map_err(|e| {
        format!(
            "This server can't enforce an allowlist ({e}). Keep the database private and \
             connect through an SSH tunnel instead."
        )
    })
}

/// Whether iptables with a DOCKER-USER chain can be used here; the reason
/// when it can't. Shared by every feature that writes rules there.
pub async fn firewall_usable() -> Result<(), String> {
    #[cfg(test)]
    if TEST_FIREWALL.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    probe_firewall().await.map_err(|e| format!("{e:#}"))
}

/// Whether iptables and Docker's DOCKER-USER chain are usable.
async fn probe_firewall() -> Result<()> {
    let out = crate::process::command("iptables")
        .args(["-w", "5", "-n", "-L", "DOCKER-USER"])
        .output()
        .await
        .context("iptables isn't installed")?;
    anyhow::ensure!(
        out.status.success(),
        "the iptables DOCKER-USER chain doesn't exist ({})",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Load `rules` into the kernel (and make sure DOCKER-USER jumps to them).
async fn apply(rules: &BTreeMap<i64, Vec<String>>) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    probe_firewall().await?;
    let mut child = crate::process::command("iptables-restore")
        .args(["-w", "5", "--noflush"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("iptables-restore isn't installed")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(ruleset(rules).as_bytes()).await?;
    }
    let out = child.wait_with_output().await?;
    anyhow::ensure!(
        out.status.success(),
        "iptables-restore failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    // The jump, first in DOCKER-USER (an empty TOND-DB chain is harmless).
    let jumps = crate::process::command("iptables")
        .args(["-w", "5", "-C", "DOCKER-USER", "-j", CHAIN])
        .status()
        .await?;
    if !jumps.success() {
        let inserted = crate::process::command("iptables")
            .args(["-w", "5", "-I", "DOCKER-USER", "1", "-j", CHAIN])
            .output()
            .await?;
        anyhow::ensure!(
            inserted.status.success(),
            "couldn't hook {CHAIN} into DOCKER-USER: {}",
            String::from_utf8_lossy(&inserted.stderr).trim()
        );
    }
    Ok(())
}

/// Load the allowlist rules the databases call for, unless they are loaded
/// already. `force` reloads anyway (the periodic re-check).
async fn reconcile(state: &AppState, force: bool) -> Result<()> {
    let dbs: Vec<ManagedDatabase> =
        sqlx::query_as("SELECT * FROM databases WHERE public_access = 1")
            .fetch_all(&state.db)
            .await?;
    let rules = desired_rules(&dbs);
    let mut loaded = LOADED.lock().await;
    if !force && loaded.as_ref() == Some(&rules) {
        return Ok(());
    }
    if rules.is_empty() && loaded.is_none() {
        // Nothing to enforce and nothing of ours loaded: don't touch the
        // firewall (a stale chain from a previous run is emptied below once
        // any rule is loaded, and holds only what we put there).
        *loaded = Some(rules);
        let available = probe_firewall().await;
        set_status(FirewallStatus {
            available: available.is_ok(),
            applied_ports: Vec::new(),
            error: available.err().map(|e| format!("{e:#}")),
        });
        return Ok(());
    }
    match apply(&rules).await {
        Ok(()) => {
            set_status(FirewallStatus {
                available: true,
                applied_ports: rules.keys().copied().collect(),
                error: None,
            });
            *loaded = Some(rules);
            Ok(())
        }
        Err(e) => {
            set_status(FirewallStatus {
                available: probe_firewall().await.is_ok(),
                applied_ports: Vec::new(),
                error: Some(format!("{e:#}")),
            });
            *loaded = None;
            Err(e)
        }
    }
}

/// Apply allowlist changes right away (after a database update).
pub async fn reconcile_now(state: &AppState) {
    if let Err(e) = reconcile(state, false).await {
        tracing::warn!("db-access: couldn't load the database allowlists: {e:#}");
    }
}

/// Close a public database's published port for a major upgrade (its
/// `upgrade` state must already say `running`) and confirm the DROP rule is
/// loaded. The seal is derived from that state, so it lifts by itself once
/// the upgrade is confirmed-pending or failed, including the `failed` that
/// `recover_interrupted` records after a crash; the kernel keeps the rule
/// while tond-server is down, so the port stays closed in between.
pub async fn seal_for_upgrade(state: &AppState, db: &ManagedDatabase) -> Result<()> {
    let Some(port) = db.host_port.filter(|_| db.public_access) else {
        return Ok(());
    };
    reconcile(state, false).await?;
    anyhow::ensure!(
        firewall_status().applied_ports.contains(&port),
        "couldn't close public port {port} in the server firewall"
    );
    Ok(())
}

/// The "db-access" loop: keeps the allowlist rules loaded and TLS
/// certificates fresh. Wakes on database and certificate changes; re-checks
/// hourly only while allowlists exist.
pub async fn run_access_loop(state: AppState) {
    const RECHECK: Duration = Duration::from_secs(3600);
    let lp = loops::register("db-access");
    let mut watch = Watch::new(&[Table::Databases, Table::Certificates]);
    let mut force = true;
    loop {
        let run = lp.run();
        let result = async {
            reconcile(&state, force).await?;
            refresh_tls_certificates(&state).await
        }
        .await;
        match result {
            Ok(()) => drop(run),
            Err(e) => {
                tracing::warn!("db-access: {e:#}");
                run.fail(&format!("{e:#}"));
            }
        }
        let timeout = (!firewall_status().applied_ports.is_empty()).then_some(RECHECK);
        force = lp.wait(timeout, &mut watch).await == loops::Wake::Timer;
    }
}

// ── TLS ───────────────────────────────────────────────────────────────────

/// `<data>/databases/.<name>.tls` — a sibling of the data directory.
pub fn tls_dir(volume_path: &str, name: &str) -> String {
    let parent = Path::new(volume_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    format!("{parent}/.{name}.tls")
}

/// The container command that serves TLS: copy the key to a private file
/// owned by the engine's user, then start the engine through the image's
/// own entrypoint.
pub fn tls_command(db_type: &str) -> Vec<String> {
    let (user, exec) = match db_type {
        "postgres" => (
            "postgres",
            "exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/tond-tls/tls.crt -c ssl_key_file=/tmp/tond-tls/tls.key",
        ),
        "mysql" => (
            "mysql",
            "exec docker-entrypoint.sh mysqld --ssl-cert=/tmp/tond-tls/tls.crt --ssl-key=/tmp/tond-tls/tls.key",
        ),
        _ => (
            "mysql",
            "exec docker-entrypoint.sh mariadbd --ssl-cert=/tmp/tond-tls/tls.crt --ssl-key=/tmp/tond-tls/tls.key",
        ),
    };
    let script = format!(
        "set -e; install -d -o {user} -g {user} -m 0700 /tmp/tond-tls; \
         install -o {user} -g {user} -m 0600 {TLS_MOUNT}/tls.key /tmp/tond-tls/tls.key; \
         install -o {user} -g {user} -m 0644 {TLS_MOUNT}/tls.crt /tmp/tond-tls/tls.crt; \
         {exec}"
    );
    vec!["sh".to_string(), "-c".to_string(), script]
}

/// The certificate and key to serve: the dashboard domain's ACME
/// certificate, else a self-signed one for that name (or "tond-database").
async fn certificate_source(state: &AppState) -> Result<(Vec<u8>, Vec<u8>)> {
    let domain = state
        .settings
        .read()
        .await
        .dashboard_domain
        .trim()
        .to_string();
    if !domain.is_empty()
        && let Some((cert_path, key_path)) = sqlx::query_as::<_, (String, String)>(
            "SELECT cert_path, key_path FROM certificates
             WHERE domain = ? AND cert_type = 'acme' AND status = 'active'",
        )
        .bind(&domain)
        .fetch_optional(&state.db)
        .await?
        && let (Ok(cert), Ok(key)) = (
            tokio::fs::read(&cert_path).await,
            tokio::fs::read(&key_path).await,
        )
    {
        return Ok((cert, key));
    }
    let name = if domain.is_empty() {
        "tond-database".to_string()
    } else {
        domain
    };
    let generated = rcgen::generate_simple_self_signed(vec![name])
        .context("couldn't generate a self-signed certificate")?;
    Ok((
        generated.cert.pem().into_bytes(),
        generated.signing_key.serialize_pem().into_bytes(),
    ))
}

/// Write the certificate for a database (0644 cert, 0600 key; the directory
/// is 0700 and root-owned, mounted read-only).
pub async fn write_tls_files(state: &AppState, volume_path: &str, name: &str) -> Result<()> {
    let dir = tls_dir(volume_path, name);
    tokio::fs::create_dir_all(&dir).await?;
    let (cert, key) = certificate_source(state).await?;
    write_mode(&Path::new(&dir).join("tls.crt"), &cert, 0o644).await?;
    write_mode(&Path::new(&dir).join("tls.key"), &key, 0o600).await?;
    set_mode(Path::new(&dir), 0o700).await;
    Ok(())
}

async fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    tokio::fs::write(path, bytes).await?;
    set_mode(path, mode).await;
    Ok(())
}

async fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await;
}

/// After the dashboard certificate was renewed: refresh the copy of every
/// TLS database still serving the old one and restart it (the engines read
/// their certificate at start).
async fn refresh_tls_certificates(state: &AppState) -> Result<()> {
    let dbs: Vec<ManagedDatabase> = sqlx::query_as("SELECT * FROM databases WHERE tls = 1")
        .fetch_all(&state.db)
        .await?;
    let dbs: Vec<ManagedDatabase> = dbs
        .into_iter()
        .filter(|d| supports_tls(&d.db_type))
        .collect();
    if dbs.is_empty() {
        return Ok(());
    }
    let domain = state
        .settings
        .read()
        .await
        .dashboard_domain
        .trim()
        .to_string();
    let Some((cert_path,)) = sqlx::query_as::<_, (String,)>(
        "SELECT cert_path FROM certificates WHERE domain = ? AND cert_type = 'acme' AND status = 'active'",
    )
    .bind(&domain)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    let current = tokio::fs::read(&cert_path).await.unwrap_or_default();
    for db in dbs {
        let volume = crate::handlers::databases::volume_path_for(&db);
        let copy = Path::new(&tls_dir(&volume, &db.name)).join("tls.crt");
        let served = tokio::fs::read(&copy).await.unwrap_or_default();
        if served == current || current.is_empty() {
            continue;
        }
        write_tls_files(state, &volume, &db.name).await?;
        if let Some(cid) = &db.container_id
            && db.status == "running"
        {
            tracing::info!("db-access: new certificate for {}, restarting it", db.name);
            let _ = crate::docker::stop_container(&state.docker, cid).await;
            let _ = crate::docker::start_container(&state.docker, cid).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allowlist_entries_are_validated_and_canonicalised() {
        assert_eq!(
            normalize_allowlist(&entries(&[
                " 203.0.113.7 ",
                "198.51.100.77/24",
                "203.0.113.7/32",
                ""
            ])),
            Ok(entries(&["203.0.113.7", "198.51.100.0/24"]))
        );
        for bad in [
            "not-an-ip",
            "10.0.0.0/33",
            "2001:db8::1",
            "0.0.0.0/0",
            "1.2.3.4/x",
        ] {
            assert!(
                normalize_allowlist(&entries(&[bad])).is_err(),
                "{bad} must be refused"
            );
        }
        let many: Vec<String> = (0..=MAX_ALLOWLIST)
            .map(|i| format!("10.0.{}.1", i))
            .collect();
        assert!(normalize_allowlist(&many).is_err());
    }

    fn public_db(name: &str, port: i64, allow: &[&str]) -> ManagedDatabase {
        let mut db = ManagedDatabase::new(name.to_string(), "postgres".to_string(), None);
        db.public_access = true;
        db.host_port = Some(port);
        db.allowlist = serde_json::to_string(&entries(allow)).unwrap_or_default();
        db
    }

    #[test]
    fn rules_cover_public_databases_with_an_allowlist_only() {
        let mut private = public_db("private", 5402, &["10.0.0.1"]);
        private.public_access = false;
        let dbs = vec![
            public_db("a", 5400, &["203.0.113.7", "198.51.100.0/24"]),
            public_db("open", 5401, &[]),
            private,
        ];
        let rules = desired_rules(&dbs);
        assert_eq!(rules.keys().copied().collect::<Vec<_>>(), vec![5400]);

        let text = ruleset(&rules);
        assert!(text.starts_with("*filter\n:TOND-DB - [0:0]\n"));
        assert!(text.ends_with("COMMIT\n"));
        let m = "-p tcp -m conntrack --ctstate DNAT --ctdir ORIGINAL --ctorigdstport 5400";
        let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("-A")).collect();
        assert_eq!(
            lines,
            vec![
                format!("-A TOND-DB {m} -s 203.0.113.7 -j RETURN"),
                format!("-A TOND-DB {m} -s 198.51.100.0/24 -j RETURN"),
                format!("-A TOND-DB {m} -j DROP"),
            ],
            "allowed sources return, everything else to that port is dropped last"
        );
        assert_eq!(
            ruleset(&BTreeMap::new()),
            "*filter\n:TOND-DB - [0:0]\nCOMMIT\n"
        );
    }

    fn upgrading(mut db: ManagedDatabase, status: &str) -> ManagedDatabase {
        db.upgrade = Some(format!(
            r#"{{"status":"{status}","from_image":"","to_image":"","from_version":"16","to_version":"17","step":"","backup_id":null,"started_at":"","finished_at":null,"error":null,"rollback_dir":null}}"#
        ));
        assert_eq!(db.upgrade_state().map(|u| u.status), Some(status.to_string()));
        db
    }

    #[test]
    fn a_running_upgrade_closes_the_public_port_to_everyone() {
        let mut private = upgrading(public_db("private", 5403, &[]), "running");
        private.public_access = false;
        let dbs = vec![
            upgrading(public_db("open", 5400, &[]), "running"),
            upgrading(public_db("listed", 5401, &["203.0.113.7"]), "running"),
            upgrading(public_db("waiting", 5402, &[]), "awaiting_confirmation"),
            upgrading(public_db("failed", 5404, &["203.0.113.7"]), "failed"),
            private,
        ];
        let rules = desired_rules(&dbs);
        assert_eq!(
            rules,
            BTreeMap::from([
                (5400, vec![]),
                (5401, vec![]),
                (5404, vec!["203.0.113.7".to_string()]),
            ]),
            "running upgrades drop every source (even allowlisted ones); finished, failed \
             and private databases are back to their normal rules"
        );
        let text = ruleset(&rules);
        let m = "-p tcp -m conntrack --ctstate DNAT --ctdir ORIGINAL --ctorigdstport";
        assert!(text.contains(&format!("-A TOND-DB {m} 5400 -j DROP\n")));
        assert!(text.contains(&format!("-A TOND-DB {m} 5401 -j DROP\n")));
        assert!(!text.contains("5401 -s"), "{text}");

        let fw = FirewallStatus {
            available: true,
            applied_ports: vec![5400],
            error: None,
        };
        assert_eq!(access_warning(&dbs[0], &fw), None, "closed, so no 'open to everyone'");
    }

    #[test]
    fn public_databases_without_an_allowlist_warn() {
        let ok = FirewallStatus {
            available: true,
            applied_ports: vec![5400],
            error: None,
        };
        let mut private = public_db("p", 5400, &[]);
        private.public_access = false;
        assert_eq!(access_warning(&private, &ok), None);
        assert!(
            access_warning(&public_db("o", 5401, &[]), &ok)
                .is_some_and(|w| w.contains("every address"))
        );
        assert_eq!(
            access_warning(&public_db("a", 5400, &["1.2.3.4"]), &ok),
            None
        );
        let down = FirewallStatus {
            available: false,
            applied_ports: vec![],
            error: Some("no iptables".to_string()),
        };
        assert!(
            access_warning(&public_db("a", 5400, &["1.2.3.4"]), &down)
                .is_some_and(|w| w.contains("isn't enforced") && w.contains("no iptables"))
        );
    }

    #[test]
    fn tls_commands_copy_the_key_privately_and_keep_the_entrypoint() {
        for (kind, server) in [
            ("postgres", "postgres -c ssl=on"),
            ("mysql", "mysqld --ssl-cert"),
            ("mariadb", "mariadbd --ssl-cert"),
        ] {
            let cmd = tls_command(kind);
            assert_eq!(cmd[..2], ["sh".to_string(), "-c".to_string()]);
            assert!(cmd[2].contains("-m 0600 /etc/tond/tls/tls.key"), "{kind}");
            assert!(
                cmd[2].contains(&format!("exec docker-entrypoint.sh {server}")),
                "{kind}: {}",
                cmd[2]
            );
        }
        assert_eq!(
            tls_dir("/opt/tond/data/databases/shop", "shop"),
            "/opt/tond/data/databases/.shop.tls"
        );
    }
}
