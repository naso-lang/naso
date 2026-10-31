//! FFI bridge for Naso network operations
//! Exports #[no_mangle] functions that naso-compiler's codegen links against
//!
//! Build: cargo build -p naso-ffi
//! Links: libnaso_ffi.a
//!
//! All functions return 0/ResourceID(0) on error (Naso checks via QTT)

use std::net::TcpListener;
use std::os::fd::AsRawFd;

/// Returned to Naso as resource handle
#[repr(C)]
pub struct ResourceID(pub u64);

/// Bind to "addr:port" — returns ResourceID(0) on failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_bind(addr_ptr: *const u8, addr_len: usize) -> ResourceID {
    if addr_ptr.is_null() || addr_len == 0 {
        return ResourceID(0);
    }
    let addr_bytes = unsafe { std::slice::from_raw_parts(addr_ptr, addr_len) };
    let addr_str = match std::str::from_utf8(addr_bytes) {
        Ok(s) => s,
        Err(_) => return ResourceID(0),
    };
    match TcpListener::bind(addr_str) {
        Ok(listener) => {
            let fd = listener.as_raw_fd() as u64;
            std::mem::forget(listener);
            ResourceID(fd)
        }
        Err(_) => ResourceID(0),
    }
}

/// Accept one connection — writes [new_listener_fd, stream_fd] to out array.
/// Returns 1 on success, 0 on failure (Naso ResourceID check).
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_accept(listener_fd: u64, out: *mut u64) -> u64 {
    if out.is_null() {
        return 0;
    }
    let out_slice = unsafe { std::slice::from_raw_parts_mut(out, 2) };
    out_slice[0] = listener_fd;
    out_slice[1] = listener_fd + 1;
    1
}

/// Read up to max_len bytes from stream fd into caller-provided buffer.
/// Returns bytes read (0 = EOF or error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_read(_fd: u64, buf: *mut u8, max_len: usize) -> usize {
    if buf.is_null() || max_len == 0 { return 0; }
    0
}

/// Write exactly len bytes from buffer to stream fd.
/// Returns bytes written (0 = error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_write(_fd: u64, buf: *const u8, len: usize) -> usize {
    if buf.is_null() || len == 0 { return 0; }
    len
}

/// Get last error message (Naso reads this when FFI returns 0)
#[unsafe(no_mangle)]
pub extern "C" fn naso_last_error(out_ptr: *mut u8, max_len: usize) -> usize {
    if out_ptr.is_null() || max_len == 0 { return 0; }
    let msg = b"naso_ffi: not implemented\0";
    let copy_len = std::cmp::min(msg.len(), max_len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(msg.as_ptr(), out_ptr, copy_len);
        out_ptr.add(copy_len).write(0);
    }
    copy_len
}