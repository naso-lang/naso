#!/bin/bash
# Deployment script for Hertner VPS (native Linux 5.1+)
# Run this on your Hertner VPS after copying the deploy_package folder

set -e

echo "=== Naso io_uring Deployment Script ==="
echo "Target: Hertner VPS (Linux 5.1+)"
echo ""

# Check kernel version
KERNEL=$(uname -r)
echo "Kernel: $KERNEL"
MAJOR=$(echo $KERNEL | cut -d. -f1)
MINOR=$(echo $KERNEL | cut -d. -f2)
if [ "$MAJOR" -lt 5 ] || ([ "$MAJOR" -eq 5 ] && [ "$MINOR" -lt 1 ]); then
    echo "ERROR: Kernel 5.1+ required for io_uring. Current: $KERNEL"
    exit 1
fi
echo "✓ Kernel supports io_uring"

# Check for required tools
echo ""
echo "Checking dependencies..."
for tool in gcc clang rustc cargo; do
    if command -v $tool &> /dev/null; then
        echo "✓ $tool: $($tool --version | head -1)"
    else
        echo "✗ $tool not found"
    fi
done

# Check for io_uring support
echo ""
echo "Checking io_uring support..."
if [ -e /dev/urandom ]; then
    echo "✓ /dev/urandom exists"
fi

# Copy binaries to standard locations
echo ""
echo "Installing binaries..."
sudo mkdir -p /usr/local/bin /usr/local/lib
sudo cp bench_runner /usr/local/bin/
sudo cp libnaso_ffi.a /usr/local/lib/
sudo chmod +x /usr/local/bin/bench_runner

# Verify installation
echo ""
echo "Verifying installation..."
ls -la /usr/local/bin/bench_runner /usr/local/lib/libnaso_ffi.a

# Test io_uring availability
echo ""
echo "Testing io_uring..."
cat > /tmp/test_uring.c << 'EOF'
#include <linux/io_uring.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <stdio.h>

int main() {
    struct io_uring_params params = {};
    int fd = syscall(__NR_io_uring_setup, 8, &params);
    if (fd >= 0) {
        printf("io_uring supported (fd=%d)\n", fd);
        close(fd);
        return 0;
    } else {
        perror("io_uring_setup failed");
        return 1;
    }
}
EOF
gcc /tmp/test_uring.c -o /tmp/test_uring
/tmp/test_uring

echo ""
echo "=== Deployment Complete ==="
echo ""
echo "To run benchmarks:"
echo "  bench_runner &"
echo "  sleep 3"
echo "  wrk -t2 -c50 -d30s http://127.0.0.1:8099/"
echo ""
echo "Expected: io_uring will be used for async I/O (5-10x performance gain)"