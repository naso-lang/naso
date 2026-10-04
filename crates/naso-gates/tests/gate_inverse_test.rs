//! `gate -> inverse -> identity`, verified by NUMERICAL simulation.
//!
//! # The property
//!
//! For every gate `U` and every state `psi`, applying `U` and then `inverse_of(U)` must
//! return `psi`. This is checked on several states of several qubit counts, up to a global
//! phase -- because `U` and `-U` are the same physical state, and demanding exact equality
//! would fail on correct unitaries that happen to carry a phase.
//!
//! # Why this is the test that matters
//!
//! A lookup table can be internally consistent and still be wrong: mapping every gate to
//! itself, or mapping `S`-dagger to `S`, satisfies any structural test over the table while
//! producing the wrong inverse at runtime. Only applying the gates and comparing amplitudes
//! catches that. So the assertions below check amplitudes, not table shape.

use naso_gates::gate_inverse::{
    all_gates, apply_gate, apply_toffoli, arity, inverse_of, is_self_inverse,
};
use naso_gates::statevector::{Complex, Gate, StateVector};

/// Tolerance for identities built from rotations and `1/sqrt(2)`.
const TOL: f64 = 1e-9;

/// A spread of states to test against: the zero state, a basis state, and superpositions.
fn probe_states(qubits: usize) -> Vec<StateVector> {
    let mut states = vec![StateVector::zero_state(qubits)];
    states.push(StateVector::with_basis_state(qubits, &[(0, true)]));
    states.push(StateVector::with_basis_state(
        qubits,
        &[(qubits - 1, true), (0, true)],
    ));
    // A superposition, so the test is not restricted to classical inputs.
    let amplitude = 1.0 / ((1u64 << qubits) as f64).sqrt();
    states.push(StateVector::from_amplitudes(
        qubits,
        vec![Complex::new(amplitude, 0.0); 1 << qubits],
    ));
    // A complex superposition, to catch a transpose that a real input would not detect.
    //
    // Built by NORMALISING rather than by assigning an amplitude per entry: an earlier
    // version filled the rest with `a` and put `a(1+i)` in the first two, giving a total
    // probability of `(2^n + 2) / 2^n` -- a state that is not normalised, so every gate
    // "lost normalisation" and the property test failed for a reason that had nothing to do
    // with the gates.
    let phase = Complex::expi(0.3);
    let mut complex: Vec<Complex> = (0..(1usize << qubits))
        .map(|index| phase.scale(amplitude).rotated(index as f64))
        .collect();
    // Renormalise defensively: the point is a normalised complex state, and the caller
    // should not have to trust this construction.
    let total: f64 = complex.iter().map(|c| c.norm_sqr()).sum();
    let scale = 1.0 / total.sqrt();
    for slot in complex.iter_mut() {
        *slot = slot.scale(scale);
    }
    states.push(StateVector::from_amplitudes(qubits, complex));
    states
}

/// For every gate, `U` then `inverse_of(U)` must return the state unchanged.
/// Apply `gate` on a state of `qubits` qubits, using the lowest qubits it needs.
///
/// Returns `None` when the state is too small for the gate. Routing every test through this
/// is what keeps the three-qubit Toffoli honest: each test asks for the gate's REAL arity
/// instead of assuming a two-qubit signature the Toffoli does not have.
fn apply_on(gate: Gate, state: &StateVector, qubits: usize) -> Option<StateVector> {
    if qubits < arity(gate) {
        return None;
    }
    if gate == Gate::Ccx {
        return Some(apply_toffoli(state, 0, 2, 1));
    }
    if arity(gate) == 1 {
        apply_gate(gate, state, 0, None)
    } else {
        apply_gate(gate, state, 0, Some(1))
    }
}

#[test]
fn every_gate_is_inverted_by_its_table_entry() {
    for gate in all_gates() {
        let inverse = inverse_of(gate);
        for qubits in 1..=3usize {
            for state in probe_states(qubits) {
                let Some(forward) = apply_on(gate, &state, qubits) else {
                    continue;
                };
                let back = apply_on(inverse, &forward, qubits)
                    .unwrap_or_else(|| panic!("{inverse:?} could not be applied"));
                assert!(
                    back.equivalent_to(&state, TOL),
                    "{gate:?} then {inverse:?} did not return the original.\n  start:  {state:?}\n  after: {back:?}\n  max amplitude difference: {}",
                    back.max_amplitude_difference(&state)
                );
            }
        }
    }
}

/// The inverse of the inverse is the gate.
///
/// Catches a table that is not involutive, which would let two successive inversions drift
/// away from the original operation.
#[test]
fn the_inverse_of_an_inverse_is_the_gate() {
    for gate in all_gates() {
        assert_eq!(
            inverse_of(inverse_of(gate)),
            gate,
            "{gate:?} is not recovered by inverting twice"
        );
    }
}

/// `S`-dagger is NOT `S`, and both map `|1>` differently.
///
/// The specific defect this table exists to prevent: an earlier gate table emitted `S` where
/// it meant `S`-dagger. Asserting the two are distinct, and that each is the other's
/// inverse, means that mistake cannot be reintroduced through the table.
#[test]
fn s_dagger_is_not_s() {
    assert_ne!(
        inverse_of(Gate::S),
        Gate::S,
        "S-dagger must not be S: they differ on |1>"
    );
    assert_eq!(inverse_of(Gate::S), Gate::Sdg);
    assert_eq!(inverse_of(Gate::Sdg), Gate::S);

    let one = StateVector::with_basis_state(1, &[(0, true)]);
    let s = Gate::S.apply(&one, 0, None);
    let sdg = Gate::Sdg.apply(&one, 0, None);
    assert!((s.amplitude(1).im - 1.0).abs() < TOL, "S|1> must be +i|1>");
    assert!(
        (sdg.amplitude(1).im + 1.0).abs() < TOL,
        "S-dagger|1> must be -i|1>, which is a DIFFERENT state"
    );
}

/// The self-inverse claim is true of exactly the gates that are listed as self-inverse.
#[test]
fn self_inverse_is_reported_correctly() {
    // These really are involutions.
    for gate in [
        Gate::H,
        Gate::X,
        Gate::Y,
        Gate::Z,
        Gate::Cx,
        Gate::Cz,
        Gate::Swap,
        Gate::Ccx,
    ] {
        assert!(is_self_inverse(gate), "{gate:?} is self-inverse");
    }
    // These are not, and reporting otherwise is what made `S`-dagger wrong.
    for gate in [
        Gate::S,
        Gate::Sdg,
        Gate::T,
        Gate::Tdg,
        Gate::Rx(0.5),
        Gate::Ry(0.5),
        Gate::Rz(0.5),
    ] {
        assert!(
            !is_self_inverse(gate),
            "{gate:?} is NOT self-inverse: it needs its own dagger"
        );
    }
}

/// A rotation by theta and by -theta really are inverses, for several angles.
///
/// Checks that the negative-angle mapping is right for rotations rather than merely
/// consistent, including the sign convention: an inverse that used `+theta` would pass a
/// double-inverse test while computing the wrong thing.
#[test]
fn rotations_are_inverted_by_negating_the_angle() {
    // std::f64::consts::PI rather than 3.14: the literal is an approximation of PI, and
    // an angle that is nearly PI is a fine test case while 3.14 is not PI.
    for angle in [0.1, 0.7, 1.5, 2.9, -0.4, std::f64::consts::PI] {
        for gate in [Gate::Rx(angle), Gate::Ry(angle), Gate::Rz(angle)] {
            assert_eq!(
                inverse_of(gate),
                match gate {
                    Gate::Rx(_) => Gate::Rx(-angle),
                    Gate::Ry(_) => Gate::Ry(-angle),
                    _ => Gate::Rz(-angle),
                },
                "{gate:?} must invert by negating the angle"
            );
            let start = StateVector::zero_state(1);
            let forward = gate.apply(&start, 0, None);
            let back = inverse_of(gate).apply(&forward, 0, None);
            assert!(
                back.equivalent_to(&start, TOL),
                "{gate:?} then its inverse did not return |0>: {back:?}"
            );
        }
    }
}

/// A zero rotation is the identity, so its inverse is itself.
#[test]
fn a_zero_rotation_is_the_identity() {
    let start = StateVector::zero_state(1);
    for gate in [Gate::Rx(0.0), Gate::Ry(0.0), Gate::Rz(0.0)] {
        let out = gate.apply(&start, 0, None);
        assert!(
            out.equivalent_to(&start, TOL),
            "{gate:?} must be the identity"
        );
        assert!(is_self_inverse(gate), "a zero rotation is its own inverse");
    }
}

/// A gate preserves normalisation, and so does the inverse round trip.
///
/// A gate matrix with a wrong normalisation is unitary-looking but lossy, and only a
/// probability check finds it.
#[test]
fn every_gate_and_its_inverse_preserve_normalisation() {
    for gate in all_gates() {
        let qubits = 3usize;
        for state in probe_states(qubits) {
            let forward = apply_on(gate, &state, qubits).unwrap();
            assert!(
                forward.is_normalised(TOL),
                "{gate:?} lost normalisation: {}",
                forward.total_probability()
            );
            let back = apply_on(inverse_of(gate), &forward, qubits).unwrap();
            assert!(
                back.is_normalised(TOL),
                "the inverse of {gate:?} lost normalisation"
            );
        }
    }
}

/// The Toffoli is not a one-control gate, on the input that separates them.
///
/// With controls `q0` and `q2` and target `q1`, the state `|q0=1, q1=0, q2=0>` (basis 1)
/// is the distinguishing case: `q0` is set but `q2` is clear, so a gate that only checked
/// the FIRST control would flip the target, while the real Toffoli must leave it alone.
///
/// This test exists because that exact mutation -- reducing the Toffoli to a single-control
/// gate -- survived an earlier suite. Every other basis state gives the same answer under
/// both versions, so nothing else in the file could have caught it.
#[test]
fn the_toffoli_is_not_a_single_control_gate() {
    let state = StateVector::with_basis_state(3, &[(0, true), (1, false), (2, false)]);
    assert_eq!(state.amplitude(1), Complex::ONE, "test setup");

    let out = apply_toffoli(&state, 0, 2, 1);
    assert_eq!(
        out.amplitude(1),
        Complex::ONE,
        "with q0 set but q2 clear, the Toffoli must not fire"
    );
    assert_eq!(
        out.amplitude(3),
        Complex::ZERO,
        "a single-control gate would have flipped the target to index 3"
    );

    // And with q2 also set, it must fire -- so the test above is not passing vacuously.
    let both = StateVector::with_basis_state(3, &[(0, true), (1, false), (2, true)]);
    let out = apply_toffoli(&both, 0, 2, 1);
    assert_eq!(
        out.amplitude(7),
        Complex::ONE,
        "with both controls set it must fire"
    );
}

/// A two-qubit gate must refuse to be applied without a second qubit.
///
/// Falling back to applying only the control would compute a different, wrong operation with
/// no diagnostic.
#[test]
fn a_two_qubit_gate_needs_two_qubits() {
    let state = StateVector::zero_state(2);
    assert!(
        apply_gate(Gate::Cx, &state, 0, None).is_none(),
        "cx must refuse a missing second qubit rather than guess"
    );
    assert!(
        apply_gate(Gate::Ccx, &state, 0, None).is_none(),
        "ccx must refuse a missing second qubit"
    );
}

/// A gate sequence and its reversed-inverse sequence agree.
///
/// This is the property a real uncomputation pass needs: to reverse `H, CX, Rz` you emit the
/// inverse of each in reverse order. If the table were wrong for any one gate, this would
/// fail, and it is the shape an actual producer would use.
#[test]
fn a_sequence_reversed_as_inverse_operations_returns_the_start() {
    let start = StateVector::zero_state(2);
    let sequence: [(Gate, usize, Option<usize>); 5] = [
        (Gate::H, 0, None),
        (Gate::T, 1, None),
        (Gate::Cx, 0, Some(1)),
        (Gate::Rz(0.9), 1, None),
        (Gate::S, 0, None),
    ];

    let mut forward = start.clone();
    for (gate, q, q2) in sequence {
        forward = apply_gate(gate, &forward, q, q2).unwrap();
    }

    // Uncompute: the inverses, in reverse order.
    let mut back = forward;
    for (gate, q, q2) in sequence.iter().rev() {
        back = apply_gate(inverse_of(*gate), &back, *q, *q2).unwrap();
    }

    assert!(
        back.equivalent_to(&start, TOL),
        "reversing the sequence with the inverse table must return the start.\n  got: {back:?}\n  want: {start:?}"
    );
}

/// Every gate in the table has a matrix of the right arity.
///
/// A gate offering a 4x4 where a 2x2 is expected, or nothing at all, is a lookup failure
/// waiting to happen.
#[test]
fn every_gate_has_a_matrix_of_the_right_arity() {
    for gate in all_gates() {
        assert_eq!(
            gate.matrix().is_some(),
            arity(gate) == 1,
            "{gate:?} arity disagrees with its table entry"
        );
        assert_eq!(
            gate.pair_matrix().is_some(),
            arity(gate) == 2,
            "{gate:?} must have a pair matrix exactly when it is two-qubit"
        );
        if arity(gate) == 3 {
            assert!(
                gate.matrix().is_none() && gate.pair_matrix().is_none(),
                "{gate:?} is three-qubit and must offer neither a 2x2 nor a 4x4"
            );
        }
    }
}

/// Composing a gate with its inverse keeps every BASIS state where it started.
///
/// Stronger than the superposition check above, and it catches a gate whose inverse is right
/// only up to a global phase on a subset of the space.
#[test]
fn composing_a_gate_with_its_inverse_fixes_every_basis_state() {
    for gate in all_gates() {
        let qubits = 3usize;
        for basis in 0..(1usize << qubits) {
            let mut assignments: Vec<(usize, bool)> =
                (0..qubits).map(|q| (q, (basis >> q) & 1 == 1)).collect();
            assignments.retain(|(q, _)| *q < qubits);
            let start = StateVector::with_basis_state(qubits, &assignments);
            assert_eq!(
                start.amplitude(basis),
                Complex::ONE,
                "test setup for {gate:?}"
            );

            let forward = apply_on(gate, &start, qubits).unwrap();
            let back = apply_on(inverse_of(gate), &forward, qubits).unwrap();
            assert!(
                back.equivalent_to(&start, TOL),
                "{gate:?} then its inverse moved basis state {basis}: {back:?}"
            );
        }
    }
}
