#![allow(unused)]

// The prelude is imported by default in every naso source file.
// It re-exports commonly used types and functions from the standard library.

// Re-export core prelude
pub use crate::core::prelude::*;

// Re-export standard library prelude
pub use crate::std::prelude::*;
