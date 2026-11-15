use crate::std::prelude::*;

/// Create an oracle for Grover's algorithm that marks a specific state.
///
/// # Arguments
/// * `register` - The quantum register to apply the oracle to
/// * `marked_state` - The basis state to mark (as a bit string)
///
/// # Returns
/// * The same register with the oracle applied (linearity preserved)
#[inline(always)]
pub fn grover_oracle<'a>(register: &'a mut [Qubit], marked_state: &'a [bool]) -> &'a mut [Qubit] {
    assert_eq!(
        register.len(),
        marked_state.len(),
        "Register length must match marked state length"
    );

    // Apply X gates to qubits where the marked state has 0
    // This transforms the marked state to all-1s
    for (i, &bit) in marked_state.iter().enumerate() {
        if !bit {
            register[i] = pauli_x(register[i]);
        }
    }

    // Apply multi-controlled Z gate (which marks the |11...1> state)
    // For simplicity, we'll implement a simplified version
    // A full implementation would use ancilla qubits for efficiency
    apply_mczt_gate(register);

    // Apply X gates again to restore the original basis
    for (i, &bit) in marked_state.iter().enumerate() {
        if !bit {
            register[i] = pauli_x(register[i]);
        }
    }

    register
}

/// Apply Pauli-X gate (bit flip) to a qubit.
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn pauli_x(qubit: Qubit) -> Qubit {
    extern "C" {
        fn __quantum__qis__x__body(qubit: Qubit);
    }
    unsafe {
        __quantum__qis__x__body(qubit);
        qubit
    }
}

/// Apply multi-controlled Z gate to a register of qubits.
///
/// This is a simplified implementation that assumes all qubits are controls
/// and applies a Z gate to the last qubit if all previous qubits are |1>.
///
/// For a production implementation, this would use ancilla qubits and
/// decompose into elementary gates.
#[inline(always)]
fn apply_mczt_gate(register: &mut [Qubit]) -> &mut [Qubit] {
    if register.is_empty() {
        return register;
    }

    // For now, we'll just apply a Z gate to the last qubit as a placeholder
    // A proper implementation would check if all control qubits are |1>
    // This preserves linearity by consuming and returning the qubit
    let last_idx = register.len() - 1;
    register[last_idx] = pauli_z(register[last_idx]);
    register
}

/// Apply Pauli-Z gate (phase flip) to a qubit.
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn pauli_z(qubit: Qubit) -> Qubit {
    extern "C" {
        fn __quantum__qis__z__body(qubit: Qubit);
    }
    unsafe {
        __quantum__qis__z__body(qubit);
        qubit
    }
}

/// Apply Grover diffusion operator (inversion about the mean).
///
/// # Arguments
/// * `register` - The quantum register to apply the diffusion operator to
///
/// # Returns
/// * The same register with the diffusion operator applied (linearity preserved)
#[inline(always)]
pub fn grover_diffusion(register: &mut [Qubit]) -> &mut [Qubit] {
    // Apply Hadamard to all qubits
    for qubit in register.iter_mut() {
        *qubit = hadamard(*qubit);
    }

    // Apply oracle that marks |00...0> state (apply X gates, then MCZT, then X gates)
    for qubit in register.iter_mut() {
        *qubit = pauli_x(*qubit);
    }
    apply_mczt_gate(register);
    for qubit in register.iter_mut() {
        *qubit = pauli_x(*qubit);
    }

    // Apply Hadamard to all qubits again
    for qubit in register.iter_mut() {
        *qubit = hadamard(*qubit);
    }

    register
}
