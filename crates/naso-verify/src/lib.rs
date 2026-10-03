//! naso-verify: SMT-based formal verification engine for the Naso programming language.
//!
//! This crate provides:
//! - AST to SMT-LIB2 lowering for QTT quantity constraints, linear resources, MVS, quantum uncomputation, and polyhedral invariants
//! - Z3 solver FFI bridge with incremental solving, model extraction, and unsat core support
//! - Automated provers for quantum uncomputation safety and [1]-quantity leak detection
//! - CLI integration via `naso verify` command

#![allow(unused_imports)]

extern crate serde;

pub mod cache;
#[cfg(feature = "z3")]
pub mod cli;
pub mod config;
pub mod error;
#[cfg(feature = "z3")]
pub mod lower;
pub mod model;
#[cfg(feature = "z3")]
pub mod mvs;
#[cfg(feature = "z3")]
pub mod output;
#[cfg(feature = "z3")]
pub mod polyhedral;
#[cfg(feature = "z3")]
pub mod prover;
pub mod quantity;
pub mod quantum;
pub mod smtlib;
#[cfg(feature = "z3")]
pub mod solver;

#[cfg(feature = "z3")]
use crate::config::SolverConfig;
#[cfg(feature = "z3")]
use crate::error::VerifyError;
#[cfg(feature = "z3")]
use crate::solver::VerifyResult;
#[cfg(feature = "z3")]
use naso_compiler::ast::Program;

/// Main verification entry point: lower AST to SMT-LIB2 and solve.
#[cfg(feature = "z3")]
pub fn verify(program: &Program, config: SolverConfig) -> Result<VerifyResult, VerifyError> {
    let smt_script = lower::lower_to_smtlib(program)?;
    solver::verify(&smt_script, config)
}

/// Verify with default configuration (QF_UFLIA logic, 30s timeout, models enabled).
#[cfg(feature = "z3")]
pub fn verify_default(program: &Program) -> Result<VerifyResult, VerifyError> {
    verify(program, SolverConfig::default())
}

/// Re-export CLI types for compiler integration
#[cfg(feature = "z3")]
pub use cli::{VerifyCliConfig, VerifyMode, parse_verify_args};

// The gate matrices, the adjoint relation, and the CPU state-vector simulator live in
// `naso-gates`, a leaf crate with no dependencies.
//
// They were moved there rather than reimplemented because the dependency arrow pointed the
// wrong way: this crate depends on `naso-compiler`, so the compiler could not reach a gate
// table that lived here. A shared leaf breaks the cycle without inverting it, and it keeps a
// single definition -- a table duplicated into two crates would drift, and drift in an
// adjoint table computes wrong inverses instead of failing.
pub use naso_gates::{gate_inverse, statevector};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_module_compiles() {
        // Smoke test: ensure the crate compiles and public API is accessible
        #[cfg(feature = "z3")]
        {
            let _ = SolverConfig::default();
        }
    }
}
