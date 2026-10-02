//! End-to-end: a schedule band must become a real LLVM loop that RUNS.
//!
//! # The gap this file pins
//!
//! `lower_schedule_tree` lowers a `ScheduleTree` to loops via `LoopEmitter`, but it had
//! no caller: `build_module` walked the PIR statements directly and emitted each one
//! once. A `forall` therefore compiled to straight-line code that executed its body a
//! single time -- arithmetically wrong, with no diagnostic, because the schedule tree
//! that recorded the loop nest was built and then discarded.
//!
//! These tests check the loop by RUNNING it, and by counting the iterations on the CPU.

#![cfg(feature = "llvm")]

use inkwell::targets::{InitializationConfig, Target};
use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::schedule_lowering::lower_schedule_tree;
use naso_compiler::ir::AccessRelations;
use naso_compiler::ir::affine_domain::AffineDomain;
use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn tool(name: &str) -> String {
    match std::env::var("LLVM_SYS_170_PREFIX") {
        Ok(p) if !p.is_empty() => format!("{p}/bin/{name}"),
        _ => name.to_string(),
    }
}

struct CaseDir(PathBuf);
static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        // Unique per CALL, not per process: the harness runs tests as threads of one
        // process, so a shared directory would have cases overwrite each other's
        // binaries. That produced one test reading another's result.
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("naso-loop-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).expect("mkdir");
        CaseDir(d)
    }
    fn path(&self, n: &str) -> PathBuf {
        self.0.join(n)
    }
}
impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Lower a schedule and run `sum_loop`, which must return the number of iterations the
/// loop body actually executed.
///
/// The driver is generated here rather than in the compiler, so the arithmetic being
/// checked is the LOOP's, not the harness's: the body increments a counter, and the
/// result is whatever the CPU counted.
const HARNESS_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
extern int64_t sum_loop(int64_t n);
int main(void) {
    printf("%lld\n", (long long)sum_loop(4));
    return 0;
}
"#;

fn run(ir: &str, what: &str) -> Vec<u8> {
    let dir = CaseDir::new();
    let ll = dir.path("c.ll");
    let obj = dir.path("c.o");
    let c = dir.path("h.c");
    let exe = dir.path("c");
    std::fs::write(&ll, ir).unwrap();
    std::fs::write(&c, HARNESS_C).unwrap();
    for (cmd, label) in [
        (
            Command::new(tool("llc"))
                .arg("-filetype=obj")
                .arg(&ll)
                .arg("-o")
                .arg(&obj),
            "llc",
        ),
        (
            Command::new(tool("clang"))
                .arg(&c)
                .arg(&obj)
                .arg("-o")
                .arg(&exe),
            "clang",
        ),
    ] {
        let out = cmd.output().unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(
            out.status.success(),
            "{label} failed: {}\n--- IR ---\n{ir}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = Command::new(&exe).output().unwrap();
    assert!(
        out.status.success(),
        "running the compiled loop failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = what;
    out.stdout
}

/// A schedule whose single band runs statement 0 over `0..n`, with statement 0's body
/// being `result = result + 1`.
fn build_loop_ir() -> String {
    // Both the context and the module are leaked deliberately and live for the rest of
    // the test process.
    //
    // `lower_schedule_tree` borrows the context AND the module for the same lifetime,
    // and inkwell's `Module<'ctx>` is INVARIANT over `'ctx`. That forces the two
    // borrows to be the same region, which a stack-local pair cannot satisfy: `module`
    // borrows `cc`, so the borrow of `cc` would have to outlive `module`'s own drop,
    // and drop order is the reverse of declaration order. Leaking sidesteps the
    // question entirely. `ScheduleLowering`'s own unit tests use the same trick.
    let cc: &'static CodegenContext = Box::leak(Box::new(
        CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("context"),
    ));
    let llvm = cc.llvm_context();
    let module: &'static inkwell::module::Module<'static> =
        Box::leak(Box::new(llvm.create_module("loop_test")));
    let i64t = llvm.i64_type();
    let fn_type = i64t.fn_type(&[i64t.into()], false);
    let function = module.add_function("sum_loop", fn_type, None);

    // The loop counter lives in a global the band body increments, so the value the CPU
    // reports is the number of iterations actually performed.
    let counter = module.add_global(i64t, None, "counter");
    counter.set_initializer(&i64t.const_zero());
    // The band emitter works from its own `LlvmValueBuilder`, not the module builder, so
    // the global must be reachable by NAME at emission time. It is resolved from the
    // module by the variable lookup the Assign arm performs.

    // A band over 0..n.
    //
    // The constraints are what `iterator_bounds` reads to recover the loop bounds, so an
    // unconstrained domain yields NO bounds and `extract_bounds` returns an empty vec --
    // at which point the band emits no loop at all and the function body never runs. That
    // is what the first version of this test did, and it produced
    // `define i64 @sum_loop(i64 %0) { entry: ret i64 0 }`.
    //
    // Half-open `0..n` is spelled `i >= 0` and `i <= n-1`; with no parameters both are
    // plain integer constants, so the loop is statically bounded.
    let iter = AffineDomain::new(
        1,
        0,
        vec![
            // i >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1], 0),
            // `-i >= -3` means `i <= 3`, so the loop runs 4 times.
            //
            // This is the natural spelling. It previously had to be written as
            // `-i >= 4`, which is the OPPOSITE inequality, because
            // `AffineDomain::iterator_bounds` negated the constant as well as dividing
            // by the negative coefficient and so returned the upper bound with inverted
            // sign -- a `0..3` loop then compared `iv < -3` and never ran its body. That
            // compensation hid the bug; the correct arithmetic is pinned in
            // `affine_domain`'s unit tests.
            //
            // It also used to be written `-i >= -4`, i.e. `i <= 4`, to get 4 iterations.
            // That compensated for the emitter comparing `iv < upper` against what is
            // an INCLUSIVE bound: the front end encodes the half-open source range
            // `0..4` as `i <= 4 - 1`, so the emitter must compare `iv <= upper`.
            // With the exclusive compare, every loop this backend emitted from a real
            // `.naso` source dropped its final iteration -- `0..4` summed 0+1+2 and
            // reported 3. Both the encoding and the comparison are now the honest
            // half-open ones, and this test asserts the trip count the source asks for.
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1], -3),
        ],
    );
    let map = naso_compiler::ir::affine_map::AffineMap::new(vec![
        naso_compiler::ir::affine_map::AffineMapPiece::new(
            iter,
            naso_compiler::ir::affine_map::Matrix::identity(1),
        ),
    ]);

    // `counter = counter + 1` -- the body's only job is to be observable, so the value
    // the CPU reports is the number of iterations actually performed. A body that
    // computed something and stored nothing would pass a structural check and fail here.
    let stmt = PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(1, 0),
        body: PirExpr::Assign {
            target: Box::new(PirExpr::Var("counter".into())),
            value: Box::new(PirExpr::Binary {
                op: naso_compiler::ir::pir_types::BinaryOp::Add,
                left: Box::new(PirExpr::Var("counter".into())),
                right: Box::new(PirExpr::IntLit(1)),
            }),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    let schedule = ScheduleTree::new(
        ScheduleNode::Band {
            members: vec![map],
            coincident: vec![false],
            // This hand-built band declares no iterator, and the statement body
            // never reads one. Leaving the name empty is the honest encoding of
            // that: the loop is real, its induction variable simply has no source
            // spelling to bind.
            iterators: vec![String::new()],
            child: Box::new(ScheduleNode::domain(
                StmtId(0),
                AffineDomain::universe(1, 0),
            )),
        },
        vec![],
    );
    let pir = PirModule {
        statements: vec![stmt],
        schedule,
        accesses: AccessRelations::new(),
        quantities: HashMap::new(),
        parameters: vec![],
        // A hand-built module declares no function parameters, so the entry
        // function keeps the bare `void()` signature these tests call.
        function_params: vec![],
        extern_functions: vec![],
    };

    lower_schedule_tree(
        cc,
        module,
        function,
        &pir.schedule,
        &pir,
        &pir.quantities,
        &pir.accesses,
    )
    .unwrap_or_else(|e| panic!("schedule lowering failed: {e}"));

    module.print_to_string().to_string()
}

#[test]
fn a_band_becomes_a_real_loop_that_runs_on_the_cpu() {
    let _ = Target::initialize_native(&InitializationConfig::default()).map(|_| ());
    let ir = build_loop_ir();
    let out = String::from_utf8_lossy(&run(&ir, "sum_loop"))
        .trim()
        .to_string();
    println!("IR:\n{ir}\nreturned: {out}");
    assert!(ir.contains("br "), "the band must emit branches: {ir}");
}

// ---------------------------------------------------------------------------
// The band BODY, not just the loop structure
// ---------------------------------------------------------------------------

/// The driver for `a_band_body_actually_runs_on_the_cpu`.
///
/// It reads the `counter` GLOBAL the band body increments, rather than the generated
/// function's return value. That distinction is the whole point: `lower_schedule_tree`
/// ends a non-void function with a ZERO of its return type (`ret i64 0`), so a driver
/// that printed the return value would read 0 whether or not the body did anything.
/// Reading the global is the only way to see what the loop actually computed.
const COUNTER_HARNESS_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
extern int64_t sum_loop(int64_t n);
extern int64_t counter;
int main(void) {
    sum_loop(4);
    printf("%lld\n", (long long)counter);
    return 0;
}
"#;

/// Compile `ir`, link it against `driver`, run it, and return the printed integer.
fn run_with_driver(ir: &str, driver: &str) -> i64 {
    let dir = CaseDir::new();
    let ll = dir.path("b.ll");
    let obj = dir.path("b.o");
    let c = dir.path("h.c");
    let exe = dir.path("b");
    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&c, driver).expect("write C driver");
    for (cmd, label) in [
        (
            Command::new(tool("llc"))
                .arg("-filetype=obj")
                .arg(&ll)
                .arg("-o")
                .arg(&obj),
            "llc",
        ),
        (
            Command::new(tool("clang"))
                .arg(&c)
                .arg(&obj)
                .arg("-o")
                .arg(&exe),
            "clang",
        ),
    ] {
        let out = cmd.output().unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(
            out.status.success(),
            "{label} failed: {}\n--- IR ---\n{ir}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = Command::new(&exe)
        .output()
        .expect("run the compiled program");
    assert!(
        out.status.success(),
        "the compiled loop failed to run: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<i64>()
        .unwrap_or_else(|e| panic!("driver printed {e}; expected an integer.\n--- IR ---\n{ir}"))
}

/// The band body must actually EXECUTE, not merely exist.
///
/// `emit_statement_body` used to be a stub returning `Ok(())` "because the access
/// emitter handles it". It does not: the access emitter only handles
/// `AccessRelation`s, and a plain expression body has none. So the band lowered to a
/// correct loop -- preheader, header, phi, latch, exit -- with an EMPTY body, and the
/// loop iterated the right number of times while computing nothing.
///
/// The counter is incremented once per iteration and read back off the CPU, so this
/// cannot be satisfied by an arm that emits the right instructions and drops them, nor
/// by one that emits a loop and no body. `4` is also not a value any stub could
/// produce by accident: the body must run exactly that many times.
///
/// The trip count is 4 (`0..4`), chosen because it is neither 0 nor 1: a body that ran
/// once, or a loop that never entered its body, would both read differently.
#[test]
fn a_band_body_actually_runs_on_the_cpu() {
    let _ = Target::initialize_native(&InitializationConfig::default()).map(|_| ());
    let ir = build_loop_ir();
    let got = run_with_driver(&ir, COUNTER_HARNESS_C);
    assert_eq!(
        got, 4,
        "the band body must increment the counter once per iteration, 4 times. \
         Got {got}.\n--- IR ---\n{ir}"
    );
}
