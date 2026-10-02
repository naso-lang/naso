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

// ===== `break` / `continue`: parsed, lowered, and refused =====
//
// Before this, `break` had NO representation anywhere in the pipeline. It lexed as a
// plain IDENTIFIER, so `if c { break; }` parsed into `ExprKind::Var("break")` -- no
// syntax error, no missing-keyword error, and an AST indistinguishable from a
// program that legitimately reads a variable named `break`.
//
// The only thing standing between that and a silent wrong answer was the strict
// unbound-read refusal in the LLVM backend, which failed the program with "read of
// `break`: no allocation is known for it". That names a VARIABLE the author never
// wrote, which sends a reader looking for a missing declaration instead of a missing
// language feature. So the diagnostic was accurate about the compiler's state and
// misleading about the program.

/// The lexer must produce a `break` KEYWORD, not an identifier.
///
/// This is the root of the whole problem, so it is asserted first and on its own. A
/// test that only checked the eventual diagnostic would pass if `break` were still
/// an identifier that happened to be refused later -- which is the state this suite
/// was written to end.
#[test]
fn break_and_continue_lex_as_keywords_and_not_as_identifiers() {
    use naso_compiler::lexer::token::TokenKind;

    let toks = naso_compiler::lexer::Lexer::tokenize("break continue");
    assert_eq!(
        toks.len(),
        2,
        "`break continue` must lex as exactly two tokens, got {toks:?}"
    );
    assert!(
        matches!(toks[0], TokenKind::Break),
        "`break` must lex as the keyword, not an identifier. Got {:?}. As an identifier \
         it produced Var(\"break\") and reached codegen as an unbound read of a \
         variable the author never wrote.",
        toks[0]
    );
    assert!(
        matches!(toks[1], TokenKind::Continue),
        "`continue` must lex as the keyword, not an identifier. Got {:?}",
        toks[1]
    );
}

/// The parser must build `ExprKind::Break`, not `ExprKind::Var("break")`.
///
/// A distinct check from the lexer one: the tokens can be right and the parser still
/// route them to the identifier path.
#[test]
fn the_parser_builds_a_break_node_rather_than_a_variable_read() {
    let src = "fn f(n: i64) -> i64 {\n  while n > 0 { break; }\n  n\n}\n";
    let program = naso_compiler::parser::parse_program(src).expect("parses");
    let json = serde_json::to_string(&program).expect("serialises");
    assert!(
        json.contains("\"Break\""),
        "the AST must contain a Break node.\n{json}"
    );
    assert!(
        !json.contains("\"Var\":{\"name\":\"break\""),
        "`break` must not become a variable read. This is the exact shape that made the \
         construct invisible: a program mentioning `break` typechecked and lowered, \
         then failed with a message about an undeclared VARIABLE.\n{json}"
    );
}

/// `break;` with no value, `break v;` with one, and `continue` must all lower to
/// their own PIR nodes rather than to a variable read.
#[test]
fn break_and_continue_lower_to_their_own_pir_nodes() {
    for (tag, src, needle, forbidden) in [
        (
            "bare break",
            "fn f() -> i64 { let mut i = 0; while true { i = i + 1; if i > 3 { break; } } i }\n",
            "Break",
            "Var(\"break\")",
        ),
        (
            "break with a value",
            "fn f() -> i64 { let mut i = 0; while true { i = i + 1; if i > 3 { break i; } } i }\n",
            "Break",
            "Var(\"break\")",
        ),
        (
            "continue",
            "fn f() -> i64 { let mut i = 0; while i < 9 { i = i + 1; if i > 2 { continue; } } i }\n",
            "Continue",
            "Var(\"continue\")",
        ),
    ] {
        let program = naso_compiler::parser::parse_program(src)
            .unwrap_or_else(|e| panic!("{tag}: must parse: {e:?}"));
        let pir = naso_compiler::lowering::lower_program(&program)
            .unwrap_or_else(|e| panic!("{tag}: must lower: {e}"));
        let dump = format!("{pir:?}");
        assert!(
            dump.contains(needle),
            "{tag}: expected a {needle} node in the PIR.\n{dump}"
        );
        assert!(
            !dump.contains(forbidden),
            "{tag}: {forbidden} must not appear -- that is the mis-parse this work \
             removed.\n{dump}"
        );
    }
}

/// The backend refuses `break`, and says what is missing rather than what is wrong.
///
/// Paired with `a_while_loop_runs_its_body_until_the_condition_becomes_false`, which
/// proves `while` itself works. Without that pairing, a backend refusing everything
/// would pass this suite.
#[test]
fn break_is_refused_by_the_backend_with_a_message_that_names_the_construct() {
    let msg = compile_error(
        "fn f(n: i64) -> i64 { let mut i = 0; while i < n { i = i + 1; if i > 3 { break; } } i }\n",
    );
    assert!(
        msg.contains("`break` is parsed and lowered, but this backend does not yet emit it"),
        "the diagnostic must name `break` and say the backend does not emit it, rather \
         than describing a missing variable: {msg}"
    );
    assert!(
        msg.contains("no loop stack is threaded"),
        "the diagnostic must name the actual missing mechanism, so a reader knows this \
         is a backend gap and not a bad program: {msg}"
    );
    assert!(
        msg.contains("Refused rather than compiled to a no-op")
            || msg.contains("rather than compiled to a no-op"),
        "the diagnostic must say why it refuses instead of approximating, since a \
         no-op `break` inside an `if` would exit no loop at all: {msg}"
    );
    assert!(
        !msg.contains("no allocation is known for it"),
        "the OLD failure named an undeclared variable, which sent readers looking for a \
         missing declaration. That wording must be gone: {msg}"
    );
}

/// The same for `continue`, whose wrong implementation is an infinite loop.
#[test]
fn continue_is_refused_by_the_backend_with_a_message_that_names_the_construct() {
    let msg = compile_error(
        "fn f(n: i64) -> i64 { let mut i = 0; while i < n { i = i + 1; if i > 2 { continue; } } i }\n",
    );
    assert!(
        msg.contains("`continue` is parsed and lowered, but this backend does not yet emit it"),
        "the diagnostic must name `continue` and the gap: {msg}"
    );
    assert!(
        msg.contains("would be an infinite loop"),
        "the diagnostic must say what the wrong implementation would do: {msg}"
    );
}

/// `break v;` is refused too, and specifically for carrying a value.
///
/// A `break` with a value is a strictly bigger job than a bare one: the loop has to
/// produce that value at its exit, so the loop's own type is constrained by it. That
/// is worth stating rather than folding into the bare-`break` message.
#[test]
fn a_break_carrying_a_value_is_also_refused_and_is_a_distinct_case() {
    let bare = compile_error(
        "fn f(n: i64) -> i64 { let mut i = 0; while i < n { i = i + 1; if i > 3 { break; } } i }\n",
    );
    let with_value = compile_error(
        "fn f(n: i64) -> i64 { let mut i = 0; while i < n { i = i + 1; if i > 3 { break i; } } i }\n",
    );
    assert!(
        !with_value.is_empty() && !bare.is_empty(),
        "both forms must be refused rather than one silently accepted"
    );
    // Both must be refused by the SAME mechanism, since neither is emitted. What must
    // not happen is `break i;` compiling while `break;` is refused -- that would mean
    // the value-carrying form silently dropped `i`, computing a different answer.
    assert!(
        with_value.contains("`break`") && bare.contains("`break`"),
        "both forms must be refused as `break`\n--- bare ---\n{bare}\n--- with value ---\n{with_value}"
    );
}

/// A plain `while` still compiles after the lexer gained two new keywords.
///
/// `break` and `continue` were added to the lexer at priority 3 alongside `while` and
/// `forall`. A new keyword is a new way to break every program that used the
/// identifier, so "the other things still work" is its own risk.
#[test]
fn adding_the_break_and_continue_keywords_did_not_disturb_plain_while_loops() {
    let src = "fn k(n: i64) -> i64 { let mut i = 0; while i < n { i = i + 1; } i }\n";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_k(long);
int main(void){ printf("%ld %ld\n", naso_k(0), naso_k(4)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 4",
        "a `while` loop with no `break` must be unaffected by the new keywords.\n--- IR ---\n{ir}"
    );
}

// ===== `for`: a counted loop, lowered onto the `while` above =====

/// `for i in n { ... }` runs its body exactly `n` times, with `i` taking 0..n.
///
/// Three inputs including 0 and 1, which catch the two easy wrong answers: a loop
/// that runs `n-1` times (missing the last iteration) and one that runs `n+1` (not
/// re-testing the guard). `sum_for(5)` is 0+1+2+3+4 = 10 and `sum_for(10)` is
/// 0+..+9 = 45.
#[test]
fn a_for_loop_runs_its_body_exactly_the_counted_number_of_times() {
    let src = "\
fn sum_for(n: i64) -> i64 {
    let mut t = 0;
    for i in n { t = t + i; }
    t
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_sum_for(long);
int main(void){ printf("%ld %ld %ld\n", naso_sum_for(0), naso_sum_for(5), naso_sum_for(10)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 10 45",
        "the body must run exactly n times over i = 0..n. A body that ran n-1 times \
         gives 6 for n=5 and 36 for n=10; one that ran n+1 times gives 15 and 55.\n--- IR ---\n{ir}"
    );
}

/// The loop variable must hold the CURRENT iteration's value, re-read every time.
///
/// This is the bug the first version of this change had. The counter was bound and
/// advanced correctly, but the loop variable was never bound, so reading it failed.
/// The tempting wrong fix -- binding `i` once before the loop -- is worse than a
/// refusal, because it makes every iteration contribute the same amount: `sum_for(5)`
/// would be 0 five times over, i.e. 0, which looks like a plausible empty-loop result.
///
/// So `sum_for(3)` must be 0+1+2 = 3, and a frozen `i` gives 0.
#[test]
fn the_loop_variable_holds_the_current_iterations_value() {
    let src = "\
fn sum_for(n: i64) -> i64 {
    let mut t = 0;
    for i in n { t = t + i; }
    t
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_sum_for(long);
int main(void){ printf("%ld %ld\n", naso_sum_for(3), naso_sum_for(7)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "3 21",
        "0+1+2 = 3 for n=3 and 0+1+..+6 = 21 for n=7. A loop variable frozen at its \
         initial value would give 0 for both, which is indistinguishable from an \
         empty loop.\n--- IR ---
{ir}"
    );
}

/// Two `for` loops with DIFFERENT variable names in one function both work.
///
/// I wrote this test to pin that the counter name is derived from the loop variable,
// so two loops cannot collide. It does NOT do that. Mutating the counter name to a
// single shared `__for_counter` still passes all 19 tests -- because each `for`
// re-binds the counter with a `Let`, and the guard RE-LOADS it each iteration, so a
// shared name is re-initialised before the second loop reads it.
//
// The derivation is sound, not lucky: the second `for` pushes `Let { name: counter }`
// and then the `While` guard loads that same slot. A shared name therefore still
// starts the second loop from 0. So there is no collision bug here, and claiming one
// would have put a false claim in the test suite. The behaviour is kept as coverage
// for two loops in one function, which is worth having on its own, and the comment
// says what it actually establishes.
#[test]
fn two_for_loops_in_one_function_each_start_from_their_own_counter() {
    let src = "\
fn two(n: i64) -> i64 {
    let mut a = 0;
    for i in n { a = a + i; }
    let mut b = 0;
    for j in n { b = b + j; }
    a + b
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_two(long);
int main(void){ printf("%ld %ld\n", naso_two(4), naso_two(6)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "12 30",
        "each loop sums 0..n-1, so for n=4 each gives 6 and together 12; for n=6 each \
         gives 0+1+2+3+4+5 = 15 and together 30. If the second loop started from the \
         first's final counter value these totals would differ.\n--- IR ---\n{ir}"
    );
}

/// `for` re-reads its bound, so a variable bound by an `if` before it is honoured.
///
/// The guard compares against a variable's value on every iteration. If the count
/// expression were evaluated once and frozen, a program that changes the bound
/// between two `for` loops would use the first loop's count for the second.
#[test]
fn a_for_loop_re_reads_its_bound_from_the_current_value() {
    let src = "\
fn f(n: i64) -> i64 {
    let mut t = 0;
    let mut k = n;
    for i in k { t = t + i; }
    k = 2;
    let mut u = 0;
    for i in k { u = u + i; }
    t + u
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_f(long);
int main(void){ printf("%ld\n", naso_f(3)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "4",
        "the first loop sums 0..2 = 3; k is then set to 2, so the second loop also \
         sums 0..1 = 1; together 4. A frozen count would make the second loop run \
         three times as well, giving 3 + 3 = 6.\n--- IR ---\n{ir}"
    );
}

/// A `return` inside a `for` body is refused, and says why.
///
/// Distinct from the `while` case: hoisting the return would skip the counter
/// increment, so the loop would never advance and would run forever even if the
/// return did not already exit.
#[test]
fn a_return_inside_a_for_body_is_refused_rather_than_compiled_to_the_wrong_value() {
    let msg = compile_error("fn f(n: i64) -> i64 { for i in n { return i; } return 0; }\n");
    assert!(
        msg.contains("a `return` inside a `for` body"),
        "the diagnostic must name the construct it refuses: {msg}"
    );
    assert!(
        msg.contains("the loop would never advance"),
        "the diagnostic must give the `for`-specific reason -- a skipped counter \
         increment makes the loop non-terminating, which is worse than in a `while`: {msg}"
    );
}
