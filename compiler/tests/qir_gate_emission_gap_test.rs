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

/// A Bell pair, split across statements so each operand's alloca is separate:
///
///     let [1] a = qalloc(1);
///     let [1] b = qalloc(1);
///     hadamard(a);
///     cnot(a, b);
///
/// The circuit is only correct if the `h` and the `cx`'s control resolve to the SAME qubit,
/// which is what makes this the discriminating case for operand identity.
const BELL_PAIR: &str = "fn bell() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    hadamard(a);\n    cnot(a, b);\n    let m = measure(a);\n    let n = measure(b);\n    let _ = m;\n    let _ = n;\n}\n";

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
/// A circuit whose qubits are bound in EARLIER statements now compiles to a correct QIR
/// circuit.
///
/// # What this asserts, and why it is the identity that matters
///
/// The source is a Bell pair: allocate two qubits, `hadamard(a)`, `cnot(a, b)`. The emitted
/// text is checked for OPERAND IDENTITY -- that the `h` and the first `cx` argument are loads
/// of the SAME alloca `%a`, and that `%b` is a different one. A presence check
/// (`qir.contains("call void @qir.h(")`) would be satisfied by a circuit applying `h` and `cx`
/// to unrelated qubits, which is exactly what this backend used to emit.
///
/// # Why it could not work before
///
/// `build_statement` emitted one `void` function per PIR statement, so a binding made by one
/// statement was invisible to the next -- the gate read an unbound name and got the integer 0.
/// Three changes fixed that, and all three are needed:
///
///   * statements are emitted into ONE `qir_entry` function in order, so the program's order
///     IS the emitted order and nothing is an unreferenced function;
///   * a statement-position `let` outlives its own placeholder body, via `in_statement_position`
///     -- set by the caller, never inferred from the body's shape, because a genuine body of
///     `0` is indistinguishable from the placeholder;
///   * a qubit-producing operation yields a null POINTER, not the generic integer zero.
///
/// # What this does NOT claim
///
/// QIR text is not execution. This circuit has never been run against a QPU or a simulator,
/// and `qir_entry` carries no ENTRYPOINT attribute, so no QIR driver would find it either.
/// What is established is that the emitted module is a well-formed, correctly-ordered circuit
/// on the intended qubits -- which is what makes numerical verification possible next.
#[test]
fn a_bell_pair_compiles_to_one_ordered_circuit_on_the_right_qubits() {
    let dir = scratch("qir_bell");
    let (ok, stderr) = build(&dir, "bell", BELL_PAIR, "qir");
    assert!(
        ok,
        "a Bell pair should compile to QIR now. It did not, so the per-statement scoping \
         defect is back or something else refuses it: {stderr}"
    );

    let qir = std::fs::read_to_string(dir.join("bell.out")).expect("read emitted QIR");

    // ONE function, so the order of the statements is the order of the circuit.
    assert_eq!(
        qir.matches("define void @qir_entry").count(),
        1,
        "statements must be emitted into a single entry function: {qir}"
    );
    assert!(
        !qir.contains("qir_stmt_"),
        "no per-statement functions should remain -- nothing would order them: {qir}"
    );

    // Exactly two allocations for two qubits. More means an operand that was not reused.
    assert_eq!(
        qir.matches("call ptr @qir.qubit_alloc()").count(),
        2,
        "two qubits should mean two allocations: {qir}"
    );

    // The identity check: the gate operands are loads of the same two allocas, and the
    // `cx` takes a-load then b-load, so it controls `a` onto `b` as the source says.
    let (h_operand, cx_control, cx_target) = operands_of_gates(&qir);
    assert_eq!(
        h_operand, cx_control,
        "`hadamard(a)` and `cnot(a, b)` must act on the SAME first qubit; got h on {h_operand} \
         and cx controlled on {cx_control}"
    );
    assert_ne!(
        h_operand, cx_target,
        "`cnot(a, b)` must target the SECOND qubit, not the one the h acted on; both were \
         {cx_target}"
    );

    // And the program order must be the emitted order: allocate, h, cx, measure.
    let i_alloc = qir.find("call ptr @qir.qubit_alloc()").expect("allocation");
    let i_h = qir.find("call void @qir.h(").expect("h");
    let i_cx = qir.find("call void @qir.cx(").expect("cx");
    assert!(
        i_alloc < i_h && i_h < i_cx,
        "the circuit must read allocate -> h -> cx; offsets were {i_alloc}, {i_h}, {i_cx}: {qir}"
    );
}

/// The SSA names the `h` and the two `cx` operands were given.
///
/// Both operand sites are `load ptr, ptr %a` -- the SAME alloca, loaded twice -- which is
/// what makes the identity check meaningful. Comparing the load RESULTS (`%a2` vs `%a3`)
/// would be wrong: those are distinct SSA names precisely because they are two separate
/// loads, and they would compare unequal even in a correct circuit. So this strips the
/// `load ..., ptr ` prefix and returns the alloca each operand came from.
fn operands_of_gates(qir: &str) -> (String, String, String) {
    let call_args = |needle: &str| -> Vec<String> {
        let start = qir.find(needle).expect("gate call present");
        let line = qir[start..].lines().next().expect("call occupies one line");
        let args = line
            .rsplit_once('(')
            .expect("a call has an argument list")
            .1;
        args.trim_end_matches(')')
            .split(',')
            .map(|a| a.trim().to_string())
            .collect()
    };

    // The CALL line names the load's RESULT (`%a3`), which is a distinct SSA name on every
    // use -- so it cannot be compared. What identifies the qubit is the alloca that load
    // READS, which is on the preceding line: `%a3 = load ptr, ptr %a, align 8` -> `%a`.
    //
    // So walk back from the call to the instruction that defined its operand. Reporting the
    // slot name rather than the value is what makes "same qubit" a decidable question.
    let pointee = |arg: &str| -> String {
        let value = arg.trim_start_matches("ptr ").trim();
        let def = qir
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{value} =")))
            .unwrap_or_else(|| panic!("no defining instruction for operand {arg:?}"));
        let after = def
            .split_once(", ptr ")
            .unwrap_or_else(|| panic!("operand {value} is not a pointer load: {def}"));
        after
            .1
            .split(',')
            .next()
            .expect("the loaded-from slot")
            .trim()
            .to_owned()
    };

    let h = call_args("call void @qir.h(");
    let cx = call_args("call void @qir.cx(");
    assert_eq!(h.len(), 1, "hadamard takes one operand, got {h:?}");
    assert_eq!(cx.len(), 2, "cnot takes two operands, got {cx:?}");
    (pointee(&h[0]), pointee(&cx[0]), pointee(&cx[1]))
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
