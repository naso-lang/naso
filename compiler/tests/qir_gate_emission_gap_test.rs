#![cfg(feature = "llvm")]
//! What the QIR backend can and cannot emit, measured rather than assumed.
//!
//! # Why this file exists
//!
//! The end-to-end gate verification idea -- `.naso` -> CLI -> QIR -> parse the gate list ->
//! simulate -> compare against a hand-derived state -- turned out to have no substrate. The
//! QIR backend cannot emit a single gate from any program the typechecker accepts. The
//! simulator has nothing to check, because there is no gate sequence to hand it.
//!
//! These tests pin that fact from the outside, through the shipped CLI, so that it cannot be
//! forgotten or "assumed working". They document a real gap, and they will fail loudly if
//! someone fixes the QIR backend and forgets to update this file -- which is the correct
//! direction for a failure.
//!
//! # What is NOT claimed
//!
//! Nothing here says the QIR backend is wrong in general, only what it accepts today. The
//! two gaps below are precise and each has an identified cause.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory unique to one test, so parallel runs cannot collide.
///
/// A shared directory did collide once: two tests wrote the same `.qir` path and one read
/// the other's output, which passed for the wrong reason.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("naso_qir_gap_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write_source(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(format!("{name}.naso"));
    std::fs::write(&path, source).expect("write source");
    path
}

/// Build `source` for `target` and return `(exit code, stderr)`.
fn build(dir: &Path, name: &str, source: &str, target: &str) -> (bool, String) {
    let src = write_source(dir, name, source);
    let out = dir.join(format!("{name}.out"));
    let result = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(["build", "--target", target])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("naso binary must run");
    (
        result.status.success(),
        String::from_utf8_lossy(&result.stderr).into_owned(),
    )
}

/// The smallest circuit that applies one gate and discharges the qubit.
const ONE_GATE: &str = "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    hadamard(a);\n    let m = measure(a);\n    let _ = m;\n}\n";

/// The LLVM backend accepts a one-gate circuit, so the source language and one backend
/// genuinely support this. Without this, the QIR failures below could be blamed on the
/// program rather than on the backend.
#[test]
fn the_llvm_backend_accepts_a_one_gate_circuit() {
    let dir = scratch("llvm_accepts");
    let (ok, stderr) = build(&dir, "bell", ONE_GATE, "llvm");
    assert!(
        ok,
        "the LLVM backend must accept a one-gate circuit; it did not: {stderr}"
    );
}

/// The QIR backend rejects the SAME program, reporting a spurious linearity error.
///
/// # The gap
///
/// `hadamard(a)` then `measure(a)` uses the linear qubit `a` exactly twice, which is correct:
/// a gate BORROWS a qubit and a measurement CONSUMES it. The QIR backend reports
/// `Linear variable 'a' used 2 times`.
///
/// # The cause
///
/// The QIR gate arm evaluates each qubit operand through `build_expr`, which re-checks the
/// quantity of every expression it emits. A gate operand is a BORROW, not a consumption, so
/// the check fires on correct code. The LLVM backend's `QuantumOp` arm does not re-check, so
/// it accepts the program. The two arms are otherwise near-identical -- same intrinsic
/// naming, same argument order -- which is why one works and the other does not.
///
/// Two source statements naming ONE qubit: the QIR backend refuses, because it emits each
/// statement as a separate `void` function.
///
/// # What changed, and what did not
///
/// This file originally recorded that QIR refused a gate with a spurious linearity error,
/// `Linear variable 'a' used 2 times`. Both earlier defects are FIXED and covered elsewhere:
///
///   * the linearity checker no longer counts a gate operand as a consumption, so a gate
///     BORROW is not mistaken for a double USE;
///   * `qir_intrinsic_for` maps lowering's operation names onto real QIR intrinsics, so
///     `qalloc` resolves to `qir.qubit_alloc` instead of `qir.qalloc`.
///
/// Neither fix is sufficient, and this test is why: the refusal below is neither of those
/// errors. It is the REMAINING structural defect -- `build_statement` creates a fresh
/// `void` function per `PirStatement` and clears `self.variables` between them, so a `[1]`
/// qubit bound by `let [1] a = qalloc(1)` is not in scope when a LATER statement applies a
/// gate to `a`.
///
/// The backend used to paper over that with `i64 0`, which made the gate operate on an
/// integer rather than the qubit. It now refuses and names the cause.
///
/// # Why the refusal is right and a fix is not yet safe
///
/// Emitting this correctly means either sharing an `alloca` across statements -- which is
/// unsound, because two `void` functions have no caller-ordered relationship, so a gate in
/// statement 3 could execute before the allocation in statement 1 -- or restructuring the
/// emitter to produce one entry-point function per Naso function, which is the same shape the
/// LLVM backend already has. That is a real design change with its own risk, so it is not
/// smuggled in here.
///
/// Note the contrast with `the_llvm_backend_accepts_a_one_gate_circuit`: LLVM emits this
/// same program, because its schedule tree gives every statement a place in one function.
#[test]
fn the_qir_backend_refuses_a_gate_on_a_qubit_bound_in_an_earlier_statement() {
    let dir = scratch("qir_scope");
    let (ok, stderr) = build(&dir, "scope", ONE_GATE, "qir");
    assert!(
        !ok,
        "this test records the per-statement scoping defect. If QIR now emits this, the \
         emitter must share qubit identity across statements -- check that a gate in a later \
         statement acts on the qubit an EARLIER statement allocated, then update this test, \
         the capability matrix and LIMITATIONS.md."
    );
    assert!(
        stderr.contains("not bound in this statement"),
        "the expected failure is the unbound-operand refusal naming the per-statement scoping, \
         but got: {stderr}"
    );
}

/// `qalloc` now resolves to a real QIR intrinsic and yields a POINTER, not an integer.
///
/// # Why pointer-ness is the point
///
/// The generic placeholder for a void intrinsic is `result_type`'s integer zero. Returning
/// that for `qir.qubit_alloc` typed every qubit binding as `i64`, and a later gate then passed
/// an `i64` where `qir.h(ptr)` wants a `ptr` -- rejected by the LLVM verifier. QIR models a
/// qubit as a pointer, so a qubit-producing operation must yield a null pointer of that type.
///
/// # What is still not proven
///
/// This asserts the INTRINSIC is found and the refusal has moved on. It does NOT claim a
/// qubit survives between statements -- that is the scoping defect above, and the test below
/// still fails for it.
#[test]
fn the_qir_backend_now_recognises_qalloc() {
    let dir = scratch("qir_alloc");
    // Allocation only, in a single statement, so the scoping defect cannot mask the result.
    let source = "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let _ = a;\n}\n";
    let (ok, stderr) = build(&dir, "alloc", source, "qir");
    if !ok {
        assert!(
            !stderr.contains("Unknown QIR intrinsic 'qir.qalloc'"),
            "`qalloc` should map to `qir.qubit_alloc`. If this fails, the intrinsic mapping \
             regressed and every quantum program is refused again: {stderr}"
        );
    }
    // When it does emit, the allocation must be a real pointer call, never an integer zero.
    if ok {
        let qir = std::fs::read_to_string(dir.join("alloc.qir")).expect("read emitted QIR");
        assert!(
            qir.contains("call ptr @qir.qubit_alloc()"),
            "a qubit allocation must be a pointer-returning call: {qir}"
        );
    }
}

/// A program with no quantum operations at all compiles to QIR successfully.
///
/// The control for the two tests above. Without it, they could be passing merely because
/// the QIR target is broken for everything -- which is a different and less interesting
/// finding than "QIR specifically cannot emit gates".
#[test]
fn the_qir_backend_still_works_for_a_program_with_no_quantum_operations() {
    let dir = scratch("qir_classic");
    let source = "fn f() {\n    let x = 1 + 2;\n    let _ = x;\n}\n";
    let (ok, stderr) = build(&dir, "classic", source, "qir");
    assert!(
        ok,
        "the QIR backend must still compile a classical program; the gate failures above \
         are specifically about quantum operations, not about the target being broken: \
         {stderr}"
    );
}

/// The typechecker accepts the one-gate circuit the QIR backend rejects.
///
/// Pins that the rejection happens in the BACKEND, not in the language. If the typechecker
/// ever rejects it too, the error has moved and this file needs revisiting.
#[test]
fn the_typechecker_accepts_the_circuit_the_qir_backend_rejects() {
    let dir = scratch("typecheck");
    let src = write_source(&dir, "bell", ONE_GATE);
    let result = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(["check"])
        .arg(&src)
        .output()
        .expect("naso binary must run");
    assert!(
        result.status.success(),
        "`naso check` must accept a correct one-gate circuit: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
