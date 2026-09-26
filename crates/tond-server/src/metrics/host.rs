// metrics/host.rs — cached host CPU / memory / disk gauges
//
// The collector samples the host once per tick and stores the result here;
// `GET /api/v1/system/metrics` and the disk-usage alert only read the cache.
// (The old handler built a full `sysinfo::System` — enumerating every process
// on the host — and slept 200 ms inside the request, for every open tab.)
//
//   CPU     a persistent sysinfo::System with only CPU usage refreshed: it
//           reads /proc/stat and reports the delta since the previous tick,
//           so no sleep is needed.
//   Memory  /proc/meminfo via sysinfo (used = total − available).
//   Disk    statvfs() of the filesystem holding the Tond data directory
//           (tond.db, volumes, backups) — the one that fills up — and of `/`
//           when that is a different filesystem (Docker's images usually live
//           there). Blocks and inodes. The mount point is found once by
//           walking up until the device changes.
//   Load    /proc/loadavg; swap from /proc/meminfo.
//
// The same samples feed the host history (host_history.rs: one row a minute,
// the disk-full forecast `disk_full_eta`).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

pub use super::host_history::{DiskEta, disk_full_eta};

/// Blocks and inodes of one filesystem (df-style: total = used + available
/// to unprivileged users).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsUsage {
    pub used: u64,
    pub total: u64,
    pub inodes_used: u64,
    pub inodes_total: u64,
}

/// Latest host sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostSnapshot {
    /// Average CPU usage across all cores (0 – 100).
    pub cpu_percent: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    /// total − available
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub inodes_used: u64,
    pub inodes_total: u64,
    /// Mount point of the filesystem the disk numbers describe.
    pub disk_mount: String,
    /// `/`, when it is not the data directory's filesystem.
    pub root: Option<FsUsage>,
    /// Unix seconds.
    pub collected_at: i64,
}

impl HostSnapshot {
    pub fn mem_percent(&self) -> f64 {
        percent(self.mem_used, self.mem_total)
    }

    pub fn disk_percent(&self) -> f64 {
        percent(self.disk_used, self.disk_total)
    }

    /// The data directory's filesystem.
    pub fn data_fs(&self) -> FsUsage {
        FsUsage {
            used: self.disk_used,
            total: self.disk_total,
            inodes_used: self.inodes_used,
            inodes_total: self.inodes_total,
        }
    }

    /// `/` (the data directory's filesystem when they are the same).
    pub fn root_fs(&self) -> FsUsage {
        self.root.clone().unwrap_or_else(|| self.data_fs())
    }
}

fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}

static SNAPSHOT: RwLock<Option<HostSnapshot>> = RwLock::new(None);

/// The latest cached host sample (None until the collector's first tick).
pub fn snapshot() -> Option<HostSnapshot> {
    SNAPSHOT.read().ok().and_then(|s| s.clone())
}

fn store(snapshot: HostSnapshot) {
    if let Ok(mut s) = SNAPSHOT.write() {
        *s = Some(snapshot);
    }
}

/// Blocks and inodes of the filesystem containing `path`, df-style:
/// used = blocks − free, total = used + available-to-unprivileged.
pub fn fs_usage(path: &Path) -> Option<FsUsage> {
    let st = rustix::fs::statvfs(path).ok()?;
    let frsize = if st.f_frsize > 0 {
        st.f_frsize
    } else {
        st.f_bsize
    };
    let used = st
        .f_blocks
        .saturating_sub(st.f_bfree)
        .saturating_mul(frsize);
    let avail = st.f_bavail.saturating_mul(frsize);
    Some(FsUsage {
        used,
        total: used.saturating_add(avail),
        inodes_used: st.f_files.saturating_sub(st.f_ffree),
        inodes_total: st.f_files,
    })
}

/// Device id of the filesystem holding `path`.
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// Mount point of the filesystem containing `path` (walks up while the device
/// id stays the same).
pub fn mount_point(path: &Path) -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let Ok(start) = std::fs::canonicalize(path) else {
        return PathBuf::from("/");
    };
    let Ok(meta) = std::fs::metadata(&start) else {
        return PathBuf::from("/");
    };
    let dev = meta.dev();
    let mut current = start.clone();
    while let Some(parent) = current.parent() {
        match std::fs::metadata(parent) {
            Ok(m) if m.dev() == dev => current = parent.to_path_buf(),
            _ => break,
        }
    }
    current
}

/// Samples the host; owned by the collector task.
pub struct HostSampler {
    sys: System,
    disk_path: PathBuf,
    disk_mount: String,
    /// `/` is a different filesystem than the data directory's.
    separate_root: bool,
}

impl HostSampler {
    /// `disk_path` is the directory whose filesystem is reported (the Tond
    /// data directory). The existing ancestor is used if it does not exist.
    pub fn new(disk_path: PathBuf) -> Self {
        let disk_path = existing_ancestor(&disk_path);
        let disk_mount = mount_point(&disk_path).display().to_string();
        let separate_root = device_of(&disk_path) != device_of(Path::new("/"));
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram().with_swap()),
        );
        Self {
            sys,
            disk_path,
            disk_mount,
            separate_root,
        }
    }

    /// Take a sample and publish it to the cache.
    pub fn sample(&mut self, now: i64) -> HostSnapshot {
        self.sys.refresh_cpu_usage();
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
        let cpus = self.sys.cpus();
        let cpu_percent = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| f64::from(c.cpu_usage())).sum::<f64>() / cpus.len() as f64
        };
        let load = System::load_average();
        let data = fs_usage(&self.disk_path).unwrap_or_default();
        let root = if self.separate_root {
            fs_usage(Path::new("/"))
        } else {
            None
        };
        let snapshot = HostSnapshot {
            cpu_percent,
            load1: load.one,
            load5: load.five,
            load15: load.fifteen,
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            disk_used: data.used,
            disk_total: data.total,
            inodes_used: data.inodes_used,
            inodes_total: data.inodes_total,
            disk_mount: self.disk_mount.clone(),
            root,
            collected_at: now,
        };
        store(snapshot.clone());
        snapshot
    }
}

fn existing_ancestor(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    while !p.exists() {
        match p.parent() {
            Some(parent) => p = parent.to_path_buf(),
            None => return PathBuf::from("/"),
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_handle_zero_totals() {
        let s = HostSnapshot {
            mem_used: 1,
            mem_total: 4,
            disk_used: 0,
            disk_total: 0,
            ..Default::default()
        };
        assert_eq!(s.mem_percent(), 25.0);
        assert_eq!(s.disk_percent(), 0.0);
    }

    #[test]
    fn disk_usage_and_mount_point_describe_a_real_filesystem() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let fs = fs_usage(dir.path()).unwrap_or_else(|| panic!("statvfs failed"));
        assert!(fs.total > 0 && fs.used <= fs.total);
        assert!(fs.inodes_used <= fs.inodes_total);
        let mount = mount_point(dir.path());
        assert!(
            dir.path()
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&mount))
        );
    }

    #[test]
    fn sampler_publishes_to_the_cache_and_tolerates_missing_paths() {
        let mut sampler = HostSampler::new(PathBuf::from("/definitely/not/here/tond"));
        let snap = sampler.sample(1_700_000_000);
        assert!(snap.mem_total > 0);
        assert!(snap.disk_total > 0, "falls back to an existing ancestor");
        assert!(snap.load1 >= 0.0);
        assert!(snap.root_fs().total > 0);
        assert_eq!(snapshot().map(|s| s.collected_at), Some(1_700_000_000));
    }
}
