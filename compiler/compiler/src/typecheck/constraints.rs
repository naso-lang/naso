//! Quantity constraints and solving for the Naso type checker

#![allow(clippy::result_large_err)]

use crate::ast::*;
use crate::typecheck::error::TypeError;
use crate::typecheck::*;
use indexmap::IndexMap;

/// A quantity constraint: q1 <= q2
#[derive(Debug, Clone)]
pub struct QtyConstraint {
    pub lhs: Quantity,
    pub rhs: Quantity,
    pub span: Span,
    pub reason: String,
}

impl QtyConstraint {
    pub fn new(lhs: Quantity, rhs: Quantity, span: Span, reason: impl Into<String>) -> Self {
        Self {
            lhs,
            rhs,
            span,
            reason: reason.into(),
        }
    }
}

/// Set of quantity constraints to be solved
#[derive(Debug, Clone, Default)]
pub struct ConstraintSet {
    pub constraints: Vec<QtyConstraint>,
}

impl ConstraintSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, constraint: QtyConstraint) {
        self.constraints.push(constraint);
    }

    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}

/// Solve all quantity constraints
pub fn solve(
    constraints: &mut ConstraintSet,
    _env: &mut type_env::TypeEnv,
    _meta_vars: &mut IndexMap<MetaVar, Option<Type>>,
) -> Result<(), TypeError> {
    // For now, just check that all constraints are satisfiable
    // A full solver would do more sophisticated constraint solving
    for constraint in &constraints.constraints {
        if !crate::typecheck::unify::qty_subtype(constraint.lhs, constraint.rhs) {
            return Err(TypeError::QuantityMismatch {
                expected: constraint.rhs,
                found: constraint.lhs,
                span: constraint.span,
            });
        }
    }
    Ok(())
}

/// Re-export canonical quantity operations from unify.rs
pub use crate::typecheck::unify::{
    is_erasable, is_linear, is_unrestricted, qty_consume, qty_join, qty_meet, qty_subtype,
};
