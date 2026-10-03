//! A statement-position `reversible { ... }` must be REFUSED, not silently dropped.
//!
//! # The defect this file exists for
//!
//! `LoweringContext::lower_reversible_block` used to look like this:
//!
//! ```ignore
//! for stmt in &block.body.stmts { self.lower_stmt(stmt)?; }
//! // Create schedule tree for reversible block
//! // (simplified - full implementation in reversible_lowering.rs)
//! Ok(())
//! ```
//!
//! Measured, against a control:
//!
//! ```text
//! fn g(a: i64) -> i64 { let b = a + 1; b }                  -> 2 statements
//! fn g(a: i64) -> i64 { reversible { let b = a + 1; b } }  -> 1 statement
//! fn f(q: Many) -> Many { h(q) }                            -> 1 statement
//! fn f(q: Many) -> Many { reversible { h(q) } }             -> 0 statements
//! ```
//!
//! Two separate failures in one function. The block body was dropped, and -- worse --
//! no inverse was generated either. The "schedule tree" that was never built WAS the
//! uncomputation pass. `reversible { h(q) }` is defined as "apply `h`, then uncompute
//! it"; what it emitted instead was a partial forward walk and `Ok(())`.
//!
//! # Why the already-landed `PirExpr::Reversible` refusals did not catch it
//!
//! The LLVM and QIR backends both refuse a `PirExpr::Reversible` rather than emitting a
//! circuit with no adjoint. Neither guard fires here, because this path never CONSTRUCTS
//! a `PirExpr::Reversible`. It emits no node for a backend to reject, so the whole guard
//! layer is bypassed.
//!
//! That is the same shape as the Cranelift backend, which returned a hardcoded `42` for
//! every program: the defect was visible only by asking what this path does with its
//! input, not by asking whether the input is correct.

#![cfg(feature = "llvm")]

use naso_compiler::lowering::{LoweringError, lower_program};

/// Lower a source file, expecting the reversible-block refusal.
///
/// `Ok(_)` is a failure, and the panic prints the module so the shape of what was
/// silently produced is visible in the test output.
fn refuse(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("test source must parse: {e}"));
    match lower_program(&program) {
        Ok(module) => panic!(
            "a `reversible {{ ... }}` block lowered successfully and reported no \
             diagnostic. It emitted:\n{module:?}\n\n\
             The forward pass and the uncomputation pass are both required. Emitting \
             either one alone is silently wrong."
        ),
        Err(LoweringError::Unsupported(msg)) => msg,
        Err(other) => panic!(
            "the refusal must be `LoweringError::Unsupported` naming the construct, \
             not a different error that hides the real cause. Got: {other}"
        ),
    }
}

/// The regression: a gate call inside `reversible` produced ZERO statements and `Ok`.
///
/// This is the exact input that was fully swallowed. Note the callee is defined AFTER
/// the caller, so a correct lowering could not have gotten away with source order.
#[test]
fn a_reversible_block_containing_a_gate_call_is_refused() {
    let msg = refuse(
        "\
fn f(q: Many) -> Many { reversible { h(q) } }
fn h(q: Many) -> Many { q }
",
    );
    assert!(
        msg.contains("reversible"),
        "the refusal must name the construct: {msg}"
    );
}

/// A reversible block over plain integer statements was ALSO dropped, silently.
///
/// Worth its own test because it has no quantum syntax at all: someone could easily
/// conclude the construct only misbehaves on qubits and leave this path unfixed.
#[test]
fn a_reversible_block_of_plain_integer_statements_is_refused() {
    let msg = refuse("fn g(a: i64) -> i64 { reversible { let b = a + 1; b } }");
    assert!(
        msg.contains("reversible"),
        "the refusal must name the construct: {msg}"
    );
}

/// The refusal must report how much was being dropped.
///
/// An author seeing "unsupported" with no detail cannot tell a `reversible` block from
/// an unrelated construct, and will not know the block was the problem.
#[test]
fn the_refusal_reports_how_many_statements_were_dropped() {
    let msg = refuse("fn g(a: i64) -> i64 { reversible { let b = a + 1; let c = b + 2; c } }");
    assert!(
        msg.contains('2') && msg.contains("statement"),
        "the refusal should say how many statements were dropped: {msg}"
    );
}

/// A reversible block with no statements is still refused, not waved through.
///
/// With an empty body the old code looped zero times and returned `Ok`, having produced
/// nothing. Empty is not a special case that is accidentally correct.
#[test]
fn an_empty_reversible_block_is_still_refused() {
    let msg = refuse("fn g(a: i64) -> i64 { reversible { } }");
    assert!(
        msg.contains("reversible"),
        "an empty reversible block must be refused, not silently accepted: {msg}"
    );
}

/// A reversible block nested inside an `if` is refused too.
///
/// The refusal has to survive being reached from a nested control-flow position, where
/// an earlier version of the return-in-block work showed constructs can be hoisted or
/// reordered on the way down.
#[test]
fn a_reversible_block_nested_in_an_if_is_refused() {
    let msg =
        refuse("fn g(a: i64) -> i64 { if a > 0 { reversible { let b = a + 1; b } } else { 0 } }");
    assert!(
        msg.contains("reversible"),
        "a nested reversible block must be refused: {msg}"
    );
}

/// The refusal must not be satisfied by refusing for an unrelated reason.
///
/// Guards against the failure mode where the construct is refused for the WRONG cause --
/// e.g. an unsupported operand type -- which would let the real gap back in behind the
/// diagnostic. The same hazard killed an earlier version of the WGSL recursion test.
#[test]
fn the_refusal_names_uncomputation_not_an_unrelated_construct() {
    let msg = refuse("fn g(a: i64) -> i64 { reversible { let b = a + 1; b } }");
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("uncomput") || lower.contains("inverse"),
        "the refusal must say that the uncomputation pass is missing, since that is \
         the whole point of the construct: {msg}"
    );
}
