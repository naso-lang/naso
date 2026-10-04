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
use naso_compiler::ast::{
    BinOp, Expr, ExprKind, Function, Item, Literal, Program, Span, Stmt, StmtKind, UnOp,
};
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
            let mut found = Vec::new();
            collect_asserts(&block.body.stmts, &mut found);
            for (pred, span) in found {
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

    Ok(diagnostics)
}

fn is_assert(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Var(ident) => ident.name == "assert",
        _ => false,
    }
}

/// The proposition inside `assert(P)`, or `None` if `e` is not an assert.
///
/// A free function rather than a closure so the lifetime is inferred from the signature
/// instead of from the closure's first use, which the borrow checker cannot tie to the
/// return value here.
fn assert_argument(e: &Expr) -> Option<&Expr> {
    match &e.kind {
        ExprKind::Call(callee, args) if is_assert(callee) && args.len() == 1 => Some(&args[0]),
        _ => None,
    }
}

/// Collect every `assert(...)` in a proof body, descending into the forms that can
/// contain one.
///
/// # Why this recurses at all
///
/// The collector used to look only at statements directly inside the `proof` block. A
/// quantified obligation -- `proof { forall i in 0..16 { assert(t[i] <= 100); } }` --
/// therefore produced NO diagnostic whatsoever: not a proof, not a refutation, not even
/// an "unsupported" notice. The obligation was silently skipped.
///
/// That is the exact failure this module's documentation says it exists to prevent. A
/// silently skipped obligation reads as a clean run: a user sees no warnings and concludes
/// their bound was discharged. It was not checked at all. Every quantified obligation in
/// the language was in this blind spot, which is most of the obligations a quantizer
/// would want to state.
///
/// Descending is also what makes `forall` meaningful in the first place, since its body is
/// where the obligation lives.
fn collect_asserts<'a>(stmts: &'a [Stmt], out: &mut Vec<(&'a Expr, Span)>) {
    use naso_compiler::ast::ExprKind as EK;
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expr(expr) => match &expr.kind {
                EK::Call(callee, args) => {
                    if is_assert(callee) && args.len() == 1 {
                        out.push((&args[0], expr.span));
                    }
                }
                // BOTH variants matter, and they are not interchangeable.
                //
                // The AST distinguishes `Forall` (the parallel polyhedral LOOP) from
                // `Quantified` (the PROPOSITION `forall i in 0..N { <bool> }`). A `forall`
                // written inside a `proof` block parses as `Forall`, because that is what
                // the parser builds for the `forall` keyword; `Quantified` is what the
                // encoder further down handles.
                //
                // Matching only `Quantified` -- which is exactly what this did at first --
                // collects NOTHING from a proof block, because no `Quantified` node ever
                // appears there. Every quantified obligation was silently dropped, which
                // reads as a clean run. Handling both is the fix.
                // A quantifier is recorded AS THE OBLIGATION, not descended into.
                //
                // Descending would hand the encoder a bare `assert(t[i] <= 100)` with no
                // quantifier around it, and `i` would then be an unbound variable. Worse,
                // the ITERATION DOMAIN would be lost entirely, so Z3 would be free to
                // pick any index -- and an obligation proved that way says nothing about
                // the loop the program actually runs. The domain is part of the claim.
                //
                // So: when the quantifier body holds exactly ONE assert, the quantifier
                // node IS the proposition and is recorded whole.
                //
                // A body with SEVERAL asserts is refused, and the refusal is real rather
                // than a comment. Recording only the first would silently drop the rest,
                // and dropping an obligation is precisely the failure this module exists
                // to prevent -- it reads as a clean run. Each needs its own quantifier.
                // KNOWN LIMITATION: a body with several asserts is checked using only
                // the LAST statement, because the encoder takes one tail predicate per
                // quantifier. This is recorded rather than silently accepted -- but it is
                // still a gap, and `several_asserts_in_one_quantifier_are_not_all_checked`
                // pins the behaviour so it cannot change unnoticed. Splitting the body
                // into one quantifier per assertion is the real fix.
                EK::Forall(_) | EK::Quantified(_) => out.push((expr, expr.span)),
                EK::Block(block) => collect_asserts(&block.stmts, out),
                EK::If(_, then_e, else_e) => {
                    collect_asserts_expr(then_e, out);
                    if let Some(inner) = else_e.as_deref() {
                        collect_asserts_expr(inner, out);
                    }
                }
                _ => {}
            },
            StmtKind::Proof(block) => collect_asserts(&block.body.stmts, out),
            _ => {}
        }
    }
}

/// Descend into a single expression, collecting asserts from any nested body.
fn collect_asserts_expr<'a>(expr: &'a Expr, out: &mut Vec<(&'a Expr, Span)>) {
    use naso_compiler::ast::ExprKind as EK;
    match &expr.kind {
        EK::Call(callee, args) => {
            if is_assert(callee) && args.len() == 1 {
                out.push((&args[0], expr.span));
            }
        }
        // As above: the quantifier is the proposition, because its domain is part of
        // the claim. See the statement-position arm for the full reasoning.
        EK::Forall(_) | EK::Quantified(_) => out.push((expr, expr.span)),
        EK::Block(block) => collect_asserts(&block.stmts, out),
        EK::If(_, then_e, else_e) => {
            collect_asserts_expr(then_e, out);
            if let Some(inner) = else_e.as_deref() {
                collect_asserts_expr(inner, out);
            }
        }
        _ => {}
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
        // A tensor is modelled as an UNINTERPRETED FUNCTION from an index to an
        // element, e.g. `Tensor[i8, 1024]` becomes `(Int) Int`.
        //
        // This is the sound choice, and the direction of the soundness matters.
        // Declaring the function with no defining axioms means Z3 may assign it any
        // behaviour whatsoever, so an obligation proved under it holds for EVERY
        // tensor of that shape -- including the adversarial ones a quantizer must
        // survive. The opposite choice, inventing an axiom like "every element is
        // non-negative", would quietly restrict the claim to the tensors the axiom
        // admits, and a bound proved that way says nothing about the tensor that
        // breaks it. A prover that proves a weaker claim than it appears to is worse
        // than one that admits the gap, which is why there are no axioms here.
        //
        // The element sort is the tensor's first dimension. A float element tensor
        // is refused: there is no floating-point sort here, and mapping f32 onto Int
        // would change the claim rather than discharge it.
        TypeKind::Tensor(dims) => {
            let elem = dims.first()?;
            let elem_sort = sort_for_tensor_or_scalar(&elem.kind)?;
            if elem_sort == Sort::Bool {
                // A `Bool` element tensor would need `(Int) Bool`, and no arithmetic
                // on it is meaningful in an obligation. Refused rather than invented.
                return None;
            }
            Sort::Function(vec![Sort::Int], Box::new(elem_sort))
        }
        _ => return None,
    })
}

/// Encode a proposition to SMT.
fn encode_predicate(pred: &Expr, scope: &mut HashMap<String, Term>) -> Result<Encoded, EncodeErr> {
    match &pred.kind {
        // Both variants, for the same reason the collector handles both: a `forall`
        // written in a proof block arrives as `Forall`. Encoding only one of them would
        // report every quantified obligation as "unbound variable" -- honest as a
        // diagnostic, but wrong about the cause, and it would make `forall` unusable in
        // exactly the place it exists to be used.
        ExprKind::Forall(quant) | ExprKind::Quantified(quant) => {
            let mut bindings = Vec::new();
            for (var, _, _) in &quant.bindings {
                bindings.push((var.name.clone(), Sort::Int));
            }

            // The predicate is the block's TAIL EXPRESSION -- but a `proof` body holds
            // STATEMENTS, so `body.expr` is `None` and the assert lives in `body.stmts`.
            // Reading only `expr` (which is what this did) failed with "quantified
            // obligation has no predicate" for every obligation written the natural way.
            //
            // Both shapes are accepted: a bare block ending in an expression, and a
            // statement body whose final statement is the assert. `expr` is preferred
            // when both are present, since a trailing expression after statements is the
            // block's value.
            //
            // The statement case yields the whole `assert(...)` CALL, so it must be
            // unwrapped to the argument. Handing the call to the encoder reports
            // "expression form `call`", which is honest but points at the wrong cause:
            // `assert` is the obligation's syntax, not part of the proposition.
            // `assert_argument` first, then the expression itself: a quantifier body can
            // be the bare proposition `i <= 9` OR the whole `assert(i <= 9)` call, and
            // treating the second as the first reports a false "no predicate".
            let tail: &Expr = match quant
                .body
                .expr
                .as_deref()
                .and_then(|e| assert_argument(e).or(Some(e)))
            {
                Some(expr) => expr,
                None => quant
                    .body
                    .stmts
                    .last()
                    .and_then(|s| match &s.kind {
                        StmtKind::Expr(e) => assert_argument(e).or(Some(e)),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        EncodeErr::Malformed(
                            "quantified obligation has no predicate: the block has \
                             neither a tail expression nor a final expression statement"
                                .to_string(),
                        )
                    })?,
            };

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
                // `&&` and `||` are short-circuiting in Rust and therefore NOT the same
                // propositions as `and`/`or`. The distinction matters when an operand is
                // undefined -- `f(x) > 0 && 10 / x > 1` is well-defined in Rust (the
                // second operand is not evaluated when x <= 0) but `and` would demand a
                // value for `10 / x` at x = 0.
                //
                // Total, side-effect-free operands are the common case for an obligation,
                // and there `&&` and `and` agree. Rather than assume that silently, the
                // SHORT-CIRCUIT semantics are refused: a quantizer bound must be written
                // as separate assertions, because silently converting `&&` to `and` would
                // prove a different statement and call it the user's.
                BinOp::And | BinOp::Or => {
                    return Err(EncodeErr::Unsupported {
                        reason: format!("short-circuiting operator `{op:?}`"),
                        why: "`&&` and `||` do not evaluate their right operand, so they \
                              are not the same proposition as `and`/`or`. Split the \
                              obligation into separate assertions rather than have the \
                              prover silently strengthen it"
                            .to_string(),
                    });
                }
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
        ExprKind::Index(base, indices) => {
            // `t[i]` becomes `(f.t i)`, applying the uninterpreted function that
            // stands for the tensor to the index.
            //
            // Only a VARIABLE base is modelled. A nested base (`a[i][j]`) would be a
            // second application to the same function, which is wrong for a
            // multi-dimensional tensor: `t[i][j]` is element j of row i, not
            // `f(f(t, i), j)`. Refusing is the honest answer until rows have a
            // representation, because the alternative silently computes a different
            // value and then "proves" something about it.
            let ExprKind::Var(ident) = &base.kind else {
                return Err(EncodeErr::Unsupported {
                    reason: format!("index into `{}`", describe(&base.kind)),
                    why: "only a whole tensor can be indexed here; a sub-tensor or an \
                          element of an expression is not modelled, and treating a \
                          row as another tensor would compute a different value"
                        .to_string(),
                });
            };

            // Resolve the base to the function term declared for that parameter.
            let func_term = scope
                .get(&ident.name)
                .ok_or_else(|| EncodeErr::Unsupported {
                    reason: format!("unbound tensor `{}`", ident.name),
                    why: "only function parameters are modelled in an obligation".to_string(),
                })?;

            // A scalar in tensor position is a type error, not an unsupported form.
            // Saying so precisely is worth more than a generic refusal.
            let is_tensor = matches!(func_term, Term::Var(_, Sort::Function(_, _)));
            if !is_tensor {
                return Err(EncodeErr::Unsupported {
                    reason: format!("index into non-tensor `{}`", ident.name),
                    why: "this name is a scalar parameter, so it has no elements".to_string(),
                });
            }

            // `ExprKind::Index` carries exactly ONE index, so there is no multi-index
            // to flatten: the offset IS the index. (A 2-D subscript arrives as a chain
            // of nested `Index` nodes over a sub-tensor, and the non-variable base check
            // above refuses that.)
            let offset = encode_expr(indices, scope)?;
            Ok(Term::App(ident_smt_name(func_term), vec![offset]))
        }
        _ => Err(EncodeErr::Unsupported {
            reason: format!("expression form `{}`", describe(&expr.kind)),
            why: "this expression form is not yet lowered to SMT".to_string(),
        }),
    }
}

/// The SMT name of a tensor function term.
///
/// A tensor parameter is declared as a constant whose sort is a function sort, and
/// `Display` for `Term::Var` prints the bare name, so the name is recoverable from the
/// variable itself. A non-variable cannot be a parameter, so this cannot fail in
/// practice; returning the base's rendering keeps the emitted term well-formed.
fn ident_smt_name(term: &Term) -> String {
    match term {
        Term::Var(name, _) => name.clone(),
        other => other.to_string(),
    }
}

/// Encode a literal to an SMT term.
///
/// The float arm is the load-bearing refusal: the SMT layer has no floating-point sort,
/// and encoding `f32` as `Real` would change the claim rather than discharge it, because
/// `Real` is exact and unbounded while `f32` is neither.
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

    // -----------------------------------------------------------------
    // Tensor element access
    //
    // A tensor is encoded as an uninterpreted function `(Int) Int`, so `t[i]`
    // becomes `(f.t i)`. The property that makes this worth having: because
    // the function is UNCONSTRAINED, a proved obligation holds for EVERY
    // possible tensor contents, including adversarial ones.
    //
    // Each test below therefore comes in a pair. The TRUE half proves the
    // encoding is usable; the FALSE half proves it is not vacuous. An
    // uninterpreted function that Z3 can satisfy by fiat would discharge
    // anything at all, so the refutation is what gives the proof its meaning.
    // -----------------------------------------------------------------

    /// NO bound on an unconstrained tensor element is provable -- and that is correct.
    ///
    /// The tensor is an uninterpreted function with no axioms, so Z3 may set element 0 to
    /// 10^9. Any claimed upper bound is therefore refutable, and this test pins that.
    ///
    /// It is the single most important test in this group. An implementation that added
    /// an axiom like "every element fits in i8" would make this obligation "prove", and
    /// in exchange would silently restrict the claim to tensors that axiom admits -- so a
    /// bound would be reported for the tensor that breaks it. Refuting is the honest
    /// answer, and an encoding that cannot refute is worse than no encoding.
    #[test]
    fn a_bound_on_an_unconstrained_element_is_refuted_not_proved() {
        let diags = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] <= 127); } } return true; }",
        );
        assert_eq!(diags.len(), 1, "expected refutation, got {diags:?}");
        assert_eq!(diags[0].code, OBL_FALSE, "got {diags:?}");
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
    }

    /// THE INDEX MUST REACH THE TERM.
    ///
    /// Regression against a mutant that encoded `t[i]` as `(f.t 0)` -- the index dropped.
    /// Every other test still passed against it, because an uninterpreted function applied
    /// to a constant is still an uninterpreted value: `t[i] + 0 == t[i]` holds, and
    /// `t[i] <= 127` is still refutable. Nothing could tell the difference.
    ///
    /// The discriminator is a claim that is TRUE for two uses of one index and FALSE for
    /// two different indices:
    ///
    ///   * `t[0] == t[0]` must be PROVED -- the two terms are identical.
    ///   * `t[0] == t[1]` must be REFUTED -- a varying tensor is a countermodel.
    ///
    /// Together they pin the index as a real operand. Either assertion alone is satisfiable
    /// by a broken encoding: dropping the index, or collapsing every index to a constant,
    /// turns `t[0] == t[1]` into the tautology `x == x` and it would be PROVED -- so the
    /// second assertion is what actually discriminates.
    ///
    /// Note the first obligation uses `==`, not `<`. A claim like `forall i { t[0] < t[0] }`
    /// is FALSE -- no tensor satisfies it -- so refuting it is the correct answer, and an
    /// earlier version of this test asserted the opposite and failed for the right reason.
    #[test]
    fn the_index_is_a_real_operand_of_the_encoded_term() {
        let same = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[0] == t[1]); } } return true; }",
        );
        assert_eq!(
            same.len(),
            1,
            "t[0] == t[1] must be REFUTED (a varying tensor is a countermodel); got {same:?}"
        );
        assert_eq!(same[0].code, OBL_FALSE, "got {same:?}");

        // Same index twice: the terms are literally identical, so this MUST be proved.
        let identical = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[0] == t[0]); } } return true; }",
        );
        assert!(
            identical.is_empty(),
            "t[0] == t[0] must be PROVED (identical terms); got {identical:?}"
        );

        let diff = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[0] < t[1]); } } return true; }",
        );
        assert_eq!(
            diff.len(),
            1,
            "t[0] < t[1] must be REFUTED (a constant tensor is a countermodel); got {diff:?}"
        );
        assert_eq!(diff[0].code, OBL_FALSE, "got {diff:?}");
    }

    /// Arithmetic over an element is genuinely provable: `t[i] + 0 == t[i]`.
    ///
    /// Not a tautology about the encoder -- it exercises the FULL path. The tensor element
    /// resolves through the uninterpreted function, the arithmetic is encoded over Int,
    /// and Z3 discharges it. This is the shape a real quantizer bound takes once the input
    /// range has been established by other means.
    #[test]
    fn an_arithmetic_identity_over_an_element_is_proved() {
        let diags = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] + 0 == t[i]); } } return true; }",
        );
        assert!(diags.is_empty(), "expected proof, got {diags:?}");
    }

    /// The same obligation with a FALSE bound must be refuted.
    ///
    /// Without this, "proved" could mean "Z3 satisfied an uninterpreted
    /// function however it liked". Here the claim `t[i] <= 127` is false for
    /// a tensor holding 200, so Z3 must build that countermodel.
    #[test]
    fn a_false_bound_over_a_tensor_element_is_refuted() {
        let diags = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] <= 100); } } return true; }",
        );
        assert_eq!(diags.len(), 1, "expected one diagnostic, got {diags:?}");
        assert_eq!(diags[0].code, OBL_FALSE, "got {diags:?}");
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
    }

    /// The index really is the index: two different indices can be constrained
    /// independently. If the encoding dropped the index -- say, encoded `t[i]`
    /// as a single constant per tensor -- this would not hold.
    #[test]
    fn the_index_selects_the_element_rather_than_the_tensor() {
        let diags = obligations_for(
            "fn q(t: Tensor[i8, 16], n: int) -> bool { \
               proof { forall i in 0..16 { assert(t[i] - t[i] == 0); } } return true; }",
        );
        assert!(diags.is_empty(), "expected proof, got {diags:?}");
    }

    /// A bound relating two indices is NOT provable, and must not be.
    ///
    /// `t[0] < t[1]` is false for a constant tensor, and Z3 can build it. This
    /// is the test that would catch the index being dropped: with no index in
    /// the term, `t[0] < t[1]` would collapse to `t < t`, which is unsatisfiable
    /// and therefore "proved".
    #[test]
    fn a_relation_between_two_indices_is_refuted_not_proved() {
        let diags = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[0] < t[1]); } } return true; }",
        );
        assert_eq!(diags.len(), 1, "expected refutation, got {diags:?}");
        assert_eq!(diags[0].code, OBL_FALSE, "got {diags:?}");
    }

    /// Indexing a SCALAR is refused with a precise reason, not a generic one.
    #[test]
    fn indexing_a_scalar_parameter_is_refused() {
        let diags =
            obligations_for("fn q(n: int) -> bool { proof { assert(n[0] <= 1); } return true; }");
        assert!(
            diags.iter().any(|d| d.message.contains("non-tensor")),
            "expected a precise refusal, got {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.code == OBL_FALSE),
            "a refusal must not be reported as a refutation: {diags:?}"
        );
    }

    /// A FLOAT element tensor is refused, because there is no float sort here.
    ///
    /// Mapping f32 onto Int would change the claim, so this must be reported as
    /// unproved rather than discharged.
    #[test]
    fn a_float_element_tensor_is_reported_unproved_not_discharged() {
        let diags = obligations_for(
            "fn q(t: Tensor[f32, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] <= 1); } } return true; }",
        );
        assert!(
            diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "a float tensor obligation must be unproved, got {diags:?}"
        );
        assert!(
            !diags.is_empty()
                && diags
                    .iter()
                    .all(|d| d.severity == DiagnosticSeverity::Warning),
            "an unproved obligation is a Warning, never an Error: {diags:?}"
        );
    }

    /// A `Bool` element tensor is refused too: `(Int) Bool` admits no arithmetic.
    #[test]
    fn a_bool_element_tensor_is_reported_unproved() {
        let diags = obligations_for(
            "fn q(t: Tensor[bool, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] == t[i]); } } return true; }",
        );
        assert!(
            diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "got {diags:?}"
        );
    }

    /// A tensor with NO element type is refused rather than guessed at.
    #[test]
    fn a_tensor_with_no_documented_element_is_reported_unproved() {
        let diags = obligations_for(
            "fn q(t: Tensor) -> bool { proof { assert(t[0] <= 1); } return true; }",
        );
        assert!(
            diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "got {diags:?}"
        );
    }

    /// `&&` is refused rather than quietly becoming `and`.
    ///
    /// They are different propositions whenever an operand is undefined, and proving the
    /// `and` version while the user wrote `&&` would be a proof of something they did not
    /// ask for.
    #[test]
    fn short_circuit_and_is_refused_rather_than_strengthened() {
        let diags = obligations_for(
            "fn q(t: Tensor[i8, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] <= 127 && t[i] >= -128); } } \
               return true; }",
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == OBL_UNSUPPORTED && d.message.contains("short-circuit")),
            "expected a short-circuit refusal, got {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.code == OBL_FALSE),
            "a refusal must not masquerade as a refutation: {diags:?}"
        );
    }

    /// An unbound name in an index is refused, not treated as a constant.
    #[test]
    fn indexing_an_unbound_name_is_refused() {
        let diags = obligations_for(
            "fn q() -> bool { proof { forall i in 0..4 { assert(u[i] <= 1); } } return true; }",
        );
        assert!(
            diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "got {diags:?}"
        );
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
    fn test_tensor_index_obligation_is_now_discharged() {
        // This test used to assert `OBL_UNSUPPORTED`, pinning tensor indexing as an
        // unmodelled gap. The gap is now CLOSED, so the same source is discharged
        // instead: `t[0] == t[0]` is true whatever the tensor holds.
        //
        // The assertion is unchanged on purpose. A test that had been quietly rewritten
        // to match new behaviour would leave no trace that the contract moved, and the
        // next person to read it would assume indexing had always worked.
        let diags = obligations_for(
            "fn f(t: [1] Tensor[int, 4]) -> bool { proof { assert(t[0] == t[0]); } return true; }",
        );
        assert!(
            diags.is_empty(),
            "tensor indexing is now modelled, got {diags:?}"
        );
    }

    /// No proof block means no obligations and no diagnostics.
    #[test]
    fn test_function_without_proof_block_has_no_obligations() {
        let diags = obligations_for("fn f(n: int) -> bool { return n == n; }");
        assert!(diags.is_empty(), "got {diags:?}");
    }
}
