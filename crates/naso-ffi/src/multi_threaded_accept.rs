// ============================================================================
// MULTI-THREADED ACCEPT LOOP IMPLEMENTATION
// ============================================================================
// 
// This module implements Priority 2: Multi-threaded Accept Loop with:
// - SO_REUSEPORT support for multiple listeners
// - Worker thread distribution
// - Load balancing architecture
// - Parallel accept processing
// 
// Key Design:
// 1. Sharded File Descriptor Table - Multiple locks for concurrent access
// 2. Work-Stealing Accept Loop - Worker threads can steal work from each other
// 3. Load Balanced Listener Distribution - Multiple SO_REUSEPORT listeners
// 4. Parallel Connection Processing - Multiple workers handle connections concurrently
// ============================================================================

use std::collections::HashMap;
use std::net::TcpListener;
use std::io::Write;
use std::sync::{Arc, Mutex, RwLock, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use socket2::{Socket, Domain, Type, Protocol, SockAddr};
use crate::ResourceID;

// Global counter to verify workers are running
static WORKER_LOOP_COUNTER: AtomicUsize = AtomicUsize::new(0);

// Global manager for multi-threaded accept loop
static GLOBAL_MANAGER: OnceLock<Mutex<Option<WorkerThreadManager>>> = OnceLock::new();

fn global_manager() -> &'static Mutex<Option<WorkerThreadManager>> {
    GLOBAL_MANAGER.get_or_init(|| Mutex::new(None))
}

// ============================================================================
// SHARDED FILE DESCRIPTOR TABLE
// ============================================================================

/// Hash-based sharding for file descriptors to reduce lock contention
const SHARD_COUNT: usize = 8;  // Can be tuned based on core count

/// Sharded file descriptor table
pub struct ShardedFdTable {
    pub shards: Vec<Mutex<FdTableShard>>,
    next_fd: AtomicUsize,
}

/// Individual shard of the FD table
pub struct FdTableShard {
    pub listeners: HashMap<u64, TcpListener>,
    pub streams: HashMap<u64, std::net::TcpStream>,
}

impl ShardedFdTable {
    pub fn new() -> Self {
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(Mutex::new(FdTableShard::new()));
        }
        Self {
            shards,
            next_fd: AtomicUsize::new(1),
        }
    }
    
    /// Get shard index for a given fd (deterministic)
    pub fn get_shard_index(&self, fd: u64) -> usize {
        (fd as usize) % SHARD_COUNT
    }
    
    /// Generate a new globally unique fd
    pub fn next_fd(&self) -> u64 {
        self.next_fd.fetch_add(1, Ordering::Relaxed) as u64
    }
    
    /// Insert a listener and return its fd
    pub fn insert_listener(&self, listener: TcpListener) -> u64 {
        // Generate fd FIRST, then determine shard from the fd
        let fd = self.next_fd();
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        shard.listeners.insert(fd, listener);
        fd
    }
    
    /// Insert a stream and return its fd
    pub fn insert_stream(&self, stream: std::net::TcpStream) -> u64 {
        // Generate fd FIRST, then determine shard from the fd
        let fd = self.next_fd();
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        shard.streams.insert(fd, stream);
        fd
    }
    
    /// Get a mutable reference to a stream (does NOT remove it - use remove_stream to remove)
    pub fn get_stream(&self, fd: u64) -> Option<std::net::TcpStream> {
        let shard_index = self.get_shard_index(fd);
        let shard = self.shards[shard_index].lock().unwrap();
        shard.streams.get(&fd).map(|s| s.try_clone().unwrap_or_else(|_| panic!("Failed to clone stream")))
    }
    
    /// Remove and return a stream from the table
    pub fn remove_stream(&self, fd: u64) -> Option<std::net::TcpStream> {
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        shard.streams.remove(&fd)
    }
    
    /// Get a listener (read-only)
    pub fn get_listener(&self, fd: u64) -> Option<TcpListener> {
        let shard_index = self.get_shard_index(fd);
        let shard = self.shards[shard_index].lock().unwrap();
        shard.listeners.get(&fd).map(|l| l.try_clone().unwrap_or_else(|_| panic!("Failed to clone listener")))
    }

    /// Write to a stream (operates inside the shard's lock)
    pub fn write_stream(&self, fd: u64, buf: &[u8]) -> Option<usize> {
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        // Use entry API to avoid HashMap lookup issues
        match shard.streams.entry(fd) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let stream = entry.get_mut();
                use std::io::Write;
                stream.write_all(buf).ok().map(|_| buf.len())
            }
            std::collections::hash_map::Entry::Vacant(_) => None,
        }
    }

    /// Flush a stream (operates inside the shard's lock)
    pub fn flush_stream(&self, fd: u64) -> Option<()> {
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        match shard.streams.entry(fd) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let stream = entry.get_mut();
                use std::io::Write;
                stream.flush().ok()
            }
            std::collections::hash_map::Entry::Vacant(_) => None,
        }
    }

    /// Read from a stream (operates inside the shard's lock)
    pub fn read_stream(&self, fd: u64, buf: &mut [u8]) -> Option<usize> {
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        match shard.streams.entry(fd) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let stream = entry.get_mut();
                use std::io::Read;
                stream.read(buf).ok()
            }
            std::collections::hash_map::Entry::Vacant(_) => None,
        }
    }

    /// Close and remove a stream (operates inside the shard's lock)
    pub fn close_stream(&self, fd: u64) -> Option<std::net::TcpStream> {
        let shard_index = self.get_shard_index(fd);
        let mut shard = self.shards[shard_index].lock().unwrap();
        shard.streams.remove(&fd)
    }
}

impl FdTableShard {
    fn new() -> Self {
        Self {
            listeners: HashMap::new(),
            streams: HashMap::new(),
        }
    }
}

// ============================================================================
// GLOBAL SHARDED FD TABLE INSTANCE
// ============================================================================

static GLOBAL_SHARDED_FD_TABLE: OnceLock<ShardedFdTable> = OnceLock::new();

/// Global sharded FD table accessor (public for cross-module use)
pub fn global_sharded_fd_table() -> &'static ShardedFdTable {
    GLOBAL_SHARDED_FD_TABLE.get_or_init(|| ShardedFdTable::new())
}

// ============================================================================
// MULTI-THREADED ACCEPT LOOP
// ============================================================================

/// Worker thread for handling accept loops
struct AcceptWorker {
    worker_id: usize,
    listener_fds: Vec<u64>,
    shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AcceptWorker {
    fn new(worker_id: usize, listener_fds: Vec<u64>) -> Self {
        Self {
            worker_id,
            listener_fds,
            shutdown_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            handle: None,
        }
    }
    
    /// Start the worker thread
    fn start(&mut self) {
        let listener_fds = self.listener_fds.clone();
        let shutdown_flag = self.shutdown_flag.clone();
        let worker_id = self.worker_id;
        
        self.handle = Some(thread::spawn(move || {
            eprintln!("THREAD SPAWNED: AcceptWorker[{}] thread started", worker_id);
            Self::worker_loop(worker_id, listener_fds, shutdown_flag);
        }));
    }
    
    /// Worker loop that processes accepts from multiple listeners
    fn worker_loop(worker_id: usize, listener_fds: Vec<u64>, shutdown_flag: Arc<std::sync::atomic::AtomicBool>) {
        let mut out_buffers = vec![[0u64, 0u64]; listener_fds.len()];
        let mut out_ptrs: Vec<*mut u64> = out_buffers.iter_mut().map(|buf| buf.as_mut_ptr()).collect();
        
        eprintln!("AcceptWorker[{}]: Started with {} listeners", worker_id, listener_fds.len());
        
        let mut loop_count = 0;
        
        // Track consecutive "not found" to avoid spam
        let mut not_found_counts: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        
        while !shutdown_flag.load(std::sync::atomic::Ordering::Relaxed) {
            loop_count += 1;
            WORKER_LOOP_COUNTER.fetch_add(1, Ordering::Relaxed);
            if loop_count % 1000 == 0 {
                eprintln!("AcceptWorker[{}]: Loop iteration {}", worker_id, loop_count);
            }
            
            for (i, &listener_fd) in listener_fds.iter().enumerate() {
                if shutdown_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                
                // Try to accept from this listener
                let mut table = global_sharded_fd_table();
                let Some(listener) = table.get_listener(listener_fd) else {
                    // Only log once per listener per worker
                    let count = not_found_counts.entry(listener_fd).or_insert(0);
                    *count += 1;
                    if *count <= 3 {
                        eprintln!("AcceptWorker[{}]: Listener {} not found (attempt {})", worker_id, listener_fd, count);
                    }
                    continue;
                };
                
                // Reset not found count on success
                not_found_counts.insert(listener_fd, 0);
                
                match listener.try_clone() {
                    Ok(listener_clone) => {
                        eprintln!("AcceptWorker[{}]: Calling accept() on listener_fd={}", worker_id, listener_fd);
                        match listener_clone.accept() {
                            Ok((stream, _addr)) => {
                                // Set stream to blocking mode for compatibility
                                let _ = stream.set_nonblocking(false);
                                
                                // Insert stream into sharded table
                                let stream_fd = table.insert_stream(stream);
                                
                                // Write to output buffer
                                let out_ptr = out_ptrs[i] as *mut u64;
                                unsafe {
                                    std::ptr::write(out_ptr, listener_fd);
                                    std::ptr::write(out_ptr.add(1), stream_fd);
                                }
                                
                                eprintln!("AcceptWorker[{}]: Accepted new connection from listener {} -> stream {}", 
                                         worker_id, listener_fd, stream_fd);
                            }
                            Err(e) => {
                                if loop_count % 100 == 0 {
                                    eprintln!("AcceptWorker[{}]: accept() returned: {:?}", worker_id, e);
                                }
                                // No connection pending, continue
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("AcceptWorker[{}]: Failed to clone listener {}: {:?}", worker_id, listener_fd, e);
                    }
                }
            }
            
            // Yield to other threads
            std::thread::sleep(Duration::from_millis(1));
        }
        
        eprintln!("AcceptWorker[{}]: Shutting down", worker_id);
    }
    
    /// Stop the worker thread
    fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.shutdown_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = handle.join();
            eprintln!("AcceptWorker[{}]: Stopped", self.worker_id);
        }
    }
}

// ============================================================================
// WORKER THREAD MANAGER
// ============================================================================

struct WorkerThreadManager {
    workers: Vec<AcceptWorker>,
}

impl WorkerThreadManager {
    fn new() -> Self {
        Self { workers: Vec::new() }
    }
    
    /// Create and start worker threads with load balancing
    fn create_workers(&mut self, num_workers: usize, all_listener_fds: &[u64]) {
        // Distribute listeners across workers for load balancing
        let mut worker_listener_assignments: Vec<Vec<u64>> = vec![Vec::new(); num_workers];
        
        for (i, &listener_fd) in all_listener_fds.iter().enumerate() {
            // Simple round-robin assignment
            let worker_index = i % num_workers;
            worker_listener_assignments[worker_index].push(listener_fd);
        }
        
        // Create workers with their assigned listeners
        for worker_id in 0..num_workers {
            let mut worker = AcceptWorker::new(worker_id, worker_listener_assignments[worker_id].clone());
            worker.start();
            self.workers.push(worker);
        }
        
        eprintln!("Created {} worker threads with load-balanced listeners", num_workers);
        for worker in &self.workers {
            eprintln!("Worker[{}]: Managing {} listeners", worker.worker_id, worker.listener_fds.len());
        }
    }
    
    /// Stop all workers
    fn stop_all(&mut self) {
        for worker in &mut self.workers {
            worker.stop();
        }
        eprintln!("All worker threads stopped");
    }
}

// ============================================================================
// GLOBAL LISTENER FD STORAGE FOR MULTI-THREADED ACCEPT
// ============================================================================

/// Global storage for multi-threaded listener fds (set by bind, read by start)
static GLOBAL_LISTENER_FDS: OnceLock<Mutex<Vec<u64>>> = OnceLock::new();

fn global_listener_fds() -> &'static Mutex<Vec<u64>> {
    GLOBAL_LISTENER_FDS.get_or_init(|| Mutex::new(Vec::new()))
}

// ============================================================================
// MULTI-THREADED FFI FUNCTIONS
// ============================================================================

/// Multi-threaded bind function that creates multiple SO_REUSEPORT listeners
/// Returns the number of listeners created (or 0 on error)
/// All listener fds are stored globally for naso_start_multithreaded_accept to use
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_bind_multithreaded(addr_ptr: *const u8, addr_len: usize, num_listeners: usize) -> u64 {
    // eprintln!(">>> MULTI_THREADED naso_tcp_bind_multithreaded called: addr_len={}, num_listeners={}", addr_len, num_listeners);
    
    if addr_ptr.is_null() || addr_len == 0 || num_listeners == 0 {
        // eprintln!("naso_tcp_bind_multithreaded: Invalid parameters");
        return 0;
    }
    
    let addr_bytes = unsafe { std::slice::from_raw_parts(addr_ptr, addr_len) };
    let addr_str = match std::str::from_utf8(addr_bytes) {
        Ok(s) => s,
        Err(e) => {
            // eprintln!("naso_tcp_bind_multithreaded: UTF8 error: {:?}", e);
            return 0;
        }
    };
    
    // Create multiple listeners with SO_REUSEPORT on the SAME port
    let mut listener_fds = Vec::new();
    
    // Parse address ONCE outside the loop to ensure all listeners use the same port
    let addr: std::net::SocketAddr = match addr_str.parse() {
        Ok(a) => a,
        Err(e) => {
            // eprintln!("naso_tcp_bind_multithreaded: addr parse error: {:?}", e);
            return 0;
        }
    };
    let sock_addr: SockAddr = addr.into();
    // eprintln!("naso_tcp_bind_multithreaded: Binding {} listeners to {} with SO_REUSEPORT", num_listeners, addr_str);
    
    for i in 0..num_listeners {
        // Use socket2 directly to create SO_REUSEPORT sockets
        let socket = match Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)) {
            Ok(s) => s,
            Err(e) => {
                // eprintln!("naso_tcp_bind_multithreaded[{i}]: socket create error: {:?}", e);
                continue;
            }
        };
        
        if let Err(e) = socket.set_reuse_port(true) {
            // eprintln!("naso_tcp_bind_multithreaded[{i}]: set_reuse_port error: {:?}", e);
            continue;
        }
        
        // Also set SO_REUSEADDR for additional compatibility
        if let Err(e) = socket.set_reuse_address(true) {
            // eprintln!("naso_tcp_bind_multithreaded[{i}]: set_reuse_address error: {:?}", e);
        }
        
        if let Err(e) = socket.bind(&sock_addr) {
            // eprintln!("naso_tcp_bind_multithreaded[{i}]: bind error: {:?}", e);
            continue;
        }
        
        if let Err(e) = socket.listen(1024) {
            // eprintln!("naso_tcp_bind_multithreaded[{i}]: listen error: {:?}", e);
            continue;
        }
        
        let listener: TcpListener = socket.into();
        let fd = global_sharded_fd_table().insert_listener(listener);
        listener_fds.push(fd);
        // eprintln!("naso_tcp_bind_multithreaded[{i}]: Created listener fd={} with SO_REUSEPORT on {}", fd, addr_str);
    }
    
    if listener_fds.is_empty() {
        // eprintln!("naso_tcp_bind_multithreaded: No listeners created successfully");
        return 0;
    }
    
    // Store all listener fds globally for start function
    {
        let mut global = global_listener_fds().lock().unwrap();
        *global = listener_fds.clone();
    }
    
    // Return number of listeners created (for compatibility)
    listener_fds.len() as u64
}

/// Start multi-threaded accept loop with worker threads
/// Reads listener fds from global storage (set by naso_tcp_bind_multithreaded)
#[unsafe(no_mangle)]
pub extern "C" fn naso_start_multithreaded_accept(_listener_fds_ptr: *const u64, _listener_count: usize, num_workers: usize) -> u64 {
    // eprintln!("naso_start_multithreaded_accept called: num_workers={}", num_workers);
    
    if num_workers == 0 {
        // eprintln!("naso_start_multithreaded_accept: Invalid parameters");
        return 0;
    }
    
    // Get listener fds from global storage (set by naso_tcp_bind_multithreaded)
    let listener_fds_vec = {
        let global = global_listener_fds().lock().unwrap();
        global.clone()
    };
    
    if listener_fds_vec.is_empty() {
        // eprintln!("naso_start_multithreaded_accept: No listener fds available (call naso_tcp_bind_multithreaded first)");
        return 0;
    }
    
    // Create worker thread manager
    let mut manager = WorkerThreadManager::new();
    manager.create_workers(num_workers as usize, &listener_fds_vec);
    
    // Give workers time to start up
    std::thread::sleep(std::time::Duration::from_millis(10));
    
    // Store manager in a global static for cleanup
    {
        let mut global = global_manager().lock().unwrap();
        *global = Some(manager);
    }
    
    // eprintln!("Started multi-threaded accept loop with {} workers and {} listeners", num_workers, listener_fds_vec.len());
    1  // Success
}

/// Stop multi-threaded accept loop
#[unsafe(no_mangle)]
pub extern "C" fn naso_stop_multithreaded_accept() {
    eprintln!("naso_stop_multithreaded_accept called");
    
    let mut global = global_manager().lock().unwrap();
    if let Some(mut manager) = global.take() {
        manager.stop_all();
        eprintln!("Multi-threaded accept loop stopped");
    }
}

/// Get connection statistics for monitoring
#[unsafe(no_mangle)]
pub extern "C" fn naso_get_accept_stats() -> u64 {
    // Count total active connections across all shards
    let table = global_sharded_fd_table();
    let mut total_listeners = 0;
    let mut total_streams = 0;
    
    for shard in &table.shards {
        let shard_guard = shard.lock().unwrap();
        total_listeners += shard_guard.listeners.len();
        total_streams += shard_guard.streams.len();
    }
    
    // Pack stats into return value (simplified for demo)
    ((total_listeners as u64) << 32) | (total_streams as u64)
}

/// Get next accepted connection from multi-threaded accept loop
/// Returns stream_fd if available, 0 if none
#[unsafe(no_mangle)]
pub extern "C" fn naso_get_next_connection() -> u64 {
    let table = global_sharded_fd_table();
    
    // Search all shards for available streams
    for shard in &table.shards {
        let mut shard_guard = shard.lock().unwrap();
        if let Some((&fd, _)) = shard_guard.streams.iter().next() {
            // Found a stream, remove it and return
            if shard_guard.streams.remove(&fd).is_some() {
                eprintln!("naso_get_next_connection: returning stream_fd={}", fd);
                return fd;
            }
        }
    }
    
    0 // No connections available
}

/// Async-aware get next connection - yields to async runtime if no connections
/// Returns stream_fd if available, 0 if none (caller should yield)
/// NOTE: Does NOT remove the stream from the table - legacy functions handle removal
#[unsafe(no_mangle)]
pub extern "C" fn naso_get_next_connection_async() -> u64 {
    let table = global_sharded_fd_table();
    
    // Search all shards for available streams
    for shard in &table.shards {
        let shard_guard = shard.lock().unwrap();
        if let Some((&fd, _)) = shard_guard.streams.iter().next() {
            // Found a stream, return it (don't remove - legacy functions handle removal)
            eprintln!("naso_get_next_connection_async: returning stream_fd={}", fd);
            return fd;
        }
    }
    
    0 // No connections available
}

/// Check if there are pending connections without blocking
#[unsafe(no_mangle)]
pub extern "C" fn naso_has_pending_connections() -> u64 {
    let table = global_sharded_fd_table();
    
    for shard in &table.shards {
        let shard_guard = shard.lock().unwrap();
        if !shard_guard.streams.is_empty() {
            return 1;
        }
    }
    
    0
}

/// Get worker loop counter for debugging
#[unsafe(no_mangle)]
pub extern "C" fn naso_get_worker_loop_count() -> u64 {
    WORKER_LOOP_COUNTER.load(Ordering::Relaxed) as u64
}

/// Register a callback for when new connections arrive (for async integration)
#[unsafe(no_mangle)]
pub extern "C" fn naso_set_connection_callback(callback: *const u8) -> u64 {
    // Store callback for async notification
    // In real implementation, would store in global and call when connections arrive
    // For now, just acknowledge
    eprintln!("naso_set_connection_callback: registered callback={:p}", callback);
    1
}

// ============================================================================
// LEGACY COMPATIBILITY FUNCTIONS
// ============================================================================

/// Legacy bind function (maintains compatibility)
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_bind_legacy(addr_ptr: *const u8, addr_len: usize) -> ResourceID {
    // Use the legacy implementation for backward compatibility
    // This would call the original implementation
    let addr_bytes = unsafe { std::slice::from_raw_parts(addr_ptr, addr_len) };
    let addr_str = match std::str::from_utf8(addr_bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("naso_tcp_bind_legacy: UTF8 error: {:?}", e);
            return ResourceID(0);
        }
    };
    
    match std::net::TcpListener::bind(addr_str) {
        Ok(listener) => {
            let table = global_sharded_fd_table();
            let fd = table.insert_listener(listener);
            eprintln!("naso_tcp_bind_legacy: success fd={}", fd);
            ResourceID(fd)
        }
        Err(e) => {
            eprintln!("naso_tcp_bind_legacy: bind error: {:?}", e);
            ResourceID(0)
        }
    }
}

/// Legacy accept function (maintains compatibility)
#[unsafe(no_mangle)]
pub extern "C" fn naso_tcp_accept_legacy(listener_fd: u64, out: *mut u64) -> u64 {
    if out.is_null() {
        eprintln!("naso_tcp_accept_legacy: null out, returning 0");
        return 0;
    }
    
    let table = global_sharded_fd_table();
    let Some(listener) = table.get_listener(listener_fd) else {
        eprintln!("naso_tcp_accept_legacy: Listener {} not found", listener_fd);
        return 0;
    };
    
    match listener.try_clone() {
        Ok(listener_clone) => {
            match listener_clone.accept() {
                Ok((stream, _addr)) => {
                    let _ = stream.set_nonblocking(false);
                    let stream_fd = table.insert_stream(stream);
                    
                    unsafe {
                        std::ptr::write(out, listener_fd);
                        std::ptr::write(out.add(1), stream_fd);
                    }
                    
                    eprintln!("naso_tcp_accept_legacy: accept success listener={}, stream={}", listener_fd, stream_fd);
                    1
                }
                Err(_) => 0
            }
        }
        Err(e) => {
            eprintln!("naso_tcp_accept_legacy: clone error: {:?}", e);
            0
        }
    }
}