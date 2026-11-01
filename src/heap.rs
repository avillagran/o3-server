//! Reclaim transient snapshot storage on the worker that allocated it.
// Rust uses mimalloc, while bundled SQLite uses the platform C allocator.
pub fn collect() {
    // mimalloc's retired small pages otherwise survive until this worker makes
    // enough subsequent allocations, even with a zero purge delay.
    unsafe {
        libmimalloc_sys::mi_collect(true);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}
