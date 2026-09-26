// Memory ceiling of the snapshot archive pipeline.
//
// Archives a ~200 MB synthetic snapshot directory (tar → gzip → chunked
// AES-256-GCM → SHA-256 → sink) and asserts that peak heap use stays a small
// constant: nothing may buffer the whole archive. This test binary has its
// own counting global allocator, so it measures only this pipeline.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

#[allow(dead_code)]
#[path = "../src/snapshot_archive.rs"]
mod snapshot_archive;

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size > layout.size() {
                let grow = new_size - layout.size();
                let now = CURRENT.fetch_add(grow, Ordering::Relaxed) + grow;
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Counts bytes and discards them (stands in for the S3 upload).
struct CountingSink(u64);

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const MIB: usize = 1024 * 1024;
const DATA_MIB: usize = 200;
/// Generous for tar/gzip/AES buffers (a few hundred KiB in practice), far
/// below the data size.
const CEILING: usize = 16 * MIB;

#[test]
fn archiving_200_mb_keeps_heap_use_constant() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let volume = dir.path().join("volumes").join("web");
    std::fs::create_dir_all(&volume).unwrap_or_else(|e| panic!("mkdir: {e}"));

    // Incompressible data (gzip can't shrink it), written 1 MiB at a time.
    let mut block = vec![0u8; MIB];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for byte in block.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    for file in 0..4 {
        let mut out = std::fs::File::create(volume.join(format!("blob-{file}.bin")))
            .unwrap_or_else(|e| panic!("create: {e}"));
        for i in 0..DATA_MIB / 4 {
            block[0] = i as u8; // keep 1 MiB blocks distinct
            block[1] = file as u8;
            out.write_all(&block)
                .unwrap_or_else(|e| panic!("write: {e}"));
        }
    }
    drop(block);

    let baseline = CURRENT.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);

    let (summary, sink) = snapshot_archive::write_archive(
        dir.path(),
        CountingSink(0),
        "memory-test-secret",
        "snap-memory",
    )
    .unwrap_or_else(|e| panic!("write_archive: {e:#}"));

    let peak_growth = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    assert!(
        summary.bytes >= (DATA_MIB * MIB) as u64,
        "the archive must hold all the data ({} bytes)",
        summary.bytes
    );
    assert_eq!(sink.0, summary.bytes);
    assert!(
        peak_growth < CEILING,
        "archiving {DATA_MIB} MiB grew the heap by {:.1} MiB (ceiling {} MiB): \
         something buffers the archive",
        peak_growth as f64 / MIB as f64,
        CEILING / MIB
    );
}
