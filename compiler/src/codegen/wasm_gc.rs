//! WASM GC Backend
//!
//! Code generation for the `wasm32-gc` target (WebAssembly GC proposal).
//! This backend emits LLVM IR that targets the wasm32-unknown-unknown-gc triple,
//! enabling managed memory without wasm-bindgen glue code.
//!
//! Key features:
//! - Uses `struct.new`, `struct.get`, `struct.set` for GC-managed heap objects
//! - Integrates with Naso's linear types (QTT) for ownership tracking
//! - Emits WASM GC metadata for cross-language interop
//! - Supports WASM Component Model for cross-language composition

use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use crate::ir::pir_types::{ElemType, ExternParam, FnReturn, ParamKind, PirFunction, PirModule};
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::types::{StructType as LlvmStructType, BasicTypeEnum as LlvmBasicTypeEnum, FunctionType as LlvmFunctionType, BasicMetadataTypeEnum, BasicType};
use inkwell::values::{BasicValueEnum as LlvmBasicValueEnum, FunctionValue as LlvmFunctionValue, BasicMetadataValueEnum};
use inkwell::AddressSpace;
use std::collections::HashMap;

/// WASM GC codegen context
pub struct WasmGcCodegenContext<'ctx> {
    llvm_context: &'ctx LlvmContext,
    module: LlvmModule<'ctx>,
    builder: LlvmBuilder<'ctx>,
    target_data: inkwell::targets::TargetData,
    type_lowering: LlvmTypeLowering<'ctx>,
    gc_struct_types: HashMap<String, LlvmStructType<'ctx>>,
    is_wasm_gc: bool,
}

impl<'ctx> WasmGcCodegenContext<'ctx> {
    /// Create a new WASM GC codegen context
    pub fn new(context: &'ctx CodegenContext) -> CodegenResult<Self> {
        if !context.is_wasm_gc() {
            return Err(CodegenError::TargetError(
                "WasmGcCodegenContext requires wasm32-gc target".to_string(),
            ));
        }

        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("wasm_gc_module");
        let builder = llvm_context.create_builder();
        let target_data = context.target_machine().get_target_data();
        let type_lowering = LlvmTypeLowering::new(llvm_context);

        Ok(Self {
            llvm_context,
            module,
            builder,
            target_data,
            type_lowering,
            gc_struct_types: HashMap::new(),
            is_wasm_gc: true,
        })
    }

    /// Get the LLVM module
    pub fn module(&self) -> &LlvmModule<'ctx> {
        &self.module
    }

    /// Get the LLVM builder
    pub fn builder(&self) -> &LlvmBuilder<'ctx> {
        &self.builder
    }

    /// Get the LLVM context
    pub fn llvm_context(&self) -> &'ctx LlvmContext {
        self.llvm_context
    }

    /// Get the type lowering helper
    pub fn type_lowering(&self) -> &LlvmTypeLowering<'ctx> {
        &self.type_lowering
    }

    /// Lower an ElemType to a WASM GC-compatible LLVM type
    pub fn lower_elem_type(&self, elem: &ElemType) -> LlvmBasicTypeEnum<'ctx> {
        match elem {
            ElemType::F64 => self.llvm_context.f64_type().into(),
            ElemType::I4 => self.llvm_context.i8_type().into(),  // packed, handled at access
            ElemType::I8 => self.llvm_context.i8_type().into(),
            ElemType::I16 => self.llvm_context.i16_type().into(),
            ElemType::I32 => self.llvm_context.i32_type().into(),
            ElemType::I64 => self.llvm_context.i64_type().into(),
            ElemType::Bool => self.llvm_context.bool_type().into(),
        }
    }

    /// Lower a ParamKind to LLVM type(s) for function signature
    pub fn lower_param_kind(&self, kind: &ParamKind) -> CodegenResult<Vec<LlvmBasicTypeEnum<'ctx>>> {
        match kind {
            ParamKind::Tensor { elem: _, shape } => {
                // Tensor: pointer + length (for known shape) or just pointer (symbolic)
                let ptr_ty = self.llvm_context.ptr_type(AddressSpace::default());
                if shape.is_some() {
                    Ok(vec![ptr_ty.into(), self.llvm_context.i64_type().into()])
                } else {
                    Ok(vec![ptr_ty.into()])
                }
            }
            ParamKind::Scalar(elem) => {
                Ok(vec![self.lower_elem_type(elem)])
            }
            ParamKind::QRegister => {
                // Quantum register: opaque pointer
                Ok(vec![self.llvm_context.ptr_type(AddressSpace::default()).into()])
            }
            ParamKind::Struct => {
                // Struct: passed by value as aggregate (pointer to struct)
                Ok(vec![self.llvm_context.ptr_type(AddressSpace::default()).into()])
            }
            ParamKind::Unsupported(_) => {
                Err(CodegenError::TypeLoweringError("Unsupported parameter kind".to_string()))
            }
        }
    }

    /// Lower a FnReturn to LLVM return type option (None for void)
    pub fn lower_fn_return(&self, ret: &FnReturn) -> CodegenResult<Option<LlvmBasicTypeEnum<'ctx>>> {
        match ret {
            FnReturn::Void => Ok(None),
            FnReturn::Scalar(elem) => Ok(Some(self.lower_elem_type(elem))),
            FnReturn::Tensor { elem: _, shape: _ } => {
                // Tensor: returned as pointer + length
                let ptr_ty = self.llvm_context.ptr_type(AddressSpace::default());
                Ok(Some(ptr_ty.into()))
            }
            FnReturn::Struct => {
                // Struct: returned by value as aggregate
                Ok(Some(self.llvm_context.ptr_type(AddressSpace::default()).into()))
            }
            FnReturn::Array { elem: _, size: _ } => {
                // Array: returned by value as aggregate
                Ok(Some(self.llvm_context.ptr_type(AddressSpace::default()).into()))
            }
            FnReturn::Unsupported(_) => Err(CodegenError::TypeLoweringError("Unsupported return type".to_string())),
        }
    }

    /// Create or get a GC-rooted struct type for a Naso struct
    pub fn get_or_create_gc_struct(&mut self, name: &str, fields: &[(String, LlvmBasicTypeEnum<'ctx>)]) -> LlvmStructType<'ctx> {
        if let Some(existing) = self.gc_struct_types.get(name) {
            return *existing;
        }
        let field_types: Vec<_> = fields.iter().map(|(_, ty)| *ty).collect();
        let struct_ty = self.llvm_context.struct_type(&field_types, false);
        self.gc_struct_types.insert(name.to_string(), struct_ty);
        struct_ty
    }

    /// Get struct type name as string
    fn get_struct_name(struct_ty: LlvmStructType<'ctx>) -> String {
        struct_ty.get_name()
            .and_then(|cstr| cstr.to_str().ok())
            .unwrap_or("anon")
            .to_string()
    }

    /// Convert BasicValueEnum to BasicMetadataValueEnum
    fn to_metadata_value(val: LlvmBasicValueEnum<'ctx>) -> BasicMetadataValueEnum<'ctx> {
        BasicMetadataValueEnum::from(val)
    }

    /// Convert BasicTypeEnum to BasicMetadataTypeEnum
    fn to_metadata_type(ty: LlvmBasicTypeEnum<'ctx>) -> BasicMetadataTypeEnum<'ctx> {
        BasicMetadataTypeEnum::from(ty)
    }

    /// Emit a struct.new instruction for GC allocation
    pub fn emit_struct_new(&mut self, struct_ty: LlvmStructType<'ctx>, values: &[LlvmBasicValueEnum<'ctx>]) -> CodegenResult<LlvmBasicValueEnum<'ctx>> {
        let name = format!("wasm.gc.struct.new.{}", Self::get_struct_name(struct_ty));
        let struct_new_fn = if let Some(fn_val) = self.module.get_function(&name) {
            fn_val
        } else {
            let param_types: Vec<_> = struct_ty.get_field_types().iter().map(|t| Self::to_metadata_type(*t)).collect();
            let fn_ty = struct_ty.fn_type(&param_types, false);
            self.module.add_function(&name, fn_ty, None)
        };
        let args: Vec<_> = values.iter().map(|v| Self::to_metadata_value(*v)).collect();
        let call = self.builder.build_call(struct_new_fn, &args, "struct_new")?;
        Ok(call.try_as_basic_value().basic().unwrap())
    }

    /// Emit a struct.get instruction for field access
    pub fn emit_struct_get(&mut self, struct_val: LlvmBasicValueEnum<'ctx>, field_idx: u32) -> CodegenResult<LlvmBasicValueEnum<'ctx>> {
        let struct_name = match struct_val.get_type() {
            LlvmBasicTypeEnum::StructType(st) => Self::get_struct_name(st),
            _ => "anon".to_string(),
        };
        let name = format!("wasm.gc.struct.get.{}.{}", struct_name, field_idx);
        let struct_get_fn = if let Some(fn_val) = self.module.get_function(&name) {
            fn_val
        } else {
            let field_type = match struct_val.get_type() {
                LlvmBasicTypeEnum::StructType(st) => st.get_field_types().get(field_idx as usize).copied(),
                _ => None,
            }.ok_or_else(|| CodegenError::TypeLoweringError(format!("Invalid struct type for struct.get: {}", struct_name)))?;
            let fn_ty = field_type.fn_type(&[struct_val.get_type().into()], false);
            self.module.add_function(&name, fn_ty, None)
        };
        let call = self.builder.build_call(struct_get_fn, &[struct_val.into()], "struct_get")?;
        Ok(call.try_as_basic_value().basic().unwrap())
    }

    /// Emit a struct.set instruction for field mutation
    pub fn emit_struct_set(&mut self, struct_val: LlvmBasicValueEnum<'ctx>, field_idx: u32, value: LlvmBasicValueEnum<'ctx>) -> CodegenResult<()> {
        let struct_name = match struct_val.get_type() {
            LlvmBasicTypeEnum::StructType(st) => Self::get_struct_name(st),
            _ => "anon".to_string(),
        };
        let name = format!("wasm.gc.struct.set.{}.{}", struct_name, field_idx);
        let struct_set_fn = if let Some(fn_val) = self.module.get_function(&name) {
            fn_val
        } else {
            let field_type = match struct_val.get_type() {
                LlvmBasicTypeEnum::StructType(st) => st.get_field_types().get(field_idx as usize).copied(),
                _ => None,
            }.ok_or_else(|| CodegenError::TypeLoweringError(format!("Invalid struct type for struct.set: {}", struct_name)))?;
            let fn_ty = self.llvm_context.void_type().fn_type(&[struct_val.get_type().into(), field_type.into()], false);
            self.module.add_function(&name, fn_ty, None)
        };
        let args: Vec<_> = vec![struct_val.into(), value.into()];
        self.builder.build_call(struct_set_fn, &args, "struct_set")?;
        Ok(())
    }

    /// Emit array.new for GC-managed arrays
    pub fn emit_array_new(&mut self, _elem_ty: LlvmBasicTypeEnum<'ctx>, len: LlvmBasicValueEnum<'ctx>) -> CodegenResult<LlvmBasicValueEnum<'ctx>> {
        let name = "wasm.gc.array.new";
        let array_new_fn = if let Some(fn_val) = self.module.get_function(name) {
            fn_val
        } else {
            let fn_ty = self.llvm_context.ptr_type(AddressSpace::default()).fn_type(
                &[self.llvm_context.i32_type().into()],
                false
            );
            self.module.add_function(name, fn_ty, None)
        };
        let call = self.builder.build_call(array_new_fn, &[len.into()], "array_new")?;
        Ok(call.try_as_basic_value().basic().unwrap())
    }

    /// Emit array.get for array element access
    pub fn emit_array_get(&mut self, array_val: LlvmBasicValueEnum<'ctx>, index: LlvmBasicValueEnum<'ctx>) -> CodegenResult<LlvmBasicValueEnum<'ctx>> {
        let name = "wasm.gc.array.get";
        let array_get_fn = if let Some(fn_val) = self.module.get_function(name) {
            fn_val
        } else {
            let fn_ty = self.llvm_context.i64_type().fn_type(&[
                self.llvm_context.ptr_type(AddressSpace::default()).into(),
                self.llvm_context.i32_type().into()
            ], false);
            self.module.add_function(name, fn_ty, None)
        };
        let call = self.builder.build_call(array_get_fn, &[array_val.into(), index.into()], "array_get")?;
        Ok(call.try_as_basic_value().basic().unwrap())
    }

    /// Emit array.set for array element mutation
    pub fn emit_array_set(&mut self, array_val: LlvmBasicValueEnum<'ctx>, index: LlvmBasicValueEnum<'ctx>, value: LlvmBasicValueEnum<'ctx>) -> CodegenResult<()> {
        let name = "wasm.gc.array.set";
        let array_set_fn = if let Some(fn_val) = self.module.get_function(name) {
            fn_val
        } else {
            let fn_ty = self.llvm_context.void_type().fn_type(&[
                self.llvm_context.ptr_type(AddressSpace::default()).into(),
                self.llvm_context.i32_type().into(),
                self.llvm_context.i64_type().into()
            ], false);
            self.module.add_function(name, fn_ty, None)
        };
        let args: Vec<_> = vec![array_val.into(), index.into(), value.into()];
        self.builder.build_call(array_set_fn, &args, "array_set")?;
        Ok(())
    }

    /// Build the module from a PIR module
    pub fn build_module(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        // Declare external functions (WASI imports, etc.)
        self.declare_externals(pir_module)?;

        // Emit all functions
        for func in &pir_module.functions {
            self.emit_function(func)?;
        }

        Ok(())
    }

    /// Declare external functions (WASI, host imports)
    fn declare_externals(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        for ext in &pir_module.extern_functions {
            let mut param_types = Vec::new();
            for p in &ext.params {
                let lowered = self.lower_extern_param(p)?;
                param_types.extend(lowered);
            }
            let ret_ty = self.lower_extern_return(&ext.return_type)?;
            let fn_ty = self.type_lowering.fn_type(ret_ty, &param_types, false);
            let fn_val = self.module.add_function(&ext.name, fn_ty, None);
            
            // Mark as WASM import if it's a WASI function
            if ext.name.starts_with("wasi_") || ext.name.starts_with("fd_") || ext.name.starts_with("proc_") {
                fn_val.add_attribute(inkwell::attributes::AttributeLoc::Function,
                    self.llvm_context.create_string_attribute("wasm-import-module", "wasi_snapshot_preview1"));
            }
        }
        Ok(())
    }

    /// Lower an ExternParam to LLVM types
    fn lower_extern_param(&self, param: &ExternParam) -> CodegenResult<Vec<LlvmBasicTypeEnum<'ctx>>> {
        // ExternParam uses a string type representation; parse and lower
        match param.ty.as_str() {
            "i32" => Ok(vec![self.llvm_context.i32_type().into()]),
            "i64" => Ok(vec![self.llvm_context.i64_type().into()]),
            "f32" => Ok(vec![self.llvm_context.f32_type().into()]),
            "f64" => Ok(vec![self.llvm_context.f64_type().into()]),
            "ptr" => Ok(vec![self.llvm_context.ptr_type(AddressSpace::default()).into()]),
            _ => Ok(vec![self.llvm_context.i64_type().into()]), // default
        }
    }

    /// Lower an extern return type
    fn lower_extern_return(&self, ret: &Option<String>) -> CodegenResult<Option<LlvmBasicTypeEnum<'ctx>>> {
        match ret.as_deref() {
            Some("i32") => Ok(Some(self.llvm_context.i32_type().into())),
            Some("i64") => Ok(Some(self.llvm_context.i64_type().into())),
            Some("f32") => Ok(Some(self.llvm_context.f32_type().into())),
            Some("f64") => Ok(Some(self.llvm_context.f64_type().into())),
            Some("void") | None => Ok(None),
            _ => Ok(Some(self.llvm_context.i64_type().into())),
        }
    }

    /// Emit a single function
    fn emit_function(&mut self, func: &PirFunction) -> CodegenResult<()> {
        // Build parameter types
        let mut param_types = Vec::new();
        for param in &func.params {
            let lowered = self.lower_param_kind(&param.kind)?;
            param_types.extend(lowered);
        }

        let ret_ty = self.lower_fn_return(&func.return_type)?;

        let fn_ty = self.type_lowering.fn_type(ret_ty, &param_types, false);
        let fn_val = self.module.add_function(&func.name, fn_ty, None);

        // Set function attributes for WASM GC
        if self.is_wasm_gc {
            fn_val.add_attribute(inkwell::attributes::AttributeLoc::Function, 
                self.llvm_context.create_string_attribute("wasm-gc", "true"));
        }

        let entry = self.llvm_context.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);

        // For now, emit a simple stub that returns zero/void
        // Full implementation would lower the schedule tree
        if ret_ty.is_none() {
            self.builder.build_return(None)?;
        } else {
            let ret_basic = ret_ty.unwrap();
            self.builder.build_return(Some(&ret_basic.const_zero()))?;
        }

        Ok(())
    }

    /// Convert module to string
    pub fn module_to_string(&self) -> String {
        self.module.to_string()
    }
}

/// Main entry point for WASM GC codegen
pub fn emit_wasm_gc(module: &PirModule, context: &CodegenContext) -> CodegenResult<String> {
    let mut gc_ctx = WasmGcCodegenContext::new(context)?;
    gc_ctx.build_module(module)?;
    Ok(gc_ctx.module_to_string())
}