// metrics/cgroup.rs — read container resource usage straight from cgroup v2
//
// Every Docker container lives in its own cgroup. Reading a handful of small
// files there costs microseconds and puts no load on dockerd (the old
// collector called `docker stats` per container, sequentially, which took
// ~1-2 s each):
//
//   cpu.stat        usage_usec             cumulative CPU time (µs)
//   memory.current                         bytes charged, incl. page cache
//   memory.stat     inactive_file          reclaimable cache (subtracted, like
//                                          `docker stats` does)
//   memory.peak                            high-water mark (kernel ≥ 5.19)
//   memory.max                             limit, or "max"
//   memory.events   oom_kill               processes killed by the OOM killer
//   pids.current                           process / thread count
//   cgroup.procs                           a pid inside the container
//
// Network bytes are per network namespace, not per cgroup: they come from
// /proc/<pid>/net/dev of any process in the container (all share the netns).
//
// The cgroup directory is found by probing the two layouts Docker uses
// (cgroupfs driver: <root>/docker/<id>; systemd driver:
// <root>/system.slice/docker-<id>.scope), falling back to the container's
// /proc/<pid>/cgroup when Docker reports its pid.

use std::io;
use std::path::{Path, PathBuf};

/// One reading of a container's cgroup counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CgroupReading {
    /// Cumulative CPU time in microseconds.
    pub cpu_usage_usec: u64,
    /// Working-set memory in bytes (memory.current − inactive_file).
    pub mem: u64,
    /// memory.peak when the kernel provides it.
    pub mem_peak: Option<u64>,
    /// memory.max, or None when unlimited.
    pub mem_limit: Option<u64>,
    /// Cumulative OOM kills in this cgroup.
    pub oom_kills: u64,
    /// Current number of tasks.
    pub pids: u64,
    /// First pid in cgroup.procs (None when the cgroup is empty).
    pub first_pid: Option<u32>,
}

/// Value of `key` in a flat-keyed cgroup file ("key value" per line).
pub fn keyed_value(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_ascii_whitespace();
        (parts.next()? == key)
            .then(|| parts.next()?.parse().ok())
            .flatten()
    })
}

/// Parse a single-value cgroup file such as memory.max ("max" → None).
pub fn single_value(contents: &str) -> Option<u64> {
    let v = contents.trim();
    if v == "max" { None } else { v.parse().ok() }
}

/// Sum received and transmitted bytes over every interface except loopback
/// in the contents of /proc/<pid>/net/dev.
pub fn parse_net_dev(contents: &str) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in contents.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        rx = rx.saturating_add(fields[0].parse().unwrap_or(0));
        tx = tx.saturating_add(fields[8].parse().unwrap_or(0));
    }
    (rx, tx)
}

/// The cgroup v2 path in the contents of /proc/<pid>/cgroup ("0::/path").
pub fn parse_proc_cgroup(contents: &str) -> Option<&str> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::trim)
        .filter(|p| p.starts_with('/'))
}

/// Whether `id` looks like a full Docker container id (64 lowercase hex).
fn is_container_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Probe the usual Docker cgroup locations for container `id`.
pub fn find_cgroup_dir(cgroup_root: &Path, id: &str) -> Option<PathBuf> {
    if !is_container_id(id) {
        return None;
    }
    [
        cgroup_root.join("docker").join(id),
        cgroup_root
            .join("system.slice")
            .join(format!("docker-{id}.scope")),
    ]
    .into_iter()
    .find(|p| p.join("cpu.stat").is_file())
}

/// Resolve the cgroup directory of a process via /proc/<pid>/cgroup.
pub fn cgroup_dir_for_pid(cgroup_root: &Path, proc_root: &Path, pid: u32) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
    let rel = parse_proc_cgroup(&contents)?;
    // Reject anything that could escape the cgroup root.
    if rel.split('/').any(|c| c == "..") {
        return None;
    }
    let dir = cgroup_root.join(rel.trim_start_matches('/'));
    dir.join("cpu.stat").is_file().then_some(dir)
}

/// Read every counter from the cgroup directory `dir`. Fails only when the
/// cgroup is gone (cpu.stat unreadable); optional files default sensibly.
pub fn read_cgroup(dir: &Path) -> io::Result<CgroupReading> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name));
    let cpu = read("cpu.stat")?;
    let cpu_usage_usec = keyed_value(&cpu, "usage_usec").unwrap_or(0);

    let current = read("memory.current")
        .ok()
        .and_then(|s| single_value(&s))
        .unwrap_or(0);
    let inactive_file = read("memory.stat")
        .ok()
        .and_then(|s| keyed_value(&s, "inactive_file"))
        .unwrap_or(0);
    let mem_peak = read("memory.peak").ok().and_then(|s| single_value(&s));
    let mem_limit = read("memory.max").ok().and_then(|s| single_value(&s));
    let oom_kills = read("memory.events")
        .ok()
        .and_then(|s| keyed_value(&s, "oom_kill"))
        .unwrap_or(0);
    let pids = read("pids.current")
        .ok()
        .and_then(|s| single_value(&s))
        .unwrap_or(0);
    let first_pid = read("cgroup.procs")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.trim().parse().ok()));

    Ok(CgroupReading {
        cpu_usage_usec,
        mem: current.saturating_sub(inactive_file),
        mem_peak,
        mem_limit,
        oom_kills,
        pids,
        first_pid,
    })
}

/// Cumulative (rx, tx) bytes of the network namespace `pid` lives in.
pub fn read_net_dev(proc_root: &Path, pid: u32) -> io::Result<(u64, u64)> {
    let contents = std::fs::read_to_string(proc_root.join(pid.to_string()).join("net/dev"))?;
    Ok(parse_net_dev(&contents))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const NET_DEV: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:    9999       5    0    0    0     0          0         0     9999       5    0    0    0     0       0          0
 tunl0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
  eth0:     410       5    0    0    0     0          0         0       42       1    0    0    0     0       0          0
  eth1:     100       1    0    0    0     0          0         0       58       1    0    0    0     0       0          0
";

    /// Write a fake container cgroup under `root`.
    pub fn write_fake_cgroup(dir: &Path, usage_usec: u64, current: u64, oom_kills: u64) {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let files = [
            (
                "cpu.stat",
                format!("usage_usec {usage_usec}\nuser_usec 1\nsystem_usec 2\n"),
            ),
            ("memory.current", format!("{current}\n")),
            (
                "memory.stat",
                "anon 100\nfile 2000\ninactive_file 1000\n".to_string(),
            ),
            ("memory.peak", "7860224\n".to_string()),
            ("memory.max", "max\n".to_string()),
            (
                "memory.events",
                format!("low 0\nhigh 0\nmax 0\noom 0\noom_kill {oom_kills}\n"),
            ),
            ("pids.current", "3\n".to_string()),
            ("cgroup.procs", "563\n564\n".to_string()),
        ];
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap_or_else(|e| panic!("write {name}: {e}"));
        }
    }

    #[test]
    fn keyed_and_single_values_parse() {
        assert_eq!(
            keyed_value("usage_usec 11472\nuser_usec 6692\n", "usage_usec"),
            Some(11472)
        );
        assert_eq!(keyed_value("oom 1\noom_kill 3\n", "oom_kill"), Some(3));
        assert_eq!(keyed_value("oom 1\n", "oom_kill"), None);
        assert_eq!(single_value("max\n"), None);
        assert_eq!(single_value("536870912\n"), Some(536_870_912));
        assert_eq!(single_value("garbage"), None);
    }

    #[test]
    fn net_dev_sums_non_loopback_interfaces() {
        assert_eq!(parse_net_dev(NET_DEV), (510, 100));
        assert_eq!(parse_net_dev(""), (0, 0));
    }

    #[test]
    fn proc_cgroup_path_is_extracted() {
        assert_eq!(
            parse_proc_cgroup("0::/system.slice/docker-abc.scope\n"),
            Some("/system.slice/docker-abc.scope")
        );
        assert_eq!(parse_proc_cgroup("1:name=systemd:/x\n"), None);
    }

    #[test]
    fn find_cgroup_dir_probes_both_driver_layouts() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let id_a = "a".repeat(64);
        let id_b = "b".repeat(64);
        write_fake_cgroup(&root.path().join("docker").join(&id_a), 1, 1, 0);
        write_fake_cgroup(
            &root
                .path()
                .join("system.slice")
                .join(format!("docker-{id_b}.scope")),
            1,
            1,
            0,
        );
        assert_eq!(
            find_cgroup_dir(root.path(), &id_a),
            Some(root.path().join("docker").join(&id_a))
        );
        assert!(find_cgroup_dir(root.path(), &id_b).is_some());
        assert_eq!(find_cgroup_dir(root.path(), &"c".repeat(64)), None);
        assert_eq!(find_cgroup_dir(root.path(), "../etc"), None);
    }

    #[test]
    fn cgroup_dir_for_pid_follows_proc_and_rejects_escapes() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let cg = root.path().join("cg");
        let proc_root = root.path().join("proc");
        write_fake_cgroup(&cg.join("custom/ctr"), 1, 1, 0);
        std::fs::create_dir_all(proc_root.join("42")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(proc_root.join("42/cgroup"), "0::/custom/ctr\n")
            .unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(proc_root.join("43")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(proc_root.join("43/cgroup"), "0::/../ctr\n")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            cgroup_dir_for_pid(&cg, &proc_root, 42),
            Some(cg.join("custom/ctr"))
        );
        assert_eq!(cgroup_dir_for_pid(&cg, &proc_root, 43), None);
        assert_eq!(cgroup_dir_for_pid(&cg, &proc_root, 44), None);
    }

    #[test]
    fn read_cgroup_reads_every_counter() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dir = root.path().join("ctr");
        write_fake_cgroup(&dir, 11_472, 2_019_328, 2);
        let r = read_cgroup(&dir).unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(
            r,
            CgroupReading {
                cpu_usage_usec: 11_472,
                mem: 2_018_328,
                mem_peak: Some(7_860_224),
                mem_limit: None,
                oom_kills: 2,
                pids: 3,
                first_pid: Some(563),
            }
        );
        assert!(read_cgroup(&root.path().join("gone")).is_err());
    }

    #[test]
    fn read_net_dev_reads_proc_file() {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        std::fs::create_dir_all(root.path().join("7/net")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(root.path().join("7/net/dev"), NET_DEV).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read_net_dev(root.path(), 7).ok(), Some((510, 100)));
        assert!(read_net_dev(root.path(), 8).is_err());
    }
}
