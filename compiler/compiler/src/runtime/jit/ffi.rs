use std::ffi::c_void;
use std::os::raw::c_int;

#[repr(C)]
pub struct JitContext {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub fn jit_context_create() -> *mut JitContext;
    pub fn jit_context_destroy(ctx: *mut JitContext);
    pub fn jit_context_add_function(
        ctx: *mut JitContext,
        name: *const i8,
        func_ptr: *const c_void,
        signature: *const i8,
    ) -> c_int;
    pub fn jit_context_compile(ctx: *mut JitContext, ir: *const i8) -> c_int;
    pub fn jit_context_run(
        ctx: *mut JitContext,
        name: *const i8,
        args: *mut *mut c_void,
    ) -> *mut c_void;
}

#[cfg(all(test, feature = "cranelift"))]
mod tests {
    use super::*;

    #[test]
    fn test_ffi_signatures() {
        unsafe {
            let ctx = jit_context_create();
            assert!(!ctx.is_null());
            jit_context_destroy(ctx);
        }
    }
}
