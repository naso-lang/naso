#![cfg(feature = "llvm")]

//! Arithmetic and ordering are not defined on `bool`; equality and logic are.
//!
//! # What was wrong
//!
//! `unify_kinds` accepts `(Bool, Bool)`, which is correct for `a == b` and wrong for
//! `a + b`. The arithmetic arm of `infer_binary` called unification without first asking
//! whether the operator meant anything, so every operator on two booleans was accepted:
//!
//! ```text
//! + - * / % == != < > && || & |   -> all ACCEPTED
//! ```
//!
//! and reached LLVM as:
//!
//! ```llvm
//! %add = add i1 %a3, %b4      // `true + true`
//! %lt  = icmp slt i1 %a3, %b4 // `a < b`
//! ```
//!
//! `add i1` has no defined meaning and wraps to 0. `icmp slt i1` is a signed comparison of
//! 0 and 1, which is meaningless -- booleans have no order. Both compiled, both ran, both
//! computed something the programmer did not write.
//!
//! # The rule
//!
//! Legal on `bool`: `==` `!=` `&&` `||` `&` `|` `^`.
//! Refused on `bool`: `+` `-` `*` `/` `%` `<` `<=` `>` `>=`.
//!
//! The bitwise ones are legal and are NOT a concession: on `i1` they *are* the logical
//! connectives, just without short-circuiting. `&&` short-circuits, `&` does not, and
//! confusing them on a function call would change whether the call happens.
//!
//! This suite checks the rule at the CLI, which is where a user meets it.

/// Compile a program through the shipped binary, returning the diagnostic text.
///
/// The temp directory is UNIQUE PER CALL, not per process. These tests run in parallel
/// inside one test binary, so a shared directory means two tests writing `t.naso` at the
/// same time -- and one of them then compiles the OTHER test's source. That produces
/// failures that look like compiler bugs and are not: `arithmetic_on_two_booleans_is_refused`
/// reported `/` as ACCEPTED because the file it read was an equality program from a
/// concurrent test. Unique per call costs one directory and removes the whole class.
fn diagnose(src: &str) -> (bool, String) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("boolops-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("t.naso");
    std::fs::write(&path, src).expect("write");
    let out = dir.join("t.ll");

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_naso"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("spawn naso");
    let _ = std::fs::remove_dir_all(&dir);

    (
        status.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&status.stderr),
            String::from_utf8_lossy(&status.stdout)
        ),
    )
}

/// Arithmetic on two booleans must be refused, and the message must explain why.
///
/// Not asserted as "not accepted": a program can fail to compile for many reasons, and only
/// this diagnostic makes the failure actionable. It has to say the operator is arithmetic
/// and bool is not a number, or a user has no idea what to type instead.
#[test]
fn arithmetic_on_two_booleans_is_refused_with_a_reason() {
    for op in ["+", "-", "*", "/", "%"] {
        let src = format!("fn f(a: bool, b: bool) -> bool {{ a {op} b }}\n");
        let (ok, msg) = diagnose(&src);
        assert!(!ok, "`{op}` on two bools was ACCEPTED");
        assert!(
            msg.contains("not defined on `bool`"),
            "the `{op}` diagnostic must say the operator is undefined on bool: {msg}"
        );
        assert!(
            msg.contains("not a number"),
            "the `{op}` diagnostic must say bool is not a number, so the user learns WHY \
             rather than just that something mismatched: {msg}"
        );
    }
}

/// Ordering comparisons on booleans must be refused: booleans have no order.
///
/// `a < b` reached LLVM as `icmp slt i1`, a signed comparison of 0 and 1. `false < true`
/// therefore answered "1 < 0" = false, while a user asking "is false smaller than true"
/// expects true. A confidently wrong answer, not an error.
#[test]
fn ordering_two_booleans_is_refused() {
    for op in ["<", "<=", ">", ">="] {
        let src = format!("fn f(a: bool, b: bool) -> bool {{ a {op} b }}\n");
        let (ok, msg) = diagnose(&src);
        assert!(!ok, "`{op}` on two bools was ACCEPTED");
        assert!(
            msg.contains("not defined on `bool`"),
            "the `{op}` diagnostic must say the operator is undefined on bool: {msg}"
        );
    }
}

/// Equality on booleans is DEFINED and must keep working.
///
/// The refusal must not overreach. `a == b` on two bools is the most ordinary thing a user
/// writes, and a rule that rejected it would be worse than the bug it fixed.
#[test]
fn equality_on_two_booleans_is_still_accepted() {
    for op in ["==", "!="] {
        let src = format!("fn f(a: bool, b: bool) -> bool {{ a {op} b }}\n");
        let (ok, msg) = diagnose(&src);
        assert!(
            ok,
            "`{op}` on two bools must be accepted, but it was refused: {msg}"
        );
    }
}

/// Short-circuiting logic must keep working.
#[test]
fn logical_operators_on_two_booleans_are_still_accepted() {
    for op in ["&&", "||"] {
        let src = format!("fn f(a: bool, b: bool) -> bool {{ a {op} b }}\n");
        let (ok, msg) = diagnose(&src);
        assert!(
            ok,
            "`{op}` on two bools must be accepted, but it was refused: {msg}"
        );
    }
}

/// The bitwise operators are legal on `bool`, and that is not an oversight.
///
/// On `i1`, `&` IS the logical connective. It differs from `&&` only in short-circuiting,
/// which is a real difference when an operand is a function call -- so refusing it would
/// remove a legitimate distinction rather than prevent an error.
#[test]
fn bitwise_operators_on_two_booleans_are_accepted_because_they_are_logical() {
    for op in ["&", "|"] {
        let src = format!("fn f(a: bool, b: bool) -> bool {{ a {op} b }}\n");
        let (ok, msg) = diagnose(&src);
        assert!(
            ok,
            "`{op}` on two bools must be accepted, but it was refused: {msg}"
        );
    }
}

/// A bool mixed with a number must still be refused, on both sides.
///
/// `numeric_common` only promotes integer/float pairs, so these fall through to unification
/// and are caught there -- but the failure mode was a confusing "type mismatch" rather than
/// the operator diagnostic, so it is pinned separately.
#[test]
fn a_bool_mixed_with_a_number_is_refused() {
    for src in [
        "fn f(a: i64, b: bool) -> i64 { a + b }",
        "fn f(a: bool, b: i64) -> i64 { a + b }",
        "fn f(a: f32, b: bool) -> f32 { a + b }",
    ] {
        let (ok, msg) = &diagnose(&format!("{src}\n"));
        assert!(
            !ok,
            "`{src}` must be refused: a bool is not a number and the promotion rule must \
             not cover it."
        );
        assert!(
            !msg.is_empty(),
            "`{src}` must produce a diagnostic, not fail silently"
        );
    }
}

/// Numeric arithmetic must be entirely unaffected by the bool rule.
///
/// The rule is a guard on a bool operand, so a mutation that made it fire unconditionally
/// would refuse all arithmetic. Pinned here with real programs.
#[test]
fn numeric_arithmetic_is_unaffected() {
    for src in [
        "fn f(a: i64, b: i64) -> i64 { a + b }",
        "fn f(a: f32, b: f32) -> f32 { a * b }",
        "fn f(a: i64, b: f32) -> f64 { a * b }",
        "fn f(a: i8, b: i64) -> i64 { a + b }",
        "fn f(a: i64, b: i64) -> bool { a < b }",
        "fn f(a: f32, b: f32) -> bool { a >= b }",
    ] {
        let (ok, msg) = diagnose(&format!("{src}\n"));
        assert!(ok, "`{src}` must still compile, but it was refused: {msg}");
    }
}

/// The rule itself, asserted at the library level so it is stated in exactly one place.
///
/// The CLI tests above pin the behaviour; this pins the DECLARATION, so the
/// "meaningful on bool" list cannot change without a test noticing.
#[test]
fn the_bool_operator_rule_is_explicit() {
    use naso_compiler::ast::expr::BinOp;

    // Defined on bool.
    for (op, name) in [
        (BinOp::Eq, "=="),
        (BinOp::Ne, "!="),
        (BinOp::And, "&&"),
        (BinOp::Or, "||"),
        (BinOp::BitAnd, "&"),
        (BinOp::BitOr, "|"),
        (BinOp::BitXor, "^"),
    ] {
        assert!(
            naso_compiler::typecheck::inference::bool_operator(op),
            "`{name}` IS defined on bool and must not be refused"
        );
    }

    // Not defined on bool.
    for (op, name) in [
        (BinOp::Add, "+"),
        (BinOp::Sub, "-"),
        (BinOp::Mul, "*"),
        (BinOp::Div, "/"),
        (BinOp::Rem, "%"),
        (BinOp::Lt, "<"),
        (BinOp::Le, "<="),
        (BinOp::Gt, ">"),
        (BinOp::Ge, ">="),
        (BinOp::Shl, "<<"),
        (BinOp::Shr, ">>"),
    ] {
        assert!(
            !naso_compiler::typecheck::inference::bool_operator(op),
            "`{name}` is NOT defined on bool, so bool_operator must say so. If a new \
             operator has a meaning on bool, add it here deliberately rather than by \
             accident."
        );
    }
}

/// The codegen backstop must exist, because the typechecker is not the only door.
///
/// The tests above all go through the CLI, so they exercise the TYPECHECKER. That layer now
/// refuses `true + true` before codegen runs, which means a mutation that removed the
/// LLVM-level guard entirely would survive all of them: the guard is unreachable from the
/// source-language path by design.
///
/// It is not unreachable, though. `promote_binary_operands` is the single call site for
/// every binary operation in the backend, and PIR can be handed to the public LLVM entry
/// directly, exactly as the QIR and WGSL entry points already can. A mutation that put the
/// same-width arm back in front of the one-bit arm survived the CLI tests for exactly this
/// reason -- `i1 + i1` is same-width, so it matched there and reached `add i1`.
//
// So this asserts the ARM ORDER directly in the source, which is the only place the
// ordering is expressed.
#[test]
fn the_codegen_backstop_checks_the_bool_shape_before_the_same_width_case() {
    let source = include_str!("../src/codegen/llvm/expr_lowering.rs");

    let match_start = source
        .find("match (left.get_type(), right.get_type()) {")
        .expect("promote_binary_operands must match on operand types");
    // End the region at the mixed-width arm, which is the next one after these two. A
    // fixed byte window was wrong here: both guards are short, so swapping the arms moved
    // neither token far, and a window-based check could not tell the order at all. This is
    // why the region is bounded by structure rather than length.
    let region_end = source
        .get(match_start..)
        .and_then(|r| r.find("(BasicTypeEnum::IntType(_), BasicTypeEnum::IntType(_)) => {"))
        .map(|o| match_start + o)
        .expect("the mixed-width arm must follow the one-bit and same-width arms");
    let region = &source[match_start..region_end];

    // Matched on the ARM COMMENTS, not on expressions inside the guards.
    //
    // An earlier version searched for `get_bit_width() == 1` and `r.get_bit_width() ==` and
    // both live inside the SAME arm's condition (`l == 1 || r == 1`), 24 bytes apart, so
    // swapping the arms did not move either token and the ordering could not be observed at
    // all. The mutant survived for that reason, twice.
    let one_bit = region
        .find("// A one-bit integer is a lowered `bool`")
        .expect("the one-bit (bool) arm must exist in promote_binary_operands");
    let same_width = region
        .find("// Same-width integers already agree")
        .expect("the same-width fast path must exist");

    assert!(
        one_bit < same_width,
        "the one-bit `bool` arm must come BEFORE the same-width arm. `i1 + i1` is \
         same-width, so the other order lets arithmetic on two booleans through to \
         `add i1`, which wraps to 0. Offsets were one_bit={one_bit}, same_width={same_width}."
    );

    // The guard must also consult the operator, or it refuses `==` and `&` on booleans.
    assert!(
        region.contains("op_is_arithmetic"),
        "the one-bit guard must be conditioned on the operator being arithmetic. This \\
         function handles EVERY binary operation, so a width-only guard also refuses the \\
         correct and necessary `icmp eq i1` and `and i1`."
    );
}

/// The operator guard and the typechecker's rule must agree.
///
/// Two lists describing "is this operator meaningful on a bool" exist in different layers,
/// keyed on different representations -- LLVM `i1` here, source `bool` in the typechecker.
/// They have to agree or a program is accepted by one layer and refused by the other, so
/// the sets are compared directly rather than trusted to stay in step.
#[test]
fn the_codegen_and_typechecker_operator_lists_agree() {
    use naso_compiler::ast::expr::BinOp;

    // Arithmetic per the typechecker: refused on bool.
    let tyc_arithmetic = [BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Rem];

    // The same set, spelled as the PIR `BinaryOp` variants the backend sees. If someone
    // adds an arithmetic operator to one enum and not the other, this list is where the
    // disagreement shows up.
    let cg_arithmetic = [
        "BinaryOp::Add",
        "BinaryOp::Sub",
        "BinaryOp::Mul",
        "BinaryOp::Div",
        "BinaryOp::Mod",
    ];

    let source = include_str!("../src/codegen/llvm/expr_lowering.rs");
    for variant in cg_arithmetic {
        assert!(
            source.contains(variant),
            "the codegen arithmetic list must include {variant}. Naso's BinOp::Rem lowers \
             to BinaryOp::Mod, and BinaryOp::Div is shared, so all five must be present or \
             the two layers will disagree about whether arithmetic on bool is refused."
        );
    }

    // And every one of them must actually be refused on bool by the typechecker.
    for op in tyc_arithmetic {
        assert!(
            !naso_compiler::typecheck::inference::bool_operator(op),
            "{op:?} is arithmetic and must not be defined on bool"
        );
    }
}
