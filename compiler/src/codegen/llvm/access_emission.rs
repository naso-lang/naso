//! Access Relation Emission for LLVM
//!
//! Lowers PIR AccessRelations to LLVM GEP, load, and store instructions.
//! Handles alias.scope metadata for [1]-quantity linearity verification.

use crate::ast::Quantity;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::{
    access_relation::{AccessRelation, AccessType},
    pir_types::PirExpr,
};
use inkwell::types::BasicTypeEnum;
use inkwell::values::{
    AnyValue, BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, InstructionOpcode,
    IntValue, PointerValue,
};

/// Access emitter for memory operations
pub struct AccessEmitter<'ctx> {
    context: &'ctx inkwell::context::Context,
}

/// A resolved base address for an access: the pointer plus the element type its
/// GEP must be built against.
///
/// LLVM 17 uses opaque pointers, so the pointee type cannot be recovered from
/// the `PointerValue`; a GEP built with the wrong element type silently indexes
/// the wrong memory, so the two are resolved together and never guessed.
struct BasePointer<'ctx> {
    ptr: PointerValue<'ctx>,
    elem_type: BasicTypeEnum<'ctx>,
}

impl<'ctx> AccessEmitter<'ctx> {
    pub fn new(context: &'ctx inkwell::context::Context) -> CodegenResult<Self> {
        Ok(Self { context })
    }

    /// Emit an access relation (load/store/GEP)
    pub fn emit_access(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        access: &AccessRelation,
        _stmt_body: &PirExpr,
        quantities: &crate::ir::pir_types::QuantityMap,
    ) -> CodegenResult<()> {
        // Get the array/base pointer (and the element type the GEP needs)
        let base = self.get_base_pointer(value_builder, access)?;

        // Compute GEP indices from access map
        let indices = self.compute_gep_indices(value_builder, access)?;

        // Build GEP
        let gep = self.build_gep(value_builder, base, access, &indices)?;

        // Add alias.scope metadata for [1]-quantity variables
        self.add_alias_metadata(gep, access, quantities)?;

        // Emit load or store based on access type
        match access.access_type {
            AccessType::Read => {
                self.emit_load(value_builder, gep, access)?;
            }
            AccessType::Write => {
                self.emit_store(value_builder, gep, access)?;
            }
            AccessType::ReadWrite => {
                self.emit_load(value_builder, gep, access)?;
                self.emit_store(value_builder, gep, access)?;
            }
            AccessType::Reduction => {
                self.emit_reduction(value_builder, gep, access)?;
            }
        }

        Ok(())
    }

    /// The function currently being built, as seen by `value_builder`.
    fn current_function(
        &self,
        value_builder: &LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        value_builder
            .builder()
            .get_insert_block()
            .and_then(|block| block.get_parent())
            .ok_or_else(|| {
                CodegenError::InstructionError(
                    "no function is currently being built (builder has no insertion block)"
                        .to_string(),
                )
            })
    }

    /// Get base pointer for the array, together with the element type the GEP
    /// must be built against.
    ///
    /// The base is an `alloca` in the current function named `array_name`.
    /// `alloca` is the only place that still records an allocated type under
    /// LLVM 17's opaque pointers, which is why it is the only acceptable base.
    ///
    /// If no such allocation exists the access cannot be lowered, and this
    /// returns an error rather than a null pointer: a GEP against null is a
    /// silently wrong address, which is worse than refusing to emit.
    fn get_base_pointer(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        access: &AccessRelation,
    ) -> CodegenResult<BasePointer<'ctx>> {
        let array_name = access.array_name.as_deref().ok_or_else(|| {
            CodegenError::InstructionError(format!(
                "access relation for statement {:?} has no array_name, so its base pointer is unresolvable",
                access.stmt_id
            ))
        })?;

        let function = self.current_function(value_builder)?;

        // 1. A function parameter of the same name. Under LLVM 17's opaque
        //    pointers the pointee type is gone from the type system, so the
        //    element type a GEP needs cannot be recovered from a parameter --
        //    this is reported rather than guessed.
        for param in function.get_params() {
            if param.get_name().to_string_lossy() == array_name && param.is_pointer_value() {
                return Err(CodegenError::InstructionError(format!(
                    "base '{array_name}' is a pointer parameter of function '{}'; its pointee \
                     type is not recoverable under opaque pointers, so no correct GEP can be built",
                    function.get_name().to_string_lossy()
                )));
            }
        }

        // 2. A named alloca. `alloca` records the allocated type, which is
        //    exactly the element type the GEP must use.
        let mut block = function.get_first_basic_block();
        while let Some(current) = block {
            for inst in current.get_instructions() {
                if inst.get_opcode() != InstructionOpcode::Alloca {
                    continue;
                }
                if inst.get_name().map(|n| n.to_string_lossy()) != Some(array_name.into()) {
                    continue;
                }
                // Operand 0 of an `alloca` is the pointer it defines.
                let ptr = inst.get_operand(0).and_then(|op| op.value());
                let elem_type = inst.get_allocated_type().map_err(|e| {
                    CodegenError::InstructionError(format!("alloca '{array_name}': {e:?}"))
                })?;
                return Ok(BasePointer {
                    ptr: ptr
                        .ok_or_else(|| {
                            CodegenError::InstructionError(format!(
                                "alloca '{array_name}' has no pointer operand"
                            ))
                        })?
                        .into_pointer_value(),
                    elem_type,
                });
            }
            block = current.get_next_basic_block();
        }

        Err(CodegenError::InstructionError(format!(
            "no base pointer named '{array_name}' in function '{}': \
             it is neither a parameter nor an alloca",
            function.get_name().to_string_lossy()
        )))
    }

    /// Compute GEP indices from access map
    ///
    /// One index is produced per output row of every piece of the access map:
    /// `idx[i] = constant[i] + sum_d matrix[i][d] * x[d]`.
    fn compute_gep_indices(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        access: &AccessRelation,
    ) -> CodegenResult<Vec<BasicValueEnum<'ctx>>> {
        let function = self.current_function(value_builder)?;
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);

        let mut indices = Vec::new();

        for piece in &access.access_map.pieces {
            let matrix = &piece.matrix;
            if matrix.data.len() < matrix.rows * matrix.cols {
                return Err(CodegenError::InstructionError(format!(
                    "access map matrix claims {}x{} but holds {} coefficients",
                    matrix.rows,
                    matrix.cols,
                    matrix.data.len()
                )));
            }

            for row in 0..matrix.rows {
                let mut acc: Option<IntValue<'ctx>> = None;

                // Translation term of this output row.
                let constant = matrix.constant.get(row).copied().unwrap_or(0);
                if constant != 0 {
                    acc = Some(value_builder.build_int_constant(
                        int_type,
                        constant as u64,
                        "gep_const",
                    ));
                }

                for dim in 0..matrix.cols {
                    let coeff = matrix.get(row, dim);
                    if coeff == 0 {
                        continue;
                    }
                    let dim_val = self.get_dimension_value(function, dim)?.into_int_value();
                    // `build_int_constant` takes the raw bit pattern, so a
                    // negative coefficient arrives as its two's-complement i64.
                    let coeff_val =
                        value_builder.build_int_constant(int_type, coeff as u64, "coeff");
                    let term = value_builder.build_int_mul(dim_val, coeff_val, "term")?;
                    acc = Some(match acc {
                        Some(current) => value_builder.build_int_add(current, term, "sum")?,
                        None => term,
                    });
                }

                let index = acc
                    .unwrap_or_else(|| value_builder.build_int_constant(int_type, 0, "gep_zero"));
                indices.push(index.into());
            }
        }

        Ok(indices)
    }

    /// Get value for a dimension (induction variable or parameter)
    ///
    /// Looks for a live value named after the dimension, first among the
    /// function's parameters and then among the instructions already emitted
    /// in the function (an induction-variable phi or its loaded value). When no
    /// such value exists this errors: emitting a constant `0` here would build a
    /// GEP that points at the array origin and silently reads the wrong
    /// element for every iteration but the first.
    fn get_dimension_value(
        &self,
        function: FunctionValue<'ctx>,
        dim: usize,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let dim_name = format!("dim_{dim}");
        let iv_name = format!("iv_{dim}");

        // Parameters.
        for param in function.get_params() {
            let name = param.get_name().to_string_lossy().into_owned();
            if name == dim_name || name == iv_name {
                if let Ok(int_val) = IntValue::try_from(param) {
                    return Ok(int_val.into());
                }
            }
        }

        // Already-emitted instructions (phi nodes, loads, ...).
        let mut block = function.get_first_basic_block();
        while let Some(current) = block {
            for inst in current.get_instructions() {
                let name = match inst.get_name() {
                    Some(name) => name.to_string_lossy().into_owned(),
                    None => continue,
                };
                let matches = name == dim_name
                    || name == iv_name
                    // The loop emitter names its induction variable without a
                    // dimension suffix; that is only unambiguous for dim 0.
                    || (dim == 0 && (name == "iv_phi" || name == "iv_val"));
                if !matches {
                    continue;
                }
                if let Ok(int_val) = IntValue::try_from(inst.as_any_value_enum()) {
                    return Ok(int_val.into());
                }
            }
            block = current.get_next_basic_block();
        }

        Err(CodegenError::InstructionError(format!(
            "dimension {dim} of the access map has no live value: expected a parameter \
             or instruction named '{dim_name}' or '{iv_name}' in function '{}'",
            function.get_name().to_string_lossy()
        )))
    }

    /// Build GEP instruction
    fn build_gep(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        base: BasePointer<'ctx>,
        access: &AccessRelation,
        indices: &[BasicValueEnum<'ctx>],
    ) -> CodegenResult<PointerValue<'ctx>> {
        value_builder.build_gep(
            base.elem_type,
            base.ptr,
            indices,
            &format!("gep_{}", access.array_name.as_deref().unwrap_or("mem")),
        )
    }

    /// Attach `!alias.scope` and `!noalias` metadata to the GEP for [1]-quantity
    /// variables.
    ///
    /// These are instruction-level metadata kinds in LLVM, not module-level named
    /// metadata, so they are attached to the instruction the alias analysis
    /// actually reads. `inkwell::builder::Builder` has no module accessor and
    /// inkwell 0.10 exposes no way to recover the owning module from a
    /// `FunctionValue`, so module-level attachment is not reachable from here.
    fn add_alias_metadata(
        &self,
        gep: PointerValue<'ctx>,
        access: &AccessRelation,
        quantities: &crate::ir::pir_types::QuantityMap,
    ) -> CodegenResult<()> {
        let array_name = match access.array_name.as_deref() {
            Some(name) => name,
            None => return Ok(()),
        };
        if !matches!(quantities.get(array_name), Some(Quantity::One)) {
            return Ok(());
        }

        let scope_name = format!("linear_{array_name}");
        let instruction = gep.as_instruction_value().ok_or_else(|| {
            CodegenError::InstructionError(format!(
                "GEP for '{array_name}' is not an instruction, so alias metadata cannot be attached"
            ))
        })?;

        // LLVM expects `!alias.scope !{!0}` where `!0 = !{!"name"}`.
        let scope = self.context.metadata_node(&[BasicMetadataValueEnum::from(
            self.context.metadata_string(&scope_name),
        )]);
        let scope_list = self
            .context
            .metadata_node(&[BasicMetadataValueEnum::from(scope)]);

        // `!noalias` must reference a distinct scope node.
        let noalias_scope = self.context.metadata_node(&[BasicMetadataValueEnum::from(
            self.context
                .metadata_string(&format!("{scope_name}_noalias")),
        )]);
        let noalias_list = self
            .context
            .metadata_node(&[BasicMetadataValueEnum::from(noalias_scope)]);

        instruction
            .set_metadata(scope_list, self.context.get_kind_id("alias.scope"))
            .map_err(|e| CodegenError::InstructionError(format!("alias.scope metadata: {e:?}")))?;
        instruction
            .set_metadata(noalias_list, self.context.get_kind_id("noalias"))
            .map_err(|e| CodegenError::InstructionError(format!("noalias metadata: {e:?}")))?;

        Ok(())
    }

    /// Emit load instruction
    fn emit_load(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        gep: PointerValue<'ctx>,
        access: &AccessRelation,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let name = access.array_name.as_deref().unwrap_or("load");
        value_builder.build_load(gep, name)
    }

    /// Emit store instruction
    fn emit_store(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        gep: PointerValue<'ctx>,
        _access: &AccessRelation,
    ) -> CodegenResult<()> {
        // Need a value to store - for now store zero
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let zero = value_builder.build_int_constant(int_type, 0, "store_zero");
        value_builder.build_store(gep, zero.into())?;
        Ok(())
    }

    /// Emit reduction operation
    fn emit_reduction(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        gep: PointerValue<'ctx>,
        access: &AccessRelation,
    ) -> CodegenResult<()> {
        // Load current value, add new value, store back
        let loaded = self.emit_load(value_builder, gep, access)?;
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let one = value_builder.build_int_constant(int_type, 1, "red_one");
        let result = value_builder.build_int_add(loaded.into_int_value(), one, "red_add")?;
        value_builder.build_store(gep, result.into())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};

    #[test]
    fn test_access_emitter_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let emitter = AccessEmitter::new(context.llvm_context());
        assert!(emitter.is_ok());
    }
}
