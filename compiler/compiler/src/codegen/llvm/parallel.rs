//! Parallel Emission for LLVM
//!
//! Emits parallel loop constructs with LLVM metadata for OpenMP
//! and automatic parallelization.

use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::affine_map::AffineMap;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::llvm_sys::core::{LLVMMetadataAsValue, LLVMValueAsMetadata};
use inkwell::llvm_sys::debuginfo::{LLVMMetadataReplaceAllUsesWith, LLVMTemporaryMDNode};
use inkwell::llvm_sys::prelude::LLVMMetadataRef;
use inkwell::values::{AsValueRef, BasicValue, BasicValueEnum, MetadataValue};

/// Parallel emitter for parallel bands
pub struct ParallelEmitter<'ctx> {
    context: &'ctx inkwell::context::Context,
}

impl<'ctx> ParallelEmitter<'ctx> {
    /// Build a self-referential `!llvm.loop` metadata node.
    ///
    /// LLVM's loop-metadata contract requires the node to reference itself as its
    /// first two operands:
    ///
    /// ```text
    /// !0 = distinct !{!0, !1, !1}
    /// !1 = !{!"llvm.loop.parallel_accesses"}
    /// ```
    ///
    /// `inkwell`'s safe API cannot express that cycle (a node cannot be an operand
    /// of itself at construction time), so the node is built with a temporary
    /// placeholder and the placeholder is patched to point at the finished node
    /// via `LLVMMetadataReplaceAllUsesWith`.
    fn loop_metadata(
        context: &'ctx Context,
        loop_property: MetadataValue<'ctx>,
    ) -> MetadataValue<'ctx> {
        // A temporary node stands in for the self-reference until it is known.
        let temp = unsafe { LLVMTemporaryMDNode(context.raw(), std::ptr::null_mut(), 0) };
        let temp_as_value = unsafe { LLVMMetadataAsValue(context.raw(), temp) };

        // !0 = !{!temp, !temp, !1}
        let node = context.metadata_node(&[
            unsafe { MetadataValue::new(temp_as_value) }.into(),
            unsafe { MetadataValue::new(temp_as_value) }.into(),
            loop_property.into(),
        ]);

        // Patch the temporary operand so it becomes the node itself.
        // `LLVMMetadataReplaceAllUsesWith` takes ownership of the temporary node
        // and frees it, so it must NOT be disposed separately.
        let node_ref = unsafe { LLVMValueAsMetadata(node.as_value_ref()) } as LLVMMetadataRef;
        unsafe {
            LLVMMetadataReplaceAllUsesWith(temp, node_ref);
        }

        node
    }
    pub fn new(context: &'ctx inkwell::context::Context) -> CodegenResult<Self> {
        Ok(Self { context })
    }

    /// Emit a parallel band with llvm.loop.parallel_accesses metadata
    pub fn emit_parallel_band<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        bounds: &[LoopBounds<'ctx>],
        _members: &[AffineMap],
        _child: &crate::ir::schedule_tree::ScheduleNode,
        mut lower_child: F,
    ) -> CodegenResult<()>
    where
        F: FnMut(&mut dyn ScheduleLoweringLike<'ctx>) -> CodegenResult<()>,
    {
        // Emit each loop with parallel metadata
        for bound in bounds {
            self.emit_parallel_loop(value_builder, bound, |vb| {
                lower_child(&mut MockLowering { value_builder: vb })
            })?;
        }
        Ok(())
    }

    /// Emit a single parallel loop
    fn emit_parallel_loop<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        bounds: &LoopBounds<'ctx>,
        mut body_builder: F,
    ) -> CodegenResult<()>
    where
        F: FnMut(&mut LlvmValueBuilder<'ctx>) -> CodegenResult<()>,
    {
        let func = value_builder
            .builder()
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        // Create loop blocks (same structure as sequential but with metadata)
        let preheader = self.context.append_basic_block(func, "par_loop_preheader");
        let header = self.context.append_basic_block(func, "par_loop_header");
        let body = self.context.append_basic_block(func, "par_loop_body");
        let latch = self.context.append_basic_block(func, "par_loop_latch");
        let exit = self.context.append_basic_block(func, "par_loop_exit");

        // Branch to preheader
        value_builder.build_unconditional_branch(preheader)?;

        // Preheader: initialize induction variable
        value_builder.builder().position_at_end(preheader);
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        // The lower bound is the initial value directly; it is not required to be a
        // constant, so a symbolic lower bound reaches here as a real `i64` value of
        // the same type the phi and the compare use. See the matching note in
        // `loop_emission::LoopEmitter::emit_single_loop`.
        let init_val = bounds.lower.into_int_value();
        let iv_alloca = value_builder.build_alloca(int_type.into(), "iv")?;
        value_builder.build_store(iv_alloca, init_val.into())?;
        value_builder.build_unconditional_branch(header)?;

        // Header: phi node for induction variable
        value_builder.builder().position_at_end(header);
        let phi = value_builder.build_phi(int_type.into(), "iv_phi")?;
        phi.add_incoming(&[(&init_val as &dyn BasicValue<'ctx>, preheader)]);

        // Load current induction variable value
        let iv_val = value_builder
            .build_load(iv_alloca, "iv_val")?
            .into_int_value();

        // Compare with upper bound.
        //
        // The comparison is INCLUSIVE (`<=`): `bounds.upper` is the largest value
        // the domain admits, and the front end encodes the half-open source range
        // `lo..hi` as `i <= hi - 1`. See the matching comment in
        // `loop_emission::LoopEmitter::emit_single_loop`.
        let upper_val = bounds.upper.into_int_value();
        let cond =
            value_builder.build_int_compare(IntPredicate::SLE, iv_val, upper_val, "loop_cond")?;

        value_builder.build_conditional_branch(cond, body, exit)?;

        // Body
        value_builder.builder().position_at_end(body);

        // Bind the induction variable's source spelling for the body, exactly as
        // `loop_emission` does for a sequential band: a body reading `i` resolves
        // through the value builder's scope, and an unbound `i` reads zero, which
        // would make a correctly-iterated loop compute the wrong answer.
        if let Some(name) = &bounds.iterator_name {
            value_builder.add_variable(name.clone(), iv_alloca, int_type.into());
        }
        body_builder(value_builder)?;
        if let Some(name) = &bounds.iterator_name {
            value_builder.remove_variable(name);
        }
        value_builder.build_unconditional_branch(latch)?;

        // Latch: increment induction variable
        value_builder.builder().position_at_end(latch);
        let step_val = value_builder.build_int_constant(int_type, bounds.step as u64, "iv_step");
        let next_iv = value_builder.build_int_add(iv_val, step_val, "iv_next")?;
        value_builder.build_store(iv_alloca, next_iv.into())?;

        // Add incoming to phi
        phi.add_incoming(&[(&next_iv as &dyn BasicValue<'ctx>, latch)]);
        value_builder.build_unconditional_branch(header)?;

        // The `!llvm.loop` metadata is attached to the latch terminator (the back edge),
        // which is the only place LLVM recognizes loop metadata on.
        self.add_parallel_metadata(value_builder, latch)?;

        // Exit
        value_builder.builder().position_at_end(exit);

        Ok(())
    }

    /// Add LLVM parallel loop metadata
    ///
    /// The metadata is a self-referential `!llvm.loop` node attached to the latch
    /// terminator (the loop's back edge), which is the only instruction LLVM
    /// recognizes loop metadata on:
    ///
    /// ```text
    /// br label %header, !llvm.loop !{!0, !0, !1, !1}
    /// !0 = distinct !{!0, !1, !1}
    /// !1 = !{!"llvm.loop.parallel_accesses"}
    /// ```
    fn add_parallel_metadata(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        latch: BasicBlock<'ctx>,
    ) -> CodegenResult<()> {
        // Add llvm.loop.parallel_accesses metadata to the loop
        // This tells LLVM that iterations of this loop can be executed in parallel
        let context = value_builder.type_lowering().context();

        let parallel_md = context.metadata_string("llvm.loop.parallel_accesses");

        // Also record the OpenMP-compatible scheduling directive alongside it.
        let omp_md = context.metadata_string("omp parallel for");
        let omp_node = context.metadata_node(&[omp_md.into()]);

        // Clang's operand layout for !llvm.loop.parallel_accesses:
        //   !{!"llvm.loop.parallel_accesses", <access list>, <loop control>}
        let parallel_node =
            context.metadata_node(&[parallel_md.into(), omp_node.into(), omp_node.into()]);
        let loop_node = Self::loop_metadata(context, parallel_node);

        let kind_id = context.get_kind_id("llvm.loop");
        let latch_terminator = latch.get_last_instruction().ok_or_else(|| {
            CodegenError::InstructionError(
                "parallel loop latch has no terminator to attach metadata to".to_string(),
            )
        })?;
        latch_terminator
            .set_metadata(loop_node, kind_id)
            .map_err(|e| CodegenError::InstructionError(format!("{e:?}")))?;

        Ok(())
    }

    /// Emit OpenMP-style parallel region
    pub fn emit_parallel_region<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        _num_threads: Option<BasicValueEnum<'ctx>>,
        mut region_builder: F,
    ) -> CodegenResult<()>
    where
        F: FnMut(&mut LlvmValueBuilder<'ctx>) -> CodegenResult<()>,
    {
        // Create parallel region entry/exit blocks
        let func = value_builder
            .builder()
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let entry = self.context.append_basic_block(func, "omp_parallel_entry");
        let exit = self.context.append_basic_block(func, "omp_parallel_exit");

        // Branch to parallel region
        value_builder.build_unconditional_branch(entry)?;

        value_builder.builder().position_at_end(entry);
        region_builder(value_builder)?;
        value_builder.build_unconditional_branch(exit)?;

        value_builder.builder().position_at_end(exit);
        Ok(())
    }

    /// Emit SIMD/vectorized loop
    pub fn emit_simd_loop<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        bounds: &LoopBounds<'ctx>,
        simd_width: usize,
        mut body_builder: F,
    ) -> CodegenResult<()>
    where
        F: FnMut(&mut LlvmValueBuilder<'ctx>) -> CodegenResult<()>,
    {
        // Similar to parallel loop but with SIMD metadata
        let func = value_builder
            .builder()
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let preheader = self.context.append_basic_block(func, "simd_loop_preheader");
        let header = self.context.append_basic_block(func, "simd_loop_header");
        let body = self.context.append_basic_block(func, "simd_loop_body");
        let latch = self.context.append_basic_block(func, "simd_loop_latch");
        let exit = self.context.append_basic_block(func, "simd_loop_exit");

        value_builder.build_unconditional_branch(preheader)?;

        value_builder.builder().position_at_end(preheader);
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let lower_const = bounds
            .lower
            .into_int_value()
            .get_zero_extended_constant()
            .ok_or_else(|| {
                CodegenError::InstructionError(
                    "SIMD loop lower bound is not an integer constant".to_string(),
                )
            })?;
        let init_val = value_builder.build_int_constant(int_type, lower_const, "iv_init");
        let iv_alloca = value_builder.build_alloca(int_type.into(), "iv")?;
        value_builder.build_store(iv_alloca, init_val.into())?;
        value_builder.build_unconditional_branch(header)?;

        value_builder.builder().position_at_end(header);
        let phi = value_builder.build_phi(int_type.into(), "iv_phi")?;
        phi.add_incoming(&[(&init_val as &dyn BasicValue<'ctx>, preheader)]);

        let iv_val = value_builder
            .build_load(iv_alloca, "iv_val")?
            .into_int_value();
        let upper_val = bounds.upper.into_int_value();
        let cond =
            value_builder.build_int_compare(IntPredicate::SLE, iv_val, upper_val, "loop_cond")?;
        value_builder.build_conditional_branch(cond, body, exit)?;

        value_builder.builder().position_at_end(body);

        body_builder(value_builder)?;
        value_builder.build_unconditional_branch(latch)?;

        value_builder.builder().position_at_end(latch);
        let step_val = value_builder.build_int_constant(
            int_type,
            (bounds.step * simd_width as i64) as u64,
            "iv_step",
        );
        let next_iv = value_builder.build_int_add(iv_val, step_val, "iv_next")?;
        value_builder.build_store(iv_alloca, next_iv.into())?;
        phi.add_incoming(&[(&next_iv as &dyn BasicValue<'ctx>, latch)]);
        value_builder.build_unconditional_branch(header)?;

        // Attach the vectorization metadata to the latch back edge, where LLVM
        // expects `!llvm.loop` to live.
        self.add_simd_metadata(value_builder, latch, simd_width)?;

        value_builder.builder().position_at_end(exit);
        Ok(())
    }

    /// Add SIMD metadata
    ///
    /// Attached to the latch back edge as a self-referential `!llvm.loop` node
    /// carrying `llvm.loop.vectorize.width`.
    fn add_simd_metadata(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        latch: BasicBlock<'ctx>,
        width: usize,
    ) -> CodegenResult<()> {
        let context = value_builder.type_lowering().context();

        let i32_type = context.i32_type();
        let width_md = context.metadata_string(&format!("llvm.loop.vectorize.width {width}"));
        let width_node = context.metadata_node(&[
            width_md.into(),
            i32_type.const_int(width as u64, false).into(),
        ]);

        let loop_node = Self::loop_metadata(context, width_node);

        let kind_id = context.get_kind_id("llvm.loop");
        let latch_terminator = latch.get_last_instruction().ok_or_else(|| {
            CodegenError::InstructionError(
                "SIMD loop latch has no terminator to attach metadata to".to_string(),
            )
        })?;
        latch_terminator
            .set_metadata(loop_node, kind_id)
            .map_err(|e| CodegenError::InstructionError(format!("{e:?}")))?;

        Ok(())
    }
}

/// Loop bounds structure (shared with loop_emission)
#[derive(Debug, Clone)]
pub struct LoopBounds<'ctx> {
    pub iterator_dim: usize,
    pub lower: BasicValueEnum<'ctx>,
    /// The source spelling of this loop's induction variable, when known.
    /// See `loop_emission::LoopBounds::iterator_name`.
    pub iterator_name: Option<String>,
    pub upper: BasicValueEnum<'ctx>,
    pub step: i64,
}

/// Trait for schedule lowering to allow mocking
pub trait ScheduleLoweringLike<'ctx> {
    fn value_builder(&mut self) -> &mut LlvmValueBuilder<'ctx>;
}

struct MockLowering<'ctx, 'a> {
    value_builder: &'a mut LlvmValueBuilder<'ctx>,
}

impl<'ctx, 'a> ScheduleLoweringLike<'ctx> for MockLowering<'ctx, 'a> {
    fn value_builder(&mut self) -> &mut LlvmValueBuilder<'ctx> {
        self.value_builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};

    #[test]
    fn test_parallel_emitter_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let emitter = ParallelEmitter::new(context.llvm_context());
        assert!(emitter.is_ok());
    }
}
