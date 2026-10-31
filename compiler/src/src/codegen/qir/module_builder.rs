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
use std::collections::{HashMap, HashSet};

/// The operation name if `expr` is a quantum operation, otherwise `""`.
///
/// Used at the point a `let` binds a value, to decide whether the name being bound is a
/// qubit. Only a direct quantum operation is recognised: anything else -- a computation, a
/// scalar read, a call -- binds a name that is not a qubit, and an unrecognised shape is left
/// on the classical path so it is not refused as a quantum operand.
fn value_op(expr: &PirExpr) -> &str {
    match expr {
        PirExpr::QuantumOp { op, .. } => op.as_str(),
        _ => "",
    }
}

/// Whether a lowered quantum operation PRODUCES a qubit rather than acting on one.
///
/// `qalloc` allocates. Every other quantum operation either acts on existing qubits or
/// returns a classical result, so claiming a pointer for those would be wrong in the other
/// direction.
///
/// This is what distinguishes `ptr null` from `i64 0` as the value of `let [1] q = qalloc(1)`.
/// QIR models a qubit as a pointer while the builder's generic placeholder is an integer, and
/// an integer passed to `qir.h(ptr)` is rejected by the LLVM verifier. Deciding this from the
/// operation is the only place that knows both facts.
fn qir_intrinsic_returns_qubit(op: &str) -> bool {
    op.eq_ignore_ascii_case("qalloc")
}

/// The QIR intrinsic name a lowered quantum operation name refers to, if one exists.
///
/// # Why the mapping is explicit
///
/// Lowering names operations the way `GateKind`'s `Display` spells them -- `"H"`, `"CX"`,
/// `"reset"` -- and names allocation `"qalloc"`. The intrinsic table declares `qir.h`,
/// `qir.cx` and `qir.qubit_alloc`. Concatenating `qir.{op}` therefore looked up names that do
/// not exist, and the backend refused every gate and every allocation. This was the second of
/// the two defects that stopped the QIR backend emitting any circuit.
///
/// # Why `None` rather than a guess
///
/// An unmapped operation returns `None`, which the caller turns into a refusal. Falling back
/// to `qir.h` -- which a former, deleted QIR gate table once did -- would turn a mis-spelled or
/// not-yet-supported operation into a Hadamard: working code applying the wrong unitary.
fn qir_intrinsic_for(op: &str) -> Option<&'static str> {
    // Lowering names a gate the way `GateKind`'s `Display` spells it -- "H", "CX" -- while
    // hand-built PIR and the hardware exporters use the lowercase
    // intrinsic-style name -- "h", "cx". Both spellings name the same operation, so both are
    // accepted here. Normalising on case rather than duplicating every arm keeps the mapping
    // to one line per operation, so a gate cannot be added to one spelling and forgotten in
    // the other.
    let op = op.to_ascii_lowercase();
    Some(match op.as_str() {
        // Allocation and release. Not gates, but quantum operations with no `GateKind`.
        "qalloc" => "qir.qubit_alloc",
        "qfree" => "qir.qubit_release",

        // Single-qubit unitaries.
        "h" | "hadamard" => "qir.h",
        "x" | "pauli_x" => "qir.x",
        "y" | "pauli_y" => "qir.y",
        "z" | "pauli_z" => "qir.z",
        "s" => "qir.s",
        "t" => "qir.t",
        "rx" => "qir.rx",
        "ry" => "qir.ry",
        "rz" => "qir.rz",

        // Two-qubit gates.
        "cx" | "cnot" => "qir.cx",
        "cy" => "qir.cy",
        "cz" => "qir.cz",

        // Measurement. The base profile declares `qir.mz`/`qir.mx`/`qir.my` for a per-basis
        // read and a `qir.measure` that writes through a result pointer. Naso measures in the
        // computational basis, so `mz` is the read that matches.
        "measure" | "mz" => "qir.mz",
        "mx" => "qir.mx",
        "my" => "qir.my",

        // `phase(theta, q)` is a Z rotation by `theta`.
        "phase" => "qir.rz",

        // Two-qubit exchange, and the three-qubit Toffoli.
        "swap" => "qir.swap",
        "iswap" => "qir.iswap",
        "ccx" | "toffoli" => "qir.ccx",

        // The dynamic family: a call whose arity is chosen at runtime. QIR takes these
        // through `qir.controlled`, with `qir.adjoint` for a reversed circuit.
        "controlled" => "qir.controlled",
        "adjoint" => "qir.adjoint",

        // `reset(q)` returns the qubit to |0> IN PLACE. The base profile declares no reset
        // intrinsic, so it is REFUSED rather than approximated by a release -- a release
        // hands the qubit away, which is not what `reset` means.
        "reset" => return None,

        // `entangle(q...)` has no single-intinsic implementation, and approximating it by a
        // CNOT would silently compute something else. Refused; see the module note.
        "entangle" => return None,

        // Anything else: an operation with no declared intrinsic. See above on why this is
        // not a guess.
        _ => return None,
    })
}

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

    /// Names this module bound to a QUBIT, as opposed to a scalar.
    ///
    /// The backend emits one `void` function per PIR statement, so a qubit bound by
    /// `qalloc` in an earlier statement is genuinely not in scope when a later statement uses
    /// it. That must be refused -- applying the gate to a placeholder integer would emit valid
    /// QIR that is not the source program -- but a CLASSICAL binding that crosses statements
    /// is ordinary and must keep working.
    ///
    /// This set is what tells the two apart, and it is populated at the point of allocation,
    /// where the backend knows the value is a qubit.
    quantum_var: HashSet<String>,

    /// Whether the `Let` being built occupies a whole statement, and so must outlive its own
    /// fabricated body.
    ///
    /// `LetBinding` carries no body, so lowering `let a = qalloc(1);` at statement position
    /// produces a `Let` whose `body` is a placeholder. Popping the binding after that
    /// placeholder made it invisible to the NEXT statement, so a gate applied to `a` read an
    /// unbound name.
    ///
    /// The flag is set by the CALLER, never inferred from the body's shape: a genuine
    /// expression body of `0` is indistinguishable from the placeholder, so guessing would
    /// either leak a binding past its scope or wrongly free one.
    in_statement_position: bool,
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
            quantum_var: HashSet::new(),
            in_statement_position: false,
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
            quantum_var: HashSet::new(),
            in_statement_position: false,
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

        // Build every statement into ONE function, in order.
        //
        // This used to emit one `void` function per statement (`qir_stmt_<id>`), which had two
        // consequences, both visible in the output rather than inferred:
        //
        // 1. NOTHING CALLED those functions and nothing gave them an order. A module of
        //    unreferenced functions does not specify a circuit -- a consumer cannot know the
        //    `h` must precede the `cx`.
        // 2. Locals could not cross a statement boundary, because each function has its own
        //    frame, so a `[1]` qubit bound by `qalloc` in one statement was unbound in the
        //    next and the backend substituted the integer 0.
        //
        // One entry function fixes both by construction: statements are emitted in sequence
        // into a single basic block, so their order is the program's order and a binding made
        // by one statement is visible to the next.
        let void_type = self.llvm_context.void_type();
        let fn_type = void_type.fn_type(&[], false);
        let function = self.module.add_function("qir_entry", fn_type, None);
        self.current_function = Some(function);
        let entry = self.llvm_context.append_basic_block(function, "entry");
        self.current_block = Some(entry);
        self.builder.position_at_end(entry);

        for stmt in &pir_module.statements {
            self.build_statement(stmt)?;
        }

        self.builder
            .build_return(None)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        self.current_function = None;
        self.current_block = None;

        // Verify the module
        self.module
            .verify()
            .map_err(|e| CodegenError::VerificationError(e.to_string()))?;

        Ok(())
    }

    /// Build one PIR statement into the open `qir_entry` function.
    ///
    /// This no longer creates a function, and it no longer clears the locals. Both were what
    /// made a binding from an earlier statement invisible, and clearing them mid-function
    /// would be the same bug in a new place: `variables` maps a name to its `alloca`, and an
    /// alloca belongs to the function that created it.
    fn build_statement(&mut self, stmt: &PirStatement) -> CodegenResult<()> {
        if self.current_function.is_none() {
            return Err(CodegenError::QirError(
                "build_statement was called with no open entry function".to_string(),
            ));
        }
        // Build the statement body.
        //
        // A statement-position `Let` carries a fabricated body, so the flag is set here -- at
        // the CALLER, which knows this is a whole statement -- rather than inferred from the
        // body's shape. A genuine expression body of `0` is indistinguishable from the
        // placeholder, so guessing would either leak a binding or wrongly free one.
        let was = self.in_statement_position;
        self.in_statement_position = true;
        let result = self.build_expr(&stmt.body);
        self.in_statement_position = was;
        result?;
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
                } else if self.quantum_var.contains(name) {
                    //
                    // REFUSED, not zero -- but ONLY for a quantum operand.
                    //
                    // A `[1]` qubit bound in an EARLIER `PirStatement` is unbound here,
                    // because every statement is emitted as its own `void` function with its
                    // own `variables` map (see `build_statement`). So a two-statement circuit
                    //
                    //     let [1] a: Qubit = qalloc(1);  // statement 0
                    //     hadamard(a);                    // statement 1
                    //
                    // found `a` unbound and used the integer 0, which LLVM's verifier then
                    // rejected. Returning zero also hides the real problem: the qubit exists,
                    // it is just not reachable from here.
                    //
                    // The scope of this refusal is deliberately narrow. A CLASSICAL binding
                    // that crosses statements is legal and must keep working -- the module's
                    // statements are the Naso program's statements, and a scalar computed in
                    // one and used in the next is ordinary. Refusing those would reject
                    // correct programs to fix a quantum-specific problem. So the check applies
                    // only to names the quantum binder introduced.
                    Err(CodegenError::QirError(format!(
                        "quantum operand `{name}` is not bound in this statement. The QIR \
                         backend emits one void function per PIR statement, so a qubit \
                         allocated in an earlier statement is not in scope here. Emitting a \
                         placeholder instead would apply the gate to something other than \
                         that qubit."
                    )))
                } else {
                    // A classical name that is genuinely unbound. The zero keeps the previous
                    // behaviour for this path; it is not a quantum operand, so it cannot
                    // misidentify a qubit.
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

                // If the bound value is a qubit, remember that. `build_statement` clears
                // `variables` between statements, so this set is what lets a later
                // statement distinguish "a qubit that went out of scope" from "a scalar that
                // was never defined".
                if qir_intrinsic_returns_qubit(value_op(value)) {
                    self.quantum_var.insert(name.clone());
                }

                let result = self.build_expr(body)?;

                // A statement-position `let` outlives its own statement. See the field's doc
                // comment: the placeholder body would otherwise end the binding's scope
                // immediately, and the next statement could not see it. An expression-position
                // `let` still scopes normally, which is what keeps a binding inside a block
                // from leaking.
                if !self.in_statement_position {
                    self.variables.remove(name);
                    self.quantum_var.remove(name);
                }
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
            //
            // REFUSED, matching the LLVM backend.
            //
            // This ran `body` and dropped `inverse`, on the comment "For QIR, we just
            // build the body / The inverse would be handled by quantum compiler".
            //
            // There is no quantum compiler doing that. Nothing else in this backend
            // reads `inverse`. The comment described work that does not exist.
            //
            // (`ancilla_emission.rs` used to be named here as the exception. It was a
            // 682-line scaffold with no `mod` declaration in `qir/mod.rs`, so it was never
            // compiled and never read anything. It has been deleted rather than wired.)
            //
            // For a QIR program this is the worst place to be wrong. QIR's entire
            // contract is that the emitted circuit is reversible, and a `Reversible`
            // whose inverse is dropped emits a circuit that is not: it is the forward
            // operation with no adjoint, so the garbage the block existed to uncompute
            // stays in the register. Emitting that under a QIR type signature asserts
            // reversibility the output does not have.
            //
            // Refusing is also the honest answer about what this backend can verify:
            // nothing here executes on a QPU or checks an adjoint, so there is no
            // evidence a `Reversible` came out reversible.
            PirExpr::Reversible { .. } => Err(CodegenError::UnsupportedFeature(
                "a `Reversible` block reached QIR codegen, and this backend does not \
                 emit adjoints. The INVERSE is not optional -- it is the part that \
                 uncomputes, and QIR's contract is that the emitted circuit is \
                 reversible. Emitting the body alone would produce the forward \
                 operation with no adjoint, asserting a reversibility the output does \
                 not have. Refused rather than emitting a circuit that lies about \
                 being reversible."
                    .to_string(),
            )),
            //
            // REFUSED, not ignored.
            //
            // A `return` in the middle of a QIR function needs the function's result
            // type threaded into `build_expr`, which only the function emitter knows.
            // Guessing it would coerce the returned value to something the signature
            // does not say, which is a wrong answer wearing a valid signature.
            //
            // The LLVM backend emits it. This one refuses until it can do the same
            // without inventing a type.
            PirExpr::Return { value } => Err(CodegenError::UnsupportedFeature(
                match value {
                    Some(_) => {
                        "a `return` with a value inside a nested block is not \
                                emitted by the QIR backend yet. Emitting it would mean \
                                inventing the function's result type here, which only \
                                the function emitter knows, and coercing the returned \
                                value to a type the signature does not state."
                    }
                    None => {
                        "a bare `return` inside a nested block is not emitted by \
                             the QIR backend yet."
                    }
                }
                .to_string(),
            )),
            //
            // REFUSED, not passed through.
            //
            // `Index` used to discard the subscript and return the base, so `q[1]` and `q[2]`
            // -- two DIFFERENT qubits -- emitted as the same operand. Combined with the
            // per-statement function split below, that made the emitted QIR a valid LLVM
            // module that was NOT the source program: a fixture saying
            //
            //     H q[1]; CNOT q[1], q[2]
            //
            // emitted `h` on one allocation and `cx` on two others, i.e. a circuit applying
            // gates to unrelated qubits. It passed validation, and the tests asserted only
            // that the text `call void @qir.h(` appeared -- which it did.
            //
            // That is the worst class of failure here: valid output, wrong quantum program.
            // Selecting a qubit out of a register array needs a `getelementptr` and a load
            // from a real allocation, which this backend does not build. Refused until it
            // does.
            PirExpr::Index { base, .. } => Err(CodegenError::UnsupportedFeature(format!(
                "indexing `{base:?}` is not emitted by the QIR backend. Selecting a qubit \
                 out of a register array needs a real allocation to index into, and \
                 discarding the subscript would apply gates to the wrong qubits -- emitting \
                 valid QIR that does not compute the source program."
            ))),
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
                // Map the lowering's operation name to a QIR intrinsic.
                //
                // Lowering emits `GateKind`'s Display spelling -- "H", "CX", "reset" -- plus
                // `qalloc` for allocation. Those are NOT QIR intrinsic names: the intrinsic
                // table declares `qir.h`, `qir.cx` and `qir.qubit_alloc`. Building the name
                // as `qir.{op}` therefore looked up `qir.H` and `qir.qalloc`, neither of
                // which exists, so every gate AND every allocation was refused. This was the
                // second of the two defects that stopped the QIR backend emitting any circuit.
                let intrinsic_name = qir_intrinsic_for(op).ok_or_else(|| {
                    CodegenError::QirError(format!("no QIR intrinsic for quantum operation `{op}`"))
                })?;
                let func = self.get_intrinsic(intrinsic_name).ok_or_else(|| {
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
                // build_expr must return one.
                if let Some(value) = call.try_as_basic_value().basic() {
                    return Ok(value);
                }

                // `qir.qubit_alloc` returns a Qubit, which QIR models as a POINTER. The
                // generic placeholder below is an integer, so returning it here typed the
                // binding as `i64`, and every later use of that qubit passed an `i64` where a
                // gate wants a `ptr` -- which the LLVM verifier rejects. A qubit-producing
                // operation therefore yields a null pointer of the right type.
                if qir_intrinsic_returns_qubit(op) {
                    return Ok(self
                        .llvm_context
                        .ptr_type(AddressSpace::default())
                        .const_null()
                        .into());
                }
                Ok(self.result_type.const_zero().into())
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

    /// Every operation `qir_intrinsic_for` resolves MUST name a declared intrinsic.
    ///
    /// # Why this is the invariant, not "there is one gate table"
    ///
    /// This backend used to carry a second, undeclared gate-name table in
    /// `qir/classifier.rs` -- a full duplicate of the one above, with its own arms. Two
    /// copies of an intrinsic map do not fail to build when they disagree, they emit a
    /// call to a symbol that does not exist, or silently apply the wrong unitary. That
    /// file is now deleted, and this test is what keeps the property that motivated its
    /// deletion: a name reaching `call_intrinsic` has to be a real, declared entry point.
    ///
    /// The deleted classifier mapped `sdg` to `qir.s__adj` "because S-dagger is the
    /// adjoint of S". That name is not in the QIR base profile and is not in
    /// `QIR_INTRINSICS` at all, so that mapping was correct as quantum and broken as code:
    /// it passed the string checks written about it and named a symbol no runtime has.
    /// `sdg` is correctly REFUSED here instead, via the `_ => return None` arm, because
    /// the base profile declares no S-dagger entry point and there is no angle-taking
    /// intrinsic to express one as.
    ///
    /// Enumerated as DATA rather than by scanning source text: a source scan cannot
    /// enumerate the arms, so it cannot cover a table that grows.
    #[test]
    fn every_resolved_intrinsic_is_a_declared_one_and_unknown_ops_are_refused() {
        // Every operation the lowering can name. `reset` and `entangle` are excluded
        // because they resolve to None on purpose, and are asserted separately below.
        let ops = [
            "qalloc",
            "qfree",
            "h",
            "hadamard",
            "x",
            "pauli_x",
            "y",
            "pauli_y",
            "z",
            "pauli_z",
            "s",
            "t",
            "rx",
            "ry",
            "rz",
            "cx",
            "cnot",
            "cy",
            "cz",
            "measure",
            "mz",
            "mx",
            "my",
            "phase",
            "swap",
            "iswap",
            "ccx",
            "toffoli",
            "controlled",
            "adjoint",
        ];

        for op in ops {
            let resolved = qir_intrinsic_for(op)
                .unwrap_or_else(|| panic!("`{op}` is lowered but resolves to no intrinsic"));
            assert!(
                QIR_INTRINSICS.iter().any(|i| i.name == resolved),
                "`{op}` resolves to `{resolved}`, which is not in QIR_INTRINSICS. The call \
                 would name a symbol no runtime defines. Either declare it in `primitives.rs` \
                 or refuse the operation -- never resolve to a name that does not exist."
            );
        }

        // NO DUPLICATE NAMES in the declaration table.
        //
        // `qir.r1` was declared TWICE here -- once already present, once added again when the
        // rotation angle was plumbed through, because the new entry was not checked against
        // the existing list. LLVM does not reject a repeated declaration of the same symbol;
        // it renames the second to `qir.r1.1` and emits it. So the module declared
        //
        //     declare void @qir.r1(double, ptr)
        //     declare void @qir.r1.1(double, ptr)
        //
        // and `qir.r1.1` names a function no runtime defines. Every assertion above still
        // passed, and the emitted text looked plausible on a glance: it had the right
        // intrinsic, with the right signature, on the right qubit.
        //
        // Uniqueness is checked rather than left to review because the failure is invisible to
        // the existing invariants -- each of them asks whether a RESOLVED name is present, and
        // `qir.r1` is present twice, so both questions pass. It also does not surface as a
        // link error in normal use, because nothing links QIR text output; it surfaces only as
        // a bogus symbol in the emitted module.
        let mut seen = std::collections::HashSet::new();
        for intrinsic in QIR_INTRINSICS {
            assert!(
                seen.insert(intrinsic.name),
                "`{}` is declared more than once in QIR_INTRINSICS. LLVM renames the repeat to \
                 `{}.1` and emits it, so the module would declare a symbol no runtime defines. \
                 Two entries for one intrinsic means one of them was added without checking the \
                 list -- if the second has the same signature, it is pure duplication; if it \
                 differs, the table is ambiguous about what the intrinsic actually is.",
                intrinsic.name,
                intrinsic.name
            );
        }

        // The two refusals, asserted individually. `entangle` approximated by a CNOT and
        // `reset` approximated by a release both compile into a circuit computing
        // something else, which is the fabrication class this mapping must not have.
        assert_eq!(
            qir_intrinsic_for("reset"),
            None,
            "`reset(q)` returns the qubit to |0> IN PLACE. A release hands the qubit away, \
             which is not what reset means."
        );
        assert_eq!(
            qir_intrinsic_for("entangle"),
            None,
            "`entangle` has no single-intrinsic implementation; a CNOT would be a different \
             operation."
        );

        // No fallback: an operation with no intrinsic is refused, never defaulted. A
        // default of `qir.h` is what turned a mis-spelled gate name into working code
        // applying a Hadamard.
        //
        // `sdg` and `tdg` are in this list deliberately. They are real, correct quantum
        // operations with no base-profile entry point, and QIR's answer for them is a
        // Z rotation by a sign-dependent angle -- which is exactly the rotation this
        // backend refuses to emit, because it has no angle to supply. Refusing is the
        // honest result; mapping them onto `s`/`t` would apply the wrong phase.
        for op in ["sdg", "tdg", "", "not_a_gate", "H2", "ccz", "ryz"] {
            assert_eq!(
                qir_intrinsic_for(op),
                None,
                "`{op}` has no intrinsic and must be refused, not resolved to something"
            );
        }
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
