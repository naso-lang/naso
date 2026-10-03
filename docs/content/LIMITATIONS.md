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

**Two rows below are genuine gaps rather than design.** LLVM accepts both programs, so the
source language and one backend support them:

- `qalloc` lowers to a quantum operation whose name has no entry in the QIR intrinsic table,
  so a circuit cannot even be allocated. LLVM's quantum arm declares whatever intrinsic a
  name implies; QIR has a fixed declared set and refuses anything outside it.
- A gate operand is *borrowed*, not consumed, but the QIR gate arm re-checks quantity on
  each operand. So `hadamard(a)` followed by `measure(a)` — a correct program that uses the
  linear qubit once in each of two different senses — is reported as using `a` twice.

Until both are fixed, the QIR backend emits no gate sequence at all. That is what blocks
end-to-end numerical verification of compiler-emitted circuits: there is no gate list to
hand a simulator. The gate matrices and their inverses are themselves verified in
`naso-gates` against a CPU state-vector simulator; what is missing is the link from
compiled output into that check.

| Construct | QIR | WGSL straight-line | WGSL compute | Note |
|---|---|---|---|---|
| empty function | **emits** | **emits** | **refused** | a compute kernel with no tensor parameter has nothing to bind <!-- construct:empty function --> |
| qubit allocation and measurement | **refused** | **refused** | **refused** | `qalloc` has no entry in the QIR intrinsic table <!-- construct:qubit allocation and measurement --> |
| a single gate | **refused** | **refused** | **refused** | a gate BORROWS a qubit, but the QIR gate arm re-checks quantity per operand and reports a double use <!-- construct:a single gate --> |
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
