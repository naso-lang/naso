//! Execution Configuration and Results
//!
//! Defines how quantum circuits are executed and what results are returned.

use crate::backend::Observable;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for circuit execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Number of shots (measurements)
    pub shots: usize,
    /// Random seed for reproducibility
    pub seed: Option<u64>,
    /// Whether to return full state vector
    pub return_statevector: bool,
    /// Whether to return probabilities
    pub return_probabilities: bool,
    /// Whether to measure all qubits (default: true)
    pub measure_all: bool,
    /// Specific qubits to measure (if not measure_all)
    pub measure_qubits: Vec<usize>,
    /// Optimization level for circuit transpilation
    pub optimization_level: u8,
    /// Initial layout mapping (virtual -> physical qubits)
    pub initial_layout: Option<Vec<usize>>,
    /// Dynamical decoupling sequence
    pub dynamical_decoupling: Option<DynamicalDecouplingConfig>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            shots: 1024,
            seed: None,
            return_statevector: false,
            return_probabilities: false,
            measure_all: true,
            measure_qubits: vec![],
            optimization_level: 1,
            initial_layout: None,
            dynamical_decoupling: None,
        }
    }
}

impl ExecutionConfig {
    pub fn new(shots: usize) -> Self {
        Self { shots, ..Default::default() }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_statevector(mut self) -> Self {
        self.return_statevector = true;
        self
    }

    pub fn with_measure_qubits(mut self, qubits: Vec<usize>) -> Self {
        self.measure_all = false;
        self.measure_qubits = qubits;
        self
    }

    pub fn with_optimization(mut self, level: u8) -> Self {
        self.optimization_level = level.min(3);
        self
    }
}

/// Dynamical decoupling configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicalDecouplingConfig {
    /// Sequence type (XY4, XY8, CPMG, etc.)
    pub sequence: String,
    /// Spacing between pulses (in dt units)
    pub spacing: usize,
    /// Whether to enable on idle qubits
    pub enable_on_idle: bool,
}

/// Result of circuit execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Measurement counts (bitstring -> count)
    pub counts: HashMap<String, usize>,
    /// Full state vector amplitudes (basis -> (real, imag))
    pub statevector: Option<HashMap<String, (f64, f64)>>,
    /// Probabilities for each basis state
    pub probabilities: Option<HashMap<String, f64>>,
    /// Expectation values for observables
    pub expectation_values: Vec<ExpectationValue>,
    /// Metadata (timing, backend info, etc.)
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ExecutionResult {
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
            statevector: None,
            probabilities: None,
            expectation_values: vec![],
            metadata: HashMap::new(),
        }
    }

    pub fn total_shots(&self) -> usize {
        self.counts.values().sum()
    }

    pub fn get_probability(&self, bitstring: &str) -> f64 {
        let total = self.total_shots() as f64;
        if total == 0.0 {
            return 0.0;
        }
        *self.counts.get(bitstring).unwrap_or(&0) as f64 / total
    }

    pub fn most_likely(&self) -> Option<(&String, &usize)> {
        self.counts.iter().max_by_key(|&(_, v)| v)
    }
}

impl Default for ExecutionResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Expectation value of an observable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectationValue {
    /// Observable that was measured
    pub observable: Observable,
    /// Expectation value
    pub value: f64,
    /// Standard error
    pub std_err: f64,
    /// Number of shots used
    pub shots: usize,
    /// Variance
    pub variance: f64,
}

impl ExpectationValue {
    pub fn new(observable: Observable, value: f64, std_err: f64, shots: usize) -> Self {
        Self {
            observable,
            value,
            std_err,
            shots,
            variance: std_err * std_err * shots as f64,
        }
    }
}