// ============================================================================
// FFI EXPORTS
// ============================================================================

use std::sync::OnceLock;
static GLOBAL_THREAD_POOL: OnceLock<std::sync::Mutex<Option<LockFreeThreadPool>>> = OnceLock::new();

/// Initialize the lock-free thread pool
#[unsafe(no_mangle)]
pub extern "C" fn naso_lf_thread_pool_init(worker_count: u64) -> u64 {
    let pool = LockFreeThreadPool::new(worker_count as usize);
    GLOBAL_THREAD_POOL.get_or_init(|| std::sync::Mutex::new(Some(pool)));
    // This is incomplete - needs to return status
    // For now, just indicate success
    1
}

/// Spawn a task on the lock-free thread pool
#[unsafe(no_mangle)]
pub extern "C" fn naso_lf_spawn(task_fn: *const u8, task_len: usize) -> u64 {
    // Placeholder for FFI task spawning
    1
}

/// Shutdown the lock-free thread pool
#[unsafe(no_mangle)]
pub extern "C" fn naso_lf_thread_pool_shutdown() -> u64 {
    if let Some(mutex) = GLOBAL_THREAD_POOL.get() {
        if let Some(mut pool) = mutex.lock().unwrap().take() {
            pool.shutdown();
        }
    }
    1
}