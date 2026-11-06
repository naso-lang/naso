#!/bin/bash
# Phase 4: Thread Pool Optimization Benchmarking Script
# Tests various worker thread configurations to find optimal performance

set -e

NASO_BIN="/tmp/cargo-target/release/naso"
RESULTS_DIR="/home/node/naso/phase4_results"
mkdir -p "$RESULTS_DIR"

echo "=== Phase 4: Thread Pool Optimization Benchmarking ==="
echo "Results will be saved to: $RESULTS_DIR"
echo ""

# Build naso first
echo "Building naso..."
cd /home/node/naso
cargo build -p naso --release 2>&1 | tail -20

# Test configurations: 1, 2, 4, 8, 16, 32 threads
CONFIGS=(1 2 4 8 16 32)

for threads in "${CONFIGS[@]}"; do
    echo ""
    echo "=================================================="
    echo "Testing with $threads worker threads"
    echo "=================================================="
    
    # Build test with specific worker count
    cat > /tmp/thread_test_${threads}.nasoc << 'EOF'
// Thread pool benchmark - test async HTTP server with configurable workers
extern "C" {
    fn naso_write(fd: u64, buf: *u8, len: usize) -> usize;
    fn naso_read(fd: u64, buf: *u8, max_len: usize) -> usize;
    fn naso_tcp_bind_reuseport(addr: *u8, len: usize) -> u64;
    fn naso_tcp_accept_async(listener_fd: u64, out: *u64, out_len: usize, callback: *u8, callback_len: usize) -> u64;
    fn naso_read_async(fd: u64, buf: *u8, len: usize, callback: *u8, callback_len: usize) -> u64;
    fn naso_write_async(fd: u64, buf: *u8, len: usize, callback: *u8, callback_len: usize) -> u64;
    fn naso_runtime_new(workers: u64) -> *u8;
    fn naso_runtime_run(rt: *u8) -> u64;
    fn naso_spawn(rt: *u8, task: *u8, len: usize) -> *u8;
    fn naso_yield() -> u64;
    fn naso_close(fd: u64) -> u64;
}

const HTTP_RESPONSE: [u8] = [
    72, 84, 84, 80, 47, 49, 46, 49, 32, 50, 48, 48, 32, 79, 75, 13,  // "HTTP/1.1 200 OK\r"
    10, 67, 111, 110, 116, 101, 110, 116, 45, 76, 101, 110, 103, 116, 104, 58, 32,  // "\nContent-Length: "
    50, 13, 10, 13, 10, 72, 101, 108, 108, 111, 32, 87, 111, 114, 108, 100,  // "2\r\n\r\nHello World"
];

async fn handle_client(client_fd: u64) -> () {
    let mut buf = [0u8; 2048];
    let _bytes_read = naso_read(client_fd, buf as *u8, 2048);
    let _bytes_written = naso_write(client_fd, HTTP_RESPONSE as *u8, 78);
    naso_close(client_fd);
}

async fn accept_loop(listener_fd: u64, runtime: *u8) -> () {
    let mut out = [0u64, 0u64];
    let out_ptr = out as *u64;
    
    while true {
        // Simplified: use sync accept for benchmarking
        // Real async would use naso_tcp_accept_async with callback
        let mut out_sync = [0u64, 0u64];
        let mut result = naso_tcp_accept_async(listener_fd, out_ptr, 2, 0 as *u8, 0);
        if result != 0 {
            let client_fd = out[1];
            if client_fd != 0 {
                // Spawn handler on runtime
                let _ = naso_spawn(runtime, 0 as *u8, 0);
            }
        }
        naso_yield();
    }
}

fn main() -> () {
    let addr = [49, 50, 55, 46, 48, 46, 48, 46, 49, 58, 56, 48, 52, 56, 0];  // "127.0.0.1:8048"
    let listener_fd = naso_tcp_bind_reuseport(addr as *u8, 15);
    
    // Test with configurable worker count
EOF
    echo "    let runtime = naso_runtime_new(${threads});" >> /tmp/thread_test_${threads}.nasoc
    cat >> /tmp/thread_test_${threads}.nasoc << 'EOF'
    let _ = naso_spawn(runtime, 0 as *u8, 0);
    naso_runtime_run(runtime);
}
EOF

    # Compile
    echo "Compiling test for ${threads} threads..."
    ${NASO_BIN} build --target llvm /tmp/thread_test_${threads}.nasoc -o /tmp/thread_test_${threads}.ll 2>&1 | tail -5
    llc -opaque-pointers -filetype=obj /tmp/thread_test_${threads}.ll -o /tmp/thread_test_${threads}.o
    clang -o /tmp/thread_test_${threads} /tmp/thread_test_${threads}.o /tmp/entry.o /tmp/cargo-target/release/libnaso_ffi.a -lpthread -ldl 2>&1 | tail -5
    
    # Run benchmark
    echo "Running benchmark for ${threads} threads (10 seconds)..."
    /tmp/thread_test_${threads} &
    SERVER_PID=$!
    sleep 2
    
    # Use wrk for benchmarking if available, otherwise use simple curl loop
    if command -v wrk &> /dev/null; then
        wrk -t4 -c100 -d10s http://127.0.0.1:8048/ > "$RESULTS_DIR/thread_${threads}_wrk.txt" 2>&1
        tail -20 "$RESULTS_DIR/thread_${threads}_wrk.txt"
    else
        # Simple benchmark with curl
        echo "wrk not found, using curl loop..."
        for i in {1..100}; do
            curl -s -o /dev/null -w "%{time_total}\n" http://127.0.0.1:8048/ >> "$RESULTS_DIR/thread_${threads}_curl.txt" 2>/dev/null || true
        done
        # Calculate average
        if [ -f "$RESULTS_DIR/thread_${threads}_curl.txt" ]; then
            awk '{sum+=$1} END {if (NR>0) print "Avg latency:", sum/NR, "s"; print "Requests:", NR}' "$RESULTS_DIR/thread_${threads}_curl.txt"
        fi
    fi
    
    kill $SERVER_PID 2>/dev/null || true
    sleep 1
done

echo ""
echo "=== Thread Pool Benchmarking Complete ==="
echo "Results in: $RESULTS_DIR"