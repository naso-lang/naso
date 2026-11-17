#!/bin/bash

echo "Starting naso benchmark..."

# Start server
timeout 5 /tmp/cargo-target/release/bench_runner > /tmp/naso_server.log 2>&1 &
SERVER_PID=$!

# Wait for server to start
sleep 2

# Test single request
echo "Testing single request..."
curl -v http://127.0.0.1:8099/plaintext 2>&1

# Check server output
echo "\n=== Server Output ==="
cat /tmp/naso_server.log

# Kill server
kill $SERVER_PID 2>/dev/null