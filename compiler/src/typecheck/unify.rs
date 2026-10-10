//! Unification for the Naso type checker
//!
//! Implements type and quantity unification with metavariable solving
//! Quantity lattice: Zero <: One <: Bounded(N) <: Many

#![allow(clippy::result_large_err)]

use crate::ast::*;
use crate::typecheck::error::TypeError;
use crate::typecheck::*;

/// Quantity lattice: Zero <: One <: Bounded(N) <: Many
///
/// Join (LUB) table:
/// | q1 \ q2 | Zero | One | Bounded(M) | Many |
/// |---------|------|-----|------------|------|
/// | Zero    | Zero | One | Bounded(M) | Many |
/// | One     | One  | One | Bounded(max(1,M)) | Many |
/// | Bounded(N) | Bounded(N) | Bounded(max(1,N)) | Bounded(max(N,M)) | Many |
/// | Many    | Many | Many | Many | Many |
///
/// Errors: Zero only unifies with Zero.
/// Unify two types, accumulating constraints in the checker
pub fn unify_types(checker: &mut TypeChecker, ty1: &Type, ty2: &Type) -> Result<Type, TypeError> {
    // Handle metavariables first
    if let TypeKind::Meta(mv) = &ty1.kind {
        return unify_meta_var(checker, *mv, ty2);
    }
    if let TypeKind::Meta(mv) = &ty2.kind {
        return unify_meta_var(checker, *mv, ty1);
    }

    // Unify quantities with error reporting for Zero mismatch
    let unified_qty = unify_quantity(ty1.quantity, ty2.quantity, ty1.span.merge(ty2.span))?;

    // Unify type kinds
    let unified_kind = unify_kinds(checker, &ty1.kind, &ty2.kind)?;

    Ok(Type::new(
        unified_kind,
        unified_qty,
        ty1.span.merge(ty2.span),
    ))
}

/// Unify two type kinds
fn unify_kinds(
    checker: &mut TypeChecker,
    k1: &TypeKind,
    k2: &TypeKind,
) -> Result<TypeKind, TypeError> {
    match (k1, k2) {
        // Exact matches
        (TypeKind::Unit, TypeKind::Unit) => Ok(TypeKind::Unit),
        (TypeKind::Bool, TypeKind::Bool) => Ok(TypeKind::Bool),
        (TypeKind::Int, TypeKind::Int) => Ok(TypeKind::Int),
        (TypeKind::UInt, TypeKind::UInt) => Ok(TypeKind::UInt),
        (TypeKind::Float, TypeKind::Float) => Ok(TypeKind::Float),
        (TypeKind::Nat, TypeKind::Nat) => Ok(TypeKind::Nat),
        (TypeKind::Quint8, TypeKind::Quint8) => Ok(TypeKind::Quint8),
        (TypeKind::Qubit, TypeKind::Qubit) => Ok(TypeKind::Qubit),
        (TypeKind::String, TypeKind::String) => Ok(TypeKind::String),
        (TypeKind::Char, TypeKind::Char) => Ok(TypeKind::Char),
        (TypeKind::Error, _) | (_, TypeKind::Error) => Ok(TypeKind::Error),

        // Named types
        (TypeKind::Named(name1, args1), TypeKind::Named(name2, args2)) => {
            if name1.name != name2.name {
                return Err(TypeError::TypeMismatch {
                    expected: Type::new(k1.clone(), Quantity::Many, Span::default()),
                    found: Type::new(k2.clone(), Quantity::Many, Span::default()),
                    span: Span::default(),
                });
            }
            // Unify type arguments
            if args1.len() != args2.len() {
                return Err(TypeError::TypeArgumentCountMismatch {
                    name: name1.clone(),
                    expected: args1.len(),
                    found: args2.len(),
                    span: Span::default(),
                });
            }
            let mut unified_args = Vec::new();
            for (a1, a2) in args1.iter().zip(args2) {
                unified_args.push(unify_type_args(checker, a1, a2)?);
            }
            Ok(TypeKind::Named(name1.clone(), unified_args))
        }

        // Tuple types
        (TypeKind::Tuple(elems1), TypeKind::Tuple(elems2)) => {
            if elems1.len() != elems2.len() {
                return Err(TypeError::TypeMismatch {
                    expected: Type::new(k1.clone(), Quantity::Many, Span::default()),
                    found: Type::new(k2.clone(), Quantity::Many, Span::default()),
                    span: Span::default(),
                });
            }
            let mut unified_elems = Vec::new();
            for (e1, e2) in elems1.iter().zip(elems2) {
                unified_elems.push(unify_types(checker, e1, e2)?);
            }
            Ok(TypeKind::Tuple(unified_elems))
        }

        // Function types
        (TypeKind::Function(params1, ret1), TypeKind::Function(params2, ret2)) => {
            if params1.len() != params2.len() {
                return Err(TypeError::TypeMismatch {
                    expected: Type::new(k1.clone(), Quantity::Many, Span::default()),
                    found: Type::new(k2.clone(), Quantity::Many, Span::default()),
                    span: Span::default(),
                });
            }
            let mut unified_params = Vec::new();
            for (p1, p2) in params1.iter().zip(params2.iter()) {
                unified_params.push(unify_types(checker, p1, p2)?);
            }
            let unified_ret = unify_types(checker, ret1, ret2)?;
            Ok(TypeKind::Function(unified_params, Box::new(unified_ret)))
        }

        // Projection types
        (TypeKind::Projection(inner1), TypeKind::Projection(inner2)) => {
            let unified = unify_types(checker, inner1, inner2)?;
            Ok(TypeKind::Projection(Box::new(unified)))
        }

        // Reversible types
        (TypeKind::Reversible(inner1), TypeKind::Reversible(inner2)) => {
            let unified = unify_types(checker, inner1, inner2)?;
            Ok(TypeKind::Reversible(Box::new(unified)))
        }

        // Dependent types - Pi
        (TypeKind::Pi(name1, domain1, codomain1), TypeKind::Pi(_name2, domain2, codomain2)) => {
            let unified_domain = unify_types(checker, domain1, domain2)?;
            let unified_codomain = unify_types(checker, codomain1, codomain2)?;
            Ok(TypeKind::Pi(
                name1.clone(),
                Box::new(unified_domain),
                Box::new(unified_codomain),
            ))
        }

        // Dependent types - Sigma
        (TypeKind::Sigma(name1, fst1, snd1), TypeKind::Sigma(_name2, fst2, snd2)) => {
            let unified_fst = unify_types(checker, fst1, fst2)?;
            let unified_snd = unify_types(checker, snd1, snd2)?;
            Ok(TypeKind::Sigma(
                name1.clone(),
                Box::new(unified_fst),
                Box::new(unified_snd),
            ))
        }

        // Type-level lambda
        (TypeKind::Lambda(param1, body1), TypeKind::Lambda(_param2, body2)) => {
            let unified_body = unify_types(checker, body1, body2)?;
            Ok(TypeKind::Lambda(param1.clone(), Box::new(unified_body)))
        }

        // Type application
        (TypeKind::App(fun1, arg1), TypeKind::App(fun2, arg2)) => {
            let unified_fun = unify_types(checker, fun1, fun2)?;
            let unified_arg = unify_types(checker, arg1, arg2)?;
            Ok(TypeKind::App(Box::new(unified_fun), Box::new(unified_arg)))
        }

        // Universe levels
        (TypeKind::Universe(l1), TypeKind::Universe(l2)) if l1 == l2 => Ok(TypeKind::Universe(*l1)),

        // Type variables
        (TypeKind::Var(v1), TypeKind::Var(v2)) if v1 == v2 => Ok(TypeKind::Var(*v1)),

        // Quint8 coerces to UInt (it's an unsigned 8-bit value stored in u32 storage).
        // This lets bitwise shifts and ANDs work when mixing quint8 tensor elements
        // with integer literals in dequantization.
        (TypeKind::UInt, TypeKind::Quint8) => Ok(TypeKind::UInt),
        (TypeKind::Quint8, TypeKind::UInt) => Ok(TypeKind::UInt),
        (TypeKind::Int, TypeKind::Quint8) => Ok(TypeKind::Int),
        (TypeKind::Quint8, TypeKind::Int) => Ok(TypeKind::Int),
        // Metavariables handled above

        // Mismatch
        _ => Err(TypeError::TypeMismatch {
            expected: Type::new(k1.clone(), Quantity::Many, Span::default()),
            found: Type::new(k2.clone(), Quantity::Many, Span::default()),
            span: Span::default(),
        }),
    }
}

/// Unify type arguments
fn unify_type_args(
    checker: &mut TypeChecker,
    a1: &TypeArg,
    a2: &TypeArg,
) -> Result<TypeArg, TypeError> {
    match (a1, a2) {
        (TypeArg::Type(t1), TypeArg::Type(t2)) => Ok(TypeArg::Type(unify_types(checker, t1, t2)?)),
        (TypeArg::Nat(n1), TypeArg::Nat(n2)) => {
            // For now, require exact match
            if n1 == n2 {
                Ok(TypeArg::Nat(n1.clone()))
            } else {
                Err(TypeError::TypeMismatch {
                    expected: Type::new(TypeKind::Nat, Quantity::Many, Span::default()),
                    found: Type::new(TypeKind::Nat, Quantity::Many, Span::default()),
                    span: Span::default(),
                })
            }
        }
        (TypeArg::Quantity(q1), TypeArg::Quantity(q2)) => {
            let unified = qty_join(*q1, *q2);
            Ok(TypeArg::Quantity(unified))
        }
        _ => Err(TypeError::TypeMismatch {
            expected: Type::new(TypeKind::Error, Quantity::Many, Span::default()),
            found: Type::new(TypeKind::Error, Quantity::Many, Span::default()),
            span: Span::default(),
        }),
    }
}

/// Unify a metavariable with a type
fn unify_meta_var(checker: &mut TypeChecker, mv: MetaVar, ty: &Type) -> Result<Type, TypeError> {
    // Check if already solved
    let solution = checker.lookup_meta(&mv).cloned();
    if let Some(Some(solution)) = solution {
        return unify_types(checker, &solution, ty);
    }

    // Occurs check
    if occurs_in_type(mv, ty) {
        return Err(TypeError::OccursCheck {
            meta_var: mv,
            ty: ty.clone(),
            span: ty.span,
        });
    }

    // Register solution
    checker.register_meta(mv, Some(ty.clone()));
    Ok(ty.clone())
}

/// Check if a metavariable occurs in a type
fn occurs_in_type(mv: MetaVar, ty: &Type) -> bool {
    occurs_in_kind(mv, &ty.kind)
}

fn occurs_in_kind(mv: MetaVar, kind: &TypeKind) -> bool {
    match kind {
        TypeKind::Meta(m) => *m == mv,
        TypeKind::Function(params, ret) => {
            params.iter().any(|p| occurs_in_type(mv, p)) || occurs_in_type(mv, ret)
        }
        TypeKind::Projection(inner) => occurs_in_type(mv, inner),
        TypeKind::Reversible(inner) => occurs_in_type(mv, inner),
        TypeKind::Pi(_, domain, codomain) => {
            occurs_in_type(mv, domain) || occurs_in_type(mv, codomain)
        }
        TypeKind::Sigma(_, fst, snd) => occurs_in_type(mv, fst) || occurs_in_type(mv, snd),
        TypeKind::Lambda(_, body) => occurs_in_type(mv, body),
        TypeKind::App(fun, arg) => occurs_in_type(mv, fun) || occurs_in_type(mv, arg),
        TypeKind::Named(_, args) => args.iter().any(|a| occurs_in_arg(mv, a)),
        TypeKind::QRegister(dims) | TypeKind::Tensor(dims) => {
            dims.iter().any(|d| occurs_in_type(mv, d))
        }
        _ => false,
    }
}

fn occurs_in_arg(mv: MetaVar, arg: &TypeArg) -> bool {
    match arg {
        TypeArg::Type(ty) => occurs_in_type(mv, ty),
        _ => false,
    }
}

/// Compute the join (least upper bound) of two quantities
/// Lattice: Zero < One < Bounded(N) < Many
/// Join returns the least quantity that is >= both inputs
/// Per spec join table:
/// | q1 \ q2 | Zero | One | Bounded(M) | Many |
/// |---------|------|-----|------------|------|
/// | Zero    | Zero | One | Bounded(M) | Many |
/// | One     | One  | One | Bounded(max(1,M)) | Many |
/// | Bounded(N) | Bounded(N) | Bounded(max(1,N)) | Bounded(max(N,M)) | Many |
/// | Many    | Many | Many | Many | Many |
pub fn qty_join(q1: Quantity, q2: Quantity) -> Quantity {
    use Quantity::*;
    match (q1, q2) {
        (a, b) if a == b => a,
        (Zero, b) | (b, Zero) => b,
        (One, Many) | (Many, One) => Many,
        (One, Bounded(n)) | (Bounded(n), One) if n >= 1 => Bounded(n.max(1)),
        (Bounded(0), _) | (_, Bounded(0)) => Zero,
        (Bounded(n1), Bounded(n2)) => Bounded(n1.max(n2)),
        (Bounded(_), Many) | (Many, Bounded(_)) => Many,
        (Many, Many) => Many,
        (a, b) => {
            // Fallback: take the maximum in the lattice order
            let order = |q: &Quantity| match q {
                Zero => 0,
                One => 1,
                Bounded(n) => 2 + *n as usize,
                Many => 1000,
            };
            if order(&a) > order(&b) { a } else { b }
        }
    }
}

/// Compute the meet (greatest lower bound) of two quantities
/// Meet returns the greatest quantity that is <= both inputs
pub fn qty_meet(q1: Quantity, q2: Quantity) -> Quantity {
    use Quantity::*;
    match (q1, q2) {
        (a, b) if a == b => a,
        (Zero, _) | (_, Zero) => Zero,
        (One, Many) | (Many, One) => One,
        (One, Bounded(n)) | (Bounded(n), One) if n >= 1 => One,
        (Bounded(n1), Bounded(n2)) => Bounded(n1.min(n2)),
        (Bounded(n), Many) | (Many, Bounded(n)) => Bounded(n),
        (Many, Many) => Many,
        (a, b) => {
            // Fallback: take the less restrictive (higher in lattice)
            let order = |q: &Quantity| match q {
                Zero => 0,
                One => 1,
                Bounded(n) => 2 + *n as usize,
                Many => 1000,
            };
            if order(&a) < order(&b) { a } else { b }
        }
    }
}

/// Check if q1 <= q2 (subtyping in the quantity lattice)
/// Zero <= One <= Bounded(N) <= Many
pub fn qty_subtype(q1: Quantity, q2: Quantity) -> bool {
    match (q1, q2) {
        (a, b) if a == b => true,
        (Quantity::Zero, _) => true,
        (Quantity::One, Quantity::Many) => true,
        (Quantity::One, Quantity::Bounded(n)) if n >= 1 => true,
        (Quantity::Bounded(n1), Quantity::Bounded(n2)) => n1 <= n2,
        (Quantity::Bounded(_), Quantity::Many) => true,
        (_, Quantity::Many) => true,
        _ => false,
    }
}

/// Unify two quantities with error reporting (used when unification must fail on Zero vs non-Zero)
pub fn unify_quantity(q1: Quantity, q2: Quantity, span: Span) -> Result<Quantity, TypeError> {
    // Zero can only unify with Zero
    if q1 == Quantity::Zero && q2 != Quantity::Zero {
        return Err(TypeError::QuantityMismatch {
            expected: q1,
            found: q2,
            span,
        });
    }
    if q2 == Quantity::Zero && q1 != Quantity::Zero {
        return Err(TypeError::QuantityMismatch {
            expected: q2,
            found: q1,
            span,
        });
    }

    // For all other cases, return the join
    Ok(qty_join(q1, q2))
}

/// Quantity arithmetic for consumption
pub fn qty_consume(q: Quantity) -> Quantity {
    use Quantity::*;
    match q {
        Zero => Zero,
        One => Zero, // Consuming a linear variable makes it unavailable
        Bounded(0) => Zero,
        Bounded(n) => Bounded(n.saturating_sub(1)),
        Many => Many,
    }
}

/// Check if a quantity is erasable (can be used at compile-time only)
pub fn is_erasable(q: Quantity) -> bool {
    q == Quantity::Zero
}

/// Check if a quantity is linear (must be used exactly once)
pub fn is_linear(q: Quantity) -> bool {
    q == Quantity::One
}

/// Check if a quantity is unrestricted
pub fn is_unrestricted(q: Quantity) -> bool {
    q == Quantity::Many
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Quantity;
    use crate::ast::Span;

    #[test]
    fn test_unify_quantity_zero_zero() {
        let result = unify_quantity(Quantity::Zero, Quantity::Zero, Span::default());
        assert!(matches!(result, Ok(Quantity::Zero)));
    }

    #[test]
    fn test_unify_quantity_one_one() {
        let result = unify_quantity(Quantity::One, Quantity::One, Span::default());
        assert!(matches!(result, Ok(Quantity::One)));
    }

    #[test]
    fn test_unify_quantity_one_many() {
        // Per join table: One ⊔ Many = Many (least upper bound)
        let result = unify_quantity(Quantity::One, Quantity::Many, Span::default());
        assert!(matches!(result, Ok(Quantity::Many)));
    }

    #[test]
    fn test_unify_quantity_one_bounded() {
        let result = unify_quantity(Quantity::One, Quantity::Bounded(3), Span::default());
        assert!(matches!(result, Ok(Quantity::Bounded(3))));
    }

    #[test]
    fn test_unify_quantity_bounded_bounded() {
        let result = unify_quantity(Quantity::Bounded(2), Quantity::Bounded(5), Span::default());
        assert!(matches!(result, Ok(Quantity::Bounded(5))));
    }

    #[test]
    fn test_unify_quantity_bounded_many() {
        let result = unify_quantity(Quantity::Bounded(2), Quantity::Many, Span::default());
        assert!(matches!(result, Ok(Quantity::Many)));
    }

    #[test]
    fn test_unify_quantity_zero_one_error() {
        let result = unify_quantity(Quantity::Zero, Quantity::One, Span::default());
        assert!(result.is_err());
        match result {
            Err(crate::typecheck::error::TypeError::QuantityMismatch {
                expected, found, ..
            }) => {
                assert_eq!(expected, Quantity::Zero);
                assert_eq!(found, Quantity::One);
            }
            _ => panic!("Expected QuantityMismatch error"),
        }
    }

    #[test]
    fn test_qty_subtype_one_many() {
        assert!(qty_subtype(Quantity::One, Quantity::Many));
    }

    #[test]
    fn test_qty_subtype_many_one() {
        assert!(!qty_subtype(Quantity::Many, Quantity::One));
    }

    #[test]
    fn test_qty_subtype_bounded() {
        assert!(qty_subtype(Quantity::Bounded(2), Quantity::Bounded(5)));
        assert!(!qty_subtype(Quantity::Bounded(5), Quantity::Bounded(2)));
    }

    #[test]
    fn test_qty_join_one_many() {
        assert_eq!(qty_join(Quantity::One, Quantity::Many), Quantity::Many);
    }

    #[test]
    fn test_qty_meet_one_many() {
        assert_eq!(qty_meet(Quantity::One, Quantity::Many), Quantity::One);
    }

    #[test]
    fn test_qty_consume_one() {
        assert_eq!(qty_consume(Quantity::One), Quantity::Zero);
    }

    #[test]
    fn test_qty_consume_bounded() {
        assert_eq!(qty_consume(Quantity::Bounded(3)), Quantity::Bounded(2));
    }

    #[test]
    fn test_qty_join_zero() {
        // Zero ⊔ X = X (join with Zero returns the other)
        assert_eq!(qty_join(Quantity::Zero, Quantity::One), Quantity::One);
        assert_eq!(
            qty_join(Quantity::Zero, Quantity::Bounded(5)),
            Quantity::Bounded(5)
        );
        assert_eq!(qty_join(Quantity::Zero, Quantity::Many), Quantity::Many);
    }

    #[test]
    fn test_qty_meet_zero() {
        // Zero ⊓ X = Zero (meet with Zero returns Zero)
        assert_eq!(qty_meet(Quantity::Zero, Quantity::One), Quantity::Zero);
        assert_eq!(
            qty_meet(Quantity::Zero, Quantity::Bounded(5)),
            Quantity::Zero
        );
        assert_eq!(qty_meet(Quantity::Zero, Quantity::Many), Quantity::Zero);
    }

    #[test]
    fn test_qty_join_one_bounded() {
        // One ⊔ Bounded(n) = Bounded(max(1, n))
        assert_eq!(
            qty_join(Quantity::One, Quantity::Bounded(3)),
            Quantity::Bounded(3)
        );
        assert_eq!(
            qty_join(Quantity::One, Quantity::Bounded(1)),
            Quantity::Bounded(1)
        );
    }

    #[test]
    fn test_qty_meet_one_bounded() {
        // One ⊓ Bounded(n) = One (if n >= 1)
        assert_eq!(qty_meet(Quantity::One, Quantity::Bounded(3)), Quantity::One);
        assert_eq!(qty_meet(Quantity::One, Quantity::Bounded(1)), Quantity::One);
    }
}
