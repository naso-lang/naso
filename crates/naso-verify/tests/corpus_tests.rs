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
// Pinned known gaps.
//
// Each of these is a real soundness or capability gap. They are asserted HERE,
// as tests that currently PASS, so the behaviour cannot change silently. When one
// is fixed, the test fails and gets inverted -- which is the point of naming it.
// ---------------------------------------------------------------------------

/// GAP: a linear value consumed in only ONE arm of an `if` is accepted.
///
/// The checker does not join the two branches' move sets, so the missing
/// consumption in the other arm goes unnoticed. This is a soundness gap: a
/// resource that should be proven consumed is not.
#[test]
fn branch_local_linear_leak_is_a_known_gap() {
    let src = "fn f(c: bool, x: [1] i32) {\n    if c { let consume a = x; let _ = a; }\n}\n";
    let errs = diagnostics(src);
    assert!(
        errs.is_empty(),
        "KNOWN GAP: expected this to be accepted today. If it is now rejected, \
         the gap is fixed -- move this case into linearity/invalid.naso, delete \
         this test, and fix the fixture comment. Got: {errs:?}"
    );
}

/// GAP: a plain `let` between a move and a later `consume` widens `[1]` to `[*]`.
///
/// `let y = x;` yields a `[1]` binding, but re-binding it with
/// `let consume z = y;` then fails with a quantity mismatch, because the
/// intervening plain `let` widened the quantity. The workaround is to consume
/// directly, which is what the valid corpus does.
#[test]
fn plain_let_between_moves_is_a_known_gap() {
    let src = "fn f(x: [1] i32) { let y = x; let consume z = y; let _ = z; }\n";
    let errs = diagnostics(src);
    assert!(
        !errs.is_empty(),
        "KNOWN GAP: expected a quantity mismatch today. If this now typechecks, \
         the gap is fixed -- the linearity corpus can use the two-step move \
         again. Got: {errs:?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("quantity")),
        "expected a quantity diagnostic, got: {errs:?}"
    );
}

/// GAP: a measured qubit is not required to be reset.
///
/// Measurement collapses a qubit to a classical value, which arguably still
/// needs a reset to return the qubit to |0>. The checker has no notion of
/// uncomputation as such, only of linearity, so it does not object.
#[test]
fn measured_qubit_is_not_required_to_be_reset() {
    let src = "fn f() { let [1] q: Qubit = qalloc(1); let m = measure(q); let _ = m; }\n";
    let errs = diagnostics(src);
    assert!(
        errs.is_empty(),
        "KNOWN GAP: expected this to be accepted today. If it is now rejected, \
         a measured-but-unreset case belongs in uncomputation/invalid.naso. \
         Got: {errs:?}"
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
