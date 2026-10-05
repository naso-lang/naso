# naso — engineering state

Written for the next agent (or the next session of this one) after a context
compaction. **This file is the handoff.** Read it before touching anything.

Last updated: 2026-10-04, at commit `63ff0d9` (pushed, CI green: 1644 passed, 0 failed).

### If you are resuming, read these three numbers and then `git log --oneline -12`

1. `naso-verify` is the verifier, and it is a **binary, not a subcommand**. It cannot be
   `naso verify`: it depends on `naso-compiler`, so the compiler cannot depend on it back.
   Exit codes: `0` discharged, `1` refuted, **`2` UNDECIDED**, `3` usage, `4` input,
   `5` internal, `6` nothing-discharged. **Undecidable is not passing.** Never let that
   collapse back to 0; the whole point is that a green build means something.
2. `kernels/quant_error_bound.naso` and `kernels/quant_int8.naso` both **fully discharge**
   through the binary. `abs`/`min`/`max`/`clamp` have exact SMT definitions. `round` is
   REFUSED — see the next section, this is the live limitation.
3. Floats are **exact reals**, not IEEE-754. Every proof about a quantiser here is a
   statement about the *mathematics* of quantisation. Nothing bounds runtime rounding.

### The three ways this codebase has lied to you recently

Worth knowing before you trust anything, because each was mine and each shipped green:

- **A silent match arm.** `encode_quantity_expr` had `_ => "no special quantity handling
  needed"` over compound expressions. That skipped `forall` loops — the one place a loop
  body consumes a linear tensor — so the verifier reported *correct* programs as leaking.
  A catch-all in an AST walker is never "just a default".
- **A test that could not fail.** `prove_custom_vc` returned `Ok(Vec::new())`, which every
  caller reads as a clean bill of health; its test body was `// Smoke test`. And twenty
  obligation tests asserted `is_empty()`, which cannot tell a proof from a silence.
  **Prefer asserting the count and the exact codes over asserting emptiness.**
- **A mutation harness that hid failures.** It parsed the first `test result:` line, so a
  failure in any later test binary was invisible and a mutant looked alive. Count failures
  across **all** binaries. That rule is written into the `naso-mutation-harness` skill.

If you add an AST walker, match **exhaustively** with no `_` arm. New variants should be
compile errors, not silent skips.

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

- `63ff0d9` define `abs`/`min`/`max`/`clamp` exactly; discharge the int8 quantiser
- `bd01917` ship `naso-verify`: a real verifier with an honest exit status
- `49a3902` prove a quantisation error bound over exact reals
- `1b60643` prove callee preconditions at every call site
- `7afc260` typecheck preconditions; fix erased-reference semantics in linearity
- `04b77db` sub-byte i4: two values per byte, measured not asserted
- `2ae9111` remove duplicate `qir.r1` declaration; forbid duplicate intrinsics
- `d757117` implement `rz`, carrying its angle from source to hardware
- `4ee9559` separate rotation refusal from rotation unreachability
- `eb56055` wire reversible uncomputation for gate sequences
- `ea5582a` delete 682 dead lines, repair six fabricated inverses

### Current matrix (all green at `63ff0d9`)

- LLVM workspace: **1042 passed, 0 failed**
- Default workspace: **634 passed, 0 failed**
- Cranelift honesty: **5 passed, 0 failed**
- `cargo fmt --all --check`: clean
- `cargo clippy -q --all-targets`: **zero** warnings
- CI run `37247733544`: **1644 passed, 0 failed**, read from the runner log at `63ff0d9`

NOTE ON THE CRANELIFT COUNT: an earlier revision of this file recorded 12. That was simply
WRONG. `compiler/tests/cranelift_honesty_test.rs` has exactly 5 `#[test]` functions and
`git log 04b77db..HEAD -- compiler/tests/cranelift_honesty_test.rs` is empty, so the count has
been 5 the whole time. Nothing was removed. Recording a number nobody checked is the same
class of error as the rest of what this project is auditing.

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
   The prover checks `pre_1 ∧ .. AND pre_n ⇒ goal`. A range premise makes a clamp bound
   provable without weakening the encoding. Preconditions are TYPECHECKED (undefined name or
   non-bool premise is a compile error). **Call sites are now checked too**: for
   `g(a1..an)` the prover discharges `caller_requires => g.requires[a1/x1..an/xn]`, so a
   call no premise establishes is reported AT THE CALL SITE. Limits, all recorded in
   LIMITATIONS.md: it is an obligation over the caller's parameters (so an unsatisfiable
   caller premise makes it vacuous, like every other obligation here); only DIRECT calls are
   resolved, so a method call is not; and an unencodable substituted form fails loudly as an
   undeclared symbol -- a wrong refusal, never a wrong proof.
   Do NOT add a "fits in i8" axiom instead — that proves a claim about the tensors the
   axiom admits and reports it for the one that breaks.
2. ~~Design a sound float abstraction for scale/zero-point error bounds~~ -- DONE as EXACT
   REAL (rational) arithmetic. `f32`/`f64` encode as the dyadic rational they are, exactly;
   `kernels/quant_error_bound.naso` discharges the round-to-nearest half-step bound for a
   scalar and for a whole tensor, and `kernels/scale_f32.naso`'s `scale > 0` is now a real
   precondition. The boundary is deliberate and recorded in LIMITATIONS.md: this is exact
   real arithmetic, so it is sound for a bound about the MATHEMATICS of quantisation and
   says NOTHING about IEEE-754 rounding. REMAINING if that is ever wanted: an error term
   per float operation (interval or directed-rounding arithmetic).
   The original concern -- "encoding f32 as Real would change the claim" -- was half right:
   it is a different claim, and it is the USEFUL one, provided the boundary is stated rather
   than assumed. Refusing all float reasoning discharges no quantisation bound at all, and
   that is an absence of a property, not a soundness one.
3. ~~Wire a real `naso verify` subcommand~~ -- DONE, as the `naso-verify` binary, NOT a
   subcommand. `naso-verify` depends on `naso-compiler`, so the compiler cannot depend on it
   back; Cargo rejects the cycle. Exit codes: 0 discharged, 1 refuted, 2 UNDECIDED, 3 usage,
   4 input, 5 internal, 6 nothing-discharged-under-`--require-obligations`. Code 2 is the load
   bearing one -- "I could not look at it" must never be a green build. Modes:
   `all`, `uncomputation`, `linearity`, `obligations`. `custom` was DELETED rather than wired:
   `prove_custom_vc` returned an empty diagnostic list, which is indistinguishable from a pass.
4. `round` needs a universally quantified half-step axiom (`x - 0.5 <= round(x) <= x + 0.5`) for
   an exact nearest-integer encoding. This is the next blocker on the quantiser: `abs`,
   `min`, `max`, `clamp` are exact, but `round` is refused, so a stated range bounds the
   DIVISION rather than the rounded quotient. Emitting the axiom needs the encoder to collect
   axioms across a whole script rather than build one term, which is why it is separate work.
5. Prove something about the tensor kernels themselves -- currently the error bound is proved
   about an UNINTERPRETED tensor function, so nothing ties it to the packed `i4` layout that
   actually executes. That link is the missing piece between "proof-carrying quantisation" and
   "proof-carrying quantisation that means anything".
6. Make a malformed SMT script a loud failure rather than a silent `sat`. This is the
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
