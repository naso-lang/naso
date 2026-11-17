//! Quantum Circuit Representation
//!
//! Provides a hardware-agnostic circuit representation that can be
//! compiled to different backend instruction sets.

use crate::backend::{Observable, ShotResult};
use anyhow::Result;
use naso_gates::statevector::{Gate, StateVector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A quantum gate operation in a circuit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateOperation {
    /// Gate name (e.g., "H", "CX", "RZ", "RZZ")
    pub name: String,
    /// Qubits the gate acts on
    pub qubits: Vec<usize>,
    /// Parameters (angles, etc.)
    pub params: Vec<f64>,
    /// Condition for conditional execution (mid-circuit measurement)
    pub condition: Option<Condition>,
}

impl GateOperation {
    pub fn new(name: String, qubits: Vec<usize>) -> Self {
        Self {
            name,
            qubits,
            params: vec![],
            condition: None,
        }
    }

    pub fn with_params(mut self, params: Vec<f64>) -> Self {
        self.params = params;
        self
    }

    pub fn with_condition(mut self, condition: Condition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Convert to naso_gates::Gate for simulation
    pub fn to_naso_gate(&self) -> Option<Gate> {
        use naso_gates::statevector::Gate;
        let name = self.name.to_ascii_lowercase();
        let params = &self.params;

        Some(match name.as_str() {
            "h" | "hadamard" => Gate::H,
            "x" | "pauli_x" => Gate::X,
            "y" | "pauli_y" => Gate::Y,
            "z" | "pauli_z" => Gate::Z,
            "s" => Gate::S,
            "sdg" => Gate::Sdg,
            "t" => Gate::T,
            "tdg" => Gate::Tdg,
            "cx" | "cnot" => Gate::Cx,
            "cy" => Gate::Cy,
            "cz" => Gate::Cz,
            "ccx" | "toffoli" => Gate::Ccx,
            "swap" => Gate::Swap,
            "iswap" => Gate::Iswap,
            "cswap" => Gate::Cswap,
            "rx" if params.len() == 1 => Gate::Rx(params[0]),
            "ry" if params.len() == 1 => Gate::Ry(params[0]),
            "rz" if params.len() == 1 => Gate::Rz(params[0]),
            "crx" if params.len() == 1 => Gate::Crx(params[0]),
            "cry" if params.len() == 1 => Gate::Cry(params[0]),
            "crz" if params.len() == 1 => Gate::Crz(params[0]),
            "rxx" if params.len() == 1 => Gate::Rxx(params[0]),
            "ryy" if params.len() == 1 => Gate::Ryy(params[0]),
            "rzz" if params.len() == 1 => Gate::Rzz(params[0]),
            "cphase" if params.len() == 1 => Gate::Cphase(params[0]),
            _ => return None,
        })
    }
}

/// Condition for conditional execution based on classical bits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Classical register to check
    pub register: String,
    /// Value to compare against (as bitstring)
    pub value: String,
    /// Comparison operator
    pub op: ConditionOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConditionOp {
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessThanOrEqual,
    GreaterThanOrEqual,
}

/// Quantum circuit builder
#[derive(Debug, Clone, Default)]
pub struct CircuitBuilder {
    num_qubits: usize,
    num_classical_bits: usize,
    operations: Vec<GateOperation>,
    classical_registers: HashMap<String, usize>,
}

impl CircuitBuilder {
    pub fn new(num_qubits: usize) -> Self {
        Self {
            num_qubits,
            num_classical_bits: 0,
            operations: vec![],
            classical_registers: HashMap::new(),
        }
    }

    pub fn with_classical_bits(mut self, num: usize) -> Self {
        self.num_classical_bits = num;
        self
    }

    pub fn add_gate(&mut self, gate: GateOperation) -> &mut Self {
        self.operations.push(gate);
        self
    }

    pub fn h(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("H".to_string(), vec![qubit]));
        self
    }

    pub fn x(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("X".to_string(), vec![qubit]));
        self
    }

    pub fn y(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("Y".to_string(), vec![qubit]));
        self
    }

    pub fn z(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("Z".to_string(), vec![qubit]));
        self
    }

    pub fn s(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("S".to_string(), vec![qubit]));
        self
    }

    pub fn sdg(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("Sdg".to_string(), vec![qubit]));
        self
    }

    pub fn t(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("T".to_string(), vec![qubit]));
        self
    }

    pub fn tdg(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("Tdg".to_string(), vec![qubit]));
        self
    }

    pub fn cx(&mut self, control: usize, target: usize) -> &mut Self {
        self.add_gate(GateOperation::new("CX".to_string(), vec![control, target]));
        self
    }

    pub fn cy(&mut self, control: usize, target: usize) -> &mut Self {
        self.add_gate(GateOperation::new("CY".to_string(), vec![control, target]));
        self
    }

    pub fn cz(&mut self, control: usize, target: usize) -> &mut Self {
        self.add_gate(GateOperation::new("CZ".to_string(), vec![control, target]));
        self
    }

    pub fn ccx(&mut self, control1: usize, control2: usize, target: usize) -> &mut Self {
        self.add_gate(GateOperation::new("CCX".to_string(), vec![control1, control2, target]));
        self
    }

    pub fn swap(&mut self, q1: usize, q2: usize) -> &mut Self {
        self.add_gate(GateOperation::new("SWAP".to_string(), vec![q1, q2]));
        self
    }

    pub fn iswap(&mut self, q1: usize, q2: usize) -> &mut Self {
        self.add_gate(GateOperation::new("ISWAP".to_string(), vec![q1, q2]));
        self
    }

    pub fn rx(&mut self, qubit: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("RX".to_string(), vec![qubit]).with_params(vec![angle]));
        self
    }

    pub fn ry(&mut self, qubit: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("RY".to_string(), vec![qubit]).with_params(vec![angle]));
        self
    }

    pub fn rz(&mut self, qubit: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("RZ".to_string(), vec![qubit]).with_params(vec![angle]));
        self
    }

    pub fn crx(&mut self, control: usize, target: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("CRX".to_string(), vec![control, target]).with_params(vec![angle]));
        self
    }

    pub fn cry(&mut self, control: usize, target: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("CRY".to_string(), vec![control, target]).with_params(vec![angle]));
        self
    }

    pub fn crz(&mut self, control: usize, target: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("CRZ".to_string(), vec![control, target]).with_params(vec![angle]));
        self
    }

    pub fn rxx(&mut self, q1: usize, q2: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("RXX".to_string(), vec![q1, q2]).with_params(vec![angle]));
        self
    }

    pub fn ryy(&mut self, q1: usize, q2: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("RYY".to_string(), vec![q1, q2]).with_params(vec![angle]));
        self
    }

    pub fn rzz(&mut self, q1: usize, q2: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("RZZ".to_string(), vec![q1, q2]).with_params(vec![angle]));
        self
    }

    pub fn cphase(&mut self, control: usize, target: usize, angle: f64) -> &mut Self {
        self.add_gate(GateOperation::new("CPHASE".to_string(), vec![control, target]).with_params(vec![angle]));
        self
    }

    pub fn cswap(&mut self, control: usize, target1: usize, target2: usize) -> &mut Self {
        self.add_gate(GateOperation::new("CSWAP".to_string(), vec![control, target1, target2]));
        self
    }

    pub fn measure(&mut self, qubit: usize, clbit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("MEASURE".to_string(), vec![qubit, clbit]));
        self
    }

    pub fn reset(&mut self, qubit: usize) -> &mut Self {
        self.add_gate(GateOperation::new("RESET".to_string(), vec![qubit]));
        self
    }

    pub fn build(self) -> QuantumCircuit {
        QuantumCircuit {
            num_qubits: self.num_qubits,
            num_classical_bits: self.num_classical_bits,
            operations: self.operations,
            classical_registers: self.classical_registers,
        }
    }
}

/// A quantum circuit - sequence of gate operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantumCircuit {
    pub num_qubits: usize,
    pub num_classical_bits: usize,
    pub operations: Vec<GateOperation>,
    pub classical_registers: HashMap<String, usize>,
}

impl QuantumCircuit {
    pub fn new(num_qubits: usize) -> Self {
        Self {
            num_qubits,
            num_classical_bits: 0,
            operations: vec![],
            classical_registers: HashMap::new(),
        }
    }

    pub fn builder(num_qubits: usize) -> CircuitBuilder {
        CircuitBuilder::new(num_qubits)
    }

    pub fn num_qubits(&self) -> usize {
        self.num_qubits
    }

    pub fn num_classical_bits(&self) -> usize {
        self.num_classical_bits
    }

    pub fn operations(&self) -> &[GateOperation] {
        &self.operations
    }

    pub fn depth(&self) -> usize {
        // Simplified depth calculation - count layers
        let mut qubit_last_layer = vec![0; self.num_qubits];
        let mut max_depth = 0;

        for op in &self.operations {
            let mut layer = 0;
            for &q in &op.qubits {
                layer = layer.max(qubit_last_layer[q]);
            }
            for &q in &op.qubits {
                qubit_last_layer[q] = layer + 1;
            }
            max_depth = max_depth.max(layer + 1);
        }
        max_depth
    }

    pub fn gate_count(&self) -> usize {
        self.operations.len()
    }

    pub fn count_gate(&self, name: &str) -> usize {
        self.operations.iter().filter(|op| op.name.to_ascii_lowercase() == name.to_ascii_lowercase()).count()
    }

    /// Simulate this circuit using the state vector simulator
    pub fn simulate(&self, config: &crate::simulator::SimulationConfig) -> Result<crate::execution::ExecutionResult> {
        let simulator = crate::simulator::StateVectorSimulator::new(config.clone());
        simulator.simulate(self)
    }

    /// Convert to QASM string
    pub fn to_qasm(&self) -> String {
        let mut qasm = String::new();
        qasm.push_str("OPENQASM 2.0;\n");
        qasm.push_str("include \"qelib1.inc\";\n");
        qasm.push_str(&format!("qreg q[{}];\n", self.num_qubits));
        if self.num_classical_bits > 0 {
            qasm.push_str(&format!("creg c[{}];\n", self.num_classical_bits));
        }

        for op in &self.operations {
            let name = op.name.to_ascii_lowercase();
            let qubits = op.qubits.iter().map(|q| format!("q[{}]", q)).collect::<Vec<_>>().join(", ");
            
            if !op.params.is_empty() {
                let params = op.params.iter().map(|p| format!("{:.10}", p)).collect::<Vec<_>>().join(", ");
                qasm.push_str(&format!("{}({}) {};\n", name, params, qubits));
            } else {
                qasm.push_str(&format!("{} {};\n", name, qubits));
            }
        }
        qasm
    }

    /// Convert to QIR-like representation
    pub fn to_qir(&self) -> String {
        let mut qir = String::new();
        qir.push_str(&format!("declare void @qir_entry() {{\n"));
        qir.push_str("entry:\n");
        
        for op in &self.operations {
            let name = op.name.to_ascii_lowercase();
            let qubits = op.qubits.iter().map(|q| format!("%q{}", q)).collect::<Vec<_>>().join(", ");
            
            if !op.params.is_empty() {
                let params = op.params.iter().map(|p| format!("{}", p)).collect::<Vec<_>>().join(", ");
                qir.push_str(&format!("  call void @qir.{}({}, {})\n", name, params, qubits));
            } else {
                qir.push_str(&format!("  call void @qir.{}({})\n", name, qubits));
            }
        }
        
        qir.push_str("  ret void\n");
        qir.push_str("}\n");
        qir
    }
}

impl From<CircuitBuilder> for QuantumCircuit {
    fn from(builder: CircuitBuilder) -> Self {
        builder.build()
    }
}