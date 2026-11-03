//! Async/await lowering to LLVM IR - stub implementation
//!
//! This module provides the basic structure for async/await codegen.
//! Full implementation will be added incrementally.

use crate::codegen::error::{CodegenError, CodegenResult};
use crate::ir::pir_types::PirExpr;
use inkwell::values::BasicValueEnum;

/// Placeholder for async lowering state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncState {
    Start = 0,
    Await(u32),
    Done = 255,
}

/// Async lowering context (stub)
pub struct AsyncLowerer<'ctx, 'm> {
    _module: &'m crate::codegen::llvm::module_builder::LLVMModuleBuilder<'ctx>,
    _quantities: &'m std::collections::HashMap<String, crate::ast::Quantity>,
}

impl<'ctx, 'm> AsyncLowerer<'ctx, 'm> {
    pub fn new(
        module: &'m crate::codegen::llvm::module_builder::LLVMModuleBuilder<'ctx>,
        quantities: &'m std::collections::HashMap<String, crate::ast::Quantity>,
    ) -> Self {
        Self {
            _module: module,
            _quantities: quantities,
        }
    }

    /// Build an async expression: `async { ... }` - STUB
    pub fn build_async(&mut self, _body: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>> {
        Err(CodegenError::UnsupportedFeature(
            "async lowering not yet implemented".to_string()
        ))
    }

    /// Build an await expression: `await future` - STUB
    pub fn build_await(&mut self, _future: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>> {
        Err(CodegenError::UnsupportedFeature(
            "await lowering not yet implemented".to_string()
        ))
    }
}

/// Extension trait for async lowering (stub)
pub trait AsyncLoweringExt<'ctx> {
    fn build_async(&mut self, body: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>>;
    fn build_await(&mut self, future: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>>;
}