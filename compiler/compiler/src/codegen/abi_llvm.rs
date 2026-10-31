//! ABI and Type Lowering - LLVM Implementation
//!
//! LLVM-specific lowering of quantity-aware types to inkwell types.

// `LlvmAggregateType`, `LlvmPointerType` and `QuantityAwareType` come in via the
// `pub use ...::*` glob at the bottom of this file; importing them privately here
// as well would shadow that public re-export.
#[cfg(feature = "llvm")]
use crate::codegen::error::{CodegenError, CodegenResult};
#[cfg(feature = "llvm")]
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
#[cfg(feature = "llvm")]
use inkwell::types::{BasicType, BasicTypeEnum};

#[cfg(feature = "llvm")]
impl QuantityAwareType {
    /// Get the LLVM type for this quantity-aware type
    pub fn to_llvm_type<'ctx>(
        &self,
        lowering: &LlvmTypeLowering<'ctx>,
    ) -> CodegenResult<BasicTypeEnum<'ctx>> {
        match self {
            QuantityAwareType::Erased => Err(CodegenError::TypeLoweringError(
                "Cannot lower erased type to LLVM".to_string(),
            )),
            QuantityAwareType::Linear(agg) => agg.to_llvm_type(lowering),
            QuantityAwareType::Unrestricted(ptr) => ptr.to_llvm_type(lowering),
            QuantityAwareType::Qubit => Ok(lowering.qubit_type().into()),
            QuantityAwareType::Result => Ok(lowering.result_type().into()),
        }
    }
}

#[cfg(feature = "llvm")]
impl LlvmAggregateType {
    fn to_llvm_type<'ctx>(
        &self,
        lowering: &LlvmTypeLowering<'ctx>,
    ) -> CodegenResult<BasicTypeEnum<'ctx>> {
        match self {
            LlvmAggregateType::Int(width) => Ok(lowering.int_type(*width).into()),
            LlvmAggregateType::Float(width) => Ok(lowering.float_type(*width).into()),
            LlvmAggregateType::Struct(fields) => {
                let field_types: CodegenResult<Vec<_>> =
                    fields.iter().map(|f| f.to_llvm_type(lowering)).collect();
                let struct_type = lowering.context().struct_type(&field_types?, false);
                Ok(struct_type.into())
            }
            LlvmAggregateType::Array(elem, size) => {
                let elem_type = elem.to_llvm_type(lowering)?;
                let array_type = elem_type.array_type(*size as u32);
                Ok(array_type.into())
            }
            LlvmAggregateType::Tuple(elems) => {
                let elem_types: CodegenResult<Vec<_>> =
                    elems.iter().map(|e| e.to_llvm_type(lowering)).collect();
                let struct_type = lowering.context().struct_type(&elem_types?, false);
                Ok(struct_type.into())
            }
        }
    }
}

#[cfg(feature = "llvm")]
impl LlvmPointerType {
    /// Lower this pointer type.
    ///
    /// LLVM 15+ uses opaque pointers, so the pointee is still lowered (to keep
    /// errors for unlowerable pointees) but does not appear in the pointer
    /// type itself. The address space is honoured and validated, not truncated.
    fn to_llvm_type<'ctx>(
        &self,
        lowering: &LlvmTypeLowering<'ctx>,
    ) -> CodegenResult<BasicTypeEnum<'ctx>> {
        let _pointee_type = self.pointee.to_llvm_type(lowering)?;
        let addr_space = inkwell::AddressSpace::try_from(self.address_space).map_err(|_| {
            CodegenError::TypeLoweringError(format!(
                "Address space {} does not fit in LLVM's 24-bit address space",
                self.address_space
            ))
        })?;
        Ok(lowering.context().ptr_type(addr_space).into())
    }
}

// Re-export core types
pub use crate::codegen::abi::abi_core::*;
