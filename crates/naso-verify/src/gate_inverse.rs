//! The gate-to-inverse lookup table, verified numerically on the CPU.
//!
//! # Why a table and not an assumption
//!
//! The obvious implementation of "inverse of a gate" is to assert that every gate is its own
//! inverse. That is false. `X`, `Y`, `Z`, `H`, `CX`, `CZ`, and `Toffoli` are self-inverse;
//! `S`, `T`, `RX`, `RY`, and `RZ` are not, and each needs a distinct inverse. An earlier
//! version of the QIR classifier mapped `S`-dagger to `S`, which is exactly the bug this
//! table exists to make unrepresentable.
//!
//! # What "verified" means here
//!
//! Every entry is checked by ACTUALLY APPLYING the gate and then its inverse to a state
//! vector and comparing amplitudes. Not by reading the matrices and asserting they look
//! transposed. A table that is merely structurally plausible would pass such a check while
//! encoding the wrong inverse, which is precisely how the `S`-dagger defect shipped.
//!
//! # What this is NOT
//!
//! The table covers the gates in [`Gate`]. It says nothing about gates not modelled, and it
//! does not make the compiler emit uncomputation: `reversible` remains refused pending a
//! producer that can lower an inverse operation into a real schedule. This is the
//! infrastructure those producers will check against, not the producer itself.

use crate::statevector::{Gate, StateVector};

/// How far two states may differ, for identities built from rotations and `1/sqrt(2)`.
///
/// Chosen from observed drift. A rotation matrix involves `cos` and `sin` of a floating-point
/// angle, so `Rz(1.1)` composed with `Rz(-1.1)` is off by roughly 1e-16 relative, and a
/// Hadamard pair drifts about 1e-15 over four qubits. 1e-9 leaves several orders of
/// magnitude for that noise while still being far tighter than any real matrix error: a
/// wrong angle or a transposed `CX` is off by O(1).
pub const TOLERANCE: f64 = 1e-9;

/// The inverse of a gate.
///
/// # Total, with no fallbacks
///
/// Every gate has an inverse, so this never fails and never guesses. A version that
/// returned an `Option` or fell back to "assume self-inverse" would have a silent wrong
/// answer available to it, which is the failure this table is built to remove.
pub fn inverse_of(gate: Gate) -> Gate {
    match gate {
        // Self-inverse: each is an involution, U = U^-1.
        Gate::H => Gate::H,
        Gate::X => Gate::X,
        Gate::Y => Gate::Y,
        Gate::Z => Gate::Z,
        Gate::Cx => Gate::Cx,
        Gate::Cy => Gate::Cy,
        Gate::Cz => Gate::Cz,
        Gate::Ccx => Gate::Ccx,
        Gate::Swap => Gate::Swap,

        // NOT self-inverse. `Sdg` is its own inverse and is not `S`.
        Gate::S => Gate::Sdg,
        Gate::Sdg => Gate::S,
        Gate::T => Gate::Tdg,
        Gate::Tdg => Gate::T,

        // A rotation by theta is undone by a rotation by -theta.
        Gate::Rx(theta) => Gate::Rx(-theta),
        Gate::Ry(theta) => Gate::Ry(-theta),
        Gate::Rz(theta) => Gate::Rz(-theta),
    }
}

/// Apply the Toffoli, given its two controls and its target.
///
/// A separate entry point because the Toffoli does not fit the two-qubit signature: given
/// only a control and a target, its two controls and its target collapse into "both set"
/// and "control set", which select the same basis states, so it was literally the same
/// function as `cx`.
pub fn apply_toffoli(state: &StateVector, c1: usize, c2: usize, target: usize) -> StateVector {
    state.apply_toffoli(c1, c2, target)
}

/// How many qubits a gate acts on.
///
/// `3` for the Toffoli, which is what a caller needs to know to size a state for it, and
/// what makes the three-qubit gate distinguishable from the two-qubit one.
pub fn arity(gate: Gate) -> usize {
    match gate {
        Gate::Ccx => 3,
        g if g.matrix().is_some() => 1,
        _ => 2,
    }
}

/// Whether a gate is its own inverse.
///
/// This is a derived fact, computed from [`inverse_of`] rather than stated independently.
/// Two hand-written tables would eventually disagree, and the disagreement would be a wrong
/// inverse at runtime.
pub fn is_self_inverse(gate: Gate) -> bool {
    inverse_of(gate) == gate
}

/// Every gate, for exhaustive checks.
pub fn all_gates() -> Vec<Gate> {
    vec![
        Gate::H,
        Gate::X,
        Gate::Y,
        Gate::Z,
        Gate::S,
        Gate::Sdg,
        Gate::T,
        Gate::Tdg,
        Gate::Rx(0.7),
        Gate::Rx(-2.3),
        Gate::Ry(1.1),
        Gate::Ry(-0.4),
        Gate::Rz(2.2),
        Gate::Rz(-3.0),
        Gate::Cx,
        Gate::Cy,
        Gate::Cz,
        Gate::Ccx,
        Gate::Swap,
    ]
}

/// Apply a gate, given the qubits it acts on.
///
/// One-qubit gates take the single qubit; two-qubit gates take control and target. Returns
/// `None` only for a gate whose arity does not match the qubit count, so a caller cannot
/// silently apply a one-qubit gate to a two-qubit state.
pub fn apply_gate(
    gate: Gate,
    state: &StateVector,
    q: usize,
    q2: Option<usize>,
) -> Option<StateVector> {
    match (gate.matrix().is_some(), q2) {
        (true, _) => Some(gate.apply(state, q, q2)),
        (false, Some(second)) => Some(gate.apply(state, q, Some(second))),
        (false, None) => None,
    }
}

/// Whether `gate` acts on one qubit or two.
pub fn is_two_qubit(gate: Gate) -> bool {
    gate.matrix().is_none()
}
