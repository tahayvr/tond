// metrics/docker_host.rs — Docker daemon health and Docker's disk usage
//
//   health()           ping + version + info (storage driver, root dir,
//                      warnings) and dockerd's own memory and uptime from
//                      /proc. Cached for HEALTH_TTL so a System page polled by
//                      several tabs costs dockerd one call a minute.
//   refresh_disk()     `docker system df` (images, containers, volumes, build
//                      cache) plus the size of the Tond data directory (hard
//                      links counted once: snapshots share files). Expensive
//                      for dockerd and the disk, so it runs every DF_EVERY
//                      from the metrics collector's tick (spawned, never
//                      blocking the tick) and on demand; one at a time.
//   cleanup_preview()  what "Free up space" would remove: unused build cache
//                      and dangling images (untagged layers no container
//                      uses). Nothing else — tagged images are rollback
//                      targets, volumes hold data, containers belong to apps.
//   cleanup()          removes exactly that.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use bollard::Docker;
use bollard::query_parameters::{
    ListImagesOptionsBuilder, PruneBuildOptionsBuilder, PruneImagesOptionsBuilder,
};
use serde::Serialize;
use tokio::sync::Mutex;

/// How long a Docker health answer is reused.
const HEALTH_TTL: Duration = Duration::from_secs(60);
/// How often the collector refreshes the disk breakdown.
pub const DF_EVERY: Duration = Duration::from_secs(30 * 60);
/// A manual refresh is ignored if the cached one is younger than this.
const DF_MIN_AGE: Duration = Duration::from_secs(20);
const DOCKER_TIMEOUT: Duration = Duration::from_secs(30);

/// Docker daemon health for the services panel.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DockerHealth {
    pub reachable: bool,
    pub version: Option<String>,
    pub api_version: Option<String>,
    pub storage_driver: Option<String>,
    pub root_dir: Option<String>,
    pub cgroup_version: Option<String>,
    /// `docker info` warnings (e.g. "No swap limit support").
    pub warnings: Vec<String>,
    pub containers: Option<i64>,
    pub containers_running: Option<i64>,
    pub images: Option<i64>,
    /// dockerd's resident memory.
    pub rss_bytes: Option<u64>,
    pub uptime_secs: Option<u64>,
    /// Why Docker is unreachable.
    pub error: Option<String>,
    /// Unix seconds.
    pub checked_at: i64,
}

static HEALTH: LazyLock<RwLock<Option<(Instant, DockerHealth)>>> =
    LazyLock::new(|| RwLock::new(None));

/// Docker health, cached for a minute.
pub async fn health(docker: &Docker) -> DockerHealth {
    if let Ok(guard) = HEALTH.read()
        && let Some((at, h)) = guard.as_ref()
        && at.elapsed() < HEALTH_TTL
    {
        return h.clone();
    }
    let h = probe_health(docker).await;
    if let Ok(mut guard) = HEALTH.write() {
        *guard = Some((Instant::now(), h.clone()));
    }
    h
}

/// Whether Docker answered its last health probe (fresh probe if the cache
/// is stale). Used by the heartbeat.
pub async fn docker_reachable(docker: &Docker) -> Result<(), String> {
    let h = health(docker).await;
    if h.reachable {
        Ok(())
    } else {
        Err(h.error.unwrap_or_else(|| "Docker is not answering".into()))
    }
}

async fn probe_health(docker: &Docker) -> DockerHealth {
    let now = chrono::Utc::now().timestamp();
    let ping = tokio::time::timeout(Duration::from_secs(5), docker.ping()).await;
    match ping {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            return DockerHealth {
                error: Some(format!("Docker is not answering: {e}")),
                checked_at: now,
                ..Default::default()
            };
        }
        Err(_) => {
            return DockerHealth {
                error: Some("Docker did not answer within 5 s".into()),
                checked_at: now,
                ..Default::default()
            };
        }
    }
    let version = docker.version().await.ok();
    let info = docker.info().await.ok();
    let (rss_bytes, uptime_secs) = dockerd_process();
    DockerHealth {
        reachable: true,
        version: version.as_ref().and_then(|v| v.version.clone()),
        api_version: version.as_ref().and_then(|v| v.api_version.clone()),
        storage_driver: info.as_ref().and_then(|i| i.driver.clone()),
        root_dir: info.as_ref().and_then(|i| i.docker_root_dir.clone()),
        cgroup_version: info
            .as_ref()
            .and_then(|i| i.cgroup_version.as_ref())
            .map(|v| v.to_string()),
        warnings: info
            .as_ref()
            .and_then(|i| i.warnings.clone())
            .unwrap_or_default(),
        containers: info.as_ref().and_then(|i| i.containers),
        containers_running: info.as_ref().and_then(|i| i.containers_running),
        images: info.as_ref().and_then(|i| i.images),
        rss_bytes,
        uptime_secs,
        error: None,
        checked_at: now,
    }
}

/// dockerd's RSS and uptime from /proc (None when unknown, e.g. on macOS).
fn dockerd_process() -> (Option<u64>, Option<u64>) {
    let Some(pid) = std::fs::read_to_string("/var/run/docker.pid")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return (None, None);
    };
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok();
    let rss = status.as_deref().and_then(vm_rss_bytes);
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok();
    let boot_uptime = std::fs::read_to_string("/proc/uptime").ok();
    let uptime = match (stat.as_deref(), boot_uptime.as_deref()) {
        (Some(stat), Some(up)) => process_uptime(stat, up),
        _ => None,
    };
    (rss, uptime)
}

fn vm_rss_bytes(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

/// Seconds a process has been running: /proc/uptime minus its start time
/// (field 22 of /proc/<pid>/stat, in clock ticks of 1/100 s).
fn process_uptime(stat: &str, proc_uptime: &str) -> Option<u64> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // after_comm starts at field 3; starttime is field 22 -> index 19.
    let start_ticks: f64 = after_comm.split_whitespace().nth(19)?.parse().ok()?;
    let up: f64 = proc_uptime.split_whitespace().next()?.parse().ok()?;
    Some((up - start_ticks / 100.0).max(0.0) as u64)
}

// ── Disk usage ──

/// One category of the disk breakdown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct UsageItem {
    pub bytes: i64,
    pub count: i64,
    /// What `docker … prune` could free (not all of it is safe to free).
    pub reclaimable: i64,
}

/// `docker system df` plus the Tond data directory.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DiskUsage {
    pub images: UsageItem,
    pub containers: UsageItem,
    pub volumes: UsageItem,
    pub build_cache: UsageItem,
    /// Bytes on disk under the data directory (tond.db, volumes, backups).
    pub tond_data: Option<i64>,
    pub data_dir: String,
    /// Docker's data root (from `docker info`), to tell whether its usage is
    /// on the same filesystem as the data directory.
    pub docker_root: Option<String>,
    /// Docker's data lives on the data directory's filesystem.
    pub docker_on_data_fs: bool,
    pub error: Option<String>,
    /// Unix seconds.
    pub collected_at: i64,
    /// How long the measurement took.
    pub took_ms: u64,
}

static DISK: RwLock<Option<(Instant, DiskUsage)>> = RwLock::new(None);
static DISK_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// The last measured breakdown (None until the first measurement).
pub fn cached_disk() -> Option<DiskUsage> {
    DISK.read()
        .ok()
        .and_then(|d| d.as_ref().map(|(_, u)| u.clone()))
}

/// Age of the cached breakdown.
pub fn disk_age() -> Option<Duration> {
    DISK.read()
        .ok()
        .and_then(|d| d.as_ref().map(|(at, _)| at.elapsed()))
}

/// Measure again (unless another measurement is running or the cache is
/// younger than DF_MIN_AGE, in which case the cached value is returned).
pub async fn refresh_disk(docker: &Docker, data_dir: &Path) -> DiskUsage {
    let _guard = DISK_LOCK.lock().await;
    if let Some(age) = disk_age()
        && age < DF_MIN_AGE
        && let Some(cached) = cached_disk()
    {
        return cached;
    }
    let usage = measure_disk(docker, data_dir).await;
    if let Ok(mut d) = DISK.write() {
        *d = Some((Instant::now(), usage.clone()));
    }
    usage
}

fn item(u: Option<(Option<i64>, Option<i64>, Option<i64>)>) -> UsageItem {
    let (bytes, count, reclaimable) = u.unwrap_or_default();
    UsageItem {
        bytes: bytes.unwrap_or(0),
        count: count.unwrap_or(0),
        reclaimable: reclaimable.unwrap_or(0),
    }
}

async fn measure_disk(docker: &Docker, data_dir: &Path) -> DiskUsage {
    let started = Instant::now();
    let now = chrono::Utc::now().timestamp();
    let mut usage = DiskUsage {
        data_dir: data_dir.display().to_string(),
        collected_at: now,
        ..Default::default()
    };
    match tokio::time::timeout(DOCKER_TIMEOUT, docker.df(None)).await {
        Ok(Ok(df)) => {
            usage.images = item(
                df.image_usage
                    .map(|u| (u.total_size, u.total_count, u.reclaimable)),
            );
            usage.containers = item(
                df.container_usage
                    .map(|u| (u.total_size, u.total_count, u.reclaimable)),
            );
            usage.volumes = item(
                df.volume_usage
                    .map(|u| (u.total_size, u.total_count, u.reclaimable)),
            );
            usage.build_cache = item(
                df.build_cache_usage
                    .map(|u| (u.total_size, u.total_count, u.reclaimable)),
            );
        }
        Ok(Err(e)) => usage.error = Some(format!("docker system df failed: {e}")),
        Err(_) => {
            usage.error = Some(format!(
                "docker system df took longer than {} s",
                DOCKER_TIMEOUT.as_secs()
            ))
        }
    }
    usage.docker_root = health(docker).await.root_dir;
    usage.docker_on_data_fs = usage
        .docker_root
        .as_deref()
        .is_some_and(|root| same_filesystem(Path::new(root), data_dir));
    let dir = data_dir.to_path_buf();
    usage.tond_data = tokio::task::spawn_blocking(move || dir_size(&dir))
        .await
        .ok()
        .flatten();
    usage.took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    usage
}

fn same_filesystem(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

/// Bytes allocated on disk under `dir`, staying on its filesystem and
/// counting hard-linked files once.
pub fn dir_size(dir: &Path) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    let root_dev = std::fs::metadata(dir).ok()?.dev();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut total: u64 = 0;
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .flatten()
    {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.dev() != root_dev {
            continue;
        }
        if meta.is_file() && meta.nlink() > 1 && !seen.insert(meta.ino()) {
            continue;
        }
        total = total.saturating_add(meta.blocks().saturating_mul(512));
    }
    i64::try_from(total).ok()
}

// ── Free up space ──

/// One dangling image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DanglingImage {
    pub id: String,
    pub bytes: i64,
    /// Unix seconds.
    pub created: i64,
}

/// What "Free up space" would remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CleanupPreview {
    /// Unused build cache.
    pub build_cache_bytes: i64,
    pub build_cache_entries: i64,
    pub dangling_images: Vec<DanglingImage>,
    pub dangling_bytes: i64,
    /// Sum of the above (upper bound: layers shared with other images are
    /// only freed when nothing else uses them).
    pub total_bytes: i64,
}

/// What was removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CleanupResult {
    pub build_cache_bytes: i64,
    pub build_cache_entries: i64,
    pub images_deleted: i64,
    pub image_bytes: i64,
    pub total_bytes: i64,
}

fn dangling_filter() -> std::collections::HashMap<&'static str, Vec<&'static str>> {
    std::collections::HashMap::from([("dangling", vec!["true"])])
}

pub async fn cleanup_preview(docker: &Docker) -> anyhow::Result<CleanupPreview> {
    let images = docker
        .list_images(Some(
            ListImagesOptionsBuilder::default()
                .all(false)
                .filters(&dangling_filter())
                .build(),
        ))
        .await?;
    let dangling_images: Vec<DanglingImage> = images
        .into_iter()
        // In use by a (stopped) container: `image prune` keeps it.
        .filter(|i| i.containers <= 0)
        .map(|i| DanglingImage {
            id: i
                .id
                .trim_start_matches("sha256:")
                .chars()
                .take(12)
                .collect(),
            bytes: i.size,
            created: i.created,
        })
        .collect();
    let dangling_bytes = dangling_images.iter().map(|i| i.bytes).sum();
    let build = tokio::time::timeout(DOCKER_TIMEOUT, docker.df(None))
        .await
        .map_err(|_| anyhow::anyhow!("docker system df timed out"))??
        .build_cache_usage;
    let (build_cache_bytes, build_cache_entries) = build
        .map(|b| {
            (
                b.reclaimable.unwrap_or(0),
                b.total_count.unwrap_or(0) - b.active_count.unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    Ok(CleanupPreview {
        build_cache_bytes,
        build_cache_entries: build_cache_entries.max(0),
        total_bytes: build_cache_bytes + dangling_bytes,
        dangling_images,
        dangling_bytes,
    })
}

/// Remove unused build cache and dangling images.
pub async fn cleanup(docker: &Docker, data_dir: PathBuf) -> anyhow::Result<CleanupResult> {
    let build = docker
        .prune_build(Some(PruneBuildOptionsBuilder::default().all(true).build()))
        .await?;
    let images = docker
        .prune_images(Some(
            PruneImagesOptionsBuilder::default()
                .filters(&dangling_filter())
                .build(),
        ))
        .await?;
    let build_cache_bytes = build.space_reclaimed.unwrap_or(0);
    let image_bytes = images.space_reclaimed.unwrap_or(0);
    let result = CleanupResult {
        build_cache_bytes,
        build_cache_entries: build.caches_deleted.map_or(0, |c| c.len() as i64),
        images_deleted: images.images_deleted.map_or(0, |d| {
            d.iter().filter(|i| i.deleted.is_some()).count() as i64
        }),
        image_bytes,
        total_bytes: build_cache_bytes + image_bytes,
    };
    // Show the new numbers right away.
    if let Ok(mut d) = DISK.write() {
        *d = None;
    }
    let docker = docker.clone();
    tokio::spawn(async move {
        refresh_disk(&docker, &data_dir).await;
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dockerd_memory_and_uptime_from_proc() {
        assert_eq!(
            vm_rss_bytes("Name:\tdockerd\nVmRSS:\t   2048 kB\n"),
            Some(2048 * 1024)
        );
        assert_eq!(vm_rss_bytes("Name:\tdockerd\n"), None);
        // starttime (field 22) = 50_000 ticks = 500 s after boot.
        let stat = "123 (dockerd) S 1 123 123 0 -1 4194560 1 0 0 0 10 5 0 0 20 0 12 0 50000 0";
        assert_eq!(process_uptime(stat, "3600.50 7000.00"), Some(3100));
        assert_eq!(process_uptime("garbage", "1 2"), None);
    }

    #[test]
    fn dir_size_counts_hard_links_once() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let a = dir.path().join("a");
        std::fs::write(&a, vec![1u8; 64 * 1024]).unwrap_or_else(|e| panic!("write: {e}"));
        let one = dir_size(dir.path()).unwrap_or_default();
        assert!(one >= 64 * 1024);
        std::fs::hard_link(&a, dir.path().join("b")).unwrap_or_else(|e| panic!("link: {e}"));
        assert_eq!(dir_size(dir.path()), Some(one), "the link adds nothing");
        std::fs::write(dir.path().join("c"), vec![1u8; 64 * 1024])
            .unwrap_or_else(|e| panic!("write: {e}"));
        assert!(dir_size(dir.path()).unwrap_or_default() >= one + 64 * 1024);
    }

    #[tokio::test]
    async fn an_unreachable_docker_is_reported_not_fatal() {
        let docker =
            Docker::connect_with_http("http://127.0.0.1:1", 1, bollard::API_DEFAULT_VERSION)
                .unwrap_or_else(|e| panic!("docker: {e}"));
        let h = probe_health(&docker).await;
        assert!(!h.reachable);
        assert!(h.error.is_some());
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let usage = measure_disk(&docker, dir.path()).await;
        assert!(usage.error.is_some());
        assert!(
            usage.tond_data.is_some(),
            "the data directory is still measured"
        );
    }
}
