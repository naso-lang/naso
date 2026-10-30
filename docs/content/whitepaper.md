---
title: "Technical Whitepaper"
weight: 1
---

# Technical Whitepaper: Naso — A Non-aliasing Affine Systems Orchestrator for Provably Safe Quantum & AI Computing

## Executive Summary & The Naso Thesis

Classical memory models (C++, Rust) and quantum SDKs fail at hardware boundaries for three fundamental reasons:

**1. Implicit Pointer Aliasing:** C++ allows arbitrary pointer aliasing; Rust's borrow checker prevents it but at the cost of pervasive lifetime annotations and the friction of `&mut` vs `&` distinction. Neither model accounts for quantum state, where aliasing is physically forbidden by the no-cloning theorem.
A qubit cannot be "borrowed immutably"—observation collapses it.

**2. Untracked Ancilla Decoherence:** Quantum algorithms require temporary ancilla qubits that must be returned to the $|0\rangle$ state before deallocation (Bennett uncomputation).
Existing quantum languages (Q#, Qiskit, Cirq) treat this as a runtime convention or library discipline. When ancilla are implicitly dropped while entangled, the resulting decoherence corrupts the entire computation—silently, irrecoverably, and often only discovered on physical hardware.

**3. Unverified Loop Bounds:** Tensor accelerators (GPUs, TPUs, NPUs) and quantum pulse schedulers require exact iteration space bounds for optimal mapping. Classical compilers rely on heuristic dependence analysis that fails on imperfectly nested loops, non-affine accesses, and dynamic bounds.
The result is suboptimal schedules or, worse, out-of-bounds memory access that manifests only under specific input sizes.

**The Naso Thesis:** *A single type system integrating Quantitative Type Theory (QTT), Mutable Value Semantics (MVS), Polyhedral Loop Compilation, and Z3 SMT verification can simultaneously eliminate all three failure modes at compile time with zero runtime overhead.*
Naso achieves this by making quantity, mutability, and loop structure first-class citizens in the type system, verified by an embedded Z3 solver before any code generation.