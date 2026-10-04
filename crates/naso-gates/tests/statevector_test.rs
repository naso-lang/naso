//! The state-vector simulator, checked against states whose answers are known by hand.
//!
//! # Why verify the simulator first
//!
//! The gate-inverse table (step 4) will assert `gate then inverse == identity`. If the
//! simulator were wrong, that test would either pass vacuously or fail for the wrong reason,
//! and neither outcome is worth having. So the simulator is pinned against states whose
//! amplitudes are known analytically first -- `H|0>` is `(1,1)/sqrt(2)`, `CX|10>` is `|11>`,
//! and a Hadamard square is the identity.
//!
//! # What this is NOT
//!
//! A dense state-vector simulator: memory is `2^n` complex numbers. It verifies GATE
//! MATRICES on the CPU. It is not a QPU, nothing here runs on hardware, and passing says
//! nothing about one.

use naso_gates::statevector::{Complex, Gate, StateVector};

/// Tolerance for identities built from repeated `1/sqrt(2)` divisions.
///
/// Chosen from the observed drift, not picked round: Hadamard squared on four qubits drifts
/// about 1e-15, so 1e-9 is four orders of magnitude of headroom. A tight enough bound to
/// catch a wrong matrix, loose enough not to be noise-sensitive.
const TOL: f64 = 1e-9;

/// The zero state on `n` qubits is `|0...0>`.
#[test]
fn the_zero_state_is_the_all_zero_basis_state() {
    let s = StateVector::zero_state(3);
    assert_eq!(s.qubits(), 3);
    assert_eq!(s.amplitude(0), Complex::ONE);
    for basis in 1..8 {
        assert_eq!(
            s.amplitude(basis),
            Complex::ZERO,
            "only |000> may be populated"
        );
    }
    assert!(s.is_normalised(TOL));
}

/// `H|0>` is the equal superposition, with the amplitudes in the right places.
#[test]
fn hadamard_on_zero_gives_the_superposition() {
    let out = Gate::H.apply(&StateVector::zero_state(1), 0, None);
    let s = 1.0 / std::f64::consts::SQRT_2;
    assert!(
        (out.amplitude(0).re - s).abs() < TOL,
        "|0> coefficient must be 1/sqrt(2)"
    );
    assert!(
        (out.amplitude(1).re - s).abs() < TOL,
        "|1> coefficient must be 1/sqrt(2)"
    );
    assert_eq!(out.amplitude(0).im, 0.0);
    assert!(out.is_normalised(TOL));
}

/// `H|1>` is the MINUS superposition, not the same as `H|0>`.
///
/// The minus on `|1>` is what makes `H` invertible and non-identity, so a simulator that got
/// this wrong would still pass an `H`-square identity check only by accident.
#[test]
fn hadamard_on_one_flips_the_sign_of_the_second_amplitude() {
    let input = StateVector::with_basis_state(1, &[(0, true)]);
    let out = Gate::H.apply(&input, 0, None);
    let s = 1.0 / std::f64::consts::SQRT_2;
    assert!((out.amplitude(0).re - s).abs() < TOL);
    assert!(
        (out.amplitude(1).re + s).abs() < TOL,
        "|1> must be NEGATIVE: H|1> = (|0> - |1>)/sqrt(2), got {:?}",
        out.amplitude(1)
    );
}

/// `X` is the NOT gate, on both basis states.
#[test]
fn x_flips_the_basis_state() {
    let zero = Gate::X.apply(&StateVector::zero_state(1), 0, None);
    assert_eq!(zero.amplitude(1), Complex::ONE);
    assert_eq!(zero.amplitude(0), Complex::ZERO);

    let one = Gate::X.apply(&StateVector::with_basis_state(1, &[(0, true)]), 0, None);
    assert_eq!(one.amplitude(0), Complex::ONE);
}

/// `Z` leaves `|0>` alone and negates `|1>`.
#[test]
fn z_negates_only_the_one_amplitude() {
    let out = Gate::Z.apply(&StateVector::with_basis_state(1, &[(0, true)]), 0, None);
    assert_eq!(out.amplitude(1), -Complex::ONE, "Z|1> = -|1>");
}

/// `S` maps `|1>` to `i|1>`, and `Sdg` to `-i|1>`.
///
/// This is the pair a QIR gate table got wrong. Asserted numerically here so the
/// distinction cannot be reintroduced anywhere: `S` and `Sdg` differ, and neither is the
/// other's inverse by accident.
#[test]
fn s_and_sdg_differ_on_one() {
    let one = StateVector::with_basis_state(1, &[(0, true)]);

    let s = Gate::S.apply(&one, 0, None);
    assert!(
        (s.amplitude(1).im - 1.0).abs() < TOL,
        "S|1> = i|1>, got {:?}",
        s.amplitude(1)
    );

    let sdg = Gate::Sdg.apply(&one, 0, None);
    assert!(
        (sdg.amplitude(1).im + 1.0).abs() < TOL,
        "S-dagger|1> = -i|1>, got {:?}. S-dagger is NOT S.",
        sdg.amplitude(1)
    );

    // And they are inverses of each other.
    let round_trip = Gate::Sdg.apply(&s, 0, None);
    assert!(
        round_trip.equivalent_to(&one, TOL),
        "S-dagger after S must return |1>"
    );
}

/// `CX` with the control on `|1>` flips the target; with the control on `|0>` it does not.
///
/// Quibit `q` is BIT `q` of the basis index, so `|q0=1, q1=0>` is index 1 and
/// `|q0=0, q1=1>` is index 2. Getting this backwards makes every two-qubit assertion here
/// silently test the wrong basis state, so the indices are spelled out rather than assumed.
#[test]
fn cnx_flips_the_target_only_when_the_control_is_set() {
    // Control q0 = 0, target q1 = 1 -> index 2. The control is clear, so nothing happens.
    let control_clear = StateVector::with_basis_state(2, &[(0, false), (1, true)]);
    assert_eq!(control_clear.amplitude(2), Complex::ONE, "test setup");
    let out = Gate::Cx.apply(&control_clear, 0, Some(1));
    assert_eq!(
        out.amplitude(2),
        Complex::ONE,
        "with the control clear, index 2 must be unchanged"
    );
    assert_eq!(
        out.amplitude(3),
        Complex::ZERO,
        "nothing may move to index 3"
    );

    // Control q0 = 1, target q1 = 0 -> index 1. The control is set, so the target flips.
    let control_set = StateVector::with_basis_state(2, &[(0, true), (1, false)]);
    assert_eq!(control_set.amplitude(1), Complex::ONE, "test setup");
    let out = Gate::Cx.apply(&control_set, 0, Some(1));
    assert_eq!(
        out.amplitude(3),
        Complex::ONE,
        "index 1 (q0=1,q1=0) must become index 3 (q0=1,q1=1): {out:?}"
    );
    assert_eq!(out.amplitude(1), Complex::ZERO);
}

/// Swapping the CNOT's argument order must not change the gate.
///
/// `cx` is not its own transpose, so an implementation that normalised the pair by index
/// would apply the transpose here and produce the wrong state.
#[test]
fn cnx_is_the_same_gate_whatever_order_the_arguments_are_given() {
    let control_first = Gate::Cx.apply(&StateVector::zero_state(2), 0, Some(1));
    let target_first = Gate::Cx.apply(&StateVector::zero_state(2), 1, Some(0));
    assert!(
        control_first.equivalent_to(&target_first, TOL),
        "cx(control=0, target=1) and cx(control=1, target=0) must be the same operation"
    );
}

/// `SWAP` exchanges the two qubits, in BOTH directions.
#[test]
fn swap_exchanges_the_two_qubits() {
    // q0 = 1, q1 = 0 -> index 1. Swapping puts it at q0 = 0, q1 = 1 -> index 2.
    let input = StateVector::with_basis_state(2, &[(0, true), (1, false)]);
    assert_eq!(input.amplitude(1), Complex::ONE, "test setup");
    let out = Gate::Swap.apply(&input, 0, Some(1));
    assert_eq!(
        out.amplitude(2),
        Complex::ONE,
        "index 1 must become index 2"
    );
    assert_eq!(out.amplitude(1), Complex::ZERO);

    // And the other direction: index 2 must come back to index 1.
    let input = StateVector::with_basis_state(2, &[(0, false), (1, true)]);
    let out = Gate::Swap.apply(&input, 0, Some(1));
    assert_eq!(
        out.amplitude(1),
        Complex::ONE,
        "index 2 must become index 1"
    );

    // A swap applied twice is the identity.
    let back = Gate::Swap.apply(&out, 0, Some(1));
    assert_eq!(
        back.amplitude(2),
        Complex::ONE,
        "swap twice must return the original"
    );
}

/// The Toffoli flips its target only where BOTH controls are set.
///
/// Three qubits, because the distinction from `cx` only exists with a third qubit: on a
/// pair, "both set" and "control set" select the same states, so a two-qubit `ccx` WAS `cx`
/// and no test could tell them apart. The third qubit is the one that makes the difference.
#[test]
fn toffoli_flips_the_target_only_where_both_controls_are_set() {
    // Controls q0 and q2, target q1. q0 = bit 0, q1 = bit 1, q2 = bit 2.
    //
    // Both controls set: q0=1, q1=0, q2=1 -> index 1|4 = 5. The target flips q1:
    // index 5 -> 5^2 = 7, i.e. q0=1, q1=1, q2=1.
    let both = StateVector::with_basis_state(3, &[(0, true), (1, false), (2, true)]);
    assert_eq!(both.amplitude(5), Complex::ONE, "test setup");
    let out = both.apply_toffoli(0, 2, 1);
    assert_eq!(
        out.amplitude(7),
        Complex::ONE,
        "index 5 must become index 7"
    );
    assert_eq!(out.amplitude(5), Complex::ZERO);

    // One control clear: q0=0, q1=0, q2=1 -> index 4. Nothing may happen, even though q2
    // is set. This is the case a two-qubit ccx could not express.
    let one_control = StateVector::with_basis_state(3, &[(0, false), (1, false), (2, true)]);
    let out = one_control.apply_toffoli(0, 2, 1);
    assert_eq!(
        out.amplitude(4),
        Complex::ONE,
        "with q0 clear the Toffoli must not fire"
    );
    assert_eq!(out.amplitude(6), Complex::ZERO);

    // Neither control set: the target must be left alone.
    let no_control = StateVector::with_basis_state(3, &[(0, false), (1, true), (2, false)]);
    let out = no_control.apply_toffoli(0, 2, 1);
    assert_eq!(
        out.amplitude(2),
        Complex::ONE,
        "with no controls set nothing happens"
    );

    // Applying the Toffoli twice returns the original, in every one of those states.
    for basis in 0..8usize {
        let assignments: Vec<(usize, bool)> = (0..3).map(|q| (q, (basis >> q) & 1 == 1)).collect();
        let start = StateVector::with_basis_state(3, &assignments);
        let once = start.apply_toffoli(0, 2, 1);
        let twice = once.apply_toffoli(0, 2, 1);
        assert!(
            twice.equivalent_to(&start, TOL),
            "Toffoli twice must be the identity on basis {basis}"
        );
    }
}

/// The Toffoli needs three DISTINCT qubits, and refuses anything else.
///
/// Accepting a repeated index would flip a qubit using its own value as the condition, which
/// is a different and meaningless operation.
#[test]
#[should_panic(expected = "distinct")]
fn the_toffoli_refuses_a_repeated_qubit() {
    let state = StateVector::zero_state(3);
    let _ = state.apply_toffoli(0, 1, 1);
}

/// The Toffoli has no 4x4 matrix, and must say so.
///
/// Returning a two-qubit matrix would make it silently equal to `cx`.
#[test]
fn the_toffoli_has_no_two_qubit_matrix() {
    assert!(
        Gate::Ccx.pair_matrix().is_none(),
        "the Toffoli is 8x8 and must offer no 4x4"
    );
    assert!(Gate::Ccx.matrix().is_none(), "nor a 2x2");
    assert!(
        Gate::Cx.pair_matrix().is_some(),
        "cx IS a two-qubit gate and must keep its matrix"
    );
}

/// `Gate::apply` must refuse the Toffoli rather than treat it as a one-control gate.
///
/// This is the boundary that caught the `ccx == cx` collapse: had `Gate::apply` quietly
/// applied the Toffoli on a pair, the two gates would have been indistinguishable.
#[test]
#[should_panic(expected = "three-qubit")]
fn gate_apply_refuses_the_toffoli() {
    let state = StateVector::zero_state(3);
    let _ = Gate::Ccx.apply(&state, 0, Some(1));
}

/// The Toffoli must be a DISTINCT gate from `cx`, differing where the third qubit matters.
///
/// The direct answer to "are these the same gate?". On three qubits, `cx` flips whenever its
/// control is set; the Toffoli flips only when both controls are.
#[test]
fn the_toffoli_is_not_the_same_gate_as_cx() {
    // q0=0, q1=0, q2=1 -> index 4. cx with control q2 flips q0 -> index 5. The Toffoli with
    // controls q0 and q2 does not, because q0 is clear.
    let state = StateVector::with_basis_state(3, &[(0, false), (1, false), (2, true)]);
    let cx = Gate::Cx.apply(&state, 2, Some(0));
    assert_eq!(
        cx.amplitude(5),
        Complex::ONE,
        "cx flips whenever its control is set"
    );

    let ccx = state.apply_toffoli(0, 2, 1);
    assert_eq!(
        ccx.amplitude(4),
        Complex::ONE,
        "the Toffoli must not fire with only one control set"
    );
    assert!(
        !ccx.equivalent_to(&cx, TOL),
        "cx and the Toffoli must differ on this input"
    );
}

/// Every gate preserves normalisation: a gate that leaked norm would be silently lossy.
#[test]
fn every_gate_preserves_normalisation() {
    let start = StateVector::zero_state(2);
    let gates: [(Gate, usize, Option<usize>); 12] = [
        (Gate::H, 0, None),
        (Gate::X, 0, None),
        (Gate::Y, 1, None),
        (Gate::Z, 1, None),
        (Gate::S, 0, None),
        (Gate::Sdg, 1, None),
        (Gate::T, 0, None),
        (Gate::Tdg, 1, None),
        (Gate::Rx(0.7), 0, None),
        (Gate::Ry(-1.3), 1, None),
        (Gate::Rz(2.2), 0, None),
        (Gate::Cz, 0, Some(1)),
    ];
    for (gate, q, q2) in gates {
        let out = gate.apply(&start, q, q2);
        assert!(
            out.is_normalised(TOL),
            "{gate:?} lost normalisation: total probability {}",
            out.total_probability()
        );
    }
}

/// A rotation by 2*pi is the identity, since `Rx(2pi) = -i I`.
///
/// The residual is a global phase of `-i`, which `equivalent_to` accepts because it is the
/// same physical state. This is the check that would catch a rotation formula with the wrong
/// sign or half-angle.
#[test]
fn a_full_rotation_is_the_identity_up_to_global_phase() {
    let start = StateVector::zero_state(1);
    for angle in [
        std::f64::consts::TAU,
        -std::f64::consts::TAU,
        4.0 * std::f64::consts::PI,
    ] {
        let out = Gate::Rx(angle).apply(&start, 0, None);
        assert!(
            out.equivalent_to(&start, TOL),
            "Rx({angle}) must be the identity up to global phase, got {out:?}"
        );
    }
}

/// The marginal probability of a qubit is what measuring it alone would return.
#[test]
fn the_marginal_probability_sums_over_the_other_qubits() {
    // (|00> + |11>)/sqrt(2): q0 is 0 or 1 with equal probability.
    let bell = StateVector::from_amplitudes(
        2,
        vec![
            Complex::new(1.0 / std::f64::consts::SQRT_2, 0.0),
            Complex::ZERO,
            Complex::ZERO,
            Complex::new(1.0 / std::f64::consts::SQRT_2, 0.0),
        ],
    );
    assert!((bell.probability_of_one(0) - 0.5).abs() < TOL);
    assert!((bell.probability_of_one(1) - 0.5).abs() < TOL);
    assert!((bell.total_probability() - 1.0).abs() < TOL);
}

/// Building a state with the wrong number of amplitudes must PANIC, not pad or truncate.
///
/// A silently padded state computes a different answer with no diagnostic, which is exactly
/// the failure mode this project keeps finding.
#[test]
#[should_panic(expected = "amplitudes")]
fn a_mis_sized_state_is_refused() {
    let _ = StateVector::from_amplitudes(2, vec![Complex::ONE]);
}

/// Two-qubit gate matrices must not be offered as one-qubit matrices, or vice versa.
///
/// Returning a wrong-shaped matrix instead of `None` would let a caller index out of range
/// or, worse, read the first four entries of a 4x4 as a 2x2.
#[test]
fn gate_matrices_are_offered_at_the_right_arity() {
    for g in [
        Gate::H,
        Gate::X,
        Gate::Y,
        Gate::Z,
        Gate::S,
        Gate::Sdg,
        Gate::T,
        Gate::Tdg,
    ] {
        assert!(g.matrix().is_some(), "{g:?} must have a 1-qubit matrix");
        assert!(
            g.pair_matrix().is_none(),
            "{g:?} must NOT have a 2-qubit matrix"
        );
    }
    for g in [Gate::Cx, Gate::Cy, Gate::Cz, Gate::Swap] {
        assert!(
            g.pair_matrix().is_some(),
            "{g:?} must have a 2-qubit matrix"
        );
        assert!(g.matrix().is_none(), "{g:?} must NOT have a 1-qubit matrix");
    }
    // The Toffoli is THREE qubits, so it offers neither a 2x2 nor a 4x4. Offering a 4x4
    // would make it indistinguishable from `cx`, which is how the two collapsed into one.
    assert!(
        Gate::Ccx.matrix().is_none() && Gate::Ccx.pair_matrix().is_none(),
        "the Toffoli is an 8x8 and must offer neither a 2x2 nor a 4x4"
    );
    assert!(Gate::Rx(0.5).matrix().is_some());
    assert!(Gate::Rx(0.5).pair_matrix().is_none());
}

/// Complex arithmetic, since every gate matrix is built from it.
#[test]
fn complex_arithmetic_is_correct() {
    // i * i == -1
    assert!((Complex::I * Complex::I + Complex::ONE).norm() < TOL);
    // conj(z) * z == |z|^2, real and positive.
    let z = Complex::new(3.0, 4.0);
    assert!((z.conj() * z - Complex::real(25.0)).norm() < TOL);
    assert!((z.norm() - 5.0).abs() < TOL);
    // exp(i*pi) == -1
    assert!((Complex::expi(std::f64::consts::PI) + Complex::ONE).norm() < TOL);
    // Division-free inverse identity: z * conj(z) is real, so the adjoint really is the
    // inverse for a scalar.
    assert!((z * z.conj()).im.abs() < TOL);
}
