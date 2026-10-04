# naso — engineering state

Written for the next agent (or the next session of this one) after a context
compaction. **This file is the handoff.** Read it before touching anything.

Last updated: 2026-10-04, at commit `04b77db` (pushed, CI green).

---

## Build environment (set these every session)

```sh
export PATH=/home/linuxbrew/.linuxbrew/opt/llvm@17/bin:$PATH
export LLVM_SYS_170_PREFIX=/home/linuxbrew/.linuxbrew/opt/llvm@17
export CARGO_INCREMENTAL=0
```

- Cargo target dir is `/var/tmp/cargo-target`. **Never restore `target/` into the repo.**
- Use `-j 1` for large test runs. A parallel run was SIGKILLed before.
- Disk is tight (~2GB on `/home/node`, 10GB on `/var/tmp`). Before big runs:
  `find /var/tmp/cargo-target/debug/deps -maxdepth 1 -type f -executable ! -name '*.so' -delete`
  (rebuildable test binaries only; keeps `.rlib`/`.rmeta` so linking stays fast).
  This recovered ~4GB.
- **Never** work around memory pressure by changing `RUSTFLAGS`, `CARGO_PROFILE_*`,
  or any other build-affecting variable. Each distinct value creates a *second* Cargo
  fingerprint tree. This happened: it duplicated the build tree and ended in
  `signal 7 [Bus error]` at 69MB free. `signal 9` = linker OOM (~2.4GB RAM free).
  `signal 7` = disk/mmap exhaustion.
- Never read test output from `tail -N`; extern-C aborts truncate it. Use `tee` to a
  log and grep that.

---

## Mission

Miniature quantized models. The load-bearing capability is **sub-byte quantization
with machine-checked bounds**. Packing alone is commodity — every toolchain does it.
The differentiator is *verified* compact quantization.

---

## Where things stand

### Done and CI-verified

| Area | State |
|---|---|
| Native quantum execution | `.naso` → LLVM IR → `llc` → `cc` → run, linked `libnaso_gates.a` |
| `rz` rotation | Angle flows source → `qir.r1(double, ptr)` → runtime. Inverse negates it |
| Reversible uncomputation | Gate sequences emit forward pass + uncomputation |
| **Sub-byte `i4`** | **New.** Real packing: `Tensor[i4,16]` = 8 bytes, verified by execution |

Commits, newest first:

- `04b77db` sub-byte i4: two values per byte, measured not asserted
- `2ae9111` remove duplicate `qir.r1` declaration; forbid duplicate intrinsics
- `d757117` implement `rz`, carrying its angle from source to hardware
- `4ee9559` separate rotation refusal from rotation unreachability
- `eb56055` wire reversible uncomputation for gate sequences
- `ea5582a` delete 682 dead lines, repair six fabricated inverses

### Current matrix (all green at `04b77db`)

- LLVM workspace: **929 passed, 0 failed**
- Default workspace: **521 passed, 0 failed**
- Cranelift honesty: **12 passed, 0 failed**
- `cargo fmt --all --check`: clean
- `cargo clippy -D warnings`: clean for default, LLVM, and Cranelift
- CI run `37194704690`: **1368 passed, 0 failed**, 95 result lines, all 3 jobs success

---

## The `i4` work, in detail

LLVM has no `i4` type. `Tensor[i4, N]` therefore occupies `N/2` bytes:
two values per byte, low nibble first.

- `TensorBinding::sub_byte` (in `value_builder.rs`) records that two values share a
  byte. Without it an `i4` tensor and an `i8` tensor are indistinguishable.
- `tensor_gep` builds `getelementptr i8` at `i >> 1` for sub-byte tensors.
- `sub_byte_load` shifts by `(i & 1) * 4`, masks, then sign-extends from 4 bits
  arithmetically (shift sign bit into bit 7, multiply by 8). No branch, no `select`.
- `sub_byte_store` is read-modify-write: `merged = (byte & clear) | placed`.
- `ElemType::I4` lives in `compiler/src/ir/pir_types.rs`.

**The honesty rule that governs this feature:** a compiler that accepts `i4` and gives
it a whole byte is worse than useless — it makes every downstream size claim a lie.
So the tests assert the **byte count**, by running a real executable and checking which
bytes of a pre-filled buffer the kernel actually touched. A plain store would leave the
right byte count and the wrong data.

### Two bugs this work uncovered

1. **`lshr i8 %b, i64 0`** — `sub_byte_load` shifted by an index-width value. Invalid
   IR; the function failed LLVM verification. Only a **literal** index (`x[0]`, i32)
   triggered it, so all the `forall`-based tests passed. `capability_matrix_test` caught
   it. The shift amount is now narrowed to `i8`. This is why that cross-check test
   matters — do not remove it as redundant.
2. **My own test decoder was wrong**, not the compiler: 4-bit two's complement is
   `n - 16` for `n >= 8`, not `n - 8`. `0xF` is `-1`. The test was fixed.

Also: a phantom "parser bug" where `0..16` parsed as `0..8` was a **truncated fixture**
(`write_file` reported success without writing). No compiler defect existed. Verify
file contents; do not trust a write that claims success.

### Mutation results for `i4`

3 valid mutants (store nibble offset, clear-mask source, load sign step) all compiled,
**changed the emitted IR**, and were killed — 9/13 and 7/13 failures. **Zero survivors.**

Several other candidates were **invalid setup**, and that distinction matters:

- Did not compile (LLVM rejected the shift width, or Rust borrow error).
- LLVM constant-folded them into **byte-identical IR** → *equivalent*, not surviving.

**Always verify a mutant changed the emitted IR before calling it a survivor.** A
mutation that folds away is not a test of anything.

Beware: during one mutation run a file restore failed silently and corrupted the
baseline, producing fake results. Re-verify with a checksum before trusting any
survivor claim.

---

## Slop audit (2026-10-04)

Asked directly whether the tree contained placeholder work. It did:

- **`crates/naso-verify/src/qir_circuit.rs.bak`** — a tracked 384-line near-duplicate of
  `qir_circuit.rs`, differing only in one comment. Deleted. A stale copy is worse than no
  copy: it drifts and reads as authoritative.
- **Four stdlib functions that computed the answer and threw it away.**
  `maximum`, `minimum`, `relu` and `sigmoid` in `stdlib/src/std/tensor/ops.rs` each built
  the correct `Vec` and then called `unimplemented!()`. The caller's panic was the only
  observable behaviour, and the computation was dead code.

  They survived because **the stdlib had no `tests/` directory at all**. A suite that never
  runs cannot notice a function that panics.

  Fixed by adding `Tensor::from_quantity_vec`, a constructor generic over `Q` that picks
  storage from the quantity: `Zero` for Q0 (refusing a non-empty vector, since a `[0]`-use
  value must not reach runtime), `Linear` for Q1, `Heap`/`Arc` for QStar. Defaulting
  everything to `Linear` would let a `QStar` tensor claim exclusive ownership QTT forbids.

  `stdlib/tests/tensor_elementwise_ops_test.rs` — 20 tests asserting computed VALUES.
  6 mutants, all killed, no survivors: maximum→min (3/20), relu→identity (3/20),
  sigmoid `exp(-x)`→`exp(+x)` (3/20), QStar given Linear storage (2/20), shape assert
  removed (2/20), Q0 element guard removed (1/20).

**The general lesson:** absence of tests is what let this survive. When adding a feature,
check whether the crate it lands in has a test harness at all.

## Known gaps (honest — do not paper over)

Documented in `docs/content/LIMITATIONS.md`.

- **`u4` is not distinct.** Lexed and parsed, but reuses the signed nibble path, so it
  currently behaves as `i4`. Treat as unimplemented until a test distinguishes them.
- **No `i2`.** The addressing math generalizes; nothing is built or measured.
- **Tensor element bounds ARE discharged now** (`b6e11b2`). A tensor parameter is an
  uninterpreted function `Int -> scalar`, so an obligation holds for every tensor. The
  consequence is deliberate: `t[i] <= 127` is **REFUTED**, since an unconstrained tensor
  has no range. That is honest, and it means an input-range bound must come from real
  evidence, not from the element type.
- **Malformed SMT scripts are now a loud `ParseError`** (`verify()` refuses instead of
  answering). Detection is STRUCTURAL: paren balance + assertion count vs. what Z3 loaded.
  A script that parses but is semantically wrong (undeclared symbol, mistyped sort) is
  still dropped by Z3 without a diagnostic. Narrows the hole; does not close it.
- Counting alone was NOT enough: an unbalanced paren swallows the assertions, so the scanner
  and Z3 agree on zero and the count check passes. The balance check is what catches that.
- **`&&`/`||` are REFUSED**, not encoded as `and`/`or`. They short-circuit, so they are a
  different proposition when an operand is undefined. Split the assertion instead.
- **No float reasoning in the prover.** Scale/zero-point error bounds need an interval
  or rational abstraction. Do not pretend the integer prover handles floats.
- **No `naso verify` CLI subcommand.** The verifier is a real library with passing
  tests, but the CLI does not expose it, despite `VERIFICATION.md` implying a workflow.
  Do not paper over this mismatch.
- **Odd extents** (`Tensor[i4, 15]`) leave a partial trailing byte; unspecified and not
  rejected. Do not rely on it.
- `rx` / `ry` refused — no `qir.rx`/`qir.ry` in the base profile, so admitting them
  would declare entry points the runtime does not export.
- `entangle` refused — CNOT is not an approximation for an unspecified operation.
- QIR declares more intrinsics than the runtime exports, and QIR output is not linked.
  **Do not claim native QIR execution.**

---

## Recommended next step

**Proof-carrying quantization.** `ExprKind::Index` is DONE (`b6e11b2`). What remains:

1. ~~Give the prover a way to state an input range~~ -- DONE via `requires { .. }`.
   The prover now checks `pre_1 ∧ .. ∧ pre_n ⇒ goal`. A range premise makes a clamp bound
   provable without weakening the encoding. REMAINING: **call sites do not check
   `requires`**. Writing one is a promise, not a guarantee, so a function with an
   unsatisfiable precondition is a function nobody can call correctly. That is the next
   thing to build, and it is the difference between a precondition and a verified one.
   Do NOT add a "fits in i8" axiom instead — that proves a claim about the tensors the
   axiom admits and reports it for the one that breaks.
2. Design a sound float abstraction (interval or rational) for scale/zero-point error
   bounds. Do not extend the integer prover and hope.
3. Wire a real `naso verify` subcommand with defined semantics, exit codes, honest
   reporting of unsupported obligations, and end-to-end tests.
4. Make a malformed SMT script a loud failure rather than a silent `sat`. This is the
   highest-value remaining fix: today it degrades "proved" into "refuted" quietly.

Lower priority: `u4` distinct storage, `i2`, `release` builtin, runtime-valued `rz`.

---

## Working rules (learned the hard way)

- **IR text is not evidence that a computation happened.** LLVM constant-folds. Compile
  with `llc`, link a C driver with `clang`, RUN it, check stdout and exit status.
- **Green CI is not evidence — read the runner log and confirm the test COUNT.**
- Never report a prover/verifier as working without a test demonstrating it. Flag false
  negatives explicitly.
- Never assert a fixed outcome from stochastic behaviour; assert the **distribution**
  over many runs. A fixed RNG seed passes a link-and-run test while being wrong half
  the time.
- A mutation that fails to compile, or compiles without changing behaviour, is **invalid
  setup, not a survivor.** Prove both.
- No hardcoded returns, fake outputs, or stubs presented as working.
- Never preserve API keys, tokens, passwords, or connection strings. Use `[REDACTED]`.
- `docs/content/LIMITATIONS.md` and `.github/workflows/verify.yml` must be updated for
  **every** behavior change.
