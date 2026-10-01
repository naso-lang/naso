//! LLVM Backend
//!
//! Provides LLVM IR generation via inkwell bindings.

#[cfg(feature = "llvm")]
pub mod access_emission;
#[cfg(feature = "llvm")]
pub mod expr_lowering;
#[cfg(feature = "llvm")]
pub mod loop_emission;
#[cfg(feature = "llvm")]
pub mod module_builder;
#[cfg(feature = "llvm")]
pub mod parallel;
#[cfg(feature = "llvm")]
pub mod polyhedral_opts;
#[cfg(feature = "llvm")]
pub mod schedule_lowering;
#[cfg(feature = "llvm")]
pub mod type_lowering;
#[cfg(feature = "llvm")]
pub mod value_builder;

#[cfg(feature = "llvm")]
pub use access_emission::AccessEmitter;
#[cfg(feature = "llvm")]
pub use expr_lowering::PirExprLowerer;
#[cfg(feature = "llvm")]
pub use loop_emission::LoopEmitter;
#[cfg(feature = "llvm")]
pub use module_builder::LLVMModuleBuilder;
#[cfg(feature = "llvm")]
pub use parallel::ParallelEmitter;
#[cfg(feature = "llvm")]
pub use polyhedral_opts::PolyhedralOptimizer;
#[cfg(feature = "llvm")]
pub use schedule_lowering::{ScheduleLowering, lower_schedule_tree};
#[cfg(feature = "llvm")]
pub use type_lowering::{LlvmType, LlvmTypeLowering};
#[cfg(feature = "llvm")]
pub use value_builder::LlvmValueBuilder;
