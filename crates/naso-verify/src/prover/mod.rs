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
///
/// # REFUSED
///
/// This used to run the `predicate`, finalise the context, discard the result, and return
/// `Ok(Vec::new())` -- an empty diagnostic list, which every caller reads as "no problems
/// found". `naso verify --mode custom` would therefore have reported a clean bill of health
/// on a verification condition that was never checked, and the CLI help advertised exactly
/// that. A stub that reports success is the most dangerous shape a stub can take: it is
/// indistinguishable from a proof.
///
/// It now refuses. The lowering machinery is left in place so an implementation has
/// somewhere to start, but nothing pretends the end-to-end path works.
#[cfg(feature = "z3")]
pub fn prove_custom_vc(
    _program: &Program,
    vc_name: &str,
    _predicate: impl FnOnce(&mut LoweringContext) -> Result<(), VerifyError>,
) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    Err(VerifyError::Config(format!(
        "custom verification condition `{vc_name}` is not implemented: this prover builds a \
         lowering context and then discards it, returning an empty diagnostic list, which a \
         caller cannot distinguish from a clean result. Refusing rather than reporting an \
         unperformed check as passed."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use naso_compiler::parser::parse_program;

    /// The custom-VC prover must REFUSE, not return an empty diagnostic list.
    ///
    /// The old test in this module had the body `// Smoke test` and asserted nothing, so
    /// `prove_custom_vc` returning `Ok(vec![])` for every input was never noticed. This
    /// pins the refusal, because the difference between "no diagnostics" and "not checked"
    /// is invisible to everything downstream.
    #[test]
    fn a_custom_verification_condition_is_refused_rather_than_reported_as_passing() {
        let program = parse_program("fn f(n: int) -> bool { return true; }").expect("parse");
        let result = prove_custom_vc(&program, "my_vc", |_ctx| Ok(()));
        let err = result.expect_err("must not report an empty diagnostic list as success");
        assert!(
            err.to_string().contains("not implemented"),
            "the refusal must say the check was not performed, got: {err}"
        );
    }
}
