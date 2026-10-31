pub mod dispatcher;
pub mod exporter;
#[cfg(feature = "cranelift")]
pub mod jit;
/// Naso Runtime Execution Engine
///
/// Provides the runtime infrastructure for executing compiled Naso programs,
/// including quantum statevector simulation, classical-quantum JIT execution,
/// and hardware backend integration.
pub mod statevector;

pub use dispatcher::{ExecutionTarget, RuntimeDispatcher};
pub use statevector::{MeasurementResult, SimulatorConfig, StatevectorSimulator};

/// Main runtime entry point
#[allow(clippy::new_without_default)]
pub struct Runtime {
    dispatcher: RuntimeDispatcher,
    #[allow(dead_code)]
    config: RuntimeConfig,
}

/// Configuration for the runtime engine
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Default execution target
    pub default_target: ExecutionTarget,
    /// Statevector simulator configuration
    pub simulator_config: SimulatorConfig,
    /// Enable automatic uncomputation
    pub auto_uncompute: bool,
    /// Maximum qubits for simulation
    pub max_qubits: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            default_target: ExecutionTarget::Simulator,
            simulator_config: SimulatorConfig::default(),
            auto_uncompute: true,
            max_qubits: 30,
        }
    }
}

impl Runtime {
    /// Create a new runtime with default configuration
    pub fn new() -> Self {
        Self::with_config(RuntimeConfig::default())
    }

    /// Create a new runtime with custom configuration
    pub fn with_config(config: RuntimeConfig) -> Self {
        let dispatcher = RuntimeDispatcher::new(config.clone());
        Self { dispatcher, config }
    }

    /// Execute a QIR module
    #[cfg(feature = "llvm")]
    pub fn execute_qir(
        &mut self,
        module: &crate::codegen::qir::QIRModule,
    ) -> Result<ExecutionResult, RuntimeError> {
        self.dispatcher.execute(module)
    }

    #[cfg(not(feature = "llvm"))]
    pub fn execute_qir(&self, _module: &()) -> Result<ExecutionResult, RuntimeError> {
        Err(RuntimeError::ExecutionFailed(
            "QIR execution requires LLVM feature".into(),
        ))
    }

    /// Execute a compiled program
    pub fn execute(&mut self, program: &CompiledProgram) -> Result<ExecutionResult, RuntimeError> {
        self.dispatcher.execute_program(program)
    }

    /// Get the dispatcher for advanced usage
    pub fn dispatcher(&self) -> &RuntimeDispatcher {
        &self.dispatcher
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of program execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Classical return values
    pub classical_outputs: Vec<ClassicalValue>,
    /// Measurement results from quantum execution
    pub measurements: Vec<MeasurementResult>,
    /// Execution statistics
    pub stats: ExecutionStats,
}

/// Classical value representation
#[derive(Debug, Clone)]
pub enum ClassicalValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<ClassicalValue>),
    Unit,
}

/// Execution statistics
#[derive(Debug, Clone, Default)]
pub struct ExecutionStats {
    pub execution_time_ns: u64,
    pub gate_count: usize,
    pub measurement_count: usize,
    pub memory_peak_bytes: usize,
}

/// Runtime errors
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Invalid program: {0}")]
    InvalidProgram(String),
    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),
    #[error("Quantity violation: {0}")]
    QuantityViolation(String),
    #[error("Hardware backend error: {0}")]
    HardwareBackendError(String),
}

/// Compiled program representation
#[derive(Debug, Clone)]
pub struct CompiledProgram {
    #[cfg(feature = "llvm")]
    pub qir_module: Option<crate::codegen::qir::QIRModule>,
    #[cfg(not(feature = "llvm"))]
    pub qir_module: Option<()>, // Placeholder when LLVM feature is disabled
    pub llvm_module: Option<Vec<u8>>, // Serialized LLVM IR
    pub metadata: ProgramMetadata,
}

/// Program metadata
#[derive(Debug, Clone, Default)]
pub struct ProgramMetadata {
    pub qubit_count: usize,
    pub classical_memory_bytes: usize,
    pub entry_points: Vec<String>,
}
