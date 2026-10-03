//! Linearity must be checked at every emission boundary, not just by the typechecker.
//!
//! # What was missing
//!
//! `verify_wgsl_linearity` existed, was correct, and had its own tests. It was
//! **called by nothing but those tests.** Not one pipeline invoked it.
//!
//! `CodegenPipeline::emit_qir` had no check at all, and that is not a stylistic gap.
//! A QIR program whose linear values are used twice is not a slower program -- it is
//! a program whose quantum state has been destroyed and then reused. LLVM's module
//! verifier accepts it, so the invalid circuit left the backend looking healthy.
//!
//! Measured before this change:
//!
//! ```text
//! emit_qir(uses a [1] twice) == Ok(1461 chars of valid QIR)
//! ```
//!
//! # Scope, stated precisely
//!
//! This is **defense in depth, not a reachable CLI bug.** The typechecker rejects a
//! doubled `[1]` value and the CLI runs it before selecting a target, so no source
//! program reaches these entry points violating.
//!
//! What this closes is the library path. `CodegenPipeline::emit_qir` and
//! `generate_wgsl` are public API. A PIR module built by hand, produced by a
//! lowering pass, or checked in as a fixture can carry a violation no typechecker
//! ever saw -- and the backends are exactly where that must be caught, because the
//! output is a circuit or a shader with no consumer able to detect the problem.
//!
//! # Why the ordering matters
//!
//! `generate_wgsl` already *refused* a violating module -- but because it could not
//! emit `Stmts`, which is unrelated to linearity. A refusal for the wrong reason is
//! not a check, and it hides the real diagnostic from whoever has to fix the program.
//! Linearity is therefore checked FIRST, so a violation is always reported AS a
//! violation.

#![cfg(feature = "llvm")]

use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::codegen::CodegenPipeline;
use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::wgsl::{WgslTarget, generate_wgsl};
use naso_compiler::ir::affine_domain::AffineDomain;
use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
use naso_compiler::ir::schedule_tree::StmtId;

/// A pipeline over the host target.
fn pipeline() -> CodegenPipeline {
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
    CodegenPipeline::new(cc)
}

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

/// An integer literal.
fn int(n: i64) -> PirExpr {
    PirExpr::IntLit(n)
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

/// QIR must refuse a module that uses a `[1]` value twice.
///
/// The headline regression. This returned 1461 characters of valid, verifying QIR
/// built from a module whose quantum state would be destroyed and then reused.
#[test]
fn qir_refuses_a_linear_value_used_twice() {
    let err = pipeline()
        .emit_qir(&module_with(block(vec![q(), q()])))
        .expect_err("a doubled [1] value must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("used 2 times"),
        "the diagnostic must name the real count: {msg}"
    );
}

/// The refusal must come from linearity, not from an unrelated emitter failure.
///
/// Before the fix, QIR emitted successfully. A version that refused only because it
/// could not emit some node would pass the test above while still being wrong -- the
/// check has to be about the quantity.
#[test]
fn the_qir_refusal_is_about_linearity() {
    let err = pipeline()
        .emit_qir(&module_with(block(vec![q(), q()])))
        .expect_err("must refuse");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("linear"),
        "the diagnostic must name linearity as the cause: {err}"
    );
}

/// A doubled `[1]` value nested inside a conditional must also be refused.
///
/// One level deeper. A check that only walked the top-level statement list would
/// pass this.
#[test]
fn qir_refuses_a_doubled_value_inside_a_conditional() {
    let body = cond_if(int(1), block(vec![q(), q()]), int(0));
    assert!(
        pipeline().emit_qir(&module_with(body)).is_err(),
        "a violation inside an `if` arm must be refused too"
    );
}

/// The same value returned from two branches counts twice.
///
/// Both arms return the same `[1]` resource. One return edge has to be wrong.
#[test]
fn qir_refuses_the_same_value_returned_twice() {
    let body = block(vec![ret(q()), ret(q())]);
    assert!(
        pipeline().emit_qir(&module_with(body)).is_err(),
        "two returns of the same linear value must be refused"
    );
}

/// A module that consumes a `[1]` value exactly once must NOT be refused for
/// linearity.
///
/// The other half, and the one that keeps this from being "always reject". If the
/// boundary refused everything, every test above would pass and the backend would be
/// useless.
#[test]
fn qir_still_emits_a_legal_single_use() {
    match pipeline().emit_qir(&module_with(block(vec![q()]))) {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            assert!(
                !msg.to_lowercase().contains("linear"),
                "a single use is legal; refusing it as a linearity problem would be \\
                 a different bug: {msg}"
            );
        }
    }
}

/// WGSL must report a violation AS a linearity problem, not by accident.
///
/// This is the ordering point. `generate_wgsl` already refused this module -- because
/// it cannot emit `Stmts`. If linearity is not checked first, the diagnostic names an
/// emitter limitation and the user goes looking in the wrong place.
#[test]
fn wgsl_reports_a_violation_as_a_linearity_problem() {
    let err = generate_wgsl(&module_with(block(vec![q(), q()])), WgslTarget::WebGpu)
        .expect_err("must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("used 2 times") && msg.to_lowercase().contains("linear"),
        "the refusal must be about the doubled quantity, not the emitter: {msg}"
    );
}

/// A violation inside a conditional is reported as linearity by WGSL too.
///
/// Same reason as the test above, one level deeper.
#[test]
fn wgsl_reports_a_nested_violation_as_linearity() {
    let body = cond_if(int(1), block(vec![q(), q()]), int(0));
    let err = generate_wgsl(&module_with(body), WgslTarget::WebGpu).expect_err("must refuse");
    assert!(
        err.to_string().contains("used 2 times"),
        "the nested count must still be reported: {err}"
    );
}

/// A single use must not be reported as a violation by WGSL.
#[test]
fn wgsl_does_not_reject_a_legal_single_use() {
    match generate_wgsl(&module_with(block(vec![q()])), WgslTarget::WebGpu) {
        Ok(_) => {}
        Err(e) => assert!(
            !e.to_string().to_lowercase().contains("linear"),
            "a single use is legal: {e}"
        ),
    }
}

/// A module with no `[1]` values at all is untouched by the boundary.
#[test]
fn a_module_with_no_linear_values_is_not_rejected() {
    let mut m = PirModule::default();
    m.statements.push(PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: block(vec![int(1), int(2)]),
        quantity: Quantity::One,
        mutability: Mutability::Immutable,
        span: None,
    });
    match pipeline().emit_qir(&m) {
        Ok(_) => {}
        Err(e) => assert!(
            !e.to_string().to_lowercase().contains("linear"),
            "there is no linear value to violate: {e}"
        ),
    }
}

/// A qubit is NOT a consume-once linear value, and the fixture says so.
///
/// This test exists because the fixture was silently wrong in a way nothing caught.
///
/// `teleport.pir` declared `q[0] = One`, `q[1] = One`, `q[2] = One`, which means
/// "consumed exactly once". Teleportation then applies 3-4 gates to each qubit:
///
///     q[0]: CNOT, H, measure      -- 3 uses
///     q[1]: H, CNOT, CNOT, measure
///     q[2]: CNOT, X, Z            -- 3 uses
///
/// A gate does not consume a qubit, it entangles it and hands it on. `One` is the
/// annotation for a resource that leaves the program when used.
///
/// The annotation was never enforced, because the fixture's parser DISCARDED the
/// operand names: `H q[1]` became a gate on a fresh anonymous `qir.qubit_alloc()`, so
/// none of the three declared names appeared in the lowered body at all. Wiring
/// linearity into the QIR boundary is what finally surfaced it -- the check counted
/// occurrences of `q[0]`, found zero, and was right to complain.
///
/// So this test pins `Many`, and the gate-count reasoning, so the annotation cannot
/// be "corrected" back to `One` by someone reading `One` as more restrictive and
/// therefore better.
#[test]
fn teleport_qubits_are_annotated_many_not_one() {
    let text = std::fs::read_to_string("tests/fixtures/teleport.pir")
        .or_else(|_| std::fs::read_to_string("compiler/tests/fixtures/teleport.pir"))
        .expect("teleport fixture");
    for q in ["q[0]", "q[1]", "q[2]"] {
        assert!(
            text.contains(&format!("{q} = Many")),
            "{q} must be `Many`. It is a qubit wire, used by several gates, and a \
             gate entangles rather than consumes. `One` means consumed exactly once \
             and is why this fixture was rejected at the QIR boundary."
        );
    }
}

/// The measured classical bits are still `[0]`-style erasures, not qubit wires.
///
/// The other half: correcting the qubits must not have turned the measurement
/// results into reusable values. They are classical bits read out of the system.
#[test]
fn teleport_measurement_bits_keep_their_own_quantities() {
    let text = std::fs::read_to_string("tests/fixtures/teleport.pir")
        .or_else(|_| std::fs::read_to_string("compiler/tests/fixtures/teleport.pir"))
        .expect("teleport fixture");
    assert!(
        text.contains("b0 = Zero") && text.contains("b1 = Zero"),
        "the measured bits are classical and keep their own annotations"
    );
}
