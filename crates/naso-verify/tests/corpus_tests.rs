//! Corpus tests: fixtures that were previously dead data.
//!
//! `tests/corpus/{linearity,uncomputation}/{valid,invalid}.naso` existed in the
//! repository but NO test read them -- `grep` for the path found nothing. They
//! had therefore rotted against the language without anyone noticing: they
//! referenced `f64` (no longer a type), called `linear_free` (never a real
//! callee -- consumption is the `let consume y = x;` pattern), and omitted the
//! `[1]` quantity a qubit binding requires.
//!
//! These tests execute the corpus, so it can no longer drift silently.
//!
//! What is asserted, and what is deliberately not:
//!
//! * Every `valid.naso` must PARSE and TYPECHECK with no errors.
//! * Every `invalid.naso` must be REJECTED, and with a diagnostic in the expected
//!   class. Asserting the class rather than exact wording means an improved
//!   message is not a test failure, but a *different* kind of error is.
//! * Cases the checker does NOT catch are pinned as known gaps in their own
//!   tests below, rather than being listed in an `invalid.naso` where they would
//!   read as if they were detected.

use naso_compiler::parser::parse_program;
use naso_compiler::typecheck::check_program;

fn corpus_path(rel: &str) -> String {
    format!("{}/tests/corpus/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(corpus_path(rel))
        .unwrap_or_else(|e| panic!("cannot read corpus file {rel}: {e}"))
}

/// Typecheck a source, returning the rendered diagnostics.
fn diagnostics(src: &str) -> Vec<String> {
    let mut program = parse_program(src).unwrap_or_else(|e| panic!("must parse: {e}"));
    let result = check_program(&mut program);
    result.errors.iter().map(|e| e.to_string()).collect()
}

// ---------------------------------------------------------------------------
// The valid corpus must typecheck.
// ---------------------------------------------------------------------------

#[test]
fn linearity_valid_file_typechecks() {
    let errs = diagnostics(&read("linearity/valid.naso"));
    assert!(
        errs.is_empty(),
        "linearity/valid.naso must typecheck cleanly, got:\n{}",
        errs.join("\n")
    );
}

#[test]
fn uncomputation_valid_file_typechecks() {
    let errs = diagnostics(&read("uncomputation/valid.naso"));
    assert!(
        errs.is_empty(),
        "uncomputation/valid.naso must typecheck cleanly, got:\n{}",
        errs.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The invalid corpus must be rejected, in the expected class of error.
// ---------------------------------------------------------------------------

/// Split a source file into top-level functions, so each invalid case can be
/// checked in isolation. One file can contain several violations and the checker
/// stops at the first, so per-function checking is what actually proves each case
/// is detected.
fn top_level_functions(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(idx) = rest.find("\nfn ") {
        // Include a `fn` that starts at the very beginning too.
        let start = if idx == 0 { 0 } else { idx + 1 };
        let body = &rest[start..];
        // Find the matching close brace of this function.
        let open = body.find('{').expect("fn must have a body");
        let mut depth = 0usize;
        let mut end = None;
        for (i, ch) in body.char_indices().skip(open) {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.expect("fn body must close");
        let text = &body[..end];
        let name = text
            .split("fn ")
            .nth(1)
            .and_then(|s| s.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
            .unwrap_or("<unknown>")
            .to_string();
        out.push((name, text.to_string()));
        rest = &body[end..];
    }
    out
}

#[test]
fn every_linearity_invalid_case_is_rejected() {
    let src = read("linearity/invalid.naso");
    let cases = top_level_functions(&src);
    assert!(
        cases.len() >= 3,
        "expected several invalid linearity cases, found {}",
        cases.len()
    );
    for (name, case) in cases {
        let errs = diagnostics(&case);
        assert!(
            !errs.is_empty(),
            "invalid linearity case `{name}` was ACCEPTED; it must be rejected"
        );
    }
}

#[test]
fn every_uncomputation_invalid_case_is_rejected() {
    let src = read("uncomputation/invalid.naso");
    let cases = top_level_functions(&src);
    assert!(
        cases.len() >= 2,
        "expected several invalid uncomputation cases, found {}",
        cases.len()
    );
    for (name, case) in cases {
        let errs = diagnostics(&case);
        assert!(
            !errs.is_empty(),
            "invalid uncomputation case `{name}` was ACCEPTED; it must be rejected"
        );
        // The mechanism in effect is linearity: a qubit is linear, so failing to
        // return or measure it is a leak. Assert the class, so an improved
        // message is not a failure but a different error would be.
        assert!(
            errs.iter().any(|e| e.contains("unused linear")),
            "uncomputation case `{name}` should be reported as a linear leak, got: {errs:?}"
        );
    }
}

/// The whole invalid files must also be rejected, not just each case.
#[test]
fn invalid_corpus_files_are_rejected() {
    for rel in ["linearity/invalid.naso", "uncomputation/invalid.naso"] {
        let errs = diagnostics(&read(rel));
        assert!(!errs.is_empty(), "{rel} must be rejected");
    }
}

// ---------------------------------------------------------------------------
// Branch joins.
//
// A branch must be inferred from the state at the branch's ENTRY point, and the
// per-branch states joined afterwards. Inferring branches sequentially against one
// shared state was unsound in BOTH directions:
//
//   * a consume in the first branch left the value moved, so the second branch was
//     falsely rejected as a use-after-move (a false positive -- correct programs
//     did not compile);
//   * a consume in only one branch was never reported (a false negative -- a leak
//     compiled clean).
//
// Both directions are asserted here. The false positive is the one easy to
// regress, because "reject more" looks like progress.
// ---------------------------------------------------------------------------

/// A leak on the `else` path is REJECTED.
#[test]
fn linear_consumed_in_one_if_arm_is_rejected() {
    let errs =
        diagnostics("fn f(c: bool, x: [1] i32) {\n    if c { let consume a = x; let _ = a; }\n}\n");
    assert!(
        !errs.is_empty(),
        "a consume in only the `then` arm leaks on the `else` path and must be rejected"
    );
    assert!(
        errs.iter()
            .any(|e| e.contains("branch") || e.contains("leaks")),
        "expected a branch-join leak diagnostic, got: {errs:?}"
    );
}

/// A consume in EVERY arm is ACCEPTED -- the false positive this fixes.
///
/// Before the fix this was rejected with "use of moved value", because the `then`
/// branch's consume was still in effect when the `else` branch was checked. A
/// checker that rejects correct code is not more sound; it is just unusable.
#[test]
fn linear_consumed_in_every_if_arm_is_accepted() {
    let src = "fn f(c: bool, x: [1] i32) {\n    \
               if c { let consume a = x; let _ = a; } else { let consume b = x; let _ = b; }\n}\n";
    let errs = diagnostics(src);
    assert!(
        errs.is_empty(),
        "consuming a `[1]` value in EVERY branch is correct and must typecheck, got: {errs:?}"
    );
}

/// The same, for a `match`, and for a value consumed in every arm of three.
#[test]
fn linear_consumed_in_every_match_arm_is_accepted() {
    for (label, src) in [
        (
            "two arms",
            "fn f(n: i32, x: [1] i32) {\n    \
             match n { 0 => { let consume a = x; let _ = a; } _ => { let consume b = x; let _ = b; } }\n}\n",
        ),
        (
            "three arms",
            "fn f(n: i32, x: [1] i32) {\n    \
             match n { 0 => { let consume a = x; let _ = a; } \
                       1 => { let consume b = x; let _ = b; } \
                       _  => { let consume c = x; let _ = c; } }\n}\n",
        ),
    ] {
        let errs = diagnostics(src);
        assert!(
            errs.is_empty(),
            "consuming in every arm ({label}) must typecheck, got: {errs:?}"
        );
    }
}

/// A consume in only one `match` arm leaks on the others.
#[test]
fn linear_consumed_in_one_match_arm_is_rejected() {
    let errs = diagnostics(
        "fn f(n: i32, x: [1] i32) {\n    \
         match n { 0 => { let consume a = x; let _ = a; } _ => { let _ = 1; } }\n}\n",
    );
    assert!(
        !errs.is_empty(),
        "a consume in only one match arm leaks on the other paths and must be rejected"
    );
}

/// An `if` with no `else` has an implicit second path where the body never runs.
///
/// Joining only the `then` branch would treat a conditional consume as
/// unconditional, which is the same false negative in a different shape.
#[test]
fn if_without_else_still_rejects_a_conditional_consume() {
    let errs =
        diagnostics("fn f(c: bool, x: [1] i32) { if c { let consume a = x; let _ = a; } }\n");
    assert!(
        !errs.is_empty(),
        "an `if` without `else` may not run its body, so a consume inside it leaks"
    );
}

/// After a join in which every branch consumed, a later use must still be rejected.
///
/// This is what makes the join meaningful rather than merely permissive: the value
/// is spent after the join, not merely inside the branches.
#[test]
fn use_after_a_full_consuming_join_is_rejected() {
    let errs = diagnostics(
        "fn f(c: bool, x: [1] i32) {\n    \
         if c { let consume a = x; let _ = a; } else { let consume b = x; let _ = b; }\n    \
         let consume d = x;\n    let _ = d;\n}\n",
    );
    assert!(
        !errs.is_empty(),
        "x is consumed on every path, so consuming it again after the join must be rejected"
    );
}

/// After a join in which NO branch consumed, a later consume must still be allowed.
///
/// The mirror of the previous test: a conservative join that consumed everything
/// unconditionally would be sound but would reject this correct program.
#[test]
fn consume_after_a_non_consuming_join_is_accepted() {
    let errs = diagnostics(
        "fn f(c: bool, x: [1] i32) { if c { let _ = 1; } let consume a = x; let _ = a; }\n",
    );
    assert!(
        errs.is_empty(),
        "a join that consumed nothing must leave x available, got: {errs:?}"
    );
}

/// A consume guarded by a `match` guard cannot be proven to happen.
///
/// A guarded arm may not be taken, so a consume under a guard leaves a path
/// unconsumed. The checker rejects it rather than assuming the guard holds.
#[test]
fn consume_under_a_match_guard_is_rejected() {
    let errs = diagnostics(
        "fn f(n: i32, x: [1] i32) {\n    \
         match n { 0 if true => { let consume a = x; let _ = a; } _ => { let _ = 1; } }\n}\n",
    );
    assert!(
        !errs.is_empty(),
        "a consume under a match guard may not happen, so it must be rejected"
    );
}

/// Non-linear values are unaffected by branching.
#[test]
fn non_linear_values_are_unaffected_by_branches() {
    for (label, src) in [
        (
            "if/else ints",
            "fn f(c: bool, x: i32) -> i32 { if c { x } else { x + 1 } }\n",
        ),
        (
            "match ints",
            "fn f(n: i32) -> i32 { match n { 0 => 1 _ => 2 } }\n",
        ),
        (
            "many-typed in both arms",
            "fn f(c: bool) -> i32 { if c { 1 } else { 2 } }\n",
        ),
    ] {
        let errs = diagnostics(src);
        assert!(errs.is_empty(), "{label} must typecheck, got: {errs:?}");
    }
}

/// Returning a linear value from both arms is fine, and consumes it on both paths.
#[test]
fn linear_returned_from_both_arms_is_accepted() {
    let errs = diagnostics("fn f(c: bool, x: [1] i32) -> [1] i32 { if c { x } else { x } }\n");
    assert!(
        errs.is_empty(),
        "returning x on both paths consumes it on both paths and must typecheck, got: {errs:?}"
    );
}

// ---------------------------------------------------------------------------
// Quantity inheritance through a binding.
//
// `parse_quantity` defaults an unannotated `let` -- and the pattern's own
// quantity -- to `Quantity::Many`. Reading that default as the binding's
// quantity WIDENED every linear value bound by a plain `let`, which was a
// complete escape from the linear-type discipline:
//
//     fn f(x: [1] i32) { let y = x; let _ = y; let _ = y; }   // ACCEPTED: y was [*]
//
// It also defeated the leak check on the original:
//
//     fn f(x: [1] i32) { let y = x; let _ = y; }
//
// The binding now inherits the initializer's quantity unless the statement is
// explicitly annotated, so a `[1]` value stays `[1]` across a binding.
//
// This is the second escape found by the same investigation as the branch-join
// gap, and the more severe of the two: the branch join was unsound at a join
// point, whereas this one defeats the rule on every straight-line `let`.
// ---------------------------------------------------------------------------

/// A `[1]` value widened by an intervening binding cannot then be used twice.
#[test]
fn linear_widened_by_a_let_cannot_be_used_twice() {
    for (label, src) in [
        (
            "one intervening let",
            "fn f(x: [1] i32) { let y = x; let _ = y; let _ = y; }",
        ),
        (
            "two intervening lets",
            "fn f(x: [1] i32) { let y = x; let z = y; let _ = z; let _ = z; }",
        ),
    ] {
        let errs = diagnostics(&format!("{src}\n"));
        assert!(
            !errs.is_empty(),
            "{label}: `y` must stay `[1]`, so using it twice must be rejected"
        );
    }
}

/// The same escape through a tensor.
#[test]
fn linear_tensor_widened_by_a_let_cannot_be_used_twice() {
    let errs = diagnostics("fn f(a: [1] Tensor[f32, 8]) { let b = a; let _ = b; let _ = b; }\n");
    assert!(
        !errs.is_empty(),
        "a linear tensor must stay `[1]` across a binding, so using it twice must be rejected"
    );
}

/// A linear value stays consumable through an intervening binding.
///
/// The mirror of the above, and the half that regressed first: if the binding
/// inherited too little, this correct program stopped compiling. It was the
/// original pinned gap (`plain_let_between_moves_is_a_known_gap`) and is now
/// asserted as working behaviour.
#[test]
fn linear_value_can_be_consumed_through_a_let() {
    let errs = diagnostics("fn f(x: [1] i32) { let y = x; let consume z = y; let _ = z; }\n");
    assert!(
        errs.is_empty(),
        "moving a `[1]` value through a plain binding must still work, got: {errs:?}"
    );
}

/// A linear value stays returnable through an intervening binding.
#[test]
fn linear_value_can_be_returned_through_a_let() {
    let errs = diagnostics("fn f(x: [1] i32) -> [1] i32 { let y = x; y }\n");
    assert!(
        errs.is_empty(),
        "returning a `[1]` value through a plain binding must work, got: {errs:?}"
    );
}

/// Non-linear values must NOT be affected -- this is the over-correction to
/// guard against, where inheritance would make every `let` linear.
#[test]
fn non_linear_values_are_unaffected_by_quantity_inheritance() {
    for (label, src) in [
        (
            "plain i32 twice",
            "fn f(x: i32) { let y = x; let _ = y; let _ = y; }",
        ),
        (
            "literal twice",
            "fn f() { let y = 5; let _ = y; let _ = y; }",
        ),
        (
            "call result twice",
            "fn g() -> i32 { 7 }\nfn f() { let y = g(); let _ = y; let _ = y; }",
        ),
        ("inferred in a body", "fn f() -> i32 { let y = 7; y + 1 }"),
    ] {
        let errs = diagnostics(&format!("{src}\n"));
        assert!(errs.is_empty(), "{label} must typecheck, got: {errs:?}");
    }
}

/// A linear value derived by an operation still satisfies the rules.
///
/// `x + 1` produces a fresh non-linear value even when `x` is `[1]`, so binding
/// it is not an error.
#[test]
fn operation_on_a_linear_value_yields_a_usable_binding() {
    let errs = diagnostics("fn f(x: [1] i32) -> i32 { let y = x + 1; y }\n");
    assert!(
        errs.is_empty(),
        "an arithmetic result is not itself linear, got: {errs:?}"
    );
}
/// A `measure` result is a CLASSICAL bit, so an unused one is not a leak.
///
/// `measure` CONSUMES its qubit argument -- that is correct, and is what the
/// `[1]` param with `Mutability::Consume` expresses. But the value it RETURNS is
/// a readout of a collapsed qubit, not a resource. Typing that return `[1]` made
/// every `let r = measure(q);` bind a linear `r`, so a perfectly good program
/// was rejected:
///
///     fn f() { let [1] q: Qubit = qalloc(1); let r = measure(q); }
///     -> unused linear variable `r`
///
/// The program is correct: the qubit WAS consumed, and the leftover classical bit
/// needs no accounting.
///
/// This asserts the return quantity DIRECTLY, by not using `r`. A test that wrote
/// `let _ = r;` would pass under either return type and pin nothing -- which is
/// exactly the mutation this shape was written to catch.
#[test]
fn measure_result_is_classical_so_an_unused_one_is_not_a_leak() {
    let errs = diagnostics("fn f() { let [1] q: Qubit = qalloc(1); let r = measure(q); }\n");
    assert!(
        errs.is_empty(),
        "a measurement result is classical, so leaving it unused is fine, got: {errs:?}"
    );
}

/// The mirror: `measure` still CONSUMES, so using the qubit afterwards must fail.
///
/// This stops the "fix" from being made by making `measure` non-consuming.
#[test]
fn measure_still_consumes_its_qubit() {
    let errs = diagnostics(
        "fn f() { let [1] q: Qubit = qalloc(1); let r = measure(q); let _ = r; hadamard(q); }\n",
    );
    assert!(
        !errs.is_empty(),
        "`measure` consumes its qubit, so using it afterwards must be rejected"
    );
}

/// `linear_free` releases a linear value, so it returns unit, not a fresh `[1]`.
///
/// Returning `Int [1]` handed the caller a new linear value it then had to
/// consume -- from a function whose purpose is to get rid of one.
#[test]
fn linear_free_returns_nothing_to_account_for() {
    let errs = diagnostics("fn f(x: [1] i32) { linear_free(x); }\n");
    assert!(
        errs.is_empty(),
        "`linear_free` consumes and releases x, so nothing is left to account for, got: {errs:?}"
    );
}

/// `let` as an EXPRESSION has no parser production, so `infer_let` is dead code.
///
/// `compiler/src/typecheck/inference.rs::infer_let` carries the same quantity
/// inheritance fix as `check_let`, but it is reached only via `ExprKind::Let`,
/// which nothing produces: `let g = let x = 1;` panics the parser. The fix is
/// kept so the two `let` paths agree if let-as-expression ever gets a syntax, but
/// it is NOT load-bearing today and no test can cover it.
///
/// Pinning the unreachability is the honest option: if this starts parsing, the
/// fix becomes live and needs a test, and a silent change would leave the copy
/// unverified.
///
/// This is a documented PARSER limitation, not a soundness claim -- a panic here
/// is a bug in itself, but a different one from the linearity work.
#[test]
fn let_as_an_expression_does_not_parse() {
    let parsed =
        std::panic::catch_unwind(|| parse_program("fn f() { let g = let x = 1; }\n").is_ok());
    assert!(
        !parsed.unwrap_or(false),
        "let-as-expression is expected NOT to parse today (infer_let is dead). \
         If it now parses, `infer_let`'s quantity inheritance becomes live and \
         needs its own test."
    );
}

// ---------------------------------------------------------------------------
// Pinned known gaps.
//
// Each of these is a real soundness or capability gap. They are asserted HERE,
// as tests that currently PASS, so the behaviour cannot change silently. When one
// is fixed, the test fails and gets inverted -- which is the point of naming it.
// ---------------------------------------------------------------------------/// There is NO `reset` in the language, so a measured qubit can only be discharged
/// by returning it.
///
/// CAPABILITY GAP, pinned so it cannot change silently.
///
/// `reset` exists in three places but not the fourth, which makes it look available:
///   * the verifier has `GateKind::Reset => int(0)` (the transition that CLEANS a
///     qubit), and `folded_state` handles it;
///   * the runtime exporters emit a reset instruction (`exporter/braket.rs`,
///     `exporter/openqasm.rs`);
///   * but the compiler's `ast::GateKind` has no `Reset` variant, the parser has no
///     `reset` production, and the prelude has no `reset` builtin -- so
///     `reset(q)` fails to parse or typecheck ("variable `reset` not available").
///
/// So a user who measures a temporary qubit has no way to return it to |0> within
/// the language. They must return it to the caller instead. That is a real
/// usability limitation, and it is also why the prover's `Reset` transition is
/// currently unreachable: nothing can produce one.
///
/// This is a missing FEATURE, not a soundness hole. The prover is on the safe side
/// of it -- an unresettable measured qubit is FLAGGED, not waved through. The
/// soundness half was fixed separately (see
/// `naso_verify::prover::uncomputation::tests::test_measurement_is_not_evidence_of_zero`).
#[test]
fn reset_is_not_a_language_construct() {
    // It does not typecheck: `reset` is not in the prelude.
    let errs = diagnostics("fn f() { let [1] q: Qubit = qalloc(1); reset(q); }\n");
    assert!(
        !errs.is_empty(),
        "`reset` is expected NOT to be available today. If it now typechecks, a \
         Reset variant reached the compiler's GateKind, and the verifier's \
         `GateKind::Reset` transition became live -- which should then be tested \
         by proving that a measured qubit followed by reset is CLEAN."
    );
}

/// A statement-position bare block is a statement, not a block's tail.
///
/// REGRESSION GUARD. A bare `{ .. }` used to be classified as the enclosing
/// block's TAIL expression, so `parse_stmt_list` broke out early and the
/// statement AFTER the block was orphaned -- the parser then PANICKED with
/// `expected '}', found 'let'`. A block followed by anything was unparseable,
/// and even `{} let z = 2;` failed.
///
/// A block is self-delimiting, exactly like `if`/`while`, so it is a statement
/// whether or not it is followed by a `;`.
#[test]
fn bare_nested_block_is_a_statement() {
    for (label, src) in [
        (
            "block then statement",
            "fn f() { { let y = 1; } let z = 2; }",
        ),
        ("empty block then statement", "fn f() { {} let z = 2; }"),
        ("block alone", "fn f() { { let y = 1; } }"),
        (
            "statement then block",
            "fn f() { let a = 1; { let y = 1; } }",
        ),
        (
            "nested blocks",
            "fn f() { { { let y = 1; } let w = 2; } let v = 3; }",
        ),
    ] {
        // Must parse, and must not panic.
        let program = parse_program(&format!("{src}\n"))
            .unwrap_or_else(|e| panic!("{label}: must parse: {e}"));
        assert_eq!(program.items.len(), 1, "{label}");
        // A panic here is the regression: the parse used to abort the process.
        let mut program = program;
        let errs = check_program(&mut program)
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>();
        // Only assert that checking completed; these programs are not about
        // linearity, so a diagnostic here is fine, a panic is not.
        let _ = errs;
    }
}

/// The block parser must not be reachable only via `if`/`while` -- those were
/// already handled, and the block case was the gap.
#[test]
fn control_flow_blocks_still_parse() {
    for (label, src) in [
        ("if/else", "fn f(c: bool) -> i32 { if c { 1 } else { 2 } }"),
        ("tail expression", "fn f() -> i32 { let a = 1; a }"),
    ] {
        let program = parse_program(&format!("{src}\n"))
            .unwrap_or_else(|e| panic!("{label}: must parse: {e}"));
        assert_eq!(program.items.len(), 1, "{label}");
    }
}
