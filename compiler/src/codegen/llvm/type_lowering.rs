// @generated
#[cfg(feature = "llvm")]

/// LLVM Type Lowering
///
/// Maps Naso PIR types (with quantities) to LLVM types using inkwell.
use crate::codegen::abi::{
    FloatWidth, IntWidth, LlvmAggregateType, LlvmPointerType, QuantityAwareType,
};
use crate::codegen::error::{CodegenError, CodegenResult};
use inkwell::AddressSpace;
use inkwell::context::Context;
use inkwell::types::{
    BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType, IntType, PointerType, StructType,
    VoidType,
};
use std::collections::HashMap;

/// Wrapper for LLVM type lowering context
pub struct LlvmTypeLowering<'ctx> {
    context: &'ctx Context,
    /// Cache of named struct types
    struct_cache: HashMap<String, StructType<'ctx>>,
    /// Qubit type (opaque pointer)
    qubit_type: PointerType<'ctx>,
    /// Result type (i1)
    result_type: IntType<'ctx>,
    /// Address space for quantum types
    quantum_address_space: u32,
}

impl<'ctx> LlvmTypeLowering<'ctx> {
    /// Create a new type lowering context
    pub fn new(context: &'ctx Context) -> Self {
        let qubit_type = context.ptr_type(AddressSpace::from(0)); // Generic address space for qubits
        let result_type = context.bool_type(); // i1

        Self {
            context,
            struct_cache: HashMap::new(),
            qubit_type,
            result_type,
            quantum_address_space: 0,
        }
    }

    /// Get the LLVM context
    pub fn context(&self) -> &'ctx Context {
        self.context
    }

    /// Get the qubit type (opaque pointer)
    pub fn qubit_type(&self) -> PointerType<'ctx> {
        self.qubit_type
    }

    /// Get the result type (i1)
    pub fn result_type(&self) -> IntType<'ctx> {
        self.result_type
    }

    /// Get integer type by width
    pub fn int_type(&self, width: IntWidth) -> IntType<'ctx> {
        match width {
            IntWidth::I1 => self.context.bool_type(),
            IntWidth::I8 => self.context.i8_type(),
            IntWidth::I16 => self.context.i16_type(),
            IntWidth::I32 => self.context.i32_type(),
            IntWidth::I64 => self.context.i64_type(),
            IntWidth::I128 => self.context.i128_type(),
        }
    }

    /// Get float type by width
    pub fn float_type(&self, width: FloatWidth) -> FloatType<'ctx> {
        match width {
            FloatWidth::F16 => self.context.f16_type(),
            FloatWidth::F32 => self.context.f32_type(),
            FloatWidth::F64 => self.context.f64_type(),
            FloatWidth::F128 => self.context.f128_type(),
        }
    }

    /// Get or create a named struct type
    pub fn get_or_create_struct(
        &mut self,
        name: &str,
        fields: &[BasicTypeEnum<'ctx>],
        is_packed: bool,
    ) -> StructType<'ctx> {
        if let Some(existing) = self.struct_cache.get(name) {
            return *existing;
        }
        let struct_type = self.context.struct_type(fields, is_packed);
        self.struct_cache.insert(name.to_string(), struct_type);
        struct_type
    }

    /// Get a struct type by name (must have been created)
    pub fn get_struct(&self, name: &str) -> Option<StructType<'ctx>> {
        self.struct_cache.get(name).copied()
    }

    /// Lower a quantity-aware type to LLVM
    pub fn lower_quantity_aware(
        &mut self,
        qty_ty: &QuantityAwareType,
    ) -> CodegenResult<BasicTypeEnum<'ctx>> {
        match qty_ty {
            QuantityAwareType::Erased => Err(CodegenError::TypeLoweringError(
                "Cannot lower erased type".to_string(),
            )),
            QuantityAwareType::Linear(agg) => self.lower_aggregate(agg),
            QuantityAwareType::Unrestricted(ptr) => self.lower_pointer(ptr),
            QuantityAwareType::Qubit => Ok(self.qubit_type.into()),
            QuantityAwareType::Result => Ok(self.result_type.into()),
        }
    }

    /// Lower an aggregate type
    fn lower_aggregate(&mut self, agg: &LlvmAggregateType) -> CodegenResult<BasicTypeEnum<'ctx>> {
        match agg {
            LlvmAggregateType::Int(width) => Ok(self.int_type(*width).into()),
            LlvmAggregateType::Float(width) => Ok(self.float_type(*width).into()),
            LlvmAggregateType::Struct(fields) => {
                let field_types: CodegenResult<Vec<_>> = fields
                    .iter()
                    .map(|f| self.lower_quantity_aware(f))
                    .collect();
                let struct_type = self.context.struct_type(&field_types?, false);
                Ok(struct_type.into())
            }
            LlvmAggregateType::Array(elem, size) => {
                let elem_type = self.lower_quantity_aware(elem)?;
                let array_type = elem_type.array_type(*size as u32);
                Ok(array_type.into())
            }
            LlvmAggregateType::Tuple(elems) => {
                let elem_types: CodegenResult<Vec<_>> =
                    elems.iter().map(|e| self.lower_quantity_aware(e)).collect();
                let struct_type = self.context.struct_type(&elem_types?, false);
                Ok(struct_type.into())
            }
        }
    }

    /// Lower a pointer type
    ///
    /// LLVM 15+ uses opaque pointers, so the pointee only matters for its own
    /// lowering validity, not for the resulting pointer type's identity. We
    /// still lower the pointee first so invalid pointees are reported, then
    /// build the pointer through the Context (the non-deprecated API).
    fn lower_pointer(&mut self, ptr: &LlvmPointerType) -> CodegenResult<BasicTypeEnum<'ctx>> {
        // Validate the pointee lowers cleanly even though opaque pointers
        // erase it in the resulting type.
        let _pointee_type = self.lower_quantity_aware(&ptr.pointee)?;
        let addr_space = self.address_space(ptr.address_space)?;
        Ok(self.context.ptr_type(addr_space).into())
    }

    /// Convert a raw address space id into an inkwell `AddressSpace`.
    ///
    /// Address spaces are 24-bit unsigned integers in LLVM; anything wider
    /// cannot be represented and is rejected rather than silently truncated.
    fn address_space(&self, address_space: u32) -> CodegenResult<AddressSpace> {
        AddressSpace::try_from(address_space).map_err(|_| {
            CodegenError::TypeLoweringError(format!(
                "Address space {address_space} does not fit in LLVM's 24-bit address space"
            ))
        })
    }

    /// Get void type
    pub fn void_type(&self) -> VoidType<'ctx> {
        self.context.void_type()
    }

    /// Get pointer type for the given address space.
    ///
    /// LLVM 15+ has opaque pointers, so the `pointee` argument does not
    /// affect the pointer type; it is retained for call-site readability and
    /// must still be a valid basic type. The address space is honoured.
    pub fn ptr_type(
        &self,
        pointee: BasicTypeEnum<'ctx>,
        address_space: u32,
    ) -> CodegenResult<PointerType<'ctx>> {
        // Keep the pointee meaningful: opaque pointers erase it in the IR, but
        // dropping it entirely would lose the caller's intent silently.
        let _ = pointee;
        Ok(self.context.ptr_type(self.address_space(address_space)?))
    }

    /// Get function type
    pub fn fn_type(
        &self,
        ret: Option<BasicTypeEnum<'ctx>>,
        params: &[BasicTypeEnum<'ctx>],
        is_var_args: bool,
    ) -> inkwell::types::FunctionType<'ctx> {
        let params: Vec<BasicMetadataTypeEnum<'ctx>> = params.iter().map(|p| (*p).into()).collect();
        match ret {
            Some(r) => r.fn_type(&params, is_var_args),
            None => self.void_type().fn_type(&params, is_var_args),
        }
    }
}

/// Convenience type alias for LLVM basic types
pub type LlvmType<'ctx> = BasicTypeEnum<'ctx>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::abi::{IntWidth, LlvmAggregateType, QuantityAwareType};

    #[test]
    fn test_type_lowering_creation() {
        let context = Context::create();
        let lowering = LlvmTypeLowering::new(&context);
        assert_eq!(lowering.int_type(IntWidth::I32).get_bit_width(), 32);
        assert_eq!(lowering.result_type().get_bit_width(), 1);
    }

    #[test]
    fn test_lower_linear_int() {
        let context = Context::create();
        let mut lowering = LlvmTypeLowering::new(&context);
        let qty_ty = QuantityAwareType::Linear(LlvmAggregateType::Int(IntWidth::I64));
        let result = lowering.lower_quantity_aware(&qty_ty);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BasicTypeEnum::IntType(_)));
    }

    #[test]
    fn test_lower_unrestricted_pointer() {
        let context = Context::create();
        let mut lowering = LlvmTypeLowering::new(&context);
        let qty_ty = QuantityAwareType::Unrestricted(crate::codegen::abi::LlvmPointerType {
            pointee: Box::new(QuantityAwareType::Linear(LlvmAggregateType::Int(
                IntWidth::I32,
            ))),
            address_space: 0,
        });
        let result = lowering.lower_quantity_aware(&qty_ty);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BasicTypeEnum::PointerType(_)));
    }

    #[test]
    fn test_lower_qubit() {
        let context = Context::create();
        let mut lowering = LlvmTypeLowering::new(&context);
        let qty_ty = QuantityAwareType::Qubit;
        let result = lowering.lower_quantity_aware(&qty_ty);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BasicTypeEnum::PointerType(_)));
    }

    #[test]
    fn test_lower_result() {
        let context = Context::create();
        let mut lowering = LlvmTypeLowering::new(&context);
        let qty_ty = QuantityAwareType::Result;
        let result = lowering.lower_quantity_aware(&qty_ty);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), BasicTypeEnum::IntType(t) if t.get_bit_width() == 1));
    }

    #[test]
    fn test_struct_caching() {
        let context = Context::create();
        let mut lowering = LlvmTypeLowering::new(&context);
        let fields = vec![context.i32_type().into(), context.i64_type().into()];

        let s1 = lowering.get_or_create_struct("test_struct", &fields, false);
        let s2 = lowering.get_or_create_struct("test_struct", &fields, false);

        assert_eq!(s1, s2);
    }
}
