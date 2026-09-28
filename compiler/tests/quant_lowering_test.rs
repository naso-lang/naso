//! Quantization Lowering Tests
//!
//! Tests for SIMD lowering of INT8 symmetric quantization kernels
//! and Z3 verification integration.

use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::ir::{
    access_relation::{AccessRelation, AccessRelations, AccessType},
    affine_domain::AffineDomain,
    affine_map::{AffineMap, Matrix},
    pir_types::{BinaryOp, PirExpr, PirModule, PirStatement, QuantityMap, ValidationError},
    schedule_tree::{ScheduleNode, ScheduleTree, StmtId},
    validate::{validate_pir, validate_schedule_detailed},
};
use naso_compiler::lowering::{lower_quantization_to_simd, simd::SimdTarget};
use std::collections::HashMap;

/// Construct a test PIR module for INT8 quantization
fn construct_quant_module() -> PirModule {
    let domain = AffineDomain::new(
        1,
        1,
        vec![
            // i >= 0 => 1*i + 0*N >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
            // i <= N-1 => -i + N >= 1 (since i - N <= -1 => -i + N >= 1)
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 1], 1),
        ],
    )
    .with_name("quant_domain".to_string());

    let mut m = Matrix::new(1, 2);
    m.set(0, 0, 1);
    let m_for_schedule = m.clone();
    let schedule = AffineMap::total(domain.clone(), m_for_schedule);

    // Quantization call: output[i] = round(input[i] / scale) as i8
    let stmt = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Call {
            name: "quantize_int8_symmetric".to_string(),
            args: vec![
                PirExpr::Var("input".to_string()),
                PirExpr::Var("output".to_string()),
                PirExpr::Var("scale".to_string()),
            ],
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let schedule_tree = ScheduleTree::new(
        ScheduleNode::band(
            vec![schedule],
            vec![false],
            ScheduleNode::domain(StmtId(0), domain.clone()),
        ),
        vec!["N".to_string()],
    );

    let mut accesses = AccessRelations::new();
    let access_map = AffineMap::total(domain.clone(), m);
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Read,
        )
        .with_array_name("input"),
    );
    accesses.add(
        AccessRelation::new(StmtId(0), domain, access_map, AccessType::Write).with_array_name(
            "output",
        ),
    );

    let mut quantities = HashMap::new();
    quantities.insert("input".to_string(), Quantity::One);
    quantities.insert("output".to_string(), Quantity::One);
    // scale is [0] quantity - proof only, not in runtime
    quantities.insert("N".to_string(), Quantity::Zero);

    PirModule::new(
        vec![stmt],
        schedule_tree,
        accesses,
        quantities,
        vec!["N".to_string()],
    )
}

/// Construct a test PIR module for INT8 dequantization
fn construct_dequant_module() -> PirModule {
    let domain = AffineDomain::new(
        1,
        1,
        vec![
            // i >= 0 => 1*i + 0*N >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
            // i <= N-1 => -i + N >= 1
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 1], 1),
        ],
    )
    .with_name("dequant_domain".to_string());

    let mut m = Matrix::new(1, 2);
    m.set(0, 0, 1);
    let m_for_schedule = m.clone();
    let schedule = AffineMap::total(domain.clone(), m_for_schedule);

    // Dequantization call: output[i] = input[i] as f32 * scale
    let stmt = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Call {
            name: "dequantize_int8_symmetric".to_string(),
            args: vec![
                PirExpr::Var("input".to_string()),
                PirExpr::Var("output".to_string()),
                PirExpr::Var("scale".to_string()),
            ],
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let schedule_tree = ScheduleTree::new(
        ScheduleNode::band(
            vec![schedule],
            vec![false],
            ScheduleNode::domain(StmtId(0), domain.clone()),
        ),
        vec!["N".to_string()],
    );

    let mut accesses = AccessRelations::new();
    let access_map = AffineMap::total(domain.clone(), m);
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Read,
        )
        .with_array_name("input"),
    );
    accesses.add(
        AccessRelation::new(StmtId(0), domain, access_map, AccessType::Write).with_array_name(
            "output",
        ),
    );

    let mut quantities = HashMap::new();
    quantities.insert("input".to_string(), Quantity::One);
    quantities.insert("output".to_string(), Quantity::One);
    // scale is [0] quantity - proof only, not in runtime
    quantities.insert("N".to_string(), Quantity::Zero);

    PirModule::new(
        vec![stmt],
        schedule_tree,
        accesses,
        quantities,
        vec!["N".to_string()],
    )
}

#[test]
fn test_quant_module_validates() {
    let module = construct_quant_module();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "Quantization module validation failed: {:?}",
        result
    );
}

#[test]
fn test_dequant_module_validates() {
    let module = construct_dequant_module();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "Dequantization module validation failed: {:?}",
        result
    );
}

#[test]
fn test_avx2_quant_lowering() {
    let module = construct_quant_module();
    let result = lower_quantization_to_simd(&module, SimdTarget::Avx2);
    assert!(result.is_ok(), "AVX2 quantization lowering failed: {:?}", result);

    let lowered = result.unwrap();
    assert!(!lowered.statements.is_empty());
}

#[test]
fn test_avx2_dequant_lowering() {
    let module = construct_dequant_module();
    let result = lower_quantization_to_simd(&module, SimdTarget::Avx2);
    assert!(result.is_ok(), "AVX2 dequantization lowering failed: {:?}", result);

    let lowered = result.unwrap();
    assert!(!lowered.statements.is_empty());
}

#[test]
fn test_neon_quant_lowering() {
    let module = construct_quant_module();
    let result = lower_quantization_to_simd(&module, SimdTarget::Neon);
    assert!(result.is_ok(), "NEON quantization lowering failed: {:?}", result);
}

#[test]
fn test_neon_dequant_lowering() {
    let module = construct_dequant_module();
    let result = lower_quantization_to_simd(&module, SimdTarget::Neon);
    assert!(result.is_ok(), "NEON dequantization lowering failed: {:?}", result);
}

#[test]
fn test_scalar_quant_lowering() {
    let module = construct_quant_module();
    let result = lower_quantization_to_simd(&module, SimdTarget::Scalar);
    assert!(result.is_ok(), "Scalar quantization lowering failed: {:?}", result);
}

#[test]
fn test_linearity_preserved_after_lowering() {
    let module = construct_quant_module();
    let lowered = lower_quantization_to_simd(&module, SimdTarget::Avx2).unwrap();

    // Verify linear quantities are preserved
    for (var, qty) in &lowered.quantities {
        if qty == &Quantity::One {
            let count = count_var_occurrences_in_module(&lowered, var);
            assert_eq!(
                count, 1,
                "Linear variable '{}' used {} times after lowering",
                var, count
            );
        }
    }
}

#[test]
fn test_zero_quantity_erased_after_lowering() {
    let module = construct_quant_module();
    let lowered = lower_quantization_to_simd(&module, SimdTarget::Avx2).unwrap();

    // Verify zero-quantity variables don't appear in runtime
    for (var, qty) in &lowered.quantities {
        if qty == &Quantity::Zero {
            for stmt in &lowered.statements {
                assert!(
                    !expr_contains_var(&stmt.body, var),
                    "Zero-quantity variable '{}' appears in runtime statement {}",
                    var, stmt.id
                );
            }
        }
    }
}

#[test]
fn test_vector_width_avx2() {
    let ctx = naso_compiler::lowering::simd::SimdLoweringContext::new(SimdTarget::Avx2);
    assert_eq!(ctx.vector_width(), 8);
}

#[test]
fn test_vector_width_neon() {
    let ctx = naso_compiler::lowering::simd::SimdLoweringContext::new(SimdTarget::Neon);
    assert_eq!(ctx.vector_width(), 4);
}

#[test]
fn test_vector_width_scalar() {
    let ctx = naso_compiler::lowering::simd::SimdLoweringContext::new(SimdTarget::Scalar);
    assert_eq!(ctx.vector_width(), 1);
}

#[test]
fn test_simd_target_variants() {
    let targets = [SimdTarget::Avx2, SimdTarget::Neon, SimdTarget::Scalar];
    for target in targets {
        let module = construct_quant_module();
        let result = lower_quantization_to_simd(&module, target);
        assert!(result.is_ok(), "Lowering failed for target {:?}", target);
    }
}

// Helper functions

fn count_var_occurrences_in_module(module: &PirModule, var: &str) -> usize {
    module
        .statements
        .iter()
        .map(|s| count_in_expr(&s.body, var))
        .sum()
}

fn count_in_expr(expr: &PirExpr, var: &str) -> usize {
    match expr {
        PirExpr::Var(v) if v == var => 1,
        PirExpr::Binary { left, right, .. } => {
            count_in_expr(left, var) + count_in_expr(right, var)
        }
        PirExpr::Let { value, body, .. } => {
            count_in_expr(value, var) + count_in_expr(body, var)
        }
        PirExpr::Unary { expr, .. } => count_in_expr(expr, var),
        PirExpr::Call { args, .. } => args.iter().map(|a| count_in_expr(a, var)).sum(),
        PirExpr::Index { base, indices } => {
            count_in_expr(base, var)
                + indices.iter().map(|i| count_in_expr(i, var)).sum::<usize>()
        }
        PirExpr::Field { base, .. } => count_in_expr(base, var),
        PirExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            count_in_expr(cond, var)
                + count_in_expr(then_branch, var)
                + count_in_expr(else_branch, var)
        }
        PirExpr::Reversible { body, inverse } => {
            count_in_expr(body, var) + count_in_expr(inverse, var)
        }
        PirExpr::QuantumOp {
            op: _,
            args,
            qubits,
        } => {
            args.iter().map(|a| count_in_expr(a, var)).sum::<usize>()
                + qubits.iter().map(|q| count_in_expr(q, var)).sum::<usize>()
        }
        _ => 0,
    }
}

fn expr_contains_var(expr: &PirExpr, var: &str) -> bool {
    match expr {
        PirExpr::Var(v) => v == var,
        PirExpr::Binary { left, right, .. } => {
            expr_contains_var(left, var) || expr_contains_var(right, var)
        }
        PirExpr::Let { value, body, .. } => {
            expr_contains_var(value, var) || expr_contains_var(body, var)
        }
        PirExpr::Unary { expr, .. } => expr_contains_var(expr, var),
        PirExpr::Call { args, .. } => args.iter().any(|a| expr_contains_var(a, var)),
        PirExpr::Index { base, indices } => {
            expr_contains_var(base, var) || indices.iter().any(|i| expr_contains_var(i, var))
        }
        PirExpr::Field { base, .. } => expr_contains_var(base, var),
        PirExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_contains_var(cond, var)
                || expr_contains_var(then_branch, var)
                || expr_contains_var(else_branch, var)
        }
        PirExpr::Reversible { body, inverse } => {
            expr_contains_var(body, var) || expr_contains_var(inverse, var)
        }
        PirExpr::QuantumOp {
            op: _,
            args,
            qubits,
        } => {
            args.iter().any(|a| expr_contains_var(a, var))
                || qubits.iter().any(|q| expr_contains_var(q, var))
        }
        PirExpr::IntLit(_) | PirExpr::FloatLit(_) | PirExpr::BoolLit(_) => false,
    }
}