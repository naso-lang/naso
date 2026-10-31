// syscalls.rs - Bare-metal quantum/classical traps for the Naso runtime
// Provides low-level system calls for memory allocation, barriers, and measurement.

/// Allocates a block of memory for a quantum object.
// In a real implementation, this might interface with a quantum memory allocator.
// For now, we'll use the system allocator via the QttArena (which would be set up by the runtime).
#[no_mangle]
pub extern "C" fn sys_qalloc(size: usize, align: usize) -> *mut u8 {
    // This is a placeholder. In reality, we would get the current QttArena from a thread-local
    // and allocate with quantity [1] (linear) or [*] depending on the object.
    // For simplicity, we'll return null to indicate that this function must be implemented
    // by the runtime or replaced with a proper allocator.
    core::ptr::null_mut()
}

/// Frees a block of memory allocated by sys_qalloc.
#[no_mangle]
pub extern "C" fn sys_qfree(ptr: *mut u8, size: usize, align: usize) {
    // Placeholder: in a linear allocator, free is a no-op until reset.
    // We do nothing here.
}

/// Issues a memory barrier to ensure ordering of memory operations.
#[no_mangle]
pub extern "C" fn sys_barrier() {
    // Placeholder: architecture-specific barrier instruction.
    // For example, on x86: _mm_mfence();
    // We'll use compiler fence for now.
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

/// Triggers a measurement trap (for quantum measurement that collapses the state).
/// This would typically involve interaction with a quantum processor or simulator.
#[no_mangle]
pub extern "C" fn sys_measure_trap(qubit_ptr: *mut u8) -> i32 {
    // Placeholder: return a random bit (0 or 1) for demonstration.
    // In reality, this would measure the qubit and return the outcome.
    use core::sync::atomic::{AtomicU32, Ordering};
    static SEED: AtomicU32 = AtomicU32::new(0);
    let seed = SEED.fetch_add(1, Ordering::Relaxed);
    // Simple LCG for demonstration
    let rand = ((seed.wrapping_mul(1664525)).wrapping_add(1013904223)) & 0x7fffffff;
    (rand % 2) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_qalloc_returns_null() {
        // This test just checks that the function exists and returns null (placeholder).
        let ptr = unsafe { sys_qalloc(1024, 8) };
        assert!(ptr.is_null());
    }

    #[test]
    fn test_sys_qfree_does_not_crash() {
        // Just call it to ensure it doesn't crash.
        unsafe { sys_qfree(core::ptr::null_mut(), 0, 0) };
    }

    #[test]
    fn test_sys_barrier_does_not_crash() {
        unsafe { sys_barrier() };
    }

    #[test]
    fn test_sys_measure_trap_returns_bit() {
        let result = unsafe { sys_measure_trap(core::ptr::null_mut()) };
        assert!(result == 0 || result == 1);
    }
}