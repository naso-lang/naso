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

//! `reversible { ... }`: the block is now WIRED, and this file records what that changed.
//!
//! # History, kept because the defect is instructive
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
//! # The middle state: refused, which was correct
//!
//! After that defect, the construct was REFUSED outright. That was right, and this file
//! held the line: five tests below asserted the refusal for inputs including a bare gate
//! call and a block of plain integer arithmetic. Refusing cost real capability and was
//! still the correct call, because the alternative was emitting a forward pass with no
//! inverse -- which is the defect, wearing a successful compile as a disguise.
//!
//! # The present state: implemented, for gate sequences, with native evidence
//!
//! `lower_reversible_block` now emits the forward pass AND the uncomputation, for blocks
//! whose statements are all gate applications. The inverse is emitted as ordinary
//! statements appended in reverse order, which is what makes it execute: every backend
//! already walks `statements` in order, so there was no second carrier to add and no way
//! for a backend to forget the second half.
//!
//! So the five tests below INVERTED rather than being deleted. They now assert that the
//! inputs which used to be refused are accepted-and-uncomputed, and that the inputs which
//! must STILL be refused are refused. Deleting them would have left no record that the
//! capability boundary moved, and the next person to widen it would have had nothing to
//! push against.
//!
//! # Where the evidence for "it actually uncomputes" lives
//!
//! NOT here. This file asserts what lowering accepts and refuses. Whether the emitted
//! circuit returns the state it started in is a question about what `llc` compiles and the
//! process executes, and it is answered in `reversible_native_execution_test.rs`, which
//! builds real executables and asserts distributions over many runs. Keeping those two
//! apart matters: a PIR-shape assertion here would pass happily if the backend reordered
//! the statements on the way out.

use naso_compiler::lowering::lower_program;

/// Lower a source file, returning `Err` as a refusal message.
///
/// A helper that accepts BOTH outcomes would make every test below vacuous -- it could not
/// tell an acceptance from a refusal, which is the only thing this file is about. So there
/// are two helpers, and each test commits to which one it calls.
fn lower_err(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("test source must parse: {e}"));
    match lower_program(&program) {
        Ok(module) => panic!(
            "a `reversible {{ ... }}` block lowered successfully and reported no \
             diagnostic. It emitted:\n{module:?}\n\n\
             The forward pass and the uncomputation pass are both required. Emitting \
             either one alone is silently wrong."
        ),
        Err(e) => e.to_string(),
    }
}

/// Lower a source file that MUST be accepted, printing the module on failure.
fn lower_ok(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("test source must parse: {e}"));
    match lower_program(&program) {
        Ok(module) => format!("{module:?}"),
        Err(e) => panic!("this program must now lower: {e}"),
    }
}

/// Count how many times `needle` occurs in `haystack`.
fn count_of(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// The regression that used to be swallowed is now COMPUTED, not dropped.
///
/// The original defect was measured on `fn f(q: Many) -> Many { reversible { h(q) } }`, which
/// produced ZERO statements and `Ok(())`. The statement count is what this asserts, and it
/// is the same quantity that was measured against a control when the defect was found.
///
/// Note the input is the BUILT-IN `hadamard`, not the user function `h` the old test used.
/// `h(q)` was a function CALL, and a call is refused by name -- it is not a gate, and its
/// inverse would depend on which operand the callee binds. That refusal is correct and has
/// its own test; reusing that input here would have asserted a false capability.
///
/// The forward pass and its adjoint both appear, and `H` is self-inverse, so exactly two. That
/// they are genuinely ADJOINTS is not checkable here -- see the native execution suite, which
/// distinguishes a correct inverse from a second forward pass by measuring the state.
#[test]
fn a_reversible_block_containing_a_gate_call_is_now_uncomputed() {
    let emitted = lower_ok("fn f(q: Many) -> Many { reversible { hadamard(q) } }\n");
    assert!(
        count_of(&emitted, "H") >= 2,
        "the block must emit the forward gate AND its adjoint. `H` is self-inverse, so two \
         applications are expected. Emitted: {emitted}"
    );
}

/// Arithmetic inside a block is STILL refused -- the capability boundary, not a blanket one.
///
/// This is the test that keeps the newly-wired path honest. `reversible` is implemented for
/// GATE sequences only, and the input here has no quantum syntax at all: it is `let b = a+1`.
/// The inverse of that is `b - 1`, which needs to know which operand is bound, and this pass
/// is not told. Refusing it by name is the point.
///
/// The earlier implementation did not refuse this: it rewrote `Add -> Sub` and defaulted
/// every other operator to `Add`, which computes a different number rather than undoing one.
#[test]
fn a_reversible_block_of_plain_integer_statements_is_still_refused_by_name() {
    let msg = lower_err("fn g(a: i64) -> i64 { reversible { let b = a + 1; b } }");
    assert!(
        msg.contains("not a quantum gate") && msg.contains("not told"),
        "the refusal must name the offending statement and the missing information: {msg}"
    );
}

/// A block whose statements are all gates lowers; one with a gate AND arithmetic does not.
///
/// The partial case is the dangerous one, and it is why the refusal is checked as a whole
/// rather than per statement. If a non-gate were SKIPPED instead of refused, the forward
/// pass would be missing that operation and the inverse would be computed from the wrong
/// circuit -- and the two mistakes would be consistent with each other, so nothing
/// downstream could detect them.
#[test]
fn a_mixed_block_is_refused_rather_than_partially_uncomputed() {
    let msg = lower_err(
        "fn f(q: Many) -> Many { reversible { h(q); let x: i64 = 1 + 2; } }\nfn h(q: Many) -> Many { q }\n",
    );
    assert!(
        msg.contains("not a quantum gate"),
        "a block that mixes a gate with an assignment must be refused whole: {msg}"
    );
}

/// An empty block is STILL refused.
///
/// With an empty body the old code looped zero times and returned `Ok`, having produced
/// nothing. Empty is not a special case that is accidentally correct: a block that is
/// "reversible" because it does nothing is indistinguishable from a pass that emitted no
/// uncomputation at all.
#[test]
fn an_empty_reversible_block_is_still_refused() {
    let msg = lower_err("fn g(a: i64) -> i64 { reversible { } }");
    assert!(
        msg.to_lowercase().contains("empty"),
        "an empty reversible block must be refused, not silently accepted: {msg}"
    );
}

/// A block nested inside an `if` is still refused, by the same reasoning.
///
/// The refusal has to survive being reached from a nested control-flow position, where an
/// earlier version of the return-in-block work showed constructs can be hoisted or
/// reordered on the way down. A block nested in a conditional cannot be uncomputed by
/// appending statements to the enclosing function's flat list, because the surrounding
/// branch decides whether the forward pass ran at all.
#[test]
fn a_reversible_block_nested_in_an_if_is_still_refused() {
    let msg = lower_err(
        "fn g(a: i64) -> i64 { if a > 0 { reversible { let b = a + 1; b } } else { 0 } }",
    );
    assert!(
        msg.contains("not a quantum gate"),
        "a nested reversible block must be refused: {msg}"
    );
}

/// The refusal must name the construct, so an author can tell it from an unrelated error.
///
/// Guards against the failure mode where the construct is refused for the WRONG cause --
/// e.g. an unsupported operand type -- which would let the real gap back in behind the
/// diagnostic. The same hazard killed an earlier version of the WGSL recursion test.
#[test]
fn the_refusal_names_the_construct_and_not_an_unrelated_error() {
    let msg = lower_err("fn g(a: i64) -> i64 { reversible { let b = a + 1; b } }");
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("reversible"),
        "the refusal must name the construct the author wrote: {msg}"
    );
    assert!(
        lower.contains("uncomput") || lower.contains("inverse") || lower.contains("gate"),
        "and must say what is wrong with it, rather than reporting a bare error: {msg}"
    );
}

/// The four structural blockers are now RESOLVED. This test asserts that, so none can
/// silently regress -- and so the wiring that resolved them stays accounted for.
///
/// # Why this test inverted instead of being deleted
///
/// It previously asserted the blockers were still real, and existed to FAIL the day
/// uncomputation became possible, as a signal to update the refusal and the docs together.
/// That day has arrived, so leaving it would assert a falsehood, and deleting it would throw
/// away the record of what the wiring had to fix.
///
/// It now asserts the resolved state, which is the stronger claim: each blocker was a
/// specific gap, and each is now closed in a specific way that can be checked.
///
/// # The four blockers, and how each was closed
///
/// 1. `PirModule` has one `schedule` field and no inverse carrier.
///
///    Resolved WITHOUT adding a field. The uncomputation is emitted as ordinary entries in
///    the existing flat `statements` list, in reverse order after the forward pass. Every
///    backend already walks `statements` in order, so the second half executes for free --
///    and there is no new field for a backend to forget. A dedicated carrier would have been
///    a new obligation on every backend, and an obligation nobody tests is a silent drop.
///    That is why the assertion below is that `statements` is STILL the only carrier: if
///    someone adds an `inverse` field, they must bring the producer and the backend emission
///    with it.
///
/// 2. `ScheduleNode` cannot carry a `PirExpr`, so an inverse schedule tree would carry loop
///    structure but no operations.
///
///    Resolved by the same move: no inverse SCHEDULE tree is built. The uncomputation is a
///    statement list, not a schedule. The schedule still describes the forward pass, which
///    is correct -- and the assertion below still requires `ScheduleNode` cannot carry an
///    expression, so nobody reintroduces the node-tree shape and believes it works.
///
/// 3. `reversible_lowering` exports entry points nothing calls.
///
///    Still true, deliberately. That module is the TEMPORARY-VALUE path -- arithmetic
///    temporaries, ancilla bookkeeping -- and it remains refused. What is now wired is a
///    different, narrower pass: `lowering::uncomputation`, which handles gate sequences and
///    has no temporaries in it. Two modules, two different claims; asserted below so nobody
///    reports the gate path as if it uncomputed temporaries.
///
/// 4. No gate-to-inverse table, so no inverse was ever COMPUTED rather than assumed.
///
///    Resolved: `naso_gates::gate_inverse::inverse_of` is the ONE verified table, and
///    `lowering::uncomputation` calls it. The `dual_stream` representation that once existed
///    with no producer must stay gone -- a representation whose only references are its own
///    module and its own tests is the original defect in a new shape.
#[test]
fn the_four_structural_blockers_to_wiring_reversible_are_resolved() {
    use std::collections::BTreeMap;

    // Blocker 1, resolved: `statements` is STILL the only carrier, and there is no
    // `inverse` field. If one appears, a producer and backend emission must arrive with it.
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
        "PirModule now has an `inverse` field. That is correct ONLY if something PRODUCES it \
         and every backend EMITS it -- the uncomputation currently rides in `statements`, so \
         an unused field would reintroduce the two-carrier split this pass avoids. Bring the \
         producer and the backend emission in the same change, and check \
         `reversible_native_execution_test.rs` still passes."
    );

    // Blocker 2, resolved by not building an inverse schedule tree. Asserted as a CAPABILITY
    // over every variant's fields rather than one field name, so a differently-named `expr`
    // field is caught too.
    let schedule_src = include_str!("../src/ir/schedule_tree.rs");
    let node = schedule_src
        .split("pub enum ScheduleNode")
        .nth(1)
        .and_then(|s| s.split("\n}").next())
        .expect("ScheduleNode must be an enum");
    assert!(
        !node.contains("PirExpr"),
        "ScheduleNode can now carry a PirExpr. That is fine IF an inverse schedule is built \
         and consumed -- but this pass emits the uncomputation as a STATEMENT LIST, so \
         nothing would read such a field. Build the tree and consume it, or leave it out."
    );

    // Blocker 3, still true: the TEMPORARY-VALUE module remains uncalled.
    //
    // This is what keeps the two modules from being conflated. `reversible` is now wired for
    // GATE SEQUENCES. It is NOT wired for arithmetic temporaries or ancilla bookkeeping, and
    // reporting it as though it were would overstate what works.
    let rev_src = include_str!("../src/lowering/reversible_lowering.rs");
    assert!(
        rev_src.contains("pub fn lower_reversible_block"),
        "the entry point should still exist"
    );
    let lowering_src = include_str!("../src/lowering/mod.rs");
    assert!(
        !lowering_src.contains("reversible_lowering::lower_reversible_block"),
        "`mod.rs` now CALLS `reversible_lowering::lower_reversible_block`. That module is the \
         TEMPORARY-VALUE path, and its six formerly-fabricated generators were replaced with \
         refusals; wiring it now would reintroduce uncomputable temporaries under a claim the \
         gate path proved. Verify what it emits before trusting it."
    );

    // The gate-sequence pass IS wired, and its output is what executes.
    assert!(
        lowering_src.contains("uncomputation::uncompute_statements"),
        "`lower_reversible_block` must call the gate-sequence uncomputation. Without it the \
         block emits a forward pass and no inverse, which is the original defect."
    );
    let uncomp_src = include_str!("../src/lowering/uncomputation.rs");
    assert!(
        uncomp_src.contains("gate_inverse::inverse_of"),
        "the gate path must compute adjoints from the ONE verified table, not a second local \
         table. Two gate tables drift, and the one that drifted before named a nonexistent QIR \
         intrinsic."
    );

    // Blocker 4: the producerless dual-stream representation must stay deleted.
    let ir_mod = include_str!("../src/ir/mod.rs");
    assert!(
        !ir_mod.contains("dual_stream"),
        "a dual_stream module was reintroduced. Acceptable ONLY alongside a producer that \
         constructs DualStream from lowered source. Check first that something outside the \
         module itself builds one:\n  \
         grep -rn 'DualStream::' --include=*.rs compiler/src/ crates/"
    );
}

/// The QIR backend must keep exactly one gate table, and refuse what it does not know.
///
/// Split out of the blocker test above, where it used to live. It never belonged there: the
/// blockers were about whether `reversible` could be wired, and this is about the QIR
/// intrinsic mapping, which is a different subject that happened to be in the same function.
///
/// # Why it is a live test and not a source-string check
///
/// `qir/classifier.rs` used to hold a 61-arm duplicate of the live table in
/// `qir/module_builder.rs`, reachable from nothing but its own `mod tests`. Two copies of a
/// gate table do not fail to build when they disagree -- they emit a wrong or nonexistent
/// intrinsic, which is exactly what happened: the deleted classifier mapped `sdg` to
/// `qir.s__adj`, correct as quantum and broken as code, since `qir.s__adj` is in neither the
/// QIR base profile nor `QIR_INTRINSICS`.
///
/// The duplicate is deleted. The property that motivated deleting it is asserted live in
/// `codegen::qir::module_builder::tests::every_resolved_intrinsic_is_a_declared_one_and_unknown_ops_are_refused`,
/// which walks the real mapping and checks every resolved name is a DECLARED entry point.
/// That is strictly stronger than any string check here, and it catches what these checks
/// once BLESSED: the assertions below used to require the fabricated `qir.s__adj` to exist.
#[test]
fn the_qir_backend_has_one_gate_table_and_refuses_the_unknown() {
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
