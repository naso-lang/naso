//! `while` in the LLVM backend, proven by EXECUTION.
//!
//! # Why this is not a `forall`
//!
//! A `forall` lowers to a schedule band, because its trip count is statically
//! affine and therefore has a polyhedral form for Polly to transform. A `while`
//! has no such domain -- how many times it runs depends on values computed inside
//! its own body -- so it lowers to a header/body/exit CFG cycle instead. Treating
//! it as a band would mean inventing bounds.
//!
//! Before this, `while` was refused outright with a raw `Debug` dump of the AST
//! node (`Unsupported construct: While(Expr { kind: Binary(Lt, ...) ... })`),
//! which tells a reader nothing about what to do.
//!
//! # The two bugs this class of loop invites
//!
//! 1. **Testing the condition once instead of every iteration.** The guard reads
//!    variables the body mutates, so it must be re-evaluated in the header on
//!    every pass. Caching the compare would either loop forever or never run.
//! 2. **Losing the body.** The body's statements must be emitted INTO the loop
//!    block, not left in the enclosing function. A body that is dropped produces a
//!    well-formed loop that does nothing -- the `IntLit(0)` bug, one level down.
//!
//! Both are invisible in the IR text, so every test here runs the code.

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
            std::env::temp_dir().join(format!("naso-while-{}-{}-{}", tag, std::process::id(), n));
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

    let out = Command::new(dir.join("prog")).output().expect("run exe");
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

/// `count(n)` increments `i` from 0 while `i < n`, then returns `i`.
///
/// Three inputs, including 0 so the loop is never entered. A dropped body gives
/// 0 for every input; a condition tested once gives `n > 0` for every input.
/// Neither matches `0 1 5`.
#[test]
fn a_while_loop_runs_its_body_until_the_condition_becomes_false() {
    let src = "\
fn count(n: i64) -> i64 {
    let mut i = 0;
    while i < n { i = i + 1; }
    i
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_count(long);
int main(void){ printf("%ld %ld %ld %ld\n", naso_count(0), naso_count(1), naso_count(5), naso_count(100)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 1 5 100",
        "the loop must run exactly n times: 0 iterations for n=0, 1 for n=1, 5 for \
         n=5, 100 for n=100. If the body were dropped every answer would be 0; if the \
         condition were tested only once, every n>0 would give the same answer.\n--- IR ---\n{ir}"
    );
}

/// A `while` whose body accumulates, so the trip count is visible in the result.
///
/// `sum_to(n)` adds 1..n. n=5 must give 15, n=0 must give 0. This distinguishes a
/// loop that iterates the right number of times from one that merely terminates:
/// an off-by-one in the guard gives 10 or 21 instead of 15.
#[test]
fn a_while_loop_accumulates_its_body_over_every_iteration() {
    let src = "\
fn sum_to(n: i64) -> i64 {
    let mut total = 0;
    let mut i = 1;
    while i <= n { total = total + i; i = i + 1; }
    total
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_sum_to(long);
int main(void){ printf("%ld %ld %ld\n", naso_sum_to(0), naso_sum_to(5), naso_sum_to(10)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 15 55",
        "1+2+3+4+5 = 15 for n=5, and 1+..+10 = 55 for n=10; n=0 must skip the loop \
         entirely and give 0. An off-by-one in the guard gives 10 or 21 for n=5.\n--- IR ---\n{ir}"
    );
}

/// A `while` nested inside an `if`, so the loop is reached conditionally.
///
/// The `if` runs the loop only for positive input. This is the composition most
/// likely to break, because the loop's CFG and the branch's CFG both want to own
/// the insertion point.
#[test]
fn a_while_loop_inside_an_if_only_runs_when_the_branch_is_taken() {
    let src = "\
fn count_if(x: i64) -> i64 {
    let mut i = 0;
    if x > 0 { while i < x { i = i + 1; } }
    i
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_count_if(long);
int main(void){ printf("%ld %ld %ld\n", naso_count_if(-5), naso_count_if(0), naso_count_if(3)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 0 3",
        "negative input must skip the loop and leave i at 0; x=0 must also skip it \
         because 0 < 0 is false; x=3 must iterate three times. If the `if` stopped \
         controlling the loop, x=-5 would run its body.\n--- IR ---\n{ir}"
    );
}

/// A `let` inside a `while` body, bound fresh on each iteration.
///
/// If the binding were hoisted out of the loop it would keep its first value; if
/// it were freed early, the second iteration would be refused. Either way the sum
/// would differ from the real one, which is what makes this worth executing.
///
/// 2+4+6+8 = 20 for n=4.
#[test]
fn a_let_inside_a_while_body_is_bound_again_on_every_iteration() {
    let src = "\
fn sum_dbl(n: i64) -> i64 {
    let mut total = 0;
    let mut i = 1;
    while i <= n { let d = i * 2; total = total + d; i = i + 1; }
    total
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_sum_dbl(long);
int main(void){ printf("%ld %ld\n", naso_sum_dbl(4), naso_sum_dbl(1)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "20 2",
        "d is i*2 for i=1,2,3,4, so 2 + 4 + 6 + 8 = 20; with n=1 only d=2 runs, giving \
         2. A binding hoisted out of the loop would make every d equal to 2, giving 8 \
         for n=4.\n--- IR ---\n{ir}"
    );
}

/// The loop's result must be read AFTER the loop, not before.
///
/// `i` is a slot, and the value the function returns is a fresh load from it. A
/// stale read of the value from before the loop would return 0 for every input.
#[test]
fn the_result_read_after_a_while_loop_observes_the_loops_final_value() {
    let src = "\
fn count(n: i64) -> i64 {
    let mut i = 0;
    while i < n { i = i + 1; }
    i
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_count(long);
int main(void){ printf("%ld %ld\n", naso_count(7), naso_count(1)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "7 1",
        "the load must happen after the loop exits: 7 for n=7 and 1 for n=1.\n--- IR ---\n{ir}"
    );
}

/// A `return` inside a `while` body is refused, and the refusal says why.
///
/// Same reason as inside an `if`: hoisting it would skip the rest of the body and
/// every later iteration. Hoisting would also mean the loop condition never
/// reaches a false, so the loop would be unreachable.
#[test]
fn a_return_inside_a_while_body_is_refused_rather_than_compiled_to_the_wrong_value() {
    let msg = compile_error(
        "fn f(n: i64) -> i64 { let mut i = 0; while i < n { return i; } return 0; }\n",
    );
    assert!(
        msg.contains("a `return` inside a `while` body"),
        "the diagnostic must name the construct it refuses: {msg}"
    );
    assert!(
        msg.contains("skip the rest of the body"),
        "the diagnostic must say why, not merely that: {msg}"
    );
}

/// The generated IR must RE-TEST the condition in the header, not once on entry.
///
/// This one is a structural assertion on purpose. Execution proves the loop
/// terminates and computes the right answer, but it cannot distinguish "tested
/// once because the body has no effect on the guard" from "tested every
/// iteration" -- both give the same numbers for a loop whose body does change the
/// guard only through the accumulator. Counting the compares pins the structure
/// that makes the loop a loop.
#[test]
fn the_header_block_branches_on_the_condition_to_the_body_or_the_exit() {
    let src = "\
fn count(n: i64) -> i64 {
    let mut i = 0;
    while i < n { i = i + 1; }
    i
}
";
    let ir = build_ir(src);

    assert!(
        ir.contains("while_cond:") && ir.contains("while_body:") && ir.contains("while_exit:"),
        "the loop must lower to a header/body/exit cycle with these block names.\n--- IR ---\n{ir}"
    );

    // The HEADER must contain a CONDITIONAL branch over the condition, choosing
    // between the body and the exit.
    //
    // This is deliberately stronger than counting `icmp` instructions. I first wrote it
    // that way and the mutation that removes the header's conditional branch --
    // replacing it with an unconditional jump to the body, which loops forever --
    // SURVIVED, because the compare was still emitted and only its result went
    // unused. Counting instructions proves a compare exists; it does not prove
    // anything branches on it. So the assertion is on the branch itself.
    let header = ir
        .split("while_cond:")
        .nth(1)
        .and_then(|rest| rest.split("while_body:").next())
        .expect("header block present");
    let header_branch = header
        .lines()
        .find(|l| l.trim_start().starts_with("br "))
        .unwrap_or_else(|| panic!("the header block must end in a branch.\n--- IR ---\n{ir}"));
    assert!(
        header_branch.contains("i1")
            && header_branch.contains("while_body")
            && header_branch.contains("while_exit"),
        "the header must branch CONDITIONALLY on the condition, to the body or the \
         exit. An unconditional jump to the body would make the loop run forever, \
         because nothing would ever reach the exit. Found: {header_branch:?}\n--- IR ---\n{ir}"
    );

    // And the BODY must jump back to the header, or the loop would run once.
    let body = ir
        .split("while_body:")
        .nth(1)
        .and_then(|rest| rest.split("while_exit:").next())
        .expect("body block present");
    assert!(
        body.contains("br label %while_cond"),
        "the body must branch back to the header to re-test the condition.\n--- IR ---\n{ir}"
    );

    // The BODY must contain the statement it was given.
    //
    // Dropping the body produces a well-formed, terminating-looking loop that never
    // changes anything. Every execution test for this compiler then HANGS rather than
    // failing, because the induction variable never advances and the header branch is
    // correct the whole time. A hang is a failure, but it is a slow, ambiguous one --
    // it looks like a stuck test runner rather than a wrong compiler.
    //
    // So the body's effect is asserted structurally: `i = i + 1` must appear as a
    // store inside `while_body`. That distinguishes "the loop ran its body" from
    // "the loop had an empty body" without executing anything.
    assert!(
        body.contains("add i64") && body.contains("store i64"),
        "the body must contain the update it was given (`i = i + 1`), as an add and a \
         store. An empty body makes the loop well-formed but never advances the \
         induction variable, so it runs forever instead of failing.\n--- IR ---\n{ir}"
    );
}
