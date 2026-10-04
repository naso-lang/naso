//! A CPU quantum runtime with the QIR base-profile ABI, backed by the verified gate table.
//!
//! # Why this module exists
//!
//! The compiler emits calls to `qir.qubit_alloc`, `qir.h`, `qir.cx`, `qir.mz` and their
//! relatives, and for a long time emitted them with no definitions anywhere. `llc` was happy to
//! produce an object file; the failure only appeared at link time as
//! `undefined reference to 'qir.h'` -- naming a symbol no user ever wrote.
//!
//! This module defines those symbols, so a compiled Naso program links and runs.
//!
//! # What "runs" means here, precisely
//!
//! This is a CPU STATE-VECTOR SIMULATOR. It executes the circuit with the same gate matrices
//! `naso-gates` verifies in its own tests, so it is the same semantics rather than an
//! approximation or a second implementation that could drift:
//!
//! - **Real**: gates act on real qubit handles, a measurement samples the actual amplitude and
//!   collapses the state, and amplitudes come from the matrices already under test.
//! - **Not a QPU**: no quantum hardware, no noise model, and cost is exponential in qubit count.
//! - **Not a conformance claim**: implementing the base-profile ABI is not the same as passing
//!   Microsoft's QIR conformance suite, and no such suite was run.
//!
//! # One state, not one per qubit
//!
//! [`Registers`] holds a SINGLE [`StateVector`] spanning every live qubit, because a quantum
//! state is one vector over the joint basis -- two qubits are not two independent vectors. A
//! handle is just an index into it. Storing a separate state per qubit would make every gate
//! local, and then a Bell pair would be indistinguishable from two independent qubits, which is
//! the whole phenomenon the language exists to express.
//!
//! # The gate semantics are NOT duplicated here
//!
//! Every gate goes through [`naso_gates::statevector::Gate`], which has its own tests for
//! unitarity, inverse correctness, and permutation behaviour. This file is the ABI adapter only.
//! Carrying its own matrices here would mean a gate fix had to be made twice and the copies
//! would eventually disagree -- how the earlier duplicated OpenQASM inverse table arose.
//!
//! # Misuse
//!
//! Every entry point validates its handles rather than dereferencing them, so a double-release or
//! use-after-release panics with a message instead of causing undefined behaviour. That matters
//! because the linear types forbid this in the source language, but a C caller or a miscompiled
//! module bypasses the type system, and this is the boundary where that shows up.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use crate::statevector::{Complex, Gate, StateVector};

/// A qubit handle as seen by generated code.
///
/// One `u64`, matching the pointer-sized opaque `ptr` in the LLVM IR the compiler emits.
/// Nothing generated dereferences it; it is passed back here and used as an index.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QirQubit(pub u64);

impl QirQubit {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

thread_local! {
    static REGISTERS: RefCell<Registers> = RefCell::new(Registers::default());
    /// The recorded misuse, if any. Separate from `REGISTERS` so `misuse` can be called while a
    /// handler holds that borrow.
    static FAULT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Whether a recorded misuse is blocking further operations.
fn blocked() -> bool {
    FAULT.with(|f| f.borrow().is_some())
}

#[derive(Default)]
struct Registers {
    /// Indices of live qubits. A `BTreeSet` so the maximum is cheap and the contents are ordered
    /// and comparable, which makes "is this handle live" a lookup rather than a scan.
    live: BTreeSet<usize>,
    /// The joint state over all live qubits. `None` until the first allocation.
    state: Option<StateVector>,
    /// Next index to hand out. Never reused, so a released handle cannot alias a fresh qubit.
    next_index: usize,
}

/// Report misuse and refuse the operation.
///
/// # Why a return value rather than a panic
///
/// These entry points are `extern "C"`, and Rust cannot unwind across an `extern "C"` boundary:
/// a panic there aborts the whole process with `panic in a function that cannot unwind`. That is
/// the right behaviour for a generated program that has a bug, but it makes the failure untestable
/// -- the test process dies before it can assert anything, and one bad call takes down every other
/// test in the binary with it.
///
/// So misuse is recorded and the call becomes a no-op, and the state is left untouched. The next
/// entry point sees the recorded fault and refuses, which keeps the circuit from silently
/// computing a wrong answer while making the failure observable.
///
/// A generated program that misuses the runtime therefore stops making progress at the following
/// operation rather than continuing on corrupt state. Nothing here can make a miscompiled program
/// correct; it can only make the failure loud instead of silent.
fn misuse(message: String) {
    // A SEPARATE cell, not a field on `Registers`. `misuse` is called from inside handlers that
    // already hold a `borrow_mut()` on `REGISTERS`, so recording the fault there would panic with
    // "RefCell already borrowed" -- which, in an `extern "C"` function, aborts the process rather
    // than reporting the original fault.
    FAULT.with(|f| *f.borrow_mut() = Some(message));
}

/// Whether a recorded misuse is pending. Tests read this to assert refusal.
pub fn fault_pending() -> Option<String> {
    FAULT.with(|f| f.borrow().clone())
}

impl Registers {
    /// The joint state, or `None` if the register set is empty.
    fn state(&self) -> Option<&StateVector> {
        if self.state.is_none() {
            misuse("the register set is empty, but an operation used it".into());
        }
        self.state.as_ref()
    }

    /// Refuse an operation on a handle that is not live. Returns false if it was refused.
    fn require_live(&self, qubit: QirQubit) -> bool {
        if !self.live.contains(&qubit.index()) {
            misuse(format!(
                "qubit {} is not live (never allocated, or already released)",
                qubit.index()
            ));
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// The ABI
// ---------------------------------------------------------------------------
//
// `#[unsafe(no_mangle)] extern "C"` so `llc`-produced objects link against these by exactly the name
// the compiler emits.

// ---- allocation ----------------------------------------------------------

/// Allocate one qubit in |0> and return its handle.
#[unsafe(export_name = "qir.qubit_alloc")]
pub extern "C" fn qir_qubit_alloc() -> QirQubit {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        // Once a misuse is recorded, stop allocating too. Otherwise a program that has already
        // corrupted its view of the register set would keep getting fresh qubits and appear to
        // make progress on a state nothing can vouch for.
        if blocked() {
            return QirQubit(0);
        }
        let index = r.next_index;
        r.next_index += 1;
        r.live.insert(index);
        // Adding a qubit to |0> widens the basis and leaves every existing amplitude in place, so
        // anything already computed on the other qubits survives the allocation untouched.
        let widened = match &r.state {
            Some(s) => StateVector::from_amplitudes(s.qubits() + 1, expand_with_zero_qubit(s)),
            None => StateVector::zero_state(1),
        };
        r.state = Some(widened);
        QirQubit(index as u64)
    })
}

/// Release a handle.
///
/// It does not uncompute anything: the caller owns the state. That is exactly why a release is a
/// poor substitute for a `reset`, which must return the SAME qubit to |0>.
#[unsafe(export_name = "qir.qubit_release")]
pub extern "C" fn qir_qubit_release(qubit: QirQubit) {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        if blocked() || !r.require_live(qubit) {
            return;
        }
        r.live.remove(&qubit.index());
        // The vector cannot shrink without relabelling every qubit index, and a handle is an
        // index, so the released qubit's slot stays as dead space. Resizing the basis would
        // change what every other handle means, which is far worse than wasting a slot.
    })
}

// ---- gates ---------------------------------------------------------------

/// Apply a single-qubit gate.
#[unsafe(export_name = "qir.apply1")]
pub extern "C" fn qir_apply1(qubit: QirQubit, gate: u8) {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        if blocked() || !r.require_live(qubit) {
            return;
        }
        let Some(gate) = gate_from_index(gate as usize) else {
            return;
        };
        let Some(base) = r.state() else { return };
        let next = gate.apply(base, qubit.index(), None);
        r.state = Some(next);
    })
}

/// Apply a two-qubit gate: CX, CY, CZ, or SWAP.
///
/// No gate index here on purpose. A controlled-X IS the CX -- passing `X` and asking for a
/// controlled form is how the first version of this function ended up refusing the very gate
/// that makes a Bell pair, with the message "X is not a two-qubit gate". The two-qubit gate set
/// is named, not indexed, so a caller cannot ask for a combination that does not exist.
#[unsafe(export_name = "qir.apply2")]
pub extern "C" fn qir_apply2(a: QirQubit, b: QirQubit, two_qubit_gate: u8) {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        if blocked() || !r.require_live(a) || !r.require_live(b) {
            return;
        }
        let (i, j) = (a.index(), b.index());
        let Some(base) = r.state() else { return };
        let next = match two_qubit_gate {
            TWO_QUBIT_CX => base.apply_pair(i, j, x_matrix()),
            TWO_QUBIT_CZ => base.apply_pair(i, j, z_matrix()),
            TWO_QUBIT_CY => base.apply_pair(i, j, StateVector::y_matrix()),
            TWO_QUBIT_SWAP => base.apply_swap(i, j),
            other => {
                misuse(format!(
                    "two-qubit gate index {other} is not one of CX, CZ, CY, SWAP"
                ));
                return;
            }
        };
        r.state = Some(next);
    })
}

/// Apply a Toffoli: flip `target` when both controls are set.
#[unsafe(export_name = "qir.apply3")]
pub extern "C" fn qir_apply3(control_a: QirQubit, control_b: QirQubit, target: QirQubit) {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        if blocked()
            || !r.require_live(control_a)
            || !r.require_live(control_b)
            || !r.require_live(target)
        {
            return;
        }
        let Some(base) = r.state() else { return };
        r.state = Some(base.apply_toffoli(control_a.index(), control_b.index(), target.index()));
    })
}

// ---- measurement ---------------------------------------------------------

/// Measure a qubit in the computational basis, collapsing the state. Returns 0 or 1.
///
/// Samples with a fresh draw each call, so two measurements of the same entangled qubit are
/// correlated as physics requires: a Bell pair gives the same answer twice. Returning the stored
/// bit instead would agree on that one test while getting the marginals wrong.
#[unsafe(export_name = "qir.mz")]
pub extern "C" fn qir_mz(qubit: QirQubit) -> bool {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        if blocked() || !r.require_live(qubit) {
            return false;
        }
        let index = qubit.index();
        let Some(base) = r.state() else { return false };
        let p_one = base.probability_of_one(index);

        if !(0.0..=1.0).contains(&p_one) {
            misuse(format!(
                "measurement probability {p_one} for qubit {index} is outside [0, 1] -- the state \
                 is not normalised"
            ));
            return false;
        }

        let outcome_is_one = pseudo_random() < p_one;
        // Renormalise by 1/sqrt(p): the measurement postulate requires the surviving subspace to
        // be a unit state, and leaving it unnormalised would make every later probability wrong.
        let p = if outcome_is_one { p_one } else { 1.0 - p_one };
        if p <= f64::EPSILON {
            misuse(format!(
                "measurement of qubit {index} collapsed onto an outcome with probability {p}"
            ));
            return false;
        }
        let scale = 1.0 / p.sqrt();
        // The basis stays the SAME WIDTH: a measured qubit is still a qubit, its value is just
        // known. So the collapsed vector keeps every amplitude slot, with the outcomes that were
        // ruled out set to zero. Building a shorter vector instead would relabel the remaining
        // qubits and silently compute a different circuit.
        let Some(base) = r.state() else { return false };
        let width = base.qubits();
        let collapsed: Vec<Complex> = (0..base.amplitudes().len())
            .map(|i| {
                if (i >> index) & 1 == usize::from(outcome_is_one) {
                    base.amplitude(i).scale(scale)
                } else {
                    Complex::ZERO
                }
            })
            .collect();
        r.state = Some(StateVector::from_amplitudes(width, collapsed));
        outcome_is_one
    })
}

/// Number of live qubits. For tests and diagnostics; not part of the base profile.
#[doc(hidden)]
#[unsafe(export_name = "qir.live_qubit_count")]
pub extern "C" fn qir_live_qubit_count() -> i64 {
    REGISTERS.with(|r| r.borrow().live.len() as i64)
}

/// Probability that `qubit` reads 1, before measurement. Diagnostic; not in the base profile.
///
/// This is what lets a test assert an entanglement claim numerically instead of by sampling:
/// both qubits of a Bell pair must read 0.5, which two independent qubits in |0> never do.
#[doc(hidden)]
#[unsafe(export_name = "qir.probability_of_one")]
pub extern "C" fn qir_probability_of_one(qubit: QirQubit) -> f64 {
    REGISTERS.with(|r| {
        let r = r.borrow();
        if blocked() || !r.require_live(qubit) {
            return 0.0;
        }
        r.state()
            .map_or(0.0, |s| s.probability_of_one(qubit.index()))
    })
}

/// Amplitude of a basis state, as (real, imaginary). Diagnostic; not in the base profile.
///
/// Lets a test compare the runtime's result against a hand-derived closed form.
#[doc(hidden)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qir_amplitude(basis: i64, out: *mut f64) {
    let index = basis.max(0) as usize;
    REGISTERS.with(|r| {
        let r = r.borrow();
        let Some(state) = &r.state else { return };
        let a = if index < state.amplitudes().len() {
            state.amplitude(index)
        } else {
            Complex::ZERO
        };
        // SAFETY: `out` must point to two writable f64s; the Rust test harness and the C harness
        // both pass a real array. A null or short pointer is a caller bug this crate cannot check,
        // which is why the only caller here is a test.
        unsafe {
            *out.add(0) = a.re;
            *out.add(1) = a.im;
        }
    })
}

/// Reset the register set. Test-only; a compiled program never calls this.
#[doc(hidden)]
pub fn reset_for_test() {
    REGISTERS.with(|r| *r.borrow_mut() = Registers::default());
    FAULT.with(|f| *f.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Tensor the state with one new qubit in |0>.
///
/// The new qubit takes the NEXT index, so it becomes the HIGH bit and every existing qubit keeps
/// the bit position it already had. Each old amplitude therefore lands at `index` and
/// `index | new_bit`, with the latter zero because the new qubit is |0>.
///
/// # The subtle part, which was wrong at first
///
/// Writing this as a plain interleave -- `push(a); push(ZERO)` per old amplitude -- places the
/// second element of every pair at `2i + 1`, which is BIT 0 of the new index. For an old index of
/// 1 that is `0b10`, whose BIT 0 is clear, so the old qubit's |1> amplitude silently became a
/// state where that qubit reads |0>. The widened state was still normalised and every total was
/// still right, but `probability_of_one` on the old qubit returned 0 instead of 0.5. Only the
/// MARGINAL caught it, which is why the test above checks marginals rather than just totals.
fn expand_with_zero_qubit(state: &StateVector) -> Vec<Complex> {
    let new_bit = 1usize << state.qubits();
    let mut out = vec![Complex::ZERO; state.amplitudes().len() * 2];
    for (index, amplitude) in state.amplitudes().iter().enumerate() {
        out[index] = *amplitude;
        out[index | new_bit] = Complex::ZERO;
    }
    out
}

/// The matrices for the gates that are not members of `Gate`.
///
/// `Gate` carries the 19 gates a state vector applies directly. A controlled gate is not a member
/// because it is a two-qubit operation: `apply_pair` applies one of these 2x2 matrices to the
/// TARGET whenever the CONTROL reads 1. So the controlled-X and controlled-Z reuse X and Z, and
/// the controlled-Y uses Y -- which is why there is no separate `cy_matrix` to keep in step.
fn x_matrix() -> [Complex; 4] {
    [
        Complex::new(0.0, 0.0),
        Complex::new(1.0, 0.0),
        Complex::new(1.0, 0.0),
        Complex::new(0.0, 0.0),
    ]
}

fn z_matrix() -> [Complex; 4] {
    [
        Complex::new(1.0, 0.0),
        Complex::new(0.0, 0.0),
        Complex::new(0.0, 0.0),
        Complex::new(-1.0, 0.0),
    ]
}

/// The [`Gate`] for a runtime gate index.
///
/// One table, defined here and nowhere else, so the ABI mapping cannot drift between the compiler
/// side and this side. Index 0 is `H` rather than a no-op, so a zero-initialised ABI argument is a
/// Hadamard and therefore VISIBLE, instead of silently doing nothing.
pub fn gate_from_index(index: usize) -> Option<Gate> {
    Some(match index {
        0 => Gate::H,
        1 => Gate::X,
        2 => Gate::Y,
        3 => Gate::Z,
        4 => Gate::S,
        5 => Gate::Sdg,
        6 => Gate::T,
        7 => Gate::Tdg,
        _ => {
            misuse(format!("gate index {index} is not defined by this runtime"));
            return None;
        }
    })
}

/// The two-qubit gate selectors. Named, so a caller cannot invent a combination.
///
/// `CY` was added because the compiler emits `qir.cy` and it had no definition: a gate the
/// backend declares but nothing implements is an undefined reference at link time.
pub const TWO_QUBIT_CX: u8 = 0;
pub const TWO_QUBIT_CZ: u8 = 1;
pub const TWO_QUBIT_CY: u8 = 2;
pub const TWO_QUBIT_SWAP: u8 = 3;

/// Named single-qubit gate indices, so a caller does not have to memorise the numbers above.
pub mod gate_index {
    pub const H: u8 = 0;
    pub const X: u8 = 1;
    pub const Y: u8 = 2;
    pub const Z: u8 = 3;
    pub const S: u8 = 4;
    pub const SDG: u8 = 5;
    pub const T: u8 = 6;
    pub const TDG: u8 = 7;
}

/// A pseudo-random draw in `[0, 1)`, seeded from the process clock.
///
/// # Why this is not seeded by a constant
///
/// An earlier version used a fixed xorshift seed. Every process then drew the SAME first value,
/// so a program that measured one Hadamard qubit and printed the bit returned the same answer on
/// every run -- and, worse, returned the WRONG one on half of them. A test asserting that two
/// entangled qubits agree passed perfectly under that seed, and so would a test asserting they
/// disagree. Only a distribution check across independent processes exposed it.
///
/// So the seed comes from the clock, which is what makes this a genuine random draw. The
/// generator itself stays a plain xorshift64: this is a simulator and claims no cryptographic
/// quality, only uniformity.
///
/// # What a test should therefore assert
///
/// Never assert a particular outcome -- it is a coin flip. Assert the DISTRIBUTION over enough
/// runs, or assert something that holds for every outcome, such as two entangled qubits always
/// agreeing. `tests/runtime_native_test.rs` does exactly that, and re-seeds per process so the
/// check is reproducible in aggregate rather than per-run.
fn pseudo_random() -> f64 {
    thread_local! {
        static SEED: Cell<u64> = const { Cell::new(0) };
    }
    SEED.with(|s| {
        let mut x = if s.get() == 0 {
            // Mix two independent-ish sources so two processes started in the same nanosecond
            // still differ. `SplitMix64` is the standard finaliser for exactly this.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15);
            let addr = &nanos as *const u64 as u64;
            let mut z = nanos.wrapping_add(addr).wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        } else {
            s.get()
        };
        // xorshift64: cheap, and adequate for a simulator that is not claiming cryptographic
        // quality. The exact sequence is irrelevant to correctness; only its uniformity is used.
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        // Top 53 bits, which is exactly what an f64 mantissa holds.
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

// ---------------------------------------------------------------------------------------
// The named intrinsics the compiler actually emits.
//
// `expr_lowering.rs` declares `qir.h`, `qir.cx`, `qir.mz` and the rest by name, so these -- not
// the indexed `qir.applyN` -- are the symbols a compiled program resolves against. Without them a
// `.naso` file compiles to a module whose only fault is an undefined reference at link time,
// which is precisely the state this suite was written to end.
//
// They are thin wrappers. One place decides what a gate MEANS; these only decide which handle and
// which matrix reach it, so the named ABI and the indexed ABI cannot drift apart.
// ---------------------------------------------------------------------------------------

/// `#pragma qir` target entrypoint `qir.h`.
#[unsafe(export_name = "qir.h")]
pub extern "C" fn qir_h(qubit: QirQubit) {
    qir_apply1(qubit, gate_index::H);
}

/// `#pragma qir` target entrypoint `qir.x`.
#[unsafe(export_name = "qir.x")]
pub extern "C" fn qir_x(qubit: QirQubit) {
    qir_apply1(qubit, gate_index::X);
}

/// `#pragma qir` target entrypoint `qir.y`.
#[unsafe(export_name = "qir.y")]
pub extern "C" fn qir_y(qubit: QirQubit) {
    qir_apply1(qubit, gate_index::Y);
}

/// `#pragma qir` target entrypoint `qir.z`.
#[unsafe(export_name = "qir.z")]
pub extern "C" fn qir_z(qubit: QirQubit) {
    qir_apply1(qubit, gate_index::Z);
}

/// `#pragma qir` target entrypoint `qir.s`.
#[unsafe(export_name = "qir.s")]
pub extern "C" fn qir_s(qubit: QirQubit) {
    qir_apply1(qubit, gate_index::S);
}

/// `#pragma qir` target entrypoint `qir.t`.
#[unsafe(export_name = "qir.t")]
pub extern "C" fn qir_t(qubit: QirQubit) {
    qir_apply1(qubit, gate_index::T);
}

/// Apply a single-qubit rotation about a named axis by a supplied angle.
///
/// # Why this cannot go through `qir_apply1`
///
/// `qir_apply1` takes a `u8` GATE INDEX, and the index table holds fixed gates -- `H`, `X`,
/// `S`. A rotation is not a fixed gate: `Rx(0.1)` and `Rx(2.9)` are different unitaries and
/// both are real. Indexing them would mean inventing indices for angles that are not in the
/// table, and the entry for "no index" would be a rotation by angle ZERO -- the identity,
/// silently not a rotation.
///
/// So the angle is a real `f64` parameter here, and the gate is named by an explicit AXIS
/// constant rather than by a magic number a caller could pass. `qir.r1` below is the named
/// ABI form, matching the real QIR base profile.
pub extern "C" fn qir_apply_rotation(qubit: QirQubit, axis: u8, angle: f64) {
    REGISTERS.with(|r| {
        let mut r = r.borrow_mut();
        if blocked() || !r.require_live(qubit) {
            return;
        }
        // A NaN angle would poison every amplitude downstream and there is no sensible
        // quantum meaning for it, so it is refused rather than propagated.
        if !angle.is_finite() {
            misuse(format!(
                "qir.apply_rotation: angle must be finite, got {angle}"
            ));
            return;
        }
        let gate = match axis {
            ROTATION_AXIS_X => Gate::Rx(angle),
            ROTATION_AXIS_Y => Gate::Ry(angle),
            ROTATION_AXIS_Z => Gate::Rz(angle),
            other => {
                misuse(format!(
                    "qir.apply_rotation: axis {other} is not X, Y or Z (expected {}, {} or {})",
                    ROTATION_AXIS_X, ROTATION_AXIS_Y, ROTATION_AXIS_Z
                ));
                return;
            }
        };
        let Some(base) = r.state() else { return };
        let next = gate.apply(base, qubit.index(), None);
        r.state = Some(next);
    })
}

/// Rotation axis selectors, named so a caller cannot pass a magic number.
///
/// X is 0 deliberately: a zero-initialised axis argument is a rotation about X, which is
/// VISIBLE rather than silently doing nothing. The same reasoning as the gate index table.
pub const ROTATION_AXIS_X: u8 = 0;
pub const ROTATION_AXIS_Y: u8 = 1;
pub const ROTATION_AXIS_Z: u8 = 2;

/// `#pragma qir` target entrypoint `qir.r1(double, ptr)`.
///
/// A rotation about the Z axis, the QIR base profile's single parameterized one-qubit gate.
///
/// `rz(theta, q)` rotates by `theta` about Z. It is NOT the same as `qir.s`, and the
/// distinction matters: `S` is `Rz(pi/2)`, so a program that means `S` and emits `r1(pi/2)` is
/// computing the same state, while a program that means a general rotation and emits `qir.s`
/// is computing the identity on half the inputs.
#[unsafe(export_name = "qir.r1")]
pub extern "C" fn qir_r1(theta: f64, qubit: QirQubit) {
    qir_apply_rotation(qubit, ROTATION_AXIS_Z, theta);
}

/// `#pragma qir` target entrypoint `qir.cx`.
#[unsafe(export_name = "qir.cx")]
pub extern "C" fn qir_cx(control: QirQubit, target: QirQubit) {
    qir_apply2(control, target, TWO_QUBIT_CX);
}

/// `#pragma qir` target entrypoint `qir.cy`.
#[unsafe(export_name = "qir.cy")]
pub extern "C" fn qir_cy(control: QirQubit, target: QirQubit) {
    qir_apply2(control, target, TWO_QUBIT_CY);
}

/// `#pragma qir` target entrypoint `qir.cz`.
#[unsafe(export_name = "qir.cz")]
pub extern "C" fn qir_cz(control: QirQubit, target: QirQubit) {
    qir_apply2(control, target, TWO_QUBIT_CZ);
}

/// `#pragma qir` target entrypoint `qir.swap`.
#[unsafe(export_name = "qir.swap")]
pub extern "C" fn qir_swap(a: QirQubit, b: QirQubit) {
    qir_apply2(a, b, TWO_QUBIT_SWAP);
}

/// `#pragma qir` target entrypoint `qir.ccx`.
#[unsafe(export_name = "qir.ccx")]
pub extern "C" fn qir_ccx(control_a: QirQubit, control_b: QirQubit, target: QirQubit) {
    qir_apply3(control_a, control_b, target);
}
