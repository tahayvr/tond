// procstat.rs — cheap process self-measurement from /proc (Linux only)
//
// Used by tond-server and tond-proxy to report their own resource use
// (GET /api/v1/system/self, /metrics, the proxy's `stats` control request).
// Everything is read on demand from /proc/self; nothing runs in the
// background. On other platforms (macOS dev machines) every value is None.

use serde::{Deserialize, Serialize};

/// Kernel clock ticks per second for /proc/<pid>/stat CPU times. USER_HZ is
/// part of the Linux ABI and 100 on every architecture Tond supports.
const CLOCK_TICKS: f64 = 100.0;

/// One snapshot of this process's resource use.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessSample {
    /// Resident set size in bytes.
    pub rss_bytes: Option<u64>,
    /// User + system CPU time consumed since the process started.
    pub cpu_seconds: Option<f64>,
    /// Open file descriptors.
    pub open_fds: Option<u64>,
    /// OS threads.
    pub threads: Option<u64>,
    /// Voluntary + involuntary context switches summed over all threads: a
    /// proxy for how often the process wakes up.
    pub context_switches: Option<u64>,
}

impl ProcessSample {
    /// Measure the current process.
    pub fn current() -> Self {
        let status = std::fs::read_to_string("/proc/self/status").ok();
        let stat = std::fs::read_to_string("/proc/self/stat").ok();
        Self {
            rss_bytes: status
                .as_deref()
                .and_then(|s| status_field(s, "VmRSS:"))
                .map(|kb| kb * 1024),
            cpu_seconds: stat.as_deref().and_then(cpu_seconds_from_stat),
            open_fds: count_dir_entries("/proc/self/fd"),
            threads: status.as_deref().and_then(|s| status_field(s, "Threads:")),
            context_switches: context_switches("/proc/self/task"),
        }
    }
}

/// CPU use in percent of one core between two samples taken `elapsed_secs` apart.
pub fn cpu_percent(prev: &ProcessSample, now: &ProcessSample, elapsed_secs: f64) -> Option<f64> {
    let (a, b) = (prev.cpu_seconds?, now.cpu_seconds?);
    if elapsed_secs <= 0.0 {
        return None;
    }
    Some(((b - a).max(0.0) / elapsed_secs) * 100.0)
}

/// First number after `key` in a /proc status file ("VmRSS:   1234 kB" -> 1234).
fn status_field(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// utime + stime (fields 14 and 15) from /proc/<pid>/stat, in seconds. The
/// command name (field 2) may contain spaces, so parse after its closing ')'.
fn cpu_seconds_from_stat(stat: &str) -> Option<f64> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // after_comm starts at field 3 (state); utime is field 14 -> index 11.
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / CLOCK_TICKS)
}

fn count_dir_entries(path: &str) -> Option<u64> {
    Some(std::fs::read_dir(path).ok()?.count() as u64)
}

fn context_switches(task_dir: &str) -> Option<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(task_dir).ok()?.flatten() {
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        total += status_field(&status, "voluntary_ctxt_switches:").unwrap_or(0);
        total += status_field(&status, "nonvoluntary_ctxt_switches:").unwrap_or(0);
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_fields() {
        let status = "Name:\ttond-server\nThreads:\t7\nVmRSS:\t   12345 kB\n";
        assert_eq!(status_field(status, "VmRSS:"), Some(12345));
        assert_eq!(status_field(status, "Threads:"), Some(7));
        assert_eq!(status_field(status, "VmSwap:"), None);
    }

    #[test]
    fn parses_cpu_time_even_with_spaces_in_the_command_name() {
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime
        let stat = "42 (tond (odd) name) S 1 42 42 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 7 0";
        assert_eq!(cpu_seconds_from_stat(stat), Some(3.0));
        assert_eq!(cpu_seconds_from_stat("garbage"), None);
    }

    #[test]
    fn cpu_percent_is_relative_to_one_core() {
        let a = ProcessSample {
            cpu_seconds: Some(1.0),
            ..Default::default()
        };
        let b = ProcessSample {
            cpu_seconds: Some(1.5),
            ..Default::default()
        };
        assert_eq!(cpu_percent(&a, &b, 10.0), Some(5.0));
        assert_eq!(cpu_percent(&a, &b, 0.0), None);
        assert_eq!(cpu_percent(&ProcessSample::default(), &b, 1.0), None);
    }

    #[test]
    fn current_sample_is_populated_on_linux_and_empty_elsewhere() {
        let s = ProcessSample::current();
        if cfg!(target_os = "linux") {
            assert!(s.rss_bytes.is_some_and(|r| r > 0));
            assert!(s.open_fds.is_some());
        } else {
            assert_eq!(s.rss_bytes, None);
        }
    }
}
