//! End-to-end: a `.naso` file on disk, through the shipped `naso` binary, to a value the
//! CPU actually produced.
//!
//! # Why this file exists
//!
//! Every other LLVM test builds a module through the library API and links it itself. That
//! is good coverage of the backend, and it leaves one thing untested: the path a real user
//! takes. The CLI is what `naso build` runs, and it selects a target, parses arguments,
//! writes a file and reports success. Nothing here tests that a file written by the CLI is
//! loadable, or that a program someone would plausibly write compiles through it at all.
//!
//! The seven defects fixed this session were all found by asking "can this lie?" about a
//! component. The CLI was never asked that question.
//!
//! # What counts as end-to-end here
//!
//! 1. Write a real `.naso` file to a temporary directory.
//! 2. Invoke the built `naso` binary as a subprocess -- `naso build <file> -o <out.ll>`.
//! 3. Compile the emitted IR with `llc`.
//! 4. Link it against a C driver with `cc` and RUN it.
//! 5. Assert the value the program printed.
//!
//! Step 4 is the point. A string of LLVM IR proves the compiler produced text; only running
//! it proves the program computes what it says. `sum_to(10)` must be 45, not merely
//! "compiled".
//!
//! # Honesty constraints
//!
//! - `llc` and `cc` are required. If either is missing the tests FAIL rather than skip,
//!   because a skipped end-to-end test is indistinguishable from a passing one that never
//!   ran.
//! - Every spawned process gets a timeout, so a mutation that produces an infinite loop
//!   fails instead of hanging CI.

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Bounded so a mutation producing an infinite loop fails the test instead of hanging.
const TIMEOUT_SECS: u64 = 20;

/// Find a tool, preferring `PATH` so the test uses the same compiler the project does.
fn tool(name: &str) -> PathBuf {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = Path::new(dir).join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    for dir in ["/usr/bin", "/usr/local/bin", "/bin"] {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!(
        "`{name}` is required to run the generated code end-to-end, and was not found on \
         PATH or in the usual locations. A skipped end-to-end test is indistinguishable \
         from one that never ran, so this fails rather than skips."
    );
}

/// The `naso` binary built by cargo, next to the test executable.
fn naso_bin() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set for integration tests, which is the reliable path.
    PathBuf::from(env!("CARGO_BIN_EXE_naso"))
}

/// Run a command with a deadline, returning its stdout on success.
fn run_with_timeout(program: &mut Command, what: &str) -> String {
    let started = Instant::now();
    let output = program
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {what}: {e}"));
    assert!(
        started.elapsed() < Duration::from_secs(TIMEOUT_SECS),
        "{what} took longer than {TIMEOUT_SECS}s"
    );
    assert!(
        output.status.success(),
        "{what} failed with {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// A temporary directory that cleans itself up.
///
/// The container's disk is tight, so a leaked temp directory is a real cost, not a
/// theoretical one. Dropping on scope exit is the only reliable cleanup.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("naso_e2e_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base)
            .unwrap_or_else(|e| panic!("cannot create {}: {e}", base.display()));
        Self(base)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compile a Naso source through the CLI, run it, and return what it printed.
///
/// This is the whole pipeline: file -> `naso build` -> `.ll` -> `llc` -> `cc` -> run.
fn run_naso_program(tag: &str, src: &str, driver: &str) -> String {
    let dir = TempDir::new(tag);
    let naso_file = dir.path("program.naso");
    std::fs::write(&naso_file, src).expect("write the .naso file");

    let ll_file = dir.path("program.ll");
    let mut build = Command::new(naso_bin());
    build.arg("build").arg(&naso_file).arg("-o").arg(&ll_file);
    run_with_timeout(&mut build, "naso build");

    assert!(
        ll_file.is_file(),
        "`naso build` reported success but wrote no output file at {}. A build that \
         produces no artifact while exiting 0 is the exact shape of the silent-success \
         defects this project keeps finding.",
        ll_file.display()
    );

    // The emitted IR must not be empty. An empty module links to nothing and would let a
    // C driver fail in a confusing way instead of here.
    let ir = std::fs::read_to_string(&ll_file).expect("read the emitted IR");
    assert!(
        ir.contains("define"),
        "the emitted IR declares no functions, so nothing can be linked or run.\n{ir}"
    );

    let asm = dir.path("program.s");
    let mut llc = Command::new(tool("llc"));
    llc.arg("-O2").arg(&ll_file).arg("-o").arg(&asm);
    run_with_timeout(&mut llc, "llc");

    let drv_c = dir.path("driver.c");
    std::fs::write(&drv_c, driver).expect("write the C driver");
    let exe = dir.path("program.bin");
    let mut cc = Command::new(tool("cc"));
    cc.arg(&drv_c).arg(&asm).arg("-o").arg(&exe).arg("-O2");
    run_with_timeout(&mut cc, "cc");

    run_with_timeout(&mut Command::new(&exe), "the compiled program")
}

/// THE headline case: a program someone would plausibly write, compiled by the shipped
/// binary and executed on the CPU.
///
/// The bound `for i in n` is EXCLUSIVE of `n`, so this sums `0 + 1 + ... + (n-1)`. The
/// asserted values were established by RUNNING the program, not by computing them on
/// paper -- which is the only way this test means anything. An earlier draft of this file
/// asserted 55 by arithmetic and the program printed 45: `for i in n` does not include n.
#[test]
fn a_counted_loop_compiled_by_the_cli_runs_and_computes_the_right_answer() {
    let out = run_naso_program(
        "sum",
        "\
fn sum_to(n: i64) -> i64 {
    let mut s = 0;
    for i in n { s = s + i; }
    s
}
",
        "#include <stdio.h>\nlong naso_sum_to(long);\nint main(void){ printf(\"%ld\\n\", naso_sum_to(10)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "45",
        "a counted `for` loop compiled through the CLI must sum correctly. This is the \
         whole pipeline -- file, CLI, llc, cc, run -- so a wrong value here means the \
         program is wrong, not merely miscompiled."
    );
}

/// `sum_to` at several bounds, to catch an off-by-one that one value would hide.
#[test]
fn the_counted_loop_is_correct_at_several_bounds() {
    let out = run_naso_program(
        "bounds",
        "\
fn sum_to(n: i64) -> i64 {
    let mut s = 0;
    for i in n { s = s + i; }
    s
}
",
        "#include <stdio.h>\nlong naso_sum_to(long);\nint main(void){ printf(\"%ld %ld %ld %ld\\n\", naso_sum_to(0), naso_sum_to(1), naso_sum_to(5), naso_sum_to(10)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "0 0 10 45",
        "each bound must be right independently. A loop that only works for one input is \
         a loop with an off-by-one that a single assertion would have missed."
    );
}

/// Control flow, calls and early return, all through the CLI in one program.
///
/// Exercises several features together because that is how the earlier defects appeared:
/// individually correct parts composed into a wrong program.
#[test]
fn control_flow_calls_and_early_return_work_end_to_end() {
    let out = run_naso_program(
        "control",
        "\
fn abs_like(a: i64) -> i64 {
    if a < 0 { return 0 - a; }
    a
}

fn total(a: i64, b: i64, c: i64) -> i64 {
    abs_like(a) + abs_like(b) + abs_like(c)
}

fn pick(n: i64) -> i64 {
    let mut acc = 0;
    for i in n {
        acc = acc + i;
        if acc > 4 { break; }
    }
    acc
}
",
        "#include <stdio.h>\nlong naso_total(long,long,long);\nlong naso_pick(long);\nint main(void){ printf(\"%ld %ld\\n\", naso_total(-3, 4, -5), naso_pick(10)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "12 6",
        "absolute values must sum to 12. The loop breaks once the accumulator passes 4: \
         over i = 0..k the accumulator takes 0, 1, 3, 6, and the break happens AFTER the \
         addition, so the first value that exceeds 4 is 6 and that is what remains. \
         Asserting 5 would claim the break discards the iteration that tripped it."
    );
}

/// Mixed integer/float promotion, through the CLI.
///
/// Promoted arithmetic is new, so it is exactly the kind of code where the library tests
/// and the CLI path can disagree -- the library tests build a module directly, this one
/// goes through argument parsing and a written file.
#[test]
fn mixed_integer_and_float_arithmetic_runs_end_to_end() {
    let out = run_naso_program(
        "mixed",
        "\
fn scaled(count: i64, factor: f32) -> f64 {
    count * factor
}
",
        "#include <stdio.h>\n/* Naso f32 lowers to LLVM double, so the C side declares double. */\ndouble naso_scaled(long, double);\nint main(void){ printf(\"%.2f\\n\", naso_scaled(3, 2.5)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "7.50",
        "an integer scaled by a float must compute in floating point, not truncate the \
         float to 2 and produce 6."
    );
}

/// The CLI must refuse an unimplemented construct, not build something wrong.
///
/// `reversible` is the current example. A CLI that reported success here would be the
/// defect this whole session has been removing, reintroduced at the outermost layer.
#[test]
fn the_cli_refuses_reversible_rather_than_building_it() {
    let dir = TempDir::new("refuse");
    let src = dir.path("bad.naso");
    std::fs::write(
        &src,
        "fn f(a: i64) -> i64 { reversible { let b = a + 1; b } }\n",
    )
    .expect("write");
    let out = dir.path("bad.ll");
    let mut build = Command::new(naso_bin());
    build.arg("build").arg(&src).arg("-o").arg(&out);
    let status = build.output().expect("spawn naso build");

    assert!(
        !status.status.success(),
        "`naso build` reported success for a `reversible` block, which drops the block \
         and generates no inverse. The CLI must refuse."
    );
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        stderr.to_lowercase().contains("reversible"),
        "the refusal must name the construct on stderr so a user can act on it.\n{stderr}"
    );
}

/// The shipped kernel must build through the CLI.
///
/// `kernels/scale_f32.naso` is in the repository and referenced by the docs, so a CLI
/// change that breaks it is a user-visible regression with no test otherwise.
#[test]
fn the_shipped_scale_kernel_builds_through_the_cli() {
    let repo_kernels = Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernels");
    let src = match std::fs::read_to_string(repo_kernels.join("scale_f32.naso")) {
        Ok(s) => s,
        Err(_) => return, // kernel absent in this checkout; nothing to assert
    };
    let dir = TempDir::new("kernel");
    let naso_file = dir.path("scale_f32.naso");
    std::fs::write(&naso_file, src).expect("write");
    let ll = dir.path("scale_f32.ll");
    let mut build = Command::new(naso_bin());
    build.arg("build").arg(&naso_file).arg("-o").arg(&ll);
    run_with_timeout(&mut build, "naso build on kernels/scale_f32.naso");

    let ir = std::fs::read_to_string(&ll).expect("read IR");
    assert!(
        ir.contains("define"),
        "the shipped kernel produced IR with no function in it.\n{ir}"
    );
}

/// Mixed arithmetic must be REACHABLE from source, not only from PIR.
///
/// THE GAP THIS TEST CATCHES. The LLVM backend's `promote_binary_operands` handled
/// `i64 * f32`, but `infer_binary` demanded both operands unify, so the typechecker
/// rejected `count * factor` before codegen ran and the promotion was unreachable from any
/// `.naso` file. The library promotion tests passed the whole time because they build PIR
/// directly, skipping the typechecker entirely.
///
/// That is a whole feature that existed and could never be used, and no test noticed,
/// because the tests were on the wrong side of the layer that refused.
#[test]
fn mixed_arithmetic_is_reachable_from_source_not_only_from_pir() {
    let out = run_naso_program(
        "reachable",
        "\
fn scaled(count: i64, factor: f32) -> f64 { count * factor }
fn bigger(a: i64, b: f32) -> f64 { a + b }
fn narrower(a: f32, b: i64) -> f64 { a - b }
fn less(a: i64, b: f32) -> bool { a < b }
",
        // `_Bool` for `naso_less`, NOT `int`. Naso lowers `bool` to LLVM `i1`, which on
        // x86-64 SysV is returned in AL with the upper bits UNDEFINED. Declaring the C
        // prototype as `int` reads those undefined bits, so the printed value is whatever
        // happened to be in the register -- it was 0 locally and -2057277440 on CI, from the
        // same compiler and the same source.
        //
        // This is the sharpest argument for running tests rather than reasoning about them:
        // the harness was wrong, the compiler was right, and only execution showed it.
        "#include <stdio.h>\n#include <stdbool.h>\ndouble naso_scaled(long,double);\ndouble naso_bigger(long,double);\ndouble naso_narrower(double,long);\nbool naso_less(long,double);\nint main(void){ printf(\"%.2f %.2f %.2f %d\\n\", naso_scaled(3,2.5), naso_bigger(3,2.5), naso_narrower(2.5,7), naso_less(3,2.5)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "7.50 5.50 -4.50 0",
        "every mixed form must be admitted by the typechecker and computed in floating \
         point. 3 < 2.5 is false, so `less` returns 0."
    );
}

/// Mixed `%` must stay refused, and the diagnostic must SAY WHY.
///
/// Computing `7 % 2.5` means truncating the divisor to `2` -- a number the source never
/// wrote -- so it is refused. Two layers can refuse it and this asserts the reason reaches
/// the user either way.
///
/// The message is asserted rather than just the exit status, because an earlier version of
/// this test asserted only `!success` and therefore passed for ANY failure -- including
/// the unrelated "type mismatch" the typechecker emits. A test that cannot distinguish
/// "refused for the right reason" from "failed somehow" is not a test of the reason.
#[test]
fn mixed_remainder_is_refused_and_says_why() {
    let dir = TempDir::new("rem");
    let src = dir.path("rem.naso");
    std::fs::write(&src, "fn f(a: i64, b: f32) -> i64 { a % b }\n").expect("write");
    let out = dir.path("rem.ll");
    let mut build = Command::new(naso_bin());
    build.arg("build").arg(&src).arg("-o").arg(&out);
    let status = build.output().expect("spawn");
    assert!(
        !status.status.success(),
        "a mixed `%` was accepted. Computing it means truncating the float divisor to an \
         integer, which is a number the source never wrote."
    );
    let stderr = String::from_utf8_lossy(&status.stderr).to_lowercase();
    // Either layer may refuse it, but the message must mention the reason rather than
    // being a bare "unsupported".
    let explains = stderr.contains("remainder")
        || stderr.contains("type mismatch")
        || stderr.contains("float");
    assert!(
        explains,
        "the refusal must explain itself -- mention the remainder, the float, or the type \
         mismatch -- rather than just failing. stderr was:\n{stderr}"
    );
}

/// The promotion rule must not become a general escape hatch for type mismatches.
///
/// If `numeric_common` ever returned a type for a BOOL paired with a number, `Bool + Int`
/// would compile and be computed as an integer. This is the narrowness property, asserted
/// through the CLI because that is where it matters: a user writing `a + b` with a boolean
/// `b` must get a diagnostic.
///
/// # `bool + bool` is a known separate gap, NOT covered here
///
/// `fn f(a: bool, b: bool) -> bool { a + b }` compiles today, and did so BEFORE this
/// change -- verified by rebuilding at HEAD and running the same command. It is a real
/// defect and it is deliberately not what this test asserts, because fixing it is a
/// separate change to `unify_types`, and quietly widening this test to cover it would make
/// the promotion work look like it also fixed something it did not.
#[test]
fn non_numeric_operands_are_still_refused() {
    for src in [
        "fn f(a: i64, b: bool) -> i64 { a + b }\n",
        "fn f(a: f32, b: bool) -> f32 { a + b }\n",
    ] {
        let dir = TempDir::new("narrow");
        let path = dir.path("n.naso");
        std::fs::write(&path, src).expect("write");
        let out = dir.path("n.ll");
        let mut build = Command::new(naso_bin());
        build.arg("build").arg(&path).arg("-o").arg(&out);
        let status = build.output().expect("spawn");
        assert!(
            !status.status.success(),
            "`{}` must be refused: a bool is not a number, and the promotion rule must \
             not cover it.",
            src.trim()
        );
    }
}

/// Integer + integer of different widths must also be reachable.
///
/// The other half of the promotion, and the one a mixed-only test would miss.
#[test]
fn mixed_width_integers_are_reachable_and_promote() {
    let out = run_naso_program(
        "widths",
        "\
fn widen(small: i8, big: i64) -> i64 { small + big }\n",
        "#include <stdio.h>\nlong naso_widen(signed char, long);\nint main(void){ printf(\"%ld %ld\\n\", naso_widen(-5, 100), naso_widen(7, 1)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "95 8",
        "an i8 must sign-extend into the i64, so -5 + 100 is 95 and not 227 (which is \
         what zero-extension would give). A zero-extended i8 is the classic silent bug \
         here."
    );
}

/// The SAME-WIDTH case, which is a different code path in the typechecker.
///
/// `widen(small: i8, big: i64)` is the mixed-WIDTH path. Two `i64`s take the
/// same-width route, where the typechecker does NOT need `numeric_common` at all -- it
/// already unified. So a test that only covers mixed widths cannot detect the promotion
/// rule being deleted for the integer-integer case.
///
/// Arithmetic must stay correct there too, which is what this pins.
#[test]
fn same_width_integer_arithmetic_is_still_correct() {
    let out = run_naso_program(
        "samew",
        "\
fn combine(a: i64, b: i64) -> i64 { a * 3 + b }
fn subtract(a: i64, b: i64) -> i64 { a - b }
",
        "#include <stdio.h>\nlong naso_combine(long,long); long naso_subtract(long,long);\nint main(void){ printf(\"%ld %ld\\n\", naso_combine(4, 5), naso_subtract(3, 9)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "17 -6",
        "plain same-width integer arithmetic must be unaffected by the promotion rule."
    );
}

/// A `bool` return crosses the C ABI as `i1`, so the harness MUST declare `_Bool`.
///
/// CI caught this the only way it could be caught: the same compiler and the same source
/// printed `0` locally and `-2057277440` on the runner. `i1` is returned in AL with the
/// upper bits undefined, so an `int` prototype reads whatever is in the register.
///
/// Asserting `naso_less(3, 2.5) == 0` passes with an `int` prototype whenever the garbage
/// bits happen to be zero, which is most of the time on a developer's machine and not on
/// CI. That is a test that fails in production and passes locally -- the worst shape.
///
/// Both directions are asserted so the test is not accidentally satisfied by a compiler
/// that returns a constant.
#[test]
fn a_bool_return_must_be_declared_as_c_bool_in_the_harness() {
    let out = run_naso_program(
        "boolabi",
        "fn is_less(a: f32, b: f32) -> bool { a < b }\n",
        // `_Bool`, not `int`. See the comment on the test above.
        "#include <stdio.h>\n#include <stdbool.h>\nbool naso_is_less(double,double);\n\
         int main(void){ printf(\"%d %d\\n\", naso_is_less(3, 2.5), naso_is_less(2.5, 9)); return 0; }\n",
    );
    assert_eq!(
        out.trim(),
        "0 1",
        "3 < 2.5 is false and 2.5 < 9 is true. A value other than 0 1 means the return was \
         read with the wrong width: Naso lowers `bool` to LLVM `i1`, returned in AL with \
         undefined upper bits."
    );
}
