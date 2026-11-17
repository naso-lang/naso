#!/bin/bash

# Kill any existing server
pkill -9 -f bench_runner 2>/dev/null
sleep 1

# Start the server
/tmp/cargo-target/release/bench_runner 2>&1 &

# Wait for server to start
sleep 2

# Test with curl
echo "Testing with curl..."
curl -v http://127.0.0.1:8099/plaintext 2>&1

# Kill the server
pkill -9 -f bench_runner 2>/dev/null