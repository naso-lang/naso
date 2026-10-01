use crate::ast::Quantity;
/// Amazon Braket Hardware Backend Exporter
///
/// Lowers QIR modules to Amazon Braket JSON AST structure for execution
/// on AWS quantum hardware (superconducting, ion trap, photonic).

#[cfg(feature = "llvm")]
use crate::codegen::qir::{QIRModule, QIROperation};
use crate::runtime::exporter::{
    DecompositionRule, ExportMetadata, ExportResult, ExporterError, HardwareExporter, TargetBackend,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Braket IR (Intermediate Representation) types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BraketIR {
    /// Braket IR version
    pub braket_schema_header: BraketSchemaHeader,
    /// Circuit definition
    pub circuit: BraketCircuit,
    /// Device parameters (optional)
    pub device_parameters: Option<Value>,
}

/// Braket schema header
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BraketSchemaHeader {
    pub name: String,
    pub version: String,
}

/// Braket circuit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BraketCircuit {
    /// Number of qubits
    pub qubit_count: usize,
    /// Instructions/gates
    pub instructions: Vec<BraketInstruction>,
    /// Results types (measurements, observables)
    pub results: Vec<BraketResult>,
}

/// Braket instruction (gate operation)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum BraketInstruction {
    /// Single qubit gate
    #[serde(rename = "gate")]
    Gate {
        #[serde(rename = "gate")]
        gate_name: String,
        target: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        angle: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        control: Option<usize>,
    },
    /// Two qubit gate
    #[serde(rename = "gate")]
    TwoQubitGate {
        #[serde(rename = "gate")]
        gate_name: String,
        target: usize,
        control: usize,
    },
    /// Multi-qubit gate (CCX, etc.)
    #[serde(rename = "gate")]
    MultiQubitGate {
        #[serde(rename = "gate")]
        gate_name: String,
        targets: Vec<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        controls: Option<Vec<usize>>,
    },
    /// Measurement
    #[serde(rename = "measurement")]
    Measurement {
        targets: Vec<usize>,
        /// Serde name must not collide with this enum's internal `type` tag;
        /// the Braket schema field is `measurementType`.
        measurement_type: String,
    },
    /// Reset
    #[serde(rename = "reset")]
    Reset { target: usize },
    /// Barrier
    #[serde(rename = "barrier")]
    Barrier { targets: Vec<usize> },
}

/// Braket result type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum BraketResult {
    /// Sample measurement results
    #[serde(rename = "sample")]
    Sample { targets: Option<Vec<usize>> },
    /// Expectation value of observable
    #[serde(rename = "expectation")]
    Expectation {
        observable: Vec<String>,
        targets: Vec<usize>,
    },
    /// Probability of computational basis states
    #[serde(rename = "probability")]
    Probability { targets: Option<Vec<usize>> },
    /// Amplitude of specific states
    #[serde(rename = "amplitude")]
    Amplitude { states: Vec<String> },
    /// Variance of observable
    #[serde(rename = "variance")]
    Variance {
        observable: Vec<String>,
        targets: Vec<usize>,
    },
}

/// Amazon Braket exporter
pub struct BraketExporter {
    /// Target device ARN (optional)
    device_arn: Option<String>,
    /// Include device parameters
    include_device_params: bool,
    /// Default shots for sampling
    default_shots: usize,
}

impl Default for BraketExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl BraketExporter {
    /// Create a new Braket exporter
    pub fn new() -> Self {
        Self {
            device_arn: None,
            include_device_params: false,
            default_shots: 1000,
        }
    }

    /// Set target device ARN
    pub fn with_device_arn(mut self, arn: String) -> Self {
        self.device_arn = Some(arn);
        self
    }

    /// Set default shots
    pub fn with_shots(mut self, shots: usize) -> Self {
        self.default_shots = shots;
        self
    }

    /// Export a QIR module to Braket IR
    fn export_module(&self, module: &QIRModule) -> Result<ExportResult, ExporterError> {
        let mut instructions = Vec::new();
        let mut results = Vec::new();
        let mut operation_counts: HashMap<String, usize> = HashMap::new();
        let mut gate_depth = 0;
        let mut current_depth = 0;

        // Track measurements for result declaration
        let mut measurement_targets = Vec::new();

        // Process operations
        for op in &module.operations {
            match op {
                QIROperation::AllocateQubit { index, quantity } => {
                    // Qubits are implicit in Braket by qubit count
                    // Track quantity for reset/barrier insertion
                    if *quantity == Quantity::Zero {
                        // Will add reset at release
                    }
                }
                QIROperation::ReleaseQubit { index } => {
                    let quantity = module
                        .qubit_quantities
                        .get(*index)
                        .cloned()
                        .unwrap_or(Quantity::Many);
                    if quantity == Quantity::Zero {
                        // [0] qubit: emit reset
                        instructions.push(BraketInstruction::Reset { target: *index });
                        *operation_counts.entry("reset".to_string()).or_insert(0) += 1;
                        current_depth += 1;
                    } else if quantity == Quantity::One {
                        // [1] qubit: add barrier
                        instructions.push(BraketInstruction::Barrier {
                            targets: vec![*index],
                        });
                        *operation_counts.entry("barrier".to_string()).or_insert(0) += 1;
                        current_depth += 1;
                    }
                    gate_depth = gate_depth.max(current_depth);
                }
                QIROperation::Gate {
                    name,
                    qubits,
                    params,
                } => {
                    let braket_gate = self.map_gate_name(name);
                    let instruction = self.format_gate(&braket_gate, qubits, params);
                    instructions.push(instruction);

                    *operation_counts.entry(braket_gate).or_insert(0) += 1;
                    current_depth += 1;
                    gate_depth = gate_depth.max(current_depth);
                }
                QIROperation::Measure { qubit, basis } => {
                    // Handle non-Z basis with pre-rotation
                    if *basis != 0 {
                        let basis_gate = match basis {
                            1 => "h",   // X basis
                            2 => "sdg", // Y basis (S† then H)
                            _ => "h",
                        };
                        instructions.push(BraketInstruction::Gate {
                            gate_name: basis_gate.to_string(),
                            target: *qubit,
                            angle: None,
                            control: None,
                        });
                        *operation_counts.entry(basis_gate.to_string()).or_insert(0) += 1;
                        current_depth += 1;
                    }

                    // Add measurement instruction
                    instructions.push(BraketInstruction::Measurement {
                        targets: vec![*qubit],
                        measurement_type: "sample".to_string(),
                    });
                    *operation_counts.entry("measure".to_string()).or_insert(0) += 1;
                    measurement_targets.push(*qubit);
                    current_depth += 1;
                    gate_depth = gate_depth.max(current_depth);
                }
            }
        }

        // Add result types
        if !measurement_targets.is_empty() {
            results.push(BraketResult::Sample {
                targets: Some(measurement_targets.clone()),
            });
        }

        // Build Braket IR
        let braket_ir = BraketIR {
            braket_schema_header: BraketSchemaHeader {
                name: "braket.ir.jaqcd.program".to_string(),
                version: "1".to_string(),
            },
            circuit: BraketCircuit {
                qubit_count: module.qubit_count,
                instructions,
                results,
            },
            device_parameters: self
                .device_arn
                .as_ref()
                .map(|arn| json!({ "deviceArn": arn })),
        };

        // Serialize to JSON
        let output = serde_json::to_string_pretty(&braket_ir).map_err(|e| {
            ExporterError::ExportFailed(format!("JSON serialization failed: {}", e))
        })?;

        let mut metadata = ExportMetadata::default();
        metadata.qubit_count = module.qubit_count;
        metadata.gate_count = operation_counts.values().sum();
        metadata.gate_depth = gate_depth;
        metadata.measurement_count = measurement_targets.len();
        metadata.operation_counts = operation_counts;
        metadata.qubit_quantities = module.qubit_quantities.clone();
        metadata.classical_bit_count = measurement_targets.len();

        Ok(ExportResult {
            output,
            metadata,
            backend: TargetBackend::Braket,
        })
    }

    /// Map QIR gate name to Braket gate name
    fn map_gate_name(&self, qir_name: &str) -> String {
        let name = qir_name.strip_prefix("qir.").unwrap_or(qir_name);

        // Map to Braket gate names
        match name {
            "h" => "h".to_string(),
            "x" => "x".to_string(),
            "y" => "y".to_string(),
            "z" => "z".to_string(),
            "s" => "s".to_string(),
            "sdg" => "si".to_string(), // Braket uses "si" for S†
            "t" => "t".to_string(),
            "tdg" => "ti".to_string(), // Braket uses "ti" for T†
            "rx" => "rx".to_string(),
            "ry" => "ry".to_string(),
            "rz" => "rz".to_string(),
            "r1" => "rz".to_string(),
            "rt1" => "rz".to_string(),
            "cx" => "cnot".to_string(), // Braket uses "cnot"
            "cy" => "cy".to_string(),
            "cz" => "cz".to_string(),
            "ccx" => "ccnot".to_string(), // Braket uses "ccnot"
            "swap" => "swap".to_string(),
            "iswap" => "iswap".to_string(),
            "cphase" => "cphaseshift".to_string(), // Braket uses "cphaseshift"
            "crx" => "crx".to_string(),
            "cry" => "cry".to_string(),
            "crz" => "crz".to_string(),
            "mz" => "measure".to_string(),
            _ => name.to_string(),
        }
    }

    /// Format a gate as Braket instruction
    fn format_gate(&self, name: &str, qubits: &[usize], params: &[f64]) -> BraketInstruction {
        match qubits.len() {
            1 => {
                // Single qubit gate
                BraketInstruction::Gate {
                    gate_name: name.to_string(),
                    target: qubits[0],
                    angle: if params.is_empty() {
                        None
                    } else {
                        Some(params[0])
                    },
                    control: None,
                }
            }
            2 => {
                // Two qubit gate
                if name == "cx" || name == "cnot" || name == "cy" || name == "cz" {
                    BraketInstruction::TwoQubitGate {
                        gate_name: name.to_string(),
                        control: qubits[0],
                        target: qubits[1],
                    }
                } else if name == "swap" || name == "iswap" {
                    BraketInstruction::TwoQubitGate {
                        gate_name: name.to_string(),
                        control: qubits[0],
                        target: qubits[1],
                    }
                } else {
                    // Controlled rotation gates
                    BraketInstruction::TwoQubitGate {
                        gate_name: name.to_string(),
                        control: qubits[0],
                        target: qubits[1],
                    }
                }
            }
            n if n >= 3 => {
                // Multi-qubit gate (CCX, etc.)
                BraketInstruction::MultiQubitGate {
                    gate_name: name.to_string(),
                    targets: qubits[1..].to_vec(),
                    controls: Some(vec![qubits[0]]),
                }
            }
            _ => {
                // Fallback
                BraketInstruction::Gate {
                    gate_name: name.to_string(),
                    target: 0,
                    angle: None,
                    control: None,
                }
            }
        }
    }
}

impl HardwareExporter for BraketExporter {
    fn export(&self, module: &QIRModule) -> Result<ExportResult, ExporterError> {
        self.export_module(module)
    }

    fn target_backend(&self) -> TargetBackend {
        TargetBackend::Braket
    }

    fn supported_gates(&self) -> &'static [&'static str] {
        &[
            "h",
            "x",
            "y",
            "z",
            "s",
            "si",
            "t",
            "ti",
            "rx",
            "ry",
            "rz",
            "cnot",
            "cy",
            "cz",
            "ccnot",
            "swap",
            "iswap",
            "cphaseshift",
            "crx",
            "cry",
            "crz",
            "measure",
            "reset",
            "barrier",
        ]
    }

    fn decomposition_rules(&self) -> Vec<DecompositionRule> {
        vec![
            DecompositionRule {
                source_gate: "ccx".to_string(),
                target_gates: vec![
                    "h".to_string(),
                    "cnot".to_string(),
                    "t".to_string(),
                    "ti".to_string(),
                ],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "swap".to_string(),
                target_gates: vec!["cnot".to_string()],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "iswap".to_string(),
                target_gates: vec!["cnot".to_string(), "h".to_string(), "s".to_string()],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "cphase".to_string(),
                target_gates: vec!["rz".to_string(), "cnot".to_string()],
                is_exact: true,
                error_bound: None,
            },
            // Braket-specific: S† and T† are native
            DecompositionRule {
                source_gate: "sdg".to_string(),
                target_gates: vec!["si".to_string()],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "tdg".to_string(),
                target_gates: vec!["ti".to_string()],
                is_exact: true,
                error_bound: None,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Quantity;
    use crate::codegen::qir::{QIRModule, QIROperation};

    #[test]
    fn test_braket_exporter_creation() {
        let exporter = BraketExporter::new();
        assert_eq!(exporter.target_backend(), TargetBackend::Braket);
        assert!(!exporter.supported_gates().is_empty());
    }

    #[test]
    fn test_map_gate_name() {
        let exporter = BraketExporter::new();
        assert_eq!(exporter.map_gate_name("qir.h"), "h");
        assert_eq!(exporter.map_gate_name("qir.cx"), "cnot");
        assert_eq!(exporter.map_gate_name("qir.ccx"), "ccnot");
        assert_eq!(exporter.map_gate_name("qir.sdg"), "si");
        assert_eq!(exporter.map_gate_name("qir.tdg"), "ti");
    }

    #[test]
    fn test_export_simple_circuit() {
        let exporter = BraketExporter::new();
        let module = QIRModule {
            qubit_count: 2,
            qubit_quantities: vec![Quantity::Many, Quantity::Many],
            operations: vec![
                QIROperation::Gate {
                    name: "h".to_string(),
                    qubits: vec![0],
                    params: vec![],
                },
                QIROperation::Gate {
                    name: "cx".to_string(),
                    qubits: vec![0, 1],
                    params: vec![],
                },
                QIROperation::Measure { qubit: 0, basis: 0 },
                QIROperation::Measure { qubit: 1, basis: 0 },
            ],
        };

        let result = exporter.export(&module).unwrap();
        let braket_ir: BraketIR = serde_json::from_str(&result.output).unwrap();

        assert_eq!(
            braket_ir.braket_schema_header.name,
            "braket.ir.jaqcd.program"
        );
        assert_eq!(braket_ir.circuit.qubit_count, 2);
        assert_eq!(braket_ir.circuit.instructions.len(), 4); // h, cnot, measure, measure
        assert_eq!(result.metadata.qubit_count, 2);
        assert_eq!(result.metadata.gate_count, 2);
        assert_eq!(result.metadata.measurement_count, 2);
    }

    #[test]
    fn test_export_with_zero_quantity() {
        let exporter = BraketExporter::new();
        let module = QIRModule {
            qubit_count: 1,
            qubit_quantities: vec![Quantity::Zero],
            operations: vec![
                QIROperation::AllocateQubit {
                    index: 0,
                    quantity: Quantity::Zero,
                },
                QIROperation::Gate {
                    name: "h".to_string(),
                    qubits: vec![0],
                    params: vec![],
                },
                QIROperation::ReleaseQubit { index: 0 },
            ],
        };

        let result = exporter.export(&module).unwrap();
        let braket_ir: BraketIR = serde_json::from_str(&result.output).unwrap();

        // [0] qubit should have reset on release
        let has_reset = braket_ir
            .circuit
            .instructions
            .iter()
            .any(|i| matches!(i, BraketInstruction::Reset { .. }));
        assert!(has_reset);
    }

    #[test]
    fn test_export_with_one_quantity() {
        let exporter = BraketExporter::new();
        let module = QIRModule {
            qubit_count: 1,
            qubit_quantities: vec![Quantity::One],
            operations: vec![
                QIROperation::AllocateQubit {
                    index: 0,
                    quantity: Quantity::One,
                },
                QIROperation::Gate {
                    name: "h".to_string(),
                    qubits: vec![0],
                    params: vec![],
                },
                QIROperation::ReleaseQubit { index: 0 },
            ],
        };

        let result = exporter.export(&module).unwrap();
        let braket_ir: BraketIR = serde_json::from_str(&result.output).unwrap();

        // [1] qubit should have barrier on release
        let has_barrier = braket_ir
            .circuit
            .instructions
            .iter()
            .any(|i| matches!(i, BraketInstruction::Barrier { .. }));
        assert!(has_barrier);
    }

    #[test]
    fn test_measurement_basis_x() {
        let exporter = BraketExporter::new();
        let module = QIRModule {
            qubit_count: 1,
            qubit_quantities: vec![Quantity::Many],
            operations: vec![
                QIROperation::Measure { qubit: 0, basis: 1 }, // X basis
            ],
        };

        let result = exporter.export(&module).unwrap();
        let braket_ir: BraketIR = serde_json::from_str(&result.output).unwrap();

        // X basis measurement should have H before measure
        let has_h =
            braket_ir.circuit.instructions.iter().any(
                |i| matches!(i, BraketInstruction::Gate { gate_name, .. } if gate_name == "h"),
            );
        assert!(has_h);
    }

    #[test]
    fn test_braket_ir_serialization() {
        let exporter = BraketExporter::new();
        let module = QIRModule {
            qubit_count: 2,
            qubit_quantities: vec![Quantity::Many, Quantity::Many],
            operations: vec![
                QIROperation::Gate {
                    name: "h".to_string(),
                    qubits: vec![0],
                    params: vec![],
                },
                QIROperation::Gate {
                    name: "cx".to_string(),
                    qubits: vec![0, 1],
                    params: vec![],
                },
            ],
        };

        let result = exporter.export(&module).unwrap();

        // Verify valid JSON
        let value: Value = serde_json::from_str(&result.output).unwrap();
        assert!(value.get("braketSchemaHeader").is_some());
        assert!(value.get("circuit").is_some());
        assert_eq!(value["circuit"]["qubitCount"], 2);
    }
}
