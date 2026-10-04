//! `entangle` must be REFUSED by the LLVM backend, not emitted as a dangling call.
//!
//! # The defect this file exists for
//!
//! `naso check` accepted `entangle`, and `naso build --target llvm` accepted it too, emitting
//! an unresolved declaration:
//!
//! ```text
//! call void @qir.entangle(i1 %a1, i1 %b2, i1 %c3)
//! declare void @qir.entangle(i1, i1, i1)
//! ```
//!
//! Nothing defines `qir.entangle`. `llc` is perfectly happy -- it emits an object file -- and
//! the link then fails:
//!
//! ```text
//! /usr/bin/ld: undefined reference to `qir_entang`
//! ```
//!
//! So the pipeline reported success for a module that cannot be linked. That is the class of
//! defect this repo has been systematically closing: exit 0 while delivering something that
//! does not work.
//!
//! # Why this one cannot simply be modelled
//!
//! `entangle(a, b, c)` takes an arbitrary number of qubits, and the QIR base profile declares no
//! intrinsic for it. Approximating it by a CNOT over the first two qubits would compute
//! something DIFFERENT from what the source says -- the third qubit would be left unentangled --
//! and that difference is invisible in the emitted text. Refusing is the only honest outcome.
//!
//! Note the QIR backend already refused this correctly. The LLVM backend had no allowlist at
//! all: it formatted `qir.{op}` for ANY operation name, so a lowerer that invented a spelling
//! would get a plausible-looking module with no diagnostic.
//!
//! # What is NOT claimed here
//!
//! This does not establish that native quantum execution works. The whole LLVM quantum path
//! emits declarations with no definitions -- `qir.H`, `qir.CX`, `qir.measure` included -- so
//! every quantum program is unlinkable today, `entangle` included. This test pins the refusal so
//! the gap is visible rather than silent; it is not a step toward a QIR runtime, and no runtime
//! has executed any of this.

#![cfg(feature = "llvm")]

use std::process::Command;

use naso_compiler::codegen::llvm::module_builder::LLVMModuleBuilder;
use naso_compiler::codegen::{CodegenContext, CodegenTarget, OptLevel};

/// Run the shipped CLI and return `(success, combined output)`.
fn naso(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(args)
        .output()
        .expect("naso binary runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// A per-TEST scratch directory.
///
/// One directory shared by every test here is wrong: `cargo test` runs tests in parallel, so a
/// shared directory that is wiped on entry deletes another test's sources while it runs. The
/// symptom is a file that "does not exist" in a test that created it moments earlier, which reads
/// like a compiler bug and is not one. Each test gets its own, named after it.
fn scratch(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("naso_entangle_{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write(dir: &std::path::Path, name: &str, src: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, src).expect("write source");
    p
}

/// Two qubits and a valid `entangle`, which is the program the rest of this file is about.
const TWO_QUBITS: &str = "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    let e = entangle(a, b);\n    let _ = e;\n}\n";

/// A well-typed `entangle` is REFUSED by the LLVM backend, with a diagnostic naming it.
///
/// Not `Emits`. The refusal is what makes the missing intrinsic visible instead of shipping a
/// module that fails at link time.
#[test]
fn llvm_refuses_entangle_rather_than_emitting_a_dangling_call() {
    let dir = scratch("two");
    let src = write(&dir, "two.naso", TWO_QUBITS);
    let out = dir.join("two.ll");

    let (ok, text) = naso(&[
        "build",
        "--target",
        "llvm",
        src.to_str().expect("path"),
        "-o",
        out.to_str().expect("path"),
    ]);

    assert!(
        !ok,
        "the LLVM backend accepted `entangle`; it emits `qir.entangle`, which nothing defines, \
         so the module cannot link:\n{text}"
    );
    assert!(
        text.contains("entangle"),
        "the refusal must name the construct it refused:\n{text}"
    );
    assert!(
        !out.exists(),
        "no LLVM module should be written for a refused program"
    );
}

/// The refusal survives every qubit count.
///
/// `entangle` is variadic, so a guard written for one arity is not a guard. Probed from zero to
/// four qubits: a zero- or one-qubit call asserts nothing and is refused by the typechecker
/// before codegen, and the rest are refused by the backend.
#[test]
fn entangle_is_refused_at_every_arity() {
    let dir = scratch("arity");
    for n in 0..=4usize {
        let decls: String = (0..n)
            .map(|i| format!("    let [1] q{i}: Qubit = qalloc(1);\n"))
            .collect();
        let args: Vec<String> = (0..n).map(|i| format!("q{i}")).collect();
        let src = format!(
            "fn f() {{\n{decls}    let e = entangle({});\n    let _ = e;\n}}\n",
            args.join(", ")
        );
        let path = write(&dir, &format!("arity{n}.naso"), &src);
        let out = dir.join(format!("arity{n}.ll"));

        let (ok, text) = naso(&[
            "build",
            "--target",
            "llvm",
            path.to_str().expect("path"),
            "-o",
            out.to_str().expect("path"),
        ]);
        assert!(!ok, "entangle over {n} qubits was accepted:\n{text}");
        assert!(
            !out.exists(),
            "no module should be written for a refused {n}-qubit entangle"
        );
    }
}

/// Zero- and one-qubit `entangle` are refused by the TYPECHECKER, before any backend sees them.
///
/// The arity is the meaning: `entangle` infers a `QRegister` whose dimension is the number of
/// qubits, so `entangle()` inferred a zero-dimensional register and typechecked as `OK` while
/// asserting nothing at all. That is a type error, not a degenerate case, and it has to be
/// caught where the type is known -- no backend should be reachable with it.
#[test]
fn entangle_needs_at_least_two_qubits_and_the_typechecker_says_so() {
    let dir = scratch("typecheck");
    for (n, decls) in [
        (0usize, String::new()),
        (1, "    let [1] q0: Qubit = qalloc(1);\n".to_string()),
    ] {
        let args: Vec<String> = (0..n).map(|i| format!("q{i}")).collect();
        let src = format!(
            "fn f() {{\n{decls}    let e = entangle({});\n    let _ = e;\n}}\n",
            args.join(", ")
        );
        let path = write(&dir, &format!("tc{n}.naso"), &src);

        let (ok, text) = naso(&["check", path.to_str().expect("path")]);
        assert!(
            !ok,
            "a {n}-qubit `entangle` typechecked OK; it infers a {n}-dimensional register while \
             asserting nothing:\n{text}"
        );
        assert!(
            text.contains("argument count mismatch"),
            "the diagnostic should be an arity error, got:\n{text}"
        );
    }
}

/// The QIR backend already refused `entangle`. This is an anti-drift guard on THAT.
///
/// The LLVM backend had no allowlist and so had nothing to refuse against. If a future change
/// adds one to LLVM, this still pins QIR. If it is deleted here, the next person to touch the
/// QIR intrinsic table has no test telling them `entangle` was deliberately excluded.
#[test]
fn qir_still_refuses_entangle() {
    let dir = scratch("qir");
    let src = write(&dir, "qir.naso", TWO_QUBITS);
    let out = dir.join("qir.out");

    let (ok, text) = naso(&[
        "build",
        "--target",
        "qir",
        src.to_str().expect("path"),
        "-o",
        out.to_str().expect("path"),
    ]);
    assert!(!ok, "the QIR backend accepted `entangle`:\n{text}");
    assert!(
        text.contains("entangle"),
        "the QIR refusal must name the construct:\n{text}"
    );
}

/// The refusal is specific to `entangle`: a CNOT over the same two qubits still emits.
///
/// A guard written as "refuse anything variadic" or "refuse any two-qubit op" would pass every
/// assertion above while quietly breaking ordinary gates. This is the control.
#[test]
fn a_cnot_over_the_same_two_qubits_still_emits() {
    let dir = scratch("cnot");
    let src = write(
        &dir,
        "cx.naso",
        "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    cnot(a, b);\n    let m = measure(a);\n    let n = measure(b);\n    let _ = m;\n    let _ = n;\n}\n",
    );
    let out = dir.join("cx.ll");

    let (ok, text) = naso(&[
        "build",
        "--target",
        "llvm",
        src.to_str().expect("path"),
        "-o",
        out.to_str().expect("path"),
    ]);
    assert!(ok, "a CNOT must still emit:\n{text}");
    let ir = std::fs::read_to_string(&out).expect("read emitted IR");
    assert!(
        ir.contains("qir.cx"),
        "the CNOT should appear in the emitted module under its QIR name:\n{ir}"
    );
    assert!(
        !ir.contains("entangle"),
        "nothing in this program should produce an entangle call:\n{ir}"
    );
}

/// Every gate name either producer can emit is accepted, in BOTH spellings.
///
/// This is an anti-drift guard written after the allowlist was wrong once. `GateKind`'s `Display`
/// emits `H` and `CX`; the structural fixture parser in `codegen_tests.rs` emits `h` and `cx` for
/// the same gates. An allowlist carrying only the first silently broke five fixture suites with
/// "`h` has no LLVM quantum intrinsic". The gate set is shared, so a name missing from either
/// spelling is a real refusal rather than a cosmetic mismatch.
///
/// Every gate name either producer can emit is accepted, in BOTH spellings.
///
/// This is an anti-drift guard written after the allowlist was wrong twice. `GateKind`'s `Display`
/// emits `H` and `CX`; the structural fixture parser in `codegen_tests.rs` emits `h` and `cx` for
/// the same gates. A table carrying only one set silently broke five fixture suites with
/// "`h` has no LLVM quantum intrinsic". The gate set is shared, so a name missing from either
/// spelling is a real refusal rather than a cosmetic mismatch.
///
/// The names are written out as literals rather than read from `GateKind`, because `Literal` is
/// not publicly re-exported so a rotation cannot be constructed here, and because a test that
/// re-derives its expectations from the table it is checking proves nothing about names it never
/// looks at. A name this test has never heard of is exactly the one that would be refused.
///
/// These have no source spelling, so they are checked against the mapping directly rather than by
/// compiling a program. `hadamard` and `cnot` ARE source spellings -- the lowerer maps them to
/// `H` and `CX` -- and are covered end to end by `gates_with_a_source_spelling_still_compile`.
#[test]
fn every_gate_name_both_producers_can_emit_is_accepted() {
    // As `GateKind::Display` spells it -- what the real lowerer emits.
    for name in ["H", "X", "Y", "Z", "S", "T", "CX", "CY", "CZ"] {
        assert!(
            backend_accepts(name),
            "`{name}` is what `GateKind::Display` produces, so the backend must accept it"
        );
    }
    // As `tests/codegen_tests.rs::gate_name` spells it -- also covered by `codegen_tests.rs`, and
    // pinned here by name so removing one from the mapping shows up as this failure.
    for name in [
        "h", "x", "y", "z", "s", "t", "cx", "cy", "cz", "ccx", "swap", "mz",
    ] {
        assert!(
            backend_accepts(name),
            "`{name}` is what the fixture parser produces, so the backend must accept it"
        );
    }
}

/// Rotations are refused, because a QIR rotation takes an ANGLE the lowering does not supply.
///
/// `qir.rz(double, ptr)` takes an angle; this lowering emits the bare name `RZ` with no angle
/// operand. Emitting the call anyway would either fail to type-check or -- worse -- declare a
/// zero-argument function and compute a rotation by an angle of zero, which is the identity and
/// silently not a rotation. Refused, which is the honest outcome until the angle is plumbed
/// through.
///
/// # What is actually true about rotations, which the earlier version of this comment got wrong
///
/// This test previously claimed `RX`/`RY`/`RZ` "reach the quantum arm as a bare name". They do
/// not. They cannot, and the distinction matters because it changes what would have to happen
/// to lift the refusal:
///
/// - The lexer has exactly four quantum keywords: `entangle`, `hadamard`, `reset`, `cnot`.
/// - The parser builds `QuantumOp::ApplyGate` only for `GateKind::H`, `GateKind::CX` and
///   `GateKind::Reset`.
/// - `GateKind::RX/RY/RZ` are therefore only ever MATCHED -- in `Display`, in `gate_arity`, and
///   in `naso-verify`'s transition table -- and never CONSTRUCTED anywhere in the repository.
///
/// So writing `rz(0.5, a)` does not reach any rotation check at all: `rz` is not a keyword, so it
/// lexes as an ordinary identifier and is reported as an undefined variable. The rotation refusal
/// is real and worth keeping -- a future keyword would hit it -- but it is guarding a path that no
/// source program can currently take.
///
/// That is why the assertion below is on the NAME TABLE rather than on a compiled program: there
/// is no source program that can reach a rotation, so a source-level test could only ever assert
/// that `rz` is an undefined variable, which is a fact about the lexer and says nothing about
/// rotations.
///
#[test]
fn rotations_are_refused_until_their_angle_is_supplied() {
    for name in ["RX", "RY", "RZ", "rx", "ry", "rz"] {
        assert!(
            !backend_accepts(name),
            "`{name}` needs an angle argument this lowering does not pass, so it must be refused \
             rather than emitted as a zero-angle rotation"
        );
    }
}

/// No rotation can be written in source, so the rotation refusal guards an unreachable path.
///
/// # The companion to `rotations_are_refused_until_their_angle_is_supplied`
///
/// That test asserts the backend table refuses `RX`/`RY`/`RZ`. This one asserts WHY that
/// refusal is currently unreachable from source, and it exists so the two facts cannot drift:
/// the moment a rotation keyword is added, this fails and points at the angle plumbing that
/// then has to work.
///
/// # What is asserted, and why each part
///
/// 1. The lexer has no rotation keyword -- read from the live source, so a new keyword shows
///    up here instead of silently making this test wrong.
/// 2. The parser constructs `ApplyGate` only for `H`, `CX` and `Reset` -- likewise read from
///    the live source, so a new `ApplyGate(GateKind::RZ, ..)` is caught.
/// 3. A program that writes `rz(0.5, a)` is refused as an UNDEFINED VARIABLE, not as a
///    rotation -- proving the claim end to end rather than by inspection.
///
/// Point 3 is the load-bearing one. The other two are structural greps that could both be
/// satisfied while some other path still built a rotation; compiling an actual program closes
/// that.
#[test]
fn rotations_cannot_be_written_in_source() {
    let parser = include_str!("../src/parser/expr.rs");
    let lexer = include_str!("../src/lexer/token.rs");

    // 1. No rotation keyword in the lexer's token enum.
    for kw in ["Hadamard", "CNot", "Reset", "Entangle"] {
        assert!(
            lexer.contains(&format!("{kw},")),
            "the four quantum keywords are expected in the token enum; if a fifth -- a \
             rotation -- has been added, this test must be revisited, because the rotation \
             refusal would then be reachable and its angle plumbing would have to work"
        );
    }
    for kw in ["Rx", "Rz", "Ry"] {
        assert!(
            !lexer.contains(&format!("{kw},")),
            "`{kw}` now appears to be a lexer token. A rotation is therefore constructible, \
             so the angle must be plumbed through before the backend admits it -- otherwise a \
             rotation would be emitted with an angle of zero, which is the identity."
        );
    }

    // 2. The parser builds ApplyGate only for H, CX and Reset.
    for gate in ["GateKind::H", "GateKind::CX", "GateKind::Reset"] {
        assert!(
            parser.contains(&format!("ApplyGate({gate},")),
            "expected the parser to still construct {gate}"
        );
    }
    for gate in ["GateKind::RX", "GateKind::RY", "GateKind::RZ"] {
        assert!(
            !parser.contains(&format!("ApplyGate({gate},")),
            "the parser now constructs {gate}, so a rotation IS reachable from source. The \
             angle is discarded by `lower_quantum_op` (which sets `args: vec![]`), so it would \
             be emitted as a zero-angle rotation -- the identity, silently not a rotation. \
             Plumb the angle through before admitting it in the backend table."
        );
    }

    // 3. End to end: writing `rz(theta, q)` is an undefined variable, not a rotation.
    let src = "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    rz(0.5, a);\n}\n";
    let Ok(mut parsed) = naso_compiler::parser::parse_program(src) else {
        panic!(
            "a program naming an undefined identifier must still PARSE -- it is a name lookup \
             failure, not a syntax error"
        );
    };
    let checked = naso_compiler::typecheck::check_program(&mut parsed);
    assert!(
        !checked.errors.is_empty(),
        "`rz` is not a keyword, so it lexes as an ordinary identifier and must not typecheck \
         as a rotation. An empty error list means a rotation became constructible without the \
         lexer or parser changing, which would defeat the refusal entirely."
    );
    let all = checked
        .errors
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        all.contains("rz"),
        "the diagnostic must name `rz`, so an author can tell an unknown gate from a real one. \
         Got: {all}"
    );
}

/// `reset` and `entangle` stay refused, even though both are reachable from source.
///
/// `reset` is the subtler of the two: it is a real `GateKind` variant and it mutates a qubit in
/// place, so a plausible-looking substitute exists. A release is not it -- a release hands the
/// qubit away, whereas `reset` returns the SAME qubit to |0> and leaves the binding usable.
#[test]
fn reset_and_entangle_stay_refused() {
    for name in ["reset", "entangle"] {
        assert!(
            !backend_accepts(name),
            "`{name}` has no QIR intrinsic and must stay refused"
        );
    }
}

/// Whether this backend's quantum allowlist admits `op`.
///
/// Direct rather than through a source program, because most of these names have NO source
/// spelling: `GateKind`'s `Display` emits `H` and `CX` internally and no `.naso` file can say
/// them. Testing coverage through source programs therefore skips exactly the spellings most at
/// risk of drifting, which is how the allowlist lost the fixture spellings once already.
fn backend_accepts(op: &str) -> bool {
    naso_compiler::codegen::llvm::expr_lowering::quantum_intrinsic_for(op).is_some()
}

/// A gate spelled the way source spells it still compiles end to end.
///
/// The allowlist check above is about the name table; this is the control that the real pipeline
/// still works, so a guard written as "refuse everything" cannot pass.
#[test]
fn gates_with_a_source_spelling_still_compile() {
    for (spelling, src) in [
        (
            "hadamard",
            "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    hadamard(a);\n    let m = measure(a);\n    let _ = m;\n}\n",
        ),
        (
            "cnot",
            "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    cnot(a, b);\n    let m = measure(a);\n    let n = measure(b);\n    let _ = m;\n    let _ = n;\n}\n",
        ),
    ] {
        let Ok(parsed) = naso_compiler::parser::parse_program(src) else {
            panic!("{spelling} fixture should parse");
        };
        let pir = naso_compiler::lowering::lower_program(&parsed)
            .unwrap_or_else(|e| panic!("{spelling} should lower: {e}"));
        let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("context");
        let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");
        builder
            .build_module(&pir)
            .unwrap_or_else(|e| panic!("{spelling} should emit for LLVM: {e}"));
    }
}
