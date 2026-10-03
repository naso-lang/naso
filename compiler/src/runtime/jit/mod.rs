// A dormant FFI JIT layer. DISABLED -- see below.
//
// # Why this is off
//
// `ffi.rs` declares five `extern "C"` functions against a C library that does not
// exist anywhere in this repository:
//
//     jit_context_create, jit_context_destroy, jit_context_add_function,
//     jit_context_compile, jit_context_run
//
// Nothing defines them -- not a `.c` file, not a build script, not another crate.
// The declarations resolved fine as long as nothing *referenced* them, but the tests
// in both files do, so `--features cranelift --all-targets` failed to link:
//
//     rust-lld: error: undefined symbol: jit_context_create
//     rust-lld: error: undefined symbol: jit_context_destroy
//
// That failure is why this sat unnoticed. The `cranelift` feature was never built by
// CI, so a backend returning the constant 42 as any program's result AND a layer
// that could not link were both invisible at the same time. Fixing only the first
// left the second to surface immediately.
//
// # Nothing here is reachable
//
// `JitEngine` is referenced from nowhere outside this directory. It is not called by
// the CLI, not by `CodegenPipeline`, not by any backend. Turning these modules on
// would not make a JIT appear; it would only add an unlinkable dependency.
//
// # Turning it on
//
// Do not simply remove these `cfg` attributes. Either:
//
//   1. Provide the symbols -- write the C library, or point `ffi.rs` at the real
//      Cranelift C API (cranelift-jit / cranelift-codegen's C bindings) and change
//      these declarations to match it, or
//   2. Delete `engine.rs` and `ffi.rs`, if the intent was the Rust
//      `codegen::cranelift::jit::CraneliftJit` harness instead. Those two are
//      different designs and this layer is not a caller of that one.
//
// Option 2 loses nothing that anything currently uses. Option 1 is real work and
// should carry its own tests, which these -- calling into a library that cannot link
// -- could never provide.
//
// The declarations are kept in place rather than deleted so the intended interface is
// visible for whoever implements option 1.

#[cfg(all(feature = "cranelift", feature = "naso-jit-ffi"))]
pub mod engine;
#[cfg(all(feature = "cranelift", feature = "naso-jit-ffi"))]
pub mod ffi;
