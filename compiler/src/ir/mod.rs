//! Polyhedral Intermediate Representation (PIR) for Naso Compiler
//!
//! This module defines the core data structures for polyhedral compilation:
//! - Affine domains (iteration spaces with parameters)
//! - Affine maps (piecewise quasi-affine functions)
//! - Schedule trees (hierarchical loop schedules)
//! - Access relations (memory access patterns)
//! - PIR module container with quantity tracking

pub mod access_relation;
pub mod affine_domain;
pub mod affine_map;
pub mod pir_types;
pub mod pretty_print;
pub mod schedule_tree;
pub mod validate;

#[cfg(test)]
pub mod tests;

// Re-exports for convenience
pub use access_relation::{AccessRelation, AccessRelations, AccessType};
pub use affine_domain::{AffineConstraint, AffineDomain, ConstraintType};
pub use affine_map::{AffineMap, AffineMapPiece, Matrix};
pub use pir_types::{
    BinaryOp, PirExpr, PirModule, PirStatement, QuantityMap, UnaryOp, ValidationError,
};
pub use pretty_print::{format_golden_fixture, pir_to_json, pir_to_string};
pub use schedule_tree::{ScheduleNode, ScheduleTree, ScheduleValidationError, StmtId};
pub use validate::{
    ScheduleValidationReport, validate_accesses_for_dependence, validate_domain, validate_map,
    validate_pir, validate_schedule_detailed,
};
