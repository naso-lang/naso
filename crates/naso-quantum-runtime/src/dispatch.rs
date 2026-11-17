//! Kernel Dispatcher
//!
//! High-level interface for dispatching quantum kernels to backends.
//! Provides a unified API for quantum kernel execution with automatic
//! backend selection, error mitigation, and result processing.

use crate::backend::{BackendCapabilities, BackendConfig, QuantumBackend, Observable};
use crate::circuit::QuantumCircuit;
use crate::error_mitigation::{CompositeMitigation, ErrorMitigation, ZNEConfig, ZNEMitigation, PECConfig, PECMitigation, DDConfig, DDMitigation, MeasurementErrorMitigation};
use crate::execution::{ExecutionConfig, ExecutionResult, ExpectationValue};
use crate::hardware::{IBMBackend, IonQBackend, RigettiBackend};
use crate::{BatchJob, BatchResult, JobHandle, JobManager, JobStatus, QuantumJob};
use crate::simulator::StateVectorSimulator;
use crate::simulator::SimulationConfig;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Configuration for kernel dispatch
#[derive(Debug, Clone)]
pub struct DispatchConfig {
    /// Default backend to use
    pub default_backend: String,
    /// Whether to enable error mitigation by default
    pub enable_error_mitigation: bool,
    /// ZNE configuration
    pub zne_config: ZNEConfig,
    /// PEC configuration
    pub pec_config: PECConfig,
    /// DD configuration
    pub dd_config: DDConfig,
    /// Maximum parallel jobs
    pub max_parallel_jobs: usize,
    /// Default shots
    pub default_shots: usize,
    /// Timeout for job execution
    pub job_timeout_secs: u64,
}

impl Default for DispatchConfig {
    fn default() -> Self {
        Self {
            default_backend: "simulator".to_string(),
            enable_error_mitigation: false,
            zne_config: ZNEConfig::default(),
            pec_config: PECConfig::default(),
            dd_config: DDConfig::default(),
            max_parallel_jobs: 4,
            default_shots: 1024,
            job_timeout_secs: 300,
        }
    }
}

/// Result of kernel dispatch
#[derive(Debug, Clone)]
pub struct DispatchResult {
    pub job_handle: JobHandle,
    pub result: Option<ExecutionResult>,
    pub status: JobStatus,
    pub mitigated_result: Option<ExecutionResult>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl DispatchResult {
    pub fn new(job_handle: JobHandle) -> Self {
        Self {
            job_handle,
            result: None,
            status: JobStatus::Queued,
            mitigated_result: None,
            metadata: HashMap::new(),
        }
    }
}

/// High-level kernel dispatcher
pub struct KernelDispatcher {
    config: DispatchConfig,
    job_manager: Arc<JobManager>,
    simulator: Option<StateVectorSimulator>,
    mitigation: CompositeMitigation,
}

impl KernelDispatcher {
    /// Create a new kernel dispatcher
    pub fn new(config: DispatchConfig) -> Self {
        let job_manager = Arc::new(JobManager::new());
        
        // Build composite mitigation
        let mut mitigation = CompositeMitigation::new();
        mitigation = mitigation.add(Box::new(ZNEMitigation::new(config.zne_config.clone())));
        mitigation = mitigation.add(Box::new(PECMitigation::new(config.pec_config.clone())));
        mitigation = mitigation.add(Box::new(DDMitigation::new(config.dd_config.clone())));
        mitigation = mitigation.add(Box::new(MeasurementErrorMitigation::new(10))); // Will be updated per circuit
        
        Self {
            config,
            job_manager,
            simulator: None,
            mitigation,
        }
    }

    /// Initialize with a simulator
    pub fn with_simulator(mut self, config: SimulationConfig) -> Self {
        self.simulator = Some(StateVectorSimulator::new(config));
        self
    }

    /// Initialize with an IBM backend
    pub async fn with_ibm_backend(mut self, config: BackendConfig) -> Result<Self> {
        let backend = Arc::new(IBMBackend::new(config)?);
        self.job_manager.register_backend("ibm".to_string(), backend);
        if self.config.default_backend == "simulator" {
            self.config.default_backend = "ibm".to_string();
        }
        Ok(self)
    }

    /// Initialize with an IonQ backend
    pub async fn with_ionq_backend(mut self, config: BackendConfig) -> Result<Self> {
        let backend = Arc::new(IonQBackend::new(config)?);
        self.job_manager.register_backend("ionq".to_string(), backend);
        if self.config.default_backend == "simulator" {
            self.config.default_backend = "ionq".to_string();
        }
        Ok(self)
    }

    /// Initialize with a Rigetti backend
    pub async fn with_rigetti_backend(mut self, config: BackendConfig) -> Result<Self> {
        let backend = Arc::new(RigettiBackend::new(config)?);
        self.job_manager.register_backend("rigetti".to_string(), backend);
        if self.config.default_backend == "simulator" {
            self.config.default_backend = "rigetti".to_string();
        }
        Ok(self)
    }

    /// Dispatch a quantum circuit for execution
    pub async fn dispatch(
        &self,
        circuit: QuantumCircuit,
        config: Option<ExecutionConfig>,
    ) -> Result<DispatchResult> {
        let exec_config = config.unwrap_or_else(|| ExecutionConfig::new(self.config.default_shots));
        
        // Apply error mitigation if enabled
        let circuits = if self.config.enable_error_mitigation {
            self.mitigation.mitigate(&circuit)
        } else {
            vec![circuit]
        };

        let job = QuantumJob::new(circuits[0].clone(), exec_config)
            .with_backend(self.config.default_backend.clone());

        let handle = self.job_manager.submit_job(job, None).await?;
        let status = self.job_manager.job_status(&handle).await?;

        Ok(DispatchResult {
            job_handle: handle,
            result: None,
            status,
            mitigated_result: None,
            metadata: HashMap::new(),
        })
    }

    /// Dispatch and wait for result
    pub async fn dispatch_and_wait(
        &self,
        circuit: QuantumCircuit,
        config: Option<ExecutionConfig>,
    ) -> Result<DispatchResult> {
        let mut result = self.dispatch(circuit, config).await?;
        
        // Poll for completion
        let timeout = Duration::from_secs(self.config.job_timeout_secs);
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_secs(5);
        
        while start.elapsed() < timeout {
            let status = self.job_manager.job_status(&result.job_handle).await?;
            match status {
                JobStatus::Completed => {
                    let exec_result = self.job_manager.job_result(&result.job_handle).await?;
                    result.result = Some(exec_result.clone());
                    result.status = JobStatus::Completed;
                    
                    // Apply error mitigation post-processing
                    if self.config.enable_error_mitigation {
                        let mitigated = self.mitigation.process_results(&[exec_result]);
                        result.mitigated_result = Some(mitigated);
                    }
                    return Ok(result);
                }
                JobStatus::Failed(err) => {
                    return Err(anyhow!("Job failed: {}", err));
                }
                JobStatus::Cancelled => {
                    return Err(anyhow!("Job was cancelled"));
                }
                _ => {
                    sleep(poll_interval).await;
                }
            }
        }
        
        Err(anyhow!("Job timed out"))
    }

    /// Execute circuit on simulator (no queue)
    pub fn execute_simulator(
        &self,
        circuit: &QuantumCircuit,
        config: Option<SimulationConfig>,
    ) -> Result<ExecutionResult> {
        let sim_config = config.unwrap_or_default();
        let simulator = self.simulator.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No simulator initialized"))?;
        
        // Note: simulator.simulate requires &mut self, but we only have &self
        // For now, we'll use a workaround by cloning the simulator
        // In a real implementation, we'd use interior mutability or a different design
        let mut sim = simulator.clone();
        sim.simulate(circuit)
    }

    /// Execute with error mitigation on simulator
    pub fn execute_simulator_mitigated(
        &self,
        circuit: &QuantumCircuit,
        config: Option<SimulationConfig>,
    ) -> Result<ExecutionResult> {
        let result = self.execute_simulator(circuit, config)?;
        
        if self.config.enable_error_mitigation {
            Ok(self.mitigation.process_results(&[result]))
        } else {
            Ok(result)
        }
    }

    /// Dispatch a batch of circuits
    pub async fn dispatch_batch(&self, batch: BatchJob) -> Result<BatchResult> {
        let mut handles = Vec::new();
        let mut results = Vec::new();
        
        for job in batch.jobs {
            let handle = self.job_manager.submit_job(job, batch.backend_preference.as_deref()).await?;
            handles.push(handle);
        }
        
        // Wait for all jobs
        for handle in &handles {
            let result = self.job_manager.job_result(handle).await;
            results.push(result.ok());
        }
        
        Ok(BatchResult { handles, results })
    }

    /// Get job status
    pub async fn job_status(&self, handle: &JobHandle) -> Result<JobStatus> {
        self.job_manager.job_status(handle).await
    }

    /// Get job result
    pub async fn job_result(&self, handle: &JobHandle) -> Result<ExecutionResult> {
        self.job_manager.job_result(handle).await
    }

    /// Cancel a job
    pub async fn cancel_job(&self, handle: &JobHandle) -> Result<()> {
        self.job_manager.cancel_job(handle).await
    }

    /// Submit a quantum job with custom backend
    pub async fn submit_job(
        &self,
        circuit: QuantumCircuit,
        config: ExecutionConfig,
        backend: Option<String>,
    ) -> Result<JobHandle> {
        let job = QuantumJob::new(circuit, config)
            .with_backend(backend.unwrap_or_else(|| self.config.default_backend.clone()));
        
        self.job_manager.submit_job(job, None).await
    }

    /// Run VQE (Variational Quantum Eigensolver)
    pub async fn run_vqe(
        &self,
        _hamiltonian: Vec<Observable>,
        _ansatz: QuantumCircuit,
        initial_params: Vec<f64>,
        _max_iterations: usize,
    ) -> Result<VQEResult> {
        // VQE implementation would go here
        // This is a placeholder for the optimization loop
        Ok(VQEResult {
            optimal_params: initial_params,
            ground_state_energy: 0.0,
            iterations: 0,
            converged: false,
            history: vec![],
        })
    }

    /// Run QAOA (Quantum Approximate Optimization Algorithm)
    pub async fn run_qaoa(
        &self,
        _cost_hamiltonian: Vec<Observable>,
        _mixer_hamiltonian: Vec<Observable>,
        initial_params: Vec<f64>,
        _p: usize, // Number of QAOA layers
    ) -> Result<QAOAResult> {
        // QAOA implementation would go here
        Ok(QAOAResult {
            optimal_params: initial_params,
            best_value: 0.0,
            iterations: 0,
        })
    }

    /// Get list of available backends
    pub fn available_backends(&self) -> Vec<String> {
        vec!["simulator".to_string(), "ibm".to_string(), "ionq".to_string(), "rigetti".to_string()]
    }

    /// Get backend capabilities
    pub fn backend_capabilities(&self, _backend: &str) -> Option<&BackendCapabilities> {
        // Would need to query actual backends
        None
    }
}

/// VQE result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VQEResult {
    pub optimal_params: Vec<f64>,
    pub ground_state_energy: f64,
    pub iterations: usize,
    pub converged: bool,
    pub history: Vec<f64>,
}

/// QAOA result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QAOAResult {
    pub optimal_params: Vec<f64>,
    pub best_value: f64,
    pub iterations: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::QuantumCircuit;

    #[tokio::test]
    async fn test_dispatcher_creation() {
        let config = DispatchConfig::default();
        let dispatcher = KernelDispatcher::new(config);
        assert_eq!(dispatcher.config.default_backend, "simulator");
    }

    #[test]
    fn test_simulator_execution() {
        let mut builder = QuantumCircuit::builder(2);
        builder.h(0);
        builder.cx(0, 1);
        let circuit = builder.build();

        let config = DispatchConfig::default();
        let dispatcher = KernelDispatcher::new(config)
            .with_simulator(SimulationConfig::new(100).with_seed(42));

        let result = dispatcher.execute_simulator(&circuit, None).unwrap();
        assert!(result.counts.get("00").unwrap_or(&0) > &0);
        assert!(result.counts.get("11").unwrap_or(&0) > &0);
    }
}