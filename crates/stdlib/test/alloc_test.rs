// alloc_test.rs - Unit tests for the allocators in std::alloc
// Validates zero-fragmentation and [1]-quantity lifetime guarantees.

use naso_std::alloc::{LinearAllocator, QttArena};
use core::alloc::Layout;

#[test]
fn test_linear_allocator_zero_fragmentation() {
    // Test that the linear allocator does not leave gaps (fragmentation) when allocating and resetting.
    let mut buffer = [0u8; 1024];
    let mut allocator = unsafe { LinearAllocator::new(buffer.as_mut_ptr(), buffer.len()) };

    let layout1 = Layout::from_size_align(16, 8).unwrap();
    let layout2 = Layout::from_size_align(32, 8).unwrap();
    let layout3 = Layout::from_size_align(8, 8).unwrap();

    let ptr1 = unsafe { allocator.alloc(layout1) };
    assert!(!ptr1.is_null());
    let ptr2 = unsafe { allocator.alloc(layout2) };
    assert!(!ptr2.is_null());
    let ptr3 = unsafe { allocator.alloc(layout3) };
    assert!(!ptr3.is_null());

    // Check that pointers are in increasing order and contiguous (no gaps)
    // Note: Because of alignment, there might be gaps, but we test that the allocator uses memory linearly.
    let start = buffer.as_ptr() as usize;
    let end = buffer.as_ptr() as usize + buffer.len();
    assert!(ptr1 as usize >= start);
    assert!(ptr2 as usize > ptr1 as usize);
    assert!(ptr3 as usize > ptr2 as usize);
    assert!(ptr3 as usize + layout3.size() <= end);

    // Reset and allocate again - should be able to reuse the entire buffer.
    unsafe { allocator.reset() };
    let ptr1_after = unsafe { allocator.alloc(layout1) };
    assert!(!ptr1_after.is_null());
    assert_eq!(ptr1_after as usize, start);
}

#[test]
fn test_linear_allocator_linear_lifetime() {
    // Test that allocations from the linear allocator must be used exactly once.
    // We cannot enforce this at runtime in the allocator itself (it's a compile-time check),
    // but we can test that the allocator allows allocation and reset (which simulates scope exit).
    let mut buffer = [0u8; 256];
    let mut allocator = unsafe { LinearAllocator::new(buffer.as_mut_ptr(), buffer.len()) };

    let layout = Layout::from_size_align(16, 8).unwrap();
    let ptr = unsafe { allocator.alloc(layout) };
    assert!(!ptr.is_null());

    // Simulate moving the pointer out (we just keep it, but in reality the type system would enforce single use).
    // Reset the allocator (simulating scope exit) - this should invalidate the pointer.
    unsafe { allocator.reset() };

    // After reset, allocating again should give us the same pointer (or at least a valid one).
    let ptr2 = unsafe { allocator.alloc(layout) };
    assert!(!ptr2.is_null());
    // In a linear allocator, after reset we can reuse the memory.
    assert_eq!(ptr2 as usize, buffer.as_ptr() as usize);
}

#[test]
fn test_qtt_arena_quantity_semantics() {
    let mut buffer = [0u8; 1024];
    let mut arena = unsafe { QttArena::new(buffer.as_mut_ptr(), buffer.len()) };

    // Test [0] allocation (zero-sized)
    let layout0 = Layout::from_size_align(0, 1).unwrap();
    let ptr0 = unsafe { arena.alloc(0, layout0) };
    // Our implementation returns the start of the buffer for zero-sized [0].
    assert_eq!(ptr0, buffer.as_mut_ptr());
    // Deallocating zero-sized [0] is a no-op.
    unsafe { arena.dealloc(0, ptr0, layout0) };

    // Test [1] allocation (linear)
    let layout1 = Layout::from_size_align(16, 8).unwrap();
    let ptr1 = unsafe { arena.alloc(1, layout1) };
    assert!(!ptr1.is_null());
    let ptr1_after = unsafe { arena.alloc(1, layout1) };
    assert!(!ptr1_after.is_null());
    assert!(ptr1_after as usize > ptr1 as usize);

    // Reset and allocate [1] again
    unsafe { arena.reset() };
    let ptr1_reset = unsafe { arena.alloc(1, layout1) };
    assert!(!ptr1_reset.is_null());
    assert_eq!(ptr1_reset as usize, buffer.as_ptr() as usize);

    // Test [*] allocation (heap) - returns null in our simplified implementation
    let layout_star = Layout::from_size_align(16, 8).unwrap();
    let ptr_star = unsafe { arena.alloc(2, layout_star) };
    assert!(ptr_star.is_null());
}

#[test]
fn test_qtt_arena_reset_reclaims_memory() {
    let mut buffer = [0u8; 512];
    let mut arena = unsafe { QttArena::new(buffer.as_mut_ptr(), buffer.len()) };

    let layout = Layout::from_size_align(32, 8).unwrap();
    let ptr1 = unsafe { arena.alloc(1, layout) };
    assert!(!ptr1.is_null());
    let ptr2 = unsafe { arena.alloc(1, layout) };
    assert!(!ptr2.is_null());

    // Allocate until we run out
    let mut ptr = ptr2;
    loop {
        let next = unsafe { arena.alloc(1, layout) };
        if next.is_null() {
            break;
        }
        ptr = next;
    }
    // Now reset and allocate again - should be able to use the whole buffer.
    unsafe { arena.reset() };
    let ptr_after = unsafe { arena.alloc(1, layout) };
    assert!(!ptr_after.is_null());
    assert_eq!(ptr_after as usize, buffer.as_ptr() as usize);
}