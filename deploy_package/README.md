# Naso io_uring Deployment Package for Hertner VPS

## Package Contents

```
deploy_package/
├── bench_runner          # Main binary with io_uring support
├── libnaso_ffi.a         # Static library with FFI
├── deploy_hertner.sh     # Deployment script for Hertner VPS
└── README.md             # This file
```

## Quick Deployment

### 1. Copy to your Hertner VPS
```bash
# From your local machine:
scp -r deploy_package/ user@your-hertner-vps:~/

# Or if using this Agent37 environment:
# Download the package and upload to your VPS
```

### 2. Run on Hertner VPS
```bash
ssh user@your-hertner-vps
cd ~/deploy_package
chmod +x deploy_hertner.sh
sudo ./deploy_hertner.sh
```

### 3. Run Benchmarks
```bash
# Start the server
bench_runner &

# Wait for startup
sleep 3

# Run benchmarks
wrk -t2 -c50 -d30s http://127.0.0.1:8099/
```

## Requirements

- **Linux kernel 5.1+** (required for io_uring)
- **gcc/clang** for testing
- **wrk** for benchmarking (`sudo apt install wrk`)

## Verify io_uring Support

The deploy script will test io_uring support. Expected output:
```
io_uring supported (fd=3)
```

If you see "io_uring_setup failed", your kernel is too old (< 5.1).

## Expected Performance

With io_uring on native Linux 5.1+:
- **Expected**: 15,000-30,000 req/s (5-10x over current ~3,000 req/s)
- **Mechanism**: Batched syscalls, zero-copy I/O, async accept/read/write

## Troubleshooting

### Kernel too old
```bash
uname -r
# If < 5.1, upgrade kernel or use different VPS
```

### Permission denied
```bash
# Run deploy script with sudo
sudo ./deploy_hertner.sh
```

### Port already in use
```bash
lsof -i :8099
pkill -f bench_runner
```

## Architecture

```
┌─────────────────────────────────────┐
│         Hertner VPS (Linux 5.1+)    │
├─────────────────────────────────────┤
│  bench_runner (io_uring backend)   │
│  ├── Lock-free thread pool          │
│  ├── AVX2/AVX-512 SIMD parsing      │
│  ├── mimalloc allocator             │
│  ├── PGO-optimized code             │
│  └── io_uring async I/O ← NEW       │
│       ├── Batched syscalls          │
│       ├── Zero-copy reads/writes    │
│       └── Async accept              │
└─────────────────────────────────────┘
```

## Expected Results

| Configuration | Req/sec | Improvement |
|---------------|---------|-------------|
| Current (gVisor) | ~3,000 | Baseline |
| **Hertner + io_uring** | **15,000-30,000** | **5-10x** |

## Support

If issues arise, check:
1. Kernel version: `uname -r` (must be 5.1+)
2. io_uring test: Run the test in deploy script manually
3. Logs: Check `/tmp/bench_*.log` for server output