// @generated
#[cfg(feature = "cranelift")]
/// Cranelift JIT Compilation
///
/// Stub implementation for fast JIT compilation and execution.
use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::ir::pir_types::PirModule;
use cranelift::codegen::isa::CallConv;
use cranelift::prelude::{
    AbiParam, FunctionBuilder, FunctionBuilderContext, InstBuilder, Signature, types,
};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};

/// Cranelift JIT Compiler
pub struct CraneliftJit {
    module: JITModule,
    ctx: cranelift::codegen::Context,
}

impl CraneliftJit {
    /// Create a new Cranelift JIT compiler
    pub fn new() -> CodegenResult<Self> {
        // Use the native ISA builder to create a JIT builder with default settings
        let builder = JITBuilder::new(default_libcall_names()).map_err(|e| {
            CodegenError::CraneliftError(format!("Failed to create JIT builder: {}", e))
        })?;

        let module = JITModule::new(builder);
        let ctx = module.make_context();

        Ok(Self { module, ctx })
    }

    /// Compile a function that returns the constant 42.
    ///
    /// This takes NO arguments from a PIR module and computes NOTHING. It is a
    /// JIT-harness self-test, and its result must never be presented as a Naso
    /// program's. `compile_and_execute` refuses rather than routing through here.
    pub fn compile_trivial_function(&mut self) -> CodegenResult<FuncId> {
        self.ctx.func.signature = Signature {
            params: vec![],
            returns: vec![AbiParam::new(types::I32)],
            call_conv: CallConv::SystemV,
        };

        let mut func_ctx = FunctionBuilderContext::new();
        let mut bcx = FunctionBuilder::new(&mut self.ctx.func, &mut func_ctx);

        let block = bcx.create_block();
        bcx.switch_to_block(block);
        bcx.seal_block(block);

        let iconst = bcx.ins().iconst(types::I32, 42);
        bcx.ins().return_(&[iconst]);

        bcx.finalize();

        let func_id = self
            .module
            .declare_function("trivial_fn", Linkage::Export, &self.ctx.func.signature)
            .map_err(|e| {
                CodegenError::CraneliftError(format!("Failed to declare function: {}", e))
            })?;

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| {
                CodegenError::CraneliftError(format!("Failed to define function: {}", e))
            })?;

        self.module.clear_context(&mut self.ctx);

        Ok(func_id)
    }

    /// Execute a function compiled by `compile_trivial_function`.
    ///
    /// The transmute below assumes the SystemV `() -> i32` signature that
    /// `compile_trivial_function` declares. Passing any other `FuncId` would call it
    /// with the wrong ABI, so the caller must be the trivial path only. That is why
    /// this is private to the harness's own tests rather than part of the pipeline.
    pub(crate) fn execute_function(&mut self, func_id: FuncId) -> CodegenResult<i32> {
        self.module
            .finalize_definitions()
            .map_err(|e| CodegenError::CraneliftError(format!("Failed to finalize: {}", e)))?;

        let code_ptr = self.module.get_finalized_function(func_id);

        // Cast to function pointer and call
        let func: extern "C" fn() -> i32 = unsafe { std::mem::transmute(code_ptr) };
        let result = func();

        Ok(result)
    }

    /// Compile and execute a trivial function (returns 42)
    pub fn compile_and_execute_trivial(&mut self) -> CodegenResult<i32> {
        let func_id = self.compile_trivial_function()?;
        self.execute_function(func_id)
    }
}

/// Compile and execute a PIR module via Cranelift JIT.
///
/// # REFUSED -- this backend does not execute Naso code
///
/// This function used to ignore `_module` entirely, JIT a hardcoded function that
/// returns the literal 42, and return that. Reached from the CLI it was worse than
/// wrong, because it was indistinguishable from success:
///
/// ```text
/// $ naso build --target cranelift kernels/sum_1_to_10.naso   # LLVM: 45
/// JIT execution result: 42
/// $ naso build --target cranelift kernels/returns_7.naso      # LLVM: 7
/// JIT execution result: 42
/// ```
///
/// Two different programs, the same output, exit status 0. `42` is the JIT stub's
/// own constant, not a value anything computed. Nothing downstream can detect this:
/// the number is a plausible `i32` and the call succeeded.
///
/// So this refuses. The Cranelift backend has no lowering from PIR -- no statement
/// walk, no control flow, no ABI, no memory model -- and `CraneliftJit`'s only
/// function is a constant-returning stub. Reporting success from it would be a
/// fabricated answer, which is the one outcome this language must never produce.
///
/// # Why CI did not catch it
///
/// The `cranelift` feature is never built or tested in CI. Every workflow line passes
/// `--features llvm`, so this file was not compiled by a single check until an audit
/// asked what it did. Two fixes are needed, not one: refuse here, and add the feature
/// to CI so the next gap in it cannot hide.
///
/// `CraneliftJit` itself is kept. It is a working JIT harness and its constant is
/// honestly labelled `trivial_fn`, but it takes no PIR and so cannot be reachable
/// from the compiler pipeline.
pub fn compile_and_execute(_module: &PirModule, _context: &CodegenContext) -> CodegenResult<i32> {
    Err(CodegenError::CraneliftError(
        "The Cranelift backend does not execute Naso programs yet, and no Naso \
         lowering exists for it: there is no statement walk, no control flow, no \
         ABI, and no memory model.\n\n\
         This used to JIT a function that returns the constant 42 and print that as \
         the program's result. Two unrelated programs both printed 42, with exit \
         status 0, which is a fabricated answer rather than a visible failure.\n\n\
         Use `--target llvm` or `--target qir`. When the Cranelift lowering lands, \
         this must be implemented rather than stubbed."
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jit_creation() {
        let jit = CraneliftJit::new();
        assert!(jit.is_ok());
    }

    #[test]
    fn test_trivial_compile_and_execute() {
        let mut jit = CraneliftJit::new().unwrap();
        let result = jit.compile_and_execute_trivial();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }
}
