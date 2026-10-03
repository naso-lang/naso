/// QIR Module Builder
/// Builds QIR-compatible LLVM IR modules with quantum intrinsics.
use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::qir::primitives::QIR_INTRINSICS;
use crate::codegen::qir::profile::{QirProfile, QirProfileKind};
use crate::ir::pir_types::{PirExpr, PirModule, PirStatement};
use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::types::{BasicTypeEnum, IntType, PointerType};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PointerValue};
use std::collections::HashMap;

// QIR Module Builder for generating quantum IR
pub struct QIRModuleBuilder<'ctx> {
    context: &'ctx CodegenContext,
    module: LlvmModule<'ctx>,
    builder: LlvmBuilder<'ctx>,
    llvm_context: &'ctx LlvmContext,
    profile: QirProfile,
    // Qubit type (opaque pointer)
    qubit_type: PointerType<'ctx>,
    // Result type (i1)
    result_type: IntType<'ctx>,
    // Current function being built
    current_function: Option<FunctionValue<'ctx>>,
    // Current basic block
    current_block: Option<BasicBlock<'ctx>>,
    // Variable allocations: pointer plus the pointee type (required by
    // opaque-pointer `build_load` in inkwell 0.10)
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    // Declared intrinsics
    declared_intrinsics: HashMap<String, FunctionValue<'ctx>>,
    /// Enclosing loops, outermost first, so `break`/`continue` take the LAST.
    ///
    /// The type is shared with the LLVM backend so both resolve early exits the same
    /// way. Two copies would be free to drift, and a drift between backends means a
    /// program that exits its loop on one target and not the other.
    loop_stack: Vec<crate::codegen::llvm::expr_lowering::LoopTargets<'ctx>>,
}

impl<'ctx> QIRModuleBuilder<'ctx> {
    // Create a new QIR module builder
    pub fn new(context: &'ctx CodegenContext) -> CodegenResult<Self> {
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("qir_module");
        let builder = llvm_context.create_builder();

        // QIR types
        let qubit_type = llvm_context.ptr_type(AddressSpace::from(0));
        let result_type = llvm_context.bool_type();

        let profile = QirProfile::new(QirProfileKind::Base);

        let mut qir_builder = Self {
            context,
            module,
            builder,
            llvm_context,
            profile,
            qubit_type,
            result_type,
            current_function: None,
            current_block: None,
            variables: HashMap::new(),
            declared_intrinsics: HashMap::new(),
            loop_stack: Vec::new(),
        };

        // Declare QIR intrinsics
        qir_builder.declare_intrinsics()?;
        // Add QIR metadata
        qir_builder.add_qir_metadata()?;

        Ok(qir_builder)
    }

    // Create a new QIR module builder with specific profile
    pub fn with_profile(context: &'ctx CodegenContext, profile: QirProfile) -> CodegenResult<Self> {
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("qir_module");
        let builder = llvm_context.create_builder();

        let qubit_type = llvm_context.ptr_type(AddressSpace::from(0));
        let result_type = llvm_context.bool_type();

        let mut qir_builder = Self {
            context,
            module,
            builder,
            llvm_context,
            profile,
            qubit_type,
            result_type,
            current_function: None,
            current_block: None,
            variables: HashMap::new(),
            declared_intrinsics: HashMap::new(),
            loop_stack: Vec::new(),
        };

        qir_builder.declare_intrinsics()?;
        qir_builder.add_qir_metadata()?;

        Ok(qir_builder)
    }

    // Get the underlying LLVM module
    pub fn module(&self) -> &LlvmModule<'ctx> {
        &self.module
    }

    // Get the LLVM context
    pub fn llvm_context(&self) -> &'ctx LlvmContext {
        self.llvm_context
    }

    // Get the qubit type
    pub fn qubit_type(&self) -> PointerType<'ctx> {
        self.qubit_type
    }

    // Get the result type
    pub fn result_type(&self) -> IntType<'ctx> {
        self.result_type
    }

    // Get the QIR profile
    pub fn profile(&self) -> &QirProfile {
        &self.profile
    }

    // Declare all QIR intrinsics
    fn declare_intrinsics(&mut self) -> CodegenResult<()> {
        for intrinsic in QIR_INTRINSICS {
            let fn_type = intrinsic.function_type(self);
            let func = self.module.add_function(intrinsic.name, fn_type, None);
            self.declared_intrinsics
                .insert(intrinsic.name.to_string(), func);
        }
        Ok(())
    }

    // Add QIR module metadata
    fn add_qir_metadata(&mut self) -> CodegenResult<()> {
        // Add QIR version metadata
        let version_md = self.llvm_context.metadata_string("1.0");
        let version_node = self.llvm_context.metadata_node(&[version_md.into()]);
        self.module
            .add_global_metadata("qir.version", &version_node)
            .map_err(|e| CodegenError::EmissionError(e.to_string()))?;

        // Add profile metadata
        let profile_md = self
            .llvm_context
            .metadata_string(self.profile.kind().as_str());
        let profile_node = self.llvm_context.metadata_node(&[profile_md.into()]);
        self.module
            .add_global_metadata("qir.profile", &profile_node)
            .map_err(|e| CodegenError::EmissionError(e.to_string()))?;

        // Add target triple metadata
        let target_md = self
            .llvm_context
            .metadata_string(self.context.target_triple().to_string().as_str());
        let target_node = self.llvm_context.metadata_node(&[target_md.into()]);
        self.module
            .add_global_metadata("qir.target", &target_node)
            .map_err(|e| CodegenError::EmissionError(e.to_string()))?;

        Ok(())
    }

    // Get an intrinsic by name
    pub fn get_intrinsic(&self, name: &str) -> Option<FunctionValue<'ctx>> {
        self.declared_intrinsics.get(name).copied()
    }

    // Call a QIR intrinsic
    pub fn call_intrinsic(
        &mut self,
        name: &str,
        args: &[BasicValueEnum<'ctx>],
        result_name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let func = self
            .get_intrinsic(name)
            .ok_or_else(|| CodegenError::QirError(format!("Intrinsic '{}' not found", name)))?;

        let meta_args: Vec<BasicMetadataValueEnum<'ctx>> =
            args.iter().map(|a| (*a).into()).collect();
        let call_site = self
            .builder
            .build_call(func, &meta_args, result_name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        call_site
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| CodegenError::QirError(format!("Intrinsic '{}' returned void", name)))
    }

    // Build the entire PIR module as QIR
    //
    // # This backend has no notion of a Naso FUNCTION
    //
    // It walks `pir_module.statements` -- the flat compatibility list -- and emits
    // one `void @qir_stmt_N()` per statement. `pir_module.functions` is never
    // consulted, so a declared return type has nowhere to go: no typed function is
    // emitted at all, and the caller's value simply does not exist.
    //
    // That is the silent wrong answer, not a harmless omission. A caller asking for
    // `f`'s result gets no `f` in the module and no diagnostic saying why, and
    // emitting `ret 0` instead would be worse still -- zero is the one value a
    // caller cannot distinguish from a computed zero. So a module containing a
    // value-returning function is REFUSED here, naming the function.
    //
    // This is not a regression: the same module already failed to compile the
    // moment its body used a float, because `build_expr` unwraps every operand as
    // an `IntValue`. QIR is currently usable only for qubit/gate statement bodies,
    // which return nothing. Saying so is strictly more useful than emitting a
    // module whose functions are missing.
    pub fn build_module(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        for func in &pir_module.functions {
            if func.return_type != crate::ir::pir_types::FnReturn::Void {
                return Err(CodegenError::QirError(format!(
                    "function `{}` declares a return type, and the QIR backend emits one \
                     `void` function per PIR statement rather than a typed function per \
                     Naso function, so this value has nowhere to go. Nothing is returned \
                     in its place: zero would be silently wrong, because a caller cannot \
                     distinguish it from a computed zero. Use the LLVM or WGSL backend \
                     for a function that returns a value.",
                    func.name
                )));
            }
        }

        // Build quantum operations from statements
        for stmt in &pir_module.statements {
            self.build_statement(stmt)?;
        }

        // Verify the module
        self.module
            .verify()
            .map_err(|e| CodegenError::VerificationError(e.to_string()))?;

        Ok(())
    }

    // Build a PIR statement
    fn build_statement(&mut self, stmt: &PirStatement) -> CodegenResult<()> {
        // Create a function for this statement
        let func_name = format!("qir_stmt_{}", stmt.id.0);
        let void_type = self.llvm_context.void_type();
        let fn_type = void_type.fn_type(&[], false);
        let function = self.module.add_function(&func_name, fn_type, None);

        self.current_function = Some(function);
        let entry = self.llvm_context.append_basic_block(function, "entry");
        self.current_block = Some(entry);
        self.builder.position_at_end(entry);

        // Build the statement body
        self.build_expr(&stmt.body)?;

        // Return void
        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        self.current_function = None;
        self.current_block = None;
        self.variables.clear();

        Ok(())
    }

    // Build a PIR expression as QIR
    fn build_expr(&mut self, expr: &PirExpr) -> CodegenResult<BasicValueEnum<'ctx>> {
        match expr {
            PirExpr::IntLit(val) => {
                let int_type = self.llvm_context.i64_type();
                Ok(int_type.const_int(*val as u64, false).into())
            }
            PirExpr::FloatLit(val) => {
                let float_type = self.llvm_context.f64_type();
                let parsed = val.parse::<f64>().unwrap_or(0.0);
                Ok(float_type.const_float(parsed).into())
            }
            PirExpr::BoolLit(val) => Ok(self.result_type.const_int(*val as u64, false).into()),
            PirExpr::Var(name) => {
                if let Some((ptr, pointee_ty)) = self.variables.get(name) {
                    Ok(self
                        .builder
                        .build_load(*pointee_ty, *ptr, name)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?)
                } else {
                    // Return zero for undefined
                    Ok(self.llvm_context.i64_type().const_int(0, false).into())
                }
            }
            PirExpr::Call { name, args } => {
                // Check if it's a QIR intrinsic
                if QIR_INTRINSICS.iter().any(|i| i.name == *name) {
                    let arg_values: CodegenResult<Vec<_>> =
                        args.iter().map(|a| self.build_expr(a)).collect();
                    let arg_values = arg_values?;
                    return self.call_intrinsic(name, &arg_values, "call_result");
                }

                // Regular function call
                let arg_values: CodegenResult<Vec<_>> =
                    args.iter().map(|a| self.build_expr(a)).collect();
                let arg_values = arg_values?;

                let func = self.module.get_function(name).ok_or_else(|| {
                    CodegenError::FunctionBuildError(format!("Function '{}' not found", name))
                })?;

                let meta_args: Vec<BasicMetadataValueEnum<'ctx>> =
                    arg_values.iter().map(|a| (*a).into()).collect();
                let call = self
                    .builder
                    .build_call(func, &meta_args, "call")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                // A void call yields no value; callers of build_expr need a
                // BasicValueEnum, so substitute the QIR result type's zero.
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.result_type.const_zero().into()))
            }
            PirExpr::Let {
                name, value, body, ..
            } => {
                let val = self.build_expr(value)?;
                let alloca = self
                    .builder
                    .build_alloca(val.get_type(), name)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.builder
                    .build_store(alloca, val)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.variables
                    .insert(name.clone(), (alloca, val.get_type()));

                let result = self.build_expr(body)?;

                self.variables.remove(name);
                Ok(result)
            }
            // QIR models qubits and their measurement, not integer arithmetic.
            // Refused rather than dropped: a dropped cast would emit QIR that computes
            // something other than what the source says.
            PirExpr::Cast { expr, width, .. } => Err(CodegenError::UnsupportedFeature(format!(
                "a cast to i{} of {expr:?} is not expressible in QIR",
                width.unwrap_or(32)
            ))),
            // QIR has no memory model, so there is nothing to store into.
            // Reported rather than ignored: evaluating the value and dropping it would
            // emit a QIR program that silently omits the program's effect.
            PirExpr::Assign { target, .. } => Err(CodegenError::UnsupportedFeature(format!(
                "assignment to {target:?} is not expressible in QIR, which models \
                 qubits and no general memory"
            ))),
            // A statement sequence yields no value; every element is emitted in
            // order and the value slot is filled with the i1 zero.
            PirExpr::Stmts(parts) => {
                let mut last: Option<BasicValueEnum<'ctx>> = None;
                for part in parts {
                    last = Some(self.build_expr(part)?);
                }
                match last {
                    Some(v) => Ok(v),
                    None => Ok(self
                        .context
                        .llvm_context()
                        .i32_type()
                        .const_int(0, false)
                        .into()),
                }
            }
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.build_expr(cond)?;
                let cond_bool = self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        cond_val.into_int_value(),
                        self.result_type.const_zero(),
                        "if_cond",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                let func = self.current_function().unwrap();
                let then_block = self.llvm_context.append_basic_block(func, "then");
                let else_block = self.llvm_context.append_basic_block(func, "else");
                let merge_block = self.llvm_context.append_basic_block(func, "if_merge");

                self.builder
                    .build_conditional_branch(cond_bool, then_block, else_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                self.current_block = Some(then_block);
                self.builder.position_at_end(then_block);
                let then_val = self.build_expr(then_branch)?;
                self.builder
                    .build_unconditional_branch(merge_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                let then_block_end = self.current_block().unwrap();

                self.current_block = Some(else_block);
                self.builder.position_at_end(else_block);
                let else_val = self.build_expr(else_branch)?;
                self.builder
                    .build_unconditional_branch(merge_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                let else_block_end = self.current_block().unwrap();

                self.current_block = Some(merge_block);
                self.builder.position_at_end(merge_block);
                let phi = self
                    .builder
                    .build_phi(then_val.get_type(), "if_phi")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                phi.add_incoming(&[(&then_val, then_block_end), (&else_val, else_block_end)]);
                Ok(phi.as_basic_value())
            }
            //
            // A `while` produces no value. Returning the body's last value would make
            // the loop's result depend on whether it ran at all, so this synthesises a
            // fresh zero of the RESULT type -- the same convention as a statement in an
            // expression position, and the reason the PIR documents the value as unit.
            //
            // REFUSED, not approximated.
            //
            // `break`/`continue` are lowered to nodes without a target on purpose: the
            // enclosing loop is known only from the CFG the backend is building. That
            // is the right design, and it means emitting them needs the backend to
            // thread a loop stack through `build_expr` -- a real change, not a
            // two-line branch.
            //
            // What matters here is that the alternative was rejected rather than
            // reached for. Treating `break` as "skip to the end of the loop" without a
            // loop stack, or as a no-op, produces a program that builds clean and
            // computes the wrong thing -- for `break` inside an `if`, skipping nothing
            // at all. So the backend says what is missing and what does work.
            PirExpr::Break { value } => {
                let _ = value;
                Err(CodegenError::UnsupportedFeature(
                    "`break` is parsed and lowered, but this backend does not yet emit \
                     it: the enclosing loop is known only from the control-flow graph \
                     being built, and no loop stack is threaded through expression \
                     lowering. It is refused rather than compiled to a no-op, because a \
                     `break` inside an `if` that did nothing would exit no loop at all \
                     and still build. `while` loops themselves work; see \
                     `llvm_while_execution_test`."
                        .to_string(),
                ))
            }
            PirExpr::Continue => Err(CodegenError::UnsupportedFeature(
                "`continue` is parsed and lowered, but this backend does not yet emit \
                 it, for the same reason as `break`: restarting the innermost loop \
                 requires the loop the control-flow graph is inside, which expression \
                 lowering is not currently given. Refused rather than compiled to a \
                 no-op, which would be an infinite loop."
                    .to_string(),
            )),
            PirExpr::While { cond, body, step } => {
                let func = self.current_function().unwrap();
                let header = self.llvm_context.append_basic_block(func, "while_cond");
                let body_block = self.llvm_context.append_basic_block(func, "while_body");
                let exit_block = self.llvm_context.append_basic_block(func, "while_exit");

                self.current_block = Some(header);
                self.builder.position_at_end(header);
                let cond_val = self.build_expr(cond)?;
                let cond_bool = self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        cond_val.into_int_value(),
                        self.result_type.const_zero(),
                        "while_cond_val",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.builder
                    .build_conditional_branch(cond_bool, body_block, exit_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                self.current_block = Some(body_block);
                self.builder.position_at_end(body_block);
                self.build_expr(body)?;

                // The step runs in its own block, which is where `continue` will land.
                // A counted loop advances its counter here, so branching straight back
                // to the header from a `continue` would skip the increment and the loop
                // would never terminate.
                let step_block = self.llvm_context.append_basic_block(func, "while_step");
                self.loop_stack
                    .push(crate::codegen::llvm::expr_lowering::LoopTargets {
                        continue_target: step_block,
                        break_target: exit_block,
                        // A runtime CFG loop has real exit edges.
                        affine_band: false,
                    });
                if self
                    .builder
                    .get_insert_block()
                    .and_then(|b| b.get_terminator())
                    .is_none()
                {
                    self.builder
                        .build_unconditional_branch(step_block)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                }
                self.loop_stack.pop();

                self.current_block = Some(step_block);
                self.builder.position_at_end(step_block);
                if let Some(st) = step {
                    self.build_expr(st)?;
                }
                self.builder
                    .build_unconditional_branch(header)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                self.current_block = Some(exit_block);
                self.builder.position_at_end(exit_block);
                Ok(self.result_type.const_zero().into())
            }
            PirExpr::Reversible { body, inverse: _ } => {
                // For QIR, we just build the body
                // The inverse would be handled by quantum compiler
                self.build_expr(body)
            }
            PirExpr::Index { base, indices: _ } => {
                let base_val = self.build_expr(base)?;
                Ok(base_val)
            }
            PirExpr::Field { base, field: _ } => {
                let base_val = self.build_expr(base)?;
                Ok(base_val)
            }
            PirExpr::Binary { op, left, right } => {
                let l = self.build_expr(left)?;
                let r = self.build_expr(right)?;
                self.build_binary_op(*op, l, r)
            }
            PirExpr::Unary { op, expr } => {
                let e = self.build_expr(expr)?;
                self.build_unary_op(*op, e)
            }
            PirExpr::QuantumOp { op, args, qubits } => {
                // Lower to the corresponding QIR intrinsic call, e.g. "h" ->
                // "qir.h". Value arguments come first, then the qubits they
                // act on.
                let intrinsic_name = format!("qir.{}", op);
                let func = self.get_intrinsic(&intrinsic_name).ok_or_else(|| {
                    CodegenError::QirError(format!(
                        "Unknown QIR intrinsic '{}' for quantum op '{}'",
                        intrinsic_name, op
                    ))
                })?;

                let mut arg_values: Vec<BasicValueEnum<'ctx>> = Vec::new();
                for a in args {
                    arg_values.push(self.build_expr(a)?);
                }
                for q in qubits {
                    arg_values.push(self.build_expr(q)?);
                }

                let meta_args: Vec<BasicMetadataValueEnum<'ctx>> =
                    arg_values.iter().map(|a| (*a).into()).collect();
                let call = self
                    .builder
                    .build_call(func, &meta_args, op)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Void-returning intrinsics (gates, releases) produce no value;
                // build_expr must return one, so yield the QIR result zero.
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.result_type.const_zero().into()))
            }
        }
    }

    fn build_binary_op(
        &mut self,
        op: crate::ir::pir_types::BinaryOp,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        use inkwell::IntPredicate;
        let left_int = left.into_int_value();
        let right_int = right.into_int_value();

        let result = match op {
            crate::ir::pir_types::BinaryOp::Add => {
                self.builder.build_int_add(left_int, right_int, "add")
            }
            crate::ir::pir_types::BinaryOp::Sub => {
                self.builder.build_int_sub(left_int, right_int, "sub")
            }
            crate::ir::pir_types::BinaryOp::Mul => {
                self.builder.build_int_mul(left_int, right_int, "mul")
            }
            crate::ir::pir_types::BinaryOp::Div => self
                .builder
                .build_int_signed_div(left_int, right_int, "div"),
            crate::ir::pir_types::BinaryOp::Mod => self
                .builder
                .build_int_signed_rem(left_int, right_int, "mod"),
            crate::ir::pir_types::BinaryOp::And => {
                self.builder.build_and(left_int, right_int, "and")
            }
            crate::ir::pir_types::BinaryOp::Or => self.builder.build_or(left_int, right_int, "or"),
            crate::ir::pir_types::BinaryOp::Xor => {
                self.builder.build_xor(left_int, right_int, "xor")
            }
            crate::ir::pir_types::BinaryOp::Eq => {
                self.builder
                    .build_int_compare(IntPredicate::EQ, left_int, right_int, "eq")
            }
            crate::ir::pir_types::BinaryOp::Ne => {
                self.builder
                    .build_int_compare(IntPredicate::NE, left_int, right_int, "ne")
            }
            crate::ir::pir_types::BinaryOp::Lt => {
                self.builder
                    .build_int_compare(IntPredicate::SLT, left_int, right_int, "lt")
            }
            crate::ir::pir_types::BinaryOp::Le => {
                self.builder
                    .build_int_compare(IntPredicate::SLE, left_int, right_int, "le")
            }
            crate::ir::pir_types::BinaryOp::Gt => {
                self.builder
                    .build_int_compare(IntPredicate::SGT, left_int, right_int, "gt")
            }
            crate::ir::pir_types::BinaryOp::Ge => {
                self.builder
                    .build_int_compare(IntPredicate::SGE, left_int, right_int, "ge")
            }
            crate::ir::pir_types::BinaryOp::Shl => {
                self.builder.build_left_shift(left_int, right_int, "shl")
            }
            crate::ir::pir_types::BinaryOp::Shr => self
                .builder
                .build_right_shift(left_int, right_int, true, "shr"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        Ok(result.into())
    }

    fn build_unary_op(
        &mut self,
        op: crate::ir::pir_types::UnaryOp,
        expr: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let int_val = expr.into_int_value();
        let result = match op {
            crate::ir::pir_types::UnaryOp::Neg => self.builder.build_int_neg(int_val, "neg"),
            crate::ir::pir_types::UnaryOp::Not => self.builder.build_not(int_val, "not"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        Ok(result.into())
    }

    // Convert module to QIR text format (.qir file)
    pub fn module_to_string(&self) -> String {
        self.module.print_to_string().to_string()
    }

    // Write module to .qir file
    pub fn write_qir_file(&self, path: &std::path::Path) -> CodegenResult<()> {
        self.module
            .print_to_file(path)
            .map_err(|e| CodegenError::EmissionError(e.to_string()))
    }

    fn current_function(&self) -> Option<FunctionValue<'ctx>> {
        self.current_function
    }

    fn current_block(&self) -> Option<BasicBlock<'ctx>> {
        self.current_block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};

    #[test]
    fn test_qir_module_builder_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let builder = QIRModuleBuilder::new(&context);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_qir_intrinsics_declared() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let builder = QIRModuleBuilder::new(&context).unwrap();

        // Check that key intrinsics are declared
        assert!(builder.get_intrinsic("qir.qubit_alloc").is_some());
        assert!(builder.get_intrinsic("qir.h").is_some());
        assert!(builder.get_intrinsic("qir.cx").is_some());
        assert!(builder.get_intrinsic("qir.mz").is_some());
    }

    #[test]
    fn test_qir_metadata() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let builder = QIRModuleBuilder::new(&context).unwrap();

        let ir = builder.module_to_string();
        assert!(ir.contains("qir.version"));
        assert!(ir.contains("qir.profile"));
    }

    // `call_intrinsic` must be called from INSIDE a function.
    // LLVM instructions can only be emitted into a basic block, and a module has no
    // block to emit into. Calling this at module scope therefore fails with "Builder
    // position is not set" -- which is correct, and is what the previous version of
    // this test asserted against. The test now builds a real function first, which is
    // how the one production caller uses it.
    #[test]
    fn test_qubit_allocation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let mut builder = QIRModuleBuilder::new(&context).unwrap();

        // A function body gives the builder a block to emit into. These are the same
        // module's private fields, which a sibling test may reach.
        let void_ty = context.llvm_context().void_type();
        let fn_type = void_ty.fn_type(&[], false);
        let func = builder.module.add_function("alloc_test", fn_type, None);
        let entry = context.llvm_context().append_basic_block(func, "entry");
        builder.builder.position_at_end(entry);

        builder
            .call_intrinsic("qir.qubit_alloc", &[], "q")
            .expect("allocating a qubit inside a function must succeed");
        // A void function still needs a terminator, or the module is invalid IR.
        builder
            .builder
            .build_return(None)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
            .expect("terminator");

        // The call must survive into the module text, and LLVM must accept it.
        let ir = builder.module_to_string();
        assert!(
            ir.contains("qubit_alloc"),
            "the intrinsic call must be emitted: {ir}"
        );
        builder
            .module()
            .verify()
            .unwrap_or_else(|e| panic!("LLVM rejected the module: {e}\n{ir}"));
    }

    /// A module whose function DECLARES a return type is refused, naming it.
    ///
    /// This backend emits one `void @qir_stmt_N()` per PIR statement and never reads
    /// `pir_module.functions`, so a declared return type has nowhere to go: no typed
    /// function is emitted and the caller's value simply does not exist. That is the
    /// silent wrong answer -- worse than a zero, because there is not even a zero to
    /// mistake for a computed one. Refusing says which function and why.
    #[test]
    fn a_value_returning_function_is_refused_rather_than_silently_dropped() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let mut builder = QIRModuleBuilder::new(&context).unwrap();

        let mut module = PirModule::default();
        module.functions.push(crate::ir::pir_types::PirFunction {
            name: "score".to_string(),
            params: Vec::new(),
            statements: Vec::new(),
            schedule: crate::ir::schedule_tree::ScheduleTree::new(
                crate::ir::schedule_tree::ScheduleNode::Empty,
                vec![],
            ),
            accesses: crate::ir::AccessRelations::default(),
            quantities: HashMap::new(),
            return_type: crate::ir::pir_types::FnReturn::Scalar(
                crate::ir::pir_types::ElemType::F64,
            ),
            return_stmt: None,
            tail_return_stmt: None,
            span: None,
        });

        let err = builder
            .build_module(&module)
            .expect_err("a value-returning function must be refused, not dropped");
        let msg = err.to_string();
        assert!(msg.contains("`score`"), "must name the function: {msg}");
        assert!(
            msg.contains("zero would be silently wrong"),
            "must say why no value is invented: {msg}"
        );
    }

    /// A module of VOID functions is unaffected by that refusal.
    ///
    /// QIR's actual use -- qubit allocation and gate application -- returns nothing,
    /// so the refusal above must not have made the backend unusable.
    #[test]
    fn a_void_function_still_builds() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let mut builder = QIRModuleBuilder::new(&context).unwrap();
        let mut module = PirModule::default();
        module.functions.push(crate::ir::pir_types::PirFunction {
            name: "apply_h".to_string(),
            params: Vec::new(),
            statements: Vec::new(),
            schedule: crate::ir::schedule_tree::ScheduleTree::new(
                crate::ir::schedule_tree::ScheduleNode::Empty,
                vec![],
            ),
            accesses: crate::ir::AccessRelations::default(),
            quantities: HashMap::new(),
            return_type: crate::ir::pir_types::FnReturn::Void,
            return_stmt: None,
            tail_return_stmt: None,
            span: None,
        });
        builder
            .build_module(&module)
            .expect("a void function must still build: QIR's real programs are void");
    }
}
