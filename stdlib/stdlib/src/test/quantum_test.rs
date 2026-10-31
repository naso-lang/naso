use naso_std::prelude::*;
use naso_std::std::quantum::*;

#[test]
fn test_qalloc_qfree() {
    let qubit = qalloc();
    // Qubit should be allocated and ready to use
    let measured = measure(qubit);
    assert!(measured == false); // Should be |0> state
}

#[test]
fn test_hadamard() {
    let qubit = qalloc();
    let hadamard_qubit = hadamard(qubit);
    // Hadamard puts qubit in superposition
    let measured = measure(hadamard_qubit);
    // Measurement should be random 0 or 1 with 50% probability
    // We can't test the exact value due to randomness, but we can test it returns a bool
    let _ = measured; // Just to use the value
}

#[test]
fn test_cnot() {
    let control = qalloc();
    let target = qalloc();
    let (control_out, target_out) = cnot(control, target);
    // Both qubits should still be valid
    let _ = measure(control_out);
    let _ = measure(target_out);
}

#[test]
fn test_bell_pair() {
    // Create |00> state
    let q1 = qalloc();
    let q2 = qalloc();
    
    // Apply Hadamard to first qubit
    let q1_h = hadamard(q1);
    
    // Apply CNOT to create Bell state
    let (q1_final, q2_final) = cnot(q1_h, q2);
    
    // Measure both qubits - they should be correlated
    let m1 = measure(q1_final);
    let m2 = measure(q2_final);
    
    // In Bell state |00> + |11>, measurements should be equal
    assert!(m1 == m2);
}

#[test]
fn test_measurement_consumes_qubit() {
    let qubit = qalloc();
    let result = measure(qubit);
    // After measurement, qubit should be consumed
    // If we try to use it again, it should fail to compile
    // This test just verifies measurement returns a bool
    let _ = result;
}

#[test]
fn test_qft_simple() {
    let mut register = [qalloc(), qalloc()];
    let result = qft(&mut register);
    assert_eq!(result.len(), 2);
    // Just test that it compiles and returns the right type
    let _ = measure(result[0]);
    let _ = measure(result[1]);
}

#[test]
fn test_grover_oracle_simple() {
    let mut register = [qalloc(), qalloc()];
    let marked_state = [true, false]; // Mark |10> state
    let result = grover_oracle(&mut register, &marked_state);
    assert_eq!(result.len(), 2);
    // Just test that it compiles and returns the right type
    let _ = measure(result[0]);
    let _ = measure(result[1]);
}