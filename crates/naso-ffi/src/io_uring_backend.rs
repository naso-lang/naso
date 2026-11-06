//! io_uring backend for high-performance async I/O
//! This replaces the mio-based epoll backend with io_uring for batched syscalls

use io_uring::{opcode, types, IoUring, cqueue};
use std::collections::HashMap;
use std::net::TcpStream;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Waker;

/// Buffer pool for zero-copy I/O
pub struct BufferPool {
    buffers: Vec<Vec<u8>>,
    free_indices: Vec<usize>,
    buffer_size: usize,
}

impl BufferPool {
    pub fn new(buffer_count: usize, buffer_size: usize) -> Self {
        let mut buffers = Vec::with_capacity(buffer_count);
        let mut free_indices = Vec::with_capacity(buffer_count);
        
        for i in 0..buffer_count {
            buffers.push(vec![0u8; buffer_size]);
            free_indices.push(i);
        }
        
        Self {
            buffers,
            free_indices,
            buffer_size,
        }
    }
    
    pub fn acquire(&mut self) -> Option<(usize, &mut [u8])> {
        self.free_indices
            .pop()
            .map(|idx| (idx, &mut self.buffers[idx] as &mut [u8]))
    }
    
    pub fn release(&mut self, idx: usize) {
        self.free_indices.push(idx);
    }
    
    pub fn get_ptr(&self, idx: usize) -> *const u8 {
        self.buffers[idx].as_ptr()
    }
    
    pub fn get_mut_ptr(&mut self, idx: usize) -> *mut u8 {
        self.buffers[idx].as_mut_ptr()
    }
}

/// io_uring operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UringOp {
    Accept,
    Read,
    Write,
    Connect,
}

/// Pending io_uring operation
struct PendingOp {
    op_type: UringOp,
    fd: u64,
    buf_idx: Option<usize>,
    offset: usize,
    len: usize,
    callback: usize,
    waker: Option<Waker>,
    out_ptr: usize, // For accept: *mut u64 to write [listener_fd, conn_fd]
}

/// io_uring-based async poller
pub struct UringPoller {
    ring: IoUring,
    pending: HashMap<u64, PendingOp>, // keyed by user_data
    buffer_pool: BufferPool,
    next_user_data: u64,
    listener_fds: HashMap<u64, u64>, // token -> listener_fd
}

impl UringPoller {
    pub fn new(capacity: u32) -> std::io::Result<Self> {
        let ring = IoUring::new(capacity)?;
        Ok(Self {
            ring,
            pending: HashMap::new(),
            buffer_pool: BufferPool::new(1024, 8192), // 1K buffers of 8KB each
            next_user_data: 1,
            listener_fds: HashMap::new(),
        })
    }
    
    /// Submit an async accept operation
    pub fn submit_accept(
        &mut self,
        listener_fd: u64,
        out_ptr: *mut u64,
        callback: usize,
    ) -> std::io::Result<u64> {
        let user_data = self.next_user_data;
        self.next_user_data += 1;
        
        let op = opcode::Accept::new(
            types::Fd(listener_fd as i32),
            std::ptr::null_mut(), // sockaddr
            std::ptr::null_mut(), // socklen
        )
        .build()
        .user_data(user_data);
        
        unsafe {
            self.ring.submission().push(&op).map_err(|e| std::io::Error::other(e))?;
        }
        
        self.pending.insert(user_data, PendingOp {
            op_type: UringOp::Accept,
            fd: listener_fd,
            buf_idx: None,
            offset: 0,
            len: 0,
            callback,
            waker: None,
            out_ptr: out_ptr as usize,
        });
        
        self.listener_fds.insert(user_data, listener_fd);
        
        Ok(user_data)
    }
    
    /// Submit an async read operation with zero-copy buffer
    pub fn submit_read(
        &mut self,
        fd: u64,
        buf_ptr: *mut u8,
        len: usize,
        callback: usize,
    ) -> std::io::Result<u64> {
        let user_data = self.next_user_data;
        self.next_user_data += 1;
        
        // Use fixed buffer if available, otherwise direct pointer
        let buf_idx = if buf_ptr.is_null() {
            // Acquire from pool for zero-copy
            self.buffer_pool.acquire().map(|(idx, _buf)| idx)
        } else {
            None
        };
        
        let op = opcode::Read::new(
            types::Fd(fd as i32),
            if let Some(idx) = buf_idx {
                self.buffer_pool.get_mut_ptr(idx) as *mut _
            } else {
                buf_ptr as *mut _
            },
            len as u32,
        )
        .build()
        .user_data(user_data);
        
        unsafe {
            self.ring.submission().push(&op).map_err(|e| std::io::Error::other(e))?;
        }
        
        self.pending.insert(user_data, PendingOp {
            op_type: UringOp::Read,
            fd,
            buf_idx,
            offset: 0,
            len,
            callback,
            waker: None,
            out_ptr: 0,
        });
        
        Ok(user_data)
    }
    
    /// Submit an async write operation
    pub fn submit_write(
        &mut self,
        fd: u64,
        buf_ptr: *const u8,
        len: usize,
        callback: usize,
    ) -> std::io::Result<u64> {
        let user_data = self.next_user_data;
        self.next_user_data += 1;
        
        let op = opcode::Write::new(
            types::Fd(fd as i32),
            buf_ptr as *const _,
            len as u32,
        )
        .build()
        .user_data(user_data);
        
        unsafe {
            self.ring.submission().push(&op).map_err(|e| std::io::Error::other(e))?;
        }
        
        self.pending.insert(user_data, PendingOp {
            op_type: UringOp::Write,
            fd,
            buf_idx: None,
            offset: 0,
            len,
            callback,
            waker: None,
            out_ptr: 0,
        });
        
        Ok(user_data)
    }
    
    /// Submit multiple operations in a batch (key optimization)
    pub fn submit_batch(&mut self) -> std::io::Result<u32> {
        self.ring.submit().map_err(|e| std::io::Error::other(e)).map(|v| v as u32)
    }
    
    /// Wait for completions and process them
    pub fn wait_and_process(&mut self, min_completions: u32) -> std::io::Result<u32> {
        let submitted = self.submit_batch()?;
        if submitted == 0 {
            return Ok(0);
        }
        
        // Wait for at least min_completions
        let mut completed = 0;
        while completed < min_completions {
            let cqes: Vec<_> = self.ring.completion().collect();
            for cqe in cqes {
                completed += 1;
                self.process_completion(cqe);
            }
            if completed < min_completions {
                // Need more completions, wait
                self.ring.submit_and_wait(1)?;
            }
        }
        
        Ok(completed as u32)
    }
    
    /// Process a single completion queue entry
    fn process_completion(&mut self, cqe: io_uring::cqueue::Entry) {
        let user_data = cqe.user_data();
        let result = cqe.result();
        
        if let Some(mut op) = self.pending.remove(&user_data) {
            match op.op_type {
                UringOp::Accept => {
                    if result >= 0 {
                        let conn_fd = result as u64;
                        let out_ptr = op.out_ptr as *mut u64;
                        unsafe {
                            std::ptr::write(out_ptr, op.fd);      // listener_fd
                            std::ptr::write(out_ptr.add(1), conn_fd); // conn_fd
                        }
                    }
                    
                    // Call the callback
                    if op.callback != 0 {
                        let callback: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(op.callback) };
                        callback(std::ptr::null_mut());
                    }
                    
                    // Re-submit accept for the same listener (keep-alive accept loop)
                    let _ = self.submit_accept(op.fd, op.out_ptr as *mut u64, op.callback);
                }
                
                UringOp::Read => {
                    let bytes_read = if result >= 0 { result as usize } else { 0 };
                    
                    // If using buffer pool, data is in pool buffer
                    // In real implementation, would copy or pass buffer to callback
                    
                    if op.callback != 0 {
                        let callback: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(op.callback) };
                        callback(std::ptr::null_mut());
                    }
                    
                    // Release buffer back to pool if used
                    if let Some(idx) = op.buf_idx {
                        self.buffer_pool.release(idx);
                    }
                }
                
                UringOp::Write => {
                    let _bytes_written = if result >= 0 { result as usize } else { 0 };
                    
                    if op.callback != 0 {
                        let callback: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(op.callback) };
                        callback(std::ptr::null_mut());
                    }
                }
                
                UringOp::Connect => {
                    // Handle connect completion
                    if op.callback != 0 {
                        let callback: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(op.callback) };
                        callback(std::ptr::null_mut());
                    }
                }
            }
        }
    }
    
    /// Poll for completions without blocking (for integration with runtime)
    pub fn poll_completions(&mut self) -> std::io::Result<u32> {
        let cqes: Vec<_> = self.ring.completion().collect();
        let mut count = 0;
        for cqe in cqes {
            count += 1;
            self.process_completion(cqe);
        }
        Ok(count)
    }
}

/// Global io_uring poller instance
static URING_POLLER: OnceLock<Mutex<UringPoller>> = OnceLock::new();

fn uring_poller() -> &'static Mutex<UringPoller> {
    URING_POLLER.get_or_init(|| {
        Mutex::new(UringPoller::new(4096).expect("Failed to create io_uring"))
    })
}

/// FFI: Initialize io_uring backend
#[unsafe(no_mangle)]
pub extern "C" fn naso_uring_init() -> u64 {
    match UringPoller::new(4096) {
        Ok(poller) => {
            let _ = URING_POLLER.set(Mutex::new(poller));
            1
        }
        Err(_) => 0,
    }
}

/// FFI: Submit async accept with io_uring
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_accept_uring(
    listener_fd: u64,
    out_ptr: *mut u64,
    callback: *const u8,
) -> u64 {
    let mut poller = uring_poller().lock().unwrap();
    match poller.submit_accept(listener_fd, out_ptr, callback as usize) {
        Ok(user_data) => user_data,
        Err(_) => 0,
    }
}

/// FFI: Submit async read with io_uring (zero-copy)
#[unsafe(no_mangle)]
pub extern "C" fn naso_read_uring(
    fd: u64,
    buf_ptr: *mut u8,
    len: usize,
    callback: *const u8,
) -> u64 {
    let mut poller = uring_poller().lock().unwrap();
    match poller.submit_read(fd, buf_ptr, len, callback as usize) {
        Ok(user_data) => user_data,
        Err(_) => 0,
    }
}

/// FFI: Submit async write with io_uring
#[unsafe(no_mangle)]
pub extern "C" fn naso_write_uring(
    fd: u64,
    buf_ptr: *const u8,
    len: usize,
    callback: *const u8,
) -> u64 {
    let mut poller = uring_poller().lock().unwrap();
    match poller.submit_write(fd, buf_ptr, len, callback as usize) {
        Ok(user_data) => user_data,
        Err(_) => 0,
    }
}

/// FFI: Batch submit all pending operations
#[unsafe(no_mangle)]
pub extern "C" fn naso_uring_submit() -> u64 {
    let mut poller = uring_poller().lock().unwrap();
    match poller.submit_batch() {
        Ok(count) => count as u64,
        Err(_) => 0,
    }
}

/// FFI: Wait for and process completions
#[unsafe(no_mangle)]
pub extern "C" fn naso_uring_wait(min_completions: u64) -> u64 {
    let mut poller = uring_poller().lock().unwrap();
    match poller.wait_and_process(min_completions as u32) {
        Ok(count) => count as u64,
        Err(_) => 0,
    }
}

/// FFI: Poll completions non-blocking
#[unsafe(no_mangle)]
pub extern "C" fn naso_uring_poll() -> u64 {
    let mut poller = uring_poller().lock().unwrap();
    match poller.poll_completions() {
        Ok(count) => count as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_uring_creation() {
        let poller = UringPoller::new(256);
        assert!(poller.is_ok());
    }
    
    #[test]
    fn test_buffer_pool() {
        let mut pool = BufferPool::new(10, 1024);
        let buf = pool.acquire();
        assert!(buf.is_some());
        assert_eq!(buf.unwrap().len(), 1024);
    }
}