//! End-to-end: a `forall` in a real `.naso` file must become a real, iterated LLVM loop.
//!
//! # The gap this file pins
//!
//! `LLVMModuleBuilder::build_module` walked `pir_module.statements` and emitted one
//! basic block per statement. A `forall i in 0..4 { .. }` is ONE PIR statement whose
//! domain is `nest(i)`, so that path executed the body exactly once: the sum below
//! came out as 0 rather than 6. No diagnostic, because the schedule tree that
//! recorded the iteration bounds WAS built -- by `lowering::loop_extraction` -- and
//! then discarded unread.
//!
//! `build_module` now lowers the schedule tree through
//! `codegen::llvm::schedule_lowering::lower_schedule_tree_into`, which is what turns a
//! `Band` into a preheader / header-with-phi / body / latch / exit loop nest.
//!
//! # Why this runs the code rather than reading the IR
//!
//! Same reasoning as `llvm_execution_test`: the IR text cannot distinguish "emitted a
//! loop" from "emitted a loop that computes the right thing". `icmp sle i64 %iv_val, 3`
//! appears in the IR whether the body adds the induction variable or not, and an
//! `add` in the body appears whether its result is stored or discarded. The only
//! check that cannot be satisfied by an arm that computes the right value and throws
//! it away is to run the module and read what the CPU produced.
//!
//! # Coverage of the path
//!
//! This file goes all the way from `.naso` TEXT: `parser::parse_program` ->
//! `lowering::lower_program` -> the production `LLVMModuleBuilder::build_module` ->
//! `llc` -> `clang` -> execution. Nothing here constructs a `PirModule` by hand, so
//! the front end's band construction is exercised too; a hand-built PIR could pass
//! while the real lowering produced no band at all.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;
use naso_compiler::ir::pir_types::PirModule;

/// LLVM toolchain prefix; falls back to bare names when the tools are only on `PATH`.
fn tool(name: &str) -> String {
    match std::env::var("LLVM_SYS_170_PREFIX") {
        Ok(prefix) if !prefix.is_empty() => format!("{prefix}/bin/{name}"),
        _ => name.to_string(),
    }
}

/// A temp directory for ONE case, removed on drop.
///
/// Unique per call, not per process: the harness runs tests as threads of one
/// process, so a shared directory would have cases overwrite each other's binaries.
struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-forall-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        CaseDir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The C driver: call the generated entry function, then print the result global.
///
/// The value is read from a global rather than the function's return value because
/// `naso_entry` returns void -- PIR has no return node, so the generated entry
/// function cannot hand a value back.
const HARNESS_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
extern void naso_entry(void);
extern int64_t result;
int main(void) {
    naso_entry();
    printf("%lld\n", (long long)result);
    return 0;
}
"#;

fn run(cmd: &mut Command, what: &str, ir: &str) -> Vec<u8> {
    let out = cmd.output().unwrap_or_else(|e| {
        panic!("could not start `{what}`: {e}. Is the LLVM 17 toolchain on PATH?")
    });
    assert!(
        out.status.success(),
        "{what} failed ({:?}):\n{}\n--- compiler-generated IR ---\n{ir}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Compile `ir`, link the C driver, run it, and return the printed integer.
fn execute(ir: &str) -> i64 {
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("harness.c");
    let exe = dir.path("case");

    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&csrc, HARNESS_C).expect("write C harness");
    run(
        Command::new(tool("llc"))
            .arg("-filetype=obj")
            .arg(&ll)
            .arg("-o")
            .arg(&obj),
        "llc",
        ir,
    );
    run(
        Command::new(tool("clang"))
            .arg(&csrc)
            .arg(&obj)
            .arg("-o")
            .arg(&exe),
        "clang",
        ir,
    );
    let out = run(&mut Command::new(&exe), "the compiled program", ir);
    String::from_utf8_lossy(&out)
        .trim()
        .parse::<i64>()
        .unwrap_or_else(|e| panic!("driver printed {e}; expected an integer.\n--- IR ---\n{ir}"))
}

/// Parse and lower `.naso` SOURCE TEXT to PIR, exactly as the CLI does.
fn lower_source(src: &str) -> PirModule {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e}"));
    naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower to PIR:\n{src}\nerror: {e}"))
}

/// Compile `src` through the production `build_module` path and RUN it.
///
/// A `result` global is added to the module before lowering, so the `.naso` source
/// can end with `result = total;` and the harness can read what the program computed.
fn build_and_run(src: &str) -> (i64, String) {
    let pir = lower_source(src);
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    let dest = builder
        .type_lowering()
        .int_type(naso_compiler::codegen::abi::IntWidth::I64);
    let global = builder.module().add_global(dest, None, "result");
    global.set_initializer(&dest.const_zero());
    builder.add_variable("result".to_string(), global.as_pointer_value(), dest.into());

    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("production build_module failed:\n{src}\nerror: {e}\n{pir:?}"));
    let ir = builder.module_to_string();
    (execute(&ir), ir)
}

/// The control: `src` sums `i` over `forall i in 0..4`, and the CPU must report 6.
///
/// 6 is the whole test. It is 0 + 1 + 2 + 3, so it requires FOUR iterations of a body
/// that reads the induction variable and accumulates into a binding created BEFORE
/// the loop. A statement-walking `build_module` produces three distinct wrong answers
/// here, and the assertion separates all of them:
///
///   * body never runs (schedule dropped the statement) -> 0, the global's initializer;
///   * body runs once (the old loop-once behaviour)     -> 0 + 0 = 0, because the
///     induction variable `i` is unbound on the first iteration and reads as zero;
///   * body runs but `i` is unbound every iteration    -> 0;
///   * inclusive/exclusive bound mix-up drops the last  -> 0 + 1 + 2 = 3.
///
/// Only 0 + 1 + 2 + 3 survives all of them, so this cannot be satisfied by a
/// backend that emits *a* loop, or by one that emits the right instructions and
/// discards the result.
#[test]
fn a_forall_in_naso_source_sums_every_iteration_on_the_cpu() {
    let src = "\
fn sum_to_4() -> i64 {
    let mut total = 0;
    forall i in 0..4 {
        total = total + i;
    }
    result = total;
}
";
    let (got, ir) = build_and_run(src);
    assert_eq!(
        got, 6,
        "forall i in 0..4 {{ total = total + i }} must sum 0+1+2+3 = 6 on the CPU. \
         Got {got}.\n--- IR ---\n{ir}"
    );
}

/// The trip count itself, independent of what the body computes.
///
/// This separates "the loop iterated four times" from "the body produced 6". If the
/// loop ran three times, or once, or not at all, the counter disagrees even if some
/// other arithmetic happened to land on 6.
#[test]
fn a_forall_in_naso_source_iterates_exactly_its_trip_count() {
    let src = "\
fn count_iterations() -> i64 {
    let mut n = 0;
    forall i in 0..4 {
        n = n + 1;
    }
    result = n;
}
";
    let (got, ir) = build_and_run(src);
    assert_eq!(
        got, 4,
        "the band body must run once per iteration: 4 iterations of `n = n + 1`. \
         Got {got}.\n--- IR ---\n{ir}"
    );
}

/// A non-zero lower bound must shift the loop, not just bound from zero.
///
/// `forall i in 2..5` sums 2 + 3 + 4 = 9. If the lower bound were dropped the sum
/// would be 0 + 1 + 2 + 3 = 6, and if the bound were treated as inclusive at both
/// ends it would be 2 + 3 + 4 + 5 = 14. 9 is the only value consistent with the
/// source.
#[test]
fn a_forall_with_a_nonzero_lower_bound_starts_at_that_bound() {
    let src = "\
fn sum_from_two() -> i64 {
    let mut total = 0;
    forall i in 2..5 {
        total = total + i;
    }
    result = total;
}
";
    let (got, ir) = build_and_run(src);
    assert_eq!(
        got, 9,
        "forall i in 2..5 must sum 2+3+4 = 9. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// A statement in `statements` that no `Domain` node in the schedule covers must be
/// REFUSED, naming the statement.
///
/// The schedule tree is now the only thing that emits a statement, so an uncovered
/// statement compiles to nothing. Emitting it anyway would run the program twice;
/// skipping it silently would drop work the program asked for. Both are wrong, and
/// the honest answer is a diagnostic.
#[test]
fn a_statement_the_schedule_tree_does_not_cover_is_refused() {
    use naso_compiler::ast::{Mutability, Quantity};
    use naso_compiler::ir::affine_domain::AffineDomain;
    use naso_compiler::ir::pir_types::{PirExpr, PirStatement};
    use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    // Statement 0 IS scheduled; statement 1 is not.
    let stmt = |id: usize| PirStatement {
        id: StmtId(id),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::IntLit(id as i64),
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    let pir = PirModule {
        statements: vec![stmt(0), stmt(1)],
        schedule: ScheduleTree::new(
            ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
            vec![],
        ),
        ..Default::default()
    };

    let err = builder
        .build_module(&pir)
        .expect_err("a statement no schedule node covers must be refused, not dropped");
    let msg = err.to_string();
    assert!(
        msg.contains("S1"),
        "the diagnostic must name the uncovered statement: {msg}"
    );
}

/// A band whose bounds cannot be resolved must be REFUSED, not run once.
///
/// `forall i in 0..n` with a symbolic `n` records only `i >= 0`, so no trip count can
/// be computed. The alternative -- emitting the body as straight-line code -- is the
/// exact silent wrong answer this change set out to remove, so it is an error.
///
/// The refusal is asserted on the diagnostic naming the cause, because a test that
/// only checked "it failed" would also pass if the build failed for an unrelated
/// reason.
#[test]
fn a_forall_with_a_symbolic_bound_is_refused_rather_than_run_once() {
    let src = "\
fn sum_to(n: i64) -> i64 {
    let mut total = 0;
    forall i in 0..n {
        total = total + i;
    }
    result = total;
}
";
    let pir = lower_source(src);
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    let err = builder
        .build_module(&pir)
        .expect_err("a symbolic loop bound has no trip count and must not compile to a single run");
    let msg = err.to_string();
    assert!(
        msg.contains("loop bounds"),
        "the diagnostic must say the bounds are the problem: {msg}"
    );
}

/// A nested `forall` must NEST, not sequence or multiply.
///
/// Two separate defects made a 3x2 nest wrong, and both produced IR that looked like
/// a perfectly ordinary loop nest:
///
/// 1. `emit_sequential_band` iterated the band's bounds emitting SIBLING loops and ran
///    the child in each, so the inner loop ran once in total rather than once per outer
///    iteration. It now recurses, nesting each level in the previous.
/// 2. `extract_bounds` took the bounds for EVERY iterator dimension of EVERY band
///    member. A 2-dimensional band has 2 members, each naming a 2-dimensional domain,
///    so that yielded 4 bounds instead of 2 and built a 4-deep nest -- 36 iterations
///    instead of 6.
///
/// 6 is reachable by neither defect: sequencing gave 2, and the multiplied bound gave
/// 36, so this assertion distinguishes both from correct behaviour.
#[test]
fn a_nested_forall_nests_rather_than_sequences() {
    let src = "\
fn nested() -> i64 {
    let mut total = 0;
    forall i in 0..3 {
        forall j in 0..2 {
            total = total + 1;
        }
    }
    result = total;
}
";
    let (got, ir) = build_and_run(src);
    assert_eq!(
        got, 6,
        "a 3x2 nest must accumulate 6: 2 from sequencing, 36 from multiplied bounds, \
         and only correct nesting gives 6. Got {got}.\n--- IR ---\n{ir}"
    );
}
