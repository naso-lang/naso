//! SIMD Lowering for Quantization Kernels
//!
//! Lowers polyhedral forall loops for quantization operations
//! into packed SIMD/vector compute instructions while preserving memory
//! uncomputation and linear type invariants.

use crate::ast::Quantity;
use crate::ir::{AffineDomain, PirExpr, PirModule, PirStatement, StmtId};
use crate::lowering::LoweringError;

/// SIMD target architectures
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdTarget {
    /// AVX2 / AVX-512 (x86_64)
    Avx2,
    /// NEON (ARM)
    Neon,
    /// Scalar fallback with vectorization hints
    Scalar,
}

/// SIMD instruction kind for quantization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdQuantOp {
    /// Quantize f32 -> i8 with symmetric scaling
    QuantizeSymmetric,
    /// Dequantize i8 -> f32 with symmetric scaling
    DequantizeSymmetric,
}

/// SIMD lowering context
pub struct SimdLoweringContext {
    target: SimdTarget,
    vector_width: usize,
}

impl SimdLoweringContext {
    /// Create a new SIMD lowering context
    pub fn new(target: SimdTarget) -> Self {
        let vector_width = match target {
            SimdTarget::Avx2 => 8,
            SimdTarget::Neon => 4,
            SimdTarget::Scalar => 1,
        };
        Self {
            target,
            vector_width,
        }
    }

    /// Lower a quantization forall loop to SIMD instructions
    pub fn lower_quantization_loop(
        &self,
        loop_var: &str,
        _bound: &PirExpr,
        body: &PirExpr,
        quant_op: SimdQuantOp,
    ) -> Result<Vec<PirStatement>, LoweringError> {
        let (input_var, output_var, scale_var) = self.extract_quantization_vars(body, quant_op)?;
        let vec_domain = self.create_vectorized_domain(loop_var)?;
        self.generate_simd_quantization(&vec_domain, &input_var, &output_var, &scale_var, quant_op)
    }

    /// Extract input, output, and scale variables from quantization expression
    fn extract_quantization_vars(
        &self,
        expr: &PirExpr,
        quant_op: SimdQuantOp,
    ) -> Result<(String, String, String), LoweringError> {
        match expr {
            PirExpr::Call { name, args } => {
                if (quant_op == SimdQuantOp::QuantizeSymmetric && name.contains("quantize"))
                    || (quant_op == SimdQuantOp::DequantizeSymmetric && name.contains("dequantize"))
                {
                    if args.len() >= 3 {
                        if let (PirExpr::Var(input), PirExpr::Var(output), PirExpr::Var(scale)) =
                            (&args[0], &args[1], &args[2])
                        {
                            return Ok((input.clone(), output.clone(), scale.clone()));
                        }
                    }
                }
                Err(LoweringError::Unsupported(
                    "Could not extract quantization variables from call".to_string(),
                ))
            }
            PirExpr::Index { base, indices: _ } => {
                if let PirExpr::Var(output_name) = &**base {
                    return Ok((
                        "input".to_string(),
                        output_name.clone(),
                        "scale".to_string(),
                    ));
                }
                Err(LoweringError::Unsupported(
                    "Could not extract variables from index expression".to_string(),
                ))
            }
            _ => Err(LoweringError::Unsupported(
                "Unsupported expression type for quantization".to_string(),
            )),
        }
    }

    /// Create a vectorized domain for SIMD iteration
    fn create_vectorized_domain(&self, loop_var: &str) -> Result<AffineDomain, LoweringError> {
        let domain = AffineDomain::universe(1, 0).with_name(format!("vec_{}", loop_var));
        Ok(domain)
    }

    /// Generate SIMD quantization instructions
    fn generate_simd_quantization(
        &self,
        domain: &AffineDomain,
        input_var: &str,
        output_var: &str,
        scale_var: &str,
        quant_op: SimdQuantOp,
    ) -> Result<Vec<PirStatement>, LoweringError> {
        match self.target {
            SimdTarget::Avx2 => {
                self.generate_avx2_quantization(domain, input_var, output_var, scale_var, quant_op)
            }
            SimdTarget::Neon => {
                self.generate_neon_quantization(domain, input_var, output_var, scale_var, quant_op)
            }
            SimdTarget::Scalar => self
                .generate_scalar_quantization(domain, input_var, output_var, scale_var, quant_op),
        }
    }

    /// Generate AVX2 quantization instructions
    fn generate_avx2_quantization(
        &self,
        domain: &AffineDomain,
        input_var: &str,
        output_var: &str,
        scale_var: &str,
        quant_op: SimdQuantOp,
    ) -> Result<Vec<PirStatement>, LoweringError> {
        let name = match quant_op {
            SimdQuantOp::QuantizeSymmetric => "avx2_quantize_symmetric",
            SimdQuantOp::DequantizeSymmetric => "avx2_dequantize_symmetric",
        };
        Ok(vec![
            self.make_stmt(domain, name, input_var, output_var, scale_var),
        ])
    }

    /// Generate NEON quantization instructions
    fn generate_neon_quantization(
        &self,
        domain: &AffineDomain,
        input_var: &str,
        output_var: &str,
        scale_var: &str,
        quant_op: SimdQuantOp,
    ) -> Result<Vec<PirStatement>, LoweringError> {
        let name = match quant_op {
            SimdQuantOp::QuantizeSymmetric => "neon_quantize_symmetric",
            SimdQuantOp::DequantizeSymmetric => "neon_dequantize_symmetric",
        };
        Ok(vec![
            self.make_stmt(domain, name, input_var, output_var, scale_var),
        ])
    }

    /// Generate scalar fallback with vectorization hints
    fn generate_scalar_quantization(
        &self,
        domain: &AffineDomain,
        input_var: &str,
        output_var: &str,
        scale_var: &str,
        quant_op: SimdQuantOp,
    ) -> Result<Vec<PirStatement>, LoweringError> {
        let name = match quant_op {
            SimdQuantOp::QuantizeSymmetric => "scalar_quantize_symmetric",
            SimdQuantOp::DequantizeSymmetric => "scalar_dequantize_symmetric",
        };
        Ok(vec![
            self.make_stmt(domain, name, input_var, output_var, scale_var),
        ])
    }

    fn make_stmt(
        &self,
        domain: &AffineDomain,
        name: &str,
        input_var: &str,
        output_var: &str,
        scale_var: &str,
    ) -> PirStatement {
        PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::Call {
                name: name.to_string(),
                args: vec![
                    PirExpr::Var(input_var.to_string()),
                    PirExpr::Var(output_var.to_string()),
                    PirExpr::Var(scale_var.to_string()),
                ],
            },
            quantity: Quantity::Many,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        }
    }
}

/// Entry point: lower quantization loops in a PIR module to SIMD
pub fn lower_quantization_to_simd(
    module: &PirModule,
    target: SimdTarget,
) -> Result<PirModule, LoweringError> {
    let ctx = SimdLoweringContext::new(target);
    let mut new_stmts = Vec::new();

    for stmt in &module.statements {
        if ctx.is_quantization_loop(&stmt.body) {
            let quant_op = if let PirExpr::Call { name, .. } = &stmt.body {
                if name.contains("dequantize") {
                    SimdQuantOp::DequantizeSymmetric
                } else {
                    SimdQuantOp::QuantizeSymmetric
                }
            } else {
                SimdQuantOp::QuantizeSymmetric
            };
            let stmts =
                ctx.lower_quantization_loop("", &PirExpr::IntLit(0), &stmt.body, quant_op)?;
            new_stmts.extend(stmts);
        } else {
            new_stmts.push(stmt.clone());
        }
    }

    Ok(PirModule::new(
        new_stmts,
        module.schedule.clone(),
        module.accesses.clone(),
        module.quantities.clone(),
        module.parameters.clone(),
    ))
}

/// Verify that SIMD lowering preserves linear type invariants
pub fn verify_simd_linearity(module: &PirModule) -> Result<(), LoweringError> {
    for (var, qty) in &module.quantities {
        if qty == &Quantity::One {
            let count = count_var_occurrences_in_module(module, var);
            if count != 1 {
                return Err(LoweringError::QuantityMismatch(format!(
                    "Linear variable '{}' used {} times after SIMD lowering",
                    var, count
                )));
            }
        }
    }
    Ok(())
}

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
        PirExpr::Binary { left, right, .. } => count_in_expr(left, var) + count_in_expr(right, var),
        PirExpr::Let { value, body, .. } => count_in_expr(value, var) + count_in_expr(body, var),
        PirExpr::Unary { expr, .. } => count_in_expr(expr, var),
        PirExpr::Call { args, .. } => args.iter().map(|a| count_in_expr(a, var)).sum(),
        PirExpr::Index { base, indices } => {
            count_in_expr(base, var) + indices.iter().map(|i| count_in_expr(i, var)).sum::<usize>()
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

impl SimdLoweringContext {
    fn is_quantization_loop(&self, expr: &PirExpr) -> bool {
        matches!(expr, PirExpr::Call { name, .. } if name.contains("quantize") || name.contains("dequantize"))
    }

    /// Get the vector width for current target
    pub fn vector_width(&self) -> usize {
        self.vector_width
    }

    /// Get target architecture
    pub fn target(&self) -> SimdTarget {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Mutability, Quantity};
    use crate::ir::{
        access_relation::{AccessRelation, AccessRelations, AccessType},
        affine_domain::AffineDomain,
        affine_map::{AffineMap, Matrix},
        schedule_tree::{ScheduleNode, ScheduleTree},
    };
    use std::collections::HashMap;

    fn make_test_module() -> PirModule {
        let domain = AffineDomain::universe(1, 0);
        let mut m = Matrix::new(1, 1);
        m.set(0, 0, 1);
        let schedule = AffineMap::total(domain.clone(), m.clone());

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
            AccessRelation::new(StmtId(0), domain, access_map, AccessType::Write)
                .with_array_name("output"),
        );

        let mut quantities = HashMap::new();
        quantities.insert("input".to_string(), Quantity::One);
        quantities.insert("output".to_string(), Quantity::One);
        quantities.insert("scale".to_string(), Quantity::Zero);
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
    fn test_avx2_lowering() {
        let module = make_test_module();
        let result = lower_quantization_to_simd(&module, SimdTarget::Avx2);
        assert!(result.is_ok());
        let lowered = result.unwrap();
        assert!(!lowered.statements.is_empty());
    }

    #[test]
    fn test_neon_lowering() {
        let module = make_test_module();
        let result = lower_quantization_to_simd(&module, SimdTarget::Neon);
        assert!(result.is_ok());
    }

    #[test]
    fn test_scalar_lowering() {
        let module = make_test_module();
        let result = lower_quantization_to_simd(&module, SimdTarget::Scalar);
        assert!(result.is_ok());
    }

    #[test]
    fn test_linearity_preserved() {
        let module = make_test_module();
        let lowered = lower_quantization_to_simd(&module, SimdTarget::Avx2).unwrap();
        let result = verify_simd_linearity(&lowered);
        assert!(result.is_ok());
    }

    #[test]
    fn test_vector_width() {
        let ctx_avx2 = SimdLoweringContext::new(SimdTarget::Avx2);
        assert_eq!(ctx_avx2.vector_width(), 8);

        let ctx_neon = SimdLoweringContext::new(SimdTarget::Neon);
        assert_eq!(ctx_neon.vector_width(), 4);

        let ctx_scalar = SimdLoweringContext::new(SimdTarget::Scalar);
        assert_eq!(ctx_scalar.vector_width(), 1);
    }
}
