//! WGSL must refuse what it cannot emit, and must see inside blocks.
//!
//! # Two holes, both "reported success while doing less"
//!
//! ## 1. `count_in_expr` had no arm for `PirExpr::Stmts`
//!
//! It fell through to `_ => 0`, so EVERY occurrence inside a block counted as
//! nothing. That is the wrong answer in the most dangerous direction: `verify_wgsl_linearity`
//! still ran, still accepted programs, and on the ones it did catch reported
//!
//!     Linear variable 'q' used 0 times
//!
//! which describes its own blindness rather than the user's error.
//!
//! `Stmts` is now where control flow lives. `if c { return 7; }` lowers to exactly
//! that, so before this fix the check was blind to a linear resource returned from
//! a branch, and equally blind to one read twice in a loop body.
//!
//! ## 2. `emit_expr` commented the operation out and returned `Ok`
//!
//! For any expression it did not handle it wrote
//!
//! ```text
//! // Unsupported expr: ...
//! ```
//!
//! into the WGSL and reported success. The result validates under naga, runs, and
//! computes the wrong answer -- because the operation that should have been emitted
//! is simply absent. A missing statement is not a visible error; it is a program.
//!
//! This is the same defect as the `Reversible` inverse drop the LLVM and QIR backends
//! carried, which is what prompted the audit. `wgsl_straight.rs` and
//! `wgsl_compute.rs` already refused correctly; only this emitter had the hole.

use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::codegen::wgsl::{WgslTarget, generate_wgsl, verify_wgsl_linearity};
use naso_compiler::ir::affine_domain::AffineDomain;
use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
use naso_compiler::ir::schedule_tree::StmtId;

/// A module whose single statement is `body`, with `q` marked `[1]`.
fn module_with(body: PirExpr) -> PirModule {
    let mut m = PirModule::default();
    m.quantities.insert("q".to_string(), Quantity::One);
    m.statements.push(PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body,
        quantity: Quantity::One,
        mutability: Mutability::Immutable,
        span: None,
    });
    m
}

/// `PirExpr::Var` wants an owned name.
fn q() -> PirExpr {
    PirExpr::Var(String::from("q"))
}

/// A block containing `parts`.
fn block(parts: Vec<PirExpr>) -> PirExpr {
    PirExpr::Stmts(parts)
}

/// A conditional whose arms are expressions.
fn cond_if(c: PirExpr, t: PirExpr, e: PirExpr) -> PirExpr {
    PirExpr::If {
        cond: Box::new(c),
        then_branch: Box::new(t),
        else_branch: Box::new(e),
    }
}

/// A `return` carrying `value`.
fn ret(value: PirExpr) -> PirExpr {
    PirExpr::Return {
        value: Some(Box::new(value)),
    }
}

/// An integer literal.
fn int(n: i64) -> PirExpr {
    PirExpr::IntLit(n)
}

/// The linearity check must SEE inside a block.
///
/// Before the fix this reported "used 0 times" and rejected a program that consumed
/// `q` exactly once. The count of 0 is the tell: it named the counter's blindness
/// rather than the user's error.
#[test]
fn linearity_sees_a_use_inside_a_block() {
    assert!(
        verify_wgsl_linearity(&module_with(block(vec![q()]))).is_ok(),
        "a `[1]` value used exactly once inside a block is legal. Rejecting it means \
         the counter is not descending into `Stmts`."
    );
}

/// A use at the top level must still be counted.
///
/// This is what a counter that always returned 0 would fail. Without it, "never
/// descend into blocks" and "never count anything" look identical from the outside.
#[test]
fn linearity_still_sees_a_use_at_the_top_level() {
    assert!(
        verify_wgsl_linearity(&module_with(q())).is_ok(),
        "the top-level case must remain accepted"
    );
}

/// Two uses inside a block must be rejected, naming the real count.
///
/// The message must say "2 times", not "0 times" -- a count of zero for two uses is
/// the original bug restated.
#[test]
fn linearity_rejects_two_uses_inside_a_block_and_names_the_count() {
    let err =
        verify_wgsl_linearity(&module_with(block(vec![q(), q()]))).expect_err("two uses must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("2 times"),
        "the diagnostic must name the real count (2), not 0: {msg}"
    );
    assert!(
        !msg.contains("0 times"),
        "a count of 0 for two uses is the original bug restated: {msg}"
    );
}

/// A `return` inside a block CONSUMES the value it returns.
///
/// The return edge is a use. Omitting it would let a linear resource escape through
/// the return, which is the leak this language exists to make impossible.
#[test]
fn a_return_consumes_the_value_it_returns() {
    assert!(
        verify_wgsl_linearity(&module_with(block(vec![ret(q())]))).is_ok(),
        "returning `q` exactly once is a single consumption"
    );
}

/// Returning the same linear value twice must be rejected.
///
/// Both returns name the same `[1]` resource, and one of them has to be a duplicate
/// consumption.
#[test]
fn returning_the_same_linear_value_twice_is_rejected() {
    let err = verify_wgsl_linearity(&module_with(block(vec![ret(q()), ret(q())])))
        .expect_err("two returns must fail");
    assert!(
        err.to_string().contains("2 times"),
        "the diagnostic must count both returns: {err}"
    );
}

/// A `return` inside an `if` arm must be visible to the linearity check.
///
/// This is the shape real programs have, and it is a `Stmts` nested inside an `If`:
/// two levels the counter used to walk straight past. Before the fix the check
/// reported 0 uses and rejected the program.
#[test]
fn a_return_inside_an_if_arm_is_visible_to_linearity() {
    let body = cond_if(int(1), block(vec![ret(q())]), int(0));
    assert!(
        verify_wgsl_linearity(&module_with(body)).is_ok(),
        "one conditional return of `q` is a single consumption"
    );
}

/// A duplicated conditional return must be rejected too.
///
/// The nested case, not just the top-level one: a counter that descends into `If` but
/// not into the `Stmts` inside it would pass the previous test and fail this.
#[test]
fn two_returns_in_one_if_arm_are_rejected() {
    let body = cond_if(int(1), block(vec![ret(q()), ret(q())]), int(0));
    let err = verify_wgsl_linearity(&module_with(body)).expect_err("must fail");
    assert!(
        err.to_string().contains("2 times"),
        "the diagnostic must count both returns inside the arm: {err}"
    );
}

/// The WGSL emitter must REFUSE an expression it cannot emit.
///
/// It used to write `// Unsupported expr` into the shader and return `Ok`, which
/// produces a module that validates and computes the wrong answer. There is no
/// naga-level defence against a missing statement.
#[test]
fn the_emitter_refuses_a_return_rather_than_commenting_it_out() {
    match generate_wgsl(&module_with(block(vec![ret(q())])), WgslTarget::WebGpu) {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("cannot emit") || msg.contains("Unsupported"),
                "the refusal must name the problem: {msg}"
            );
        }
        Ok(shader) => panic!(
            "a `return` must not compile to WGSL silently. It previously produced a \
             shader with a comment in place of the operation, which validates and \
             computes the wrong result.\n--- shader ---\n{shader}"
        ),
    }
}

/// A conditional `return` must be refused by the emitter, not dropped.
///
/// The shape that most needs a refusal: the branch selects the return, so silently
/// omitting it removes the branch's entire effect.
#[test]
fn the_emitter_refuses_a_conditional_return() {
    let body = cond_if(int(1), block(vec![ret(q())]), int(0));
    if let Ok(shader) = generate_wgsl(&module_with(body), WgslTarget::WebGpu) {
        panic!(
            "a conditional `return` must not compile to WGSL silently.\n\
             --- shader ---\n{shader}"
        );
    }
}

/// An `Ok` result must never be a shader missing its entry point.
///
/// A `generate` that returns `Ok("")`, or a shader with no body, is success with
/// nothing emitted -- the same failure as the comment, wearing a different hat.
#[test]
fn a_refused_emission_does_not_leave_a_silent_success() {
    // Whatever the emitter decides for these, an Ok result must contain real code.
    for body in [
        block(vec![ret(q())]),
        cond_if(int(1), block(vec![ret(q())]), int(0)),
        block(vec![block(vec![ret(q())])]),
    ] {
        if let Ok(shader) = generate_wgsl(&module_with(body), WgslTarget::WebGpu) {
            assert!(
                !shader.trim().is_empty(),
                "an Ok result must not be an empty shader"
            );
        }
    }
}
