//! Code Generation Context - LLVM Implementation
//!
//! LLVM-specific context implementation using inkwell.
//! Provides stub when llvm feature is not enabled.

#[cfg(feature = "llvm")]
use super::context_core::{CodegenTarget, OptLevel};
#[cfg(feature = "llvm")]
use crate::codegen::error::{CodegenError, CodegenResult};
#[cfg(feature = "llvm")]
use inkwell::OptimizationLevel;
#[cfg(feature = "llvm")]
use inkwell::context::Context as LlvmContext;
#[cfg(feature = "llvm")]
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
#[cfg(feature = "llvm")]
use std::path::Path;
#[cfg(feature = "llvm")]
use target_lexicon::{Architecture, OperatingSystem, Triple};

#[cfg(feature = "llvm")]
/// LLVM-specific code generation context
pub struct CodegenContext {
    /// LLVM context (owned)
    llvm_context: LlvmContext,
    /// Target machine for code generation
    target_machine: TargetMachine,
    /// Target triple
    target_triple: Triple,
    /// Optimization level
    opt_level: OptLevel,
    /// Data layout string
    data_layout: String,
    /// Whether to emit debug info
    emit_debug: bool,
}

#[cfg(feature = "llvm")]
impl CodegenContext {
    /// Create a new codegen context for the given target and optimization level
    pub fn new(target: CodegenTarget, opt_level: OptLevel) -> CodegenResult<Self> {
        Self::with_debug(target, opt_level, false)
    }

    /// Create a new codegen context with debug info option
    pub fn with_debug(
        target: CodegenTarget,
        opt_level: OptLevel,
        emit_debug: bool,
    ) -> CodegenResult<Self> {
        // Initialize LLVM targets
        Target::initialize_all(&InitializationConfig::default());

        let llvm_context = LlvmContext::create();
        let target_triple_str = target.triple();
        let target_triple = TargetTriple::create(target_triple_str);
        let target = Target::from_triple(&target_triple)
            .map_err(|e| CodegenError::TargetError(format!("Failed to get target: {}", e)))?;

        // Create target machine
        let cpu = "generic";
        let features = "";

        let target_machine = target
            .create_target_machine(
                &target_triple,
                cpu,
                features,
                inkwell_opt_level(opt_level),
                RelocMode::Default,
                CodeModel::Default,
            )
            .ok_or_else(|| {
                CodegenError::TargetError("Failed to create target machine".to_string())
            })?;

        // inkwell 0.10's `DataLayout` has no `Display` impl; the layout string
        // is a `&CStr` behind `as_str()`.
        let data_layout = target_machine
            .get_target_data()
            .get_data_layout()
            .as_str()
            .to_string_lossy()
            .into_owned();

        Ok(Self {
            llvm_context,
            target_machine,
            target_triple: target_triple_str
                .parse()
                .map_err(|e| CodegenError::TargetError(format!("Invalid target triple: {}", e)))?,
            opt_level,
            data_layout,
            emit_debug,
        })
    }

    /// Get the LLVM context
    pub fn llvm_context(&self) -> &LlvmContext {
        &self.llvm_context
    }

    /// Get the target machine
    pub fn target_machine(&self) -> &TargetMachine {
        &self.target_machine
    }

    /// Get the target triple
    pub fn target_triple(&self) -> &Triple {
        &self.target_triple
    }

    /// Get the optimization level
    pub fn opt_level(&self) -> OptLevel {
        self.opt_level
    }

    /// Get the data layout string
    pub fn data_layout(&self) -> &str {
        &self.data_layout
    }

    /// Check if debug info should be emitted
    pub fn emit_debug(&self) -> bool {
        self.emit_debug
    }

    /// Get the pointer size in bits for this target
    pub fn pointer_size(&self) -> u32 {
        // inkwell 0.10 replaced `get_pointer_size()` with
        // `get_pointer_byte_size(address_space)`; `None` means the default
        // (data) address space.
        self.target_machine
            .get_target_data()
            .get_pointer_byte_size(None)
            * 8
    }

    /// Write the module to an object file
    pub fn write_object_file(
        &self,
        module: &inkwell::module::Module,
        path: &Path,
    ) -> CodegenResult<()> {
        self.target_machine
            .write_to_file(module, FileType::Object, path)
            .map_err(|e| CodegenError::EmissionError(format!("Failed to write object file: {}", e)))
    }

    /// Write the module to assembly file
    pub fn write_assembly_file(
        &self,
        module: &inkwell::module::Module,
        path: &Path,
    ) -> CodegenResult<()> {
        self.target_machine
            .write_to_file(module, FileType::Assembly, path)
            .map_err(|e| {
                CodegenError::EmissionError(format!("Failed to write assembly file: {}", e))
            })
    }

    /// Get target architecture
    pub fn architecture(&self) -> Architecture {
        self.target_triple.architecture
    }

    /// Get target operating system
    pub fn operating_system(&self) -> OperatingSystem {
        self.target_triple.operating_system
    }

    /// Check if target is GPU (NVPTX)
    pub fn is_gpu(&self) -> bool {
        matches!(self.target_triple.architecture, Architecture::Nvptx64)
    }

    /// Check if target is WebAssembly
    pub fn is_wasm(&self) -> bool {
        matches!(self.target_triple.architecture, Architecture::Wasm32)
    }
}

#[cfg(feature = "llvm")]
impl std::fmt::Debug for CodegenContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodegenContext")
            .field("target_triple", &self.target_triple.to_string())
            .field("opt_level", &self.opt_level)
            .field("data_layout", &self.data_layout)
            .field("emit_debug", &self.emit_debug)
            .finish()
    }
}

#[cfg(feature = "llvm")]
/// Map the crate's `OptLevel` onto inkwell's `OptimizationLevel`.
///
/// inkwell 0.10 has no `From<OptLevel> for OptimizationLevel`, and the two are
/// distinct types that only happen to share variant names.
fn inkwell_opt_level(opt_level: OptLevel) -> OptimizationLevel {
    match opt_level {
        OptLevel::None => OptimizationLevel::None,
        OptLevel::Less => OptimizationLevel::Less,
        OptLevel::Default => OptimizationLevel::Default,
        OptLevel::Aggressive => OptimizationLevel::Aggressive,
    }
}

// Stub implementation when llvm feature is not enabled
#[cfg(not(feature = "llvm"))]
use super::context_core::{CodegenTarget, OptLevel};
#[cfg(not(feature = "llvm"))]
use crate::codegen::error::{CodegenError, CodegenResult};

#[cfg(not(feature = "llvm"))]
/// Stub CodegenContext when LLVM is not available
pub struct CodegenContext;

#[cfg(not(feature = "llvm"))]
impl CodegenContext {
    /// Create a new codegen context (stub - returns error)
    pub fn new(_target: CodegenTarget, _opt_level: OptLevel) -> CodegenResult<Self> {
        Err(CodegenError::UnsupportedFeature(
            "LLVM backend not enabled. Compile with 'llvm' feature.".to_string(),
        ))
    }

    /// Create a new codegen context with debug info option (stub - returns error)
    pub fn with_debug(
        _target: CodegenTarget,
        _opt_level: OptLevel,
        _emit_debug: bool,
    ) -> CodegenResult<Self> {
        Err(CodegenError::UnsupportedFeature(
            "LLVM backend not enabled. Compile with 'llvm' feature.".to_string(),
        ))
    }
}

// Re-export core types (already imported above)
// pub use super::context_core::{CodegenTarget, OptLevel};
