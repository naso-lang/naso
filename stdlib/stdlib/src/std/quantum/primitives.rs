use crate::std::prelude::*;

/// Allocate a new qubit in the |0⟩ state.
///
/// Returns a linear qubit ([1]) that must be consumed exactly once.
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn qalloc() -> Qubit {
    // This will lower to the appropriate QIR intrinsic
    extern "C" {
        fn __quantum__qubit__allocate() -> Qubit;
    }
    unsafe { __quantum__qubit__allocate() }
}

/// Deallocate a qubit, resetting it to |0⟩ before release.
///
/// Consumes the qubit exactly once.
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn qfree(qubit: Qubit) {
    // This will lower to the appropriate QIR intrinsic
    extern "C" {
        fn __quantum__qubit__release(qubit: Qubit);
    }
    unsafe { __quantum__qubit__release(qubit) }
}

/// Apply a Hadamard gate to a qubit.
///
/// The qubit is consumed and a new qubit is returned (linearity preserved).
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn hadamard(qubit: Qubit) -> Qubit {
    // This will lower to the appropriate QIR intrinsic
    extern "C" {
        fn __quantum__qis__h__body(qubit: Qubit);
    }
    unsafe {
        __quantum__qis__h__body(qubit);
        qubit // Return the same qubit (now transformed)
    }
}

/// Apply a CNOT gate with control and target qubits.
///
/// Both qubits are consumed and returned (linearity preserved).
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn cnot(control: Qubit, target: Qubit) -> (Qubit, Qubit) {
    // This will lower to the appropriate QIR intrinsic
    extern "C" {
        fn __quantum__qis__cnot__body(control: Qubit, target: Qubit);
    }
    unsafe {
        __quantum__qis__cnot__body(control, target);
        (control, target) // Return both qubits (now transformed)
    }
}

/// Measure a qubit in the computational basis.
///
/// Consumes the qubit and returns a classical bit (0 or 1).
#[inline(always)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn measure(qubit: Qubit) -> bool {
    // This will lower to the appropriate QIR intrinsic
    extern "C" {
        fn __quantum__qis__mz__body(qubit: Qubit) -> bool;
    }
    unsafe { __quantum__qis__mz__body(qubit) }
}
