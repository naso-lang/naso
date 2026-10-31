//! Loop Emission for LLVM
//!
//! Emits LLVM loop structures from polyhedral schedule bands.
//! Handles sequential loops, parallel bands, and induction variables.

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

/// Loop emitter for sequential and parallel bands
pub struct LoopEmitter<'ctx> {
    context: &'ctx inkwell::context::Context,
}

impl<'ctx> LoopEmitter<'ctx> {
    pub fn new(context: &'ctx inkwell::context::Context) -> CodegenResult<Self> {
        Ok(Self { context })
    }

    /// Emit a sequential band (non-parallel loops)
    pub fn emit_sequential_band<F>(
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
        // A band with N bounds is an N-DEEP nest, so the emission must RECURSE: each
        // loop's body contains the next loop, not the next loop's body.
        //
        // This used to be a flat `for bound in bounds`, emitting SIBLING loops and
        // running the child in each one. A 2-dimensional band therefore executed its
        // body once per dimension instead of once per point: a `0..3` by `0..2` nest
        // accumulated 10 instead of 6. The generated IR looked like two ordinary loops,
        // so nothing about it signalled the error.
        self.emit_nest(value_builder, bounds, &mut lower_child)
    }

    /// Emit the nest from `depth` onwards: one loop per bound, each nested in the
    /// previous, with the child lowered in the innermost body.
    fn emit_nest<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        bounds: &[LoopBounds<'ctx>],
        lower_child: &mut F,
    ) -> CodegenResult<()>
    where
        F: FnMut(&mut dyn ScheduleLoweringLike<'ctx>) -> CodegenResult<()>,
    {
        let Some((bound, rest)) = bounds.split_first() else {
            // Innermost level: no more dimensions, so the band's child goes here.
            return lower_child(&mut MockLowering { value_builder });
        };

        // `emit_single_loop` calls this closure with the builder positioned in THIS
        // loop's body, so recursing from inside it nests the next dimension rather than
        // following it.
        self.emit_single_loop(value_builder, bound, false, |vb| {
            self.emit_nest(vb, rest, lower_child)
        })
    }

    /// Emit a parallel band (with llvm.loop.parallel_accesses metadata)
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
        // Emit loops with parallel metadata
        for bound in bounds {
            self.emit_single_loop_with_metadata(value_builder, bound, true, |vb| {
                lower_child(&mut MockLowering { value_builder: vb })
            })?;
        }
        Ok(())
    }

    /// Emit a single loop with induction variable
    fn emit_single_loop<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        bounds: &LoopBounds<'ctx>,
        is_parallel: bool,
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

        // Create loop blocks
        let preheader = self.context.append_basic_block(func, "loop_preheader");
        let header = self.context.append_basic_block(func, "loop_header");
        let body = self.context.append_basic_block(func, "loop_body");
        let latch = self.context.append_basic_block(func, "loop_latch");
        let exit = self.context.append_basic_block(func, "loop_exit");

        // Branch to preheader
        value_builder.build_unconditional_branch(preheader)?;

        // Preheader: initialize induction variable
        value_builder.builder().position_at_end(preheader);
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        // The lower bound is used as the induction variable's initial value directly.
        //
        // It used to be required to be a CONSTANT (`get_zero_extended_constant`), which
        // meant a symbolic lower bound (`forall i in n..m`) had nowhere to go even
        // though the value was already a real `i64` computed from the parameter. The
        // value is an `IntValue` of the same `i64` type the phi and the compare use, so
        // passing it through is not a conversion.
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
        // The comparison is INCLUSIVE (`<=`). `AffineDomain::iterator_bounds`
        // recovers the largest value the domain admits, and the front end encodes
        // the half-open source range `lo..hi` as `i <= hi - 1`
        // (`lowering::loop_extraction::domain_from_nest`). Comparing with `<`
        // against that inclusive bound dropped the final iteration: `forall i in
        // 0..4` emitted `icmp slt i64 %iv_val, 3`, ran three times, and summed
        // 0+1+2 instead of 0+1+2+3 -- a loop that looked correct and computed a
        // wrong answer with no diagnostic.
        let upper_val = bounds.upper.into_int_value();
        let cond =
            value_builder.build_int_compare(IntPredicate::SLE, iv_val, upper_val, "loop_cond")?;

        value_builder.build_conditional_branch(cond, body, exit)?;

        // Body
        value_builder.builder().position_at_end(body);
        // Bind the induction variable's SOURCE SPELLING to the same alloca the phi
        // drives, so a body that reads `i` sees the current iteration. The phi alone
        // is not reachable by name: `PirExpr::Var` resolves through the value
        // builder's scope map, and before this binding `i` had no entry there, so
        // `total = total + i` read an unbound name and computed zero every
        // iteration -- a loop that iterated correctly and produced a wrong answer.
        //
        // The binding is removed after the body so it does not leak past the loop.
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
        // `bounds.step` is signed; a negative step would wrap as a huge
        // unsigned constant, so reject it rather than emit a wrong IV.
        if bounds.step <= 0 {
            return Err(CodegenError::InstructionError(format!(
                "loop step must be positive, got {}",
                bounds.step
            )));
        }
        let step_val = value_builder.build_int_constant(int_type, bounds.step as u64, "iv_step");
        let next_iv = value_builder.build_int_add(iv_val, step_val, "iv_next")?;
        value_builder.build_store(iv_alloca, next_iv.into())?;

        // Add incoming to phi
        phi.add_incoming(&[(&next_iv as &dyn BasicValue<'ctx>, latch)]);
        value_builder.build_unconditional_branch(header)?;

        // `!llvm.loop` metadata belongs on the latch terminator (the back edge).
        self.add_loop_metadata(value_builder, latch, is_parallel)?;

        // Exit
        value_builder.builder().position_at_end(exit);

        Ok(())
    }

    /// Build a self-referential `!llvm.loop` metadata node.
    ///
    /// LLVM requires the node to reference itself as its first two operands:
    ///
    /// ```text
    /// !0 = distinct !{!0, !1, !1}
    /// !1 = !{"llvm.loop.unroll.disable"}
    /// ```
    ///
    /// inkwell's safe API cannot express that cycle (a node cannot be an operand
    /// of itself at construction), so a temporary placeholder is patched to point
    /// at the finished node via `LLVMMetadataReplaceAllUsesWith`.
    fn loop_metadata(
        context: &'ctx Context,
        loop_property: MetadataValue<'ctx>,
    ) -> MetadataValue<'ctx> {
        let temp = unsafe { LLVMTemporaryMDNode(context.raw(), std::ptr::null_mut(), 0) };
        let temp_as_value = unsafe { LLVMMetadataAsValue(context.raw(), temp) };

        let node = context.metadata_node(&[
            unsafe { MetadataValue::new(temp_as_value) }.into(),
            unsafe { MetadataValue::new(temp_as_value) }.into(),
            loop_property.into(),
        ]);

        // `LLVMMetadataReplaceAllUsesWith` takes ownership of the temporary node
        // and frees it, so it must NOT be disposed separately.
        let node_ref = unsafe { LLVMValueAsMetadata(node.as_value_ref()) } as LLVMMetadataRef;
        unsafe {
            LLVMMetadataReplaceAllUsesWith(temp, node_ref);
        }

        node
    }

    /// Attach `!llvm.loop` to the loop's back edge.
    ///
    /// Parallel loops get `llvm.loop.parallel_accesses`; sequential loops get
    /// `llvm.loop.unroll.disable`, which is the conservative hint that keeps LLVM
    /// from unrolling a polyhedral loop whose trip count is not known here.
    fn add_loop_metadata(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        latch: BasicBlock<'ctx>,
        is_parallel: bool,
    ) -> CodegenResult<()> {
        let context = value_builder.type_lowering().context();

        let property_name = if is_parallel {
            "llvm.loop.parallel_accesses"
        } else {
            "llvm.loop.unroll.disable"
        };
        let property_md = context.metadata_string(property_name);
        let property_node = context.metadata_node(&[property_md.into()]);
        let loop_node = Self::loop_metadata(context, property_node);

        let kind_id = context.get_kind_id("llvm.loop");
        let latch_terminator = latch.get_last_instruction().ok_or_else(|| {
            CodegenError::InstructionError(
                "loop latch has no terminator to attach metadata to".to_string(),
            )
        })?;
        latch_terminator
            .set_metadata(loop_node, kind_id)
            .map_err(|e| CodegenError::InstructionError(format!("{e:?}")))?;

        Ok(())
    }

    /// Emit a single loop with parallel metadata
    fn emit_single_loop_with_metadata<F>(
        &self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        bounds: &LoopBounds<'ctx>,
        is_parallel: bool,
        body_builder: F,
    ) -> CodegenResult<()>
    where
        F: FnMut(&mut LlvmValueBuilder<'ctx>) -> CodegenResult<()>,
    {
        // Same block structure as a sequential loop; the difference is the
        // `!llvm.loop` metadata attached to the back edge.
        self.emit_single_loop(value_builder, bounds, is_parallel, body_builder)
    }
}

/// Loop bounds structure
#[derive(Debug, Clone)]
pub struct LoopBounds<'ctx> {
    pub iterator_dim: usize,
    pub lower: BasicValueEnum<'ctx>,
    /// The source spelling of this loop's induction variable, when known.
    ///
    /// Bound into the value builder's scope for the duration of the loop body, so
    /// `forall i in 0..n { ... i ... }` can read `i`. `None` means the domain does
    /// not name the iterator, and the body then has no binding for it -- reported
    /// rather than guessed.
    pub iterator_name: Option<String>,
    pub upper: BasicValueEnum<'ctx>,
    pub step: i64,
}

/// Trait for schedule lowering to allow mocking in loop emission
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
    fn test_loop_emitter_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let emitter = LoopEmitter::new(context.llvm_context());
        assert!(emitter.is_ok());
    }
}
