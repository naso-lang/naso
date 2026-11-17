#!/bin/bash
# Link Naso LLVM IR with naso-ffi using rustc

set -e

NASO_OBJ="/tmp/bench_simple.o"
FFI_LIB="/tmp/cargo-target/release/libnaso_ffi.a"
OUTPUT="/tmp/bench_simple"

# Use rustc to link the object file with the naso-ffi static library
# We need to provide the Rust standard library and other dependencies
rustc "$NASO_OBJ" \
  "$FFI_LIB" \
  -o "$OUTPUT" \
  -L /home/node/.rustup/toolchains/1.98.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib \
  --extern std=/home/node/.rustup/toolchains/1.98.0-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/libstd-*.rlib \
  -C link-args=-lpthread \
  -C link-args=-ldl \
  -C link-args=-lm \
  -C link-args=-lpthread \
  -C link-args=-lrt \
  -C link-args=-lgcc_s \
  -C link-args=-lc \
  -C link-args=-lutil \
  -C link-args=-lunwind \
  2>&1

echo "Link result: $?"