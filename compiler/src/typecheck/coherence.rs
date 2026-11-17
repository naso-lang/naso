//! Coherence checking for trait implementations
//!
//! Implements orphan rules and overlap detection to ensure
//! trait implementations are well-formed and non-ambiguous.

use crate::ast::{Ident, GenericParam, GenericKind, TraitImpl, Type, TypeKind, TypeArg};
use crate::typecheck::error::TypeError;
use crate::typecheck::type_env::TypeEnv;
use std::collections::HashSet;

/// Check coherence of all trait implementations in the program
pub fn check_coherence(env: &TypeEnv) -> Result<(), TypeError> {
    // Check orphan rules for each trait impl
    for impls in env.trait_impls.values() {
        for impl_def in impls {
            check_orphan_rules(env, impl_def)?;
        }
    }
    
    // Check for overlapping implementations
    check_overlap(env)?;
    
    Ok(())
}

/// Check orphan rules for a trait implementation.
/// 
/// Orphan rules (similar to Rust):
/// At least one of the following must be true:
/// 1. The trait is defined in the current crate
/// 2. The implementing type (`for_ty`) is a local type (defined in current crate)
/// 3. The implementation is for a fundamental type (like references, Box, etc.)
fn check_orphan_rules(env: &TypeEnv, impl_def: &TraitImpl) -> Result<(), TypeError> {
    let trait_name = impl_def.trait_path.last()
        .ok_or_else(|| TypeError::InferenceError {
            message: "Empty trait path".to_string(),
            span: impl_def.span,
        })?;
    
    // Check if trait is local (defined in current crate)
    let trait_is_local = env.traits.contains_key(trait_name);
    
    // Check if the implementing type is local
    let for_ty_is_local = is_local_type(env, &impl_def.for_ty);
    
    // Check if it's a fundamental type wrapper
    let is_fundamental = is_fundamental_wrapper(&impl_def.for_ty);
    
    if !trait_is_local && !for_ty_is_local && !is_fundamental {
        return Err(TypeError::OrphanRuleViolation {
            trait_name: trait_name.name.clone(),
            for_type: format!("{}", impl_def.for_ty),
            span: impl_def.span,
        });
    }
    
    Ok(())
}

/// Check if a type is defined in the current crate (local type)
fn is_local_type(env: &TypeEnv, ty: &Type) -> bool {
    match &ty.kind {
        TypeKind::Named(name, _) => env.types.contains_key(name),
        TypeKind::Pointer(inner) | TypeKind::Projection(inner) => is_local_type(env, inner),
        TypeKind::Array(elem, _) => is_local_type(env, elem),
        TypeKind::Tuple(elems) => elems.iter().any(|e| is_local_type(env, e)),
        _ => false,
    }
}

/// Check if a type is a fundamental wrapper (like &T, &mut T, Box<T>)
/// Fundamental types can be implemented for external traits
fn is_fundamental_wrapper(ty: &Type) -> bool {
    match &ty.kind {
        TypeKind::Pointer(_) | TypeKind::Projection(_) => true,
        _ => false,
    }
}

/// Check for overlapping trait implementations.
/// Two implementations overlap if there exists a type that could match both.
fn check_overlap(env: &TypeEnv) -> Result<(), TypeError> {
    for impls in env.trait_impls.values() {
        // Compare each pair of implementations for the same trait
        for i in 0..impls.len() {
            for j in (i + 1)..impls.len() {
                if impls_overlap(&impls[i], &impls[j]) {
                    return Err(TypeError::OverlappingImpls {
                        trait_name: impls[i].trait_path.last()
                            .map(|n| n.name.clone())
                            .unwrap_or_else(|| "unknown".to_string()),
                        impl1_span: impls[i].span,
                        impl2_span: impls[j].span,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Check if two trait implementations overlap.
/// Returns true if there exists a type that could satisfy both implementations.
fn impls_overlap(impl1: &TraitImpl, impl2: &TraitImpl) -> bool {
    // Quick check: if they're the exact same for_ty, they overlap
    if types_match_simple(&impl1.for_ty, &impl2.for_ty) {
        return true;
    }
    
    // Check if either impl has a type variable that could match the other's concrete type
    // This handles cases like `impl[T] Trait for T` vs `impl Trait for TypeA`
    if could_unify(&impl1.for_ty, &impl2.for_ty, &impl1.generics, &impl2.generics) {
        return true;
    }
    
    // Check if both are for generic type parameters that could unify
    // For now, we do a conservative check - if both have generic parameters,
    // they might overlap
    let has_generics1 = has_generic_params(&impl1.for_ty);
    let has_generics2 = has_generic_params(&impl2.for_ty);
    
    if has_generics1 && has_generics2 {
        // Could potentially overlap - need more sophisticated check
        // For now, we allow it but warn in the future
        return false;
    }
    
    // If one is a concrete type and the other is generic, they might overlap
    // if the concrete type could match the generic
    if has_generics1 != has_generics2 {
        let (generic_ty, concrete_ty) = if has_generics1 {
            (&impl1.for_ty, &impl2.for_ty)
        } else {
            (&impl2.for_ty, &impl1.for_ty)
        };
        
        if concrete_could_match_generic(concrete_ty, generic_ty) {
            return true;
        }
    }
    
    false
}

/// Check if two types could unify (i.e., there exists a substitution that makes them equal)
/// This is a simplified version - a full implementation would use the type checker's unification logic
fn could_unify(ty1: &Type, ty2: &Type, generics1: &[GenericParam], generics2: &[GenericParam]) -> bool {
    // If either is a type variable or metavariable, they could unify with anything
    matches!(&ty1.kind, TypeKind::Var(_) | TypeKind::Meta(_)) 
        || matches!(&ty2.kind, TypeKind::Var(_) | TypeKind::Meta(_))
        || concrete_could_match_generic(ty1, ty2)
        || concrete_could_match_generic(ty2, ty1)
        // Check if one is a generic param ref that could match the other
        || is_generic_param_ref(ty1, generics1) && !has_generic_params(ty2)
        || is_generic_param_ref(ty2, generics2) && !has_generic_params(ty1)
}

/// Check if a type is a generic parameter reference (named type that matches a generic param name)
fn is_generic_param_ref(ty: &Type, generics: &[GenericParam]) -> bool {
    match &ty.kind {
        TypeKind::Named(name, args) if args.is_empty() => {
            generics.iter().any(|g| g.name.name == name.name)
        }
        _ => false,
    }
}

/// Check if a type has generic parameters
fn has_generic_params(ty: &Type) -> bool {
    match &ty.kind {
        TypeKind::Named(_, args) => args.iter().any(|arg| match arg {
            TypeArg::Type(ty) => has_generic_params(ty),
            TypeArg::Nat(_) => true, // Nat args could be generic
            TypeArg::Quantity(_) => false,
        }),
        TypeKind::Pointer(inner) | TypeKind::Projection(inner) => has_generic_params(inner),
        TypeKind::Array(elem, _) => has_generic_params(elem),
        TypeKind::Tuple(elems) => elems.iter().any(has_generic_params),
        TypeKind::Function(params, ret) => {
            params.iter().any(has_generic_params) || has_generic_params(ret)
        }
        TypeKind::Var(_) | TypeKind::Meta(_) => true,
        _ => false,
    }
}

/// Check if a concrete type could match a generic type pattern
fn concrete_could_match_generic(concrete: &Type, generic: &Type) -> bool {
    match (&concrete.kind, &generic.kind) {
        (TypeKind::Named(c_name, c_args), TypeKind::Named(g_name, g_args)) => {
            if c_name.name != g_name.name {
                return false;
            }
            if c_args.len() != g_args.len() {
                return false;
            }
            // Check if all generic args are variables that could match
            c_args.iter().zip(g_args.iter()).all(|(c_arg, g_arg)| match (c_arg, g_arg) {
                (TypeArg::Type(c_ty), TypeArg::Type(g_ty)) => concrete_could_match_generic(c_ty, g_ty),
                (TypeArg::Nat(_), TypeArg::Nat(_)) => true, // Nat args could match
                _ => false,
            })
        }
        (TypeKind::Pointer(c_inner), TypeKind::Pointer(g_inner)) |
        (TypeKind::Projection(c_inner), TypeKind::Projection(g_inner)) => {
            concrete_could_match_generic(c_inner, g_inner)
        }
        (TypeKind::Array(c_elem, c_size), TypeKind::Array(g_elem, g_size)) => {
            concrete_could_match_generic(c_elem, g_elem) && 
            nat_expr_could_match(c_size.as_ref(), g_size.as_ref())
        }
        (TypeKind::Tuple(c_elems), TypeKind::Tuple(g_elems)) => {
            c_elems.len() == g_elems.len() &&
            c_elems.iter().zip(g_elems.iter()).all(|(c, g)| concrete_could_match_generic(c, g))
        }
        (_, TypeKind::Var(_)) | (_, TypeKind::Meta(_)) => true, // Generic type variable matches anything
        _ => false,
    }
}

/// Check if two NatExpr could match
fn nat_expr_could_match(c_size: Option<&crate::ast::ty::NatExpr>, g_size: Option<&crate::ast::ty::NatExpr>) -> bool {
    match (c_size, g_size) {
        (Some(_), Some(_)) => true, // Conservative: could match
        (None, None) => true,
        _ => false,
    }
}

/// Simple structural type match (used for exact overlap detection)
fn types_match_simple(ty1: &Type, ty2: &Type) -> bool {
    ty1.kind == ty2.kind
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Ident, Span, Type, TypeKind, Quantity};
    use crate::typecheck::type_env::TypeEnv;
    
    #[test]
    fn test_orphan_rule_local_trait() {
        let mut env = TypeEnv::new();
        // Add a local trait
        // ... test would require more setup
    }
}