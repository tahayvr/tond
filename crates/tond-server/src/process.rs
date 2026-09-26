// process.rs — spawning child processes
//
// tond-server runs with OOMScoreAdjust=-900 so the kernel's OOM killer picks
// anything else first. Children inherit that value, which would shield git
// clones, docker CLI builds, tar and host-terminal shells as well. Every child
// is reset to a neutral score of 0 (raising the score needs no privilege).

use rustix::fs::{Mode, OFlags};

/// A `tokio::process::Command` whose child starts with oom_score_adj 0.
pub fn command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    // SAFETY: the closure only calls async-signal-safe functions (open, write,
    // close via rustix) and does not allocate, as required between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            reset_own_oom_score();
            Ok(())
        });
    }
    cmd
}

/// Reset the OOM score of an already running child (for spawners without a
/// pre-exec hook, such as the PTY used by the terminal). Best effort.
pub fn reset_oom_score(pid: u32) {
    let _ = std::fs::write(format!("/proc/{pid}/oom_score_adj"), "0");
}

fn reset_own_oom_score() {
    if let Ok(fd) = rustix::fs::open(
        c"/proc/self/oom_score_adj",
        OFlags::WRONLY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        let _ = rustix::io::write(&fd, b"0");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn children_run_normally_and_start_with_a_neutral_oom_score() {
        let output = command("sh")
            .args([
                "-c",
                "cat /proc/self/oom_score_adj 2>/dev/null || echo none",
            ])
            .output()
            .await
            .unwrap_or_else(|err| panic!("spawn: {err}"));
        assert!(output.status.success());
        let score = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Linux reports 0; systems without /proc (macOS dev machines) print "none".
        assert!(
            score == "0" || score == "none",
            "unexpected oom_score_adj {score}"
        );
    }
}
