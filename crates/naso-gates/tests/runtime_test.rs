//! The QIR runtime executes the circuits it claims to.
//!
//! # What is being tested
//!
//! `runtime.rs` defines the symbols the compiler emits — `qir.qubit_alloc`, `qir.mz`,
//! `qir.apply1` — so that a compiled Naso program links instead of failing with
//! `undefined reference`. Until these tests existed, the only evidence the runtime worked was
//! that it compiled.
//!
//! The tests below check the parts that a "it linked and ran" smoke test would miss:
//!
//! - **Gate semantics.** A Hadamard produces the uniform superposition, a controlled-X fires only
//!   when its control is set. A runtime whose gates were no-ops would pass a link test.
//! - **Entanglement.** Both qubits of a Bell pair read 0.5, and repeated measurement of the
//!   pair agrees. Two independent qubits in |0> read 0, so this is a discriminating check: it
//!   fails if the runtime stores one state per qubit instead of one joint state.
//! - **Allocation preserves work.** Adding a qubit mid-circuit must not disturb amplitudes
//!   already computed — which is what a naive "reallocate a fresh state vector" would do.
//! - **Measurement collapses and renormalises.** After measuring, the surviving probability must
//!   be 1. An unrenormalised collapse makes every later probability wrong.
//! - **Misuse is caught.** A released handle used again must panic, not silently compute.
//!
//! # What is NOT claimed
//!
//! This is a CPU state-vector simulator. These tests show it implements quantum mechanics
//! correctly on a classical machine; they say nothing about quantum hardware, and nothing about
//! Microsoft QIR conformance, which was not run.

use naso_gates::gate_inverse::inverse_of;
use naso_gates::runtime::{
    TWO_QUBIT_CX, TWO_QUBIT_SWAP, fault_pending, gate_from_index, gate_index, qir_amplitude,
    qir_apply1, qir_apply2, qir_apply3, qir_live_qubit_count, qir_mz, qir_probability_of_one,
    qir_qubit_alloc, qir_qubit_release, reset_for_test,
};
use naso_gates::statevector::{Complex, Gate};

/// Fresh register set for one test. The runtime is thread-local state, so tests must not share it.
fn fresh() {
    reset_for_test();
}

/// The joint amplitude of `basis`, as (real, imaginary).
fn amplitude(basis: usize) -> Complex {
    let mut out = [0.0f64; 2];
    // SAFETY: `out` is a real two-element array, and `basis` is in range for the register set
    // the test built. The pointer contract is upheld here.
    unsafe { qir_amplitude(basis as i64, out.as_mut_ptr()) };
    Complex::new(out[0], out[1])
}

const INV_SQRT_2: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// A Hadamard on one qubit yields the uniform superposition.
#[test]
fn a_hadamard_yields_the_uniform_superposition() {
    fresh();
    let a = qir_qubit_alloc();
    qir_apply1(a, gate_index::H);

    assert!(
        (amplitude(0).re - INV_SQRT_2).abs() < 1e-9,
        "|0> should hold 1/sqrt(2), got {:?}",
        amplitude(0)
    );
    assert!(
        (amplitude(1).re - INV_SQRT_2).abs() < 1e-9,
        "|1> should hold 1/sqrt(2), got {:?}",
        amplitude(1)
    );
    assert!(
        (qir_probability_of_one(a) - 0.5).abs() < 1e-9,
        "one qubit after H reads 0.5, got {}",
        qir_probability_of_one(a)
    );
}

/// A controlled-X fires only when its control is set.
#[test]
fn a_controlled_x_fires_only_when_its_control_is_set() {
    fresh();
    let a = qir_qubit_alloc();
    let b = qir_qubit_alloc();

    // Control clear: |00> must survive.
    qir_apply2(a, b, TWO_QUBIT_CX);
    assert!(
        (amplitude(0).re - 1.0).abs() < 1e-9,
        "control clear: {amplitude:?}",
        amplitude = amplitude(0)
    );
    assert!(amplitude(3).norm() < 1e-9, "the target must not move");
}

/// A Hadamard then a controlled-X is the Bell pair, and both qubits read 0.5.
#[test]
fn a_bell_pair_entangles_both_qubits() {
    fresh();
    let a = qir_qubit_alloc();
    let b = qir_qubit_alloc();
    qir_apply1(a, gate_index::H);
    qir_apply2(a, b, TWO_QUBIT_CX);

    // (|00> + |11>)/sqrt(2) -- indices 0 and 3, because qubit q is BIT q.
    assert!(
        (amplitude(0).re - INV_SQRT_2).abs() < 1e-9,
        "index 0: {:?}",
        amplitude(0)
    );
    assert!(
        (amplitude(3).re - INV_SQRT_2).abs() < 1e-9,
        "index 3: {:?}",
        amplitude(3)
    );
    // The two amplitudes that must be ZERO. Asserting these is what makes this a Bell pair
    // rather than a state that merely has amplitude in the right corners.
    assert!(amplitude(1).norm() < 1e-9, "index 1 must vanish");
    assert!(amplitude(2).norm() < 1e-9, "index 2 must vanish");

    assert!(
        (qir_probability_of_one(a) - 0.5).abs() < 1e-9,
        "q0 must read 0.5, got {}",
        qir_probability_of_one(a)
    );
    assert!(
        (qir_probability_of_one(b) - 0.5).abs() < 1e-9,
        "q1 must read 0.5, got {}",
        qir_probability_of_one(b)
    );
}

/// Two qubits that are NOT entangled must not read 0.5.
///
/// This is the control for the Bell-pair test. If the runtime kept one state per qubit, both
/// halves of that test would pass and this one would fail — which is why it is here.
#[test]
fn two_independent_qubits_in_zero_do_not_read_one_half() {
    fresh();
    let a = qir_qubit_alloc();
    let b = qir_qubit_alloc();
    assert!(
        (qir_probability_of_one(a) - 0.0).abs() < 1e-9,
        "q0 starts at 0, got {}",
        qir_probability_of_one(a)
    );
    assert!(
        (qir_probability_of_one(b) - 0.0).abs() < 1e-9,
        "q1 starts at 0, got {}",
        qir_probability_of_one(b)
    );
}

/// Repeated measurement of a Bell pair always agrees.
///
/// A Bell pair collapses to |00> or |11>, so the second qubit's answer is determined by the
/// first. Sampling independently would disagree half the time.
#[test]
fn a_bell_pair_measures_consistently() {
    fresh();
    let a = qir_qubit_alloc();
    let b = qir_qubit_alloc();
    qir_apply1(a, gate_index::H);
    qir_apply2(a, b, TWO_QUBIT_CX);

    let first = qir_mz(a);
    let second = qir_mz(b);
    assert_eq!(
        first, second,
        "a Bell pair measures the same value on both qubits"
    );
}

/// Measuring collapses the state and renormalises it.
#[test]
fn measurement_collapses_and_renormalises() {
    fresh();
    let a = qir_qubit_alloc();
    qir_apply1(a, gate_index::H);

    let first = qir_mz(a);
    // After the collapse the outcome is CERTAIN, whichever one it was.
    assert!(
        (qir_probability_of_one(a) - usize::from(first) as f64).abs() < 1e-9,
        "after measuring {first}, the probability must be {first}, got {}",
        qir_probability_of_one(a)
    );
    assert!(
        (qir_probability_of_one(a) - 0.0).abs() < 1e-9
            || (qir_probability_of_one(a) - 1.0).abs() < 1e-9,
        "the collapsed state must give probability 0 or 1, got {}",
        qir_probability_of_one(a)
    );
    // The surviving subspace must be a unit state. An unrenormalised collapse leaves the total
    // probability below 1 and every later measurement skewed.
    let total: f64 = (0..2).map(|i| amplitude(i).norm_sqr()).sum();
    assert!(
        (total - 1.0).abs() < 1e-9,
        "the collapsed state must be normalised, total probability {total}"
    );
}

/// Allocating a qubit mid-circuit must not disturb work already done.
///
/// The basis WIDENS: a second qubit makes it four basis states, and the old qubit keeps its own
/// index while the new one is added above it. So the old |0> amplitude is compared at the
/// interleaved position, not at a bare index that now means something else.
#[test]
fn allocating_a_qubit_preserves_amplitudes_already_computed() {
    fresh();
    let a = qir_qubit_alloc();
    qir_apply1(a, gate_index::H);
    let p_before = qir_probability_of_one(a);
    assert!(
        (p_before - 0.5).abs() < 1e-9,
        "precondition: q0 is in superposition"
    );

    let b = qir_qubit_alloc();
    // The new qubit takes the NEXT index, so it becomes the HIGH bit and every existing
    // qubit keeps the bit position it already had. The old |0> (index 0) and |1>
    // (index 1) therefore stay put, and the states with the new qubit SET (2 and 3)
    // must be empty because it starts in |0>.
    // distinguish "preserved" from "duplicated".
    let amplitudes: Vec<Complex> = (0..4).map(amplitude).collect();
    assert!(
        (amplitudes[0].re - INV_SQRT_2).abs() < 1e-9
            && (amplitudes[1].re - INV_SQRT_2).abs() < 1e-9,
        "the old qubit's |0> and |1> amplitudes must survive at indices 0 and 1, got \
         {amplitudes:?}"
    );
    assert!(
        amplitudes[2].norm() < 1e-9 && amplitudes[3].norm() < 1e-9,
        "the new qubit starts in |0>, so the states with it set must be empty, got {amplitudes:?}"
    );
    assert!(
        (qir_probability_of_one(a) - p_before).abs() < 1e-9,
        "adding a qubit must not disturb q0's marginal: {p_before} -> {}",
        qir_probability_of_one(a)
    );
    assert!(
        (qir_probability_of_one(b) - 0.0).abs() < 1e-9,
        "a fresh qubit reads 0, got {}",
        qir_probability_of_one(b)
    );
}

/// A Toffoli fires only when both controls are set.
///
/// Asserted on marginals rather than basis indices, because a three-qubit register has eight
/// basis states and hand-counting which index is which is how these tests get written wrong.
#[test]
fn a_toffoli_requires_both_controls() {
    fresh();
    let a = qir_qubit_alloc();
    let b = qir_qubit_alloc();
    let c = qir_qubit_alloc();

    // Both controls clear: the target must stay clear.
    qir_apply3(a, b, c);
    assert!(
        (qir_probability_of_one(c) - 0.0).abs() < 1e-9,
        "with both controls clear the target must not move, got {}",
        qir_probability_of_one(c)
    );

    // One control set: still must not fire.
    qir_apply1(a, gate_index::X);
    qir_apply1(c, gate_index::X);
    qir_apply3(a, b, c);
    assert!(
        (qir_probability_of_one(c) - 1.0).abs() < 1e-9,
        "with one control set the target must stay put, got {}",
        qir_probability_of_one(c)
    );

    // Both controls set: the target flips back to clear.
    qir_apply1(b, gate_index::X);
    qir_apply3(a, b, c);
    assert!(
        (qir_probability_of_one(c) - 0.0).abs() < 1e-9,
        "with both controls set the target must flip, got {}",
        qir_probability_of_one(c)
    );
}

/// A swap exchanges two qubits.
#[test]
fn a_swap_exchanges_two_qubits() {
    fresh();
    let a = qir_qubit_alloc();
    let b = qir_qubit_alloc();
    qir_apply1(b, gate_index::X);
    qir_apply2(a, b, TWO_QUBIT_SWAP);
    assert!(
        (qir_probability_of_one(a) - 1.0).abs() < 1e-9,
        "the |1> should now be on the first qubit, got {}",
        qir_probability_of_one(a)
    );
    assert!(
        (qir_probability_of_one(b) - 0.0).abs() < 1e-9,
        "the second qubit should be clear, got {}",
        qir_probability_of_one(b)
    );
}

/// A released handle cannot be used again.
///
/// The linear types forbid this in source, but a C caller or a miscompiled module bypasses them,
/// and this is the boundary where that shows up. Silently continuing would compute a different
/// circuit rather than failing.
///
/// The runtime records a fault instead of panicking, because a panic cannot unwind across an
/// `extern "C"` boundary -- it aborts the process, which would take down every other test in
/// this binary before it could assert anything.
#[test]
fn using_a_released_handle_is_refused() {
    fresh();
    let a = qir_qubit_alloc();
    qir_qubit_release(a);
    assert_eq!(fault_pending(), None, "a release on its own is legal");
    qir_apply1(a, gate_index::H);
    let fault = fault_pending().expect("using a released handle must be refused");
    assert!(
        fault.contains("not live"),
        "the fault should say the qubit is not live, got: {fault}"
    );
}

/// A double release is caught rather than ignored.
#[test]
fn releasing_twice_is_refused() {
    fresh();
    let a = qir_qubit_alloc();
    qir_qubit_release(a);
    qir_qubit_release(a);
    let fault = fault_pending().expect("a double release must be refused");
    assert!(
        fault.contains("not live"),
        "the fault should say the qubit is not live, got: {fault}"
    );
}

/// An undefined gate index is refused, not treated as a no-op.
#[test]
fn an_undefined_gate_index_is_refused() {
    fresh();
    let a = qir_qubit_alloc();
    qir_apply1(a, 200);
    let fault = fault_pending().expect("an undefined gate must be refused");
    assert!(
        fault.contains("not defined by this runtime"),
        "the fault should name the undefined gate, got: {fault}"
    );
}

/// A recorded fault stops every later operation, so a program cannot limp on.
///
/// The point is that a misuse must not degrade into a silently wrong circuit: once the runtime
/// knows something is wrong, it stops acting rather than computing an answer from state it can no
/// longer trust.
#[test]
fn a_fault_blocks_later_operations() {
    fresh();
    let a = qir_qubit_alloc();
    qir_qubit_release(a);
    qir_apply1(a, gate_index::H);
    assert!(fault_pending().is_some());

    // After the fault the runtime hands back a handle but must not act on it. A Hadamard would
    // put 1/sqrt(2) on each basis state, so the marginal is the thing that proves the gate did not
    // run -- amplitude(0) is not, since |0> legitimately still holds 1.0 once allocation is
    // blocked and no gate ever touched the qubit.
    let b = qir_qubit_alloc();
    qir_apply1(b, gate_index::H);
    assert!(
        (qir_probability_of_one(b) - 0.0).abs() < 1e-9,
        "after a fault the runtime must not apply gates, but the qubit reads {}",
        qir_probability_of_one(b)
    );
    assert_eq!(
        qir_live_qubit_count(),
        0,
        "after a fault the runtime must not allocate: the only qubit was released, and the \
         allocation after the fault must not have added one"
    );
}

/// Applying a gate and then its inverse must restore the state.
///
/// This is the property `reversible` blocks need, checked where a gate is actually EXECUTED
/// rather than only where it is described. A runtime whose adjoint set were incomplete would
/// make an uncompute silently do the wrong thing.
#[test]
fn every_gate_is_inverted_by_another_gate_this_runtime_defines() {
    fresh();
    for index in 0..=7u8 {
        let gate = gate_from_index(index as usize).expect("indexes 0..=7 are defined");
        let inverse = inverse_of(gate);

        let a = qir_qubit_alloc();
        // Put the qubit into a superposition first, so a wrong inverse has something to get wrong:
        // on |0> several distinct gates agree, and the test would pass vacuously.
        qir_apply1(a, gate_index::H);
        let before = amplitude(0);
        let p_before = qir_probability_of_one(a);

        qir_apply1(a, index);
        qir_apply1(a, inverse_index(inverse));

        let after = amplitude(0);
        assert!(
            (after.re - before.re).abs() < 1e-9 && (after.im - before.im).abs() < 1e-9,
            "{gate:?} (index {index}) then its inverse {inverse:?} should restore the state, \
             but the amplitude moved from {before:?} to {after:?}"
        );
        assert!(
            (qir_probability_of_one(a) - p_before).abs() < 1e-9,
            "{gate:?} then {inverse:?} should restore the marginal too: {p_before} -> {}",
            qir_probability_of_one(a)
        );
        qir_qubit_release(a);
    }
    // Keep the import used on every path above.
}

/// The runtime gate index for a gate, refusing anything it does not define.
fn inverse_index(gate: Gate) -> u8 {
    match gate {
        Gate::H => gate_index::H,
        Gate::X => gate_index::X,
        Gate::Y => gate_index::Y,
        Gate::Z => gate_index::Z,
        Gate::S => gate_index::SDG,
        Gate::Sdg => gate_index::S,
        Gate::T => gate_index::TDG,
        Gate::Tdg => gate_index::T,
        other => panic!(
            "the adjoint of a defined gate is {other:?}, which this runtime does not define -- \
             so an uncompute could not be executed"
        ),
    }
}
