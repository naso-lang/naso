//! Code Generation Context - Core Types
//!
//! Core types for codegen that don't require LLVM.

/// Target platforms supported by Naso
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenTarget {
    /// Auto-detect host target
    Host,
    /// NVIDIA PTX (GPU)
    Nvptx64,
    /// WebAssembly 32-bit (legacy, no GC)
    Wasm32,
    /// WebAssembly 32-bit with GC (wasm32-gc target)
    Wasm32Gc,
    /// WebAssembly 32-bit WASI Preview 1
    Wasm32WasiP1,
    /// WebAssembly 32-bit WASI Preview 2
    Wasm32WasiP2,
    /// WebAssembly 32-bit WASI Preview 3
    Wasm32WasiP3,
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
            CodegenTarget::Wasm32Gc => "wasm32-unknown-unknown-gc",
            CodegenTarget::Wasm32WasiP1 => "wasm32-wasip1",
            CodegenTarget::Wasm32WasiP2 => "wasm32-wasip2",
            CodegenTarget::Wasm32WasiP3 => "wasm32-wasip3",
            CodegenTarget::Aarch64 => "aarch64-unknown-linux-gnu",
            CodegenTarget::Custom(t) => t,
        }
    }

    /// Get the target features string for WASM targets
    pub fn wasm_features(&self) -> &'static str {
        match self {
            CodegenTarget::Wasm32Gc | CodegenTarget::Wasm32WasiP1 | CodegenTarget::Wasm32WasiP2 | CodegenTarget::Wasm32WasiP3 => {
                // Enable threads and simd128 for WASM GC targets
                "+atomics,+bulk-memory,+mutable-globals,+sign-ext,+simd128"
            }
            CodegenTarget::Wasm32 => "+atomics,+bulk-memory,+mutable-globals,+sign-ext",
            _ => "",
        }
    }

    #[allow(clippy::should_implement_trait)]
    /// Parse a target from a string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "host" | "native" => Ok(CodegenTarget::Host),
            "nvptx64" | "cuda" | "gpu" => Ok(CodegenTarget::Nvptx64),
            "wasm32" | "wasm" => Ok(CodegenTarget::Wasm32),
            "wasm32-gc" | "wasmgc" | "gc" => Ok(CodegenTarget::Wasm32Gc),
            "wasm32-wasip1" | "wasip1" | "wasi1" => Ok(CodegenTarget::Wasm32WasiP1),
            "wasm32-wasip2" | "wasip2" | "wasi2" => Ok(CodegenTarget::Wasm32WasiP2),
            "wasm32-wasip3" | "wasip3" | "wasi3" => Ok(CodegenTarget::Wasm32WasiP3),
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