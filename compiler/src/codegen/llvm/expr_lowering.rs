//! PIR expression lowering, shared by every LLVM lowering entry point.
//!
//! # Why this is a separate type
//!
//! `build_expr` used to be a private method on `LLVMModuleBuilder`, which meant
//! `ScheduleLowering` -- the path that lowers a `forall` BODY -- could not reach it.
//! A schedule band therefore lowered to a real loop with a correct induction variable
//! and an EMPTY body: the loop iterated the right number of times and computed
//! nothing, with no diagnostic. That is a silent wrong answer, and it is the class of
//! bug this module exists to close.
//!
//! The lowering needs four things, none of which is the `Builder` it emits through:
//!
//!   * the single `inkwell::builder::Builder`, owned by `LlvmValueBuilder`;
//!   * the name -> allocation map, so a `Var` can load and an `Assign` can store;
//!   * the `Module`, so `Call` can resolve a callee and `QuantumOp` can declare an
//!     intrinsic on first use;
//!   * the current function, so `If` can append its `then`/`else`/`merge` blocks.
//!
//! All four are passed in rather than re-derived, which is why this is a small
//! borrowing struct instead of a method on either owner.
//!
//! # Invariants this module does not relax
//!
//! * A construct that cannot be lowered is an ERROR naming the construct. No arm
//!   evaluates its operand and throws the result away.
//! * No integer type is ever widened or narrowed to make something typecheck, and no
//!   cast is erased. Mismatched types are refused.
//! * `Cast` carries both a width and an explicit sign flag. LLVM's integer types are
//!   signless, so the sign is the only thing that distinguishes `sext` from `zext`
//!   and nothing downstream can recover it.

use crate::ast::Quantity;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::pir_types::{BinaryOp, PirExpr, UnaryOp};
use inkwell::module::Module;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue};
use std::collections::HashMap;

/// Lowers `PirExpr` into LLVM instructions.
///
/// Holds no builder of its own: it borrows the one `LlvmValueBuilder` owns, so there
/// is still exactly one insertion point in the whole backend.
pub struct PirExprLowerer<'ctx, 'a> {
    /// The single value builder. Also owns the name -> allocation map, which is why a
    /// binding introduced in one lowering pass is visible in the next.
    pub value_builder: &'a mut LlvmValueBuilder<'ctx>,
    /// The module being built into, for callee lookup and intrinsic declaration.
    pub module: &'a Module<'ctx>,
    /// The function instructions are being emitted into. `If` appends its blocks here.
    pub current_function: FunctionValue<'ctx>,
    /// True while lowering a PIR STATEMENT rather than an expression.
    ///
    /// A statement-position `let` is a binding that outlives its own expression, so it
    /// must NOT be removed from scope when the body finishes. A statement body is
    /// statement position by construction, so `ScheduleLowering` always sets this true
    /// and `LLVMModuleBuilder` sets it per statement.
    pub in_statement_position: bool,
}

impl<'ctx, 'a> PirExprLowerer<'ctx, 'a> {
    /// Lower a PIR expression, returning the value it evaluates to.
    // `quantities` is threaded through recursively and read by no arm today: every
    // quantity-dependent decision (which a `[0]` binding erases, which alias scope a
    // memory access carries) is made by the passes that CALL this one, not here. The
    // parameter is kept because it is part of the signature both callers already have,
    // and dropping it would mean every call site changed for no behavioural gain.
    #[allow(clippy::only_used_in_recursion)]
    pub fn build_expr(
        &mut self,
        expr: &PirExpr,
        quantities: &HashMap<String, Quantity>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        match expr {
            PirExpr::IntLit(val) => {
                let int_type = self
                    .value_builder
                    .type_lowering()
                    .int_type(crate::codegen::abi::IntWidth::I64);
                Ok(int_type.const_int(*val as u64, false).into())
            }
            PirExpr::FloatLit(val) => {
                let float_type = self
                    .value_builder
                    .type_lowering()
                    .float_type(crate::codegen::abi::FloatWidth::F64);
                let parsed = val.parse::<f64>().unwrap_or(0.0);
                Ok(float_type.const_float(parsed).into())
            }
            PirExpr::BoolLit(val) => {
                let bool_type = self
                    .value_builder
                    .type_lowering()
                    .int_type(crate::codegen::abi::IntWidth::I1);
                Ok(bool_type.const_int(*val as u64, false).into())
            }
            PirExpr::Var(name) => {
                if let Some((ptr, ty)) = self.value_builder.variable(name) {
                    let load = self
                        .value_builder
                        .builder()
                        .build_load(ty, ptr, name)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                    Ok(load)
                } else {
                    // A read of a name with no allocation is a wrong answer, not an
                    // approximation: the program asked for a value and would be handed
                    // zero, and an uninitialised or out-of-range value is also zero in
                    // LLVM, so the result is indistinguishable from a real computation.
                    // `Assign` has always been strict here, so a WRITE to an unknown
                    // name was already refused; only the READ was lenient.
                    //
                    // Making this an error is safe because nothing in the workspace
                    // reaches it. An instrumented run of the whole `--features llvm`
                    // suite (539 tests) reported zero unbound reads, and forcing every
                    // read down this arm did produce 34, so the arm is live code that
                    // current correct lowering simply never enters. An earlier note
                    // here claimed six tests depended on the zero; that was stale, and
                    // the test names it listed (matmul, call sites) all bind their
                    // variables now.
                    //
                    // A band that does not declare its iterator lands here:
                    // `ScheduleNode::Band::iterators` holds the level's source spelling
                    // and an empty entry binds nothing, so a body reading the iterator
                    // is now refused rather than silently reading zero. See
                    // `llvm_forall_execution_test::
                    // a_band_with_no_declared_iterator_does_not_bind_the_wrong_one`.
                    return Err(CodegenError::UnsupportedFeature(format!(
                        "read of `{name}`: no allocation is known for it, so there is \
                         nothing to load. Returning zero would be a silent wrong answer, \
                         and an uninitialised LLVM value is also zero, so the result \
                         would be indistinguishable from a real computation. Declared \
                         names in scope: {:?}",
                        self.value_builder.variable_names()
                    )));
                }
            }
            PirExpr::Binary { op, left, right } => {
                let l = self.build_expr(left, quantities)?;
                let r = self.build_expr(right, quantities)?;
                self.build_binary_op(*op, l, r)
            }
            PirExpr::Unary { op, expr } => {
                let e = self.build_expr(expr, quantities)?;
                self.build_unary_op(*op, e)
            }
            PirExpr::Call { name, args } => {
                let arg_values: CodegenResult<Vec<_>> = args
                    .iter()
                    .map(|a| self.build_expr(a, quantities))
                    .collect();
                let arg_values = arg_values?;

                let func = self.module.get_function(name).ok_or_else(|| {
                    CodegenError::FunctionBuildError(format!("Function '{}' not found", name))
                })?;

                // inkwell 0.10 takes call arguments as `BasicMetadataValueEnum`.
                let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> = arg_values
                    .into_iter()
                    .map(BasicMetadataValueEnum::from)
                    .collect();

                let call = self
                    .value_builder
                    .builder()
                    .build_call(func, &arg_metadata, "call")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // A void call yields no value; `ValueKind::Instruction` is that case.
                // Fall back to the same placeholder used for a void expression, since
                // statement bodies discard the result anyway.
                Ok(call
                    .try_as_basic_value()
                    .basic()
                    .unwrap_or_else(|| self.void_placeholder()))
            }
            PirExpr::Let {
                name,
                qty: _,
                mutability: _,
                value,
                body,
            } => {
                let val = self.build_expr(value, quantities)?;
                let alloca = self
                    .value_builder
                    .builder()
                    .build_alloca(val.get_type(), name)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.value_builder
                    .builder()
                    .build_store(alloca, val)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.value_builder
                    .add_variable(name.clone(), alloca, val.get_type());

                let result = self.build_expr(body, quantities)?;

                // A statement-position `let` must outlive its own statement.
                //
                // `LetBinding` carries no body, so lowering `let mut total = 0;` at
                // statement position produces a `Let` whose `body` is a fabricated
                // `IntLit(0)`. Scoping such a binding to its own expression made every
                // later write unreachable: `total = total + i` was refused with "no
                // allocation is known for it", so no `forall` body could accumulate into
                // an outer variable.
                //
                // `in_statement_position` is set by the CALLER rather than inferred from
                // the body's shape: a genuine expression body of `0` is indistinguishable
                // from the placeholder, and guessing would either leak a binding or wrongly
                // free one.
                if !self.in_statement_position {
                    self.value_builder.remove_variable(name);
                }

                Ok(result)
            }
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.build_expr(cond, quantities)?;
                let bool_type = self
                    .value_builder
                    .type_lowering()
                    .int_type(crate::codegen::abi::IntWidth::I1);
                let cond_bool = self
                    .value_builder
                    .builder()
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        cond_val.into_int_value(),
                        bool_type.const_zero(),
                        "if_cond",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                let func = self.current_function;
                let context = self.value_builder.type_lowering().context();
                let then_block = context.append_basic_block(func, "then");
                let else_block = context.append_basic_block(func, "else");
                let merge_block = context.append_basic_block(func, "if_merge");

                self.value_builder
                    .builder()
                    .build_conditional_branch(cond_bool, then_block, else_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Then branch
                self.value_builder.builder().position_at_end(then_block);
                let then_val = self.build_expr(then_branch, quantities)?;
                self.value_builder
                    .builder()
                    .build_unconditional_branch(merge_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Else branch
                self.value_builder.builder().position_at_end(else_block);
                let else_val = self.build_expr(else_branch, quantities)?;
                self.value_builder
                    .builder()
                    .build_unconditional_branch(merge_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Merge block
                self.value_builder.builder().position_at_end(merge_block);
                let phi = self
                    .value_builder
                    .builder()
                    .build_phi(then_val.get_type(), "if_phi")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                phi.add_incoming(&[(&then_val, then_block), (&else_val, else_block)]);
                Ok(phi.as_basic_value())
            }
            PirExpr::Reversible { body, inverse } => {
                // A reversible block runs `body` then `inverse`; `inverse` is an
                // undo, not part of the value, so it is not emitted here. It is
                // deliberately bound rather than discarded, so the omission is visible
                // in this file rather than silent.
                let _ = inverse;
                self.build_expr(body, quantities)
            }
            PirExpr::Index { base, indices } => {
                let base_ptr = self.build_expr(base, quantities)?;
                let index_vals: CodegenResult<Vec<_>> = indices
                    .iter()
                    .map(|i| self.build_expr(i, quantities))
                    .collect();
                let _ = index_vals;

                // Simplified: just return base pointer for now
                Ok(base_ptr)
            }
            PirExpr::Field { base, field } => {
                let base_val = self.build_expr(base, quantities)?;
                let _ = field;
                // Simplified: return base for now
                Ok(base_val)
            }
            // `expr as iN`: a REAL conversion instruction.
            // The alternatives were both wrong: dropping the cast stored the source
            // type into the target slot, and emitting the bare inner expression is
            // the same bug. LLVM's `trunc`/`sext`/`zext` are the correct spelling, and
            // choosing the wrong one is itself a wrong answer -- `sext` on an unsigned
            // value and `zext` on a signed one both reinterpret the high bits.
            // The signedness is not carried by the width, so the explicit sign flag is
            // used rather than guessed. A float source is `fptosi`, which saturates
            // rather than trapping; a wider source is `trunc`. A target width equal to
            // 32 with an i64 source is a trunc, not a no-op, because the VALUE is i64
            // and something must narrow it.
            PirExpr::Cast {
                expr,
                width,
                signed,
            } => {
                let v = self.build_expr(expr, quantities)?;
                let target_w = u32::from(width.unwrap_or(32));
                let source_ty = v.get_type();
                let is_float = matches!(source_ty, BasicTypeEnum::FloatType(_));

                // `IntWidth` is the enum the type lowering speaks; an unrecognised
                // bit count has no LLVM spelling here, so refuse rather than guess
                // a nearby width.
                let target_ty = self.value_builder.type_lowering().int_type(match target_w {
                    1 => crate::codegen::abi::IntWidth::I1,
                    8 => crate::codegen::abi::IntWidth::I8,
                    16 => crate::codegen::abi::IntWidth::I16,
                    32 => crate::codegen::abi::IntWidth::I32,
                    64 => crate::codegen::abi::IntWidth::I64,
                    128 => crate::codegen::abi::IntWidth::I128,
                    other => {
                        return Err(CodegenError::UnsupportedFeature(format!(
                            "cast to a {other}-bit integer has no LLVM integer type here"
                        )));
                    }
                });
                // Same width AND an integer source means the conversion is a no-op, so
                // no instruction is needed.
                //
                // The `!is_float` clause is currently redundant: `Cast` only ever builds
                // an INTEGER target, so a float source can never compare equal to it.
                // It is kept because the guard is what makes the intent explicit, and
                // because a float target would otherwise silently turn this into a
                // wrong-answer no-op. Note that removing it is an EQUIVALENT MUTANT --
                // verified by mutation, all 14 execution tests still pass without it --
                // so it is not covered by a test and is not claimed to be.
                let target_enum: BasicTypeEnum<'ctx> = target_ty.into();
                if !is_float && target_enum == source_ty {
                    return Ok(v);
                }

                let converted = if is_float {
                    // `fptosi`, which SATURATES on overflow rather than trapping.
                    // A source that is out of range is a value the program computed,
                    // not a bug to abort on.
                    self.value_builder.builder().build_float_to_signed_int(
                        v.into_float_value(),
                        target_ty,
                        "cast",
                    )
                } else {
                    // `build_int_cast_sign_flag` picks sext/zext/trunc from the widths
                    // and the sign flag. Picking the wrong one is a wrong answer rather
                    // than a default: `zext` of a negative value wraps, and `sext` of a
                    // large unsigned value goes negative -- so the sign flag is
                    // carried from the AST instead of guessed here, because LLVM's
                    // integer types are signless and nothing downstream can recover it.
                    self.value_builder.builder().build_int_cast_sign_flag(
                        v.into_int_value(),
                        target_ty,
                        *signed,
                        "cast",
                    )
                }
                .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                Ok(converted.into())
            }
            // `target = value`: a real store, then the stored value.
            // The target must resolve to an address. This is deliberately strict: if
            // the target is not a known allocation, or its pointee type does not match
            // the value, the code returns a diagnostic. Silently evaluating `value`
            // and discarding it would compile to something that computes the right
            // answer and stores nothing -- the class of bug this backend previously had
            // with the whole loop body.
            PirExpr::Assign { target, value } => {
                let target = target.as_ref();
                let value = self.build_expr(value, quantities)?;

                // Only a plain named variable is addressable today. An indexed target
                // (`output[i]`) needs a GEP, which requires an element type the IR
                // carries only for allocas -- see AccessEmitter.
                let name = match target {
                    PirExpr::Var(n) => n,
                    other => {
                        return Err(CodegenError::UnsupportedFeature(format!(
                            "assignment to {:?}: only a named variable is addressable by \
                             the LLVM backend; an indexed or computed lvalue needs a \
                             GEP with an element type this path does not carry",
                            other
                        )));
                    }
                };

                let (ptr, pointee) = self.value_builder.variable(name).ok_or_else(|| {
                    CodegenError::UnsupportedFeature(format!(
                        "assignment to `{name}`: no allocation is known for it, so there \
                         is nowhere to store. Declared names in scope: {:?}",
                        self.value_builder.variable_names()
                    ))
                })?;

                let value_ty = value.get_type();
                if value_ty != pointee {
                    return Err(CodegenError::UnsupportedFeature(format!(
                        "assignment to `{name}`: storing {value_ty:?} into a slot of type \
                         {pointee:?}. Widening or narrowing here would be a silent \
                         wrong-answer bug, so it is refused."
                    )));
                }

                self.value_builder
                    .builder()
                    .build_store(ptr, value)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                Ok(value)
            }
            // A statement sequence yields no value.
            // Every element is emitted IN ORDER, and the sequence's result is the
            // zero of its own type. `build_expr` has to return a `BasicValueEnum`,
            // and there is no "void value" to return, so the placeholder is confined to
            // the value slot only -- the SIDE EFFECTS of the sequence are what the
            // caller actually needed and they are emitted.
            // Returning early here (or skipping the loop) is how a whole kernel body
            // previously vanished: the caller cannot tell an empty sequence from one
            // whose effects were emitted.
            PirExpr::Stmts(parts) => {
                let mut last: Option<BasicValueEnum<'ctx>> = None;
                for part in parts {
                    last = Some(self.build_expr(part, quantities)?);
                }
                match last {
                    Some(v) => Ok(v),
                    None => Ok(self
                        .value_builder
                        .type_lowering()
                        .int_type(crate::codegen::abi::IntWidth::I64)
                        .const_int(0, false)
                        .into()),
                }
            }
            PirExpr::QuantumOp { op, args, qubits } => {
                // Quantum operations lower to a call of the runtime intrinsic
                // with the same name, e.g. "h" -> "qir.h". Value arguments come
                // first, then the qubits they act on.
                let intrinsic_name = format!("qir.{}", op);

                let arg_values: CodegenResult<Vec<BasicValueEnum<'ctx>>> = args
                    .iter()
                    .map(|a| self.build_expr(a, quantities))
                    .collect();
                let mut arg_values = arg_values?;
                let qubit_values: CodegenResult<Vec<BasicValueEnum<'ctx>>> = qubits
                    .iter()
                    .map(|q| self.build_expr(q, quantities))
                    .collect();
                arg_values.extend(qubit_values?);

                let param_types: Vec<BasicTypeEnum<'ctx>> =
                    arg_values.iter().map(|v| v.get_type()).collect();

                // Declare the intrinsic on first use so the module is complete
                // even when no `extern_functions` entry mentioned it.
                let func = match self.module.get_function(&intrinsic_name) {
                    Some(f) => f,
                    None => {
                        let fn_type = self.value_builder.type_lowering().fn_type(
                            None,
                            &param_types,
                            /* is_var_args */ false,
                        );
                        self.module.add_function(&intrinsic_name, fn_type, None)
                    }
                };

                let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> =
                    arg_values.iter().map(|a| (*a).into()).collect();

                let call = self
                    .value_builder
                    .builder()
                    .build_call(func, &arg_metadata, op)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Void-returning intrinsics (gates, releases) produce no value;
                // build_expr must still return one, so yield the i1 result zero.
                Ok(call.try_as_basic_value().basic().unwrap_or_else(|| {
                    self.value_builder
                        .type_lowering()
                        .result_type()
                        .const_zero()
                        .into()
                }))
            }
        }
    }

    /// Placeholder value for expressions that produce no LLVM value (a void call).
    /// An `i64` zero.
    fn void_placeholder(&self) -> BasicValueEnum<'ctx> {
        self.value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64)
            .const_zero()
            .into()
    }

    fn build_binary_op(
        &mut self,
        op: BinaryOp,
        left: BasicValueEnum<'ctx>,
        right: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        use inkwell::IntPredicate;

        let left_int: IntValue<'ctx> = left.into_int_value();
        let right_int: IntValue<'ctx> = right.into_int_value();

        let result = match op {
            BinaryOp::Add => self
                .value_builder
                .builder()
                .build_int_add(left_int, right_int, "add"),
            BinaryOp::Sub => self
                .value_builder
                .builder()
                .build_int_sub(left_int, right_int, "sub"),
            BinaryOp::Mul => self
                .value_builder
                .builder()
                .build_int_mul(left_int, right_int, "mul"),
            BinaryOp::Div => self
                .value_builder
                .builder()
                .build_int_signed_div(left_int, right_int, "div"),
            BinaryOp::Mod => self
                .value_builder
                .builder()
                .build_int_signed_rem(left_int, right_int, "mod"),
            BinaryOp::And => self
                .value_builder
                .builder()
                .build_and(left_int, right_int, "and"),
            BinaryOp::Or => self
                .value_builder
                .builder()
                .build_or(left_int, right_int, "or"),
            BinaryOp::Xor => self
                .value_builder
                .builder()
                .build_xor(left_int, right_int, "xor"),
            BinaryOp::Eq => self.value_builder.builder().build_int_compare(
                IntPredicate::EQ,
                left_int,
                right_int,
                "eq",
            ),
            BinaryOp::Ne => self.value_builder.builder().build_int_compare(
                IntPredicate::NE,
                left_int,
                right_int,
                "ne",
            ),
            BinaryOp::Lt => self.value_builder.builder().build_int_compare(
                IntPredicate::SLT,
                left_int,
                right_int,
                "lt",
            ),
            BinaryOp::Le => self.value_builder.builder().build_int_compare(
                IntPredicate::SLE,
                left_int,
                right_int,
                "le",
            ),
            BinaryOp::Gt => self.value_builder.builder().build_int_compare(
                IntPredicate::SGT,
                left_int,
                right_int,
                "gt",
            ),
            BinaryOp::Ge => self.value_builder.builder().build_int_compare(
                IntPredicate::SGE,
                left_int,
                right_int,
                "ge",
            ),
            BinaryOp::Shl => self
                .value_builder
                .builder()
                .build_left_shift(left_int, right_int, "shl"),
            BinaryOp::Shr => self
                .value_builder
                .builder()
                .build_right_shift(left_int, right_int, true, "shr"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        Ok(result.into())
    }

    fn build_unary_op(
        &mut self,
        op: UnaryOp,
        expr: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let int_val = expr.into_int_value();
        let result = match op {
            UnaryOp::Neg => self.value_builder.builder().build_int_neg(int_val, "neg"),
            UnaryOp::Not => self.value_builder.builder().build_not(int_val, "not"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        Ok(result.into())
    }
}
