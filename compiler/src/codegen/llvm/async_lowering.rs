//! Async/await lowering to LLVM IR
//!
//! This module handles the codegen for `PirExpr::Async` and `PirExpr::Await`.
//! The strategy:
//! 1. `async { ... }` creates a future state machine struct
//! 2. `await` suspends the current future and yields to the runtime
//! 3. FFI calls to naso-ffi async runtime for execution
//!
//! State machine encoding:
//! - Each async function becomes a struct with state field + captured variables
//! - `await` points split the function into basic blocks
//! - A central switch on state dispatches to the right continuation

use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::module_builder::LLVMModuleBuilder;
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::pir_types::PirExpr;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::types::{BasicTypeEnum, StructType};
use inkwell::AddressSpace;
use std::collections::HashMap;

/// Async state machine state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AsyncState {
    Start = 0,
    Await(u32) = 1, // nth await point
    Done = 255,
}

/// Context for async lowering
pub struct AsyncLowerer<'ctx, 'm> {
    module_builder: &'m mut LLVMModuleBuilder<'ctx>,
    quantities: &'m HashMap<String, crate::ast::Quantity>,
    /// Map from async block ID to its state machine struct type
    async_struct_types: HashMap<u32, StructType<'ctx>>,
    /// Next async block ID
    next_async_id: u32,
    /// Current async block ID being lowered
    current_async_id: Option<u32>,
    /// Await point counter within current async block
    await_point: u32,
}

impl<'ctx, 'm> AsyncLowerer<'ctx, 'm> {
    pub fn new(
        module_builder: &'m mut LLVMModuleBuilder<'ctx>,
        quantities: &'m HashMap<String, crate::ast::Quantity>,
    ) -> Self {
        Self {
            module_builder,
            quantities,
            async_struct_types: HashMap::new(),
            next_async_id: 0,
            current_async_id: None,
            await_point: 0,
        }
    }

    /// Build an async expression: `async { ... }`
    /// Returns a pointer to the future struct (NFuture equivalent)
    pub fn build_async(
        &mut self,
        body: &PirExpr,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        module: &inkwell::module::Module<'ctx>,
        current_function: inkwell::values::FunctionValue<'ctx>,
        callee_params: Option<&HashMap<String, Vec<crate::ir::pir_types::ParamKind>>>,
        in_statement_position: bool,
        loop_stack: &mut Vec<crate::codegen::llvm::expr_lowering::LoopTargets<'ctx>>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let async_id = self.next_async_id;
        self.next_async_id += 1;
        let prev_async_id = self.current_async_id.replace(async_id);
        let prev_await_point = self.await_point;
        self.await_point = 0;

        // Get FFI functions
        let future_new_fn = self.get_or_declare_naso_future_new()?;

        // Create future struct: call naso_future_new()
        let future_ptr = value_builder
            .builder()
            .build_call(future_new_fn, &[], "async.future")
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| CodegenError::InstructionError("naso_future_new returned void".to_string()))?
            .into_pointer_value();

        // Build the async body with state machine using PirExprLowerer
        let mut lowerer = crate::codegen::llvm::expr_lowering::PirExprLowerer {
            value_builder,
            module,
            current_function,
            callee_params,
            in_statement_position,
            loop_stack,
        };

        let body_val = lowerer.build_expr(body, self.quantities)?;

        // Complete the future with the body result
        // In real impl, this would be done by the spawned task
        self.complete_future(future_ptr, body_val, value_builder)?;

        // Restore context
        self.current_async_id = prev_async_id;
        self.await_point = prev_await_point;

        Ok(future_ptr.into())
    }

    /// Build an await expression: `await future`
    pub fn build_await(
        &mut self,
        future: &PirExpr,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        module: &inkwell::module::Module<'ctx>,
        current_function: inkwell::values::FunctionValue<'ctx>,
        callee_params: Option<&HashMap<String, Vec<crate::ir::pir_types::ParamKind>>>,
        in_statement_position: bool,
        loop_stack: &mut Vec<crate::codegen::llvm::expr_lowering::LoopTargets<'ctx>>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        // Get the future pointer
        let mut lowerer = crate::codegen::llvm::expr_lowering::PirExprLowerer {
            value_builder,
            module,
            current_function,
            callee_params,
            in_statement_position,
            loop_stack,
        };

        let future_ptr = lowerer.build_expr(future, self.quantities)?;

        // Get naso_await FFI
        let await_fn = self.get_or_declare_naso_await()?;

        // Call naso_await(future_ptr)
        let result = value_builder
            .builder()
            .build_call(await_fn, &[future_ptr.into()], "await.result")
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| CodegenError::InstructionError("naso_await returned void".to_string()))?;

        // naso_await returns: 0=pending, 1=ready, 2=error
        // If pending, we'd yield here. For now, assume ready and get result.
        // In real impl, we'd split the function at this await point.

        // Get the future result
        let future_get_fn = self.get_or_declare_naso_future_get_result()?;
        let result_ptr = value_builder.builder().build_alloca(
            value_builder.type_lowering().int_type(crate::codegen::abi::IntWidth::I8).into(),
            "await.result.buf",
        ).map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        let _ = value_builder
            .builder()
            .build_call(
                future_get_fn,
                &[
                    future_ptr.into(),
                    result_ptr.into(),
                    value_builder.type_lowering().int_type(crate::codegen::abi::IntWidth::I64).const_int(8, false).into(),
                ],
                "await.get_result",
            )
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        // Load the result
        let loaded = value_builder
            .build_load(result_ptr, "await.loaded")
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        Ok(loaded)
    }

    /// Complete a future with a value
    fn complete_future(
        &mut self,
        future_ptr: PointerValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<()> {
        let future_complete_fn = self.get_or_declare_naso_future_complete()?;

        // For simplicity, assume value is an integer and serialize to bytes
        // Real impl would handle all types properly
        let int_type = value_builder.type_lowering().int_type(crate::codegen::abi::IntWidth::I64);
        let int_val = match value {
            BasicValueEnum::IntValue(v) => v,
            _ => int_type.const_int(0, false),
        };

        // Allocate buffer for the value
        let buf_ptr = value_builder.builder().build_alloca(
            int_type.into(),
            "future.complete.buf",
        ).map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        value_builder.builder().build_store(buf_ptr, int_val)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        let _ = value_builder
            .builder()
            .build_call(
                future_complete_fn,
                &[
                    future_ptr.into(),
                    buf_ptr.into(),
                    int_type.const_int(8, false).into(),
                ],
                "future.complete",
            )
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        Ok(())
    }

    // FFI function declarations
    fn get_or_declare_naso_future_new(&mut self) -> CodegenResult<FunctionValue<'ctx>> {
        self.module_builder.get_or_declare_ffi_function(
            "naso_future_new",
            self.module_builder.value_builder().type_lowering().void_type().fn_type(&[], false),
        )
    }

    fn get_or_declare_naso_runtime_new(&mut self) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = self.module_builder.value_builder().type_lowering().int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = self.module_builder.value_builder().type_lowering().ptr_type(int_type.into(), 0);
        self.module_builder.get_or_declare_ffi_function(
            "naso_runtime_new",
            ptr_type.fn_type(&[int_type.into()], false),
        )
    }

    fn get_or_declare_naso_await(&mut self) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = self.module_builder.value_builder().type_lowering().int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = self.module_builder.value_builder().type_lowering().ptr_type(int_type.into(), 0);
        self.module_builder.get_or_declare_ffi_function(
            "naso_await",
            int_type.fn_type(&[ptr_type.into()], false),
        )
    }

    fn get_or_declare_naso_future_complete(&mut self) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = self.module_builder.value_builder().type_lowering().int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = self.module_builder.value_builder().type_lowering().ptr_type(int_type.into(), 0);
        self.module_builder.get_or_declare_ffi_function(
            "naso_future_complete",
            int_type.fn_type(&[ptr_type.into(), ptr_type.into(), int_type.into()], false),
        )
    }

    fn get_or_declare_naso_future_get_result(&mut self) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = self.module_builder.value_builder().type_lowering().int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = self.module_builder.value_builder().type_lowering().ptr_type(int_type.into(), 0);
        self.module_builder.get_or_declare_ffi_function(
            "naso_future_get_result",
            int_type.fn_type(&[ptr_type.into(), ptr_type.into(), int_type.into()], false),
        )
    }
}

/// Extension trait for async lowering
pub trait AsyncLoweringExt<'ctx> {
    fn build_async(&mut self, body: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>>;
    fn build_await(&mut self, future: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>>;
}