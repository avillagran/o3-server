//! Opt-in aggregate heap diagnostics. Never emits allocation contents or identifiers.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
pub struct Measured;
fn add(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = mimalloc::MiMalloc.alloc(l);
        if !p.is_null() {
            add(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        mimalloc::MiMalloc.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = mimalloc::MiMalloc.alloc_zeroed(l);
        if !p.is_null() {
            add(l.size());
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let p = mimalloc::MiMalloc.realloc(p, l, n);
        if !p.is_null() {
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
            eprintln!("{{\"rust_live_bytes\":{},\"rust_peak_bytes\":{},\"sqlite_live_bytes\":{used},\"sqlite_peak_bytes\":{peak}}}", LIVE.load(Relaxed),PEAK.load(Relaxed));
        }
    });
}
