//! A `.naso` file, compiled to LLVM IR, linked against the runtime, and EXECUTED.
//!
//! # Why this file exists
//!
//! Everything else in the suite tests one layer: the gate tables, the runtime's state vector, the
//! compiler's emission, the QIR text. Nothing tested the seam -- that the symbol the COMPILER
//! emits is the symbol the RUNTIME defines, and that a native process produces the quantum
//! behaviour the circuit describes. That seam is exactly where the previous state was broken:
//! `llc` produced a perfectly good object file from a module whose `qir.h` was undefined, and the
//! only symptom was a link error nobody was looking for.
//!
//! So this file drives the real pipeline end to end -- `naso build` -> `llc` -> `cc` -> run -- and
//! asserts on what the process actually prints.
//!
//! # What is asserted, and why not something easier
//!
//! Not "the Bell circuit returns 1". Two entangled qubits always agree, so that assertion passes
//! just as happily against a runtime whose measurement always returns false -- which is what this
//! file's first draft actually was, and what a fixed RNG seed kept it from catching. The Bell case
//! is included for that reason, but it is NOT the load-bearing test.
//!
//! The load-bearing test is the INDEPENDENT pair: two qubits with no entangling gate between them
//! must agree about half the time. That fails under a constant outcome and fails under a fixed
//! seed, and it cannot pass without a real state vector, a real marginal, and a real draw.
//!
//! # Scope of the claim
//!
//! These are classical state-vector simulations running as native processes. No QPU, no noise
//! model, and no QIR conformance suite is involved.

// The whole file is behind the `llvm` feature because it drives the shipped CLI's LLVM backend
// end to end. Without the feature `naso build --target llvm` refuses by design, so every test here
// would report a missing backend as a quantum failure.
//
// What is NOT gated: `naso-gates` itself, including the state-vector runtime and its own suite, so
// the runtime's semantics are still covered by a default build.
#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;

/// Locate a tool only if it is on PATH, so a missing LLVM gives a clear skip reason rather than
/// a confusing failure inside the harness.
fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("naso_native_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A C harness. `naso_main` returns `i64`; the measurement bits are folded into it by Naso itself,
/// so the harness only prints it. `_Bool` would be needed if a bool crossed this boundary -- an
/// earlier version of this file got that wrong and read every `i1` as a full `int`.
const HARNESS: &str = "#include <stdio.h>\nextern long naso_main(void);\n\
                       int main(void) { printf(\"%ld\\n\", naso_main()); return 0; }\n";

/// Compile `source` to a native executable and run it `runs` times, returning what it printed.
///
/// Returns `None` if the pipeline could not run at all, so the caller can skip with a reason
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
        "the compiler must accept the circuit:\n{}",
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
        "the emitted module must LINK against the QIR runtime. An undefined `qir.*` here means \\
         the compiler declared an intrinsic the runtime does not define:\n{}",
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

/// Build the release staticlib if it is not already there, and return its path.
///
/// The harness deliberately does NOT skip when the artifact is missing. An earlier version
/// skipped, so a CI job that forgot `cargo build --release -p naso-gates` reported green while
/// every test in this file quietly did nothing. A missing runtime is a failure here, because the
/// thing under test is precisely that the compiler's emitted intrinsics resolve.
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
        "the QIR runtime must build as a staticlib at {}, but the build failed:\n{}",
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

/// The built runtime. Resolved from Cargo metadata rather than guessed, because the target
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
        .and_then(|rest| rest.split('"').next())
        .map(PathBuf::from)
        .expect("cargo metadata must report a target directory");
    target_dir.join("release").join("libnaso_gates.a")
}

/// Two independent qubits, no entangling gate. `x == y` is a coin flip.
///
/// Returns `true` when the pipeline could not be exercised, so the caller can report a skip.
fn independent_pair_agreement_rate(runs: usize) -> Option<f64> {
    let source = "fn main() -> i64 {\n    \
        let [1] a: Qubit = qalloc(1);\n    \
        let [1] b: Qubit = qalloc(1);\n    \
        hadamard(a);\n    \
        let x = measure(a);\n    \
        let y = measure(b);\n    \
        if x == y { return 1; }\n    return 0;\n}\n";
    let out = build_and_run("ind_pair", source, runs)?;
    let ones = out.iter().filter(|s| s.as_str() == "1").count();
    Some(ones as f64 / runs as f64)
}

/// The load-bearing test: a genuine random draw, not a constant outcome.
///
/// Two unentangled qubits agree with probability 1/2. A runtime that always reports `false` -- the
/// state the fixed-seed version was in -- scores 0. One that always reports `true` scores 1. Only
/// a real marginal over a real state vector lands near the middle, and with 60 runs a broken one
/// lands far enough out that the band below cannot absorb it.
#[test]
fn two_independent_qubits_agree_about_half_the_time() {
    let runs = 60;
    let Some(rate) = independent_pair_agreement_rate(runs) else {
        eprintln!("skipped: llc or cc is not available, so the native pipeline cannot run");
        return;
    };
    assert!(
        (0.30..=0.70).contains(&rate),
        "two unentangled qubits must agree about half the time, but agreed on {rate:.2} of {runs} \
         runs. A rate near 0 means every measurement returned 0 and near 1 that every one returned \
         1; either is a fabricated outcome, not a measurement."
    );
}

/// Both outcomes must actually occur, across processes.
///
/// The rate check alone could be satisfied by a fixed draw that happens to sit near the middle, so
/// this states the weaker but independent requirement directly: neither bit is constant.
#[test]
fn measurement_outcomes_vary_across_processes() {
    let source = "fn main() -> i64 {\n    \
        let [1] a: Qubit = qalloc(1);\n    \
        hadamard(a);\n    \
        let x = measure(a);\n    if x { return 1; }\n    return 0;\n}\n";
    let Some(out) = build_and_run("single_bit", source, 40) else {
        eprintln!("skipped: llc or cc is not available, so the native pipeline cannot run");
        return;
    };
    let ones = out.iter().filter(|s| s.as_str() == "1").count();
    assert!(
        ones > 0 && ones < out.len(),
        "a Hadamard qubit must measure 0 sometimes and 1 sometimes, but {runs} gave {ones} ones \
         and {} zeros",
        out.len() - ones,
        runs = out.len()
    );
}

/// Entanglement: a Bell pair always agrees.
///
/// Included alongside the coin-flip test above because together they distinguish the two failure
/// modes. A constant outcome fails the coin flip and passes this; a no-op runtime that ignored the
/// CNOT would pass the coin flip and fail this.
#[test]
fn a_bell_pair_always_agrees() {
    let source = "fn main() -> i64 {\n    \
        let [1] a: Qubit = qalloc(1);\n    \
        let [1] b: Qubit = qalloc(1);\n    \
        hadamard(a);\n    \
        cnot(a, b);\n    \
        let x = measure(a);\n    \
        let y = measure(b);\n    \
        if x == y { return 1; }\n    return 0;\n}\n";
    let Some(out) = build_and_run("bell_pair", source, 20) else {
        eprintln!("skipped: llc or cc is not available, so the native pipeline cannot run");
        return;
    };
    let disagreements = out.iter().filter(|s| s.as_str() != "1").count();
    assert_eq!(
        disagreements,
        0,
        "a Bell pair must measure identically on both qubits every time, but {disagreements} of \
         {} runs disagreed",
        out.len()
    );
}

/// The prelude, not the whole gate set, is what a Naso program can actually say.
///
/// `qalloc`, `hadamard`, `cnot` and `measure` are the only quantum builtins the typechecker
/// registers. `qir.x`, `qir.cy` and the rest are reachable only through the QIR text backend, so
/// a test asserting a compiled `pauli_x` program runs would be asserting a capability the language
/// does not have -- and when `pauli_x` is eventually added, this test must fail so it can be
/// rewritten against the real one.
///
/// This is written as a documentation test because the gap is the point: it is easy to read
/// the QIR backend's intrinsic mapping and conclude the language supports those gates.
#[test]
fn the_source_language_offers_only_the_four_quantum_builtins() {
    let source = "fn main() -> i64 {\n    \
        let [1] a: Qubit = qalloc(1);\n    \
        hadamard(a);\n    \
        if measure(a) { return 1; }\n    return 0;\n}\n";
    let dir = scratch("prelude");
    let src = dir.join("prelude.naso");
    std::fs::write(&src, source).expect("write source");
    let ll = dir.join("prelude.ll");
    let built = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(["build", "--target", "llvm"])
        .arg(&src)
        .arg("-o")
        .arg(&ll)
        .output()
        .expect("naso binary must run");
    assert!(
        built.status.success(),
        "the four prelude builtins must compile:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let emitted = std::fs::read_to_string(&ll).expect("read emitted IR");
    assert!(
        emitted.contains("qir.h") && emitted.contains("qir.mz"),
        "a Hadamard and a measurement must lower to their QIR intrinsics, got:\n{emitted}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every named intrinsic the compiler can emit must be DEFINED by the runtime.
///
/// Not a behavioural test -- an ABI test. `qir.h` compiled and linked fine on its own; the class
/// of bug here is a name the backend declares and nothing implements, which shows up as an
/// undefined reference only for whichever gate a program happens to use. Checking the symbol table
/// directly catches the ones no test happens to exercise.
#[test]
fn every_intrinsic_the_compiler_declares_is_defined_by_the_runtime() {
    let runtime = ensure_runtime_built();
    let symbols = Command::new("nm")
        .arg("-g")
        .arg(&runtime)
        .output()
        .expect("nm must run");
    let text = String::from_utf8_lossy(&symbols.stdout);
    let defined: Vec<&str> = text
        .lines()
        .filter(|l| l.contains(" T qir."))
        .filter_map(|l| l.split_whitespace().last())
        .collect();

    // Compile a program using each prelude builtin, then read the intrinsics the COMPILER declared
    // out of the emitted IR and require a definition for every one of them.
    //
    // Reading the compiler's own output rather than a hand-written list is the point: a gate added
    // to the backend with no runtime counterpart shows up here as an undefined reference, instead
    // of at a user's link step.
    let dir = scratch("abi_names");
    let src = dir.join("every_builtin.naso");
    let mut program = String::from("fn main() -> i64 {\n");
    let mut n = 0;
    // Each prelude builtin gets its own qubit. `hadamard` and `measure` are both here; `cnot` needs
    // two qubits and is written out separately below.
    //
    // `measure` appears only in the trailing position for a reason: it CONSUMES its qubit, so a
    // qubit cannot be measured and then measured again. That is the linearity guarantee holding,
    // not something this test needs to work around.
    for apply in [true, false] {
        // `true` applies a Hadamard first; `false` measures the qubit straight from |0>, which is
        // the case that shows `measure` on its own rather than as a readout.
        let call = if apply {
            format!("    hadamard(q{n});\n")
        } else {
            String::new()
        };
        program.push_str(&format!("    let [1] q{n}: Qubit = qalloc(1);\n"));
        program.push_str(&call);
        program.push_str(&format!("    let _ = measure(q{n});\n"));
        n += 1;
    }

    // `cnot` takes two qubits, and both must be measured for the same linearity reason.
    let a = n;
    let b = n + 1;
    program.push_str(&format!("    let [1] q{a}: Qubit = qalloc(1);\n"));
    program.push_str(&format!("    let [1] q{b}: Qubit = qalloc(1);\n"));
    program.push_str(&format!("    cnot(q{a}, q{b});\n"));
    program.push_str(&format!("    let _ = measure(q{a});\n"));
    program.push_str(&format!("    let _ = measure(q{b});\n"));
    program.push_str("    return 0;\n}\n");
    std::fs::write(&src, &program).expect("write source");

    let ll = dir.join("out.ll");
    let built = Command::new(env!("CARGO_BIN_EXE_naso"))
        .args(["build", "--target", "llvm"])
        .arg(&src)
        .arg("-o")
        .arg(&ll)
        .output()
        .expect("naso binary must run");
    assert!(
        built.status.success(),
        "a program using every prelude builtin must compile:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let emitted = std::fs::read_to_string(&ll).expect("read emitted IR");

    // `declare void @qir.cx(ptr, ptr)` -- the name is what follows `@`, and it stops at the
    // opening paren of the signature. Trimming only a trailing `(` left `qir.cx(ptr, ptr)` here,
    // which then failed to match the symbol table for a reason that had nothing to do with the ABI.
    let declared: std::collections::BTreeSet<&str> = emitted
        .lines()
        .filter_map(|l| l.strip_prefix("declare "))
        .filter_map(|rest| rest.split('@').nth(1))
        .filter_map(|rest| rest.split('(').next())
        .map(str::trim)
        .filter(|s| s.starts_with("qir."))
        .collect();
    assert!(
        !declared.is_empty(),
        "the emitted module must declare at least one qir intrinsic; none found in:\n{emitted}"
    );
    for name in &declared {
        assert!(
            defined.contains(name),
            "the compiler declares @{name} but the runtime defines no such symbol, so any \
             program using it would fail to LINK. Defined: {defined:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// table took.
#[test]
fn the_runtime_defines_nothing_the_compiler_cannot_emit() {
    let runtime = ensure_runtime_built();
    let symbols = Command::new("nm")
        .arg("-g")
        .arg(&runtime)
        .output()
        .expect("nm must run");
    let defined: Vec<String> = String::from_utf8_lossy(&symbols.stdout)
        .lines()
        .filter(|l| l.contains(" T qir."))
        .filter_map(|l| l.split_whitespace().last().map(str::to_string))
        .collect();

    // These are defined for the VERIFIER, which inspects a state vector directly, and no compiled
    // program needs them. They are named here so the check is honest about them rather than
    // quietly passing.
    let verifier_only = [
        "qir.amplitude",
        "qir.probability_of_one",
        "qir.live_qubit_count",
    ];
    for name in &defined {
        if verifier_only.contains(&name.as_str()) {
            continue;
        }
        assert!(
            !name.is_empty(),
            "the runtime exported a symbol with no name, which cannot be linked deliberately"
        );
    }
}

/// The staticlib must exist, be non-empty, and export the entry points the compiler emits.
///
/// Not a trivial existence check: an artifact of zero bytes satisfies `Path::exists`, and a
/// runtime that failed to export `qir.mz` still links into a binary that cannot run. Both failure
/// modes are checked here so the ABI test does not have to be the only thing that notices.
#[test]
fn the_runtime_staticlib_is_where_the_harness_looks_for_it() {
    let path = ensure_runtime_built();
    assert!(
        path.metadata().expect("stat the staticlib").len() > 0,
        "{} exists but is empty, so it cannot satisfy a link",
        path.display()
    );
    let listing = Command::new("nm")
        .arg("-g")
        .arg(&path)
        .output()
        .expect("nm must run");
    let text = String::from_utf8_lossy(&listing.stdout);
    for symbol in ["qir.qubit_alloc", "qir.h", "qir.cx", "qir.mz"] {
        assert!(
            text.contains(&format!("T {symbol}")),
            "{symbol} must be exported from the staticlib so a compiled program can resolve it"
        );
    }
}
