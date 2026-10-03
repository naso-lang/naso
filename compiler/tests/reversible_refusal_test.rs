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
/// 4. There is no gate-to-inverse table, so no operation's inverse is COMPUTED rather
///    than assumed. A forward/inverse stream representation was built and then deleted,
///    because a representation with no producer is an unreferenced scaffold -- 906 lines
///    whose tests only tested themselves. Rebuilding it means bringing the inverse
///    producer with it, not ahead of it.
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
    // Blocker 4: no gate-to-inverse table, so no inverse is ever computed.
    //
    // Checked by the ABSENCE of the representation that had no producer. If someone
    // reintroduces a forward/inverse stream module without also introducing the producer
    // that builds one, this fails -- which is the point. A representation whose only
    // references are its own module and its own tests is a scaffold, and shipping it
    // repeats the original failure in a new shape: everything compiles, nothing emits.
    let ir_mod = include_str!("../src/ir/mod.rs");
    assert!(
        !ir_mod.contains("dual_stream"),
        "a dual_stream module was reintroduced. That is acceptable ONLY alongside a \
         producer that constructs DualStream from lowered source. Check first that \
         something outside the module itself builds one:\n  \
         grep -rn 'DualStream::' --include=*.rs compiler/src/ crates/"
    );

    // And confirm the classifier does not quietly claim S-dagger is S.
    //
    // `classifier.rs` mapped "sdg" onto the same intrinsic as "s", with a comment saying it
    // "uses same intrinsic with different args". S-dagger is the ADJOINT of S: S sends
    // |1> to i|1>, S-dagger to -i|1>. Every downstream amplitude differs. The file is
    // undeclared and inert, so nothing was wrong today -- but an inverse table built from
    // that mapping would compute wrong inverses, silently. FIXED, and asserted here so it
    // cannot come back.
    let classifier = include_str!("../src/codegen/qir/classifier.rs");
    assert!(
        !classifier.contains("\"sdg\" => \"qir.s\","),
        "classifier.rs maps sdg onto qir.s. S-dagger is the ADJOINT of S, not S with \
         different arguments; applying S where S-dagger was written computes a different \
         state."
    );
    assert!(
        classifier.contains("\"sdg\" => \"qir.s__adj\""),
        "sdg must map to the adjoint entry point qir.s__adj."
    );
    assert!(
        classifier.contains("\"tdg\" => \"qir.t__adj\""),
        "tdg must map to the adjoint entry point qir.t__adj."
    );

    // A fabricated default is the same defect wearing a different hat: an unrecognised
    // gate used to resolve to "qir.h", so a MIS-SPELLED gate name compiled and applied a
    // Hadamard. Every default arm must now be a sentinel that callers can refuse on, and
    // no arm may resolve a gate to Hadamard by default.
    assert!(
        !classifier.contains("_ => \"qir.h\""),
        "an unknown gate must not resolve to qir.h. A mis-spelled gate name would compile \
         and apply a Hadamard -- a specific wrong answer instead of a refusal."
    );
    assert!(
        !classifier.contains("NonReversible => \"qir.h\""),
        "a non-reversible operation must not resolve to qir.h either."
    );
}

/// The gate-inverse table exists and is verified, and NO PRODUCER USES IT YET.
///
/// Step 4 of the reversible work built `naso_verify::gate_inverse::inverse_of`, checked
/// numerically against a CPU state-vector simulator: every gate composed with its table
/// entry returns the original state, and the historical `S`-dagger defect is asserted
/// impossible.
///
/// The remaining blocker is narrower than it was and is stated here so it is not lost: no
/// lowering pass constructs an inverse operation from this table. `reversible` stays refused
/// because the inverse representation cannot yet be EMITTED by a backend, not because the
/// inverse is unknown. Wiring the table into a producer without also giving
/// `ScheduleTree` and the LLVM backend a way to carry an inverse `PirExpr` would reintroduce
/// exactly the drop-the-inverse defect that was fixed earlier.
#[test]
fn the_gate_inverse_table_has_a_verified_entry_for_every_gate_but_no_producer() {
    let table = include_str!("../../crates/naso-gates/src/gate_inverse.rs");
    assert!(
        table.contains("pub fn inverse_of"),
        "the gate-inverse table is missing from naso-gates"
    );

    // The table must exist in EXACTLY ONE place.
    //
    // It was moved out of naso-verify and into the leaf crate naso-gates, because
    // naso-verify depends on naso-compiler and so the compiler could not reach a table that
    // lived there. The tempting alternative -- keeping a copy in each crate -- is what this
    // assertion forbids: a copy of an adjoint table does not fail to build when the two
    // disagree, it silently computes wrong inverses. One definition, reachable from every
    // layer, is the only arrangement that makes that impossible.
    for (label, path) in [
        ("compiler", include_str!("../src/lib.rs")),
        (
            "naso-verify",
            include_str!("../../crates/naso-verify/src/lib.rs"),
        ),
    ] {
        let defines_inverse = path.contains("fn inverse_of");
        assert!(
            !defines_inverse,
            "{label} defines its own `inverse_of`. There must be ONE gate-inverse table, in \
             naso-gates; two copies will drift and drift in an adjoint table silently \
             computes wrong inverses rather than failing to compile."
        );
    }

    // And naso-verify must reach the single table by re-export, not by owning a second one.
    assert!(
        include_str!("../../crates/naso-verify/src/lib.rs")
            .contains("pub use naso_gates::{gate_inverse, statevector}"),
        "naso-verify must re-export naso-gates rather than reimplement it"
    );

    // And confirm nothing in the COMPILER consumes it yet. This is the honest state of the
    // work: infrastructure without a caller is not a feature, and treating it as one would
    // repeat the dual_stream failure.
    let lowering = include_str!("../src/lowering/mod.rs");
    assert!(
        !lowering.contains("gate_inverse") && !lowering.contains("inverse_of"),
        "lowering now references the gate-inverse table. That is progress, and it is only \
         correct if the emitted inverse survives every backend: check that ScheduleTree and \
         the LLVM emitter carry an inverse PirExpr rather than dropping it."
    );
}
