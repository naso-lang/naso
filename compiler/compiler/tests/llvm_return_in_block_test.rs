//! `if` in the LLVM backend, proven by EXECUTION.
//!
//! # Why these tests exist at all
//!
//! Naso had an `if` that compiled to nothing. The chain was:
//!
//! 1. The parser built a correct `ExprKind::If`. Verified by `naso parse` on
//!    `fn pick(x: f32) -> f32 { if x > 0.0 { 1.0 } else { -1.0 } }` -- an `If`
//!    with a `Binary` condition and two `Block` arms, each with a tail.
//! 2. `PirExpr::If` existed, and the LLVM backend had a `then`/`else`/`phi`
//!    emitter for it. Nothing ever reached it.
//! 3. `lower_expr` had NO `ExprKind::Block` arm. An `if` arm is a block, so
//!    lowering failed with `Unsupported construct: Block(...)`.
//! 4. Fixing (3) exposed the next bug: lowering substitutes `IntLit(0)` for a
//!    missing `else`, and phi'ing that against a `double` made LLVM reject the
//!    module -- `PHI node operands are not the same type as the result!`.
//! 5. Fixing (4) exposed the worst one. A `return` INSIDE an `if` set
//!    `pending_return_stmt` to a statement that the new block arm then lifted
//!    out and dropped, so `if x > 0.0 { return 1.0; } return -1.0;` emitted an
//!    EMPTY `then` block, a discarded phi, and a function that ALWAYS returned
//!    -1.0. The condition was computed correctly and controlled nothing.
//!
//! Step 5 is why these are execution tests. Every one of them asserts a value,
//! and a value is the only thing that distinguishes "the condition controls
//! this" from "the condition was computed and thrown away".
//!
//! Read the module doc of `llvm_execution_test.rs` for the harness contract:
//! IR text is not evidence that a computation happened, because LLVM
//! constant-folds instructions into the following store.

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
        let p = std::env::temp_dir().join(format!("naso-if-{}-{}-{}", tag, std::process::id(), n));
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
/// NOT a hardcoded path. An earlier version of this file used
/// `/home/linuxbrew/.linuxbrew/opt/llvm@17/bin/{name}` and passed locally on this
/// container while failing on every CI run, because CI installs LLVM system-wide
/// and puts it on `PATH`. Preferring `PATH` and falling back to the Homebrew
/// prefix works in both places, so the test tests the COMPILER rather than the
/// layout of one machine.
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

fn lower_source(src: &str) -> naso_compiler::ir::PirModule {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}"));
    naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\n{src}\nerror: {e:?}"))
}

fn build_ir(src: &str) -> String {
    let pir = lower_source(src);
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed:\n{src}\nerror: {e}\n{pir:?}"));
    builder.module().to_string()
}

/// Compile the IR with `llc`, link a C driver, run it, and return what it printed.
///
/// `llc -relocation-model=pic` is required because the ABI guard puts C string
/// literals in the module.
fn run(ir: &str, driver: &str) -> String {
    let dir = CaseDir::new("run");
    let ll = dir.join("m.ll");
    let obj = dir.join("m.o");
    let csrc = dir.join("d.c");
    let exe = dir.join("prog");
    std::fs::write(&ll, ir).expect("write ll");
    std::fs::write(&csrc, driver).expect("write driver");

    let llc = Command::new(tool("llc"))
        .args(["-filetype=obj", "-relocation-model=pic"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("run llc");
    assert!(
        llc.status.success(),
        "llc failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&llc.stderr)
    );

    // `-lm` because a Naso program calling `round`/`floor`/`ceil` lowers to libm.
    let clang = Command::new(tool("clang"))
        .arg("-O0")
        .arg(&obj)
        .arg(&csrc)
        .arg("-lm")
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run clang");
    assert!(
        clang.status.success(),
        "clang link failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&clang.stderr)
    );

    let out = Command::new(&exe).output().expect("run exe");
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

/// Both arms of the conditional, so a mutant that swaps them cannot pass.
fn pick_driver() -> String {
    r#"
#include <stdio.h>
double naso_pick(double);
int main(void){
  printf("%.1f %.1f %.1f %.1f\n",
         naso_pick(2.0), naso_pick(-3.0), naso_pick(0.0), naso_pick(0.5));
  return 0;
}
"#
    .to_string()
}

/// `pick(x)` is `1.0` when `x > 0.0` and `-1.0` otherwise.
///
/// The four inputs cover both branches and the boundary: 2.0 and 0.5 are
/// positive, -3.0 is not, and 0.0 is exactly the boundary where `>` is false.
/// A `>=` bug would show up only in the 0.0 case, which is why it is here.
///
/// A phi-node swap gives `[-1.0 1.0 -1.0 -1.0]`, and inverting the condition
/// gives `[-1.0 1.0 1.0 -1.0]`. Neither matches `1.0 -1.0 -1.0 1.0`.
#[test]
fn an_if_expression_selects_a_branch_by_the_value_of_its_condition() {
    let src = "\
fn pick(x: f32) -> f32 {
    if x > 0.0 { 1.0 } else { -1.0 }
}
";
    let ir = build_ir(src);
    let got = run(&ir, &pick_driver());
    assert_eq!(
        got, "1.0 -1.0 -1.0 1.0",
        "an `if` must select by its condition. `pick(2.0)=1.0`, `pick(-3.0)=-1.0`, \
         `pick(0.0)=-1.0` because `0.0 > 0.0` is FALSE, `pick(0.5)=1.0`.\n--- IR ---\n{ir}"
    );
}

/// Compile and run a one-argument function at three inputs.
fn run3(src: &str, sym: &str, inputs: [i64; 3]) -> String {
    let ir = build_ir(src);
    let calls = inputs
        .iter()
        .map(|n| format!("naso_{sym}({n})"))
        .collect::<Vec<_>>()
        .join(", ");
    let driver = format!(
        "#include <stdio.h>\nlong naso_{sym}(long);\n\
         int main(void){{ printf(\"%ld %ld %ld\\n\", {calls}); return 0; }}\n"
    );
    run(&ir, &driver)
}

/// Compile and run an `f32`-returning function at three inputs.
///
/// A separate helper rather than a format argument: the integer runner prints `%ld`
/// and would hand back the raw bit pattern of a float, which is how a coercion bug
/// hides from an integer-only test.
fn run3f(src: &str, sym: &str, inputs: [i64; 3]) -> String {
    let ir = build_ir(src);
    let calls = inputs
        .iter()
        .map(|n| format!("naso_{sym}({n})"))
        .collect::<Vec<_>>()
        .join(", ");
    let driver = format!(
        "#include <stdio.h>\ndouble naso_{sym}(long);\n\
         int main(void){{ printf(\"%f %f %f\\n\", {calls}); return 0; }}\n"
    );
    run(&ir, &driver)
}

/// An early return must leave from the branch that fires.
///
/// This is the case the whole change exists for. Hoisting made it return -1 for
/// EVERY input: the condition was computed correctly and controlled nothing.
#[test]
fn an_early_return_leaves_from_the_branch_that_fires() {
    let src = "\
fn guard(n: i64) -> i64 {
    if n > 0 { return 7; }
    return -1;
}
";
    assert_eq!(
        run3(src, "guard", [-1, 0, 5]),
        "-1 -1 7",
        "only the positive input may return 7. -1 -1 7 is a hoisted return, which is \
         the exact bug this change fixes: the condition was evaluated and controlled \
         nothing."
    );
}

/// Both arms returning means the statement after the `if` is unreachable.
///
/// `return 99` cannot run. It is kept in the source to prove the compiler does not
/// let it leak into the result -- a hoisting bug would return 99 for every input.
#[test]
fn returns_in_both_arms_take_precedence_over_a_later_return() {
    let src = "\
fn both(n: i64) -> i64 {
    if n > 0 { return 1; } else { return 2; }
    return 99;
}
";
    assert_eq!(
        run3(src, "both", [5, -1, 0]),
        "1 2 2",
        "each arm returns its own value. 99 anywhere means the later return escaped \
         the branches."
    );
}

/// A nested `return` must stay ahead of the outer arm's own.
///
/// `n = 9` reaches the inner `return 1`. If the arm's trailing `return 2` were
/// reordered ahead of the inner `if`, the inner return would be unreachable and this
/// input would give 2 -- a silent wrong answer that verifies.
#[test]
fn a_nested_return_is_not_reordered_behind_the_outer_one() {
    let src = "\
fn nested(n: i64) -> i64 {
    if n > 0 {
        if n > 5 { return 1; }
        return 2;
    }
    return 3;
}
";
    assert_eq!(
        run3(src, "nested", [-1, 3, 9]),
        "3 2 1",
        "3 inputs, three distinct exits. `9` must be 1: it reaches the INNER return. \
         A `2` there means the arm's trailing return was hoisted above the inner `if`, \
         which is a reordering bug and not a style question."
    );
}

/// A `return` inside a loop body leaves the FUNCTION, not just the loop.
///
/// `return 100` must skip the `20` after the loop for n = 9, while n = 0 falls through
/// to it. A `return` lowered as a loop `break` would give 20 for both.
#[test]
fn a_return_inside_a_loop_body_exits_the_function() {
    let src = "\
fn esc(n: i64) -> i64 {
    let mut i = 0;
    while i < n { i = i + 1; if i > 2 { return 100; } }
    20
}
";
    assert_eq!(
        run3(src, "esc", [0, 3, 9]),
        "20 100 100",
        "n=0 never enters the loop and returns 20. n=3 and n=9 both reach `return \
         100`, which must leave the FUNCTION. 20 for those would mean the return was \
         treated as a loop exit."
    );
}

/// A `return` inside a counted `for` body behaves the same way.
#[test]
fn a_return_inside_a_counted_loop_body_exits_the_function() {
    let src = "\
fn esc2(n: i64) -> i64 {
    for i in n { if i > 1 { return 55; } }
    20
}
";
    assert_eq!(
        run3(src, "esc2", [0, 1, 5]),
        "20 20 55",
        "n=5 reaches i=2 and returns 55. 20 there would mean the return became a \
         `break` -- it must leave the function, not just the loop."
    );
}

/// `if` as a value expression must keep working.
///
/// The regression guard on everything above. `return` support changed how a block's
/// parts are collected, and a value-yielding `if` depends on the same collection. A
/// pass that dropped the tails would give 0 for every input here.
#[test]
fn an_if_used_as_a_value_still_yields_its_branch() {
    let src = "\
fn pick(n: i64) -> i64 {
    let x = if n > 0 { 1 } else { 2 };
    x
}
";
    assert_eq!(
        run3(src, "pick", [-1, 0, 5]),
        "2 2 1",
        "0 for every input would mean the arms' values were lost when `return` \
         handling was added to the same block-lowering path."
    );
}

/// A `return` followed by statements in the same arm is refused.
///
/// Those statements cannot run. Emitting them would compile to a different program
/// rather than a visibly wrong one, so the refusal must name the problem.
#[test]
fn a_return_followed_by_statements_in_the_same_arm_is_refused() {
    let msg = compile_error("fn f(n: i64) -> i64 { if n > 0 { return 1; n = 5; } return -1; }\n");
    assert!(
        msg.contains("cannot run"),
        "the refusal must say the following statements are unreachable: {msg}"
    );
}

/// A `return` with NO trailing semicolon must stay in place.
///
/// The parser treats a statement without a terminating `;` as a block's TAIL
/// EXPRESSION -- the block's value. `return` produces no value, so classifying it
/// as a tail appended it AFTER the block's statements, reordering the block.
///
/// This test exists because the mutation check found that removing `Return` from
/// `is_control_flow_stmt` -- which is exactly that classification bug -- did NOT fail
/// any other test in this file. Every other arm here writes `return x;` with a
/// semicolon, so the tail path was never exercised. `break` and `continue` are
/// control flow in the same way and belong in that list for the same reason.
#[test]
fn a_return_without_a_trailing_semicolon_is_not_treated_as_a_block_tail() {
    let src = "\
fn nosemi(n: i64) -> i64 {
    if n > 0 {
        if n > 5 { return 1 }
        return 2
    }
    return 3
}
";
    assert_eq!(
        run3(src, "nosemi", [-1, 3, 9]),
        "3 2 1",
        "`9` must reach the inner `return 1`. If the arm's `return 2` were read as the \
         block's tail expression it would be appended after the inner `if`, making \
         `return 1` unreachable and yielding 2."
    );
}

/// A `return` inside an affine `forall` band exits the function.
///
/// This case was REFUSED for the same reason `if` was, and the refusal has the same
/// bug: a `return` in a band was hoisted out of it. For an affine band that is
/// additionally visible, because the band is a real emitted loop -- a hoisted return
/// would run on the first iteration whether or not the condition held.
///
/// These expectations are worth reading twice. The band `0..3` is {0, 1, 2}; no
/// element exceeds 2, so the guard never fires and the result is the trailing `0`.
/// Only `n = 9` reaches an element that does.
#[test]
fn a_return_inside_an_affine_band_exits_at_the_first_matching_iteration() {
    let src = "\
fn band(n: i64) -> i64 {
    forall i in 0..n { if i > 2 { return i; } }
    0
}
";
    assert_eq!(
        run3(src, "band", [3, 2, 9]),
        "0 0 3",
        "`n=9` reaches i=3, the first element above 2, and returns it. n=3 and n=2 \
         reach no element above 2 and fall through to 0 -- those bands stop at 2 and \
         1, so the guard never holds. A 3 for n=3 would mean the return fired on a \
         condition that was false."
    );
}

/// The band must stop at the first hit, not keep iterating and return the last.
///
/// `77` is returned from inside the band; `7` trails it. A band that continued past
/// the hit would still return 77 here, so the discriminator is the n=9 case above
/// returning the FIRST match (3) rather than the last (8).
#[test]
fn an_affine_band_does_not_continue_past_an_early_return() {
    let src = "\
fn band2(n: i64) -> i64 {
    forall i in 0..n { if i > 6 { return 77; } }
    7
}
";
    assert_eq!(
        run3(src, "band2", [3, 5, 9]),
        "7 7 77",
        "the band returns 77 when it hits"
    );
}

/// The band must still fall through when the guard never holds.
#[test]
fn an_affine_band_with_no_match_returns_the_trailing_value() {
    let src = "\
fn band3(n: i64) -> i64 {
    forall i in 0..n { if i > 99 { return i; } }
    0
}
";
    assert_eq!(
        run3(src, "band3", [1, 5, 200]),
        "0 0 100",
        "n=1 and n=5 reach no element above 99 and fall through to 0. n=200 reaches \
         i=100, the first element above 99, and returns it -- so the band does return \
         from inside, and a 0 there would mean the band never returned at all."
    );
}

/// A returned value is coerced to the function's REAL declared return type.
///
/// The coercion path is the dangerous one: an `i64` returned from an `f32` function
/// through an unchecked path would be REINTERPRETED rather than converted, and a
/// wrong number would leave through a signature that looks correct.
///
/// This test exists because the mutation check found a plausible mutant that
/// SKIPPED coercion inside an affine band and passed all 12 tests in this file. Every
/// other case returns a value whose type already matches its function's, so coercion
/// was a no-op and removing it changed nothing. This one does not match.
///
/// The emitted bit pattern for the correct answer is `0x41CFF80000000000`. A
/// REINTERPRETATION of the i64 rather than a CONVERSION would emit the raw integer
/// bits as a float, which is a wildly different number rather than a subtle drift.
#[test]
fn a_return_inside_a_band_coerces_to_the_declared_return_type() {
    let src = "\
fn coerce(n: i64) -> f32 {
    forall i in 0..n { if i > 1 { return 1072693248; } }
    0.0
}
";
    // n=2 -> band {0,1}: i>1 never holds, falls through to 0.0.
    // n=3 -> band {0,1,2}: i=2 returns 1072693248 converted to f32.
    let out = run3f(src, "coerce", [1, 2, 3]);
    assert_eq!(
        out, "0.000000 0.000000 1072693248.000000",
        "the band's `return` must CONVERT i64 to the f32 the function declares. \
         A reinterpretation would yield a huge float instead, and 0.0 for n=3 would \
         mean the band never returned at all."
    );
}
