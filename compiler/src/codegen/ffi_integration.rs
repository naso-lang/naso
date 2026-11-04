//! FFI integration layer — links Naso extern declarations to Rust symbols
//! Generated from stdlib/src/std/sys/net.naso extern block and async.naso
//!
//! The FFI symbols (naso_tcp_bind etc.) are exported as extern "C" functions
//! in the naso-ffi crate. LLVM codegen emits extern declarations that the
//! linker resolves against libnaso_ffi.a at final binary link time.

/// Maps Naso extern fn names to Rust FFI symbol names
/// These are the actual #[unsafe(no_mangle)] names from crates/naso-ffi/src/lib.rs
pub static FFI_SYMBOLS: &[(&str, &str)] = &[
    ("naso_tcp_bind", "naso_tcp_bind"),
    ("naso_tcp_accept", "naso_tcp_accept"),
    ("naso_read", "naso_read"),
    ("naso_write", "naso_write"),
    ("naso_close", "naso_close"),
    ("naso_get_memory_stats", "naso_get_memory_stats"),
    ("naso_last_error", "naso_last_error"),
    ("naso_accept_get_client", "naso_accept_get_client"),
    // Async runtime FFI
    ("naso_runtime_new", "naso_runtime_new"),
    ("naso_runtime_free", "naso_runtime_free"),
    ("naso_runtime_run", "naso_runtime_run"),
    ("naso_spawn", "naso_spawn"),
    ("naso_yield", "naso_yield"),
    ("naso_future_new", "naso_future_new"),
    ("naso_future_complete", "naso_future_complete"),
    ("naso_future_get_result", "naso_future_get_result"),
    ("naso_future_free", "naso_future_free"),
    ("naso_channel_new", "naso_channel_new"),
    ("naso_sender_send", "naso_sender_send"),
    ("naso_receiver_recv", "naso_receiver_recv"),
    ("naso_channel_free", "naso_channel_free"),
    // Epoll/poll FFI
    ("naso_poll_add", "naso_poll_add"),
    ("naso_poll_wait", "naso_poll_wait"),
    ("naso_poll_set_waker", "naso_poll_set_waker"),
    ("naso_epoll_create", "naso_epoll_create"),
    ("naso_epoll_ctl", "naso_epoll_ctl"),
    ("naso_epoll_wait", "naso_epoll_wait"),
    // Async I/O FFI
    ("naso_tcp_accept_async", "naso_tcp_accept_async"),
    ("naso_tcp_connect_async", "naso_tcp_connect_async"),
    ("naso_read_async", "naso_read_async"),
    ("naso_write_async", "naso_write_async"),
    ("naso_timer_sleep_async", "naso_timer_sleep_async"),
    // Connection pool FFI
    ("naso_pool_create", "naso_pool_create"),
    ("naso_pool_get_connection", "naso_pool_get_connection"),
    ("naso_pool_return_connection", "naso_pool_return_connection"),
    ("naso_pool_free", "naso_pool_free"),
    ("naso_pool_get_stats", "naso_pool_get_stats"),
    // Simplified pool FFI (scalar args for Naso examples)
    ("naso_pool_create_simple", "naso_pool_create_simple"),
    ("naso_pool_get_connection_simple", "naso_pool_get_connection_simple"),
    ("naso_pool_return_connection_simple", "naso_pool_return_connection_simple"),
    ("naso_pool_close_simple", "naso_pool_close_simple"),
];

/// Returns the Rust symbol name for a Naso extern function
pub fn resolve_symbol(naso_fn: &str) -> Option<&'static str> {
    FFI_SYMBOLS
        .iter()
        .find(|(naso, _)| *naso == naso_fn)
        .map(|(_, rust)| *rust)
}
