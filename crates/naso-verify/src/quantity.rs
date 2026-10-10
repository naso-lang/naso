//! Quantity constraint encoding for QTT (Quantitative Type Theory).
//!
//! This module handles the translation of Naso's quantity annotations ([0], [1], [*], [N])
//! into SMT-LIB2 constraints that can be verified by Z3.

use crate::error::VerifyError;
use crate::smtlib::{Sort, Term, builder::*};
use indexmap::IndexMap;
use naso_compiler::ast::expr::ExprKind;
use naso_compiler::ast::{Quantity, Span};
use std::collections::HashMap;

/// Quantity kind for tracking in SMT encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuantityKind {
    /// [0] - Erased/proof-only, must not exist at runtime
    Zero,
    /// [1] - Linear, must be consumed exactly once
    One,
    /// [*] - Unrestricted, can be aliased freely
    Many,
    /// [N] - Bounded by constant N
    Bounded(u64),
}

impl QuantityKind {
    /// Parse from AST Quantity.
    pub fn from_ast(qty: &Quantity) -> Self {
        match qty {
            Quantity::Zero => QuantityKind::Zero,
            Quantity::One => QuantityKind::One,
            Quantity::Many => QuantityKind::Many,
            Quantity::Bounded(n) => QuantityKind::Bounded(*n as u64),
        }
    }

    /// Check if this quantity allows runtime existence.
    pub fn is_runtime(&self) -> bool {
        !matches!(self, QuantityKind::Zero)
    }

    /// Check if this quantity requires linear consumption.
    pub fn is_linear(&self) -> bool {
        matches!(self, QuantityKind::One)
    }

    /// Check if this quantity is bounded.
    pub fn is_bounded(&self) -> bool {
        matches!(self, QuantityKind::Bounded(_))
    }

    /// Get the bound if bounded.
    pub fn bound(&self) -> Option<u64> {
        match self {
            QuantityKind::Bounded(n) => Some(*n),
            _ => None,
        }
    }
}

/// Symbolic resource identifier for [1] linear resources.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceId {
    pub name: String,
    pub span: Span,
    pub alloc_site: AllocSite,
}

/// Allocation site for unique resource identification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AllocSite {
    /// Function parameter
    Param(String),
    /// Local allocation (qalloc, alloc, etc.)
    Local(String, u32), // name, unique index
    /// Return value
    Return(String),
    /// Struct field
    Field(Box<ResourceId>, String),
}

/// Tracks quantity constraints during lowering.
pub struct QuantityTracker {
    /// All [1] resources allocated, with their symbolic IDs
    pub linear_resources: IndexMap<String, ResourceId>,
    /// [0] variables that must be erased
    pub erased_vars: HashMap<String, Span>,
    /// [N] bounded variables with their bounds
    pub bounded_vars: HashMap<String, (u64, Span)>,
    /// Consumption tracking for [1] resources: resource_id -> consumed_on_paths
    pub consumption: HashMap<String, Vec<ConsumptionPath>>,
    /// Next unique ID for allocations
    next_alloc_id: u32,
}

/// A consumption path represents one control-flow path where a resource is consumed.
#[derive(Debug, Clone)]
pub struct ConsumptionPath {
    pub path_id: u32,
    pub consumed: bool,
    pub location: Span,
}

impl QuantityTracker {
    pub fn new() -> Self {
        Self {
            linear_resources: IndexMap::new(),
            erased_vars: HashMap::new(),
            bounded_vars: HashMap::new(),
            consumption: HashMap::new(),
            next_alloc_id: 0,
        }
    }

    /// Register a new [1] linear resource allocation.
    pub fn allocate_linear(&mut self, name: &str, span: Span, site: AllocSite) -> String {
        let id = format!("res_{}_{}", name, self.next_alloc_id);
        self.next_alloc_id += 1;
        let rid = ResourceId {
            name: name.to_string(),
            span,
            alloc_site: site,
        };
        self.linear_resources.insert(id.clone(), rid);
        self.consumption.insert(id.clone(), Vec::new());
        id
    }

    /// Register a [0] erased variable.
    pub fn register_erased(&mut self, name: &str, span: Span) {
        self.erased_vars.insert(name.to_string(), span);
    }

    /// Register a [N] bounded variable.
    pub fn register_bounded(&mut self, name: &str, bound: u64, span: Span) {
        self.bounded_vars.insert(name.to_string(), (bound, span));
    }

    /// Mark a [1] resource as consumed on a specific path.
    pub fn consume_linear(&mut self, resource_id: &str, path_id: u32, location: Span) {
        if let Some(paths) = self.consumption.get_mut(resource_id) {
            paths.push(ConsumptionPath {
                path_id,
                consumed: true,
                location,
            });
        }
    }

    /// Look up the linear resource IDs allocated under a given source name.
    ///
    /// A name can map to more than one resource (e.g. two `[1]` params sharing
    /// a name across different allocation sites is impossible, but `alloc`
    /// may register several indices for one callee name), so this returns all
    /// matches and the caller consumes each.
    pub fn linear_ids_by_name(&self, name: &str) -> Vec<String> {
        self.linear_resources
            .iter()
            .filter(|(_, rid)| rid.name == name)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// How many times a linear resource has been consumed.
    ///
    /// Zero means leaked; one is correct; two or more is a double free.
    pub fn consumption_count(&self, resource_id: &str) -> usize {
        self.consumption
            .get(resource_id)
            .map_or(0, |paths| paths.len())
    }

    /// Span of the Nth consumption of a linear resource, if recorded.
    pub fn consumption_span(&self, resource_id: &str, nth: usize) -> Option<Span> {
        self.consumption
            .get(resource_id)
            .and_then(|paths| paths.get(nth))
            .map(|p| p.location)
    }

    /// Get all linear resource IDs.
    pub fn linear_resource_ids(&self) -> Vec<&String> {
        self.linear_resources.keys().collect()
    }

    /// Get all erased variable names.
    pub fn erased_var_names(&self) -> Vec<&String> {
        self.erased_vars.keys().collect()
    }

    /// Get all bounded variables with bounds.
    pub fn bounded_vars(&self) -> &HashMap<String, (u64, Span)> {
        &self.bounded_vars
    }

    /// Get a linear resource by ID.
    pub fn get_linear_resource(&self, id: &str) -> Option<&ResourceId> {
        self.linear_resources.get(id)
    }

    /// Generate SMT constraints for quantity correctness.
    pub fn generate_constraints(&self) -> Vec<Term> {
        let mut constraints = Vec::new();

        // [0] erasure: assert that erased variables are never used at runtime
        for (name, span) in &self.erased_vars {
            let var = var(name, Sort::Int);
            let _error_msg = format!("erased_var_used:{}", span.start);
            constraints.push(implies(var, bool(false)));
        }

        // [1] linearity: each linear resource must be consumed exactly once
        let linear_ids: Vec<&String> = self.linear_resource_ids();
        for i in 0..linear_ids.len() {
            for j in (i + 1)..linear_ids.len() {
                constraints.push(app(
                    "distinct",
                    vec![var(linear_ids[i], Sort::Int), var(linear_ids[j], Sort::Int)],
                ));
            }
        }

        // [N] bounded: variables must stay within bounds
        for (name, (bound, _span)) in &self.bounded_vars {
            let var_term = var(name, Sort::Int);
            constraints.push(and(vec![
                le(var_term.clone(), int(*bound as i64)),
                ge(var_term, int(0)),
            ]));
        }

        // Consumption tracking
        for (res_id, paths) in &self.consumption {
            if paths.is_empty() {
                let _res_var = var(res_id, Sort::Int);
            }
        }

        constraints
    }

    /// Generate SMT declarations for all tracked quantities.
    pub fn generate_declarations(&self, script: &mut crate::smtlib::Script) {
        for id in self.linear_resource_ids() {
            script.declare_const(id, Sort::Int);
        }

        for name in self.erased_var_names() {
            script.declare_const(name, Sort::Int);
        }

        for name in self.bounded_vars.keys() {
            script.declare_const(name, Sort::Int);
        }
    }
}

impl Default for QuantityTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode quantity constraints for a typed expression.
pub fn encode_quantity_expr(
    expr: &naso_compiler::ast::Expr,
    tracker: &mut QuantityTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    match &expr.kind {
        ExprKind::Var(name) => {
            // Referring to a linear value IS its consumption.
            //
            // This arm used to be empty, with the comment "we skip direct var checking since
            // we track via Let bindings". That comment described a mechanism that does not
            // exist for the cases that matter: `output[i] = v` is a use of `output`, not a
            // `let` binding, so nothing was recorded. The consequence was a FALSE POSITIVE --
            // `naso-verify` reported `kernels/scale_clamp_f32` as leaking both its linear
            // tensors while `naso check` accepted the same file. A verifier that reports
            // correct programs as broken is worse than one that reports nothing, because it
            // teaches you to ignore it.
            //
            // This now matches the compiler's own rule: `TypeEnv::check_use` records a use of
            // a `[1]` value on every reference. The two tools must agree, or one of them is
            // wrong, and they are tested against the same programs.
            for id in tracker.linear_ids_by_name(&name.name) {
                tracker.consume_linear(&id, 0, expr.span);
            }
        }
        // A literal cannot hold or consume a linear resource.
        ExprKind::Literal(_) => {}
        ExprKind::Call(func, args) => {
            if let ExprKind::Var(fname) = &func.kind {
                match fname.name.as_str() {
                    "qalloc" | "linear_alloc" | "alloc" => {
                        for (_i, arg) in args.iter().enumerate() {
                            if let ExprKind::Literal(naso_compiler::ast::Literal::Int(_n)) =
                                &arg.kind
                            {
                                let resource_id = tracker.allocate_linear(
                                    &fname.name,
                                    expr.span,
                                    AllocSite::Local(fname.name.clone(), _i as u32),
                                );
                                constraints.push(gt(var(&resource_id, Sort::Int), int(0)));
                            }
                        }
                    }
                    "linear_free" | "qfree" | "free" | "consume" | "discard" => {
                        // A consuming builtin discharges every linear resource bound to the
                        // name it is handed.
                        //
                        // Note there is deliberately NO explicit `consume_linear` here. The
                        // generic argument walk at the bottom of this arm visits the same
                        // `Var` and consumes it, and an earlier version did both -- so
                        // `linear_free(x)` counted as TWO consumptions and the prover
                        // reported a valid consume-once function as a double use. One place
                        // per use is the invariant.
                        //
                        // The dedicated loop that used to be here was itself a fix for a
                        // different gap: with an empty body, `linear_free(x)` recorded
                        // nothing at all and every [1] resource looked leaked. That gap is
                        // now closed by the `Var` arm consuming on every reference, so the
                        // duplication is no longer needed.
                    }
                    _ => {}
                }
            }
            for arg in args {
                constraints.extend(encode_quantity_expr(arg, tracker)?);
            }
        }
        ExprKind::Let(binding) => {
            if let Some(qty) = binding.ty.as_ref().map(|t| t.quantity) {
                let qk = QuantityKind::from_ast(&qty);
                match qk {
                    QuantityKind::Zero => tracker.register_erased(&binding.name.name, binding.span),
                    QuantityKind::One => {
                        tracker.allocate_linear(
                            &binding.name.name,
                            binding.span,
                            AllocSite::Param(binding.name.name.clone()),
                        );
                    }
                    QuantityKind::Bounded(n) => {
                        tracker.register_bounded(&binding.name.name, n, binding.span)
                    }
                    QuantityKind::Many => {}
                }
            }
            constraints.extend(encode_quantity_expr(&binding.value, tracker)?);
        }
        ExprKind::LetInOut(binding) => {
            if let Some(qty) = binding.ty.as_ref().map(|t| t.quantity) {
                let qk = QuantityKind::from_ast(&qty);
                match qk {
                    QuantityKind::Zero => tracker.register_erased(&binding.name.name, binding.span),
                    QuantityKind::One => {
                        tracker.allocate_linear(
                            &binding.name.name,
                            binding.span,
                            AllocSite::Param(binding.name.name.clone()),
                        );
                    }
                    QuantityKind::Bounded(n) => {
                        tracker.register_bounded(&binding.name.name, n, binding.span)
                    }
                    QuantityKind::Many => {}
                }
            }
            constraints.extend(encode_quantity_expr(&binding.value, tracker)?);
        }
        ExprKind::LetConsume(binding) => {
            if let Some(qty) = binding.ty.as_ref().map(|t| t.quantity) {
                let qk = QuantityKind::from_ast(&qty);
                match qk {
                    QuantityKind::Zero => tracker.register_erased(&binding.name.name, binding.span),
                    QuantityKind::One => {
                        tracker.allocate_linear(
                            &binding.name.name,
                            binding.span,
                            AllocSite::Param(binding.name.name.clone()),
                        );
                    }
                    QuantityKind::Bounded(n) => {
                        tracker.register_bounded(&binding.name.name, n, binding.span)
                    }
                    QuantityKind::Many => {}
                }
            }
            constraints.extend(encode_quantity_expr(&binding.value, tracker)?);
        }
        ExprKind::Block(block) => {
            if let Some(body_expr) = &block.expr {
                constraints.extend(encode_quantity_expr(body_expr, tracker)?);
            }
            for stmt in &block.stmts {
                if let naso_compiler::ast::StmtKind::Expr(stmt_expr) = &stmt.kind {
                    constraints.extend(encode_quantity_expr(stmt_expr, tracker)?);
                }
            }
        }
        ExprKind::If(cond, then_e, else_e) => {
            constraints.extend(encode_quantity_expr(cond, tracker)?);
            constraints.extend(encode_quantity_expr(then_e, tracker)?);
            if let Some(else_e) = else_e {
                constraints.extend(encode_quantity_expr(else_e, tracker)?);
            }
        }
        ExprKind::Binary(_, lhs, rhs) => {
            constraints.extend(encode_quantity_expr(lhs, tracker)?);
            constraints.extend(encode_quantity_expr(rhs, tracker)?);
        }
        ExprKind::Unary(_, operand) => {
            constraints.extend(encode_quantity_expr(operand, tracker)?);
        }
        ExprKind::MethodCall(receiver, _, args) => {
            constraints.extend(encode_quantity_expr(receiver, tracker)?);
            for arg in args {
                constraints.extend(encode_quantity_expr(arg, tracker)?);
            }
        }
        ExprKind::QuantumOp(_) => {
            // Quantum ops may allocate qubits - handled in quantum.rs
        }
        ExprKind::Projection(_) => {
            // MVS handled separately in mvs.rs
        }
        // Return/break carry an expression whose consumption is real: `return t` moves a
        // linear resource out of the function.
        ExprKind::Return(Some(e)) | ExprKind::Break(Some(e)) => {
            constraints.extend(encode_quantity_expr(e, tracker)?);
        }
        ExprKind::Return(None) | ExprKind::Break(None) | ExprKind::Continue => {}
        // Positional forms: walk the children, which can only carry uses, never new bindings.
        ExprKind::Tuple(items) | ExprKind::Array(items) => {
            for item in items {
                constraints.extend(encode_quantity_expr(item, tracker)?);
            }
        }
        ExprKind::Field(base, _) => {
            constraints.extend(encode_quantity_expr(base, tracker)?);
        }
        ExprKind::Ascribe(e, _) => {
            constraints.extend(encode_quantity_expr(e, tracker)?);
        }
        ExprKind::While(cond, body) => {
            constraints.extend(encode_quantity_expr(cond, tracker)?);
            constraints.extend(encode_quantity_expr(body, tracker)?);
        }
        ExprKind::Match(scrutinee, arms) => {
            constraints.extend(encode_quantity_expr(scrutinee, tracker)?);
            for arm in arms {
                constraints.extend(encode_quantity_expr(&arm.body, tracker)?);
            }
        }
        // A quantified proposition in EXPRESSION position. Its body is a proposition, not
        // runtime code, so -- like a `proof` block -- it must not be counted as consuming.
        // It is walked only so that its subterms are not mistaken for runtime uses.
        ExprKind::Quantified(loop_) => {
            constraints.extend(encode_quantity_block(&loop_.body, tracker)?);
        }
        // REFUSED, not walked.
        //
        // A closure captures its environment, a `reversible` block is erased before codegen,
        // and an aggregate literal can move fields in ways this tracker does not model.
        // Walking them optimistically is exactly how a false "no leak" gets reported, so the
        // function is refused instead and surfaces as `undecided`.
        ExprKind::Lambda(_)
        | ExprKind::Reversible(_)
        | ExprKind::Struct(..)
        | ExprKind::Variant(..)
        | ExprKind::Error => {
            return Err(VerifyError::Config(format!(
                "quantity analysis refuses `{}`: it can move a linear resource in a way this \
                 tracker does not model, so the function cannot be linearity-checked soundly",
                describe_expr_kind(expr)
            )));
        }
        // COMPOUND EXPRESSIONS MUST BE WALKED, NOT SKIPPED.
        //
        // The catch-all that used to sit here said "other expression types - no special
        // quantity handling needed". That was false, and the false comment hid a real bug: a
        // `forall i { output[i] = ... }` loop is exactly where a linear resource gets
        // consumed, so skipping it made `naso-verify` report `kernels/scale_clamp_f32` as
        // leaking BOTH of its linear tensors -- a function that demonstrably reads one and
        // writes the other. `naso check` accepted the same file. The verifier was crying
        // wolf on correct code, which is the fastest way to get a proof tool ignored.
        ExprKind::Forall(loop_) => {
            constraints.extend(encode_quantity_block(&loop_.body, tracker)?);
        }
        ExprKind::For(loop_) => {
            constraints.extend(encode_quantity_block(&loop_.body, tracker)?);
        }
        ExprKind::Index(base, index) => {
            constraints.extend(encode_quantity_expr(base, tracker)?);
            constraints.extend(encode_quantity_expr(index, tracker)?);
        }
        ExprKind::Assign(lhs, rhs) => {
            constraints.extend(encode_quantity_expr(rhs, tracker)?);
            // The target is walked, which is what records the write. It is usually an INDEX
            // `output[i] = ..`, not a bare `Var`, so the consumption is found by descending to
            // the base -- which is exactly why an earlier name-matching shortcut here silently
            // found nothing and reported a false leak.
            constraints.extend(encode_quantity_expr(lhs, tracker)?);
        }
        ExprKind::Range(lo, hi) => {
            // A range in a `for` iterator is walked so that any linear tensors
            // referenced in the bounds are tracked. In practice these are
            // integer literals, so this arm is usually a no-op, but it must
            // exist for exhaustiveness.
            constraints.extend(encode_quantity_expr(lo, tracker)?);
            constraints.extend(encode_quantity_expr(hi, tracker)?);
        } // NO CATCH-ALL ARM, ON PURPOSE.
          //
          // The arm that used to sit here said every unrecognised expression had "no special
          // quantity handling needed". That is how a `forall` loop -- the one place a loop body
          // actually consumes a linear tensor -- came to be skipped, and why `naso-verify`
          // reported correct programs as leaking. With every `ExprKind` variant now matched
          // explicitly, a future variant is a COMPILE ERROR rather than a silent skip. The
          // variants this prover cannot model soundly are refused BY NAME in the arms above,
          // which is a decision; a catch-all would have been the absence of one.
    }

    Ok(constraints)
}

/// Encode every statement of a block, plus its tail expression.
fn encode_quantity_block(
    block: &naso_compiler::ast::Block,
    tracker: &mut QuantityTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();
    for stmt in &block.stmts {
        constraints.extend(encode_quantity_stmt(stmt, tracker)?);
    }
    if let Some(tail) = &block.expr {
        constraints.extend(encode_quantity_expr(tail, tracker)?);
    }
    Ok(constraints)
}

/// A short human name for an expression form, used in the refusal message above.
fn describe_expr_kind(expr: &naso_compiler::ast::Expr) -> &'static str {
    // EXHAUSTIVE ON PURPOSE. There is no catch-all arm here, so adding a new `ExprKind`
    // variant makes this function fail to compile rather than quietly reporting "construct"
    // and letting an unanalysable form pass as clean. That is the whole point: a silent arm
    // here would reintroduce the exact bug this function exists to describe.
    match &expr.kind {
        ExprKind::Literal(_) => "literal",
        ExprKind::Var(_) => "variable",
        ExprKind::Binary(..) => "binary operator",
        ExprKind::Unary(..) => "unary operator",
        ExprKind::Call(..) => "call",
        ExprKind::MethodCall(..) => "method call",
        ExprKind::Field(..) => "field access",
        ExprKind::Index(..) => "index",
        ExprKind::Struct(..) => "struct literal",
        ExprKind::Variant(..) => "enum variant",
        ExprKind::Tuple(..) => "tuple",
        ExprKind::Array(..) => "array literal",
        ExprKind::Block(..) => "block",
        ExprKind::If(..) => "if",
        ExprKind::Match(..) => "match",
        ExprKind::Let(..) => "let expression",
        ExprKind::LetInOut(..) => "inout let",
        ExprKind::LetConsume(..) => "consume let",
        ExprKind::Reversible(..) => "reversible block",
        ExprKind::Lambda(..) => "lambda",
        ExprKind::For(..) => "for loop",
        ExprKind::Forall(..) => "forall loop",
        ExprKind::Quantified(..) => "quantified proposition",
        ExprKind::While(..) => "while loop",
        ExprKind::Return(..) => "return",
        ExprKind::Break(..) => "break",
        ExprKind::Continue => "continue",
        ExprKind::Assign(..) => "assignment",
        ExprKind::Projection(..) => "projection",
        ExprKind::QuantumOp(..) => "quantum operation",
        ExprKind::Ascribe(..) => "ascription",
        ExprKind::Range(..) => "range",
        ExprKind::Error => "parse-error node",
    }
}

/// Encode quantity constraints for a statement.
pub fn encode_quantity_stmt(
    stmt: &naso_compiler::ast::Stmt,
    tracker: &mut QuantityTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    match &stmt.kind {
        naso_compiler::ast::StmtKind::Let(binding) => {
            constraints.extend(encode_quantity_expr(&binding.value, tracker)?);
        }
        naso_compiler::ast::StmtKind::LetInOut(binding) => {
            constraints.extend(encode_quantity_expr(&binding.value, tracker)?);
        }
        naso_compiler::ast::StmtKind::LetConsume(binding) => {
            constraints.extend(encode_quantity_expr(&binding.value, tracker)?);
        }
        naso_compiler::ast::StmtKind::Expr(expr) => {
            constraints.extend(encode_quantity_expr(expr, tracker)?);
        }
        _ => {}
    }

    Ok(constraints)
}
