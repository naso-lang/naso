// stdlib/test/integration_harness.rs - Integration test harness for stdlib
// Validates the entire standard library works together across all backends.

use naso_std::std::quantum::{qalloc, qfree, hadamard, cnot, measure, bell_pair, qft, grover_oracle};
use naso_std::std::tensor::{Tensor, matmul, add, sub, scale, transpose, contract, outer_product};
use naso_std::std::sys::{sys_qalloc, sys_qfree, sys_barrier, sys_measure_trap};
use naso_std::std::alloc::{LinearAllocator, QttArena};

/// Run all integration tests
pub fn run_integration_tests() {
    test_quantum_algorithms();
    test_tensor_contractions();
    test_system_calls();
    test_memory_safety();
    println!("All integration tests passed!");
}

/// Test quantum algorithms using stdlib
fn test_quantum_algorithms() {
    println!("Testing quantum algorithms...");
    
    // Test basic quantum operations
    let qubits = qalloc(2);
    hadamard(qubits[0]);
    cnot(qubits[0], qubits[1]);
    let result = measure(qubits[0]);
    qfree(qubits);
    assert!(result == 0 || result == 1);
    
    // Test Bell pair creation
    let bell = bell_pair();
    let result0 = measure(bell[0]);
    let result1 = measure(bell[1]);
    assert_eq!(result0, result1); // Bell pair should be correlated
    qfree(bell);
    
    // Test QFT (Quantum Fourier Transform)
    let qft_qubits = qalloc(4);
    qft(&mut qft_qubits);
    let _ = measure(qft_qubits[0]);
    qfree(qft_qubits);
    
    // Test Grover oracle
    let oracle_qubits = qalloc(3);
    let oracle = grover_oracle(|x| x == 5);
    oracle(&oracle_qubits);
    qfree(oracle_qubits);
    
    println!("  Quantum algorithms: OK");
}

/// Test tensor operations using stdlib
fn test_tensor_contractions() {
    println!("Testing tensor contractions...");
    
    // Create test tensors
    let a = Tensor::new(vec![2, 3], vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let b = Tensor::new(vec![3, 2], vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    
    // Test matrix multiplication
    let c = matmul(&a, &b);
    assert_eq!(c.shape(), vec![2, 2]);
    
    // Test add/sub
    let a2 = Tensor::new(vec![2, 3], vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let sum = add(&a, &a2);
    assert_eq!(sum.data()[0], 2.0);
    
    // Test scale
    let scaled = scale(&a, 2.0);
    assert_eq!(scaled.data()[0], 2.0);
    
    // Test transpose
    let transposed = transpose(&a);
    assert_eq!(transposed.shape(), vec![3, 2]);
    
    // Test outer product
    let v1 = Tensor::new(vec![3], vec![1.0, 2.0, 3.0]);
    let v2 = Tensor::new(vec![2], vec![1.0, 2.0]);
    let outer = outer_product(&v1, &v2);
    assert_eq!(outer.shape(), vec![3, 2]);
    
    // Test contract
    let t1 = Tensor::new(vec![2, 3], vec![1.0; 6]);
    let t2 = Tensor::new(vec![3, 4], vec![1.0; 12]);
    let contracted = contract(&t1, &t2, &[(1, 0)]);
    assert_eq!(contracted.shape(), vec![2, 4]);
    
    println!("  Tensor contractions: OK");
}

/// Test system calls using stdlib
fn test_system_calls() {
    println!("Testing system calls...");
    
    // Test memory allocation
    let ptr = unsafe { sys_qalloc(1024, 8) };
    // In a real implementation, this would return a valid pointer
    // For now, we just ensure the function is callable
    unsafe { sys_qfree(ptr, 1024, 8) };
    
    // Test barrier
    sys_barrier();
    
    // Test measure trap
    let result = unsafe { sys_measure_trap(std::ptr::null_mut()) };
    assert!(result == 0 || result == 1);
    
    println!("  System calls: OK");
}

/// Test memory safety with linear allocators
fn test_memory_safety() {
    println!("Testing memory safety...");
    
    // Test LinearAllocator
    let mut buffer = [0u8; 1024];
    let mut allocator = unsafe { LinearAllocator::new(buffer.as_mut_ptr(), buffer.len()) };
    
    let layout = std::alloc::Layout::from_size_align(16, 8).unwrap();
    let ptr1 = unsafe { allocator.alloc(layout) };
    assert!(!ptr1.is_null());
    
    let ptr2 = unsafe { allocator.alloc(layout) };
    assert!(!ptr2.is_null());
    assert!(ptr2 as usize > ptr1 as usize);
    
    // Reset and reuse
    unsafe { allocator.reset() };
    let ptr3 = unsafe { allocator.alloc(layout) };
    assert!(!ptr3.is_null());
    assert_eq!(ptr3 as usize, buffer.as_ptr() as usize);
    
    // Test QttArena
    let mut buffer2 = [0u8; 1024];
    let mut arena = unsafe { QttArena::new(buffer2.as_mut_ptr(), buffer2.len()) };
    
    // [0] quantity
    let layout0 = std::alloc::Layout::from_size_align(0, 1).unwrap();
    let ptr0 = unsafe { arena.alloc(0, layout0) };
    unsafe { arena.dealloc(0, ptr0, layout0) };
    
    // [1] quantity
    let ptr1 = unsafe { arena.alloc(1, layout) };
    assert!(!ptr1.is_null());
    
    // Reset and reuse
    unsafe { arena.reset() };
    let ptr1_after = unsafe { arena.alloc(1, layout) };
    assert!(!ptr1_after.is_null());
    assert_eq!(ptr1_after as usize, buffer2.as_ptr() as usize);
    
    println!("  Memory safety: OK");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_integrations() {
        run_integration_tests();
    }
}