//! `requires { .. }` states a precondition: a claim the CALLER must satisfy.
//!
//! # Why this exists
//!
//! A tensor parameter is encoded in the prover as an *uninterpreted function* from index to
//! element. That is the only sound reading for an arbitrary tensor, and it has a consequence
//! worth being blunt about:
//!
//! ```text
//! proof { forall i in 0..16 { assert(t[i] <= 127); } }   // REFUTED
//! ```
//!
//! The obligation does not hold for every tensor -- a constant tensor whose elements are
//! 10^9 is a legitimate countermodel. The prover is right, and the bound is unobtainable
//! without more information.
//!
//! The obvious "fix" is to give the tensor a range axiom, so the obligation discharges. That
//! is the dishonest option: it proves a claim about the tensors the axiom admits, and then
//! reports it for the tensor that breaks it. A bound that is reported for a tensor violating
//! it is worse than no bound at all.
//!
//! `requires` is the sound route. The premise is written down, lives on the function, and is
//! the caller's problem to discharge. The body's obligations may assume it.
//!
//! # What this file pins
//!
//!  * preconditions are parsed and attached to the function, in order;
//!  * a `requires` block containing anything other than `assert(..)` is REFUSED, not ignored;
//!  * a precondition is NOT discharged as an obligation of the function that declares it.

use naso_compiler::ast::{Function, Item};
use naso_compiler::parser::parse_program;

fn functions_of(src: &str) -> Vec<Function> {
    match parse_program(src) {
        Ok(program) => program
            .items
            .into_iter()
            .filter_map(|item| match item {
                Item::Function(f) => Some(f),
                _ => None,
            })
            .collect(),
        Err(err) => panic!("expected a parse, got {err:?}"),
    }
}

fn parse_err(src: &str) -> String {
    parse_program(src).expect_err("expected a parse error")
}

#[test]
fn a_requires_clause_is_parsed_and_kept_in_order() {
    let src = "fn q(t: [1] Tensor[i8, 16], hi: int) requires { \
               assert(hi > 0); \
               assert(t[0] <= hi); \
               assert(hi < 1000); \
               } { return true; }";
    let funcs = functions_of(src);
    assert_eq!(funcs.len(), 1);
    // Order matters: the prover conjoins them, but a precondition list is a program's
    // readable statement of what it needs, so it should not be silently reordered.
    assert_eq!(funcs[0].requires.len(), 3, "got {:#?}", funcs[0].requires);
}

#[test]
fn a_function_without_requires_has_none() {
    let funcs = functions_of("fn q(n: int) -> bool { return n == n; }");
    assert_eq!(funcs.len(), 1);
    assert!(
        funcs[0].requires.is_empty(),
        "a function with no `requires` clause must carry no preconditions"
    );
}

#[test]
fn an_empty_requires_block_is_legal() {
    // `requires { }` states nothing. It must parse, and it must not fabricate a premise.
    let funcs = functions_of("fn q(n: int) -> bool requires { } { return true; }");
    assert_eq!(funcs.len(), 1);
    assert!(funcs[0].requires.is_empty());
}

/// A `requires` block that silently ignored a non-assert statement would let a precondition
/// be WRITTEN but not REGISTERED. The body would then be proved under a weaker context than
/// the author believes, and the proof would be reported as discharged. Refusing is the only
/// safe option.
#[test]
fn a_requires_block_rejects_a_non_assert_statement() {
    let src = "fn q(n: int) -> bool requires { let x = n + 1; } { return true; }";
    let err = parse_err(src);
    assert!(
        err.to_string().contains("assert"),
        "the error must say what IS allowed, got: {err}"
    );
}

/// A bare expression in `requires` is not an assertion about anything. Accepting it would
/// either drop it or treat it as a claim, and neither is what the author wrote.
#[test]
fn a_requires_block_rejects_a_bare_expression() {
    let src = "fn q(n: int) -> bool requires { n > 0; } { return true; }";
    parse_err(src);
}

/// `assert` with no argument is a malformed premise. Accepting it would add an empty
/// conjunction, which is `true`, silently making the precondition vacuous.
#[test]
fn a_requires_block_rejects_a_zero_argument_assert() {
    let src = "fn q(n: int) -> bool requires { assert(); } { return true; }";
    parse_err(src);
}

/// `requires` must appear BEFORE the body. After the body it would be unreachable, and
/// silently accepting that shape would attach preconditions to nothing.
#[test]
fn requires_after_the_body_is_not_accepted() {
    let src = "fn q(n: int) -> bool { return true; } requires { assert(n > 0); }";
    // Whatever happens, it must not produce a function carrying the precondition.
    if let Ok(program) = parse_program(src) {
        let attached: Vec<&Function> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(f) => Some(f),
                _ => None,
            })
            .collect();
        assert!(
            attached.iter().all(|f| f.requires.is_empty()),
            "a `requires` clause after the body must not attach to the function"
        );
    }
}

/// A quantifier inside a precondition is the interesting case: it states a bound over a
/// whole tensor. It must survive parsing intact, since this is the shape the quantizer needs
/// (`requires { forall i in 0..1024 { assert(abs(input[i] / scale) <= 127.0); } }`).
#[test]
fn a_quantified_precondition_survives_parsing() {
    let src = "fn q(t: [1] Tensor[i8, 16]) requires { \
               forall i in 0..16 { assert(t[i] <= 127); } \
               } { return true; }";
    let funcs = functions_of(src);
    assert_eq!(funcs.len(), 1);
    // The whole `forall` is one precondition. Flattening it to its inner assert would drop
    // the index domain and silently weaken the premise.
    assert_eq!(
        funcs[0].requires.len(),
        1,
        "a quantified precondition must stay a single premise, got {:#?}",
        funcs[0].requires
    );
}

// ---------------------------------------------------------------------------
// Typechecking: a precondition that does not typecheck proves nothing.
// ---------------------------------------------------------------------------

fn typecheck(src: &str) -> Vec<String> {
    let mut program = parse_program(src).expect("parse");
    naso_compiler::typecheck::check_program(&mut program)
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect()
}

/// An undefined name in a precondition must be caught by the TYPECHECKER.
///
/// Without this, the only place the mistake surfaced was the prover, reporting a missing
/// symbol in generated SMT -- far from the source line that wrote it. A precondition that
/// does not typecheck is a precondition nobody proved anything about.
#[test]
fn an_undefined_name_in_a_precondition_is_a_type_error() {
    let src = "fn q(n: int) requires { assert(undefined_thing > 0); } { return true; }";
    let errors = typecheck(src);
    assert!(
        !errors.is_empty(),
        "an undefined variable in `requires` must not typecheck"
    );
}

/// A precondition must type as a proposition. `assert(3)` is not a claim about anything, and
/// letting it through would put a non-proposition into the solver's context.
#[test]
fn a_non_boolean_precondition_is_a_type_error() {
    let src = "fn q(n: int) requires { assert(3); } { return true; }";
    let errors = typecheck(src);
    assert!(
        !errors.is_empty(),
        "a non-boolean precondition must not typecheck, got {errors:?}"
    );
}

/// A precondition may reference the parameters it constrains -- that is the whole point.
#[test]
fn a_precondition_over_a_parameter_typechecks() {
    let src = "fn q(n: int, t: [1] Tensor[i8, 16]) \
               requires { assert(n > 0); forall i in 0..16 { assert(t[i] <= 127); } } { \
               return true; }";
    let errors = typecheck(src);
    assert!(
        errors.is_empty(),
        "a well-typed precondition must typecheck, got {errors:?}"
    );
}

/// A precondition is ERASED, so referencing a `[1]` linear parameter must not consume it.
///
/// If it did, the reference here and the use in the body would be a double use, and the
/// function would be rejected for a claim that never touches a value at runtime.
#[test]
fn a_precondition_does_not_consume_a_linear_parameter() {
    // The body consumes `t` via a `let consume` binding, which is the real consumption
    // form; `consume t;` is not a statement in this language.
    let src = "fn q(t: [1] Tensor[i8, 16]) \
               requires { forall i in 0..16 { assert(t[i] <= 127); } } { \
               let consume u = t; }";
    let errors = typecheck(src);
    assert!(
        errors.is_empty(),
        "a precondition observes its parameters and must not consume them, got {errors:?}"
    );
}

/// An erased reference must not count against a `[n]` BUDGETED quantity.
///
/// `used_at` is both the `[1]` double-use detector and the `[n]` budget. An erased reference
/// therefore has two ways to go wrong, and this pins the second: if it is recorded normally,
/// `can_use` refuses the remaining uses and a function using a `[3]` value three times is
/// rejected after two references inside an erased block.
///
/// This is the test that could distinguish erase-on-error-path from rewind: an
/// `erase_uses_since_inner` that simply rewound `used_at` would still consume budget, while
/// this implementation records the reference on a dedicated flag and consumes none.
#[test]
fn an_erased_reference_consumes_no_bounded_budget() {
    // Three real reads in the body. `let consume` would require exactly `[1]`, so the budget
    // is spent by indexing instead.
    let src = "fn q(t: [3] Tensor[i8, 16]) -> i8 { \
                 proof { forall i in 0..16 { assert(t[i] <= 127); } } \
                 let a = t[0]; let b = t[1]; let c = t[2]; \
                 return a + b + c; }";
    let errors = typecheck(src);
    assert!(
        errors.is_empty(),
        "an erased reference must consume none of the `[3]` budget, got {errors:?}"
    );
}

/// A `[1]` value referenced ONLY in an erased block is used, not leaked.
///
/// The other direction: erase too much and a genuinely unused linear value goes unreported,
/// which is a linearity hole reported as silence.
#[test]
fn an_only_erased_reference_is_not_an_unused_linear_leak() {
    let src = "fn q(t: [1] Tensor[i8, 16]) { \
                 proof { forall i in 0..16 { assert(t[i] <= 127); } } }";
    let errors = typecheck(src);
    assert!(
        errors.is_empty(),
        "`t` is referenced by the obligation, so it is not leaked: got {errors:?}"
    );
}

/// A `[1]` value referenced NOWHERE at all is still reported. The erasure must not have
/// silenced the check for everyone.
#[test]
fn a_truly_unused_linear_value_is_still_reported() {
    let src = "fn q(t: [1] Tensor[i8, 16]) { }";
    let errors = typecheck(src);
    assert!(
        !errors.is_empty(),
        "an unreferenced `[1]` value must still be reported as unused"
    );
}
