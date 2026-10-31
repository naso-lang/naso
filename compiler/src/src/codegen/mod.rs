//! Code Generation Infrastructure
//!
//! This module provides the codegen pipeline for Naso, supporting multiple backends:
//! - LLVM (via inkwell) for native code generation
//! - QIR (Quantum Intermediate Representation) for quantum programs
//! - Cranelift: NOT IMPLEMENTED. It refuses, rather than returning a stub constant.
//!   There is no PIR lowering for it (no statement walk, control flow, ABI, or
//!   memory model), so `naso build --target cranelift` reports that plainly.
//!   It previously JIT-compiled a function returning the literal 42 and printed
//!   that as the result of any program -- two unrelated programs, same output,
//!   exit 0. See `cranelift::jit::compile_and_execute`.
//! - WGSL for WebGPU compute shaders

pub mod abi;
pub mod context;
pub mod error;
pub mod schedule_consumer;
pub mod validate;
pub mod wgsl;
pub mod wgsl_compute;
pub mod wgsl_straight;

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
pub use wgsl_straight::generate_wgsl_straight_line;

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

    /// Generate QIR from a PIR module.
    ///
    /// Linearity is checked BEFORE emission.
    ///
    /// This backend had no check at all, and that is not a stylistic gap. A QIR
    /// program whose linear values are used twice is not a slower program -- it is a
    /// program whose quantum state has been destroyed and then reused, and no
    /// downstream consumer can detect it. LLVM's module verifier accepts such a
    /// module, so the invalid circuit left this backend looking healthy:
    ///
    /// ```text
    ///     emit_qir(uses [1] twice) == Ok(1461 chars of valid QIR)
    /// ```
    ///
    /// The check used here is the same one the WGSL backend exposes.
    ///
    /// SCOPE, stated precisely so this is not oversold: the typechecker already
    /// rejects a doubled `[1]` value, and the CLI runs it before selecting a target,
    /// so no source program can reach here with this violation. What this closes is
    /// the library path -- `CodegenPipeline::emit_qir` is public API, and a PIR
    /// module built by hand, by a lowering pass, or by a fixture can carry a
    /// violation that no typechecker ever saw. Emitting a QIR circuit from such a
    /// module destroys and reuses quantum state, and nothing downstream can detect
    /// it, so the check belongs at the emission boundary rather than only upstream.
    pub fn emit_qir(&self, module: &PirModule) -> CodegenResult<String> {
        verify_wgsl_linearity(module)?;
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
    /// WGSL compute-shader text.
    ///
    /// Unlike the others this needs no LLVM: it is a text generator, so it is
    /// available in a default build. It is currently the straight-line backend
    /// only -- see `wgsl_straight`.
    Wgsl,
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
