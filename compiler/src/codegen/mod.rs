//! Code Generation Infrastructure
//!
//! This module provides the codegen pipeline for Naso, supporting multiple backends:
//! - LLVM (via inkwell) for native code generation
//! - QIR (Quantum Intermediate Representation) for quantum programs
//! - Cranelift for fast JIT compilation
//! - WGSL for WebGPU compute shaders

pub mod abi;
pub mod context;
pub mod error;
pub mod validate;
pub mod wgsl;

#[cfg(feature = "cranelift")]
pub mod cranelift;
#[cfg(feature = "llvm")]
pub mod llvm;
#[cfg(feature = "llvm")]
pub mod qir;

use crate::ir::pir_types::PirModule;

pub use abi::{QuantityAwareType, lower_pir_module_types, lower_pir_type};
pub use context::{CodegenTarget, OptLevel};
pub use error::{CodegenError, CodegenResult};

#[cfg(feature = "llvm")]
pub use context::CodegenContext;
#[cfg(feature = "llvm")]
pub use inkwell::OptimizationLevel;
#[cfg(feature = "llvm")]
pub use inkwell::builder::Builder as LlvmBuilder;
#[cfg(feature = "llvm")]
/// Re-export inkwell types for convenience
pub use inkwell::context::Context as LlvmContext;
#[cfg(feature = "llvm")]
pub use inkwell::module::Module as LlvmModule;
#[cfg(feature = "llvm")]
pub use inkwell::targets::{InitializationConfig, Target, TargetMachine};

// Re-export WGSL codegen
pub use wgsl::{WgslTarget, generate_wgsl, verify_wgsl_linearity};

/// Main entry point for code generation
#[cfg(feature = "llvm")]
pub struct CodegenPipeline {
    context: CodegenContext,
}

#[cfg(feature = "llvm")]
impl CodegenPipeline {
    pub fn new(context: CodegenContext) -> Self {
        Self { context }
    }

    /// Generate LLVM IR from a PIR module
    pub fn emit_llvm(&self, module: &PirModule) -> CodegenResult<String> {
        let mut builder = llvm::LLVMModuleBuilder::new(&self.context)?;
        builder.build_module(module)?;
        Ok(builder.module_to_string())
    }

    /// Generate QIR from a PIR module
    pub fn emit_qir(&self, module: &PirModule) -> CodegenResult<String> {
        let mut builder = qir::QIRModuleBuilder::new(&self.context)?;
        builder.build_module(module)?;
        Ok(builder.module_to_string())
    }

    #[cfg(feature = "cranelift")]
    /// Compile and execute via Cranelift JIT (stub)
    pub fn execute_cranelift_jit(&self, module: &PirModule) -> CodegenResult<i32> {
        cranelift::jit::compile_and_execute(module, &self.context)
    }

    #[cfg(not(feature = "cranelift"))]
    /// Compile and execute via Cranelift JIT (stub when not available)
    pub fn execute_cranelift_jit(&self, _module: &PirModule) -> CodegenResult<i32> {
        Err(CodegenError::UnsupportedFeature(
            "Cranelift backend not enabled. Compile with 'cranelift' feature.".to_string(),
        ))
    }
}

/// Stub CodegenPipeline when LLVM is not available
#[cfg(not(feature = "llvm"))]
pub struct CodegenPipeline;

#[cfg(not(feature = "llvm"))]
impl CodegenPipeline {
    pub fn new(_context: ()) -> Self {
        Self
    }

    /// Generate LLVM IR from a PIR module (stub when LLVM not available)
    pub fn emit_llvm(&self, _module: &PirModule) -> CodegenResult<String> {
        Err(CodegenError::UnsupportedFeature(
            "LLVM backend not enabled. Compile with 'llvm' feature.".to_string(),
        ))
    }

    /// Generate QIR from a PIR module (stub when LLVM not available)
    pub fn emit_qir(&self, _module: &PirModule) -> CodegenResult<String> {
        Err(CodegenError::UnsupportedFeature(
            "QIR backend requires LLVM. Compile with 'llvm' feature.".to_string(),
        ))
    }

    /// Compile and execute via Cranelift JIT (stub when not available)
    pub fn execute_cranelift_jit(&self, _module: &PirModule) -> CodegenResult<i32> {
        Err(CodegenError::UnsupportedFeature(
            "Cranelift backend not enabled. Compile with 'cranelift' feature.".to_string(),
        ))
    }
}

/// Target backend for code generation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Llvm,
    Qir,
    Cranelift,
}

/// Configuration for code generation
#[derive(Debug, Clone)]
pub struct CodegenConfig {
    pub backend: Backend,
    pub target: CodegenTarget,
    pub opt_level: OptLevel,
    pub output_path: Option<std::path::PathBuf>,
    pub emit_debug: bool,
}

impl Default for CodegenConfig {
    fn default() -> Self {
        Self {
            backend: Backend::Llvm,
            target: CodegenTarget::Host,
            opt_level: OptLevel::Default,
            output_path: None,
            emit_debug: false,
        }
    }
}
