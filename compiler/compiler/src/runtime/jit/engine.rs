use super::ffi::{
    JitContext, jit_context_add_function, jit_context_compile, jit_context_create,
    jit_context_destroy, jit_context_run,
};
use std::ffi::CString;
use std::os::raw::c_void;

pub struct JitEngine {
    ctx: *mut JitContext,
}

impl JitEngine {
    pub fn new() -> Result<Self, String> {
        let ctx = unsafe { jit_context_create() };
        if ctx.is_null() {
            return Err("Failed to create JIT context".to_string());
        }
        Ok(Self { ctx })
    }

    pub fn add_function(&mut self, name: &str, func: fn(), signature: &str) -> Result<(), String> {
        let c_name = CString::new(name).map_err(|_| "Invalid function name")?;
        let c_signature = CString::new(signature).map_err(|_| "Invalid signature")?;

        let func_ptr = func as *const c_void;

        let result = unsafe {
            jit_context_add_function(self.ctx, c_name.as_ptr(), func_ptr, c_signature.as_ptr())
        };

        if result != 0 {
            return Err("Failed to add function".to_string());
        }
        Ok(())
    }

    pub fn compile(&mut self, ir: &str) -> Result<(), String> {
        let c_ir = CString::new(ir).map_err(|_| "Invalid IR")?;
        let result = unsafe { jit_context_compile(self.ctx, c_ir.as_ptr()) };
        if result != 0 {
            return Err("Compilation failed".to_string());
        }
        Ok(())
    }

    pub fn run(&mut self, name: &str, args: &mut [*mut c_void]) -> Result<*mut c_void, String> {
        let c_name = CString::new(name).map_err(|_| "Invalid function name")?;
        let result = unsafe { jit_context_run(self.ctx, c_name.as_ptr(), args.as_mut_ptr()) };
        if result.is_null() {
            return Err("Execution failed".to_string());
        }
        Ok(result)
    }
}

impl Drop for JitEngine {
    fn drop(&mut self) {
        unsafe { jit_context_destroy(self.ctx) };
    }
}

unsafe impl Send for JitEngine {}
unsafe impl Sync for JitEngine {}

#[cfg(all(test, feature = "cranelift"))]
mod tests {
    use super::*;

    #[test]
    fn test_engine_lifecycle() {
        let engine = JitEngine::new();
        assert!(engine.is_ok());
    }
}
