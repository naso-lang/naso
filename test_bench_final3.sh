#!/bin/bash

# Kill any existing server
pkill -9 -f bench_runner 2>/dev/null
sleep 2

# Start the server in the background
timeout 3 /tmp/cargo-target/release/bench_runner > /tmp/server_output.txt 2>&1 &
SERVER_PID=$!

# Wait for server to start
sleep 2

# Test with netcat (simpler test)
echo "Testing with netcat..."
nc -z 127.0.0.1 8099 2>&1
echo "Netcat test result: $?"

# Check server output
echo "=== Server output ==="
cat /tmp/server_output.txt

# Kill the server
kill $SERVER_PID 2>/dev/null