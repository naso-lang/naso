---
title: "Limitations"
weight: 3
description: "What Naso's backends actually accept today, measured rather than claimed."
---

# Limitations

This page is **generated from a test**, not written by hand.

Every row below is measured by running the construct through the real pipeline --
parse, lower, LLVM build -- and recording whether it was accepted. The table lives in
`compiler/tests/capability_matrix_test.rs`, and three tests keep this page honest:

- `the_capability_matrix_matches_reality` fails if a row's recorded outcome differs from
  what the compiler does today.
- `every_refused_construct_names_a_reason` fails if a refusal stops explaining itself.
- `the_published_limitations_document_matches_the_matrix` fails if this page drifts from
  the measured table.

So a backend changing behaviour without updating this page breaks CI.

## Why this page exists

Seven defects shipped in which a backend produced output that claimed more than it
delivered, and every one exited 0:

1. QIR emitted a `reversible` block with the adjoint **discarded**.
2. WGSL linearity checking did not recurse into blocks.
3. WGSL wrote `// Unsupported expr` into a shader and reported success.
4. `return` inside a branch was hoisted, so `guard(5)` returned `-1` instead of `7`.
5. The Cranelift backend returned a hardcoded `42` for **every** program.
6. WGSL emitted a callee after its caller; the resulting module did not parse.
7. A statement-position `reversible { ... }` dropped the block and generated no inverse.

Documentation cannot prevent that class of bug -- it drifts. What prevents it is a test
that compares belief against behaviour, which is what the capability matrix is.

## The rule

A construct is either **correct** or **refused**. There is no third state.

"Emits something close" is not an acceptable outcome for a backend that cannot lower the
construct, and neither is a placeholder value, a shader comment, or an exit status of 0.

## LLVM backend

| Construct | Status | Note |
|---|---|---|

| integer arithmetic | **emits** | <!-- construct:integer arithmetic --> |
| float arithmetic | **emits** | <!-- construct:float arithmetic --> |
| `if` as an expression | **emits** | <!-- construct:`if` as an expression --> |
| `if` without `else` | **emits** | <!-- construct:`if` without `else` --> |
| `while` | **emits** | <!-- construct:`while` --> |
| counted `for` | **emits** | lowers onto the `while` CFG <!-- construct:counted `for` --> |
| `break` | **emits** | <!-- construct:`break` --> |
| `continue` | **emits** | <!-- construct:`continue` --> |
| `return` inside `if` | **emits** | lowered in place, not hoisted <!-- construct:`return` inside `if` --> |
| function calls | **emits** | one LLVM symbol per Naso function <!-- construct:function calls --> |
| numeric casts | **emits** | width-preserving, signedness-tracked <!-- construct:numeric casts --> |
| scalar parameters | **emits** | <!-- construct:scalar parameters --> |
| tensor parameters | **emits** | caller-supplied pointer plus a length <!-- construct:tensor parameters --> |
| 2-D tensors | **emits** | row-major <!-- construct:2-D tensors --> |
| sub-byte i4 tensors | **emits** | PACKED: 8 values in 4 bytes, two per byte, low nibble first. Proven by execution in `sub_byte_i4_execution_test.rs`, which asserts the byte count, not just the IR shape <!-- construct:sub-byte i4 tensors --> |
| scalar i4 | **refused** | no neighbour to share a byte with, so a scalar i4 would have to be a byte and the sub-byte claim would be fiction. Refused rather than silently widened <!-- construct:scalar i4 --> |
| quantum parameters | **emits** | `Qubit` and `QRegister` both map to the pointer ABI <!-- construct:quantum parameters --> |
| symbolic loop bounds | **emits** | affine schedule bands <!-- construct:symbolic loop bounds --> |
| `reversible { ... }` | **refused** | gate sequences uncompute, rotations included; arithmetic, measurement, empty and nested blocks still refused <!-- construct:`reversible { ... }` --> |
| nested tensors | **refused** | one ptr per tensor has a single stride <!-- construct:nested tensors --> |
| tensor with a zero extent | **refused** | no elements to index <!-- construct:tensor with a zero extent --> |
| quantity used as a type | **refused** | `Many` is a quantity; write `[1] Qubit` <!-- construct:quantity used as a type --> |
| widthless `int` parameter | **refused** | no exact ABI slot without a width <!-- construct:widthless `int` parameter --> |
| `break` in an affine `forall` band | **refused** | the DOMAIN TRANSFORMATION is known and tested (`ir::early_exit`): a `break` at a constant prefix of the iteration shortens the band to `min(hi, k - 1)`, which stays affine, and `continue` leaves the domain alone and predicates the body. What is missing is emission of the shortened band, so the refusal stands. A guard depending on runtime data (`i >= n / 2`) has no affine answer and needs a `while` loop <!-- construct:`break` in an affine `forall` band --> |
| expression-position `reversible` | **refused** | <!-- construct:expression-position `reversible` --> |
| `entangle` | **refused** | NO LLVM INTRINSIC. The QIR base profile declares none, and approximating it by a CNOT over the first two qubits would leave any further qubit unentangled while looking correct in the emitted text. `entangle` over fewer than two qubits is refused by the typechecker instead: its result type is a `QRegister` of dimension = arity, so `entangle()` inferred a zero-dimensional register and typechecked while asserting nothing <!-- construct:`entangle` --> |
| adding two booleans | **refused** | FIXED. `unify_kinds` accepts `(Bool, Bool)` for `==`, and the arithmetic arm called it without asking whether the operator applied, so `true + true` reached LLVM as `add i1` and wrapped to `0`. Refused by `typecheck::inference::bool_operator`; ordering comparisons (`<`, `>=`) are refused for the same reason since booleans have no order <!-- construct:adding two booleans --> |

## Other targets

These are not covered by the matrix above, which probes the LLVM pipeline. They are
honest about their current state:

| Target | State |
|---|---|
| **LLVM** | The working backend. Executes natively; verified by running generated IR. |
| **QIR** | Structural emission only. A validated circuit text, **not** QPU execution. |
| **WGSL** | Generates shaders, validated with `naga`. No GPU execution has been performed: there is no `/dev/dri`, no Vulkan ICD, and no browser WebGPU in the build environment. |
| **Cranelift** | **Not implemented.** `naso build --target cranelift` refuses with an explicit diagnostic rather than returning a result. |

## QIR and WGSL backends

These refuse a *different* set of constructs than LLVM, and most of the refusals are correct
for them: QIR emits one void quantum operation and has no scalar arithmetic at all, so
refusing `a + b` is not a worse LLVM. Recording these in the same table as the LLVM rows
would imply otherwise, so they are separate.

**Both of these gaps are fixed.** They used to make the QIR backend emit no gate sequence at
all, which left nothing to verify numerically:

- `qalloc` lowering is now in the QIR intrinsic table, so a circuit can be allocated.
- A gate operand is *borrowed*, not consumed, and the QIR gate arm now treats it that way. So
  `hadamard(a)` followed by `measure(a)` — a correct program that uses the linear qubit once in
  each of two different senses — compiles.

The QIR backend now emits one ordered entry function, so a gate acts on the qubit the source
named rather than on a fresh allocation per statement. **What that is checked by, and what it
is not:** the emitted text is parsed back into a gate sequence and replayed on the CPU
state-vector simulator in `naso-gates`, compared against states derived by hand — the Bell
pair, a single Hadamard, and a controlled gate with its control both set and clear. That
verifies operand identity, gate order, and gate completeness on the emitted artifact. It does
**not** verify that a real QIR runtime interprets `qir.*` the same way: is not a claim about Microsoft's own tooling: no QIR conformance suite has been run against any
of this. `qir_entry` also carries no `ENTRYPOINT` marker, because emitting one needs an LLVM
attribute registration inkwell's named-enum path does not provide.

## A Naso quantum program now executes on the CPU

`crates/naso-gates/src/runtime.rs` defines the `qir.*` symbols as a CPU state-vector simulator, so
a `.naso` file compiles all the way to a native process: `naso build --target llvm` -> `llc` -> `cc`
-> run, with no undefined reference left at the end. `compiler/tests/native_quantum_runtime_test.rs`
drives that whole pipeline and asserts on what the process prints.

Getting there required fixing the **ABI**, which was wrong in three ways. `qir.qalloc` returned
`void`, so no handle was ever produced. A qubit lowered to `alloca i1` plus a classical
`store i1 false`, so it was not a qubit. And `qir.measure` returned `void`, so **every measurement
a Naso program performed was discarded**. LLVM now uses the pointer-based ABI the QIR backend
already declared correctly in `qir::primitives::QIR_INTRINSICS`, through one shared mapping instead
of two divergent copies.

What is real: gates act on real handles, and measurement samples the actual amplitude, collapses
the state, and renormalises it. Two unentangled qubits agree about half the time; a Bell pair
agrees every time. Both facts are asserted as **distributions across processes**, never as a fixed
outcome -- a constant-answer runtime passes the Bell check and fails the coin-flip one.

What is **not** claimed: no quantum hardware, no noise model, exponential cost, and no QIR
conformance suite. The source language still offers only four quantum builtins -- `qalloc`,
`hadamard`, `cnot`, `measure` -- so `qir.x`, `qir.cy` and the rest are reachable only through the
QIR text backend or the verifier, not by writing a Naso program.

The gate matrices and their inverses are separately verified in `naso-gates` against the same
CPU simulator, and the bridge refuses any `qir.*` call it does not model rather than skipping
it — skipping would verify a circuit *smaller* than the one emitted.

| Construct | QIR | WGSL straight-line | WGSL compute | Note |
|---|---|---|---|---|
| empty function | **emits** | **emits** | **refused** | a compute kernel with no tensor parameter has nothing to bind <!-- construct:empty function --> |
| qubit allocation and measurement | **emits** | **refused** | **refused** | emits one ordered `qir_entry` allocating the qubit and reading it back with `qir.mz` <!-- construct:qubit allocation and measurement --> |
| a single gate | **emits** | **refused** | **refused** | emits `qir.h` on the allocated qubit; a gate BORROWS, so the qubit is still readable by the measurement <!-- construct:a single gate --> |
| scalar float multiply | **refused** | **emits** | **refused** | QIR has no scalar arithmetic; it emits one void quantum operation <!-- construct:scalar float multiply --> |
| scalar integer add | **refused** | **emits** | **refused** | <!-- construct:scalar integer add --> |
| function call | **refused** | **emits** | **refused** | <!-- construct:function call --> |
| tensor kernel entry point | **emits** | **refused** | **emits** | the only shape the compute path accepts is a tensor-parameter kernel <!-- construct:tensor kernel entry point --> |
| `if` in straight-line WGSL | **refused** | **refused** | **refused** | control flow has no place in a straight-line shader <!-- construct:`if` in straight-line WGSL --> |
| tensor subscript in straight-line WGSL | **refused** | **refused** | **refused** | a `tensor` type has no WGSL equivalent in that position <!-- construct:tensor subscript in straight-line WGSL --> |
| assignment | **refused** | **emits** | **refused** | assignment to a `Var` is not expressible in QIR <!-- construct:assignment --> |

**QIR output is structural.** A validated circuit text is not QPU execution.

**WGSL is validated with `naga`, which is not GPU execution.** There is no `/dev/dri`, no
Vulkan ICD, and no browser WebGPU in the build environment, so no generated shader has
ever been run. `naga` proves the module parses and its types line up; it proves nothing
about what the shader computes.

**A compute kernel is chosen by name and must own tensor parameters.** The compute WGSL
path takes the kernel name as an argument, and refuses a kernel with no tensor parameter
because the ABI is defined in terms of those bindings.

## Verification claims

Stated precisely, because the distinction matters:

- WGSL is checked with `naga`. `naga` validation is **not** GPU execution.
- QIR emission is structural. It is **not** quantum execution. The native
  `.naso` -> LLVM IR -> `llc` -> `cc` -> run pipeline executes the four prelude builtins
  (`qalloc`, `hadamard`, `cnot`, `measure`) against `libnaso_gates.a`; nothing beyond
  those four is reachable from source.
- `sdg` / `tdg` are refused by the QIR backend. They are real, correct quantum operations,
  but the QIR base profile declares no entry point for them and expresses them instead as a
  Z rotation by a sign-dependent angle — which is the rotation this backend already refuses,
  because it has no angle to supply. Mapping them onto `s`/`t` would apply the wrong phase,
  which is the historical defect the gate-inverse table exists to prevent.
- Float proof obligations are reported as warnings, not discharged mathematical-real
  proofs.
- Pointer ABI guards validate the supplied lengths. They cannot prove non-nullness,
  memory safety, lifetime, provenance, or allocation shape.

## Sub-byte `i4` storage

`i4` and `u4` are real types with real packed storage, not names for a byte. `Tensor[i4, N]`
occupies `N / 2` bytes: two values per byte, low nibble first. This is the load-bearing step
toward a miniature model, and it is measured rather than asserted —
`compiler/tests/sub_byte_i4_execution_test.rs` compiles a quantizer, RUNS it, and checks that
16 values write exactly 8 bytes and that a pre-filled 16-byte buffer still has its last 8
bytes untouched after the kernel runs.

**What is implemented**

- `i4` as two's complement over `-8..7`, decoded arithmetically with no branch and no
  `select`: the sign bit is shifted into bit 7 and multiplied by 8.
- Read-modify-write stores. A plain store would clobber the neighbour, and the tensor would
  still occupy the right number of bytes — so the compression ratio would check out while
  the data was garbage. A dedicated test writes one element and asserts the other half of
  that byte survived.
- `as i4` narrowing, which saturates through the source-level `clamp`. Values outside the
  range clamp; they do not wrap.
- Scalar `i4` is **refused**, not widened. A scalar has no neighbour to share a byte with, so
  admitting it would mean a one-byte `i4`, which is the fiction this feature exists to
  eliminate.

**What is not implemented, and is refused or absent rather than faked**

- **`u4` is lexed and parsed but has no distinct storage.** It reuses the signed nibble path,
  so `u4` currently behaves as `i4`. Treat it as unimplemented until a test distinguishes
  them.
- **No `i2`.** The addressing math generalizes, but nothing has been built or measured.
- **Tensor element bounds are modelled; range bounds are not — use `requires`.** A tensor
  parameter is encoded as an *uninterpreted function* from index to element, so an obligation
  about `t[i]` is checked for every possible tensor. The consequence is deliberate and worth
  stating plainly: a bound like `t[i] <= 127` is **refuted** with no premise, because a
  constant tensor with a huge element is a legitimate countermodel.
- **Preconditions are now checked at call sites, with three honest limits.**
  For a call `g(a1, .., an)` the prover discharges
  `caller_requires ⇒ g.requires[a1/x1, .., an/xn]` — the callee's premise with its parameters
  replaced by the caller's actual argument expressions. A call that no premise establishes is
  reported at the call site. The limits:
  1. It is an obligation over the CALLER's parameters, exactly like every other obligation
     here. It is proved for all valuations of those parameters, so it is a statement about the
     caller's own promises, not a runtime check on a particular argument. If the caller's
     preconditions are unsatisfiable the call-site obligation becomes vacuous — the same
     caveat that applies to every obligation in this prover.
  2. Only DIRECT calls to a function with a source definition are checked. A method call names
     its callee through a field, so `t.check(x)` is not resolved to `check` and is not checked.
  3. The substitution handles the expression forms the prover can encode. A form outside that
     set is carried through unrewritten, so a precondition naming it refers to a parameter no
     SMT constant declares and the obligation FAILS LOUDLY as an undeclared symbol. The failure
     mode is a wrong refusal, never a wrong proof.
- **Floats are encoded as EXACT reals. This is sound for a quantisation bound and is NOT a
  claim about IEEE-754 rounding.** `f32`/`f64` literals are emitted as the dyadic rational
  they actually are (`(/ m 2^k)`, or an exact decimal where one terminates), and a float
  parameter is an unconstrained `Real`. So a discharged obligation is a statement about the
  MATHEMATICS of the computation — the ideal quantisation, the exact scale — and nothing here
  bounds the rounding error of the compiled floating-point code, which has a 24-bit
  significand and is neither exact nor unbounded.
  - USE THIS for: "given `s > 0` and `x` within half a step of `q*s`, the dequantised value
    is within half a step of `x`". `kernels/quant_error_bound.naso` is that bound, and it is
    discharged.
  - DO NOT USE THIS for: bit-exact reproducibility, or anything that depends on the
    significand. Closing that gap needs an error term on every float operation, which is not
    built.
  - Non-finite literals are REFUSED, never coerced to a finite stand-in.
- **A proposition may mention a linear value repeatedly; an erased reference is not
  consumption.** A `proof` or `requires` block is erased before codegen, so a reference
  there records nothing: it can neither trip the `[1]` double-use check nor consume a `[n]`
  budget. That means `forall i { assert(t[i] <= 10); assert(t[i] >= 0); }` is legal, and a
  two-sided range can be stated at all.
  The consequence is deliberate and worth stating: a `[1]` value referenced ONLY inside
  erased regions is reported as an **unused linear leak**, because nothing at runtime touches
  it. An earlier version excused it; that let a linear tensor be declared, proved about, and
  silently dropped, which is the failure this compiler exists to make impossible.
- **`abs`, `min`, `max` and `clamp` are defined exactly; `round` is NOT.**

  These four intrinsics are encoded as their real mathematical definitions (`abs` via `ite`,
  `clamp` composed from `min`/`max`), so an obligation discharged using them is a true
  statement about the real function. `kernels/quant_int8.naso` now discharges completely as a
  result, including a theorem that needs **no premise at all**: `clamp(v, -128, 127)` lies in
  `[-128, 127]` for every real `v`. That is what makes the `as i8` narrowing well-defined for
  inputs that violate the quantiser's contract, so the clamp is proved load-bearing rather
  than asserted to be.

  An intrinsic with no exact definition is REFUSED, never declared uninterpreted. Declaring
  `abs` uninterpreted would be strictly worse than refusing: Z3 would treat it as an arbitrary
  function, "prove" claims that are false of the real one, and report success.

- **`round` is axiomatised by its bounding property, with one honest escape hatch.**

  Nearest-integer rounding has no closed exact form over an exact real, and SMT-LIB's `to_int`
  truncates toward zero rather than rounding. It cannot be defined like `abs`/`min`/`max`/`clamp`,
  so `round(x)` is emitted as an uninterpreted `Real -> Real` function and the encoder asserts,
  for every obligation that uses it:

      forall x:Real. (x - 1/2) <= round(x) <= (x + 1/2)

  This is **sound for bounds**: an obligation like `round(x) <= x + 0.5` discharges (it is the
  axiom's upper edge), and a claim that contradicts it refutes. As of `59a43b2` the shipped
  kernel `kernels/quant_int8.naso` itself states the bound -- its proof blocks assert that the
  runtime `round(input[i]/scale)` step's error is <= 1/2 -- so
  `the_shipped_int8_quantiser_kernel_discharges_completely` pins the axiom path through the
  real kernel, not only unit tests.

  The escape hatch is **incompleteness**, stated explicitly: the axiom is a bound, not an
  exact definition, so it does NOT resolve a tie. `round(0.5) == 1.0` is Undecided (both 0 and 1
  satisfy the bound), and because deciding it requires Z3 to find a *model* under a universally
  quantified real axiom -- which does not resolve within the 30s solver budget -- such a query
  is reported as Undecided (`OBL-002`, exit status 2), never as proved. That is the correct,
  conservative answer, not a defect. The consequence for the quantiser is unchanged: the stated
  range `abs(input[i] / scale) <= 127` constrains the **division**, the ideal scale, not the
  rounded quotient. A value of exactly 127.4 divided in and then rounded gives 127 and is fine,
  but that reasoning is not what the proof says. This is the same family of gap as the IEEE-754
  rounding limitation above, one level further down.

- **`naso verify` is not a subcommand of `naso`; it is the `naso-verify` binary.**
  `naso-verify` depends on `naso-compiler` (it parses and lowers the AST), so the compiler
  crate cannot depend on `naso-verify` to implement a subcommand -- that is a dependency cycle,
  which Cargo rejects. The verifier therefore ships as its own binary. This is a real
  constraint, not a preference, and it is why `naso verify` does not exist.

### Exit codes, and what "undecided" means

`naso-verify` distinguishes states that all look like "no output" if collapsed into one:

| code | meaning |
|------|---------|
| 0 | every obligation was DISCHARGED |
| 1 | at least one obligation was REFUTED -- a claim proven false |
| 2 | at least one obligation was LEFT UNDECIDED -- neither confirmed nor refuted |
| 3 | usage error (bad arguments) |
| 4 | input error (file missing, or the program did not parse) |
| 5 | internal verifier failure (solver error, malformed SMT) |
| 6 | no obligations found, and `--require-obligations` was given |

**Code 2 is the one that matters.** Some constructs have no encoding yet and are reported as
unsupported. A tool that returned 0 for both "I proved it" and "I could not look at it" would
have a green build that carries no information. Undecidable is not passing.

`--require-obligations` closes the other hole: without it, a file whose `proof` block has a
typo produces zero obligations and exits 0, indistinguishable from having proved everything.

### The verifier refused three things rather than guess

- `--mode custom` is gone. `prove_custom_vc` built a lowering context, discarded it, and
  returned `Ok(Vec::new())` -- an empty diagnostic list, which every caller reads as "no
  problems found", so `--mode custom` reported a clean bill of health on a verification
  condition that was never checked. It now returns an error, and `--vc-name` is no longer
  accepted.
- The quantity walker's catch-all claimed unrecognised expressions had "no special quantity
  handling needed". That is how `forall i { output[i] = .. }` came to be skipped -- the one
  place a loop body consumes a linear tensor -- and why `naso-verify` reported
  `kernels/scale_clamp_f32` as leaking both its linear tensors while `naso check` accepted the
  same file. Every `ExprKind` is now matched explicitly, so a future variant is a compile
  error; forms the tracker cannot model soundly (closures, `reversible` blocks, aggregates) are
  refused by name.
- A reference to a linear value now counts as its consumption, matching the compiler's rule in
  `TypeEnv::check_use`. A bare variable reference previously recorded nothing at all.
  `naso check` and `naso-verify` now agree on both double-use and leak, including on the
  program used as the original fixture for this bug: both rejected it, for the same reason.

- **A proved obligation is still not "this works for all inputs."** It means no countermodel
  exists within the supported fragment. Unsupported operators, unresolved callees and
  unsatisfiable caller premises are all outside what that sentence covers.
- **A malformed SMT script is now a loud error, but detection is structural, not semantic.**
  Z3's `Solver::from_string` returns `()` and *discards* its error code, so an unparseable
  script used to be dropped silently and answered `sat` for a solver holding no assertions —
  which a verification driver reads as "obligation refuted". `verify()` now refuses instead,
  by checking two things against the script: that parentheses balance, and that the number of
  assertions Z3 actually loaded matches the number the script declares. Both checks are
  structural. A script that is *syntactically* fine but semantically wrong — an undeclared
  symbol, a mistyped sort — is still rejected by Z3 without a diagnostic, so this narrows the
  hole rather than closing it. Treat a `ParseError` as an emitter bug, never as a failed proof.
- **No float reasoning in the prover.** Scale and zero-point error bounds still need an
  interval or rational abstraction. Claiming otherwise would be the exact dishonesty this
  page exists to prevent.
- **`&&` and `||` are refused, not encoded.** They short-circuit, so they are not the same
  proposition as `and`/`or` when an operand is undefined. Split the obligation into separate
  assertions rather than have the prover silently strengthen what you wrote.
- **No `naso verify` subcommand.** The verifier is a real library with passing tests, but the
  CLI does not expose it, so no source-level verification workflow exists yet.
- **Shape must have an even extent.** `Tensor[i4, 15]` is a partial trailing byte whose
  handling is unspecified; it is not rejected either, so do not rely on it.

## Not implemented

Kept deliberately, and refused rather than approximated:

- `reversible { ... }` over anything that is not a gate sequence. A block whose statements
  are all quantum gate applications now lowers, emits the forward pass, and emits the
  uncomputation — and `compiler/tests/reversible_native_execution_test.rs` builds real
  executables and asserts the state is restored over many runs. Everything else in the
  construct is still refused, including the case that was refused longest:

  - Arithmetic and assignments. The inverse of `x = a*b` is `x/b`, which needs to know which
    operand the statement binds, and this pass is not told. The audited generator in
    `reversible_lowering.rs` used to map `Mul -> Div` and default everything else to `Add`,
    which computes a different number rather than undoing one; all six of those fabrications
    are fixed and now refuse by name.
  - Measurement, and `qalloc` inside a block.
  - Empty blocks, and blocks nested in control flow, where the surrounding branch decides
    whether the forward pass ran at all.

  **Rotations are now implemented for `rz`.** `rz(theta, q)` is a keyword, the angle is
  required, and it reaches the runtime as a real `f64` operand on `qir.r1(double, ptr)` — the
  QIR base profile's one parameterized single-qubit gate. So a rotation is emitted rather than
  refused, and inside a `reversible` block it is uncomputed by negating the angle. Verified
  natively: `H; rz(0)` measures 0 every run, `H; rz(pi/2); H` measures ~50/50, and
  `H; rz(pi); H` measures 1 every run, so the angle demonstrably reaches the hardware.

  What is still refused, and why:

  - **`rx` and `ry`.** Real gates in the simulator, but the base profile has no `qir.rx` or
    `qir.ry`, so admitting them would declare entry points this runtime does not export — a
    module that compiles and then fails to LINK. One rotation that works beats three that lie.
  - **A non-literal angle.** `rz(theta, q)` requires a literal. A QIR rotation takes its angle
    as a call-site operand, so a runtime angle expression is not yet plumbed through.
  - **A missing angle.** `rz(q)` is a parse error, because a rotation by an angle of zero is the
    identity — a call that looks like it rotates and does nothing.

  One limit is worth stating plainly: **`rz` cannot be observed by any measurement**, so its
  uncomputation is verified structurally (on the emitted IR, that the two `qir.r1` calls carry
  `+theta` and `-theta`) and at the runtime level, where the amplitude is readable. Three
  separate attempts to test it by measurement distribution failed, because `rz` is a global
  phase and leaves the probability of measuring 1 unchanged for every angle — each would have
  passed against a build with the rotation dropped entirely. Linearity blocks the obvious fix:
  every `[1]` qubit must be consumed, and `measure` is the only consuming operation, so the
  phase is always collapsed before a compiled program could read it. Adding a `release` builtin
  is what would make a native test possible, and that needs its own linear-type argument.

  So `compiler/src/lowering/reversible_lowering.rs` — the TEMPORARY-VALUE path, with its ancilla
  bookkeeping — remains **unwired and refused**. What is wired is a narrower pass,
  `compiler/src/lowering/uncomputation.rs`, which handles gate sequences only. Two modules,
  two different claims: reporting the gate path as though it uncomputed temporaries would
  overstate what works.

  The uncomputation rides in the existing flat `statements` list, in reverse order after the
  forward pass. That is deliberate: every backend already walks `statements` in order, so the
  second half executes with no backend change and, critically, with no new field that a
  backend could silently forget. A dedicated `inverse` carrier would have been an obligation
  on every backend, and an obligation nobody tests is a silent drop.

- `break` / `continue` inside affine `forall` bands. The iteration-domain transformation for
  a constant-prefix guard is implemented and tested in `ir::early_exit`, but no backend emits
  the shortened band yet, and a guard on runtime data has no affine form. A `while` loop
  exit.
- Cranelift, entirely.

## QIR emits valid modules that are not the source program

The QIR backend has, at times, produced LLVM modules that pass every structural and
verification check while computing a different quantum program than the source. This is
recorded because it is the failure mode most likely to be mistaken for success.

Three separate mechanisms contributed, all now addressed:

1. **A gate operand was counted as a consumption.** A `[1]` qubit is linear, so it may be
   *borrowed* any number of times; only being consumed spends it. The linearity checker
   counted every operand occurrence, so `hadamard(a)` followed by `measure(a)` was reported
   as using the qubit twice — a spurious refusal on correct code.

2. **`qalloc` was looked up as `qir.qalloc`.** Lowering names operations the way
   `GateKind` displays them (`H`, `CX`, `reset`) plus `qalloc`; the QIR intrinsic table
   declares `qir.h`, `qir.cx` and `qir.qubit_alloc`. No circuit could be allocated, let alone
   gated. An unrecognised operation is now refused rather than guessed at.

3. **Operand identity was not preserved.** This is the serious one, and it is a property of
   the test fixtures rather than of a single function. `compiler/tests/codegen_tests.rs`
   lowers every quantum operand in a fixture body to a *fresh* `qir.qubit_alloc()`, so
   `teleport.pir` — which says `H q[1]; CNOT q[1], q[2]` and requires `q[1]` to be one qubit in
   both places — emitted an `h` on one allocation and a `cx` on two different ones. The
   emitted module validated. It was not teleportation: no Bell pair was ever created.

The tests passed throughout, because they asserted that the emitted text contained
`call void @qir.h(` and `call void @qir.ccx(`. That text was present. **A test that checks a
gate is present cannot detect a gate being applied to the wrong qubit.**

Two consequences are now enforced:

- `assert_fixture_cannot_share_qubits` asserts the allocation-to-gate ratio. A circuit that
  reuses qubits allocates fewer of them than it applies gates to; the current fixtures emit
  *more*, and that is now a recorded failure mode rather than an invisible one.
- `test_teleport_golden_fixture` asserts that the measurement statement is still reported as
  missing. The builder emits `h`, `cx`, `x` and `z` but drops `S_alice_measure` entirely, so
  Bob's conditional corrections never appear.

The fix has three parts, all now in place: statements are emitted into ONE `qir_entry`
function in order (so the program's order is the circuit's, and nothing is an unreferenced
function), a statement-position `let` outlives its own placeholder body, and a
qubit-producing operation yields a pointer rather than the generic integer zero. The `let`
lifetime is decided by a flag the CALLER sets, never inferred from the body's shape — a
genuine expression body of `0` is indistinguishable from the placeholder.

**What `emits` does not mean here.** A Bell pair now compiles to the correct circuit, with
operand identity pinned by test: `compiler/tests/qir_gate_emission_gap_test` compares the
alloca each gate operand was loaded from, because comparing the load *results* would
compare unequal SSA names even in a correct circuit.

But QIR output from this backend is still structural text. Nothing executes `qir_entry`,
which carries no `ENTRYPOINT` attribute, and no QPU or simulator has seen this output.
Register-array indexing (`q[1]`) remains refused, and the `teleport.pir` fixture still cannot
express a shared qubit — its parser gives every operand a fresh allocation, which is a
property of the fixture, not the backend.
