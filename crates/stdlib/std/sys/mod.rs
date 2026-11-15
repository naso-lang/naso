// sys/mod.rs - System module for Naso standard library
// Re-exports syscalls and panic handlers.

pub mod syscalls;
pub mod panic;

pub use syscalls::*;
pub use panic::*;