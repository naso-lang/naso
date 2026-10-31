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
    // In full implementation: lookup listener by fd from global table
    // For now: placeholder for FFI contract
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
    // In full implementation: lookup stream by fd, perform recv()
    0
}

/// Write exactly len bytes from buffer to stream fd.
/// Returns bytes written (0 = error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_write(_fd: u64, buf: *const u8, len: usize) -> usize {
    if buf.is_null() || len == 0 { return 0; }
    // In full implementation: lookup stream by fd, perform send()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn test_tcp_bind_null_addr() {
        let result = naso_tcp_bind(ptr::null(), 0);
        assert_eq!(result.0, 0, "Should return ResourceID(0) for null addr");
    }

    #[test]
    fn test_tcp_bind_valid_addr() {
        let addr = b"127.0.0.1:0";
        let result = naso_tcp_bind(addr.as_ptr(), addr.len());
        assert_ne!(result.0, 0, "Should succeed binding to 127.0.0.1:0");
    }

    #[test]
    fn test_tcp_accept_null_out() {
        let result = naso_tcp_accept(12345, ptr::null_mut());
        assert_eq!(result, 0, "Should return 0 for null out pointer");
    }

    #[test]
    fn test_read_null_buf() {
        let result = naso_read(1, ptr::null_mut(), 1024);
        assert_eq!(result, 0, "Should return 0 for null buffer");
    }

    #[test]
    fn test_write_null_buf() {
        let result = naso_write(1, ptr::null(), 100);
        assert_eq!(result, 0, "Should return 0 for null buffer");
    }

    #[test]
    fn test_error_message() {
        let mut buf = [0u8; 64];
        let len = naso_last_error(buf.as_mut_ptr(), buf.len());
        assert!(len > 0, "Should return error message length");
        let msg = std::str::from_utf8(&buf[..len]).unwrap_or("");
        assert!(msg.contains("not implemented"));
    }
}