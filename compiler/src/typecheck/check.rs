//! Type checking functions for the Naso type checker
//!
//! Implements the bidirectional typing rules: check mode (type-directed)
//! Full Quantitative Type Theory (QTT) enforcement:
//! - [0] erased variables: compile-time only, cannot appear at runtime
//! - [1] linear variables: consumed exactly once across all branches
//! - [N] bounded variables: consumed at most N times
//! - [*] unrestricted: any number of uses

#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]

use crate::ast::expr::{LetConsumeBinding, LetInOutBinding};
use crate::ast::*;
use crate::typecheck::constraints::{is_erasable, qty_subtype};
use crate::typecheck::error::TypeError;
use crate::typecheck::inference::infer_expr;
use crate::typecheck::*;

/// Check an expression against an expected type (Check mode)
pub fn check_expr(
    checker: &mut TypeChecker,
    expr: &Expr,
    expected: &Type,
) -> Result<(), TypeError> {
    // First, synthesize the type of the expression
    let inferred = infer_expr(checker, expr)?;

    // Unify with expected type (including quantity)
    unify::unify_types(checker, &inferred, expected)?;

    // Additional QTT checks for quantity
    check_quantity_consumption(checker, expr, &inferred, expected)?;

    Ok(())
}

/// Check quantity consumption rules
fn check_quantity_consumption(
    _checker: &mut TypeChecker,
    expr: &Expr,
    inferred: &Type,
    expected: &Type,
) -> Result<(), TypeError> {
    // If expected quantity is Zero, the expression must be erasable
    if expected.quantity == Quantity::Zero && !is_erasable(inferred.quantity) {
        return Err(TypeError::QuantityMismatch {
            expected: Quantity::Zero,
            found: inferred.quantity,
            span: expr.span,
        });
    }

    // If inferred is Zero but expected is not, that's an error
    if inferred.quantity == Quantity::Zero && expected.quantity != Quantity::Zero {
        return Err(TypeError::ErasedVariableUsedAtRuntime {
            name: inferred.to_ident(),
            span: expr.span,
        });
    }

    // Check subtyping: inferred qty must be <= expected qty
    if !qty_subtype(inferred.quantity, expected.quantity) {
        return Err(TypeError::QuantityMismatch {
            expected: expected.quantity,
            found: inferred.quantity,
            span: expr.span,
        });
    }

    Ok(())
}

/// Check a statement
pub fn check_stmt(checker: &mut TypeChecker, stmt: &Stmt) -> Result<(), TypeError> {
    match &stmt.kind {
        StmtKind::Let(let_stmt) => check_let(checker, let_stmt),
        StmtKind::LetInOut(let_inout) => check_let_inout(checker, let_inout),
        StmtKind::Expr(expr) => {
            let _ = infer_expr(checker, expr)?;
            Ok(())
        }
        StmtKind::Return(opt_expr) => check_return(checker, opt_expr.as_ref()),
        StmtKind::Item(item) => check_item(checker, item),
        StmtKind::Reversible(block) => check_reversible(checker, block),
        StmtKind::Proof(block) => check_proof(checker, block),
        StmtKind::Break(opt_expr) => check_break(checker, opt_expr.as_ref()),
        StmtKind::Continue => Ok(()),
        StmtKind::Empty => Ok(()),
        StmtKind::LetConsume(let_consume) => check_let_consume(checker, let_consume),
        StmtKind::Error => Ok(()),
    }
}
fn check_let(checker: &mut TypeChecker, let_stmt: &LetStmt) -> Result<(), TypeError> {
    // Infer the type of the initializer
    let init_ty = infer_expr(checker, &let_stmt.value)?;

    // Handle tuple patterns by destructuring the tuple type
    let mut bindings = Vec::new();
    let mut pattern_qtys = Vec::new(); // Pattern quantity for each binding
    match (&let_stmt.pattern.kind, &init_ty.kind) {
        (PatternKind::Tuple(patterns), TypeKind::Tuple(element_types)) => {
            // Match each pattern element with corresponding tuple element type
            if patterns.len() != element_types.len() {
                return Err(TypeError::PatternTupleArityMismatch {
                    pattern_len: patterns.len(),
                    tuple_len: element_types.len(),
                    span: let_stmt.pattern.span,
                });
            }
            for (p, ty) in patterns.iter().zip(element_types.iter()) {
                let name = match &p.kind {
                    PatternKind::Ident(ident) => ident.clone(),
                    _ => panic!("Expected ident in tuple pattern"),
                };
                // Infer pattern quantity from expected type if let binding has no explicit quantity
                let pattern_qty = if let_stmt.quantity == Quantity::Many {
                    ty.quantity
                } else {
                    p.quantity
                };
                bindings.push((name.clone(), ty.clone()));
                pattern_qtys.push(pattern_qty);
            }
        }
        (PatternKind::Ident(ident), _) => {
            // Simple identifier binding.
            //
            // The quantity is inherited from the initializer unless the statement
            // carries an explicit annotation. `parse_quantity` defaults an
            // unannotated `let` -- and the pattern's own quantity -- to
            // `Quantity::Many`, so reading `pattern.quantity` directly WIDENED a
            // linear value on every plain `let`, escaping the linear-type discipline
            // through a single intervening binding:
            //
            //     fn f(x: [1] i32) { let y = x; let _ = y; let _ = y; }
            //
            // `y` became `[*]`, so the "a `[1]` value is used exactly once" rule
            // never applied to it. The same widening also defeated the leak check
            // on `x` itself:
            //
            //     fn f(x: [1] i32) { let y = x; let _ = y; }   // x never consumed
            //
            // which is a false negative in the property this checker exists to
            // enforce. The tuple arm below already inherited correctly; this arm
            // did not, which is why a single-element binding slipped through while
            // destructuring a tuple of them behaved.
            //
            // Only `Quantity::Many` is replaced, since that is both the unannotated
            // default and an explicit `[*]`. Inheriting on an explicit `[*]` is
            // deliberate: silently widening a `[1]` value is never something to
            // permit, and there is no way here to tell an explicit `[*]` from the
            // default -- `naso parse` reports `Many` for both, so a mutation that
            // always inherits is behaviourally equivalent rather than untested.
            let pattern_qty = if let_stmt.quantity == Quantity::Many {
                init_ty.quantity
            } else {
                let_stmt.quantity
            };
            bindings.push((ident.clone(), init_ty.clone()));
            pattern_qtys.push(pattern_qty);
        }
        _ => {
            // For other patterns (wildcard, struct, etc.), bind the whole value
            // Use a synthetic name for now
            let synthetic_name = Ident::new("__pattern_binding".to_string(), let_stmt.pattern.span);
            bindings.push((synthetic_name.clone(), init_ty.clone()));
            pattern_qtys.push(let_stmt.pattern.quantity);
        }
    };

    // Check if Qubit type requires quantity One (use pattern quantity for each binding)
    for (i, (_, ty)) in bindings.iter().enumerate() {
        if matches!(ty.kind, TypeKind::Qubit) && pattern_qtys[i] != Quantity::One {
            return Err(TypeError::QubitQuantityMismatch {
                found: pattern_qtys[i],
                span: let_stmt.pattern.span,
            });
        }
    }

    // If explicit type annotation, check against it
    if let Some(ann_ty) = &let_stmt.ty {
        checker.check_expr(&let_stmt.value, ann_ty)?;
    }

    // Bind each variable in the pattern (use pattern quantity for each binding)
    for (i, (name, ty)) in bindings.iter().enumerate() {
        checker.env.bind_var(
            name.clone(),
            ty.clone(),
            pattern_qtys[i],
            let_stmt.mutability,
        );
    }

    // Validate quantity/mutability combinations (use pattern quantity for each binding)
    for (i, (name, _)) in bindings.iter().enumerate() {
        validate_binding_quantity_mutability(
            name,
            pattern_qtys[i],
            let_stmt.mutability,
            let_stmt.span,
        )?;
    }

    Ok(())
}

#[allow(dead_code)]
fn extract_pattern_names(pattern: &Pattern) -> Vec<Ident> {
    match &pattern.kind {
        PatternKind::Ident(ident) => vec![ident.clone()],
        PatternKind::Tuple(patterns) => {
            let mut names = Vec::new();
            for p in patterns {
                names.extend(extract_pattern_names(p));
            }
            names
        }
        PatternKind::Struct(_, fields) => {
            let mut names = Vec::new();
            for f in fields {
                names.extend(extract_pattern_names(&f.pattern));
            }
            names
        }
        PatternKind::Variant(_, _, patterns) => {
            let mut names = Vec::new();
            for p in patterns {
                names.extend(extract_pattern_names(p));
            }
            names
        }
        PatternKind::Array(patterns) => {
            let mut names = Vec::new();
            for p in patterns {
                names.extend(extract_pattern_names(p));
            }
            names
        }
        PatternKind::Or(a, b) => {
            let mut names = extract_pattern_names(a);
            names.extend(extract_pattern_names(b));
            names
        }
        PatternKind::Ref(p) | PatternKind::InOut(p) | PatternKind::Consume(p) => {
            extract_pattern_names(p)
        }
        PatternKind::Guard(p, _) => extract_pattern_names(p),
        PatternKind::Wildcard
        | PatternKind::Literal(_)
        | PatternKind::Error
        | PatternKind::Range(_, _) => vec![],
    }
}

/// Check inout let binding
fn check_let_inout(checker: &mut TypeChecker, stmt: &LetInOutStmt) -> Result<(), TypeError> {
    // Convert LetInOutStmt to LetInOutBinding for type checking
    let binding = LetInOutBinding {
        name: stmt.name.clone(),
        ty: stmt.ty.clone(),
        value: stmt.value.clone(),
        span: stmt.span,
    };

    // The value must be a place expression (variable, field access, etc.)
    // with quantity One (unique ownership)
    let value_ty = infer_expr(checker, &binding.value)?;

    // Check that the value has quantity One (linear)
    if value_ty.quantity != Quantity::One {
        return Err(TypeError::InOutRequiresUnique {
            found_qty: value_ty.quantity,
            span: binding.span,
        });
    }

    // Verify it's a place expression (not a temporary)
    if !is_place_expr(&binding.value) {
        return Err(TypeError::InOutRequiresUnique {
            found_qty: Quantity::Many, // Not a place
            span: binding.span,
        });
    }

    // Extract the place for alias tracking
    let place = expr_to_place(&binding.value)?;

    // Start inout borrow (checks for aliasing)
    checker
        .env
        .borrow_inout(binding.name.clone(), place, binding.span)?;

    // Bind as inout with the inferred type
    checker.env.bind_var(
        binding.name.clone(),
        value_ty,
        Quantity::One,
        Mutability::InOut,
    );

    Ok(())
}

/// Check return statement
fn check_return(checker: &mut TypeChecker, opt_expr: Option<&Expr>) -> Result<(), TypeError> {
    if let Some(expr) = opt_expr {
        let expr_ty = infer_expr(checker, expr)?;
        if let Some(expected) = checker.current_fn_ret.clone() {
            unify::unify_types(checker, &expr_ty, &expected)?;
        }
    } else {
        // Empty return - check against Unit
        if let Some(expected) = checker.current_fn_ret.clone() {
            unify::unify_types(checker, &Type::unit(Span::default()), &expected)?;
        }
    }
    Ok(())
}

/// Check item (type, function, etc.)
fn check_item(checker: &mut TypeChecker, item: &Item) -> Result<(), TypeError> {
    match item {
        Item::Function(f) => {
            // Register function signature only - body checking happens in second pass
            checker.env.insert_function(f.clone());
            Ok(())
        }
        Item::TypeDef(t) => {
            checker.env.insert_type_def(t.clone());
            Ok(())
        }
        Item::Const(c) => {
            checker.env.insert_const(c.clone());
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Check reversible block
fn check_reversible(checker: &mut TypeChecker, block: &ReversibleBlock) -> Result<(), TypeError> {
    let prev_reversible = checker.in_reversible;
    checker.in_reversible = true;

    let guard = checker.env.enter_scope();

    // Check body statements in pure mode
    for stmt in &block.body.stmts {
        check_stmt(checker, stmt)?;

        // Verify no impure operations
        check_pure_statement(checker, stmt)?;
    }

    // Verify all variables have uncomputation steps (stub)
    // In real implementation, check uncompute DAG

    checker.env.exit_scope(guard)?;
    checker.in_reversible = prev_reversible;
    Ok(())
}

/// Check a `proof { .. }` block.
///
/// The body is typechecked as an ordinary block, in its own scope so its
/// bindings cannot leak into the enclosing runtime code. Purity is *not*
/// required: an obligation is not executed, and restricting what may appear in
/// an obligation would only make some obligations inexpressible.
///
/// `in_reversible` is deliberately left alone. A proof block is a statement
/// about a function, not an operation inside a reversible region, so entering
/// one does not change whether the surrounding code is reversible.
///
/// This establishes that the obligations are well-typed. It does not
/// discharge them -- that is `naso-verify`'s job, which reads the block from
/// the AST.
fn check_proof(checker: &mut TypeChecker, block: &ProofBlock) -> Result<(), TypeError> {
    let guard = checker.env.enter_scope();
    let prev_proof = checker.in_proof;
    checker.in_proof = true;
    // Entering an erased region: references from here observe without consuming.
    checker.env.erased_depth += 1;
    // A proof block is erased, so it observes values without consuming them.
    // Without this, a quantified obligation about a `[1]` linear parameter
    // would consume it and make the runtime loop report a double use.
    let uses = checker.env.snapshot_uses();
    let outcome = (|| {
        for stmt in &block.body.stmts {
            check_stmt(checker, stmt)?;
        }
        Ok(())
    })();
    // Erase, do not rewind. `restore_uses` rewinds `used_at` to its previous value, so a `[1]`
    // parameter referenced ONLY inside a proof block comes back looking untouched and is
    // then reported as an unused linear variable -- a leak reported as an omission.
    //
    // The erase must also run on the ERROR path: a proof block that fails has already
    // recorded uses, and leaving them in place makes the next error blame the proof block
    // for a double use instead of reporting the real fault.
    checker.env.erase_uses_since(&uses);
    checker.env.erased_depth -= 1;
    checker.in_proof = prev_proof;
    checker.env.exit_scope(guard)?;
    outcome
}

/// Check that a statement is pure (no I/O, measurement, etc.)
fn check_pure_statement(checker: &mut TypeChecker, stmt: &Stmt) -> Result<(), TypeError> {
    match &stmt.kind {
        StmtKind::Expr(expr) => check_pure_expr(checker, expr),
        StmtKind::Let(_) | StmtKind::LetInOut(_) => Ok(()), // Bindings are pure
        StmtKind::Return(_) => Ok(()),
        StmtKind::Item(_) => Ok(()),
        _ => Err(TypeError::ImpureInReversible {
            operation: "statement".to_string(),
            span: stmt.span,
        }),
    }
}

/// Check that an expression is pure
fn check_pure_expr(checker: &mut TypeChecker, expr: &Expr) -> Result<(), TypeError> {
    match &expr.kind {
        ExprKind::QuantumOp(QuantumOp::Measure(_) | QuantumOp::Hamiltonian(_, _)) => {
            Err(TypeError::ImpureInReversible {
                operation: "quantum measurement/hamiltonian".to_string(),
                span: expr.span,
            })
        }
        ExprKind::QuantumOp(_) => Ok(()),
        ExprKind::Call(callee, _) => {
            // Check if callee is impure
            let callee_ty = infer_expr(checker, callee)?;
            if let TypeKind::Function(_, ret) = &callee_ty.kind {
                if ret.quantity == Quantity::Zero {
                    // Could be pure, but need more analysis
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Check break expression
fn check_break(checker: &mut TypeChecker, opt_expr: Option<&Expr>) -> Result<(), TypeError> {
    // Would need loop context tracking - simplified
    if let Some(expr) = opt_expr {
        infer_expr(checker, expr)?;
    }
    Ok(())
}

/// Check function definition
pub fn check_function(checker: &mut TypeChecker, func: &Function) -> Result<(), TypeError> {
    let prev_ret = checker.current_fn_ret.clone();
    checker.current_fn_ret = func.ret_ty.clone();

    let guard = checker.env.enter_scope();

    // Bind Nat generic parameters as values, so they are usable as terms.
    //
    // `fn quantize[N: nat](t: Tensor[f32, N])` needs `N` available as a *value*
    // -- `forall i in 0..N` is a loop over a term, and `t[i]` indexes by one.
    // Binding only the type left `N` undefined as an expression, which made
    // the parser panic with "expected identifier, found 'let'" when a loop body
    // used it. Type parameters stay types; only Nat params are values.
    for generic in &func.generics {
        if generic.kind == GenericKind::Nat {
            checker.env.bind_var(
                generic.name.clone(),
                Type::new(TypeKind::Nat, Quantity::Many, generic.span),
                Quantity::Many,
                Mutability::Immutable,
            );
        }
    }

    // Bind function parameters with their quantities and mutabilities
    for param in &func.params {
        // Use the quantity from the type (e.g., `x: [1] i32`) as the authoritative quantity
        let param_qty = param.ty.quantity;
        checker.env.bind_var(
            param.name.clone(),
            param.ty.clone(),
            param_qty,
            param.mutability,
        );

        // Validate parameter quantity/mutability
        validate_binding_quantity_mutability(&param.name, param_qty, param.mutability, param.span)?;
    }

    // Check preconditions, once the parameters they talk about are in scope.
    //
    // Without this, a `requires` block is never typechecked, so a precondition could name an
    // undefined variable and the only place that surfaced would be the prover -- reporting a
    // missing symbol in generated SMT, far from the source that wrote it. A precondition that
    // does not typecheck is a precondition nobody proved anything about.
    if !func.requires.is_empty() {
        let prev_proof = checker.in_proof;
        checker.in_proof = true;
        // Entering an erased region, same as a `proof` block: a precondition observes the
        // values it constrains without consuming them.
        checker.env.erased_depth += 1;
        // Like a proof block, a precondition is ERASED: it observes values without
        // consuming them, so a `[1]` linear parameter referenced in `requires` is not
        // reported as used up here and again in the body.
        let uses = checker.env.snapshot_uses();
        for pred in &func.requires {
            // `ExprKind::Forall` is the LOOP form and infers to `()`; `ExprKind::Quantified`
            // is the PROPOSITION form and infers to `Bool`, with the body's tail checked
            // against `bool`. A precondition is a proposition, so a quantified premise must
            // go through the latter.
            //
            // Calling `check_expr` on the `Forall` form instead rejected every quantified
            // precondition with "expected `()`, found `Bool`" once the statement-level
            // expectation was applied -- the loop and the proposition genuinely have
            // different types, and only one of them is a claim.
            // The restore must run on the ERROR path too. A precondition that fails to
            // typecheck has already recorded a use of whatever it referenced, and returning
            // early with that use still in place makes the NEXT error report it as a
            // double use -- pointing at the precondition rather than at the real problem.
            let outcome = match &pred.kind {
                ExprKind::Forall(loop_) => check_precondition_quantifier(checker, loop_, pred.span),
                _ => {
                    let bool_ty = Type::new(TypeKind::Bool, Quantity::Many, pred.span);
                    checker.check_expr(pred, &bool_ty)
                }
            };
            checker.env.erase_uses_since(&uses);
            outcome?;
        }
        checker.env.erase_uses_since(&uses);
        checker.env.erased_depth -= 1;
        checker.in_proof = prev_proof;
    }

    // Check function body
    check_block(checker, &func.body)?;

    checker.env.exit_scope(guard)?;
    checker.current_fn_ret = prev_ret;
    Ok(())
}

/// Check a `forall` appearing in a `requires` block.
///
/// `infer_quantified` handles the EXPRESSION shape, where the proposition is the body's tail
/// expression: `forall i in a..b { t[i] <= 10 }`. The source form used for obligations puts
/// the claim in STATEMENT position instead -- `forall i in a..b { assert(t[i] <= 10); }` --
/// so the block has no tail and `infer_quantified` rejects it with "a quantified proposition
/// must have a boolean body".
///
/// Both shapes are accepted here, and a block whose last statement is the `assert` is checked
/// against that assert's ARGUMENT. Requiring a tail instead would reject exactly the form
/// every existing obligation and kernel uses, and would push authors toward a weaker spelling
/// that happens to parse.
fn check_precondition_quantifier(
    checker: &mut TypeChecker,
    quant: &ForallLoop,
    span: Span,
) -> Result<(), TypeError> {
    let bool_ty = Type::new(TypeKind::Bool, Quantity::Many, span);
    if let Some(tail) = &quant.body.expr {
        // Expression form: the tail IS the proposition.
        let pred_ty = infer_expr(checker, tail)?;
        unify::unify_types(checker, &pred_ty, &bool_ty)?;
        return Ok(());
    }

    // Statement form: the last `assert(..)` supplies the proposition.
    let Some(last) = quant.body.stmts.last() else {
        return Err(TypeError::QuantifiedBodyNotBool {
            span: quant.body.span,
        });
    };
    let StmtKind::Expr(expr) = &last.kind else {
        return Err(TypeError::QuantifiedBodyNotBool {
            span: quant.body.span,
        });
    };
    let ExprKind::Call(_, args) = &expr.kind else {
        return Err(TypeError::QuantifiedBodyNotBool {
            span: quant.body.span,
        });
    };
    let Some(predicate) = args.first() else {
        return Err(TypeError::QuantifiedBodyNotBool {
            span: quant.body.span,
        });
    };

    infer_quantified_with_body(checker, quant, predicate, span)
}

/// Infer a quantified proposition whose body ends in an explicit predicate expression.
fn infer_quantified_with_body(
    checker: &mut TypeChecker,
    quant: &ForallLoop,
    predicate: &Expr,
    span: Span,
) -> Result<(), TypeError> {
    let guard = checker.env.enter_scope();
    for (var, lower, upper) in &quant.bindings {
        let _ = infer_expr(checker, lower)?;
        let _ = infer_expr(checker, upper)?;
        // Infer the bound variable's type from its range literal bounds, not a
        // hard-coded `Int`. A float range (`forall t in 0.0..1.0`) makes `t` a Float,
        // so `round(t)` (which expects f32) typechecks and the obligation encoder
        // can bind `t` as a Real. Integer ranges keep `Int` (the polyhedral path).
        let bound_ty = type_of_range_bound(lower, span);
        checker
            .env
            .bind_var(var.clone(), bound_ty, Quantity::Many, Mutability::Immutable);
    }
    // Every statement is checked EXCEPT the trailing `assert`, whose argument is `predicate`.
    // Checking it as a statement and then inferring `predicate` separately records the same
    // use of a linear variable twice, and the second one is reported as a double use at the
    // same span -- an error the author cannot act on.
    let stmts = &quant.body.stmts;
    let body_stmts = stmts.len().saturating_sub(1);
    for stmt in stmts.iter().take(body_stmts) {
        check_stmt(checker, stmt)?;
    }
    let bool_ty = Type::new(TypeKind::Bool, Quantity::Many, span);
    let pred_ty = infer_expr(checker, predicate)?;
    unify::unify_types(checker, &pred_ty, &bool_ty)?;
    checker.env.exit_scope(guard)?;
    Ok(())
}

/// Check block
pub fn check_block(checker: &mut TypeChecker, block: &Block) -> Result<(), TypeError> {
    let guard = checker.env.enter_scope();

    for stmt in &block.stmts {
        check_stmt(checker, stmt)?;
    }

    if let Some(expr) = &block.expr {
        infer_expr(checker, expr)?;
    }

    checker.env.exit_scope(guard)?;
    Ok(())
}

/// Check pattern against scrutinee type
pub fn check_pattern(
    checker: &mut TypeChecker,
    pattern: &Pattern,
    scrutinee_ty: &Type,
) -> Result<type_env::PatternBindings, TypeError> {
    let mut bindings = type_env::PatternBindings::new();

    match &pattern.kind {
        PatternKind::Ident(ident) => {
            let info = type_env::VarInfo::new(
                scrutinee_ty.clone(),
                pattern.quantity,
                Mutability::Immutable,
                pattern.span,
            );
            bindings.insert(ident.clone(), info);
            Ok(bindings)
        }
        PatternKind::Wildcard => Ok(bindings),
        PatternKind::Literal(lit) => {
            let lit_ty = literal_type(lit, pattern.span);
            unify::unify_types(checker, &lit_ty, scrutinee_ty)?;
            Ok(bindings)
        }
        PatternKind::Struct(name, fields) => {
            // Look up struct type - clone the fields we need to avoid borrow issues
            let struct_fields = checker.env.lookup_type(name).and_then(|td| {
                if let TypeDefKind::Struct(fields) = &td.kind {
                    Some(fields.clone())
                } else {
                    None
                }
            });

            if let Some(struct_fields) = struct_fields {
                for field_pat in fields {
                    if let Some(field_def) = struct_fields.iter().find(|f| f.name == field_pat.name)
                    {
                        let field_bindings =
                            checker.check_pattern(&field_pat.pattern, &field_def.ty)?;
                        bindings.extend(field_bindings);
                    }
                }
            }
            Ok(bindings)
        }
        PatternKind::Variant(enum_name, variant_name, fields) => {
            // Look up enum variant - clone the fields we need to avoid borrow issues
            let variant_fields = checker.env.lookup_type(enum_name).and_then(|td| {
                if let TypeDefKind::Enum(variants) = &td.kind {
                    variants
                        .iter()
                        .find(|v| v.name == *variant_name)
                        .map(|v| v.fields.clone())
                } else {
                    None
                }
            });

            if let Some(variant_fields) = variant_fields {
                for (field_pat, field_def) in fields.iter().zip(&variant_fields) {
                    let field_bindings = checker.check_pattern(field_pat, &field_def.ty)?;
                    bindings.extend(field_bindings);
                }
            }
            Ok(bindings)
        }
        PatternKind::Tuple(patterns) => {
            if let TypeKind::Tuple(types) = &scrutinee_ty.kind {
                for (pat, ty) in patterns.iter().zip(types) {
                    let field_bindings = checker.check_pattern(pat, ty)?;
                    bindings.extend(field_bindings);
                }
            }
            Ok(bindings)
        }
        PatternKind::Or(a, b) => {
            // All alternatives must bind the same variables with same types
            let mut first_bindings = None;
            for pat in [a.as_ref(), b.as_ref()] {
                let b = checker.check_pattern(pat, scrutinee_ty)?;
                if first_bindings.is_none() {
                    first_bindings = Some(b);
                } else {
                    // Verify compatibility - simplified
                }
            }
            Ok(first_bindings.unwrap_or_default())
        }
        PatternKind::Array(patterns) => {
            if let TypeKind::Array(elem_ty, _) = &scrutinee_ty.kind {
                for pat in patterns {
                    let field_bindings = checker.check_pattern(pat, elem_ty)?;
                    bindings.extend(field_bindings);
                }
            }
            Ok(bindings)
        }
        PatternKind::Range(_, _) => {
            // Range pattern - check both bounds
            let start_ty = Type::new(TypeKind::Int, Quantity::Many, pattern.span);
            let end_ty = Type::new(TypeKind::Int, Quantity::Many, pattern.span);
            unify::unify_types(checker, &start_ty, scrutinee_ty)?;
            unify::unify_types(checker, &end_ty, scrutinee_ty)?;
            Ok(bindings)
        }
        PatternKind::Ref(inner) => {
            // Reference pattern - check inner pattern
            checker.check_pattern(inner, scrutinee_ty)
        }
        PatternKind::InOut(inner) => {
            // Inout pattern - check inner pattern
            checker.check_pattern(inner, scrutinee_ty)
        }
        PatternKind::Consume(inner) => {
            // Consume pattern - check inner pattern
            checker.check_pattern(inner, scrutinee_ty)
        }
        PatternKind::Guard(pat, guard_expr) => {
            let bindings = checker.check_pattern(pat, scrutinee_ty)?;
            checker.check_expr(guard_expr, &Type::bool(guard_expr.span))?;
            Ok(bindings)
        }
        _ => Ok(bindings),
    }
}

/// Get type of a literal
fn literal_type(lit: &Literal, span: Span) -> Type {
    match lit {
        Literal::Int(_) => Type::new(TypeKind::Int, Quantity::Many, span),
        Literal::UInt(_) => Type::new(TypeKind::UInt, Quantity::Many, span),
        Literal::Float(_) => Type::new(TypeKind::Float, Quantity::Many, span),
        Literal::Bool(_) => Type::new(TypeKind::Bool, Quantity::Many, span),
        Literal::String(_) => Type::new(TypeKind::String, Quantity::Many, span),
        Literal::Char(_) => Type::new(TypeKind::Char, Quantity::Many, span),
        Literal::Unit => Type::unit(span),
    }
}

/// Infer the type of a `forall` bound variable from its range-literal bounds.
/// A float bound -> Float (Real in SMT); integer bounds -> Int. Non-literal bounds
/// fall back to Int, preserving the existing `forall i in 0..N` behaviour for the
/// polyhedral/parallel path. Only the PROPOSITIONAL `forall` reaches here with float
/// ranges -- the parallel loop form is a separate `ExprKind::Forall` and is refused
/// over floats by `loop_extraction` before a bound var type is assigned.
fn type_of_range_bound(bound: &Expr, span: Span) -> Type {
    if let ExprKind::Literal(Literal::Float(_)) = &bound.kind {
        Type::new(TypeKind::Float, Quantity::Many, span)
    } else {
        Type::new(TypeKind::Int, Quantity::Many, span)
    }
}
/// Validate quantity/mutability combination
fn validate_binding_quantity_mutability(
    _name: &Ident,
    qty: Quantity,
    mutability: Mutability,
    span: Span,
) -> Result<(), TypeError> {
    match (qty, mutability) {
        (Quantity::Zero, Mutability::InOut) => Err(TypeError::InOutRequiresUnique {
            found_qty: Quantity::Zero,
            span,
        }),
        (Quantity::Zero, Mutability::Consume) => Err(TypeError::QuantityMismatch {
            expected: Quantity::One,
            found: Quantity::Zero,
            span,
        }),
        (Quantity::One, Mutability::InOut) => Ok(()), // Valid: linear inout
        (Quantity::One, Mutability::Consume) => Ok(()), // Valid: linear consume
        (Quantity::Bounded(_), Mutability::InOut) => Ok(()), // Valid: bounded inout
        (Quantity::Bounded(_), Mutability::Consume) => Err(TypeError::QuantityMismatch {
            expected: Quantity::One,
            found: qty,
            span,
        }),
        (Quantity::Many, Mutability::InOut) => Err(TypeError::InOutRequiresUnique {
            found_qty: Quantity::Many,
            span,
        }),
        (Quantity::Many, Mutability::Consume) => Err(TypeError::QuantityMismatch {
            expected: Quantity::One,
            found: Quantity::Many,
            span,
        }),
        _ => Ok(()),
    }
}

/// Check if an expression is a place (assignable)
fn is_place_expr(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Var(_)
            | ExprKind::Field(_, _)
            | ExprKind::Index(_, _)
            | ExprKind::Projection(_)
            | ExprKind::Unary(UnOp::Deref, _)
    )
}

/// Convert expression to Place for alias tracking
fn expr_to_place(expr: &Expr) -> Result<type_env::Place, TypeError> {
    match &expr.kind {
        ExprKind::Var(ident) => Ok(type_env::Place::Var(ident.clone())),
        ExprKind::Field(base, field) => {
            let base_place = expr_to_place(base)?;
            Ok(type_env::Place::Field(Box::new(base_place), field.clone()))
        }
        ExprKind::Index(base, index) => {
            let base_place = expr_to_place(base)?;
            // For simplicity, use a string key
            Ok(type_env::Place::Index(
                Box::new(base_place),
                format!("{:?}", index),
            ))
        }
        ExprKind::Projection(base) => {
            let base_place = expr_to_place(base)?;
            Ok(type_env::Place::Deref(Box::new(base_place)))
        }
        ExprKind::Unary(UnOp::Deref, base) => {
            let base_place = expr_to_place(base)?;
            Ok(type_env::Place::Deref(Box::new(base_place)))
        }
        _ => Err(TypeError::InOutRequiresUnique {
            found_qty: Quantity::Many,
            span: expr.span,
        }),
    }
}

/// Check consume let binding
fn check_let_consume(checker: &mut TypeChecker, stmt: &LetConsumeStmt) -> Result<(), TypeError> {
    // Convert LetConsumeStmt to LetConsumeBinding for type checking
    let binding = LetConsumeBinding {
        name: stmt.name.clone(),
        ty: stmt.ty.clone(),
        value: stmt.value.clone(),
        span: stmt.span,
    };

    // The value must be a place expression with quantity One (linear)
    let value_ty = infer_expr(checker, &binding.value)?;

    // Check that the value has quantity One (linear)
    if value_ty.quantity != Quantity::One {
        return Err(TypeError::QuantityMismatch {
            expected: Quantity::One,
            found: value_ty.quantity,
            span: binding.span,
        });
    }

    // Verify it's a place expression
    if !is_place_expr(&binding.value) {
        return Err(TypeError::QuantityMismatch {
            expected: Quantity::One,
            found: Quantity::Many,
            span: binding.span,
        });
    }

    // Extract the place for alias tracking
    let place = expr_to_place(&binding.value)?;

    // Mark the source variable as moved
    checker
        .env
        .move_var(&place_to_ident(&place), binding.span)?;

    // Bind as consume with the inferred type
    checker.env.bind_var(
        binding.name.clone(),
        value_ty,
        Quantity::One,
        Mutability::Consume,
    );

    Ok(())
}

/// Convert Place back to Ident for move tracking
fn place_to_ident(place: &type_env::Place) -> Ident {
    match place {
        type_env::Place::Var(ident) => ident.clone(),
        type_env::Place::Field(base, _) => place_to_ident(base),
        type_env::Place::Index(base, _) => place_to_ident(base),
        type_env::Place::Deref(base) => place_to_ident(base),
    }
}
