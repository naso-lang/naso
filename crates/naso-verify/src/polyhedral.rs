//! Polyhedral loop invariant encoding for SMT-LIB2.
//!
//! This module translates Naso's polyhedral loop constructs (forall, tile, fuse)
//! into quantified SMT-LIB2 formulas over iteration domains.

#[cfg(feature = "z3")]
use crate::error::VerifyError;
#[cfg(feature = "z3")]
use crate::smtlib::{Sort, Term, builder::*};
#[cfg(feature = "z3")]
use indexmap::IndexMap;
#[cfg(feature = "z3")]
use naso_compiler::ast::expr::ExprKind;

#[cfg(feature = "z3")]
/// Polyhedral iteration domain: a set of integer tuples defined by affine constraints.
#[derive(Debug, Clone)]
pub struct IterationDomain {
    /// Dimension names (e.g., ["i", "j", "k"])
    pub dims: Vec<String>,
    /// Affine constraints: A * x + b >= 0
    /// Each constraint is (coefficients, constant)
    pub constraints: Vec<(Vec<i64>, i64)>,
}

#[cfg(feature = "z3")]
impl IterationDomain {
    pub fn new(dims: Vec<String>) -> Self {
        Self {
            dims,
            constraints: Vec::new(),
        }
    }

    pub fn add_constraint(&mut self, coeffs: Vec<i64>, constant: i64) {
        self.constraints.push((coeffs, constant));
    }

    /// Generate SMT formula for: x in domain
    pub fn in_domain(&self, vars: &[Term]) -> Term {
        let mut conds = Vec::new();
        for (coeffs, constant) in &self.constraints {
            // Sum(coeffs[i] * vars[i]) + constant >= 0
            let mut sum_terms = Vec::new();
            for (i, coeff) in coeffs.iter().enumerate() {
                if *coeff != 0 {
                    sum_terms.push(mul(vec![int(*coeff), vars[i].clone()]));
                }
            }
            let sum = if sum_terms.is_empty() {
                int(0)
            } else if sum_terms.len() == 1 {
                sum_terms[0].clone()
            } else {
                add(sum_terms)
            };
            conds.push(ge(add(vec![sum, int(*constant)]), int(0)));
        }
        if conds.is_empty() {
            bool(true)
        } else if conds.len() == 1 {
            conds[0].clone()
        } else {
            and(conds)
        }
    }
}

#[cfg(feature = "z3")]
/// Loop invariant: a property that holds at loop entry and is preserved by each iteration.
#[derive(Debug, Clone)]
pub struct LoopInvariant {
    /// The quantified variables (loop indices)
    pub vars: Vec<(String, Sort)>,
    /// The iteration domain
    pub domain: IterationDomain,
    /// The invariant predicate
    pub predicate: Term,
}

#[cfg(feature = "z3")]
impl LoopInvariant {
    pub fn new(vars: Vec<(String, Sort)>, domain: IterationDomain, predicate: Term) -> Self {
        Self {
            vars,
            domain,
            predicate,
        }
    }

    /// Generate SMT formula: forall vars in domain. predicate
    pub fn to_smt(&self) -> Term {
        let var_terms: Vec<Term> = self
            .vars
            .iter()
            .map(|(name, sort)| var(name, sort.clone()))
            .collect();

        let in_dom = self.domain.in_domain(&var_terms);
        let body = implies(in_dom, self.predicate.clone());

        forall(self.vars.clone(), body)
    }
}

#[cfg(feature = "z3")]
/// Tiling information for polyhedral optimization.
#[derive(Debug, Clone)]
pub struct TilingInfo {
    /// Original loop indices
    pub original_indices: Vec<String>,
    /// Tile sizes
    pub tile_sizes: Vec<u64>,
    /// Tile indices (outer loops)
    pub tile_indices: Vec<String>,
    /// Intra-tile indices (inner loops)
    pub intra_indices: Vec<String>,
}

#[cfg(feature = "z3")]
/// Fusion information for polyhedral optimization.
#[derive(Debug, Clone)]
pub struct FusionInfo {
    /// Loops being fused
    pub loop_ids: Vec<String>,
    /// Fused iteration domain
    pub fused_domain: IterationDomain,
    /// Schedule: mapping from original to fused indices
    pub schedule: IndexMap<String, Vec<Term>>, // original_var -> affine expression in fused vars
}

#[cfg(feature = "z3")]
/// Tracks polyhedral state during lowering.
pub struct PolyhedralTracker {
    /// Current loop invariants being tracked
    invariants: Vec<LoopInvariant>,
    /// Tiling information
    tilings: IndexMap<String, TilingInfo>,
    /// Fusion information
    fusions: IndexMap<String, FusionInfo>,
    /// Current loop nesting level
    loop_depth: u32,
    /// Loop index variables at each level
    loop_indices: Vec<String>,
}

#[cfg(feature = "z3")]
impl PolyhedralTracker {
    pub fn new() -> Self {
        Self {
            invariants: Vec::new(),
            tilings: IndexMap::new(),
            fusions: IndexMap::new(),
            loop_depth: 0,
            loop_indices: Vec::new(),
        }
    }

    /// Enter a forall loop.
    pub fn enter_forall(&mut self, index: String, domain: IterationDomain) {
        self.loop_depth += 1;
        self.loop_indices.push(index);
        // Store the domain for the current loop level
        // In a full implementation, this would be used for invariant generation
        let _ = domain; // Domain stored for future use in invariant generation
    }

    /// Exit a forall loop.
    pub fn exit_forall(&mut self) -> Option<String> {
        self.loop_depth = self.loop_depth.saturating_sub(1);
        self.loop_indices.pop()
    }

    /// Add a loop invariant at current nesting level.
    pub fn add_invariant(&mut self, predicate: Term) {
        if self.loop_depth == 0 {
            return; // Not in a loop
        }

        // Build domain from current loop indices
        let domain = IterationDomain::new(self.loop_indices.clone());
        // In real implementation, domain constraints come from loop bounds
        // For now, assume 0 <= i < N for each index

        let vars: Vec<(String, Sort)> = self
            .loop_indices
            .iter()
            .map(|name| (name.clone(), Sort::Int))
            .collect();

        self.invariants
            .push(LoopInvariant::new(vars, domain, predicate));
    }

    /// Register tiling transformation.
    pub fn register_tiling(&mut self, loop_id: String, info: TilingInfo) {
        self.tilings.insert(loop_id, info);
    }

    /// Register fusion transformation.
    pub fn register_fusion(&mut self, fusion_id: String, info: FusionInfo) {
        self.fusions.insert(fusion_id, info);
    }

    /// Get all invariants.
    pub fn invariants(&self) -> &[LoopInvariant] {
        &self.invariants
    }

    /// Generate SMT constraints for all invariants.
    pub fn generate_invariant_constraints(&self) -> Vec<Term> {
        self.invariants.iter().map(|inv| inv.to_smt()).collect()
    }

    /// Generate SMT declarations for loop indices.
    pub fn generate_declarations(&self, script: &mut crate::smtlib::Script) {
        for inv in &self.invariants {
            for (name, sort) in &inv.vars {
                script.declare_const(name, sort.clone());
            }
        }
    }
}

#[cfg(feature = "z3")]
impl Default for PolyhedralTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "z3")]
/// Encode polyhedral constructs for a function.
pub fn encode_polyhedral_function(
    func: &naso_compiler::ast::Function,
    tracker: &mut PolyhedralTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    if let Some(body_expr) = &func.body.expr {
        constraints.extend(encode_polyhedral_expr(body_expr.as_ref(), tracker)?);
    }
    for stmt in &func.body.stmts {
        if let naso_compiler::ast::StmtKind::Expr(expr) = &stmt.kind {
            constraints.extend(encode_polyhedral_expr(expr, tracker)?);
        }
    }

    // Add all invariant constraints
    constraints.extend(tracker.generate_invariant_constraints());

    Ok(constraints)
}

#[cfg(feature = "z3")]
/// Encode polyhedral constructs in an expression.
pub fn encode_polyhedral_expr(
    expr: &naso_compiler::ast::Expr,
    tracker: &mut PolyhedralTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    match &expr.kind {
        ExprKind::For(loop_expr) => {
            // Parse domain bounds from loop iterator expression
            let mut iter_domain = IterationDomain::new(vec![loop_expr.var.name.clone()]);

            // Extract bounds from loop iterator - check if it's a range-like call
            #[allow(clippy::collapsible_if)]
            if let ExprKind::Call(func, args) = &loop_expr.iter.kind {
                if let ExprKind::Var(fname) = &func.kind {
                    if fname.name == "range" && args.len() >= 2 {
                        // range(start, end)
                        if let ExprKind::Literal(naso_compiler::ast::Literal::Int(s)) =
                            &args[0].kind
                        {
                            iter_domain.add_constraint(vec![1], -s); // i - s >= 0
                        }
                        if let ExprKind::Literal(naso_compiler::ast::Literal::Int(e)) =
                            &args[1].kind
                        {
                            iter_domain.add_constraint(vec![-1], e - 1); // -i + e - 1 >= 0
                        }
                    }
                }
            }

            tracker.enter_forall(loop_expr.var.name.clone(), iter_domain.clone());

            // Process body - loop body is a Block
            if let Some(body_expr) = &loop_expr.body.expr {
                constraints.extend(encode_polyhedral_expr(body_expr.as_ref(), tracker)?);
            }
            for stmt in &loop_expr.body.stmts {
                if let naso_compiler::ast::StmtKind::Expr(stmt_expr) = &stmt.kind {
                    constraints.extend(encode_polyhedral_expr(stmt_expr, tracker)?);
                }
            }

            tracker.exit_forall();
        }
        ExprKind::Call(_func, args) => {
            for arg in args {
                constraints.extend(encode_polyhedral_expr(arg, tracker)?);
            }
        }
        ExprKind::Let(binding) => {
            constraints.extend(encode_polyhedral_expr(&binding.value, tracker)?);
            // Let binding body is handled by the outer context
        }
        ExprKind::LetInOut(binding) => {
            constraints.extend(encode_polyhedral_expr(&binding.value, tracker)?);
            // LetInOut binding body is handled by the outer context
        }
        ExprKind::LetConsume(binding) => {
            constraints.extend(encode_polyhedral_expr(&binding.value, tracker)?);
            // LetConsume binding body is handled by the outer context
        }
        ExprKind::Block(block) => {
            if let Some(body_expr) = &block.expr {
                constraints.extend(encode_polyhedral_expr(body_expr.as_ref(), tracker)?);
            }
            for stmt in &block.stmts {
                if let naso_compiler::ast::StmtKind::Expr(stmt_expr) = &stmt.kind {
                    constraints.extend(encode_polyhedral_expr(stmt_expr, tracker)?);
                }
            }
        }
        ExprKind::If(cond, then_e, else_e) => {
            constraints.extend(encode_polyhedral_expr(cond, tracker)?);
            constraints.extend(encode_polyhedral_expr(then_e, tracker)?);
            if let Some(else_e) = else_e {
                constraints.extend(encode_polyhedral_expr(else_e, tracker)?);
            }
        }
        ExprKind::Binary(_, lhs, rhs) => {
            constraints.extend(encode_polyhedral_expr(lhs, tracker)?);
            constraints.extend(encode_polyhedral_expr(rhs, tracker)?);
        }
        ExprKind::Unary(_, operand) => {
            constraints.extend(encode_polyhedral_expr(operand, tracker)?);
        }
        ExprKind::MethodCall(receiver, _, args) => {
            constraints.extend(encode_polyhedral_expr(receiver, tracker)?);
            for arg in args {
                constraints.extend(encode_polyhedral_expr(arg, tracker)?);
            }
        }
        ExprKind::QuantumOp(_) => {
            // Quantum ops don't have polyhedral content
        }
        _ => {
            // For other expression types, we don't need special handling
        }
    }

    Ok(constraints)
}
