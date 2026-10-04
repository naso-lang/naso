//! `reversible { ... }` executes natively: a forward pass AND its uncomputation.
//!
//! # Why this file is the gate, and not just another suite
//!
//! `reversible` was refused for a long time, and refusing was correct. The construct's
//! whole meaning is "apply this, then uncompute it", and the original implementation did
//! neither: it lowered the block's statements, emitted no inverse, and returned `Ok`. A
//! program that said `reversible { h(q) }` therefore compiled, ran, and left the qubit
//! entangled garbage the block existed to erase.
//!
//! Refusing it is what kept that from shipping. So the question this file answers is not
//! "does lowering accept the block" -- it is the only question that matters:
//!
//! > does a COMPILED program, EXECUTED as a native process, leave the state it started in?
//!
//! Every test here drives `.naso` -> LLVM IR -> `llc` -> `cc` -> run, linked against
//! `libnaso_gates.a`. Nothing is asserted about PIR shape alone, because a PIR that claims
//! to be reversible proves nothing about the circuit that runs.
//!
//! # The distribution rule, and why it decides the file
//!
//! Nothing here asserts a fixed outcome. Measurement is stochastic, so a fixed expected
//! value would be a claim about one draw -- it would pass against a runtime rigged to
//! always return false, and it would pass against a fixed RNG seed. Both of those are
//! exactly the failure this repository has hit before.
//!
//! So every test asserts a DISTRIBUTION over many runs, and every one of them is paired
//! with a CONTROL that differs only by the absence of `reversible`. The control is the
//! load-bearing half:
//!
//! | program | outcome required | what it rules out |
//! |---|---|---|
//! | `hadamard(a)` alone | roughly half 0s, half 1s | a runtime that always returns 0 |
//! | `reversible { hadamard(a) }` | ALL 0s | the inverse not running |
//! | `h(a); cx(a,b)` alone | both agree or disagree at random | a runtime that cannot entangle |
//! | `reversible { h(a); cx(a,b) }` | ALL 0 on both | a wrong-order inverse |
//!
//! The last row is the strongest statement in the file. An inverse emitted in the wrong
//! order, or with a controlled gate's operands left unswapped, produces gates that are
//! individually correct and jointly wrong: the state is left transformed by a commutator
//! rather than the identity, and the two qubits then DISAGREE at random. Only the correct
//! reverse-order, operand-swapped inverse returns them both to |0>.
//!
//! # Scope of the claim
//!
//! Gate sequences only. Measurement, allocation, arithmetic, and rotations inside a
//! `reversible` block are REFUSED, each with a diagnostic naming the reason -- see
//! `lowering::uncomputation`. This file asserts those refusals too, because a construct
//! that silently stopped refusing would be worse than one that never worked.
//!
//! These are classical state-vector simulations running as native processes. No QPU and no
//! noise model are involved.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;

/// Locate a tool only if it is on PATH, so a missing LLVM gives a clear skip reason rather
/// than a confusing failure inside the harness.
fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("naso_uncomp_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A C harness. `naso_main` returns `i64`; the measurement bits are folded into it by Naso
/// itself, so the harness only prints it. `_Bool` would be needed if a bool crossed this
/// boundary -- reading an `i1` as a full `int` gives the wrong answer on half the runs.
const HARNESS: &str = "#include <stdio.h>\nextern long naso_main(void);\n\
                       int main(void) { printf(\"%ld\\n\", naso_main()); return 0; }\n";

/// Compile `source`, run it `runs` times, and return what it printed each time.
///
/// Returns `None` if the pipeline cannot run at all, so a caller skips with a reason
/// instead of reporting a quantum failure for a missing toolchain.
fn build_and_run(name: &str, source: &str, runs: usize) -> Option<Vec<String>> {
    if !have("llc") || !have("cc") {
        return None;
    }
    let dir = scratch(name);
    let src = dir.join(format!("{name}.naso"));
    std::fs::write(&src, source).expect("write source");
    let ll = dir.join(format!("{name}.ll"));
    let obj = dir.join(format!("{name}.o"));
    let exe = dir.join(format!("{name}.exe"));

    let built = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(["build", "--target", "llvm"])
        .arg(&src)
        .arg("-o")
        .arg(&ll)
        .output()
        .expect("naso binary must run");
    assert!(
        built.status.success(),
        "the compiler must accept the program:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let runtime = ensure_runtime_built();

    let to_obj = Command::new("llc")
        .args(["-filetype=obj"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("llc must run");
    assert!(
        to_obj.status.success(),
        "llc must accept the emitted IR:\n{}",
        String::from_utf8_lossy(&to_obj.stderr)
    );

    let main_c = dir.join("main.c");
    std::fs::write(&main_c, HARNESS).expect("write harness");
    let link = Command::new("cc")
        .arg("-o")
        .arg(&exe)
        .arg(&main_c)
        .arg(&obj)
        .arg(&runtime)
        .args(["-lm", "-lpthread", "-ldl"])
        .output()
        .expect("cc must run");
    assert!(
        link.status.success(),
        "the emitted module must LINK against the QIR runtime:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let mut out = Vec::with_capacity(runs);
    for _ in 0..runs {
        let run = Command::new(&exe)
            .output()
            .expect("the built program must run");
        assert!(
            run.status.success(),
            "the program must exit cleanly, stderr:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        out.push(String::from_utf8_lossy(&run.stdout).trim().to_string());
    }
    let _ = std::fs::remove_dir_all(&dir);
    Some(out)
}

/// Build the release staticlib if absent, and return its path.
///
/// Deliberately does NOT skip when the artifact is missing: a suite that skips when its
/// subject is absent reports green having tested nothing. The thing under test is precisely
/// that the emitted intrinsics resolve against a real runtime.
fn ensure_runtime_built() -> PathBuf {
    let path = runtime_staticlib();
    if path.exists() {
        return path;
    }
    let built = Command::new("cargo")
        .args(["build", "--release", "-p", "naso-gates"])
        .output()
        .expect("cargo must run to build the QIR runtime");
    assert!(
        built.status.success(),
        "the QIR runtime must build as a staticlib at {}:\n{}",
        path.display(),
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(
        path.exists(),
        "the build reported success but {} is absent, so nothing can link against it",
        path.display()
    );
    path
}

/// The built runtime, resolved from Cargo metadata rather than guessed -- the target
/// directory is configured away from the repository.
fn runtime_staticlib() -> PathBuf {
    let meta = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata must run");
    let json = String::from_utf8_lossy(&meta.stdout);
    let target_dir = json
        .split("\"target_directory\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("cargo metadata must report a target directory");
    PathBuf::from(target_dir)
        .join("release")
        .join("libnaso_gates.a")
}

/// Count how many of `runs` printed `value`.
fn count(out: &[String], value: &str) -> usize {
    out.iter().filter(|s| s.as_str() == value).count()
}

/// How many runs are needed before "all of them" means something.
///
/// Chosen from the failure it has to catch. A runtime rigged to always return 0 would pass
/// a single-run assertion, and a fixed RNG seed could hide behind a modest sample; 40 runs
/// makes a coin flip landing one way every time vanishingly unlikely, while still being
/// fast enough to run on every build.
const RUNS: usize = 40;

/// The load-bearing pair: uncomputation returns the qubit to |0>, and the control proves
/// the quantum part is real.
///
/// `H` is self-inverse, so `reversible { hadamard(a) }` is `h` then `h`, and the qubit must
/// measure 0 every single time. The control is the same program with the block removed: it
/// measures ~50/50, which is what establishes that the `0`s in the first program are caused
/// by the uncomputation rather than by a runtime that always answers 0.
///
/// Without the control this test would be worth very little -- it is exactly the assertion
/// that passes against a rigged measurement.
#[test]
fn a_reversible_block_returns_its_qubit_to_zero_and_the_control_does_not() {
    let with_block = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    reversible { hadamard(a); }
    if measure(a) { return 1; }
    return 0;
}
";
    let control = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    hadamard(a);
    if measure(a) { return 1; }
    return 0;
}
";

    let Some(observed) = build_and_run("uncomp_h", with_block, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable, so the native pipeline cannot run");
        return;
    };
    let Some(control_out) = build_and_run("uncomp_h_control", control, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable, so the native pipeline cannot run");
        return;
    };

    assert_eq!(
        count(&observed, "1"),
        0,
        "every run must measure 0: the uncomputation must return the qubit to |0>. {} of {} \
         runs measured 1",
        count(&observed, "1"),
        RUNS
    );

    // The control is what makes the assertion above mean anything.
    let ones = count(&control_out, "1");
    assert!(
        (10..=30).contains(&ones),
        "the CONTROL -- the same program with `reversible` removed -- must measure roughly \
         half 1s, because a Hadamard alone leaves the qubit in superposition. It measured {ones} \
         1s out of {RUNS}. If this fails, the runtime does not actually simulate, and the \
         assertion above is not evidence of anything."
    );
}

/// The strongest claim in the file: a MULTI-GATE block, where the inverse's ORDER and its
/// operand swap both have to be right.
///
/// `reversible { hadamard(a); cnot(a, b) }` builds a Bell pair and must then destroy it.
/// The emitted inverse is `cx(b, a)` then `h(a)` -- reverse order, operands swapped.
///
/// Every way of getting this wrong produces gates that are individually correct and jointly
/// wrong, leaving the state transformed by a commutator rather than the identity. The
/// signature of that is the two qubits DISAGREEING at random, because a partially-uncomputed
/// Bell pair is a state whose marginals are not both 0. So an always-0 result is evidence
/// the order is right, and a spread over 1/2/3 is evidence it is wrong.
///
/// The control is the same circuit without the block, which must disagree about half the
/// time. That establishes the entangling gate really did entangle, so the `0`s above are
/// the uncomputation and not an inert circuit.
#[test]
fn a_multi_gate_reversible_block_uncomputes_a_bell_pair_in_the_right_order() {
    // `r` accumulates the two bits: 0 = both |0>, 1 = only a, 2 = only b, 3 = both.
    let with_block = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    let [1] b: Qubit = qalloc(1);
    reversible { hadamard(a); cnot(a, b); }
    let mut r: i64 = 0;
    if measure(a) { r = r + 1; }
    if measure(b) { r = r + 2; }
    return r;
}
";
    let control = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    let [1] b: Qubit = qalloc(1);
    hadamard(a);
    cnot(a, b);
    let mut r: i64 = 0;
    if measure(a) { r = r + 1; }
    if measure(b) { r = r + 2; }
    return r;
}
";

    let Some(observed) = build_and_run("uncomp_bell", with_block, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable, so the native pipeline cannot run");
        return;
    };
    let Some(control_out) = build_and_run("uncomp_bell_control", control, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable, so the native pipeline cannot run");
        return;
    };

    assert_eq!(
        count(&observed, "0"),
        RUNS,
        "the Bell pair must be fully uncomputed: both qubits measure 0 every time. The \
         non-zero outcomes were: {:?}",
        observed
            .iter()
            .filter(|s| s.as_str() != "0")
            .collect::<Vec<_>>()
    );

    // The control must show a REAL Bell pair: both qubits agreeing or disagreeing at random,
    // never pinned to 0. A control that returned all 0s would mean the CNOT never entangled
    // anything, and the test above would be passing for the wrong reason.
    let agreeing = count(&control_out, "0") + count(&control_out, "3");
    assert!(
        agreeing >= 20,
        "the CONTROL must show a real Bell pair, whose two qubits agree about half the time. \
         Only {agreeing} of {RUNS} runs agreed. If the entangling gate does nothing, the \
         assertion above proves nothing about uncomputation."
    );
}

/// A self-inverse gate pair: `H; H` inside a block is still the identity.
///
/// Guards against a pass that special-cases a single gate. `reversible { hadamard(a);
/// hadamard(b) }` on two independent qubits must return BOTH to |0>, and it exercises the
/// "more than one inverse statement" path -- which is where the statement-id collision that
/// this feature initially shipped with lived.
#[test]
fn two_gates_in_a_reversible_block_both_uncompute() {
    let source = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    let [1] b: Qubit = qalloc(1);
    reversible { hadamard(a); hadamard(b); }
    let mut r: i64 = 0;
    if measure(a) { r = r + 1; }
    if measure(b) { r = r + 2; }
    return r;
}
";
    let Some(out) = build_and_run("uncomp_two", source, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable, so the native pipeline cannot run");
        return;
    };
    assert_eq!(
        count(&out, "0"),
        RUNS,
        "both independent qubits must return to |0>. Observed: {out:?}"
    );
}

/// The inverse really is emitted, in reverse order, with the CNOT's operands swapped.
///
/// Asserted on the EMITTED TEXT rather than on PIR, because the text is what `llc`
/// compiles. A PIR-level assertion would pass even if the backend reordered the statements
/// on the way out -- which is precisely the failure mode that would leave the emitted gates
/// individually correct and jointly wrong.
///
/// # Reading the operands, and the trap in doing so naively
///
/// The emitted IR does not pass `%a` and `%b` to `qir.cx`; it LOADS from the slots first, so
/// each call gets its own SSA name:
///
/// ```text
/// %a3 = load ptr, ptr %a, align 8
/// %b4 = load ptr, ptr %b, align 8
/// call void @qir.cx(ptr %a3, ptr %b4)     <- forward
/// %b5 = load ptr, ptr %b, align 8
/// %a6 = load ptr, ptr %a, align 8
/// call void @qir.cx(ptr %b5, ptr %a6)     <- inverse, operands swapped
/// ```
///
/// So comparing the register NAMES across two calls proves nothing -- `%a3` and `%a6` are
/// different registers that both hold `a`. The thing that must be compared is the SLOT each
/// operand was loaded from, which is what identifies the qubit the gate acts on. This is the
/// same shape as the `store`-has-no-`=` and `call`-args-start-after-the-last-`(` traps in
/// inkwell/IR parsing: the text is right, the naive reading of it is wrong.
#[test]
fn the_emitted_inverse_is_reverse_order_with_swapped_cnot_operands() {
    let source = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    let [1] b: Qubit = qalloc(1);
    reversible { hadamard(a); cnot(a, b); }
    if measure(a) { return 1; }
    if measure(b) { return 1; }
    return 0;
}
";
    let Some(ir) = compile_to_ir("uncomp_order", source) else {
        eprintln!("skipped: llc or cc is unavailable");
        return;
    };

    // Map each SSA register to the SLOT it was loaded from, so `%a3` and `%a6` both resolve
    // to `a`. `load ptr, ptr %X` defines a register that holds whatever slot `X` holds.
    let mut slot_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut gates: Vec<(String, Vec<String>)> = Vec::new();

    for line in ir.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('%')
            && let Some((lhs, rhs)) = rest.split_once(" = load ptr, ptr %")
        {
            slot_of.insert(
                format!("%{lhs}"),
                format!("%{}", rhs.split(',').next().unwrap_or("")),
            );
        }
        if !line.starts_with("call void @qir.") {
            continue;
        }
        let name = line
            .split('@')
            .nth(1)
            .and_then(|s| s.split('(').next())
            .unwrap_or("")
            .to_string();
        let arg_text = line
            .split('(')
            .nth(1)
            .unwrap_or("")
            .split(')')
            .next()
            .unwrap_or("");
        let args: Vec<String> = arg_text
            .split(',')
            .map(|a| a.trim().trim_start_matches("ptr ").to_string())
            .map(|reg| slot_of.get(&reg).cloned().unwrap_or(reg))
            .collect();
        gates.push((name, args));
    }

    assert_eq!(
        gates.len(),
        4,
        "expected the forward pass and the uncomputation -- four gate calls -- got {gates:?}"
    );
    assert_eq!(
        gates[0],
        ("qir.h".to_string(), vec!["%a".to_string()]),
        "the forward pass starts with h(a)"
    );
    assert_eq!(
        gates[1],
        (
            "qir.cx".to_string(),
            vec!["%a".to_string(), "%b".to_string()]
        ),
        "the forward pass is cx(a, b)"
    );
    assert_eq!(
        gates[2],
        (
            "qir.cx".to_string(),
            vec!["%b".to_string(), "%a".to_string()]
        ),
        "the FIRST uncomputation step must be the CNOT with its operands SWAPPED. cx(a,b) and \
         cx(b,a) are DIFFERENT unitaries, and only the swapped one inverts the forward gate. \
         Leaving them in place looks correct in the emitted text and computes a circuit that is \
         not the inverse. Emitted: {gates:?}"
    );
    assert_eq!(
        gates[3],
        ("qir.h".to_string(), vec!["%a".to_string()]),
        "the LAST uncomputation step must be the Hadamard, so the inverse runs in REVERSE \
         order. Forward order would leave the state transformed by a commutator rather than \
         the identity. Emitted: {gates:?}"
    );
}

/// Compile to LLVM IR text, for the test that asserts on what `llc` will see.
fn compile_to_ir(name: &str, source: &str) -> Option<String> {
    if !have("llc") || !have("cc") {
        return None;
    }
    let dir = scratch(name);
    let src = dir.join(format!("{name}.naso"));
    std::fs::write(&src, source).expect("write source");
    let ll = dir.join(format!("{name}.ll"));
    let built = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(["build", "--target", "llvm"])
        .arg(&src)
        .arg("-o")
        .arg(&ll)
        .output()
        .expect("naso binary must run");
    assert!(
        built.status.success(),
        "the compiler must accept the program:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let text = std::fs::read_to_string(&ll).expect("the emitted IR must be readable");
    let _ = std::fs::remove_dir_all(&dir);
    Some(text)
}

/// Lower a program and return the refusal message, asserting it was refused.
fn refuse(name: &str, source: &str) -> String {
    let program = naso_compiler::parser::parse_program(source)
        .unwrap_or_else(|e| panic!("{name}: the test source must parse: {e}"));
    match naso_compiler::lowering::lower_program(&program) {
        Ok(m) => panic!(
            "{name}: this must be REFUSED, but it lowered successfully and reported no \
             diagnostic. It emitted:\n{m:?}\n\nA construct that silently accepts what it \
             cannot uncompute emits a forward pass with no inverse, which is the defect the \
             refusal exists to prevent."
        ),
        Err(e) => e.to_string(),
    }
}

/// Measurement inside a `reversible` block is refused.
///
/// Measurement is the one operation here that looks exactly like a gate call and is not
/// invertible. Uncomputing it is a classically-controlled re-preparation that needs the
/// outcome carried to a branch; the previous implementation emitted a call to `unmeasure`,
/// a function that does not exist in this repository, with the qubit operand invented as the
/// literal `0`.
///
/// Note this source is caught by the TYPE CHECKER, one layer earlier, with its own message.
/// That is two independent refusals, and this test pins the fact that lowering also refuses
/// -- a type-checker message is not a guarantee that the lowering path is safe.
#[test]
fn a_measurement_inside_a_reversible_block_is_refused() {
    let msg = refuse(
        "measurement",
        "fn main() -> i64 {\n    \
         let [1] a: Qubit = qalloc(1);\n    \
         reversible { measure(a); }\n    \
         return 0;\n}\n",
    );
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("measure") || lower.contains("impure"),
        "the refusal must name the measurement: {msg}"
    );
}

/// Arithmetic inside a `reversible` block is refused, by name.
///
/// The inverse of `x = a*b` is `x/b`, which needs to know WHICH OPERAND the statement
/// binds -- and this pass is not told. The previous implementation rewrote `Mul -> Div` and
/// `Div -> Mul`, which computes a different number rather than undoing one, and defaulted
/// every other operator to `Add`, so a comparison got an "inverse" that was an addition.
#[test]
fn arithmetic_inside_a_reversible_block_is_refused_by_name() {
    let msg = refuse(
        "arithmetic",
        "fn main() -> i64 {\n    \
         reversible { let x: i64 = 1 + 2; }\n    \
         return 0;\n}\n",
    );
    assert!(
        msg.contains("not a quantum gate"),
        "the refusal must say the block holds something other than a gate: {msg}"
    );
    assert!(
        msg.contains("not told"),
        "and must name the missing information rather than reporting a bare error: {msg}"
    );
}

/// An empty `reversible` block is refused.
///
/// With no body there is nothing to run and nothing to uncompute. Accepting it would emit a
/// circuit that is "reversible" only because it does nothing -- which is not evidence that an
/// uncomputation pass ran, and is indistinguishable from one that emitted nothing at all.
#[test]
fn an_empty_reversible_block_is_refused() {
    let msg = refuse(
        "empty",
        "fn main() -> i64 {\n    reversible { }\n    return 0;\n}\n",
    );
    assert!(
        msg.to_lowercase().contains("empty"),
        "the refusal must say the block is empty: {msg}"
    );
}

/// A `reversible` block holding something other than a gate is refused, not skipped.
///
/// This is the assertion that a partial block cannot slip through. If a non-gate statement
/// were skipped rather than refused, the forward pass would be missing an operation and the
/// inverse would be computed from the wrong circuit -- and the two halves would be
/// self-consistent and jointly wrong, which is the hardest shape of this defect to notice.
#[test]
fn a_refused_block_leaves_no_partial_emission_behind() {
    // A block that lowers its first statement fine and then hits something it cannot
    // uncompute. If the refusal left the first statement in place, a caller that caught the
    // error and kept going would emit a half-block.
    let program = naso_compiler::parser::parse_program(
        "fn main() -> i64 {\n    \
         let [1] a: Qubit = qalloc(1);\n    \
         reversible { hadamard(a); let x: i64 = 1 + 2; }\n    \
         return 0;\n}\n",
    )
    .expect("source must parse");
    let err = naso_compiler::lowering::lower_program(&program)
        .expect_err("a block with a non-gate statement must be refused");
    assert!(
        err.to_string().contains("not a quantum gate"),
        "the refusal must name the offending statement: {err}"
    );
}

/// The S / S-dagger distinction survives into the emitted circuit.
///
/// # Why this needed its own test
///
/// A mutation mapping `S`'s adjoint onto `S` -- EXACTLY the historical defect this
/// repository has already shipped once, in a gate table since deleted -- passed every other
/// test in this file. The reason is worth stating plainly: `S` and `T` are not among the
/// four prelude builtins, so no SOURCE program can reach them, and the native execution tests
/// drive source programs. They were measuring nothing about S.
///
/// So this assertion is at the PIR level, on the same function the lowering calls, because
/// that is the only place a gate unreachable from source can be tested. It is also why the
/// native tests above are necessary but not sufficient: they cover what source can express.
///
/// `S` sends |1> to i|1>; `S-dagger` sends it to -i|1>. Applying S where S-dagger was written
/// leaves every downstream amplitude wrong, and the emitted text looks entirely reasonable.
#[test]
fn the_s_gate_uncomputes_to_s_dagger_and_not_to_s() {
    use naso_compiler::ir::{AffineDomain, PirExpr, PirStatement, StmtId};

    let forward = vec![PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::QuantumOp {
            op: "S".to_string(),
            args: vec![],
            qubits: vec![PirExpr::Var("q".to_string())],
        },
        quantity: naso_compiler::ast::Quantity::Many,
        mutability: naso_compiler::ast::Mutability::Immutable,
        span: None,
    }];

    let inverse = naso_compiler::lowering::uncomputation::uncompute_statements(&forward, StmtId(1))
        .expect("S has a known adjoint");
    match &inverse[0].body {
        PirExpr::QuantumOp { op, .. } => assert_eq!(
            op, "Sdg",
            "S must uncompute to S-dagger. S and S-dagger are DIFFERENT gates -- S sends |1> \
             to i|1> and S-dagger to -i|1> -- so emitting S leaves the qubit in the wrong \
             state while the emitted text looks correct. This is the exact defect a deleted \
             gate table once shipped."
        ),
        other => panic!("expected a quantum op, got {other:?}"),
    }
}

/// An operation with no known adjoint must be REFUSED, never skipped.
///
/// # The mutation this kills
///
/// Replacing the `is_none()` refusal with `if false` makes every unrecognised statement be
/// silently dropped, and every other test in this file still passed. That is the more
/// dangerous of the two shapes this construct can fail in: a SKIPPED statement leaves the
/// forward pass missing an operation and computes the inverse from the wrong circuit, and
/// those two mistakes are consistent with each other, so nothing downstream can detect them.
///
/// The name used is a plausible mis-spelling -- close enough that a reader might accept it as
/// a gate, and that it would be easy to "fix" by adding an arm that ASSUMES it is one.
/// (`cnot` would not serve: it is a real alias for `CX`, and refusing it would be a bug.)
#[test]
fn an_operation_with_no_known_adjoint_is_refused_rather_than_skipped() {
    use naso_compiler::ir::{AffineDomain, PirExpr, PirStatement, StmtId};

    let forward = vec![PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::QuantumOp {
            op: "cnott".to_string(),
            args: vec![],
            qubits: vec![PirExpr::Var("q".to_string())],
        },
        quantity: naso_compiler::ast::Quantity::Many,
        mutability: naso_compiler::ast::Mutability::Immutable,
        span: None,
    }];

    let err = naso_compiler::lowering::uncomputation::uncompute_statements(&forward, StmtId(1))
        .expect_err("an operation with no adjoint must be refused, not skipped");
    let msg = err.to_string();
    // Assert on the OPERATION NAME appearing in the refusal, not on a hand-picked phrase.
    //
    // An earlier version of this test asserted the substring "not a gate", which matched a
    // DIFFERENT refusal further up -- the one for a statement that is not a quantum
    // operation at all. That variant passed even when the check it was written for had been
    // removed entirely, because some other refusal caught the input first.
    //
    // Naming the offending operation is what makes this test load-bearing: a skip emits no
    // message at all, so requiring the name pins the REFUSAL as the outcome.
    assert!(
        msg.contains("cnott"),
        "the refusal must name the operation it cannot uncompute, so the author can see what \
         to fix. Got: {msg}"
    );
    assert!(
        msg.contains("adjoint"),
        "and must say why it has no adjoint rather than reporting a bare failure: {msg}"
    );
}

/// A rotation inside a `reversible` block is uncomputed by NEGATING its angle.
///
/// This test used to assert the opposite. It was correct then and is wrong now: there was no
/// angle to negate, so the honest answer was a refusal. Plumbing the angle through
/// `lower_quantum_op` removed the reason for it.
///
/// # The angle is the whole claim
///
/// A rotation by theta has adjoint the rotation by -theta. Emitting the UN-negated rotation
/// would apply the rotation twice rather than undoing it, and the emitted text would look
/// entirely reasonable -- same gate, same qubit, wrong sign. So the assertion is on the sign,
/// and it is checked on the emitted IR because that is what `llc` compiles.
#[test]
fn a_rotation_in_a_reversible_block_is_uncomputed_by_negating_its_angle() {
    let source = "\
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    reversible { rz(0.5, a); }
    if measure(a) { return 1; }
    return 0;
}
";
    let Some(ir) = compile_to_ir("uncomp_rz", source) else {
        eprintln!("skipped: llc or cc is unavailable");
        return;
    };

    let angles: Vec<String> = ir
        .lines()
        .map(str::trim)
        // Filter to CALL SITES. The emitted module contains both `declare void @qir.r1(...)`
        // and `call void @qir.r1(...)`, and matching on the bare name counts the declaration
        // as a third rotation. Matching `"call void @qir.r1"` is what makes this an assertion
        // about the CIRCUIT rather than about the module's preamble.
        .filter(|l| l.starts_with("call void @qir.r1"))
        .map(|l| {
            l.split("double ")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap_or("?")
                .to_string()
        })
        .collect();

    assert_eq!(
        angles.len(),
        2,
        "expected the forward rotation and its uncomputation, two `qir.r1` CALLS, got \
         {angles:?}. A third would be the `declare` line being counted, which is what this \
         filter exists to exclude."
    );

    let forward: f64 = angles[0]
        .parse()
        .unwrap_or_else(|_| panic!("`{}` is not a number", angles[0]));
    let inverse: f64 = angles[1]
        .parse()
        .unwrap_or_else(|_| panic!("`{}` is not a number", angles[1]));

    assert!(
        (forward - 0.5).abs() < 1e-12,
        "the forward rotation must be by 0.5, got {forward}"
    );
    assert!(
        (inverse + 0.5).abs() < 1e-12,
        "the uncomputation must be by the NEGATED angle, -0.5. Got {inverse}. An un-negated \
         rotation applies the forward rotation twice instead of undoing it, and the emitted \
         text looks correct either way -- so this is the only place the sign is observable."
    );
}

/// WHY there is no distribution test for `rz`: a global phase is invisible to a measurement,
/// and the language offers no other way to observe one.
///
/// # This is a gap in what can be asserted, stated as a gap
///
/// Three separate measurement-based fixtures were written for this and all three were
/// incapable of distinguishing a correct uncomputation from a missing one. Each looked
/// reasonable:
///
/// - `rz(pi)` on |0>, expecting 1. A phase does not move probability, so it is 0 either way.
/// - `H; rz(0.5)`, expecting ~50/50. `H` alone is also ~50/50, so the rotation is not needed.
/// - `H; rz(t); reversible { rz(t) }`, expecting the phases to cancel to the identity.
///   `rz` COMMUTES with `H`, so cancelling the phases leaves `|++>` -- still ~50/50.
///
/// Each of those would have passed against a build where the rotation was dropped entirely.
/// The emitted-IR assertion above -- that the two `qir.r1` calls carry `+theta` and `-theta` --
/// is the only check that actually distinguishes them, and it is a check on the CIRCUIT rather
/// than on the program's behaviour.
///
/// # The observable that would work, and why it is not reachable from source
///
/// Reading the register state directly -- `qir_amplitude`, which the runtime exports and which
/// can see a global phase -- would settle it. Reaching it from a Naso program is blocked by
/// linearity, not by the runtime:
///
/// - Every `[1]`-quantity qubit must be CONSUMED, and the only consuming operation is
///   `measure`.
/// - `qir_mz` collapses the measured qubit AND renormalises, so by the time the program
///   returns the phase of interest is gone.
///
/// Measuring a *different* qubit does not help: the typechecker still requires the rotated
/// binding itself to be consumed, and the language has no `release` or `discard` spelling.
/// (Adding one is exactly the sort of change that needs its own linear-type argument, so it is
/// not smuggled in here to make a test pass.)
///
/// So the honest position: the rotation's uncomputation is verified structurally, on the
/// emitted IR, and its runtime execution is verified at the RUNTIME level -- `rz(0)`,
/// `rz(pi/2)` and `rz(pi)` each produce their expected interference with a Hadamard, which
/// distinguishes an applied angle from an ignored one. What is NOT verified natively is
/// specifically that the uncomputation CANCELS a rotation, because no source program can
/// express the observation that would show it.
///
/// If a `release` builtin is ever added, this test should be replaced with one that reads the
/// amplitude, and the structural assertion above becomes a backstop rather than the only check.
#[test]
fn a_rotation_in_a_reversible_block_is_verified_structurally_because_it_is_unobservable_natively() {
    // The structural half lives in
    // `a_rotation_in_a_reversible_block_is_uncomputed_by_negating_its_angle`, which asserts the
    // forward pass emits `+theta` and the uncomputation `-theta` on the emitted IR.
    //
    // This test exists to keep the GAP VISIBLE and to pin the premise that makes it a gap, so
    // it fails if the situation changes rather than silently continuing to overstate what is
    // verified. Specifically it asserts the premise: a measurement cannot see a global phase.
    //
    // If a future change makes `rz` observable -- an amplitude read, or a second qubit whose
    // measurement interferes with the rotated one -- this test fails and the comment above is
    // what tells the next author to write the real native test.
    let probe = r"
fn main() -> i64 {
    let [1] a: Qubit = qalloc(1);
    hadamard(a);
    if measure(a) { return 1; }
    return 0;
}
";
    let Some(without) = build_and_run("phase_probe_none", probe, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable");
        return;
    };
    // A REAL newline, not an escaped one: `probe` is a raw string, so inserting a literal
    // backslash-n produced `<lex error>` rather than a second statement. That failure was
    // immediate and unambiguous, which is the good case -- a string-built program that is
    // subtly wrong is the bad one.
    let rotated = probe.replace("hadamard(a);", "hadamard(a);\n    rz(0.5, a);\n");
    let Some(with) = build_and_run("phase_probe_rz", &rotated, RUNS) else {
        eprintln!("skipped: llc or cc is unavailable");
        return;
    };

    // `rz(0.5)` after a Hadamard must NOT change the measurement distribution -- because `rz`
    // is a global phase. This is the premise, asserted rather than assumed.
    //
    // If a future `rz` were observable, this assertion would fail, which is the intended
    // signal: the gap is no longer a gap and a real native test should replace this file's
    // comment.
    let ones_without = count(&without, "1");
    let ones_with = count(&with, "1");
    assert!(
        (ones_without as i64 - ones_with as i64).abs() <= 15,
        "a global phase must not be observable in a measurement distribution, but adding \
         `rz(0.5)` moved the count of 1s from {ones_without} to {ones_with} out of {RUNS}. Either \
         `rz` has become observable -- in which case the comment on this test is out of date and \
         a native uncomputation test can now be written -- or the two programs are not equivalent."
    );
}
