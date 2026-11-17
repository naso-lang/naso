#!/bin/bash
# Test script for bench_runner with output capture

# Kill any existing server
pkill -9 -f bench_runner 2>/dev/null
sleep 1

# Start server in background, capture output
/tmp/cargo-target/release/bench_runner > /tmp/server.log 2>&1 &
SERVER_PID=$!

# Wait for server to start
sleep 2

# Test with curl
echo "Testing with curl..."
curl -v http://127.0.0.1:8099/plaintext 2>&1

# Give server time to log
sleep 1

# Kill server
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null

# Show server logs
echo "=== SERVER LOGS ==="
cat /tmp/server.log

echo "Test complete"