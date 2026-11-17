#!/bin/bash

BENCH_RUNS=10

# Kill any existing server
pkill -9 -f bench_runner 2>/dev/null
sleep 2

# Start server in background
/tmp/cargo-target/release/bench_runner 2>&1 &
SERVER_PID=$!

# Wait for server to start
sleep 2

# Benchmark Test
START_TIME=$(date +%s%N)

for i in $(seq 1 $BENCH_RUNS); do
    curl -s http://127.0.0.1:8099/plaintext > /dev/null 2>&1
    END_TIME=$(date +%s%N)
    TOTAL_TIME=$((END_TIME - START_TIME))
    AVG_MS=$((TOTAL_TIME / (i * 1000)))
    echo "Request $i: $((TOTAL_TIME / 1000000))ms avg"
    START_TIME=$END_TIME
done

# Calculate total average
TOTAL_RUNTIME=$((END_TIME - START_TIME / 1000))
AVG_PER_REQUEST=$((TOTAL_RUNTIME / BENCH_RUNS))

# Kill server
kill $SERVER_PID 2>/dev/null

# Results
echo "\n=== BENCHMARK RESULTS ==="
echo "Total requests: $BENCH_RUNS"
echo "Total runtime: ${TOTAL_RUNTIME}ms"
echo "Average per request: ${AVG_PER_REQUEST}ms"
echo "Requests per second: $(echo "scale=2; $BENCH_RUNS * 1000 / $TOTAL_RUNTIME" | bc)"