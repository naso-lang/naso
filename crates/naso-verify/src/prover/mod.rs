/// Automated provers for quantum uncomputation and linearity.
///
/// This module provides the high-level prover interface that orchestrates
/// the SMT-based verification of quantum uncomputation safety and
/// [1]-quantity leak detection.
#[cfg(feature = "z3")]
pub mod cfg;
#[cfg(feature = "z3")]
pub mod linearity;
pub mod obligations;
#[cfg(feature = "z3")]
pub mod uncomputation;

#[cfg(feature = "z3")]
use crate::error::VerifyError;
#[cfg(feature = "z3")]
use crate::lower::LoweringContext;
#[cfg(feature = "z3")]
use crate::model::VerifyDiagnostic;
#[cfg(feature = "z3")]
use naso_compiler::ast::Program;

// Re-export prover functions from submodules
#[cfg(feature = "z3")]
pub use linearity::prove_linearity;
#[cfg(feature = "z3")]
pub use obligations::prove_obligations;
#[cfg(feature = "z3")]
pub use uncomputation::prove_uncomputation;

/// Main prover entry point: run all provers on an AST.
#[cfg(feature = "z3")]
pub fn run_all_provers(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    // Run uncomputation prover
    diagnostics.extend(uncomputation::prove_uncomputation(program)?);

    // Run linearity prover
    diagnostics.extend(linearity::prove_linearity(program)?);

    // Run the proof-obligation prover: actually check the `assert`s written in
    // `proof { .. }` blocks.
    diagnostics.extend(obligations::prove_obligations(program)?);

    Ok(diagnostics)
}

/// Run only the proof-obligation prover.
#[cfg(feature = "z3")]
pub fn run_obligation_prover(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    obligations::prove_obligations(program)
}

/// Run only the uncomputation prover.
#[cfg(feature = "z3")]
pub fn run_uncomputation_prover(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    uncomputation::prove_uncomputation(program)
}

/// Run only the linearity prover.
#[cfg(feature = "z3")]
pub fn run_linearity_prover(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    linearity::prove_linearity(program)
}

/// Prove a custom verification condition.
#[cfg(feature = "z3")]
pub fn prove_custom_vc(
    _program: &Program,
    _vc_name: &str,
    predicate: impl FnOnce(&mut LoweringContext) -> Result<(), VerifyError>,
) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut ctx = LoweringContext::new();
    predicate(&mut ctx)?;
    ctx.finalize()?;
    // In real implementation, would run solver and extract diagnostics
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prover_module_compiles() {
        // Smoke test
    }
}
