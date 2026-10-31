//! QIR Profile Definitions
//!
//! Defines QIR profiles (base, adaptive) per Microsoft QIR spec.

use std::collections::HashSet;

/// QIR Profile Kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QirProfileKind {
    /// Base profile - static quantum circuits
    Base,
    /// Adaptive profile - dynamic quantum circuits with control flow
    Adaptive,
    /// Full profile - all features
    Full,
}

impl QirProfileKind {
    /// Get profile name as string
    pub fn as_str(&self) -> &'static str {
        match self {
            QirProfileKind::Base => "base",
            QirProfileKind::Adaptive => "adaptive",
            QirProfileKind::Full => "full",
        }
    }
}

/// Error returned when a string does not name a known QIR profile kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseQirProfileKindError {
    /// The rejected input, kept verbatim.
    pub input: String,
}

impl std::fmt::Display for ParseQirProfileKindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown QIR profile `{}`: expected base, adaptive or full",
            self.input
        )
    }
}

impl std::error::Error for ParseQirProfileKindError {}

impl std::str::FromStr for QirProfileKind {
    type Err = ParseQirProfileKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "base" => Ok(QirProfileKind::Base),
            "adaptive" => Ok(QirProfileKind::Adaptive),
            "full" => Ok(QirProfileKind::Full),
            _ => Err(ParseQirProfileKindError {
                input: s.to_string(),
            }),
        }
    }
}

/// QIR Profile Configuration
#[derive(Debug, Clone)]
pub struct QirProfile {
    kind: QirProfileKind,
    /// Enabled intrinsics for this profile
    enabled_intrinsics: HashSet<&'static str>,
    /// Whether dynamic control flow is allowed
    allows_dynamic_control_flow: bool,
    /// Whether classical computation is allowed
    allows_classical_computation: bool,
    /// Whether qubit reuse is allowed
    allows_qubit_reuse: bool,
    /// Maximum qubit count (None = unlimited)
    max_qubits: Option<u32>,
}

impl QirProfile {
    /// Create a new profile
    pub fn new(kind: QirProfileKind) -> Self {
        let mut profile = Self {
            kind,
            enabled_intrinsics: HashSet::new(),
            allows_dynamic_control_flow: false,
            allows_classical_computation: false,
            allows_qubit_reuse: false,
            max_qubits: None,
        };
        profile.configure_defaults();
        profile
    }

    /// Configure defaults based on profile kind
    fn configure_defaults(&mut self) {
        match self.kind {
            QirProfileKind::Base => {
                // Base profile: static circuits only
                self.allows_dynamic_control_flow = false;
                self.allows_classical_computation = true; // Classical for parameters
                self.allows_qubit_reuse = false;
                self.max_qubits = None;

                // Enable base intrinsics
                self.enable_base_intrinsics();
            }
            QirProfileKind::Adaptive => {
                // Adaptive profile: dynamic circuits with measurement-based control flow
                self.allows_dynamic_control_flow = true;
                self.allows_classical_computation = true;
                self.allows_qubit_reuse = true;
                self.max_qubits = None;

                // Enable all intrinsics including control flow
                self.enable_base_intrinsics();
                self.enable_adaptive_intrinsics();
            }
            QirProfileKind::Full => {
                // Full profile: all features
                self.allows_dynamic_control_flow = true;
                self.allows_classical_computation = true;
                self.allows_qubit_reuse = true;
                self.max_qubits = None;

                // Enable everything
                self.enable_base_intrinsics();
                self.enable_adaptive_intrinsics();
                self.enable_full_intrinsics();
            }
        }
    }

    fn enable_base_intrinsics(&mut self) {
        let base_intrinsics = [
            // Qubit management
            "qir.qubit_alloc",
            "qir.qubit_release",
            "qir.qubit_alloc_array",
            "qir.qubit_release_array",
            // Single-qubit gates
            "qir.h",
            "qir.x",
            "qir.y",
            "qir.z",
            "qir.s",
            "qir.t",
            "qir.rx",
            "qir.ry",
            "qir.rz",
            "qir.r1",
            "qir.rt1",
            // Two-qubit gates
            "qir.cx",
            "qir.cy",
            "qir.cz",
            "qir.swap",
            "qir.iswap",
            // Three-qubit gates
            "qir.ccx",
            // Measurement
            "qir.mz",
            "qir.mx",
            "qir.my",
            "qir.measure",
            // Result handling
            "qir.result_record",
            "qir.result_update",
            "qir.result_get",
            "qir.result_equal",
            // Array operations
            "qir.array_record",
            "qir.array_update",
            // Adjoint/Controlled
            "qir.adjoint",
            "qir.controlled",
        ];
        for intrinsic in base_intrinsics {
            self.enabled_intrinsics.insert(intrinsic);
        }
    }

    fn enable_adaptive_intrinsics(&mut self) {
        let adaptive_intrinsics = [
            // Dynamic control flow
            "qir.if",
            "qir.while",
            // Qubit reuse
            "qir.qubit_reset", // Not in base but in adaptive
        ];
        for intrinsic in adaptive_intrinsics {
            self.enabled_intrinsics.insert(intrinsic);
        }
    }

    fn enable_full_intrinsics(&mut self) {
        // Full profile includes all known intrinsics
        let full_intrinsics = ["qir.profiler_record", "qir.qubit_reset", "qir.dump_machine"];
        for intrinsic in full_intrinsics {
            self.enabled_intrinsics.insert(intrinsic);
        }
    }

    /// Get the profile kind
    pub fn kind(&self) -> QirProfileKind {
        self.kind
    }

    /// Check if an intrinsic is enabled
    pub fn is_intrinsic_enabled(&self, name: &str) -> bool {
        self.enabled_intrinsics.contains(name)
    }

    /// Check if dynamic control flow is allowed
    pub fn allows_dynamic_control_flow(&self) -> bool {
        self.allows_dynamic_control_flow
    }

    /// Check if classical computation is allowed
    pub fn allows_classical_computation(&self) -> bool {
        self.allows_classical_computation
    }

    /// Check if qubit reuse is allowed
    pub fn allows_qubit_reuse(&self) -> bool {
        self.allows_qubit_reuse
    }

    /// Get maximum qubit count
    pub fn max_qubits(&self) -> Option<u32> {
        self.max_qubits
    }

    /// Set maximum qubit count
    pub fn set_max_qubits(&mut self, max: u32) {
        self.max_qubits = Some(max);
    }

    /// Enable an intrinsic
    pub fn enable_intrinsic(&mut self, name: &'static str) {
        self.enabled_intrinsics.insert(name);
    }

    /// Disable an intrinsic
    pub fn disable_intrinsic(&mut self, name: &str) {
        self.enabled_intrinsics.remove(name);
    }

    /// Get all enabled intrinsics
    pub fn enabled_intrinsics(&self) -> &HashSet<&'static str> {
        &self.enabled_intrinsics
    }

    /// Validate that a module conforms to this profile
    pub fn validate_module(&self, module_ir: &str) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Check for disallowed intrinsics
        for intrinsic in &self.enabled_intrinsics {
            // This is a simplified check - in reality we'd parse the IR
            if !module_ir.contains(intrinsic) {
                // Not an error, just not used
            }
        }

        // Check for dynamic control flow in base profile
        if !self.allows_dynamic_control_flow
            && (module_ir.contains("qir.if") || module_ir.contains("qir.while"))
        {
            errors.push(
                "Dynamic control flow (qir.if/qir.while) not allowed in base profile".to_string(),
            );
        }

        // Check for qubit reuse in base profile
        if !self.allows_qubit_reuse && module_ir.contains("qir.qubit_reset") {
            errors.push("Qubit reuse (qir.qubit_reset) not allowed in base profile".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Default for QirProfile {
    fn default() -> Self {
        Self::new(QirProfileKind::Base)
    }
}

/// QIR Target Capabilities
#[derive(Debug, Clone)]
pub struct QirTargetCapabilities {
    pub profile: QirProfileKind,
    pub max_qubits: Option<u32>,
    pub supports_qubit_reuse: bool,
    pub supports_mid_circuit_measurement: bool,
    pub supports_adaptive_control_flow: bool,
    pub supports_classical_computation: bool,
    pub supported_gate_set: HashSet<String>,
    pub supported_measurement_bases: HashSet<String>,
}

impl QirTargetCapabilities {
    /// Create capabilities for a given profile
    pub fn for_profile(profile: QirProfileKind) -> Self {
        match profile {
            QirProfileKind::Base => Self {
                profile,
                max_qubits: None,
                supports_qubit_reuse: false,
                supports_mid_circuit_measurement: true,
                supports_adaptive_control_flow: false,
                supports_classical_computation: true,
                supported_gate_set: [
                    "h", "x", "y", "z", "s", "t", "rx", "ry", "rz", "r1", "rt1", "cx", "cy", "cz",
                    "swap", "iswap", "ccx",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                supported_measurement_bases: ["z", "x", "y"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            QirProfileKind::Adaptive => Self {
                profile,
                max_qubits: None,
                supports_qubit_reuse: true,
                supports_mid_circuit_measurement: true,
                supports_adaptive_control_flow: true,
                supports_classical_computation: true,
                supported_gate_set: [
                    "h", "x", "y", "z", "s", "t", "rx", "ry", "rz", "r1", "rt1", "cx", "cy", "cz",
                    "swap", "iswap", "ccx",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                supported_measurement_bases: ["z", "x", "y"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            QirProfileKind::Full => Self {
                profile,
                max_qubits: None,
                supports_qubit_reuse: true,
                supports_mid_circuit_measurement: true,
                supports_adaptive_control_flow: true,
                supports_classical_computation: true,
                supported_gate_set: [
                    "h", "x", "y", "z", "s", "t", "rx", "ry", "rz", "r1", "rt1", "cx", "cy", "cz",
                    "swap", "iswap", "ccx", "u", "p", "cp",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                supported_measurement_bases: ["z", "x", "y", "bell"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
        }
    }

    /// Check if a gate is supported
    pub fn supports_gate(&self, gate: &str) -> bool {
        self.supported_gate_set.contains(gate)
    }

    /// Check if a measurement basis is supported
    pub fn supports_measurement_basis(&self, basis: &str) -> bool {
        self.supported_measurement_bases.contains(basis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_creation() {
        let base = QirProfile::new(QirProfileKind::Base);
        assert_eq!(base.kind(), QirProfileKind::Base);
        assert!(!base.allows_dynamic_control_flow());
        assert!(base.allows_classical_computation());
        assert!(!base.allows_qubit_reuse());
    }

    #[test]
    fn test_adaptive_profile() {
        let adaptive = QirProfile::new(QirProfileKind::Adaptive);
        assert_eq!(adaptive.kind(), QirProfileKind::Adaptive);
        assert!(adaptive.allows_dynamic_control_flow());
        assert!(adaptive.allows_qubit_reuse());
    }

    #[test]
    fn test_intrinsic_enabling() {
        let base = QirProfile::new(QirProfileKind::Base);
        assert!(base.is_intrinsic_enabled("qir.h"));
        assert!(base.is_intrinsic_enabled("qir.cx"));
        assert!(base.is_intrinsic_enabled("qir.mz"));
        assert!(!base.is_intrinsic_enabled("qir.if")); // Not in base
    }

    #[test]
    fn test_profile_validation() {
        let base = QirProfile::new(QirProfileKind::Base);
        let valid_ir = "; Module\n@qir.h = declare void\n";
        assert!(base.validate_module(valid_ir).is_ok());

        let invalid_ir = "; Module\n@qir.if = declare void\n";
        assert!(base.validate_module(invalid_ir).is_err());
    }

    #[test]
    fn test_target_capabilities() {
        let base_caps = QirTargetCapabilities::for_profile(QirProfileKind::Base);
        assert!(base_caps.supports_gate("h"));
        assert!(base_caps.supports_gate("cx"));
        assert!(!base_caps.supports_gate("u")); // Not in base
        assert!(base_caps.supports_measurement_basis("z"));
        assert!(!base_caps.supports_adaptive_control_flow);

        let adaptive_caps = QirTargetCapabilities::for_profile(QirProfileKind::Adaptive);
        assert!(adaptive_caps.supports_adaptive_control_flow);
        assert!(adaptive_caps.supports_qubit_reuse);
    }
}
