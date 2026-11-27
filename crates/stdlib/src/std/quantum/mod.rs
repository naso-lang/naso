// Quantum Standard Library for Naso
// Provides quantum primitives and algorithms.

// Declare modules
mod primitives;
mod grover;
mod qft;
mod variational;
mod qec;
mod hybrid;
mod hardware_vqe;
mod hardware_qaoa;
mod hardware_qec;
mod hpc_hybrid;
mod benchmarks;

// Re-export core quantum primitives
pub use primitives::*;

// Re-export quantum algorithms
pub use grover::*;
pub use qft::*;

// Re-export variational algorithms (VQE, QAOA)
pub use variational::*;

// Re-export QEC codes
pub use qec::*;

// Re-export hybrid classical/quantum algorithms
pub use hybrid::*;

// Re-export hardware-specific VQE demonstrations
pub use hardware_vqe::*;

// Re-export hardware-specific QAOA demonstrations
pub use hardware_qaoa::*;

// Re-export hardware-specific QEC demonstrations
pub use hardware_qec::*;

// Re-export hybrid HPC workflows
pub use hpc_hybrid::*;

// Re-export benchmark suite
pub use benchmarks::*;