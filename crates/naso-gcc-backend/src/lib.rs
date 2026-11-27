//! GCC Backend for Naso Compiler
//!
//! This crate provides a GCC code generation backend for the Naso compiler,
//! enabling compilation to native code via GCC's libgccjit or static compilation.
//!
//! This is part of Phase 30: Multiple Implementations - GCC/LLVM/MSVC backends.

use naso_compiler::codegen::{CodegenBackend, CodegenContext, CodegenResult, TargetTriple};
use naso_compiler::ir::Module as IrModule;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GccBackendError {
    #[error("GCC JIT context creation failed: {0}")]
    ContextCreation(String),
    #[error("Type translation failed: {0}")]
    TypeTranslation(String),
    #[error("Function compilation failed: {0}")]
    FunctionCompilation(String),
    #[error("Module emission failed: {0}")]
    ModuleEmission(String),
    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(String),
}

/// GCC code generation backend
pub struct GccBackend {
    context: Option<gcc::Context>,
    target: TargetTriple,
}

impl GccBackend {
    /// Create a new GCC backend for the given target
    pub fn new(target: TargetTriple) -> Result<Self, GccBackendError> {
        let context = gcc::Context::new()
            .map_err(|e| GccBackendError::ContextCreation(e.to_string()))?;

        Ok(Self {
            context: Some(context),
            target,
        })
    }

    /// Compile an IR module to native code using GCC JIT
    pub fn compile_module(
        &mut self,
        module: &IrModule,
        output_path: &Path,
    ) -> Result<CodegenResult, GccBackendError> {
        let ctx = self.context.as_mut()
            .ok_or_else(|| GccBackendError::ContextCreation("Context not initialized".into()))?;

        // Set output options
        ctx.set_output_file(output_path.to_str().unwrap())
            .map_err(|e| GccBackendError::ModuleEmission(e.to_string()))?;

        // Compile each function
        for func in &module.functions {
            self.compile_function(ctx, func)?;
        }

        // Emit the binary
        ctx.compile()
            .map_err(|e| GccBackendError::ModuleEmission(e.to_string()))?;

        Ok(CodegenResult {
            output_path: output_path.to_path_buf(),
            target: self.target,
            backend: "gcc".to_string(),
        })
    }

    fn compile_function(
        &self,
        ctx: &mut gcc::Context,
        func: &naso_compiler::ir::Function,
    ) -> Result<(), GccBackendError> {
        // Create GCC function type
        let return_type = self.translate_type(&func.return_type)?;
        let param_types: Result<Vec<_>, _> = func.params.iter()
            .map(|p| self.translate_type(&p.ty))
            .collect();
        let param_types = param_types?;

        let fn_type = ctx.new_function_type(
            gcc::FunctionKind::Function,
            return_type,
            &param_types,
        ).map_err(|e| GccBackendError::FunctionCompilation(e.to_string()))?;

        // Create function
        let gcc_fn = ctx.new_function(
            &func.name,
            fn_type,
        ).map_err(|e| GccBackendError::FunctionCompilation(e.to_string()))?;

        // Build function body
        let block = gcc_fn.new_block("entry");
        let mut builder = gcc::Builder::new(block);

        // Allocate parameters
        let mut param_values = Vec::new();
        for (i, param) in func.params.iter().enumerate() {
            let param_val = gcc_fn.param(i as u32);
            param_values.push(param_val);
        }

        // Compile basic blocks
        for bb in &func.blocks {
            let gcc_block = gcc_fn.new_block(&bb.name);
            builder = builder.set_block(gcc_block);

            for instr in &bb.instructions {
                self.compile_instruction(&mut builder, &gcc_fn, instr, &param_values)?;
            }
        }

        Ok(())
    }

    fn translate_type(&self, ty: &naso_compiler::ir::Type) -> Result<gcc::Type, GccBackendError> {
        let ctx = self.context.as_ref()
            .ok_or_else(|| GccBackendError::ContextCreation("Context not initialized".into()))?;

        match ty {
            naso_compiler::ir::Type::Bool => Ok(ctx.bool_type()),
            naso_compiler::ir::Type::Int(bits) => match bits {
                8 => Ok(ctx.int_type(8)),
                16 => Ok(ctx.int_type(16)),
                32 => Ok(ctx.int_type(32)),
                64 => Ok(ctx.int_type(64)),
                128 => Ok(ctx.int_type(128)),
                _ => Err(GccBackendError::TypeTranslation(format!("Unsupported int size: {}", bits))),
            },
            naso_compiler::ir::Type::UInt(bits) => match bits {
                8 => Ok(ctx.unsigned_type(8)),
                16 => Ok(ctx.unsigned_type(16)),
                32 => Ok(ctx.unsigned_type(32)),
                64 => Ok(ctx.unsigned_type(64)),
                128 => Ok(ctx.unsigned_type(128)),
                _ => Err(GccBackendError::TypeTranslation(format!("Unsupported uint size: {}", bits))),
            },
            naso_compiler::ir::Type::Float(bits) => match bits {
                32 => Ok(ctx.float_type()),
                64 => Ok(ctx.double_type()),
                _ => Err(GccBackendError::TypeTranslation(format!("Unsupported float size: {}", bits))),
            },
            naso_compiler::ir::Type::Pointer(inner) => {
                let inner_type = self.translate_type(inner)?;
                Ok(ctx.pointer_type(inner_type))
            }
            naso_compiler::ir::Type::Array(inner, len) => {
                let inner_type = self.translate_type(inner)?;
                Ok(ctx.array_type(inner_type, *len as i64))
            }
            naso_compiler::ir::Type::Struct(fields) => {
                let field_types: Result<Vec<_>, _> = fields.iter()
                    .map(|f| self.translate_type(&f.ty))
                    .collect();
                let field_types = field_types?;
                Ok(ctx.struct_type(None, &field_types))
            }
            naso_compiler::ir::Type::Function(params, ret) => {
                let param_types: Result<Vec<_>, _> = params.iter()
                    .map(|p| self.translate_type(p))
                    .collect();
                let ret_type = self.translate_type(ret)?;
                Ok(ctx.function_type(ret_type, &param_types))
            }
            naso_compiler::ir::Type::Quantum(qty, _) => {
                // Quantum types map to opaque pointers in GCC
                Ok(ctx.pointer_type(ctx.void_type()))
            }
            _ => Err(GccBackendError::TypeTranslation(format!("Unsupported type: {:?}", ty))),
        }
    }

    fn compile_instruction(
        &self,
        builder: &mut gcc::Builder,
        fn_ctx: &gcc::Function,
        instr: &naso_compiler::ir::Instruction,
        params: &[gcc::Value],
    ) -> Result<(), GccBackendError> {
        use naso_compiler::ir::Instruction::*;

        match instr {
            Return(val) => {
                let v = self.compile_value(builder, val, params)?;
                builder.ret(Some(v));
            }
            ReturnVoid => {
                builder.ret(None);
            }
            Call { func, args, dest } => {
                // Function call compilation
                let arg_vals: Result<Vec<_>, _> = args.iter()
                    .map(|a| self.compile_value(builder, a, params))
                    .collect();
                let arg_vals = arg_vals?;

                // For now, assume direct function call
                // In practice, would need function lookup
                let call_result = builder.call(fn_ctx, &arg_vals);
                if let Some(dest) = dest {
                    // Store result
                }
            }
            BinaryOp { op, lhs, rhs, dest } => {
                let l = self.compile_value(builder, lhs, params)?;
                let r = self.compile_value(builder, rhs, params)?;
                let result = match op {
                    naso_compiler::ir::BinaryOp::Add => builder.add(l, r),
                    naso_compiler::ir::BinaryOp::Sub => builder.sub(l, r),
                    naso_compiler::ir::BinaryOp::Mul => builder.mul(l, r),
                    naso_compiler::ir::BinaryOp::Div => builder.div(l, r),
                    naso_compiler::ir::BinaryOp::Mod => builder.modulo(l, r),
                    naso_compiler::ir::BinaryOp::And => builder.bitwise_and(l, r),
                    naso_compiler::ir::BinaryOp::Or => builder.bitwise_or(l, r),
                    naso_compiler::ir::BinaryOp::Xor => builder.bitwise_xor(l, r),
                    naso_compiler::ir::BinaryOp::Shl => builder.lshift(l, r),
                    naso_compiler::ir::BinaryOp::Shr => builder.rshift(l, r),
                    naso_compiler::ir::BinaryOp::Eq => builder.equal(l, r),
                    naso_compiler::ir::BinaryOp::Ne => builder.not(builder.equal(l, r)),
                    naso_compiler::ir::BinaryOp::Lt => builder.less_than(l, r),
                    naso_compiler::ir::BinaryOp::Le => builder.less_or_equal(l, r),
                    naso_compiler::ir::BinaryOp::Gt => builder.greater_than(l, r),
                    naso_compiler::ir::BinaryOp::Ge => builder.greater_or_equal(l, r),
                };
                if let Some(dest) = dest {
                    // Store to dest
                }
            }
            Load { src, dest } => {
                let val = builder.load(src);
                // Store to dest
            }
            Store { src, dest } => {
                builder.store(src, dest);
            }
            Branch { target } => {
                builder.goto(target);
            }
            CondBranch { cond, then_bb, else_bb } => {
                let c = self.compile_value(builder, cond, params)?;
                builder.cond_br(c, then_bb, else_bb);
            }
            Alloc { ty, dest } => {
                let gcc_ty = self.translate_type(ty)?;
                let alloca = builder.alloca(gcc_ty);
                // Store to dest
            }
            QuantumOp { op, qubits, params: qparams, dest } => {
                // Quantum operations - delegate to runtime
                self.compile_quantum_op(builder, fn_ctx, op, qubits, qparams, dest)?;
            }
            EffectOp { effect, operation, args, dest } => {
                // Effect operations - delegate to runtime
                self.compile_effect_op(builder, fn_ctx, effect, operation, args, dest)?;
            }
            _ => {
                return Err(GccBackendError::UnsupportedFeature(
                    format!("Instruction not yet implemented: {:?}", instr)
                ));
            }
        }

        Ok(())
    }

    fn compile_value(
        &self,
        builder: &mut gcc::Builder,
        val: &naso_compiler::ir::Value,
        params: &[gcc::Value],
    ) -> Result<gcc::Value, GccBackendError> {
        use naso_compiler::ir::Value::*;

        match val {
            Constant(c) => self.compile_constant(builder, c),
            Param(idx) => Ok(params[*idx].clone()),
            Local(name) => {
                // Would need local variable lookup
                Err(GccBackendError::UnsupportedFeature("Local variable lookup".into()))
            }
            Global(name) => {
                // Global variable
                Err(GccBackendError::UnsupportedFeature("Global variable".into()))
            }
        }
    }

    fn compile_constant(
        &self,
        builder: &mut gcc::Builder,
        c: &naso_compiler::ir::Constant,
    ) -> Result<gcc::Value, GccBackendError> {
        let ctx = self.context.as_ref()
            .ok_or_else(|| GccBackendError::ContextCreation("Context not initialized".into()))?;

        use naso_compiler::ir::Constant::*;

        match c {
            Bool(b) => Ok(builder.bool(*b)),
            Int(i, bits) => {
                let ty = match bits {
                    8 => ctx.int_type(8),
                    16 => ctx.int_type(16),
                    32 => ctx.int_type(32),
                    64 => ctx.int_type(64),
                    128 => ctx.int_type(128),
                    _ => return Err(GccBackendError::TypeTranslation(format!("Unsupported int size: {}", bits))),
                };
                Ok(builder.constant(ty, *i as i64))
            }
            UInt(u, bits) => {
                let ty = match bits {
                    8 => ctx.unsigned_type(8),
                    16 => ctx.unsigned_type(16),
                    32 => ctx.unsigned_type(32),
                    64 => ctx.unsigned_type(64),
                    128 => ctx.unsigned_type(128),
                    _ => return Err(GccBackendError::TypeTranslation(format!("Unsupported uint size: {}", bits))),
                };
                Ok(builder.constant(ty, *u as u64))
            }
            Float(f, bits) => {
                let ty = match bits {
                    32 => ctx.float_type(),
                    64 => ctx.double_type(),
                    _ => return Err(GccBackendError::TypeTranslation(format!("Unsupported float size: {}", bits))),
                };
                Ok(builder.constant(ty, *f as f64))
            }
            Null => Ok(builder.null_pointer(ctx.pointer_type(ctx.void_type()))),
            Undef => Err(GccBackendError::UnsupportedFeature("Undef constant".into())),
        }
    }

    fn compile_quantum_op(
        &self,
        _builder: &mut gcc::Builder,
        _fn_ctx: &gcc::Function,
        _op: &naso_compiler::ir::QuantumOp,
        _qubits: &[naso_compiler::ir::Value],
        _params: &[naso_compiler::ir::Value],
        _dest: &Option<naso_compiler::ir::Value>,
    ) -> Result<(), GccBackendError> {
        // Quantum operations are lowered to runtime calls
        // This would call into naso-quantum-runtime
        Err(GccBackendError::UnsupportedFeature("Quantum ops need runtime integration".into()))
    }

    fn compile_effect_op(
        &self,
        _builder: &mut gcc::Builder,
        _fn_ctx: &gcc::Function,
        _effect: &str,
        _operation: &str,
        _args: &[naso_compiler::ir::Value],
        _dest: &Option<naso_compiler::ir::Value>,
    ) -> Result<(), GccBackendError> {
        // Effect operations are lowered to runtime calls
        Err(GccBackendError::UnsupportedFeature("Effect ops need runtime integration".into()))
    }
}

impl CodegenBackend for GccBackend {
    fn name(&self) -> &str {
        "gcc"
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
        // GCC supports most targets
        matches!(target.arch.as_str(), "x86_64" | "aarch64" | "riscv64" | "arm")
    }
}

/// Static compilation mode (ahead-of-time)
pub struct GccStaticBackend {
    target: TargetTriple,
    optimization_level: u8,
}

impl GccStaticBackend {
    pub fn new(target: TargetTriple, optimization_level: u8) -> Self {
        Self { target, optimization_level }
    }

    /// Compile IR to object file using GCC as a static compiler
    pub fn compile_to_object(
        &self,
        module: &IrModule,
        output: &Path,
    ) -> Result<CodegenResult, GccBackendError> {
        // Generate C code from IR, then invoke GCC
        let c_code = self.generate_c_code(module)?;
        let temp_dir = tempfile::tempdir().unwrap();
        let c_file = temp_dir.path().join("module.c");
        std::fs::write(&c_file, c_code)?;

        // Invoke GCC
        let mut cmd = std::process::Command::new("gcc");
        cmd.args([
            "-c",
            c_file.to_str().unwrap(),
            "-o", output.to_str().unwrap(),
            &format!("-O{}", self.optimization_level),
            "-std=c11",
            "-Wall",
            "-Wextra",
        ]);

        let status = cmd.status()
            .map_err(|e| GccBackendError::ModuleEmission(format!("Failed to run gcc: {}", e)))?;

        if !status.success() {
            return Err(GccBackendError::ModuleEmission("GCC compilation failed".into()));
        }

        Ok(CodegenResult {
            output_path: output.to_path_buf(),
            target: self.target,
            backend: "gcc-static".to_string(),
        })
    }

    fn generate_c_code(&self, module: &IrModule) -> Result<String, GccBackendError> {
        let mut code = String::new();

        // Headers
        code.push_str("#include <stdint.h>\n");
        code.push_str("#include <stdbool.h>\n");
        code.push_str("#include <stdlib.h>\n\n");

        // Type definitions
        code.push_str("// Type definitions\n");
        code.push_str("typedef int8_t i8;\n");
        code.push_str("typedef int16_t i16;\n");
        code.push_str("typedef int32_t i32;\n");
        code.push_str("typedef int64_t i64;\n");
        code.push_str("typedef uint8_t u8;\n");
        code.push_str("typedef uint16_t u16;\n");
        code.push_str("typedef uint32_t u32;\n");
        code.push_str("typedef uint64_t u64;\n");
        code.push_str("typedef float f32;\n");
        code.push_str("typedef double f64;\n");
        code.push_str("typedef bool bool_t;\n\n");

        // Quantum runtime forward declarations
        code.push_str("// Quantum runtime\n");
        code.push_str("typedef struct Qubit Qubit;\n");
        code.push_str("extern Qubit* naso_qubit_alloc(void);\n");
        code.push_str("extern void naso_qubit_free(Qubit*);\n");
        code.push_str("extern void naso_gate_h(Qubit*);\n");
        code.push_str("extern void naso_gate_x(Qubit*);\n");
        code.push_str("extern bool naso_measure(Qubit*);\n\n");

        // Effect runtime forward declarations
        code.push_str("// Effect runtime\n");
        code.push_str("extern void* naso_effect_perform(const char*, void**);\n");
        code.push_str("extern void naso_effect_handle(const char*, void*);\n\n");

        // Function declarations and definitions
        for func in &module.functions {
            code.push_str(&self.generate_c_function(func)?);
            code.push('\n');
        }

        Ok(code)
    }

    fn generate_c_function(&self, func: &naso_compiler::ir::Function) -> Result<String, GccBackendError> {
        let mut code = String::new();

        // Return type
        let ret_type = self.ir_type_to_c(&func.return_type)?;

        // Parameters
        let params: Result<Vec<String>, _> = func.params.iter()
            .map(|p| Ok(format!("{} {}", self.ir_type_to_c(&p.ty)?, p.name)))
            .collect();
        let params = params?.join(", ");

        code.push_str(&format!("{} {}({}) {{\n", ret_type, func.name, params));

        // Basic blocks
        for bb in &func.blocks {
            code.push_str(&format!("{}:\n", bb.name));
            for instr in &bb.instructions {
                code.push_str(&self.generate_c_instruction(instr)?);
            }
        }

        code.push_str("}\n");
        Ok(code)
    }

    fn ir_type_to_c(&self, ty: &naso_compiler::ir::Type) -> Result<String, GccBackendError> {
        use naso_compiler::ir::Type::*;

        Ok(match ty {
            Bool => "bool_t".into(),
            Int(8) => "i8".into(),
            Int(16) => "i16".into(),
            Int(32) => "i32".into(),
            Int(64) => "i64".into(),
            Int(128) => "__int128".into(),
            UInt(8) => "u8".into(),
            UInt(16) => "u16".into(),
            UInt(32) => "u32".into(),
            UInt(64) => "u64".into(),
            UInt(128) => "__uint128_t".into(),
            Float(32) => "f32".into(),
            Float(64) => "f64".into(),
            Pointer(inner) => format!("{}*", self.ir_type_to_c(inner)?),
            Array(inner, len) => format!("{}[{}]", self.ir_type_to_c(inner)?, len),
            Struct(_) => "struct { ... }".into(), // Would need named structs
            Function(_, _) => "void*".into(), // Function pointers
            Quantum(_, _) => "Qubit*".into(),
            _ => return Err(GccBackendError::TypeTranslation(format!("Unsupported type for C: {:?}", ty))),
        })
    }

    fn generate_c_instruction(&self, instr: &naso_compiler::ir::Instruction) -> Result<String, GccBackendError> {
        use naso_compiler::ir::Instruction::*;

        Ok(match instr {
            Return(val) => format!("    return {};\n", self.value_to_c(val)?),
            ReturnVoid => "    return;\n".into(),
            BinaryOp { op, lhs, rhs, dest } => {
                let l = self.value_to_c(lhs)?;
                let r = self.value_to_c(rhs)?;
                let d = self.value_to_c(dest)?;
                let op_str = match op {
                    naso_compiler::ir::BinaryOp::Add => "+",
                    naso_compiler::ir::BinaryOp::Sub => "-",
                    naso_compiler::ir::BinaryOp::Mul => "*",
                    naso_compiler::ir::BinaryOp::Div => "/",
                    naso_compiler::ir::BinaryOp::Mod => "%",
                    naso_compiler::ir::BinaryOp::And => "&",
                    naso_compiler::ir::BinaryOp::Or => "|",
                    naso_compiler::ir::BinaryOp::Xor => "^",
                    naso_compiler::ir::BinaryOp::Shl => "<<",
                    naso_compiler::ir::BinaryOp::Shr => ">>",
                    naso_compiler::ir::BinaryOp::Eq => "==",
                    naso_compiler::ir::BinaryOp::Ne => "!=",
                    naso_compiler::ir::BinaryOp::Lt => "<",
                    naso_compiler::ir::BinaryOp::Le => "<=",
                    naso_compiler::ir::BinaryOp::Gt => ">",
                    naso_compiler::ir::BinaryOp::Ge => ">=",
                };
                format!("    {} = {} {} {};\n", d, l, op_str, r)
            }
            Load { src, dest } => {
                let s = self.value_to_c(src)?;
                let d = self.value_to_c(dest)?;
                format!("    {} = *{};\n", d, s)
            }
            Store { src, dest } => {
                let s = self.value_to_c(src)?;
                let d = self.value_to_c(dest)?;
                format!("    *{} = {};\n", d, s)
            }
            Branch { target } => format!("    goto {};\n", target),
            CondBranch { cond, then_bb, else_bb } => {
                let c = self.value_to_c(cond)?;
                format!("    if ({}) goto {}; else goto {};\n", c, then_bb, else_bb)
            }
            Alloc { dest, .. } => {
                let d = self.value_to_c(dest)?;
                format!("    // alloc {}\n", d)
            }
            Call { func, args, dest } => {
                let args_c: Result<Vec<_>, _> = args.iter()
                    .map(|a| self.value_to_c(a))
                    .collect();
                let args_str = args_c?.join(", ");
                if let Some(dest) = dest {
                    let d = self.value_to_c(dest)?;
                    format!("    {} = {}({});\n", d, func, args_str)
                } else {
                    format!("    {}({});\n", func, args_str)
                }
            }
            QuantumOp { op, qubits, .. } => {
                let qubit_args: Result<Vec<_>, _> = qubits.iter()
                    .map(|q| self.value_to_c(q))
                    .collect();
                let qubit_str = qubit_args?.join(", ");
                let runtime_fn = match op {
                    naso_compiler::ir::QuantumOp::Alloc => "naso_qubit_alloc",
                    naso_compiler::ir::QuantumOp::Free => "naso_qubit_free",
                    naso_compiler::ir::QuantumOp::H => "naso_gate_h",
                    naso_compiler::ir::QuantumOp::X => "naso_gate_x",
                    naso_compiler::ir::QuantumOp::Measure => "naso_measure",
                    _ => "naso_quantum_unknown",
                };
                format!("    {}({});\n", runtime_fn, qubit_str)
            }
            EffectOp { effect, operation, args, .. } => {
                let args_c: Result<Vec<_>, _> = args.iter()
                    .map(|a| self.value_to_c(a))
                    .collect();
                let args_str = args_c?.join(", ");
                format!("    naso_effect_perform(\"{}.{}\", (void*[]){{{}}});\n", effect, operation, args_str)
            }
            _ => format!("    // TODO: {:?}\n", instr),
        })
    }

    fn value_to_c(&self, val: &naso_compiler::ir::Value) -> Result<String, GccBackendError> {
        use naso_compiler::ir::Value::*;

        Ok(match val {
            Constant(c) => self.constant_to_c(c)?,
            Param(idx) => format!("param_{}", idx),
            Local(name) => format!("local_{}", name),
            Global(name) => name.clone(),
        })
    }

    fn constant_to_c(&self, c: &naso_compiler::ir::Constant) -> Result<String, GccBackendError> {
        use naso_compiler::ir::Constant::*;

        Ok(match c {
            Bool(b) => if *b { "true" } else { "false" }.into(),
            Int(i, _) => i.to_string(),
            UInt(u, _) => u.to_string(),
            Float(f, 32) => format!("{}f", f),
            Float(f, 64) => f.to_string(),
            Null => "NULL".into(),
            Undef => "0".into(), // Undefined behavior
        })
    }
}