# Naso "Better Than Rust in Every Way" — Phased Roadmap

**Mission**: Make Naso superior to Rust across all dimensions: performance, safety, quantum, developer experience, and language capabilities.

**Current Baseline**: 28,947/25,183 req/s (0.8-1.1× Rust actix-web) — close on HTTP, but Rust wins on ecosystem, compile times, tooling, adoption.

---

## Phase Priority Framework

| Priority | Criteria |
|----------|----------|
| **P0** | Blockers for "better than Rust" claim — must work reliably |
| **P1** | Core differentiators where Naso can *win* (quantum, verification, QTT) |
| **P2** | Performance parity + wins (SIMD, lock-free, compile speed) |
| **P3** | Developer experience (LSP, diagnostics, package manager, docs) |
| **P4** | Ecosystem (stdlib, package registry, CI integrations) |

---

## Phase 14: Core Language Completeness (P0 — Foundation)

**Goal**: Naso language feature-complete for systems programming

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **14.1 True async/await lowering** | Compiler: state machine codegen | Zero-cost async without `.await` pollution; structured concurrency built-in |
| **14.2 Array returns from functions** | Compiler: LLVM sret for `[u8; N]` | No `Box<[u8]>` heap allocation; stack-allocated returns |
| **14.3 Function pointer callbacks** | Compiler: `&fn` → FFI trampolines | First-class async callbacks without `Arc<Mutex<dyn Fn>>` |
| **14.4 Large inline arrays (>256 elements)** | Parser: fix type unification | HTTP buffers on stack; no heap allocation for request/response |
| **14.5 Generic associated types (GATs)** | Type checker: QTT + GATs | More expressive than Rust's GATs with quantity tracking |
| **14.6 Const generics with Nat kinds** | Type checker: `Array[u8, N]` where `N: Nat` | Dependent types for buffer sizes — verified at compile time |

**Validation**: All `examples/*.naso` compile + type-check; `techempower_phase11.naso` runs without workarounds.

---

## Phase 15: Quantum-First Systems Programming (P1 — Core Differentiator)

**Goal**: Only systems language with native quantum + classical unification

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **15.1 QIR backend production** | Codegen: QIR → LLVM → quantum runtime | Write quantum kernels in same language as classical; no FFI boundary |
| **15.2 Qubit linear types** | QTT: `[1] Qubit` ownership | Qubit leaks = compile error; Rust has no quantum types |
| **15.3 Automatic uncomputation** | Compiler: inverse gate synthesis | Temporary qubits auto-returned to \|0⟩; no manual cleanup |
| **15.4 Hybrid classical/quantum kernels** | Compiler: polyhedral + QIR scheduling | Classical pre/post-processing fused with quantum circuits |
| **15.5 Quantum error correction types** | Stdlib: `QEC<Code, Distance>` types | Verified QEC at type level; Rust has no equivalent |
| **15.6 VQE/QAOA high-level abstractions** | Stdlib: variational algorithms | Quantum ML in 50 lines vs 5000 in Rust + Python |

**Validation**: `kernels/quant_*.naso` discharge fully; quantum kernels run on simulator + hardware.

---

## Phase 16: Formal Verification Built-In (P1 — Safety Win)

**Goal**: Verification is a language feature, not an external tool

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **16.1 `naso verify` as compiler phase** | CLI: `naso build --verify` | Verification runs with every build; no separate CI step |
| **16.2 Proof-carrying quantization** | Verifier: tie bounds to `i4` execution | Quantization error bounds *proven* for packed execution; Rust has no equivalent |
| **16.3 Linear resource proofs** | Verifier: QTT linearity + MVS | No leaks, no double-free, no use-after-move — *proven*, not just checked |
| **16.4 Polyhedral loop invariants** | Verifier: affine schedule correctness | Loop transformations (tiling, fusion) *proven* correct |
| **16.5 SMT script hardening** | Verifier: malformed script = loud failure | No silent `sat` degradation; honest verification |
| **16.6 Verification result caching** | Verifier: incremental proving | Re-verify only changed functions; sub-second feedback loop |

**Validation**: `cargo build --verify` on stdlib = 100% discharge; mutation testing kills all mutants.

---

## Phase 17: Performance Dominance (P2 — Beat Rust on Benchmarks)

**Goal**: Naso ≥ Rust on *all* standard benchmarks (TechEmpower, SPEC, custom)

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **17.1 SIMD HTTP parsing** | Stdlib: `httparse` integration | +40% CPU efficiency; zero-copy parsing |
| **17.2 Lock-free thread pool** | Runtime: crossbeam → custom lock-free | Lower latency tail; no mutex contention at scale |
| **17.3 Stack-allocated state machines** | Compiler: async lowering optimization | No heap allocation per `await`; faster task spawn |
| **17.4 True callback completion** | Runtime: io_uring CQE → direct callback | Eliminates `naso_yield()` polling overhead |
| **17.5 Compile-time SIMD autovectorization** | Codegen: polyhedral + LLVM vectorizer | Loop autovectorization without `#[simd]` annotations |
| **17.6 Profile-guided optimization (PGO)** | Build: LLVM PGO + Naso instrumentation | Hot paths optimized from real workloads |
| **17.7 Binary size optimization** | Codegen: LTO + dead code elimination | Smaller binaries than Rust; faster cold starts |
| **17.8 Cross-compilation without Docker** | Build: `naso build --target aarch64` | Native cross-compile; no `cross` crate needed |

**Validation**: 
- TechEmpower plaintext: **≥ 35,000 req/s** (beat actix-web 31K)
- SPEC CPU 2017: **≥ Rust** on all sub-benchmarks
- Binary size: **≤ 80% of Rust equivalent**

---

## Phase 18: Developer Experience Superiority (P3 — Daily Driver)

**Goal**: Writing Naso feels better than writing Rust

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **18.1 Incremental compilation** | Compiler: query-based incremental | Sub-second rebuilds; Rust 10-60s |
| **18.2 IDE diagnostics < 100ms** | LSP: on-type-check | Real-time feedback; Rust Analyzer 500ms-2s |
| **18.3 Error messages with fixes** | Type checker: structured diagnostics | `error[E0382]: borrow of moved value` → `help: clone here` + auto-fix |
| **18.4 Package manager (`naso pkg`)** | Tooling: built-in registry + resolution | No `Cargo.toml` editing; `naso add http` |
| **18.5 Built-in formatter (`naso fmt`)** | Tooling: `naso fmt --check` | One tool; no `rustfmt` + `cargo fmt` |
| **18.6 Built-in test runner (`naso test`)** | Tooling: `#[test]` + benchmarks | `naso test --bench` = criterion built-in |
| **18.7 Documentation generator (`naso doc`)** | Tooling: `naso doc --open` | Better than `cargo doc`; quantum-aware |
| **18.8 REPL (`naso repl`)** | Runtime: JIT via Cranelift | Interactive systems programming; Rust has no REPL |

**Validation**: New contributor writes HTTP server in < 30 min; all tools work offline.

---

## Phase 19: Standard Library Completeness (P3 — Batteries Included)

**Goal**: `naso_std` covers all systems programming needs without external crates

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **19.1 Async I/O traits** | Stdlib: `AsyncRead`/`AsyncWrite` + io_uring | No `tokio`/`async-std` split; one runtime |
| **19.2 HTTP client + server** | Stdlib: production HTTP/1.1 + HTTP/2 | No `reqwest`/`hyper`/`axum` decisions |
| **19.3 TLS (rustls equivalent)** | Stdlib: `naso_tls` with verified crypto | Verified TLS; no `rustls`/`openssl` choice |
| **19.4 Serialization (serde equivalent)** | Stdlib: `naso_serde` with QTT types | Quantities preserved through (de)serialization |
| **19.5 Database drivers** | Stdlib: `naso_sql` + `naso_redis` | Verified SQL; no `sqlx`/`diesel` |
| **19.6 Observability** | Stdlib: `naso_metrics` + `naso_tracing` | Built-in OpenTelemetry; no `tracing`/`metrics` |
| **19.7 CLI framework** | Stdlib: `naso_clap` with typed args | Clap power + QTT validation |
| **19.8 Embedded/hardware HAL** | Stdlib: `naso_hal` for MCU targets | `no_std` + verified register access |

**Validation**: `naso new web-app` → production service in 5 min; zero external dependencies.

---

## Phase 20: Ecosystem & Adoption (P4 — Network Effects)

**Goal**: Naso viable for production teams today

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **20.1 Package registry (`crates.naso-lang.org`)** | Infra: namespaced packages | No crate squatting; verified packages only |
| **20.2 CI/CD templates** | Tooling: GitHub Actions + GitLab CI | `naso ci init` → working pipeline |
| **20.3 VS Code extension (official)** | Tooling: LSP + debugger + quantum viz | Better than rust-analyzer; quantum circuit viewer |
| **20.4 IntelliJ/Rider plugin** | Tooling: JetBrains IDE support | Enterprise IDE support day one |
| **20.5 Language server protocol spec** | Spec: `naso-lsp` as reference impl | Other editors can implement easily |
| **20.6 Learning resources** | Docs: book, by-example, quantum guide | `naso-book` + `quantum-by-example` |
| **20.7 Migration tooling** | Tooling: `cargo2naso` for Rust projects | Automated Rust → Naso translation |
| **20.8 Corporate support program** | Business: SLA, training, consulting | Red Hat-style support for enterprises |

**Validation**: 3+ production teams using Naso; 100+ packages in registry.

---

## Phase 21: Advanced Compiler Research (P2/P3 — Long-Term Moat)

**Goal**: Compiler innovations Rust cannot easily copy

| Task | Component | Why It Beats Rust |
|------|-----------|-------------------|
| **21.1 Polyhedral auto-scheduling** | Codegen: Pluto/ISL integration | Auto-parallelize + tile loops for CPU/GPU/quantum |
| **21.2 Effect system** | Type checker: algebraic effects | `async`/`try`/`gen` as effects; no colored functions |
| **21.3 Gradual verification** | Verifier: `#[verify]` per-function | Verify critical paths; skip boilerplate |
| **21.4 Dependent type erasure** | Compiler: proof → runtime check | Full dependent types; proofs erased or checked |
| **21.5 Quantum-classical co-optimization** | Codegen: unified schedule | Classical prep + quantum circuit + postproc = one schedule |
| **21.6 Self-optimizing JIT** | Runtime: Cranelift + profile feedback | Hot paths recompiled at runtime; faster than AOT |

---

## Dependency Graph (Critical Path)

```
Phase 14 (Language Completeness)
    │
    ├──→ Phase 15 (Quantum) ◄──┐
    │                            │
    ├──→ Phase 16 (Verification) │
    │                            │
    └──→ Phase 17 (Performance)──┤
         │                       │
         ├──→ Phase 18 (DX) ◄────┤
         │                       │
         ├──→ Phase 19 (Stdlib)──┤
         │                       │
         └──→ Phase 20 (Ecosystem)◄──┘
                       │
                       └──→ Phase 21 (Research Moat)
```

**Critical Path**: 14 → 17 → 18/19 → 20 (everything depends on 14; 15/16 can parallelize after 14)

---

## Resource Allocation (Suggested)

| Phase | Effort | Timeline | Team |
|-------|--------|----------|------|
| 14 | 3 engineers × 3 months | Q1 2027 | Compiler team |
| 15 | 2 engineers × 4 months | Q1-Q2 2027 | Quantum + compiler |
| 16 | 2 engineers × 3 months | Q1-Q2 2027 | Verification team |
| 17 | 3 engineers × 4 months | Q2-Q3 2027 | Runtime + compiler |
| 18 | 2 engineers × 3 months | Q2-Q3 2027 | Tooling team |
| 19 | 3 engineers × 4 months | Q3-Q4 2027 | Stdlib team |
| 20 | 2 engineers × 3 months | Q4 2027 | Infra + docs |
| 21 | 2 researchers × ongoing | 2028+ | Research team |

---

## Success Metrics (Definition of "Better Than Rust")

| Dimension | Metric | Target |
|-----------|--------|--------|
| **Performance** | TechEmpower plaintext | ≥ 35,000 req/s (Rust: 31,724) |
| **Performance** | SPEC CPU 2017 geometric mean | ≥ Rust 1.98 |
| **Performance** | Binary size (release) | ≤ 80% Rust equivalent |
| **Performance** | Compile time (clean) | ≤ 50% Rust equivalent |
| **Performance** | Incremental compile (1 line) | < 1 second |
| **Safety** | Verification coverage (stdlib) | 100% discharged |
| **Safety** | Mutation testing survival rate | 0% |
| **Quantum** | Quantum kernel LOC vs Q#/Cirq | ≤ 20% |
| **Quantum** | Hybrid kernel execution | Single binary, no FFI |
| **DX** | Time to first HTTP server | < 5 minutes |
| **DX** | IDE diagnostic latency | < 100ms |
| **Ecosystem** | Package registry size (Year 1) | 500+ verified packages |
| **Ecosystem** | Production adopters (Year 2) | 10+ teams |

---

## Immediate Next Actions (This Week)

1. **Create GitHub Project** with all phases as milestones
2. **Break Phase 14 into issues** with `good first issue` labels
3. **Assign owners** for each phase (compiler, runtime, stdlib, tooling, verification, quantum)
4. **Set up CI tracking** for "better than Rust" metrics dashboard
5. **Write RFC process** for language changes (like Rust RFCs but faster)

---

## Summary

| Phase | Priority | Focus | Beats Rust By |
|-------|----------|-------|---------------|
| **14** | P0 | Language completeness | True async, array returns, GATs, dependent types |
| **15** | P1 | Quantum-first | Only language with native quantum + classical |
| **16** | P1 | Built-in verification | Proofs compile-time; quantization bounds proven |
| **17** | P2 | Performance dominance | SIMD, lock-free, stack state machines, PGO |
| **18** | P3 | Developer experience | Incremental compile, REPL, better errors, pkg mgr |
| **19** | P3 | Batteries included stdlib | No external crate decisions |
| **20** | P4 | Ecosystem | Verified packages, migration tooling, IDE support |
| **21** | P2/3 | Research moat | Polyhedral scheduling, effects, gradual verification |

**Total**: 8 phases, ~2 years to "better than Rust in every way" with 17 engineers.

**Start with Phase 14** — it unblocks everything else and delivers immediate language usability wins.