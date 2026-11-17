//! Naso Quantum Runtime
//!
//! Provides the execution layer for Naso quantum programs:
//! - QIR interpreter (CPU state-vector simulator)
//! - Quantum hardware backend interfaces (IBM, IonQ, Rigetti)
//! - Quantum error mitigation (ZNE, PEC, DD)
//! - Kernel dispatch and job management
//! - Hybrid classical/quantum execution

pub mod backend;
pub mod circuit;
pub mod dispatch;
pub mod error_mitigation;
pub mod execution;
pub mod hardware;
pub mod job;
pub mod qir_executor;
pub mod simulator;
pub mod types;

pub use backend::{QuantumBackend, BackendCapabilities, BackendConfig, ShotResult, Observable, CalibrationData, BackendProperties};
pub use circuit::{QuantumCircuit, GateOperation, CircuitBuilder};
pub use error_mitigation::{ErrorMitigation, ZNEConfig, ZNEMitigation, PECConfig, PECMitigation, DDConfig, DDMitigation, MeasurementErrorMitigation, ZNEMethod};
pub use execution::{ExecutionConfig, ExecutionResult, ExpectationValue};
pub use hardware::{HardwareBackend, IBMBackend, IonQBackend, RigettiBackend};
pub use job::{BatchJob, BatchResult, JobManager};
pub use qir_executor::{QIRExecutor, QIRRuntime, QIROperation, QIRCircuit};
pub use simulator::{StateVectorSimulator, SimulationConfig, DDSequence};
pub use types::{JobHandle, JobStatus, QuantumJob};
pub use dispatch::{KernelDispatcher, DispatchConfig, DispatchResult, VQEResult, QAOAResult};

/// Version of the quantum runtime
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize the quantum runtime (logging, global state)
pub fn init() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    });
}