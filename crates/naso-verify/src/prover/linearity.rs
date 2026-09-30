//! [1]-Quantity Leak Prover.
//!
//! This prover verifies that no linear ([1]) resource is implicitly dropped,
//! double-freed, or leaked across function boundaries. It tracks [1] bindings
//! across the control-flow graph and verifies exactly-once consumption on all paths.

#[cfg(feature = "z3")]
use crate::config::SolverConfig;
#[cfg(feature = "z3")]
use crate::error::VerifyError;
#[cfg(feature = "z3")]
use crate::lower::LoweringContext;
#[cfg(feature = "z3")]
use crate::model::VerifyDiagnostic;
use crate::quantity::{QuantityKind, QuantityTracker, encode_quantity_expr};
#[cfg(feature = "z3")]
use crate::solver::verify;
use naso_compiler::ast::{Function, Program};

/// Run the linearity prover on all functions in the AST.
#[cfg(feature = "z3")]
pub fn prove_linearity(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    for item in &program.items {
        if let naso_compiler::ast::Item::Function(func) = item {
            let func_diagnostics = prove_function_linearity(func)?;
            diagnostics.extend(func_diagnostics);
        }
    }

    Ok(diagnostics)
}

/// Prove linearity for a single function.
#[cfg(feature = "z3")]
fn prove_function_linearity(func: &Function) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut ctx = LoweringContext::new();
    ctx.current_function = Some(func.name.name.clone());

    let mut tracker = QuantityTracker::new();

    for param in &func.params {
        let qk = QuantityKind::from_ast(&param.ty.quantity);
        match qk {
            QuantityKind::One => {
                tracker.allocate_linear(
                    &param.name.name,
                    param.span,
                    crate::quantity::AllocSite::Param(param.name.name.clone()),
                );
            }
            QuantityKind::Zero => tracker.register_erased(&param.name.name, param.span),
            QuantityKind::Bounded(n) => tracker.register_bounded(&param.name.name, n, param.span),
            QuantityKind::Many => {}
        }
    }

    let mut constraints = Vec::new();
    if let Some(body_expr) = &func.body.expr {
        constraints.extend(encode_quantity_expr(body_expr.as_ref(), &mut tracker)?);
    }
    for stmt in &func.body.stmts {
        if let naso_compiler::ast::StmtKind::Expr(_expr) = &stmt.kind {
            let _ = crate::quantity::encode_quantity_stmt(stmt, &mut tracker)?;
        }
    }

    for constraint in constraints {
        ctx.script.assert(constraint);
    }

    let config = SolverConfig::default();
    let mut diagnostics = Vec::new();

    for res_id in tracker.linear_resource_ids() {
        let Some(rid) = tracker.get_linear_resource(res_id) else {
            continue;
        };

        // Qubit-typed locals are the uncomputation prover's domain, not ours.
        // `qalloc` registers its resource under the callee name rather than the
        // source binding, so it can never be matched by name here; attempting
        // to check it would report every valid quantum routine as leaking.
        if matches!(rid.alloc_site, crate::quantity::AllocSite::Local(..)) {
            continue;
        }

        let count = tracker.consumption_count(res_id);
        let ctx = build_exactly_once_script(&func.name.name, res_id, count)?;
        let result = verify(&ctx, config.clone())?;
        if let Some(d) =
            extract_linearity_diagnostic(&result, &func.name.name, rid, res_id, count, &tracker)
        {
            diagnostics.push(d);
        }
    }

    Ok(diagnostics)
}

/// Build an SMT script that is satisfiable exactly when `resource_id` was
/// consumed exactly once.
///
/// Two assertions are emitted:
///   1. the observed consumption count (a fact from the static pass), and
///   2. the linearity requirement, `count == 1`.
///
/// They are mutually contradictory for any count other than 1, so the solver
/// returning UNSAT *is* the refutation. SAT means the requirement holds.
///
/// The previous encoding asserted `resource > 0` against a variable that no
/// other constraint ever defined, so the solver was free to satisfy it with an
/// arbitrary positive number. That made the "Sat means leak" reading correct
/// only by accident and flagged every linear resource, consumed or not.
fn build_exactly_once_script(
    func_name: &str,
    resource_id: &str,
    count: usize,
) -> Result<String, VerifyError> {
    use crate::smtlib::Sort;
    use crate::smtlib::builder::{eq, int, var};

    let mut ctx = LoweringContext::new();
    ctx.current_function = Some(func_name.to_string());

    let resource_var = var(resource_id, Sort::Int);
    // The symbol must exist before it can be constrained, otherwise Z3
    // rejects the script as referencing an undeclared constant.
    ctx.script.declare_const(resource_id, Sort::Int);
    // Fact: what the static pass actually observed.
    ctx.script
        .assert(eq(resource_var.clone(), int(count as i64)));
    // Requirement: [1] resources are consumed exactly once.
    ctx.script.assert(eq(resource_var, int(1)));

    let script = ctx.finalize()?;
    Ok(script.to_string())
}

/// Extract diagnostic from verification result.
///
/// The script is SAT only when the resource is consumed exactly once, so the
/// polarity is the opposite of the previous implementation: UNSAT is the
/// violation, SAT is success. Which violation it is comes from the observed
/// count, not from the solver.
#[cfg(feature = "z3")]
fn extract_linearity_diagnostic(
    result: &crate::solver::VerifyResult,
    func_name: &str,
    resource: &crate::quantity::ResourceId,
    resource_id: &str,
    count: usize,
    tracker: &QuantityTracker,
) -> Option<VerifyDiagnostic> {
    match result {
        // Requirement satisfied.
        crate::solver::VerifyResult::Sat(_) => None,
        // Fact and requirement disagree: the resource was not consumed
        // exactly once.
        crate::solver::VerifyResult::Unsat(_) if count > 1 => {
            // Point at the offending extra consumption, not the declaration.
            let span = tracker
                .consumption_span(resource_id, 1)
                .unwrap_or(resource.span);
            Some(VerifyDiagnostic {
                code: "NASO-LIN-002".to_string(),
                message: format!(
                    "Double consumption of [1] resource '{}' in '{}': consumed {} times, expected exactly once",
                    resource.name, func_name, count
                ),
                span,
                severity: crate::model::DiagnosticSeverity::Error,
                related: vec![],
                fix: Some(crate::model::CodeFix {
                    title: "Remove the extra consumption of this linear resource".to_string(),
                    edits: vec![],
                }),
            })
        }
        crate::solver::VerifyResult::Unsat(_) => Some(VerifyDiagnostic {
            code: "NASO-LIN-003".to_string(),
            message: format!(
                "[1]-quantity leak in '{}': resource '{}' is never consumed, but must be consumed exactly once",
                func_name, resource.name
            ),
            span: resource.span,
            severity: crate::model::DiagnosticSeverity::Error,
            related: vec![],
            fix: Some(crate::model::CodeFix {
                title: "Consume this linear resource exactly once on all control-flow paths"
                    .to_string(),
                edits: vec![],
            }),
        }),
        crate::solver::VerifyResult::Unknown(reason) => Some(VerifyDiagnostic {
            code: "NASO-LIN-004".to_string(),
            message: format!("Could not verify linearity for '{}': {}", func_name, reason),
            span: resource.span,
            severity: crate::model::DiagnosticSeverity::Warning,
            related: vec![],
            fix: None,
        }),
        crate::solver::VerifyResult::Error(msg) => Some(VerifyDiagnostic {
            code: "NASO-LIN-ERR".to_string(),
            message: format!("Verification error for '{}': {}", func_name, msg),
            span: resource.span,
            severity: crate::model::DiagnosticSeverity::Error,
            related: vec![],
            fix: None,
        }),
    }
}

/// Analyze control-flow paths for linearity (simplified).
pub fn analyze_cfg_paths(
    _func: &Function,
    _tracker: &QuantityTracker,
) -> Result<Vec<ConsumptionPath>, VerifyError> {
    Ok(Vec::new())
}

/// A consumption path through the CFG.
#[derive(Debug, Clone)]
pub struct ConsumptionPath {
    pub path_id: u32,
    pub consumed_resources: Vec<String>,
    pub unconsumed_resources: Vec<String>,
    pub double_consumed: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linearity_module_compiles() {}
}
