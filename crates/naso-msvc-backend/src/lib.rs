//! MSVC Backend for Naso Compiler
//!
//! This crate provides an MSVC-compatible code generation backend for the Naso compiler,
//! enabling compilation to native Windows code via LLVM IR that MSVC can consume,
//! or direct COFF/PE emission.
//!
//! This is part of Phase 30: Multiple Implementations - GCC/LLVM/MSVC backends.

use naso_compiler::codegen::{CodegenBackend, CodegenContext, CodegenResult, TargetTriple};
use naso_compiler::ir::Module as IrModule;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MsvcBackendError {
    #[error("LLVM context creation failed: {0}")]
    ContextCreation(String),
    #[error("Type translation failed: {0}")]
    TypeTranslation(String),
    #[error("Function compilation failed: {0}")]
    FunctionCompilation(String),
    #[error("Module emission failed: {0}")]
    ModuleEmission(String),
    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(String),
    #[error("MSVC-specific error: {0}")]
    MsvcSpecific(String),
}

/// MSVC code generation backend using LLVM IR
pub struct MsvcBackend {
    context: inkwell::context::Context,
    module: Option<inkwell::module::Module>,
    target: TargetTriple,
    target_machine: Option<inkwell::targets::TargetMachine>,
}

impl MsvcBackend {
    /// Create a new MSVC backend for the given target
    pub fn new(target: TargetTriple) -> Result<Self, MsvcBackendError> {
        let context = inkwell::context::Context::create();
        
        // Initialize LLVM targets
        inkwell::targets::Target::initialize_all(&inkwell::targets::InitializationConfig::default());

        // Get target triple for MSVC
        let triple_str = match target.arch.as_str() {
            "x86_64" => "x86_64-pc-windows-msvc",
            "aarch64" => "aarch64-pc-windows-msvc",
            _ => return Err(MsvcBackendError::MsvcSpecific(format!("Unsupported architecture for MSVC: {}", target.arch))),
        };

        let triple = inkwell::targets::TargetTriple::create(triple_str);
        let target = inkwell::targets::Target::from_triple(&triple)
            .map_err(|e| MsvcBackendError::ContextCreation(e.to_string()))?;

        let target_machine = target.create_target_machine(
            &triple,
            "generic",
            "",
            inkwell::targets::OptimizationLevel::Default,
            inkwell::targets::RelocMode::Default,
            inkwell::targets::CodeModel::Default,
        ).map_err(|e| MsvcBackendError::ContextCreation(e.to_string()))?;

        Ok(Self {
            context,
            module: None,
            target,
            target_machine: Some(target_machine),
        })
    }

    /// Compile an IR module to LLVM IR suitable for MSVC
    pub fn compile_module(
        &mut self,
        module: &IrModule,
        output_path: &Path,
    ) -> Result<CodegenResult, MsvcBackendError> {
        let llvm_module = self.context.create_module("naso_module");
        self.module = Some(llvm_module);

        // Set target triple for MSVC
        llvm_module.set_triple(&inkwell::targets::TargetTriple::create(
            match self.target.arch.as_str() {
                "x86_64" => "x86_64-pc-windows-msvc",
                "aarch64" => "aarch64-pc-windows-msvc",
                _ => "x86_64-pc-windows-msvc",
            }
        ));

        // Set data layout for MSVC
        llvm_module.set_data_layout(&self.target_machine.as_ref()
            .unwrap()
            .get_target_data()
            .get_data_layout());

        // Declare external runtime functions
        self.declare_runtime_functions(&llvm_module)?;

        // Compile each function
        for func in &module.functions {
            self.compile_function(&llvm_module, func)?;
        }

        // Emit LLVM IR
        llvm_module.print_to_file(output_path)
            .map_err(|e| MsvcBackendError::ModuleEmission(e.to_string()))?;

        // Also emit object file if requested
        if output_path.extension().map_or(false, |ext| ext == "obj") {
            self.emit_object_file(output_path)?;
        }

        Ok(CodegenResult {
            output_path: output_path.to_path_buf(),
            target: self.target,
            backend: "msvc".to_string(),
        })
    }

    fn declare_runtime_functions(&self, module: &inkwell::module::Module) -> Result<(), MsvcBackendError> {
        let void_type = self.context.void_type();
        let i8_ptr_type = self.context.ptr_type(inkwell::AddressSpace::default());
        let i32_type = self.context.i32_type();
        let i1_type = self.context.bool_type();

        // Quantum runtime
        module.add_function("naso_qubit_alloc", i8_ptr_type.fn_type(&[], false), None);
        module.add_function("naso_qubit_free", void_type.fn_type(&[i8_ptr_type.into()], false), None);
        module.add_function("naso_gate_h", void_type.fn_type(&[i8_ptr_type.into()], false), None);
        module.add_function("naso_gate_x", void_type.fn_type(&[i8_ptr_type.into()], false), None);
        module.add_function("naso_gate_y", void_type.fn_type(&[i8_ptr_type.into()], false), None);
        module.add_function("naso_gate_z", void_type.fn_type(&[i8_ptr_type.into()], false), None);
        module.add_function("naso_gate_cnot", void_type.fn_type(&[i8_ptr_type.into(), i8_ptr_type.into()], false), None);
        module.add_function("naso_measure", i1_type.fn_type(&[i8_ptr_type.into()], false), None);

        // Effect runtime
        module.add_function("naso_effect_perform", i8_ptr_type.fn_type(&[
            self.context.ptr_type(inkwell::AddressSpace::default()).into(), // effect name
            self.context.ptr_type(inkwell::AddressSpace::default()).into(), // args array
        ], false), None);
        module.add_function("naso_effect_handle", void_type.fn_type(&[
            self.context.ptr_type(inkwell::AddressSpace::default()).into(), // effect name
            self.context.ptr_type(inkwell::AddressSpace::default()).into(), // handler
        ], false), None);

        // Memory management
        module.add_function("naso_alloc", i8_ptr_type.fn_type(&[i32_type.into()], false), None);
        module.add_function("naso_free", void_type.fn_type(&[i8_ptr_type.into()], false), None);

        // Panic/unwind
        module.add_function("naso_panic", void_type.fn_type(&[
            self.context.ptr_type(inkwell::AddressSpace::default()).into(), // message
        ], false), None);

        Ok(())
    }

    fn compile_function(
        &self,
        module: &inkwell::module::Module,
        func: &naso_compiler::ir::Function,
    ) -> Result<(), MsvcBackendError> {
        let return_type = self.translate_type(&func.return_type)?;
        let param_types: Result<Vec<_>, _> = func.params.iter()
            .map(|p| self.translate_type(&p.ty))
            .collect();
        let param_types = param_types?;

        let fn_type = return_type.fn_type(&param_types.iter().map(|t| t.into()).collect::<Vec<_>>(), false);
        let llvm_fn = module.add_function(&func.name, fn_type, None);

        // Set calling convention for MSVC
        llvm_fn.set_call_conventions(inkwell::calling_convention::CallingConvention::MicrosoftFastcall);

        // Create entry block
        let entry = self.context.append_basic_block(llvm_fn, "entry");
        let mut builder = self.context.create_builder();
        builder.position_at_end(entry);

        // Build basic blocks
        let mut block_map = std::collections::HashMap::new();
        for bb in &func.blocks {
            let llvm_bb = self.context.append_basic_block(llvm_fn, &bb.name);
            block_map.insert(bb.name.clone(), llvm_bb);
        }

        // Position at entry and compile
        builder.position_at_end(entry);

        for bb in &func.blocks {
            let llvm_bb = block_map[&bb.name];
            builder.position_at_end(llvm_bb);

            for instr in &bb.instructions {
                self.compile_instruction(&mut builder, llvm_fn, instr, &block_map)?;
            }
        }

        Ok(())
    }

    fn translate_type(&self, ty: &naso_compiler::ir::Type) -> Result<inkwell::types::AnyTypeEnum<'_>, MsvcBackendError> {
        use naso_compiler::ir::Type::*;

        Ok(match ty {
            Bool => self.context.bool_type().as_any_type_enum(),
            Int(8) => self.context.i8_type().as_any_type_enum(),
            Int(16) => self.context.i16_type().as_any_type_enum(),
            Int(32) => self.context.i32_type().as_any_type_enum(),
            Int(64) => self.context.i64_type().as_any_type_enum(),
            Int(128) => self.context.i128_type().as_any_type_enum(),
            UInt(8) => self.context.i8_type().as_any_type_enum(), // LLVM uses same type
            UInt(16) => self.context.i16_type().as_any_type_enum(),
            UInt(32) => self.context.i32_type().as_any_type_enum(),
            UInt(64) => self.context.i64_type().as_any_type_enum(),
            UInt(128) => self.context.i128_type().as_any_type_enum(),
            Float(32) => self.context.f32_type().as_any_type_enum(),
            Float(64) => self.context.f64_type().as_any_type_enum(),
            Pointer(inner) => {
                let inner_type = self.translate_type(inner)?;
                self.context.ptr_type(inkwell::AddressSpace::default()).as_any_type_enum()
            }
            Array(inner, len) => {
                let inner_type = self.translate_type(inner)?;
                inner_type.array_type(*len as u32).as_any_type_enum()
            }
            Struct(fields) => {
                let field_types: Result<Vec<_>, _> = fields.iter()
                    .map(|f| self.translate_type(&f.ty))
                    .collect();
                let field_types = field_types?;
                let struct_type = self.context.struct_type(&field_types, false);
                struct_type.as_any_type_enum()
            }
            Function(params, ret) => {
                let param_types: Result<Vec<_>, _> = params.iter()
                    .map(|p| self.translate_type(p))
                    .collect();
                let ret_type = self.translate_type(ret)?;
                ret_type.fn_type(&param_types.iter().map(|t| t.into()).collect::<Vec<_>>(), false).as_any_type_enum()
            }
            Quantum(_, _) => {
                // Opaque pointer for quantum types
                self.context.ptr_type(inkwell::AddressSpace::default()).as_any_type_enum()
            }
            _ => return Err(MsvcBackendError::TypeTranslation(format!("Unsupported type: {:?}", ty))),
        })
    }

    fn compile_instruction(
        &self,
        builder: &mut inkwell::builder::Builder<'_>,
        fn_ctx: inkwell::values::FunctionValue<'_>,
        instr: &naso_compiler::ir::Instruction,
        block_map: &std::collections::HashMap<String, inkwell::basic_block::BasicBlock>,
    ) -> Result<(), MsvcBackendError> {
        use naso_compiler::ir::Instruction::*;

        match instr {
            Return(val) => {
                let v = self.compile_value(builder, val, fn_ctx)?;
                builder.build_return(Some(&v))?;
            }
            ReturnVoid => {
                builder.build_return(None)?;
            }
            Call { func, args, dest } => {
                let arg_vals: Result<Vec<_>, _> = args.iter()
                    .map(|a| self.compile_value(builder, a, fn_ctx))
                    .collect();
                let arg_vals = arg_vals?;

                // Look up function
                let module = fn_ctx.get_global_parent();
                let callee = module.get_function(func)
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("Function not found: {}", func)))?;

                let call_site = builder.build_call(callee, &arg_vals, "call")?;
                if let Some(dest) = dest {
                    // Store result to dest
                    // Would need dest handling
                }
            }
            BinaryOp { op, lhs, rhs, dest } => {
                let l = self.compile_value(builder, lhs, fn_ctx)?;
                let r = self.compile_value(builder, rhs, params)?;
                let result = match op {
                    naso_compiler::ir::BinaryOp::Add => builder.build_int_add(l.into_int_value(), r.into_int_value(), "add")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Sub => builder.build_int_sub(l.into_int_value(), r.into_int_value(), "sub")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Mul => builder.build_int_mul(l.into_int_value(), r.into_int_value(), "mul")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Div => builder.build_int_signed_div(l.into_int_value(), r.into_int_value(), "div")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Mod => builder.build_int_signed_rem(l.into_int_value(), r.into_int_value(), "mod")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::And => builder.build_and(l.into_int_value(), r.into_int_value(), "and")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Or => builder.build_or(l.into_int_value(), r.into_int_value(), "or")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Xor => builder.build_xor(l.into_int_value(), r.into_int_value(), "xor")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Shl => builder.build_left_shift(l.into_int_value(), r.into_int_value(), "shl")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Shr => builder.build_right_shift(l.into_int_value(), r.into_int_value(), false, "shr")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Eq => builder.build_int_compare(inkwell::IntPredicate::EQ, l.into_int_value(), r.into_int_value(), "eq")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Ne => builder.build_int_compare(inkwell::IntPredicate::NE, l.into_int_value(), r.into_int_value(), "ne")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Lt => builder.build_int_compare(inkwell::IntPredicate::SLT, l.into_int_value(), r.into_int_value(), "lt")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Le => builder.build_int_compare(inkwell::IntPredicate::SLE, l.into_int_value(), r.into_int_value(), "le")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Gt => builder.build_int_compare(inkwell::IntPredicate::SGT, l.into_int_value(), r.into_int_value(), "gt")?.as_any_value_enum(),
                    naso_compiler::ir::BinaryOp::Ge => builder.build_int_compare(inkwell::IntPredicate::SGE, l.into_int_value(), r.into_int_value(), "ge")?.as_any_value_enum(),
                };
                if let Some(dest) = dest {
                    // Store to dest
                }
            }
            Load { src, dest } => {
                let src_val = self.compile_value(builder, src, fn_ctx)?;
                let loaded = builder.build_load(src_val.into_pointer_value(), "load")?;
                if let Some(dest) = dest {
                    // Store to dest
                }
            }
            Store { src, dest } => {
                let src_val = self.compile_value(builder, src, fn_ctx)?;
                let dest_val = self.compile_value(builder, dest, fn_ctx)?;
                builder.build_store(dest_val.into_pointer_value(), src_val)?;
            }
            Branch { target } => {
                let target_bb = block_map.get(target)
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("Block not found: {}", target)))?;
                builder.build_unconditional_branch(*target_bb)?;
            }
            CondBranch { cond, then_bb, else_bb } => {
                let c = self.compile_value(builder, cond, fn_ctx)?;
                let then_block = block_map.get(then_bb)
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("Block not found: {}", then_bb)))?;
                let else_block = block_map.get(else_bb)
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("Block not found: {}", else_bb)))?;
                builder.build_conditional_branch(c.into_int_value(), *then_block, *else_block)?;
            }
            Alloc { ty, dest } => {
                let alloca_type = self.translate_type(ty)?;
                let alloca = builder.build_alloca(alloca_type.into_int_type(), "alloca")?;
                if let Some(dest) = dest {
                    // Store alloca to dest
                }
            }
            QuantumOp { op, qubits, params: qparams, dest } => {
                self.compile_quantum_op(builder, fn_ctx, op, qubits, qparams, dest)?;
            }
            EffectOp { effect, operation, args, dest } => {
                self.compile_effect_op(builder, fn_ctx, effect, operation, args, dest)?;
            }
            _ => {
                return Err(MsvcBackendError::UnsupportedFeature(
                    format!("Instruction not yet implemented: {:?}", instr)
                ));
            }
        }

        Ok(())
    }

    fn compile_value(
        &self,
        builder: &inkwell::builder::Builder<'_>,
        val: &naso_compiler::ir::Value,
        fn_ctx: inkwell::values::FunctionValue<'_>,
    ) -> Result<inkwell::values::AnyValueEnum<'_>, MsvcBackendError> {
        use naso_compiler::ir::Value::*;

        match val {
            Constant(c) => self.compile_constant(builder, c),
            Param(idx) => Ok(fn_ctx.get_nth_param(*idx as u32)
                .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("Param {} not found", idx)))?
                .as_any_value_enum()),
            Local(name) => {
                // Local variable lookup would need symbol table
                Err(MsvcBackendError::UnsupportedFeature("Local variable lookup".into()))
            }
            Global(name) => {
                let module = fn_ctx.get_global_parent();
                module.get_global(name)
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("Global not found: {}", name)))?
                    .as_any_value_enum()
            }
        }
    }

    fn compile_constant(
        &self,
        builder: &inkwell::builder::Builder<'_>,
        c: &naso_compiler::ir::Constant,
    ) -> Result<inkwell::values::AnyValueEnum<'_>, MsvcBackendError> {
        use naso_compiler::ir::Constant::*;

        Ok(match c {
            Bool(b) => self.context.bool_type().const_int(*b as u64, false).as_any_value_enum(),
            Int(i, bits) => {
                let ty = match bits {
                    8 => self.context.i8_type(),
                    16 => self.context.i16_type(),
                    32 => self.context.i32_type(),
                    64 => self.context.i64_type(),
                    128 => self.context.i128_type(),
                    _ => return Err(MsvcBackendError::TypeTranslation(format!("Unsupported int size: {}", bits))),
                };
                ty.const_int(*i as u64, true).as_any_value_enum()
            }
            UInt(u, bits) => {
                let ty = match bits {
                    8 => self.context.i8_type(),
                    16 => self.context.i16_type(),
                    32 => self.context.i32_type(),
                    64 => self.context.i64_type(),
                    128 => self.context.i128_type(),
                    _ => return Err(MsvcBackendError::TypeTranslation(format!("Unsupported uint size: {}", bits))),
                };
                ty.const_int(*u as u64, false).as_any_value_enum()
            }
            Float(f, bits) => {
                let ty = match bits {
                    32 => self.context.f32_type(),
                    64 => self.context.f64_type(),
                    _ => return Err(MsvcBackendError::TypeTranslation(format!("Unsupported float size: {}", bits))),
                };
                ty.const_float(*f as f64).as_any_value_enum()
            }
            Null => self.context.ptr_type(inkwell::AddressSpace::default()).const_null().as_any_value_enum(),
            Undef => return Err(MsvcBackendError::UnsupportedFeature("Undef constant".into())),
        })
    }

    fn compile_quantum_op(
        &self,
        builder: &inkwell::builder::Builder<'_>,
        fn_ctx: inkwell::values::FunctionValue<'_>,
        op: &naso_compiler::ir::QuantumOp,
        qubits: &[naso_compiler::ir::Value],
        params: &[naso_compiler::ir::Value],
        dest: &Option<naso_compiler::ir::Value>,
    ) -> Result<(), MsvcBackendError> {
        let module = fn_ctx.get_global_parent();
        let i8_ptr = self.context.ptr_type(inkwell::AddressSpace::default());

        let qubit_vals: Result<Vec<_>, _> = qubits.iter()
            .map(|q| self.compile_value(builder, q, fn_ctx))
            .collect();
        let qubit_vals = qubit_vals?;

        let runtime_fn = match op {
            naso_compiler::ir::QuantumOp::Alloc => {
                let alloc_fn = module.get_function("naso_qubit_alloc")
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation("naso_qubit_alloc not found".into()))?;
                let call = builder.build_call(alloc_fn, &[], "qubit_alloc")?;
                if let Some(dest) = dest {
                    let dest_val = self.compile_value(builder, dest, fn_ctx)?;
                    builder.build_store(dest_val.into_pointer_value(), call.try_as_basic_value().left().unwrap())?;
                }
                return Ok(());
            }
            naso_compiler::ir::QuantumOp::Free => "naso_qubit_free",
            naso_compiler::ir::QuantumOp::H => "naso_gate_h",
            naso_compiler::ir::QuantumOp::X => "naso_gate_x",
            naso_compiler::ir::QuantumOp::Y => "naso_gate_y",
            naso_compiler::ir::QuantumOp::Z => "naso_gate_z",
            naso_compiler::ir::QuantumOp::CNot => "naso_gate_cnot",
            naso_compiler::ir::QuantumOp::Measure => {
                let measure_fn = module.get_function("naso_measure")
                    .ok_or_else(|| MsvcBackendError::FunctionCompilation("naso_measure not found".into()))?;
                let q = qubit_vals[0].into_pointer_value();
                let call = builder.build_call(measure_fn, &[q.into()], "qubit_measure")?;
                if let Some(dest) = dest {
                    let dest_val = self.compile_value(builder, dest, fn_ctx)?;
                    builder.build_store(dest_val.into_pointer_value(), call.try_as_basic_value().left().unwrap())?;
                }
                return Ok(());
            }
            _ => return Err(MsvcBackendError::UnsupportedFeature(format!("Quantum op: {:?}", op))),
        };

        let fn_val = module.get_function(runtime_fn)
            .ok_or_else(|| MsvcBackendError::FunctionCompilation(format!("{} not found", runtime_fn)))?;

        let args: Vec<_> = qubit_vals.iter().map(|v| v.into()).collect();
        builder.build_call(fn_val, &args, "quantum_call")?;

        Ok(())
    }

    fn compile_effect_op(
        &self,
        builder: &inkwell::builder::Builder<'_>,
        fn_ctx: inkwell::values::FunctionValue<'_>,
        effect: &str,
        operation: &str,
        args: &[naso_compiler::ir::Value],
        dest: &Option<naso_compiler::ir::Value>,
    ) -> Result<(), MsvcBackendError> {
        let module = fn_ctx.get_global_parent();
        let i8_ptr = self.context.ptr_type(inkwell::AddressSpace::default());

        // Create effect name string
        let effect_name = format!("{}.{}", effect, operation);
        let effect_name_str = builder.build_global_string_ptr(&effect_name, "effect_name")?;

        // Create args array
        let arg_vals: Result<Vec<_>, _> = args.iter()
            .map(|a| self.compile_value(builder, a, fn_ctx))
            .collect();
        let arg_vals = arg_vals?;

        // Allocate array for arguments
        let array_type = i8_ptr.array_type(arg_vals.len() as u32);
        let array_alloca = builder.build_alloca(array_type, "effect_args")?;

        // Store each argument
        for (i, arg) in arg_vals.iter().enumerate() {
            let idx = self.context.i32_type().const_int(i as u64, false);
            let elem_ptr = unsafe {
                builder.build_gep(array_alloca, &[idx], "arg_elem")?
            };
            builder.build_store(elem_ptr, arg)?;
        }

        // Call effect perform
        let perform_fn = module.get_function("naso_effect_perform")
            .ok_or_else(|| MsvcBackendError::FunctionCompilation("naso_effect_perform not found".into()))?;

        let call = builder.build_call(perform_fn, &[
            effect_name_str.as_pointer_value().into(),
            array_alloca.into(),
        ], "effect_call")?;

        if let Some(dest) = dest {
            let dest_val = self.compile_value(builder, dest, fn_ctx)?;
            builder.build_store(dest_val.into_pointer_value(), call.try_as_basic_value().left().unwrap())?;
        }

        Ok(())
    }

    fn emit_object_file(&self, output_path: &Path) -> Result<(), MsvcBackendError> {
        let module = self.module.as_ref()
            .ok_or_else(|| MsvcBackendError::ModuleEmission("No module to emit".into()))?;

        let target_machine = self.target_machine.as_ref()
            .ok_or_else(|| MsvcBackendError::ContextCreation("No target machine".into()))?;

        target_machine.write_to_file(module, inkwell::targets::FileType::Object, output_path)
            .map_err(|e| MsvcBackendError::ModuleEmission(e.to_string()))?;

        Ok(())
    }
}

impl CodegenBackend for MsvcBackend {
    fn name(&self) -> &str {
        "msvc"
    }

    fn target(&self) -> TargetTriple {
        self.target
    }

    fn compile(
        &mut self,
        module: &IrModule,
        output: &Path,
    ) -> Result<CodegenResult, Box<dyn std::error::Error>> {
        self.compile_module(module, output)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }

    fn supports_target(&self, target: TargetTriple) -> bool {
        matches!(target.arch.as_str(), "x86_64" | "aarch64")
            && target.os == "windows"
    }
}

/// MSVC-compatible static library backend
pub struct MsvcStaticLibBackend {
    target: TargetTriple,
}

impl MsvcStaticLibBackend {
    pub fn new(target: TargetTriple) -> Self {
        Self { target }
    }

    /// Compile to MSVC-compatible static library (.lib)
    pub fn compile_to_static_lib(
        &self,
        module: &IrModule,
        output_path: &Path,
    ) -> Result<CodegenResult, MsvcBackendError> {
        // First emit object files
        let mut msvc_backend = MsvcBackend::new(self.target)?;
        let temp_dir = tempfile::tempdir().unwrap();
        
        // Compile each function to separate object file
        for func in &module.functions {
            let obj_path = temp_dir.path().join(format!("{}.obj", func.name));
            msvc_backend.compile_module(&IrModule {
                functions: vec![func.clone()],
                ..module.clone()
            }, &obj_path)?;
        }

        // Use lib.exe to create static library
        let obj_files: Vec<_> = std::fs::read_dir(temp_dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "obj"))
            .map(|e| e.path())
            .collect();

        let mut cmd = std::process::Command::new("lib.exe");
        cmd.arg(format!("/OUT:{}", output_path.display()));
        for obj in obj_files {
            cmd.arg(obj);
        }

        let status = cmd.status()
            .map_err(|e| MsvcBackendError::ModuleEmission(format!("Failed to run lib.exe: {}", e)))?;

        if !status.success() {
            return Err(MsvcBackendError::ModuleEmission("lib.exe failed".into()));
        }

        Ok(CodegenResult {
            output_path: output_path.to_path_buf(),
            target: self.target,
            backend: "msvc-staticlib".to_string(),
        })
    }
}