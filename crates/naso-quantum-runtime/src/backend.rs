//! Quantum Backend Trait and Configuration
//!
//! Defines the common interface for all quantum backends (simulator, hardware, etc.)

use crate::circuit::QuantumCircuit;
use crate::execution::{ExecutionConfig, ExecutionResult, ExpectationValue};
use crate::simulator::SimulationConfig;
use crate::types::{JobHandle, JobStatus, QuantumJob};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Capabilities that a quantum backend may support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCapabilities {
    /// Maximum number of qubits
    pub max_qubits: usize,
    /// Supported gate set
    pub supported_gates: Vec<String>,
    /// Whether the backend supports mid-circuit measurement
    pub supports_mid_circuit_measurement: bool,
    /// Whether the backend supports conditional operations
    pub supports_conditional_ops: bool,
    /// Whether the backend supports reset
    pub supports_reset: bool,
    /// Whether the backend supports dynamic circuits
    pub supports_dynamic_circuits: bool,
    /// Maximum shots per job
    pub max_shots: usize,
    /// Maximum circuit depth
    pub max_depth: Option<usize>,
    /// Whether the backend is a simulator
    pub is_simulator: bool,
    /// Backend-specific features
    pub features: HashMap<String, serde_json::Value>,
}

impl Default for BackendCapabilities {
    fn default() -> Self {
        Self {
            max_qubits: 0,
            supported_gates: vec![],
            supports_mid_circuit_measurement: false,
            supports_conditional_ops: false,
            supports_reset: false,
            supports_dynamic_circuits: false,
            max_shots: 10000,
            max_depth: None,
            is_simulator: false,
            features: HashMap::new(),
        }
    }
}

/// Configuration for a quantum backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    /// Backend name/identifier
    pub name: String,
    /// Backend type (simulator, ibm, ionq, rigetti, etc.)
    pub backend_type: String,
    /// API token for hardware backends
    pub api_token: Option<String>,
    /// API URL for hardware backends
    pub api_url: Option<String>,
    /// Backend-specific options
    pub options: HashMap<String, serde_json::Value>,
    /// Timeout for job submission (seconds)
    pub timeout_secs: u64,
    /// Polling interval for job status (seconds)
    pub poll_interval_secs: u64,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            backend_type: "simulator".to_string(),
            api_token: None,
            api_url: None,
            options: HashMap::new(),
            timeout_secs: 300,
            poll_interval_secs: 5,
        }
    }
}

/// Result of a single shot execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotResult {
    /// Measurement bitstring (e.g., "0101")
    pub bitstring: String,
    /// Probability/amplitude (for simulator)
    pub probability: Option<f64>,
    /// Timestamp
    pub timestamp: u64,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Trait for all quantum backends
#[async_trait::async_trait]
pub trait QuantumBackend: Send + Sync {
    /// Get backend capabilities
    fn capabilities(&self) -> &BackendCapabilities;

    /// Get backend configuration
    fn config(&self) -> &BackendConfig;

    /// Execute a quantum circuit and return measurement counts
    async fn execute(
        &self,
        circuit: &QuantumCircuit,
        config: &ExecutionConfig,
    ) -> Result<ExecutionResult>;

    /// Execute and return expectation values for observables
    async fn execute_expectation(
        &self,
        circuit: &QuantumCircuit,
        observables: &[Observable],
        config: &ExecutionConfig,
    ) -> Result<Vec<ExpectationValue>>;

    /// Submit a job asynchronously
    async fn submit_job(&self, job: QuantumJob) -> Result<JobHandle>;

    /// Get job status
    async fn job_status(&self, handle: &JobHandle) -> Result<JobStatus>;

    /// Get job result (blocks until complete)
    async fn job_result(&self, handle: &JobHandle) -> Result<ExecutionResult>;

    /// Cancel a job
    async fn cancel_job(&self, handle: &JobHandle) -> Result<()>;

    /// Calibrate the backend (for hardware)
    async fn calibrate(&self) -> Result<CalibrationData>;

    /// Get backend properties (T1, T2, gate errors, etc.)
    async fn properties(&self) -> Result<BackendProperties>;

    /// Close/cleanup backend
    async fn close(&self) -> Result<()>;
}

/// Observable for expectation value measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observable {
    /// Pauli string (e.g., "ZIZ", "XX", "Y")
    pub pauli_string: String,
    /// Coefficient
    pub coefficient: f64,
    /// Qubits this observable acts on
    pub qubits: Vec<usize>,
}

impl Observable {
    pub fn new(pauli_string: String, coefficient: f64, qubits: Vec<usize>) -> Self {
        Self { pauli_string, coefficient, qubits }
    }

    pub fn z(qubit: usize) -> Self {
        Self::new("Z".to_string(), 1.0, vec![qubit])
    }

    pub fn x(qubit: usize) -> Self {
        Self::new("X".to_string(), 1.0, vec![qubit])
    }

    pub fn y(qubit: usize) -> Self {
        Self::new("Y".to_string(), 1.0, vec![qubit])
    }

    pub fn zz(qubit1: usize, qubit2: usize) -> Self {
        Self::new("ZZ".to_string(), 1.0, vec![qubit1, qubit2])
    }
}

/// Calibration data for a backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationData {
    pub timestamp: u64,
    pub t1_times: Vec<f64>,
    pub t2_times: Vec<f64>,
    pub gate_errors: HashMap<String, f64>,
    pub readout_errors: Vec<f64>,
    pub frequency: Vec<f64>,
    pub anharmonicity: Vec<f64>,
}

/// Backend properties (static device info)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendProperties {
    pub backend_name: String,
    pub backend_version: String,
    pub n_qubits: usize,
    pub basis_gates: Vec<String>,
    pub coupling_map: Vec<[usize; 2]>,
    pub dt: Option<f64>, // dt in seconds
    pub dtm: Option<f64>, // dtm in seconds
    pub meas_map: Option<Vec<Vec<usize>>>,
    pub qubit_properties: Vec<QubitProperty>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QubitProperty {
    pub t1: Option<f64>,
    pub t2: Option<f64>,
    pub frequency: Option<f64>,
    pub anharmonicity: Option<f64>,
    pub readout_error: Option<f64>,
    pub prob_meas0_prep1: Option<f64>,
    pub prob_meas1_prep0: Option<f64>,
}