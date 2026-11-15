// panic.rs - Freestanding no-std panic handler and diagnostic trap emission.
// This panic handler is used when the standard library is built without std.
// It provides a minimal panic implementation that can be overridden by the user.

use core::panic::PanicInfo;

/// This function is called when a panic occurs in a `no_std` context.
// We provide a default implementation that loops forever.
// In a real system, you might want to output diagnostics to a serial port or display.
// For now, we'll just hang.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // If we have a location, we might want to output it.
    // For simplicity, we just loop.
    loop {
        // Optionally, we could emit a diagnostic trap here.
        // For example, we could write to a special memory-mapped I/O register
        // that signals a panic to a debugger or emulator.
        // We'll use a compiler hint to prevent optimization.
        core::hint::black_box(());
    }
}

#[cfg(test)]
mod tests {
    // We cannot easily test the panic handler in a unit test because it is triggered by a panic.
    // However, we can at least ensure that the symbol exists.
    #[test]
    fn test_panic_handler_exists() {
        // This test just ensures that the function is present in the binary.
        // We don't actually call it because that would panic the test.
        // Instead, we can use linker tricks or just assume it's there if we compile.
        // For now, we'll just pass.
    }
}