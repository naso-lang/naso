//! QIR Execution Bridge
//!
//! Provides the bridge between QIR output from the Naso compiler
//! and the naso-quantum-runtime execution layer. Converts QIR
//! operations to executable quantum circuits.

use crate::circuit::{GateOperation, QuantumCircuit};
use crate::execution::{ExecutionConfig, ExecutionResult};
use crate::simulator::{StateVectorSimulator, SimulationConfig};
use crate::{JobHandle, JobManager, QuantumBackend, QuantumJob};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// QIR Operation in portable format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QIROperation {
    /// Intrinsic name (e.g., "qir.h", "qir.cx", "qir.rx")
    pub intrinsic: String,
    /// Qubit operands (as indices)
    pub qubits: Vec<u32>,
    /// Parameter values (for rotation angles, etc.)
    pub params: Vec<f64>,
    /// Source location for debugging
    pub location: Option<String>,
}

/// QIR Circuit representation (portable format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QIRCircuit {
    /// Number of qubits
    pub num_qubits: u32,
    /// Sequence of operations
    pub operations: Vec<QIROperation>,
    /// Measurement operations at the end
    pub measurements: Vec<u32>,
}

/// QIR Module containing multiple circuits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QIRModule {
    /// Module name
    pub name: String,
    /// Circuits in this module
    pub circuits: Vec<QIRCircuit>,
    /// Global qubit count
    pub num_qubits: u32,
}

impl QIRCircuit {
    /// Create a new empty QIR circuit
    pub fn new(num_qubits: u32) -> Self {
        Self {
            num_qubits,
            operations: Vec::new(),
            measurements: Vec::new(),
        }
    }

    /// Add an operation
    pub fn add_operation(&mut self, op: QIROperation) {
        if op.intrinsic.starts_with("qir.m") || op.intrinsic == "qir.measure" {
            if let Some(q) = op.qubits.first() {
                self.measurements.push(*q);
            }
        } else {
            self.operations.push(op);
        }
    }

    /// Parse from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    /// Parse from JSON file
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content)
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Save to JSON file
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<()> {
        let json = self.to_json()?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

/// QIR Executor - runs QIR circuits on quantum backends
pub struct QIRExecutor {
    /// Simulator for local execution
    simulator: Option<StateVectorSimulator>,
    /// Job manager for hardware backends
    job_manager: JobManager,
    /// Default execution config
    default_config: ExecutionConfig,
}

impl QIRExecutor {
    /// Create a new QIR executor
    pub fn new() -> Self {
        Self {
            simulator: None,
            job_manager: JobManager::new(),
            default_config: ExecutionConfig::new(1024),
        }
    }

    /// Initialize with a simulator
    pub fn with_simulator(mut self, config: SimulationConfig) -> Self {
        self.simulator = Some(StateVectorSimulator::new(config));
        self
    }

    /// Register a hardware backend
    pub async fn with_backend(
        mut self,
        name: &str,
        backend: Arc<dyn QuantumBackend>,
    ) -> Result<Self> {
        self.job_manager.register_backend(name.to_string(), backend);
        Ok(self)
    }

    /// Convert QIR circuit to runtime QuantumCircuit
    pub fn qir_to_circuit(&self, qir: &QIRCircuit) -> Result<QuantumCircuit> {
        let mut builder = QuantumCircuit::builder(qir.num_qubits as usize);

        for op in &qir.operations {
            self.apply_qir_operation(&mut builder, op)?;
        }

        // Add measurements
        for &q in &qir.measurements {
            builder.measure(q as usize, q as usize);
        }

        Ok(builder.build())
    }

    /// Apply a single QIR operation to the circuit builder
    fn apply_qir_operation(
        &self,
        builder: &mut crate::circuit::CircuitBuilder,
        op: &QIROperation,
    ) -> Result<()> {
        match op.intrinsic.as_str() {
            // Single-qubit gates
            "qir.h" => {
                builder.h(op.qubits[0] as usize);
            }
            "qir.x" => {
                builder.x(op.qubits[0] as usize);
            }
            "qir.y" => {
                builder.y(op.qubits[0] as usize);
            }
            "qir.z" => {
                builder.z(op.qubits[0] as usize);
            }
            "qir.s" => {
                builder.s(op.qubits[0] as usize);
            }
            "qir.t" => {
                builder.t(op.qubits[0] as usize);
            }
            "qir.rx" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.rx(op.qubits[0] as usize, theta);
            }
            "qir.ry" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.ry(op.qubits[0] as usize, theta);
            }
            "qir.rz" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.rz(op.qubits[0] as usize, theta);
            }
            "qir.r1" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                // r1(theta, qubit) = rz on target qubit
                if op.qubits.len() >= 2 {
                    builder.rz(op.qubits[1] as usize, theta);
                } else if !op.qubits.is_empty() {
                    builder.rz(op.qubits[0] as usize, theta);
                }
            }

            // Two-qubit gates
            "qir.cx" => {
                builder.cx(op.qubits[0] as usize, op.qubits[1] as usize);
            }
            "qir.cy" => {
                builder.cy(op.qubits[0] as usize, op.qubits[1] as usize);
            }
            "qir.cz" => {
                builder.cz(op.qubits[0] as usize, op.qubits[1] as usize);
            }
            "qir.ccx" => {
                builder.ccx(
                    op.qubits[0] as usize,
                    op.qubits[1] as usize,
                    op.qubits[2] as usize,
                );
            }
            "qir.swap" => {
                builder.swap(op.qubits[0] as usize, op.qubits[1] as usize);
            }
            "qir.iswap" => {
                builder.iswap(op.qubits[0] as usize, op.qubits[1] as usize);
            }

            // Controlled rotations
            "qir.crx" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.crx(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }
            "qir.cry" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.cry(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }
            "qir.crz" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.crz(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }

            // Ising couplings
            "qir.rxx" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.rxx(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }
            "qir.ryy" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.ryy(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }
            "qir.rzz" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.rzz(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }

            // Controlled phase
            "qir.cphase" => {
                let theta = op.params.first().copied().unwrap_or(0.0);
                builder.cphase(op.qubits[0] as usize, op.qubits[1] as usize, theta);
            }

            // Fredkin
            "qir.cswap" => {
                builder.cswap(
                    op.qubits[0] as usize,
                    op.qubits[1] as usize,
                    op.qubits[2] as usize,
                );
            }

            // Measurement
            "qir.mz" | "qir.mx" | "qir.my" | "qir.measure" => {
                builder.measure(op.qubits[0] as usize, op.qubits[0] as usize);
            }

            // Qubit allocation/release - handled by qubit count
            "qir.qubit_alloc" | "qir.qubit_alloc_array" => {
                // Qubits are allocated by builder size
            }
            "qir.qubit_release" | "qir.qubit_release_array" => {
                // Qubits are released implicitly
            }

            // Dynamic operations - not yet supported
            "qir.controlled" => {
                return Err(anyhow!("Dynamic controlled operation not yet supported"));
            }
            "qir.adjoint" => {
                return Err(anyhow!("Dynamic adjoint operation not yet supported"));
            }

            _ => {
                return Err(anyhow!("Unknown QIR intrinsic: {}", op.intrinsic));
            }
        }

        Ok(())
    }

    /// Execute a QIR circuit on the simulator
    pub fn execute_simulator(
        &self,
        qir: &QIRCircuit,
        config: Option<ExecutionConfig>,
    ) -> Result<ExecutionResult> {
        let simulator = self.simulator.as_ref()
            .ok_or_else(|| anyhow!("No simulator initialized"))?;

        let circuit = self.qir_to_circuit(qir)?;
        let exec_config = config.unwrap_or(self.default_config.clone());

        // Note: simulator requires &mut, but we have &self
        let mut sim = simulator.clone();
        sim.simulate(&circuit)
    }

    /// Execute a QIR circuit with error mitigation
    pub fn execute_simulator_mitigated(
        &self,
        qir: &QIRCircuit,
        config: Option<ExecutionConfig>,
    ) -> Result<ExecutionResult> {
        let result = self.execute_simulator(qir, config)?;

        // Apply error mitigation if enabled
        // (Would integrate with error_mitigation module)
        Ok(result)
    }

    /// Submit QIR circuit to hardware backend
    pub async fn submit_to_backend(
        &self,
        qir: &QIRCircuit,
        backend_name: &str,
        config: Option<ExecutionConfig>,
    ) -> Result<JobHandle> {
        let circuit = self.qir_to_circuit(qir)?;
        let exec_config = config.unwrap_or(self.default_config.clone());

        let job = QuantumJob::new(circuit, exec_config)
            .with_backend(backend_name.to_string());

        self.job_manager.submit_job(job, Some(backend_name)).await
    }

    /// Execute QIR from JSON string
    pub fn execute_json(
        &self,
        json: &str,
        config: Option<ExecutionConfig>,
    ) -> Result<ExecutionResult> {
        let qir = QIRCircuit::from_json(json)?;
        self.execute_simulator(&qir, config)
    }

    /// Execute QIR from file
    pub fn execute_file(
        &self,
        path: &std::path::Path,
        config: Option<ExecutionConfig>,
    ) -> Result<ExecutionResult> {
        let qir = QIRCircuit::from_file(path)?;
        self.execute_simulator(&qir, config)
    }

    /// Execute QIR module (multiple circuits)
    pub fn execute_module(
        &self,
        module: &QIRModule,
        config: Option<ExecutionConfig>,
    ) -> Result<Vec<ExecutionResult>> {
        module.circuits.iter()
            .map(|c| self.execute_simulator(c, config.clone()))
            .collect()
    }
}

impl Default for QIRExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// High-level QIR Runtime
pub struct QIRRuntime {
    executor: QIRExecutor,
}

impl QIRRuntime {
    /// Create a new QIR runtime
    pub fn new() -> Self {
        Self {
            executor: QIRExecutor::new(),
        }
    }

    /// Initialize with simulator
    pub fn with_simulator(mut self, config: SimulationConfig) -> Self {
        self.executor = self.executor.with_simulator(config);
        self
    }

    /// Execute QIR from JSON string
    pub fn execute_json(
        &self,
        json: &str,
        config: Option<ExecutionConfig>,
    ) -> Result<ExecutionResult> {
        self.executor.execute_json(json, config)
    }

    /// Execute QIR from file
    pub fn execute_file(
        &self,
        path: &std::path::Path,
        config: Option<ExecutionConfig>,
    ) -> Result<ExecutionResult> {
        self.executor.execute_file(path, config)
    }

    /// Execute QIR module
    pub fn execute_module(
        &self,
        module: &QIRModule,
        config: Option<ExecutionConfig>,
    ) -> Result<Vec<ExecutionResult>> {
        self.executor.execute_module(module, config)
    }

    /// Get the executor for advanced usage
    pub fn executor(&self) -> &QIRExecutor {
        &self.executor
    }
}

impl Default for QIRRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create common QIR circuits
pub mod qir_builder {
    use super::*;

    /// Create a Bell pair circuit
    pub fn bell_pair() -> QIRCircuit {
        let mut circuit = QIRCircuit::new(2);
        circuit.add_operation(QIROperation {
            intrinsic: "qir.h".to_string(),
            qubits: vec![0],
            params: vec![],
            location: None,
        });
        circuit.add_operation(QIROperation {
            intrinsic: "qir.cx".to_string(),
            qubits: vec![0, 1],
            params: vec![],
            location: None,
        });
        circuit.measurements = vec![0, 1];
        circuit
    }

    /// Create a GHZ state circuit
    pub fn ghz_state(n: u32) -> QIRCircuit {
        let mut circuit = QIRCircuit::new(n);
        circuit.add_operation(QIROperation {
            intrinsic: "qir.h".to_string(),
            qubits: vec![0],
            params: vec![],
            location: None,
        });
        for i in 1..n {
            circuit.add_operation(QIROperation {
                intrinsic: "qir.cx".to_string(),
                qubits: vec![0, i],
                params: vec![],
                location: None,
            });
        }
        circuit.measurements = (0..n).collect();
        circuit
    }

    /// Create a W state circuit
    pub fn w_state(n: u32) -> QIRCircuit {
        let mut circuit = QIRCircuit::new(n);
        // W state preparation (simplified)
        for i in 0..n {
            let theta = 2.0 * (1.0 / (n - i) as f64).sqrt().acos();
            circuit.add_operation(QIROperation {
                intrinsic: "qir.ry".to_string(),
                qubits: vec![i],
                params: vec![theta],
                location: None,
            });
            if i + 1 < n {
                circuit.add_operation(QIROperation {
                    intrinsic: "qir.cx".to_string(),
                    qubits: vec![i, i + 1],
                    params: vec![],
                    location: None,
                });
            }
        }
        circuit.measurements = (0..n).collect();
        circuit
    }

    /// Create a VQE ansatz circuit
    pub fn vqe_ansatz(n: u32, params: &[f64]) -> QIRCircuit {
        let mut circuit = QIRCircuit::new(n);
        let mut param_idx = 0;

        // Hardware-efficient ansatz
        for layer in 0..3 {
            // Single-qubit rotations
            for i in 0..n {
                if param_idx < params.len() {
                    circuit.add_operation(QIROperation {
                        intrinsic: "qir.rx".to_string(),
                        qubits: vec![i],
                        params: vec![params[param_idx]],
                        location: None,
                    });
                    param_idx += 1;
                }
                if param_idx < params.len() {
                    circuit.add_operation(QIROperation {
                        intrinsic: "qir.ry".to_string(),
                        qubits: vec![i],
                        params: vec![params[param_idx]],
                        location: None,
                    });
                    param_idx += 1;
                }
                if param_idx < params.len() {
                    circuit.add_operation(QIROperation {
                        intrinsic: "qir.rz".to_string(),
                        qubits: vec![i],
                        params: vec![params[param_idx]],
                        location: None,
                    });
                    param_idx += 1;
                }
            }

            // Entangling layer
            for i in 0..n - 1 {
                circuit.add_operation(QIROperation {
                    intrinsic: "qir.cx".to_string(),
                    qubits: vec![i, i + 1],
                    params: vec![],
                    location: None,
                });
            }
            if n > 2 {
                circuit.add_operation(QIROperation {
                    intrinsic: "qir.cx".to_string(),
                    qubits: vec![n - 1, 0],
                    params: vec![],
                    location: None,
                });
            }
        }
        circuit.measurements = (0..n).collect();
        circuit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qir_operation_mapping() {
        let executor = QIRExecutor::new();
        let mut builder = QuantumCircuit::builder(2);

        // Test H gate
        let op = QIROperation {
            intrinsic: "qir.h".to_string(),
            qubits: vec![0],
            params: vec![],
            location: None,
        };
        executor.apply_qir_operation(&mut builder, &op).unwrap();

        // Test CX gate
        let op = QIROperation {
            intrinsic: "qir.cx".to_string(),
            qubits: vec![0, 1],
            params: vec![],
            location: None,
        };
        executor.apply_qir_operation(&mut builder, &op).unwrap();

        // Test RX gate
        let op = QIROperation {
            intrinsic: "qir.rx".to_string(),
            qubits: vec![0],
            params: vec![1.57],
            location: None,
        };
        executor.apply_qir_operation(&mut builder, &op).unwrap();

        let circuit = builder.build();
        assert_eq!(circuit.num_qubits, 2);
        assert_eq!(circuit.operations.len(), 3);
    }

    #[test]
    fn test_qir_circuit_conversion() {
        let executor = QIRExecutor::new();
        let qir = QIRCircuit {
            num_qubits: 2,
            operations: vec![
                QIROperation {
                    intrinsic: "qir.h".to_string(),
                    qubits: vec![0],
                    params: vec![],
                    location: None,
                },
                QIROperation {
                    intrinsic: "qir.cx".to_string(),
                    qubits: vec![0, 1],
                    params: vec![],
                    location: None,
                },
            ],
            measurements: vec![0, 1],
        };

        let circuit = executor.qir_to_circuit(&qir).unwrap();
        assert_eq!(circuit.num_qubits, 2);
        assert_eq!(circuit.operations.len(), 4); // H, CX, Measure, Measure
    }

    #[test]
    fn test_qir_json_serialization() {
        let circuit = qir_builder::bell_pair();
        let json = circuit.to_json().unwrap();
        let parsed = QIRCircuit::from_json(&json).unwrap();
        assert_eq!(parsed.num_qubits, 2);
        assert_eq!(parsed.operations.len(), 2);
    }

    #[test]
    fn test_qir_builder_bell_pair() {
        let circuit = qir_builder::bell_pair();
        assert_eq!(circuit.num_qubits, 2);
        assert_eq!(circuit.operations.len(), 2);
        assert_eq!(circuit.measurements, vec![0, 1]);
    }

    #[test]
    fn test_qir_builder_ghz_state() {
        let circuit = qir_builder::ghz_state(3);
        assert_eq!(circuit.num_qubits, 3);
        assert_eq!(circuit.operations.len(), 3); // H + 2 CX
        assert_eq!(circuit.measurements, vec![0, 1, 2]);
    }
}