#!/usr/bin/env bash
# Cross-compilation script for Naso - Phase 17.8
# Usage: ./cross_build.sh [target] [profile]

set -e

target="${1:-aarch64-unknown-linux-gnu}"
profile="${2:-release}"

echo "Cross-compiling for $target in $profile mode..."

# Check for cross-compilation toolchain
if ! command -v aarch64-linux-gnu-gcc &> /dev/null; then
    echo "Error: aarch64-linux-gnu-gcc not found in PATH"
    echo "Install with: sudo apt-get install gcc-aarch64-linux-gnu"
    echo "Or use Docker: cross build --target aarch64-unknown-linux-gnu"
    exit 1
fi

# Configure cargo for cross-compilation
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

# Build with cross-compilation
cargo build --target "$target" --profile "$profile" -p naso-ffi 2>&1 | tail -20

if [ $? -eq 0 ]; then
    echo "Cross-compilation completed for $target"
    echo "Binary: target/$target/$profile/libnaso_ffi.so"
else
    echo "Cross-compilation failed"
    exit 1
fi