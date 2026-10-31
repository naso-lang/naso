//! Code Generation Context - Core Types
//!
//! Core types for codegen that don't require LLVM.

/// Target platforms supported by Naso
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenTarget {
    /// Auto-detect host target
    Host,
    /// NVIDIA PTX (GPU)
    Nvptx64,
    /// WebAssembly 32-bit
    Wasm32,
    /// ARM64 Linux
    Aarch64,
    /// Custom target triple
    Custom(&'static str),
}

impl CodegenTarget {
    /// Get the target triple string
    pub fn triple(&self) -> &'static str {
        match self {
            CodegenTarget::Host => "x86_64-pc-windows-msvc", // Default fallback
            CodegenTarget::Nvptx64 => "nvptx64-nvidia-cuda",
            CodegenTarget::Wasm32 => "wasm32-unknown-unknown",
            CodegenTarget::Aarch64 => "aarch64-unknown-linux-gnu",
            CodegenTarget::Custom(t) => t,
        }
    }

    #[allow(clippy::should_implement_trait)]
    /// Parse a target from a string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "host" | "native" => Ok(CodegenTarget::Host),
            "nvptx64" | "cuda" | "gpu" => Ok(CodegenTarget::Nvptx64),
            "wasm32" | "wasm" => Ok(CodegenTarget::Wasm32),
            "aarch64" | "arm64" => Ok(CodegenTarget::Aarch64),
            other => Ok(CodegenTarget::Custom(Box::leak(
                other.to_string().into_boxed_str(),
            ))),
        }
    }
}

impl std::str::FromStr for CodegenTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s)
    }
}

/// Optimization levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    None = 0,
    Less = 1,
    Default = 2,
    Aggressive = 3,
}

impl From<OptLevel> for u32 {
    fn from(opt: OptLevel) -> Self {
        match opt {
            OptLevel::None => 0,
            OptLevel::Less => 1,
            OptLevel::Default => 2,
            OptLevel::Aggressive => 3,
        }
    }
}
