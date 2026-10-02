//! QIR (Quantum Intermediate Representation) Backend
//!
//! Generates QIR-compatible LLVM IR for quantum programs following the Microsoft QIR spec.

#[cfg(feature = "llvm")]
pub mod module_builder;
pub mod primitives;
pub mod profile;

#[cfg(feature = "llvm")]
pub use module_builder::QIRModuleBuilder;
pub use primitives::{QirIntrinsic, QirPrimitive};
pub use profile::{QirProfile, QirProfileKind};

/// QIR Module representation (for runtime execution)
#[derive(Debug, Clone)]
pub struct QIRModule {
    /// Number of qubits in the circuit
    pub qubit_count: usize,
    /// Qubit quantities for each qubit
    pub qubit_quantities: Vec<crate::ast::Quantity>,
    /// Quantum operations
    pub operations: Vec<QIROperation>,
}

/// QIR Operation
#[derive(Debug, Clone)]
pub enum QIROperation {
    AllocateQubit {
        index: usize,
        quantity: crate::ast::Quantity,
    },
    ReleaseQubit {
        index: usize,
    },
    Gate {
        name: String,
        qubits: Vec<usize>,
        params: Vec<f64>,
    },
    Measure {
        qubit: usize,
        basis: usize,
    },
}

impl QIRModule {
    /// Get the number of qubits
    pub fn qubit_count(&self) -> usize {
        self.qubit_count
    }

    /// Get qubit quantities
    pub fn qubit_quantities(&self) -> &[crate::ast::Quantity] {
        &self.qubit_quantities
    }
}
