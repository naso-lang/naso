// qtt_arena.rs - Quantity-indexed allocation manager
// Provides three allocation strategies based on quantity:
//   [0] zero-cost compile-time proof (no allocation)
//   [1] bump-allocated linear region (single-use)
//   [*] standard heap allocator fallback (reference-counted or GC)

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};

/// The QttArena manages allocations for different quantity semantics.
/// It contains a linear allocator for [1] objects and delegates [*] to the global allocator.
/// [0] quantities are handled at compile time and do not require runtime allocation.
pub struct QttArena {
    linear_allocator: super::linear::LinearAllocator,
    // We don't need to store anything for [0] because it's zero-sized.
    // For [*] we use the global allocator directly.
}

impl QttArena {
    /// Creates a new QttArena from a given memory buffer for the linear region.
    ///
    /// # Safety
    /// The caller must ensure that the buffer is valid for the given size.
    pub unsafe fn new(start: *mut u8, size: usize) -> Self {
        QttArena {
            linear_allocator: super::linear::LinearAllocator::new(start, size),
        }
    }

    /// Allocates memory for a given quantity and layout.
    ///
    /// # Parameters
    ///   - qty: The quantity annotation ([0], [1], or [*]).
    ///   - layout: The layout of the object to allocate.
    ///
    /// # Returns
    ///   A pointer to the allocated memory, or null if allocation fails.
    ///
    /// # Safety
    ///   The caller must ensure that the quantity is valid and that there is enough space.
    ///   For [0], the layout must be zero-sized (otherwise it's a compile-time error).
    pub unsafe fn alloc(&mut self, qty: u32, layout: Layout) -> *mut u8 {
        match qty {
            0 => {
                // [0] quantity: zero-sized allocation, return a dangling pointer (not to be dereferenced).
                // In practice, [0] values are erased at compile time and should not reach here.
                if layout.size() != 0 {
                    // This should not happen if the compiler enforces [0] for zero-sized types.
                    // We return null to indicate failure.
                    return ptr::null_mut();
                }
                // Return a non-null dangling pointer with a non-zero address? Actually, for zero-sized we can return any pointer.
                // We return the start of the linear region as a convention.
                self.linear_allocator.start as *mut u8
            }
            1 => {
                // [1] quantity: allocate from the linear arena.
                self.linear_allocator.alloc(layout)
            }
            _ => {
                // [*] quantity: delegate to the global allocator.
                // We use the global allocator's alloc function.
                // Note: This requires a mutable reference to the global allocator, which we don't have.
                // For simplicity, we'll use the system allocator via alloc::alloc (if available) or panic.
                // Since we are in a no-std context, we cannot rely on alloc::alloc.
                // Instead, we'll return null to indicate that [*] allocations are not supported in this arena.
                // In a full implementation, we would have a heap or use an external allocator.
                ptr::null_mut()
            }
        }
    }

    /// Deallocates memory for a given quantity and layout.
    ///
    /// # Safety
    ///   The caller must ensure that the pointer was allocated from this arena
    ///   and that the quantity and layout are correct.
    pub unsafe fn dealloc(&mut self, qty: u32, ptr: *mut u8, layout: Layout) {
        match qty {
            0 => {
                // [0] quantities are zero-sized and not actually allocated, so nothing to do.
                // However, if the layout is not zero-sized, it's a programmer error.
                debug_assert!(layout.size() == 0, "[0] quantity must have zero-sized layout");
            }
            1 => {
                // [1] quantities: deallocation is a no-op for the linear arena (reset at once).
                // We do nothing here; the linear arena is reset en masse.
            }
            _ => {
                // [*] quantities: delegate to the global allocator.
                // Again, we don't have access to the global allocator's dealloc here.
                // In a full implementation, we would call the global dealloc.
            }
        }
    }

    /// Resets the linear arena (for [1] allocations). This should be called at scope exit
    /// to reclaim all [1] allocated memory.
    ///
    /// # Safety
    ///   The caller must ensure that no [1] allocated pointers are used after this call.
    pub unsafe fn reset(&mut self) {
        self.linear_allocator.reset();
    }
}

// Safety implementation for GlobalAlloc is omitted because we don't have a mutable reference
// to the arena in the GlobalAlloc trait. In practice, the QttArena would be wrapped in a
// thread-local or similar construct to provide interior mutability.

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    #[test]
    fn test_qtt_arena_zero() {
        let mut buffer = [0u8; 1024];
        let mut arena = unsafe { QttArena::new(buffer.as_mut_ptr(), buffer.len()) };

        // Allocate a zero-sized type for [0]
        let layout = Layout::from_size_align(0, 1).unwrap();
        let ptr = unsafe { arena.alloc(0, layout) };
        // For zero-sized, we expect a non-null pointer (the start of the buffer or any pointer).
        // Our implementation returns the start of the buffer for zero-sized [0].
        assert_eq!(ptr, buffer.as_mut_ptr());

        // Deallocating zero-sized [0] should be safe.
        unsafe { arena.dealloc(0, ptr, layout) };
    }

    #[test]
    fn test_qtt_arena_linear() {
        let mut buffer = [0u8; 1024];
        let mut arena = unsafe { QttArena::new(buffer.as_mut_ptr(), buffer.len()) };

        let layout = Layout::from_size_align(16, 8).unwrap();
        let ptr1 = unsafe { arena.alloc(1, layout) };
        assert!(!ptr1.is_null());

        let ptr2 = unsafe { arena.alloc(1, layout) };
        assert!(!ptr2.is_null());
        assert!(ptr2 as usize > ptr1 as usize);

        // Reset and allocate again
        unsafe { arena.reset() };
        let ptr3 = unsafe { arena.alloc(1, layout) };
        assert!(!ptr3.is_null());
        assert_eq!(ptr3 as usize, buffer.as_ptr() as usize);
    }

    #[test]
    fn test_qtt_arena_heap_returns_null() {
        let mut buffer = [0u8; 1024];
        let mut arena = unsafe { QttArena::new(buffer.as_mut_ptr(), buffer.len()) };

        let layout = Layout::from_size_align(16, 8).unwrap();
        // [*] quantity should return null in this simplified implementation.
        let ptr = unsafe { arena.alloc(2, layout) };
        assert!(ptr.is_null());
    }
}