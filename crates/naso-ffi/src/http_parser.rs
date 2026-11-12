// ============================================================================
// HTTP Parser FFI
// ============================================================================
// This module provides HTTP request parsing and response building functions
// with C-compatible FFI interface for Naso.
// ============================================================================

use std::ptr;
use std::time::{Instant, Duration};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// Connection state for Keep-Alive support
#[derive(Debug, Clone)]
struct ConnectionState {
    pub idle_since: Instant,
    pub request_count: u64,
    pub last_activity: Instant,
    pub keep_alive_timeout: Duration,
    pub max_requests: u64,
    pub current_requests: u64,
    pub pipelined_requests: Vec<Vec<u8>>, // Buffer for pipelined requests
    pub is_active: bool,
}

impl ConnectionState {
    fn new(keep_alive_timeout_ms: u64, max_requests: u64) -> Self {
        let now = Instant::now();
        Self {
            idle_since: now,
            request_count: 0,
            last_activity: now,
            keep_alive_timeout: Duration::from_millis(keep_alive_timeout_ms),
            max_requests,
            current_requests: 0,
            pipelined_requests: Vec::new(),
            is_active: true,
        }
    }
    
    fn update_activity(&mut self) {
        self.last_activity = Instant::now();
        if !self.is_active {
            self.is_active = true;
        }
    }
    
    fn is_timed_out(&self) -> bool {
        self.last_activity.elapsed() > self.keep_alive_timeout
    }
    
    fn can_accept_request(&mut self) -> bool {
        if !self.is_active {
            return false;
        }
        if self.current_requests >= self.max_requests {
            return false;
        }
        true
    }
    
    fn increment_requests(&mut self) -> bool {
        if self.request_count >= self.max_requests {
            return false;
        }
        self.request_count += 1;
        self.current_requests += 1;
        self.update_activity();
        true
    }
    
    fn decrement_requests(&mut self) {
        if self.current_requests > 0 {
            self.current_requests -= 1;
        }
        self.update_activity();
    }
}

// Global connection manager for Keep-Alive support
static CONNECTION_MANAGER: OnceLock<Mutex<ConnectionManager>> = OnceLock::new();

struct ConnectionManager {
    connections: HashMap<u64, ConnectionState>, // fd -> connection state
    default_timeout_ms: u64,
    default_max_requests: u64,
}

impl ConnectionManager {
    fn new() -> Self {
        Self {
            connections: HashMap::new(),
            default_timeout_ms: 30000, // 30 seconds default
            default_max_requests: 100,  // 100 requests per connection
        }
    }
    
    fn get_or_create_connection(&mut self, fd: u64) -> &mut ConnectionState {
        self.connections.entry(fd).or_insert_with(|| {
            ConnectionState::new(self.default_timeout_ms, self.default_max_requests)
        })
    }
    
    fn cleanup_timed_out_connections(&mut self) {
        let mut timed_out_fds: Vec<u64> = Vec::new();
        for (fd, state) in &self.connections {
            if state.is_timed_out() {
                timed_out_fds.push(*fd);
            }
        }
        
        for fd in timed_out_fds {
            if let Some(_) = self.connections.remove(&fd) {
                eprintln!("Cleaned up timed out connection fd={}", fd);
            }
        }
    }
    
    fn add_pipelined_request(&mut self, fd: u64, request_data: Vec<u8>) {
        if let Some(state) = self.connections.get_mut(&fd) {
            if state.can_accept_request() {
                state.pipelined_requests.push(request_data);
                state.increment_requests();
            }
        }
    }
    
    fn get_pipelined_request(&mut self, fd: u64) -> Option<Vec<u8>> {
        if let Some(state) = self.connections.get_mut(&fd) {
            state.decrement_requests();
            state.pipelined_requests.pop_front()
        } else {
            None
        }
    }
}

fn connection_manager() -> &'static Mutex<ConnectionManager> {
    CONNECTION_MANAGER.get_or_init(|| Mutex::new(ConnectionManager::new()))
}

fn parse_connection_header(request_data: &[u8]) -> (bool, u64, u64) {
    let mut has_keep_alive = false;
    let mut timeout_ms = 30000; // default 30 seconds
    let mut max_requests = 100; // default 100 requests
    
    let request_str = match std::str::from_utf8(request_data) {
        Ok(s) => s,
        Err(_) => return (has_keep_alive, timeout_ms, max_requests),
    };
    
    let lines: Vec<&str> = request_str.lines().collect();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        
        let line_lower = line.to_lowercase();
        
        // Check for Connection header
        if line_lower.starts_with("connection: ") {
            let connection_value = line[12..].trim();
            if connection_value.to_lowercase().contains("keep-alive") {
                has_keep_alive = true;
            }
        }
        
        // Check for Keep-Alive header
        if line_lower.starts_with("keep-alive: ") {
            let ka_value = line[12..].trim();
            let mut parts = ka_value.split(' ');
            while let Some(part) = parts.next() {
                if let Some(value) = part.strip_prefix("timeout=") {
                    if let Ok(val) = value.parse::<u64>() {
                        timeout_ms = val * 1000; // Convert seconds to milliseconds
                    }
                } else if let Some(value) = part.strip_prefix("max=") {
                    if let Ok(val) = value.parse::<u64>() {
                        max_requests = val;
                    }
                }
            }
        }
    }
    
    (has_keep_alive, timeout_ms, max_requests)
}

// Connection state for Keep-Alive support
#[derive(Debug, Clone)]
struct ConnectionState {
    pub idle_since: Instant,
    pub request_count: u64,
    pub last_activity: Instant,
    pub keep_alive_timeout: Duration,
    pub max_requests: u64,
    pub current_requests: u64,
    pub pipelined_requests: Vec<Vec<u8>>, // Buffer for pipelined requests
    pub is_active: bool,
}

impl ConnectionState {
    fn new(keep_alive_timeout_ms: u64, max_requests: u64) -> Self {
        let now = Instant::now();
        Self {
            idle_since: now,
            request_count: 0,
            last_activity: now,
            keep_alive_timeout: Duration::from_millis(keep_alive_timeout_ms),
            max_requests,
            current_requests: 0,
            pipelined_requests: Vec::new(),
            is_active: true,
        }
    }
    
    fn update_activity(&mut self) {
        self.last_activity = Instant::now();
        if !self.is_active {
            self.is_active = true;
        }
    }
    
    fn is_timed_out(&self) -> bool {
        self.last_activity.elapsed() > self.keep_alive_timeout
    }
    
    fn can_accept_request(&mut self) -> bool {
        if !self.is_active {
            return false;
        }
        if self.current_requests >= self.max_requests {
            return false;
        }
        true
    }
    
    fn increment_requests(&mut self) -> bool {
        if self.request_count >= self.max_requests {
            return false;
        }
        self.request_count += 1;
        self.current_requests += 1;
        self.update_activity();
        true
    }
    
    fn decrement_requests(&mut self) {
        if self.current_requests > 0 {
            self.current_requests -= 1;
        }
        self.update_activity();
    }
}

/// Parse HTTP request into method, path, and headers
/// Returns 1 on success, 0 on failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_parse_http_request(
    buf: *const u8,
    len: usize,
    out_method: *mut u8,
    out_method_len: usize,
    out_path: *mut u8,
    out_path_len: usize,
    out_headers: *mut u8,
    out_headers_len: usize,
) -> u64 {
    if buf.is_null() || out_method.is_null() || out_path.is_null() || out_headers.is_null() {
        return 0;
    }

    let buf_slice = unsafe { std::slice::from_raw_parts(buf, len) };

    // Find first line (method + path)
    let request_line_end = buf_slice.iter().position(|&b| b == b'\n');
    if request_line_end.is_none() {
        return 0;
    }
    let request_line_end = request_line_end.unwrap();
    let request_line = &buf_slice[..request_line_end];
    // Remove trailing \r if present
    let request_line = if request_line.ends_with(&[b'\r']) {
        &request_line[..request_line.len() - 1]
    } else {
        request_line
    };

    // Parse method (first word)
    let method_end = request_line.iter().position(|&b| b == b' ');
    if method_end.is_none() {
        return 0;
    }
    let method_end = method_end.unwrap();
    let method = &request_line[..method_end];

    // Parse path (second word)
    let path_start = method_end + 1;
    let path_end = request_line[path_start..].iter().position(|&b| b == b' ');
    if path_end.is_none() {
        return 0;
    }
    let path_end = path_start + path_end.unwrap();
    let path = &request_line[path_start..path_end];

    // Check output buffer sizes
    if method.len() >= out_method_len - 1 {
        return 0;
    }
    if path.len() >= out_path_len - 1 {
        return 0;
    }

    // Copy method
    unsafe {
        ptr::copy_nonoverlapping(method.as_ptr(), out_method, method.len());
        out_method.add(method.len()).write(0);
    }

    // Copy path
    unsafe {
        ptr::copy_nonoverlapping(path.as_ptr(), out_path, path.len());
        out_path.add(path.len()).write(0);
    }

    // Find headers start (after CRLF)
    let headers_start = request_line_end + 1;
    if headers_start < len && buf_slice[headers_start] == b'\r' {
        // Empty headers (CRLF immediately after request line)
        unsafe {
            out_headers.write(0);
        };
        return 1;
    }

    // Find end of headers (double CRLF)
    let mut line_end = headers_start;
    let mut total_headers_len = 0;
    while line_end + 1 < len {
        if buf_slice[line_end] == b'\r' && buf_slice[line_end + 1] == b'\n' {
            if line_end + 2 < len && buf_slice[line_end + 2] == b'\r' && buf_slice[line_end + 3] == b'\n' {
                // Found double CRLF - end of headers
                break;
            }
            line_end += 2;
        } else {
            line_end += 1;
        }
    }

    if line_end >= len {
        return 0;
    }

    let headers_len = line_end - headers_start;
    if headers_len >= out_headers_len - 1 {
        return 0;
    }

    unsafe {
        ptr::copy_nonoverlapping(buf_slice.as_ptr().add(headers_start), out_headers, headers_len);
        out_headers.add(headers_len).write(0);
    }

    // Found valid request line + headers
    (line_end + 2) as u64
}

// Check if a connection supports Keep-Alive
/// Check if a connection is configured for Keep-Alive
/// Returns 1 if Keep-Alive is supported, 0 otherwise
#[unsafe(no_mangle)]
pub extern "C" fn naso_check_keep_alive_support(stream_fd: u64) -> u64 {
    let mut manager = connection_manager().lock().unwrap();
    let state_opt = manager.connections.get(&stream_fd);
    match state_opt {
        Some(state) => {
            if state.is_active && state.request_count < state.max_requests {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

// Get connection statistics
/// Get connection statistics for monitoring
/// Returns 1 on success, 0 on failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_get_connection_stats(
    stream_fd: u64,
    out_requests: *mut u64,
    out_idle_ms: *mut u64,
) -> u64 {
    let mut manager = connection_manager().lock().unwrap();
    let state_opt = manager.connections.get(&stream_fd);
    
    if out_requests.is_null() || out_idle_ms.is_null() {
        return 0;
    }
    
    match state_opt {
        Some(state) => {
            let now = Instant::now();
            let idle_ms = state.last_activity.elapsed().as_millis() as u64;
            
            unsafe {
                *out_requests = state.request_count;
                *out_idle_ms = idle_ms;
            }
            1
        }
        None => 0,
    }
}

// Add pipelined request support
/// Add a pipelined request to the connection buffer
/// Returns 1 on success, 0 on failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_add_pipelined_request(
    stream_fd: u64,
    request_ptr: *const u8,
    request_len: usize,
) -> u64 {
    let mut manager = connection_manager().lock().unwrap();
    
    if request_ptr.is_null() || request_len == 0 {
        return 0;
    }
    
    let request_data = unsafe { std::slice::from_raw_parts(request_ptr, request_len).to_vec() };
    manager.add_pipelined_request(stream_fd, request_data);
    1
}

// Process pipelined requests
/// Process pipelined requests from buffer
/// Returns 1 if request was processed, 0 if buffer empty or failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_process_pipelined_request(
    stream_fd: u64,
    out_request_ptr: *mut u8,
    out_request_len: usize,
) -> u64 {
    let mut manager = connection_manager().lock().unwrap();
    
    if out_request_ptr.is_null() {
        return 0;
    }
    
    let request_opt = manager.get_pipelined_request(stream_fd);
    match request_opt {
        Some(request_data) => {
            let copy_len = request_data.len().min(out_request_len);
            unsafe {
                std::ptr::copy_nonoverlapping(request_data.as_ptr(), out_request_ptr, copy_len);
            }
            copy_len as u64
        }
        None => 0,
    }
}

// Enhanced connection cleanup
/// Cleanup timed out connections and return count of cleaned up connections
/// Returns number of cleaned up connections
#[unsafe(no_mangle)]
pub extern "C" fn naso_cleanup_connections() -> u64 {
    let mut manager = connection_manager().lock().unwrap();
    let initial_count = manager.connections.len() as u64;
    manager.cleanup_timed_out_connections();
    let final_count = manager.connections.len() as u64;
    initial_count - final_count
}

// Enhanced HTTP response with proper Keep-Alive support
/// Build HTTP response with Keep-Alive support
/// Returns bytes written on success, 0 on failure
#[unsafe(no_mangle)]
pub extern "C" fn naso_build_response_enhanced(
    method_ptr: *const u8,
    method_len: u64,
    path_ptr: *const u8,
    path_len: u64,
    headers_ptr: *const u8,
    headers_len: u64,
    body_ptr: *const u8,
    body_len: u64,
    out_ptr: *mut u8,
    out_len: u64,
    stream_fd: u64,
    enable_keep_alive: u8,
) -> u64 {
    if out_ptr.is_null() {
        return 0;
    }
    
    // Parse connection header for Keep-Alive configuration
    let mut has_keep_alive = enable_keep_alive != 0;
    let mut timeout_ms = 30000; // default 30 seconds
    let mut max_requests = 100; // default 100 requests
    
    if headers_ptr.is_not_null() && headers_len > 0 {
        let headers_data = unsafe { std::slice::from_raw_parts(headers_ptr, headers_len) };
        let (parsed_keep_alive, parsed_timeout, parsed_max) = parse_connection_header(headers_data);
        has_keep_alive = has_keep_alive || parsed_keep_alive;
        timeout_ms = parsed_timeout;
        max_requests = parsed_max;
    }
    
    // Update connection state if Keep-Alive is enabled
    if has_keep_alive && enable_keep_alive != 0 {
        let mut manager = connection_manager().lock().unwrap();
        let conn_state = manager.get_or_create_connection(stream_fd);
        conn_state.keep_alive_timeout = Duration::from_millis(timeout_ms);
        conn_state.max_requests = max_requests;
    }
    
    // Simple routing based on path (same as original)
    let mut status = "200 OK";
    let mut content: *const u8 = "Hello World".as_ptr();
    let mut content_len = 11usize;
    
    if method_ptr.is_null() || path_ptr.is_null() {
        status = "400 Bad Request";
        content = "Invalid Request".as_ptr();
        content_len = 17;
    } else {
        let method_str = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(method_ptr, method_len as usize)) };
        let path_str = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(path_ptr, path_len as usize)) };
        
        if path_str.starts_with("/echo") && body_len > 0 {
            // Echo request body
            content = body_ptr;
            content_len = body_len as usize;
        } else if path_str == "/health" {
            // Health check endpoint
            content = "OK".as_ptr();
            content_len = 2;
        } else if path_str == "/info" {
            // For /info, we'd need a temp buffer - skip for now, use default
        }
    }
    
    // Build full response into output buffer
    let status_line = format!("HTTP/1.1 {}\r\n", status);
    
    let keep_alive_header = if has_keep_alive {
        format!("Connection: keep-alive\r\nKeep-Alive: timeout={}, max={}\r\n", 
                timeout_ms / 1000, max_requests)
    } else {
        String::from("Connection: close\r\n")
    };
    
    let headers = format!(
        "Content-Type: text/plain\r\nContent-Length: {}\r\n{}",
        content_len, keep_alive_header
    );
    
    let total_len = status_line.len() + headers.len() + content_len;
    
    if total_len > out_len as usize {
        return 0;
    }
    
    let mut offset = 0;
    unsafe {
        // Copy status line
        ptr::copy_nonoverlapping(status_line.as_ptr(), out_ptr.add(offset), status_line.len());
        offset += status_line.len();
        
        // Copy headers
        ptr::copy_nonoverlapping(headers.as_ptr(), out_ptr.add(offset), headers.len());
        offset += headers.len();
        
        // Copy content
        if content_len > 0 {
            ptr::copy_nonoverlapping(content, out_ptr.add(offset), content_len);
            offset += content_len;
        }
    }
    
    total_len as u64
}