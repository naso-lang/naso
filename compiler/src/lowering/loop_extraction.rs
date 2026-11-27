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

/// Convert a loop nest to schedule tree bands.
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
/// # `coincident`: a dependence claim, derived from the accesses
///
/// See [`level_is_parallel`]. It used to be `vec![depth == 1; depth]`, i.e. "a
/// one-dimensional band has nothing to serialise it against". That is not a
/// dependence argument: `forall i in 0..n { total = total + i; }` is
/// one-dimensional and carries a loop-carried dependence through `total`, so
/// the flag was a claim about dimensionality standing in for a claim about
/// memory. It routed such a band to the parallel emitter, which attached
/// `llvm.loop.parallel_accesses` / `omp parallel for` to a loop that LLVM would
/// have to trust.
///
/// # Iterators
///
/// Each level's source spelling is carried on `ScheduleNode::Band::iterators`,
/// not encoded in the domain's debug name.
///
/// # Domain
///
/// Each iterator contributes its bounds as inequality constraints over the
/// iterator space, with SYMBOLIC bounds contributing a constraint over a
/// parameter dimension rather than being skipped: `0..N` becomes `i >= 0` and
/// `-i + N >= 1`. `contains` therefore over-approximates only for a symbolic
/// bound the parameter list does not name, which is the safe direction: it can
/// admit too much, never too little.
use crate::ir::access_relation::{AccessRelation, AccessRelations, AccessType};
use crate::ir::affine_domain::{AffineConstraint, AffineDomain};
use crate::ir::affine_map::{AffineMap, AffineMapPiece, Matrix};
use crate::ir::schedule_tree::ScheduleNode;

/// Is loop level `level` of this band free of a dependence that serialises it?
///
/// # The rule
///
/// A level is parallel only if the band carries nothing that serialises it:
///
///  1. **Every writing access is injective in `level`.** A single-row affine map
///     `c + k*i_level + ...` reaches a different location at every value of `i_level`
///     exactly when `k != 0`. A write with `k == 0` -- the shape of `total = total + i`,
///     where the access is to a single scalar -- is reached by EVERY iteration, so it is
///     a loop-carried dependence. `A[i]`, with `k == 1`, is not.
///  2. **No two writes go to the same array.** A location in A is not a location in B,
///     but two writes into A are not provably independent of each other -- see the next
///     section.
///
/// Reads never contribute: a read of `A[i]` alongside the write of `A[i]` is the same
/// iteration reading what it just wrote, not a carried dependence. A band that only
/// reads is parallel.
///
/// # What is deliberately NOT concluded
///
/// Two writes to the same array with DIFFERENT coefficients (`A[i]` and `A[2i]`) do
/// collide, just not on every iteration: `A[2]` is written by the first at `i = 2` and
/// by the second at `i = 1`. Ruling that out needs a range test over the two accesses,
/// which this stage does not have, so the pair is treated as unproven and the level
/// comes out sequential. That is the conservative direction, and it is the same reason
/// an access map with more than one row is refused below rather than reasoned about.
///
/// # Assumption about column indexing
///
/// Column `level` of an access map is read as the band's level-`level` iterator.
/// Nothing in the IR currently guarantees the two use the same iterator ordering, so
/// this is stated rather than assumed silently; the guard in `coeff_in` refuses to
/// reason at all when the access map has fewer iterator dimensions than the band has
/// levels.
///
/// # Not yet reached from `.naso` source
///
/// The AST-to-PIR lowering records no `AccessRelation` for a `forall` body today, so
/// `loop_nest_to_bands` is handed an empty set and every level comes out sequential.
/// That is the correct output for "nothing was examined", and it is why a real `.naso`
/// loop is no longer tagged `omp parallel for`. The rule itself is exercised directly
/// against hand-built access relations in the tests below.
pub fn level_is_parallel(level: usize, accesses: &[&AccessRelation]) -> bool {
    // No access information at all: nothing was examined, so nothing may be claimed.
    if accesses.is_empty() {
        return false;
    }
    // Every writing access must be injective in `level`: two DIFFERENT iterations of
    // `level` must reach different locations through it, or the write is carried across
    // iterations.
    for a in accesses {
        if writes(a) && !is_injective_in(a, level) {
            return false;
        }
    }
    // Two writes to the SAME array are not provably independent of each other, whatever
    // their coefficients. A differing coefficient is not enough: `A[i]` and `A[2i]`
    // both reach `A[2]`, at `i = 2` and `i = 1` respectively. Ruling that out needs a
    // range test over the two accesses, which this stage does not have, so the pair is
    // treated as unproven and the level comes out sequential. Writes to DIFFERENT
    // arrays cannot conflict.
    for (i, a) in accesses.iter().enumerate() {
        for b in &accesses[i + 1..] {
            if writes(a) && writes(b) && same_array(a, b) {
                return false;
            }
        }
    }
    true
}

/// Does this access WRITE its location?
///
/// `AccessType::Reduction` is an accumulate-in-place: it reads the old value and writes
/// a new one, so it writes. `AccessRelation::is_write` deliberately does not include it,
/// and that helper is used elsewhere with a narrower meaning, so the question is asked
/// here explicitly rather than by changing the shared one.
fn writes(a: &AccessRelation) -> bool {
    matches!(
        a.access_type,
        AccessType::Write | AccessType::ReadWrite | AccessType::Reduction
    )
}

/// Two accesses to the same named array. Either one unnamed means "possibly the same",
/// since the array is not known.
fn same_array(a: &AccessRelation, b: &AccessRelation) -> bool {
    match (&a.array_name, &b.array_name) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// The access map's coefficient on iterator column `level`.
///
/// `None` means "this access map cannot answer the question": it is a multi-row map
/// producing a tuple of locations with no modelled component correspondence, or it has
/// fewer iterator dimensions than the band has levels, so the column does not exist.
/// Both are states a real program can reach and neither is evidence of anything.
fn coeff_in(a: &AccessRelation, level: usize) -> Option<i64> {
    for piece in &a.access_map.pieces {
        if piece.matrix.rows != 1 || piece.domain.n_iter <= level {
            return None;
        }
    }
    if a.access_map.pieces.is_empty() {
        return None;
    }
    Some(a.access_map.pieces[0].matrix.get(0, level))
}

/// Is this access injective in iterator column `level`?
///
/// A single-row affine map `c + k*i_level + ...` reaches a different location at every
/// value of `i_level` exactly when `k != 0`, so a nonzero coefficient is precisely the
/// condition for the level to be unserialised BY THIS ACCESS.
fn is_injective_in(a: &AccessRelation, level: usize) -> bool {
    coeff_in(a, level).is_some_and(|c| c != 0)
}

/// Convert a loop nest to schedule tree bands.
pub fn loop_nest_to_bands(
    nest: &LoopNest,
    accesses: &AccessRelations,
    _ctx: &mut super::LoweringContext,
) -> Result<Vec<ScheduleNode>, super::LoweringError> {
    let depth = nest.depth();
    if depth == 0 {
        return Ok(Vec::new());
    }

    // The symbolic constants this nest's bounds refer to, outermost level first,
    // lower bound before upper. These become the domain's parameter dimensions,
    // in this order, so a parameter's DIMENSION INDEX is its position here.
    let params = symbolic_params(nest);

    // Domain over `depth` iterator dimensions plus `params.len()` parameter
    // dimensions, bounded by the loop bounds.
    let domain = domain_from_nest(nest, depth, &params);
    let total_dims = depth + params.len();

    // One scheduling dimension per loop level: schedule time d = iteration d,
    // shifted so the first iteration of each loop sits at schedule time 0.
    //
    // One AffineMap per dimension, since ScheduleNode::Band documents
    // `members` as "one per loop level". Each is a single-row map.
    let mut members: Vec<AffineMap> = Vec::with_capacity(depth);
    let mut level = nest;
    let mut d = 0usize;
    loop {
        let mut row = Matrix::new(1, total_dims);
        row.set(0, d, 1);
        // Matrix apply is `m*point + constant`, so schedule time is
        // `i_d - lower_d`: the translation is NEGATIVE.
        //
        // A literal lower bound becomes a constant translation. A symbolic one
        // becomes a coefficient on its parameter dimension, which is the honest
        // affine form of `-lower`; defaulting it to 0 would silently place the
        // first iteration at schedule time `lower` instead of 0.
        //
        // An unrepresentable lower bound leaves the translation at 0, which does
        // NOT mean the first iteration is at schedule time 0 -- it means this
        // dimension's offset is unknown. Nothing can be claimed here, so nothing is.
        // The band is refused downstream: `iterator_bounds` finds no usable lower
        // bound, so `extract_bounds` reports it rather than lowering a loop whose
        // trip count it made up.
        match bound_kind(&level.lower_bound) {
            BoundKind::Literal(lo) => row.set_const(0, -lo),
            BoundKind::Symbolic(name) => {
                let p = params
                    .iter()
                    .position(|q| *q == name)
                    .expect("symbolic bound contributed its parameter");
                row.set(0, depth + p, -1);
            }
            BoundKind::Unrepresentable => {}
        }
        members.push(AffineMap {
            pieces: vec![AffineMapPiece::new(domain.clone(), row)],
        });
        d += 1;
        match &level.inner {
            Some(next) => level = next,
            None => break,
        }
    }

    // The leaf cannot be a Domain node: LoopNest does not carry the body
    // statement's id, and a Domain node pointing at the wrong statement would
    // be worse than an empty Sequence.
    let leaf = ScheduleNode::Sequence {
        children: Vec::new(),
    };

    // One flag per level, each decided by looking at the band's accesses.
    // `depth` is the level count, so the flags are `vec![level_is_parallel(..)]`.
    let coincident: Vec<bool> = (0..depth)
        .map(|level| level_is_parallel(level, &accesses_for(accesses)))
        .collect();

    Ok(vec![ScheduleNode::Band {
        members,
        coincident,
        iterators: nest.iterators().into_iter().map(str::to_string).collect(),
        child: Box::new(leaf),
    }])
}

/// The access relations this band's dependence argument is made over.
///
/// A `LoopNest` names no statements, so there is nothing to filter by yet; the
/// whole set is passed and `level_is_parallel` decides. When the front end
/// starts recording a `forall` body's accesses, this is where the statement ids
/// from the band's leaf would be used to narrow it.
fn accesses_for(accesses: &AccessRelations) -> Vec<&AccessRelation> {
    accesses.relations.iter().collect()
}

/// A loop bound, as far as the band's domain can represent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundKind<'a> {
    Literal(i64),
    /// A bare variable: the symbolic constant's name, and the name the domain's
    /// parameter dimension gets.
    Symbolic(&'a str),
    /// Anything else (`n + 1`, `f(x)`, `i * 2`). There is no encoding for it, and the
    /// domain over-approximates rather than inventing a constraint -- `extract_bounds`
    /// then finds no usable bound and refuses the band instead of guessing a trip
    /// count.
    Unrepresentable,
}

/// Classify a bound expression.
///
/// Only an int literal or a bare variable is representable in the band's domain.
fn bound_kind(expr: &Expr) -> BoundKind<'_> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(n)) => BoundKind::Literal(*n),
        ExprKind::Var(v) => BoundKind::Symbolic(&v.name),
        _ => BoundKind::Unrepresentable,
    }
}

/// The distinct symbolic constants a nest's bounds refer to, in a fixed order.
///
/// The ORDER is load-bearing: it is the parameter dimension order of the domain
/// `domain_from_nest` builds, and codegen maps a domain parameter dimension back
/// to a name through it.
fn symbolic_params(nest: &LoopNest) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut level = nest;
    loop {
        for bound in [&level.lower_bound, &level.upper_bound] {
            if let BoundKind::Symbolic(name) = bound_kind(bound) {
                if !name.is_empty() && !out.iter().any(|q| q == name) {
                    out.push(name.to_string());
                }
            }
        }
        match &level.inner {
            Some(next) => level = next,
            None => break,
        }
    }
    out
}

/// The iteration domain implied by a nest: `depth` iterator dimensions and one
/// dimension per symbolic bound, constrained by each level's bounds.
fn domain_from_nest(nest: &LoopNest, depth: usize, params: &[String]) -> AffineDomain {
    let mut constraints = Vec::new();
    let n_param = params.len();

    // One coefficient per dimension: `depth` iterators then `n_param` parameters.
    // Iterators before the current level are zeroed: level d only constrains
    // dimension d.
    let mut level_index = 0usize;
    let mut level = nest;
    loop {
        // Upper bound. A literal `hi` is `i_d <= hi - 1`, spelled `-i_d >= -(hi-1)`
        // over the integers; encoding it as `-i_d >= -hi` would be INCLUSIVE and
        // would admit `i_d == hi`, which `0..hi` does not iterate.
        //
        // A symbolic `N` gives `-i_d + N >= 1`, i.e. `i_d <= N - 1` -- the same
        // half-open range with `N` in place of `hi`. That constraint is what makes
        // a symbolic trip count computable at all; previously a symbolic bound
        // contributed nothing and the band was refused.
        let mut coeffs = vec![0i64; depth + n_param];
        coeffs[level_index] = -1;
        match bound_kind(&level.upper_bound) {
            BoundKind::Literal(hi) => {
                constraints.push(AffineConstraint::inequality(coeffs, -(hi - 1)));
            }
            BoundKind::Symbolic(name) => {
                if let Some(p) = params.iter().position(|q| q == name) {
                    coeffs[depth + p] = 1;
                    constraints.push(AffineConstraint::inequality(coeffs, 1));
                }
                // A name no parameter dimension claims, which can only happen if
                // `symbolic_params` and this loop disagree. No constraint, so the
                // domain over-approximates and the band is refused downstream.
            }
            // Unrepresentable: no constraint, deliberately. `contains` then
            // over-approximates -- admitting too much is safe, excluding a real
            // iteration is not -- and `iterator_bounds` finds no usable upper bound,
            // so `extract_bounds` refuses the band with a diagnostic naming the
            // construct rather than guessing a trip count.
            BoundKind::Unrepresentable => {}
        }
        // Lower bound: `i_d >= lo`, or `i_d - lo >= 0` for a symbolic one.
        let mut coeffs = vec![0i64; depth + n_param];
        coeffs[level_index] = 1;
        match bound_kind(&level.lower_bound) {
            BoundKind::Literal(lo) => {
                constraints.push(AffineConstraint::inequality(coeffs, lo));
            }
            BoundKind::Symbolic(name) => {
                if let Some(p) = params.iter().position(|q| q == name) {
                    coeffs[depth + p] = -1;
                    constraints.push(AffineConstraint::inequality(coeffs, 0));
                }
            }
            BoundKind::Unrepresentable => {}
        }
        level_index += 1;
        match &level.inner {
            Some(next) => level = next,
            None => break,
        }
    }

    AffineDomain::new(depth, n_param, constraints)
        .with_parameter_names(params.to_vec())
        .with_name(format!("nest depth {depth}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::AccessType;
    use crate::ir::affine_domain::AffineConstraint;
    use crate::ir::schedule_tree::StmtId;
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
    ///
    /// The access set is empty, which is what the front end currently produces; see
    /// `level_is_parallel` for why that makes every level sequential.
    fn band_for(src: &str) -> crate::ir::schedule_tree::ScheduleNode {
        band_for_with(src, &AccessRelations::new())
    }

    /// Build bands for a source against a specific set of access relations.
    fn band_for_with(
        src: &str,
        accesses: &AccessRelations,
    ) -> crate::ir::schedule_tree::ScheduleNode {
        let stmt = first_stmt(src);
        let nest = extract_loop_nest(&stmt).expect("must extract a loop nest");
        let mut ctx = super::super::LoweringContext::new();
        let bands = loop_nest_to_bands(&nest, accesses, &mut ctx).expect("bands");
        assert_eq!(bands.len(), 1, "expected exactly one band");
        bands.into_iter().next().unwrap()
    }

    /// A read of `A[i]` by statement 0 over a 1-dimensional domain `0..8`.
    fn read_a_at_i() -> AccessRelation {
        access(0, AccessType::Read, "A", |m| m.set(0, 0, 1))
    }

    /// A write to `A[i]` by statement 0 over a 1-dimensional domain `0..8`.
    fn write_a_at_i() -> AccessRelation {
        access(0, AccessType::Write, "A", |m| m.set(0, 0, 1))
    }

    /// A write to the SCALAR `total` -- one location, independent of the iterator.
    ///
    /// This is the shape of `total = total + i` inside a `forall i in ..`: a
    /// read-modify-write of a single location that every iteration touches, which is
    /// a loop-carried dependence.
    fn write_scalar_total() -> AccessRelation {
        access(0, AccessType::Reduction, "total", |_| {})
    }

    /// An access relation over a 1-D domain `0 <= i < 8`, with the access map's
    /// single row configured by `set_coeff`.
    fn access(
        stmt: usize,
        access_type: AccessType,
        array: &str,
        set_coeff: impl FnOnce(&mut Matrix),
    ) -> AccessRelation {
        let domain = AffineDomain::new(
            1,
            0,
            vec![
                AffineConstraint::inequality(vec![1], 0),
                AffineConstraint::inequality(vec![-1], -7),
            ],
        );
        let mut m = Matrix::new(1, 1);
        set_coeff(&mut m);
        AccessRelation::new(
            StmtId(stmt),
            domain.clone(),
            AffineMap::total(domain, m),
            access_type,
        )
        .with_array_name(array)
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

    /// `coincident` is a DEPENDENCE claim, so it is decided by the accesses and not
    /// by how many levels the band has.
    ///
    /// This replaces the previous assertion, which said a 1-D band is parallel and a
    /// 2-D band is not. That was a claim about dimensionality: it called
    /// `forall i in 0..4 { total = total + i; }` parallel, which carries a
    /// loop-carried dependence through `total`, and it called a 2-D band writing
    /// `t[i]` sequential, which does not. Both answers were unfounded in opposite
    /// directions.
    ///
    /// The cases below are the ones the rule actually decides:
    ///
    ///   * no accesses at all -> sequential (nothing was examined);
    ///   * a write to the scalar `total` -> sequential (every iteration reaches the
    ///     same location);
    ///   * a write to `A[i]` -> parallel (iteration `i` reaches location `i`, so no
    ///     two iterations of level 0 collide).
    #[test]
    fn coincident_follows_the_accesses_not_the_dimensionality() {
        let src = "fn f() { forall i in 0..8 { let x = i; } }";

        let none = band_for(src);
        let crate::ir::schedule_tree::ScheduleNode::Band { coincident, .. } = &none else {
            panic!("expected a Band");
        };
        assert_eq!(
            coincident,
            &vec![false],
            "with no access relations nothing was examined, so nothing may be claimed"
        );

        let mut scalar = AccessRelations::new();
        scalar.add(write_scalar_total());
        let carried = band_for_with(src, &scalar);
        let crate::ir::schedule_tree::ScheduleNode::Band { coincident, .. } = &carried else {
            panic!("expected a Band");
        };
        assert_eq!(
            coincident,
            &vec![false],
            "a reduction into one location is a loop-carried dependence"
        );

        let mut indexed = AccessRelations::new();
        indexed.add(write_a_at_i());
        let per_iteration = band_for_with(src, &indexed);
        let crate::ir::schedule_tree::ScheduleNode::Band { coincident, .. } = &per_iteration else {
            panic!("expected a Band");
        };
        assert_eq!(
            coincident,
            &vec![true],
            "A[i] is a different location per iteration of i, so the level is parallel"
        );
    }

    /// Two writes to the SAME array with the SAME coefficient conflict, and a
    /// differing coefficient is NOT taken as proof of independence.
    ///
    /// This is the part of the rule a dimensionality test could not express at all.
    /// The `A[i]` / `A[2i]` case is a deliberate limitation, not an oversight: those
    /// two accesses genuinely do collide (`A[2]` from `i = 2` and from `i = 1`), and
    /// ruling that out needs a range test this stage does not have. A test that asserted
    /// they were independent would be asserting something false.
    #[test]
    fn two_writes_to_one_array_are_judged_by_their_coefficients() {
        // `A[i]` and `A[i]`: the same location, so the level carries a WAW dependence.
        let mut same = AccessRelations::new();
        same.add(write_a_at_i());
        same.add(access(1, AccessType::Write, "A", |m| m.set(0, 0, 1)));
        assert!(
            !level_is_parallel(0, &same.relations.iter().collect::<Vec<_>>()),
            "A[i] and A[i] collide: the level is serialised"
        );

        // `A[i]` and `A[2i]`: differing coefficients. This does collide on some
        // iterations, and this stage cannot prove it does not, so the answer is
        // sequential -- the conservative direction.
        let mut differing = AccessRelations::new();
        differing.add(write_a_at_i());
        differing.add(access(1, AccessType::Write, "A", |m| m.set(0, 0, 2)));
        assert!(
            !level_is_parallel(0, &differing.relations.iter().collect::<Vec<_>>()),
            "a differing coefficient is unproven, not proven safe"
        );

        // Writes to DIFFERENT arrays cannot conflict, whatever their coefficients: a
        // location in A is not a location in B.
        let mut other_array = AccessRelations::new();
        other_array.add(write_a_at_i());
        other_array.add(access(1, AccessType::Write, "B", |m| m.set(0, 0, 2)));
        assert!(
            level_is_parallel(0, &other_array.relations.iter().collect::<Vec<_>>()),
            "A[i] and B[2i] cannot reach a shared location"
        );

        // A read of A[i] alongside the write of A[i] is the same iteration reading what
        // it just wrote, not a carried dependence, so it does not serialise the level.
        let mut read_write = AccessRelations::new();
        read_write.add(read_a_at_i());
        read_write.add(write_a_at_i());
        assert!(
            level_is_parallel(0, &read_write.relations.iter().collect::<Vec<_>>()),
            "reads do not create a loop-carried dependence"
        );
    }

    /// Read-only bands are parallel.
    #[test]
    fn a_band_that_only_reads_is_parallel() {
        let mut reads = AccessRelations::new();
        reads.add(read_a_at_i());
        assert!(
            level_is_parallel(0, &reads.relations.iter().collect::<Vec<_>>()),
            "reads alone carry no dependence"
        );
    }

    /// A level the analysis cannot decide is sequential, never parallel.
    ///
    /// The two undecidable shapes here are an access map with more than one row and a
    /// level beyond the access map's iterator range. Both are states a real program can
    /// reach, and both must come out `false`: a wrong `true` is an unfounded parallelism
    /// claim, which is the entire failure this rule exists to prevent.
    #[test]
    fn an_undecidable_level_is_sequential() {
        // Multi-row access map: no modelled component correspondence.
        let domain = AffineDomain::new(1, 0, vec![AffineConstraint::inequality(vec![1], 0)]);
        let mut m = Matrix::new(2, 1);
        m.set(0, 0, 1);
        m.set(1, 0, 1);
        let tuple_access = AccessRelation::new(
            StmtId(0),
            domain.clone(),
            AffineMap::total(domain, m),
            AccessType::Write,
        )
        .with_array_name("A");
        assert!(
            !level_is_parallel(0, &[&tuple_access]),
            "a two-row access map proves nothing about distinctness"
        );

        // A write whose level column is outside the access map's iterator range.
        let shallow = access(0, AccessType::Write, "A", |m| m.set(0, 0, 1));
        assert!(
            !level_is_parallel(3, &[&shallow]),
            "a level the access map has no column for cannot be reasoned about"
        );

        // No access relations at all.
        assert!(
            !level_is_parallel(0, &[]),
            "an empty access set is not evidence of independence"
        );
    }

    /// A symbolic bound is encoded as a PARAMETER DIMENSION, not skipped.
    ///
    /// This replaces the previous assertion, which required `contains(&[9999])` to be
    /// true because a symbolic upper bound contributed no constraint at all. That was
    /// the honest behaviour when there was nowhere to put the value, but it made the
    /// iteration space unbounded, so no trip count could be computed and the band had
    /// to be refused. The constraint is now there: `0..N` is `i >= 0` and
    /// `-i + N >= 1`, over a parameter dimension named `N`.
    ///
    /// The point is checked by EVALUATING the domain at several values of `N`, which
    /// is the only way to see that the bound moves with the parameter.
    #[test]
    fn a_symbolic_bound_becomes_a_named_parameter_dimension() {
        let band = band_for("fn f[N: nat](t: Tensor[f32,N]) { forall i in 0..N { t[i] = 1.0; } }");
        let domain = band_domain(&band);
        assert_eq!(domain.n_iter, 1, "one iterator");
        assert_eq!(domain.n_param, 1, "one symbolic constant");
        assert_eq!(
            domain.parameter_names,
            vec!["N".to_string()],
            "the parameter dimension must be NAMED, or nothing can look up its value"
        );

        for n in [0i64, 1, 4, 100] {
            for i in 0..n {
                assert!(domain.contains(&[i, n]), "i={i} must be in 0..{n}");
            }
            assert!(
                !domain.contains(&[n, n]),
                "{n} is the exclusive upper bound of 0..{n}"
            );
            assert!(!domain.contains(&[-1, n]), "below the lower bound");
        }
    }

    /// The bound `iterator_bounds` recovers from a symbolic domain is an affine
    /// expression in the parameter, not a constant.
    ///
    /// A constant here would be a fabricated trip count: the band's extent depends on
    /// `N`, and the expression is what carries that dependence to codegen.
    #[test]
    fn a_symbolic_bounds_upper_bound_is_an_expression_in_the_parameter() {
        let band = band_for("fn f[N: nat](t: Tensor[f32,N]) { forall i in 0..N { t[i] = 1.0; } }");
        let domain = band_domain(&band);
        let (lower, upper) = domain
            .iterator_bounds(0)
            .expect("a symbolic range is still a range");
        assert_eq!(lower.constant, 0, "the literal lower bound is 0");
        assert!(
            lower.coefficients.iter().all(|&c| c == 0),
            "the literal lower bound does not depend on N, so every parameter \
             coefficient is zero: {:?}",
            lower.coefficients
        );
        // `i <= N - 1`: one copy of parameter 0, minus one.
        assert_eq!(
            upper.coefficients,
            vec![1],
            "the upper bound is N plus a constant"
        );
        assert_eq!(upper.constant, -1, "`0..N` is half-open: the last i is N-1");
        assert_eq!(upper.evaluate(&[4]), 3, "with N = 4 the last i is 3");
    }

    /// A bound that is neither a literal nor a bare variable has no encoding, and the
    /// band must end up with no recoverable bounds rather than a guessed trip count.
    #[test]
    fn a_non_affine_bound_yields_no_bounds_rather_than_a_guessed_one() {
        let band =
            band_for("fn f[N: nat](t: Tensor[f32,N]) { forall i in 0..N+1 { t[i] = 1.0; } }");
        let domain = band_domain(&band);
        assert_eq!(
            domain.n_param, 0,
            "`N+1` contributes no parameter dimension"
        );
        assert!(
            domain.iterator_bounds(0).is_none(),
            "`i >= 0` alone does not bound the iteration space, and inventing the \
             missing half would be a fabricated trip count"
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
                iterators,
                ..
            } = &band
            else {
                panic!("expected a Band");
            };
            assert_eq!(members.len(), want, "members for {src}");
            assert_eq!(coincident.len(), want, "coincident flags for {src}");
            assert_eq!(
                iterators.len(),
                want,
                "one iterator name per level for {src}"
            );
        }
    }

    /// The band carries each level's source spelling, in order.
    ///
    /// This is the field that replaced parsing `AffineDomain::name` for a `nest(i)`
    /// prefix. The old channel meant the iterator's name was only as real as a format
    /// string, and a hand-built band with an unrecognised label silently bound nothing.
    #[test]
    fn a_band_carries_each_levels_iterator_name_in_source_order() {
        let band = band_for(
            "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 0..8 { t[i] = 1.0; } } }",
        );
        let crate::ir::schedule_tree::ScheduleNode::Band { iterators, .. } = &band else {
            panic!("expected a Band");
        };
        assert_eq!(iterators, &vec!["i".to_string(), "j".to_string()]);
        assert_eq!(band.iterator_at(0), Some("i"));
        assert_eq!(band.iterator_at(1), Some("j"));
        assert_eq!(band.iterator_at(2), None, "there is no third level");
    }

    /// An EMPTY iterator name means "not declared", and is reported as absent.
    ///
    /// This is the state a hand-built band is in, and it is what stops a body from
    /// reading an induction variable that was never bound: the backend binds nothing,
    /// so reading the name is a diagnostic rather than a zero read.
    #[test]
    fn an_undeclared_iterator_is_reported_as_absent() {
        let member = {
            let band = band_for("fn f() { forall i in 0..8 { let x = i; } }");
            let crate::ir::schedule_tree::ScheduleNode::Band { members, .. } = &band else {
                panic!("expected a Band");
            };
            members[0].clone()
        };
        let node = crate::ir::schedule_tree::ScheduleNode::band_with_iterators(
            vec![member],
            vec![false],
            vec![String::new()],
            crate::ir::schedule_tree::ScheduleNode::empty(),
        );
        assert_eq!(
            node.iterator_at(0),
            None,
            "an empty name must not be handed out as a binding"
        );
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
            let bands =
                loop_nest_to_bands(&nest, &AccessRelations::new(), &mut ctx).expect("bands");
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
        let bands = loop_nest_to_bands(&nest, &AccessRelations::new(), &mut ctx).expect("bands");
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
