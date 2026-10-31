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

/// A rotation IS now accepted, and its angle is what makes it a rotation rather than the
/// identity.
///
/// # This test used to assert the opposite, and the change is the point
///
/// `RZ` was refused because `qir.r1(double, ptr)` needs an angle and the lowering supplied
/// none -- so emitting it would have computed a rotation by an angle of zero, which is the
/// identity and silently not a rotation. Three things had to be true before it could be
/// lifted, and all three are now real:
///
/// 1. `qir.r1` is DECLARED in `QIR_INTRINSICS`, with a `Double` first parameter. Without the
///    declaration the emitted call named a symbol nothing defined.
/// 2. The lowering carries the angle in `PirExpr::QuantumOp::args` instead of discarding it
///    with `args: vec![]`.
/// 3. The RUNTIME exports `qir.r1`, and it applies the angle rather than treating the call as
///    a phase gate with the angle ignored.
///
/// Only `RZ` is admitted. `RX` and `RY` are real gates in the simulator and are still
/// refused, because the QIR base profile has no `qir.rx`/`qir.ry` and admitting them would
/// mean declaring entry points this runtime does not export. `RZ` is the base profile's one
/// parameterized single-qubit gate, so it is the one that can be emitted honestly.
#[test]
fn a_rotation_is_now_accepted_because_its_angle_reaches_the_intrinsic() {
    assert!(
        backend_accepts("RZ"),
        "`RZ` maps to `qir.r1`, whose angle now reaches the intrinsic as an `f64` operand, so \\
         it must be accepted"
    );
    // The lowercase spelling too: the structural fixtures emit lowercase gate names, and a
    // table carrying only one spelling silently broke five fixture suites when it was wrong.
    assert!(
        backend_accepts("rz"),
        "the lowercase spelling must be accepted as well -- two producers spell these gates \\
         differently and one shared gate set is the invariant"
    );
}

/// `RX` and `RY` stay refused, and the reason is an ABI fact rather than a gap.
///
/// They are real gates in the simulator, so this is NOT "not implemented" -- it is "no entry
/// point". The QIR base profile defines `r1` for rotations about Z and nothing for rotations
/// about X or Y, so admitting them would mean declaring `qir.rx`/`qir.ry`, which this runtime
/// does not export. That would compile and then fail to LINK, which is the failure this suite
/// exists to prevent.
///
/// So one rotation that works beats three that lie, and the refusal names the missing entry
/// point rather than the missing implementation.
#[test]
fn rx_and_ry_stay_refused_because_the_base_profile_has_no_entry_point_for_them() {
    for name in ["RX", "RY", "rx", "ry"] {
        assert!(
            !backend_accepts(name),
            "`{name}` has no QIR base-profile entry point, so admitting it would declare a \\
             symbol the runtime does not export -- compiling to a module that fails at link \\
             time"
        );
    }
}

/// The angle must be a REQUIRED argument, because a rotation with no angle is the identity.
///
/// This is the sharpest edge of the new syntax. `rz(q)` would parse as a rotation and compute
/// nothing at all, which is the exact failure the old refusal existed to prevent -- just moved
/// from the backend to the parser. The arity is therefore checked at parse time, with a
/// diagnostic that says why the angle cannot be defaulted.
#[test]
fn a_rotation_without_an_angle_is_a_parse_error_not_a_no_op() {
    let src = "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    rz(a);\n}\n";
    let err = naso_compiler::parser::parse_program(src)
        .expect_err("`rz(a)` omits the angle and must not parse");
    let msg = err.to_string();
    assert!(
        msg.contains("rz") && msg.contains("2"),
        "the diagnostic must name `rz` and the required argument count: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("angle"),
        "and must say an angle is required rather than merely that the count is wrong, because \\
         a zero angle would be the identity: {msg}"
    );
}

/// `rz` is a keyword now, so it must no longer resolve as an ordinary identifier.
///
/// This REPLACES `rotations_cannot_be_written_in_source`, which asserted the opposite: that
/// `rz` was an undefined variable. That was true, and it is now false.
///
/// It was replaced rather than deleted because the fact it pinned -- whether a rotation is
/// constructible from source at all -- is still the one that matters. Only the answer changed.
/// Deleting it would have left no record that `rz` was ever unreachable, so a future reader
/// would have no way to tell that adding the keyword is what made the angle plumbable.
#[test]
fn rz_is_now_a_keyword_and_not_an_identifier() {
    use naso_compiler::lexer::{Lexer, TokenKind};

    // The token exists -- this is what makes the rotation reachable from source.
    //
    // The qubit is MEASURED at the end, because `[1]` is linear: a program that allocates one
    // and never consumes it is an error the typechecker is right to report. That is not
    // incidental -- a rotation BORROWS its qubit exactly as `hadamard` does, which is the
    // property being confirmed here, so the fixture has to spend the value the way a real
    // program would.
    let src = "fn f() -> i64 {\n    let [1] a: Qubit = qalloc(1);\n    rz(0.5, a);\n    \
               if measure(a) { return 1; }\n    return 0;\n}\n";
    let Ok(parsed) = naso_compiler::parser::parse_program(src) else {
        panic!("`rz(0.5, a)` must parse now that `rz` is a keyword")
    };

    // And it is a ROTATION, not a call to an undefined function: a program that used `rz` as
    // a variable name would now fail, which is the accepted cost of adding a keyword.
    let mut parsed = parsed;
    let checked = naso_compiler::typecheck::check_program(&mut parsed);
    assert!(
        checked.errors.is_empty(),
        "`rz(0.5, a)` must TYPECHECK as a rotation: the angle is a float and `a` is a qubit. \
         Errors: {:?}",
        checked.errors
    );

    // The lexer really has the token, so this is not passing by accident.
    let kinds = Lexer::tokenize(src);
    assert!(
        kinds.contains(&TokenKind::Rz),
        "`rz` must lex to the Rz token; if it does not, the rotation is unreachable and the \
         assertion above passed for some other reason"
    );
}

/// `rx` and `ry` are still NOT keywords, so `rx(0.5, a)` is an undefined variable.
///
/// The deliberate counterpart to `rz_is_now_a_keyword_and_not_an_identifier`. Exposing only
/// the rotation that has an entry point means the other two must not be constructible at all,
/// so a program writing `rx(0.5, a)` gets a clear name error rather than a rotation that fails
/// at link time.
#[test]
fn rx_and_ry_are_not_keywords() {
    for name in ["rx", "ry"] {
        let src =
            format!("fn f() {{\n    let [1] a: Qubit = qalloc(1);\n    {name}(0.5, a);\n}}\n");
        let Ok(mut parsed) = naso_compiler::parser::parse_program(&src) else {
            panic!("`{name}` is not a keyword, so this must still PARSE as an identifier call")
        };
        let checked = naso_compiler::typecheck::check_program(&mut parsed);
        assert!(
            !checked.errors.is_empty(),
            "`{name}` has no base-profile entry point, so it must not typecheck as a rotation"
        );
    }
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
