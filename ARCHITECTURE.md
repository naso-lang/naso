# Cross-Compilation Setup

This directory contains the configuration and scripts for cross-compiling Naso to different targets.

## Targets Supported

1. `aarch64-unknown-linux-gnu` - ARM64 Linux with GNU toolchains
2. `x86_64-unknown-linux-musl` - x86_64 Linux with musl libc
3. `wasm32-unknown-unknown` - WebAssembly for browser/Edge Workers

## Building for Different Targets

### 1. Aarch64 Cross-Compilation

To build Naso for aarch64, you need the cross-compilation toolchain:

```bash
# Using rustup
rustup target add aarch64-unknown-linux-gnu

# Or using apt (if available)
sudo apt-get install gcc-aarch64-linux-gnu

# Build for aarch64
./cross_build.sh aarch64-unknown-linux-gnu

# Or using cargo directly
RUSTUP_TOOLCHAIN=aarch64-unknown-linux-gnu cargo build --target aarch64-unknown-linux-gnu --release
```

### 2. PGO (Profile-Guided Optimization)

PGO can significantly improve performance by using profile data to guide optimization decisions.

#### Generate Profile Data

```bash
# Generate profile data with PGO instrumentation
cargo build --profile pgo-generate --features llvm

# Run benchmarks to generate profile data
wrk -t2 -c50 -d30 http://localhost:8099/

# The profile data will be saved to /tmp/pgo-data
```

#### Use Profile Data for Optimized Build

```bash
# Build with PGO using generated profile data
cargo build --profile pgo-use --features llvm
```

#### Automated PGO Build Script

```bash
# Run the PGO build script
cd naso
./pgo_build.sh generate  # or ./pgo_build.sh use
```

## Lock-Free Thread Pool Implementation

The lock-free thread pool implementation replaces the traditional `crossbeam` channel-based approach with custom lock-free data structures:

### Key Components

1. **Lock-Free Deque**: A double-ended queue using atomic operations for work stealing
2. **Worker Threads**: Multiple threads that steal work from other threads' local queues
3. **Task System**: Thread-safe task execution with state tracking

### Performance Characteristics

- **Zero-Lock Stealing**: Threads can steal work without acquiring locks
- **Cache-Friendly**: Deterministic sharding (64 shards using `fd % 64`) reduces cache contention
- **Work-Stealing Algorithm**: Local threads process work locally, stealing from others when idle

## Current Status

### Completed
- [x] Stack overflow fix (8MB stack thread)
- [x] SIMD HTTP parsing with httparse integration
- [x] Lock-free work-stealing deque implementation
- [x] Basic cross-compilation support
- [x] PGO build profiles setup

### Remaining
- [ ] Complete PGO integration with actual instrumentation
- [ ] Finalize cross-compilation with native aarch64 toolchain
- [ ] Add PGO benchmark scripts and workflows

## Benchmarking

Current performance (wrk -t2 -c50 -d5s):
- **Lock-Free Thread Pool**: ~1,400 req/s
- **Current Target**: 30, acronyms req/s

## Usage Examples

### Building with PGO

```bash
# Generate profile data
cd naso
./pgo_build.sh generate

# Run your benchmark suite
wrk -t2 -c50 -d30 http://localhost:8099/

# Build optimized binary
cd naso
./pgo_build.sh use
```

### Cross-Compiling to Aarch64

```bash
# Ensure cross-compilation toolchain is available
rustup target add aarch64-unknown-linux-gnu

# Build for aarch64
cd naso
./cross_build.sh aarch64-unknown-linux-gnu
```

### Running Native Benchmark

```bash
# Start benchmark server
cd bench_runner
cargo run --release

# Run benchmark test
wrk -t2 -c50 -d5 http://127.0.0.1:8099/
```

## Performance Optimization Techniques

1. **SIMD Parsing**: Custom SSE/AVX instructions for HTTP parsing
2. **Deterministic Sharding**: Fixed-size sharding based on file descriptors
3. **Lock-Free Data Structures**: Atomic operations for work-stealing
4. **PGO**: Profile data-driven optimization decisions
5. **Stack Overflow Protection**: 8MB stack threads for large workloads

## Future Improvements

1. Implement more advanced SIMD optimizations
2. Add PGO integration with CMake/CMakeLists.txt
3. Support additional architectures (riscv, mips)
4. Implement lock-free priority queues
5. Add performance regression testing

## License

MIT OR Apache-2.0
