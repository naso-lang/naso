#!/bin/bash
# PGO Build Script for Naso
# Usage: ./pgo_build.sh [generate|use]

set -e

PGO_DIR="/tmp/pgo-data"
mkdir -p "$PGO_DIR"

case "$1" in
    generate)
        echo "Building with PGO generation..."
        export RUSTFLAGS="-Cprofile-generate=$PGO_DIR"
        cargo build --release --features llvm
        ;;
    use)
        echo "Building with PGO use..."
        export RUSTFLAGS="-Cprofile-use=$PGO_DIR"
        cargo build --release --features llvm
        ;;
    *)
        echo "Usage: $0 [generate|use]"
        echo "  generate - Build with profile generation"
        echo "  use      - Build with profile use"
        exit 1
        ;;
esac