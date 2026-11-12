#!/bin/bash
# Simple HTTP benchmark script

URL="http://127.0.0.1:8091"
DURATION=10
CONNECTIONS=100
REQUESTS=10000

echo "Benchmarking $URL for ${DURATION}s with ${CONNECTIONS} connections..."

# Use hey if available, otherwise use a simple loop
if command -v hey &> /dev/null; then
    hey -z ${DURATION}s -c ${CONNECTIONS} -n ${REQUESTS} $URL
elif command -v wrk &> /dev/null; then
    wrk -t4 -c ${CONNECTIONS} -d ${DURATION}s $URL
else
    echo "Installing hey..."
    go install github.com/rakyll/hey@latest 2>/dev/null || cargo install hey 2>/dev/null || echo "Could not install benchmark tool"
fi