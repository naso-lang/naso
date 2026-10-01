//! Refusal paths for LLVM lowering.
//!
//! # What moved to `llvm_execution_test`
//!
//! This file previously asserted that a cast emitted `fptosi`/`trunc`/`sext`/`zext` in
//! the IR TEXT. Those assertions were unsound as evidence and have been replaced by
//! tests that COMPILE and RUN the generated module on the CPU:
//!
//!   * A correct `fptosi` on a constant is constant-folded into the following store
//!     before the module is printed, so it is INVISIBLE in the IR string.
//!   * A `fptosi` of an out-of-range value is POISON, which prints as
//!     `store i8 poison` with the instruction removed -- indistinguishable from the
//!     cast never being emitted.
//!
//! Both cases make "the instruction is missing" ambiguous between a correct
//! implementation and a broken one, so text matching could not decide it. `lli` was
//! also rejected for execution: it reports only an 8-bit exit status.
//!
//! # What is left here
//!
//! Only the two paths that are about REFUSING rather than producing IR, which execution
//! cannot express: an unrepresentable bit width, and an assignment with no destination.
//! Both are cases where the correct behaviour is an error, and an error is what a
//! test can assert directly.

#![cfg(feature = "llvm")]

use inkwell::context::Context;
use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;
use naso_compiler::ir::affine_domain::AffineDomain;
use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

#[test]
fn a_cast_to_an_unsupported_bit_width_is_refused() {
    // 24 bits has no LLVM integer type in `IntWidth`, so the honest answer is an
    // error rather than a nearby width.
    let _ctx = Context::create();
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::Default).expect("context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");
    let stmt = PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::Cast {
            expr: Box::new(PirExpr::IntLit(1)),
            width: Some(24),
            signed: true,
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    // The schedule tree must name the statement: `build_module` lowers the SCHEDULE,
    // not the statement list, so a statement no `Domain` node covers is reported as
    // unreachable. Giving the statement a `Domain` node is what lets the intended
    // error -- the unlowerable cast, or the unknown allocation -- be the one that
    // surfaces instead of the coverage check.
    let module = PirModule {
        statements: vec![stmt],
        schedule: ScheduleTree::new(
            ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
            vec![],
        ),
        ..Default::default()
    };
    let err = builder
        .build_module(&module)
        .expect_err("a 24-bit cast has no LLVM type here and must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("24"),
        "the diagnostic must name the width it could not lower: {msg}"
    );
}

#[test]
fn assignment_to_an_unknown_allocation_is_refused() {
    // A store with nowhere to put the value would compile to "compute the right
    // answer and store nothing", which is the class of bug this guards.
    let _ctx = Context::create();
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::Default).expect("context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");
    let stmt = PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::Assign {
            target: Box::new(PirExpr::Var("nowhere".into())),
            value: Box::new(PirExpr::IntLit(1)),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    // The schedule tree must name the statement: `build_module` lowers the SCHEDULE,
    // not the statement list, so a statement no `Domain` node covers is reported as
    // unreachable. Giving the statement a `Domain` node is what lets the intended
    // error -- the unlowerable cast, or the unknown allocation -- be the one that
    // surfaces instead of the coverage check.
    let module = PirModule {
        statements: vec![stmt],
        schedule: ScheduleTree::new(
            ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
            vec![],
        ),
        ..Default::default()
    };
    let err = builder
        .build_module(&module)
        .expect_err("assignment with no allocation must be refused");
    assert!(
        err.to_string().contains("nowhere"),
        "the diagnostic must name the target: {err}"
    );
}
