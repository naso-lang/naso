//! Loop Extraction Utilities
//!
//! Extract loop nest structure from statements and convert to schedule tree
//! bands.
//!
//! # What this recognises
//!
//! A loop in this AST is `StmtKind::Expr(ExprKind::Forall(..))` -- the same
//! variant used in expression position for a quantified proposition, which the
//! parser disambiguates by position (see ExprKind::Quantified). Extraction only
//! accepts the statement-position loop form, so a proposition inside a `proof`
//! block is never mistaken for a loop.
//!
//! # Multi-binding `forall`
//!
//! `forall i in 0..4, j in 0..8 { .. }` is ONE statement with TWO iterators,
//! but `LoopNest` has a single `iterator` per level, and a polyhedral band is
//! per-iterator. Such a loop is therefore expanded into a nest of
//! single-iterator levels, outermost binding first. The alternative -- widening
//! `LoopNest` to hold a binding list -- would push the multi-dimensional case
//! into every downstream consumer, so the expansion happens once, here.
//!
//! `bindings` is still reported alongside the nest so a caller can tell how many
//! iterators the source loop actually had.
//!
//! # The `step` field
//!
//! The grammar has no step syntax: `forall i in lo..hi` always means unit step.
//! `step` is therefore always the literal `1`, synthesised here. It exists
//! because `LoopNest` is consumed as an affine band, and a band needs a stride.

use crate::ast::{Block, Expr, ExprKind, Ident, Literal, Stmt, StmtKind};

/// A loop nest: one level per iterator, outermost first.
#[derive(Debug, Clone)]
pub struct LoopNest {
    pub iterator: String,
    pub lower_bound: Expr,
    pub upper_bound: Expr,
    pub step: Expr,
    pub body: Box<Stmt>,
    /// The next loop inward, when this level's body is exactly one nested loop.
    pub inner: Option<Box<LoopNest>>,
    /// How many iterators the source `forall` bound in one statement.
    ///
    /// 1 for an ordinary loop. More than 1 means the source used the
    /// comma-separated form and this nest was expanded from it.
    pub bindings: usize,
}

impl LoopNest {
    /// The number of levels in this nest.
    pub fn depth(&self) -> usize {
        1 + self.inner.as_ref().map_or(0, |i| i.depth())
    }

    /// The iterators, outermost first.
    pub fn iterators(&self) -> Vec<&str> {
        let mut out = vec![self.iterator.as_str()];
        if let Some(inner) = &self.inner {
            out.extend(inner.iterators());
        }
        out
    }
}

/// Extract a loop nest from a statement.
///
/// Returns `None` when the statement is not a `forall` loop. A statement whose
/// body contains a loop but is not itself one also returns `None`: the caller
/// is asking "what loop is this statement", and answering with a nested loop
/// would misattribute the body.
pub fn extract_loop_nest(stmt: &Stmt) -> Option<LoopNest> {
    let loop_ = forall_loop(stmt)?;
    // Safe: forall_loop returned the Forall variant, whose bindings vec may in
    // principle be empty, so handle that rather than indexing blindly.
    if loop_.bindings.is_empty() {
        return None;
    }
    let source_body = &loop_.body;
    Some(build_nest(&loop_.bindings, 0, source_body))
}

/// The statement-position `forall` loop, if this statement is one.
///
/// Deliberately matches only `ExprKind::Forall`, never `ExprKind::Quantified`:
/// the parser reuses the same syntax for both, and a quantified proposition in
/// a `proof` block is not a loop.
fn forall_loop(stmt: &Stmt) -> Option<&crate::ast::ForallLoop> {
    let StmtKind::Expr(expr) = &stmt.kind else {
        return None;
    };
    match &expr.kind {
        ExprKind::Forall(loop_) => Some(loop_),
        _ => None,
    }
}

/// Build a nest from bindings, expanding multi-binding `forall` into levels.
///
/// `source_body` is the real body of the innermost binding. Outer levels of an
/// expanded multi-binding loop carry the remaining bindings as their body, so
/// that walking the nest yields every iterator exactly once in source order.
fn build_nest(bindings: &[(Ident, Expr, Expr)], level: usize, source_body: &Block) -> LoopNest {
    let (var, lower, upper) = &bindings[level];
    let is_innermost = level + 1 == bindings.len();

    let body_block = if is_innermost {
        source_body.clone()
    } else {
        // Not a real loop: an empty block standing for "the remaining
        // bindings". Deliberately not a fabricated forall so it cannot be
        // mistaken for one.
        Block {
            stmts: Vec::new(),
            expr: None,
            span: var.span,
        }
    };

    let inner = if is_innermost {
        // A perfect nest: the body's single statement is itself a forall.
        match source_body.stmts.as_slice() {
            [only] if source_body.expr.is_none() => forall_loop(only)
                .filter(|l| !l.bindings.is_empty())
                .map(|l| Box::new(build_nest(&l.bindings, 0, &l.body))),
            _ => None,
        }
    } else {
        Some(Box::new(build_nest(bindings, level + 1, source_body)))
    };

    LoopNest {
        iterator: var.name.clone(),
        lower_bound: lower.clone(),
        upper_bound: upper.clone(),
        step: unit_step(var.span),
        body: Box::new(Stmt::new(
            StmtKind::Expr(Expr::new(
                ExprKind::Block(Box::new(body_block)),
                var.span,
                crate::parser::next_id(),
            )),
            var.span,
            crate::parser::next_id(),
        )),
        inner,
        bindings: bindings.len(),
    }
}

fn unit_step(span: crate::ast::Span) -> Expr {
    // `forall i in lo..hi` has no step syntax, so the stride is always 1.
    Expr::new(
        ExprKind::Literal(Literal::Int(1)),
        span,
        crate::parser::next_id(),
    )
}

/// Convert loop nest to schedule tree bands.
///
/// Still unimplemented: the affine band construction needs a parameter
/// environment that the schedule tree does not yet carry. It is kept honest by
/// returning an empty Vec rather than a fabricated tree -- callers must treat
/// empty as "no bands", which is why nothing depends on it yet.
pub fn loop_nest_to_bands(
    _nest: &LoopNest,
    _ctx: &mut super::LoweringContext,
) -> Result<Vec<crate::ir::ScheduleNode>, super::LoweringError> {
    Ok(Vec::<crate::ir::ScheduleNode>::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    /// Parse a source, return the first statement of the first function's body.
    fn first_stmt(src: &str) -> Stmt {
        let program = parse_program(src).expect("parse");
        let crate::ast::Item::Function(func) = &program.items[0] else {
            panic!("expected a function");
        };
        func.body.stmts[0].clone()
    }

    /// The integer an expression evaluates to, if it is an int literal.
    ///
    /// Used instead of comparing `Expr` directly: `Expr` derives PartialEq over
    /// its span and NodeId as well as its kind, so a bound parsed from source
    /// can never equal a freshly synthesised literal even when the value
    /// matches.
    fn int_value(expr: &Expr) -> Option<i64> {
        match &expr.kind {
            ExprKind::Literal(Literal::Int(n)) => Some(*n),
            _ => None,
        }
    }

    fn assert_int(expr: &Expr, want: i64, what: &str) {
        assert_eq!(int_value(expr), Some(want), "{what}: got {:?}", expr.kind);
    }

    /// A single `forall` yields a one-level nest with the right iterator and
    /// bounds. This is the case the stub returned None for.
    #[test]
    fn test_extracts_single_forall() {
        let stmt = first_stmt("fn f() { forall i in 0..8 { let x = i; } }");
        let nest = extract_loop_nest(&stmt).expect("must extract a loop nest");
        assert_eq!(nest.iterator, "i");
        assert_eq!(nest.depth(), 1);
        assert!(nest.inner.is_none());
        assert_eq!(nest.bindings, 1);
        assert_int(&nest.lower_bound, 0, "lower bound");
        assert_int(&nest.upper_bound, 8, "upper bound");
        // No step syntax, so the stride is the literal 1.
        assert_int(&nest.step, 1, "step");
    }

    /// Nesting is the reason `inner` exists: `forall i { forall j { .. } }` is
    /// two levels, not one level with a body.
    #[test]
    fn test_extracts_nested_forall() {
        let stmt = first_stmt(
            "fn f(t: Tensor[f32, 4]) { forall i in 0..4 { forall j in 0..8 { t[i] = 1.0; } } }",
        );
        let nest = extract_loop_nest(&stmt).expect("must extract");
        assert_eq!(nest.iterator, "i");
        assert_eq!(nest.depth(), 2);
        assert_eq!(nest.iterators(), vec!["i", "j"]);

        let inner = nest.inner.as_ref().expect("inner loop");
        assert_eq!(inner.iterator, "j");
        assert_int(&inner.upper_bound, 8, "inner upper bound");
        assert_eq!(inner.depth(), 1);
    }

    /// Three levels, so `depth` and `iterators` compose rather than special-case
    /// two.
    #[test]
    fn test_extracts_three_levels() {
        let stmt = first_stmt(
            "fn f(t: Tensor[f32, 4]) { forall i in 0..4 { forall j in 0..8 { forall k in 0..16 { t[i] = 1.0; } } } }",
        );
        let nest = extract_loop_nest(&stmt).expect("must extract");
        assert_eq!(nest.depth(), 3);
        assert_eq!(nest.iterators(), vec!["i", "j", "k"]);
    }

    /// A multi-binding `forall` is one statement but N iterators. It must
    /// expand to N levels, outermost binding first.
    #[test]
    fn test_multi_binding_forall_expands_to_nest() {
        let stmt =
            first_stmt("fn f(t: Tensor[f32, 4]) { forall i in 0..4, j in 0..8 { t[i] = 1.0; } }");
        let nest = extract_loop_nest(&stmt).expect("must extract");
        assert_eq!(nest.bindings, 2, "source bound two iterators");
        assert_eq!(nest.depth(), 2, "expanded into two levels");
        assert_eq!(nest.iterators(), vec!["i", "j"]);
    }

    /// The innermost level's body must keep the real statements, not the empty
    /// placeholder used for the outer levels of an expanded multi-binding loop.
    #[test]
    fn test_innermost_body_is_preserved() {
        let stmt = first_stmt("fn f(t: Tensor[f32, 4]) { forall i in 0..4 { t[i] = 1.0; } }");
        let nest = extract_loop_nest(&stmt).expect("must extract");
        let StmtKind::Expr(body_expr) = &nest.body.kind else {
            panic!("body must be an expression statement");
        };
        let ExprKind::Block(block) = &body_expr.kind else {
            panic!("body must be a block");
        };
        assert_eq!(
            block.stmts.len(),
            1,
            "the loop's own statement must survive extraction"
        );
    }

    /// A non-loop statement is not a loop nest. Returning Some here would
    /// misattribute a body's contents to a loop that does not exist.
    #[test]
    fn test_non_loop_statement_returns_none() {
        for src in [
            "fn f() -> int { let x = 1; return x; }",
            "fn f() -> int { return 42; }",
            "fn f(a: int) -> bool { return a > 0; }",
        ] {
            let stmt = first_stmt(src);
            assert!(
                extract_loop_nest(&stmt).is_none(),
                "must not extract a loop from: {src}"
            );
        }
    }

    /// A quantified proposition is NOT a loop. The parser distinguishes them by
    /// position, and this is the check that extraction honours that: a
    /// proposition body containing a `let` must never be read as a loop body.
    #[test]
    fn test_quantified_proposition_is_not_a_loop() {
        let stmt = first_stmt(
            "fn f() -> bool { proof { assert(forall i in 0..4 { i < 4 }); } return true; }",
        );
        // The outer statement is the proof block, not a loop.
        assert!(extract_loop_nest(&stmt).is_none());
    }

    /// `while` and `for` are loops in the language but not `forall`. They are
    /// deliberately not extracted yet: polyhedral lowering needs a rectangular
    /// domain, and a `while` loop has none. Claiming otherwise would be wrong.
    #[test]
    fn test_while_and_for_are_not_extracted() {
        let src = "fn f() -> int { let n = 0; while n < 4 { n = n + 1; } return n; }";
        let stmt = first_stmt(src);
        assert!(
            extract_loop_nest(&stmt).is_none(),
            "while loops have no rectangular domain yet: {src}"
        );
    }

    /// Bounds are preserved as expressions, not just literals, so a symbolic
    /// bound survives extraction.
    #[test]
    fn test_preserves_symbolic_bounds() {
        let stmt =
            first_stmt("fn f[N: nat](t: Tensor[f32, N]) { forall i in 0..N { t[i] = 1.0; } }");
        let nest = extract_loop_nest(&stmt).expect("must extract");
        let ExprKind::Var(v) = &nest.upper_bound.kind else {
            panic!(
                "upper bound must stay a variable, got {:?}",
                nest.upper_bound.kind
            );
        };
        assert_eq!(v.name, "N");
    }
}
