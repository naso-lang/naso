// ============================================================================
// HTTP Parser FFI with SIMD optimizations
// ============================================================================
// This module provides HTTP request parsing and response building functions
// with C-compatible FFI interface for Naso.
// Uses httparse for fast parsing and SIMD for CRLF detection.
// ============================================================================

use std::ptr;
use std::time::{Instant, Duration};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use httparse;

// SIMD-optimized CRLF detection for x86_64
#[cfg(target_arch = "x86_64")]
mod simd_crlf {
    use std::arch::x86_64::*;
    
    /// Find double CRLF (\r\n\r\n) using SIMD (internal unsafe implementation)
    /// Returns the index of the first byte of the double CRLF, or None if not found
    #[target_feature(enable = "sse2,ssse3")]
    unsafe fn find_double_crlf_simd(buf: &[u8]) -> Option<usize> {
        if buf.len() < 4 {
            return None;
        }
        
        // We're looking for \r\n\r\n = 0x0D 0x0A 0x0D 0x0A
        // Use SIMD to search for \r\n (0x0D 0x0A) pairs
        let crlf = _mm_set1_epi8(0x0D); // \r
        let lf = _mm_set1_epi8(0x0A);   // \n
        
        let mut i = 0;
        while i + 16 <= buf.len() {
            let chunk = _mm_loadu_si128(buf[i..].as_ptr() as *const __m128i);
            
            // Check for \r
            let cr_mask = _mm_cmpeq_epi8(chunk, crlf);
            // Check for \n
            let lf_mask = _mm_cmpeq_epi8(chunk, lf);
            
            // Shift lf_mask by 1 to align with potential \r before \n
            let lf_shifted = _mm_alignr_epi8(lf_mask, _mm_setzero_si128(), 15);
            
            // Look for \r followed by \n
            let crlf_mask = _mm_and_si128(cr_mask, lf_shifted);
            
            let mask = _mm_movemask_epi8(crlf_mask);
            if mask != 0 {
                // Found at least one \r\n, check for double
                let first_crlf = mask.trailing_zeros() as usize;
                let abs_pos = i + first_crlf;
                
                // Check if there's another \r\n right after
                if abs_pos + 4 <= buf.len() {
                    if buf[abs_pos] == b'\r' && buf[abs_pos + 1] == b'\n' &&
                       buf[abs_pos + 2] == b'\r' && buf[abs_pos + 3] == b'\n' {
                        return Some(abs_pos);
                    }
                }
                
                // Check subsequent positions in this chunk
                let mut check_pos = abs_pos + 1;
                while check_pos < i + 16 && check_pos + 3 < buf.len() {
                    if buf[check_pos] == b'\r' && buf[check_pos + 1] == b'\n' &&
                       buf[check_pos + 2] == b'\r' && buf[check_pos + 3] == b'\n' {
                        return Some(check_pos);
                    }
                    check_pos += 1;
                }
            }
            
            i += 16;
        }
        
        // Fallback for remaining bytes
        while i + 3 < buf.len() {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
            i += 1;
        }
        
        None
    }
    
    /// Safe wrapper for find_double_crlf_simd
    pub fn find_double_crlf(buf: &[u8]) -> Option<usize> {
        if is_x86_feature_detected!("sse2") && is_x86_feature_detected!("ssse3") {
            unsafe { find_double_crlf_simd(buf) }
        } else {
            fallback_find_double_crlf(buf)
        }
    }
    
    fn fallback_find_double_crlf(buf: &[u8]) -> Option<usize> {
        for i in 0..buf.len().saturating_sub(3) {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
        }
        None
    }
}

/// Fallback non-SIMD implementation
#[cfg(not(target_arch = "x86_64"))]
mod simd_crlf {
    pub fn find_double_crlf(buf: &[u8]) -> Option<usize> {
        for i in 0..buf.len().saturating_sub(3) {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
        }
        None
    }
}

/// AVX2-optimized CRLF detection for x86_64 (32-byte chunks)
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
mod simd_crlf_avx2 {
    use std::arch::x86_64::*;
    
    /// Find double CRLF (\r\n\r\n) using AVX2 (internal unsafe implementation)
    /// Returns the index of the first byte of the double CRLF, or None if not found
    #[target_feature(enable = "avx2")]
    unsafe fn find_double_crlf_avx2(buf: &[u8]) -> Option<usize> {
        if buf.len() < 4 {
            return None;
        }
        
        // We're looking for \r\n\r\n = 0x0D 0x0A 0x0D 0x0A
        // Use AVX2 to search for \r\n (0x0D 0x0A) pairs in 32-byte chunks
        let crlf = _mm256_set1_epi8(0x0D); // \r
        let lf = _mm256_set1_epi8(0x0A);   // \n
        
        let mut i = 0;
        while i + 32 <= buf.len() {
            let chunk = _mm256_loadu_si256(buf[i..].as_ptr() as *const __m256i);
            
            // Check for \r
            let cr_mask = _mm256_cmpeq_epi8(chunk, crlf);
            // Check for \n
            let lf_mask = _mm256_cmpeq_epi8(chunk, lf);
            
            // Shift lf_mask by 1 to align with potential \r before \n
            let lf_shifted = _mm256_alignr_epi8(lf_mask, _mm256_setzero_si256(), 31);
            
            // Look for \r followed by \n
            let crlf_mask = _mm256_and_si256(cr_mask, lf_shifted);
            
            let mask = _mm256_movemask_epi8(crlf_mask);
            if mask != 0 {
                // Found at least one \r\n, check for double
                let first_crlf = mask.trailing_zeros() as usize;
                let abs_pos = i + first_crlf;
                
                // Check if there's another \r\n right after
                if abs_pos + 4 <= buf.len() {
                    if buf[abs_pos] == b'\r' && buf[abs_pos + 1] == b'\n' &&
                       buf[abs_pos + 2] == b'\r' && buf[abs_pos + 3] == b'\n' {
                        return Some(abs_pos);
                    }
                }
                
                // Check subsequent positions in this chunk
                let mut check_pos = abs_pos + 1;
                while check_pos < i + 32 && check_pos + 3 < buf.len() {
                    if buf[check_pos] == b'\r' && buf[check_pos + 1] == b'\n' &&
                       buf[check_pos + 2] == b'\r' && buf[check_pos + 3] == b'\n' {
                        return Some(check_pos);
                    }
                    check_pos += 1;
                }
            }
            
            i += 32;
        }
        
        // Fallback for remaining bytes - use SSE2 version
        if i + 16 <= buf.len() {
            // Could call SSE2 version here, but for simplicity use fallback
        }
        
        // Fallback for remaining bytes
        while i + 3 < buf.len() {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
            i += 1;
        }
        
        None
    }
    
    /// Safe wrapper for find_double_crlf_avx2
    pub fn find_double_crlf(buf: &[u8]) -> Option<usize> {
        if is_x86_feature_detected!("avx2") {
            unsafe { find_double_crlf_avx2(buf) }
        } else if is_x86_feature_detected!("sse2") && is_x86_feature_detected!("ssse3") {
            // Fall through to SSE2 version in parent module
            crate::simd_crlf::find_double_crlf(buf)
        } else {
            fallback_find_double_crlf(buf)
        }
    }
    
    fn fallback_find_double_crlf(buf: &[u8]) -> Option<usize> {
        for i in 0..buf.len().saturating_sub(3) {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
        }
        None
    }
}

/// AVX-512 optimized CRLF detection for x86_64 (64-byte chunks)
#[cfg(all(target_arch = "x86_64", target_feature = "avx512f"))]
mod simd_crlf_avx512 {
    use std::arch::x86_64::*;
    
    /// Find double CRLF (\r\n\r\n) using AVX-512 (internal unsafe implementation)
    /// Returns the index of the first byte of the double CRLF, or None if not found
    #[target_feature(enable = "avx512f")]
    unsafe fn find_double_crlf_avx512(buf: &[u8]) -> Option<usize> {
        if buf.len() < 4 {
            return None;
        }
        
        // We're looking for \r\n\r\n = 0x0D 0x0A 0x0D 0x0A
        // Use AVX-512 to search for \r\n (0x0D 0x0A) pairs in 64-byte chunks
        let crlf = _mm512_set1_epi8(0x0D); // \r
        let lf = _mm512_set1_epi8(0x0A);   // \n
        
        let mut i = 0;
        while i + 64 <= buf.len() {
            let chunk = _mm512_loadu_si512(buf[i..].as_ptr() as *const __m512i);
            
            // Check for \r
            let cr_mask = _mm512_cmpeq_epi8_mask(chunk, crlf);
            // Check for \n
            let lf_mask = _mm512_cmpeq_epi8_mask(chunk, lf);
            
            // Shift lf_mask by 1 to align with potential \r before \n
            let lf_shifted = (lf_mask >> 1) | ((lf_mask & 1) << 63);
            
            // Look for \r followed by \n
            let crlf_mask = cr_mask & lf_shifted;
            
            if crlf_mask != 0 {
                // Found at least one \r\n, check for double
                let first_crlf = crlf_mask.trailing_zeros() as usize;
                let abs_pos = i + first_crlf;
                
                // Check if there's another \r\n right after
                if abs_pos + 4 <= buf.len() {
                    if buf[abs_pos] == b'\r' && buf[abs_pos + 1] == b'\n' &&
                       buf[abs_pos + 2] == b'\r' && buf[abs_pos + 3] == b'\n' {
                        return Some(abs_pos);
                    }
                }
                
                // Check subsequent positions in this chunk
                let mut check_pos = abs_pos + 1;
                while check_pos < i + 64 && check_pos + 3 < buf.len() {
                    if buf[check_pos] == b'\r' && buf[check_pos + 1] == b'\n' &&
                       buf[check_pos + 2] == b'\r' && buf[check_pos + 3] == b'\n' {
                        return Some(check_pos);
                    }
                    check_pos += 1;
                }
            }
            
            i += 64;
        }
        
        // Fallback for remaining bytes
        while i + 3 < buf.len() {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
            i += 1;
        }
        
        None
    }
    
    /// Safe wrapper for find_double_crlf_avx512
    pub fn find_double_crlf(buf: &[u8]) -> Option<usize> {
        if is_x86_feature_detected!("avx512f") {
            unsafe { find_double_crlf_avx512(buf) }
        } else if is_x86_feature_detected!("avx2") {
            crate::simd_crlf_avx2::find_double_crlf(buf)
        } else if is_x86_feature_detected!("sse2") && is_x86_feature_detected!("ssse3") {
            crate::simd_crlf::find_double_crlf(buf)
        } else {
            fallback_find_double_crlf(buf)
        }
    }
    
    fn fallback_find_double_crlf(buf: &[u8]) -> Option<usize> {
        for i in 0..buf.len().saturating_sub(3) {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' &&
               buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
                return Some(i);
            }
        }
        None
    }
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
            if !state.pipelined_requests.is_empty() {
                Some(state.pipelined_requests.remove(0))
            } else {
                None
            }
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

/// Parse HTTP request into method, path, and headers using httparse
/// Returns bytes consumed on success, 0 on failure
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

    // Use SIMD to find end of headers (double CRLF)
    let headers_end = match simd_crlf::find_double_crlf(buf_slice) {
        Some(pos) => pos + 4, // Position after double CRLF
        None => return 0, // Incomplete request
    };

    if headers_end > len {
        return 0;
    }

    // Parse with httparse
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    let parse_result = req.parse(&buf_slice[..headers_end]);
    
    if parse_result.is_err() {
        return 0;
    }
    
    let method = req.method.unwrap_or("");
    let path = req.path.unwrap_or("");

    // Check output buffer sizes
    if method.len() >= out_method_len - 1 || path.len() >= out_path_len - 1 {
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

    // Copy headers (raw from buffer)
    let headers_len = headers_end - (buf_slice.iter().position(|&b| b == b'\n').unwrap_or(0) + 1);
    let headers_start = buf_slice.iter().position(|&b| b == b'\n').unwrap_or(0) + 1;
    let headers_end_pos = headers_start + headers_len;
    
    if headers_len >= out_headers_len - 1 {
        return 0;
    }

    unsafe {
        ptr::copy_nonoverlapping(buf_slice[headers_start..headers_end_pos].as_ptr(), out_headers, headers_len);
        out_headers.add(headers_len).write(0);
    }

    headers_end as u64
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
    
    if !headers_ptr.is_null() && headers_len > 0 {
        let headers_data = unsafe { std::slice::from_raw_parts(headers_ptr, headers_len as usize) };
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