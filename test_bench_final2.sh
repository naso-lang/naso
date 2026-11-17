#!/bin/bash

# Kill any existing server
pkill -9 -f bench_runner 2>/dev/null
sleep 2

# Start the server
/tmp/cargo-target/release/bench_runner 2>&1 &
SERVER_PID=$!

# Wait for server to start
sleep 3

# Test with curl
echo "Testing with curl..."
curl -v http://127.0.0.1:8099/plaintext 2>&1

# Check if server is still running
if ps -p $SERVER_PID > /dev/null; then
    echo "Server is still running (PID: $SERVER_PID)"
else
    echo "Server died (exit code: $?)"
fi

# Kill the server
kill $SERVER_PID 2>/dev/null