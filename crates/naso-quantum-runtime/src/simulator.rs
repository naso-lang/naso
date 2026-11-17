//! State Vector Simulator
//!
//! CPU-based quantum state vector simulation for testing and verification.
//! Uses naso-gates for the core gate operations.

use crate::circuit::{GateOperation, QuantumCircuit};
use crate::execution::{ExecutionConfig, ExecutionResult, ExpectationValue};
pub use crate::simulator::simulator::StateVectorSimulator;
pub use crate::simulator::simulator::DDSequence;
use anyhow::Result;
use naso_gates::statevector::{Complex, Gate, StateVector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod simulator;

/// Configuration for state vector simulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationConfig {
    /// Number of shots (for sampling)
    pub shots: usize,
    /// Random seed for reproducibility
    pub seed: Option<u64>,
    /// Whether to return state vector instead of sampling
    pub return_statevector: bool,
    /// Whether to return probabilities instead of sampling
    pub return_probabilities: bool,
    /// Tolerance for numerical comparisons
    pub tolerance: f64,
    /// Enable noise model
    pub noise_model: Option<NoiseModel>,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            shots: 1024,
            seed: None,
            return_statevector: false,
            return_probabilities: false,
            tolerance: 1e-10,
            noise_model: None,
        }
    }
}

/// Noise model for realistic simulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseModel {
    /// Single-qubit depolarizing error rate
    pub single_qubit_depolarizing: f64,
    /// Two-qubit depolarizing error rate
    pub two_qubit_depolarizing: f64,
    /// Readout error (probability of flipping 0<->1)
    pub readout_error: f64,
    /// T1 relaxation time (microseconds)
    pub t1_us: Option<f64>,
    /// T2 dephasing time (microseconds)
    pub t2_us: Option<f64>,
    /// Gate time (nanoseconds)
    pub gate_time_ns: f64,
}

impl Default for NoiseModel {
    fn default() -> Self {
        Self {
            single_qubit_depolarizing: 0.0,
            two_qubit_depolarizing: 0.0,
            readout_error: 0.0,
            t1_us: None,
            t2_us: None,
            gate_time_ns: 50.0,
        }
    }
}

impl SimulationConfig {
    pub fn new(shots: usize) -> Self {
        Self {
            shots,
            ..Default::default()
        }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_statevector(mut self) -> Self {
        self.return_statevector = true;
        self
    }

    pub fn with_probabilities(mut self) -> Self {
        self.return_probabilities = true;
        self
    }

    pub fn with_noise(mut self, noise: NoiseModel) -> Self {
        self.noise_model = Some(noise);
        self
    }
}