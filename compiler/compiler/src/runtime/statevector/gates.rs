/// Quantum Gates for Statevector Simulator
///
/// Defines the quantum gate operations supported by the simulator,
/// including single-qubit and two-qubit gates with their matrix representations.
use num_complex::Complex64;

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

/// Standard gate set for common quantum algorithms
pub mod standard {
    use super::*;

    /// Get the Hadamard gate
    pub fn hadamard() -> SingleQubitGate {
        SingleQubitGate::H
    }

    /// Get the Pauli-X gate
    pub fn pauli_x() -> SingleQubitGate {
        SingleQubitGate::X
    }

    /// Get the Pauli-Y gate
    pub fn pauli_y() -> SingleQubitGate {
        SingleQubitGate::Y
    }

    /// Get the Pauli-Z gate
    pub fn pauli_z() -> SingleQubitGate {
        SingleQubitGate::Z
    }

    /// Get the S gate
    pub fn s_gate() -> SingleQubitGate {
        SingleQubitGate::S
    }

    /// Get the T gate
    pub fn t_gate() -> SingleQubitGate {
        SingleQubitGate::T
    }

    /// Get the CNOT gate
    pub fn cnot() -> TwoQubitGate {
        TwoQubitGate::CX
    }

    /// Get the CZ gate
    pub fn cz() -> TwoQubitGate {
        TwoQubitGate::CZ
    }

    /// Get the SWAP gate
    pub fn swap() -> TwoQubitGate {
        TwoQubitGate::SWAP
    }
}
