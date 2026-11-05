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
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::types::StructType;
use inkwell::AddressSpace;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct AwaitPoint<'ctx> {
    state: u8,
    resume_block: inkwell::basic_block::BasicBlock<'ctx>,
    captured_vars: Vec<BasicValueEnum<'ctx>>,
}

enum AsyncState {
    Start = 0,
    Running = 1,
    Yielded = 2,
    Done = 3,
}

/// Async lowering state
pub struct AsyncLowerer<'ctx, 'm> {
    module_builder: &'m mut LLVMModuleBuilder<'ctx>,
    quantities: &'m HashMap<String, crate::ast::Quantity>,
    async_struct_types: HashMap<u32, StructType<'ctx>>,
    next_async_id: u32,
    current_async_id: Option<u32>,
    await_point: u32,
    await_points: Vec<AwaitPoint<'ctx>>,
    entry_block: Option<inkwell::basic_block::BasicBlock<'ctx>>,
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
            await_points: Vec::new(),
            entry_block: None,
        }
    }

    /// Generate callback trampoline for an await point that uses async FFI
    /// This creates the bridge between C callbacks and Naso async/await
    fn emit_callback_trampoline(
        &mut self,
        await_point: &AwaitPoint<'ctx>,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        module: &inkwell::module::Module<'ctx>,
    ) -> CodegenResult<PointerValue<'ctx>> {
        let trampoline_name = format!("naso_async_callback_trampoline_{}", await_point.state);
        let context = value_builder.type_lowering().context();
        let trampoline_fn_type = context.void_type().fn_type(
            &[inkwell::types::BasicMetadataTypeEnum::PointerType(
                context.i8_type().ptr_type(AddressSpace::default()),
            )],
            false,
        );
        
        let trampoline_fn = match module.get_function(&trampoline_name) {
            Some(existing) => existing,
            None => module.add_function(
                &trampoline_name,
                trampoline_fn_type,
                Some(inkwell::module::Linkage::External),
            ),
        };
        
        let entry_block = context.append_basic_block(trampoline_fn, "entry");
        value_builder.builder().position_at_end(entry_block);
        
        let mut arg_iter = trampoline_fn.get_param_iter();
        let state_ptr = arg_iter
            .next()
            .ok_or_else(|| {
                CodegenError::instruction_with_span(
                    "Callback trampoline missing state_ptr parameter",
                    crate::ast::Span::default(),
                )
            })?
            .into_pointer_value();
        
        let callback_func = match module.get_function("naso_tcp_accept_callback_trampoline") {
            Some(func) => func,
            None => {
                return Err(CodegenError::instruction_with_span(
                    "naso_tcp_accept_callback_trampoline not found in FFI module",
                    crate::ast::Span::default(),
                ))
            }
        };
        
        value_builder
            .builder()
            .build_call(callback_func, &[state_ptr.into()], "callback_bridge")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        value_builder.builder().build_return(None).map_err(|e| {
            CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
        })?;
        
        Ok(trampoline_fn.as_global_value().as_pointer_value())
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
        _in_statement_position: bool,
        _loop_stack: &mut Vec<crate::codegen::llvm::expr_lowering::LoopTargets<'ctx>>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let prev_async_id = self.current_async_id;
        let prev_await_point = self.await_point;
        let prev_await_points = std::mem::take(&mut self.await_points);
        let prev_entry_block = self.entry_block;
        
        let async_id = self.next_async_id;
        self.next_async_id += 1;
        self.current_async_id = Some(async_id);
        self.await_point = 0;
        
        let future_new_fn = self.get_or_declare_naso_future_new(value_builder)?;
        
        let future_ptr = value_builder
            .builder()
            .build_call(future_new_fn, &[], "async.future")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| {
                CodegenError::instruction_with_span(
                    "naso_future_new returned void".to_string(),
                    crate::ast::Span::default(),
                )
            })?
            .into_int_value();
        
        let state_machine_type = self.create_state_machine_type(value_builder)?;
        self.async_struct_types.insert(async_id, state_machine_type);
        
        let state_machine_ptr = self.alloc_state_machine(value_builder, state_machine_type)?;
        
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I8);
        let state_ptr = value_builder
            .builder()
            .build_struct_gep(state_machine_ptr, 0, "state_ptr")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        value_builder
            .builder()
            .build_store(
                state_ptr,
                int_type.const_int(AsyncState::Start as u8 as u64, false),
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        let future_field_ptr = value_builder
            .builder()
            .build_struct_gep(state_machine_ptr, 1, "future_field_ptr")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        let future_ptr_val = value_builder
            .builder()
            .build_int_to_ptr(
                future_ptr,
                value_builder.type_lowering().ptr_type(
                    value_builder.type_lowering().context().i8_type().into(),
                    AddressSpace::from(0),
                ),
                "future_field_val",
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        value_builder
            .builder()
            .build_store(future_field_ptr, future_ptr_val)
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        let mut loop_stack = Vec::new();
        let mut lowerer = crate::codegen::llvm::expr_lowering::PirExprLowerer {
            value_builder,
            module,
            current_function,
            callee_params,
            in_statement_position: true,
            loop_stack,
            module_builder: Some(self.module_builder),
            quantities: Some(self.quantities),
        };
        
        let body_val = lowerer.build_expr(body, self.quantities)?;
        
        let _done_block = value_builder
            .type_lowering()
            .context()
            .append_basic_block(current_function, "async.done");
        
        value_builder
            .builder()
            .build_store(
                state_ptr,
                int_type.const_int(AsyncState::Done as u8 as u64, false),
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        self.complete_future(future_ptr, body_val, value_builder)?;
        
        let spawn_future_fn = self.get_or_declare_naso_spawn_future(value_builder)?;
        let future_ptr_val = value_builder
            .builder()
            .build_int_to_ptr(
                future_ptr,
                value_builder.type_lowering().ptr_type(
                    value_builder.type_lowering().context().i8_type().into(),
                    AddressSpace::from(0),
                ),
                "async.spawn.future_ptr",
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        let _ = value_builder
            .builder()
            .build_call(spawn_future_fn, &[future_ptr_val.into()], "async.spawn")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        self.current_async_id = prev_async_id;
        self.await_point = prev_await_point;
        self.await_points = prev_await_points;
        self.entry_block = prev_entry_block;
        
        Ok(future_ptr.into())
    }

    /// Build an await expression: `await future`
    /// This implements await point splitting - creates separate basic blocks
    /// before and after the await point
    pub fn build_await(
        &mut self,
        future: &PirExpr,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        module: &inkwell::module::Module<'ctx>,
        current_function: inkwell::values::FunctionValue<'ctx>,
        callee_params: Option<&HashMap<String, Vec<crate::ir::pir_types::ParamKind>>>,
        _in_statement_position: bool,
        _loop_stack: &mut Vec<crate::codegen::llvm::expr_lowering::LoopTargets<'ctx>>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let mut loop_stack = Vec::new();
        let mut lowerer = crate::codegen::llvm::expr_lowering::PirExprLowerer {
            value_builder,
            module,
            current_function,
            callee_params,
            in_statement_position: true,
            loop_stack,
            module_builder: Some(self.module_builder),
            quantities: Some(self.quantities),
        };
        
        let future_ptr = lowerer.build_expr(future, self.quantities)?;
        
        let await_fn = self.get_or_declare_naso_await(value_builder)?;
        let yield_fn = self.get_or_declare_naso_yield(value_builder)?;
        
        let _before_await_block = value_builder.builder().get_insert_block().unwrap();
        
        let resume_block = value_builder
            .type_lowering()
            .context()
            .append_basic_block(current_function, "await.resume");
        
        let current_await_point = self.await_point;
        self.await_point += 1;
        
        let _int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I8);
        
        let future_ptr_val = value_builder
            .builder()
            .build_int_to_ptr(
                future_ptr.into_int_value(),
                value_builder.type_lowering().ptr_type(
                    value_builder.type_lowering().context().i8_type().into(),
                    AddressSpace::from(0),
                ),
                "await.future_ptr",
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        let _ = value_builder
            .builder()
            .build_call(await_fn, &[future_ptr_val.into()], "await.poll")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        let _ = value_builder
            .builder()
            .build_call(yield_fn, &[], "await.yield")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        value_builder
            .builder()
            .build_unconditional_branch(resume_block)
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        value_builder.builder().position_at_end(resume_block);
        
        let future_get_fn = self.get_or_declare_naso_future_get_result(value_builder)?;
        let result_type: inkwell::types::IntType<'ctx> = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I8);
        let result_ptr = value_builder
            .builder()
            .build_alloca(result_type, "await.result.buf")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        let _ = value_builder
            .builder()
            .build_call(
                future_get_fn,
                &[
                    value_builder
                        .builder()
                        .build_int_to_ptr(
                            future_ptr.into_int_value(),
                            value_builder.type_lowering().ptr_type(
                                value_builder.type_lowering().context().i8_type().into(),
                                AddressSpace::from(0),
                            ),
                            "await.get_result.future_ptr",
                        )
                        .map_err(|e| {
                            CodegenError::instruction_with_span(
                                e.to_string(),
                                crate::ast::Span::default(),
                            )
                        })?
                        .into(),
                    result_ptr.into(),
                    value_builder
                        .type_lowering()
                        .int_type(crate::codegen::abi::IntWidth::I64)
                        .const_int(8, false)
                        .into(),
                ],
                "await.get_result",
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        let loaded = value_builder
            .build_load(result_ptr, "await.loaded")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        self.await_points.push(AwaitPoint {
            state: current_await_point as u8,
            resume_block,
            captured_vars: Vec::new(),
        });
        
        Ok(loaded)
    }

    /// Create the state machine struct type for an async block
    fn create_state_machine_type(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<StructType<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I8);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        
        let state_machine_type = value_builder
            .type_lowering()
            .context()
            .struct_type(&[int_type.into(), ptr_type.into()], false);
        
        Ok(state_machine_type)
    }

    /// Allocate state machine on heap
    fn alloc_state_machine(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
        state_machine_type: StructType<'ctx>,
    ) -> CodegenResult<PointerValue<'ctx>> {
        let ptr = value_builder
            .builder()
            .build_alloca(state_machine_type, "async.state_machine")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        Ok(ptr)
    }

    /// Complete a future with a value
    fn complete_future(
        &mut self,
        future_ptr: IntValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<()> {
        let future_complete_fn = self.get_or_declare_naso_future_complete(value_builder)?;
        
        let int_type: inkwell::types::IntType<'ctx> = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let int_val = match value {
            BasicValueEnum::IntValue(v) => v,
            _ => int_type.const_int(0, false),
        };
        
        let buf_ptr = value_builder
            .builder()
            .build_alloca(int_type, "future.complete.buf")
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        value_builder
            .builder()
            .build_store(buf_ptr, int_val)
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        let _ = value_builder
            .builder()
            .build_call(
                future_complete_fn,
                &[
                    value_builder
                        .builder()
                        .build_int_to_ptr(
                            future_ptr,
                            value_builder.type_lowering().ptr_type(
                                value_builder.type_lowering().context().i8_type().into(),
                                AddressSpace::from(0),
                            ),
                            "future.complete.future_ptr",
                        )
                        .map_err(|e| {
                            CodegenError::instruction_with_span(
                                e.to_string(),
                                crate::ast::Span::default(),
                            )
                        })?
                        .into(),
                    buf_ptr.into(),
                    int_type.const_int(8, false).into(),
                ],
                "future.complete",
            )
            .map_err(|e| {
                CodegenError::instruction_with_span(e.to_string(), crate::ast::Span::default())
            })?;
        
        Ok(())
    }

    // FFI function declarations
    fn get_or_declare_naso_future_new(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = ptr_type.fn_type(&[], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_future_new", fn_type)
    }

    fn get_or_declare_naso_tcp_accept_async(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(
            &[
                int_type.into(),           // listener_fd
                ptr_type.into(),           // out_ptr
                int_type.into(),           // out_len
                ptr_type.into(),           // callback (function pointer as *u8)
                int_type.into(),           // callback_len
            ],
            false,
        );
        self.module_builder
            .get_or_declare_ffi_function("naso_tcp_accept_async", fn_type)
    }

    fn get_or_declare_naso_read_async(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(
            &[
                int_type.into(),           // fd
                ptr_type.into(),           // buf_ptr
                int_type.into(),           // len
                ptr_type.into(),           // callback (function pointer as *u8)
                int_type.into(),           // callback_len
            ],
            false,
        );
        self.module_builder
            .get_or_declare_ffi_function("naso_read_async", fn_type)
    }

    fn get_or_declare_naso_write_async(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(
            &[
                int_type.into(),           // fd
                ptr_type.into(),           // buf_ptr
                int_type.into(),           // len
                ptr_type.into(),           // callback (function pointer as *u8)
                int_type.into(),           // callback_len
            ],
            false,
        );
        self.module_builder
            .get_or_declare_ffi_function("naso_write_async", fn_type)
    }

    fn get_or_declare_naso_runtime_new(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = ptr_type.fn_type(&[int_type.into()], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_runtime_new", fn_type)
    }

    fn get_or_declare_naso_await(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(&[ptr_type.into()], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_await", fn_type)
    }

    fn get_or_declare_naso_future_complete(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(&[ptr_type.into(), ptr_type.into(), ptr_type.into()], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_future_complete", fn_type)
    }

    fn get_or_declare_naso_future_get_result(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(&[ptr_type.into(), ptr_type.into(), int_type.into()], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_future_get_result", fn_type)
    }

    fn get_or_declare_naso_spawn(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = ptr_type.fn_type(&[ptr_type.into(), ptr_type.into(), int_type.into()], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_spawn", fn_type)
    }

    fn get_or_declare_naso_spawn_future(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = int_type.fn_type(&[ptr_type.into()], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_spawn_future", fn_type)
    }

    fn get_or_declare_naso_yield(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        self.module_builder
            .get_or_declare_ffi_function("naso_yield", int_type.fn_type(&[], false))
    }

    fn get_or_declare_naso_channel_new(
        &mut self,
        value_builder: &mut LlvmValueBuilder<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        let int_type = value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        let ptr_type = value_builder
            .type_lowering()
            .ptr_type(int_type.into(), AddressSpace::from(0));
        let fn_type = ptr_type.fn_type(&[], false);
        self.module_builder
            .get_or_declare_ffi_function("naso_channel_new", fn_type)
    }
}

/// Extension trait for async lowering
pub trait AsyncLoweringExt<'ctx> {
    fn build_async(&mut self, body: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>>;
    fn build_await(&mut self, future: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>>;
}
