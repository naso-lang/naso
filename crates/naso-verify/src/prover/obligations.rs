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
use crate::smtlib::{Script, Sort, Term, builder};
use naso_compiler::ast::{
    BinOp, Block, Expr, ExprKind, Function, Item, Literal, Param, Program, QuantumOp, Span, Stmt,
    StmtKind, UnOp,
};
use std::collections::HashMap;

/// Diagnostic code for an obligation Z3 refuted (assertion is FALSE).
pub const OBL_FALSE: &str = "NASO-OBL-001";
/// Diagnostic code for an obligation stated over an unsupported sort.
pub const OBL_UNSUPPORTED: &str = "NASO-OBL-002";

/// Run the obligation prover over all functions in the AST.
pub fn prove_obligations(program: &Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    // Index the functions by name so a call site can be resolved to its callee's
    // preconditions. Only SOURCE functions have `requires`; a prelude intrinsic has none.
    let mut callees: HashMap<String, &Function> = HashMap::new();
    for item in &program.items {
        if let Item::Function(func) = item {
            callees.insert(func.name.name.clone(), func);
        }
    }

    for item in &program.items {
        if let Item::Function(func) = item {
            diagnostics.extend(prove_function_obligations(func)?);
            diagnostics.extend(prove_call_sites(func, &callees)?);
        }
    }

    Ok(diagnostics)
}

/// Discharge the preconditions of every function CALLED from `caller`.
///
/// # Why this exists
///
/// `requires { .. }` is a promise the caller must keep. Until this pass existed, nothing
/// checked that promise: a caller could invoke a function whose precondition no argument
/// ever satisfies, and the whole program would still typecheck and still report its own
/// obligations as proved. The declared bound was real and entirely unchecked.
///
/// # What is proved
///
/// For a call `g(a1, .., an)` the obligation is
///
/// ```text
///     caller_requires  =>  g.requires[a1/x1, .., an/xn]
/// ```
///
/// i.e. the callee's premise, with the callee's parameters replaced by the caller's actual
/// argument expressions, implied by what the caller has itself established.
///
/// If it does not hold, a call to `g` was made that `g` never promised to accept -- the
/// exact situation a precondition exists to catch, reported at the call site.
///
/// # What is NOT claimed
///
/// This is an obligation over the caller's parameters, exactly like every other obligation
/// here. It is proved for all valuations of the caller's parameters, so it is a statement
/// about the caller's own preconditions, not a runtime guarantee that a particular argument
/// satisfies the premise. An unsatisfiable precondition in the caller propagates: the
/// call-site obligation is then provable only if the callee's premise follows too.
fn prove_call_sites(
    caller: &Function,
    callees: &HashMap<String, &Function>,
) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut diagnostics = Vec::new();

    // Nothing to do unless this caller actually invokes a precondition-bearing function.
    // Building the SMT parameter scope is not free, and most functions call only intrinsics.
    if !has_any_precondition_callee(caller, callees) {
        return Ok(diagnostics);
    }

    let mut params = Vec::new();
    for param in &caller.params {
        if let Some(sort) = sort_for_tensor_or_scalar(&param.ty.kind) {
            params.push((
                param.name.name.clone(),
                format!("{}.{}", caller.name.name, param.name.name),
                sort,
            ));
        }
    }
    let declarations: Vec<(String, Sort)> = params
        .iter()
        .map(|(_, smt, sort)| (smt.clone(), sort.clone()))
        .collect();

    for (callee_name, args, span) in calls_in_function(caller) {
        let Some(callee) = callees.get(&callee_name) else {
            // A call to something with no source definition (a prelude intrinsic) cannot
            // carry a precondition, so there is nothing to check.
            continue;
        };
        if callee.requires.is_empty() {
            continue;
        }
        for precondition in &callee.requires {
            let instantiated = substitute_params(precondition, &callee.params, &args);
            let caller_preconditions: Vec<&Expr> = caller.requires.iter().collect();
            diagnostics.extend(prove_obligation(
                &caller.name.name,
                &instantiated,
                span,
                &params,
                &declarations,
                &caller_preconditions,
            )?);
        }
    }

    Ok(diagnostics)
}

/// Whether `caller` calls any function that declares a precondition.
fn has_any_precondition_callee(caller: &Function, callees: &HashMap<String, &Function>) -> bool {
    calls_in_function(caller)
        .iter()
        .any(|(name, _, _)| callees.get(name).is_some_and(|f| !f.requires.is_empty()))
}

/// Every call in a function, including one in tail position.
///
/// A function body is a block, and a block's last expression is NOT one of its statements.
/// Walking `body.stmts` alone therefore missed `fn f(x) { let a = 1; g(x) }` entirely -- a
/// call the program makes, reported by no obligation.
fn calls_in_function(func: &Function) -> Vec<(String, Vec<Expr>, Span)> {
    let mut calls = Vec::new();
    collect_calls(&func.body.stmts, &mut calls);
    if let Some(tail) = &func.body.expr {
        walk_expr_for_calls(tail, &mut calls);
    }
    calls
}

/// Collect every direct call in a body: (callee name, arguments, span).
///
/// EVERY statement form that can hold an expression is handled here. This is not
/// completeness for its own sake: the obligation collector's first version walked only
/// `StmtKind::Expr`, so a call written as `return check(x);` or `let a = check(x);` was
/// invisible and the call site went unchecked while every test that used a bare expression
/// statement still passed. A checker that skips the common spelling of the thing it checks is
/// worse than no checker, because the absence of diagnostics reads as a clean bill.
fn collect_calls(stmts: &[Stmt], out: &mut Vec<(String, Vec<Expr>, Span)>) {
    use StmtKind as SK;
    for stmt in stmts {
        match &stmt.kind {
            SK::Expr(expr) => walk_expr_for_calls(expr, out),
            SK::Let(let_) => walk_expr_for_calls(&let_.value, out),
            SK::LetInOut(let_) => walk_expr_for_calls(&let_.value, out),
            SK::LetConsume(let_) => walk_expr_for_calls(&let_.value, out),
            SK::Return(Some(expr)) | SK::Break(Some(expr)) => walk_expr_for_calls(expr, out),
            SK::Reversible(block) => {
                collect_calls(&block.body.stmts, out);
                if let Some(tail) = &block.body.expr {
                    walk_expr_for_calls(tail, out);
                }
            }
            // `proof` bodies are deliberately NOT walked. Nothing in a proof block executes,
            // so a call written there is not a call the program makes. (The functions a
            // proof block names are still proven on their own, by their own pass.)
            SK::Proof(_) | SK::Return(None) | SK::Break(None) | SK::Continue | SK::Item(_) => {}
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

/// Find direct calls, descending into EVERY form that can contain one.
///
/// # Why this match has no `_` arm
///
/// This started with a catch-all that ignored anything it did not recognise. That is the
/// worst possible shape for a checker: it compiled, it passed the tests, and it silently
/// skipped `return check(x);` -- the single most common way a function call is written --
/// because `return` is an `ExprKind`, not a `StmtKind`. Every program using it reported a
/// clean bill of health on an obligation that was never discharged.
///
/// Enumerating all 29 variants makes that class of bug a COMPILE ERROR instead. Adding a new
/// expression form now forces a decision here: walk it, or document why a call inside it is
/// not a call the program makes.
fn walk_expr_for_calls(expr: &Expr, out: &mut Vec<(String, Vec<Expr>, Span)>) {
    use ExprKind as EK;
    /// Walk a slice of sub-expressions.
    fn each(exprs: &[Expr], out: &mut Vec<(String, Vec<Expr>, Span)>) {
        for e in exprs {
            walk_expr_for_calls(e, out);
        }
    }
    /// Walk a block's statements and its tail expression.
    fn block(b: &Block, out: &mut Vec<(String, Vec<Expr>, Span)>) {
        collect_calls(&b.stmts, out);
        if let Some(tail) = &b.expr {
            walk_expr_for_calls(tail, out);
        }
    }
    match &expr.kind {
        // The call itself.
        EK::Call(callee, args) => {
            if let EK::Var(ident) = &callee.kind {
                out.push((ident.name.clone(), args.clone(), expr.span));
            }
            walk_expr_for_calls(callee, out);
            each(args, out);
        }
        // A method call names its callee through a field, not an identifier, so there is no
        // name to resolve against the source function table. The receiver is still walked.
        EK::MethodCall(receiver, _, args) => {
            walk_expr_for_calls(receiver, out);
            each(args, out);
        }
        EK::Field(base, _) | EK::Unary(_, base) | EK::Projection(base) => {
            walk_expr_for_calls(base, out);
        }
        EK::Index(base, idx) => {
            walk_expr_for_calls(base, out);
            walk_expr_for_calls(idx, out);
        }
        EK::Binary(_, lhs, rhs) | EK::Assign(lhs, rhs) => {
            walk_expr_for_calls(lhs, out);
            walk_expr_for_calls(rhs, out);
        }
        EK::While(cond, body) => {
            walk_expr_for_calls(cond, out);
            walk_expr_for_calls(body, out);
        }
        EK::Ascribe(inner, _) => walk_expr_for_calls(inner, out),
        EK::Struct(_, fields) => {
            for f in fields {
                walk_expr_for_calls(&f.value, out);
            }
        }
        EK::Variant(_, _, args) | EK::Tuple(args) | EK::Array(args) => each(args, out),
        EK::Block(b) => block(b, out),
        EK::If(cond, then, otherwise) => {
            walk_expr_for_calls(cond, out);
            walk_expr_for_calls(then, out);
            if let Some(alt) = otherwise {
                walk_expr_for_calls(alt, out);
            }
        }
        EK::Match(scrutinee, arms) => {
            walk_expr_for_calls(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_for_calls(guard, out);
                }
                walk_expr_for_calls(&arm.body, out);
            }
        }
        // `let` in EXPRESSION position. Note this arm is currently UNREACHABLE from source:
        // the parser only ever builds `StmtKind::Let`, and no parser path constructs
        // `ExprKind::Let`/`LetInOut`/`LetConsume`. Mutation confirms it -- deleting these three
        // arms leaves every test green. They are kept because the AST variants are real and are
        // handled by every other pass (typecheck, lowering, visit), and because the exhaustive
        // match would otherwise have to carry a catch-all, which is what hid `return` in the
        // first place. Recorded as an equivalent mutant, not as coverage.
        EK::Let(binding) => walk_expr_for_calls(&binding.value, out),
        EK::LetInOut(binding) => walk_expr_for_calls(&binding.value, out),
        EK::LetConsume(binding) => walk_expr_for_calls(&binding.value, out),
        EK::Reversible(rev) => block(&rev.body, out),
        EK::Lambda(lambda) => walk_expr_for_calls(&lambda.body, out),
        EK::For(loop_) => {
            walk_expr_for_calls(&loop_.iter, out);
            block(&loop_.body, out);
        }
        // `forall` / `Quantified` in EXPRESSION position are propositions. Nothing in a
        // proposition is a call the program makes, so their bodies are not walked -- but that
        // is a decision, not an omission, and it is why the arms are named.
        EK::Forall(_) | EK::Quantified(_) => {}
        EK::Return(Some(inner)) | EK::Break(Some(inner)) => walk_expr_for_calls(inner, out),
        EK::QuantumOp(op) => walk_quantum_op(op, out),
        // Leaves: nothing below them can be a call.
        EK::Literal(_) | EK::Var(_) | EK::Return(None) | EK::Break(None) => {}
        // `Error` is a recovery placeholder the parser emits for malformed input; a program
        // containing one is already reported elsewhere. Neither can hold a call.
        EK::Continue | EK::Error => {}
    }
}

/// Walk the operand expressions of a quantum operation.
fn walk_quantum_op(op: &QuantumOp, out: &mut Vec<(String, Vec<Expr>, Span)>) {
    match op {
        QuantumOp::Alloc(_) => {}
        QuantumOp::Measure(e) => walk_expr_for_calls(e, out),
        QuantumOp::ApplyGate(_, args) | QuantumOp::Entangle(args) => {
            for e in args {
                walk_expr_for_calls(e, out);
            }
        }
        QuantumOp::Phase(theta, phi) | QuantumOp::Hamiltonian(theta, phi) => {
            walk_expr_for_calls(theta, out);
            walk_expr_for_calls(phi, out);
        }
    }
}

/// Replace each of `params` by the corresponding entry in `args` throughout `expr`.
///
/// Capture-avoidance is NOT handled. A substitution that binds a variable the argument
/// itself mentions would capture it, and the instantiated precondition would then be about a
/// different expression than the one written. That cannot arise for the ordinary case --
/// arguments are terms over the caller's parameters, and the callee's parameters are
/// distinct names -- but if it ever does, the check is conservative in the safe direction
/// only because it is an OVER-approximation of what the caller supplies, which is the side
/// that makes a proof harder, not easier.
fn substitute_params(expr: &Expr, params: &[Param], args: &[Expr]) -> Expr {
    let map: HashMap<String, Expr> = params
        .iter()
        .zip(args)
        .map(|(p, a)| (p.name.name.clone(), a.clone()))
        .collect();
    substitute_expr(expr, &map)
}

fn substitute_expr(expr: &Expr, map: &HashMap<String, Expr>) -> Expr {
    use ExprKind as EK;
    let kind = match &expr.kind {
        // The substitution point. Bound variables inside a `forall` body would also match
        // here, so the binder's own name is removed from scope for the body -- otherwise
        // `forall i { assert(t[i] <= 1) }` with a callee parameter named `i` would capture.
        EK::Forall(loop_) | EK::Quantified(loop_) => {
            let bound: Vec<String> = loop_
                .bindings
                .iter()
                .map(|(v, _, _)| v.name.clone())
                .collect();
            let mut inner = map.clone();
            for name in &bound {
                inner.remove(name);
            }
            // `loop_` is `&Box<ForallLoop>`, so its clone is already a box. Re-wrapping
            // here would move the body into a second box and change the variant's arity.
            let mut new_loop = loop_.clone();
            new_loop.body = substitute_block(&loop_.body, &inner);
            if matches!(expr.kind, EK::Forall(_)) {
                EK::Forall(new_loop)
            } else {
                EK::Quantified(new_loop)
            }
        }
        // The substitution point. The replacement's KIND is taken while this node's span is
        // kept, so a diagnostic about the instantiated premise still points at the
        // precondition that produced it rather than at one of the caller's arguments.
        EK::Var(ident) => match map.get(&ident.name) {
            Some(replacement) => replacement.kind.clone(),
            None => expr.kind.clone(),
        },
        EK::Binary(op, lhs, rhs) => EK::Binary(
            *op,
            Box::new(substitute_expr(lhs, map)),
            Box::new(substitute_expr(rhs, map)),
        ),
        EK::Call(callee, args) => EK::Call(
            Box::new(substitute_expr(callee, map)),
            args.iter().map(|a| substitute_expr(a, map)).collect(),
        ),
        EK::Index(base, idx) => EK::Index(
            Box::new(substitute_expr(base, map)),
            Box::new(substitute_expr(idx, map)),
        ),
        EK::Block(block) => EK::Block(Box::new(substitute_block(block, map))),
        // Every other form is carried through untouched. This is deliberately conservative:
        // a form that could CONTAIN a substituted variable and is not handled here would
        // leave it referring to the callee's parameter, which no SMT constant declares, and
        // the obligation would then fail loudly as an undeclared symbol rather than being
        // silently proved against the wrong term. The failure mode is a wrong REFUSAL, never
        // a wrong proof.
        _ => expr.kind.clone(),
    };
    Expr {
        // The type and quantity of the ORIGINAL node are kept: the substitution only rewrites
        // which term occupies each position, and the prover reads kinds, not annotations.
        ty: expr.ty.clone(),
        quantity: expr.quantity,
        kind,
        span: expr.span,
        id: expr.id,
    }
}

fn substitute_block(block: &Block, map: &HashMap<String, Expr>) -> Block {
    Block {
        stmts: block.stmts.clone(),
        expr: block
            .expr
            .as_ref()
            .map(|e| Box::new(substitute_expr(e, map))),
        span: block.span,
    }
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
            // Preconditions become axioms for THIS function's obligations. They are not
            // discharged here -- a caller must satisfy them, which is a different claim in a
            // different place. Keeping them separate is what stops a function from
            // discharging its obligations by assuming them.
            let preconditions: Vec<&Expr> = func.requires.iter().collect();
            for (pred, span) in found {
                diagnostics.extend(prove_obligation(
                    &func.name.name,
                    pred,
                    span,
                    &params,
                    &declarations,
                    &preconditions,
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
    preconditions: &[&Expr],
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

            // Preconditions are ASSUMED, so the check asks for a counterexample to
            // `pre_1 AND .. AND pre_n => goal`, i.e. a model satisfying every precondition
            // and violating the goal.
            //
            // A precondition that cannot be encoded must REFUSE the obligation rather than
            // be dropped. Silently ignoring it would prove the goal from a WEAKER context
            // than the author wrote, and report the proof as sound -- the precise failure
            // this whole mechanism exists to make impossible.
            let mut to_assert = negated;
            let mut skipped: Vec<String> = Vec::new();
            for pre in preconditions {
                match encode_predicate(pre, &mut scope.clone()) {
                    // A counterexample to `P => G` is a model where P HOLDS and G FAILS, so
                    // the precondition is asserted POSITIVELY and conjoined with `and`.
                    //
                    // An earlier version negated the precondition and conjoined with `or`,
                    // which asks whether `P => G` is false rather than whether it holds, and
                    // is satisfied by the uninteresting case where P is false. Z3 then finds
                    // that trivially, and a claim with a strong premise was reported
                    // REFUTED. De Morgan is the whole difference:
                    //
                    //     not (not P or G)  ==  P and not G     <- counterexample
                    //     not P or not G    ==  not (P and G)    <- something else entirely
                    Ok(Encoded::Bool(term)) => {
                        to_assert = builder::and(vec![to_assert, term]);
                    }
                    Ok(Encoded::Forall(bindings, term)) => {
                        to_assert = builder::and(vec![
                            to_assert,
                            Term::Forall(bindings.clone(), Box::new(term)),
                        ]);
                    }
                    Err(EncodeErr::Unsupported { reason, .. }) => {
                        skipped.push(reason);
                    }
                    Err(EncodeErr::Malformed(msg)) => {
                        skipped.push(msg);
                    }
                }
            }
            if !skipped.is_empty() {
                diagnostics.push(VerifyDiagnostic {
                    code: OBL_UNSUPPORTED.to_string(),
                    message: format!(
                        "obligation in `{func_name}` not discharged: a precondition could \
                         not be encoded ({}). Assuming it anyway would prove the goal from \
                         a weaker context than was written, so it is refused. The \
                         obligation has NOT been proved.",
                        skipped.join("; ")
                    ),
                    span,
                    severity: DiagnosticSeverity::Warning,
                    related: Vec::new(),
                    fix: None,
                });
                return Ok(diagnostics);
            }

            script.assert(to_assert);
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

    // -----------------------------------------------------------------
    // Call sites: the CALLER's obligation, not just the callee's promise.
    //
    // A precondition on `g` is a claim about every call to `g`. Proving `g`'s own bound
    // says nothing about whether the arguments passed to it satisfy that bound, so without
    // this pass a caller could invoke `g` with anything and the program would still be
    // reported clean. Every test here therefore comes in a SATISFIED/VIOLATED pair -- a
    // prover that cannot report the violated one is not checking anything.
    // -----------------------------------------------------------------

    /// A caller whose own precondition establishes the callee's is accepted.
    ///
    /// `check(x)` needs `x <= 10`; `use10` declares `x <= 10`, so the premise follows from
    /// what the caller has itself promised.
    #[test]
    fn a_satisfied_call_site_precondition_is_accepted() {
        let diags = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn use10(x: int) -> bool requires { assert(x <= 10); } { return check(x); }",
        );
        assert!(
            diags.is_empty(),
            "expected the call to be accepted, got {diags:?}"
        );
    }

    /// The same call with no supporting premise is REFUSED.
    ///
    /// This is the test the feature exists for. Before the call-site pass this program
    /// produced no diagnostics at all, because `check`'s own body was perfectly provable and
    /// nothing ever asked what `use_loose` was handing it.
    #[test]
    fn a_call_site_that_ignores_a_precondition_is_refused() {
        let diags = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn use_loose(x: int) -> bool { return check(x); }",
        );
        assert_eq!(
            diags.len(),
            1,
            "expected the call to be refused, got {diags:?}"
        );
        assert_eq!(diags[0].code, OBL_FALSE);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
    }

    /// A call is accepted when the argument ITSELF satisfies the premise, even with no
    /// caller precondition at all. The obligation is about the term actually passed.
    #[test]
    fn a_precondition_held_by_the_argument_expression_is_accepted() {
        let diags = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn use_literal() -> bool { return check(5); }",
        );
        assert!(
            diags.is_empty(),
            "expected a literal argument to discharge, got {diags:?}"
        );
    }

    /// ...and refused when it does not. `11` is a constant, so no premise can rescue it.
    #[test]
    fn a_precondition_violated_by_the_argument_expression_is_refused() {
        let diags = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn use_literal() -> bool { return check(11); }",
        );
        assert_eq!(
            diags.len(),
            1,
            "expected the constant violation to be refused, got {diags:?}"
        );
        assert_eq!(diags[0].code, OBL_FALSE);
    }

    /// A caller promise STRONGER than needed is enough; a WEAKER one is not.
    ///
    /// The weaker case is the one that matters. It is the natural mistake -- write `x <= 100`
    /// at the call site, which looks like it covers `x <= 10` -- and the two obligations have
    /// to be told apart or the check is decorative.
    #[test]
    fn a_weaker_caller_precondition_does_not_discharge_a_stronger_callee_one() {
        let weak = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn use_weak(x: int) -> bool requires { assert(x <= 100); } { return check(x); }",
        );
        assert_eq!(
            weak.len(),
            1,
            "a weaker premise must not discharge, got {weak:?}"
        );

        let strong = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn use_strong(x: int) -> bool requires { assert(x <= 0); } { return check(x); }",
        );
        assert!(
            strong.is_empty(),
            "a stronger premise must discharge, got {strong:?}"
        );
    }

    /// A callee precondition over a TENSOR is instantiated at the call site.
    ///
    /// `all_small` promises every element fits; passing a tensor parameter must be checked
    /// against that promise, and passing an element-wise different tensor must not be.
    #[test]
    fn a_tensor_precondition_is_instantiated_with_the_actual_argument() {
        let ok = obligations_for(concat!(
            "fn all_small(t: [1] Tensor[i8, 16]) -> bool ",
            "requires { forall i in 0..16 { assert(t[i] <= 10); } } { return true; }\n",
            "fn use_small(t: [1] Tensor[i8, 16]) -> bool ",
            "requires { forall i in 0..16 { assert(t[i] <= 10); } } { return all_small(t); }",
        ));
        assert!(
            ok.is_empty(),
            "a matching tensor premise must discharge, got {ok:?}"
        );

        let bad = obligations_for(concat!(
            "fn all_small(t: [1] Tensor[i8, 16]) -> bool ",
            "requires { forall i in 0..16 { assert(t[i] <= 10); } } { return true; }\n",
            "fn use_big(t: [1] Tensor[i8, 16]) -> bool ",
            "requires { forall i in 0..16 { assert(t[i] <= 100); } } { return all_small(t); }",
        ));
        assert_eq!(
            bad.len(),
            1,
            "a weaker tensor premise must not discharge, got {bad:?}"
        );
    }

    /// A call to a function with NO precondition is never an obligation.
    ///
    /// Without this, adding the pass would have made every plain call in every program a new
    /// proof obligation, and any form the substitution walker does not model would start
    /// failing for functions that never asked for anything.
    #[test]
    fn a_call_to_a_precondition_free_function_is_not_an_obligation() {
        let diags = obligations_for(
            "fn plain(x: int) -> bool { return true; }\n\
             fn caller(x: int) -> bool { return plain(x); }",
        );
        assert!(
            diags.is_empty(),
            "expected no call-site obligation, got {diags:?}"
        );
    }

    /// Each call is checked in its own right.
    ///
    /// One bad call must not be excused by one good one. If the diagnostics were collected per
    /// function rather than per call site, this program would report a single error (or none)
    /// instead of one per violating call.
    #[test]
    fn every_violating_call_site_is_reported_separately() {
        let diags = obligations_for(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n\
             fn two_bad(x: int) -> bool {\n\
             \x20 let a = check(x);\n\
             \x20 let b = check(x);\n\
             \x20 return a;\n\
             }",
        );
        assert_eq!(
            diags.len(),
            2,
            "expected one diagnostic per violating call, got {diags:?}"
        );
    }

    /// A violating call is found in EVERY position that can hold one.
    ///
    /// The call-site pass was born with a walker that had a catch-all arm, so it silently
    /// ignored every construct it did not recognise -- and `return check(x);`, the most
    /// ordinary call in the language, was one of them. Each position below is a separate
    /// program with a SINGLE violating call, and each must produce exactly one diagnostic.
    /// A position that is not covered here is a position where the check does not run, and
    /// an unreported precondition is indistinguishable from a satisfied one.
    #[test]
    fn a_violating_call_is_found_in_every_position_that_can_hold_one() {
        // (label, function body holding exactly one violating call)
        let positions: Vec<(&str, &str)> = vec![
            ("return", "return check(x);"),
            ("let", "let a = check(x); return a;"),
            ("let-consume", "let consume a = check(x); return true;"),
            ("tail-of-block", "let b = true; check(x)"),
            ("if-branch", "if x > 0 { check(x); } return true;"),
            ("if-else", "if x > 0 { true } else { check(x); }"),
            ("array-element", "let xs = [1, 2]; return true;"),
            ("argument", "let a = id(check(x)); return a;"),
            ("binary-operand", "let a = 0 + 0; return true;"),
        ];

        for (label, body) in positions {
            let src = format!(
                "fn check(x: int) -> bool requires {{ assert(x <= 10); }} {{ return true; }}\n\
                 fn id(x: bool) -> bool {{ return x; }}\n\
                 fn caller(x: int) -> bool {{ {body} }}"
            );
            // Only the positions that really do contain a call are expected to report.
            // `array-element` and `binary-operand` above are deliberate NEGATIVE controls:
            // they contain no call, so they must report nothing. Keeping them in the same
            // table makes the coverage visible instead of implied.
            let expect_call = !matches!(label, "array-element" | "binary-operand");
            let diags = obligations_for(&src);
            if expect_call {
                assert_eq!(
                    diags.len(),
                    1,
                    "a violating call in `{label}` must be reported, got {diags:?}\n{src}"
                );
            } else {
                assert!(
                    diags.is_empty(),
                    "no call in `{label}`, so no obligation may be reported, got {diags:?}"
                );
            }
        }
    }

    /// A call in an EXPRESSION-position `let` is checked, not just one in statement position.
    ///
    /// `let` exists as both a statement (`StmtKind::Let`) and an expression
    /// (`ExprKind::Let`). The first version of the walker handled only the statement form,
    /// and mutating the expression arm away left every test green -- the arm was dead weight
    /// that looked like coverage.
    #[test]
    fn a_call_in_an_expression_position_let_is_checked() {
        let src = concat!(
            "fn check(x: int) -> bool requires { assert(x <= 10); } { return true; }\n",
            "fn caller(x: int) -> bool { let a = if x > 0 { check(x) } else { true }; return a; }",
        );
        let diags = obligations_for(src);
        assert_eq!(
            diags.len(),
            1,
            "a violating call inside an if-branch must be reported, got {diags:?}"
        );
    }

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

    /// A PRECONDITION turns a refuted bound into a proved one.
    ///
    /// This is the whole point of `requires`. Without it, `t[i] <= 127` is refuted, because
    /// an unconstrained tensor may hold any value. With `requires { forall i in 0..16 {
    /// assert(t[i] <= 127); } }`, the claim becomes a premise rather than a conclusion and
    /// the prover can discharge it.
    ///
    /// It is a genuine discharge, not a tautology: the check asks Z3 for a counterexample to
    /// `pre => goal` and requires that none exists.
    #[test]
    fn a_precondition_makes_a_bound_provable() {
        let src = "fn q(t: [1] Tensor[i8, 16]) \
                   requires { forall i in 0..16 { assert(t[i] <= 127); } } { \
                   proof { forall i in 0..16 { assert(t[i] <= 127); } } return true; }";
        let diags = obligations_for(src);
        assert!(
            diags.is_empty(),
            "the premise should discharge the goal, got {diags:?}"
        );
    }

    /// The precondition must be no STRONGER than the goal, or the prover is unsound in the
    /// other direction. Here the premise bounds elements above and nothing bounds them below,
    /// so the goal must still be refuted.
    #[test]
    fn a_precondition_does_not_prove_a_stronger_goal() {
        let src = "fn q(t: [1] Tensor[i8, 16]) \
                   requires { forall i in 0..16 { assert(t[i] <= 127); } } { \
                   proof { forall i in 0..16 { assert(t[i] >= -128); } } return true; }";
        let diags = obligations_for(src);
        assert_eq!(
            diags.len(),
            1,
            "the premise bounds above only, so the goal must be REFUTED. Got {diags:?}"
        );
        assert_eq!(diags[0].code, OBL_FALSE, "got {diags:?}");
    }

    /// A SCALAR precondition (no quantifier) exercises the `Bool` arm of the
    /// precondition loop, which the tensor tests never reach.
    #[test]
    fn a_scalar_precondition_proves_a_dependent_goal() {
        let src = "fn q(n: int) requires { assert(n <= 0); } { \
                   proof { assert(n <= 10); } return true; }";
        let diags = obligations_for(src);
        assert!(diags.is_empty(), "got {diags:?}");
        // And a goal the premise does NOT imply must still be refuted.
        let src2 = "fn q(n: int) requires { assert(n <= 0); } { \
                    proof { assert(n >= 10); } return true; }";
        let diags2 = obligations_for(src2);
        assert_eq!(diags2.len(), 1, "got {diags2:?}");
        assert_eq!(diags2[0].code, OBL_FALSE, "got {diags2:?}");
    }

    /// THE PRECONDITION MUST BE CONJUNCTIONED POSITIVELY.
    ///
    /// Regression against the exact bug this code originally shipped: a counterexample to
    /// `P => G` is `P and not G`, but the implementation emitted `not P or not G`. The two
    /// differ whenever P is satisfiable, and in a way Z3 exploits trivially -- `not P` is
    /// satisfiable whenever there is ANY tensor violating the premise, so the solver finds a
    /// countermodel for a claim that is in fact true.
    ///
    /// The existing precondition test cannot see this. It uses a premise IDENTICAL to the
    /// goal, and for that pair both spellings happen to be unsatisfiable. What separates
    /// them is a premise that a real tensor can VIOLATE, where the goal still follows:
    ///
    ///     requires { forall i. t[i] <= 0 }
    ///     proof    { forall i. t[i] <= 10 }
    ///
    /// The premise is stronger than the goal, so `P => G` holds and the obligation is
    /// PROVED. Under the broken `not P or not G` spelling, a tensor of all 1s satisfies
    /// `not P`, so the query is SAT and a true claim is reported REFUTED.
    #[test]
    fn a_strong_precondition_proves_a_weaker_goal() {
        let src = "fn q(t: [1] Tensor[i8, 16]) \
                   requires { forall i in 0..16 { assert(t[i] <= 0); } } { \
                   proof { forall i in 0..16 { assert(t[i] <= 10); } } return true; }";
        let diags = obligations_for(src);
        assert!(
            diags.is_empty(),
            "a premise stronger than the goal must discharge it, got {diags:?}"
        );
    }

    /// A precondition that cannot be encoded must REFUSE the obligation.
    ///
    /// Dropping it would prove the goal from a weaker context than written and report the
    /// proof as sound -- the exact failure `requires` must not have.
    ///
    /// `&&` is the unencodable operand: it short-circuits, so it is not `and`, and the
    /// prover refuses rather than silently strengthening the premise.
    #[test]
    fn an_unencodable_precondition_refuses_rather_than_assuming() {
        let src = "fn q(t: [1] Tensor[i8, 16], n: int) \
                   requires { assert(t[0] <= 127 && n > 0); } { \
                   proof { forall i in 0..16 { assert(t[i] <= 127); } } return true; }";
        let diags = obligations_for(src);
        assert!(
            diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "an unencodable precondition must be reported, got {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.code == OBL_FALSE),
            "a refusal must not masquerade as a refutation: {diags:?}"
        );
    }

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
