//! WASM Component Model Backend
//!
//! Code generation for the WASM Component Model target.
//! This enables cross-language composition without wit-bindgen boilerplate.
//!
//! Key features:
//! - Generates `.wasm` files with component model section
//! - Exports Naso functions as component interfaces
//! - Supports WASI Preview 2/3 interfaces
//! - Generates WIT (WASM Interface Types) definitions from Naso types

use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use crate::ir::pir_types::{ElemType, FnReturn, ParamKind, PirFunction, PirModule};
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::types::{StructType as LlvmStructType, BasicTypeEnum as LlvmBasicTypeEnum};
use inkwell::values::{BasicValueEnum as LlvmBasicValueEnum, FunctionValue as LlvmFunctionValue, BasicMetadataValueEnum, IntValue};
use inkwell::AddressSpace;
use std::collections::HashMap;

/// WASM Component Model codegen context
pub struct WasmComponentCodegenContext<'ctx> {
    llvm_context: &'ctx LlvmContext,
    module: LlvmModule<'ctx>,
    builder: LlvmBuilder<'ctx>,
    target_data: inkwell::targets::TargetData,
    type_lowering: LlvmTypeLowering<'ctx>,
    component_types: HashMap<String, ComponentType>,
    exports: Vec<ComponentExport>,
}

/// Component model type representation
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentType {
    /// Primitive types
    Bool,
    S8, S16, S32, S64,
    U8, U16, U32, U64,
    F32, F64,
    /// String types
    String,
    /// List types
    List(Box<ComponentType>),
    /// Record types (structs)
    Record { name: String, fields: Vec<(String, ComponentType)> },
    /// Variant types (enums)
    Variant { name: String, cases: Vec<(String, Option<ComponentType>)> },
    /// Function types
    Function { params: Vec<ComponentType>, results: Vec<ComponentType> },
    /// Resource types (opaque handles)
    Resource { name: String },
}

/// Component export definition
#[derive(Debug, Clone)]
pub struct ComponentExport {
    pub name: String,
    pub ty: ComponentType,
    pub func_name: String, // Internal function name
}

impl<'ctx> WasmComponentCodegenContext<'ctx> {
    /// Create a new WASM Component Model codegen context
    pub fn new(context: &'ctx CodegenContext) -> CodegenResult<Self> {
        if !context.is_wasm() {
            return Err(CodegenError::TargetError(
                "WasmComponentCodegenContext requires wasm32 target".to_string(),
            ));
        }

        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("wasm_component_module");
        let builder = llvm_context.create_builder();
        let target_data = context.target_machine().get_target_data();
        let type_lowering = LlvmTypeLowering::new(llvm_context);

        Ok(Self {
            llvm_context,
            module,
            builder,
            target_data,
            type_lowering,
            component_types: HashMap::new(),
            exports: Vec::new(),
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

    /// Lower an ElemType to a Component Type
    pub fn lower_elem_type(&self, elem: &ElemType) -> ComponentType {
        match elem {
            ElemType::F64 => ComponentType::F64,
            ElemType::I4 => ComponentType::S8,  // packed, handled at access
            ElemType::I8 => ComponentType::S8,
            ElemType::I16 => ComponentType::S16,
            ElemType::I32 => ComponentType::S32,
            ElemType::I64 => ComponentType::S64,
            ElemType::Bool => ComponentType::Bool,
        }
    }

    /// Lower a ParamKind to Component Type(s)
    pub fn lower_param_kind(&self, kind: &ParamKind) -> CodegenResult<Vec<ComponentType>> {
        match kind {
            ParamKind::Tensor { elem, shape: _ } => {
                // Tensor: represented as a list of elements
                let elem_ty = self.lower_elem_type(elem);
                Ok(vec![ComponentType::List(Box::new(elem_ty))])
            }
            ParamKind::Scalar(elem) => {
                Ok(vec![self.lower_elem_type(elem)])
            }
            ParamKind::QRegister => {
                // Quantum register: opaque resource
                Ok(vec![ComponentType::Resource { name: "qregister".to_string() }])
            }
            ParamKind::Struct => {
                // Struct: represented as a record
                Ok(vec![ComponentType::Record { name: "struct".to_string(), fields: vec![] }])
            }
            ParamKind::Unsupported(_) => {
                Err(CodegenError::TypeLoweringError("Unsupported parameter kind".to_string()))
            }
        }
    }

    /// Lower a FnReturn to Component Type(s)
    pub fn lower_fn_return(&self, ret: &FnReturn) -> CodegenResult<Vec<ComponentType>> {
        match ret {
            FnReturn::Void => Ok(vec![]),
            FnReturn::Scalar(elem) => Ok(vec![self.lower_elem_type(elem)]),
            FnReturn::Tensor { elem, shape: _ } => {
                let elem_ty = self.lower_elem_type(elem);
                Ok(vec![ComponentType::List(Box::new(elem_ty))])
            }
            FnReturn::Struct => {
                Ok(vec![ComponentType::Record { name: "struct".to_string(), fields: vec![] }])
            }
            FnReturn::Array { elem, size: _ } => {
                let elem_ty = self.lower_elem_type(elem);
                Ok(vec![ComponentType::List(Box::new(elem_ty))])
            }
            FnReturn::Unsupported(_) => Err(CodegenError::TypeLoweringError("Unsupported return type".to_string())),
        }
    }

    /// Register a component type
    pub fn register_type(&mut self, name: String, ty: ComponentType) {
        self.component_types.insert(name, ty);
    }

    /// Register a component export
    pub fn register_export(&mut self, export: ComponentExport) {
        self.exports.push(export);
    }

    /// Generate WIT (WASM Interface Types) definition for the component
    pub fn generate_wit(&self) -> String {
        let mut wit = String::new();
        
        // Package metadata
        wit.push_str("package naso:component\n\n");
        
        // World interface
        wit.push_str("world component {\n");
        
        // Import WASI interfaces
        wit.push_str("  import wasi:cli/run@0.2.0\n");
        wit.push_str("  import wasi:io/streams@0.2.0\n");
        wit.push_str("  import wasi:filesystem/types@0.2.0\n\n");
        
        // Export our functions
        for export in &self.exports {
            wit.push_str(&self.export_to_wit(export));
            wit.push_str("\n");
        }
        
        wit.push_str("}\n");
        
        // Type definitions
        for (name, ty) in &self.component_types {
            wit.push_str(&self.type_to_wit(name, ty));
            wit.push_str("\n");
        }
        
        wit
    }

    /// Convert an export to WIT format
    fn export_to_wit(&self, export: &ComponentExport) -> String {
        let params_wit = export.ty.to_wit_params();
        let results_wit = match &export.ty {
            ComponentType::Function { results, .. } => results.iter()
                .map(|r| r.to_wit_type())
                .collect::<Vec<_>>()
                .join(", "),
            _ => "".to_string(),
        };
        
        format!("  export {}: func({}) -> ({})", export.name, params_wit, results_wit)
    }

    /// Convert a type to WIT format
    fn type_to_wit(&self, name: &str, ty: &ComponentType) -> String {
        match ty {
            ComponentType::Record { fields, .. } => {
                let fields_wit: Vec<String> = fields.iter()
                    .map(|(fname, fty)| format!("{}: {}", fname, fty.to_wit_type()))
                    .collect();
                format!("record {} {{\n  {}\n}}", name, fields_wit.join(",\n  "))
            }
            ComponentType::Variant { cases, .. } => {
                let cases_wit: Vec<String> = cases.iter()
                    .map(|(cname, cty)| {
                        if let Some(ty) = cty {
                            format!("{}({})", cname, ty.to_wit_type())
                        } else {
                            cname.clone()
                        }
                    })
                    .collect();
                format!("variant {} {{\n  {}\n}}", name, cases_wit.join(",\n  "))
            }
            ComponentType::Resource { name } => {
                format!("resource {}", name)
            }
            _ => String::new(),
        }
    }

    /// Build the module from a PIR module
    pub fn build_module(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        // Register all function types and exports
        for func in &pir_module.functions {
            self.register_function(func)?;
        }

        // Emit all functions with component model attributes
        for func in &pir_module.functions {
            self.emit_function(func)?;
        }

        // Add component model metadata
        self.add_component_metadata()?;

        Ok(())
    }

    /// Register a function as a component export
    fn register_function(&mut self, func: &PirFunction) -> CodegenResult<()> {
        let mut param_types = Vec::new();
        for p in &func.params {
            let lowered = self.lower_param_kind(&p.kind)?;
            param_types.extend(lowered);
        }
        let ret_types = self.lower_fn_return(&func.return_type)?;

        let func_ty = ComponentType::Function {
            params: param_types,
            results: ret_types,
        };

        let export = ComponentExport {
            name: func.name.clone(),
            ty: func_ty.clone(),
            func_name: func.name.clone(),
        };

        self.register_export(export);
        self.register_type(func.name.clone(), func_ty);

        Ok(())
    }

    /// Emit a single function
    fn emit_function(&mut self, func: &PirFunction) -> CodegenResult<()> {
        // Build parameter types for LLVM
        let mut llvm_param_types = Vec::new();
        for param in &func.params {
            let lowered = self.lower_param_kind_llvm(&param.kind)?;
            llvm_param_types.extend(lowered);
        }

        let ret_ty = self.lower_fn_return_llvm(&func.return_type)?;

        let fn_ty = self.type_lowering.fn_type(ret_ty, &llvm_param_types, false);
        let fn_val = self.module.add_function(&func.name, fn_ty, None);

        // Add component model attributes
        fn_val.add_attribute(inkwell::attributes::AttributeLoc::Function,
            self.llvm_context.create_string_attribute("wasm-component", "true"));
        fn_val.add_attribute(inkwell::attributes::AttributeLoc::Function,
            self.llvm_context.create_string_attribute("wasm-export-name", &func.name));

        let entry = self.llvm_context.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);

        // Emit function body (stub for now)
        if ret_ty.is_none() {
            self.builder.build_return(None)?;
        } else {
            let ret_basic = ret_ty.unwrap();
            self.builder.build_return(Some(&ret_basic.const_zero()))?;
        }

        Ok(())
    }

    /// Lower ParamKind to LLVM types
    fn lower_param_kind_llvm(&self, kind: &ParamKind) -> CodegenResult<Vec<LlvmBasicTypeEnum<'ctx>>> {
        match kind {
            ParamKind::Tensor { elem: _, shape } => {
                let ptr_ty = self.llvm_context.ptr_type(AddressSpace::default());
                if shape.is_some() {
                    Ok(vec![ptr_ty.into(), self.llvm_context.i64_type().into()])
                } else {
                    Ok(vec![ptr_ty.into()])
                }
            }
            ParamKind::Scalar(elem) => {
                Ok(vec![self.lower_elem_type_llvm(elem)])
            }
            ParamKind::QRegister => {
                Ok(vec![self.llvm_context.ptr_type(AddressSpace::default()).into()])
            }
            ParamKind::Struct => {
                Ok(vec![self.llvm_context.ptr_type(AddressSpace::default()).into()])
            }
            ParamKind::Unsupported(_) => {
                Err(CodegenError::TypeLoweringError("Unsupported parameter kind".to_string()))
            }
        }
    }

    /// Lower ElemType to LLVM type
    fn lower_elem_type_llvm(&self, elem: &ElemType) -> LlvmBasicTypeEnum<'ctx> {
        match elem {
            ElemType::F64 => self.llvm_context.f64_type().into(),
            ElemType::I4 => self.llvm_context.i8_type().into(),
            ElemType::I8 => self.llvm_context.i8_type().into(),
            ElemType::I16 => self.llvm_context.i16_type().into(),
            ElemType::I32 => self.llvm_context.i32_type().into(),
            ElemType::I64 => self.llvm_context.i64_type().into(),
            ElemType::Bool => self.llvm_context.bool_type().into(),
        }
    }

    /// Lower FnReturn to LLVM return type option
    fn lower_fn_return_llvm(&self, ret: &FnReturn) -> CodegenResult<Option<LlvmBasicTypeEnum<'ctx>>> {
        match ret {
            FnReturn::Void => Ok(None),
            FnReturn::Scalar(elem) => Ok(Some(self.lower_elem_type_llvm(elem))),
            FnReturn::Tensor { elem: _, shape: _ } => {
                let ptr_ty = self.llvm_context.ptr_type(AddressSpace::default());
                Ok(Some(ptr_ty.into()))
            }
            FnReturn::Struct => {
                let ptr_ty = self.llvm_context.ptr_type(AddressSpace::default());
                Ok(Some(ptr_ty.into()))
            }
            FnReturn::Array { elem: _, size: _ } => {
                let ptr_ty = self.llvm_context.ptr_type(AddressSpace::default());
                Ok(Some(ptr_ty.into()))
            }
            FnReturn::Unsupported(_) => Err(CodegenError::TypeLoweringError("Unsupported return type".to_string())),
        }
    }

    /// Add component model metadata to the module
    fn add_component_metadata(&self) -> CodegenResult<()> {
        // Add module-level attributes for component model
        // Note: Module flags are not directly supported in inkwell 0.10
        // This would require using the LLVM C API directly
        // For now, we skip this and rely on function attributes
        Ok(())
    }

    /// Convert module to string
    pub fn module_to_string(&self) -> String {
        self.module.to_string()
    }
}

/// Main entry point for WASM Component Model codegen
pub fn emit_wasm_component(module: &PirModule, context: &CodegenContext) -> CodegenResult<String> {
    let mut comp_ctx = WasmComponentCodegenContext::new(context)?;
    comp_ctx.build_module(module)?;
    Ok(comp_ctx.module_to_string())
}

impl ComponentType {
    /// Convert to WIT parameter format
    fn to_wit_params(&self) -> String {
        match self {
            ComponentType::Function { params, .. } => params.iter()
                .map(|p| p.to_wit_type())
                .collect::<Vec<_>>()
                .join(", "),
            _ => self.to_wit_type(),
        }
    }

    /// Convert to WIT type string
    fn to_wit_type(&self) -> String {
        match self {
            ComponentType::Bool => "bool".to_string(),
            ComponentType::S8 => "s8".to_string(),
            ComponentType::S16 => "s16".to_string(),
            ComponentType::S32 => "s32".to_string(),
            ComponentType::S64 => "s64".to_string(),
            ComponentType::U8 => "u8".to_string(),
            ComponentType::U16 => "u16".to_string(),
            ComponentType::U32 => "u32".to_string(),
            ComponentType::U64 => "u64".to_string(),
            ComponentType::F32 => "f32".to_string(),
            ComponentType::F64 => "f64".to_string(),
            ComponentType::String => "string".to_string(),
            ComponentType::List(inner) => format!("list<{}>", inner.to_wit_type()),
            ComponentType::Record { name, .. } => name.clone(),
            ComponentType::Variant { name, .. } => name.clone(),
            ComponentType::Function { .. } => "func".to_string(),
            ComponentType::Resource { name } => name.clone(),
        }
    }
}