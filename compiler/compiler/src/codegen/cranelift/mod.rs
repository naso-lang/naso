// @generated
#[cfg(feature = "cranelift")]
/// Cranelift Backend
///
/// Provides fast JIT compilation via Cranelift for development/debugging.
pub mod jit;

pub use jit::{CraneliftJit, compile_and_execute};
