//! Hardware Backend Exporter
//!
//! Provides a unified trait for exporting QIR modules to various hardware backends
//! including OpenQASM 3.0, Amazon Braket, and other quantum hardware targets.

use crate::ast::Quantity;

#[cfg(feature = "llvm")]
mod braket;
#[cfg(feature = "llvm")]
mod openqasm;

#[cfg(feature = "llvm")]
use crate::codegen::qir::QIRModule;
use thiserror::Error;

/// Target backend for hardware export
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetBackend {
    /// OpenQASM 3.0 standard
    OpenQASM3,
    /// Amazon Braket
    Braket,
    /// IBM Quantum (future)
    IBMQuantum,
    /// IonQ (future)
    IonQ,
    /// Rigetti (future)
    Rigetti,
}

impl std::fmt::Display for TargetBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetBackend::OpenQASM3 => write!(f, "openqasm3"),
            TargetBackend::Braket => write!(f, "braket"),
            TargetBackend::IBMQuantum => write!(f, "ibm-quantum"),
            TargetBackend::IonQ => write!(f, "ionq"),
            TargetBackend::Rigetti => write!(f, "rigetti"),
        }
    }
}

impl std::str::FromStr for TargetBackend {
    type Err = ExporterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openqasm3" | "openqasm" | "qasm3" => Ok(TargetBackend::OpenQASM3),
            "braket" | "amazon-braket" => Ok(TargetBackend::Braket),
            "ibm" | "ibmq" | "ibm-quantum" => Ok(TargetBackend::IBMQuantum),
            "ionq" => Ok(TargetBackend::IonQ),
            "rigetti" => Ok(TargetBackend::Rigetti),
            _ => Err(ExporterError::UnsupportedBackend(s.to_string())),
        }
    }
}

/// Exported circuit metadata
#[derive(Debug, Clone, Default)]
pub struct ExportMetadata {
    /// Number of qubits in the circuit
    pub qubit_count: usize,
    /// Total gate count
    pub gate_count: usize,
    /// Gate depth (longest path)
    pub gate_depth: usize,
    /// Measurement count
    pub measurement_count: usize,
    /// Operation counts by type
    pub operation_counts: std::collections::HashMap<String, usize>,
    /// Qubit quantities mapping
    pub qubit_quantities: Vec<Quantity>,
    /// Classical bit count
    pub classical_bit_count: usize,
}

/// Export result containing the output and metadata
#[derive(Debug, Clone)]
pub struct ExportResult {
    /// Exported circuit as string (OpenQASM) or JSON (Braket)
    pub output: String,
    /// Export metadata
    pub metadata: ExportMetadata,
    /// Target backend used
    pub backend: TargetBackend,
}

/// Hardware exporter error types
#[derive(Debug, Error)]
pub enum ExporterError {
    #[error("Unsupported backend: {0}")]
    UnsupportedBackend(String),
    #[error("Invalid QIR module: {0}")]
    InvalidModule(String),
    #[error("Export failed: {0}")]
    ExportFailed(String),
    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),
    #[error("Quantity violation during export: {0}")]
    QuantityViolation(String),
}

#[cfg(feature = "llvm")]
/// Trait for hardware exporters
pub trait HardwareExporter {
    /// Export a QIR module to the target backend format
    fn export(&self, module: &QIRModule) -> Result<ExportResult, ExporterError>;

    /// Get the target backend
    fn target_backend(&self) -> TargetBackend;

    /// Get supported gate set for this backend
    fn supported_gates(&self) -> &'static [&'static str];

    /// Check if a gate is natively supported
    fn supports_gate(&self, gate: &str) -> bool {
        self.supported_gates().contains(&gate)
    }

    /// Get default gate decomposition rules for unsupported gates
    fn decomposition_rules(&self) -> Vec<DecompositionRule> {
        Vec::new()
    }
}

#[cfg(feature = "llvm")]
/// Gate decomposition rule
#[derive(Debug, Clone)]
pub struct DecompositionRule {
    /// Source gate name
    pub source_gate: String,
    /// Target gates to decompose into
    pub target_gates: Vec<String>,
    /// Whether this decomposition is exact or approximate
    pub is_exact: bool,
    /// Approximation error (if approximate)
    pub error_bound: Option<f64>,
}

#[cfg(feature = "llvm")]
/// Unified exporter factory
pub struct ExporterFactory;

#[cfg(feature = "llvm")]
impl ExporterFactory {
    /// Create an exporter for the given backend
    pub fn create(backend: TargetBackend) -> Box<dyn HardwareExporter> {
        match backend {
            TargetBackend::OpenQASM3 => {
                Box::new(crate::runtime::exporter::openqasm::OpenQASMExporter::new())
            }
            TargetBackend::Braket => {
                Box::new(crate::runtime::exporter::braket::BraketExporter::new())
            }
            _ => Box::new(UnsupportedExporter::new(backend)),
        }
    }

    /// Export using the factory (convenience method)
    pub fn export(
        module: &QIRModule,
        backend: TargetBackend,
    ) -> Result<ExportResult, ExporterError> {
        let exporter = Self::create(backend);
        exporter.export(module)
    }
}

#[cfg(feature = "llvm")]
/// Fallback exporter for unsupported backends
struct UnsupportedExporter {
    backend: TargetBackend,
}

#[cfg(feature = "llvm")]
impl UnsupportedExporter {
    fn new(backend: TargetBackend) -> Self {
        Self { backend }
    }
}

#[cfg(feature = "llvm")]
impl HardwareExporter for UnsupportedExporter {
    fn export(&self, _module: &QIRModule) -> Result<ExportResult, ExporterError> {
        Err(ExporterError::UnsupportedBackend(self.backend.to_string()))
    }

    fn target_backend(&self) -> TargetBackend {
        self.backend
    }

    fn supported_gates(&self) -> &'static [&'static str] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_parsing() {
        assert_eq!(
            "openqasm3".parse::<TargetBackend>().unwrap(),
            TargetBackend::OpenQASM3
        );
        assert_eq!(
            "braket".parse::<TargetBackend>().unwrap(),
            TargetBackend::Braket
        );
        assert_eq!(
            "ibm".parse::<TargetBackend>().unwrap(),
            TargetBackend::IBMQuantum
        );
        assert_eq!(
            "ionq".parse::<TargetBackend>().unwrap(),
            TargetBackend::IonQ
        );
        assert!("unknown".parse::<TargetBackend>().is_err());
    }

    #[test]
    fn test_backend_display() {
        assert_eq!(TargetBackend::OpenQASM3.to_string(), "openqasm3");
        assert_eq!(TargetBackend::Braket.to_string(), "braket");
    }
}
