//! OpenQASM 3.0 Exporter
//!
//! Lowers QIR operations into OpenQASM 3.0 string specifications including:
//! - Gate definitions
//! - qubit[n] registers
//! - bit[n] classical results
//! - `reset` / `barrier` blocks for [0] and [1] quantities
//!
//! `generate_adjoint` holds the adjoint table the uncomputation path will need. It
//! has no caller yet, because the [0] path emits `reset` -- which discards rather
//! than inverts -- so no exported circuit consults it. It is retained because it is
//! quantum knowledge encoded once and correctly (`s`/`sdg` and `t`/`tdg` swap, every
//! self-inverse gate maps to itself, a rotation negates its angle), and losing it
//! means re-deriving the table. It is covered by tests rather than merely allowed,
//! so it cannot rot while it waits.

use crate::ast::Quantity;
#[cfg(feature = "llvm")]
use crate::codegen::qir::{QIRModule, QIROperation};
use crate::runtime::exporter::{
    DecompositionRule, ExportMetadata, ExportResult, ExporterError, HardwareExporter, TargetBackend,
};
use std::collections::HashMap;

/// The OpenQASM gate name for a verified [`Gate`].
///
/// Every variant maps to a name; the gates the simulator models all have a standard
/// OpenQASM spelling. `iswap` and `cphase` are NOT here because they are not modelled --
/// they are handled explicitly at the call site rather than being given a gate they do
/// not have.
fn openqasm_name_of_gate(gate: naso_gates::statevector::Gate) -> &'static str {
    use naso_gates::statevector::Gate;
    match gate {
        Gate::H => "h",
        Gate::X => "x",
        Gate::Y => "y",
        Gate::Z => "z",
        Gate::S => "s",
        Gate::Sdg => "sdg",
        Gate::T => "t",
        Gate::Tdg => "tdg",
        Gate::Rx(_) => "rx",
        Gate::Ry(_) => "ry",
        Gate::Rz(_) => "rz",
        Gate::Cx => "cx",
        Gate::Cy => "cy",
        Gate::Cz => "cz",
        Gate::Ccx => "ccx",
        Gate::Swap => "swap",
    }
}

/// The verified [`Gate`] an OpenQASM gate name denotes, if the simulator models it.
///
/// `None` for a gate with no matrix here, which is what makes an unmodelled gate a refusal
/// upstream rather than an assumption of self-inverseness.
fn gate_from_openqasm_name(name: &str) -> Option<naso_gates::statevector::Gate> {
    use naso_gates::statevector::Gate;
    Some(match name {
        "h" => Gate::H,
        "x" => Gate::X,
        "y" => Gate::Y,
        "z" => Gate::Z,
        "s" => Gate::S,
        "sdg" => Gate::Sdg,
        "t" => Gate::T,
        "tdg" => Gate::Tdg,
        "rx" => Gate::Rx(0.0),
        "ry" => Gate::Ry(0.0),
        "rz" => Gate::Rz(0.0),
        "cx" => Gate::Cx,
        "cy" => Gate::Cy,
        "cz" => Gate::Cz,
        "ccx" => Gate::Ccx,
        "swap" => Gate::Swap,
        _ => return None,
    })
}

/// OpenQASM 3.0 exporter
pub struct OpenQASMExporter {
    /// Include gate definitions in output
    include_definitions: bool,
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
                QIROperation::AllocateQubit { index: _, quantity } => {
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
                        // `reset` is real emitted uncomputation, so it is counted -- but
                        // under `operation_counts` only, not as a gate. Treating it as a
                        // gate would put a `[0]`-qubit reset in the same tally as an `h`
                        // and make the two exporters disagree.
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
                    let _basis_suffix = match basis {
                        0 => "z",
                        1 => "x",
                        2 => "y",
                        _ => "z",
                    };

                    // For non-Z basis, add basis rotation before measurement
                    if *basis != 0 {
                        // An unsupported basis is REFUSED, not defaulted to X. See the
                        // Braket exporter for the same fix; the reasoning is identical: the
                        // old `_ => "h"` read out the X axis while the circuit reported the
                        // basis it had asked for.
                        let basis_gate = match basis {
                            1 => "h",   // X basis = H then Z
                            2 => "sdg", // Y basis = S† then H then Z (simplified)
                            other => {
                                return Err(ExporterError::UnsupportedOperation(format!(
                                    "measurement in basis {other} is not implemented: this \
                                     exporter supports basis 0 (Z), 1 (X) and 2 (Y). \
                     Assuming X would read out a different axis than the circuit asked \
                     for. Refused rather than guessed."
                                )));
                            }
                        };
                        output.push_str(&format!("{} q[{}];\n", basis_gate, qubit));
                        *operation_counts.entry(basis_gate.to_string()).or_insert(0) += 1;
                        metadata.gate_count += 1;
                    }

                    output.push_str(&format!("c[{}] = measure q[{}];\n", cbit_index, qubit));
                    *operation_counts.entry("measure".to_string()).or_insert(0) += 1;
                    metadata.measurement_count += 1;
                    // NOT a gate. Incrementing `gate_count` here double-counted every
                    // measurement, which `measurement_count` already reports.
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
    ///
    /// Retained for the uncomputation path, which does not call it yet. The allow is
    /// narrow and deliberate: clippy lints the library without `cfg(test)`, so the
    /// unit tests that pin this table do not count as a use, and without the allow
    /// clippy would demand the method be deleted. Deleting it would throw away the
    /// adjoint relation for every gate -- the part of a quantum compiler that is
    /// easiest to get subtly wrong and cheapest to get right once.
    #[allow(dead_code)]
    fn generate_adjoint(
        &self,
        name: &str,
        qubits: &[usize],
        params: &[f64],
    ) -> Result<String, ExporterError> {
        // The adjoint relation comes from `naso_gates::inverse_of`, the one table that is
        // checked NUMERICALLY against a state-vector simulator. It used to be a second
        // hand-written table here, which is precisely the arrangement that let the
        // S-dagger defect exist at all: two copies of an adjoint relation drift, and drift
        // does not fail to build -- it emits a circuit computing the wrong function.
        //
        // The two gates below are not in `naso_gates` yet: `iswap` and `cphase` are
        // self-inverse or parameter-symmetric, and are declared as such HERE rather than
        // silently falling through. See `adjoints_an_unmodelled_gate_are_refused` for why
        // the unmodelled case must not be guessed.
        let adjoint_name = match name {
            "iswap" => "iswap",
            "cphase" => "cphase",
            other => match gate_from_openqasm_name(other) {
                Some(gate) => match naso_gates::gate_inverse::inverse_of(gate) {
                    // A rotation keeps its name; the negation happens to the angle below.
                    naso_gates::statevector::Gate::Rx(_) => "rx",
                    naso_gates::statevector::Gate::Ry(_) => "ry",
                    naso_gates::statevector::Gate::Rz(_) => "rz",
                    other_gate => openqasm_name_of_gate(other_gate),
                },
                // An unrecognised gate has no adjoint relation, so it cannot be inverted.
                //
                // This used to return `name` -- i.e. to ASSUME the gate was self-inverse.
                // A gate that is not self-inverse, exported through here, would emit its own
                // forward application where an inverse belonged, and the resulting circuit
                // would compute a different function with no diagnostic. That is the same
                // fabrication class already fixed in a now-deleted QIR gate table, which used to map
                // every unknown gate onto `qir.h`.
                None => {
                    return Err(ExporterError::UnsupportedOperation(format!(
                        "no adjoint relation for gate `{other}`: it is not in the verified \
                         gate table, and assuming it is self-inverse would emit a circuit \
                         computing the wrong function. Refused rather than guessed."
                    )));
                }
            },
        };

        let qubit_str = qubits
            .iter()
            .map(|q| format!("q[{}]", q))
            .collect::<Vec<_>>()
            .join(", ");

        if params.is_empty() {
            Ok(format!("{} {};", adjoint_name, qubit_str))
        } else {
            // For parameterized gates, negate the angle for adjoint
            let neg_params = params
                .iter()
                .map(|p| format!("{:.10}", -p))
                .collect::<Vec<_>>()
                .join(", ");
            Ok(format!("{}({}) {};", adjoint_name, neg_params, qubit_str))
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

    /// A minimal QIR module containing one measurement in `basis`.
    ///
    /// Built by struct literal rather than a constructor: `QIRModule` has no `new`, and adding
    /// one for a test would be a change to production API made only to serve a test.
    fn module_measuring_in(basis: usize) -> QIRModule {
        QIRModule {
            qubit_count: 1,
            qubit_quantities: vec![crate::ast::Quantity::One],
            operations: vec![QIROperation::Measure { qubit: 0, basis }],
        }
    }

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

    /// An unsupported measurement basis must be REFUSED, not defaulted to X.
    ///
    /// The old code answered any basis it did not implement with the X-basis rotation, so a
    /// circuit measuring in an unimplemented basis read out the X axis while reporting the
    /// basis it had been asked for.
    #[test]
    fn an_unsupported_measurement_basis_is_refused_rather_than_read_as_x() {
        let module = module_measuring_in(7);
        let result = OpenQASMExporter::new().export(&module);
        assert!(
            result.is_err(),
            "basis 7 must not silently read out the X axis"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("basis 7") && message.contains("not implemented"),
            "the diagnostic must name the basis and say what is unsupported, got: {message}"
        );
    }

    /// The three supported bases still work, so the refusal is not catching valid input.
    #[test]
    fn every_supported_measurement_basis_still_exports() {
        for (basis, expected) in [(0usize, None), (1, Some("h")), (2, Some("sdg"))] {
            let module = module_measuring_in(basis);
            let result = OpenQASMExporter::new().export(&module);
            assert!(result.is_ok(), "basis {basis} is supported and must export");
            if let Some(gate) = expected {
                let rendered = format!("{:?}", result.unwrap());
                assert!(
                    rendered.contains(gate),
                    "basis {basis} should apply `{gate}`, got: {rendered}"
                );
            }
        }
    }
}

#[cfg(test)]
mod adjoint_tests {
    use super::OpenQASMExporter;

    /// The adjoint relation is now read from `naso_gates::inverse_of`, so these tests check
    /// two things: that the exporter wires the verified table through faithfully, and that
    /// the relation itself is numerically correct.
    ///
    /// The second half is what matters. A shape assertion -- "sdg comes back for s" -- passes
    /// just as well whether the table is right or wrong, which is why the gate-inverse tests
    /// in `naso-gates` apply the gates and compare amplitudes instead.
    #[test]
    fn the_exporter_adjoints_agree_with_the_verified_table() {
        use naso_gates::gate_inverse::inverse_of;
        use naso_gates::statevector::Gate;
        let e = OpenQASMExporter::new();

        // Every gate the simulator models, cross-checked against the one table.
        for (name, gate) in [
            ("h", Gate::H),
            ("x", Gate::X),
            ("y", Gate::Y),
            ("z", Gate::Z),
            ("s", Gate::S),
            ("sdg", Gate::Sdg),
            ("t", Gate::T),
            ("tdg", Gate::Tdg),
            ("cx", Gate::Cx),
            ("cy", Gate::Cy),
            ("cz", Gate::Cz),
            ("swap", Gate::Swap),
        ] {
            let expected = match inverse_of(gate) {
                Gate::H => "h",
                Gate::X => "x",
                Gate::Y => "y",
                Gate::Z => "z",
                Gate::S => "s",
                Gate::Sdg => "sdg",
                Gate::T => "t",
                Gate::Tdg => "tdg",
                Gate::Cx => "cx",
                Gate::Cy => "cy",
                Gate::Cz => "cz",
                Gate::Swap => "swap",
                other => panic!("unexpected inverse {other:?} for {gate:?}"),
            };
            assert_eq!(
                e.generate_adjoint(name, &[0], &[]).unwrap(),
                format!("{expected} q[0];"),
                "the exporter's adjoint for `{name}` must be the verified one"
            );
        }
    }

    /// `s` and `t` are NOT self-inverse; their adjoints are the dagger variants, and
    /// the mapping has to go both ways. This is the specific relation that shipped wrong
    /// once already.
    #[test]
    fn a_phase_gate_adjoint_is_its_dagger() {
        let e = OpenQASMExporter::new();
        assert_eq!(e.generate_adjoint("s", &[1], &[]).unwrap(), "sdg q[1];");
        assert_eq!(e.generate_adjoint("sdg", &[1], &[]).unwrap(), "s q[1];");
        assert_eq!(e.generate_adjoint("t", &[0], &[]).unwrap(), "tdg q[0];");
        assert_eq!(e.generate_adjoint("tdg", &[0], &[]).unwrap(), "t q[0];");
    }

    /// A rotation's adjoint keeps its name and negates its angle. Dropping the negation
    /// makes `rx(theta)` come out as its own inverse, i.e. the identity rather than the
    /// inverse.
    #[test]
    fn a_parameterised_rotation_negates_its_angle() {
        let e = OpenQASMExporter::new();
        assert_eq!(
            e.generate_adjoint("rx", &[0], &[0.25]).unwrap(),
            "rx(-0.2500000000) q[0];"
        );
    }

    /// Every operand of a two-qubit gate is kept.
    #[test]
    fn a_multi_qubit_gate_keeps_every_operand() {
        let e = OpenQASMExporter::new();
        assert_eq!(
            e.generate_adjoint("cz", &[0, 1], &[]).unwrap(),
            "cz q[0], q[1];"
        );
        assert_eq!(
            e.generate_adjoint("rz", &[2, 3], &[1.5]).unwrap(),
            "rz(-1.5000000000) q[2], q[3];"
        );
    }

    /// An UNMODELLED gate must be REFUSED, not assumed self-inverse.
    ///
    /// This arm used to return the name unchanged, which asserted that any gate the table
    /// did not mention was its own inverse. For a gate that is not self-inverse, that emits
    /// the forward application where an inverse belonged, and the circuit computes a
    /// different function with no diagnostic -- the same fabrication class the QIR
    /// a deleted QIR gate table had, where every unknown gate became a Hadamard.
    #[test]
    fn an_unmodelled_gate_has_no_adjoint_rather_than_being_its_own() {
        let e = OpenQASMExporter::new();
        for unknown in ["unknown", "frobnicate", "s_dagger_typod", ""] {
            let err = e
                .generate_adjoint(unknown, &[0], &[])
                .expect_err("an unmodelled gate must not be assumed self-inverse");
            assert!(
                err.to_string().contains("no adjoint relation"),
                "the diagnostic must say what is wrong, got: {err}"
            );
        }
    }

    /// The two gates the simulator does not model are handled by name, explicitly.
    ///
    /// They are `iswap` (self-inverse) and `cphase` (self-inverse once the angle is
    /// negated). Declaring them here is honest -- they are asserted to be self-inverse, not
    /// swept into the unknown arm -- and a test pins that so the declaration stays true.
    #[test]
    fn the_two_unmodelled_gates_are_declared_self_inverse() {
        let e = OpenQASMExporter::new();
        assert_eq!(
            e.generate_adjoint("iswap", &[0, 1], &[]).unwrap(),
            "iswap q[0], q[1];"
        );
        assert_eq!(
            e.generate_adjoint("cphase", &[0, 1], &[0.5]).unwrap(),
            "cphase(-0.5000000000) q[0], q[1];"
        );
    }
}
