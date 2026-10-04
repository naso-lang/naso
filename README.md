# Naso Programming Language

<p align="center">
  <strong>A quantum-ready, formally verified systems programming language with Quantitative Type Theory (QTT)</strong>
</p>

<p align="center">
  <a href="https://github.com/naso-lang/naso/actions/workflows/verify.yml"><img src="https://github.com/naso-lang/naso/actions/workflows/verify.yml/badge.svg" alt="CI Status"></a>
  <a href="https://github.com/naso-lang/naso/releases/tag/v1.0.0-alpha.1"><img src="https://img.shields.io/github/v/release/naso-lang/naso?include_prereleases" alt="Release"></a>
  <a href="https://github.com/naso-lang/naso/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="License: MIT OR Apache-2.0"></a>
</p>

---

> **Working on this codebase?** Read [`STATE.md`](STATE.md) first. It records the build
> environment, what is done and verified, the known gaps, and the reasoning behind the
> load-bearing design decisions. It is the handoff document — see it before changing
> anything.

## Overview

**Naso** is a next-generation systems programming language designed for **quantum computing**, **high-assurance systems**, and **formally verified software**. It unifies **Quantitative Type Theory (QTT)** with automatic uncomputation, mutable value semantics, and polyhedral cross-hardware compilation into a single, unshakeable toolchain.

### Core Philosophy

| Principle | Description |
|-----------|-------------|
| **Quantity is Law** | Memory leaks, race conditions, and quantum decoherence are **compile-time type failures**. A variable marked `[1]` **must** be consumed exactly once. A `[0]` variable exists solely for proof and **must** be erased before runtime. |
| **Uncomputation over GC** | Discarding state is a physical violation. Temporary values and intermediate DAG nodes are **mathematically inverted and cleaned up** at scope exit — no runtime garbage collection. |
| **Mutation Without Aliasing** | Pointers and reference sharing are forbidden by default. **Mutable Value Semantics (`inout`)** provides local, high-performance mutation without borrow-checker friction or lifetime annotations. |
| **Hardware Topology Unification** | Code is **target-agnostic**. The compiler — not the programmer — maps abstract polyhedral loops onto CPU threads, CUDA grids, or QIR pulse sequences. |

---

## Key Capabilities

### 🔬 Static Type System (QTT)
- **Quantitative annotations** `[0]`, `[1]`, `[*]`, `[N]` on every binding
- **Dependent types** via natural-number parameters (`Tensor[4, 4]`, `Array[i32, N]`)
- **Linear resource tracking** — single-consumption guarantees at compile time
- **Erasure proofs** — `[0]`-quantity values vanish before runtime

### 🌌 Quantum-First Design
- **Qubit allocation** (`qalloc(1)`, `qalloc(N)`) with linear ownership
- **Gate primitives** (`hadamard`, `cnot`, `measure`, `qft`, `grover_oracle`)
- **Automatic uncomputation** — temporary qubits returned to `|0⟩` state
- **QIR / OpenQASM 3.0** codegen via LLVM backend

### 🧮 Formal Verification (`naso-verify`)
- **SMT-based** (Z3) verification engine
- **Quantum uncomputation safety** — proves `|0⟩` state restoration
- **[1]-Quantity leak detection** — proves no linear resource drops
- **Mutable Value Semantics** — frame conditions & disjointness for `inout`
- **Polyhedral loop invariants** — schedule transformations & bounds
- **Output formats**: Human, JSON, SARIF 2.1.0 (GitHub Code Scanning)

### 🧠 Language Server Protocol (`naso-lsp`)
- **Real-time diagnostics** (type errors, quantity violations)
- **Hover, goto-definition, document symbols**
- **Code actions** (auto-fix common patterns)
- **Semantic highlighting** for quantities & quantum ops

### ⚡ Polyhedral Codegen
- **Target-agnostic** polyhedral IR (`naso-ir`)
- **LLVM** (CPU, NVPTX, AArch64, WASM)
- **QIR** (quantum intermediate representation)
- **Cranelift** JIT for fast iteration
- **Automatic tiling, fusion, interchange, vectorization**

### 📦 Standard Library (`naso_std`)
| Module | Features |
|--------|----------|
| `core` | Primitives, `Option`, `Result`, `Vec`, `String`, `Array` |
| `std::quantum` | `qalloc`, `qfree`, `hadamard`, `cnot`, `measure`, `bell_pair`, `qft` |
| `std::tensor` | `Tensor`, `matmul`, `add`, `sub`, `scale`, `transpose`, `contract`, `outer_product` |
| `std::sys` | `sys_qalloc`, `sys_qfree`, `sys_barrier`, `sys_measure_trap`, `volatile_load/store` |
| `std::alloc` | `LinearAllocator`, `QttArena`, `linear_alloc`, `linear_free` |

---

## Language Example

```naso
// Bell pair creation with linear qubit ownership
fn bell_pair() -> (Qubit, Qubit) {
    let [1] q0 = qalloc(1);     // Linear: must be consumed exactly once
    let [1] q1 = qalloc(1);
    hadamard(q0);               // In-place gate application
    cnot(q0, q1);               // Entangle q0 (control) -> q1 (target)
    return (q0, q1);            // Linear move: ownership transferred
}

// Conditional consumption — verified on ALL paths
fn teleport(msg: [1] Qubit, alice: [1] Qubit, bob: [1] Qubit) {
    cnot(msg, alice);
    hadamard(msg);
    let m1 = measure(msg);      // msg consumed here
    let m2 = measure(alice);    // alice consumed here
    if m1 { X(bob); }           // Classical control
    if m2 { Z(bob); }
    // bob returned implicitly (linear move)
}

// Polyhedral matrix multiplication — auto-tiled & vectorized
fn matmul(a: [1] Tensor[64, 64], b: [1] Tensor[64, 64]) -> [1] Tensor[64, 64] {
    let c = alloc_tensor([64, 64]);
    forall i in 0..64, j in 0..64 {
        let mut sum = 0.0;
        forall k in 0..64 {
            sum = sum + a[i, k] * b[k, j];
        }
        c[i, j] = sum;
    }
    return c;
}
```

---

## Crate Architecture

| Crate | Path | Purpose |
|-------|------|---------|
| **naso-compiler** | `compiler/` | Frontend (lexer, parser, typechecker), PIR lowering, multi-backend codegen (LLVM, QIR, Cranelift), CLI |
| **naso-verify** | `crates/naso-verify/` | SMT-based formal verification (Z3), quantum uncomputation, linearity, MVS, polyhedral provers |
| **naso-lsp** | `crates/naso-lsp/` | Language Server Protocol implementation (diagnostics, hover, goto-def, code actions) |
| **naso_std** | `stdlib/` | Core types, quantum primitives, tensor ops, system calls, allocators |

### Dependency Graph
```text
naso-compiler (bin: naso)
    ├── naso_std (stdlib/prelude.naso)
    ├── naso-verify (lib)
    └── naso-lsp (bin: naso-lsp)
```

---

## Quickstart

### Prerequisites
- **Rust 1.70+** (stable)
- **LLVM 17** (for `llvm` feature)
- **Z3** (bundled via `z3-sys` vendored feature)

### Installation
```bash
# From source
git clone https://github.com/naso-lang/naso
cd naso
cargo install --path compiler --features llvm   # Full LLVM + QIR backend
# OR
cargo install --path compiler                    # Minimal (no codegen)

# Verify installation
naso --help
```

### CLI Commands
```bash
# Parse and print AST as JSON
naso parse program.naso

# Print token stream
naso tokens program.naso

# Type check (no codegen)
naso check program.naso

# Build with LLVM (default)
naso build program.naso -o program.ll

# Build with QIR for quantum targets
naso build --target qir program.naso -o program.qir

# Build with Cranelift JIT (fast iteration)
naso build --target cranelift program.naso

# Run verification benchmarks
cargo run --release -p naso-verify --example run_benchmarks
```

---

## Development & Verification

### Build Workspace
```bash
# Full workspace build (all crates)
cargo build --workspace --features llvm

# Fast development build
cargo build --workspace

# Run all tests
cargo test --workspace

# Check formatting
cargo fmt --all -- --check

# Strict linting (CI-equivalent)
cargo clippy --workspace --all-targets -- -D warnings
```

### Verification Workflow
```bash
# Quick type check
cargo run -p naso-compiler -- check program.naso

# Full verification (all provers)
cargo run -p naso-verify -- program.naso

# Specific prover
cargo run -p naso-verify -- --mode=uncomputation program.naso
cargo run -p naso-verify -- --mode=linearity program.naso

# JSON output for CI
cargo run -p naso-verify -- --format=json program.naso > results.json

# SARIF for GitHub Code Scanning
cargo run -p naso-verify -- --format=sarif program.naso > results.sarif
```

### Language Server
```bash
# Build LSP server
cargo build --release -p naso-lsp

# Run (connect via VS Code, Neovim, etc.)
naso-lsp
```

### Running Benchmarks
```bash
# All categories (quantum, tensor, linear)
cargo run --release -p naso-verify --example run_benchmarks

# Specific category
cargo run --release -p naso-verify --example run_benchmarks quantum

# JSON output
cargo run --release -p naso-verify --example run_benchmarks -- json > bench.json
```

---

## Project Structure
```
naso/
├── .github/workflows/verify.yml    # CI: build, clippy, fmt, verify, LSP tests
├── Cargo.toml                      # Workspace manifest
├── compiler/                       # Frontend + codegen
│   ├── src/
│   │   ├── ast/                    # QTT-annotated AST
│   │   ├── lexer/                  # Logos-based lexer
│   │   ├── parser/                 # Recursive-descent parser
│   │   ├── typecheck/              # QTT type checker + prelude insertion
│   │   ├── ir/                     # Polyhedral IR (affine maps, schedules)
│   │   ├── codegen/                # LLVM / QIR / Cranelift backends
│   │   └── main.rs                 # CLI entry point
│   └── benches/                    # Microbenchmarks (criterion)
├── crates/
│   ├── naso-verify/                # Z3-based formal verification
│   │   ├── src/prover/             # Uncomputation, linearity, MVS, polyhedral
│   │   └── examples/run_benchmarks.rs
│   └── naso-lsp/                   # LSP server (tower-lsp)
├── stdlib/                         # Standard library (naso_std)
│   ├── src/std/                    # quantum, tensor, sys, alloc
│   └── prelude.naso                # Auto-imported prelude
├── docs/                           # Documentation (Hugo + Hextra)
├── VERIFICATION.md                 # Formal verification deep-dive
└── TYPE_RULES.md                   # QTT typing rules reference
```

---

## Contributing

### Development Setup
```bash
# Install Rust toolchain (stable)
rustup default stable

# Install LLVM 17 (macOS)
brew install llvm@17

# Install Z3 (optional, vendored by default)
brew install z3

# Run full CI locally
cargo check --workspace --features llvm
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
```

### Code Style
- **Rust 2024 edition** with `clippy::pedantic` + `clippy::nursery`
- **Quantitative types** in all public APIs
- **No `unsafe`** without verification comment
- **Documentation** for all public items

### Commit Convention
```
fix(parser): resolve nested forall parsing
feat(typecheck): add [N]-quantity support
docs(readme): add CLI quickstart section
```

---

## License

Dual-licensed under **MIT OR Apache-2.0**.

---

## Resources

- **Documentation**: https://nasolang.org/
- **Type Rules**: [`TYPE_RULES.md`](TYPE_RULES.md)
- **Verification Guide**: [`VERIFICATION.md`](VERIFICATION.md)
- **Issue Tracker**: https://github.com/naso-lang/naso/issues
- **License**: [`LICENSE.md`](LICENSE.md)

---

<p align="center">
  Built with ❤️ by the Naso team — <em>Quantity is Law</em>
</p>
