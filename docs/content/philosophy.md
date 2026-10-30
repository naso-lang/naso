---
title: "Philosophy & Zen"
weight: 15
---

# Philosophy & Zen of Naso

## The 4 Naso Axioms

These axioms are not guidelines—they are the mathematical bedrock on which Naso stands. Violate them, and the compiler rejects your code. No exceptions.

### Axiom 1: Information is Physical
> **Linear resources (`[1]`) enforce physical law—quantum no-cloning & entropy conservation are type rules, not runtime checks.**

In classical computing, copying a pointer is free. In physics, cloning a quantum state is forbidden.
Naso makes this distinction *type-theoretic*: a `[1] Qubit` cannot be copied, cannot be implicitly dropped, cannot be aliased. The type system *is* the physics engine. When you write `let q = qalloc()`, you are not "allocating memory"—you are asserting exclusive ownership of a physical two-level quantum system.
The compiler enforces the no-cloning theorem at the type level. Entropy is not a metaphor here; `[1]` resources *must* be consumed exactly once, mirroring Landauer's principle: erasure has a thermodynamic cost, and Naso makes you pay it explicitly via `uncompute` or consumption.

### Axiom 2: Proofs Over Overhead
> **Compile-time Z3 proofs replace runtime checks, GC pauses, and dynamic tracing. If Z3 cannot prove it, the machine will not execute it.**

Every array bounds check, every null check, every linear usage verification, every quantum uncomputation validity—these are not "safety features" that add overhead.
They are *proof obligations* discharged by Z3 during compilation. The generated machine code contains *zero* runtime checks for verified properties. A `[0] Proof` carries exactly zero bits at runtime. A linear buffer `inout` compiles to a register-to-register move with no bounds checks, no reference counting, no borrow-tracking metadata.
The overhead is shifted left—into the compiler, where it belongs.

### Axiom 3: Deterministic Execution

> **No non-deterministic allocation, no hidden background runtime, no GC pauses. Every cycle is accounted for in the type system.**

Naso has no garbage collector. No finalizers. No weak references. No background threads sweeping memory.
Memory is managed by *scope* and *quantity*: `[1]` resources are deallocated at their single consumption point; `[*]` resources follow lexical scope (RAII); `[0]` resources never exist at runtime. Quantum ancilla are returned to $|0\rangle$ by verified `uncompute` blocks.
The execution timeline is fully determined by the source code—there are no "stop-the-world" events, no allocation jitter, no priority inversions from a GC thread. This is not "real-time" as a feature; it is *determinism* as a consequence of the type system.

### Axiom 4: Zero Hidden State
> **Uncompute intermediate quantum ancilla explicitly or fail the build. Bennett uncomputation is mandatory, not optional.**

In quantum computing, every temporary qubit (ancilla) used during computation *must* be returned to the $|0\rangle$ state before it can be safely deallocated or reused.
Failure to do so leaves entanglement "leaking" into the environment—decoherence that corrupts the entire computation. Naso makes this a *compile-time requirement*: the `uncompute { ... }` block is not syntactic sugar; it is a verified adjoint generator.
The compiler *proves* (via Z3) that the block's forward computation is unitary and that its adjoint correctly restores all ancilla to $|0\rangle$. If you omit the `uncompute` block, or if the block contains irreversible operations (measurement, classical control flow on quantum state), the build fails.
There is no "runtime cleanup" for quantum state—physics doesn't allow it, and Naso doesn't pretend it does.