//! FFI integration layer — links Naso extern declarations to Rust symbols
//! Generated from stdlib/src/std/sys/net.naso extern block
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
    ("naso_last_error", "naso_last_error"),
];

/// Returns the Rust symbol name for a Naso extern function
pub fn resolve_symbol(naso_fn: &str) -> Option<&'static str> {
    FFI_SYMBOLS
        .iter()
        .find(|(naso, _)| *naso == naso_fn)
        .map(|(_, rust)| *rust)
}
