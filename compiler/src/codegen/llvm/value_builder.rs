// @generated
#[cfg(feature = "llvm")]
// LLVM Value Builder
//
// Typed builders for functions, globals, metadata, and common IR patterns.
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::module::Module;
use inkwell::types::{BasicTypeEnum, FunctionType, StructType};
use inkwell::values::{
    AnyValue, BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, GlobalValue,
    InstructionOpcode, InstructionValue, IntValue, MetadataValue, PointerValue,
};
use std::collections::HashMap;

/// Typed value builder for LLVM IR construction
pub struct LlvmValueBuilder<'ctx> {
    builder: Builder<'ctx>,
    type_lowering: LlvmTypeLowering<'ctx>,
    /// Metadata nodes
    metadata: HashMap<String, MetadataValue<'ctx>>,
    /// Pointee types for pointers handed out by this builder.
    ///
    /// LLVM 17 uses opaque pointers, so a `PointerValue` no longer carries the
    /// type it points at. Loads and GEPs need that type explicitly, so we
    /// remember it for every pointer we mint here (allocas, GEPs, struct GEPs).
    ptr_pointee_types: HashMap<PointerValue<'ctx>, BasicTypeEnum<'ctx>>,
    /// Named variable allocations (name -> pointer, pointee type).
    ///
    /// This lives here, next to the builder that emits the loads and stores, so that
    /// every lowering path shares ONE scope: `LLVMModuleBuilder` (which walks PIR
    /// statements) and `ScheduleLowering` (which walks a schedule tree and emits loop
    /// bodies) both borrow the same value builder. When the map was private to
    /// `LLVMModuleBuilder`, the schedule path had no bindings at all, so a band body
    /// could not even read or write the variable it was supposed to accumulate into.
    ///
    /// The pointee type is stored alongside the pointer for the same opaque-pointer
    /// reason as `ptr_pointee_types`: LLVM 17 cannot recover it from the pointer.
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
}

impl<'ctx> LlvmValueBuilder<'ctx> {
    /// Create a new value builder
    pub fn new(builder: Builder<'ctx>, type_lowering: LlvmTypeLowering<'ctx>) -> Self {
        Self {
            builder,
            type_lowering,
            metadata: HashMap::new(),
            ptr_pointee_types: HashMap::new(),
            variables: HashMap::new(),
        }
    }

    /// Get the underlying builder
    pub fn builder(&self) -> &Builder<'ctx> {
        &self.builder
    }

    /// Get the type lowering
    pub fn type_lowering(&self) -> &LlvmTypeLowering<'ctx> {
        &self.type_lowering
    }

    /// Record a named allocation: the pointer plus the type it points at.
    ///
    /// `ty` is the pointee type of the allocation; LLVM 17 opaque pointers do not
    /// carry it, so it is recorded here for the later `build_load` and for the
    /// assignment width check.
    pub fn add_variable(&mut self, name: String, ptr: PointerValue<'ctx>, ty: BasicTypeEnum<'ctx>) {
        self.variables.insert(name, (ptr, ty));
    }

    /// Register every global in `module` as a variable named after it.
    ///
    /// A statement walked by `LLVMModuleBuilder` can bind its own storage with a `let`,
    /// so it always has a destination for an assignment. A schedule tree has no such
    /// step: the only nameable storage a band body has is what already exists in the
    /// module. Without this, `counter = counter + 1` in a band body was refused with
    /// "no allocation is known for it" -- correctly, but for want of any way to say
    /// where `counter` lives.
    ///
    /// Registering a global is not a guess about its type: `GlobalValue` reports the
    /// type it was declared with. A name that is not a global stays unbound, so an
    /// assignment to it is still refused rather than silently dropped.
    pub fn register_module_globals(&mut self, module: &Module<'ctx>) -> CodegenResult<()> {
        for global in module.get_globals() {
            let name = match global.get_name().to_str() {
                Ok(n) if !n.is_empty() => n.to_string(),
                // LLVM renames an unnamed global to a numeric slot name; binding it
                // under that name would be meaningless, so it is left unregistered.
                _ => continue,
            };
            // `GlobalValue::get_value_type` is an `AnyTypeEnum`, which also covers void
            // and function types. Those cannot be loaded or stored, and coercing one
            // into a `BasicTypeEnum` would be inventing a type. inkwell's own
            // `as_basic_type_enum` PANICS on exactly these, so `TryFrom` is used
            // instead: a global whose type is not loadable is reported by name.
            let value_ty = BasicTypeEnum::try_from(global.get_value_type()).map_err(|_| {
                CodegenError::UnsupportedFeature(format!(
                    "global `{name}` has a type that is not loadable or storable, so it \
                     cannot be a variable this backend binds by name"
                ))
            })?;
            self.variables
                .insert(name, (global.as_pointer_value(), value_ty));
        }
        Ok(())
    }

    /// Look up a named allocation and its pointee type.
    pub fn variable(&self, name: &str) -> Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)> {
        self.variables.get(name).copied()
    }

    /// The names currently in scope, for a diagnostic that says what IS available.
    pub fn variable_names(&self) -> Vec<String> {
        self.variables.keys().cloned().collect()
    }

    /// Bring a name out of scope (a non-statement-position `let` leaving its body).
    pub fn remove_variable(&mut self, name: &str) {
        self.variables.remove(name);
    }

    /// Bind a function argument to `name`, so the body can read it by name.
    ///
    /// The argument is stored into an alloca rather than left as a bare SSA value
    /// because every other binding in this scope is an allocation: `PirExpr::Var`
    /// resolves through `variables` and loads, and loop bounds are lowered as
    /// `sum c_j * P_j` loads of exactly these slots. An argument that stayed an SSA
    /// value would need a second, parallel name-resolution path.
    ///
    /// The pointee type is recorded in `ptr_pointee_types` as well as in
    /// `variables`, for the same reason `build_load` requires it: LLVM 17 opaque
    /// pointers do not carry it.
    ///
    /// The argument's own type is used, not an assumed `i64`. A function whose
    /// parameter is not an integer cannot be a loop bound, and giving it an integer
    /// type here would be inventing a conversion.
    pub fn bind_argument(&mut self, name: &str, arg: BasicValueEnum<'ctx>) -> CodegenResult<()> {
        let ty = arg.get_type();
        let alloca = self.build_alloca(ty, name)?;
        self.build_store(alloca, arg)?;
        self.add_variable(name.to_string(), alloca, ty);
        Ok(())
    }

    /// Drop every binding. Called when a function body finishes.
    pub fn clear_variables(&mut self) {
        self.variables.clear();
    }

    /// Build a function with the given signature and body builder
    pub fn build_function<F>(
        &mut self,
        module: &Module<'ctx>,
        name: &str,
        ret_type: Option<BasicTypeEnum<'ctx>>,
        param_types: &[BasicTypeEnum<'ctx>],
        param_names: &[&str],
        body_builder: F,
    ) -> CodegenResult<FunctionValue<'ctx>>
    where
        F: FnOnce(&mut LlvmValueBuilder<'ctx>, &[BasicValueEnum<'ctx>]) -> CodegenResult<()>,
    {
        let fn_type = self.type_lowering.fn_type(ret_type, param_types, false);
        let function = module.add_function(name, fn_type, None);

        // Set parameter names
        for (i, param_name) in param_names.iter().enumerate() {
            if let Some(param) = function.get_nth_param(i as u32) {
                param.set_name(param_name);
            }
        }

        // Create entry block
        let entry = self
            .type_lowering
            .context()
            .append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Collect parameter values
        let params: Vec<BasicValueEnum<'ctx>> = (0..param_types.len())
            .map(|i| function.get_nth_param(i as u32).unwrap())
            .collect();

        // Build function body
        body_builder(self, &params)?;

        // A basic block MUST end in a terminator or the module is invalid IR, and LLVM's
        // verifier rejects it. This was not emitted here, so every function built through
        // this path produced `define void @f() { entry: }` -- unparseable by llc, and
        // unrunnable. The two `value_builder` tests that had never passed were failing
        // on exactly this.
        //
        // A body that emitted its own terminator is left alone; `ret` is not appended
        // twice.
        let needs_terminator = entry
            .get_last_instruction()
            .map(|last| {
                !matches!(
                    last.get_opcode(),
                    InstructionOpcode::Return
                        | InstructionOpcode::Br
                        | InstructionOpcode::Switch
                        | InstructionOpcode::Unreachable
                        | InstructionOpcode::Resume
                        | InstructionOpcode::Invoke
                        | InstructionOpcode::CallBr
                        | InstructionOpcode::CatchRet
                        | InstructionOpcode::CatchSwitch
                        | InstructionOpcode::CleanupRet
                )
            })
            .unwrap_or(true);
        if needs_terminator {
            match ret_type {
                // `ret void` is spelled `build_return(None)`: inkwell has no separate
                // `build_ret_void`.
                None => self
                    .builder
                    .build_return(None)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?,
                Some(rt) => {
                    // Returning a fabricated zero is a wrong answer waiting to happen, so
                    // it is only done for the scalar types where zero is genuinely the
                    // right fallback for a body that forgot to return. Anything else is
                    // refused rather than invented.
                    let zero: BasicValueEnum<'ctx> = match rt {
                        BasicTypeEnum::IntType(t) => t.const_zero().into(),
                        BasicTypeEnum::FloatType(t) => t.const_zero().into(),
                        _ => {
                            return Err(CodegenError::UnsupportedFeature(format!(
                                "cannot terminate `{name}`: its body emitted no return and \
                                 its return type {rt:?} has no zero value"
                            )));
                        }
                    };
                    self.builder
                        .build_return(Some(&zero))
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?
                }
            };
        }

        // Verify function. inkwell 0.10's `verify` reports through a bool and
        // prints diagnostics to stderr when `print` is true, so on failure we
        // surface the offending function body as the error message.
        if !function.verify(true) {
            return Err(CodegenError::VerificationError(format!(
                "function `{name}` failed LLVM verification:\n{}",
                function.print_to_string()
            )));
        }

        Ok(function)
    }

    /// Build a function that returns void
    pub fn build_void_function<F>(
        &mut self,
        module: &Module<'ctx>,
        name: &str,
        param_types: &[BasicTypeEnum<'ctx>],
        param_names: &[&str],
        body_builder: F,
    ) -> CodegenResult<FunctionValue<'ctx>>
    where
        F: FnOnce(&mut LlvmValueBuilder<'ctx>, &[BasicValueEnum<'ctx>]) -> CodegenResult<()>,
    {
        self.build_function(module, name, None, param_types, param_names, body_builder)
    }

    /// Create an alloca instruction in the entry block
    pub fn build_alloca(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> CodegenResult<PointerValue<'ctx>> {
        let current_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let entry = current_fn.get_first_basic_block().unwrap();
        let first_inst = entry.get_first_instruction();

        // Emit through a scratch builder positioned in the entry block so the
        // caller's insertion point is left exactly where it was. (inkwell 0.10
        // has no `get_insert_point`, so saving/restoring in place is not
        // possible.)
        let entry_builder = self.type_lowering.context().create_builder();
        if let Some(inst) = first_inst {
            entry_builder.position_before(&inst);
        } else {
            entry_builder.position_at_end(entry);
        }

        let alloca = entry_builder
            .build_alloca(ty, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        self.ptr_pointee_types.insert(alloca, ty);
        Ok(alloca)
    }

    /// Build a load instruction
    pub fn build_load(
        &mut self,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        // Opaque pointers: the load needs the pointee type explicitly. Recover
        // it from the pointers this builder handed out.
        let pointee = *self.ptr_pointee_types.get(&ptr).ok_or_else(|| {
            CodegenError::InstructionError(format!(
                "cannot load from {ptr:?}: unknown pointee type (opaque pointers \
                 require the pointer to come from build_alloca/build_gep/build_struct_gep \
                 on this value builder)"
            ))
        })?;
        self.builder
            .build_load(pointee, ptr, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build a store instruction
    pub fn build_store(
        &mut self,
        ptr: PointerValue<'ctx>,
        val: BasicValueEnum<'ctx>,
    ) -> CodegenResult<InstructionValue<'ctx>> {
        self.builder
            .build_store(ptr, val)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build a global variable
    pub fn build_global(
        &mut self,
        module: &Module<'ctx>,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
        init: Option<BasicValueEnum<'ctx>>,
        is_constant: bool,
    ) -> CodegenResult<GlobalValue<'ctx>> {
        let global = module.add_global(ty, None, name);
        if let Some(init_val) = init {
            global.set_initializer(&init_val);
        }
        global.set_constant(is_constant);
        Ok(global)
    }

    /// Build a global string pointer
    pub fn build_global_string(
        &mut self,
        module: &Module<'ctx>,
        name: &str,
        value: &str,
    ) -> CodegenResult<GlobalValue<'ctx>> {
        let _ = self
            .builder
            .build_global_string_ptr(value, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        // The global string is already added to module by build_global_string_ptr
        // We need to find it
        let global = module
            .get_global(name)
            .ok_or_else(|| CodegenError::InstructionError("Global string not found".to_string()))?;
        Ok(global)
    }

    /// Build a struct type with named fields
    pub fn build_struct_type(
        &mut self,
        name: &str,
        fields: &[BasicTypeEnum<'ctx>],
        is_packed: bool,
    ) -> StructType<'ctx> {
        self.type_lowering
            .get_or_create_struct(name, fields, is_packed)
    }

    /// Build a struct value (aggregate constant or runtime)
    pub fn build_struct_value(
        &mut self,
        struct_ty: StructType<'ctx>,
        fields: &[BasicValueEnum<'ctx>],
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        // For runtime values, we need to allocate and store each field
        let alloca = self.build_alloca(struct_ty.into(), "struct_tmp")?;
        for (i, field) in fields.iter().enumerate() {
            let gep = self
                .builder
                .build_struct_gep(struct_ty, alloca, i as u32, "field_gep")
                .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
            self.ptr_pointee_types.insert(gep, struct_ty.into());
            self.build_store(gep, *field)?;
        }
        self.build_load(alloca, "struct_val")
    }

    /// Extract a field from a struct value
    pub fn build_extract_value(
        &mut self,
        struct_val: BasicValueEnum<'ctx>,
        index: u32,
        name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        self.builder
            .build_extract_value(struct_val.into_struct_value(), index, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Insert a value into a struct
    pub fn build_insert_value(
        &mut self,
        struct_val: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
        index: u32,
        name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let aggregate = self
            .builder
            .build_insert_value(struct_val.into_struct_value(), value, index, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        // `build_insert_value` yields an `AggregateValueEnum`, which has no
        // `From<AggregateValueEnum> for BasicValueEnum`; widen it by hand.
        Ok(match aggregate {
            inkwell::values::AggregateValueEnum::ArrayValue(v) => v.into(),
            inkwell::values::AggregateValueEnum::StructValue(v) => v.into(),
        })
    }

    /// Build a GEP (getelementptr) for arrays/pointers
    pub fn build_gep(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        ptr: PointerValue<'ctx>,
        indices: &[inkwell::values::BasicValueEnum<'ctx>],
        name: &str,
    ) -> CodegenResult<PointerValue<'ctx>> {
        // GEP indices are always integers.
        let indices: Vec<inkwell::values::IntValue<'ctx>> = indices
            .iter()
            .map(|idx| {
                IntValue::try_from(*idx).map_err(|_| {
                    CodegenError::InstructionError("GEP indices must be integer values".to_string())
                })
            })
            .collect::<CodegenResult<Vec<_>>>()?;

        // SAFETY: `ty` is the element type of the pointee as tracked by
        // `ptr_pointee_types` (or supplied by the caller), and the indices are
        // integers, which is what LLVMBuildGEP2 requires.
        let gep = unsafe {
            self.builder
                .build_gep(ty, ptr, &indices, name)
                .map_err(|e| CodegenError::InstructionError(e.to_string()))?
        };
        self.ptr_pointee_types.insert(gep, ty);
        Ok(gep)
    }

    /// Build a struct GEP
    pub fn build_struct_gep(
        &mut self,
        struct_ty: StructType<'ctx>,
        ptr: PointerValue<'ctx>,
        index: u32,
        name: &str,
    ) -> CodegenResult<PointerValue<'ctx>> {
        let gep = self
            .builder
            .build_struct_gep(struct_ty, ptr, index, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        self.ptr_pointee_types.insert(gep, struct_ty.into());
        Ok(gep)
    }

    /// Build a function call
    pub fn build_call(
        &mut self,
        func: FunctionValue<'ctx>,
        args: &[BasicValueEnum<'ctx>],
        name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let args: Vec<BasicMetadataValueEnum<'ctx>> = args
            .iter()
            .map(|a| BasicMetadataValueEnum::from(*a))
            .collect();
        let call_site = self
            .builder
            .build_call(func, &args, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        call_site
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| CodegenError::InstructionError("Call returned void".to_string()))
    }

    /// Build an indirect function call
    pub fn build_indirect_call(
        &mut self,
        fn_ty: FunctionType<'ctx>,
        func_ptr: PointerValue<'ctx>,
        args: &[BasicValueEnum<'ctx>],
        name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let args: Vec<BasicMetadataValueEnum<'ctx>> = args
            .iter()
            .map(|a| BasicMetadataValueEnum::from(*a))
            .collect();
        let call_site = self
            .builder
            .build_indirect_call(fn_ty, func_ptr, &args, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        call_site.try_as_basic_value().basic().ok_or_else(|| {
            CodegenError::InstructionError("Indirect call returned void".to_string())
        })
    }

    /// Build a return instruction
    pub fn build_return(
        &mut self,
        val: Option<BasicValueEnum<'ctx>>,
    ) -> CodegenResult<InstructionValue<'ctx>> {
        self.builder
            .build_return(val.as_ref().map(|v| v as &dyn BasicValue<'ctx>))
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build an unconditional branch
    pub fn build_unconditional_branch(
        &mut self,
        dest: BasicBlock<'ctx>,
    ) -> CodegenResult<InstructionValue<'ctx>> {
        self.builder
            .build_unconditional_branch(dest)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build a conditional branch
    pub fn build_conditional_branch(
        &mut self,
        cond: inkwell::values::IntValue<'ctx>,
        then_bb: BasicBlock<'ctx>,
        else_bb: BasicBlock<'ctx>,
    ) -> CodegenResult<InstructionValue<'ctx>> {
        self.builder
            .build_conditional_branch(cond, then_bb, else_bb)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build a PHI node
    pub fn build_phi(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::PhiValue<'ctx>> {
        self.builder
            .build_phi(ty, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Add incoming values to a PHI node
    pub fn add_phi_incoming(
        &mut self,
        phi: &inkwell::values::PhiValue<'ctx>,
        values: &[(&BasicValueEnum<'ctx>, BasicBlock<'ctx>)],
    ) {
        // inkwell 0.10 takes `&[(&dyn BasicValue, BasicBlock)]`.
        let incoming: Vec<(&dyn BasicValue<'ctx>, BasicBlock<'ctx>)> = values
            .iter()
            .map(|(v, bb)| (*v as &dyn BasicValue<'ctx>, *bb))
            .collect();
        phi.add_incoming(&incoming);
    }

    /// Build integer constant
    pub fn build_int_constant(
        &mut self,
        ty: inkwell::types::IntType<'ctx>,
        value: u64,
        _name: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        ty.const_int(value, false)
    }

    /// Build float constant
    pub fn build_float_constant(
        &mut self,
        ty: inkwell::types::FloatType<'ctx>,
        value: f64,
        _name: &str,
    ) -> inkwell::values::FloatValue<'ctx> {
        ty.const_float(value)
    }

    /// Build a zero value for a type
    pub fn build_zero(&mut self, ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        ty.const_zero()
    }

    /// Build an undefined value for a type
    pub fn build_undef(&mut self, ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        // `BasicTypeEnum` has no `get_undef` in inkwell 0.10; dispatch per variant.
        match ty {
            BasicTypeEnum::ArrayType(t) => t.get_undef().into(),
            BasicTypeEnum::FloatType(t) => t.get_undef().into(),
            BasicTypeEnum::IntType(t) => t.get_undef().into(),
            BasicTypeEnum::PointerType(t) => t.get_undef().into(),
            BasicTypeEnum::StructType(t) => t.get_undef().into(),
            BasicTypeEnum::VectorType(t) => t.get_undef().into(),
            BasicTypeEnum::ScalableVectorType(t) => t.get_undef().into(),
        }
    }

    /// Build integer arithmetic
    pub fn build_int_add(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_int_add(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    pub fn build_int_sub(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_int_sub(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    pub fn build_int_mul(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_int_mul(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    pub fn build_int_signed_div(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_int_signed_div(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build integer comparison
    pub fn build_int_compare(
        &mut self,
        pred: inkwell::IntPredicate,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_int_compare(pred, lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build bitwise operations
    pub fn build_and(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_and(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    pub fn build_or(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_or(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    pub fn build_xor(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_xor(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Build shift operations
    pub fn build_left_shift(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_left_shift(lhs, rhs, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    pub fn build_right_shift(
        &mut self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
        is_arithmetic: bool,
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        self.builder
            .build_right_shift(lhs, rhs, is_arithmetic, name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Add metadata to the module
    pub fn add_metadata(
        &mut self,
        module: &Module<'ctx>,
        kind: &str,
        value: &str,
    ) -> CodegenResult<MetadataValue<'ctx>> {
        let md_string = self.type_lowering.context().metadata_string(value);
        let md_node = self
            .type_lowering
            .context()
            .metadata_node(&[BasicMetadataValueEnum::from(md_string)]);
        module
            .add_global_metadata(kind, &md_node)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        self.metadata.insert(kind.to_string(), md_node);
        Ok(md_node)
    }

    /// Get or create debug location
    ///
    /// `DILocation`s can only be minted by a `DebugInfoBuilder`, so the caller
    /// supplies one (inkwell has no way to attach a DIBuilder to a `Builder`).
    pub fn create_debug_location(
        &self,
        debug_info: &inkwell::debug_info::DebugInfoBuilder<'ctx>,
        line: u32,
        col: u32,
        scope: inkwell::debug_info::DIScope<'ctx>,
    ) -> inkwell::debug_info::DILocation<'ctx> {
        debug_info.create_debug_location(self.type_lowering.context(), line, col, scope, None)
    }

    /// Set debug location for subsequent instructions
    pub fn set_debug_location(&mut self, loc: inkwell::debug_info::DILocation<'ctx>) {
        self.builder.set_current_debug_location(loc);
    }

    /// Clear debug location
    pub fn clear_debug_location(&mut self) {
        self.builder.unset_current_debug_location();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};

    #[test]
    fn test_value_builder_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let llvm_context = context.llvm_context();
        let builder = llvm_context.create_builder();
        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let _value_builder = LlvmValueBuilder::new(builder, type_lowering);
        // Just test it compiles
    }

    #[test]
    fn test_build_function() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("test");
        let builder = llvm_context.create_builder();
        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let mut value_builder = LlvmValueBuilder::new(builder, type_lowering);

        let fn_val = value_builder.build_void_function(&module, "test_fn", &[], &[], |_, _| Ok(()));
        assert!(fn_val.is_ok());
    }

    /// A function body that emits nothing must still get a terminator.
    ///
    /// Without one the module is `define void @f() { entry: }`, which is invalid IR:
    /// `llc` rejects it and LLVM's verifier reports a missing terminator. This was the
    /// failure behind `test_build_function` and `test_build_alloca`, neither of which
    /// had ever passed. The module-level `verify` below is the real check -- a test that
    /// only called `build_function` would still pass with an unterminated block.
    #[test]
    fn an_empty_body_still_produces_a_terminated_function() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("test");
        let builder = llvm_context.create_builder();
        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let mut value_builder = LlvmValueBuilder::new(builder, type_lowering);

        value_builder
            .build_void_function(&module, "empty", &[], &[], |_, _| Ok(()))
            .expect("an empty void body must still build");

        let ir = module.print_to_string().to_string();
        module
            .verify()
            .unwrap_or_else(|e| panic!("LLVM rejected the module: {e}\n{ir}"));
        assert!(
            ir.contains("ret void"),
            "an empty void function must end in `ret void`: {ir}"
        );
    }

    /// A body that already emitted its own `ret` must not get a second one.
    ///
    /// Appending an unconditional terminator to an already-terminated block produces IR
    /// with unreachable instructions after the return, which is at best noise and can be
    /// rejected outright depending on the block layout.
    #[test]
    fn a_body_that_already_terminates_is_not_terminated_twice() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("test");
        let builder = llvm_context.create_builder();
        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let mut value_builder = LlvmValueBuilder::new(builder, type_lowering);

        value_builder
            .build_void_function(&module, "already", &[], &[], |vb, _| {
                vb.builder()
                    .build_return(None)
                    .map_err(|e| crate::codegen::CodegenError::InstructionError(e.to_string()))?;
                Ok(())
            })
            .expect("a self-terminated body must build");

        let ir = module.print_to_string().to_string();
        module
            .verify()
            .unwrap_or_else(|e| panic!("LLVM rejected the module: {e}\n{ir}"));
        assert_eq!(
            ir.matches("ret void").count(),
            1,
            "exactly one `ret void` is expected, not a duplicate: {ir}"
        );
    }

    #[test]
    fn test_build_alloca() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("test");
        let builder = llvm_context.create_builder();
        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let mut value_builder = LlvmValueBuilder::new(builder, type_lowering);

        let fn_val = value_builder.build_void_function(&module, "test_fn", &[], &[], |vb, _| {
            let alloca =
                vb.build_alloca(vb.type_lowering().context().i32_type().into(), "test_var")?;
            assert!(!alloca.is_null());
            Ok(())
        });
        assert!(fn_val.is_ok());
    }
}
