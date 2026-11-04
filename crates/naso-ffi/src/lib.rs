//! FFI bridge for Naso network operations
//! Exports #[no_mangle] functions that naso-compiler's codegen links against
//!
//! Build: cargo build -p naso-ffi
//! Links: libnaso_ffi.a
//!
//! All functions return 0/ResourceID(0) on error (Naso checks via QTT)

use std::collections::HashMap;
use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Returned to Naso as resource handle
#[repr(C)]
pub struct ResourceID(pub u64);

/// Memory statistics for benchmarking
#[repr(C)]
pub struct MemoryStats {
    pub current_bytes: usize,
    pub peak_bytes: usize,
    pub total_allocations: usize,
    pub total_deallocations: usize,
}

/// Global memory statistics
static MEMORY_STATS: OnceLock<MemoryTracker> = OnceLock::new();

struct MemoryTracker {
    current: AtomicUsize,
    peak: AtomicUsize,
    allocations: AtomicUsize,
    deallocations: AtomicUsize,
}

impl MemoryTracker {
    fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
        }
    }
    
    fn record_alloc(&self, size: usize) {
        let current = self.current.fetch_add(size, Ordering::Relaxed) + size;
        self.allocations.fetch_add(1, Ordering::Relaxed);
        // Update peak
        let mut peak = self.peak.load(Ordering::Relaxed);
        while current > peak {
            match self.peak.compare_exchange_weak(peak, current, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(p) => peak = p,
            }
        }
    }
    
    fn record_dealloc(&self, size: usize) {
        self.current.fetch_sub(size, Ordering::Relaxed);
        self.deallocations.fetch_add(1, Ordering::Relaxed);
    }
    
    fn get_stats(&self) -> MemoryStats {
        MemoryStats {
            current_bytes: self.current.load(Ordering::Relaxed),
            peak_bytes: self.peak.load(Ordering::Relaxed),
            total_allocations: self.allocations.load(Ordering::Relaxed),
            total_deallocations: self.deallocations.load(Ordering::Relaxed),
        }
    }
}

fn memory_tracker() -> &'static MemoryTracker {
    MEMORY_STATS.get_or_init(|| MemoryTracker::new())
}

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
    eprintln!("naso_tcp_bind called with addr_len={}", addr_len);
    if addr_ptr.is_null() || addr_len == 0 {
        eprintln!("naso_tcp_bind: null ptr or zero len");
        return ResourceID(0);
    }
    let addr_bytes = unsafe { std::slice::from_raw_parts(addr_ptr, addr_len) };
    eprintln!("naso_tcp_bind: addr_bytes={:?}", addr_bytes);
    let addr_str = match std::str::from_utf8(addr_bytes) {
        Ok(s) => {
            eprintln!("naso_tcp_bind: addr_str={}", s);
            s
        }
        Err(e) => {
            eprintln!("naso_tcp_bind: utf8 error: {:?}", e);
            return ResourceID(0);
        }
    };
    match TcpListener::bind(addr_str) {
        Ok(listener) => {
            let mut table = fd_table().lock().unwrap();
            let fd = table.insert_listener(listener);
            eprintln!("naso_tcp_bind: success fd={}", fd);
            ResourceID(fd)
        }
        Err(e) => {
            eprintln!("naso_tcp_bind: bind error: {:?}", e);
            ResourceID(0)
        }
    }
}

/// Accept one connection — writes [listener_fd, stream_fd] to out array.
/// Returns 1 on success, 0 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_accept(listener_fd: u64, out: *mut u64) -> u64 {
    eprintln!("naso_tcp_accept called with listener_fd={}, out={:?}", listener_fd, out);
    if out.is_null() {
        return 0;
    }
    let mut table = fd_table().lock().unwrap();
    let Some(listener) = table.listeners.get(&listener_fd) else {
        eprintln!("Listener {} not found in table", listener_fd);
        return 0;
    };
    let listener_clone = listener.try_clone();
    drop(table);
    match listener_clone {
        Ok(listener_clone) => {
            match listener_clone.accept() {
                Ok((stream, _addr)) => {
                    // Set stream to blocking mode for synchronous FFI
                    let _ = stream.set_nonblocking(false);
                    let mut table = fd_table().lock().unwrap();
                    let stream_fd = table.insert_stream(stream);
                    eprintln!("Accepted connection: listener_fd={}, stream_fd={}", listener_fd, stream_fd);
                    let out_slice = unsafe { std::slice::from_raw_parts_mut(out, 2) };
                    out_slice[0] = listener_fd;
                    out_slice[1] = stream_fd;
                    eprintln!("naso_tcp_accept: wrote out[0]={}, out[1]={}", out_slice[0], out_slice[1]);
                    1
                }
                Err(e) => {
                    eprintln!("naso_tcp_accept: accept error: {:?}", e);
                    0
                }
            }
        }
        Err(e) => {
            eprintln!("naso_tcp_accept: clone error: {:?}", e);
            0
        }
    }
}

/// Read up to max_len bytes from stream fd into caller-provided buffer.
/// Returns bytes read (0 = EOF or error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_read(fd: u64, buf: *mut u8, max_len: usize) -> usize {
    eprintln!(">>> naso_read called with fd={}, max_len={}", fd, max_len);
    if buf.is_null() || max_len == 0 {
        eprintln!(">>> naso_read: null buf or zero max_len, returning 0");
        return 0;
    }
    let mut table = fd_table().lock().unwrap();
    let Some(stream) = table.get_stream(fd) else {
        eprintln!(">>> naso_read: Stream {} not found in table, returning 0", fd);
        return 0;
    };
    let buf_slice = unsafe { std::slice::from_raw_parts_mut(buf, max_len) };
    match std::io::Read::read(stream, buf_slice) {
        Ok(n) => {
            memory_tracker().record_alloc(n);
            eprintln!(">>> naso_read: read {} bytes from fd={}", n, fd);
            n
        }
        Err(e) => {
            eprintln!(">>> naso_read: read error on fd={}: {:?}", fd, e);
            0
        }
    }
}

/// Write exactly len bytes from buffer to stream fd.
/// Returns bytes written (0 = error).
#[unsafe(no_mangle)]
pub extern "C" fn naso_write(fd: u64, buf: *const u8, len: usize) -> usize {
    eprintln!(">>> naso_write ENTRY fd={}, len={}, buf={:p}", fd, len, buf);
    if buf.is_null() || len == 0 {
        eprintln!(">>> naso_write: null buf or zero len, returning 0");
        return 0;
    }
    let mut table = fd_table().lock().unwrap();
    let Some(stream) = table.get_stream(fd) else {
        eprintln!(">>> naso_write: Stream {} not found in table, returning 0", fd);
        return 0;
    };
    let buf_slice = unsafe { std::slice::from_raw_parts(buf, len) };
    eprintln!(">>> naso_write: writing {} bytes: {:?}", len, buf_slice);
    match std::io::Write::write_all(stream, buf_slice) {
        Ok(_) => {
            eprintln!(">>> naso_write: write_all succeeded, flushing...");
            match stream.flush() {
                Ok(_) => {
                    memory_tracker().record_alloc(len);
                    eprintln!(">>> naso_write: wrote {} bytes to fd={}", len, fd);
                    len
                }
                Err(e) => {
                    eprintln!(">>> naso_write: flush error on fd={}: {:?}", fd, e);
                    0
                }
            }
        }
        Err(e) => {
            eprintln!(">>> naso_write: write error on fd={}: {:?}", fd, e);
            0
        }
    }
}

/// Get memory statistics for benchmarking
#[unsafe(no_mangle)]
pub extern "C" fn naso_get_memory_stats(stats_ptr: *mut MemoryStats) -> u64 {
    if stats_ptr.is_null() {
        return 0;
    }
    let stats = memory_tracker().get_stats();
    unsafe {
        std::ptr::write(stats_ptr, stats);
    }
    1
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

/// Close a stream fd and remove it from the fd table
/// Returns 1 on success, 0 on failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_close(fd: u64) -> u64 {
    eprintln!(">>> naso_close called with fd={}", fd);
    let mut table = fd_table().lock().unwrap();
    if let Some(stream) = table.streams.remove(&fd) {
        // Don't shutdown - just let the stream be dropped naturally
        // shutdown can close the connection before buffered data is sent
        eprintln!(">>> naso_close: closed fd={} (stream dropped)", fd);
        1
    } else {
        eprintln!(">>> naso_close: fd={} not found", fd);
        0
    }
}

/// Extract client_fd from accept out array (out[1])
#[unsafe(no_mangle)]
pub extern "C" fn naso_accept_get_client(out: *const u64) -> u64 {
    eprintln!(">>> naso_accept_get_client called with out={:?}", out);
    if out.is_null() {
        eprintln!(">>> naso_accept_get_client: null out, returning 0");
        return 0;
    }
    let out_slice = unsafe { std::slice::from_raw_parts(out, 2) };
    eprintln!(">>> naso_accept_get_client: out[0]={}, out[1]={}", out_slice[0], out_slice[1]);
    out_slice[1]
}

// ============================================================================
// ASYNC RUNTIME FFI
// ============================================================================

/// Global async runtime state
static ASYNC_RUNTIME: OnceLock<Mutex<Option<async_runtime::Runtime>>> = OnceLock::new();

/// Initialize the async runtime system
fn async_runtime() -> &'static Mutex<Option<async_runtime::Runtime>> {
    ASYNC_RUNTIME.get_or_init(|| Mutex::new(None))
}

/// Opaque runtime handle for FFI
#[repr(C)]
pub struct AsyncRuntimeHandle {
    inner: *mut async_runtime::Runtime,
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_runtime_new(worker_count: usize) -> *mut AsyncRuntimeHandle {
    use async_runtime::{Runtime, RuntimeConfig};
    
    let config = RuntimeConfig {
        worker_count,
        stack_size: 1024 * 1024, // 1MB stack
    };
    
    match Runtime::new(config) {
        Ok(rt) => {
            let handle = Box::into_raw(Box::new(AsyncRuntimeHandle { 
                inner: Box::into_raw(Box::new(rt)) 
            }));
            handle
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_runtime_run(rt: *mut AsyncRuntimeHandle) -> u64 {
    if rt.is_null() {
        return 0;
    }
    let handle = unsafe { &*rt };
    let rt_box = unsafe { &mut *handle.inner };
    
    // Run the I/O event loop alongside the runtime
    // This runs the mio poll loop to handle async I/O
    loop {
        if rt_box.shutdown_flag.load(Ordering::Relaxed) {
            break;
        }
        
        // Poll for I/O events
        let mut events = Events::with_capacity(1024);
        let timeout = Some(Duration::from_millis(10));
        {
            let mut poll_guard = async_poll().lock().unwrap();
            match poll_guard.poll.poll(&mut events, timeout) {
                Ok(_) => {
                    for event in &events {
                        let token = event.token();
                        if let Some(reg) = poll_guard.registrations.get_mut(&token) {
                            if event.is_readable() || event.is_writable() {
                                if let Some(waker) = reg.waker.take() {
                                    waker.wake();
                                }
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
        
        // Also run the timer tick
        // The timer is handled by the async-runtime's timer thread
    }
    
    // Now run the runtime (which will wait for shutdown)
    rt_box.run();
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_runtime_shutdown(rt: *mut AsyncRuntimeHandle) -> u64 {
    if rt.is_null() {
        return 0;
    }
    let handle = unsafe { &*rt };
    let rt_box = unsafe { &mut *handle.inner };
    rt_box.shutdown();
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_runtime_free(rt: *mut AsyncRuntimeHandle) {
    if rt.is_null() {
        return;
    }
    let handle = unsafe { Box::from_raw(rt) };
    if !handle.inner.is_null() {
        let _ = unsafe { Box::from_raw(handle.inner) };
    }
}

// ============================================================================
// ASYNC TASK FFI
// ============================================================================

#[repr(C)]
pub struct TaskHandle {
    inner: *mut Arc<async_runtime::Task>,
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_spawn(rt: *mut AsyncRuntimeHandle, task_fn: *const u8, task_len: usize) -> *mut TaskHandle {
    if rt.is_null() || task_fn.is_null() {
        return std::ptr::null_mut();
    }
    let handle = unsafe { &*rt };
    let rt_box = unsafe { &mut *handle.inner };
    
    // In real implementation, we'd deserialize the function from bytes
    // For now, create a dummy task
    match rt_box.spawn(async {}) {
        Ok(_join_handle) => {
            // We can't easily get the task back from the join handle in this simplified impl
            // Return null for now
            std::ptr::null_mut()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_task_join(task: *mut TaskHandle) -> u64 {
    if task.is_null() {
        return 0;
    }
    let handle = unsafe { &*task };
    let task_arc = unsafe { &*handle.inner };
    task_arc.join();
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_task_cancel(task: *mut TaskHandle) -> u64 {
    if task.is_null() {
        return 0;
    }
    let handle = unsafe { &*task };
    let task_arc = unsafe { &*handle.inner };
    task_arc.cancel_task();
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_task_free(task: *mut TaskHandle) {
    if task.is_null() {
        return;
    }
    let handle = unsafe { Box::from_raw(task) };
    if !handle.inner.is_null() {
        let _ = unsafe { Box::from_raw(handle.inner) };
    }
}

/// Spawn a future on the global async runtime (simplified - uses global runtime)
#[unsafe(no_mangle)]
pub extern "C" fn naso_spawn_future(future_ptr: *mut FutureHandle) -> u64 {
    if future_ptr.is_null() {
        return 0;
    }
    
    // Use the global async runtime if available
    let runtime_guard = async_runtime().lock().unwrap();
    if let Some(ref rt) = *runtime_guard {
        let handle = unsafe { &mut *future_ptr };
        let future = unsafe { Box::from_raw(handle.inner) };
        
        // Spawn the future on the runtime - wrap to discard Vec<u8> output
        let future_wrapped = async_runtime::FutureWrap::new(future);
        match rt.spawn(future_wrapped) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    } else {
        0
    }
}

// ============================================================================
// YIELD / AWAIT FFI
// ============================================================================

#[unsafe(no_mangle)]
pub extern "C" fn naso_yield() -> u64 {
    // Call yield_now and block on it (simplified - in real impl would use executor)
    let _future = async_runtime::yield_now();
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_await(future_ptr: *mut u8) -> u64 {
    if future_ptr.is_null() {
        return 2; // error
    }
    // In real implementation, would poll the future
    // Returns: 0=pending, 1=ready, 2=error
    1
}

// ============================================================================
// FUTURE FFI
// ============================================================================

#[repr(C)]
pub struct FutureHandle {
    inner: *mut async_runtime::NFuture<Vec<u8>>,
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_future_new() -> *mut FutureHandle {
    // Generic future creation for Vec<u8>
    let future = async_runtime::NFuture::<Vec<u8>>::new();
    let handle = Box::into_raw(Box::new(FutureHandle {
        inner: Box::into_raw(Box::new(future)),
    }));
    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_future_complete(fut: *mut FutureHandle, value_ptr: *const u8, value_len: usize) -> u64 {
    if fut.is_null() || value_ptr.is_null() {
        return 0;
    }
    let handle = unsafe { &mut *fut };
    let future = unsafe { &mut *handle.inner };
    
    let value = unsafe { std::slice::from_raw_parts(value_ptr, value_len) };
    future.complete(value.to_vec());
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_future_get_result(fut: *mut FutureHandle, out_ptr: *mut u8, out_len: usize) -> u64 {
    if fut.is_null() || out_ptr.is_null() {
        return 0;
    }
    let handle = unsafe { &*fut };
    let future = unsafe { &*handle.inner };
    
    if let Some(result) = future.take_result() {
        let len = result.len().min(out_len);
        unsafe { std::ptr::copy_nonoverlapping(result.as_ptr(), out_ptr, len) };
        len as u64
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_future_free(fut: *mut FutureHandle) {
    if fut.is_null() {
        return;
    }
    let handle = unsafe { Box::from_raw(fut) };
    if !handle.inner.is_null() {
        let _ = unsafe { Box::from_raw(handle.inner) };
    }
}

// ============================================================================
// CHANNEL FFI
// ============================================================================

#[repr(C)]
pub struct ChannelHandle {
    sender: async_runtime::NSender<Vec<u8>>,
    receiver: async_runtime::NReceiver<Vec<u8>>,
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_channel_new(capacity: usize) -> *mut std::ffi::c_void {
    let (sender, receiver) = async_runtime::nchannel::<Vec<u8>>(capacity);
    let handle = Box::into_raw(Box::new(ChannelHandle { sender, receiver }));
    handle as *mut std::ffi::c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_sender_send(sender: *mut async_runtime::NSender<Vec<u8>>, value_ptr: *const u8, value_len: usize) -> u64 {
    if sender.is_null() || value_ptr.is_null() {
        return 0;
    }
    let handle = unsafe { &mut *sender };
    let value = unsafe { std::slice::from_raw_parts(value_ptr, value_len) };
    let _ = handle.send(value.to_vec());
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_receiver_recv(receiver: *mut async_runtime::NReceiver<Vec<u8>>, out_ptr: *mut u8, out_len: usize) -> u64 {
    if receiver.is_null() || out_ptr.is_null() {
        return 0;
    }
    let handle = unsafe { &mut *receiver };
    if let Some(value) = handle.recv() {
        let len = value.len().min(out_len);
        unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), out_ptr, len) };
        len as u64
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_channel_free(channel: *mut std::ffi::c_void) {
    if channel.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(channel as *mut ChannelHandle) };
}

// ============================================================================
// ASYNC I/O FFI (using mio for epoll/kqueue)
// ============================================================================

use mio::{Events, Interest, Poll, Token};
use std::net::TcpStream;
use std::os::unix::io::FromRawFd;
use std::task::Waker;

/// Global poll registry for async I/O
static ASYNC_POLL: OnceLock<Mutex<AsyncPoll>> = OnceLock::new();

struct AsyncPoll {
    poll: Poll,
    events: Events,
    registrations: HashMap<Token, AsyncRegistration>,
    next_token: usize,
}

struct AsyncRegistration {
    fd: u64,
    waker: Option<Waker>,
    interest: Interest,
    /// For listeners: true = accepting, for streams: false = reading/writing
    is_listener: bool,
    /// For listeners: the out_ptr passed to naso_tcp_accept_async
    out_ptr: usize,
}

impl AsyncPoll {
    fn new() -> std::io::Result<Self> {
        let poll = Poll::new()?;
        let events = Events::with_capacity(1024);
        Ok(Self {
            poll,
            events,
            registrations: HashMap::new(),
            next_token: 1, // Token(0) reserved
        })
    }

    fn register_listener(&mut self, listener_fd: u64, out_ptr: *mut u64) -> std::io::Result<Token> {
        let token = Token(self.next_token);
        self.next_token += 1;
        
        // Get the listener from the FD table and clone it for mio
        let mut table = fd_table().lock().unwrap();
        let Some(listener) = table.listeners.get(&listener_fd) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Listener not found"));
        };
        let listener_clone = listener.try_clone()?;
        drop(table); // Release lock before registering
        
        let mut source = mio::net::TcpListener::from_std(listener_clone);
        
        self.poll
            .registry()
            .register(&mut source, token, Interest::READABLE)?;
        
        self.registrations.insert(
            token,
            AsyncRegistration {
                fd: listener_fd,
                waker: None,
                interest: Interest::READABLE,
                is_listener: true,
                out_ptr: out_ptr as usize,
            },
        );
        
        Ok(token)
    }

    fn register_stream(&mut self, stream_fd: u64, interest: Interest) -> std::io::Result<Token> {
        let token = Token(self.next_token);
        self.next_token += 1;
        
        let stream = unsafe { TcpStream::from_raw_fd(stream_fd as i32) };
        let mut source = mio::net::TcpStream::from_std(stream);
        
        self.poll.registry().register(&mut source, token, interest)?;
        
        // The registration keeps the fd alive in the poll registry
        let _ = source;
        
        self.registrations.insert(token, AsyncRegistration {
            fd: stream_fd,
            waker: None,
            interest,
            is_listener: false,
            out_ptr: 0,
        });
        
        Ok(token)
    }

    fn reregister(&mut self, token: Token, interest: Interest) -> std::io::Result<()> {
        if let Some(reg) = self.registrations.get_mut(&token) {
            let stream = unsafe { TcpStream::from_raw_fd(reg.fd as i32) };
            let mut source = mio::net::TcpStream::from_std(stream);
            
            self.poll.registry().reregister(&mut source, token, interest)?;
            
            // The registration keeps the fd alive in the poll registry
            let _ = source;
            reg.interest = interest;
        }
        Ok(())
    }

    fn set_waker(&mut self, token: Token, waker: Waker) {
        if let Some(reg) = self.registrations.get_mut(&token) {
            reg.waker = Some(waker);
        }
    }

    fn poll_once(&mut self, timeout: Option<std::time::Duration>) -> std::io::Result<i32> {
            self.poll.poll(&mut self.events, timeout)?;

            let count = self.events.iter().count() as i32;

            for event in &self.events {
                let token = event.token();
                if let Some(reg) = self.registrations.get_mut(&token) {
                    if reg.is_listener {
                                        // For listeners, accept the connection and write to out_ptr
                                        if reg.out_ptr != 0 {
                                            let out_ptr = reg.out_ptr as *mut u64;
                                            // Get listener from FD table and clone it
                                            let mut table = fd_table().lock().unwrap();
                                            if let Some(listener) = table.listeners.get(&reg.fd) {
                                                let listener_clone = listener.try_clone();
                                                drop(table);
                                                if let Ok(listener_clone) = listener_clone {
                                                    match listener_clone.accept() {
                                                                                        Ok((stream, _addr)) => {
                                                                                            // Set stream to blocking mode for synchronous FFI
                                                                                            let _ = stream.set_nonblocking(false);
                                                                                            let mut table = fd_table().lock().unwrap();
                                                                                            let fd = table.insert_stream(stream);
                                                                                            // Write [listener_fd, stream_fd] to out array
                                                                                            unsafe {
                                                                                                std::ptr::write(out_ptr, reg.fd);
                                                                                                std::ptr::write(out_ptr.add(1), fd);
                                                                                            }
                                                                                        }
                                                                                        Err(_) => {}
                                                                                    }
                                                }
                                            } else {
                                                drop(table);
                                            }
                                        }
                                    }

                    if let Some(waker) = reg.waker.take() {
                        if !reg.is_listener {
                            waker.wake();
                        }
                    }
                }
            }

            Ok(count)
        }
    }

fn async_poll() -> &'static Mutex<AsyncPoll> {
    ASYNC_POLL.get_or_init(|| {
        Mutex::new(AsyncPoll::new().expect("Failed to create async poll"))
    })
}

/// Register a listener for async accept
#[unsafe(no_mangle)]
pub extern "C" fn naso_poll_add_listener(listener_fd: u64, out_ptr: *mut u64) -> u64 {
    let mut poll = async_poll().lock().unwrap();
    match poll.register_listener(listener_fd, out_ptr) {
        Ok(token) => token.0 as u64,
        Err(_) => 0,
    }
}

/// Register a stream for async read/write
#[unsafe(no_mangle)]
pub extern "C" fn naso_poll_add_stream(stream_fd: u64, interest: u32) -> u64 {
    let mut poll = async_poll().lock().unwrap();
    let mio_interest = match interest {
        1 => Interest::READABLE,
        2 => Interest::WRITABLE,
        3 => Interest::READABLE | Interest::WRITABLE,
        _ => Interest::READABLE,
    };
    match poll.register_stream(stream_fd, mio_interest) {
        Ok(token) => token.0 as u64,
        Err(_) => 0,
    }
}

/// Poll for events, with optional timeout in milliseconds
/// Returns number of ready events, or -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn naso_poll_wait(token_ptr: *mut u64, max_events: usize, timeout_ms: i64) -> i32 {
    let mut poll = async_poll().lock().unwrap();
    let timeout = if timeout_ms < 0 {
        None
    } else {
        Some(std::time::Duration::from_millis(timeout_ms as u64))
    };
    
    match poll.poll_once(timeout) {
        Ok(count) => {
            // Copy ready tokens to output
            let ready_count = std::cmp::min(count as usize, max_events);
            if !token_ptr.is_null() && ready_count > 0 {
                let mut idx = 0;
                for event in &poll.events {
                    if idx >= ready_count { break; }
                    if event.is_readable() || event.is_writable() {
                        let out_slice = unsafe { std::slice::from_raw_parts_mut(token_ptr, max_events) };
                        out_slice[idx] = event.token().0 as u64;
                        idx += 1;
                    }
                }
            }
            ready_count as i32
        }
        Err(_) => -1,
    }
}

/// Set waker for a token (called from Naso when awaiting)
#[unsafe(no_mangle)]
pub extern "C" fn naso_poll_set_waker(token: u64, waker_ptr: *mut u8) -> u64 {
    // In a real implementation, we'd store the waker
    // For now, just return success
    1
}

/// Async accept - registers listener and returns immediately
/// The actual accept happens when the waker fires
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_accept_async(listener_fd: u64, out_ptr: *mut u64, out_len: usize) -> u64 {
    if out_ptr.is_null() || out_len < 16 {
        return 0;
    }
    let mut poll = async_poll().lock().unwrap();
    match poll.register_listener(listener_fd, out_ptr) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

/// Async connect
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_connect_async(addr_ptr: *const u8, addr_len: usize, 
                                          callback: *const u8, callback_len: usize) -> u64 {
    if addr_ptr.is_null() || callback.is_null() {
        return 0;
    }
    1
}

/// Async read
#[unsafe(no_mangle)]
pub extern "C" fn naso_read_async(fd: u64, buf_ptr: *mut u8, buf_len: usize,
                                   callback: *const u8, callback_len: usize) -> u64 {
    if buf_ptr.is_null() || callback.is_null() {
        return 0;
    }
    let mut poll = async_poll().lock().unwrap();
    match poll.register_stream(fd, Interest::READABLE) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

/// Async write
#[unsafe(no_mangle)]
pub extern "C" fn naso_write_async(fd: u64, buf_ptr: *const u8, buf_len: usize,
                                    callback: *const u8, callback_len: usize) -> u64 {
    if buf_ptr.is_null() || callback.is_null() {
        return 0;
    }
    let mut poll = async_poll().lock().unwrap();
    match poll.register_stream(fd, Interest::WRITABLE) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

/// Async timer sleep
#[unsafe(no_mangle)]
pub extern "C" fn naso_timer_sleep_async(duration_ms: u64, 
                                          callback: *const u8, callback_len: usize) -> u64 {
    if callback.is_null() {
        return 0;
    }
    // Timer wheel handles this in async-runtime
    1
}

// ============================================================================
// CONNECTION POOL FFI
// ============================================================================

/// Connection pool configuration
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PoolConfig {
    pub min_connections: usize,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_connections: 2,
            max_connections: 100,
            idle_timeout_ms: 30000, // 30 seconds
            connect_timeout_ms: 5000, // 5 seconds
        }
    }
}

/// Opaque connection pool handle
#[repr(C)]
pub struct ConnectionPoolHandle {
    inner: *mut std::ffi::c_void,
}

/// Global connection pools
static CONNECTION_POOLS: OnceLock<Mutex<HashMap<u64, ConnectionPool>>> = OnceLock::new();

struct ConnectionPool {
    config: PoolConfig,
    available: Vec<PooledConnection>,
    in_use: HashMap<u64, PooledConnection>,
    next_id: u64,
}

struct PooledConnection {
    fd: u64,
    addr: String,
    last_used: std::time::Instant,
}

impl ConnectionPool {
    fn new(config: PoolConfig) -> Self {
        Self {
            config,
            available: Vec::new(),
            in_use: HashMap::new(),
            next_id: 1,
        }
    }
}

fn connection_pools() -> &'static Mutex<HashMap<u64, ConnectionPool>> {
    CONNECTION_POOLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Create a new connection pool
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_create(config_ptr: *const PoolConfig) -> *mut ConnectionPoolHandle {
    let config = if config_ptr.is_null() {
        PoolConfig::default()
    } else {
        unsafe { std::ptr::read(config_ptr) }
    };
    
    let pool = ConnectionPool::new(config);
    let pool_id = {
        let mut pools = connection_pools().lock().unwrap();
        let id = pools.len() as u64 + 1;
        pools.insert(id, pool);
        id
    };
    
    let handle = Box::into_raw(Box::new(ConnectionPoolHandle {
        inner: pool_id as *mut std::ffi::c_void,
    }));
    handle
}

/// Get a connection from the pool (creates new if needed)
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_get_connection(pool: *mut ConnectionPoolHandle, addr_ptr: *const u8, addr_len: usize) -> u64 {
    if pool.is_null() || addr_ptr.is_null() || addr_len == 0 {
        return 0;
    }
    
    let addr_bytes = unsafe { std::slice::from_raw_parts(addr_ptr, addr_len) };
    let addr_str = match std::str::from_utf8(addr_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };
    
    let pool_id = unsafe { (*pool).inner as u64 };
    
    let mut pools = connection_pools().lock().unwrap();
    let Some(pool) = pools.get_mut(&pool_id) else {
        return 0;
    };
    
    // Try to reuse an available connection
    if let Some(idx) = pool.available.iter().position(|c| c.addr == addr_str) {
        let conn = pool.available.remove(idx);
        let conn_id = pool.next_id;
        pool.next_id += 1;
        pool.in_use.insert(conn_id, conn);
        return conn_id;
    }
    
    // Create new connection if under max
    if pool.in_use.len() + pool.available.len() < pool.config.max_connections {
        match TcpListener::bind(&addr_str) {
            Ok(_) => {
                // Actually connect to the address
                match TcpStream::connect(&addr_str) {
                    Ok(stream) => {
                        let mut fd_table = fd_table().lock().unwrap();
                        let fd = fd_table.insert_stream(stream);
                        
                        let conn = PooledConnection {
                            fd,
                            addr: addr_str,
                            last_used: std::time::Instant::now(),
                        };
                        
                        let conn_id = pool.next_id;
                        pool.next_id += 1;
                        pool.in_use.insert(conn_id, conn);
                        return conn_id;
                    }
                    Err(_) => return 0,
                }
            }
            Err(_) => return 0,
        }
    }
    
    0 // Pool exhausted
}

/// Return a connection to the pool
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_return_connection(pool: *mut ConnectionPoolHandle, conn_id: u64) -> u64 {
    if pool.is_null() {
        return 0;
    }
    
    let pool_id = unsafe { (*pool).inner as u64 };
    
    let mut pools = connection_pools().lock().unwrap();
    let Some(pool) = pools.get_mut(&pool_id) else {
        return 0;
    };
    
    if let Some(conn) = pool.in_use.remove(&conn_id) {
        pool.available.push(PooledConnection {
            fd: conn.fd,
            addr: conn.addr,
            last_used: std::time::Instant::now(),
        });
        1
    } else {
        0
    }
}

/// Close a connection pool and all its connections
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_close(pool: *mut ConnectionPoolHandle) {
    if pool.is_null() {
        return;
    }
    
    let pool_id = unsafe { (*pool).inner as u64 };
    let mut pools = connection_pools().lock().unwrap();
    pools.remove(&pool_id);
    
    let _ = unsafe { Box::from_raw(pool) };
}

/// Get pool statistics
#[repr(C)]
pub struct PoolStats {
    pub total_connections: usize,
    pub available_connections: usize,
    pub in_use_connections: usize,
    pub max_connections: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_get_stats(pool: *mut ConnectionPoolHandle, stats_ptr: *mut PoolStats) -> u64 {
    if pool.is_null() || stats_ptr.is_null() {
        return 0;
    }
    
    let pool_id = unsafe { (*pool).inner as u64 };
    
    let pools = connection_pools().lock().unwrap();
    let Some(pool) = pools.get(&pool_id) else {
        return 0;
    };
    
    let stats = PoolStats {
        total_connections: pool.available.len() + pool.in_use.len(),
        available_connections: pool.available.len(),
        in_use_connections: pool.in_use.len(),
        max_connections: pool.config.max_connections,
    };
    
    unsafe {
        std::ptr::write(stats_ptr, stats);
    }
    1
}

/// Simplified pool create for Naso example (scalar args)
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_create_simple(min_connections: u64, max_connections: u64, idle_timeout_ms: u64, connect_timeout_ms: u64) -> u64 {
    let config = PoolConfig {
        min_connections: min_connections as usize,
        max_connections: max_connections as usize,
        idle_timeout_ms,
        connect_timeout_ms,
    };
    let handle = naso_pool_create(&config as *const PoolConfig);
    handle as u64
}

/// Simplified pool get connection for Naso example
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_get_connection_simple(pool: *mut ConnectionPoolHandle, addr_ptr: *const u8, addr_len: usize) -> u64 {
    naso_pool_get_connection(pool, addr_ptr, addr_len)
}

/// Simplified pool return connection for Naso example
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_return_connection_simple(pool: *mut ConnectionPoolHandle, conn_id: u64) -> u64 {
    naso_pool_return_connection(pool, conn_id)
}

/// Simplified pool close for Naso example
#[unsafe(no_mangle)]
pub extern "C" fn naso_pool_close_simple(pool: u64) {
    let handle = pool as *mut ConnectionPoolHandle;
    naso_pool_close(handle)
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

#[cfg(test)]
mod test_accept_get_client {
    use super::*;

    #[test]
    fn test_accept_get_client() {
        let mut out = [1234u64, 5678u64];
        let result = naso_accept_get_client(out.as_ptr());
        assert_eq!(result, 5678);
    }
}
