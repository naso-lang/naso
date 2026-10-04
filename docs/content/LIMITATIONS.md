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
| quantum parameters | **emits** | `Qubit` and `QRegister` both map to the pointer ABI <!-- construct:quantum parameters --> |
| symbolic loop bounds | **emits** | affine schedule bands <!-- construct:symbolic loop bounds --> |
| `reversible { ... }` | **refused** | no inverse is generated; used to drop the block and report success <!-- construct:`reversible { ... }` --> |
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
**not** verify that a real QIR runtime interprets `qir.*` the same way: no such runtime has
executed any of this. `qir_entry` also carries no `ENTRYPOINT` marker, because emitting one
needs an LLVM attribute registration inkwell's named-enum path does not provide.

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
- QIR emission is structural. It is **not** quantum execution.
- Float proof obligations are reported as warnings, not discharged mathematical-real
  proofs.
- Pointer ABI guards validate the supplied lengths. They cannot prove non-nullness,
  memory safety, lifetime, provenance, or allocation shape.

## Not implemented

Kept deliberately, and refused rather than approximated:

- `reversible { ... }` uncomputation. `compiler/src/lowering/reversible_lowering.rs`
  contains an inverse generator that nothing calls; it is unwired and unverified, and its
  measurement path fabricates a qubit operand.
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
