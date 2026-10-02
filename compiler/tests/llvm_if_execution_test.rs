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

/// An `if` arm may be a bare block whose tail is the value.
///
/// Multi-statement arms with a `let` are a separate, still-unsupported thing --
/// a `let` inside an arm has no scope of its own, and the PIR records the
/// binding without an allocation for it. See
/// `a_let_inside_an_if_arm_has_no_scope_and_is_refused` for that case. What is
/// pinned here is that a tail expression is the arm's value, which is the part
/// `if` itself needed.
#[test]
fn an_if_arm_yields_its_blocks_tail_expression() {
    let src = "\
fn pick(x: f32) -> f32 {
    if x > 0.0 { 1.0 } else { -1.0 }
}
";
    let ir = build_ir(src);
    assert_eq!(
        run(&ir, &pick_driver()),
        "1.0 -1.0 -1.0 1.0",
        "each arm must yield the value of its block's tail.\n--- IR ---\n{ir}"
    );
}

/// A `let` INSIDE an `if` arm is refused, because the arm has no scope.
///
/// This is a real limitation and it is stated rather than worked around. The
/// binding is recorded in the PIR with no allocation, so reading it would be an
/// unbound read -- and an unbound read in this backend is refused deliberately
/// rather than returning zero, because an uninitialised LLVM value is also zero
/// and the result would be indistinguishable from a real computation.
///
/// The refusal names the variable, so the diagnostic is actionable rather than
/// "unsupported".
#[test]
fn a_let_inside_an_if_arm_has_no_scope_and_is_refused() {
    let msg = compile_error(
        "fn pick(x: f32) -> f32 { if x > 0.0 { let a = 10.0; a + 1.0 } else { -1.0 } }\n",
    );
    assert!(
        msg.contains("`a`") && msg.contains("no allocation is known for it"),
        "the refusal must name the unbound binding, so the message points at the \
         actual construct: {msg}"
    );
}

/// An `if` inside a loop, assigning to a variable declared OUTSIDE the loop.
///
/// This is the case that proves the `if` body does not merely produce a value.
/// Each iteration must actually choose which assignment runs. i = 0,1,2 take the
/// `else` and add 1 (three times); i = 3,4,5 take the `then` and add themselves
/// (3 + 4 + 5 = 12); the total is 15.
///
/// If the `then` ran unconditionally the answer would be 21, and if neither arm
/// ran it would be 0, so 15 distinguishes "the condition controls the loop".
#[test]
fn an_if_inside_a_loop_controls_which_assignment_runs_on_each_iteration() {
    let src = "\
fn count_pos(n: i64) -> i64 {
    let mut total = 0;
    forall i in 0..n {
        if i > 2 { total = total + i; } else { total = total + 1; }
    }
    total
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_count_pos(long);
int main(void){ printf("%ld\n", naso_count_pos(6)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "15",
        "the loop must take the else-arm for i=0,1,2 (1+1+1) and the then-arm for \
         i=3,4,5 (3+4+5): 3 + 12 = 15.\n--- IR ---\n{ir}"
    );
}

/// The same loop at a different trip count, so a hard-coded answer cannot pass.
#[test]
fn the_loop_body_selects_its_branch_at_every_trip_count() {
    let src = "\
fn count_pos(n: i64) -> i64 {
    let mut total = 0;
    forall i in 0..n {
        if i > 2 { total = total + i; } else { total = total + 1; }
    }
    total
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_count_pos(long);
int main(void){ printf("%ld %ld %ld\n", naso_count_pos(0), naso_count_pos(3), naso_count_pos(8)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 3 28",
        "n=0 does nothing (0); n=3 adds 1 three times (3); n=8 adds 1 for i=0,1,2 \
         and i itself for i=3..8, which is 3 + (3+4+5+6+7) = 3 + 25 = 28. Three \
         different trip counts, so no hard-coded answer can satisfy this.\n--- IR ---\n{ir}"
    );
}

/// A `return` INSIDE an `if` is refused, and the refusal says why.
///
/// This is the bug that made `if` dangerous rather than merely absent. Hoisting
/// the inner return out of the block would skip the statements after the `if`,
/// so the alternative is a function that computes a condition and then ignores
/// it. Refusing names the construct; the previous behaviour produced a function
/// that always returned -1.0 with no diagnostic at all.
#[test]
fn a_return_inside_an_if_is_refused_rather_than_compiled_to_the_wrong_value() {
    let msg = compile_error("fn p(x: f32) -> f32 { if x > 0.0 { return 1.0; } return -1.0; }\n");
    assert!(
        msg.contains("`return` inside an `if`"),
        "the diagnostic must name the construct it refuses: {msg}"
    );
    assert!(
        msg.contains("Hoisting it would skip whatever follows"),
        "the diagnostic must say WHY it is refused rather than merely that it is: {msg}"
    );
}

/// A `return` inside an `if` inside a LOOP is refused too, and for the same reason.
///
/// In a loop the return may be followed by further iterations, so hoisting it
/// skips not just the statements after the `if` but the rest of the loop. This
/// is the case where a "just hoist it" fix would be most wrong.
#[test]
fn a_return_inside_an_if_inside_a_loop_is_also_refused() {
    let msg = compile_error(
        "fn f(n: i64) -> i64 {
    forall i in 0..n {
        if i > 2 { return i; }
    }
    return 0;
}
",
    );
    assert!(
        msg.contains("`return` inside an `if`"),
        "the same refusal must apply inside a loop: {msg}"
    );
}

/// An `if` with no `else` yields a zero of the THEN-BRANCH's type, not an i32.
///
/// Lowering substitutes `IntLit(0)` for a missing `else`. Phi'ing that against a
/// `double` made LLVM reject the module outright:
///
///     PHI node operands are not the same type as the result!
///       %if_phi = phi double [ 1.000000e+00, %then ], [ 0, %else ]
///
/// The zero is now CONVERTED to the then-type. `sitofp` on zero is exactly zero
/// in double, so the value is right for every type rather than only the ones
/// that happen to work.
#[test]
fn an_if_without_an_else_yields_a_zero_of_the_value_type() {
    let src = "\
fn sgn(x: f32) -> f32 {
    if x > 0.0 { 5.0 }
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
double naso_sgn(double);
int main(void){ printf("%.1f %.1f\n", naso_sgn(2.0), naso_sgn(-1.0)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "5.0 0.0",
        "a missing `else` must contribute a typed zero: 5.0 when the condition \
         holds, 0.0 otherwise.\n--- IR ---\n{ir}"
    );
}

/// An integer-valued `if` with no `else` -- the zero must be an i64, not an i32.
#[test]
fn an_if_without_an_else_on_integers_also_agrees_with_the_value_type() {
    let src = "\
fn mag(x: i64) -> i64 {
    if x > 0 { 7 }
}
";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
long naso_mag(long);
int main(void){ printf("%ld %ld\n", naso_mag(1), naso_mag(-1)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "7 0",
        "an integer `if` with no `else` must also yield zero: 7 when positive, \
         0 otherwise.\n--- IR ---\n{ir}"
    );
}
