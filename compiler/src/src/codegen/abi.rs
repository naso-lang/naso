//! ABI and Type Lowering
//!
//! Re-exports core types and LLVM implementation.

#[path = "abi_core.rs"]
pub mod abi_core;
#[cfg(feature = "llvm")]
#[path = "abi_llvm.rs"]
pub mod abi_llvm;

pub use abi_core::*;
