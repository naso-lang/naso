//! Reversible Operation Classifier
//!
//! Categorizes PIR statements as natively reversible (CNOT, Toffoli, permutation)
//! vs. non-reversible (measurement, RNG, T-gate requiring ancilla).
//!
//! Per Microsoft QIR spec, reversible operations must preserve unitary structure.
//! Non-reversible ops require ancilla qubits or are lowered differently.

use crate::ir::pir_types::{PirExpr, PirStatement, Quantity, Mutability};
use std::collections::HashSet;

/// Classification of a reversible operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReversibleClass {
    /// Natively reversible unitary (Hadamard, Pauli, CNOT, Toffoli, etc.)
    NativeUnitary,
    /// Permutation / swap operation
    Permutation,
    /// Measurement (forward/reverse handled specially)
    Measurement,
    /// Non-reversible - requires ancilla or is impure
    NonReversible,
    /// Conditional / classical control flow
    ClassicalControl,
}

/// Classify a PirStatement into a reversible class
pub fn classify_statement(stmt: &PirStatement) -> ReversibleClass {
    // Examine the body expression for reversible patterns
    let body_class = classify_expr(&stmt.body);

    // If any part is non-reversible, classify accordingly
    let inverse_class = classify_expr(&stmt.inverse);

    // Determine final classification
    match (body_class, inverse_class) {
        (ReversibleClass::NativeUnitary, ReversibleClass::NativeUnitary) => {
            // Body and inverse are both native unitaries - fully reversible
            ReversibleClass::NativeUnitary
        }
        (ReversibleClass::Permutation, ReversibleClass::Permutation) => {
            ReversibleClass::Permutation
        }
        (ReversibleClass::Measurement, ReversibleClass::Measurement) => {
            ReversibleClass::Measurement
        }
        (ReversibleClass::ClassicalControl, ReversibleClass::ClassicalControl) => {
            ReversibleClass::ClassicalControl
        }
        _ => {
            // Mixed or non-reversible components
            ReversibleClass::NonReversible
        }
    }
}

/// Classify a PirExpr into a reversible class
fn classify_expr(expr: &PirExpr) -> ReversibleClass {
    match expr {
        PirExpr::Reversible { body, inverse } => {
            // Recurse into nested reversible blocks
            let body_c = classify_expr(body);
            let inv_c = classify_expr(inverse);
            // If both inner are native unitaries, this is native unitary
            if body_c == ReversibleClass::NativeUnitary && inv_c == ReversibleClass::NativeUnitary {
                ReversibleClass::NativeUnitary
            } else if body_c == ReversibleClass::Measurement && inv_c == ReversibleClass::Measurement {
                ReversibleClass::Measurement
            } else {
                ReversibleClass::NonReversible
            }
        }
        PirExpr::Binary { op: _, left: _, right: _ } => {
            // Binary ops are generally non-reversible unless they're just permuting qubit states
            ReversibleClass::NonReversible
        }
        PirExpr::Unary { op: _, expr: _ } => {
            // Unary ops - could be reversible depending on context
            ReversibleClass::NativeUnitary
        }
        PirExpr::Call { name, args: _ } => {
            // Function calls - check known reversible primitives
            classify_builtin(name)
        }
        PirExpr::IntLit(_) | PirExpr::FloatLit(_) | PirExpr::BoolLit(_) | PirExpr::Var(_) => {
            // Literals and variables are non-reversible (they carry classical data)
            ReversibleClass::NonReversible
        }
        PirExpr::If { cond: _, then_branch: _, else_branch: _ } => {
            ReversibleClass::ClassicalControl
        }
        PirExpr::Index { .. } | PirExpr::Field { .. } => {
            ReversibleClass::NonReversible
        }
    }
}

/// Classify a function call based on the name
fn classify_builtin(name: &str) -> ReversibleClass {
    match name.to_lowercase().as_str() {
        // Native single-qubit gates - unitary
        "h" | "hadamard" | "x" | "pauli_x" | "y" | "pauli_y" | "z" | "pauli_z" | "s" | "sdg" | "t" | "tdg" | "rx" | "ry" | "rz" => {
            ReversibleClass::NativeUnitary
        }
        // Two-qubit entangling gates
        "cx" | "cnot" | "cy" | "cz" => ReversibleClass::NativeUnitary,
        "ccx" | "toffoli" => ReversibleClass::NativeUnitary,
        "swap" => ReversibleClass::Permutation,
        // Measurement
        "measure" | "mz" | "mx" | "my" => ReversibleClass::Measurement,
        // Non-reversible: probabilistic, RNG, state prep
        "rng" | "random" | "prepare_state" | "initialize" => ReversibleClass::NonReversible,
        // Adjoint/controlled are reversible
        "adjoint" | "controlled" => ReversibleClass::NativeUnitary,
        // Classical operations
        "if" | "else" | "br" => ReversibleClass::ClassicalControl,
        // Default: unknown call is non-reversible
        _ => ReversibleClass::NonReversible,
    }
}

/// Get the QIR intrinsic name for a reversible operation
pub fn qir_intrinsic_name(class: ReversibleClass, gate_name: &str) -> &'static str {
    match class {
        ReversibleClass::NativeUnitary => match gate_name.to_lowercase().as_str() {
            "h" => "qir.h",
            "x" => "qir.x",
            "y" => "qir.y",
            "z" => "qir.z",
            "s" => "qir.s",
            "t" => "qir.t",
            // S-dagger and T-dagger are the ADJOINTS of S and T: distinct QIR
            // entry points, not the same intrinsic with different arguments.
            //
            // S = diag(1, i) and S-dagger = diag(1, -i), so applying S where S-dagger
            // was written sends |1> to i|1> instead of -i|1>. Every downstream amplitude
            // differs. The previous mapping here did exactly that.
            "sdg" => "qir.s__adj",
            "tdg" => "qir.t__adj",
            "rx" => "qir.rx",
            "ry" => "qir.ry",
            "rz" => "qir.rz",
            "r1" => "qir.r1",
            "rt1" => "qir.rt1",
            "cx" => "qir.cx",
            "cnot" => "qir.cx",
            "cy" => "qir.cy",
            "cz" => "qir.cz",
            "swap" => "qir.swap",
            "ccx" | "toffoli" => "qir.ccx",
            // Measurement
            "measure" | "mz" => "qir.mz",
            "mx" => "qir.mx",
            "my" => "qir.my",
            // An unknown gate has no intrinsic. Returning "qir.h" made every unrecognised
            // gate a Hadamard, which computes a specific wrong answer rather than
            // refusing -- the silent-fabrication failure this file must not have. A
            // mis-spelled gate name produced working code applying the wrong unitary.
            _ => "qir.unknown", // not a real entry point; callers must refuse this
        },
        ReversibleClass::Permutation => "qir.swap",
        ReversibleClass::Measurement => "qir.mz",
        ReversibleClass::ClassicalControl => "qir.if",
        // A non-reversible operation has NO unitary intrinsic, and answering "qir.h"
        // made every one of them a Hadamard. Same fabrication as the unknown-gate arm
        // above: a specific wrong answer instead of a refusal.
        ReversibleClass::NonReversible => "qir.non_reversible",
    }
}

/// Check if a statement is natively reversible (can be emitted as pure QIR intrinsics)
pub fn is_natively_reversible(stmt: &PirStatement) -> bool {
    let cls = classify_statement(stmt);
    matches!(cls, ReversibleClass::NativeUnitary | ReversibleClass::Permutation | ReversibleClass::Measurement)
}

/// Check if a statement requires ancilla qubit management
pub fn requires_ancilla_management(stmt: &PirStatement) -> bool {
    let cls = classify_statement(stmt);
    // Measurement and non-reversible ops need ancilla management
    matches!(cls, ReversibleClass::Measurement | ReversibleClass::NonReversible) ||
    name_needs_ancilla(stmt)
}

/// Check if the statement name specifically requires ancilla
fn name_needs_ancilla(stmt: &PirStatement) -> bool {
    let name = match &stmt.body {
        PirExpr::Call { name, .. } => name.to_lowercase().as_str(),
        _ => "",
    };
    matches!(name, "ccx" | "toffoli" | "mz" | "measure" | "rng" | "random")
}

/// Get ancilla requirements for a reversible statement
/// Returns (num_ancilla, needs_explicit_release)
pub fn ancilla_requirements(stmt: &PirStatement) -> (u32, bool) {
    let cls = classify_statement(stmt);

    match cls {
        ReversibleClass::NativeUnitary => (0, false), // No ancilla needed for single-qubit + CNOT
        ReversibleClass::Permutation => (0, false),
        ReversibleClass::Measurement => (1, true),  // Measurement needs ancilla for inverse
        ReversibleClass::ClassicalControl => (0, false),
        ReversibleClass::NonReversible => {
            // Estimate based on function name
            let name = match &stmt.body {
                PirExpr::Call { name, .. } => name.to_lowercase().as_str(),
                _ => "",
            };
            match name {
                "ccx" | "toffoli" => (1, true),   // Toffoli needs 1 ancilla
                "rng" | "random" => (2, true),   // RNG needs 2 ancilla
                _ => (1, true),                    // Default: 1 ancilla
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::pir_types::Quantity;

    #[test]
    fn test_classify_h_gate() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "h".to_string(), args: vec![] },
            quantity: Quantity::One,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        assert_eq!(classify_statement(&stmt), ReversibleClass::NativeUnitary);
    }

    #[test]
    fn test_classify_measurement() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "mz".to_string(), args: vec![] },
            quantity: Quantity::One,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        assert_eq!(classify_statement(&stmt), ReversibleClass::Measurement);
    }

    #[test]
    fn test_classify_nonreversible_rng() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "rng".to_string(), args: vec![] },
            quantity: Quantity::Many,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        assert_eq!(classify_statement(&stmt), ReversibleClass::NonReversible);
    }

    #[test]
    fn test_is_natively_reversible_h() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "h".to_string(), args: vec![] },
            quantity: Quantity::One,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        assert!(is_natively_reversible(&stmt));
    }

    #[test]
    fn test_is_natively_reversible_measurement() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "mz".to_string(), args: vec![] },
            quantity: Quantity::One,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        assert!(!is_natively_reversible(&stmt)); // Measurement needs special handling
    }

    #[test]
    fn test_qir_intrinsic_name() {
        let h_name = qir_intrinsic_name(ReversibleClass::NativeUnitary, "h");
        assert_eq!(h_name, "qir.h");

        let cx_name = qir_intrinsic_name(ReversibleClass::NativeUnitary, "cx");
        assert_eq!(cx_name, "qir.cx");

        let swap_name = qir_intrinsic_name(ReversibleClass::Permutation, "swap");
        assert_eq!(swap_name, "qir.swap");
    }

    #[test]
    fn test_ancilla_requirements_h() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "h".to_string(), args: vec![] },
            quantity: Quantity::One,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        let (ancilla, needs_release) = ancilla_requirements(&stmt);
        assert_eq!(ancilla, 0);
        assert!(!needs_release);
    }

    #[test]
    fn test_ancilla_requirements_measurement() {
        let stmt = PirStatement {
            id: 0,
            domain: crate::ir::affine_domain::AffineDomain::universe(1, 0),
            body: PirExpr::Call { name: "mz".to_string(), args: vec![] },
            quantity: Quantity::One,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        let (ancilla, needs_release) = ancilla_requirements(&stmt);
        assert_eq!(ancilla, 1);
        assert!(needs_release);
    }
}