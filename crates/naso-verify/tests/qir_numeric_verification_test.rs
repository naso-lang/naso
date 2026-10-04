// Everything below runs the REAL compiler: it invokes the `naso` binary, parses what the
// compiler emits, and replays it. That needs the compiler built with LLVM, so it lives in
// a gated module -- with its helpers, since a helper outside the module would be dead code
// in a default build and would fail `-D warnings`.
#[cfg(feature = "llvm")]
mod from_compiler_output {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use naso_gates::statevector::{Complex, Gate, StateVector};
    use naso_verify::qir_circuit::{EmittedOp, parse_circuit, simulate};

    /// Locate the `naso` CLI.
    ///
    /// Resolved by asking `PATH` FIRST, then the workspace target directory. Local development has
    /// a Homebrew prefix and CI installs system-wide with the binary on `PATH`, so a lookup that
    /// assumes one machine's layout is green here and red on the runner -- this test would then be
    /// measuring the build environment rather than the compiler.
    fn naso_binary() -> std::path::PathBuf {
        if let Ok(path) = std::env::var("PATH") {
            for dir in path.split(':') {
                let candidate = std::path::Path::new(dir).join("naso");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        // Fall back to the build directory.
        //
        // `CARGO_TARGET_DIR` is normally UNSET: the redirection on this machine lives in a
        // developer-local `~/.cargo/config.toml`, which cargo reads but does not export to test
        // binaries. So ask CARGO for its resolved value instead of guessing -- `cargo` from the
        // manifest directory is the same invocation that built this test, so it cannot disagree
        // about where the artifact went. Guessing from `CARGO_MANIFEST_DIR` alone finds nothing,
        // and a test that cannot find the toolchain should say so rather than skip.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut tried: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
            tried.push(std::path::PathBuf::from(dir).join("debug").join("naso"));
        }
        if let Ok(out) = Command::new("cargo")
            .arg("metadata")
            .arg("--format-version=1")
            .arg("--no-deps")
            .current_dir(&manifest)
            .output()
            && out.status.success()
        {
            // `target_directory` is the resolved value, including any config-file override.
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(at) = text.find("\"target_directory\":\"") {
                let rest = &text[at + 20..];
                if let Some(end) = rest.find('"') {
                    tried.push(
                        std::path::PathBuf::from(&rest[..end])
                            .join("debug")
                            .join("naso"),
                    );
                }
            }
        }
        for up in [2usize, 3] {
            let mut base = manifest.clone();
            for _ in 0..up {
                base = base.parent().map(|p| p.to_path_buf()).unwrap_or(base);
            }
            tried.push(base.join("target").join("debug").join("naso"));
        }

        match tried.iter().find(|p| p.is_file()).cloned() {
            Some(path) => path,
            None => panic!(
                "cannot find the `naso` CLI. This test verifies the compiler's emitted output, so \
                 the compiler's binary is required -- build it with \
                 `cargo build --features llvm --bin naso`. Tried: {tried:?}"
            ),
        }
    }

    /// The tests that run the real compiler need the compiler built with LLVM, so they are gated on
    /// `naso-verify/llvm`, which passes through to `naso-compiler/llvm`.
    ///
    /// The two parser-only tests below are NOT gated: they feed hand-written QIR to the bridge, so
    /// they cover the parser in any feature set. That matters because a suite that is entirely
    /// feature-gated reports `ok. 0 passed` under a default `cargo test`, which is indistinguishable
    /// from a pass and is how an entire verification step goes missing unnoticed.
    #[cfg(feature = "llvm")]
    fn scratch(label: &str) -> PathBuf {
        scratch_in(std::env::temp_dir().join(format!("naso_qir_verify_{label}")))
    }

    /// A fresh scratch directory for one test's output.
    fn scratch_in(dir: PathBuf) -> PathBuf {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Run the shipped CLI on `source` and return the emitted QIR text.
    #[cfg(feature = "llvm")]
    ///
    /// Through the CLI rather than the library on purpose: the flag a user types is the artifact
    /// being verified, and a library probe can pass while the command handler routes elsewhere.
    fn emit_qir(dir: &Path, name: &str, source: &str) -> String {
        let src = dir.join(format!("{name}.naso"));
        let out = dir.join(format!("{name}.qir"));
        std::fs::write(&src, source).expect("write source");

        let result = // `naso` is the compiler's binary, and this crate depends on the compiler rather than the
        // reverse -- so it is reached by PATH-resolved workspace path rather than by cargo's
        // `CARGO_BIN_EXE_` variable, which only names binaries of the crate under test.
        Command::new(naso_binary())
            .args(["build", "--target", "qir"])
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .output()
            .expect("naso binary must run");

        assert!(
            result.status.success(),
            "compiling to QIR failed for {name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::read_to_string(&out).unwrap_or_else(|e| panic!("read {name}.qir: {e}"))
    }

    /// `|psi>` for `n` qubits, all in |0>.
    fn zero_state(n: usize) -> StateVector {
        StateVector::zero_state(n)
    }

    const INV_SQRT_2: f64 = std::f64::consts::FRAC_1_SQRT_2;

    /// Compare two states amplitude by amplitude.
    #[track_caller]
    fn assert_states_match(
        got: &StateVector,
        expected: &[(usize, Complex)],
        n: usize,
        label: &str,
    ) {
        assert_eq!(
            got.qubits(),
            n,
            "{label}: state spans the wrong number of qubits"
        );
        for (index, amplitude) in got.amplitudes().iter().enumerate() {
            let want = expected
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, a)| *a)
                .unwrap_or(Complex::new(0.0, 0.0));
            assert!(
                (amplitude.re - want.re).abs() < 1e-9 && (amplitude.im - want.im).abs() < 1e-9,
                "{label}: amplitude {index} was {amplitude:?}, expected {want:?}\nfull state: {:?}",
                got.amplitudes()
            );
        }
    }

    /// A Hadamard on the only qubit produces the uniform superposition.
    // No `cfg` gate here: `naso-verify` has no `llvm` feature of its own, and gating on one would
    // silently compile this file to NOTHING -- an empty test binary that reports `ok. 0 passed`
    // and looks like a pass. Whether the compiler was built with LLVM is a property of the
    // compiler crate; the tests below assert that themselves where it matters, by requiring the
    // emitted text to actually contain a gate.
    /// The compiler's emitted QIR, checked numerically against a hand-derived state.
    ///
    /// # What this establishes, and what it does not
    ///
    /// QIR is structural text. No QPU, no simulator, and no `qir.*` runtime has executed any of
    /// it -- the `qir_entry` function carries no ENTRYPOINT marker, because emitting one needs an
    /// LLVM attribute registration inkwell's named-enum path does not provide (attempting it
    /// segfaults; see `references/inkwell-llvm-api-traps.md`).
    ///
    /// So this is the strongest check available without a runtime: take the text the compiler
    /// actually emitted, recover the gate sequence and the qubit each gate acts on, replay it
    /// through the shared gate table, and compare against a state derived independently by hand.
    ///
    /// That catches a gate applied to the wrong qubit, gates emitted out of order, and a gate
    /// missing from the output -- the three defects that made the previous generation of QIR
    /// fixtures pass while emitting a circuit that was not the source program.
    ///
    /// It does NOT establish that a real QIR runtime would interpret these calls the same way.
    /// The `qir.*` intrinsic semantics themselves remain unvalidated by execution.
    ///
    /// # Why the expected states are written out rather than computed by the same code
    ///
    /// Each expectation is the closed form for that circuit, derived by hand and written out in
    /// the test. Deriving them with the simulator would make the comparison circular: a bug in the
    /// gate table would cancel on both sides. The Bell pair is
    ///
    ///     |00> --H on q0--> (|00> + |10>)/sqrt(2) --CX(q0,q1)--> (|00> + |11>)/sqrt(2)
    ///
    /// and note the bit ORDER, which is the classic trap: qubit `q` is BIT `q` of the basis
    /// index, so `(q0=1, q1=0)` is index 1 and `(q0=0, q1=1)` is index 2.

    #[test]
    fn a_single_hadamard_is_the_uniform_superposition() {
        let dir = scratch("h");
        let qir = emit_qir(
            &dir,
            "h",
            "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    hadamard(a);\n    let m = measure(a);\n    let _ = m;\n}\n",
        );

        let circuit = parse_circuit(&qir).expect("parse emitted QIR");
        assert_eq!(
            circuit.qubits.len(),
            1,
            "one qubit should be allocated: {circuit:?}"
        );

        // The fixture measures to consume the qubit, so replay stops at `qir.mz` -- which is
        // exactly the point: the state returned is the one just BEFORE the measurement, i.e. the
        // state the gates produced.
        let (state, measured) = simulate(&circuit, &zero_state(1)).expect("replay");
        assert_eq!(measured.as_deref(), Some("q0"), "the circuit measures q0");

        // (|0> + |1>)/sqrt(2) -- indices 0 and 1, because q0 is the LOW bit.
        assert_states_match(
            &state,
            &[
                (0, Complex::new(INV_SQRT_2, 0.0)),
                (1, Complex::new(INV_SQRT_2, 0.0)),
            ],
            1,
            "one H",
        );
    }

    /// The Bell pair. This is the circuit the previous generation of fixtures could not represent.
    ///
    /// Both measurements must read the SAME entangled state, so `state.probability_of_one` for
    /// each qubit is 0.5 -- which is what distinguishes entanglement from two independent qubits
    /// in |0>, where both probabilities would be 0.
    #[test]
    fn a_bell_pair_is_entangled() {
        let dir = scratch("bell");
        let qir = emit_qir(
            &dir,
            "bell",
            "fn bell() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    hadamard(a);\n    cnot(a, b);\n    let m = measure(a);\n    let n = measure(b);\n    let _ = m;\n    let _ = n;\n}\n",
        );

        let circuit = parse_circuit(&qir).expect("parse emitted QIR");
        assert_eq!(
            circuit.qubits.len(),
            2,
            "two qubits should be allocated: {circuit:?}"
        );

        // The gates, in order, on the right qubits. This is the check that a presence assertion
        // cannot make: `qir` contains `qir.h(` either way.
        use naso_gates::statevector::Gate;
        use naso_verify::qir_circuit::EmittedOp;
        let gates: Vec<(&Gate, Vec<&str>)> = circuit
            .ops
            .iter()
            .filter_map(|op| match op {
                EmittedOp::Gate { gate, qubits } => {
                    Some((gate, qubits.iter().map(String::as_str).collect()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            gates,
            vec![(&Gate::H, vec!["q0"]), (&Gate::Cx, vec!["q0", "q1"])],
            "the emitted gate sequence and its operands must be H on q0 then CX(q0 -> q1): {circuit:?}"
        );

        // Replay. The measurement stops the run, so the state is the one just before it.
        let (state, measured) = simulate(&circuit, &zero_state(2)).expect("replay");
        assert_eq!(
            measured.as_deref(),
            Some("q0"),
            "the first measurement reads q0, so replay stops there"
        );

        // (|00> + |11>)/sqrt(2) -- indices 0 and 3. Index 3 is q0=1 AND q1=1.
        assert_states_match(
            &state,
            &[
                (0, Complex::new(INV_SQRT_2, 0.0)),
                (3, Complex::new(INV_SQRT_2, 0.0)),
            ],
            2,
            "Bell pair",
        );

        // And the entanglement claim itself: each qubit is individually random.
        assert!(
            (state.probability_of_one(0) - 0.5).abs() < 1e-9,
            "q0 should be 50/50 in a Bell pair, got {}",
            state.probability_of_one(0)
        );
        assert!(
            (state.probability_of_one(1) - 0.5).abs() < 1e-9,
            "q1 should be 50/50 in a Bell pair, got {}",
            state.probability_of_one(1)
        );
    }

    /// A CX with the control in |0> does nothing, and with it in |1> flips the target.
    ///
    /// This is the control for the Bell-pair test, and it is what makes that test discriminating:
    /// it shows the parser resolves operands to DISTINCT qubits, so "both gates act on q0" would
    /// produce a different, detectable state.
    #[test]
    fn a_controlled_gate_only_fires_when_its_control_is_set() {
        let dir = scratch("cx");
        let qir = emit_qir(
            &dir,
            "cx",
            "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    cnot(a, b);\n    let m = measure(a);\n    let n = measure(b);\n    let _ = m;\n    let _ = n;\n}\n",
        );

        let circuit = parse_circuit(&qir).expect("parse emitted QIR");

        // Control in |0>: |00> must survive unchanged. A reversed CX would give |11> here.
        let (state, _) = simulate(&circuit, &zero_state(2)).expect("replay");
        assert_states_match(
            &state,
            &[(0, Complex::new(1.0, 0.0))],
            2,
            "CX with control |0>",
        );

        // Control in |1>: |10> -> |11>. Index 1 is q0=1, q1=0.
        // Index 1 is q0=1, q1=0 -- the control set and the target clear, which is the only input
        // that distinguishes a controlled gate from an unconditional one.
        let one_control = StateVector::from_amplitudes(
            2,
            vec![
                Complex::new(0.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(0.0, 0.0),
                Complex::new(0.0, 0.0),
            ],
        );
        let (state, _) = simulate(&circuit, &one_control).expect("replay");
        assert_states_match(
            &state,
            &[(3, Complex::new(1.0, 0.0))],
            2,
            "CX with control |1>",
        );
    }

    /// The inverse relation, checked through the compiler's own output.
    ///
    /// The point is not that `inverse_of` is correct -- its own tests establish that -- but that
    /// the compiler EMITS gates the shared table can invert, so the table is reachable from the
    /// emitted artifact rather than only from tests written beside it.
    #[test]
    fn every_emitted_gate_can_be_inverted_by_the_shared_table() {
        let dir = scratch("inv");
        // A circuit using each single-qubit gate the QIR backend maps.
        let qir = emit_qir(
            &dir,
            "inv",
            "fn f() {\n    let [1] a: Qubit = qalloc(1);\n    let [1] b: Qubit = qalloc(1);\n    hadamard(a);\n    cnot(a, b);\n    let m = measure(a);\n    let n = measure(b);\n    let _ = m;\n    let _ = n;\n}\n",
        );

        let circuit = parse_circuit(&qir).expect("parse emitted QIR");
        let emitted: Vec<_> = circuit
            .ops
            .iter()
            .filter_map(|op| match op {
                EmittedOp::Gate { gate, .. } => Some(*gate),
                _ => None,
            })
            .collect();
        assert!(
            !emitted.is_empty(),
            "the fixture should emit at least one gate; {circuit:?}"
        );
        // `inverse_of` is TOTAL -- there is no unknown-gate fallback -- so this cannot fail on a
        // missing entry. What it establishes is REACHABILITY: the compiler emits gates the verified
        // adjoint relation can invert, so the relation applies to real output rather than only to
        // tests written beside it. Each gate is round-tripped NUMERICALLY, on the operands the
        // compiler actually chose, so the claim is about the emitted gate rather than the table's.
        let emitted: Vec<(Gate, Vec<String>)> = circuit
            .ops
            .iter()
            .filter_map(|op| match op {
                EmittedOp::Gate { gate, qubits } => Some((*gate, qubits.clone())),
                _ => None,
            })
            .collect();

        for (gate, qubits) in &emitted {
            let start = StateVector::zero_state(qubits.len());
            // Apply the gate and its adjoint on the same operands, in the same order: for a
            // controlled gate, inverting means running the same gate twice, not swapping operands.
            let indices: Vec<usize> = qubits
                .iter()
                .map(|name| name.trim_start_matches('q').parse().expect("qubit index"))
                .collect();
            // `apply` takes an optional SECOND qubit: one `None` for a single-qubit gate, and the
            // control/target pair for a controlled gate. Passing `None` for `cx` panics, so the
            // arity has to follow the emitted operand count.
            let apply_all = |gate: &Gate, state: StateVector| match indices.as_slice() {
                [one] => gate.apply(&state, *one, None),
                [control, target] => gate.apply(&state, *control, Some(*target)),
                [a, b, t] => state.apply_toffoli(*a, *b, *t),
                other => panic!("unexpected operand count {other:?} for {gate:?}"),
            };
            let there = apply_all(gate, start.clone());
            let inverse = naso_gates::gate_inverse::inverse_of(*gate);
            let back = apply_all(&inverse, there.clone());
            assert!(
                back.equivalent_to(&start, 1e-9),
                "applying {gate:?} on {indices:?} then its adjoint {inverse:?} did not return the \
                 initial state, so the adjoint relation does not hold for a gate the compiler \
                 emits: {back:?}"
            );
        }
    }
}

// ---- parser-only: these feed hand-written QIR to the bridge, so they run in ANY feature
// set. An entirely feature-gated suite reports `ok. 0 passed` under a default
// `cargo test`, which is indistinguishable from a pass.

use naso_verify::qir_circuit::parse_circuit;

/// An unmodelled gate is a parse ERROR, never a silent skip.
///
/// The bridge has to refuse `iswap` and the parameterised rotations rather than approximate
/// them: mapping `iswap` to `swap` computes a different permutation, and treating `rx` as
/// identity erases the rotation. Either would produce a numerically wrong circuit that still
/// verified, which is the exact failure this whole module was written to prevent.
#[test]
fn an_unmodelled_gate_is_refused_rather_than_approximated() {
    let qir = "\
define void @qir_entry() {
entry:
  %qalloc = call ptr @qir.qubit_alloc()
  %a = alloca ptr, align 8
  store ptr %qalloc, ptr %a, align 8
  %a1 = load ptr, ptr %a, align 8
  %a2 = load ptr, ptr %a, align 8
  call void @qir.iswap(ptr %a1, ptr %a2)
  ret void
}
";
    let err = parse_circuit(qir).expect_err("iswap is not modelled and must be refused");
    assert!(
        err.to_string().contains("iswap"),
        "the refusal must name the unmodelled operation, got: {err}"
    );
}

/// A gate on a qubit that was never allocated is refused.
///
/// Without this the bridge would accept a circuit acting on a qubit that does not exist, and
/// `qubit_index` would clamp it to some other qubit -- a wrong answer from a valid-looking
/// module.
#[test]
fn a_gate_on_an_unallocated_qubit_is_refused() {
    let qir = "\
define void @qir_entry() {
entry:
  %a = alloca ptr, align 8
  store ptr null, ptr %a, align 8
  %a1 = load ptr, ptr %a, align 8
  call void @qir.h(ptr %a1)
  ret void
}
";
    let err = parse_circuit(qir).expect_err("a qubit with no allocation must be refused");
    assert!(
        err.to_string()
            .contains("not the result of a qubit allocation"),
        "the refusal must say the operand was never allocated, got: {err}"
    );
}
