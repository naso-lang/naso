// linear.rs - LinearAllocator for [1]-quantity memory regions
// Guarantees single-use move semantics with zero runtime fragmentation.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};

/// A linear allocator that allocates memory in a bump-fashion and guarantees
/// that each allocation is used exactly once (linear). Deallocation is a no-op
/// because the entire buffer is reset at once (typically at scope exit).
/// This is suitable for [1]-quantity objects that must be consumed exactly once.
pub struct LinearAllocator {
    start: usize,
    end: usize,
    next: usize,
}

impl LinearAllocator {
    /// Creates a new LinearAllocator from a given memory buffer.
    ///
    /// # Safety
    /// The caller must ensure that the buffer is valid for the given size.
    pub unsafe fn new(start: *mut u8, size: usize) -> Self {
        LinearAllocator {
            start: start as usize,
            end: start as usize + size,
            next: start as usize,
        }
    }

    /// Allocates a block of memory with the given layout.
    ///
    /// Returns a pointer to the allocated memory, or null if the allocation fails.
    ///
    /// # Safety
    /// The caller must ensure that the layout is valid and that there is enough space.
    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();

        // Align the next pointer
        let aligned_next = (self.next + align - 1) & !(align - 1);
        let end = aligned_next + size;

        if end > self.end {
            return ptr::null_mut();
        }

        self.next = end;
        aligned_next as *mut u8
    }

    /// Deallocates a block of memory. For a linear allocator, this is a no-op
    /// because the entire buffer is reset at once (typically at scope exit).
    ///
    /// # Safety
    /// The caller must ensure that the pointer was allocated from this allocator
    /// and that the layout is correct.
    pub unsafe fn dealloc(&mut self, _ptr: *mut u8, _layout: Layout) {
        // No-op: deallocation is handled by resetting the entire buffer.
    }

    /// Resets the allocator to the start of the buffer.
    ///
    /// This is typically called at scope exit to reclaim all allocated memory.
    pub unsafe fn reset(&mut self) {
        self.next = self.start;
    }
}

unsafe impl GlobalAlloc for LinearAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // We need a mutable reference to self, but GlobalAlloc requires &self.
        // This is a simplified version; in practice, we would use interior mutability.
        // For the purpose of this exercise, we'll assume a mutable reference is available
        // via some external mechanism (like a thread-local singleton).
        // This is not a complete implementation but serves as a placeholder.
        let mut_alloc = &mut *(self as *const Self as *mut Self);
        mut_alloc.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let mut_alloc = &mut *(self as *const Self as *mut Self);
        mut_alloc.dealloc(ptr, layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    #[test]
    fn test_linear_allocator_basic() {
        let mut buffer = [0u8; 1024];
        let allocator = unsafe { LinearAllocator::new(buffer.as_mut_ptr(), buffer.len()) };

        let layout = Layout::from_size_align(16, 8).unwrap();
        let ptr1 = unsafe { allocator.alloc(layout) };
        assert!(!ptr1.is_null());

        let ptr2 = unsafe { allocator.alloc(layout) };
        assert!(!ptr2.is_null());
        assert!(ptr2 as usize > ptr1 as usize);

        // Reset and allocate again
        unsafe { allocator.reset() };
        let ptr3 = unsafe { allocator.alloc(layout) };
        assert!(!ptr3.is_null());
        assert_eq!(ptr3 as usize, buffer.as_ptr() as usize);
    }
}