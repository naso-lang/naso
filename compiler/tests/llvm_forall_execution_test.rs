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
///
/// `naso_entry` takes one `i64` per symbolic constant the schedule names (see
/// `schedule_parameters`), and `values` supplies what to pass. A module with no
/// symbolic bounds has the older `void()` signature, so the driver has to be told
/// which shape to declare; calling the wrong one is a link error, not a silent wrong
/// answer.
fn harness_c(names: &[&str], values: &[i64]) -> String {
    assert_eq!(
        names.len(),
        values.len(),
        "one driver argument per symbolic constant"
    );
    let decl = names
        .iter()
        .map(|n| format!("int64_t {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    // Each symbolic constant is a `static const` the driver passes by value, so the
    // generated function receives exactly the value the test chose.
    let defs = names
        .iter()
        .zip(values)
        .map(|(n, v)| format!("static const int64_t {n}_v = {v};"))
        .collect::<Vec<_>>()
        .join("\n    ");
    let args = names
        .iter()
        .map(|n| format!("{n}_v"))
        .collect::<Vec<_>>()
        .join(", ");
    let call = if args.is_empty() {
        "naso_entry()".to_string()
    } else {
        format!("naso_entry({args})")
    };
    format!(
        r#"#include <stdint.h>
#include <stdio.h>
extern void naso_entry({decl});
extern int64_t result;
int main(void) {{
    {defs}
    {call};
    printf("%lld\n", (long long)result);
    return 0;
}}
"#
    )
}

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
///
/// `param_values` supplies the `i64` argument for each symbolic constant, in the
/// order `schedule_parameters` reports them.
fn execute(ir: &str, param_names: &[String], param_values: &[i64]) -> i64 {
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("harness.c");
    let exe = dir.path("case");

    std::fs::write(&ll, ir).expect("write IR");
    let decl: Vec<&str> = param_names.iter().map(String::as_str).collect();
    std::fs::write(&csrc, harness_c(&decl, param_values)).expect("write C harness");
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
///
/// `param_values` are the values to pass for the symbolic constants the schedule
/// names, in `schedule_parameters` order. Passing the wrong count is a panic here
/// rather than a link error deep in the toolchain.
fn build_and_run_with(src: &str, param_values: &[i64]) -> (i64, String) {
    let pir = lower_source(src);
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    let dest = builder
        .type_lowering()
        .int_type(naso_compiler::codegen::abi::IntWidth::I64);
    let global = builder.module().add_global(dest, None, "result");
    global.set_initializer(&dest.const_zero());
    builder.add_variable("result".to_string(), global.as_pointer_value(), dest.into());

    let param_names =
        naso_compiler::codegen::llvm::schedule_lowering::schedule_parameters(&pir.schedule);
    assert_eq!(
        param_names.len(),
        param_values.len(),
        "the driver must pass one value per symbolic constant the schedule names: {param_names:?}"
    );

    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("production build_module failed:\n{src}\nerror: {e}\n{pir:?}"));
    let ir = builder.module_to_string();
    (execute(&ir, &param_names, param_values), ir)
}

/// `build_and_run` for a program with no symbolic bounds.
fn build_and_run(src: &str) -> (i64, String) {
    build_and_run_with(src, &[])
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

/// TASK 2: a symbolic loop bound must compile to a loop whose trip count comes from
/// the parameter AT RUNTIME.
///
/// This REPLACES `a_forall_with_a_symbolic_bound_is_refused_rather_than_run_once`,
/// which asserted the interim behaviour: `forall i in 0..n` recorded only `i >= 0`,
/// no trip count could be computed, and the band was refused. That was the right
/// interim answer -- a symbolic bound must not be invented, and a wrong trip count is
/// a wrong program -- but it left the feature missing.
///
/// What the bound's value now IS, and ISN'T:
///
///   * The domain carries `n` as a NAMED parameter dimension, so `iterator_bounds`
///     returns `i <= n - 1` as an affine expression rather than a constant.
///   * The value does NOT come from the source text. `module_builder` gives the entry
///     function one real `i64` parameter per named symbolic constant, and the driver's
///     C caller supplies it. That is the only honest source: the front end has no
///     caller, so inventing a value would compile a different program than it was
///     told.
///   * Nothing is converted on the way in. The argument IS the i64 the bound
///     arithmetic uses.
///
/// The trip count therefore depends on what the caller passes, and the test proves it
/// by running the SAME source twice with different values. A backend that hardcoded a
/// constant bound, or that ignored the parameter, gives the same answer for both and
/// fails the second.
#[test]
fn a_symbolic_loop_bound_takes_its_trip_count_from_the_caller() {
    let src = "\
fn sum_to(n: i64) -> i64 {
    let mut total = 0;
    forall i in 0..n {
        total = total + i;
    }
    result = total;
}
";
    // 0..5 sums 0+1+2+3+4 = 10; 0..4 sums 6; 0..1 sums 0.
    for (n, want) in [(1i64, 0i64), (4, 6), (5, 10), (9, 36)] {
        let (got, ir) = build_and_run_with(src, &[n]);
        assert_eq!(
            got, want,
            "forall i in 0..n with n={n} must sum {want} on the CPU, taking the trip \
             count from the function argument. Got {got}.\n--- IR ---\n{ir}"
        );
    }
}

/// TASK 2, negative direction: a symbolic bound is a RUNTIME value, so the count
/// follows the caller down to zero and up past any constant the source mentions.
///
/// `0..0` must run the body zero times (0), not once (which would read an unbound `i`
/// and add 0, coincidentally also 0 -- hence the second case at 1, where one iteration
/// is 0 + 0 = 0 but a loop that always ran once would be indistinguishable only if the
/// body ignored `i`). `0..1` sums 0, and a loop that ran twice would add 1 as well and
/// give 1.
#[test]
fn a_symbolic_loop_bound_runs_zero_times_when_the_caller_passes_zero() {
    let src = "\
fn count_up(n: i64) -> i64 {
    let mut seen = 0;
    forall i in 0..n {
        seen = seen + 1;
    }
    result = seen;
}
";
    for (n, want) in [(0i64, 0i64), (1, 1), (7, 7), (33, 33)] {
        let (got, ir) = build_and_run_with(src, &[n]);
        assert_eq!(
            got, want,
            "the body must run exactly n times with n={n}, so {want} accumulations. \
             Got {got}.\n--- IR ---\n{ir}"
        );
    }
}

/// TASK 1: a loop-carried dependence must come out SEQUENTIAL, not tagged
/// `omp parallel for`.
///
/// `total = total + i` is a read-modify-write of one scalar that every iteration
/// reaches. `coincident` used to be `vec![depth == 1; depth]`, so this 1-D band was
/// marked parallel and the parallel emitter attached
/// `!llvm.loop.parallel_accesses` / `omp parallel for` to it -- a claim about
/// independence that nothing had checked, made in the very code path whose doc comment
/// says such a claim "without having looked at the memory access pattern would be
/// exactly the kind of unfounded claim this lowering path is meant to avoid".
///
/// The correctness evidence is the CPU result, not the IR text: a metadata tag is
/// inert here (LLVM's own dependence analysis overrides it, so the loop still computes
/// the right answer even when wrongly tagged), and this test previously demonstrated
/// that. What the tag WOULD do is authorise a future transformation to reorder the
/// iterations, which is exactly what must not happen to this loop. So the assertion is
/// on the absence of the tag, and the run is there to confirm the loop still computes
/// 0+1+2+3 = 6 while carrying no parallel claim.
///
/// The value 6 also rules out the other wrong answers: 0 means the body never ran, 3
/// means the half-open bound was mishandled.
#[test]
fn a_loop_carried_dependence_is_not_tagged_parallel_and_still_computes() {
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
        "the carried dependence must still SEQUENCE correctly: 0+1+2+3 = 6. Got {got}."
    );
    for marker in [
        "parallel_accesses",
        "omp parallel for",
        "!llvm.loop.parallel",
    ] {
        assert!(
            !ir.contains(marker),
            "a loop with a carried dependence through `total` must not be tagged \
             `{marker}`. The tag authorises reordering iterations that share a \
             location.\n--- IR ---\n{ir}"
        );
    }
}

/// TASK 1, control: the same program shape WITHOUT a dependence is a different
/// question, and the tag must not simply be absent everywhere.
///
/// This pins that the previous test passes because of the dependence argument and not
/// because the emitter stopped emitting tags. A read-only band is a real parallel
/// opportunity, and the emitter still reaches for it when the analysis says so; the
/// IR here is a hand-built band with `coincident: vec![true]`, which is the shape
/// `level_is_parallel` produces for a band whose writes are all injective in the
/// iterator (e.g. `A[i] = ..`).
#[test]
fn a_band_the_analysis_calls_parallel_still_gets_the_parallel_tag() {
    use naso_compiler::ast::{Mutability, Quantity};
    use naso_compiler::ir::affine_domain::{AffineConstraint, AffineDomain};
    use naso_compiler::ir::affine_map::{AffineMap, Matrix};
    use naso_compiler::ir::pir_types::{PirExpr, PirStatement};
    use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    let dest = builder
        .type_lowering()
        .int_type(naso_compiler::codegen::abi::IntWidth::I64);
    let global = builder.module().add_global(dest, None, "result");
    global.set_initializer(&dest.const_zero());
    builder.add_variable("result".to_string(), global.as_pointer_value(), dest.into());

    // A 1-D band over `0..4` whose level IS parallel, with a declared iterator so the
    // body can read the induction variable.
    let domain = AffineDomain::new(
        1,
        0,
        vec![
            AffineConstraint::inequality(vec![1], 0),
            AffineConstraint::inequality(vec![-1], -3),
        ],
    )
    .with_name("band");
    let map = AffineMap::total(domain.clone(), Matrix::identity(1));

    let pir = PirModule {
        statements: vec![PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::IntLit(0),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        }],
        schedule: ScheduleTree::new(
            ScheduleNode::Band {
                members: vec![map],
                coincident: vec![true],
                iterators: vec!["i".to_string()],
                child: Box::new(ScheduleNode::domain(StmtId(0), domain)),
            },
            vec![],
        ),
        ..Default::default()
    };

    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed: {e}"));
    let ir = builder.module_to_string();
    assert!(
        ir.contains("parallel_accesses"),
        "a band the dependence analysis calls parallel must still be tagged, so the \
         absence in the carried-dependence test is a real decision and not a dead \
         emitter.\n--- IR ---\n{ir}"
    );
}

/// Build a one-level band over `0..4` with the given iterator declaration, whose body
/// sums the induction variable into `result`.
///
/// Returns the printed result and the IR, or the build error as a string. The band is
/// hand-built so the iterator declaration is the only thing under test: `result` is a
/// global the body accumulates into, and `i` is the induction variable the body reads.
fn run_band_with_iterator(iterators: Vec<String>) -> Result<(i64, String), String> {
    use naso_compiler::ast::{Mutability, Quantity};
    use naso_compiler::ir::affine_domain::{AffineConstraint, AffineDomain};
    use naso_compiler::ir::affine_map::{AffineMap, Matrix};
    use naso_compiler::ir::pir_types::{BinaryOp, PirExpr, PirStatement};
    use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    let dest = builder
        .type_lowering()
        .int_type(naso_compiler::codegen::abi::IntWidth::I64);
    let global = builder.module().add_global(dest, None, "result");
    global.set_initializer(&dest.const_zero());
    builder.add_variable("result".to_string(), global.as_pointer_value(), dest.into());

    let domain = AffineDomain::new(
        1,
        0,
        vec![
            AffineConstraint::inequality(vec![1], 0),
            AffineConstraint::inequality(vec![-1], -3),
        ],
    )
    .with_name("band");
    let map = AffineMap::total(domain.clone(), Matrix::identity(1));

    let body = PirExpr::Assign {
        target: Box::new(PirExpr::Var("result".to_string())),
        value: Box::new(PirExpr::Binary {
            op: BinaryOp::Add,
            left: Box::new(PirExpr::Var("result".to_string())),
            right: Box::new(PirExpr::Var("i".to_string())),
        }),
    };

    let pir = PirModule {
        statements: vec![PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body,
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        }],
        schedule: ScheduleTree::new(
            ScheduleNode::Band {
                members: vec![map],
                coincident: vec![false],
                iterators,
                child: Box::new(ScheduleNode::domain(StmtId(0), domain)),
            },
            vec![],
        ),
        ..Default::default()
    };

    builder.build_module(&pir).map_err(|e| e.to_string())?;
    let ir = builder.module_to_string();
    Ok((execute(&ir, &[], &[]), ir))
}

/// TASK 1, per-level: the parallel decision is `.all()`, not `.any()`.
///
/// `is_parallel` used to be `coincident.iter().any(|&c| c)`, which routes a band to the
/// parallel emitter as soon as ONE level claims parallelism -- and the parallel emitter
/// parallelises the WHOLE band, attaching `!llvm.loop.parallel_accesses` to it. So a
/// 2-D band whose inner level is parallel and whose outer level is not was tagged
/// wholesale, and the tag would authorise reordering the outer iterations, which carry
/// the dependence. `.all()` is the conjunction the emitter's semantics require: if any
/// level is sequential, the band must be emitted sequentially.
///
/// The band here is hand-built with `coincident: vec![false, true]`: outer level
/// sequential, inner level parallel. That is the shape `level_is_parallel` produces for
/// a body writing `A[i]` (injective in `i`) while the inner loop over `j` is itself
/// parallel -- and it is exactly the case `.any()` gets wrong.
#[test]
fn a_band_with_one_sequential_level_is_not_parallelised_as_a_whole() {
    use naso_compiler::ast::{Mutability, Quantity};
    use naso_compiler::ir::affine_domain::{AffineConstraint, AffineDomain};
    use naso_compiler::ir::affine_map::{AffineMap, Matrix};
    use naso_compiler::ir::pir_types::{PirExpr, PirStatement};
    use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    // A 2-D band over 0..2 x 0..2 whose OUTER level is sequential and whose INNER level
    // is parallel.
    let domain = AffineDomain::new(
        2,
        0,
        vec![
            AffineConstraint::inequality(vec![1, 0], 0),
            AffineConstraint::inequality(vec![-1, 0], -1),
            AffineConstraint::inequality(vec![0, 1], 0),
            AffineConstraint::inequality(vec![0, -1], -1),
        ],
    )
    .with_name("band");
    let map = AffineMap::total(domain.clone(), Matrix::identity(2));

    let pir = PirModule {
        statements: vec![PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::IntLit(0),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        }],
        schedule: ScheduleTree::new(
            ScheduleNode::Band {
                members: vec![map],
                coincident: vec![false, true],
                iterators: vec!["i".to_string(), "j".to_string()],
                child: Box::new(ScheduleNode::domain(StmtId(0), domain)),
            },
            vec![],
        ),
        ..Default::default()
    };

    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed: {e}"));
    let ir = builder.module_to_string();
    assert!(
        !ir.contains("parallel_accesses"),
        "one sequential level makes the WHOLE band sequential. The parallel emitter \
         tags the entire band, so `.any()` here would authorise reordering iterations \
         of the sequential level.\n--- IR ---\n{ir}"
    );
}

/// TASK 3: the band carries the iterator's SOURCE SPELLING, and the body's read of it
/// resolves to the induction variable.
///
/// This is the half of the fix that shows a value arrived. The old channel was a
/// `AffineDomain::name` of the form `nest(i)`, parsed back with `strip_prefix` and
/// `trim_end_matches(')')`; the name was therefore only as real as that format string,
/// and a hand-built band whose domain label did not happen to match left the body's `i`
/// unbound -- reading zero, silently, on every iteration.
///
/// 0 + 1 + 2 + 3 = 6 is the answer only if the body read the induction variable. A
/// backend that bound nothing gives 0 + 0 + 0 + 0 = 0, and one that bound the wrong
/// variable (or a stale one from a neighbouring level) gives something else again.
#[test]
fn a_band_that_declares_its_iterator_lets_the_body_read_the_induction_variable() {
    let (got, ir) = run_band_with_iterator(vec!["i".to_string()])
        .expect("a band that declares its iterator must build");
    assert_eq!(
        got, 6,
        "the body must read the declared iterator: 0+1+2+3 = 6. An unbound `i` reads \
         zero on every iteration and gives 0.\n--- IR ---\n{ir}"
    );
}

/// TASK 3, two levels: each level must bind ITS OWN iterator, not a neighbour's.
///
/// The single-level tests would still pass if the lowering bound the same name at every
/// level. This one reads both `i` and `j`, so a level that binds its neighbour's name --
/// or a list read in the wrong order -- gives a different total.
///
/// Over the inclusive ranges `i in 0..=3` by `j in 0..=3` -- note the loop condition is
/// `sle`, so a bound of `3` admits `3` -- the body accumulates `result = result +
/// 10*i + j`. That is `4 * (10*(0+1+2+3)) + 4 * (0+1+2+3)` = 264, and the `10*i` term
/// weights the outer iterator ten times the inner one, so a level that bound its
/// neighbour's name, or a list read in the wrong order, cannot land on 264.
#[test]
fn a_two_level_band_binds_each_level_its_own_iterator() {
    use naso_compiler::ast::{Mutability, Quantity};
    use naso_compiler::ir::affine_domain::{AffineConstraint, AffineDomain};
    use naso_compiler::ir::affine_map::{AffineMap, Matrix};
    use naso_compiler::ir::pir_types::{BinaryOp, PirExpr, PirStatement};
    use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");

    let dest = builder
        .type_lowering()
        .int_type(naso_compiler::codegen::abi::IntWidth::I64);
    let global = builder.module().add_global(dest, None, "result");
    global.set_initializer(&dest.const_zero());
    builder.add_variable("result".to_string(), global.as_pointer_value(), dest.into());

    let domain = AffineDomain::new(
        2,
        0,
        vec![
            AffineConstraint::inequality(vec![1, 0], 0),
            AffineConstraint::inequality(vec![-1, 0], -3),
            AffineConstraint::inequality(vec![0, 1], 0),
            AffineConstraint::inequality(vec![0, -1], -3),
        ],
    )
    .with_name("band");
    // One scheduling map per loop level, so the band has two members.
    let map = AffineMap::total(domain.clone(), Matrix::identity(2));

    // result = result + 10*i + j
    let term = |factor: i64, var: &str| PirExpr::Binary {
        op: BinaryOp::Mul,
        left: Box::new(PirExpr::IntLit(factor)),
        right: Box::new(PirExpr::Var(var.to_string())),
    };
    let step = PirExpr::Binary {
        op: BinaryOp::Add,
        left: Box::new(term(10, "i")),
        right: Box::new(term(1, "j")),
    };
    let body = PirExpr::Assign {
        target: Box::new(PirExpr::Var("result".to_string())),
        value: Box::new(PirExpr::Binary {
            op: BinaryOp::Add,
            left: Box::new(PirExpr::Var("result".to_string())),
            right: Box::new(step),
        }),
    };

    let pir = PirModule {
        statements: vec![PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body,
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        }],
        schedule: ScheduleTree::new(
            ScheduleNode::Band {
                members: vec![map.clone(), map],
                coincident: vec![false, false],
                iterators: vec!["i".to_string(), "j".to_string()],
                child: Box::new(ScheduleNode::domain(StmtId(0), domain)),
            },
            vec![],
        ),
        ..Default::default()
    };

    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed: {e}"));
    assert_eq!(
        execute(&builder.module_to_string(), &[], &[]),
        264,
        "sum over i in 0..=3, j in 0..=3 of 10*i + j must be 264; a level that bound the \
         wrong iterator would not produce this"
    );
}

/// TASK 3, negative direction: a band with NO declared iterator must not silently bind
/// the WRONG one.
///
/// The band that declares `i` gives 6; the question here is what a band declaring
/// something ELSE does with a body that reads `i`. If the backend fell back to any
/// other name -- a stale binding, a neighbouring level's iterator, a parse of the
/// domain's debug label -- the result would differ from a band that declares nothing
/// at all. So the test runs the two and requires them to agree.
///
/// Agreement at 0 is the honest expectation, and it is a limitation worth naming
/// rather than hiding: an unbound `i` reads as i64 zero, because
/// `expr_lowering`'s `PirExpr::Var` arm returns zero for a name with no allocation.
/// That is a pre-existing weakness, documented in place and previously reverted from
/// being an error, and it is NOT what this test pins. What this test pins is that the
/// iterator channel is now the band's `iterators` field and nothing else: no other name
/// is substituted, and the correct declaration is what changes the answer.
///
/// The `no declaration` case is what the OLD code did for a hand-built band whose
/// domain label did not parse, so this is the pre-existing behaviour held to not have
/// silently changed into "binds whatever was lying around".
#[test]
fn a_band_with_no_declared_iterator_does_not_bind_the_wrong_one() {
    // Declares `j`; the body reads `i`. Nothing here may resolve `i` to `j`.
    let wrong_name = run_band_with_iterator(vec!["j".to_string()])
        .expect("a band naming a variable the body never reads must still build");
    // Declares nothing at all.
    let no_name =
        run_band_with_iterator(vec![String::new()]).expect("a band with no iterator must build");

    assert_eq!(
        wrong_name.0, no_name.0,
        "a body reading `i` must get the same answer whether the band declares `j` or \
         nothing. They disagree if some other name is being substituted for `i`. \
         `j` gave {}, no name gave {}.\n--- IR (declares j) ---\n{}",
        wrong_name.0, no_name.0, wrong_name.1
    );
    // The wrong name must not appear as a binding the body could have read.
    assert!(
        !wrong_name.1.contains("\"j\""),
        "`j` was never read by the body, so it must not have been bound to the \
         induction variable.\n--- IR ---\n{}",
        wrong_name.1
    );
    // And declaring the right name must actually change the answer, so the agreement
    // above is a real observation rather than both cases being equally broken.
    let right_name = run_band_with_iterator(vec!["i".to_string()])
        .expect("a band declaring the read iterator must build");
    assert_ne!(
        right_name.0, no_name.0,
        "declaring `i` must change what the body computes, otherwise the iterator \
         field is not reaching codegen at all"
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
