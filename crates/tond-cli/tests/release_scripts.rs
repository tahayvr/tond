// The shell tests of the release scripts, so `cargo nextest run` covers them:
//
//   release/tests/update-test.sh  update.sh's state machine: backup, swap,
//                                 migrate, health check, automatic rollback
//                                 (binaries, and the database when migrations
//                                 may have run), manual rollback within 7 days
//   release/tests/distro-test.sh  the support matrix: distribution detection,
//                                 Docker's apt codename, refusing other systems
//
// They need only bash and coreutils (bash 3.2 works).

use std::path::PathBuf;
use std::process::Command;

fn run(script: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("release/tests").join(script);
    let output = Command::new("bash")
        .arg(&path)
        .current_dir(&root)
        .env_remove("TOND_INSTALL_DIR")
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("could not run bash {}: {e}", path.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{script} failed ({}):\n{stdout}\n{stderr}",
        output.status
    );
}

#[test]
fn update_script_updates_and_rolls_back() {
    run("update-test.sh");
}

#[test]
fn install_script_enforces_the_support_matrix() {
    run("distro-test.sh");
}
