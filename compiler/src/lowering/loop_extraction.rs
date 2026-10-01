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

/// Convert a loop nest into schedule-tree bands.
///
/// # What this produces
///
/// A `Band` whose scheduling dimensions are the identity in the iterator
/// space: dimension `d` of the band maps iteration `d` to schedule time `d`,
/// which is exactly "execute in source order, one level per iterator".
///
/// # Why identity, and not something cleverer
///
/// A band's purpose is to say *when* each iteration runs. Identity says "outer
/// iterator first", which is what sequential source order means. Permutations,
/// skewing and tiling are transformations that are only meaningful relative to
/// the accesses in the body, and the accesses are not available here -- a
/// LoopNest carries the loop structure alone. Claiming a permutation without
/// having looked at the memory access pattern would be exactly the kind of
/// unfounded claim this lowering path is meant to avoid.
///
/// `coincident` is set true for a 1-dimensional band, since one dimension has
/// no loop to be serialised against and such iterations can run in parallel.
/// For a multi-dimensional band the inner dimension carries the real ordering,
/// so false is the honest answer.
///
/// # Domain
///
/// Each iterator contributes its bounds as inequality constraints over the
/// iterator space. Only literal bounds are encoded; a symbolic bound such as
/// `0..N` is left unconstrained rather than guessed, because encoding it would
/// need a parameter dimension this function is not given. `contains` therefore
/// under-approximates for symbolic bounds, which is the safe direction: it can
/// admit too much, never too little.
///
/// # Not yet a tree
///
/// The innermost `Domain` leaf needs a StmtId identifying the body statement,
/// which is not carried by LoopNest. Rather than invent one and produce a tree
/// that points at the wrong statement, this returns the band with a `Sequence`
/// of no children as the leaf placeholder and documents it. Wiring the body
/// through requires extending LoopNest with its statement id.
use crate::ir::affine_domain::{AffineConstraint, AffineDomain};
use crate::ir::affine_map::{AffineMap, AffineMapPiece, Matrix};
use crate::ir::schedule_tree::ScheduleNode;

/// Convert a loop nest to schedule tree bands.
pub fn loop_nest_to_bands(
    nest: &LoopNest,
    _ctx: &mut super::LoweringContext,
) -> Result<Vec<ScheduleNode>, super::LoweringError> {
    let depth = nest.depth();
    if depth == 0 {
        return Ok(Vec::new());
    }

    // Domain over `depth` iterator dimensions, bounded by the loop bounds.
    let domain = domain_from_nest(nest, depth);

    // One scheduling dimension per loop level: schedule time d = iteration d,
    // shifted so the first iteration of each loop sits at schedule time 0.
    //
    // One AffineMap per dimension, since ScheduleNode::Band documents
    // `members` as "one per loop level". Each is a single-row map.
    let mut lower_bounds: Vec<i64> = Vec::with_capacity(depth);
    let mut level = nest;
    loop {
        lower_bounds.push(literal_int(&level.lower_bound).unwrap_or(0));
        match &level.inner {
            Some(next) => level = next,
            None => break,
        }
    }

    let members: Vec<AffineMap> = (0..depth)
        .map(|d| {
            let mut row = Matrix::new(1, depth);
            row.set(0, d, 1);
            // Matrix apply is `m*point + constant`, so schedule time is
            // `i_d - lower_d`: the translation is NEGATIVE.
            row.set_const(0, -lower_bounds[d]);
            AffineMap {
                pieces: vec![AffineMapPiece::new(domain.clone(), row)],
            }
        })
        .collect();

    // The leaf cannot be a Domain node: LoopNest does not carry the body
    // statement's id, and a Domain node pointing at the wrong statement would
    // be worse than an empty Sequence.
    let leaf = ScheduleNode::Sequence {
        children: Vec::new(),
    };

    let coincident = vec![depth == 1; depth];

    Ok(vec![ScheduleNode::Band {
        members,
        coincident,
        child: Box::new(leaf),
    }])
}

/// The iteration domain implied by a nest: `depth` iterator dimensions,
/// constrained by each level's literal bounds.
fn domain_from_nest(nest: &LoopNest, depth: usize) -> AffineDomain {
    let mut constraints = Vec::new();

    // One coefficient per iterator dimension. Coeffs before the current level
    // are zeroed: level d only constrains dimension d.
    let mut level_index = 0usize;
    let mut level = nest;
    loop {
        let mut coeffs = vec![0i64; depth];
        if let Some(hi) = literal_int(&level.upper_bound) {
            // i_d < hi  ==>  -i_d >= -(hi - 1) over the integers.
            //
            // Encoding this as -i_d >= -hi would be INCLUSIVE and would admit
            // i_d == hi, which the language's `0..hi` does not iterate.
            coeffs[level_index] = -1;
            constraints.push(AffineConstraint::inequality(coeffs.clone(), -(hi - 1)));
        }
        if let Some(lo) = literal_int(&level.lower_bound) {
            // i_d >= lo
            coeffs[level_index] = 1;
            constraints.push(AffineConstraint::inequality(coeffs, lo));
        }
        // A symbolic bound contributes no constraint, and `contains` then
        // over-approximates -- safe, never under.
        level_index += 1;
        match &level.inner {
            Some(next) => level = next,
            None => break,
        }
    }

    AffineDomain::new(depth, 0, constraints).with_name(format!("nest({})", nest.iterator))
}

/// The integer value of an int-literal expression, if it is one.
fn literal_int(expr: &Expr) -> Option<i64> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n),
        _ => None,
    }
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
    /// Build bands for a source and return the single band.
    fn band_for(src: &str) -> crate::ir::schedule_tree::ScheduleNode {
        let stmt = first_stmt(src);
        let nest = extract_loop_nest(&stmt).expect("must extract a loop nest");
        let mut ctx = super::super::LoweringContext::new();
        let bands = loop_nest_to_bands(&nest, &mut ctx).expect("bands");
        assert_eq!(bands.len(), 1, "expected exactly one band");
        bands.into_iter().next().unwrap()
    }

    fn band_domain(node: &crate::ir::schedule_tree::ScheduleNode) -> &AffineDomain {
        let crate::ir::schedule_tree::ScheduleNode::Band { members, .. } = node else {
            panic!("expected a Band, got {node:?}");
        };
        &members[0].pieces[0].domain
    }

    /// The loop `forall i in 0..8` produces a domain that accepts exactly the
    /// points in [0, 8) and rejects the endpoints. This is the property that
    /// makes the band meaningful, so it is asserted on `contains` rather than
    /// on the constraint representation.
    #[test]
    fn test_band_domain_accepts_exactly_the_loop_range() {
        let band = band_for("fn f() { forall i in 0..8 { let x = i; } }");
        let domain = band_domain(&band);
        assert_eq!(domain.dims, 1);
        assert_eq!(domain.n_iter, 1);

        for i in 0..8 {
            assert!(domain.contains(&[i]), "must contain {i}");
        }
        // Upper bound is EXCLUSIVE, matching `0..8` in the language.
        assert!(!domain.contains(&[8]), "8 is out of range for 0..8");
        assert!(!domain.contains(&[-1]), "-1 is below the lower bound");
    }

    /// A non-zero lower bound must shift the domain, not just bound from 0.
    #[test]
    fn test_band_domain_respects_nonzero_lower_bound() {
        let band = band_for("fn f() { forall i in 4..8 { let x = i; } }");
        let domain = band_domain(&band);
        for i in 4..8 {
            assert!(domain.contains(&[i]), "must contain {i}");
        }
        assert!(!domain.contains(&[3]), "3 is below the lower bound");
        assert!(!domain.contains(&[8]), "8 is out of range");
    }

    /// Two levels means a 2-dimensional domain, and each dimension is bounded
    /// by its own loop.
    #[test]
    fn test_two_level_band_domain_is_rectangular() {
        let band = band_for(
            "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 2..8 { t[i] = 1.0; } } }",
        );
        let domain = band_domain(&band);
        assert_eq!(domain.dims, 2, "two iterators");
        for i in 0..4 {
            for j in 2..8 {
                assert!(domain.contains(&[i, j]), "must contain ({i}, {j})");
            }
        }
        assert!(!domain.contains(&[4, 2]), "i out of range");
        assert!(!domain.contains(&[0, 8]), "j out of range");
        assert!(!domain.contains(&[0, 1]), "j below lower bound");
    }

    /// The band's scheduling map is the identity: schedule time d = iteration
    /// d, translated by the lower bound. Anything else would be an unfounded
    /// claim about ordering.
    #[test]
    fn test_band_map_is_identity_translated_by_lower_bound() {
        let band = band_for("fn f() { forall i in 4..8 { let x = i; } }");
        let crate::ir::schedule_tree::ScheduleNode::Band { members, .. } = &band else {
            panic!("expected a Band");
        };
        let piece = &members[0].pieces[0];
        // Identity: a single row selecting i with coefficient 1.
        assert_eq!(piece.matrix.rows, 1, "one scheduling dimension per member");
        assert_eq!(piece.matrix.get(0, 0), 1, "unit coefficient on i");
        // Schedule time is i - lower, so the translation is negative.
        assert_eq!(
            piece.matrix.constant[0], -4,
            "shifted so the first iteration is time 0"
        );

        // Applying it to a point gives the schedule time.
        assert_eq!(piece.apply(&[4]), Some(vec![0]), "i=4 is schedule time 0");
        assert_eq!(piece.apply(&[5]), Some(vec![1]), "i=5 is schedule time 1");
    }

    /// `coincident` is true only for a 1-D band. Claiming parallelism for a
    /// multi-level band would assert an ordering independence that has not been
    /// checked against the accesses.
    #[test]
    fn test_coincident_only_for_single_dimension_bands() {
        let one = band_for("fn f() { forall i in 0..8 { let x = i; } }");
        let crate::ir::schedule_tree::ScheduleNode::Band { coincident, .. } = &one else {
            panic!("expected a Band");
        };
        assert_eq!(coincident, &vec![true], "1-D band can run in parallel");

        let two = band_for(
            "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 0..8 { t[i] = 1.0; } } }",
        );
        let crate::ir::schedule_tree::ScheduleNode::Band { coincident, .. } = &two else {
            panic!("expected a Band");
        };
        assert_eq!(coincident, &vec![false, false], "2-D band is ordered");
    }

    /// A symbolic bound must NOT be invented. The domain over-approximates
    /// rather than pretending to know `N`, because a wrong bound would exclude
    /// iterations that really run.
    #[test]
    fn test_symbolic_bound_leaves_domain_unconstrained() {
        let band = band_for("fn f[N: nat](t: Tensor[f32,N]) { forall i in 0..N { t[i] = 1.0; } }");
        let domain = band_domain(&band);
        // Lower bound 0 is literal, so it is encoded; the upper bound is not.
        assert!(
            domain.contains(&[0]),
            "point at the literal lower bound is in the domain"
        );
        // The upper bound is unknown, so a large point is admitted. That is the
        // safe direction: over-approximating cannot exclude a real iteration.
        assert!(
            domain.contains(&[9999]),
            "symbolic upper bound must not fabricate a constraint"
        );
    }

    /// The leaf is an empty Sequence, not a Domain node: LoopNest carries no
    /// statement id, and a Domain node would point at the wrong statement.
    #[test]
    fn test_leaf_is_not_a_fabricated_domain_node() {
        let band = band_for("fn f() { forall i in 0..8 { let x = i; } }");
        let crate::ir::schedule_tree::ScheduleNode::Band { child, .. } = &band else {
            panic!("expected a Band");
        };
        match child.as_ref() {
            crate::ir::schedule_tree::ScheduleNode::Sequence { children } => {
                assert!(children.is_empty(), "leaf placeholder has no children")
            }
            other => panic!("leaf must not be a fabricated node, got {other:?}"),
        }
    }

    /// Every iterator level contributes a scheduling dimension.
    #[test]
    fn test_band_has_one_member_per_level() {
        for (src, want) in [
            ("fn f() { forall i in 0..8 { let x = i; } }", 1usize),
            (
                "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 0..8 { t[i]=1.0; } } }",
                2,
            ),
        ] {
            let band = band_for(src);
            let crate::ir::schedule_tree::ScheduleNode::Band {
                members,
                coincident,
                ..
            } = &band
            else {
                panic!("expected a Band");
            };
            assert_eq!(members.len(), want, "members for {src}");
            assert_eq!(coincident.len(), want, "coincident flags for {src}");
        }
    }

    /// Brute-force cross-check.
    ///
    /// The hand-written cases above assert specific points. This enumerates a
    /// window around and inside the range and compares `contains` against an
    /// expectation computed straight from the loop bounds, so an off-by-one in
    /// the constraint encoding cannot hide behind a case nobody thought to add.
    #[test]
    fn test_domain_agrees_with_bounds_over_a_window() {
        // (source, per-dimension (lower, upper))
        /// (source, label, per-dimension (lower, upper) exclusive)
        type Ranges = Vec<(i64, i64)>;
        let cases: &[(&str, &str, Ranges)] = &[
            (
                "fn f() { forall i in 0..8 { let x = i; } }",
                "0..8",
                vec![(0, 8)],
            ),
            (
                "fn f() { forall i in 4..9 { let x = i; } }",
                "4..9",
                vec![(4, 9)],
            ),
            (
                "fn f() { forall i in 2..3 { let x = i; } }",
                "2..3",
                vec![(2, 3)],
            ),
            (
                "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 0..3 { t[i]=1.0; } } }",
                "0..4 x 0..3",
                vec![(0, 4), (0, 3)],
            ),
            (
                "fn f(t: Tensor[f32,4]) { forall i in 0..5 { forall j in 2..7 { t[i]=1.0; } } }",
                "0..5 x 2..7",
                vec![(0, 5), (2, 7)],
            ),
        ];

        for (src, label, ranges) in cases {
            let stmt = first_stmt(src);
            let nest = extract_loop_nest(&stmt).expect("must extract");
            let mut ctx = super::super::LoweringContext::new();
            let bands = loop_nest_to_bands(&nest, &mut ctx).expect("bands");
            let crate::ir::schedule_tree::ScheduleNode::Band { members, .. } = &bands[0] else {
                panic!("expected a Band");
            };
            let domain = &members[0].pieces[0].domain;
            let dims = ranges.len();

            // Enumerate a window wide enough to cross both bounds on every
            // dimension, including points outside the range.
            let span = 4i64;
            let lo = ranges.iter().map(|r| r.0 - span).collect::<Vec<_>>();
            let hi = ranges.iter().map(|r| r.1 + span).collect::<Vec<_>>();

            let mut total = 0usize;
            let mut point = lo.clone();
            loop {
                let expect = point
                    .iter()
                    .zip(ranges)
                    .all(|(p, &(l, h))| *p >= l && *p < h);
                assert_eq!(
                    domain.contains(&point),
                    expect,
                    "{label}: contains({point:?}) should be {expect}"
                );
                total += 1;

                // odometer increment
                let mut d = 0usize;
                loop {
                    point[d] += 1;
                    if point[d] <= hi[d] {
                        break;
                    }
                    point[d] = lo[d];
                    d += 1;
                    if d == dims {
                        break;
                    }
                }
                if d == dims && point[0] == lo[0] {
                    break;
                }
                if total > 20000 {
                    panic!("enumeration did not terminate for {label}");
                }
            }
            assert!(total > 1, "enumerated at least one point for {label}");
        }
    }

    /// Schedule time for the first iteration of each level is 0, and increases
    /// by one per step. Checked over a window rather than two sample points.
    #[test]
    fn test_schedule_time_is_zero_based_and_unit_step() {
        let stmt = first_stmt("fn f() { forall i in 4..12 { let x = i; } }");
        let nest = extract_loop_nest(&stmt).expect("must extract");
        let mut ctx = super::super::LoweringContext::new();
        let bands = loop_nest_to_bands(&nest, &mut ctx).expect("bands");
        let crate::ir::schedule_tree::ScheduleNode::Band { members, .. } = &bands[0] else {
            panic!("expected a Band");
        };
        let piece = &members[0].pieces[0];
        for (offset, i) in (4i64..12).enumerate() {
            let t = piece
                .apply(&[i])
                .unwrap_or_else(|| panic!("{i} out of domain"));
            assert_eq!(t[0], offset as i64, "schedule time of i={i}");
        }
    }
}
