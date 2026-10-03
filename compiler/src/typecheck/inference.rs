//! Inference functions for the Naso type checker
//!
//! Implements the bidirectional typing rules: infer mode (synthesis)

#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::single_match)]

use crate::ast::ty::{TypeKind, TypeVar};
use crate::ast::*;
use crate::typecheck::check::{check_block, check_stmt};
use crate::typecheck::error::TypeError;
use crate::typecheck::*;

/// Infer a numeric cast: `expr as T`.
///
/// Both the operand and the target must be numeric. `Tensor`, `Qubit`,
/// `QRegister`, bool, unit, tuples and functions are rejected: `as` is a
/// numeric conversion, not a reinterpret or bit-cast operator.
///
/// The quantity of the result is taken from the target type, not the operand,
/// because a conversion does not consume a linear value any differently than
/// its operand would.
fn infer_cast(
    checker: &mut TypeChecker,
    inner: &Expr,
    target: &Type,
    span: Span,
) -> Result<Type, TypeError> {
    let from = infer_expr(checker, inner)?;

    let is_numeric = |k: &TypeKind| {
        matches!(
            k,
            TypeKind::Int | TypeKind::UInt | TypeKind::Float | TypeKind::Nat
        )
    };

    if !is_numeric(&from.kind) || !is_numeric(&target.kind) {
        return Err(TypeError::InvalidCast {
            from,
            to: target.clone(),
            span,
        });
    }

    Ok(Type::new(target.kind.clone(), target.quantity, span))
}

/// Infer the type of an expression (synthesis mode)
pub fn infer_expr(checker: &mut TypeChecker, expr: &Expr) -> Result<Type, TypeError> {
    match &expr.kind {
        ExprKind::Literal(lit) => infer_literal(lit, expr.span),
        ExprKind::Var(ident) => infer_var(checker, ident, expr.span),
        ExprKind::Binary(op, lhs, rhs) => infer_binary(checker, *op, lhs, rhs, expr.span),
        ExprKind::Unary(op, operand) => infer_unary(checker, *op, operand, expr.span),
        ExprKind::Call(callee, args) => infer_call(checker, callee, args, expr.span),
        ExprKind::MethodCall(receiver, method, args) => {
            infer_method_call(checker, receiver, method, args, expr.span)
        }
        ExprKind::Field(base, field) => infer_field(checker, base, field, expr.span),
        ExprKind::Index(base, index) => infer_index(checker, base, index, expr.span),
        ExprKind::Struct(name, fields) => infer_struct(checker, name, fields, expr.span),
        ExprKind::Variant(enum_name, variant_name, fields) => {
            infer_variant(checker, enum_name, variant_name, fields, expr.span)
        }
        ExprKind::Tuple(elems) => infer_tuple(checker, elems, expr.span),
        ExprKind::Array(elems) => infer_array(checker, elems, expr.span),
        ExprKind::Block(block) => infer_block(checker, block, expr.span),
        ExprKind::If(cond, then_branch, else_branch) => infer_if(
            checker,
            cond,
            then_branch,
            else_branch.as_deref(),
            expr.span,
        ),
        ExprKind::Match(scrutinee, arms) => infer_match(checker, scrutinee, arms, expr.span),
        ExprKind::Let(binding) => infer_let(checker, binding, expr.span),
        ExprKind::LetInOut(binding) => infer_let_inout(checker, binding, expr.span),
        ExprKind::LetConsume(binding) => infer_let_consume(checker, binding, expr.span),
        ExprKind::Reversible(block) => infer_reversible(checker, block, expr.span),
        ExprKind::Lambda(lambda) => infer_lambda(checker, lambda, expr.span),
        ExprKind::For(for_loop) => infer_for(checker, for_loop, expr.span),
        ExprKind::Forall(forall_loop) => infer_forall(checker, forall_loop, expr.span),
        ExprKind::Quantified(quant) => infer_quantified(checker, quant, expr.span),
        ExprKind::While(cond, body) => infer_while(checker, cond, body, expr.span),
        ExprKind::Return(opt_expr) => infer_return(checker, opt_expr.as_deref(), expr.span),
        ExprKind::Assign(lhs, rhs) => infer_assign(checker, lhs, rhs, expr.span),
        ExprKind::Projection(base) => infer_projection(checker, base, expr.span),
        ExprKind::QuantumOp(qop) => infer_quantum_op(checker, qop, expr.span),
        ExprKind::Ascribe(inner, ty) => infer_cast(checker, inner, ty, expr.span),
        ExprKind::Break(opt_expr) => infer_break(checker, opt_expr.as_deref(), expr.span),
        ExprKind::Continue => infer_continue(expr.span),
        ExprKind::Error => Ok(Type::new(TypeKind::Error, Quantity::Many, expr.span)),
    }
}

/// Infer literal type
fn infer_literal(lit: &Literal, span: Span) -> Result<Type, TypeError> {
    let ty = match lit {
        Literal::Int(_) => TypeKind::Int,
        Literal::UInt(_) => TypeKind::UInt,
        Literal::Float(_) => TypeKind::Float,
        Literal::Bool(_) => TypeKind::Bool,
        Literal::String(_) => TypeKind::String,
        Literal::Char(_) => TypeKind::Char,
        Literal::Unit => TypeKind::Unit,
    };
    Ok(Type::new(ty, Quantity::Many, span))
}

/// Infer variable type from environment
fn infer_var(checker: &mut TypeChecker, ident: &Ident, span: Span) -> Result<Type, TypeError> {
    // `assert` is handled in infer_call, not here. It is an intrinsic rather
    // than a prelude function precisely so that no code path resolves it at
    // runtime: declaring it in the prelude would let `assert(x)` outside a
    // proof block typecheck and then lower to nothing, a silent no-op where a
    // check was written.
    debug_assert_ne!(ident.name, "assert", "assert must be handled by infer_call");

    if let Some(info) = checker.env.lookup_var(ident) {
        // Return type with the variable's declared quantity, not the inferred type's quantity
        let mut ty = info.ty.clone();
        ty.quantity = info.quantity;
        // Record the use
        checker.env.use_var(ident, span)?;
        Ok(ty)
    } else {
        Err(TypeError::VariableNotAvailable {
            name: ident.clone(),
            reason: "undefined variable".to_string(),
            span,
        })
    }
}

/// Infer binary operation type
/// The common type of an integer paired with a float, or `None` if they are not.
///
/// `None` means "fall back to unification", which produces the ordinary type-mismatch
/// diagnostic. It is deliberately NOT an error by itself: this only decides whether a
/// PROMOTION applies, and the caller keeps the existing rejection path for everything else.
///
/// Only the INTEGER/FLOAT case is handled. There is deliberately no integer/integer
/// branch: `unify_types` already accepts every int-int pair regardless of width -- `i8 +
/// i64`, `i8 + i16` and `i64 + i64` all typecheck with such a branch disabled, verified by
/// probing each -- so one here would be unreachable code. A mutation disabling it
/// survived, which is how that was established. The backend owns the width rule
/// (`promote_binary_operands` sign-extends into the wider type); duplicating the width
/// arithmetic here would only create a second place for the two to disagree.
///
/// This must agree with `promote_binary_operands`. If they diverge, the typechecker admits
/// programs the backend refuses, or the backend promotes something typed as an integer --
/// the silent-wrong-answer shape this rule exists to prevent.
fn numeric_common(lhs: &Type, rhs: &Type) -> Option<Type> {
    use crate::ast::ty::TypeKind;
    let is_int = |t: &Type| matches!(t.kind, TypeKind::Int | TypeKind::UInt | TypeKind::Nat);
    if matches!(lhs.kind, TypeKind::Float) && is_int(rhs) {
        return Some(lhs.clone());
    }
    if matches!(rhs.kind, TypeKind::Float) && is_int(lhs) {
        return Some(rhs.clone());
    }
    None
}

fn infer_binary(
    checker: &mut TypeChecker,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    span: Span,
) -> Result<Type, TypeError> {
    let lhs_ty = infer_expr(checker, lhs)?;
    let rhs_ty = infer_expr(checker, rhs)?;

    // For arithmetic/comparison ops the operands are brought to a COMMON numeric type.
    //
    // This used to `unify_types` both operands, which demanded they already match, so
    // `count * factor` with `count: i64, factor: f32` was rejected here and the LLVM
    // backend's promotion could never be reached from source. The promotion exists in
    // `codegen::llvm::expr_lowering::promote_binary_operands`; this is the rule that has to
    // agree with it.
    let result_ty = match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            match numeric_common(&lhs_ty, &rhs_ty) {
                Some(t) => t,
                None => {
                    unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
                    lhs_ty
                }
            }
        }
        // `%` is arithmetic too, but only on integers. The backend refuses a mixed
        // remainder rather than truncating the float divisor, so it is NOT promoted here
        // either -- otherwise the typechecker would admit a program the backend rejects.
        BinOp::Rem => {
            unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
            lhs_ty
        }
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if numeric_common(&lhs_ty, &rhs_ty).is_none() {
                unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
            }
            Type::new(TypeKind::Bool, Quantity::Many, span)
        }
        BinOp::And | BinOp::Or => {
            unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
            unify::unify_types(
                checker,
                &lhs_ty,
                &Type::new(TypeKind::Bool, Quantity::Many, span),
            )?;
            Type::new(TypeKind::Bool, Quantity::Many, span)
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
            unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
            lhs_ty
        }
        BinOp::Assign => {
            // LHS must be a place expression, RHS type must match
            // For now, just unify
            unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
            Type::unit(span)
        }
    };
    Ok(result_ty)
}

/// Infer unary operation type
fn infer_unary(
    checker: &mut TypeChecker,
    op: UnOp,
    operand: &Expr,
    span: Span,
) -> Result<Type, TypeError> {
    let operand_ty = infer_expr(checker, operand)?;

    let result_ty = match op {
        UnOp::Neg => operand_ty,
        UnOp::Not => {
            unify::unify_types(
                checker,
                &operand_ty,
                &Type::new(TypeKind::Bool, Quantity::Many, span),
            )?;
            Type::new(TypeKind::Bool, Quantity::Many, span)
        }
        UnOp::BitNot => operand_ty,
        UnOp::Deref => {
            // Operand should be a reference type
            // For now, just return the inner type
            match &operand_ty.kind {
                TypeKind::Projection(inner) => *inner.clone(),
                _ => Type::new(TypeKind::Error, Quantity::Many, span),
            }
        }
        UnOp::InOut => {
            // Creates a projection type
            Type::new(
                TypeKind::Projection(Box::new(operand_ty)),
                Quantity::One,
                span,
            )
        }
        UnOp::Consume => {
            // Consumes the operand, produces same type with consume mutability
            checker.env.move_var(&operand_ty.to_ident(), span)?;
            operand_ty
        }
    };
    Ok(result_ty)
}

/// Infer function call type
fn infer_call(
    checker: &mut TypeChecker,
    callee: &Expr,
    args: &[Expr],
    span: Span,
) -> Result<Type, TypeError> {
    // `assert` is an intrinsic, not a prelude function, so intercept the call
    // before the callee is inferred as a function type. infer_var would
    // otherwise return Bool (the obligation's type) and the Function arm
    // below would reject it.
    if let ExprKind::Var(ident) = &callee.kind
        && ident.name == "assert"
    {
        if !checker.in_proof {
            return Err(TypeError::AssertOutsideProof { span });
        }
        if args.len() != 1 {
            return Err(TypeError::ArgumentCountMismatch {
                expected: 1,
                found: args.len(),
                span,
            });
        }
        // The obligation must be a proposition. A bool is not accepted where
        // the expression is checked against a Float, so this is the error the
        // user sees for `assert(x)` with a non-boolean argument.
        let cond = Type::new(TypeKind::Bool, Quantity::Many, span);
        checker.check_expr(&args[0], &cond)?;
        // Unit, not bool: `assert(..);` appears in statement position, where
        // the block expects a statement of type (). The obligation's
        // proposition type is only used for checking the argument.
        return Ok(Type::unit(span));
    }

    let callee_ty = infer_expr(checker, callee)?;

    // Expect callee to be a function type
    match &callee_ty.kind {
        TypeKind::Function(params, ret) => {
            // Check argument count
            if params.len() != args.len() {
                return Err(TypeError::ArgumentCountMismatch {
                    expected: params.len(),
                    found: args.len(),
                    span,
                });
            }

            // Check each argument against parameter type
            for (param, arg) in params.iter().zip(args) {
                checker.check_expr(arg, param)?;
            }

            Ok(*ret.clone())
        }
        TypeKind::Pi(_, domain, codomain) => {
            // Dependent function application: Π(x:τ₁). τ₂
            // arg must check against domain, result is codomain[arg/x]
            if args.len() != 1 {
                return Err(TypeError::ArgumentCountMismatch {
                    expected: 1,
                    found: args.len(),
                    span,
                });
            }

            // Check argument against domain
            checker.check_expr(&args[0], domain)?;

            // Substitute arg into codomain (simplified - would need proper substitution)
            // For now, return codomain as-is
            Ok(*codomain.clone())
        }
        _ => Err(TypeError::NotAFunction {
            ty: callee_ty,
            span,
        }),
    }
}

/// Infer method call type
fn infer_method_call(
    checker: &mut TypeChecker,
    receiver: &Expr,
    _method: &Ident,
    _args: &[Expr],
    span: Span,
) -> Result<Type, TypeError> {
    let _receiver_ty = infer_expr(checker, receiver)?;

    // Look up method on receiver type
    // For now, stub - would need type class / trait system
    Ok(Type::new(TypeKind::Unit, Quantity::Many, span))
}

/// Infer field access type
fn infer_field(
    checker: &mut TypeChecker,
    base: &Expr,
    field: &Ident,
    span: Span,
) -> Result<Type, TypeError> {
    let base_ty = infer_expr(checker, base)?;

    // Look up field in struct type
    match &base_ty.kind {
        TypeKind::Named(name, _) => {
            if let Some(type_def) = checker.env.lookup_type(name) {
                if let TypeDefKind::Struct(fields) = &type_def.kind {
                    for f in fields {
                        if f.name == *field {
                            return Ok(f.ty.clone());
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Err(TypeError::FieldNotFound {
        field: field.clone(),
        ty: base_ty,
        span,
    })
}

/// Infer index access type
fn infer_index(
    checker: &mut TypeChecker,
    base: &Expr,
    index: &Expr,
    span: Span,
) -> Result<Type, TypeError> {
    let base_ty = infer_expr(checker, base)?;
    let _index_ty = infer_expr(checker, index)?;

    // For arrays/tensors, return element type
    match &base_ty.kind {
        TypeKind::Array(elem, _) => Ok(*elem.clone()),
        TypeKind::Tensor(dims) if !dims.is_empty() => Ok(dims[0].clone()),
        _ => Err(TypeError::NotIndexable { ty: base_ty, span }),
    }
}

/// Infer struct literal type
fn infer_struct(
    checker: &mut TypeChecker,
    name: &Ident,
    fields: &[FieldExpr],
    span: Span,
) -> Result<Type, TypeError> {
    // Look up struct definition
    let type_def = checker.env.lookup_type(name).cloned();
    if let Some(type_def) = type_def {
        if let TypeDefKind::Struct(struct_fields) = &type_def.kind {
            // Check each field
            for field_expr in fields {
                if let Some(field_def) = struct_fields.iter().find(|f| f.name == field_expr.name) {
                    checker.check_expr(&field_expr.value, &field_def.ty)?;
                }
            }
            Ok(Type::new(
                TypeKind::Named(name.clone(), Vec::new()),
                Quantity::Many,
                span,
            ))
        } else {
            Err(TypeError::NotAStruct {
                name: name.clone(),
                span,
            })
        }
    } else {
        Err(TypeError::UndefinedType {
            name: name.clone(),
            span,
        })
    }
}

/// Infer enum variant type
fn infer_variant(
    checker: &mut TypeChecker,
    enum_name: &Ident,
    variant_name: &Ident,
    fields: &[Expr],
    span: Span,
) -> Result<Type, TypeError> {
    // Look up enum definition
    let type_def = checker.env.lookup_type(enum_name).cloned();
    if let Some(type_def) = type_def {
        if let TypeDefKind::Enum(variants) = &type_def.kind {
            if let Some(variant) = variants.iter().find(|v| v.name == *variant_name) {
                // Check field count and types
                if variant.fields.len() != fields.len() {
                    return Err(TypeError::ArgumentCountMismatch {
                        expected: variant.fields.len(),
                        found: fields.len(),
                        span,
                    });
                }
                for (field_def, field_expr) in variant.fields.iter().zip(fields) {
                    checker.check_expr(field_expr, &field_def.ty)?;
                }
                return Ok(Type::new(
                    TypeKind::Named(enum_name.clone(), Vec::new()),
                    Quantity::Many,
                    span,
                ));
            }
        }
    }

    Err(TypeError::VariantNotFound {
        enum_name: enum_name.clone(),
        variant_name: variant_name.clone(),
        span,
    })
}

/// Infer tuple type
fn infer_tuple(checker: &mut TypeChecker, elems: &[Expr], span: Span) -> Result<Type, TypeError> {
    let mut elem_types = Vec::new();
    for elem in elems {
        elem_types.push(infer_expr(checker, elem)?);
    }
    Ok(Type::new(TypeKind::Tuple(elem_types), Quantity::Many, span))
}

#[allow(dead_code)]
/// Infer sigma (dependent pair) type
fn infer_sigma(
    checker: &mut TypeChecker,
    fst: &Expr,
    snd: &Expr,
    span: Span,
) -> Result<Type, TypeError> {
    let fst_ty = infer_expr(checker, fst)?;
    let snd_ty = infer_expr(checker, snd)?;

    // Create sigma type: Σ(x:τ₁). τ₂
    // For simplicity, we create a fresh metavariable for the dependent part
    let name = Ident::new("_", span);
    let sigma_ty = Type::new(
        TypeKind::Sigma(name, Box::new(fst_ty), Box::new(snd_ty)),
        Quantity::Many,
        span,
    );

    Ok(sigma_ty)
}

/// Infer array type
fn infer_array(checker: &mut TypeChecker, elems: &[Expr], span: Span) -> Result<Type, TypeError> {
    if elems.is_empty() {
        // Empty array - element type unknown, create metavar
        let elem_mv = fresh_meta_var();
        checker.register_meta(elem_mv, None);
        let elem_ty = Type::new(TypeKind::Var(TypeVar(elem_mv.0)), Quantity::Many, span);
        return Ok(Type::new(
            TypeKind::Array(Box::new(elem_ty), None),
            Quantity::Many,
            span,
        ));
    }

    let first_ty = infer_expr(checker, &elems[0])?;
    for elem in &elems[1..] {
        let elem_ty = infer_expr(checker, elem)?;
        unify::unify_types(checker, &first_ty, &elem_ty)?;
    }

    Ok(Type::new(
        TypeKind::Array(Box::new(first_ty), None),
        Quantity::Many,
        span,
    ))
}

/// Infer block expression type
fn infer_block(checker: &mut TypeChecker, block: &Block, span: Span) -> Result<Type, TypeError> {
    let guard = checker.env.enter_scope();

    for stmt in &block.stmts {
        check_stmt(checker, stmt)?;
    }

    let result_ty = if let Some(expr) = &block.expr {
        infer_expr(checker, expr)?
    } else {
        Type::unit(span)
    };

    checker.env.exit_scope(guard)?;
    Ok(result_ty)
}

/// Infer if expression type
fn infer_if(
    checker: &mut TypeChecker,
    cond: &Expr,
    then_branch: &Expr,
    else_branch: Option<&Expr>,
    span: Span,
) -> Result<Type, TypeError> {
    // Condition must be Bool
    let cond_ty = infer_expr(checker, cond)?;
    unify::unify_types(
        checker,
        &cond_ty,
        &Type::new(TypeKind::Bool, Quantity::Many, span),
    )?;

    // Each branch is inferred from the SAME entry state, then the states are
    // joined. Inferring them sequentially against one shared state was unsound in
    // both directions: a consume in the `then` branch left the value moved, so the
    // `else` branch was falsely rejected as a use-after-move, and a consume in only
    // one branch was never reported as a leak.
    let entry = checker.env.snapshot_linear();

    checker.env.restore_linear(&entry);
    let then_ty = infer_expr(checker, then_branch)?;
    let then_state = checker.env.snapshot_linear();

    let (else_ty, else_state) = match else_branch {
        Some(else_expr) => {
            checker.env.restore_linear(&entry);
            let ty = infer_expr(checker, else_expr)?;
            (Some(ty), checker.env.snapshot_linear())
        }
        // Without an `else` there is still a second path -- the one where the
        // condition is false and the body never runs. Joining only the `then`
        // branch would treat a conditional consume as unconditional, so the
        // implicit no-op path is joined as a branch that consumes nothing.
        None => (None, entry.clone()),
    };

    checker
        .env
        .join_linear(&entry, &[then_state, else_state], span)?;

    match else_ty {
        Some(else_ty) => {
            unify::unify_types(checker, &then_ty, &else_ty)?;
            Ok(then_ty)
        }
        None => {
            // If without else returns Unit
            unify::unify_types(checker, &then_ty, &Type::unit(span))?;
            Ok(Type::unit(span))
        }
    }
}

/// Infer match expression type
fn infer_match(
    checker: &mut TypeChecker,
    scrutinee: &Expr,
    arms: &[MatchArm],
    span: Span,
) -> Result<Type, TypeError> {
    let scrutinee_ty = infer_expr(checker, scrutinee)?;

    if arms.is_empty() {
        return Ok(Type::unit(span));
    }

    // Every arm is inferred from the SAME entry state and the results are joined,
    // for the same reason as `if`: a sequential walk over one shared state both
    // falsely rejects a correct consume-in-every-arm match and misses a leak when
    // only some arms consume.
    //
    // Pattern bindings are bound per arm inside that arm's own scope, so they are
    // local to the arm and do not take part in the join. The entry snapshot is
    // taken BEFORE any pattern binding, so a name bound by one arm cannot leak
    // into another's entry state.
    let entry = checker.env.snapshot_linear();

    let mut result_ty: Option<Type> = None;
    let mut branch_states = Vec::with_capacity(arms.len());

    for arm in arms {
        checker.env.restore_linear(&entry);

        let bindings = checker.check_pattern(&arm.pattern, &scrutinee_ty)?;
        let guard = checker.env.enter_scope();
        for (name, info) in bindings.vars {
            checker
                .env
                .bind_var(name, info.ty, info.quantity, info.mutability);
        }

        let arm_ty = if let Some(guard_expr) = &arm.guard {
            infer_expr(checker, guard_expr)?
        } else {
            infer_expr(checker, &arm.body)?
        };

        checker.env.exit_scope(guard)?;
        branch_states.push(checker.env.snapshot_linear());

        if let Some(expected) = result_ty.clone() {
            unify::unify_types(checker, &expected, &arm_ty)?;
        } else {
            result_ty = Some(arm_ty);
        }
    }

    checker.env.join_linear(&entry, &branch_states, span)?;

    // A guarded arm may not be taken, so a consume that happens ONLY under a
    // guard leaves a path unconsumed and cannot be proven. Guarded arms are
    // therefore excluded from the join above -- their state is the entry state,
    // since a guarded consume does not count as an unconditional one -- and any
    // arm that consumes under a guard is reported here.
    //
    // This is conservative in the safe direction: it rejects a program it cannot
    // prove, rather than accepting an unproven one. Making it precise would need
    // an exhaustiveness analysis this checker does not have.
    if arms.iter().any(|arm| arm.guard.is_some()) {
        // A guarded arm's state was recorded with the entry state (the consume was
        // not treated as unconditional), so re-check by inspecting each guarded arm
        // against the state it produced.
        for (arm, state) in arms.iter().zip(branch_states.iter()) {
            if arm.guard.is_some() && state.any_consumed() {
                return Err(TypeError::LinearConsumedUnderGuard { span: arm.span });
            }
        }
    }

    Ok(result_ty.unwrap_or_else(|| Type::unit(span)))
}

/// Infer let binding type
fn infer_let(
    checker: &mut TypeChecker,
    binding: &LetBinding,
    _span: Span,
) -> Result<Type, TypeError> {
    let value_ty = infer_expr(checker, &binding.value)?;

    // If explicit type annotation, check against it
    if let Some(ann_ty) = &binding.ty {
        unify::unify_types(checker, &value_ty, ann_ty)?;
    }

    // The binding's quantity is inherited from the initializer unless it was
    // annotated. `parse_quantity` defaults an unannotated `let` to `Quantity::Many`,
    // so without this a linear value is silently WIDENED by binding it:
    //
    //     fn f(x: [1] i32) { let y = x; let _ = y; let _ = y; }
    //
    // `y` became `[*]`, so the "use a `[1]` value twice" rule never applied to it,
    // and the same widening also defeated the leak check on `x`:
    //
    //     fn f(x: [1] i32) { let y = x; let _ = y; }   // x never consumed: ACCEPTED
    //
    // That is a complete escape from the linear-type discipline through a single
    // intervening `let`, which is the property this checker exists to enforce.
    //
    // Only `Quantity::Many` is replaced, since that is both the unannotated default
    // and an explicit `[*]`. Inheriting on an explicit `[*]` is deliberate: widening
    // a `[1]` value is never something to permit silently, and there is no way at
    // this point to tell an explicit `[*]` from the default -- confirmed by
    // `naso parse`, which reports `Many` for both. A mutation that always inherits
    // is therefore behaviourally EQUIVALENT here, not merely untested.
    //
    // REACHABILITY: this function is currently dead. `ExprKind::Let` (a `let` used
    // as an expression) has no parser production -- `let g = let x = 1;` panics the
    // parser -- so only the `StmtKind::Let` path in check.rs is reachable today.
    // The fix is kept here so the two `let` paths agree if let-as-expression is
    // ever given a syntax, but it is NOT load-bearing yet and no test covers it,
    // because nothing can exercise it.
    let bound_quantity = if binding.quantity == Quantity::Many {
        value_ty.quantity
    } else {
        binding.quantity
    };

    // Bind the variable
    checker.env.bind_var(
        binding.name.clone(),
        value_ty.clone(),
        bound_quantity,
        binding.mutability,
    );

    Ok(value_ty)
}

/// Infer inout let binding type
fn infer_let_inout(
    checker: &mut TypeChecker,
    binding: &LetInOutBinding,
    span: Span,
) -> Result<Type, TypeError> {
    let value_ty = infer_expr(checker, &binding.value)?;

    // Value must be a place expression with quantity 1
    // For now, just check quantity
    if value_ty.quantity != Quantity::One {
        return Err(TypeError::InOutRequiresUnique {
            found_qty: value_ty.quantity,
            span,
        });
    }

    // Bind as inout
    checker.env.bind_var(
        binding.name.clone(),
        value_ty.clone(),
        Quantity::One,
        Mutability::InOut,
    );

    Ok(value_ty)
}

/// Infer consume let binding type
fn infer_let_consume(
    checker: &mut TypeChecker,
    binding: &LetConsumeBinding,
    span: Span,
) -> Result<Type, TypeError> {
    let value_ty = infer_expr(checker, &binding.value)?;

    // Value must be a place expression with quantity 1
    if value_ty.quantity != Quantity::One {
        return Err(TypeError::QuantityMismatch {
            expected: Quantity::One,
            found: value_ty.quantity,
            span,
        });
    }

    // Bind as consume with the inferred type
    checker.env.bind_var(
        binding.name.clone(),
        value_ty.clone(),
        Quantity::One,
        Mutability::Consume,
    );

    Ok(value_ty)
}

/// Infer reversible block type
fn infer_reversible(
    checker: &mut TypeChecker,
    block: &ReversibleBlock,
    span: Span,
) -> Result<Type, TypeError> {
    let prev_reversible = checker.in_reversible;
    checker.in_reversible = true;

    let guard = checker.env.enter_scope();

    // Check body statements
    for stmt in &block.body.stmts {
        check_stmt(checker, stmt)?;
    }

    // Verify all variables in scope have inverses registered
    // (stub for now)

    checker.env.exit_scope(guard)?;
    checker.in_reversible = prev_reversible;

    Ok(Type::unit(span))
}

/// Infer lambda type
fn infer_lambda(
    checker: &mut TypeChecker,
    lambda: &LambdaExpr,
    span: Span,
) -> Result<Type, TypeError> {
    let guard = checker.env.enter_scope();

    // Bind parameters
    for param in &lambda.params {
        checker.env.bind_var(
            param.name.clone(),
            param.ty.clone(),
            param.quantity,
            param.mutability,
        );
    }

    // Check body
    let body_ty = if let Some(ret_ty) = &lambda.ret_ty {
        checker.check_expr(&lambda.body, ret_ty)?;
        ret_ty.clone()
    } else {
        infer_expr(checker, &lambda.body)?
    };

    // Build function type
    let param_types: Vec<Type> = lambda.params.iter().map(|p| p.ty.clone()).collect();

    // Check if this is a dependent lambda (Pi type)
    // If any parameter has a type that depends on a previous parameter, use Pi type
    let fn_ty = if lambda.params.iter().any(|p| {
        // Check if parameter type contains dependent types
        matches!(&p.ty.kind, TypeKind::Pi(_, _, _) | TypeKind::Sigma(_, _, _))
    }) {
        // Build dependent function type (Pi)
        let mut codomain = body_ty;
        for param in lambda.params.iter().rev() {
            let name = param.name.clone();
            let domain = param.ty.clone();
            codomain = Type::new(
                TypeKind::Pi(name, Box::new(domain), Box::new(codomain)),
                Quantity::Many,
                span,
            );
        }
        codomain
    } else {
        Type::new(
            TypeKind::Function(param_types, Box::new(body_ty)),
            Quantity::Many,
            span,
        )
    };

    checker.env.exit_scope(guard)?;
    Ok(fn_ty)
}

/// Infer for loop type
fn infer_for(checker: &mut TypeChecker, for_loop: &ForLoop, span: Span) -> Result<Type, TypeError> {
    let iter_ty = infer_expr(checker, &for_loop.iter)?;

    // Iterator type should be iterable
    // For now, just bind the loop variable
    let guard = checker.env.enter_scope();
    checker.env.bind_var(
        for_loop.var.clone(),
        iter_ty, // Simplified - should be element type
        Quantity::Many,
        Mutability::Immutable,
    );

    check_block(checker, &for_loop.body)?;
    checker.env.exit_scope(guard)?;

    Ok(Type::unit(span))
}

/// Infer forall loop type (parallel polyhedral loop)
fn infer_forall(
    checker: &mut TypeChecker,
    forall_loop: &ForallLoop,
    span: Span,
) -> Result<Type, TypeError> {
    // Forall loops have multiple bindings with range expressions
    let guard = checker.env.enter_scope();

    for (var, lower, upper) in &forall_loop.bindings {
        // Check that bounds are integers
        let _lower_ty = infer_expr(checker, lower)?;
        let _upper_ty = infer_expr(checker, upper)?;

        // For simplicity, assume bounds are integer types
        // Bind the loop variable as integer type
        checker.env.bind_var(
            var.clone(),
            Type::new(TypeKind::Int, Quantity::Many, span),
            Quantity::Many,
            Mutability::Immutable,
        );
    }

    check_block(checker, &forall_loop.body)?;
    checker.env.exit_scope(guard)?;

    Ok(Type::unit(span))
}

/// Infer a quantified proposition: `forall i in a..b { predicate }`.
///
/// Always has type bool, unlike the loop form which is unit. The bound
/// variables are bound in an inner scope so they cannot leak, and the body's
/// tail expression is checked against bool. A body with no tail expression is
/// an error rather than vacuously true: `forall i in 0..N { output[i] = 0; }`
/// states no proposition, and silently treating it as `true` would let an
/// obligation that asserts nothing pass.
fn infer_quantified(
    checker: &mut TypeChecker,
    quant: &ForallLoop,
    span: Span,
) -> Result<Type, TypeError> {
    let guard = checker.env.enter_scope();

    for (var, lower, upper) in &quant.bindings {
        let _lower_ty = infer_expr(checker, lower)?;
        let _upper_ty = infer_expr(checker, upper)?;
        checker.env.bind_var(
            var.clone(),
            Type::new(TypeKind::Int, Quantity::Many, span),
            Quantity::Many,
            Mutability::Immutable,
        );
    }

    for stmt in &quant.body.stmts {
        check_stmt(checker, stmt)?;
    }

    let bool_ty = Type::new(TypeKind::Bool, Quantity::Many, span);
    match &quant.body.expr {
        Some(predicate) => {
            let pred_ty = infer_expr(checker, predicate)?;
            unify::unify_types(checker, &pred_ty, &bool_ty)?;
        }
        None => {
            return Err(TypeError::QuantifiedBodyNotBool {
                span: quant.body.span,
            });
        }
    }

    checker.env.exit_scope(guard)?;
    Ok(bool_ty)
}

/// Infer while loop type
fn infer_while(
    checker: &mut TypeChecker,
    cond: &Expr,
    body: &Expr,
    span: Span,
) -> Result<Type, TypeError> {
    let cond_ty = infer_expr(checker, cond)?;
    unify::unify_types(
        checker,
        &cond_ty,
        &Type::new(TypeKind::Bool, Quantity::Many, span),
    )?;

    infer_expr(checker, body)?;
    Ok(Type::unit(span))
}

/// Infer return expression type
fn infer_return(
    checker: &mut TypeChecker,
    opt_expr: Option<&Expr>,
    span: Span,
) -> Result<Type, TypeError> {
    if let Some(expr) = opt_expr {
        let expr_ty = infer_expr(checker, expr)?;
        if let Some(expected) = checker.current_fn_ret.clone() {
            unify::unify_types(checker, &expr_ty, &expected)?;
        }
    } else {
        // Empty return - check against Unit
        if let Some(expected) = checker.current_fn_ret.clone() {
            unify::unify_types(checker, &Type::unit(span), &expected)?;
        }
    }
    Ok(Type::never(span)) // Return type is ! (never)
}

/// Infer assignment type
fn infer_assign(
    checker: &mut TypeChecker,
    lhs: &Expr,
    rhs: &Expr,
    span: Span,
) -> Result<Type, TypeError> {
    let lhs_ty = infer_expr(checker, lhs)?;
    let rhs_ty = infer_expr(checker, rhs)?;

    // LHS must be a place expression
    // For now, just unify types
    unify::unify_types(checker, &lhs_ty, &rhs_ty)?;
    Ok(Type::unit(span))
}

/// Infer projection type
fn infer_projection(checker: &mut TypeChecker, base: &Expr, span: Span) -> Result<Type, TypeError> {
    let base_ty = infer_expr(checker, base)?;

    // Create projection type
    Ok(Type::new(
        TypeKind::Projection(Box::new(base_ty)),
        Quantity::One,
        span,
    ))
}

/// The number of qubits a gate takes, or `None` when it is variable.
///
/// `None` means the arity is decided elsewhere: `RX`/`RY`/`RZ` each take a qubit
/// plus an angle, so their AST form differs, and `Custom` names a gate this
/// compiler does not know -- refusing an unknown gate's arity would be guessing.
fn gate_arity(gate: &crate::ast::GateKind) -> Option<usize> {
    use crate::ast::GateKind as G;
    Some(match gate {
        // single-qubit gates
        G::H | G::X | G::Y | G::Z | G::S | G::T | G::Reset => 1,
        // controlled gates: (control, target)
        G::CX | G::CY | G::CZ => 2,
        // rotations take an angle, which is not a qubit; the qubit count is 1 and
        // the angle is not part of `args`.
        G::RX(_) | G::RY(_) | G::RZ(_) => 1,
        G::Custom(_) => return None,
    })
}

/// Infer quantum operation type
fn infer_quantum_op(
    checker: &mut TypeChecker,
    qop: &QuantumOp,
    span: Span,
) -> Result<Type, TypeError> {
    match qop {
        QuantumOp::Alloc(_name) => {
            // qalloc() returns a new qubit with quantity One
            Ok(Type::qubit(span))
        }
        QuantumOp::Measure(target) => {
            let target_ty = infer_expr(checker, target)?;
            // Target must be Qubit @ 1
            unify::unify_types(checker, &target_ty, &Type::qubit(span))?;
            // Mark the qubit variable as moved - extract ident if it's a variable
            if let ExprKind::Var(ident) = &target.kind {
                checker.env.move_var(ident, span)?;
            } else {
                // Fallback for non-variable expressions (shouldn't happen for measure)
                checker.env.move_var(&target_ty.to_ident(), span)?;
            }
            // The RESULT is a classical bit, so `[*]`, not `[1]`.
            //
            // This arm is the one that actually runs: the parser builds a dedicated
            // `QuantumOp::Measure` node, so `measure(q)` never resolves through the
            // prelude signature. The prelude entry agrees, but editing only the
            // prelude changes nothing -- which is why the same program was accepted
            // by one path and rejected by the other during this work.
            //
            // A measurement READS a collapsed qubit. It is not a resource: copying
            // it, branching on it, and discarding it are all fine. Returning `[1]`
            // made every `let r = measure(q);` bind a linear `r`, so a correct
            // program was rejected with "unused linear variable `r`".
            Ok(Type::new(TypeKind::Bool, Quantity::Many, span))
        }
        QuantumOp::ApplyGate(gate, args) => {
            // Arity is checked here because the gate was previously ignored
            // (`ApplyGate(_gate, args)`), so ANY argument count was accepted:
            //
            //     hadamard(a, b)   -> OK
            //     cnot(a, b, c)    -> OK
            //     reset(a, b)      -> OK
            //
            // Each argument is checked against `Type::qubit` below, so the types
            // were right; only the COUNT was unverified. A two-qubit `hadamard` is
            // not a thing, and silently accepting one means a circuit that is not
            // what the source says compiles -- which then lowers to a PIR op with
            // two qubits and runs on hardware as something the author did not write.
            let expected = gate_arity(gate);
            if let Some(expected) = expected
                && args.len() != expected
            {
                return Err(TypeError::ArgumentCountMismatch {
                    expected,
                    found: args.len(),
                    span,
                });
            }

            // Quantum gates are in-place operations that borrow qubits temporarily
            // They don't consume the qubit and don't create persistent borrows
            for arg in args {
                // Get the type without consuming the variable (borrow mode)
                // For variables, we look up the type directly without recording a use
                let arg_ty = if let ExprKind::Var(ident) = &arg.kind {
                    if let Some(info) = checker.env.lookup_var(ident) {
                        // Check that the qubit is available (not moved)
                        if info.moved {
                            return Err(TypeError::UseOfMovedValue {
                                name: ident.clone(),
                                moved_at: info.used_at.last().cloned().unwrap_or(span),
                                used_at: span,
                            });
                        }
                        // Check for Zero quantity - erased variables cannot be used at runtime
                        if info.quantity == Quantity::Zero {
                            return Err(TypeError::ErasedVariableUsedAtRuntime {
                                name: ident.clone(),
                                span: arg.span,
                            });
                        }
                        let mut ty = info.ty.clone();
                        ty.quantity = info.quantity;
                        // Don't call use_var here - we're borrowing temporarily, not consuming
                        ty
                    } else {
                        return Err(TypeError::VariableNotAvailable {
                            name: ident.clone(),
                            reason: "undefined variable".to_string(),
                            span: arg.span,
                        });
                    }
                } else {
                    // For non-variable expressions (e.g., field access), infer normally
                    infer_expr(checker, arg)?
                };
                // Validate that argument is a Qubit with quantity 1
                unify::unify_types(checker, &arg_ty, &Type::qubit(span))?;
                // No persistent borrow - gate borrows only for the expression duration
            }
            Ok(Type::unit(span))
        }
        QuantumOp::Entangle(args) => {
            for arg in args {
                let arg_ty = infer_expr(checker, arg)?;
                unify::unify_types(checker, &arg_ty, &Type::qubit(span))?;
                checker.env.move_var(&arg_ty.to_ident(), span)?;
            }
            // Returns QRegister with dimension = number of qubits
            Ok(Type::new(
                TypeKind::QRegister(args.iter().map(|_| Type::qubit(span)).collect()),
                Quantity::One,
                span,
            ))
        }
        QuantumOp::Phase(_, _) => Ok(Type::unit(span)),
        QuantumOp::Hamiltonian(_, _) => Ok(Type::unit(span)),
    }
}

/// Infer break expression type
fn infer_break(
    checker: &mut TypeChecker,
    opt_expr: Option<&Expr>,
    span: Span,
) -> Result<Type, TypeError> {
    if let Some(expr) = opt_expr {
        infer_expr(checker, expr)?;
    }
    // Break is a diverging expression - returns Never type
    Ok(Type::never(span))
}

/// Infer continue expression type
fn infer_continue(span: Span) -> Result<Type, TypeError> {
    // Continue is a diverging expression - returns Never type
    Ok(Type::never(span))
}
