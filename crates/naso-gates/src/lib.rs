//! Gate matrices, the adjoint relation, and CPU state-vector simulation for Naso.
//!
//! # Why this crate exists
//!
//! The gate table first lived in `naso-verify`, which depends on `naso-compiler`. That made
//! it unreachable from the compiler: the compiler cannot depend on the verifier that
//! depends on it. A leaf crate with no dependencies that BOTH depend on breaks the cycle
//! without inverting it.
//!
//! It equally exists to prevent a second, quieter failure: a gate table copied into both
//! crates would drift, and drift in an ADJOINT table does not fail -- it silently computes
//! wrong inverses. One definition, reachable from every layer, is the only arrangement that
//! makes that impossible rather than merely unlikely.
//!
//! # What lives here
//!
//! * [`statevector`] -- complex arithmetic, a dense state vector, and 19 gates acting on
//!   it. Runs headless on the CPU with no dependencies.
//! * [`gate_inverse`] -- the total gate-to-inverse lookup, and the helpers to apply a gate
//!   at the arity it actually needs.
//!
//! # What this is NOT
//!
//! Not a QPU. Memory is `2^n` complex numbers, so this verifies GATE MATRICES at a handful
//! of qubits and gets no better with scale. Passing here says nothing about hardware, and
//! nothing here has been executed on any quantum device.
//!
//! Still unmodelled: `entangle`, Hamiltonian operations, and several quantum proof
//! obligations.

pub mod gate_inverse;
pub mod runtime;
pub mod statevector;
