#![allow(clippy::clone_on_copy)]
#![allow(clippy::empty_line_after_doc_comments)]

/// Quantum Statevector Simulator
///
/// In-process quantum statevector simulator for executing QIR circuits.
/// Features:
/// - Statevector representation using complex-valued arrays
/// - Quantum gate application (single-qubit, two-qubit) with automatic uncomputation
/// - Measurement sampling with probability calculation
/// - Integration with QIR module execution via runtime dispatcher
/// - Support for [0], [1], [*] quantity semantics
use num_complex::Complex64;
use rand::RngCore;
use rand::SeedableRng;
use thiserror::Error;

use crate::ast::Quantity;

/// Configuration for the statevector simulator
#[derive(Debug, Clone)]
pub struct SimulatorConfig {
    /// Maximum number of qubits to simulate
    pub max_qubits: usize,
    /// Random seed for reproducible measurements
    pub seed: Option<u64>,
    /// Enable automatic uncomputation of temporary values
    pub auto_uncompute: bool,
    /// Validation mode: verify unitarity after each gate
    pub validate_unitarity: bool,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            max_qubits: 30,
            seed: None,
            auto_uncompute: true,
            validate_unitarity: false,
        }
    }
}

/// Measurement result with probability
#[derive(Debug, Clone)]
pub struct MeasurementResult {
    /// Qubit index that was measured
    pub qubit: usize,
    /// Measured value (0 or 1)
    pub value: u8,
    /// Probability of this outcome
    pub probability: f64,
    /// Quantity of the measured qubit
    pub quantity: Quantity,
}

/// Statevector simulator error types
#[derive(Debug, Error)]
pub enum SimulatorError {
    #[error("Qubit index out of bounds: {0} >= {1}")]
    QubitOutOfBounds(usize, usize),
    #[error("Invalid gate operation: {0}")]
    InvalidGate(String),
    #[error("Statevector dimension mismatch: expected {0}, got {1}")]
    DimensionMismatch(usize, usize),
    #[error("Non-unitary operation detected: {0}")]
    NonUnitaryOperation(String),
    #[error("Quantity violation: {0}")]
    QuantityViolation(String),
    #[error("Measurement of erased qubit [0]: {0}")]
    ErasedQubitMeasured(usize),
    #[error("Double consumption of linear qubit [1]: {0}")]
    LinearQubitDoubleConsumed(usize),
    #[error("Insufficient qubits: need {0}, have {1}")]
    InsufficientQubits(usize, usize),
}

/// Quantum statevector simulator
pub struct StatevectorSimulator {
    /// Statevector amplitudes (2^n complex numbers for n qubits)
    statevector: Vec<Complex64>,
    /// Number of qubits
    num_qubits: usize,
    /// Configuration
    config: SimulatorConfig,
    /// Tracking of qubit quantities for [0], [1], [*] semantics
    qubit_quantities: Vec<Quantity>,
    /// Tracking of qubit consumption for [1] linearity
    qubit_consumed: Vec<bool>,
    /// Random number generator
    rng: Box<dyn RngCore + Send>,
    /// Execution statistics
    stats: SimulatorStats,
}

/// Statistics for simulator execution
#[derive(Debug, Clone, Default)]
pub struct SimulatorStats {
    pub gate_count: usize,
    pub measurement_count: usize,
    pub uncomputation_count: usize,
    pub peak_memory_bytes: usize,
}

impl StatevectorSimulator {
    /// Create a new simulator with given number of qubits
    pub fn new(num_qubits: usize, config: SimulatorConfig) -> Result<Self, SimulatorError> {
        if num_qubits > config.max_qubits {
            return Err(SimulatorError::InsufficientQubits(
                num_qubits,
                config.max_qubits,
            ));
        }

        let dim = 1usize << num_qubits;
        let mut statevector = vec![Complex64::new(0.0, 0.0); dim];
        statevector[0] = Complex64::new(1.0, 0.0); // |0...0⟩

        let rng: Box<dyn RngCore + Send> = if let Some(seed) = config.seed {
            Box::new(rand::rngs::StdRng::seed_from_u64(seed))
        } else {
            Box::new(rand::rngs::OsRng)
        };

        Ok(Self {
            statevector,
            num_qubits,
            config,
            qubit_quantities: vec![Quantity::Many; num_qubits],
            qubit_consumed: vec![false; num_qubits],
            rng,
            stats: SimulatorStats::default(),
        })
    }

    /// Create simulator from QIR module (extracts qubit count)
    #[cfg(feature = "llvm")]
    pub fn from_qir_module(
        module: &crate::codegen::qir::QIRModule,
        config: SimulatorConfig,
    ) -> Result<Self, SimulatorError> {
        // Extract qubit count from QIR module
        let num_qubits = module.qubit_count().min(config.max_qubits);
        let mut sim = Self::new(num_qubits, config)?;

        // Initialize qubit quantities from QIR metadata
        for (idx, qty) in module.qubit_quantities().iter().enumerate() {
            if idx < num_qubits {
                sim.qubit_quantities[idx] = qty.clone();
            }
        }

        Ok(sim)
    }

    #[cfg(not(feature = "llvm"))]
    pub fn from_qir_module(_module: &(), _config: SimulatorConfig) -> Result<Self, SimulatorError> {
        Err(SimulatorError::InvalidGate(
            "QIR module requires LLVM feature".into(),
        ))
    }

    /// Get current statevector (for debugging/validation)
    pub fn statevector(&self) -> &[Complex64] {
        &self.statevector
    }

    /// Get number of qubits
    pub fn num_qubits(&self) -> usize {
        self.num_qubits
    }

    /// Set quantity for a qubit
    pub fn set_qubit_quantity(
        &mut self,
        qubit: usize,
        quantity: Quantity,
    ) -> Result<(), SimulatorError> {
        if qubit >= self.num_qubits {
            return Err(SimulatorError::QubitOutOfBounds(qubit, self.num_qubits));
        }
        self.qubit_quantities[qubit] = quantity;
        Ok(())
    }

    /// Get quantity for a qubit
    pub fn qubit_quantity(&self, qubit: usize) -> Quantity {
        if qubit >= self.num_qubits {
            return Quantity::Many;
        }
        self.qubit_quantities[qubit].clone()
    }

    /// Apply a single-qubit gate
    pub fn apply_single_qubit_gate(
        &mut self,
        gate: &SingleQubitGate,
        target: usize,
    ) -> Result<(), SimulatorError> {
        if target >= self.num_qubits {
            return Err(SimulatorError::QubitOutOfBounds(target, self.num_qubits));
        }

        // Check quantity constraints
        if self.qubit_consumed[target] && self.qubit_quantities[target] == Quantity::One {
            return Err(SimulatorError::LinearQubitDoubleConsumed(target));
        }

        let matrix = gate.matrix();
        self.apply_single_qubit_matrix(&matrix, target)?;

        self.stats.gate_count += 1;
        self.update_peak_memory();

        Ok(())
    }

    /// Apply a two-qubit gate
    pub fn apply_two_qubit_gate(
        &mut self,
        gate: &TwoQubitGate,
        control: usize,
        target: usize,
    ) -> Result<(), SimulatorError> {
        if control >= self.num_qubits || target >= self.num_qubits {
            return Err(SimulatorError::QubitOutOfBounds(
                control.max(target),
                self.num_qubits,
            ));
        }
        if control == target {
            return Err(SimulatorError::InvalidGate(
                "Control and target qubits must be different".into(),
            ));
        }

        // Check quantity constraints
        if self.qubit_consumed[control] && self.qubit_quantities[control].is_linear() {
            return Err(SimulatorError::LinearQubitDoubleConsumed(control));
        }
        if self.qubit_consumed[target] && self.qubit_quantities[target].is_linear() {
            return Err(SimulatorError::LinearQubitDoubleConsumed(target));
        }

        let matrix = gate.matrix();
        self.apply_two_qubit_matrix(&matrix, control, target)?;

        self.stats.gate_count += 1;
        self.update_peak_memory();

        Ok(())
    }

    /// Apply a controlled gate (control qubits, target gate)
    pub fn apply_controlled_gate(
        &mut self,
        controls: &[usize],
        gate: &SingleQubitGate,
        target: usize,
    ) -> Result<(), SimulatorError> {
        for &c in controls {
            if c >= self.num_qubits {
                return Err(SimulatorError::QubitOutOfBounds(c, self.num_qubits));
            }
        }
        if target >= self.num_qubits {
            return Err(SimulatorError::QubitOutOfBounds(target, self.num_qubits));
        }
        if controls.contains(&target) {
            return Err(SimulatorError::InvalidGate(
                "Target qubit cannot be a control".into(),
            ));
        }

        let matrix = gate.matrix();
        self.apply_controlled_matrix(controls, &matrix, target)?;

        self.stats.gate_count += 1;
        self.update_peak_memory();

        Ok(())
    }

    /// Measure a qubit in computational basis
    pub fn measure(&mut self, qubit: usize) -> Result<MeasurementResult, SimulatorError> {
        if qubit >= self.num_qubits {
            return Err(SimulatorError::QubitOutOfBounds(qubit, self.num_qubits));
        }

        let quantity = self.qubit_quantities[qubit].clone();

        // [0] qubits cannot be measured (they're erased)
        if quantity == Quantity::Zero {
            return Err(SimulatorError::ErasedQubitMeasured(qubit));
        }

        // [1] qubits must not be consumed already
        if quantity == Quantity::One && self.qubit_consumed[qubit] {
            return Err(SimulatorError::LinearQubitDoubleConsumed(qubit));
        }

        // Calculate probabilities
        let prob_0 = self.probability_of_zero(qubit);
        let prob_1 = 1.0 - prob_0;

        // Sample measurement outcome
        let random_val = self.rng.next_u64() as f64 / u64::MAX as f64;
        let value = if random_val < prob_0 { 0 } else { 1 };
        let probability = if value == 0 { prob_0 } else { prob_1 };

        // Collapse statevector
        self.collapse_statevector(qubit, value)?;

        // Mark [1] qubit as consumed
        if quantity.is_linear() {
            self.qubit_consumed[qubit] = true;
        }

        self.stats.measurement_count += 1;

        Ok(MeasurementResult {
            qubit,
            value,
            probability,
            quantity,
        })
    }

    /// Measure multiple qubits
    pub fn measure_all(
        &mut self,
        qubits: &[usize],
    ) -> Result<Vec<MeasurementResult>, SimulatorError> {
        let mut results = Vec::with_capacity(qubits.len());
        for &q in qubits {
            results.push(self.measure(q)?);
        }
        Ok(results)
    }

    /// Automatic uncomputation: apply inverse of a sequence of operations
    pub fn uncompute(&mut self, operations: &[QuantumOperation]) -> Result<(), SimulatorError> {
        if !self.config.auto_uncompute {
            return Ok(());
        }

        // Apply operations in reverse order with adjoint
        for op in operations.iter().rev() {
            match op.adjoint() {
                Some(adj_op) => self.apply_operation(&adj_op)?,
                None => {
                    return Err(SimulatorError::InvalidGate(format!(
                        "Operation {:?} has no adjoint",
                        op
                    )));
                }
            }
        }

        self.stats.uncomputation_count += 1;
        Ok(())
    }

    /// Apply a quantum operation
    pub fn apply_operation(&mut self, op: &QuantumOperation) -> Result<(), SimulatorError> {
        match op {
            QuantumOperation::SingleQubit { gate, target } => {
                self.apply_single_qubit_gate(gate, *target)
            }
            QuantumOperation::TwoQubit {
                gate,
                control,
                target,
            } => self.apply_two_qubit_gate(gate, *control, *target),
            QuantumOperation::Controlled {
                controls,
                gate,
                target,
            } => self.apply_controlled_gate(controls, gate, *target),
            QuantumOperation::Measure { qubit } => self.measure(*qubit).map(|_| ()),
        }
    }

    /// Execute a sequence of quantum operations
    pub fn execute_circuit(
        &mut self,
        operations: &[QuantumOperation],
    ) -> Result<Vec<MeasurementResult>, SimulatorError> {
        let mut measurements = Vec::new();

        for op in operations {
            match op {
                QuantumOperation::Measure { qubit } => {
                    measurements.push(self.measure(*qubit)?);
                }
                _ => {
                    self.apply_operation(op)?;
                }
            }
        }

        Ok(measurements)
    }

    /// Get execution statistics
    pub fn stats(&self) -> &SimulatorStats {
        &self.stats
    }

    /// Reset simulator to |0...0⟩ state
    pub fn reset(&mut self) {
        self.statevector.fill(Complex64::new(0.0, 0.0));
        self.statevector[0] = Complex64::new(1.0, 0.0);
        self.qubit_consumed.fill(false);
        self.stats = SimulatorStats::default();
    }

    // Internal methods

    fn apply_single_qubit_matrix(
        &mut self,
        matrix: &[[Complex64; 2]; 2],
        target: usize,
    ) -> Result<(), SimulatorError> {
        let dim = 1usize << self.num_qubits;
        let stride = 1usize << target;
        let block = 1usize << (target + 1);

        let mut new_state = vec![Complex64::new(0.0, 0.0); dim];

        for base in (0..dim).step_by(block) {
            for offset in 0..stride {
                let idx0 = base + offset;
                let idx1 = idx0 + stride;

                let a = self.statevector[idx0];
                let b = self.statevector[idx1];

                new_state[idx0] = matrix[0][0] * a + matrix[0][1] * b;
                new_state[idx1] = matrix[1][0] * a + matrix[1][1] * b;
            }
        }

        // Validate unitarity if enabled
        if self.config.validate_unitarity {
            self.validate_statevector(&new_state)?;
        }

        self.statevector = new_state;
        Ok(())
    }

    fn apply_two_qubit_matrix(
        &mut self,
        matrix: &[[Complex64; 4]; 4],
        control: usize,
        target: usize,
    ) -> Result<(), SimulatorError> {
        let (c, t) = if control < target {
            (control, target)
        } else {
            (target, control)
        };
        let dim = 1usize << self.num_qubits;
        let stride_c = 1usize << c;
        let stride_t = 1usize << t;
        let block = 1usize << (t + 1);

        let mut new_state = vec![Complex64::new(0.0, 0.0); dim];

        for base in (0..dim).step_by(block) {
            for offset in 0..stride_c {
                for inner in 0..stride_t {
                    let idx = base + offset + inner;
                    let idx00 = idx;
                    let idx01 = idx + stride_c;
                    let idx10 = idx + stride_t;
                    let idx11 = idx + stride_t + stride_c;

                    let a00 = self.statevector[idx00];
                    let a01 = self.statevector[idx01];
                    let a10 = self.statevector[idx10];
                    let a11 = self.statevector[idx11];

                    new_state[idx00] = matrix[0][0] * a00
                        + matrix[0][1] * a01
                        + matrix[0][2] * a10
                        + matrix[0][3] * a11;
                    new_state[idx01] = matrix[1][0] * a00
                        + matrix[1][1] * a01
                        + matrix[1][2] * a10
                        + matrix[1][3] * a11;
                    new_state[idx10] = matrix[2][0] * a00
                        + matrix[2][1] * a01
                        + matrix[2][2] * a10
                        + matrix[2][3] * a11;
                    new_state[idx11] = matrix[3][0] * a00
                        + matrix[3][1] * a01
                        + matrix[3][2] * a10
                        + matrix[3][3] * a11;
                }
            }
        }

        if self.config.validate_unitarity {
            self.validate_statevector(&new_state)?;
        }

        self.statevector = new_state;
        Ok(())
    }

    fn apply_controlled_matrix(
        &mut self,
        controls: &[usize],
        matrix: &[[Complex64; 2]; 2],
        target: usize,
    ) -> Result<(), SimulatorError> {
        // For simplicity, apply as multi-controlled single-qubit gate
        // In production, this would use more efficient decomposition
        let dim = 1usize << self.num_qubits;

        // Build control mask
        let mut control_mask = 0usize;
        for &c in controls {
            control_mask |= 1usize << c;
        }

        let stride_t = 1usize << target;
        let mut new_state = self.statevector.clone();

        for idx in 0..dim {
            // Check if all control qubits are |1⟩
            let all_controls_one = (idx & control_mask) == control_mask;
            if !all_controls_one {
                continue;
            }

            let idx0 = idx & !stride_t;
            let idx1 = idx0 | stride_t;

            let a = self.statevector[idx0];
            let b = self.statevector[idx1];

            new_state[idx0] = matrix[0][0] * a + matrix[0][1] * b;
            new_state[idx1] = matrix[1][0] * a + matrix[1][1] * b;
        }

        if self.config.validate_unitarity {
            self.validate_statevector(&new_state)?;
        }

        self.statevector = new_state;
        Ok(())
    }

    fn probability_of_zero(&self, qubit: usize) -> f64 {
        let stride = 1usize << qubit;
        let block = 1usize << (qubit + 1);
        let dim = 1usize << self.num_qubits;

        let mut prob = 0.0;
        for base in (0..dim).step_by(block) {
            for offset in 0..stride {
                let idx = base + offset;
                let amp = self.statevector[idx];
                prob += amp.norm_sqr();
            }
        }
        prob
    }

    fn collapse_statevector(&mut self, qubit: usize, value: u8) -> Result<(), SimulatorError> {
        let stride = 1usize << qubit;
        let block = 1usize << (qubit + 1);
        let dim = 1usize << self.num_qubits;

        let prob = if value == 0 {
            self.probability_of_zero(qubit)
        } else {
            1.0 - self.probability_of_zero(qubit)
        };

        if prob < 1e-15 {
            return Err(SimulatorError::InvalidGate(
                "Measurement probability is zero".into(),
            ));
        }

        let norm_factor = 1.0 / prob.sqrt();

        for base in (0..dim).step_by(block) {
            for offset in 0..stride {
                let idx0 = base + offset;
                let idx1 = idx0 + stride;

                if value == 0 {
                    self.statevector[idx0] *= norm_factor;
                    self.statevector[idx1] = Complex64::new(0.0, 0.0);
                } else {
                    self.statevector[idx0] = Complex64::new(0.0, 0.0);
                    self.statevector[idx1] *= norm_factor;
                }
            }
        }

        Ok(())
    }

    fn validate_statevector(&self, state: &[Complex64]) -> Result<(), SimulatorError> {
        let norm_sqr: f64 = state.iter().map(|c| c.norm_sqr()).sum();
        if (norm_sqr - 1.0).abs() > 1e-10 {
            return Err(SimulatorError::NonUnitaryOperation(format!(
                "Statevector norm squared = {}, expected 1.0",
                norm_sqr
            )));
        }
        Ok(())
    }

    fn update_peak_memory(&mut self) {
        let current_bytes = self.statevector.len() * std::mem::size_of::<Complex64>();
        if current_bytes > self.stats.peak_memory_bytes {
            self.stats.peak_memory_bytes = current_bytes;
        }
    }
}

/// Single-qubit gates
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SingleQubitGate {
    /// Pauli-X (NOT)
    X,
    /// Pauli-Y
    Y,
    /// Pauli-Z
    Z,
    /// Hadamard
    H,
    /// S gate (sqrt(Z))
    S,
    /// S† gate
    Sdg,
    /// T gate (sqrt(S))
    T,
    /// T† gate
    Tdg,
    /// Phase gate (Rz)
    Phase(f64),
    /// Rotation around X
    Rx(f64),
    /// Rotation around Y
    Ry(f64),
    /// Rotation around Z
    Rz(f64),
    /// U3 gate (general single-qubit)
    U3(f64, f64, f64),
}

impl SingleQubitGate {
    /// Get the 2x2 unitary matrix for this gate
    pub fn matrix(&self) -> [[Complex64; 2]; 2] {
        use std::f64::consts::PI;

        match self {
            SingleQubitGate::X => [
                [Complex64::new(0.0, 0.0), Complex64::new(1.0, 0.0)],
                [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
            ],
            SingleQubitGate::Y => [
                [Complex64::new(0.0, 0.0), Complex64::new(0.0, -1.0)],
                [Complex64::new(0.0, 1.0), Complex64::new(0.0, 0.0)],
            ],
            SingleQubitGate::Z => [
                [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
                [Complex64::new(0.0, 0.0), Complex64::new(-1.0, 0.0)],
            ],
            SingleQubitGate::H => {
                let inv_sqrt2 = 1.0 / 2.0_f64.sqrt();
                [
                    [
                        Complex64::new(inv_sqrt2, 0.0),
                        Complex64::new(inv_sqrt2, 0.0),
                    ],
                    [
                        Complex64::new(inv_sqrt2, 0.0),
                        Complex64::new(-inv_sqrt2, 0.0),
                    ],
                ]
            }
            SingleQubitGate::S => [
                [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
                [Complex64::new(0.0, 0.0), Complex64::new(0.0, 1.0)],
            ],
            SingleQubitGate::Sdg => [
                [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
                [Complex64::new(0.0, 0.0), Complex64::new(0.0, -1.0)],
            ],
            SingleQubitGate::T => {
                let angle = PI / 4.0;
                [
                    [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
                    [
                        Complex64::new(0.0, 0.0),
                        Complex64::new(angle.cos(), angle.sin()),
                    ],
                ]
            }
            SingleQubitGate::Tdg => {
                let angle = -PI / 4.0;
                [
                    [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
                    [
                        Complex64::new(0.0, 0.0),
                        Complex64::new(angle.cos(), angle.sin()),
                    ],
                ]
            }
            SingleQubitGate::Phase(theta) => {
                let half = theta / 2.0;
                [
                    [Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0)],
                    [
                        Complex64::new(0.0, 0.0),
                        Complex64::new(half.cos(), half.sin()),
                    ],
                ]
            }
            SingleQubitGate::Rx(theta) => {
                let half = theta / 2.0;
                let cos = half.cos();
                let sin = half.sin();
                [
                    [Complex64::new(cos, 0.0), Complex64::new(0.0, -sin)],
                    [Complex64::new(0.0, -sin), Complex64::new(cos, 0.0)],
                ]
            }
            SingleQubitGate::Ry(theta) => {
                let half = theta / 2.0;
                let cos = half.cos();
                let sin = half.sin();
                [
                    [Complex64::new(cos, 0.0), Complex64::new(-sin, 0.0)],
                    [Complex64::new(sin, 0.0), Complex64::new(cos, 0.0)],
                ]
            }
            SingleQubitGate::Rz(theta) => {
                let half = theta / 2.0;
                [
                    [
                        Complex64::new(half.cos(), half.sin()),
                        Complex64::new(0.0, 0.0),
                    ],
                    [
                        Complex64::new(0.0, 0.0),
                        Complex64::new(half.cos(), -half.sin()),
                    ],
                ]
            }
            SingleQubitGate::U3(theta, phi, lambda) => {
                let half = theta / 2.0;
                let cos = half.cos();
                let sin = half.sin();
                [
                    [
                        Complex64::new(cos, 0.0),
                        Complex64::new(-sin * phi.cos(), -sin * phi.sin()),
                    ],
                    [
                        Complex64::new(sin * lambda.cos(), sin * lambda.sin()),
                        Complex64::new(cos * (phi + lambda).cos(), cos * (phi + lambda).sin()),
                    ],
                ]
            }
        }
    }

    /// Get the adjoint (inverse) of this gate
    pub fn adjoint(&self) -> SingleQubitGate {
        match self {
            SingleQubitGate::X => SingleQubitGate::X,
            SingleQubitGate::Y => SingleQubitGate::Y,
            SingleQubitGate::Z => SingleQubitGate::Z,
            SingleQubitGate::H => SingleQubitGate::H,
            SingleQubitGate::S => SingleQubitGate::Sdg,
            SingleQubitGate::Sdg => SingleQubitGate::S,
            SingleQubitGate::T => SingleQubitGate::Tdg,
            SingleQubitGate::Tdg => SingleQubitGate::T,
            SingleQubitGate::Phase(theta) => SingleQubitGate::Phase(-theta),
            SingleQubitGate::Rx(theta) => SingleQubitGate::Rx(-theta),
            SingleQubitGate::Ry(theta) => SingleQubitGate::Ry(-theta),
            SingleQubitGate::Rz(theta) => SingleQubitGate::Rz(-theta),
            SingleQubitGate::U3(theta, phi, lambda) => SingleQubitGate::U3(-theta, -lambda, -phi),
        }
    }
}

/// Two-qubit gates
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TwoQubitGate {
    /// CNOT (controlled-X)
    CX,
    /// Controlled-Y
    CY,
    /// Controlled-Z
    CZ,
    /// SWAP
    SWAP,
    /// iSWAP
    ISWAP,
    /// Controlled-Phase
    CPHASE(f64),
    /// Controlled-Rx
    CRx(f64),
    /// Controlled-Ry
    CRy(f64),
    /// Controlled-Rz
    CRz(f64),
}

impl TwoQubitGate {
    /// Get the 4x4 unitary matrix for this gate
    pub fn matrix(&self) -> [[Complex64; 4]; 4] {
        let i = Complex64::new(0.0, 1.0);
        let zero = Complex64::new(0.0, 0.0);
        let one = Complex64::new(1.0, 0.0);

        match self {
            TwoQubitGate::CX => [
                [one, zero, zero, zero],
                [zero, one, zero, zero],
                [zero, zero, zero, one],
                [zero, zero, one, zero],
            ],
            TwoQubitGate::CY => [
                [one, zero, zero, zero],
                [zero, one, zero, zero],
                [zero, zero, zero, -i],
                [zero, zero, i, zero],
            ],
            TwoQubitGate::CZ => [
                [one, zero, zero, zero],
                [zero, one, zero, zero],
                [zero, zero, one, zero],
                [zero, zero, zero, -one],
            ],
            TwoQubitGate::SWAP => [
                [one, zero, zero, zero],
                [zero, zero, one, zero],
                [zero, one, zero, zero],
                [zero, zero, zero, one],
            ],
            TwoQubitGate::ISWAP => [
                [one, zero, zero, zero],
                [zero, zero, i, zero],
                [zero, i, zero, zero],
                [zero, zero, zero, one],
            ],
            TwoQubitGate::CPHASE(theta) => {
                let half = theta / 2.0;
                let phase = Complex64::new(half.cos(), half.sin());
                [
                    [one, zero, zero, zero],
                    [zero, one, zero, zero],
                    [zero, zero, one, zero],
                    [zero, zero, zero, phase],
                ]
            }
            TwoQubitGate::CRx(theta) => {
                let half = theta / 2.0;
                let cos = half.cos();
                let sin = half.sin();
                [
                    [one, zero, zero, zero],
                    [zero, one, zero, zero],
                    [
                        zero,
                        zero,
                        Complex64::new(cos, 0.0),
                        Complex64::new(0.0, -sin),
                    ],
                    [
                        zero,
                        zero,
                        Complex64::new(0.0, -sin),
                        Complex64::new(cos, 0.0),
                    ],
                ]
            }
            TwoQubitGate::CRy(theta) => {
                let half = theta / 2.0;
                let cos = half.cos();
                let sin = half.sin();
                [
                    [one, zero, zero, zero],
                    [zero, one, zero, zero],
                    [
                        zero,
                        zero,
                        Complex64::new(cos, 0.0),
                        Complex64::new(-sin, 0.0),
                    ],
                    [
                        zero,
                        zero,
                        Complex64::new(sin, 0.0),
                        Complex64::new(cos, 0.0),
                    ],
                ]
            }
            TwoQubitGate::CRz(theta) => {
                let half = theta / 2.0;
                let phase = Complex64::new(half.cos(), half.sin());
                let phase_conj = Complex64::new(half.cos(), -half.sin());
                [
                    [one, zero, zero, zero],
                    [zero, one, zero, zero],
                    [zero, zero, phase, zero],
                    [zero, zero, zero, phase_conj],
                ]
            }
        }
    }

    /// Get the adjoint of this gate
    pub fn adjoint(&self) -> TwoQubitGate {
        match self {
            TwoQubitGate::CX => TwoQubitGate::CX,
            TwoQubitGate::CY => TwoQubitGate::CY,
            TwoQubitGate::CZ => TwoQubitGate::CZ,
            TwoQubitGate::SWAP => TwoQubitGate::SWAP,
            TwoQubitGate::ISWAP => TwoQubitGate::ISWAP,
            TwoQubitGate::CPHASE(theta) => TwoQubitGate::CPHASE(-theta),
            TwoQubitGate::CRx(theta) => TwoQubitGate::CRx(-theta),
            TwoQubitGate::CRy(theta) => TwoQubitGate::CRy(-theta),
            TwoQubitGate::CRz(theta) => TwoQubitGate::CRz(-theta),
        }
    }
}

/// Quantum operations for circuit execution
#[derive(Debug, Clone)]
pub enum QuantumOperation {
    SingleQubit {
        gate: SingleQubitGate,
        target: usize,
    },
    TwoQubit {
        gate: TwoQubitGate,
        control: usize,
        target: usize,
    },
    Controlled {
        controls: Vec<usize>,
        gate: SingleQubitGate,
        target: usize,
    },
    Measure {
        qubit: usize,
    },
}

impl QuantumOperation {
    /// Get the adjoint (inverse) of this operation
    pub fn adjoint(&self) -> Option<QuantumOperation> {
        match self {
            QuantumOperation::SingleQubit { gate, target } => Some(QuantumOperation::SingleQubit {
                gate: gate.adjoint(),
                target: *target,
            }),
            QuantumOperation::TwoQubit {
                gate,
                control,
                target,
            } => Some(QuantumOperation::TwoQubit {
                gate: gate.adjoint(),
                control: *control,
                target: *target,
            }),
            QuantumOperation::Controlled {
                controls,
                gate,
                target,
            } => Some(QuantumOperation::Controlled {
                controls: controls.clone(),
                gate: gate.adjoint(),
                target: *target,
            }),
            QuantumOperation::Measure { .. } => None, // Measurement has no adjoint
        }
    }
}
