// LLVM Module Builder
//
// High-level wrapper around inkwell::module::Module for building LLVM IR.

use crate::ast::{Quantity, Span, Type, TypeKind};
use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::pir_types::{BinaryOp, PirExpr, PirModule, PirStatement, UnaryOp};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::module::Module as LlvmModule;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use std::collections::HashMap;

/// LLVM Module Builder for constructing LLVM IR from PIR
pub struct LLVMModuleBuilder<'ctx> {
    context: &'ctx CodegenContext,
    module: LlvmModule<'ctx>,
    type_lowering: LlvmTypeLowering<'ctx>,
    /// The single inkwell builder used for all instruction emission.
    ///
    /// It lives inside [`LlvmValueBuilder`]; this type does not keep a second
    /// copy because `inkwell::builder::Builder` is neither `Copy` nor `Clone`
    /// (it owns an `LLVMBuilderRef` and implements `Drop`), so two builders
    /// would silently drift apart in insertion position.
    value_builder: Option<LlvmValueBuilder<'ctx>>,
    /// Current function being built
    current_function: Option<FunctionValue<'ctx>>,
    /// Current basic block
    current_block: Option<BasicBlock<'ctx>>,
    /// Variable allocations (name -> pointer, pointee type)
    ///
    /// LLVM 17 uses opaque pointers, so the pointee type cannot be recovered
    /// from the pointer value itself; `build_load` needs it explicitly.
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    /// Named struct types
    struct_types: HashMap<String, inkwell::types::StructType<'ctx>>,
}

impl<'ctx> LLVMModuleBuilder<'ctx> {
    /// Create a new module builder
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
            variables: HashMap::new(),
            struct_types: HashMap::new(),
        })
    }

    /// The inkwell builder shared by this type and its value builder
    fn llvm_builder(&self) -> &LlvmBuilder<'ctx> {
        self.value_builder
            .as_ref()
            .expect("value_builder not initialized")
            .builder()
    }

    /// Get the underlying LLVM module
    pub fn module(&self) -> &LlvmModule<'ctx> {
        &self.module
    }

    /// Get the LLVM context
    pub fn llvm_context(&self) -> &inkwell::context::Context {
        self.context.llvm_context()
    }

    /// Get the type lowering context
    pub fn type_lowering(&mut self) -> &mut LlvmTypeLowering<'ctx> {
        &mut self.type_lowering
    }

    /// Get the value builder
    pub fn value_builder(&mut self) -> &mut LlvmValueBuilder<'ctx> {
        self.value_builder
            .as_mut()
            .expect("value_builder not initialized")
    }

    /// Set the current function
    pub fn set_current_function(&mut self, func: FunctionValue<'ctx>) {
        self.current_function = Some(func);
    }

    /// Get the current function
    pub fn current_function(&self) -> Option<FunctionValue<'ctx>> {
        self.current_function
    }

    /// Set the current basic block
    pub fn set_current_block(&mut self, block: BasicBlock<'ctx>) {
        self.current_block = Some(block);
        self.llvm_builder().position_at_end(block);
    }

    /// Get the current basic block
    pub fn current_block(&self) -> Option<BasicBlock<'ctx>> {
        self.current_block
    }

    /// Add a variable allocation
    ///
    /// `ty` is the pointee type of the allocation; LLVM 17 opaque pointers do
    /// not carry it, so it is recorded here for the later `build_load`.
    pub fn add_variable(&mut self, name: String, ptr: PointerValue<'ctx>, ty: BasicTypeEnum<'ctx>) {
        self.variables.insert(name, (ptr, ty));
    }

    /// Get a variable allocation
    pub fn get_variable(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.variables.get(name).map(|(ptr, _)| *ptr)
    }

    /// Build the entire PIR module
    pub fn build_module(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        // Declare external functions
        for extern_fn in &pir_module.extern_functions {
            self.declare_extern_function(extern_fn)?;
        }

        // Build each statement as a function
        for stmt in &pir_module.statements {
            self.build_statement(stmt, &pir_module.quantities)?;
        }

        // Verify the module
        self.module
            .verify()
            .map_err(|e| CodegenError::VerificationError(e.to_string()))?;

        Ok(())
    }

    /// Declare an external function
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

    /// Build a PIR statement as a function
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
        self.variables.clear();

        Ok(())
    }

    /// Build a PIR expression
    fn build_expr(
        &mut self,
        expr: &PirExpr,
        quantities: &HashMap<String, Quantity>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        match expr {
            PirExpr::IntLit(val) => {
                let int_type = self
                    .type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I64);
                Ok(int_type.const_int(*val as u64, false).into())
            }
            PirExpr::FloatLit(val) => {
                let float_type = self
                    .type_lowering
                    .float_type(crate::codegen::abi::FloatWidth::F64);
                let parsed = val.parse::<f64>().unwrap_or(0.0);
                Ok(float_type.const_float(parsed).into())
            }
            PirExpr::BoolLit(val) => {
                let bool_type = self
                    .type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I1);
                Ok(bool_type.const_int(*val as u64, false).into())
            }
            PirExpr::Var(name) => {
                if let Some((ptr, ty)) = self.variables.get(name).copied() {
                    let load = self
                        .llvm_builder()
                        .build_load(ty, ptr, name)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                    Ok(load)
                } else {
                    // Return zero for undefined variables (should not happen in valid IR)
                    let int_type = self
                        .type_lowering
                        .int_type(crate::codegen::abi::IntWidth::I64);
                    Ok(int_type.const_zero().into())
                }
            }
            PirExpr::Binary { op, left, right } => {
                let l = self.build_expr(left, quantities)?;
                let r = self.build_expr(right, quantities)?;
                self.build_binary_op(*op, l, r)
            }
            PirExpr::Unary { op, expr } => {
                let e = self.build_expr(expr, quantities)?;
                self.build_unary_op(*op, e)
            }
            PirExpr::Call { name, args } => {
                let arg_values: CodegenResult<Vec<_>> = args
                    .iter()
                    .map(|a| self.build_expr(a, quantities))
                    .collect();
                let arg_values = arg_values?;

                let func = self.module.get_function(name).ok_or_else(|| {
                    CodegenError::FunctionBuildError(format!("Function '{}' not found", name))
                })?;

                // inkwell 0.10 takes call arguments as `BasicMetadataValueEnum`.
                let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> = arg_values
                    .into_iter()
                    .map(BasicMetadataValueEnum::from)
                    .collect();

                let call = self
                    .llvm_builder()
                    .build_call(func, &arg_metadata, "call")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // A void call yields no value; `ValueKind::Instruction` is that
                // case. Fall back to the same placeholder used for undefined
                // variables, since statement bodies discard the result anyway.
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.void_placeholder()))
            }
            PirExpr::Let {
                name,
                qty: _,
                mutability: _,
                value,
                body,
            } => {
                // Allocate variable
                let val = self.build_expr(value, quantities)?;
                let alloca = self
                    .llvm_builder()
                    .build_alloca(val.get_type(), name)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.llvm_builder()
                    .build_store(alloca, val)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.add_variable(name.clone(), alloca, val.get_type());

                // Build body
                let result = self.build_expr(body, quantities)?;

                // Remove variable from scope
                self.variables.remove(name);

                Ok(result)
            }
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.build_expr(cond, quantities)?;
                let bool_type = self
                    .type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I1);
                let cond_bool = self
                    .llvm_builder()
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        cond_val.into_int_value(),
                        bool_type.const_zero(),
                        "if_cond",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                let func = self.current_function().unwrap();
                let then_block = self.context.llvm_context().append_basic_block(func, "then");
                let else_block = self.context.llvm_context().append_basic_block(func, "else");
                let merge_block = self
                    .context
                    .llvm_context()
                    .append_basic_block(func, "if_merge");

                self.llvm_builder()
                    .build_conditional_branch(cond_bool, then_block, else_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Then branch
                self.set_current_block(then_block);
                let then_val = self.build_expr(then_branch, quantities)?;
                self.llvm_builder()
                    .build_unconditional_branch(merge_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                let then_block_end = self.current_block().unwrap();

                // Else branch
                self.set_current_block(else_block);
                let else_val = self.build_expr(else_branch, quantities)?;
                self.llvm_builder()
                    .build_unconditional_branch(merge_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                let else_block_end = self.current_block().unwrap();

                // Merge block
                self.set_current_block(merge_block);
                let phi = self
                    .llvm_builder()
                    .build_phi(then_val.get_type(), "if_phi")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                phi.add_incoming(&[(&then_val, then_block_end), (&else_val, else_block_end)]);
                Ok(phi.as_basic_value())
            }
            PirExpr::Reversible { body, inverse } => {
                // For now, just build body and ignore inverse
                let _ = inverse;
                self.build_expr(body, quantities)
            }
            PirExpr::Index { base, indices } => {
                let base_ptr = self.build_expr(base, quantities)?;
                let index_vals: CodegenResult<Vec<_>> = indices
                    .iter()
                    .map(|i| self.build_expr(i, quantities))
                    .collect();
                let index_vals = index_vals?;
                let _ = index_vals;

                // Simplified: just return base pointer for now
                Ok(base_ptr)
            }
            PirExpr::Field { base, field } => {
                let base_val = self.build_expr(base, quantities)?;
                let _ = field;
                // Simplified: return base for now
                Ok(base_val)
            }
            PirExpr::QuantumOp { op, args, qubits } => {
                // Quantum operations lower to a call of the runtime intrinsic
                // with the same name, e.g. "h" -> "qir.h". Value arguments come
                // first, then the qubits they act on.
                let intrinsic_name = format!("qir.{}", op);

                let arg_values: CodegenResult<Vec<BasicValueEnum<'ctx>>> = args
                    .iter()
                    .map(|a| self.build_expr(a, quantities))
                    .collect();
                let mut arg_values = arg_values?;
                let qubit_values: CodegenResult<Vec<BasicValueEnum<'ctx>>> = qubits
                    .iter()
                    .map(|q| self.build_expr(q, quantities))
                    .collect();
                arg_values.extend(qubit_values?);

                let param_types: Vec<BasicTypeEnum<'ctx>> =
                    arg_values.iter().map(|v| v.get_type()).collect();

                // Declare the intrinsic on first use so the module is complete
                // even when no `extern_functions` entry mentioned it.
                let func = match self.module.get_function(&intrinsic_name) {
                    Some(f) => f,
                    None => {
                        let fn_type = self.type_lowering.fn_type(
                            None,
                            &param_types,
                            /* is_var_args */ false,
                        );
                        self.module.add_function(&intrinsic_name, fn_type, None)
                    }
                };

                let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> =
                    arg_values.iter().map(|a| (*a).into()).collect();

                let call = self
                    .llvm_builder()
                    .build_call(func, &arg_metadata, op)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Void-returning intrinsics (gates, releases) produce no value;
                // build_expr must still return one, so yield the i1 result zero.
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.type_lowering.result_type().const_zero().into()))
            }
        }
    }

    /// Placeholder value for expressions that produce no LLVM value (a void
    /// call). Mirrors the undefined-variable fallback: an `i64` zero.
    fn void_placeholder(&self) -> BasicValueEnum<'ctx> {
        let int_type = self
            .type_lowering
            .int_type(crate::codegen::abi::IntWidth::I64);
        int_type.const_zero().into()
    }

    fn build_binary_op(
        &mut self,
        op: BinaryOp,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        use inkwell::IntPredicate;

        let builder = self.llvm_builder();
        let left_int: IntValue<'ctx> = left.into_int_value();
        let right_int: IntValue<'ctx> = right.into_int_value();

        let result = match op {
            BinaryOp::Add => builder.build_int_add(left_int, right_int, "add"),
            BinaryOp::Sub => builder.build_int_sub(left_int, right_int, "sub"),
            BinaryOp::Mul => builder.build_int_mul(left_int, right_int, "mul"),
            BinaryOp::Div => builder.build_int_signed_div(left_int, right_int, "div"),
            BinaryOp::Mod => builder.build_int_signed_rem(left_int, right_int, "mod"),
            BinaryOp::And => builder.build_and(left_int, right_int, "and"),
            BinaryOp::Or => builder.build_or(left_int, right_int, "or"),
            BinaryOp::Xor => builder.build_xor(left_int, right_int, "xor"),
            BinaryOp::Eq => builder.build_int_compare(IntPredicate::EQ, left_int, right_int, "eq"),
            BinaryOp::Ne => builder.build_int_compare(IntPredicate::NE, left_int, right_int, "ne"),
            BinaryOp::Lt => builder.build_int_compare(IntPredicate::SLT, left_int, right_int, "lt"),
            BinaryOp::Le => builder.build_int_compare(IntPredicate::SLE, left_int, right_int, "le"),
            BinaryOp::Gt => builder.build_int_compare(IntPredicate::SGT, left_int, right_int, "gt"),
            BinaryOp::Ge => builder.build_int_compare(IntPredicate::SGE, left_int, right_int, "ge"),
            BinaryOp::Shl => builder.build_left_shift(left_int, right_int, "shl"),
            BinaryOp::Shr => builder.build_right_shift(left_int, right_int, true, "shr"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        Ok(result.into())
    }

    fn build_unary_op(
        &mut self,
        op: UnaryOp,
        expr: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let builder = self.llvm_builder();
        let int_val = expr.into_int_value();
        let result = match op {
            UnaryOp::Neg => builder.build_int_neg(int_val, "neg"),
            UnaryOp::Not => builder.build_not(int_val, "not"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        Ok(result.into())
    }

    /// Convert module to LLVM IR string
    pub fn module_to_string(&self) -> String {
        self.module.print_to_string().to_string()
    }

    /// Write module to .ll file
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
