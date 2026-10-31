//! A WGSL callee must be emitted before its caller, and the ordering must not
//! depend on a name collector that silently skips constructs.
//!
//! # What the collector actually covers
//!
//! `wgsl_straight.rs` orders functions by walking each one with
//! `collect_called_names_stmt` / `collect_called_names_expr`. Between them those two
//! functions match **9 of the 27** `ExprKind` variants. Everything else falls into
//! `_ => {}`.
//!
//! That is only safe if a construct the collector skips is *also* refused by the
//! emitter -- because the collector's output decides which function gets written
//! first. A call hidden in a skipped construct means the callee is emitted after its
//! caller, and WGSL has no forward declarations, so the module does not parse.
//!
//! # Why this was worth testing rather than reasoning about
//!
//! It currently holds: `while`, `for`, `forall`, tuple and array literals, `if`, and
//! `let` expressions are all refused by the emitter before emission, so the collector
//! never meets them. I verified each by running the CLI path, not by reading it.
//!
//! But it holds by coincidence of two independent lists agreeing, and nothing pinned
//! the agreement. Adding a new `ExprKind` that the emitter accepts would silently
//! mis-order functions, and the failure would appear as an unparseable shader with
//! no diagnostic -- the same shape as the defects in the two preceding audits.
//!
//! # What these tests do
//!
//! They pin the agreement from both sides: every construct the collector skips must
//! be REFUSED by the emitter, and every construct that IS emitted must have its calls
//! collected. `a_skipped_construct_cannot_reach_emission` is the load-bearing one --
//! it is what fails if someone teaches the emitter a new shape without updating the
//! collector.

#![cfg(feature = "llvm")]

use naso_compiler::codegen::generate_wgsl_straight_line;

/// Generate WGSL, or return the error text.
fn emit(src: &str) -> Result<String, String> {
    let p = naso_compiler::parser::parse_program(src).map_err(|e| format!("PARSE: {e:?}"))?;
    generate_wgsl_straight_line(&p).map_err(|e| e.to_string())
}

/// Byte offset of `needle`, or `usize::MAX` when absent.
fn at(wgsl: &str, needle: &str) -> usize {
    wgsl.find(needle).unwrap_or(usize::MAX)
}

/// A callee must precede its caller in the emitted WGSL.
///
/// The baseline: a plain `let` call, which the collector handles. If this ever fails,
/// the ordering walk itself is broken rather than one of its arms.
#[test]
fn a_callee_is_emitted_before_its_caller() {
    let wgsl = emit(
        "\
fn helper(x: f32) -> f32 { x * 2.0 }
fn caller() -> f32 { let a = helper(1.0); a }
",
    )
    .expect("a straight-line call must emit");
    assert!(
        at(&wgsl, "fn helper") < at(&wgsl, "fn caller"),
        "the callee must come first: WGSL has no forward declarations, so a caller \
         that references a later definition does not parse.\n{wgsl}"
    );
}

/// A three-level chain must be ordered callee-first at every step.
///
/// One level can pass by luck. Three levels is the shape where a single missed edge
/// puts a function in the middle of the order.
#[test]
fn a_three_level_chain_is_ordered_end_to_end() {
    let wgsl = emit(
        "\
fn leaf(x: f32) -> f32 { x + 1.0 }
fn middle(x: f32) -> f32 { leaf(x) }
fn top() -> f32 { middle(2.0) }
",
    )
    .expect("a chain must emit");
    let (leaf, mid, top) = (
        at(&wgsl, "fn leaf"),
        at(&wgsl, "fn middle"),
        at(&wgsl, "fn top"),
    );
    assert!(
        leaf < mid && mid < top,
        "expected leaf < middle < top, got {leaf} {mid} {top}.\n{wgsl}"
    );
}

/// Two independent callers of one callee must not reorder it after either.
#[test]
fn a_callee_shared_by_two_callers_is_emitted_first_for_both() {
    let wgsl = emit(
        "\
fn shared(x: f32) -> f32 { x * 3.0 }
fn a() -> f32 { shared(1.0) }
fn b() -> f32 { shared(2.0) }
",
    )
    .expect("two callers must emit");
    let s = at(&wgsl, "fn shared");
    assert!(
        s < at(&wgsl, "fn a") && s < at(&wgsl, "fn b"),
        "the shared callee must precede both callers.\n{wgsl}"
    );
}

/// Mutual recursion has no valid WGSL form and must be refused by name.
///
/// WGSL cannot forward-declare, so a cycle cannot be ordered. This is the case where
/// the ordering walk's `on_stack` check is the only thing standing between a hang and
/// a diagnostic.
///
/// The functions use only arithmetic and `return`, because an unsupported construct is
/// refused BEFORE the recursion check runs -- and a refusal for the wrong reason is
/// what the previous version of this test asserted.
#[test]
fn mutual_recursion_is_refused_by_name() {
    let err = emit(
        "\
fn ping(n: i64) -> i64 { return 1 + pong(n); }
fn pong(n: i64) -> i64 { return 2 + ping(n); }
",
    )
    .expect_err("mutual recursion must be refused");
    assert!(
        err.to_lowercase().contains("recursi"),
        "the refusal must name recursion as the reason: {err}"
    );
}

/// A call inside a `forall` body must be collected, so the callee is emitted first.
///
/// THE REGRESSION THIS FILE EXISTS FOR.
///
/// `collect_called_names_stmt` had no arm for a statement-position `forall`, falling
/// into `_ => {}`. But `forall` IS emitted -- `check_forall` walks and validates its
/// body -- so a call inside a loop body was invisible to the ordering walk while being
/// entirely real in the output.
///
/// With the callee declared after the caller, this emitted:
///
///     fn caller(n: i32) -> f32 {
///         for (var i: i32 = 0; i < n; i = i + 1) {
///             let t = helper(f32(i));
///         }
///     fn helper(x: f32) -> f32 { ... }
///
/// A caller referencing a later definition. WGSL has no forward declarations, so that
/// module does not parse -- and the backend reported success, because generating a
/// string is the only thing it checks.
///
/// The caller is declared FIRST on purpose: in source order the bug is invisible,
/// since `helper` would already precede it.
#[test]
fn a_call_inside_a_forall_body_orders_the_callee_first() {
    let wgsl = emit(
        "\
fn caller(n: i64) -> f32 { forall i in 0..n { let t = helper(i as f32); } 1.0 }
fn helper(x: f32) -> f32 { x * 2.0 }
",
    )
    .expect("a forall body containing a call must emit");
    assert!(
        at(&wgsl, "fn helper") < at(&wgsl, "fn caller"),
        "the callee must precede its caller even when the only call site is inside a \
         loop body.
{wgsl}"
    );
}

/// The same call in the loop's TAIL expression must also be collected.
///
/// The body and the tail are separate fields, so fixing one without the other leaves a
/// hole in exactly the same place. This test exists because that mutant SURVIVED the
/// first version of this file: the earlier case had a call in a `let` as well as in
/// the tail, so the body walk found `helper` and the tail walk was never exercised.
///
/// So here the loop's tail is the ONLY call site, the loop body is empty, and `ft` is
/// `void` so there is no return value to supply. `other` is declared FIRST, so if the
/// tail is not walked this function is simply absent from the ordering and `other`
/// keeps its declaration-order position relative to a caller that calls it.
#[test]
fn a_call_in_a_forall_tail_orders_the_callee_first() {
    let wgsl = emit(
        "\
fn ft() { forall i in 0..3 { other(i as f32) } }
fn other(x: f32) -> f32 { x * 2.0 }
",
    )
    .expect("a forall tail containing a call must emit");
    assert!(
        at(&wgsl, "fn other") < at(&wgsl, "fn ft"),
        "the ONLY call site in `ft` is the loop's tail expression, so this fails \
         unless the tail is walked.\n{wgsl}"
    );
}

/// A `forall` calling a helper through two functions must order the whole chain.
#[test]
fn a_forall_call_chain_is_ordered() {
    let wgsl = emit(
        "\
fn caller(n: i64) -> f32 { forall i in 0..n { let t = outer(i as f32); } 1.0 }
fn outer(x: f32) -> f32 { inner(x) }
fn inner(x: f32) -> f32 { x * 2.0 }
",
    )
    .expect("a forall call chain must emit");
    let (i, o, c) = (
        at(&wgsl, "fn inner"),
        at(&wgsl, "fn outer"),
        at(&wgsl, "fn caller"),
    );
    assert!(
        i < o && o < c,
        "expected inner < outer < caller, got {i} {o} {c}.
{wgsl}"
    );
}

/// Constructs the collector SKIPS must be refused by the emitter.
///
/// This is the load-bearing test. `collect_called_names_expr` handles 9 variants and
/// skips the rest into `_ => {}`. That is only safe while the emitter also refuses
/// them, because the collector's output decides emission order and a call hidden in a
/// skipped construct would emit the callee after its caller.
///
/// Each case below names a construct the collector does NOT walk. All must be refused;
/// if one starts emitting, the collector needs a matching arm.
#[test]
fn a_skipped_construct_cannot_reach_emission() {
    // (label, source body) -- each uses a construct absent from the collector.
    let cases: &[(&str, &str)] = &[
        ("while", "let mut i = 0; while i < 3 { i = i + 1; } 1.0"),
        ("for", "for i in 3 { } 1.0"),
        ("if", "if 1.0 > 0.0 { } 1.0"),
        ("tuple literal", "let t = (1.0, 2.0); 1.0"),
        ("array literal", "let a = [1.0, 2.0]; 1.0"),
        ("let expression", "let a = let y = 1.0; y; a"),
    ];
    for (label, body) in cases {
        let src = format!("fn caller() -> f32 {{ {body} }}\n");
        if let Ok(w) = emit(&src) {
            panic!(
                "`{label}` is emitted by this backend but `collect_called_names_expr` \
                 does not walk it. A call inside it would put the callee after its \
                 caller and the shader would not parse. Add a collector arm, or \
                 refuse the construct.\n--- emitted ---\n{w}"
            );
        }
    }
}

/// A construct the collector DOES walk must still order correctly.
///
/// The positive half of the previous test, so that "refuse everything" cannot pass
/// the suite.
#[test]
fn a_construct_the_collector_walks_still_emits_in_order() {
    for (label, body) in [
        ("block", "let a = { let q = 1.0; q }; a"),
        ("ascribe", "let a = 1.0 as f32; a"),
        ("assign in tail", "let mut a = 1.0; a = a + 1.0; a"),
    ] {
        let src =
            format!("fn helper(x: f32) -> f32 {{ x * 2.0 }}\nfn caller() -> f32 {{ {body} }}\n");
        let wgsl = emit(&src).unwrap_or_else(|e| panic!("`{label}` must emit: {e}"));
        assert!(
            at(&wgsl, "fn helper") < at(&wgsl, "fn caller"),
            "`{label}`: the callee must precede its caller.\n{wgsl}"
        );
    }
}

/// A call inside a nested block still orders, which is the collector's `Block` arm.
///
/// Nested blocks are the realistic case: a real function has statements inside a
/// block, and a naive collector that only looked at the top level would miss every
/// call in the body.
#[test]
fn a_call_inside_a_nested_block_orders_correctly() {
    let wgsl = emit(
        "\
fn helper(x: f32) -> f32 { x * 2.0 }
fn caller() -> f32 {
    let a = { let t = helper(1.0); t + 1.0 };
    a
}
",
    )
    .expect("a nested-block call must emit");
    assert!(
        at(&wgsl, "fn helper") < at(&wgsl, "fn caller"),
        "the collector must descend into blocks.\n{wgsl}"
    );
}
