// Rust HTTP server using naso-ffi - equivalent to optimized_server.naso
// This benchmarks the Rust code path vs Naso compiler output

use naso_ffi::{
    naso_tcp_bind_reuseport, naso_tcp_accept_async, naso_accept_get_client,
    naso_read_async, naso_write_async, naso_uring_poll, naso_yield,
    naso_close, naso_runtime_new, naso_runtime_run, ResourceID,
};
use std::ptr;
use std::os::raw::c_char;

const ADDR: &[u8] = b"127.0.0.1:8090\0";
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello World\n";

fn handle_client(client_fd: u64) {
    let mut buf = [0u8; 1024];
    let _ = unsafe { naso_read_async(client_fd, buf.as_mut_ptr(), buf.len(), ptr::null_mut(), 0) };
    let _ = unsafe { naso_write_async(client_fd, RESPONSE.as_ptr() as *mut u8, RESPONSE.len(), ptr::null_mut(), 0) };
    let _ = unsafe { naso_close(client_fd) };
}

fn accept_loop(fd: u64) {
    let mut out = [0u64; 4];
    loop {
        if unsafe { naso_tcp_accept_async(fd, out.as_mut_ptr(), 32, ptr::null_mut(), 0) } != 0 {
            let client_fd = unsafe { naso_accept_get_client(out.as_mut_ptr()) };
            handle_client(client_fd);
        }
        unsafe { naso_yield() };
    }
}

fn main() {
    let _ = unsafe { naso_uring_poll() }; // Initialize
    
    let fd = unsafe { naso_tcp_bind_reuseport(ADDR.as_ptr(), ADDR.len()) };
    if fd.0 == 0 {
        eprintln!("Failed to bind");
        return;
    }
    
    let runtime = unsafe { naso_runtime_new(4) };
    
    // Spawn accept loop
    std::thread::spawn(move || {
        accept_loop(fd.0);
    });
    
    // Run runtime
    unsafe { naso_runtime_run(runtime) };
}