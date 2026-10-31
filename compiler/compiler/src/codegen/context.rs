//! Code Generation Context
//!
//! Re-exports core types and LLVM implementation.

#[path = "context_core.rs"]
pub mod context_core;
#[path = "context_llvm.rs"]
pub mod context_llvm;

pub use context_core::{CodegenTarget, OptLevel};
pub use context_llvm::CodegenContext;
