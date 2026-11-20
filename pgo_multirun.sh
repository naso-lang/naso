#!/usr/bin/env bash
# Multi-run PGO profile generation script
# Generates better profile data by running multiple benchmark iterations

set -e

ITERATIONS="${1:-5}"
DURATION="${2:-10}"
BINARY="/tmp/cargo-target/release/bench_runner"

echo "=== Multi-run PGO Profile Generation ==="
echo "Iterations: $ITERATIONS"
echo "Duration per iteration: ${DURATION}s"
echo "Binary: $BINARY"
echo ""

# Clean previous profile data
rm -rf /tmp/pgo-data
mkdir -p /tmp/pgo-data

# Build with profile generation
echo "Building with profile generation..."
RUSTFLAGS="-Cprofile-generate=/tmp/pgo-data" cargo build -p naso-ffi --release --features mimalloc
RUSTFLAGS="-Cprofile-generate=/tmp/pgo-data" cargo build -p bench_runner --release

echo ""
echo "Running $ITERATIONS benchmark iterations..."

for i in $(seq 1 $ITERATIONS); do
    echo ""
    echo "=== Iteration $i/$ITERATIONS ==="
    
    # Start server
    $BINARY > /tmp/bench_pgo_$i.log 2>&1 &
    SERVER_PID=$!
    
    # Wait for server to start
    sleep 3
    
    # Check if server is still running
    if ! kill -0 $SERVER_PID 2>/dev/null; then
        echo "Server failed to start! Check /tmp/bench_pgo_$i.log"
        cat /tmp/bench_pgo_$i.log
        exit 1
    fi
    
    # Run benchmark
    echo "Running wrk benchmark for ${DURATION}s..."
    wrk -t2 -c50 -d${DURATION}s http://127.0.0.1:8099/ 2>&1 | tee /tmp/wrk_pgo_$i.log
    
    # Stop server
    kill $SERVER_PID 2>/dev/null
    wait $SERVER_PID 2>/dev/null
    
    echo "Iteration $i complete"
done

echo ""
echo "=== Merging profiles ==="
rust-profdata merge -o /tmp/pgo-data/merged.profdata /tmp/pgo-data/*.profraw

echo ""
echo "=== Building with merged profile ==="
RUSTFLAGS="-Cprofile-use=/tmp/pgo-data/merged.profdata" cargo build -p naso-ffi --release --features mimalloc
RUSTFLAGS="-Cprofile-use=/tmp/pgo-data/merged.profdata" cargo build -p bench_runner --release

echo ""
echo "=== PGO build complete ==="
echo "Run benchmark with: RUSTFLAGS=\"-Cprofile-use=/tmp/pgo-data/merged.profdata\" ./bench_runner &"