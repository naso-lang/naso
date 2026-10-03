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
/// # Why this is pinned rather than fixed here
///
/// Fixing it means teaching the QIR emitter that a gate operand is borrowed, which is a
/// change to how quantities are tracked in emission. That is real work with a real risk of
/// weakening the check that catches genuine double-consumption, and it belongs in its own
/// change with its own tests. Until then the honest state is recorded here.
#[test]
fn the_qir_backend_rejects_a_gate_the_typechecker_and_llvm_accept() {
    let dir = scratch("qir_gate");
    let (ok, stderr) = build(&dir, "bell", ONE_GATE, "qir");
    assert!(
        !ok,
        "this test records that the QIR backend cannot emit a gate. If it now can, \
         FIX IT and update the capability matrix and docs/content/LIMITATIONS.md -- \
         the end-to-end gate verification blocked on this can now be built."
    );
    assert!(
        stderr.contains("Linear variable 'a' used 2 times"),
        "the expected failure is a spurious linearity error on the gate operand, \
         but got: {stderr}"
    );
}

/// The QIR backend does not recognise `qalloc` as an intrinsic, so a circuit cannot even be
/// allocated.
///
/// This is a SECOND, independent gap: even with the borrow problem fixed, `qalloc` lowers to
/// a `PirExpr::QuantumOp` whose name has no entry in the QIR intrinsic table, and the backend
/// refuses it.
///
/// LLVM does not hit this because its `QuantumOp` arm declares whatever intrinsic the name
/// implies, on first use. QIR has a fixed declared set and refuses anything outside it.
#[test]
fn the_qir_backend_does_not_know_how_to_allocate_a_qubit() {
    let dir = scratch("qir_alloc");
    // No gate at all -- just allocate and discharge, isolating allocation from borrowing.
    let source =
        "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let m = measure(a);\n    let _ = m;\n}\n";
    let (ok, stderr) = build(&dir, "alloc", source, "qir");
    assert!(
        !ok,
        "this test records that QIR cannot allocate a qubit. If it now can, update the \
         capability matrix and LIMITATIONS.md."
    );
    assert!(
        stderr.contains("Unknown QIR intrinsic 'qir.qalloc'"),
        "the expected failure is the unknown-intrinsic refusal, but got: {stderr}"
    );
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
