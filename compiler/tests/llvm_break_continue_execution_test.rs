//! `break` and `continue` in the LLVM backend, proven by EXECUTION.
//!
//! # What made these hard
//!
//! Neither keyword was a keyword. `break` lexed as the identifier `Var("break")`,
//! so it parsed into a *variable reference*, and a program using it compiled to a
//! read of an unbound variable. Two commits back they parsed into real AST nodes
//! and were then REFUSED by every backend. This file is the point where they
//! actually leave a loop.
//!
//! # The three bugs this unlocks
//!
//! 1. **A missing `position_at_end(body)`.** The `while` emitter built the loop
//!    body wherever the builder happened to be -- the header, which already had
//!    its conditional branch. Every loop then produced an EMPTY body plus a
//!    predecessorless step block, and LLVM rejected the module as malformed.
//! 2. **Double terminators.** An `if` arm ending in `break` has already closed its
//!    block. Branching it to the merge anyway gives the block two terminators.
//!    The merge PHI had to drop the arm from its incoming list for the same reason.
//! 3. **`continue` skipping a counted loop's increment.** This is the subtle one.
//!    `continue` jumping to the loop header would run the guard again without
//!    advancing the counter, so the loop would never terminate -- an infinite loop
//!    that is indistinguishable from correct code by reading it. The fix is a
//!    dedicated step block that `continue` targets, carrying `for`'s increment.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique directory per call, so parallel tests never collide on a temp path.
struct CaseDir(PathBuf);

impl CaseDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p =
            std::env::temp_dir().join(format!("naso-break-{}-{}-{}", tag, std::process::id(), n));
        std::fs::create_dir_all(&p).expect("case dir");
        Self(p)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Locate `llc`/`clang`.
///
/// NOT a hardcoded path. An earlier test file used
/// `/home/linuxbrew/.linuxbrew/opt/llvm@17/bin/{name}`, which passed on the dev
/// container and failed on every CI run, because CI installs LLVM system-wide and
/// puts it on `PATH`. Preferring `PATH` and falling back to the Homebrew prefix
/// works in both places, so the suite tests the COMPILER rather than the layout of
/// one machine.
fn tool(name: &str) -> String {
    if let Ok(found) = Command::new(name).arg("--version").output()
        && found.status.success()
    {
        return name.to_string();
    }
    let brew = "/home/linuxbrew/.linuxbrew/opt/llvm@17/bin";
    if std::path::Path::new(brew).join(name).exists() {
        return format!("{brew}/{name}");
    }
    panic!(
        "`{name}` not found on PATH and not in {brew}; these tests execute generated \
             code, so they cannot be skipped."
    );
}

fn build_ir(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\n{src}\nerror: {e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed:\n{src}\nerror: {e}"));
    builder.module().to_string()
}

/// Compile the IR with `llc`, link a C driver, run it, return what it printed.
fn run(ir: &str, driver: &str) -> String {
    let dir = CaseDir::new("run");
    std::fs::write(dir.join("m.ll"), ir).expect("write ll");
    std::fs::write(dir.join("d.c"), driver).expect("write driver");

    let llc = Command::new(tool("llc"))
        .args([
            "-filetype=obj",
            "-relocation-model=pic",
            "m.ll",
            "-o",
            "m.o",
        ])
        .current_dir(dir.join(""))
        .output()
        .expect("run llc");
    assert!(
        llc.status.success(),
        "llc failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&llc.stderr)
    );

    // `-lm` because `round`/`floor`/`ceil` lower to libm.
    let clang = Command::new(tool("clang"))
        .args(["-O0", "m.o", "d.c", "-lm", "-o", "prog"])
        .current_dir(dir.join(""))
        .output()
        .expect("run clang");
    assert!(
        clang.status.success(),
        "clang link failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&clang.stderr)
    );

    // The program is run UNDER A TIMEOUT, and a hang is reported as a hang.
    //
    // `Command::output()` blocks forever, which turns a non-terminating loop into a
    // stuck test suite that reports nothing: no test name, no assertion, and a CI job
    // that eventually dies on its own job timeout. The first version of this helper
    // had no timeout, so a mutation that sent `continue` to the loop header instead of
    // the step block -- skipping a counted loop's increment -- wedged the whole binary
    // with zero diagnostic output.
    //
    // The timeout is generous enough that a slow machine still passes, and short
    // enough that a real infinite loop is reported within it. These programs are
    // straight-line integer arithmetic over a handful of iterations; 20s is many
    // orders of magnitude more than they need.
    let mut child = Command::new(dir.join("prog"))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn exe");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match child.try_wait().expect("poll exe") {
            Some(_) => break,
            None => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "the compiled program did not terminate within 20s.\n\
                         A loop whose counter does not advance never ends, so this is \
                         almost always an infinite loop rather than slowness: check that \
                         `continue` targets the loop's step block (which carries the \
                         increment) and not the header.\n--- IR ---\n{ir}"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    let out = child.wait_with_output().expect("read exe output");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The error text a program is refused with, or a panic.
fn compile_error(src: &str) -> String {
    let pir = match naso_compiler::lowering::lower_program(
        &naso_compiler::parser::parse_program(src)
            .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}")),
    ) {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    match builder.build_module(&pir) {
        Ok(()) => panic!("expected a refusal, but the program compiled:\n{src}"),
        Err(e) => e.to_string(),
    }
}

/// Bare `break` leaves the loop early, and the work before it is kept.
///
/// This is the test that failed with two terminators on the `then` block. An `if`
/// whose then-arm is `break` is not a conditional -- the arm does not flow to the
/// merge -- so the merge must not be given a predecessor that never arrives.
#[test]
fn break_out_of_a_while_keeps_the_work_done_before_it() {
    let src = "\
fn first_three() -> i64 {
    let mut i = 0;
    let mut s = 0;
    while i < 10 { i = i + 1; if i > 3 { break; } s = s + i; }
    s
}
";
    let ir = build_ir(src);
    // Only 1+2+3 was added: i stops advancing the moment it exceeds 3.
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_first_three(void);
int main(void){ printf("%ld\n", naso_first_three()); return 0; }
"#
        ),
        "6",
        "the loop must accumulate 1+2+3 and then stop, not run to 10 (55) and not \
         stop before the first addition (0).\n--- IR ---\n{ir}"
    );
}

/// `continue` skips the rest of the body but keeps looping.
///
/// The odd-only sum is the discriminator. A `continue` implemented as "ignore the
/// loop and fall through" would add every value and give 45; one that skips the
/// `continue` itself would also give 45; a no-op gives 55. Only real selective
/// iteration gives 25.
#[test]
fn continue_skips_the_rest_of_the_body_and_keeps_iterating() {
    let src = "\
fn odd_sum() -> i64 {
    let mut i = 0;
    let mut s = 0;
    while i < 10 { i = i + 1; if i % 2 == 0 { continue; } s = s + i; }
    s
}
";
    let ir = build_ir(src);
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_odd_sum(void);
int main(void){ printf("%ld\n", naso_odd_sum()); return 0; }
"#
        ),
        "25",
        "only the odd values 1+3+5+7+9 may be accumulated. 45 means `continue` did \
         not skip, 55 means it did nothing, 0 means the body was lost.\n--- IR ---\n{ir}"
    );
}

/// `continue` inside a counted `for` must NOT skip the counter increment.
///
/// This is the regression for the infinite-loop hazard. `for` lowers to a `while`
/// with a step block; if `continue` targeted the header instead of the step, the
/// counter would never advance past 0, the guard would never go false, and the
/// program would hang. The test also confirms even values are skipped, so a
/// `continue` that ran everything would give 45 rather than 25.
#[test]
fn continue_in_a_counted_loop_still_advances_the_counter() {
    let src = "\
fn odd_for() -> i64 {
    let mut s = 0;
    for i in 10 { if i % 2 == 0 { continue; } s = s + i; }
    s
}
";
    let ir = build_ir(src);
    // Had `continue` skipped the increment, this call would never return.
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_odd_for(void);
int main(void){ printf("%ld\n", naso_odd_for()); return 0; }
"#
        ),
        "25",
        "the counter must still advance on a `continue`, and even indices must be \
         skipped: 0+1+2+...+9 = 45 if `continue` were ignored, 25 is correct. A hang \
         here means `continue` jumped to the header and skipped the increment.\n--- IR ---\n{ir}"
    );
}

/// `break` inside a counted `for` stops it, keeping every earlier iteration.
///
/// `for i in 10 { if i == 5 { break; } s = s + i; }` adds 0+1+2+3+4 = 10. Running
/// to completion gives 45, which is what a `break` that branched to the merge
/// instead of the exit would produce.
#[test]
fn break_in_a_counted_loop_stops_the_loop_and_keeps_earlier_iterations() {
    let src = "\
fn stop_at_five() -> i64 {
    let mut s = 0;
    for i in 10 { if i == 5 { break; } s = s + i; }
    s
}
";
    let ir = build_ir(src);
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_stop_at_five(void);
int main(void){ printf("%ld\n", naso_stop_at_five()); return 0; }
"#
        ),
        "10",
        "iterations 0..4 contribute 0+1+2+3+4 = 10. 45 means `break` did not leave \
         the loop.\n--- IR ---\n{ir}"
    );
}

/// A `continue` at the end of a counted body must not lose the increment.
///
/// `for i in 10 { s = s + i; continue; }` -- the `continue` changes nothing about
/// the loop, so the answer is the full sum 45. It fails if the increment is
/// dropped (hang) or if the body's store is skipped (0).
#[test]
fn a_trailing_continue_does_not_change_a_counted_loop() {
    let src = "\
fn trailing() -> i64 {
    let mut s = 0;
    for i in 10 { s = s + i; continue; }
    s
}
";
    let ir = build_ir(src);
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_trailing(void);
int main(void){ printf("%ld\n", naso_trailing()); return 0; }
"#
        ),
        "45",
        "a trailing `continue` must not change the result: 0+1+...+9 = 45. 0 would \
         mean the body's store was skipped.\n--- IR ---\n{ir}"
    );
}

/// A `break` nested inside a `for` inside a `while` must leave only the inner one.
///
/// Both loops count; the outer one must finish its own iterations. If `break`
/// targeted the outermost loop -- a plausible bug when the stack is maintained in
/// the wrong order -- the outer loop would also stop early and the sum would be 6
/// instead of 45.
#[test]
fn break_leaves_only_the_innermost_loop() {
    // The outer counter is a SEPARATE variable. My first version drove the outer
    // loop with `s`, and the inner loop added exactly what the outer body then
    // subtracted -- so `s` never advanced and the program looped forever. It was my
    // test program that was wrong, not the compiler; an empty result here was the
    // honest answer to an infinite loop.
    let src = "\
fn nested() -> i64 {
    let mut outer = 0;
    let mut s = 0;
    while outer < 3 {
        for i in 10 { if i == 5 { break; } s = s + i; }
        outer = outer + 1;
    }
    s
}
";
    let ir = build_ir(src);
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_nested(void);
int main(void){ printf("%ld\n", naso_nested()); return 0; }
"#
        ),
        "30",
        "the inner `break` ends only the `for`, so each of the 3 outer passes adds \
         0+1+2+3+4 = 10, for 30. 10 would mean `break` also left the outer loop, \
         which is what resolving the stack in the wrong order would do.\n--- IR ---\n{ir}"
    );
}

/// A `continue` in an inner loop must not continue the OUTER loop.
///
/// The inner loop runs all 3 iterations each time (giving 3 per outer pass); the
/// outer loop must therefore run 3 times for a total of 9. If `continue` resolved to
/// the outer loop, the inner iteration would be abandoned and the total would
/// differ.
#[test]
fn continue_acts_on_the_innermost_loop_only() {
    let src = "\
fn inner_continue() -> i64 {
    let mut total = 0;
    for a in 3 {
        for b in 3 { if b == 0 { continue; } total = total + 1; }
    }
    total
}
";
    let ir = build_ir(src);
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_inner_continue(void);
int main(void){ printf("%ld\n", naso_inner_continue()); return 0; }
"#
        ),
        "6",
        "each of the 3 outer passes adds 2 (b = 1 and 2), so the total is 6. 3 would \
         mean the inner loop's later iterations were skipped; 9 would mean the \
         `continue` did nothing.\n--- IR ---\n{ir}"
    );
}

/// `break` and `continue` outside any loop are refused, naming the problem.
///
/// Silently ignoring them would let a program that means to stop early run to
/// completion -- a wrong answer, not a diagnostic.
#[test]
fn break_outside_a_loop_is_refused_rather_than_ignored() {
    let msg = compile_error(
        "\
fn f(n: i64) -> i64 {
    if n > 0 { break; }
    n
}
",
    );
    assert!(
        msg.contains("no enclosing loop"),
        "the refusal must say there is no loop to leave, not merely fail: {msg}"
    );
}

/// `continue` outside any loop is refused on the same terms.
#[test]
fn continue_outside_a_loop_is_refused_rather_than_ignored() {
    let msg = compile_error(
        "\
fn f(n: i64) -> i64 {
    if n > 0 { continue; }
    n
}
",
    );
    assert!(
        msg.contains("no enclosing loop"),
        "the refusal must say there is no loop to continue: {msg}"
    );
}

/// `break <value>` stays refused, and the reason names the missing fall-through.
///
/// A loop's result would have to merge every `break` path AND the path where the
/// condition simply goes false -- and that path computed nothing. Filling it with
/// a zero would let an uninitialised result through under the same name as a real
/// one. The message must say that, so the gap is visible rather than mysterious.
#[test]
fn break_with_a_value_is_refused_and_explains_the_fall_through_gap() {
    let msg = compile_error(
        "\
fn f(n: i64) -> i64 {
    let mut s = 0;
    while s < 10 { s = s + 1; if s == 3 { break s; } }
    s
}
",
    );
    assert!(
        msg.contains("fall-through"),
        "the refusal must explain the missing fall-through value: {msg}"
    );
}

/// Structural: a `continue` in a counted loop branches to the step, not the header.
///
/// This is the invariant that keeps `continue` from hanging a counted loop. It is
/// asserted structurally as well as by execution, because the execution failure is
/// a HANG -- a regression there would look like a stuck test rather than a wrong
/// answer, and the structural form names the defect immediately.
#[test]
fn continue_in_a_counted_loop_targets_the_step_block_not_the_header() {
    let ir = build_ir(
        "\
fn f() -> i64 {
    let mut s = 0;
    for i in 4 { if i == 2 { continue; } s = s + 1; }
    s
}
",
    );
    assert!(
        ir.contains("while_step"),
        "a counted loop must have a step block for its increment:\n{ir}"
    );
    // The body must reach the step block.
    assert!(
        ir.contains("br label %while_step"),
        "the body must fall through to the step block, which carries the increment:\n{ir}"
    );
    // The step must branch back to the header, closing the cycle.
    let step_at = ir.find("while_step:").expect("step block exists");
    let tail = &ir[step_at..];
    assert!(
        tail.contains("br label %while_cond"),
        "the step must branch back to the header, or the loop cannot iterate:\n{ir}"
    );
    //
    // EXACTLY ONE unconditional branch to the header, and it is the step's.
    //
    // Without this the mutation `continue_target: header` is caught only by the
    // execution tests HANGING -- the counter never advances, so the program never
    // returns. A hang is a valid detection but a bad one: it burns the whole test
    // timeout, reports nothing about which test failed, and looks like a stuck CI
    // job rather than a defect. Counting the branches names it in milliseconds.
    //
    // 2 would mean something other than the step jumps to the header -- and the only
    // other thing that can is a `continue`.
    let to_header = ir.matches("br label %while_cond").count();
    assert_eq!(
        to_header, 1,
        "exactly one block may branch to the loop header, and it must be the step \
         block. Found {to_header}; a `continue` branching to the header instead of the \
         step skips the increment and the loop never terminates.\n--- IR ---\n{ir}"
    );
}

/// `break v` is refused, and the refusal names a substitute that WORKS.
///
/// `break v` is not merely unimplemented -- it is redundant. A mutable binding
/// initialised before the loop already has a definite value on both exits,
/// because the initialiser covers the fall-through. This test exists so the
/// refusal is a teaching moment rather than a dead end, and so the substitute it
/// recommends is pinned by execution instead of by assertion.
#[test]
fn break_with_a_value_is_refused_and_the_substitution_executed() {
    let msg = compile_error(
        "fn f() -> i64 { let mut s = 0; while s < 10 { s = s + 1; if s == 3 { break s; } } s }\n",
    );
    assert!(
        msg.contains("let mut r = 0"),
        "the refusal must show the working substitution, not just say it is refused: {msg}"
    );
    assert!(
        msg.contains("does not need to be"),
        "the refusal must say the form is REDUNDANT, not merely unimplemented -- that is \
         the substantive claim, and it is what makes the substitution the right answer: {msg}"
    );
    assert!(
        msg.contains("fall-through"),
        "the refusal must explain WHY the sugar is unnecessary -- the fall-through \\
         has no value: {msg}"
    );
}

/// The substitution for `break v`, in a `while`, executed on the CPU.
///
/// `r` is set to `i * 10` when `i` reaches 3, giving 30. This is the exact program
/// the `break s` refusal above tells the reader to write instead.
#[test]
fn the_substitution_for_break_with_a_value_works_in_a_while() {
    let ir = build_ir(
        "\
fn first_third() -> i64 {
    let mut r = 0;
    let mut i = 0;
    while i < 10 { i = i + 1; if i == 3 { r = i * 10; break; } }
    r
}
",
    );
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_first_third(void);
int main(void){ printf("%ld\n", naso_first_third()); return 0; }
"#
        ),
        "30",
        "the break fires when i == 3, so r = 30. 0 would mean the assignment before \\
         the break was lost -- the exact bug a `break v` implementation would have \\
         to get right.\n--- IR ---\n{ir}"
    );
}

/// The substitution in a counted `for`, executed.
///
/// Same guarantee on a different loop form, because `for` lowers through a different
/// path (a synthesized counter plus a `while`) and could have dropped the assignment
/// in the step block.
#[test]
fn the_substitution_for_break_with_a_value_works_in_a_counted_for() {
    let ir = build_ir(
        "\
fn third_of_ten() -> i64 {
    let mut r = 0;
    for i in 10 { if i == 3 { r = i * 10; break; } }
    r
}
",
    );
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_third_of_ten(void);
int main(void){ printf("%ld\n", naso_third_of_ten()); return 0; }
"#
        ),
        "30",
        "the assignment before the break must survive the counted loop's lowering.\n--- IR ---\n{ir}"
    );
}

/// The fall-through case: the initialiser is what makes the substitution safe.
///
/// This is the reason the sugar is unnecessary rather than merely inconvenient. If
/// the loop finishes without breaking, `r` must still hold a sensible value -- here
/// the initialiser 0. A scheme where the loop result existed only on `break` paths
/// would have no answer here at all.
#[test]
fn the_substitution_is_defined_on_the_fall_through_path_too() {
    let ir = build_ir(
        "\
fn never_breaks() -> i64 {
    let mut r = 0;
    let mut i = 0;
    while i < 5 { i = i + 1; }
    r
}
",
    );
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_never_breaks(void);
int main(void){ printf("%ld\n", naso_never_breaks()); return 0; }
"#
        ),
        "0",
        "a loop that completes without breaking leaves the initialiser in place, so \\
         the substitution has a definite value on both exits.\n--- IR ---\n{ir}"
    );
}
