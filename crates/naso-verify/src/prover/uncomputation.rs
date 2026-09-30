//! Quantum Uncomputation Safety Prover.
//!
//! This prover verifies that all temporary qubits allocated via `qalloc`
//! are returned to the |0> state before scope exit. It encodes the
//! quantum circuit as symbolic unitary matrices and proves U_temp |0> = |0>.

#[cfg(feature = "z3")]
use crate::config::SolverConfig;
#[cfg(feature = "z3")]
use crate::error::VerifyError;
#[cfg(feature = "z3")]
use crate::lower::LoweringContext;
#[cfg(feature = "z3")]
use crate::model::VerifyDiagnostic;
#[cfg(feature = "z3")]
use crate::quantum::{GateKind, QuantumTracker};
#[cfg(feature = "z3")]
use crate::solver::verify;
use naso_compiler::ast::{Function, Program};

/// Run the uncomputation prover on all quantum functions in the AST.
#[cfg(feature = "z3")]
pub fn prove_uncomputation(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    for item in &program.items {
        #[allow(clippy::collapsible_if)]
        if let naso_compiler::ast::Item::Function(func) = item {
            if is_quantum_function(func) {
                let func_diagnostics = prove_function_uncomputation(func)?;
                diagnostics.extend(func_diagnostics);
            }
        }
    }

    Ok(diagnostics)
}

/// Check if a function contains quantum operations.
fn is_quantum_function(func: &Function) -> bool {
    contains_quantum_ops(&func.body)
}

/// Check if a block contains quantum operations.
fn contains_quantum_ops(body: &naso_compiler::ast::Block) -> bool {
    #[allow(clippy::collapsible_if)]
    if let Some(expr) = &body.expr {
        if contains_quantum_expr(expr) {
            return true;
        }
    }
    for stmt in &body.stmts {
        if contains_quantum_stmt(stmt) {
            return true;
        }
    }
    false
}

fn contains_quantum_stmt(stmt: &naso_compiler::ast::Stmt) -> bool {
    match &stmt.kind {
        naso_compiler::ast::StmtKind::Expr(expr) => contains_quantum_expr(expr),
        naso_compiler::ast::StmtKind::Let(binding) => contains_quantum_expr(&binding.value),
        naso_compiler::ast::StmtKind::LetInOut(binding) => contains_quantum_expr(&binding.value),
        naso_compiler::ast::StmtKind::LetConsume(binding) => contains_quantum_expr(&binding.value),
        _ => false,
    }
}

fn contains_quantum_expr(expr: &naso_compiler::ast::Expr) -> bool {
    match &expr.kind {
        naso_compiler::ast::ExprKind::Call(func, args) => {
            #[allow(clippy::collapsible_if)]
            if let naso_compiler::ast::ExprKind::Var(name) = &func.kind {
                if is_quantum_gate_name(&name.name) || name.name == "qalloc" || name.name == "qfree"
                {
                    return true;
                }
            }
            args.iter().any(contains_quantum_expr)
        }
        naso_compiler::ast::ExprKind::Let(binding) => contains_quantum_expr(&binding.value),
        naso_compiler::ast::ExprKind::LetInOut(binding) => contains_quantum_expr(&binding.value),
        naso_compiler::ast::ExprKind::LetConsume(binding) => contains_quantum_expr(&binding.value),
        naso_compiler::ast::ExprKind::If(_, then_e, else_e) => {
            contains_quantum_expr(then_e)
                || else_e.as_ref().is_some_and(|e| contains_quantum_expr(e))
        }
        naso_compiler::ast::ExprKind::QuantumOp(_) => true,
        naso_compiler::ast::ExprKind::Block(block) => contains_quantum_ops(block),
        naso_compiler::ast::ExprKind::Binary(_, lhs, rhs) => {
            contains_quantum_expr(lhs) || contains_quantum_expr(rhs)
        }
        naso_compiler::ast::ExprKind::Unary(_, operand) => contains_quantum_expr(operand),
        naso_compiler::ast::ExprKind::MethodCall(receiver, _, args) => {
            contains_quantum_expr(receiver) || args.iter().any(contains_quantum_expr)
        }
        _ => false,
    }
}

fn is_quantum_gate_name(name: &str) -> bool {
    matches!(
        name,
        "H" | "X"
            | "Y"
            | "Z"
            | "S"
            | "T"
            | "CX"
            | "CY"
            | "CZ"
            | "RX"
            | "RY"
            | "RZ"
            | "hadamard"
            | "cnot"
            | "measure"
    )
}

/// Prove uncomputation for a single quantum function.
#[cfg(feature = "z3")]
fn prove_function_uncomputation(func: &Function) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut ctx = LoweringContext::new();
    ctx.current_function = Some(func.name.name.clone());

    ctx.quantum.current_function = Some(func.name.name.clone());

    let mut constraints = Vec::new();
    if let Some(body_expr) = &func.body.expr {
        constraints.extend(crate::quantum::encode_quantum_expr(
            body_expr.as_ref(),
            &mut ctx.quantum,
        )?);
    }

    // Every statement kind has to be encoded, not just expression statements.
    // `let q = qalloc(1);` is a StmtKind::Let, so skipping it meant the qubit
    // was never allocated, no temporary qubits existed, and the prover
    // returned an empty diagnostic vector -- a false negative on exactly the
    // case the prover exists to catch.
    for stmt in &func.body.stmts {
        match &stmt.kind {
            naso_compiler::ast::StmtKind::Expr(expr) => {
                let _ = crate::quantum::encode_quantum_expr(expr, &mut ctx.quantum)?;
            }
            naso_compiler::ast::StmtKind::Let(binding) => {
                // Record the binding name so `qalloc` registers the qubit under
                // the name the gates will use. Only a plain identifier binding
                // (`let q = ...`) can name a qubit; tuple and wildcard patterns
                // are not tracked here.
                ctx.quantum.pending_name = match &binding.pattern.kind {
                    naso_compiler::ast::pattern::PatternKind::Ident(id) => Some(id.name.clone()),
                    _ => None,
                };
                let _ = crate::quantum::encode_quantum_expr(&binding.value, &mut ctx.quantum)?;
                ctx.quantum.pending_name = None;
            }
            _ => {}
        }
    }

    // Qubits bound by the return value are handed to the caller, not
    // discarded, so they are not temporaries that must be uncomputed.
    let mut returned_names = Vec::new();
    if let Some(body_expr) = &func.body.expr {
        collect_var_names(body_expr.as_ref(), &mut returned_names);
    }
    ctx.quantum.mark_returned(&returned_names);

    let temp_qubits: Vec<String> = ctx.quantum.temp_qubit_ids().to_vec();

    if temp_qubits.is_empty() {
        return Ok(Vec::new());
    }

    let config = SolverConfig::thorough();
    let mut diagnostics = Vec::new();

    // One query per qubit. A single shared script would make every qubit look
    // guilty as soon as any one of them failed.
    for id in temp_qubits {
        let Some(qubit) = ctx.quantum.get_qubit(&id).cloned() else {
            continue;
        };

        let mut script_ctx = LoweringContext::new();
        script_ctx.current_function = Some(func.name.name.clone());
        script_ctx.quantum.current_function = Some(func.name.name.clone());

        ctx.quantum.declare_qubit(&mut script_ctx.script, &qubit);
        // Fact: the state variable reflects the qubit's operation list.
        script_ctx
            .script
            .assert(ctx.quantum.gate_semantics_for(&qubit));
        // Requirement: a temporary must end in |0>.
        script_ctx
            .script
            .assert(ctx.quantum.uncomputation_requirement(&qubit));

        let smt_script = script_ctx.finalize()?.to_string();
        let result = verify(&smt_script, config.clone())?;

        if let Some(d) = extract_uncomputation_diagnostic(&result, &func.name.name, &qubit) {
            diagnostics.push(d);
        }
    }

    Ok(diagnostics)
}

/// Collect every plain variable name appearing in an expression.
///
/// Used to spot qubits named in the function's tail expression, i.e. the ones
/// it returns.
#[cfg(feature = "z3")]
fn collect_var_names(expr: &naso_compiler::ast::Expr, out: &mut Vec<String>) {
    match &expr.kind {
        naso_compiler::ast::ExprKind::Var(name) => out.push(name.name.clone()),
        naso_compiler::ast::ExprKind::Call(_, args) => {
            for arg in args {
                collect_var_names(arg, out);
            }
        }
        naso_compiler::ast::ExprKind::Binary(_, lhs, rhs) => {
            collect_var_names(lhs, out);
            collect_var_names(rhs, out);
        }
        naso_compiler::ast::ExprKind::Unary(_, operand) => collect_var_names(operand, out),
        naso_compiler::ast::ExprKind::Tuple(items) => {
            for item in items {
                collect_var_names(item, out);
            }
        }
        naso_compiler::ast::ExprKind::Block(block) => {
            if let Some(e) = &block.expr {
                collect_var_names(e.as_ref(), out);
            }
            for stmt in &block.stmts {
                if let naso_compiler::ast::StmtKind::Expr(e) = &stmt.kind {
                    collect_var_names(e, out);
                }
            }
        }
        _ => {}
    }
}

/// Extract diagnostic from verification result.
///
/// The script asserts the requirement `state == |0>`, so it is SAT exactly
/// when the qubit CAN be uncomputed. That makes SAT the success case. The
/// previous mapping had this backwards: it reported a diagnostic on Sat and
/// stayed silent on Unsat, so any qubit that genuinely could not be returned
/// to |0> was never reported.
#[cfg(feature = "z3")]
fn extract_uncomputation_diagnostic(
    result: &crate::solver::VerifyResult,
    func_name: &str,
    qubit: &crate::quantum::SymbolicQubit,
) -> Option<VerifyDiagnostic> {
    match result {
        // Requirement satisfiable: the qubit provably returns to |0>.
        crate::solver::VerifyResult::Sat(_) => None,
        // Requirement refuted: no reachable state has this qubit in |0>.
        crate::solver::VerifyResult::Unsat(_) => Some(VerifyDiagnostic {
            code: "NASO-UNC-001".to_string(),
            message: format!(
                "Quantum uncomputation failed in '{}': temporary qubit '{}' is not provably returned to |0> before scope exit",
                func_name, qubit.name
            ),
            span: qubit.span,
            severity: crate::model::DiagnosticSeverity::Error,
            related: vec![],
            fix: Some(crate::model::CodeFix {
                title: "Add explicit uncomputation before scope exit".to_string(),
                edits: vec![],
            }),
        }),
        crate::solver::VerifyResult::Unknown(reason) => Some(VerifyDiagnostic {
            code: "NASO-UNC-002".to_string(),
            message: format!(
                "Could not verify uncomputation for '{}': {}",
                func_name, reason
            ),
            span: qubit.span,
            severity: crate::model::DiagnosticSeverity::Warning,
            related: vec![],
            fix: None,
        }),
        crate::solver::VerifyResult::Error(msg) => Some(VerifyDiagnostic {
            code: "NASO-UNC-ERR".to_string(),
            message: format!("Verification error for '{}': {}", func_name, msg),
            span: qubit.span,
            severity: crate::model::DiagnosticSeverity::Error,
            related: vec![],
            fix: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naso_compiler::parser::parse_program;

    #[test]
    fn test_bell_pair_qubits_escape() {
        // Returned qubits must not be treated as temporaries: returning a Bell
        // pair in superposition is correct, not an uncomputation failure.
        let src = r#"
        fn bell_pair() -> [1] Qubit {
            let q0 = qalloc(1);
            let q1 = qalloc(1);
            hadamard(q0);
            cnot(q0, q1);
            (q0, q1)
        }
        "#;
        let program = parse_program(src).expect("parse failed");
        let diags = prove_uncomputation(&program).expect("prover failed");
        assert!(diags.is_empty(), "returned qubits must escape: {diags:?}");
    }

    #[test]
    fn test_self_inverse_gates_uncompute() {
        // H;H and X;X are the identity, so the qubit ends in |0>.
        for (label, gate) in [("hadamard", "hadamard"), ("X", "X")] {
            let src = format!("fn f() {{ let q = qalloc(1); {gate}(q); {gate}(q); }}");
            let program = parse_program(&src).expect("parse failed");
            let diags = prove_uncomputation(&program).expect("prover failed");
            assert!(
                diags.is_empty(),
                "{label};{label} must uncompute: {diags:?}"
            );
        }
    }

    #[test]
    fn test_phase_gate_preserves_zero() {
        // Z is diagonal: it fixes |0> up to a global phase.
        let program = parse_program("fn f() { let q = qalloc(1); Z(q); }").expect("parse failed");
        let diags = prove_uncomputation(&program).expect("prover failed");
        assert!(diags.is_empty(), "Z must preserve |0>: {diags:?}");
    }

    #[test]
    fn test_uncomputation_is_per_qubit() {
        // Only q1 is left entangled. The prover must not blame q0.
        let src = r#"
        fn f() {
            let q0 = qalloc(1);
            let q1 = qalloc(1);
            hadamard(q0);
            hadamard(q0);
            hadamard(q1);
        }
        "#;
        let program = parse_program(src).expect("parse failed");
        let diags = prove_uncomputation(&program).expect("prover failed");
        assert_eq!(diags.len(), 1, "expected exactly one diagnostic: {diags:?}");
        assert_eq!(diags[0].code, "NASO-UNC-001");
        assert!(
            diags[0].message.contains("'q1'"),
            "must name the offending qubit: {}",
            diags[0].message
        );
    }
}
