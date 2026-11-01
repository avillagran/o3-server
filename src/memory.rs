//! Opt-in aggregate heap diagnostics. Never emits allocation contents or identifiers.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static LARGE_LIVE: AtomicUsize = AtomicUsize::new(0);
static LARGE_PEAK: AtomicUsize = AtomicUsize::new(0);
static MAX_ALLOCATION: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
pub struct Measured;
fn large_add(n: usize) {
    if n >= 128 * 1024 {
        let live = LARGE_LIVE.fetch_add(n, Relaxed) + n;
        LARGE_PEAK.fetch_max(live, Relaxed);
    }
}
fn large_remove(n: usize) {
    if n >= 128 * 1024 {
        LARGE_LIVE.fetch_sub(n, Relaxed);
    }
}
fn add(n: usize) {
    MAX_ALLOCATION.fetch_max(n, Relaxed);
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = std::alloc::System.alloc(l);
        if !p.is_null() {
            add(l.size());
            large_add(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        std::alloc::System.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Relaxed);
        large_remove(l.size());
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = std::alloc::System.alloc_zeroed(l);
        if !p.is_null() {
            add(l.size());
            large_add(l.size());
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let p = std::alloc::System.realloc(p, l, n);
        if !p.is_null() {
            large_remove(l.size());
            large_add(n);
            MAX_ALLOCATION.fetch_max(n, Relaxed);
            if n >= l.size() {
                add(n - l.size());
            } else {
                LIVE.fetch_sub(l.size() - n, Relaxed);
            }
        }
        p
    }
}
pub fn start() {
    tokio::spawn(async {
        let mut signal =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).unwrap();
        while signal.recv().await.is_some() {
            let (mut used, mut peak) = (0i64, 0i64);
            unsafe {
                rusqlite::ffi::sqlite3_status64(
                    rusqlite::ffi::SQLITE_STATUS_MEMORY_USED,
                    &mut used,
                    &mut peak,
                    0,
                );
            }
            eprintln!("{{\"rust_large_live_bytes\":{},\"rust_large_peak_bytes\":{},\"rust_max_allocation_bytes\":{}}}", LARGE_LIVE.load(Relaxed), LARGE_PEAK.load(Relaxed), MAX_ALLOCATION.load(Relaxed));
            eprintln!("{{\"rust_live_bytes\":{},\"rust_peak_bytes\":{},\"sqlite_live_bytes\":{used},\"sqlite_peak_bytes\":{peak}}}", LIVE.load(Relaxed),PEAK.load(Relaxed));
        }
    });
}
