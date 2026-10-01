// LLVM Module Builder
//
// High-level wrapper around inkwell::module::Module for building LLVM IR.

use crate::ast::{Quantity, Span, Type, TypeKind};
use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::expr_lowering::PirExprLowerer;
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::pir_types::{PirExpr, PirModule, PirStatement};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::module::Module as LlvmModule;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
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
    // True while building a PIR STATEMENT, as opposed to an expression. A
    // statement-position `let` is a binding that outlives its own expression, so
    // `build_expr` must not remove it from scope when it finishes. See the `Let`
    // arm in `PirExprLowerer` for why this cannot be inferred from the
    // expression's shape.
    in_statement_position: bool,
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
            in_statement_position: false,

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
        // Each statement gets its own basic block so the emitted IR still shows where
        // one statement ends and the next begins, and a malformed statement is easy to
        // locate. Blocks fall through in order, which is the schedule's execution order.
        if !pir_module.statements.is_empty() {
            let fn_type = self.type_lowering.fn_type(None, &[], false); // `None` is void
            let function = self.module.add_function(ENTRY_NAME, fn_type, None);
            self.set_current_function(function);

            // One basic block per statement, so the IR still shows where one statement
            // ends and the next begins. Blocks fall through in order, which is the
            // schedule's execution order.
            let mut blocks: Vec<BasicBlock<'ctx>> = pir_module
                .statements
                .iter()
                .map(|stmt| {
                    self.context
                        .llvm_context()
                        .append_basic_block(function, &format!("stmt_{}", stmt.id.0))
                })
                .collect();

            let last = blocks.len() - 1;
            for (index, stmt) in pir_module.statements.iter().enumerate() {
                self.set_current_block(blocks[index]);
                // A `let` in statement position binds for the rest of the function.
                self.in_statement_position = true;
                self.build_expr(&stmt.body, &pir_module.quantities)?;

                if index == last {
                    // The last statement ends the function.
                    self.llvm_builder()
                        .build_return(None)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                } else {
                    self.llvm_builder()
                        .build_unconditional_branch(blocks[index + 1])
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                }
            }

            self.current_function = None;
            self.current_block = None;
            self.value_builder().clear_variables();
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

    // Build a PIR statement as a function
    fn build_statement(
        &mut self,
        stmt: &PirStatement,
        quantities: &HashMap<String, Quantity>,
    ) -> CodegenResult<()> {
        let func_name = format!("stmt_{}", stmt.id.0);

        // Determine function signature based on quantities used
        let params: Vec<BasicTypeEnum<'ctx>> = Vec::new(); // Simplified for now
        // Statement functions return void; `None` is the void return type.
        let ret_type: Option<BasicTypeEnum<'ctx>> = None;
        let fn_type = self.type_lowering.fn_type(ret_type, &params, false);

        let function = self.module.add_function(&func_name, fn_type, None);
        self.set_current_function(function);

        // Create entry block
        let entry = self
            .context
            .llvm_context()
            .append_basic_block(function, "entry");
        self.set_current_block(entry);

        // Build the statement body
        self.build_expr(&stmt.body, quantities)?;

        // Return void
        self.llvm_builder()
            .build_return(None)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        self.current_function = None;
        self.current_block = None;
        self.value_builder().clear_variables();

        Ok(())
    }

    // Build a PIR expression
    //
    // The logic itself lives in `PirExprLowerer`, which `ScheduleLowering` also uses
    // for schedule band bodies. Keeping it here as a private copy is what left a band
    // body with nothing to emit: the loop structure was correct and the body was
    // empty, so the loop iterated the right number of times and computed nothing.
    //
    // This method is the module-builder-shaped entry point: it supplies the current
    // function and the statement-position flag from this type's own state.
    fn build_expr(
        &mut self,
        expr: &PirExpr,
        quantities: &HashMap<String, Quantity>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let function = self.current_function().ok_or_else(|| {
            CodegenError::FunctionBuildError(
                "no function is being built: a PIR expression cannot be lowered outside \
                 one"
                .to_string(),
            )
        })?;
        let in_statement_position = self.in_statement_position;
        // Borrow the two fields separately: `value_builder` needs `&mut` (it owns the
        // builder and the variable map) while `module` needs only `&`, and taking both
        // out of `self` in one struct literal would borrow `self` mutably twice.
        let module = &self.module;
        let mut lowerer = PirExprLowerer {
            value_builder: self
                .value_builder
                .as_mut()
                .expect("value_builder not initialized"),
            module,
            current_function: function,
            in_statement_position,
        };
        lowerer.build_expr(expr, quantities)
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
        use crate::ast::Mutability;
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
