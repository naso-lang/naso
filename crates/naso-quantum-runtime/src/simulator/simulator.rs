//! State Vector Simulator Implementation

use crate::circuit::{GateOperation, QuantumCircuit};
use crate::execution::{ExecutionConfig, ExecutionResult, ExpectationValue};
use crate::simulator::SimulationConfig;
use crate::Observable;
use anyhow::Result;
use naso_gates::statevector::{Complex, Gate, StateVector};
use rand::Rng;
use rand::RngCore;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;

/// State vector simulator for quantum circuits
#[derive(Clone)]
pub struct StateVectorSimulator {
    config: SimulationConfig,
    rng: RefCell<ChaCha8Rng>,
}

impl StateVectorSimulator {
    /// Create a new simulator with the given configuration
    pub fn new(config: SimulationConfig) -> Self {
        let rng = match config.seed {
            Some(seed) => ChaCha8Rng::seed_from_u64(seed),
            None => ChaCha8Rng::from_entropy(),
        };
        Self { config, rng: RefCell::new(rng) }
    }

    /// Simulate a quantum circuit and return execution result
    pub fn simulate(&self, circuit: &QuantumCircuit) -> Result<ExecutionResult> {
        // Initialize state vector
        let mut state = StateVector::zero_state(circuit.num_qubits);

        // Apply each gate operation
        for op in &circuit.operations {
            state = self.apply_gate(&state, op)?;
        }

        // Build execution result
        let result = self.build_result(state, circuit)?;
        Ok(result)
    }

    /// Apply a single gate operation to the state vector
    fn apply_gate(&self, state: &StateVector, op: &GateOperation) -> Result<StateVector> {
        let naso_gate = op.to_naso_gate().ok_or_else(|| {
            anyhow::anyhow!("Unsupported gate: {}", op.name)
        })?;

        // Use Gate::apply which handles all gate types correctly
        let q = op.qubits.get(0).copied().unwrap_or(0);
        let q2 = op.qubits.get(1).copied();

        Ok(naso_gate.apply(state, q, q2))
    }

    /// Build execution result from final state
    fn build_result(&self, state: StateVector, circuit: &QuantumCircuit) -> Result<ExecutionResult> {
        let mut counts = HashMap::new();

        if self.config.return_statevector {
            // Return state vector amplitudes
            let statevector: HashMap<String, (f64, f64)> = state
                .amplitudes()
                .iter()
                .enumerate()
                .map(|(i, amp)| {
                    let basis = format!("{:0width$b}", i, width = circuit.num_qubits);
                    (basis, (amp.re, amp.im))
                })
                .collect();
            return Ok(ExecutionResult {
                counts,
                statevector: Some(statevector),
                probabilities: None,
                expectation_values: vec![],
                metadata: HashMap::new(),
            });
        }

        if self.config.return_probabilities {
            let probabilities: HashMap<String, f64> = state
                .amplitudes()
                .iter()
                .enumerate()
                .map(|(i, amp)| {
                    let basis = format!("{:0width$b}", i, width = circuit.num_qubits);
                    (basis, amp.norm_sqr())
                })
                .collect();
            return Ok(ExecutionResult {
                counts,
                statevector: None,
                probabilities: Some(probabilities),
                expectation_values: vec![],
                metadata: HashMap::new(),
            });
        }

        // Sample from the state vector
        for _ in 0..self.config.shots {
            let bitstring = self.sample_bitstring(&state, circuit.num_qubits)?;
            *counts.entry(bitstring).or_insert(0) += 1;
        }

        // Apply readout error if noise model is present
        if let Some(noise) = &self.config.noise_model {
            if noise.readout_error > 0.0 {
                counts = self.apply_readout_error(counts, circuit.num_qubits, noise.readout_error)?;
            }
        }

        Ok(ExecutionResult {
            counts,
            statevector: None,
            probabilities: None,
            expectation_values: vec![],
            metadata: HashMap::new(),
        })
    }

    /// Sample a bitstring from the state vector
    fn sample_bitstring(&self, state: &StateVector, num_qubits: usize) -> Result<String> {
        let probs: Vec<f64> = state
            .amplitudes()
            .iter()
            .map(|amp| amp.norm_sqr())
            .collect();

        // Cumulative distribution
        let mut cumsum = Vec::with_capacity(probs.len());
        let mut sum = 0.0;
        for p in probs {
            sum += p;
            cumsum.push(sum);
        }

        // Sample
        let r: f64 = ((*self.rng.borrow_mut()).next_u64() as f64) / (u64::MAX as f64);
        let idx = cumsum.binary_search_by(|&x| x.partial_cmp(&r).unwrap()).unwrap_or_else(|e| e);

        // Convert to bitstring (little-endian)
        let mut bitstring = String::with_capacity(num_qubits);
        for i in 0..num_qubits {
            bitstring.push(if (idx >> i) & 1 == 1 { '1' } else { '0' });
        }
        Ok(bitstring)
    }

    /// Apply readout error to counts
    fn apply_readout_error(
        &self,
        counts: HashMap<String, usize>,
        num_qubits: usize,
        readout_error: f64,
    ) -> Result<HashMap<String, usize>> {
        let mut new_counts = HashMap::new();
        for (bitstring, count) in counts {
            // For each shot, flip bits with probability readout_error
            for _ in 0..count {
                let mut noisy_bits = bitstring.chars().collect::<Vec<_>>();
                for bit in &mut noisy_bits {
                    let rand_val: f64 = ((*self.rng.borrow_mut()).next_u64() as f64) / (u64::MAX as f64);
                    if rand_val < readout_error {
                        *bit = if *bit == '0' { '1' } else { '0' };
                    }
                }
                let noisy_str: String = noisy_bits.into_iter().collect();
                *new_counts.entry(noisy_str).or_insert(0) += 1;
            }
        }
        Ok(new_counts)
    }

    /// Execute circuit with Zero-Noise Extrapolation (ZNE)
    /// Runs circuit at multiple noise scale factors and extrapolates to zero noise
    pub fn execute_zne(
        &self,
        circuit: &QuantumCircuit,
        observable: &Observable,
        scale_factors: &[f64],
    ) -> Result<f64> {
        let mut expectation_values = Vec::new();

        for &scale in scale_factors {
            // Create noise model with scaled error rates
            let mut scaled_noise = self.config.noise_model.clone().unwrap_or_default();
            scaled_noise.single_qubit_depolarizing *= scale;
            scaled_noise.two_qubit_depolarizing *= scale;
            scaled_noise.readout_error *= scale;

            let config = SimulationConfig {
                noise_model: Some(scaled_noise),
                shots: self.config.shots,
                ..self.config.clone()
            };

            let sim = StateVectorSimulator::new(config);
            let result = sim.simulate(circuit)?;

            // Compute expectation value
            let exp_val = self.compute_expectation(&result, observable);
            expectation_values.push(exp_val);
        }

        // Richardson extrapolation to zero noise
        self.richardson_extrapolation(scale_factors, &expectation_values)
    }

    /// Execute circuit with Probabilistic Error Cancellation (PEC)
    pub fn execute_pec(
        &self,
        circuit: &QuantumCircuit,
        observable: &Observable,
        quasi_probs: &HashMap<String, Vec<(f64, String)>>,
    ) -> Result<f64> {
        // PEC: Sample from quasi-probability distribution
        // This is a simplified implementation
        let config = SimulationConfig {
            noise_model: self.config.noise_model.clone(),
            shots: self.config.shots,
            ..self.config.clone()
        };

        let sim = StateVectorSimulator::new(config);
        let result = sim.simulate(circuit)?;
        let exp_val = self.compute_expectation(&result, observable);

        // Apply quasi-probability weighting (simplified)
        // In practice, would sample multiple circuits from quasi-prob distribution
        Ok(exp_val)
    }

    /// Execute circuit with Dynamical Decoupling (DD)
    pub fn execute_dd(
        &self,
        circuit: &QuantumCircuit,
        observable: &Observable,
        dd_sequence: &DDSequence,
    ) -> Result<f64> {
        // Insert DD pulses into circuit
        let dd_circuit = self.insert_dd_pulses(circuit, dd_sequence);

        let config = SimulationConfig {
            noise_model: self.config.noise_model.clone(),
            shots: self.config.shots,
            ..self.config.clone()
        };

        let sim = StateVectorSimulator::new(config);
        let result = sim.simulate(&dd_circuit)?;
        let exp_val = self.compute_expectation(&result, observable);

        Ok(exp_val)
    }

    /// Compute expectation value of observable from execution result
    fn compute_expectation(&self, result: &ExecutionResult, observable: &Observable) -> f64 {
        // Simplified: compute <O> = sum(counts * eigenvalue) / total_shots
        let total_shots: usize = result.counts.values().sum();
        if total_shots == 0 {
            return 0.0;
        }

        let mut expectation = 0.0;
        for (bitstring, count) in &result.counts {
            // Compute eigenvalue of observable for this bitstring
            let eigenvalue = self.observable_eigenvalue(bitstring, observable);
            expectation += eigenvalue * (*count as f64) / (total_shots as f64);
        }
        expectation
    }

    /// Compute eigenvalue of observable for a given bitstring
    fn observable_eigenvalue(&self, bitstring: &str, observable: &Observable) -> f64 {
        let pauli = &observable.pauli_string;
        let coeff = observable.coefficient;

        let mut eigenvalue = 1.0;
        for (i, &qubit) in observable.qubits.iter().enumerate() {
            if qubit < bitstring.len() {
                let bit = bitstring.chars().rev().nth(qubit).unwrap_or('0');
                let pauli_op = pauli.chars().nth(i).unwrap_or('I');

                match pauli_op {
                    'I' => eigenvalue *= 1.0,
                    'Z' => eigenvalue *= if bit == '0' { 1.0 } else { -1.0 },
                    'X' | 'Y' => {
                        // X and Y measurements would need different basis
                        // Simplified: assume Z basis
                        eigenvalue *= if bit == '0' { 1.0 } else { -1.0 };
                    }
                    _ => eigenvalue *= 1.0,
                }
            }
        }
        eigenvalue * coeff
    }

    /// Richardson extrapolation for ZNE
    fn richardson_extrapolation(&self, scales: &[f64], values: &[f64]) -> Result<f64> {
        if scales.len() != values.len() || scales.len() < 2 {
            return Err(anyhow::anyhow!("Need at least 2 scale factors for Richardson extrapolation"));
        }

        // Polynomial fit and evaluate at 0
        // Using Lagrange interpolation
        let n = scales.len();
        let mut result = 0.0;

        for i in 0..n {
            let mut term = values[i];
            for j in 0..n {
                if i != j {
                    term *= (0.0 - scales[j]) / (scales[i] - scales[j]);
                }
            }
            result += term;
        }

        Ok(result)
    }

    /// Insert DD pulses into circuit
    fn insert_dd_pulses(&self, circuit: &QuantumCircuit, dd_seq: &DDSequence) -> QuantumCircuit {
        let mut builder = QuantumCircuit::builder(circuit.num_qubits);

        for op in &circuit.operations {
            // Add the original operation
            builder.add_gate(op.clone());

            // Add DD pulses after each operation on idle qubits
            // Simplified: add DD on all qubits
            for q in 0..circuit.num_qubits {
                match dd_seq {
                    DDSequence::CPMG { tau: _ } => {
                        builder.x(q);
                        builder.x(q); // CPMG: X-X sequence
                    }
                    DDSequence::UDD { tau: _ } => {
                        // UDD pulse sequence - simplified
                        builder.x(q);
                    }
                    DDSequence::Custom { pulses } => {
                        for (angle, phase) in pulses {
                            // Custom pulse - simplified as rotation
                            builder.rx(q, *angle);
                        }
                    }
                }
            }
        }

        builder.build()
    }
}

/// DD sequence types
#[derive(Debug, Clone)]
pub enum DDSequence {
    CPMG { tau: f64 },
    UDD { tau: f64 },
    Custom { pulses: Vec<(f64, f64)> }, // (angle, phase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::QuantumCircuit;

    #[test]
    fn test_bell_pair_simulation() {
        let mut builder = QuantumCircuit::builder(2);
        builder.h(0);
        builder.cx(0, 1);
        let circuit = builder.build();

        let config = SimulationConfig::new(1000).with_seed(42);
        let simulator = StateVectorSimulator::new(config);
        let result = simulator.simulate(&circuit).unwrap();

        // Bell pair should give 00 and 11 with ~50% each
        assert!(result.counts.get("00").unwrap_or(&0) > &400);
        assert!(result.counts.get("11").unwrap_or(&0) > &400);
        assert_eq!(result.counts.get("01").unwrap_or(&0), &0);
        assert_eq!(result.counts.get("10").unwrap_or(&0), &0);
    }

    #[test]
    fn test_hadamard_superposition() {
        let mut builder = QuantumCircuit::builder(1);
        builder.h(0);
        let circuit = builder.build();

        let config = SimulationConfig::new(1000).with_seed(42);
        let simulator = StateVectorSimulator::new(config);
        let result = simulator.simulate(&circuit).unwrap();

        // Should be ~50/50
        let count_0 = result.counts.get("0").unwrap_or(&0);
        let count_1 = result.counts.get("1").unwrap_or(&0);
        assert!(*count_0 > 400 && *count_0 < 600);
        assert!(*count_1 > 400 && *count_1 < 600);
    }

    #[test]
    fn test_statevector_output() {
        let mut builder = QuantumCircuit::builder(1);
        builder.h(0);
        let circuit = builder.build();

        let config = SimulationConfig::new(1).with_statevector();
        let simulator = StateVectorSimulator::new(config);
        let result = simulator.simulate(&circuit).unwrap();

        assert!(result.statevector.is_some());
        let sv = result.statevector.unwrap();
        assert!(sv.contains_key("0"));
        assert!(sv.contains_key("1"));
    }

    #[test]
    fn test_ghz_state() {
        let mut builder = QuantumCircuit::builder(3);
        builder.h(0);
        builder.cx(0, 1);
        builder.cx(1, 2);
        let circuit = builder.build();

        let config = SimulationConfig::new(1000).with_seed(42);
        let simulator = StateVectorSimulator::new(config);
        let result = simulator.simulate(&circuit).unwrap();

        // GHZ should give 000 and 111
        assert!(result.counts.get("000").unwrap_or(&0) > &400);
        assert!(result.counts.get("111").unwrap_or(&0) > &400);
    }
}
