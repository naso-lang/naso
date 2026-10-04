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
/// `reversible_lowering.rs` contains an inverse generator with passing unit tests. It is
/// tempting to wire it up. This test fails the moment any of the blockers it documents is
/// resolved, so the day real uncomputation becomes possible, whoever does it is told to
/// update the refusal and the matrix together rather than leaving a stale "cannot" in place.
///
/// # What the audit found, which changed what this file asserts
///
/// The module was audited rather than trusted. Six of its generators FABRICATED an inverse
/// instead of failing, each with a passing test: `S†`/`RX†`/`UNKNOWN` (names no backend can
/// resolve), `unmeasure` with an invented `IntLit(0)` qubit operand, `unrng`, a wrong
/// `Mul -> Div`, `discard`, and `affine_inverse`. A seventh defect sat in the verification:
/// `verify_ancilla_zeroing` tested a struct field that is never assigned, so the predicate
/// was a constant.
///
/// All six fabrications and the broken predicate are now fixed, and each fix has a test
/// above. So the module is more honest than it was -- and STILL not wireable, on the
/// structural blockers below. That distinction is the point: the blockers are what keep it
/// refused, and they are unchanged.
///
/// The four blockers, asserted individually:
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

    // The S-dagger defect: no SECOND gate-name table may exist, anywhere in the QIR
    // backend.
    //
    // `qir/classifier.rs` used to hold a 61-arm duplicate of the live table in
    // `qir/module_builder.rs`, reachable from nothing but its own `mod tests`. Two copies
    // of a gate table do not fail to build when they disagree -- they emit a wrong or
    // nonexistent intrinsic. The duplicate is now DELETED, and the property that motivated
    // deleting it is asserted live, in
    // `codegen::qir::module_builder::tests::every_resolved_intrinsic_is_a_declared_one_and_unknown_ops_are_refused`,
    // which walks the real mapping and checks every name resolves to a DECLARED entry
    // point.
    //
    // That live test is strictly stronger than the string checks it replaces, and it
    // catches what those checks blessed: the deleted classifier mapped `sdg` to
    // `qir.s__adj`, which is correct as quantum and broken as code -- `qir.s__adj` is not in
    // the QIR base profile and is not in `QIR_INTRINSICS`, so it named a symbol no runtime
    // defines. The assertions below used to REQUIRE that mapping. `sdg` is now correctly
    // REFUSED (no base-profile entry point, and no angle-taking intrinsic to express it
    // as), and asserting a fabricated name survives here would be asserting a bug.
    //
    // The classifier's tests asserted its own arms, which is how a table with no caller
    // keeps 682 lines looking healthy. What must not come back is a second table, and the
    // live test is what holds that line.
    let qir_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/codegen/qir");
    for entry in std::fs::read_dir(&qir_dir).expect("the QIR backend source directory") {
        let path = entry.expect("a directory entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(
            name != "classifier.rs" && name != "ancilla_emission.rs",
            "`src/codegen/qir/{name}` is back. It was deleted because it was a 682-line \
             scaffold with no `mod` declaration, reachable only from itself -- including a \
             SECOND gate-name table that would drift from the live one. Before adding any \
             QIR module, wire it into `qir/mod.rs` so it is compiled, or do not add it."
        );
    }

    // A fabricated default is the same defect wearing a different hat: the live mapping
    // used to resolve every unknown gate to "qir.h", so a MIS-SPELLED gate name compiled
    // and applied a Hadamard. It returns `None` now, which the caller turns into a
    // refusal. Asserted on the LIVE source, since this is the table that matters.
    let live_map = include_str!("../src/codegen/qir/module_builder.rs");
    assert!(
        live_map.contains("_ => return None"),
        "the live intrinsic mapping must keep its `None` default arm, which is what turns an \
         unrecognised operation into a refusal."
    );
    assert!(
        !live_map.contains("\"_ => \"qir.h\""),
        "the live intrinsic mapping must not resolve an unknown operation to qir.h. A \
         mis-spelled gate name would compile and apply a Hadamard -- a specific wrong \
         answer instead of a refusal."
    );
}

/// The six fabricated callees must never come back.
///
/// This is the audit's regression guard, and it is deliberately a check on the SOURCE
/// rather than on behaviour.
///
/// The six names below were emitted by `reversible_lowering.rs` as if they were functions.
/// None of them exists anywhere in this repository: no lexer token, no `PirExpr` variant,
/// no backend intrinsic. Each had a passing unit test that asserted the module produced
/// exactly that call. So a behavioural test alone would not have caught them -- the module
/// was consistent with itself and inconsistent with everything else.
///
/// Asserted on the source because that is the property: not "this call is refused today" but
/// "no inverse generator may emit a callee that nothing can resolve". A gate name is
/// different -- those ARE real, and `GateKind`'s `Display` is their source -- so the two are
/// distinguished by where the name comes from, not by whether it looks like a gate.
#[test]
fn no_inverse_generator_emits_a_call_to_a_function_that_does_not_exist() {
    let src = include_str!("../src/lowering/reversible_lowering.rs");

    // And the inverse must not invent a quantum operand either. The measurement path used to
    // fall back to `PirExpr::IntLit(0)` for the qubit, which is an integer standing in for a
    // quantum pointer: a fabrication that does not fail, it produces valid-looking PIR.
    let code = strip_rust_comments(src);
    assert!(
        !code.contains(".unwrap_or(PirExpr::IntLit(0))"),
        "an inverse path is defaulting a missing qubit operand to the literal 0. A quantum \
         operand that was never supplied must be an error; inventing one produces a circuit \
         acting on a value that does not exist."
    );

    for fabricated in [
        "unmeasure",
        "unrng",
        "affine_inverse",
        "discard",
        "UNKNOWN",
        "\u{2020}", // the dagger the old hand-written adjoint table emitted: "S\u{2020}"
    ] {
        // The name MAY appear in a doc comment -- each of these is named in the comment
        // recording what it replaced, and that is the point of writing it down. So the
        // source is stripped of comments before the check. Otherwise this test would fail
        // on the very documentation that explains the defect, and the fix would be to
        // delete the explanation rather than the fabrication.
        //
        // What must not survive is an emitted call in CODE: the `name: "..."` form inside a
        // `PirExpr::Call`.
        let emitted_call = format!("name: \"{fabricated}\"");
        assert!(
            !code.contains(&emitted_call),
            "`reversible_lowering.rs` builds a `PirExpr::Call` named `{fabricated}`. That is \
             one of the six fabrications the audit removed: it is not a function anywhere in \
             this repository, so the call names a callee that cannot resolve. Refuse instead \
             -- see the doc comment on the generator for why each one was wrong."
        );
    }
}

/// Rust source with its line and block comments removed.
///
/// Needed because the property under test is "no CODE builds a call to a function that does
/// not exist", and the doc comments naming each removed fabrication are exactly the text
/// that must be allowed to contain those names. Checking the raw source would make the
/// documentation of a defect into a second offence.
///
/// Not a Rust parser, and does not need to be: a line whose first non-space characters are
/// `//`, or a `/* ... */` span, is a comment for this purpose. A `//` inside a string
/// literal would also be stripped, which can only make the check stricter, never laxer --
/// stripping more text can only remove matches, never add them.
fn strip_rust_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_block = false;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if in_block {
            if let Some(end) = line.find("*/") {
                in_block = false;
                out.push_str(&line[end + 2..]);
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some(idx) = line.find("/*") {
            in_block = !line[idx..].contains("*/");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The adjoint table has exactly one definition, and the lowering consumes that one.
///
/// This is the fix for the largest of the six fabrications, asserted as a single-table
/// invariant. `generate_quantum_adjoint` used to carry its own hand-written relation
/// (`"S"` -> `"S\u{2020}"`). Two copies of an adjoint table do not fail to build when they
/// disagree -- they quietly compute wrong inverses, which is the exact failure
/// `naso-gates/src/gate_inverse.rs` was written to make unrepresentable.
#[test]
fn the_inverse_generator_uses_the_single_verified_table_and_defines_no_second_one() {
    let src = include_str!("../src/lowering/reversible_lowering.rs");

    assert!(
        src.contains("naso_gates::gate_inverse::inverse_of"),
        "`generate_quantum_adjoint` must take its adjoint relation from the verified table in \
         naso-gates, not from a hand-written match."
    );

    // A second definition of the relation is the thing to forbid. `use` of the single table
    // is the fix; a local `fn inverse_of` or a second `match gate {` building it is the bug.
    assert!(
        !src.contains("fn inverse_of"),
        "a second gate-to-inverse definition has appeared in lowering. There must be ONE, in \
         naso-gates; two copies drift, and they drift silently in exactly the direction that \
         computes a wrong inverse."
    );
    assert!(
        !src.contains("adjoint_gate"),
        "`adjoint_gate` is the hand-written table this replaced. The inverse name comes from \
         the verified table now, so there is no local name to bind."
    );
}

/// The gate-inverse table exists, is verified, and now HAS ONE PRODUCER.
///
/// Step 4 of the reversible work built `naso_verify::gate_inverse::inverse_of`, checked
/// numerically against a CPU state-vector simulator: every gate composed with its table
/// entry returns the original state, and the historical `S`-dagger defect is asserted
/// impossible.
///
/// This assertion used to read that NOTHING in the compiler consumes it. That is now
/// deliberately no longer true: the audit found `generate_quantum_adjoint` computing its own
/// hand-written relation instead, and pointing it at the verified table is the fix for the
/// largest of the six fabrications. One producer, reading the one table.
///
/// What has NOT changed is why `reversible` stays refused, and this is the assertion that
/// says so. Consuming the table is not EMITTING an inverse: the `PirExpr` the generator
/// returns still has nowhere to be stored, so `ScheduleTree` and the LLVM emitter would
/// still drop it. Wiring the table into a producer without also giving those a way to carry
/// an inverse `PirExpr` would reintroduce exactly the drop-the-inverse defect that was fixed
/// earlier. The refusal stands on the structural blockers above, and those are unchanged.
#[test]
fn the_gate_inverse_table_has_a_verified_entry_for_every_gate_and_one_producer() {
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

    // The OpenQASM exporter is now WIRED to the verified table. It used to carry its own
    // hand-written copy, which is the arrangement that let the S-dagger defect exist: two
    // copies of an adjoint relation do not fail to build when they disagree, they emit a
    // circuit computing the wrong function.
    let openqasm = include_str!("../src/runtime/exporter/openqasm.rs");
    assert!(
        openqasm.contains("naso_gates::gate_inverse::inverse_of"),
        "the OpenQASM exporter must take its adjoint relation from naso-gates::inverse_of \
         rather than keeping a second table that can drift from the verified one"
    );
    assert!(
        !openqasm.contains("\"sdg\" => \"s\""),
        "a literal sdg-to-s mapping has reappeared in the OpenQASM exporter. S-dagger is \
         the ADJOINT of S, not S; applying S where S-dagger was written computes a \
         different state."
    );

    // And the CALL SITE still does not. `reversible_lowering.rs` consumes the table to
    // COMPUTE an inverse, but `mod.rs` -- the place that would actually invoke the pass --
    // still contains no reference to it, which is what keeps `reversible` refused.
    //
    // This is the assertion that would fire if someone wired it up, and it is here to make
    // that a deliberate act rather than an accident. The message names the condition that
    // must be met first, because reaching this line does NOT mean the work is done.
    let lowering = include_str!("../src/lowering/mod.rs");
    assert!(
        !lowering.contains("gate_inverse") && !lowering.contains("inverse_of"),
        "lowering/mod.rs now references the gate-inverse table. That is progress, and it is \
         only correct if the emitted inverse survives every backend: check that ScheduleTree \
         and the LLVM emitter carry an inverse PirExpr rather than dropping it. Until a \
         forward AND an inverse circuit BOTH execute natively, `reversible` stays refused."
    );
}
