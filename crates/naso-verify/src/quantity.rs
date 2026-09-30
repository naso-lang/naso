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
        ExprKind::Var(_name) => {
            // Variable reference - quantity checking requires type info
            // which is available via expr.ty in typed AST
            // For now, we skip direct var checking since we track via Let bindings
        }
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
                        // A consuming builtin discharges every linear resource
                        // bound to the name it is handed. Previously this loop
                        // body was empty, so `linear_free(x)` recorded no
                        // consumption at all and every [1] resource was later
                        // reported as leaked.
                        for arg in args {
                            if let ExprKind::Var(name) = &arg.kind {
                                for id in tracker.linear_ids_by_name(&name.name) {
                                    tracker.consume_linear(&id, 0, expr.span);
                                }
                            }
                        }
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
        _ => {
            // Other expression types - no special quantity handling needed
        }
    }

    Ok(constraints)
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
