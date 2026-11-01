//! Reclaim freed libc arenas after snapshots, connections and idle workers.
// Rust and bundled SQLite share the platform allocator.
/// Bound arena fragmentation and keep large transient buffers independently releasable.
/// Set these before starting threads; explicit operator allocator settings win.
pub fn configure() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
            unsafe {
                libc::mallopt(libc::M_ARENA_MAX, 2);
            }
        }
        if std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_none() {
            // Fix the threshold rather than letting glibc raise it after a burst.
            unsafe {
                libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
            }
        }
    }
}

pub fn collect() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Network actors also retire pages, independently of snapshot/HTTP completion.
/// Trim before a runtime worker sleeps, at most once a second per worker.
pub fn on_thread_park() {
    use std::{
        cell::Cell,
        time::{Duration, Instant},
    };
    thread_local! {
        static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    LAST.with(|last| {
        let now = Instant::now();
        if last
            .get()
            .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1))
        {
            last.set(Some(now));
            collect();
        }
    });
}
