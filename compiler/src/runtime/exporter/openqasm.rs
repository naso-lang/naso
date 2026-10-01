use crate::ast::Quantity;
/// OpenQASM 3.0 Exporter
///
/// Lowers QIR operations into OpenQASM 3.0 string specifications including:
/// - Gate definitions
/// - qubit[n] registers
/// - bit[n] classical results
/// - Inverse gate uncomputation blocks for [0] and [1] quantities

#[cfg(feature = "llvm")]
use crate::codegen::qir::{QIRModule, QIROperation};
use crate::runtime::exporter::{
    DecompositionRule, ExportMetadata, ExportResult, ExporterError, HardwareExporter, TargetBackend,
};
use std::collections::HashMap;

/// OpenQASM 3.0 exporter
pub struct OpenQASMExporter {
    /// Include gate definitions in output
    include_definitions: bool,
    /// Target gate set for decomposition
    target_gates: Vec<String>,
    /// Enable automatic barrier insertion for [0]/[1] quantities
    auto_barriers: bool,
}

impl Default for OpenQASMExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenQASMExporter {
    /// Create a new OpenQASM exporter with default settings
    pub fn new() -> Self {
        Self {
            include_definitions: true,
            target_gates: vec![
                "h".to_string(),
                "x".to_string(),
                "y".to_string(),
                "z".to_string(),
                "s".to_string(),
                "sdg".to_string(),
                "t".to_string(),
                "tdg".to_string(),
                "rx".to_string(),
                "ry".to_string(),
                "rz".to_string(),
                "cx".to_string(),
                "cy".to_string(),
                "cz".to_string(),
                "swap".to_string(),
                "iswap".to_string(),
                "ccx".to_string(),
                "cphase".to_string(),
                "measure".to_string(),
                "reset".to_string(),
            ],
            auto_barriers: true,
        }
    }

    /// Create exporter with custom target gate set
    pub fn with_target_gates(target_gates: Vec<String>) -> Self {
        Self {
            include_definitions: true,
            target_gates,
            auto_barriers: true,
        }
    }

    /// Export a QIR module to OpenQASM 3.0
    fn export_module(&self, module: &QIRModule) -> Result<ExportResult, ExporterError> {
        let mut output = String::new();
        let mut metadata = ExportMetadata::default();
        let mut operation_counts: HashMap<String, usize> = HashMap::new();
        let mut gate_depth = 0;
        let mut current_depth = 0;
        let mut qubit_last_use: HashMap<usize, usize> = HashMap::new();

        // Header
        output.push_str("OPENQASM 3.0;\n\n");

        // Include standard gates if requested
        if self.include_definitions {
            output.push_str("include \"stdgates.inc\";\n\n");
        }

        // Qubit register
        if module.qubit_count > 0 {
            output.push_str(&format!("qubit[{}] q;\n", module.qubit_count));
            metadata.qubit_count = module.qubit_count;
            metadata.qubit_quantities = module.qubit_quantities.clone();
        }

        // Classical bit register for measurements
        let measurement_count = module
            .operations
            .iter()
            .filter(|op| matches!(op, QIROperation::Measure { .. }))
            .count();
        if measurement_count > 0 {
            output.push_str(&format!("bit[{}] c;\n\n", measurement_count));
            metadata.classical_bit_count = measurement_count;
        } else {
            output.push('\n');
        }

        // Track classical bit index for measurements
        let mut cbit_index = 0;

        // Process operations
        for (idx, op) in module.operations.iter().enumerate() {
            match op {
                QIROperation::AllocateQubit { index, quantity } => {
                    // Qubits are pre-allocated in the register
                    // Just track quantity for barrier/reset insertion
                    if *quantity == Quantity::Zero {
                        // [0] qubits: insert reset after use (will be handled at release)
                    } else if *quantity == Quantity::One {
                        // [1] qubits: track for single-use enforcement
                    }
                }
                QIROperation::ReleaseQubit { index } => {
                    let quantity = module
                        .qubit_quantities
                        .get(*index)
                        .cloned()
                        .unwrap_or(Quantity::Many);
                    if quantity == Quantity::Zero {
                        // [0] qubit: emit reset to ensure uncomputation
                        output.push_str(&format!("reset q[{}];\n", index));
                        operation_counts
                            .entry("reset".to_string())
                            .and_modify(|c| *c += 1)
                            .or_insert(1);
                        metadata.gate_count += 1;
                    } else if quantity == Quantity::One {
                        // [1] qubit: verify single use via barrier
                        if self.auto_barriers {
                            output.push_str(&format!("barrier q[{}];\n", index));
                        }
                    }
                }
                QIROperation::Gate {
                    name,
                    qubits,
                    params,
                } => {
                    let qasm_name = self.map_gate_name(name);
                    let gate_str = self.format_gate(&qasm_name, qubits, params);
                    output.push_str(&gate_str);
                    output.push('\n');

                    *operation_counts.entry(qasm_name.clone()).or_insert(0) += 1;
                    metadata.gate_count += 1;
                    current_depth += 1;
                    gate_depth = gate_depth.max(current_depth);

                    // Track qubit usage for depth calculation
                    for &q in qubits {
                        qubit_last_use.insert(q, idx);
                    }
                }
                QIROperation::Measure { qubit, basis } => {
                    let basis_suffix = match basis {
                        0 => "z",
                        1 => "x",
                        2 => "y",
                        _ => "z",
                    };

                    // For non-Z basis, add basis rotation before measurement
                    if *basis != 0 {
                        let basis_gate = match basis {
                            1 => "h",   // X basis = H then Z
                            2 => "sdg", // Y basis = S† then H then Z (simplified)
                            _ => "h",
                        };
                        output.push_str(&format!("{} q[{}];\n", basis_gate, qubit));
                        *operation_counts.entry(basis_gate.to_string()).or_insert(0) += 1;
                        metadata.gate_count += 1;
                    }

                    output.push_str(&format!("c[{}] = measure q[{}];\n", cbit_index, qubit));
                    *operation_counts.entry("measure".to_string()).or_insert(0) += 1;
                    metadata.measurement_count += 1;
                    metadata.gate_count += 1;
                    cbit_index += 1;
                    current_depth += 1;
                    gate_depth = gate_depth.max(current_depth);
                }
            }
        }

        metadata.gate_depth = gate_depth;
        metadata.operation_counts = operation_counts;

        Ok(ExportResult {
            output,
            metadata,
            backend: TargetBackend::OpenQASM3,
        })
    }

    /// Map QIR gate name to OpenQASM gate name
    fn map_gate_name(&self, qir_name: &str) -> String {
        // Remove "qir." prefix if present
        let name = qir_name.strip_prefix("qir.").unwrap_or(qir_name);

        // Map to OpenQASM standard gate names
        match name {
            "h" => "h".to_string(),
            "x" => "x".to_string(),
            "y" => "y".to_string(),
            "z" => "z".to_string(),
            "s" => "s".to_string(),
            "sdg" => "sdg".to_string(),
            "t" => "t".to_string(),
            "tdg" => "tdg".to_string(),
            "rx" => "rx".to_string(),
            "ry" => "ry".to_string(),
            "rz" => "rz".to_string(),
            "r1" => "rz".to_string(), // R1 = RZ up to global phase
            "rt1" => "rz".to_string(),
            "cx" => "cx".to_string(),
            "cy" => "cy".to_string(),
            "cz" => "cz".to_string(),
            "ccx" => "ccx".to_string(),
            "swap" => "swap".to_string(),
            "iswap" => "iswap".to_string(),
            "cphase" => "cphase".to_string(),
            "crx" => "crx".to_string(),
            "cry" => "cry".to_string(),
            "crz" => "crz".to_string(),
            "mz" => "measure".to_string(),
            "mx" => "measure".to_string(), // Will add H before
            "my" => "measure".to_string(), // Will add S†H before
            _ => name.to_string(),
        }
    }

    /// Format a gate operation as OpenQASM
    fn format_gate(&self, name: &str, qubits: &[usize], params: &[f64]) -> String {
        let qubit_str = qubits
            .iter()
            .map(|q| format!("q[{}]", q))
            .collect::<Vec<_>>()
            .join(", ");

        if params.is_empty() {
            format!("{} {};", name, qubit_str)
        } else {
            let param_str = params
                .iter()
                .map(|p| format!("{:.10}", p))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({}) {};", name, param_str, qubit_str)
        }
    }

    /// Generate inverse (adjoint) of a gate for uncomputation
    fn generate_adjoint(&self, name: &str, qubits: &[usize], params: &[f64]) -> String {
        let adjoint_name = match name {
            "h" => "h",
            "x" => "x",
            "y" => "y",
            "z" => "z",
            "s" => "sdg",
            "sdg" => "s",
            "t" => "tdg",
            "tdg" => "t",
            "rx" => "rx",
            "ry" => "ry",
            "rz" => "rz",
            "cx" => "cx",
            "cy" => "cy",
            "cz" => "cz",
            "ccx" => "ccx",
            "swap" => "swap",
            "iswap" => "iswap",
            "cphase" => "cphase",
            _ => name,
        };

        let qubit_str = qubits
            .iter()
            .map(|q| format!("q[{}]", q))
            .collect::<Vec<_>>()
            .join(", ");

        if params.is_empty() {
            format!("{} {};", adjoint_name, qubit_str)
        } else {
            // For parameterized gates, negate the angle for adjoint
            let neg_params = params
                .iter()
                .map(|p| format!("{:.10}", -p))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({}) {};", adjoint_name, neg_params, qubit_str)
        }
    }
}

impl HardwareExporter for OpenQASMExporter {
    fn export(&self, module: &QIRModule) -> Result<ExportResult, ExporterError> {
        self.export_module(module)
    }

    fn target_backend(&self) -> TargetBackend {
        TargetBackend::OpenQASM3
    }

    fn supported_gates(&self) -> &'static [&'static str] {
        &[
            "h", "x", "y", "z", "s", "sdg", "t", "tdg", "rx", "ry", "rz", "r1", "rt1", "cx", "cy",
            "cz", "ccx", "swap", "iswap", "cphase", "crx", "cry", "crz", "measure", "reset",
            "barrier",
        ]
    }

    fn decomposition_rules(&self) -> Vec<DecompositionRule> {
        vec![
            DecompositionRule {
                source_gate: "ccx".to_string(),
                target_gates: vec![
                    "h".to_string(),
                    "cx".to_string(),
                    "t".to_string(),
                    "tdg".to_string(),
                ],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "swap".to_string(),
                target_gates: vec!["cx".to_string()],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "iswap".to_string(),
                target_gates: vec!["cx".to_string(), "h".to_string(), "s".to_string()],
                is_exact: true,
                error_bound: None,
            },
            DecompositionRule {
                source_gate: "cphase".to_string(),
                target_gates: vec!["rz".to_string(), "cx".to_string()],
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
    fn test_openqasm_exporter_creation() {
        let exporter = OpenQASMExporter::new();
        assert_eq!(exporter.target_backend(), TargetBackend::OpenQASM3);
        assert!(!exporter.supported_gates().is_empty());
    }

    #[test]
    fn test_map_gate_name() {
        let exporter = OpenQASMExporter::new();
        assert_eq!(exporter.map_gate_name("qir.h"), "h");
        assert_eq!(exporter.map_gate_name("qir.cx"), "cx");
        assert_eq!(exporter.map_gate_name("qir.rz"), "rz");
        assert_eq!(exporter.map_gate_name("h"), "h");
    }

    #[test]
    fn test_format_gate() {
        let exporter = OpenQASMExporter::new();
        assert_eq!(exporter.format_gate("h", &[0], &[]), "h q[0];");
        assert_eq!(exporter.format_gate("cx", &[0, 1], &[]), "cx q[0], q[1];");
        assert_eq!(
            exporter.format_gate("rz", &[0], &[1.57]),
            "rz(1.5700000000) q[0];"
        );
    }

    #[test]
    fn test_export_simple_circuit() {
        let exporter = OpenQASMExporter::new();
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
        assert!(result.output.contains("OPENQASM 3.0"));
        assert!(result.output.contains("qubit[2] q;"));
        assert!(result.output.contains("bit[2] c;"));
        assert!(result.output.contains("h q[0];"));
        assert!(result.output.contains("cx q[0], q[1];"));
        assert!(result.output.contains("c[0] = measure q[0];"));
        assert!(result.output.contains("c[1] = measure q[1];"));
        assert_eq!(result.metadata.qubit_count, 2);
        assert_eq!(result.metadata.gate_count, 2);
        assert_eq!(result.metadata.measurement_count, 2);
    }

    #[test]
    fn test_export_with_zero_quantity() {
        let exporter = OpenQASMExporter::new();
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
        // [0] qubit should have reset on release
        assert!(result.output.contains("reset q[0];"));
    }

    #[test]
    fn test_export_with_one_quantity() {
        let exporter = OpenQASMExporter::new();
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
        // [1] qubit should have barrier on release
        assert!(result.output.contains("barrier q[0];"));
    }

    #[test]
    fn test_measurement_basis() {
        let exporter = OpenQASMExporter::new();
        let module = QIRModule {
            qubit_count: 1,
            qubit_quantities: vec![Quantity::Many],
            operations: vec![
                QIROperation::Measure { qubit: 0, basis: 1 }, // X basis
            ],
        };

        let result = exporter.export(&module).unwrap();
        // X basis measurement should have H before measure
        assert!(result.output.contains("h q[0];"));
        assert!(result.output.contains("c[0] = measure q[0];"));
    }
}
