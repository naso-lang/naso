//! Proof-Obligation Prover.
//!
//! Reads `proof { .. }` blocks and checks whether each `assert` holds.
//!
//! This is the prover that makes `proof { .. }` mean something. Without it a
//! proof block is only an annotation: the typechecker accepts the obligation
//! and nothing ever determines whether it is true.
//!
//! ## What this prover deliberately does not do
//!
//! It does not report obligations as proved unless Z3 actually proved them.
//! Every obligation is checked by asserting its *negation* and asking Z3 for
//! UNSAT; a countermodel means the obligation is FALSE and is reported as an
//! error.
//!
//! Floating-point obligations are not discharged, because the SMT layer has no
//! floating-point sort -- encoding `f32` arithmetic as Real would silently
//! change the meaning (`Real` is unbounded and exact; `f32` is neither), and
//! a prover that "proves" a float obligation by proving a different, weaker
//! claim is worse than one that admits it cannot. Those are reported as
//! `OBL-UNSUPPORTED` at Warning severity: not proved, not disproved, and
//! visibly so. An obligation that is silently skipped is exactly the kind of
//! false negative this prover exists to avoid.

use crate::error::VerifyError;
use crate::model::{DiagnosticSeverity, RelatedInfo, VerifyDiagnostic};
use crate::smtlib::{Script, Sort, Term};
use naso_compiler::ast::{BinOp, Expr, ExprKind, Function, Item, Literal, Program, StmtKind, UnOp};
use std::collections::HashMap;

/// Diagnostic code for an obligation Z3 refuted (assertion is FALSE).
pub const OBL_FALSE: &str = "NASO-OBL-001";
/// Diagnostic code for an obligation stated over an unsupported sort.
pub const OBL_UNSUPPORTED: &str = "NASO-OBL-002";

/// Run the obligation prover over all functions in the AST.
pub fn prove_obligations(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    for item in &program.items {
        if let Item::Function(func) = item {
            diagnostics.extend(prove_function_obligations(func)?);
        }
    }

    Ok(diagnostics)
}

/// Prove every obligation stated in one function's proof blocks.
fn prove_function_obligations(func: &Function) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    // Parameter names become uninterpreted SMT constants. An obligation about
    // a parameter is a statement about *any* value of it, so it must be
    // checked for all valuations -- which is exactly what leaving the constant
    // unconstrained does.
    // (source name, SMT constant name, sort). Keyed by the source name because
    // that is how the body refers to it; the SMT constant carries a function
    // prefix so obligations from different functions cannot collide.
    let mut params = Vec::new();
    for param in &func.params {
        if let Some(sort) = sort_for_tensor_or_scalar(&param.ty.kind) {
            params.push((
                param.name.name.clone(),
                format!("{}.{}", func.name.name, param.name.name),
                sort,
            ));
        }
    }
    // Only the prefixed names are declared as SMT constants.
    let declarations: Vec<(String, Sort)> = params
        .iter()
        .map(|(_, smt, sort)| (smt.clone(), sort.clone()))
        .collect();

    for stmt in &func.body.stmts {
        if let StmtKind::Proof(block) = &stmt.kind {
            for inner in &block.body.stmts {
                let obligation = match &inner.kind {
                    StmtKind::Expr(expr) => match &expr.kind {
                        ExprKind::Call(callee, args) => {
                            if is_assert(callee) && args.len() == 1 {
                                Some((&args[0], expr.span))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((pred, span)) = obligation {
                    diagnostics.extend(prove_obligation(
                        &func.name.name,
                        pred,
                        span,
                        &params,
                        &declarations,
                    )?);
                }
            }
        }
    }

    Ok(diagnostics)
}

fn is_assert(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Var(ident) => ident.name == "assert",
        _ => false,
    }
}

/// Check one obligation.
///
/// Proves the assertion by asserting its negation and requiring UNSAT.
fn prove_obligation(
    func_name: &str,
    pred: &Expr,
    span: naso_compiler::ast::Span,
    params: &[(String, String, Sort)],
    declarations: &[(String, Sort)],
) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    // Source name -> SMT term. A parameter is an uninterpreted constant, left
    // unconstrained so the obligation is checked for *every* valuation, which
    // is what "an obligation about a parameter" means.
    let mut scope: HashMap<String, Term> = params
        .iter()
        .map(|(src, smt, sort)| (src.clone(), Term::Var(smt.clone(), sort.clone())))
        .collect();

    match encode_predicate(pred, &mut scope) {
        // Unsupported sort: report as unproved, visibly. Never as proved.
        Err(EncodeErr::Unsupported { reason, why }) => {
            diagnostics.push(VerifyDiagnostic {
                code: OBL_UNSUPPORTED.to_string(),
                message: format!(
                    "obligation in `{func_name}` not discharged: {reason} -- \
                     {why}. The assertion has NOT been proved; it is neither \
                     confirmed nor refuted."
                ),
                span,
                severity: DiagnosticSeverity::Warning,
                related: vec![RelatedInfo {
                    span,
                    message: reason,
                }],
                fix: None,
            });
        }
        Err(EncodeErr::Malformed(msg)) => {
            diagnostics.push(VerifyDiagnostic {
                code: OBL_UNSUPPORTED.to_string(),
                message: format!("obligation in `{func_name}` not discharged: {msg}"),
                span,
                severity: DiagnosticSeverity::Warning,
                related: Vec::new(),
                fix: None,
            });
        }
        Ok(body) => {
            let mut script = Script::new();
            // Must NOT be a QF_ logic. A quantified obligation emitted under
            // (set-logic QF_UFLIA) is silently discarded by Z3 -- declaring a
            // quantifier-free logic and then using a binder is inconsistent,
            // and the solver answers "satisfiable" for a script whose
            // assertion it never applied. That is a false negative of the
            // worst kind: every quantified obligation reported as refuted.
            // Verified directly against Z3: LIA, UFLIA and no-logic all give
            // the correct answer; only QF_UFLIA was wrong.
            script.set_logic("UFLIA");
            for (name, sort) in declarations {
                script.declare_const(name, sort.clone());
            }
            // A quantified predicate becomes a universally quantified term, so
            // the negation is "exists a counterexample".
            let negated = match &body {
                Encoded::Forall(bindings, term) => {
                    // !(forall ...) == (exists ...) -- the standard dual.
                    Term::Exists(bindings.clone(), Box::new(negate(term)))
                }
                Encoded::Bool(term) => negate(term),
            };
            script.assert(negated);
            script.check_sat();
            script.exit();
            let smt_text = script.to_string();

            match crate::solver::verify(&smt_text, Default::default())? {
                crate::solver::VerifyResult::Unsat(_) => {
                    // No counterexample exists: the obligation holds.
                }
                crate::solver::VerifyResult::Sat(_) => {
                    if std::env::var("NASO_DUMP_SMT").is_ok() {
                        eprintln!("--- SMT ---\n{smt_text}\n-----------");
                    }
                    diagnostics.push(VerifyDiagnostic {
                        code: OBL_FALSE.to_string(),
                        message: format!(
                            "obligation in `{func_name}` does not hold: Z3 found a \
                             counterexample"
                        ),
                        span,
                        severity: DiagnosticSeverity::Error,
                        related: Vec::new(),
                        fix: None,
                    });
                }
                other => {
                    diagnostics.push(VerifyDiagnostic {
                        code: OBL_UNSUPPORTED.to_string(),
                        message: format!(
                            "obligation in `{func_name}` not discharged: solver \
                             returned {other:?}"
                        ),
                        span,
                        severity: DiagnosticSeverity::Warning,
                        related: Vec::new(),
                        fix: None,
                    });
                }
            }
        }
    }

    Ok(diagnostics)
}

/// Conjunction of a non-empty list of terms.
fn conjoin(terms: Vec<Term>) -> Term {
    let mut iter = terms.into_iter();
    let first = iter.next().expect("called with a non-empty list");
    iter.fold(first, |acc, t| Term::App("and".to_string(), vec![acc, t]))
}

/// Encoded predicate: either a plain term, or quantified with its bindings.
enum Encoded {
    Bool(Term),
    Forall(Vec<(String, Sort)>, Term),
}

fn negate(term: &Term) -> Term {
    match term {
        // Double negation cancels; anything else gets an explicit `not`.
        Term::App(op, args) if op == "not" && args.len() == 1 => args[0].clone(),
        other => Term::App("not".to_string(), vec![other.clone()]),
    }
}

/// Why an obligation could not be encoded.
enum EncodeErr {
    /// Refuses rather than change the meaning of the claim.
    ///
    /// Carries both what was rejected and why, so the diagnostic cannot read
    /// as a generic failure -- especially important for the float case, where
    /// the reason is that encoding f32 as Real would silently prove a weaker
    /// and different statement.
    Unsupported { reason: String, why: String },
    /// Not a proposition this prover understands.
    Malformed(String),
}

impl EncodeErr {
    /// Rejected a float: the SMT layer has no floating-point sort.
    fn float(reason: String) -> Self {
        EncodeErr::Unsupported {
            reason,
            why: "no floating-point sort exists in the SMT layer, and encoding \
                  f32 as Real would change the claim"
                .to_string(),
        }
    }
}

fn sort_for_tensor_or_scalar(base: &naso_compiler::ast::TypeKind) -> Option<Sort> {
    use naso_compiler::ast::TypeKind;
    Some(match base {
        TypeKind::Int | TypeKind::Nat => Sort::Int,
        TypeKind::UInt => Sort::Int,
        TypeKind::Bool => Sort::Bool,
        // Tensors are uninterpreted here. Indexing one yields an element of
        // the element sort, which we do not model yet -- see encode_index.
        _ => return None,
    })
}

/// Encode a proposition to SMT.
fn encode_predicate(pred: &Expr, scope: &mut HashMap<String, Term>) -> Result<Encoded, EncodeErr> {
    match &pred.kind {
        ExprKind::Quantified(quant) => {
            let mut bindings = Vec::new();
            for (var, _, _) in &quant.bindings {
                bindings.push((var.name.clone(), Sort::Int));
            }

            // The body block's tail is the predicate.
            let tail = quant.body.expr.as_deref().ok_or_else(|| {
                EncodeErr::Malformed("quantified obligation has no predicate".to_string())
            })?;

            // The iteration domain is part of the claim and must be encoded.
            // Without it the bound variable is unconstrained, so Z3 can pick a
            // value outside `0..N` and refute an obligation that genuinely
            // holds -- a false negative that would make the prover
            // untrustworthy. Each binding contributes `lower <= var <= upper`,
            // conjoined under the same quantifier.
            //
            // The upper bound is EXCLUSIVE, matching Rust range semantics: the
            // kernel loops `0..1024` over `Tensor[f32, 1024]` and indexes
            // `input[i]`, which only visits 0..=1023 under an exclusive upper
            // bound. Encoding `i <= upper` instead would silently widen every
            // domain by one element -- and would refute obligations that hold
            // at the real bound.
            let mut domain_terms = Vec::new();
            for (var, lower, upper) in &quant.bindings {
                let lo = encode_expr(lower, scope)?;
                let hi = encode_expr(upper, scope)?;
                let var_term = Term::Var(var.name.clone(), Sort::Int);
                // lower <= var
                domain_terms.push(Term::App("<=".to_string(), vec![lo, var_term.clone()]));
                // var < upper
                domain_terms.push(Term::App("<".to_string(), vec![var_term, hi]));
            }

            // The bound variables are in scope in the predicate. They are
            // introduced as Term binders by Encoded::Forall, so resolve them
            // to bare vars here rather than declaring constants.
            let mut inner = scope.clone();
            for (name, sort) in &bindings {
                inner.insert(name.clone(), Term::Var(name.clone(), sort.clone()));
            }

            match encode_predicate(tail, &mut inner)? {
                Encoded::Bool(term) => {
                    // Implication, NOT conjunction.
                    //
                    // The claim is `forall i. domain(i) => pred(i)`. Encoding
                    // the body as `domain(i) AND pred(i)` instead makes the
                    // negation `exists i. NOT(domain AND pred)`, which is
                    // satisfiable by any value OUTSIDE the domain -- so Z3
                    // refutes every quantified obligation. The negation of an
                    // implication is a conjunction, which is why the domain
                    // must sit on the left of `=>` here.
                    let guarded = if domain_terms.is_empty() {
                        term
                    } else {
                        Term::App("=>".to_string(), vec![conjoin(domain_terms), term])
                    };
                    Ok(Encoded::Forall(bindings, guarded))
                }
                // A nested quantifier in the tail: reject rather than guess.
                Encoded::Forall(_, _) => Err(EncodeErr::Malformed(
                    "nested quantifier in obligation".to_string(),
                )),
            }
        }
        _ => Ok(Encoded::Bool(encode_expr(pred, scope)?)),
    }
}

/// Encode an expression to an SMT term.
fn encode_expr(expr: &Expr, scope: &HashMap<String, Term>) -> Result<Term, EncodeErr> {
    match &expr.kind {
        ExprKind::Literal(lit) => encode_literal(lit),
        ExprKind::Var(ident) => {
            // A parameter, or a variable bound by an enclosing quantifier.
            match scope.get(&ident.name) {
                Some(term) => Ok(term.clone()),
                None => Err(EncodeErr::Unsupported {
                    reason: format!("unbound variable `{}`", ident.name),
                    why: "only function parameters and quantifier bindings are \
                          modelled in an obligation"
                        .to_string(),
                }),
            }
        }
        ExprKind::Binary(op, lhs, rhs) => {
            let l = encode_expr(lhs, scope)?;
            let r = encode_expr(rhs, scope)?;
            let sym = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Eq => "=",
                BinOp::Ne => "distinct",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::Div => "div",
                other => {
                    return Err(EncodeErr::Unsupported {
                        reason: format!("operator `{other:?}`"),
                        why: "this operator has no faithful SMT encoding here".to_string(),
                    });
                }
            };
            // `distinct` is variadic and unary in the sense of taking a list.
            if sym == "distinct" {
                Ok(Term::App("distinct".to_string(), vec![l, r]))
            } else {
                Ok(Term::App(sym.to_string(), vec![l, r]))
            }
        }
        ExprKind::Unary(op, operand) => {
            let inner = encode_expr(operand, scope)?;
            match op {
                UnOp::Neg => Ok(Term::App("-".to_string(), vec![inner])),
                UnOp::Not => Ok(Term::App("not".to_string(), vec![inner])),
                other => Err(EncodeErr::Unsupported {
                    reason: format!("unary operator `{other:?}`"),
                    why: "this operator has no faithful SMT encoding here".to_string(),
                }),
            }
        }
        _ => Err(EncodeErr::Unsupported {
            reason: format!("expression form `{}`", describe(&expr.kind)),
            why: "this expression form is not yet lowered to SMT".to_string(),
        }),
    }
}

fn encode_literal(lit: &Literal) -> Result<Term, EncodeErr> {
    match lit {
        Literal::Int(i) => Ok(Term::Const(crate::smtlib::Constant::Int(*i))),
        Literal::UInt(i) => Ok(Term::Const(crate::smtlib::Constant::Int(*i as i64))),
        Literal::Bool(b) => Ok(Term::Const(crate::smtlib::Constant::Bool(*b))),
        // Refusing here is the whole point of the module docs: proving a Real
        // claim about f32 would be proving a different statement.
        // The refusal that gives this prover its value: encoding f32 as Real
        // would prove a *different* statement.
        Literal::Float(f) => Err(EncodeErr::float(format!(
            "floating-point literal `{f}` (f32/f64)"
        ))),
        other => Err(EncodeErr::Unsupported {
            reason: format!("literal `{other:?}`"),
            why: "this literal has no SMT encoding".to_string(),
        }),
    }
}

fn describe(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::Call(_, _) => "call",
        ExprKind::Index(_, _) => "tensor index",
        ExprKind::Block(_) => "block",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse a source snippet and run the obligation prover.
    fn obligations_for(src: &str) -> Vec<VerifyDiagnostic> {
        use naso_compiler::parser::parse_program;
        let program = parse_program(src).expect("parse");
        prove_obligations(&program).expect("prove")
    }

    /// A true obligation over Int must produce no diagnostics: proved by Z3.
    #[test]
    fn test_true_integer_obligation_is_proved() {
        let diags =
            obligations_for("fn f(n: int) -> bool { proof { assert(n + 0 == n); } return true; }");
        assert!(diags.is_empty(), "expected proof, got {diags:?}");
    }

    /// A false obligation must be reported as an error. This is the test that
    /// matters most: a prover that cannot refute is worse than no prover.
    #[test]
    fn test_false_integer_obligation_is_refuted() {
        let diags =
            obligations_for("fn f(n: int) -> bool { proof { assert(n == 5); } return true; }");
        assert_eq!(diags.len(), 1, "expected one diagnostic, got {diags:?}");
        assert_eq!(diags[0].code, OBL_FALSE);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
    }

    /// A quantified obligation over Int is proved by the same route.
    #[test]
    fn test_quantified_obligation_is_proved() {
        let diags = obligations_for(
            "fn f(n: int) -> bool { proof { assert(forall i in 0..10 { i <= 9 }); } return true; }",
        );
        assert!(diags.is_empty(), "expected proof, got {diags:?}");
    }

    /// Regression: the quantifier's iteration domain is part of the claim.
    ///
    /// `forall i in 0..10 { i <= 9 }` is TRUE, because `0..10` is exclusive,
    /// so `i` ranges over 0..=9. Two ways to get this wrong, both of which
    /// would refute a true obligation: omitting the domain entirely (`i`
    /// unconstrained, so Z3 picks 10), or making the upper bound inclusive
    /// (which admits i = 10). A false negative here would make the whole prover
    /// untrustworthy, so both are pinned explicitly.
    #[test]
    fn test_quantifier_domain_is_part_of_the_claim() {
        let diags = obligations_for(
            "fn f() -> bool { proof { assert(forall i in 0..10 { i <= 9 }); } return true; }",
        );
        assert!(diags.is_empty(), "domain not encoded, got {diags:?}");
    }

    /// And the domain genuinely constrains: an obligation that is false only
    /// *within* the domain is still refuted.
    #[test]
    fn test_quantified_obligation_false_within_domain_is_refuted() {
        let diags = obligations_for(
            "fn f() -> bool { proof { assert(forall i in 0..10 { i <= 5 }); } return true; }",
        );
        assert_eq!(diags.len(), 1, "expected refutation, got {diags:?}");
        assert_eq!(diags[0].code, OBL_FALSE);
    }

    /// The kernel's shape: a float precondition is NOT reported as proved.
    /// It must be visibly unsupported rather than silently accepted.
    #[test]
    fn test_float_obligation_reported_unsupported_not_proved() {
        let diags =
            obligations_for("fn f(x: f32) -> bool { proof { assert(x <= 127.0); } return true; }");
        assert_eq!(diags.len(), 1, "expected one diagnostic, got {diags:?}");
        assert_eq!(diags[0].code, OBL_UNSUPPORTED);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
        // The message must not claim the assertion holds.
        assert!(
            !diags[0].message.to_lowercase().contains("proved that"),
            "message must not claim a proof: {}",
            diags[0].message
        );
    }

    /// Tensor indexing is not modelled, so such an obligation is reported
    /// unsupported rather than assumed.
    #[test]
    fn test_tensor_index_obligation_reported_unsupported() {
        let diags = obligations_for(
            "fn f(t: [1] Tensor[int, 4]) -> bool { proof { assert(t[0] == t[0]); } return true; }",
        );
        assert_eq!(diags.len(), 1, "expected one diagnostic, got {diags:?}");
        assert_eq!(diags[0].code, OBL_UNSUPPORTED);
    }

    /// No proof block means no obligations and no diagnostics.
    #[test]
    fn test_function_without_proof_block_has_no_obligations() {
        let diags = obligations_for("fn f(n: int) -> bool { return n == n; }");
        assert!(diags.is_empty(), "got {diags:?}");
    }
}
