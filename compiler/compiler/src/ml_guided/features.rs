//! Feature extraction for ML-guided optimization
//!
//! This module provides feature extraction utilities for various ML models
//! used in the compiler optimization pipeline.

use crate::ir::pir_types::{PirFunction, PirInstruction, PirType};
use crate::ir::schedule_tree::ScheduleNode;
use ndarray::Array1;
use serde::{Deserialize, Serialize};

/// Features for instruction selection cost model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionFeatures {
    /// Opcode encoded as integer
    pub opcode: u32,
    /// Number of operands
    pub num_operands: u32,
    /// Operand types (encoded)
    pub operand_types: Vec<u32>,
    /// Result type (encoded)
    pub result_type: u32,
    /// Whether instruction has side effects
    pub has_side_effects: bool,
    /// Whether instruction is a memory operation
    pub is_memory_op: bool,
    /// Whether instruction is a control flow operation
    pub is_control_flow: bool,
    /// Whether instruction is a vector operation
    pub is_vector_op: bool,
    /// Estimated latency (cycles)
    pub estimated_latency: f32,
    /// Estimated throughput (ops/cycle)
    pub estimated_throughput: f32,
    /// Register pressure contribution
    pub register_pressure: f32,
}

impl InstructionFeatures {
    /// Extract features from a PIR instruction
    pub fn from_instruction(inst: &PirInstruction) -> Self {
        Self {
            opcode: inst.opcode as u32,
            num_operands: inst.operands.len() as u32,
            operand_types: inst.operands.iter().map(|o| o.ty as u32).collect(),
            result_type: inst.result_type as u32,
            has_side_effects: inst.has_side_effects,
            is_memory_op: matches!(inst.opcode, PirOpcode::Load | PirOpcode::Store),
            is_control_flow: matches!(inst.opcode, PirOpcode::Branch | PirOpcode::Call | PirOpcode::Return),
            is_vector_op: matches!(inst.opcode, PirOpcode::VectorAdd | PirOpcode::VectorMul | PirOpcode::VectorFma),
            estimated_latency: Self::estimate_latency(inst.opcode),
            estimated_throughput: Self::estimate_throughput(inst.opcode),
            register_pressure: inst.operands.len() as f32 * 0.1,
        }
    }

    fn estimate_latency(opcode: PirOpcode) -> f32 {
        match opcode {
            PirOpcode::Add | PirOpcode::Sub => 1.0,
            PirOpcode::Mul => 3.0,
            PirOpcode::Div => 10.0,
            PirOpcode::Load => 4.0,
            PirOpcode::Store => 2.0,
            PirOpcode::VectorAdd | PirOpcode::VectorMul => 2.0,
            PirOpcode::VectorFma => 4.0,
            _ => 1.0,
        }
    }

    fn estimate_throughput(opcode: PirOpcode) -> f32 {
        match opcode {
            PirOpcode::Add | PirOpcode::Sub | PirOpcode::Mul => 0.5,
            PirOpcode::Div => 0.1,
            PirOpcode::Load => 0.5,
            PirOpcode::Store => 1.0,
            PirOpcode::VectorAdd | PirOpcode::VectorMul => 0.5,
            PirOpcode::VectorFma => 0.5,
            _ => 1.0,
        }
    }

    /// Convert to feature vector for ML model
    pub fn to_vector(&self) -> Array1<f32> {
        let mut features = Vec::with_capacity(32);
        features.push(self.opcode as f32);
        features.push(self.num_operands as f32);
        features.push(self.result_type as f32);
        features.push(self.has_side_effects as u32 as f32);
        features.push(self.is_memory_op as u32 as f32);
        features.push(self.is_control_flow as u32 as f32);
        features.push(self.is_vector_op as u32 as f32);
        features.push(self.estimated_latency);
        features.push(self.estimated_throughput);
        features.push(self.register_pressure);

        // Pad operand types to fixed size
        for i in 0..8 {
            if i < self.operand_types.len() {
                features.push(self.operand_types[i] as f32);
            } else {
                features.push(0.0);
            }
        }

        Array1::from_vec(features)
    }
}

/// Features for predictive inlining
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InliningFeatures {
    /// Caller function size (instructions)
    pub caller_size: u32,
    /// Callee function size (instructions)
    pub callee_size: u32,
    /// Call site frequency (from profile)
    pub call_frequency: f32,
    /// Whether callee is recursive
    pub is_recursive: bool,
    /// Whether callee has loops
    pub has_loops: bool,
    /// Number of call sites to this callee
    pub num_call_sites: u32,
    /// Callee's cyclomatic complexity
    pub cyclomatic_complexity: u32,
    /// Estimated register pressure increase
    pub register_pressure_delta: f32,
    /// Estimated code size increase
    pub code_size_delta: i32,
    /// Whether callee is hot (frequently executed)
    pub is_hot: bool,
    /// Inline heuristic score (traditional)
    pub heuristic_score: f32,
}

impl InliningFeatures {
    pub fn from_call_site(caller: &PirFunction, callee: &PirFunction, profile: &ExecutionProfile) -> Self {
        Self {
            caller_size: caller.instructions.len() as u32,
            callee_size: callee.instructions.len() as u32,
            call_frequency: profile.get_call_frequency(caller.id, callee.id),
            is_recursive: callee.is_recursive,
            has_loops: callee.has_loops(),
            num_call_sites: profile.get_call_site_count(callee.id),
            cyclomatic_complexity: callee.cyclomatic_complexity(),
            register_pressure_delta: callee.max_register_pressure() as f32,
            code_size_delta: callee.instructions.len() as i32,
            is_hot: profile.is_hot(callee.id),
            heuristic_score: Self::compute_heuristic(caller, callee, profile),
        }
    }

    fn compute_heuristic(caller: &PirFunction, callee: &PirFunction, profile: &ExecutionProfile) -> f32 {
        let size_factor = 1.0 - (callee.instructions.len() as f32 / 1000.0).min(1.0);
        let freq_factor = profile.get_call_frequency(caller.id, callee.id);
        let hot_factor = if profile.is_hot(callee.id) { 1.5 } else { 1.0 };
        size_factor * freq_factor * hot_factor
    }

    pub fn to_vector(&self) -> Array1<f32> {
        Array1::from_vec(vec![
            self.caller_size as f32,
            self.callee_size as f32,
            self.call_frequency,
            self.is_recursive as u32 as f32,
            self.has_loops as u32 as f32,
            self.num_call_sites as f32,
            self.cyclomatic_complexity as f32,
            self.register_pressure_delta,
            self.code_size_delta as f32,
            self.is_hot as u32 as f32,
            self.heuristic_score,
        ])
    }
}

/// Features for auto-vectorization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopFeatures {
    /// Loop nest depth
    pub nest_depth: u32,
    /// Trip count (if known)
    pub trip_count: Option<u64>,
    /// Whether loop has constant trip count
    pub constant_trip_count: bool,
    /// Number of memory accesses
    pub memory_accesses: u32,
    /// Number of memory loads
    pub loads: u32,
    /// Number of memory stores
    pub stores: u32,
    /// Stride pattern (0=unit, 1=constant, 2=variable, 3=unknown)
    pub stride_pattern: u32,
    /// Whether loop has reductions
    pub has_reduction: bool,
    /// Reduction operation type (encoded)
    pub reduction_op: u32,
    /// Whether loop has dependencies
    pub has_dependencies: bool,
    /// Dependency distance (if known)
    pub dependency_distance: i32,
    /// Whether loop is innermost
    pub is_innermost: bool,
    /// Estimated iteration count
    pub estimated_iterations: f32,
    /// Loop body size (instructions)
    pub body_size: u32,
    /// Whether loop contains function calls
    pub has_calls: bool,
    /// Data alignment (bytes)
    pub alignment: u32,
}

impl LoopFeatures {
    pub fn from_schedule_node(node: &ScheduleNode) -> Self {
        // Extract features from schedule tree node
        Self {
            nest_depth: node.depth() as u32,
            trip_count: node.trip_count(),
            constant_trip_count: node.trip_count().is_some(),
            memory_accesses: node.count_memory_ops(),
            loads: node.count_loads(),
            stores: node.count_stores(),
            stride_pattern: node.stride_pattern() as u32,
            has_reduction: node.has_reduction(),
            reduction_op: node.reduction_op() as u32,
            has_dependencies: node.has_dependencies(),
            dependency_distance: node.dependency_distance().unwrap_or(-1),
            is_innermost: node.is_innermost(),
            estimated_iterations: node.estimated_iterations(),
            body_size: node.body_instruction_count() as u32,
            has_calls: node.has_calls(),
            alignment: node.data_alignment(),
        }
    }

    pub fn to_vector(&self) -> Array1<f32> {
        Array1::from_vec(vec![
            self.nest_depth as f32,
            self.trip_count.unwrap_or(0) as f32,
            self.constant_trip_count as u32 as f32,
            self.memory_accesses as f32,
            self.loads as f32,
            self.stores as f32,
            self.stride_pattern as f32,
            self.has_reduction as u32 as f32,
            self.reduction_op as f32,
            self.has_dependencies as u32 as f32,
            self.dependency_distance as f32,
            self.is_innermost as u32 as f32,
            self.estimated_iterations,
            self.body_size as f32,
            self.has_calls as u32 as f32,
            self.alignment as f32,
        ])
    }
}

/// Features for profile-driven recompilation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileFeatures {
    /// Function execution count
    pub execution_count: u64,
    /// Total cycles spent
    pub total_cycles: u64,
    /// Average cycles per execution
    pub avg_cycles: f32,
    /// Cache miss rate
    pub cache_miss_rate: f32,
    /// Branch misprediction rate
    pub branch_mispred_rate: f32,
    /// Instruction mix: arithmetic ratio
    pub arithmetic_ratio: f32,
    /// Instruction mix: memory ratio
    pub memory_ratio: f32,
    /// Instruction mix: control flow ratio
    pub control_flow_ratio: f32,
    /// Instruction mix: vector ratio
    pub vector_ratio: f32,
    /// Register pressure (max)
    pub max_register_pressure: u32,
    /// Spill count
    pub spill_count: u32,
    /// Whether function is hot
    pub is_hot: bool,
    /// Time since last recompilation
    pub time_since_recompile: f32,
    /// Previous recompilation count
    pub recompile_count: u32,
    /// Speedup from last recompilation
    pub last_speedup: f32,
}

impl ProfileFeatures {
    pub fn from_profile(profile: &ExecutionProfile, func_id: FunctionId) -> Self {
        let stats = profile.get_function_stats(func_id);
        Self {
            execution_count: stats.execution_count,
            total_cycles: stats.total_cycles,
            avg_cycles: stats.total_cycles as f32 / stats.execution_count.max(1) as f32,
            cache_miss_rate: stats.cache_miss_rate,
            branch_mispred_rate: stats.branch_mispred_rate,
            arithmetic_ratio: stats.arithmetic_ratio,
            memory_ratio: stats.memory_ratio,
            control_flow_ratio: stats.control_flow_ratio,
            vector_ratio: stats.vector_ratio,
            max_register_pressure: stats.max_register_pressure,
            spill_count: stats.spill_count,
            is_hot: profile.is_hot(func_id),
            time_since_recompile: profile.time_since_recompile(func_id),
            recompile_count: profile.recompile_count(func_id),
            last_speedup: profile.last_speedup(func_id),
        }
    }

    pub fn to_vector(&self) -> Array1<f32> {
        Array1::from_vec(vec![
            self.execution_count as f32,
            self.total_cycles as f32,
            self.avg_cycles,
            self.cache_miss_rate,
            self.branch_mispred_rate,
            self.arithmetic_ratio,
            self.memory_ratio,
            self.control_flow_ratio,
            self.vector_ratio,
            self.max_register_pressure as f32,
            self.spill_count as f32,
            self.is_hot as u32 as f32,
            self.time_since_recompile,
            self.recompile_count as f32,
            self.last_speedup,
        ])
    }
}

/// Features for quantum circuit optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitFeatures {
    /// Number of qubits
    pub num_qubits: u32,
    /// Circuit depth
    pub depth: u32,
    /// Number of gates
    pub num_gates: u32,
    /// Two-qubit gate count
    pub two_qubit_gates: u32,
    /// Single-qubit gate count
    pub single_qubit_gates: u32,
    /// Gate type distribution (encoded)
    pub gate_distribution: Vec<f32>,
    /// Connectivity constraints violations
    pub connectivity_violations: u32,
    /// Estimated noise/error rate
    pub estimated_error_rate: f32,
    /// T-count (for fault tolerance)
    pub t_count: u32,
    /// Clifford gate count
    pub clifford_count: u32,
    /// Whether circuit has mid-circuit measurements
    pub has_measurements: bool,
    /// Whether circuit has classical control
    pub has_classical_control: bool,
    /// Parallelization potential (0-1)
    pub parallelization_potential: f32,
}

impl CircuitFeatures {
    pub fn from_circuit(circuit: &QuantumCircuit) -> Self {
        Self {
            num_qubits: circuit.num_qubits(),
            depth: circuit.depth(),
            num_gates: circuit.num_gates(),
            two_qubit_gates: circuit.count_two_qubit_gates(),
            single_qubit_gates: circuit.count_single_qubit_gates(),
            gate_distribution: circuit.gate_distribution(),
            connectivity_violations: circuit.connectivity_violations(),
            estimated_error_rate: circuit.estimated_error_rate(),
            t_count: circuit.t_count(),
            clifford_count: circuit.clifford_count(),
            has_measurements: circuit.has_measurements(),
            has_classical_control: circuit.has_classical_control(),
            parallelization_potential: circuit.parallelization_potential(),
        }
    }

    pub fn to_vector(&self) -> Array1<f32> {
        let mut features = vec![
            self.num_qubits as f32,
            self.depth as f32,
            self.num_gates as f32,
            self.two_qubit_gates as f32,
            self.single_qubit_gates as f32,
            self.connectivity_violations as f32,
            self.estimated_error_rate,
            self.t_count as f32,
            self.clifford_count as f32,
            self.has_measurements as u32 as f32,
            self.has_classical_control as u32 as f32,
            self.parallelization_potential,
        ];
        features.extend(self.gate_distribution.iter().cloned());
        Array1::from_vec(features)
    }
}

/// Placeholder types for compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PirOpcode {
    Add, Sub, Mul, Div,
    Load, Store,
    Branch, Call, Return,
    VectorAdd, VectorMul, VectorFma,
}

/// Placeholder for execution profile
pub struct ExecutionProfile;
impl ExecutionProfile {
    pub fn get_call_frequency(&self, _caller: FunctionId, _callee: FunctionId) -> f32 { 1.0 }
    pub fn get_call_site_count(&self, _callee: FunctionId) -> u32 { 1 }
    pub fn is_hot(&self, _func: FunctionId) -> bool { false }
    pub fn get_function_stats(&self, _func: FunctionId) -> FunctionStats { FunctionStats::default() }
    pub fn time_since_recompile(&self, _func: FunctionId) -> f32 { 0.0 }
    pub fn recompile_count(&self, _func: FunctionId) -> u32 { 0 }
    pub fn last_speedup(&self, _func: FunctionId) -> f32 { 1.0 }
}

#[derive(Default)]
pub struct FunctionStats {
    pub execution_count: u64 = 0,
    pub total_cycles: u64 = 0,
    pub cache_miss_rate: f32 = 0.0,
    pub branch_mispred_rate: f32 = 0.0,
    pub arithmetic_ratio: f32 = 0.0,
    pub memory_ratio: f32 = 0.0,
    pub control_flow_ratio: f32 = 0.0,
    pub vector_ratio: f32 = 0.0,
    pub max_register_pressure: u32 = 0,
    pub spill_count: u32 = 0,
}

pub type FunctionId = u64;

/// Placeholder for PirFunction
pub struct PirFunction {
    pub id: FunctionId,
    pub instructions: Vec<PirInstruction>,
    pub is_recursive: bool,
}
impl PirFunction {
    pub fn has_loops(&self) -> bool { false }
    pub fn cyclomatic_complexity(&self) -> u32 { 1 }
    pub fn max_register_pressure(&self) -> u32 { 16 }
}

/// Placeholder for PirInstruction
pub struct PirInstruction {
    pub opcode: PirOpcode,
    pub operands: Vec<Operand>,
    pub result_type: PirType,
    pub has_side_effects: bool,
}
pub struct Operand { pub ty: PirType }
pub type PirType = u32;

/// Placeholder for QuantumCircuit
pub struct QuantumCircuit;
impl QuantumCircuit {
    pub fn num_qubits(&self) -> u32 { 0 }
    pub fn depth(&self) -> u32 { 0 }
    pub fn num_gates(&self) -> u32 { 0 }
    pub fn count_two_qubit_gates(&self) -> u32 { 0 }
    pub fn count_single_qubit_gates(&self) -> u32 { 0 }
    pub fn gate_distribution(&self) -> Vec<f32> { vec![] }
    pub fn connectivity_violations(&self) -> u32 { 0 }
    pub fn estimated_error_rate(&self) -> f32 { 0.0 }
    pub fn t_count(&self) -> u32 { 0 }
    pub fn clifford_count(&self) -> u32 { 0 }
    pub fn has_measurements(&self) -> bool { false }
    pub fn has_classical_control(&self) -> bool { false }
    pub fn parallelization_potential(&self) -> f32 { 0.0 }
}