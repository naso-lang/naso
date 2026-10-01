//! ABI and Type Lowering - Core Types
//!
//! Defines the mapping from PIR types (with quantities) to abstract LLVM types.
//! Implements quantity-aware lowering per the specification:
//! - [0] -> void (erased, never appears in runtime IR)
//! - [1] -> aggregate by-value (struct), linear: exactly one SSA def/use
//! - [*] -> pointer (opaque), unrestricted aliasing

use crate::ast::{Quantity, Span, Type, TypeKind};
use crate::codegen::error::{CodegenError, CodegenResult};
use std::collections::HashMap;

/// Quantity-aware type representation for codegen
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuantityAwareType {
    /// Erased type - [0] quantity, never appears in runtime IR
    Erased,
    /// Linear type - [1] quantity, passed by value as aggregate
    Linear(LlvmAggregateType),
    /// Unrestricted type - [*] or [N] quantity, passed as pointer
    Unrestricted(LlvmPointerType),
    /// Qubit type - special handling for quantum
    Qubit,
    /// Result type - measurement result (i1)
    Result,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlvmAggregateType {
    Int(IntWidth),
    Float(FloatWidth),
    Struct(Vec<QuantityAwareType>),
    Array(Box<QuantityAwareType>, u64),
    Tuple(Vec<QuantityAwareType>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    I1,
    I8,
    I16,
    I32,
    I64,
    I128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatWidth {
    F16,
    F32,
    F64,
    F128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmPointerType {
    pub pointee: Box<QuantityAwareType>,
    pub address_space: u32,
}

impl QuantityAwareType {
    /// Check if this type is erased (quantity [0])
    pub fn is_erased(&self) -> bool {
        matches!(self, QuantityAwareType::Erased)
    }

    /// Check if this type is linear (quantity [1])
    pub fn is_linear(&self) -> bool {
        matches!(self, QuantityAwareType::Linear(_))
    }

    /// Check if this type is unrestricted (quantity [*] or [N])
    pub fn is_unrestricted(&self) -> bool {
        matches!(self, QuantityAwareType::Unrestricted(_))
    }
}

/// Lower a PIR type (with quantity) to a quantity-aware type
pub fn lower_pir_type(
    ty: &Type,
    quantities: &HashMap<String, Quantity>,
) -> CodegenResult<QuantityAwareType> {
    let qty = ty.quantity;

    match qty {
        Quantity::Zero => Ok(QuantityAwareType::Erased),
        Quantity::One => {
            // Linear type: lower the kind to aggregate
            let agg = lower_type_kind_to_aggregate(&ty.kind, quantities)?;
            Ok(QuantityAwareType::Linear(agg))
        }
        Quantity::Bounded(n) if n > 1 => {
            // Bounded reuse: treat as unrestricted for now
            let ptr = lower_type_kind_to_pointer(&ty.kind, quantities)?;
            Ok(QuantityAwareType::Unrestricted(ptr))
        }
        Quantity::Bounded(_) => {
            // Bounded(0) or Bounded(1) - treat as Many/One
            let ptr = lower_type_kind_to_pointer(&ty.kind, quantities)?;
            Ok(QuantityAwareType::Unrestricted(ptr))
        }
        Quantity::Many => {
            // Unrestricted: lower to pointer
            let ptr = lower_type_kind_to_pointer(&ty.kind, quantities)?;
            Ok(QuantityAwareType::Unrestricted(ptr))
        }
    }
}

/// Lower all types in a PIR module
pub fn lower_pir_module_types(
    module: &crate::ir::pir_types::PirModule,
) -> CodegenResult<HashMap<String, QuantityAwareType>> {
    let mut result = HashMap::new();
    for (name, qty) in &module.quantities {
        // Create a dummy type with the quantity for lowering
        let ty = Type {
            kind: TypeKind::Int, // placeholder, would need actual type info
            quantity: *qty,
            span: Span::default(),
            nat_value: None,
            int_width: None,
        };
        let lowered = lower_pir_type(&ty, &module.quantities)?;
        result.insert(name.clone(), lowered);
    }
    Ok(result)
}

fn lower_type_kind_to_aggregate(
    kind: &TypeKind,
    quantities: &HashMap<String, Quantity>,
) -> CodegenResult<LlvmAggregateType> {
    match kind {
        TypeKind::Unit => Ok(LlvmAggregateType::Struct(vec![])),
        TypeKind::Bool => Ok(LlvmAggregateType::Int(IntWidth::I1)),
        TypeKind::Int => Ok(LlvmAggregateType::Int(IntWidth::I64)),
        TypeKind::UInt => Ok(LlvmAggregateType::Int(IntWidth::I64)),
        TypeKind::Float => Ok(LlvmAggregateType::Float(FloatWidth::F64)),
        TypeKind::Nat => Ok(LlvmAggregateType::Int(IntWidth::I64)),
        TypeKind::Qubit => Ok(LlvmAggregateType::Struct(vec![QuantityAwareType::Qubit])),
        TypeKind::QRegister(dims) => {
            // For qregister, we represent as struct of qubits
            Ok(LlvmAggregateType::Struct(vec![QuantityAwareType::Linear(
                LlvmAggregateType::Array(Box::new(QuantityAwareType::Qubit), dims.len() as u64),
            )]))
        }
        TypeKind::Tensor(dims) => {
            let elem_types: CodegenResult<Vec<_>> =
                dims.iter().map(|d| lower_pir_type(d, quantities)).collect();
            Ok(LlvmAggregateType::Struct(
                elem_types?
                    .into_iter()
                    .map(|qt| QuantityAwareType::Linear(LlvmAggregateType::Struct(vec![qt])))
                    .collect(),
            ))
        }
        TypeKind::Array(elem, size) => {
            let elem_qt = lower_pir_type(elem, quantities)?;
            let size_val = size.as_ref().and_then(|n| n.to_u64()).unwrap_or(0);
            Ok(LlvmAggregateType::Array(Box::new(elem_qt), size_val))
        }
        TypeKind::Tuple(elems) => {
            let elem_types: CodegenResult<Vec<_>> = elems
                .iter()
                .map(|e| lower_pir_type(e, quantities))
                .collect();
            Ok(LlvmAggregateType::Tuple(elem_types?))
        }
        TypeKind::Named(_name, _args) => {
            // For named types, create a struct with fields
            // This would need type definition lookup in practice
            Ok(LlvmAggregateType::Struct(vec![]))
        }
        TypeKind::Function(_params, _ret) => {
            // Function types become pointer to function
            Ok(LlvmAggregateType::Struct(vec![]))
        }
        TypeKind::Projection(inner) => {
            // Reference type becomes pointer
            let ptr = lower_type_kind_to_pointer(&inner.kind, quantities)?;
            Ok(LlvmAggregateType::Struct(vec![
                QuantityAwareType::Unrestricted(ptr),
            ]))
        }
        TypeKind::Reversible(inner) => {
            // Reversible computations are linear
            let agg = lower_type_kind_to_aggregate(&inner.kind, quantities)?;
            Ok(LlvmAggregateType::Struct(vec![QuantityAwareType::Linear(
                agg,
            )]))
        }
        TypeKind::Pi(_, domain, codomain) | TypeKind::Sigma(_, domain, codomain) => {
            // Dependent types: lower domain and codomain
            let dom_qt = lower_pir_type(domain, quantities)?;
            let cod_qt = lower_pir_type(codomain, quantities)?;
            Ok(LlvmAggregateType::Struct(vec![QuantityAwareType::Linear(
                LlvmAggregateType::Struct(vec![dom_qt, cod_qt]),
            )]))
        }
        TypeKind::Lambda(_, _) | TypeKind::App(_, _) => {
            // Type-level functions: treat as opaque
            Ok(LlvmAggregateType::Struct(vec![]))
        }
        TypeKind::Universe(_) => Ok(LlvmAggregateType::Struct(vec![])),
        TypeKind::Var(_) | TypeKind::Meta(_) => Err(CodegenError::TypeLoweringError(
            "Cannot lower unresolved type variable".to_string(),
        )),
        TypeKind::Error => Err(CodegenError::TypeLoweringError(
            "Cannot lower error type".to_string(),
        )),
        TypeKind::String => Ok(LlvmAggregateType::Struct(vec![
            QuantityAwareType::Unrestricted(LlvmPointerType {
                pointee: Box::new(QuantityAwareType::Linear(LlvmAggregateType::Int(
                    IntWidth::I8,
                ))),
                address_space: 0,
            }),
            QuantityAwareType::Linear(LlvmAggregateType::Int(IntWidth::I64)),
        ])),
        TypeKind::Char => Ok(LlvmAggregateType::Int(IntWidth::I32)),
    }
}

fn lower_type_kind_to_pointer(
    kind: &TypeKind,
    quantities: &HashMap<String, Quantity>,
) -> CodegenResult<LlvmPointerType> {
    // For unrestricted types, we create a pointer to the lowered aggregate
    // The pointee type depends on the kind
    let pointee = match kind {
        TypeKind::Qubit => QuantityAwareType::Qubit,
        TypeKind::QRegister(_) => QuantityAwareType::Linear(LlvmAggregateType::Struct(vec![])),
        _ => {
            let agg = lower_type_kind_to_aggregate(kind, quantities)?;
            QuantityAwareType::Linear(agg)
        }
    };

    Ok(LlvmPointerType {
        pointee: Box::new(pointee),
        address_space: 0,
    })
}

/// ABI calling convention for Naso functions
#[derive(Debug, Clone)]
pub struct NasoAbi {
    /// How to pass linear arguments (by value on stack/in registers)
    pub linear_by_value: bool,
    /// How to pass unrestricted arguments (by pointer)
    pub unrestricted_by_pointer: bool,
    /// Return value handling for linear types
    pub linear_return_by_pointer: bool,
}

impl Default for NasoAbi {
    fn default() -> Self {
        Self {
            linear_by_value: true,
            unrestricted_by_pointer: true,
            linear_return_by_pointer: true,
        }
    }
}

impl NasoAbi {
    /// Classify a function parameter for ABI purposes
    pub fn classify_param(&self, ty: &QuantityAwareType) -> ParamClass {
        match ty {
            QuantityAwareType::Erased => ParamClass::Ignored,
            QuantityAwareType::Linear(_) => {
                if self.linear_by_value {
                    ParamClass::ByValue
                } else {
                    ParamClass::ByPointer
                }
            }
            QuantityAwareType::Unrestricted(_) => {
                if self.unrestricted_by_pointer {
                    ParamClass::ByPointer
                } else {
                    ParamClass::ByValue
                }
            }
            QuantityAwareType::Qubit => ParamClass::ByPointer, // Qubits are always by pointer
            QuantityAwareType::Result => ParamClass::ByValue,  // i1 fits in register
        }
    }

    /// Classify return type
    pub fn classify_return(&self, ty: &QuantityAwareType) -> ReturnClass {
        match ty {
            QuantityAwareType::Erased => ReturnClass::Void,
            QuantityAwareType::Linear(_) => {
                if self.linear_return_by_pointer {
                    ReturnClass::ByPointer
                } else {
                    ReturnClass::ByValue
                }
            }
            QuantityAwareType::Unrestricted(_) => ReturnClass::ByPointer,
            QuantityAwareType::Qubit => ReturnClass::ByPointer,
            QuantityAwareType::Result => ReturnClass::ByValue,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamClass {
    Ignored,
    ByValue,
    ByPointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnClass {
    Void,
    ByValue,
    ByPointer,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Quantity, Span, Type, TypeKind};
    use std::collections::HashMap;

    #[test]
    fn test_zero_quantity_erased() {
        let ty = Type::new(TypeKind::Int, Quantity::Zero, Span::default());
        let quantities = HashMap::new();
        let result = lower_pir_type(&ty, &quantities);
        assert!(matches!(result, Ok(QuantityAwareType::Erased)));
    }

    #[test]
    fn test_one_quantity_linear() {
        let ty = Type::new(TypeKind::Int, Quantity::One, Span::default());
        let quantities = HashMap::new();
        let result = lower_pir_type(&ty, &quantities);
        assert!(matches!(result, Ok(QuantityAwareType::Linear(_))));
    }

    #[test]
    fn test_many_quantity_unrestricted() {
        let ty = Type::new(TypeKind::Int, Quantity::Many, Span::default());
        let quantities = HashMap::new();
        let result = lower_pir_type(&ty, &quantities);
        assert!(matches!(result, Ok(QuantityAwareType::Unrestricted(_))));
    }

    #[test]
    fn test_qubit_type() {
        let ty = Type::new(TypeKind::Qubit, Quantity::One, Span::default());
        let quantities = HashMap::new();
        let result = lower_pir_type(&ty, &quantities);
        assert!(matches!(result, Ok(QuantityAwareType::Linear(_))));
    }
}
