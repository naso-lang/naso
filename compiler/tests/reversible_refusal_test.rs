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

/// The reason `reversible` stays refused must stay TRUE, not merely asserted.
///
/// `reversible_lowering.rs` contains a plausible-looking inverse generator: 831 lines
/// with 12 passing unit tests. It is tempting to wire it up. This test fails the moment
/// any of the three blockers it documents is resolved, so the day real uncomputation
/// becomes possible, whoever does it is told to update the refusal and the matrix
/// together rather than leaving a stale "cannot" in place.
///
/// The three blockers, asserted individually:
///
/// 1. `PirModule` has one `schedule` field and no inverse carrier.
/// 2. `ScheduleNode` has no field that can hold a `PirExpr`, so an inverse tree built
///    from it carries loop structure but no operations.
/// 3. `reversible_lowering` exports entry points nothing calls.
#[test]
fn the_documented_blockers_to_wiring_reversible_are_still_real() {
    use std::collections::BTreeMap;

    // Blocker 1: no inverse carrier on PirModule.
    let fields: BTreeMap<&str, bool> = vec![
        ("statements", true),
        ("schedule", true),
        ("accesses", true),
        ("quantities", true),
    ]
    .into_iter()
    .collect();
    let module_src = include_str!("../src/ir/pir_types.rs");
    let pir_module = module_src
        .split("pub struct PirModule")
        .nth(1)
        .and_then(|s| s.split("}").next())
        .expect("PirModule must be a struct");
    for name in fields.keys() {
        assert!(
            pir_module.contains(&format!("pub {name}:")),
            "PirModule should still have `{name}`"
        );
    }
    assert!(
        !pir_module.contains("inverse"),
        "PirModule now has an `inverse` field, so blocker 1 is GONE. `reversible` may \\
         be wireable -- update the refusal in `lower_reversible_block`, the row in \\
         `capability_matrix_test.rs`, and `docs/content/LIMITATIONS.md` together."
    );

    // Blocker 2: ScheduleNode cannot carry an expression, so an inverse tree built
    // from schedule nodes alone has no operations in it.
    //
    // Checked as a CAPABILITY over every variant's fields, not as the presence of one
    // hard-coded field name. A mutant that adds a differently-named `expr` field is a
    // real capability change and must be caught; asserting on a literal name would miss
    // it, and adding a variant to an enum used by exhaustive matches makes such a
    // mutation uncompilable rather than detectable.
    let schedule_src = include_str!("../src/ir/schedule_tree.rs");
    let node = schedule_src
        .split("pub enum ScheduleNode")
        .nth(1)
        .and_then(|s| s.split("\n}").next())
        .expect("ScheduleNode must be an enum");
    assert!(
        !node.contains("PirExpr"),
        "ScheduleNode can now carry a PirExpr, so blocker 2 is GONE. An inverse schedule \
         could carry real operations -- re-investigate wiring `reversible`."
    );

    // Blocker 3: the entry points are still uncalled.
    let rev_src = include_str!("../src/lowering/reversible_lowering.rs");
    assert!(
        rev_src.contains("pub fn lower_reversible_block"),
        "the entry point should still exist"
    );
    let lowering_src = include_str!("../src/lowering/mod.rs");
    assert!(
        !lowering_src.contains("reversible_lowering::lower_reversible_block"),
        "`mod.rs` now CALLS `reversible_lowering::lower_reversible_block`. Blocker 3 is \\
         GONE and, if the other two still hold, the inverse is being discarded silently \\
         -- which is the defect just fixed. Verify the emitted inverse before trusting it."
    );
}
