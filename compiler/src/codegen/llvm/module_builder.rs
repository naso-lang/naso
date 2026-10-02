// LLVM Module Builder
//
// High-level wrapper around inkwell::module::Module for building LLVM IR.

use crate::ast::{Span, Type, TypeKind};
use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::schedule_lowering::{
    lower_schedule_tree_into, schedule_parameters, statements_under,
};
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::pir_types::PirModule;
use crate::ir::schedule_tree::StmtId;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::module::Module as LlvmModule;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{FunctionValue, PointerValue};
/// The single entry function generated for a whole [`PirModule`].
///
/// A `PirModule` is one program's statement list, so all of its statements go into one
/// function. Emitting a separate function per statement (the previous behaviour) made
/// bindings unreachable across statements and left `forall` bodies unlowerable.
const ENTRY_NAME: &str = "naso_entry";

use std::collections::HashMap;

// LLVM Module Builder for constructing LLVM IR from PIR
pub struct LLVMModuleBuilder<'ctx> {
    context: &'ctx CodegenContext,
    module: LlvmModule<'ctx>,
    type_lowering: LlvmTypeLowering<'ctx>,
    // The single inkwell builder used for all instruction emission.
    // It lives inside [`LlvmValueBuilder`]; this type does not keep a second
    // copy because `inkwell::builder::Builder` is neither `Copy` nor `Clone`
    // (it owns an `LLVMBuilderRef` and implements `Drop`), so two builders
    // would silently drift apart in insertion position.
    value_builder: Option<LlvmValueBuilder<'ctx>>,
    // Current function being built
    current_function: Option<FunctionValue<'ctx>>,
    // Current basic block
    current_block: Option<BasicBlock<'ctx>>,
    // Named struct types
    struct_types: HashMap<String, inkwell::types::StructType<'ctx>>,
}

impl<'ctx> LLVMModuleBuilder<'ctx> {
    // Create a new module builder
    pub fn new(context: &'ctx CodegenContext) -> CodegenResult<Self> {
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("naso_module");
        let builder = llvm_context.create_builder();
        let type_lowering = LlvmTypeLowering::new(llvm_context);

        // `LlvmValueBuilder` owns the builder and its own type lowering, so it is
        // constructed before `Self` (the builder cannot be copied out of it
        // afterwards).
        let value_builder = LlvmValueBuilder::new(builder, LlvmTypeLowering::new(llvm_context));

        Ok(Self {
            context,
            module,
            type_lowering,
            value_builder: Some(value_builder),
            current_function: None,
            current_block: None,

            struct_types: HashMap::new(),
        })
    }

    // The inkwell builder shared by this type and its value builder
    fn llvm_builder(&self) -> &LlvmBuilder<'ctx> {
        self.value_builder
            .as_ref()
            .expect("value_builder not initialized")
            .builder()
    }

    // Get the underlying LLVM module
    pub fn module(&self) -> &LlvmModule<'ctx> {
        &self.module
    }

    // Get the LLVM context
    pub fn llvm_context(&self) -> &inkwell::context::Context {
        self.context.llvm_context()
    }

    // Get the type lowering context
    pub fn type_lowering(&mut self) -> &mut LlvmTypeLowering<'ctx> {
        &mut self.type_lowering
    }

    // Get the value builder
    pub fn value_builder(&mut self) -> &mut LlvmValueBuilder<'ctx> {
        self.value_builder
            .as_mut()
            .expect("value_builder not initialized")
    }

    // Set the current function
    pub fn set_current_function(&mut self, func: FunctionValue<'ctx>) {
        self.current_function = Some(func);
    }

    // Get the current function
    pub fn current_function(&self) -> Option<FunctionValue<'ctx>> {
        self.current_function
    }

    // Set the current basic block
    pub fn set_current_block(&mut self, block: BasicBlock<'ctx>) {
        self.current_block = Some(block);
        self.llvm_builder().position_at_end(block);
    }

    // Get the current basic block
    pub fn current_block(&self) -> Option<BasicBlock<'ctx>> {
        self.current_block
    }

    // Add a variable allocation
    // `ty` is the pointee type of the allocation; LLVM 17 opaque pointers do
    // not carry it, so it is recorded for the later `build_load`.
    //
    // This writes into the value builder's map, which is the SAME map the
    // schedule-lowering path reads: a name bound here is visible inside a
    // schedule band body, and vice versa.
    pub fn add_variable(&mut self, name: String, ptr: PointerValue<'ctx>, ty: BasicTypeEnum<'ctx>) {
        self.value_builder().add_variable(name, ptr, ty);
    }

    // Get a variable allocation
    pub fn get_variable(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.value_builder
            .as_ref()
            .expect("value_builder not initialized")
            .variable(name)
            .map(|(ptr, _)| ptr)
    }

    // Build the entire PIR module
    pub fn build_module(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        // Declare external functions
        for extern_fn in &pir_module.extern_functions {
            self.declare_extern_function(extern_fn)?;
        }

        // ONE function for the whole PIR module.
        //
        // This used to emit a separate `define void @stmt_N()` per statement. That made
        // cross-statement references impossible: an alloca created for `let mut total`
        // lived in `stmt_0`, so `total = total + i` in `stmt_1` had no destination and
        // the Assign arm had to refuse it. A `PirModule` is one program's statement
        // list -- it has no function structure of its own -- so a single function is
        // both correct and what makes bindings visible across statements.
        //
        // The SCHEDULE, not the statement list, decides execution order and loop
        // structure. Walking `pir_module.statements` directly cannot express a loop at
        // all: a `forall` appears as ONE statement whose domain is `nest(i)`, so
        // walking statements ran its body exactly once -- arithmetically wrong, with
        // no diagnostic, while the band that recorded the iteration bounds sat in the
        // schedule tree unread. `lower_schedule_tree_into` emits a real loop nest
        // (preheader / header with a phi / body / latch / exit) and honours the bands.
        //
        // It also owns the function BODY: the `entry` block, the single return, and
        // the verification. Nothing is emitted here, so there is exactly one return
        // and no block is terminated twice.
        if !pir_module.statements.is_empty() {
            // The entry function takes one `i64` argument per symbolic constant the
            // SCHEDULE names -- see `schedule_parameters`. A `forall i in 0..n` is the
            // motivating case: `n` is a runtime value, and there is nowhere else for it
            // to come from. Inventing one (zero, a constant from the source text) would
            // compile a program that iterates a different number of times than it was
            // told to, so the signature carries the value instead.
            //
            // The width is i64 because loop bounds are computed in i64 throughout this
            // backend, and `PirModule::parameters` carries names only -- a narrower
            // source-level parameter type is not represented in PIR yet. Nothing is
            // converted: the argument IS the i64 the bound arithmetic uses. That is a
            // deliberate limitation, not a widening conversion.
            //
            // A module with no symbolic bounds gets the previous `void()` signature,
            // so nothing about a constant-bounded program changes.
            let param_names = schedule_parameters(&pir_module.schedule);
            let i64_type = self
                .type_lowering
                .int_type(crate::codegen::abi::IntWidth::I64);
            let param_types: Vec<inkwell::types::BasicTypeEnum<'ctx>> =
                param_names.iter().map(|_| i64_type.into()).collect();
            let fn_type = self.type_lowering.fn_type(None, &param_types, false); // `None` is void
            let function = self.module.add_function(ENTRY_NAME, fn_type, None);
            // Name the arguments in the IR so the printed module says where each
            // symbolic value came from.
            for (i, name) in param_names.iter().enumerate() {
                if let Some(arg) = function.get_nth_param(i as u32) {
                    arg.set_name(name);
                }
            }
            self.set_current_function(function);

            // The value builder is the ONE scope for the whole function. It is the
            // builder `add_variable` writes into, so a name bound before this call is
            // visible inside a band body; passing a different builder in would give
            // the loop body an empty scope and silently drop every outer binding.
            //
            // `self.module` and `self.value_builder` are borrowed separately because
            // the callee needs the module immutably and the value builder mutably,
            // and both are fields of the same `&mut self`.
            let module = &self.module;
            let value_builder = self
                .value_builder
                .as_mut()
                .expect("value_builder not initialized");
            lower_schedule_tree_into(
                self.context,
                module,
                function,
                value_builder,
                &pir_module.schedule,
                pir_module,
                &pir_module.quantities,
                &pir_module.accesses,
            )?;
            self.value_builder().clear_variables();

            self.current_function = None;
            self.current_block = None;
            self.value_builder().clear_variables();
        }

        // Every statement must be REACHABLE from the schedule tree, or it will not be
        // emitted. This is checked AFTER lowering, because the schedule tree is now
        // the only thing that emits a statement: walking the statement list as well
        // would emit it twice.
        //
        // Silently accepting a statement the schedule never mentions would compile it
        // to nothing -- the same class of silent wrong answer as the loop-once body.
        // A `[0]`-quantity statement IS in the tree and is deliberately skipped by
        // `lower_domain`, which is erasure, not a coverage gap, so it is not reported.
        let scheduled: Vec<StmtId> = statements_under(&pir_module.schedule.root);
        for stmt in &pir_module.statements {
            if !scheduled.contains(&stmt.id) {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "PIR statement {} is in `statements` but no `Domain` node in the \
                     schedule tree covers it, so the LLVM backend would emit nothing \
                     for it. Add a schedule node naming it, or drop the statement.",
                    stmt.id
                )));
            }
        }

        // Verify the module
        self.module
            .verify()
            .map_err(|e| CodegenError::VerificationError(e.to_string()))?;

        Ok(())
    }

    // Declare an external function
    fn declare_extern_function(
        &mut self,
        extern_fn: &crate::ir::pir_types::ExternFunction,
    ) -> CodegenResult<()> {
        let param_types: CodegenResult<Vec<BasicTypeEnum<'ctx>>> = extern_fn
            .params
            .iter()
            .map(|p| {
                // Extern params only carry a source-level type name plus their
                // quantity; the name is not resolvable here, so the declared
                // param type is the quantity-lowered `int` placeholder.
                let ty = Type::new(TypeKind::Int, p.quantity, Span::default());
                self.type_lowering
                    .lower_quantity_aware(&crate::codegen::abi::lower_pir_type(
                        &ty,
                        &HashMap::new(),
                    )?)
            })
            .collect();

        let param_types = param_types?;

        // `ExternFunction::return_type` is a source-level type name which cannot
        // be lowered to an LLVM type at declaration time, so the declaration
        // uses the void return signature (`None` == void for `fn_type`).
        let ret_type: Option<BasicTypeEnum<'ctx>> = None;

        let fn_type = self.type_lowering.fn_type(ret_type, &param_types, false);
        self.module.add_function(&extern_fn.name, fn_type, None);
        Ok(())
    }

    // Convert module to LLVM IR string
    pub fn module_to_string(&self) -> String {
        self.module.print_to_string().to_string()
    }

    // Write module to .ll file
    pub fn write_ll_file(&self, path: &std::path::Path) -> CodegenResult<()> {
        self.module
            .print_to_file(path)
            .map_err(|e| CodegenError::EmissionError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};

    #[test]
    fn test_module_builder_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let builder = LLVMModuleBuilder::new(&context);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_empty_module_emission() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let mut builder = LLVMModuleBuilder::new(&context).unwrap();

        // Create a minimal PIR module
        use crate::ast::{Mutability, Quantity};
        use crate::ir::access_relation::AccessRelations;
        use crate::ir::affine_domain::AffineDomain;
        use crate::ir::pir_types::{PirModule, PirStatement};
        use crate::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};
        use std::collections::HashMap;

        let domain = AffineDomain::universe(0, 0);
        let schedule = ScheduleTree::new(ScheduleNode::domain(StmtId(0), domain.clone()), vec![]);
        let accesses = AccessRelations::new();
        let quantities = HashMap::new();

        let stmt = PirStatement {
            id: StmtId(0),
            domain,
            body: crate::ir::pir_types::PirExpr::IntLit(42),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);

        let result = builder.build_module(&module);
        assert!(result.is_ok());

        let ir = builder.module_to_string();
        assert!(ir.contains("define"));
    }
}
