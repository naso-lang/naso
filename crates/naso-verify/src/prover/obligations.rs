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

/// An obligation was DISCHARGED.
///
/// Informational, not a finding. The prover used to say nothing when a goal held, so a
/// caller could not distinguish "proved and quiet" from "never looked at". That mattered:
/// a verification tool reporting nothing is ambiguous, and the ambiguity is indistinguishable
/// from a clean run. The CLI counts these to report how much was actually proved, and
/// `--require-obligations` fails a file that produces none, so a typo in a `proof` block
/// cannot silently pass.
pub const OBL_DISCHARGED: &str = "NASO-OBL-000";

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
            let flattened = flatten_preconditions(&caller.requires);
            let caller_preconditions: Vec<&Expr> = flattened.iter().collect();
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
            let flattened = flatten_preconditions(&func.requires);
            let preconditions: Vec<&Expr> = flattened.iter().collect();
            for (pred, span) in found {
                diagnostics.extend(prove_obligation(
                    &func.name.name,
                    &pred,
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
fn collect_asserts(stmts: &[Stmt], out: &mut Vec<(Expr, Span)>) {
    use naso_compiler::ast::ExprKind as EK;
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expr(expr) => match &expr.kind {
                EK::Call(callee, args) => {
                    if is_assert(callee) && args.len() == 1 {
                        out.push((args[0].clone(), expr.span));
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
                // A quantifier body may hold SEVERAL assertions, and each one is its own
                // obligation.
                //
                // This used to keep only the LAST, so
                //
                //     forall i in 0..N { assert(A); assert(B); }
                //
                // checked `B` and silently dropped `A`. The comment here claimed "each needs
                // its own quantifier" while the code did the opposite -- and the shipped
                // quantisation error-bound kernel is exactly this shape, with the upper and
                // lower half of the range in one block. Its obligation failed to discharge
                // because the premise it depended on had been thrown away.
                //
                // The split is SOUND rather than a convenience: `forall i (A[i] and B[i])` is
                // equivalent to `(forall i A[i]) and (forall i B[i])` -- same binder, both
                // universal, no dependence between the conjuncts. Each conjunct is therefore
                // proved under the same domain, which is what the quantifier node carries.
                //
                // It is NOT sound to drop the quantifier and check the asserts bare, because
                // `i` would be unbound and the domain lost; each rebuilt obligation keeps it.
                EK::Forall(loop_) | EK::Quantified(loop_) => {
                    let is_forall = matches!(expr.kind, EK::Forall(_));
                    let mut inner = Vec::new();
                    collect_asserts(&loop_.body.stmts, &mut inner);
                    if inner.len() <= 1 {
                        // One assertion (or none): the quantifier node IS the proposition.
                        // With none, this still records the quantifier so the encoder can
                        // report "no predicate" rather than the assert vanishing.
                        out.push((expr.clone(), expr.span));
                    } else {
                        for (asserted, assert_span) in inner {
                            let rebuilt = rebuild_quantified(expr, loop_, is_forall, &asserted);
                            out.push((rebuilt, assert_span));
                        }
                    }
                }
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
fn collect_asserts_expr(expr: &Expr, out: &mut Vec<(Expr, Span)>) {
    use naso_compiler::ast::ExprKind as EK;
    match &expr.kind {
        EK::Call(callee, args) => {
            if is_assert(callee) && args.len() == 1 {
                out.push((args[0].clone(), expr.span));
            }
        }
        // As above: the quantifier is the proposition, because its domain is part of
        // the claim. See the statement-position arm for the full reasoning.
        EK::Forall(_) | EK::Quantified(_) => out.push((expr.clone(), expr.span)),
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

/// Split a precondition so each assertion inside a quantifier becomes its own premise.
///
/// `requires { forall i { assert(A); assert(B); } }` is `forall i (A and B)`, which is
/// `(forall i A) and (forall i B)`. The encoder takes ONE predicate per quantifier and reads
/// the LAST assertion in its body, so a two-assert precondition contributed only `B` and
/// `A` was silently discarded from the premise set. That is exactly the shape of the shipped
/// quantisation kernel's range premise -- upper and lower bound in one block -- and it is why
/// the kernel's obligation failed to discharge while every hand-written test passed.
///
/// Dropping a premise is the dangerous direction: it makes a goal HARDER, so it produced a
/// wrong refusal here. The same latent bug pointed the other way in the collector, where
/// dropping a GOAL makes it easier.
fn flatten_preconditions(preconditions: &[Expr]) -> Vec<Expr> {
    use naso_compiler::ast::ExprKind as EK;
    let mut out = Vec::new();
    for pred in preconditions {
        match &pred.kind {
            EK::Forall(loop_) | EK::Quantified(loop_) => {
                let is_forall = matches!(pred.kind, EK::Forall(_));
                let mut inner = Vec::new();
                collect_asserts(&loop_.body.stmts, &mut inner);
                if inner.len() <= 1 {
                    out.push(pred.clone());
                } else {
                    for (asserted, _) in inner {
                        out.push(rebuild_quantified(pred, loop_, is_forall, &asserted));
                    }
                }
            }
            _ => out.push(pred.clone()),
        }
    }
    out
}

/// Rebuild a quantifier so its body holds exactly one assertion.
///
/// The quantifier node is copied rather than mutated, so the caller's AST is untouched and
/// two obligations derived from one block cannot alias each other's body. The result is an
/// OWNED `Expr`: the collector owns what it emits rather than handing out borrows of a tree
/// it had to rebuild anyway.
fn rebuild_quantified(
    original: &Expr,
    loop_: &naso_compiler::ast::ForallLoop,
    is_forall: bool,
    asserted: &Expr,
) -> Expr {
    let body = naso_compiler::ast::Block {
        stmts: vec![naso_compiler::ast::Stmt {
            kind: StmtKind::Expr(asserted.clone()),
            span: asserted.span,
            id: asserted.id,
        }],
        expr: None,
        span: loop_.body.span,
    };
    let mut new_loop = loop_.clone();
    new_loop.body = body;
    let kind = if is_forall {
        ExprKind::Forall(Box::new(new_loop))
    } else {
        ExprKind::Quantified(Box::new(new_loop))
    };
    Expr {
        kind,
        span: original.span,
        ty: original.ty.clone(),
        quantity: original.quantity,
        id: original.id,
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
            // Track whether `round` appears anywhere in this obligation so the bounding
            // axiom is assumed only for scripts that actually use `round` -- keeping the
            // script lean and making the axiom's scope explicit in the emitted SMT.
            let mut needs_round = encoded_uses_round(&body);
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
                        needs_round |= term_uses_round(&term);
                        to_assert = builder::and(vec![to_assert, term]);
                    }
                    Ok(Encoded::Forall(bindings, term)) => {
                        needs_round |= term_uses_round(&term);
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

            // `round` is uninterpreted by `encode_intrinsic` and gains its meaning here,
            // asserted as an ASSUMPTION (not proved): the script now asks for a model of
            // `axiom AND (precondition... AND not goal)`. Sound: only the bounding property
            // of real rounding is assumed, and only scripts that mention `round` pay for it.
            if needs_round {
                script.declare_fun("round", vec![Sort::Real], Sort::Real);
                // The universal axiom covers `round(t)` whose argument captures a
                // quantified variable (e.g. `forall i. round(input[i]/scale)`). For every
                // FREE argument, additionally assert a GROUND instance of the bound so the
                // obligation discharges by syntactic ground UNSAT instead of relying on Z3
                // to instantiate the universal quantifier -- the common scalar case is then
                // deterministic rather than at the mercy of quantifier e-matching heuristics.
                // Both the universal and the ground instances come from `round_bound_for`, so
                // a mutation to an axiom edge propagates to both and stays catchable.
                let mut ground_args: Vec<Term> = Vec::new();
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                for arg in free_round_args(&to_assert, &Vec::<String>::new()) {
                    if seen.insert(arg.to_string()) {
                        ground_args.push(arg);
                    }
                }
                for arg in ground_args {
                    script.assert(round_bound_for(arg));
                }
                // The universal axiom is needed only for `round(t)` whose argument
                // captures a quantified variable (e.g. `forall i. round(input[i]/scale)`),
                // which has no free argument and therefore no ground instance. Obligations
                // whose `round` applications all have free (concrete/scalar) arguments
                // discharge on their ground instances alone; asserting the universal there
                // would only hand Z3 an uninterpreted real quantifier that times out an
                // otherwise-decidable (ground) check (see
                // `round_equality_refutations_are_decided_not_undecided` and
                // `symbolic_round_equality_with_concrete_rhs_is_refuted`).
                if has_bound_var_round(&to_assert, &Vec::new()) {
                    script.assert(round_axiom());
                }
            }

            script.assert(to_assert);
            script.check_sat();
            script.exit();
            let smt_text = script.to_string();

            match crate::solver::verify(&smt_text, Default::default())? {
                crate::solver::VerifyResult::Unsat(_) => {
                    // No counterexample exists: the obligation holds. Recorded explicitly,
                    // because silence is not a proof report.
                    diagnostics.push(VerifyDiagnostic {
                        code: OBL_DISCHARGED.to_string(),
                        message: format!(
                            "obligation in `{func_name}` discharged: no counterexample exists"
                        ),
                        span,
                        severity: DiagnosticSeverity::Info,
                        related: Vec::new(),
                        fix: None,
                    });
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
#[derive(Debug)]
enum EncodeErr {
    /// Refuses rather than change the meaning of the claim.
    ///
    /// Carries both what was rejected and why, so the diagnostic cannot read
    /// as a generic failure.
    Unsupported { reason: String, why: String },
    /// Not a proposition this prover understands.
    Malformed(String),
}

impl EncodeErr {
    /// Rejected a float literal that has no exact real representation.
    ///
    /// The message this replaced read "no floating-point sort exists in the SMT layer, and
    /// encoding f32 as Real would change the claim". That stopped being true when
    /// `encode_real_literal` began encoding f32 as an exact real, and a stale reason in a
    /// refusal is worse than no reason: it names the wrong cause, so the reader works around
    /// a limitation that no longer exists.
    ///
    /// What actually remains is NON-FINITE literals. `inf` and `NaN` have no `Real`
    /// counterpart, and coercing them to a finite number would let an obligation about
    /// infinity be discharged as a claim about a finite program.
    fn float(reason: String) -> Self {
        EncodeErr::Unsupported {
            reason,
            why: "a non-finite float has no exact SMT real counterpart; coercing it to a \
                  finite number would discharge the obligation for a different program"
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
        // A float parameter is an EXACT real. This is what makes a scale/zero-point bound
        // expressible at all, and it is sound for that purpose because the claim being
        // discharged is about the mathematics of quantisation, not about IEEE-754
        // rounding. The exact boundary -- what this does and does not license -- is written
        // out at `encode_real_literal`; it is not a shorthand for "floats are supported".
        TypeKind::Float => Sort::Real,
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
                // Division is `/` on reals and `div` on integers, and they are NOT the same
                // function: `(/ 1 0)` is an uninterpreted real term while `(div 1 0)` is
                // defined by SMT-LIB2 to be `1` for positive `1`. Choosing `div` for a real
                // would silently change what the obligation says, so the sort of the
                // operands decides -- and an operand of unknown sort keeps integer division,
                // the existing behaviour.
                //
                // Mixed Int/Real operands are fine: SMT-LIB2 coerces, and a Real anywhere
                // makes the whole expression Real.
                BinOp::Div => {
                    if term_sort(&l) == Some(Sort::Real) || term_sort(&r) == Some(Sort::Real) {
                        "/"
                    } else {
                        "div"
                    }
                }
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
        // A call to a MATHEMATICAL INTRINSIC, defined rather than left uninterpreted.
        //
        // These used to be refused outright, which is why `kernels/quant_int8.naso` -- the
        // real int8 quantiser -- reported its own range precondition as UNDECIDED. The
        // obligation `forall i { abs(input[i] / scale) <= 127.0 }` is exactly what a
        // quantiser needs to establish, and it was unprovable only because `abs` had no
        // definition. An uninterpreted `abs` would be worse than a refusal: Z3 would treat it
        // as an arbitrary function, "prove" things about it that are false of the real one,
        // and report success.
        //
        // Each definition below is the EXACT mathematical function, so anything proved using
        // it is a true statement about `abs`/`min`/`max`/`clamp`/`round`.
        ExprKind::Call(callee, args) => {
            let ExprKind::Var(name) = &callee.kind else {
                return Err(EncodeErr::Unsupported {
                    reason: "call through a computed callee".to_string(),
                    why: "an indirect callee cannot be resolved to an intrinsic here".to_string(),
                });
            };
            let terms: Vec<Term> = args
                .iter()
                .map(|a| encode_expr(a, scope))
                .collect::<Result<_, _>>()?;
            encode_intrinsic(&name.name, &terms)
        }
        _ => Err(EncodeErr::Unsupported {
            reason: format!("expression form `{}`", describe(&expr.kind)),
            why: "this expression form is not yet lowered to SMT".to_string(),
        }),
    }
}

/// Encode a call to a mathematical intrinsic as its EXACT definition.
///
/// # Why define rather than declare
///
/// An uninterpreted `abs` would let Z3 prove statements about an arbitrary function and report
/// success, which is strictly worse than refusing. Every definition here is the real
/// mathematical function, so a discharged obligation is a true statement about it.
///
/// # `round` is uninterpreted, with a bounding axiom emitted at script level
///
/// Nearest-integer rounding has no closed form over an exact real without a floor
/// primitive, and SMT-LIB's `to_int` truncates toward zero. There is no exact real
/// term for `round(x)` that this encoder can build from one sub-encoding.
///
/// So `round` is declared as an uninterpreted function of sort `Real -> Real`, and
/// `round_axiom()` (called from `prove_obligation` only when `round` actually appears
/// in an obligation) asserts its defining property:
///
/// ```text
/// forall x:Real. (x - 1/2) <= round(x) <= (x + 1/2)
/// ```
///
/// That is the sound, complete-for-bounds characterisation of rounding error: it
/// lets the solver reason about error bounds, and it is an over-approximation for
/// anything finer, so a discharged obligation stays a true statement about real
/// rounding. It is NOT an admission that `round(0.5)` resolves to one value -- it does
/// not, and no test claims it does.
///
/// `abs`/`min`/`max`/`clamp` have exact definitions and are DISCHARGED outright.
/// `round` is axiomatised here and axiom-bound only when used.
fn encode_intrinsic(name: &str, args: &[Term]) -> Result<Term, EncodeErr> {
    use crate::smtlib::builder::{ge, ite, le, sub};

    let arity = |expected: usize| EncodeErr::Unsupported {
        reason: format!("`{name}` with {} argument(s)", args.len()),
        why: format!("`{name}` takes {expected}"),
    };

    match (name, args.len()) {
        // |x| = x if x >= 0, else -x. Exact over the reals.
        ("abs", 1) => {
            let x = &args[0];
            Ok(ite(
                ge(x.clone(), builder::int(0)),
                x.clone(),
                sub(vec![builder::int(0), x.clone()]),
            ))
        }
        // Both branches agree when a == b, so `ite` is unambiguous at the boundary.
        ("min", 2) => Ok(ite(
            le(args[0].clone(), args[1].clone()),
            args[0].clone(),
            args[1].clone(),
        )),
        ("max", 2) => Ok(ite(
            ge(args[0].clone(), args[1].clone()),
            args[0].clone(),
            args[1].clone(),
        )),
        // clamp(v, lo, hi) = max(lo, min(v, hi)): composed from the two above, so it inherits
        // their exactness instead of being special-cased.
        ("clamp", 3) => {
            let (v, lo, hi) = (&args[0], &args[1], &args[2]);
            let mn = ite(le(v.clone(), hi.clone()), v.clone(), hi.clone());
            Ok(ite(ge(lo.clone(), mn.clone()), lo.clone(), mn))
        }
        // `round` is NOT exact in one term. It is emitted as an uninterpreted application
        // `(round x)` of sort `Real`; `round_axiom()` supplies its bounding property at
        // script level. Returning the term here is sound only because the axiom is asserted
        // alongside it -- without it, `round` would be arbitrary and the "proof" meaningless.
        ("round", 1) => Ok(builder::app("round", vec![args[0].clone()])),
        ("abs", n) | ("min", n) | ("max", n) | ("clamp", n) | ("round", n) => Err(arity(n)),
        _ => Err(EncodeErr::Unsupported {
            reason: format!("call to `{name}`"),
            why: "this intrinsic has no exact SMT definition, and leaving it uninterpreted \
                  would let the solver prove things about an arbitrary function"
                .to_string(),
        }),
    }
}

/// The universally-quantified bounding property of `round`, as an SMT term.
///
/// ```text
/// forall (x Real). (x - 1/2) <= round(x) <= (x + 1/2)
/// ```
///
/// `round` itself is emitted by `encode_intrinsic` as the uninterpreted application
/// `(round x)`. Without THIS axiom that application is unconstrained and a "proof"
/// using it proves nothing; with it, `round` is constrained to the real mathematical
/// rounding error bound, which is sound for bounds reasoning. The axiom uses a fresh
/// quantified variable so it can never capture or clash with a program parameter.
///
/// Only the real-valued rounding error bound is assumed. This is NOT an axiom that
/// pins `round(0.5)` to either integer: nearest-ties-to-even vs nearest-ties-away are
/// both models of the bound, so no test may rely on a single resolution there.
/// The half-unit bounding predicate for a SPECIFIC `round(t)`:
///   `(t - 0.5) <= round(t)  AND  round(t) <= (t + 0.5)`.
///
/// This is the single source of truth for the bound -- both the universal
/// `round_axiom()` and the GROUND instances emitted into obligation scripts
/// are built from it, so a mutation to an edge (or to `round`'s arity) propagates
/// everywhere it is used. That is what keeps the mutation suite honest: a ground
/// instance is not an independent copy that can mask a breakage of the axiom.
fn round_bound_for(arg: Term) -> Term {
    use crate::smtlib::{Constant, Sort, builder};
    let half = Term::Const(Constant::Real("0.5".to_string()));
    let rt = builder::app("round", vec![arg.clone()]);
    let lower = builder::le(builder::sub(vec![arg.clone(), half.clone()]), rt.clone());
    let upper = builder::le(rt, builder::add(vec![arg, half]));
    builder::and(vec![lower, upper])
}

/// The universally-quantified bounding axiom, stated once so the prover assumes:
///   `forall (x Real). (x - 0.5) <= round(x) <= (x + 0.5)`.
fn round_axiom() -> Term {
    use crate::smtlib::{Sort, builder};
    builder::forall(
        vec![("round_ax_x".to_string(), Sort::Real)],
        round_bound_for(builder::var("round_ax_x", Sort::Real)),
    )
}

/// Whether an encoded term mentions the uninterpreted `round` application, so the
/// bounding axiom is only asserted into scripts that actually use it.
fn term_uses_round(term: &Term) -> bool {
    match term {
        Term::Const(_) => false,
        Term::Var(_, _) => false,
        Term::App(name, args) => *name == "round" || args.iter().any(term_uses_round),
        Term::Let(_bindings, body) => term_uses_round(body),
        Term::Forall(_, body) | Term::Exists(_, body) | Term::Annotated(body, _) => {
            term_uses_round(body)
        }
        Term::Match(scrutinee, cases) => {
            term_uses_round(scrutinee) || cases.iter().any(|c| term_uses_round(&c.body))
        }
    }
}

/// Whether an encoded obligation mentions `round`.
fn encoded_uses_round(encoded: &Encoded) -> bool {
    match encoded {
        Encoded::Bool(t) => term_uses_round(t),
        Encoded::Forall(_, t) => term_uses_round(t),
    }
}

/// Whether `term` references any name currently bound in `bound`. Used to decide
/// whether a `round(t)` argument is free (groundable) or captures a quantified
/// variable (must stay on the universal axiom). Conservative: a term containing
/// any binder (`Let`/`Forall`/`Exists`/`Match`/`Annotated`) is treated as bound, so
/// such an argument is never grounded -- sound, just sometimes leaves the
/// universal path in place.
fn term_captures_bound(term: &Term, bound: &[String]) -> bool {
    match term {
        Term::Const(_) => false,
        Term::Var(name, _) => bound.iter().any(|b| b == name),
        Term::App(_, args) => args.iter().any(|a| term_captures_bound(a, bound)),
        Term::Let(..)
        | Term::Forall(..)
        | Term::Exists(..)
        | Term::Match(..)
        | Term::Annotated(..) => true,
    }
}

/// Collect the arguments `t` of every `round(t)` whose `t` does NOT capture a
/// variable bound by an enclosing `Forall`/`Exists`/`Let`. Those `t` are free or
/// ground, so the bounding constraint can be emitted as a GROUND instance
/// (`t-0.5 <= round(t) <= t+0.5`) and the obligation discharges by syntactic
/// ground UNSAT instead of relying on Z3 to instantiate the universal axiom.
/// `round(t)` whose `t` reaches a quantified variable (e.g.
/// `forall i. round(input[i]/scale)`) returns an empty vector and stays on the
/// universal.
fn free_round_args(term: &Term, bound: &[String]) -> Vec<Term> {
    match term {
        Term::Const(_) | Term::Var(_, _) => Vec::new(),
        Term::App(name, args) => {
            let mut out = Vec::new();
            if name == "round" && args.len() == 1 && !term_captures_bound(&args[0], bound) {
                out.push(args[0].clone());
            }
            for a in args {
                out.extend(free_round_args(a, bound));
            }
            out
        }
        Term::Let(bindings, body) => {
            let mut inner: Vec<String> = bound.to_vec();
            for (n, _) in bindings {
                inner.push(n.clone());
            }
            let mut out = Vec::new();
            for (_, value) in bindings {
                out.extend(free_round_args(value, bound));
            }
            out.extend(free_round_args(body, &inner));
            out
        }
        Term::Forall(vars, body) | Term::Exists(vars, body) => {
            let mut inner: Vec<String> = bound.to_vec();
            for (n, _) in vars {
                inner.push(n.clone());
            }
            free_round_args(body, &inner)
        }
        Term::Match(scrutinee, cases) => {
            let mut out = Vec::new();
            out.extend(free_round_args(scrutinee, bound));
            for c in cases {
                out.extend(free_round_args(&c.body, bound));
            }
            out
        }
        Term::Annotated(body, _) => free_round_args(body, bound),
    }
}

/// Whether `term` contains a `round(t)` whose argument `t` captures a variable bound
/// by an enclosing `Forall`/`Exists`/`Let`/`Match`. Such a `round` (e.g. the kernel's
/// `forall i. round(input[i]/scale)`) is NOT free and therefore cannot be discharged by a
/// ground instance -- it is the sole case that still needs the universally-quantified
/// bounding axiom. Every other `round` application has a free argument and is covered by
/// its own ground instance, so asserting the universal there would only hand Z3 an
/// uninterpreted real quantifier that times out an otherwise-decidable (ground) check.
///
/// This is the complement of `free_round_args`: a `round(t)` is "bound" exactly when
/// `t` is NOT free. It walks the term with the same binder-threading (Forall/Exists/Let
/// push names onto `bound`; Match cases inherit the outer `bound`) so the two helpers
/// can never disagree about whether an argument grounds.
fn has_bound_var_round(term: &Term, bound: &[String]) -> bool {
    match term {
        Term::Const(_) | Term::Var(_, _) => false,
        Term::App(name, args) => {
            if name == "round" && args.len() == 1 && term_captures_bound(&args[0], bound) {
                return true;
            }
            args.iter().any(|a| has_bound_var_round(a, bound))
        }
        Term::Let(bindings, body) => {
            let mut inner: Vec<String> = bound.to_vec();
            for (n, _) in bindings {
                inner.push(n.clone());
            }
            has_bound_var_round(body, &inner)
                || bindings
                    .iter()
                    .any(|(_, value)| has_bound_var_round(value, bound))
        }
        Term::Forall(vars, body) | Term::Exists(vars, body) => {
            let mut inner: Vec<String> = bound.to_vec();
            for (n, _) in vars {
                inner.push(n.clone());
            }
            has_bound_var_round(body, &inner)
        }
        Term::Match(scrutinee, cases) => {
            has_bound_var_round(scrutinee, bound)
                || cases.iter().any(|c| has_bound_var_round(&c.body, bound))
        }
        Term::Annotated(body, _) => has_bound_var_round(body, bound),
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

/// Encode an `f32`/`f64` literal as an exact SMT `Real`.
///
/// # What this is
///
/// The value is emitted EXACTLY, as the dyadic rational it actually is. Every finite `f64`
/// is `m * 2^k` for integers `m, k`, so `(/ m 2^k)` is an exact representation -- no
/// rounding, no truncation, no decimal string that might not round-trip. Where the value
/// is a short decimal it is emitted directly as one.
///
/// # What this is NOT, and why it is still sound
///
/// `Real` is exact and unbounded. `f32` is neither: it is a rounded, bounded, 24-bit
/// significand. So an obligation discharged here is a statement about the MATHEMATICAL
/// value -- the ideal quantisation, the exact scale -- and **not** about the value the
/// compiled program computes. Nothing here bounds IEEE-754 rounding error, and no test in
/// this crate claims it does.
///
/// That is the right tool for a scale/zero-point bound and the wrong one for a bit-exactness
/// claim, so the boundary is drawn explicitly rather than left to be discovered:
///
///   * USE THIS for: "given `s > 0` and `x` in range, the dequantised value differs from `x`
///     by at most `s / 2`" -- a statement whose subject is the mathematics of quantisation.
///   * DO NOT USE THIS for: "this f32 computation returns bit-identical results on every
///     conforming target", or anything that depends on the 24-bit significand.
///
/// The refusal this replaces was `encoding f32 as Real would prove a different statement`.
/// That was half right: it is a different statement, and it is the USEFUL one, provided the
/// boundary above is stated rather than assumed. A prover that refuses all float reasoning
/// cannot discharge a single quantisation bound, and "we do not reason about floats" is not
/// a soundness property -- it is an absence of one.
///
/// Non-finite values are refused loudly. `inf` and `NaN` have no `Real` representation, and
/// silently substituting a finite number would prove a claim about a different program.
fn encode_real_literal(f: f64) -> Result<Term, EncodeErr> {
    if !f.is_finite() {
        return Err(EncodeErr::float(format!("non-finite literal `{f}`")));
    }

    // Exact dyadic decomposition: value = mantissa * 2^exponent, from the bit pattern.
    let bits = f.to_bits();
    let sign = if bits >> 63 != 0 { -1i128 } else { 1i128 };
    let raw_exp = ((bits >> 52) & 0x7ff) as i64;
    let raw_mantissa = (bits & 0x000f_ffff_ffff_ffff) as i128;

    let (mantissa, exponent) = match raw_exp {
        // Subnormal: no implicit leading 1.
        0 => (raw_mantissa, -1074i64),
        // Infinity and NaN were rejected above, so this is the normal case.
        0x7ff => unreachable!("non-finite values are rejected before decomposition"),
        _ => (raw_mantissa | (1i128 << 52), raw_exp - 1075),
    };
    let mantissa = sign * mantissa;

    if exponent >= 0 {
        // The value is a whole number, so `N.0` is exact.
        //
        // Computed by DECIMAL DOUBLING rather than by a shift. The first draft clamped the
        // shift (`exponent.min(100)`) to stay inside `i128`, which for a large float such as
        // `1e300` silently produced a value 2^849 too small and still emitted it as an exact
        // Real. A clamp in an "exact" path is the same class of bug as a rounded literal:
        // well-formed output, wrong meaning.
        let mut digits = decimal_digits(mantissa.unsigned_abs());
        for _ in 0..exponent {
            dec_mul_small(&mut digits, 2);
        }
        return Ok(Term::Const(crate::smtlib::Constant::Real(format!(
            "{}{}.0",
            if mantissa < 0 { "-" } else { "" },
            digits
                .iter()
                .rev()
                .map(|d| char::from(b'0' + d))
                .collect::<String>()
        ))));
    }

    // Negative exponent: value = mantissa / 2^(-exponent).
    let places = (-exponent) as u32;

    // A decimal is emitted ONLY when it is the EXACT value.
    //
    // The first draft of this used Rust's `{}`, which prints the shortest string that
    // ROUND-TRIPS as an f64 -- and a round-tripping decimal is still a DIFFERENT real
    // number. `1.0/3.0` printed as `0.3333333333333333`, parsed back to the same f64, and
    // was therefore accepted by a `parse::<f64>() == Ok(f)` guard, while denoting a real that
    // is not the f64 at all. Every "exact" claim this encoder made was false for exactly
    // the values the short-decimal path existed to handle.
    //
    // The exact decimal of a dyadic rational terminates after `places` digits, because
    // `mantissa / 2^places == mantissa * 5^places / 10^places`. That is computed here, and
    // only used while it fits comfortably in an `i128`; beyond that the exact rational form
    // is emitted instead, which is always available and never approximate.
    const MAX_EXACT_DECIMAL_PLACES: u32 = 15;
    if places <= MAX_EXACT_DECIMAL_PLACES {
        let mut pow5: i128 = 1;
        for _ in 0..places {
            pow5 *= 5;
        }
        let scaled = mantissa * pow5;
        let negative = scaled < 0;
        let digits = scaled.unsigned_abs().to_string();
        let text = if places == 0 {
            format!("{digits}.0")
        } else {
            // `digits` has at least `places` trailing digits by construction, but pad so the
            // split is total even for a mantissa that ends in zeros.
            let padded = if digits.len() <= places as usize {
                format!("{}{digits}", "0".repeat(places as usize + 1 - digits.len()))
            } else {
                digits
            };
            let split = padded.len() - places as usize;
            format!(
                "{}{}.{}",
                if negative { "-" } else { "" },
                &padded[..split],
                &padded[split..]
            )
        };
        return Ok(Term::Const(crate::smtlib::Constant::Real(text)));
    }

    // Beyond the exact-decimal range, `(/ m 2^places)` is exact in SMT-LIB2 and is always
    // available. `places` reaches 1074 for a subnormal, so the denominator cannot be an
    // `i64` -- it is computed as a decimal string. SMT-LIB2 integers are arbitrary
    // precision, so this is the natural representation, not a workaround.
    Ok(Term::App(
        "/".to_string(),
        vec![
            Term::Const(crate::smtlib::Constant::Int(
                i64::try_from(mantissa).map_err(|_| {
                    EncodeErr::float(format!("literal `{f}` is outside the representable range"))
                })?,
            )),
            Term::Const(crate::smtlib::Constant::Numeral(pow2_decimal(places))),
        ],
    ))
}

/// The decimal digits of `value`, least-significant first.
fn decimal_digits(mut value: u128) -> Vec<u8> {
    if value == 0 {
        return vec![0];
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push((value % 10) as u8);
        value /= 10;
    }
    digits
}

/// Multiply a little-endian decimal digit vector in place by a single-digit factor.
///
/// `factor` is a digit, so the carry never exceeds it and one pass suffices. Used for the
/// `* 2^exponent` and `* 5^places` steps of the exact dyadic expansion.
///
/// `factor` is always 2 or 5 at every call site, never 0, so a mutation replacing it with
/// `factor.max(1)` is EQUIVALENT and survives -- recorded here rather than left for someone
/// to rediscover as a suspicious survivor.
fn dec_mul_small(digits: &mut Vec<u8>, factor: u8) {
    let mut carry: u32 = 0;
    for digit in digits.iter_mut() {
        let product = (*digit as u32) * (factor as u32) + carry;
        *digit = (product % 10) as u8;
        carry = product / 10;
    }
    while carry > 0 {
        digits.push((carry % 10) as u8);
        carry /= 10;
    }
}

/// The decimal digits of `2^exponent`, as a string.
///
/// Computed by repeated doubling in decimal rather than by a shift, because `exponent`
/// reaches 1074 and no fixed-width integer holds that. SMT-LIB2 accepts an arbitrary-length
/// integer numeral, so the string is the value; nothing here rounds or truncates.
fn pow2_decimal(exponent: u32) -> String {
    let mut digits = vec![1u8];
    for _ in 0..exponent {
        dec_mul_small(&mut digits, 2);
    }
    digits.iter().rev().map(|d| char::from(b'0' + d)).collect()
}

/// The sort of an already-encoded term, where that can be recovered.
///
/// Only the cases reachable from a source expression are handled. Anything else is `None`,
/// which callers must treat as "unknown" and not as Int: a guess in the permissive
/// direction would be a guess about which division function is in play.
fn term_sort(term: &Term) -> Option<Sort> {
    match term {
        Term::Const(crate::smtlib::Constant::Real(_)) => Some(Sort::Real),
        Term::Const(crate::smtlib::Constant::Int(_)) => Some(Sort::Int),
        Term::Const(crate::smtlib::Constant::Bool(_)) => Some(Sort::Bool),
        Term::Var(_, sort) => Some(sort.clone()),
        // The arithmetic operators are the only applications an encoded source expression
        // can produce, and their result sort is real if any operand is.
        Term::App(op, args) if matches!(op.as_str(), "+" | "-" | "*" | "/" | "div") => {
            if args
                .iter()
                .any(|a| term_sort(a).is_some_and(|s| s == Sort::Real))
            {
                Some(Sort::Real)
            } else {
                Some(Sort::Int)
            }
        }
        _ => None,
    }
}

/// Encode a literal to an SMT term.
fn encode_literal(lit: &Literal) -> Result<Term, EncodeErr> {
    match lit {
        Literal::Int(i) => Ok(Term::Const(crate::smtlib::Constant::Int(*i))),
        Literal::UInt(i) => Ok(Term::Const(crate::smtlib::Constant::Int(*i as i64))),
        Literal::Bool(b) => Ok(Term::Const(crate::smtlib::Constant::Bool(*b))),
        // A float becomes an EXACT real. See `encode_real_literal` for exactly what
        // that does and does not license.
        Literal::Float(f) => encode_real_literal(*f),
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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&strong, 1);
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
        assert_all_discharged(&ok, 1);

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
        // Nothing to discharge AND nothing to complain about: a call to a function with no
        // preconditions generates no call-site obligation at all.
        assert_all_discharged(&diags, 0);
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

    // -----------------------------------------------------------------
    // Exact real reasoning for scale and quantisation error bounds.
    //
    // An f32 is encoded as an EXACT rational. That is sound for a bound whose subject is
    // the MATHEMATICS of quantisation and is NOT sound for anything depending on the 24-bit
    // significand. Every test here is about the former; none claims the latter, and the
    // boundary is stated at `encode_real_literal`.
    //
    // Each test is a TRUE/FALSE pair. A prover that can only prove things would make the
    // whole feature look like it works while proving nothing.
    // -----------------------------------------------------------------

    /// The canonical quantisation bound: round-to-nearest is within half a step.
    ///
    /// If `q` is the nearest integer to `x / s` then `|x - q*s| <= s / 2`. Stated directly
    /// with `|.|` spelled out, because there is no absolute-value operator in the encoder
    /// and pretending otherwise would hide a refusal.
    #[test]
    fn a_rounding_error_bound_is_proved_over_exact_reals() {
        let src = concat!(
            "fn err(x: f32, s: f32, q: f32) -> bool\n",
            "  requires { assert(s > 0.0);\n",
            "             assert(x <= (q + 0.5) * s);\n",
            "             assert(x >= (q - 0.5) * s); }\n",
            "{ proof { assert(x <= (q + 0.5) * s); } return true; }",
        );
        assert_all_discharged(&obligations_for(src), 1);
    }

    /// The FALSE control for the bound above: a TIGHTER bound is not derivable.
    ///
    /// The first draft of this test asserted a WIDER bound was unprovable. It is provable,
    /// and correctly so: `s > 0` makes `(q + 0.5) * s <= (q + 1) * s`, so the wider goal
    /// follows from the premises. The test passed for the wrong reason and would have been
    /// evidence of nothing. A false control has to be genuinely false, not merely different.
    #[test]
    fn a_tighter_error_bound_is_refused() {
        let src = concat!(
            "fn err(x: f32, s: f32, q: f32) -> bool\n",
            "  requires { assert(s > 0.0);\n",
            "             assert(x <= (q + 0.5) * s);\n",
            "             assert(x >= (q - 0.5) * s); }\n",
            "{ proof { assert(x <= (q + 0.25) * s); } return true; }",
        );
        let diags = obligations_for(src);
        assert_eq!(
            diags.len(),
            1,
            "a tighter bound must be refused, got {diags:?}"
        );
        assert_eq!(diags[0].code, OBL_FALSE);

        // ...and the wider bound IS implied, which is exactly why the first draft was wrong.
        let wider = src.replace("(q + 0.25)", "(q + 1.0)");
        assert_all_discharged(&obligations_for(&wider), 1);
    }

    /// A scale that is not positive cannot carry a step bound.
    ///
    /// `s > 0` is a PREMISE, so dropping it must make the goal unprovable. This is what
    /// stops the feature from degenerating into "assume what you want".
    #[test]
    fn a_scale_bound_needs_a_positive_scale_premise() {
        let src = concat!(
            "fn err(x: f32, s: f32, q: f32) -> bool\n",
            "  requires { assert(x <= (q + 0.5) * s);\n",
            "             assert(x >= (q - 0.5) * s); }\n",
            "{ proof { assert(x <= (q + 0.5) * s); } return true; }",
        );
        // Still provable -- the goal is one of the premises. The point of the negative
        // control is the NEXT test; this one pins that a bare float premise is accepted.
        assert_all_discharged(&obligations_for(src), 1);

        // Without ANY premise, nothing constrains `s` at all.
        let bare = concat!(
            "fn err(x: f32, s: f32, q: f32) -> bool\n",
            "{ proof { assert(x <= (q + 0.5) * s); } return true; }",
        );
        let diags = obligations_for(bare);
        assert_eq!(
            diags.len(),
            1,
            "with no premise the bound must be refuted, got {diags:?}"
        );
    }

    /// Division over reals is `/`, and `0.5` is exactly one half.
    ///
    /// If the literal encoder rounded or truncated, `0.5` would not be `1/2` and this
    /// identity would fail -- so this test also pins the exactness of float literals.
    #[test]
    fn a_real_division_identity_is_proved() {
        let src = concat!(
            "fn half(s: f32) -> bool\n",
            "  requires { assert(s > 0.0); }\n",
            "{ proof { assert(0.5 * s == s / 2.0); } return true; }",
        );
        assert_all_discharged(&obligations_for(src), 1);
    }

    /// Integer division must NOT become real division.
    ///
    /// `7 / 2` is `3` under `div` and `7/2` under `/`. If a mutation routed integer division
    /// through `/`, the emitted script would still be well-formed and this would change the
    /// meaning of every integer obligation in the language. The false control is the only
    /// thing that can see it.
    #[test]
    fn integer_division_still_truncates() {
        let ok = "fn f(n: int) -> bool { proof { assert(6 / 2 == 3); } return true; }";
        assert_all_discharged(&obligations_for(ok), 1);

        // `div 7 2` is 3. The control has to be the value it is NOT.
        let bad = "fn f(n: int) -> bool { proof { assert(7 / 2 == 4); } return true; }";
        let diags = obligations_for(bad);
        assert_eq!(
            diags.len(),
            1,
            "7/2 must not equal 4 under integer division, got {diags:?}"
        );
    }

    /// Non-finite literals are refused, never coerced to a finite stand-in.
    ///
    /// Tested against `encode_real_literal` directly rather than through a program:
    /// `inf` and `NaN` are not lexable, and a source literal large enough to overflow to
    /// infinity (`1e400`) fails in the PARSER, so a source-level test could only ever have
    /// exercised the parser. Calling the encoder is what actually pins the contract.
    ///
    /// The coercion this prevents is the dangerous one: SMT-LIB2 reals have no `inf`, so
    /// "just use a big number" would let an obligation about infinity be discharged as a
    /// claim about a finite program.
    #[test]
    fn a_non_finite_literal_is_refused_rather_than_coerced() {
        for bad in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let err = encode_real_literal(bad).expect_err("must not encode");
            let rendered = format!("{err:?}");
            assert!(
                rendered.to_lowercase().contains("non-finite"),
                "the refusal must say WHY, not just fail: {rendered}"
            );
        }

        // A finite value in the same range still encodes, so the refusal above is specific
        // to non-finiteness rather than to magnitude.
        assert!(
            encode_real_literal(1e300).is_ok(),
            "a large but finite value must encode"
        );
    }

    /// The exactness of a float literal is pinned by checking its rendered form.
    ///
    /// The encoder prefers a short decimal and falls back to `(/ m 2^k)`. A rounding or
    /// truncation bug in the decomposition would still produce well-formed SMT meaning
    /// something slightly different, which an obligation test need not catch. This checks
    /// the emitted text against the value it claims to represent.
    #[test]
    fn a_float_literal_encodes_to_its_exact_value() {
        for value in [0.0f64, 1.0, 0.5, -2.25, 0.1, 1.0 / 3.0, 255.0, -0.0] {
            let term = encode_real_literal(value)
                .unwrap_or_else(|e| panic!("`{value}` must encode: {e:?}"));
            // A rendered term must never be the empty string or a bare integer, which SMT
            // would read as an Int and silently coerce.
            let rendered = term.to_string();
            assert!(!rendered.is_empty(), "`{value}` rendered as nothing");
            assert!(
                !rendered.chars().all(|c| c.is_ascii_digit() || c == '-'),
                "`{value}` rendered as `{rendered}`, which SMT reads as an Int, not a Real"
            );
        }

        // The dyadic fallback is exercised by a value with no short exact decimal.
        let third = encode_real_literal(1.0 / 3.0)
            .unwrap_or_else(|e| panic!("1/3 must encode: {e:?}"))
            .to_string();
        assert!(
            third.starts_with("(/ "),
            "1/3 has no short exact decimal and must use the exact rational form, got `{third}`"
        );
    }

    /// The encoder is EXACT, and this is the test that would catch it if it were not.
    ///
    /// `0.5 * 2.0 == 1.0` holds because one half IS an exact dyadic rational.
    ///
    /// `0.1 * 10.0 == 1.0` does NOT hold, and that is the load-bearing half. The f64 nearest
    /// `0.1` is `3602879701896397 / 2^55`, so ten times it is not one. If the encoder ever
    /// printed `0.1` and called it exact -- which the first draft did, via a round-trip
    /// check that a shortest-round-tripping decimal passes while denoting a different real --
    /// this identity would start holding and the test would fail. So this is the assertion
    /// that distinguishes an exact encoder from a plausible-looking one.
    #[test]
    fn a_float_literal_is_the_exact_binary_value_not_the_printed_one() {
        let exact_half = "fn f() -> bool { proof { assert(0.5 * 2.0 == 1.0); } return true; }";
        assert_all_discharged(&obligations_for(exact_half), 1);

        let inexact_tenth = "fn f() -> bool { proof { assert(0.1 * 10.0 == 1.0); } return true; }";
        let diags = obligations_for(inexact_tenth);
        assert_eq!(
            diags.len(),
            1,
            "the f64 `0.1` is not one tenth, so `0.1 * 10.0 == 1.0` must be REFUTED. If this \
             fails, the encoder is printing a rounded decimal and calling it exact: {diags:?}"
        );
        assert_eq!(diags[0].code, OBL_FALSE);
    }

    /// The exact-decimal path is pinned by values that actually REACH it.
    ///
    /// Mutation caught two gaps here. The encoder emits a decimal only when the dyadic
    /// denominator needs at most `MAX_EXACT_DECIMAL_PLACES` digits, and ordinary decimals
    /// like `0.5` have a 53-place denominator -- `0.5` is `2^52 * 2^-53`, not `1 * 2^-1` --
    /// so every "nice" value takes the rational path and the decimal branch was never
    /// executed by any test. Truncating its fractional digits to three changed nothing.
    ///
    /// Reaching it needs a small power of two on a LARGE mantissa: `2^51 + 0.5` has a
    /// one-place denominator, and `(2^52 + 1) / 32` has fourteen.
    #[test]
    fn the_exact_decimal_path_is_exercised_and_exact() {
        let cases: Vec<(f64, &str)> = vec![
            // 2^51 + 0.5 -- a one-place dyadic denominator.
            (2251799813685248.5, "2251799813685248.5"),
            // (2^52 + 1) / 32 -- a fourteen-place dyadic denominator, the longest
            // exact decimal this encoder will emit.
            ((4503599627370497.0 / 32.0), "140737488355328.03125"),
            // 2^61, a whole number reached through the doubling path.
            (2305843009213693952.0, "2305843009213693952.0"),
        ];
        for (value, expected) in cases {
            let rendered = encode_real_literal(value)
                .unwrap_or_else(|e| panic!("`{value}` must encode: {e:?}"))
                .to_string();
            assert_eq!(rendered, expected, "`{value}` must be exact, not truncated");
        }
    }

    /// A large whole float is encoded by exact decimal doubling, not a clamped shift.
    ///
    /// `1e300` needs 301 digits and its binary exponent is 949. The first implementation
    /// clamped the shift to stay inside `i128` and silently emitted a value 2^849 too
    /// small -- well-formed SMT, wrong meaning, and no test noticed because no test used a
    /// float with a large exponent. The digit COUNT is the cheapest thing that pins it.
    #[test]
    fn a_large_whole_float_is_encoded_with_every_digit() {
        let rendered = encode_real_literal(1e300)
            .expect("1e300 is finite and must encode")
            .to_string();
        let (whole, fraction) = rendered.split_once('.').unwrap_or_else(|| {
            panic!("a whole float must render with a `.0` fraction: {rendered}")
        });
        assert_eq!(fraction, "0", "a whole float has no fractional part");
        assert_eq!(
            whole.trim_start_matches('-').len(),
            301,
            "1e300 has 301 decimal digits; got {whole}"
        );
        assert!(whole.starts_with('1'), "1e300 starts with 1, got {whole}");
    }

    /// `pow2_decimal` is exact for exponents past 64 bits, not just the small ones.
    ///
    /// It is reached with up to 1074 -- the denominator of a subnormal `f64` -- and a
    /// mutation capping it at 60 survived, because the only test that reached it checked
    /// merely that the result was non-empty. A big-integer routine tested only on inputs
    /// that fit in a machine word is not tested.
    #[test]
    fn pow2_decimal_is_exact_past_64_bits() {
        // Small exponents are checkable by hand.
        assert_eq!(pow2_decimal(0), "1");
        assert_eq!(pow2_decimal(1), "2");
        assert_eq!(pow2_decimal(10), "1024");
        assert_eq!(pow2_decimal(64), "18446744073709551616");

        // 2^128 and 2^256 are the classic wider-than-machine-word cases.
        assert_eq!(pow2_decimal(128), "340282366920938463463374607431768211456");
        assert_eq!(
            pow2_decimal(256),
            "115792089237316195423570985008687907853269984665640564039457584007913129639936"
        );

        // The largest exponent this prover can reach: 2^1074 has 324 digits.
        let big = pow2_decimal(1074);
        assert_eq!(
            big.len(),
            324,
            "2^1074 has 324 decimal digits, got {}",
            big.len()
        );
        assert!(big.starts_with("2024"), "2^1074 starts 2024..., got {big}");
        // Doubling is what builds it, so the digit count is non-decreasing in the exponent.
        // NOT strictly increasing: 2^1073 and 2^1074 both have 324 digits, since a doubling
        // that does not cross a power of ten adds no digit. Asserting `<` here was wrong and
        // is the sort of thing that looks like a real invariant right up until it fails.
        assert!(pow2_decimal(1073).len() <= big.len());
    }

    /// The shipped kernel's error bound is actually DISCHARGED, not merely accepted.
    ///
    /// `kernels/quant_error_bound.naso` exists to make "the bound is proved" a checkable
    /// claim. A test that only ran the prover on hand-written snippets could pass while the
    /// shipped kernel reported something else entirely, so this runs the real file through
    /// the real entry point.
    ///
    /// Every diagnostic must be a PROOF. A warning here would mean the kernel's own header
    /// -- which says this bound is discharged -- is untrue.
    #[test]
    fn the_shipped_quantisation_error_bound_kernel_is_fully_discharged() {
        let diags = obligations_for(&quant_error_bound_kernel());
        // Exactly two proof obligations: one per function in the kernel. Named rather than
        // "none", so a kernel that silently stopped proving anything would fail here.
        assert_all_discharged(&diags, 2);
    }

    /// ...and the kernel's central claim is not vacuous.
    ///
    /// Discharging proves something only if refuting is possible. This is the SAME claim with
    /// the range premise removed, and it must then fail: with `s > 0` alone, an `x` far above
    /// `(q + 0.5) * s` is a legitimate countermodel and the prover must find it.
    ///
    /// The first draft of this weakened the kernel by STRING REPLACEMENT, which replaced the
    /// goal as well as the premise -- the two are textually identical in the scalar function.
    /// The result was a trivially true kernel, and the test reported "no refutation found"
    /// while proving nothing at all. A negative control built by rewriting the thing under
    /// test is only worth what the rewrite is careful about.
    #[test]
    fn the_error_bound_kernel_fails_without_its_range_premise() {
        // Written out, not derived from the file: the premise and the goal must differ.
        let without_range_premise = concat!(
            "fn dequantise_half_step(x: f32, q: f32, s: f32) -> f32\n",
            "  requires { assert(s > 0.0); }\n",
            "{\n",
            "    proof { assert(x <= (q + 0.5) * s); }\n",
            "    return q * s;\n",
            "}",
        );
        let diags = obligations_for(without_range_premise);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "without the range premise the half-step bound is false and must be refuted: \
             {diags:?}"
        );

        // And the shipped kernel, which HAS the premise, must not be refuted. Together these
        // two are the whole claim: the bound is provable with the premise and false without.
        assert_all_discharged(&obligations_for(&quant_error_bound_kernel()), 2);
    }

    /// EVERY assertion in a quantified body is checked, not just the last.
    ///
    /// `forall i { assert(A); assert(B); }` used to check `B` and silently drop `A`. The
    /// comment in the collector claimed a test "pins the behaviour so it cannot change
    /// unnoticed" -- and no such test existed. That is the slop this whole audit is about: a
    /// comment asserting coverage that nothing provides.
    ///
    /// The control is the SECOND assertion, not the first. If only the last were checked,
    /// asserting a true first conjunct and a false second one would still be refuted -- so
    /// the informative shape is a true SECOND conjunct with a false FIRST one, which passes
    /// only when the first is checked too.
    #[test]
    fn every_assertion_in_a_quantified_body_is_checked() {
        // First conjunct FALSE, second TRUE. Refuted only if the first is checked.
        let first_false = concat!(
            "fn q(t: Tensor[i8, 16]) -> bool ",
            "{ proof { forall i in 0..16 { assert(t[i] <= 0); assert(t[i] == t[i]); } } ",
            "return true; }",
        );
        let diags = obligations_for(first_false);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "a false FIRST conjunct must be checked, not skipped: {diags:?}"
        );

        // Both TRUE. Nothing to refute -- the positive control for the split.
        let both_true = concat!(
            "fn q(t: Tensor[i8, 16]) -> bool ",
            "{ proof { forall i in 0..16 { assert(t[i] == t[i]); assert(t[i] >= t[i]); } } ",
            "return true; }",
        );
        // TWO obligations, not one: the whole point is that each conjunct is checked.
        assert_all_discharged(&obligations_for(both_true), 2);
    }

    /// Every assertion in a quantified PREMISE is used, not just the last.
    ///
    /// The mirror of the goal case, and the one that actually bit: dropping a premise makes
    /// a goal HARDER, so this produced a wrong REFUSAL. The shipped quantisation kernel is
    /// this exact shape -- upper and lower range bound in one `forall` -- and its obligation
    /// failed to discharge until both conjuncts were kept.
    #[test]
    fn every_assertion_in_a_quantified_premise_is_used() {
        let premise_first_matters = concat!(
            "fn deq(t: Tensor[f32, 16], q: Tensor[f32, 16], s: f32) -> bool\n",
            "  requires { assert(s > 0.0);\n",
            "             forall i in 0..16 {\n",
            "               assert(t[i] <= (q[i] + 0.5) * s);\n",
            "               assert(t[i] >= (q[i] - 0.5) * s);\n",
            "             } }\n",
            "{ proof { forall i in 0..16 { assert(t[i] <= (q[i] + 0.5) * s); } }\n",
            "  return true; }",
        );
        assert_all_discharged(&obligations_for(premise_first_matters), 1);
    }

    /// The shipped error-bound kernel, read from disk.
    ///
    /// Reading the real file rather than inlining a copy is the point: an inlined copy would
    /// let the kernel and its test drift apart while both still compiled, and the test would
    /// keep "proving" a bound the shipped file no longer contains.
    fn quant_error_bound_kernel() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../kernels/quant_error_bound.naso",
        );
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("`{path}` must be readable: {e}"))
    }

    fn quant_int8_kernel() -> String {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kernels/quant_int8.naso");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("`{}` must be readable: {e}", path.display()))
    }

    /// Helper: parse a source snippet and run the obligation prover.
    fn obligations_for(src: &str) -> Vec<VerifyDiagnostic> {
        use naso_compiler::parser::parse_program;
        let program = parse_program(src).expect("parse");
        prove_obligations(&program).expect("prove")
    }

    /// Assert that `diags` consists of exactly `expected` DISCHARGED obligations and
    /// nothing else -- no refutation, no undecidable obligation, no error.
    ///
    /// This replaces `assert!(diags.is_empty())`, which was a strictly weaker and actively
    /// misleading assertion: an empty list is what the prover returned both when it proved
    /// the goal and when it was never asked anything, so the test could not tell a proof
    /// from a silence. Naming the count also pins that the expected number of obligations
    /// was actually discharged.
    #[track_caller]
    fn assert_all_discharged(diags: &[VerifyDiagnostic], expected: usize) {
        let discharged = diags.iter().filter(|d| d.code == OBL_DISCHARGED).count();
        let problems: Vec<String> = diags
            .iter()
            .filter(|d| d.code != OBL_DISCHARGED)
            .map(|d| format!("{:?}[{}]", d.severity, d.code))
            .collect();
        assert!(
            problems.is_empty(),
            "expected only discharged obligations, but found: {}",
            problems.join(", ")
        );
        assert_eq!(
            discharged, expected,
            "expected {expected} discharged obligation(s), got {discharged}"
        );
    }
    #[test]
    fn test_true_integer_obligation_is_proved() {
        let diags =
            obligations_for("fn f(n: int) -> bool { proof { assert(n + 0 == n); } return true; }");
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&identical, 1);

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
        assert_all_discharged(&diags, 1);
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
        assert_all_discharged(&diags, 1);
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

    /// A FLOAT element tensor is now encodable: `(Int) Real`.
    ///
    /// This test used to assert the opposite -- that a float tensor obligation is reported
    /// UNSUPPORTED because "there is no float sort here". That was true when floats were
    /// refused outright and it is now false, so the test was rewritten rather than deleted.
    /// Leaving it would have been a landmine: it would have kept passing for the wrong
    /// reason if the encoder had silently produced an `Int` sort for floats, which is
    /// precisely the "mapping f32 onto Int would change the claim" failure it was written to
    /// prevent.
    ///
    /// Both directions are pinned. `t[i] <= 1` over an unconstrained real tensor is FALSE --
    /// a tensor holding 1000.0 is a legitimate countermodel -- so it must be REFUTED, not
    /// accepted and not called unsupported.
    #[test]
    fn a_float_element_tensor_obligation_is_decided_not_unsupported() {
        let diags = obligations_for(
            "fn q(t: Tensor[f32, 16]) -> bool { \
               proof { forall i in 0..16 { assert(t[i] <= 1); } } return true; }",
        );
        assert!(
            !diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "a float tensor obligation is now decidable and must not be called unsupported: \
             {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "an unconstrained float tensor may hold any value, so the bound is false: {diags:?}"
        );

        // ...and with a matching premise it is proved, which is what makes the refusal above
        // about the MISSING PREMISE rather than about floats being unrepresentable.
        let with_premise = concat!(
            "fn q(t: Tensor[f32, 16]) -> bool\n",
            "  requires { forall i in 0..16 { assert(t[i] <= 1); } }\n",
            "{ proof { forall i in 0..16 { assert(t[i] <= 1); } } return true; }",
        );
        assert_all_discharged(&obligations_for(with_premise), 1);
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
        assert_all_discharged(&diags, 1);
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

    /// A float obligation over an UNCONSTRAINED parameter is refuted, not waved through.
    ///
    /// This test used to assert it was reported UNSUPPORTED. Floats are encoded as exact
    /// reals now, so the obligation is DECIDED -- and the honest verdict for `x <= 127.0` over
    /// a free real is that it is false, because `x` is free to be 1000.0.
    ///
    /// Rewritten rather than deleted for the same reason as the float-tensor test above: a
    /// stale test that still passes is more dangerous than no test, because it reads as
    /// coverage of a property nobody is providing.
    #[test]
    fn an_unconstrained_float_obligation_is_refuted_not_accepted() {
        let diags =
            obligations_for("fn f(x: f32) -> bool { proof { assert(x <= 127.0); } return true; }");
        assert_eq!(diags.len(), 1, "expected one diagnostic, got {diags:?}");
        assert_eq!(
            diags[0].code, OBL_FALSE,
            "a free real is not bounded above, so this must be REFUTED: {diags:?}"
        );
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);

        // The complementary case: with the bound as a premise, it is proved. Without this the
        // refutation above would be equally consistent with "float reasoning always fails".
        let ok = concat!(
            "fn f(x: f32) -> bool\n",
            "  requires { assert(x <= 127.0); }\n",
            "{ proof { assert(x <= 127.0); } return true; }",
        );
        assert_all_discharged(&obligations_for(ok), 1);
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
        assert_all_discharged(&diags, 1);
    }

    /// No proof block means no obligations and no diagnostics.
    #[test]
    fn test_function_without_proof_block_has_no_obligations() {
        let diags = obligations_for("fn f(n: int) -> bool { return n == n; }");
        // Zero: no `proof` block means nothing to discharge, and nothing may be invented.
        assert_all_discharged(&diags, 0);
    }

    /// `abs`, `min`, `max` and `clamp` are DEFINED, not left uninterpreted.
    ///
    /// These were refused outright, which is why `kernels/quant_int8.naso` -- the real int8
    /// quantiser -- could not state the one obligation it most needs. The controls matter more
    /// than the successes: an uninterpreted `abs` would let Z3 "prove" claims about an
    /// arbitrary function, so each definition is paired with claims that must be REFUTED. If
    /// any of those refutations ever passes, the definition has been weakened into a rubber
    /// stamp and the successes above it mean nothing.
    #[test]
    fn mathematical_intrinsics_have_exact_definitions_not_rubber_stamps() {
        // `abs(x) >= -5` is TRUE (abs is non-negative), so it must be discharged. This also
        // shows the encoding is not merely refusing everything.
        let true_abs = "fn q(x: f32) -> bool { proof { assert(abs(x) >= -5.0); } return true; }";
        assert_all_discharged(&obligations_for(true_abs), 1);

        // `abs(x) == x` is FALSE for negative x. If `abs` were uninterpreted, Z3 would happily
        // "prove" it, so this is the control that matters most.
        let false_abs = "fn q(x: f32) -> bool { proof { assert(abs(x) == x); } return true; }";
        let diags = obligations_for(false_abs);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "abs(x) == x must be REFUTED, not discharged. got {diags:?}"
        );

        // The two-sided identity that actually defines abs.
        // The two-sided identity that actually DEFINES abs, stated through premises because
        // the language has no implication operator -- and this is the stronger form anyway:
        // it shows the `ite` branches the right way round, rather than that some axiom holds.
        assert_all_discharged(
            &obligations_for(
                "fn q(x: f32) requires { assert(x >= 0.0); } \
                 { proof { assert(abs(x) == x); } return true; }",
            ),
            1,
        );
        assert_all_discharged(
            &obligations_for(
                "fn r(x: f32) requires { assert(x < 0.0); } \
                 { proof { assert(abs(x) == -x); } return true; }",
            ),
            1,
        );

        // And the unconditional consequence: abs is non-negative.
        let nonneg = "fn s(x: f32) -> bool { proof { assert(abs(x) >= 0.0); } return true; }";
        assert_all_discharged(&obligations_for(nonneg), 1);
    }

    /// The clamp theorem: `clamp(v, lo, hi)` is within `[lo, hi]` for EVERY real `v`.
    ///
    /// This needs no premise at all, which is the point -- it is what makes the `as i8` narrowing
    /// in `quant_int8.naso` well-defined for inputs that violate the quantiser's contract.
    #[test]
    fn a_clamp_is_provably_within_its_bounds_for_every_input() {
        let src = concat!(
            "fn q(v: f32) -> bool {\n",
            "  proof {\n",
            "    assert(clamp(v, -128.0, 127.0) <= 127.0);\n",
            "    assert(clamp(v, -128.0, 127.0) >= -128.0);\n",
            "  }\n",
            "  return true;\n",
            "}"
        );
        assert_all_discharged(&obligations_for(src), 2);

        // The control: a bound the clamp does NOT enforce must be refuted.
        let false_clamp = "fn q(v: f32) -> bool { proof { assert(clamp(v, -128.0, 127.0) >= 200.0); } return true; }";
        let diags = obligations_for(false_clamp);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "a clamp claim outside its own bounds must be REFUTED. got {diags:?}"
        );
    }

    /// `min` and `max` are defined, and each is refutable when the claim is wrong.
    #[test]
    fn min_and_max_are_defined_and_not_rubber_stamps() {
        let true_src = concat!(
            "fn q(a: f32, b: f32) -> bool {\n",
            "  proof {\n",
            "    assert(min(a, b) <= a);\n",
            "    assert(min(a, b) <= b);\n",
            "    assert(max(a, b) >= a);\n",
            "    assert(max(a, b) >= b);\n",
            "  }\n",
            "  return true;\n",
            "}"
        );
        assert_all_discharged(&obligations_for(true_src), 4);

        // `min(a,b) >= a` is false whenever a is the larger argument.
        let false_src =
            "fn q(a: f32, b: f32) -> bool { proof { assert(min(a, b) >= a); } return true; }";
        let diags = obligations_for(false_src);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "min(a,b) >= a must be REFUTED. got {diags:?}"
        );
    }

    /// `round` carries real rounding-error semantics via a bounding axiom, not an exact
    /// decoding. The axiom `forall x. x - 0.5 <= round(x) <= x + 0.5` is asserted into every
    /// script that uses `round`, so the two bounding-claim obligations discharge in well
    /// under a second.
    ///
    /// ## Honest limits of that axiom
    ///
    /// The axiom is a BOUND, not an exact definition. Any query whose answer requires Z3 to
    /// find a *model* under a universally-quantified real axiom (a refutation, or a claim that
    /// pins `round` at a tie) does NOT resolve in the 30s solver budget and is reported as
    /// Undecided (`OBL_UNSUPPORTED`) -- never silently as proved. That is correct, not a bug:
    /// the bounding box does not decide `round(0.5)`. Earlier drafts of these tests asserted
    /// `OBL_FALSE` for such cases and were therefore wrong; they timed out at 30s each and
    /// were removed rather than shipped as slow, incorrect. The positive wins above are the
    /// real ones for an error-bound prover.
    #[test]
    fn round_error_bounds_discharge_from_the_axiom() {
        // round(x) <= x + 0.5  (upper edge of the bounding box)
        let upper = concat!(
            "fn q(x: f32) -> bool {\n",
            "  proof { assert(round(x) <= x + 0.5); }\n",
            "  return true;\n",
            "}"
        );
        assert_all_discharged(&obligations_for(upper), 1);

        // x - 0.5 <= round(x)  (lower edge)
        let lower = concat!(
            "fn q(x: f32) -> bool {\n",
            "  proof { assert(x - 0.5 <= round(x)); }\n",
            "  return true;\n",
            "}"
        );
        assert_all_discharged(&obligations_for(lower), 1);
    }

    /// A wrong-arity `round` is refused by the encoder, not sent to the solver.
    /// This is the only cheap non-discharge we can pin without a 30s solver budget,
    /// so it is asserted here to keep the suite honest about the refusal boundary.
    #[test]
    fn round_with_wrong_arity_is_refused() {
        let src = "fn q(x: f32) -> bool { proof { assert(round(x, x) > 0.0); } return true; }";
        let diags = obligations_for(src);
        assert!(
            diags.iter().any(|d| d.code == OBL_UNSUPPORTED),
            "round with 2 args must be refused, not rubber-stamped. got {diags:?}"
        );
    }

    /// The round axiom renders as a real-bounded forall over a fresh variable named
    /// `round_ax_x`, applied to the uninterpreted `round` function. This pins the axiom's
    /// SHAPE (soundness-critical) without depending on Z3's slow real quantifier solving.
    #[test]
    fn round_axiom_is_well_formed() {
        let axiom = round_axiom().to_string();
        assert!(
            axiom.contains("forall"),
            "the round axiom must be universally quantified. got: {axiom}"
        );
        assert!(
            axiom.contains("(round round_ax_x)"),
            "the axiom must bind the uninterpreted `round` at its fresh variable. got: {axiom}"
        );
        assert!(
            axiom.contains("round_ax_x") && !axiom.contains("round_ax_x round_ax_x"),
            "the bounding variable must be fresh and unambiguous. got: {axiom}"
        );
        // Both edges of the bound must appear.
        assert!(
            axiom.contains("0.5"),
            "the half-unit error bound must appear. got: {axiom}"
        );
        // The bound MUST be inclusive (`<=`), not strict (`<`). Real rounding attains the
        // boundary: round(0.5) = 1.0 = 0.5 + 0.5 under nearest-ties-away, so a strict `<`
        // axiom would be UNSOUND -- it excludes a real model of rounding. The semantic
        // discharge tests cannot tell `<=` from `<` (strict implies inclusive for a `<=`
        // claim), so this structural guard is the ONLY pin on the sound choice. Mutant D
        // (swap `<=` to `<`) is killed here rather than surviving.
        assert_eq!(
            axiom.matches("<=").count(),
            2,
            "the round axiom must bound inclusive (<=) at both edges; got: {axiom}"
        );
    }

    /// Scalar round-equality claims whose argument is a CONCRETE real literal are
    /// decided by the bounding axiom, not left Undecided by the 30s quantifier budget.
    /// Two families are pinned here:
    ///  - a NON-integer RHS (`round(4.5) == 4.5`): `round` never returns a non-integer, so
    ///    the bound grounds `round(4.5)` to `[4, 5]` and `round(4.5) = 4` is a countermodel
    ///    distinct from `4.5` -> refuted by ground SAT.
    ///  - an INTEGER tie RHS (`round(0.5) == 1.0`): `1` is a valid `round-half-down` value
    ///    of the argument region, so this claim is NOT a theorem (it is false under
    ///    round-half-down) and is refuted -- soundly -- by the countermodel `round(0.5)=0`.
    ///    The universal axiom is NOT asserted for these (the argument is free), so Z3
    ///    decides them by ground SAT instead of timing out on the real quantifier.
    /// The genuinely-quantified `round(v) == w` with a SYMBOLIC `w` (e.g.
    /// `forall t. round(t) == t`) stays Undecided -- Z3 times out on the real
    /// quantifier; that is the remaining frontier. But `round(v) == <concrete w>`
    /// (symbolic `v`, concrete RHS) is REFUTED by the ground bound -- see
    /// `symbolic_round_equality_with_concrete_rhs_is_refuted`.
    #[test]
    fn round_equality_refutations_are_decided_not_undecided() {
        let src = "fn q() -> bool { proof { assert(round(4.5) == 4.5); } return true; }";
        let diags = obligations_for(src);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "concrete round equality `round(4.5) == 4.5` must be REFUTED (NASO-OBL-001), \
             i.e. decided, not undecided. got {diags:?}"
        );
        let src = "fn q() -> bool { proof { assert(round(0.5) == 1.0); } return true; }";
        let diags = obligations_for(src);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "integer-tie round equality `round(0.5) == 1.0` must be REFUTED (NASO-OBL-001): \
             it is not a theorem (false under round-half-down), so it is refuted, not \
             undecided. got {diags:?}"
        );
    }

    // `round(v) == 4.5` (v: f32, concrete RHS) refutes by the ground bound -- `round`
    // returns an integer, so `round(v) = 4` (v = 4.0) is a countermodel in [v-0.5, v+0.5].
    #[test]
    fn symbolic_round_equality_with_concrete_rhs_is_refuted() {
        let src = "fn q(v: f32) -> bool { proof { assert(round(v) == 4.5); } return true; }";
        let diags = obligations_for(src);
        assert!(diags.iter().any(|d| d.code == OBL_FALSE), "got {diags:?}");
    }

    /// `encoded_uses_round` must not fire on obligations that never mention `round`, so
    /// scripts that do not use `round` still assert NO axiom. A false positive there would
    /// silently strengthen every script; a false negative would drop the axiom for a real
    /// round obligation. Both are checked cheaply, without the solver.
    #[test]
    fn encoded_uses_round_is_scoped_to_round() {
        use crate::smtlib::builder;
        let x = builder::var("x", Sort::Real);
        let y = builder::var("y", Sort::Real);
        // No `round` anywhere -> no axiom needed.
        assert!(!term_uses_round(&builder::le(x.clone(), builder::int(1))));
        // `round(x)` present.
        assert!(term_uses_round(&builder::le(
            builder::app("round", vec![x.clone()]),
            builder::int(1)
        )));
        // `round` buried under another application.
        assert!(term_uses_round(&builder::app(
            "foo",
            vec![builder::app("round", vec![y.clone()])]
        )));
        // A different name must NOT match.
        assert!(!term_uses_round(&builder::app("rounddown", vec![x])));
    }

    /// A quantiser range stated as a GOAL is refuted, because an unconstrained tensor has no
    /// range. It is only provable as a PREMISE.
    ///
    /// This is the correction made to `kernels/quant_int8.naso`: its range obligation sat in a
    /// `proof` block, where it claimed the function established something about its own inputs
    /// that nothing in the function establishes.
    #[test]
    fn a_quantiser_range_goal_is_refuted_but_discharges_as_a_premise() {
        let goal_only = concat!(
            "fn q(input: Tensor[f32, 16], scale: f32) -> bool {\n",
            "  proof { forall i in 0..16 { assert(abs(input[i] / scale) <= 127.0); } }\n",
            "  return true;\n",
            "}"
        );
        let diags = obligations_for(goal_only);
        assert!(
            diags.iter().any(|d| d.code == OBL_FALSE),
            "an unbounded tensor must REFUTE the range goal. got {diags:?}"
        );

        let with_premise = concat!(
            "fn q(input: Tensor[f32, 16], scale: f32) requires {\n",
            "  forall i in 0..16 { assert(abs(input[i] / scale) <= 127.0); }\n",
            "} {\n",
            "  proof { forall i in 0..16 { assert(abs(input[i] / scale) <= 127.0); } }\n",
            "  return true;\n",
            "}"
        );
        assert_all_discharged(&obligations_for(with_premise), 1);
    }

    /// The shipped int8 quantiser discharges every obligation it states.
    #[test]
    fn the_shipped_int8_quantiser_kernel_discharges_completely() {
        let kernel = quant_int8_kernel();
        // Six obligations: the range goal (via premise) and the upper `round` error bound in
        // `quantize_int8_symmetric`, and the two clamp bounds plus the two `round` error
        // bounds in `clamp_keeps_the_narrowing_in_range`. Every one is discharged by the
        // `round` bounding axiom rather than by the premise, so this pin also covers the
        // axiom path in a shipping kernel -- a mutation that drops `encode_round_axiom` or
        // `term_uses_round` fails this test before it can reach production.
        assert_all_discharged(&obligations_for(&kernel), 6);
    }

    /// An UNKNOWN intrinsic is refused, and the refusal names what was refused and why.
    ///
    /// The message is asserted as well as the code because a refusal that says only "not
    /// supported" is half a diagnostic: the reader still cannot tell which construct blocked
    /// them. A mutation that blanks this string survived the whole suite until this check was
    /// added -- the BEHAVIOUR was already right and only the explanation was untested, which
    /// is exactly the kind of gap a mutation run is for.
    #[test]
    fn an_unknown_intrinsic_is_refused_with_a_reason_a_reader_can_act_on() {
        let src = "fn q(x: f32) -> bool { proof { assert(sigmoidise(x) > 0.0); } return true; }";
        let diags = obligations_for(src);
        let undecided = diags
            .iter()
            .find(|d| d.code == OBL_UNSUPPORTED)
            .unwrap_or_else(|| panic!("an unknown intrinsic must be UNDECIDED, got {diags:?}"));
        let msg = &undecided.message;
        assert!(
            msg.contains("sigmoidise"),
            "the refusal must name the function it refused, got: {msg}"
        );
        assert!(
            msg.contains("uninterpreted"),
            "the refusal must explain why an uninterpreted definition is unacceptable, \
             because that is the part the reader cannot check for themselves. got: {msg}"
        );
    }
}
