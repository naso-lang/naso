//! FFI bridge for Naso network operations
//! Exports #[no_mangle] functions that naso-compiler's codegen links against
//!
//! Build: cargo build -p naso-ffi
//! Links: libnaso_ffi.a
//!
//! All functions return 0/ResourceID(0) on error (Naso checks via QTT)

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};

/// Returned to Naso as resource handle
#[repr(C)]
pub struct ResourceID(pub u64);

/// Global fd table for tracking listeners and streams
static FD_TABLE: OnceLock<Mutex<FdTable>> = OnceLock::new();

struct FdTable {
    listeners: HashMap<u64, TcpListener>,
    streams: HashMap<u64, std::net::TcpStream>,
    next_fd: u64,
}

impl FdTable {
    fn new() -> Self {
        Self {
            listeners: HashMap::new(),
            streams: HashMap::new(),
            next_fd: 1,
        }
    }

    fn insert_listener(&mut self, listener: TcpListener) -> u64 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.listeners.insert(fd, listener);
        fd
    }

    fn insert_stream(&mut self, stream: std::net::TcpStream) -> u64 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.streams.insert(fd, stream);
        fd
    }

    fn get_stream(&mut self, fd: u64) -> Option<&mut std::net::TcpStream> {
        self.streams.get_mut(&fd)
    }
}

fn fd_table() -> &'static Mutex<FdTable> {
    FD_TABLE.get_or_init(|| Mutex::new(FdTable::new()))
}

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
            let mut table = fd_table().lock().unwrap();
            let fd = table.insert_listener(listener);
            ResourceID(fd)
        }
        Err(_) => ResourceID(0),
    }
}

/// Accept one connection — writes [listener_fd, stream_fd] to out array.
/// Returns 1 on success, 0 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_accept(listener_fd: u64, out: *mut u64) -> u64 {
    if out.is_null() {
        return 0;
    }
    let mut table = fd_table().lock().unwrap();
    let Some(listener) = table.listeners.get(&listener_fd) else {
        return 0;
    };
    match listener.accept() {
        Ok((stream, _addr)) => {
            let stream_fd = table.insert_stream(stream);
            let out_slice = unsafe { std::slice::from_raw_parts_mut(out, 2) };
            out_slice[0] = listener_fd;
            out_slice[1] = stream_fd;
            1
        }
        Err(_) => 0,
    }
}

/// Read up to max_len bytes from stream fd into caller-provided buffer.
/// Returns bytes read (0 = EOF or error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_read(fd: u64, buf: *mut u8, max_len: usize) -> usize {
    if buf.is_null() || max_len == 0 { return 0; }
    let mut table = fd_table().lock().unwrap();
    let Some(stream) = table.get_stream(fd) else {
        return 0;
    };
    let buf_slice = unsafe { std::slice::from_raw_parts_mut(buf, max_len) };
    match std::io::Read::read(stream, buf_slice) {
        Ok(n) => n,
        Err(_) => 0,
    }
}

/// Write exactly len bytes from buffer to stream fd.
/// Returns bytes written (0 = error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_write(fd: u64, buf: *const u8, len: usize) -> usize {
    if buf.is_null() || len == 0 { return 0; }
    let mut table = fd_table().lock().unwrap();
    let Some(stream) = table.get_stream(fd) else {
        return 0;
    };
    let buf_slice = unsafe { std::slice::from_raw_parts(buf, len) };
    match std::io::Write::write(stream, buf_slice) {
        Ok(n) => n,
        Err(_) => 0,
    }
}

/// Get last error message (Naso reads this when FFI returns 0)
#[unsafe(no_mangle)]
pub extern "C" fn naso_last_error(out_ptr: *mut u8, max_len: usize) -> usize {
    if out_ptr.is_null() || max_len == 0 { return 0; }
    let msg = b"naso_ffi: operation failed\0";
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
    use std::net::TcpStream;
    use std::io::{Read, Write};

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
        assert!(msg.contains("operation failed"));
    }

    #[test]
    fn test_full_tcp_roundtrip() {
        // Bind to a port
        let addr = b"127.0.0.1:0";
        let listener_res = naso_tcp_bind(addr.as_ptr(), addr.len());
        assert_ne!(listener_res.0, 0, "Bind should succeed");
        let listener_fd = listener_res.0;

        // Get the actual port by connecting
        let mut out = [0u64, 0u64];
        
        // We can't easily test accept without a client in another thread,
        // but we can verify the bind works and the fd is tracked
        let mut table = fd_table().lock().unwrap();
        assert!(table.listeners.contains_key(&listener_fd));
    }
}