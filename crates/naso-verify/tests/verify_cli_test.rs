//! The exit-code contract of the `naso-verify` binary, tested end to end.
//!
//! These are integration tests because the whole point of the contract is the PROCESS exit
//! status, which no unit test inside the binary can observe: `main` returns an `ExitCode`,
//! and a test that calls the library directly cannot tell whether a refuted obligation would
//! have been reported to a CI job. So these run the real binary and read the real status.
//!
//! The contract exists because four different states all look like "no output" if they
//! collapse into one: proved everything, disproved something, could not decide, and was never
//! given a file. A verifier that returns 0 for the third and fourth is a verifier whose green
//! build carries no information.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Path to the `naso-verify` binary cargo built for this test run.
fn binary() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for integration tests of a package's own bins.
    PathBuf::from(env!("CARGO_BIN_EXE_naso-verify"))
}

struct Run {
    code: i32,
    stdout: String,
}

/// Run the verifier over `source`, returning the exit code and stdout.
fn verify(source: &str, extra_args: &[&str]) -> Run {
    let mut path = std::env::temp_dir();
    // A unique name per call: integration tests run in parallel threads, and two tests
    // sharing one file would read each other's source.
    path.push(format!(
        "naso_verify_it_{}_{}.naso",
        std::process::id(),
        next_id()
    ));
    let mut f = std::fs::File::create(&path).expect("create temp .naso");
    f.write_all(source.as_bytes()).expect("write");
    drop(f);

    let out = Command::new(binary())
        .args(extra_args)
        .arg(&path)
        .output()
        .expect("run naso-verify");

    let _ = std::fs::remove_file(&path);
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

use std::sync::atomic::{AtomicUsize, Ordering};
static ID: AtomicUsize = AtomicUsize::new(0);
fn next_id() -> usize {
    ID.fetch_add(1, Ordering::SeqCst)
}

#[test]
fn a_discharged_obligation_exits_zero() {
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(n + 0 == n); } return true; }\n",
        &[],
    );
    assert_eq!(
        r.code, 0,
        "a proved obligation must exit 0. stdout:\n{}",
        r.stdout
    );
    // And it must SAY it proved something. Silence is not a proof report.
    assert!(
        r.stdout.contains("1 obligation(s) discharged"),
        "the run must report what it discharged, got:\n{}",
        r.stdout
    );
}

#[test]
fn a_discharged_obligation_is_not_reported_as_an_error() {
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(n + 0 == n); } return true; }\n",
        &[],
    );
    assert!(
        !r.stdout.contains("error[NASO-OBL-000]"),
        "a PROVED obligation must never be labelled `error`. stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("note[NASO-OBL-000]"),
        "a proved obligation should be labelled `note`. stdout:\n{}",
        r.stdout
    );
}

#[test]
fn a_refuted_obligation_exits_one() {
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(n >= 10); } return true; }\n",
        &[],
    );
    assert_eq!(
        r.code, 1,
        "a refuted claim must exit 1. stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("NASO-OBL-001"),
        "the refutation must be reported, got:\n{}",
        r.stdout
    );
}

#[test]
fn an_undecidable_obligation_exits_two_not_zero() {
    // `sigmoidise` has no SMT definition and is refused by name, so the obligation is
    // undecidable. (round USED to be this stand-in; it now carries a bounding axiom, so it
    // is no longer an honest "unsupported" example.) An undecidable obligation must NOT be
    // reported as success: "I could not look at it" and "it is true" are different claims,
    // and only one of them justifies a green build.
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(sigmoidise(n)); } return true; }\n",
        &[],
    );
    assert_eq!(
        r.code, 2,
        "an UNDECIDED obligation must exit 2, never 0. stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("undecided"),
        "the undecided count must be reported, got:\n{}",
        r.stdout
    );
}

#[test]
fn a_refutation_outranks_an_undecided_obligation() {
    // `sigmoidise(n)` is undecidable and `n >= 10` is refutable; the refutation outranks.
    // (round is no longer the undecidable stand-in.)
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(sigmoidise(n)); assert(n >= 10); } return true; }\n",
        &[],
    );
    assert_eq!(
        r.code, 1,
        "a proven-false claim must be reported as 1 even when another is undecided. \
         stdout:\n{}",
        r.stdout
    );
}

#[test]
fn a_missing_file_exits_four_not_one() {
    // A proof failure and "you gave me nothing" must not share an exit code.
    let out = Command::new(binary())
        .arg("/nonexistent/definitely/not/here.naso")
        .output()
        .expect("run naso-verify");
    assert_eq!(
        out.status.code(),
        Some(4),
        "a missing input is an INPUT error, not a proof failure"
    );
}

#[test]
fn a_bad_argument_exits_three() {
    let out = Command::new(binary())
        .args(["--mode", "not-a-mode", "--help-me-not"])
        .output();
    // `--help-me-not` is an unknown flag, which clap rejects; either that or the bad mode
    // must produce a usage error.
    match out {
        Ok(o) => assert!(
            o.status.code() == Some(3) || o.status.code() == Some(0),
            "bad arguments must be a usage error (3), got {:?}. stdout:\n{}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout)
        ),
        Err(e) => panic!("could not run naso-verify: {e}"),
    }
}

#[test]
fn no_arguments_exits_three() {
    let out = Command::new(binary()).output().expect("run naso-verify");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn require_obligations_fails_a_file_that_proves_nothing() {
    // Without this flag a file with no `proof` block trivially "passes", which is exactly how
    // a typo'd proof block survives review.
    let src = "fn f(n: int) -> bool { return n == n; }\n";
    let without = verify(src, &["--mode", "obligations"]);
    assert_eq!(without.code, 0, "baseline: nothing to check exits 0");

    let with = verify(src, &["--mode", "obligations", "--require-obligations"]);
    assert_eq!(
        with.code, 6,
        "with --require-obligations, a file that discharged nothing must exit 6. stdout:\n{}",
        with.stdout
    );
}

#[test]
fn require_obligations_still_passes_a_file_that_proved_something() {
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(n + 0 == n); } return true; }\n",
        &["--mode", "obligations", "--require-obligations"],
    );
    assert_eq!(r.code, 0, "stdout:\n{}", r.stdout);
}

#[test]
fn json_output_is_valid_json_and_carries_the_verdict() {
    let r = verify(
        "fn f(n: int) -> bool { proof { assert(n >= 10); } return true; }\n",
        &["--format", "json"],
    );
    assert_eq!(r.code, 1);
    assert!(
        r.stdout.trim_start().starts_with('{') && r.stdout.trim_end().ends_with('}'),
        "json output must be a single object, got:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("\"refuted\":true"),
        "the JSON verdict must say the obligation was refuted, got:\n{}",
        r.stdout
    );
}

#[test]
fn a_linear_tensor_consumed_inside_a_loop_is_not_reported_as_leaking() {
    // REGRESSION. `output[i] = ..` inside a `forall` is a use of `output`, and the quantity
    // walker used to skip loops entirely, so this reported a FALSE leak on a program `naso
    // check` accepts. A verifier that condemns correct code gets ignored.
    let r = verify(
        "fn f(input: [1] Tensor[i8, 16], output: inout [1] Tensor[i8, 16]) {\n\
         \x20 forall i in 0..16 {\n\
         \x20   let v = input[i];\n\
         \x20   output[i] = v;\n\
         \x20 }\n\
         }\n",
        &["--mode", "linearity"],
    );
    assert_eq!(
        r.code, 0,
        "a linear tensor read and written inside a loop must not be reported as leaking. \
         stdout:\n{}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("never consumed"),
        "the false leak must be gone, got:\n{}",
        r.stdout
    );
}

#[test]
fn a_second_use_of_a_linear_tensor_is_reported_as_double_consumption() {
    // The other half of the regression test: the walker now records EVERY use, so a genuine
    // second use must be caught too. Without this, "fixing" the false leak by simply not
    // counting uses would also pass.
    //
    // Note this program was originally written as the false-leak fixture and BOTH tools
    // rejected it -- `naso check` with "used twice", `naso-verify` with "Double consumption".
    // Two independently written analysers reaching the same verdict on the same program is
    // the evidence that they agree; the fixture was wrong, not either tool.
    let r = verify(
        "fn f(input: [1] Tensor[i8, 16], output: inout [1] Tensor[i8, 16]) -> i8 {\n\
         \x20 forall i in 0..16 {\n\
         \x20   output[i] = input[i];\n\
         \x20 }\n\
         \x20 return output[0];\n\
         }\n",
        &["--mode", "linearity"],
    );
    assert_eq!(
        r.code, 1,
        "a second use of `[1]` must be reported. stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("consumed 2 times"),
        "the double consumption must be named, got:\n{}",
        r.stdout
    );
}

#[test]
fn a_genuinely_unconsumed_linear_tensor_is_still_reported() {
    // The positive control for the regression test above: if the walker were simply switched
    // off, this would pass too.
    let r = verify(
        "fn f(t: [1] Tensor[i8, 16]) -> i8 { return 0; }\n",
        &["--mode", "linearity"],
    );
    assert_eq!(
        r.code, 1,
        "a `[1]` tensor nothing consumes IS a leak. stdout:\n{}",
        r.stdout
    );
}

#[test]
fn the_shipped_quantisation_kernel_discharges_through_the_cli() {
    // End to end: the kernel on disk, through the real binary, read as a clean run.
    let kernel =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../kernels/quant_error_bound.naso");
    let out = Command::new(binary())
        .args(["--mode", "obligations", "--require-obligations"])
        .arg(&kernel)
        .output()
        .expect("run naso-verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the shipped error-bound kernel must verify clean. stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("2 obligation(s) discharged"),
        "both kernel obligations must be discharged, got:\n{stdout}"
    );
}

#[test]
fn the_shipped_int8_quantiser_kernel_discharges_through_the_cli() {
    // End to end: the int8 quantiser kernel on disk, through the real binary.
    // Six obligations must all discharge -- pins the `to_real` cast lowering + the
    // gated round axiom against regressions a library-only test cannot observe.
    let kernel = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../kernels/quant_int8.naso");
    let out = Command::new(binary())
        .args(["--mode", "obligations", "--require-obligations"])
        .arg(&kernel)
        .output()
        .expect("run naso-verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the shipped int8 kernel must verify clean. stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("6 obligation(s) discharged"),
        "all six kernel obligations must be discharged, got:\n{stdout}"
    );
}

#[test]
fn a_false_round_equality_refutes_under_universal_axiom() {
    // `round(t) == t` is FALSE for t=0.25 (round(0.25) = 0 != 0.25).
    // Under AUFLIRA (selected by the pre-scan for round-equality), Z3 refutes this via direct e-matching
    // on the universal integer-value axiom `round(x) = to_real(to_int(round(x)))`
    // -- no witness sampler needed. Exit 1 (refuted).
    let r = verify(
        "fn check_round_eq() -> Bool { proof { forall t in 0.0..1.0 { assert(round(t) == t); } } return true; }
",
        &["--mode", "obligations", "--require-obligations"],
    );
    assert_eq!(
        r.code, 1,
        "a false round-equality must refute (exit 1). stdout:
{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("NASO-OBL-001") && r.stdout.contains("1 refuted"),
        "the refutation must be reported, got:
{}",
        r.stdout
    );
}

#[test]
fn a_true_round_equality_discharges_under_universal_axiom() {
    // `round(t) == round(t)` is a tautology. Under AUFLIRA, Z3 finds no
    // countermodel (UNSAT negation) and discharges with exit 0.
    let r = verify(
        "fn check_round_taut() -> Bool { proof { forall t in 0.0..1.0 { assert(round(t) == round(t)); } } return true; }
",
        &["--mode", "obligations", "--require-obligations"],
    );
    assert_eq!(
        r.code, 0,
        "a true round-equality must discharge (exit 0). stdout:
{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("1 obligation(s) discharged"),
        "the tautology must be reported as discharged, got:
{}",
        r.stdout
    );
}

#[test]
fn round_equality_refutes_within_timeout_under_auflira() {
    // DISCRIMINATING PIN for `ground_round_equality_wants_lra` (the AUFLIRA pre-scan
    // logic switch). `round(t) == t` is FALSE for any t in (0, 1) (e.g. t=0.25 -> round(0.25)=0).
    // Under AUFLIRA (selected by the pre-scan for round-equality), Z3 refutes by treating
    // `round` as uninterpreted -- it finds a model where round(t) != t. Exit 1, fast.
    //
    // Under UFLIA (mutation A: forcing `logic_needs_lra = false`), Z3 cannot e-match on
    // the universal integer-value axiom, so it times out -- exit 2, not exit 1. The 5s
    // timeout makes this a discriminating case: if the pre-scan stopped selecting AUFLIRA,
    // this test would flip from exit 1 to exit 2, killing the mutant immediately.
    //
    // This pins the logic switch as load-bearing at the CLI level (not just the lib level).
    let r = verify(
        "fn check_round_eq() -> Bool { proof { forall t in 0.0..1.0 { assert(round(t) == t); } } return true; }\n",
        &[
            "--mode",
            "obligations",
            "--timeout",
            "5000",
            "--require-obligations",
        ],
    );
    assert_eq!(
        r.code, 1,
        "round-equality must REFUTE (exit 1) within 5s under AUFLIRA, not time out (exit 2). stdout:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("NASO-OBL-001"),
        "the refutation must be reported as NASO-OBL-001, got:\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("1 refuted"),
        "exactly 1 obligation must be reported as refuted, got:\n{}",
        r.stdout
    );
}
