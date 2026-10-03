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
use crate::ir::pir_types::{BinaryOp, ParamKind, PirExpr, UnaryOp};
use inkwell::module::Module;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue};
use std::collections::HashMap;

/// How a subscript or assignment target reads as SOURCE, for a diagnostic.
///
/// `PirExpr`'s `Debug` is the AST's debug output, so an error message built from it
/// shows Rust enum syntax (`Var("input")`, `Index { .. }`) rather than the `input` and
/// `output[i]` the reader wrote. This is what names the construct instead.
/// The LLVM type a `ParamKind::Scalar` occupies in a call's argument list.
///
/// Matched against what `build_expr` produced, so a call refuses a wrong-width
/// argument by NAME rather than letting LLVM reject the operand by index. One
/// function because the declaration-side spelling lives in `module_builder::param_slot`
/// and the call side has to agree with it: a `f64` slot read as an `f32` argument is a
/// wrong answer, not a diagnostic.
fn scalar_arg_type<'ctx>(
    value_builder: &LlvmValueBuilder<'ctx>,
    param: &ParamKind,
) -> CodegenResult<BasicTypeEnum<'ctx>> {
    use crate::codegen::abi::{FloatWidth, IntWidth};
    use crate::ir::pir_types::ElemType;
    let tl = value_builder.type_lowering();
    Ok(match param {
        ParamKind::Scalar(ElemType::F64) => tl.float_type(FloatWidth::F64).into(),
        ParamKind::Scalar(ElemType::I8) => tl.int_type(IntWidth::I8).into(),
        ParamKind::Scalar(ElemType::I16) => tl.int_type(IntWidth::I16).into(),
        ParamKind::Scalar(ElemType::I32) => tl.int_type(IntWidth::I32).into(),
        ParamKind::Scalar(ElemType::I64) => tl.int_type(IntWidth::I64).into(),
        ParamKind::Scalar(ElemType::Bool) => tl.int_type(IntWidth::I1).into(),
        other => {
            return Err(CodegenError::UnsupportedFeature(format!(
                "`{other:?}` is not a scalar, so it has no single-argument call slot."
            )));
        }
    })
}

/// Split a subscript chain into its root tensor and ALL its subscripts.
///
/// `C[i][j]` is parsed as `Index { base: Index { base: Var(C), indices: [i] },
/// indices: [j] }`. Both halves need the whole list, so the inner indices are
/// prepended until the base is a bare name.
///
/// Collapsing only the outer level would be the plausible wrong answer: `t[0]` for
/// every row, which reads like a matrix and is not one.
fn flatten_subscript<'e>(
    mut base: &'e PirExpr,
    indices: &[PirExpr],
) -> (&'e PirExpr, Vec<PirExpr>) {
    let mut all = indices.to_vec();
    while let PirExpr::Index {
        base: inner,
        indices: inner_indices,
    } = base
    {
        let mut merged = inner_indices.clone();
        merged.append(&mut all);
        all = merged;
        base = inner;
    }
    (base, all)
}

fn describe_index_base(expr: &PirExpr) -> String {
    match expr {
        PirExpr::Var(name) => format!("`{name}`"),
        PirExpr::Index { base, .. } => format!("`{}[..]`", describe_index_base(base)),
        PirExpr::Field { base, field } => format!("`{}.{field}`", describe_index_base(base)),
        other => format!("{other:?}"),
    }
}

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
    /// Each Naso function's DECLARED parameter kinds, by function name.
    ///
    /// A call site needs this to marshal arguments, and it is not derivable from the
    /// expression: `PirExpr::Call { name, args }` carries one `PirExpr` per SOURCE
    /// argument, but a tensor argument occupies TWO LLVM arguments (`ptr`, then the
    /// caller's `i64` element count). Without the callee's declaration the call site
    /// would have to guess how many LLVM arguments each source argument expands to, and
    /// a guess that is off by one produces a `call` that verifies and computes the wrong
    /// thing: the argument after a tensor's pointer would be read as a buffer.
    ///
    /// `None` -- or a name absent from the map -- means the callee is not one of this
    /// module's Naso functions: a prelude intrinsic or a declared `extern`. Those are
    /// called with exactly the arguments the source wrote, one LLVM argument each,
    /// because nothing in this backend declares an `extern` with a length parameter.
    pub callee_params: Option<&'a HashMap<String, Vec<ParamKind>>>,
    /// True while lowering a PIR STATEMENT rather than an expression.
    ///
    /// A statement-position `let` is a binding that outlives its own expression, so it
    /// must NOT be removed from scope when the body finishes. A statement body is
    /// statement position by construction, so `ScheduleLowering` always sets this true
    /// and `LLVMModuleBuilder` sets it per statement.
    pub in_statement_position: bool,
    /// The enclosing loops, outermost first, so the INNERMOST is last.
    ///
    /// `break` and `continue` carry no target in the PIR, deliberately: which loop
    /// they mean is decided by the pass that owns the control-flow graph, and a
    /// lowering pass tracking a "current loop" would bind a `break` inside an `if`
    /// to the wrong one -- silently, because the resulting program is still well
    /// formed.
    ///
    /// So the backend resolves it from the CFG it is building, and that resolution
    /// needs this stack. Each entry is the block a `continue` jumps to (the header,
    /// which re-tests the guard) and the block a `break` jumps to (the exit).
    ///
    /// The stack is pushed by the loop arm and popped on the way out, INCLUDING on
    /// the error path -- a `?` between push and pop would leave a stale loop on the
    /// stack and let a later `break` outside any loop jump to a dead block.
    pub loop_stack: Vec<LoopTargets<'ctx>>,
}

/// Where `break` and `continue` go for one enclosing loop.
#[derive(Clone, Copy)]
pub struct LoopTargets<'ctx> {
    /// Re-enter here: the block that finishes this iteration and then re-tests the
    /// guard.
    ///
    /// The STEP, not the header, and the distinction is load-bearing. A counted
    /// loop advances its counter in the step, so a `continue` aimed at the header
    /// would re-test the guard without ever moving the counter: the loop would not
    /// terminate. For a source `while` the step is empty and the two are equivalent,
    /// so one target serves both.
    pub continue_target: inkwell::basic_block::BasicBlock<'ctx>,
    /// Leave here: the block after the loop.
    pub break_target: inkwell::basic_block::BasicBlock<'ctx>,
    /// This frame is an AFFINE schedule band, not a runtime CFG loop.
    ///
    /// A band's trip count is statically known, so its body is emitted as
    /// straight-line code inside a known iteration space. There is no runtime exit to
    /// branch to, and creating one would defeat the polyhedral form the whole pass
    /// exists to produce.
    ///
    /// This flag exists so the REFUSAL can be accurate. A `break` inside a `forall`
    /// used to be reported as "`break` outside a loop: no enclosing loop is being
    /// built" -- which is false, the program has a loop, and it sends the reader
    /// looking in the wrong place entirely. The two targets on an affine frame are
    /// never branched to; the flag is checked first.
    pub affine_band: bool,
}

impl<'ctx, 'a> PirExprLowerer<'ctx, 'a> {
    /// An `i64` zero, for a construct that produces no value but sits where the PIR
    /// demands one.
    ///
    /// `break` and `continue` are control flow, not expressions with results. The PIR
    /// still types them, so a caller can demand a value; this is what they produce. It
    /// is a PLACEHOLDER: in both cases the block was terminated by the branch just
    /// emitted, so nothing downstream can read it.
    fn zero_placeholder(&mut self) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        let tl = self.value_builder.type_lowering();
        self.value_builder
            .builder()
            .build_int_cast(
                tl.int_type(crate::codegen::abi::IntWidth::I1).const_zero(),
                tl.int_type(crate::codegen::abi::IntWidth::I64),
                "control_flow_result",
            )
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Lower a PIR expression, returning the value it evaluates to.
    // `quantities` is threaded through recursively and read by no arm today: every
    // quantity-dependent decision (which a `[0]` binding erases, which alias scope a
    // memory access carries) is made by the passes that CALL this one, not here. The
    // parameter is kept because it is part of the signature both callers already have,
    // and dropping it would mean every call site changed for no behavioural gain.
    #[allow(clippy::only_used_in_recursion)]
    /// Convert `value` to `target`, or REFUSE naming both the value and the goal.
    ///
    /// Used where a value must line up with another branch's type at a join point. It
    /// handles only the conversions that are meaningful for such a value:
    ///
    /// - int -> float: `sitofp` (a signed placeholder is the honest reading)
    /// - float -> int: `fptosi`, which SATURATES rather than trapping, so a
    ///   placeholder can never become poison
    /// - int -> int: sext/zext/trunc chosen from the widths, sign-extending when the
    ///   source is narrower
    /// - float -> float: `fptrunc`/`fpext`
    ///
    /// Anything else -- pointers, aggregates, i1 -- is refused. A wrong-but-plausible
    /// value here would be worse than an error: it would flow into a phi node and
    /// become indistinguishable from a real computation.
    fn coerce_to(
        &mut self,
        value: BasicValueEnum<'ctx>,
        target: BasicTypeEnum<'ctx>,
        context_desc: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        if value.get_type() == target {
            return Ok(value);
        }
        let builder = self.value_builder.builder();
        match (value, target) {
            (BasicValueEnum::IntValue(i), BasicTypeEnum::FloatType(f)) => builder
                .build_signed_int_to_float(i, f, "coerce_to_float")
                .map(BasicValueEnum::FloatValue)
                .map_err(|e| CodegenError::InstructionError(e.to_string())),
            (BasicValueEnum::FloatValue(f), BasicTypeEnum::IntType(i)) => builder
                .build_float_to_signed_int(f, i, "coerce_to_int")
                .map(BasicValueEnum::IntValue)
                .map_err(|e| CodegenError::InstructionError(e.to_string())),
            (BasicValueEnum::IntValue(i), BasicTypeEnum::IntType(t)) => builder
                .build_int_cast_sign_flag(i, t, true, "coerce_to_int_width")
                .map(BasicValueEnum::IntValue)
                .map_err(|e| CodegenError::InstructionError(e.to_string())),
            (BasicValueEnum::FloatValue(f), BasicTypeEnum::FloatType(t)) => builder
                .build_float_cast(f, t, "coerce_float_width")
                .map(BasicValueEnum::FloatValue)
                .map_err(|e| CodegenError::InstructionError(e.to_string())),
            (other, want) => Err(CodegenError::UnsupportedFeature(format!(
                "cannot coerce {other} to {want} for {context_desc}. Only numeric \
                 conversions are supported at a join point: coercing a pointer or an \
                 aggregate would produce a value that looks computed but is not.",
            ))),
        }
    }

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
                    Err(CodegenError::UnsupportedFeature(format!(
                        "read of `{name}`: no allocation is known for it, so there is \
                         nothing to load. Returning zero would be a silent wrong answer, \
                         and an uninitialised LLVM value is also zero, so the result \
                         would be indistinguishable from a real computation. Declared \
                         names in scope: {:?}",
                        self.value_builder.variable_names()
                    )))
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
                // A call to a NASO FUNCTION marshals through the callee's own
                // declaration, because a tensor argument occupies TWO LLVM arguments
                // and only the callee knows that. Anything else -- a prelude intrinsic,
                // an `extern` -- takes exactly the arguments the source wrote.
                if self.callee_params.is_some_and(|p| p.contains_key(name)) {
                    return self.build_naso_call(name, args, quantities);
                }

                let arg_values: CodegenResult<Vec<_>> = args
                    .iter()
                    .map(|a| self.build_expr(a, quantities))
                    .collect();
                let arg_values = arg_values?;

                // The PRELUD math intrinsics are compiler intrinsics, declared in the
                // typechecker's environment with an EMPTY body because "codegen emits them
                // directly". Until now nothing did: `clamp` fell through to the module
                // lookup below and failed with "Function 'clamp' not found", so every
                // shipped kernel -- all of which clamp -- could not be built at all.
                if let Some(v) = self.build_prelude_math(name, &arg_values)? {
                    return Ok(v);
                }

                // Resolve the callee.
                //
                // A NASO FUNCTION is emitted under `naso_<name>`, so a Naso-to-Naso
                // call has to be rewritten to that symbol. Resolution goes through
                // `function_emission::symbol_for` -- the SAME function emission used to
                // name the definition -- so the two cannot disagree about the
                // convention. A second guess here, spelled `format!("naso_{name}")`,
                // would compile cleanly right up until a call landed on a symbol that
                // happened to exist and meant something else.
                //
                // By the time control reaches here the callee is NOT a Naso function in
                // this module -- those returned above through `build_naso_call`. What is
                // left is a prelude intrinsic or a declared `extern`, both of which are
                // emitted under their own name. The prefixed fallback is kept as a
                // safety net for a callee that IS a generated function but was not in
                // `callee_params` (a hand-built module with a `functions` list the call
                // site did not see), so the diagnostic below names both spellings
                // rather than only the one that was tried.
                let func = self
                    .module
                    .get_function(name)
                    .or_else(|| {
                        self.module.get_function(
                            &crate::codegen::llvm::function_emission::symbol_for(name),
                        )
                    })
                    .ok_or_else(|| {
                        CodegenError::FunctionBuildError(format!(
                            "call to `{name}` resolved to neither `{}` nor `{}`. \
                             A Naso function is emitted under `naso_<name>`, so a call \
                             to a function that is neither this module's nor a declared \
                             `extern` has no target. Nothing is substituted for it.",
                            name,
                            crate::codegen::llvm::function_emission::symbol_for(name),
                        ))
                    })?;

                // inkwell 0.10 takes call arguments as `BasicMetadataValueEnum`.
                //
                // The operand ORDER is arguments first, callee LAST: operands
                // `0 .. num_operands - 1` are the arguments and the last one is the
                // function pointer. `build_call` takes them as a slice in that order and
                // appends the callee itself, so passing them the other way round -- or
                // passing the callee in the slice -- produces IR that parses, verifies,
                // and computes the wrong thing. There is no way to catch that by reading
                // the IR; it is caught by executing the callee and checking the value.
                let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> = arg_values
                    .into_iter()
                    .map(BasicMetadataValueEnum::from)
                    .collect();

                // inkwell 0.10 takes call arguments as `BasicMetadataValueEnum`.
                let call = self
                    .value_builder
                    .builder()
                    .build_call(func, &arg_metadata, &format!("naso.call.{name}"))
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
            //
            // A `while` is a real loop, NOT a band. A `forall` lowers to a schedule
            // band because its trip count is statically affine; a `while` has no such
            // domain, so it lowers to a header/body/exit CFG cycle with the condition
            // re-evaluated in the header before EVERY iteration, including the first.
            //
            // The condition must be a re-loaded value, not a cached SSA one: it reads
            // variables the body mutates, and hoisting the compare out of the header
            // would make the loop run forever or not at all.
            //
            // The loop yields no value. Returning the body's last value would make the
            // result depend on whether the loop ran at all -- `while false {}` would
            // then differ from the same program without the loop -- so a fresh zero of
            // the RIGHT TYPE is synthesised. Taking the type from the body's value
            // would be wrong for the same reason, plus it is unavailable for an empty
            // body.
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
            //
            // `break` leaves the INNERMOST loop on the stack, which is the last entry
            // because the loop arm pushes outer-first.
            //
            // `value` is REFUSED rather than ignored. `break v;` makes the loop's result
            // be `v`, so emitting it needs the exit block to carry a phi merged from
            // every `break` path and every fall-through path -- and the phi's TYPE, which
            // is not known until one of those is built. Guessing it would silently coerce
            // every `break` value. So `break v` says so, and bare `break` works.
            //
            // The branch TERMINATES the block the `break` was in, so nothing may be
            // appended after it. `Stmts` therefore checks the block's terminator before
            // building each further part, rather than guessing which construct may come
            // last in a block.
            PirExpr::Break { value } => {
                if value.is_some() {
                    return Err(CodegenError::UnsupportedFeature(
                        "`break` WITH A VALUE is not emitted, and does not need to be: \
                         it carries no information this language cannot already express. \
                         Bind the result to a mutable variable BEFORE the loop, assign \
                         it and then use a bare `break` --

                             let mut r = 0;
                             for i in 10 { if i == 3 { r = i * 10; break; } }
                             r

                         That form has a definite value on BOTH exits, because the \
                         initialiser covers the fall-through. `break v` would need the \
                         loop result merged from every `break` path AND from the \
                         fall-through, and the fall-through computes nothing -- filling \
                         it in would mean inventing a value, which is exactly the \
                         uninitialised-result-under-a-real-name failure worth refusing. \
                         The substitution above is verified by execution in \
                         `llvm_break_continue_execution_test.rs`."
                            .to_string(),
                    ));
                }
                let targets = self.loop_stack.last().copied().ok_or_else(|| {
                    CodegenError::UnsupportedFeature(
                        "`break` outside a loop: no enclosing loop is being built, so \
                         there is nothing to leave."
                            .to_string(),
                    )
                })?;
                if targets.affine_band {
                    return Err(CodegenError::UnsupportedFeature(
                        "`break` inside an affine `forall` loop. This is NOT the same as \
                         being outside a loop -- there IS a loop here, but its trip count \
                         is statically known and its body is emitted as straight-line \
                         code inside that iteration space, so there is no runtime exit to \
                         branch to. Leaving early would also break the polyhedral form \
                         the schedule pass exists to produce: the dependence analysis \
                         that chose this band assumes every iteration runs. Use a \
                         `while` or a counted `for` when the number of iterations is \
                         not known in advance."
                            .to_string(),
                    ));
                }
                self.value_builder
                    .builder()
                    .build_unconditional_branch(targets.break_target)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                // A `break` produces no value of its own. The zero is a PLACEHOLDER for
                // a caller that demands one; the block is already terminated, so nothing
                // downstream can read it.
                Ok(self.zero_placeholder()?.into())
            }
            //
            // `continue` re-enters the loop at its HEADER, so the guard is
            // re-evaluated. It must NOT jump to the top of the body: that would skip the
            // body entirely, and for a counted loop it would skip the counter update and
            // never terminate.
            PirExpr::Continue => {
                let targets = self.loop_stack.last().copied().ok_or_else(|| {
                    CodegenError::UnsupportedFeature(
                        "`continue` outside a loop: no enclosing loop is being built, so \
                         there is no next iteration to restart."
                            .to_string(),
                    )
                })?;
                if targets.affine_band {
                    return Err(CodegenError::UnsupportedFeature(
                        "`continue` inside an affine `forall` loop. This is NOT the same \
                         as being outside a loop -- there IS a loop here, but every \
                         iteration belongs to its statically known iteration space, and \
                         the dependence analysis that scheduled the band assumes the \
                         body runs on all of them. Skipping iterations would invalidate \
                         that schedule. Use a `while` or a counted `for` when some \
                         iterations may be skipped."
                            .to_string(),
                    ));
                }
                self.value_builder
                    .builder()
                    .build_unconditional_branch(targets.continue_target)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                Ok(self.zero_placeholder()?.into())
            }
            PirExpr::While { cond, body, step } => {
                let func = self.current_function;
                let context = self.value_builder.type_lowering().context();
                let header = context.append_basic_block(func, "while_cond");
                let body_block = context.append_basic_block(func, "while_body");
                let exit_block = context.append_basic_block(func, "while_exit");

                let bool_type = self
                    .value_builder
                    .type_lowering()
                    .int_type(crate::codegen::abi::IntWidth::I1);

                // Emit the condition test in the HEADER, and end the incoming block by
                // branching on it. The block the caller was building is the block that
                // jumps INTO the header, so its terminator is written before position
                // moves.
                let entry_cond_val = self.build_expr(cond, quantities)?;
                let entry_bool = self
                    .value_builder
                    .builder()
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        entry_cond_val.into_int_value(),
                        bool_type.const_zero(),
                        "while_entry_test",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.value_builder
                    .builder()
                    .build_conditional_branch(entry_bool, header, exit_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Header: re-test each iteration.
                self.value_builder.builder().position_at_end(header);
                let header_cond_val = self.build_expr(cond, quantities)?;
                let header_bool = self
                    .value_builder
                    .builder()
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        header_cond_val.into_int_value(),
                        bool_type.const_zero(),
                        "while_cond_val",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                self.value_builder
                    .builder()
                    .build_conditional_branch(header_bool, body_block, exit_block)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                // Body, then the step, then jump back to the header.
                //
                // The loop context is pushed BEFORE the body is built and popped after,
                // including on the error path. Without the pop on the error path, a
                // failed build would leave a stale entry and a later `break` outside any
                // loop would jump to a dead block.
                //
                // `continue` targets the STEP, not the header. A counted loop advances
                // its counter there, so jumping straight to the header would skip the
                // increment: the counter would never move and the loop would never
                // terminate. That is the same infinite loop a no-op `continue` produces,
                // arrived at correctly this time.
                let step_block = context.append_basic_block(func, "while_step");
                // Position at the BODY before building it. This line was missing, so the
                // body was emitted into whichever block the builder was last positioned
                // at -- the header, which already had its conditional branch. The
                // symptom was an empty `while_body` and a `while_step` with no
                // predecessors, which LLVM rejected as malformed.
                self.value_builder.builder().position_at_end(body_block);
                self.loop_stack.push(LoopTargets {
                    continue_target: step_block,
                    break_target: exit_block,
                    // A runtime CFG loop has real exit edges, so `break` and `continue`
                    // have somewhere real to go.
                    affine_band: false,
                });
                let body_result = self.build_expr(body, quantities);
                self.loop_stack.pop();
                body_result?;

                // Fall-through from the body into the step, but ONLY if the body did not
                // already terminate its block with a `break` or `continue`. Branching an
                // already-terminated block would give it two terminators.
                if self
                    .value_builder
                    .builder()
                    .get_insert_block()
                    .and_then(|b| b.get_terminator())
                    .is_none()
                {
                    self.value_builder
                        .builder()
                        .build_unconditional_branch(step_block)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                }

                self.value_builder.builder().position_at_end(step_block);
                if let Some(st) = step {
                    // The step is built with NO loop on the stack: a `break` inside it
                    // has no loop to leave, because the step runs after the body and
                    // before the guard, and leaving the loop from there would skip the
                    // remaining step statements.
                    self.build_expr(st, quantities)?;
                }
                self.value_builder
                    .builder()
                    .build_unconditional_branch(header)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

                self.value_builder.builder().position_at_end(exit_block);
                // A `while` yields unit, so any value here is a placeholder for a
                // caller that needs one. `i1 false` zero-extended to `i64` is 0
                // without inventing a wider computation.
                Ok(self
                    .value_builder
                    .builder()
                    .build_int_cast(
                        bool_type.const_zero(),
                        self.value_builder
                            .type_lowering()
                            .int_type(crate::codegen::abi::IntWidth::I64),
                        "while_result",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?
                    .into())
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

                // Then branch.
                //
                // The branch to the merge must be emitted in the block the ARM ENDS in,
                // not in whatever block the builder happens to be pointing at.
                //
                // This is a real bug that only a nested loop exposes. An arm containing
                // a `while` ends in `while_exit`, not in `then`, because the loop moved
                // the insertion point. Branching from wherever the builder sat produced
                // `then` with no terminator, an `if_merge` with only a phi, and a
                // function LLVM rejected as malformed -- so
                // `if x > 0 { while ... }` failed to compile at all.
                //
                // So the arm's final block is read back from the builder after building
                // it. For an arm with no loop that is the arm's own block, which is the
                // previous behaviour.
                self.value_builder.builder().position_at_end(then_block);
                let then_val = self.build_expr(then_branch, quantities)?;
                //
                // An arm that ends in `break` or `continue` has ALREADY terminated its
                // block, and does not reach the merge at all. Branching it to the merge
                // anyway gives the block two terminators, which LLVM rejects as
                // malformed -- `then: br label %while_exit` followed immediately by
                // `br label %if_merge`.
                //
                // So the branch is emitted only when the arm is still open, and the
                // PHI takes an incoming entry only from an arm that reaches the merge.
                // A PHI listing a predecessor that does not jump to it is equally
                // invalid, so skipping one here and the other there is not two
                // workarounds for one bug: they are the same fact, that this arm does
                // not flow into the merge.
                let then_end = self.value_builder.builder().get_insert_block();
                let then_open = then_end.and_then(|b| b.get_terminator()).is_none();
                let then_incoming = then_end.unwrap_or(then_block);
                if then_open {
                    self.value_builder
                        .builder()
                        .build_unconditional_branch(merge_block)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                }

                // Else branch.
                //
                // When the source has NO `else`, lowering substitutes `IntLit(0)`. That
                // used to be phi'd straight against the then-value, and for a `double`
                // function LLVM rejected the module:
                //
                //   PHI node operands are not the same type as the result!
                //     %if_phi = phi double [ 1.000000e+00, %then ], [ 0, %else ]
                //
                // so `if x > 0.0 { return 1.0; }` did not compile at all. The
                // substituted literal is a standing-in for "no value", and forcing it to
                // match the then-type by reinterpreting the bits is the kind of fix that
                // produces a plausible wrong answer, so the zero is CONVERTED to the
                // then-branch's type instead. `sitofp` on zero is exactly zero in
                // double, and `fptrunc`/`zext` on zero is zero of the narrower width --
                // so the value is right for every type, not just the ones that happen
                // to work.
                let then_type = then_val.get_type();
                self.value_builder.builder().position_at_end(else_block);
                let mut else_val = self.build_expr(else_branch, quantities)?;
                let else_end = self.value_builder.builder().get_insert_block();
                if else_val.get_type() != then_type {
                    else_val = self.coerce_to(else_val, then_type, "the missing `else` branch")?;
                }
                // Same rule as the then-arm: a `break` in the else-arm closes it.
                let else_open = else_end.and_then(|b| b.get_terminator()).is_none();
                let else_incoming = else_end.unwrap_or(else_block);
                if else_open {
                    self.value_builder
                        .builder()
                        .build_unconditional_branch(merge_block)
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                }

                // Merge block
                self.value_builder.builder().position_at_end(merge_block);
                let phi = self
                    .value_builder
                    .builder()
                    .build_phi(then_type, "if_phi")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                //
                // Only an arm that actually branches to the merge is listed as a
                // predecessor. `add_incoming` takes `&dyn BasicValue` rather than the
                // enum, so the values are referenced, not moved -- `then_val` is
                // returned below.
                //
                // If NEITHER arm reaches the merge, the merge is unreachable and a phi
                // there would have no predecessors at all, which is itself invalid. The
                // then-value is returned instead: it is the one the source computed, and
                // no path from this branch reaches the merge anyway.
                let mut incoming: Vec<(
                    &dyn inkwell::values::BasicValue<'ctx>,
                    inkwell::basic_block::BasicBlock<'ctx>,
                )> = Vec::new();
                if then_open {
                    incoming.push((&then_val, then_incoming));
                }
                if else_open {
                    incoming.push((&else_val, else_incoming));
                }
                if incoming.is_empty() {
                    return Ok(then_val);
                }
                phi.add_incoming(&incoming);
                Ok(phi.as_basic_value())
            }
            //
            // REFUSED rather than emitted.
            //
            // This arm ran `body` and dropped `inverse`, on the reasoning that the
            // inverse "is an undo, not part of the value". That is true of the RESULT
            // and false of the SEMANTICS: for a reversible block the inverse is the
            // part that uncomputes. Running the body without it is not a partial
            // implementation of uncomputation, it is the opposite of one -- it leaves
            // behind precisely the garbage the block existed to erase, which for a
            // linear resource is a soundness hole rather than a missing feature.
            //
            // A comment said the omission was "deliberately bound rather than
            // discarded, so the omission is visible in this file". A comment is not a
            // guarantee. Nothing checked it, and a `Reversible` reaching here from a
            // PIR fixture compiled to a program that quietly did half of what it
            // said.
            //
            // Lowering refuses expression-position `reversible` outright, so this arm
            // normally sees nothing. It refuses anyway rather than trusting that: a
            // backend must not run a construct whose defining operation it cannot
            // perform, no matter who produced it. The suite pins this by building the
            // node directly -- with lowering refusing first, that path is otherwise
            // unreachable, and a mutation restoring the old behaviour survived every
            // other test in the file.
            PirExpr::Reversible { .. } => Err(CodegenError::UnsupportedFeature(
                "a `Reversible` block reached codegen, and this backend does not \
                 perform uncomputation. The INVERSE is not optional -- it is the part \
                 that undoes the computation -- so emitting the body alone would leave \
                 the state the block existed to erase. Refused rather than half-run."
                    .to_string(),
            )),
            // `base[indices...]`: a real load out of a caller-owned tensor buffer.
            //
            // This used to evaluate the base, throw the indices away, and return the
            // base itself -- so `input[i]` handed the body a POINTER where an element
            // belonged, and the multiply that followed was `mul double, ptr %input,
            // double %scale`, which LLVM rejects. It is now a GEP against the buffer's
            // element type followed by a load, which is what the source asks for.
            //
            // Two subscripts (`u[i][j]`) would need a row type and a row stride the IR
            // does not carry, so it is refused naming the construct rather than silently
            // indexing the flat base as if it were one dimension.
            PirExpr::Index { base, indices } => {
                let (root, all) = flatten_subscript(base, indices);
                let tensor = self.tensor_base(root)?;
                let idx: CodegenResult<Vec<BasicValueEnum<'ctx>>> =
                    all.iter().map(|i| self.build_expr(i, quantities)).collect();
                self.load_tensor_element(&tensor, &idx?)
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
                float_target,
            } => {
                let v = self.build_expr(expr, quantities)?;

                // A FLOAT TARGET: `sitofp`/`uitofp`.
                //
                // Checked BEFORE the integer path, because the integer path reads a
                // `None` width as "i32" -- so an integer conversion would be emitted for
                // a float cast, sign-extending an `i8` buffer to `i32` and leaving a
                // `double` multiply with an integer operand. That compiles, verifies,
                // and computes nonsense.
                //
                // The SOURCE must be an integer for this to mean anything. A float source
                // is REFUSED rather than converted: `as f32` on a `double` is either a
                // no-op the typechecker should have removed, or a width change this IR
                // does not represent, and neither is a value to invent.
                if let Some(target) = float_target {
                    let float_ty = self
                        .value_builder
                        .type_lowering()
                        .float_type(crate::codegen::abi::FloatWidth::F64);
                    let int_value = v.into_int_value();
                    let converted = if *signed {
                        self.value_builder
                            .builder()
                            .build_signed_int_to_float(int_value, float_ty, "cast")
                    } else {
                        self.value_builder
                            .builder()
                            .build_unsigned_int_to_float(int_value, float_ty, "cast")
                    }
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                    let _ = target;
                    return Ok(converted.into());
                }

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
                // An INDEXED target (`output[i]`) is a GEP into a caller-owned buffer, which is
                // how a kernel body writes its result. This used to refuse every indexed
                // target -- "only a named variable is addressable" -- with the reason
                // that an indexed lvalue "needs a GEP with an element type this path does
                // not carry". That element type is exactly what the tensor binding records,
                // so the refusal described a gap that had since been filled and blocked
                // every kernel body in the repository.
                let value = self.build_expr(value, quantities)?;

                // Two addressable shapes: a named variable's slot, and one subscript of
                // a bound tensor. Anything else is refused by name. An indexed target
                // resolves to an element ADDRESS rather than the value currently stored
                // there, which is what makes `output[i] = ...` a real store.
                let store_target: (inkwell::values::PointerValue<'ctx>, BasicTypeEnum<'ctx>) =
                    match target.as_ref() {
                        PirExpr::Var(name) => self.value_builder.variable(name).ok_or_else(|| {
                            CodegenError::UnsupportedFeature(format!(
                                "assignment to `{name}`: no allocation is known for it, so there \
                                 is nowhere to store. Declared names in scope: {:?}",
                                self.value_builder.variable_names()
                            ))
                        })?,
                        PirExpr::Index { base, indices } => {
                            let (root, all) = flatten_subscript(base, indices);
                            let tensor = self.tensor_base(root)?;
                            let idx: CodegenResult<Vec<BasicValueEnum<'ctx>>> = all
                                .iter()
                                .map(|i| self.build_expr(i, quantities))
                                .collect();
                            let gep = self.tensor_gep(&tensor, &idx?, "elem")?;
                            (gep, tensor.elem)
                        }
                        other => {
                            return Err(CodegenError::UnsupportedFeature(format!(
                                "assignment to {}: only a named variable or a single \
                                 subscript of a tensor parameter is addressable by the \
                                 LLVM backend",
                                describe_index_base(other)
                            )));
                        }
                    };

                let (ptr, pointee) = store_target;

                let value_ty = value.get_type();
                if value_ty != pointee {
                    return Err(CodegenError::UnsupportedFeature(format!(
                        "assignment through `{}`: storing {value_ty:?} into a slot of type \
                         {pointee:?}. Widening or narrowing here would be a silent \
                         wrong-answer bug, so it is refused.",
                        describe_index_base(target)
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
                //
                // Each part of a `Stmts` list IS a statement, so set the flag while
                // building them. Without this a `let` in an `if` arm was scoped to its
                // own expression: `if x > 0.0 { let a = 10.0; a + 1.0 }` freed `a` as
                // soon as its placeholder body finished, so reading it next was
                // refused with "no allocation is known for it". The function's own
                // statements already got this via the caller; a block's statements did
                // not, because this arm is reached from `build_expr`.
                let outer_statement_position = self.in_statement_position;
                self.in_statement_position = true;
                // A BLOCK introduces a scope, so anything it binds leaves with it.
                //
                // Without this, a binding made in an `if` arm stayed in scope after the
                // join and produced INVALID LLVM: `if x > 0.0 { let a = 10.0; a } else
                // { -1.0 } a` emitted `load double, ptr %a` in `if_merge`, where `%a`
                // had only ever been allocated inside `%then`. LLVM correctly rejected
                // it -- the alloca is not in that block's scope -- but the diagnostic
                // arrives as a verifier failure quoting the whole function rather than
                // as "that name is out of scope here". Tracking the names this block
                // added and removing them on the way out turns that into the strict
                // unbound-read refusal that already names the variable.
                let names_before: Vec<String> = self.value_builder.variable_names();
                let mut build_result = Ok(());
                for part in parts {
                    // Stop if the previous part TERMINATED this block. `break` and
                    // `continue` both end their block with an unconditional branch, so
                    // `if c { break; } s = s + i;` would otherwise try to append a store
                    // AFTER a terminator, which inkwell permits and which produces a
                    // block with two terminators.
                    //
                    // `get_insert_block` IS the current block -- it calls
                    // `LLVMGetInsertBlock`, so it tracks `position_at_end` correctly. An
                    // intermediate note here claimed it returned "the last block
                    // created", which is wrong; the empty `while_body` that note
                    // described had a different cause, found below.
                    if self
                        .value_builder
                        .builder()
                        .get_insert_block()
                        .and_then(|b| b.get_terminator())
                        .is_some()
                    {
                        break;
                    }
                    match self.build_expr(part, quantities) {
                        Ok(v) => last = Some(v),
                        Err(e) => {
                            build_result = Err(e);
                            break;
                        }
                    }
                }
                self.in_statement_position = outer_statement_position;
                // Remove only what THIS block introduced, and do it even on the error
                // path so a failed build does not leave names behind for the next arm.
                let names_after = self.value_builder.variable_names();
                let introduced: Vec<String> = names_after
                    .iter()
                    .filter(|n| !names_before.contains(n))
                    .cloned()
                    .collect();
                for name in introduced {
                    self.value_builder.remove_variable(&name);
                }
                build_result?;

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

    /// The tensor binding an index or assign target names.
    ///
    /// Only a bare name can be a base. Anything else -- an index of an index, a field
    /// access -- has no binding and is refused here rather than evaluated, because
    /// evaluating it and discarding the result is the failure mode this module exists
    /// to prevent.
    fn tensor_base(
        &self,
        base: &PirExpr,
    ) -> CodegenResult<crate::codegen::llvm::value_builder::TensorBinding<'ctx>> {
        let name = match base {
            PirExpr::Var(n) => n,
            other => {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "subscripting {}: a tensor subscript must name a bound tensor \
                     parameter, because the LLVM ABI looks the buffer up by name",
                    describe_index_base(other)
                )));
            }
        };
        self.value_builder.tensor(name).ok_or_else(|| {
            CodegenError::UnsupportedFeature(format!(
                "subscripting `{name}`, which is not a bound tensor: there is no \
                 caller-supplied buffer to index into. Tensor parameters are bound to \
                 the function's pointer arguments; declared names in scope: {:?}",
                self.value_builder.variable_names()
            ))
        })
    }

    /// GEP to one element of a tensor: `getelementptr <elem>, ptr %base, i64 %index`.
    ///
    /// The index is EXTENDED, never truncated or reinterpreted. The induction variable
    /// this backend emits for a loop bound is i64, but an index that is already an
    /// integer of another width is sign-extended to GEP's i64 index type -- widening a
    /// value for indexing is exact, whereas narrowing would silently address the wrong
    /// element, and reinterpreting (zext of a negative) would address far out of bounds.
    fn tensor_gep(
        &mut self,
        tensor: &crate::codegen::llvm::value_builder::TensorBinding<'ctx>,
        indices: &[BasicValueEnum<'ctx>],
        name: &str,
    ) -> CodegenResult<inkwell::values::PointerValue<'ctx>> {
        let flat = self.linearize_index(tensor, indices, name)?;
        self.value_builder
            .build_gep(tensor.elem, tensor.base, &[flat.into()], name)
    }

    /// Collapse a subscript list to ONE i64 element offset: `i * cols + j`.
    ///
    /// A `ptr` has a single stride, so `C[i][j]` has to be computed before the GEP.
    /// Doing this is not an optimisation: indexing only the first dimension would
    /// return column 0 for every row, which is a plausible-looking wrong answer.
    ///
    /// The row-major strides come from the DECLARED shape, not from the caller's
    /// buffer length. If the caller allocated a differently-shaped buffer the
    /// arithmetic is wrong, and that is exactly where QTT's guarantees end -- see the
    /// limitations note in `TensorBinding::shape`.
    fn linearize_index(
        &mut self,
        tensor: &crate::codegen::llvm::value_builder::TensorBinding<'ctx>,
        indices: &[BasicValueEnum<'ctx>],
        name: &str,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        let i64_ty = self
            .value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);

        let shape = tensor.shape.as_deref().unwrap_or(&[]);
        if indices.len() > shape.len() {
            return Err(CodegenError::UnsupportedFeature(format!(
                "subscripting with {} indices into a tensor of {} dimension(s): there is \
                 no stride for the extra index, and treating it as another element would \
                 address memory past what the caller supplied",
                indices.len(),
                shape.len()
            )));
        }
        // Leading dimensions the source did NOT subscript are multiplied out as zero,
        // which is the identity on the offset. That keeps a 1-D tensor's `t[i]` on the
        // fast path with no arithmetic at all.
        // `sum over d of index[d] * stride(d)`, skipping any LEADING dimension the
        // source did not subscript: its contribution is 0 * stride = 0, so the term
        // can be dropped entirely rather than emitted as a multiply by zero.
        //
        // A 1-D tensor with one subscript therefore does NO arithmetic at all, and a
        // `t[i]` still lowers to a single `getelementptr`.
        let mut offset: Option<inkwell::values::IntValue<'ctx>> = None;
        for (dim, idx_val) in indices.iter().enumerate() {
            // The row-major stride of dimension `dim` is the product of every extent
            // to its right. `dim < shape.len()` is guaranteed by the check above.
            let stride: u64 = shape[dim + 1..].iter().product();
            let raw = idx_val.into_int_value();
            let idx = if raw.get_type() == i64_ty {
                raw
            } else {
                self.value_builder
                    .builder()
                    .build_int_cast_sign_flag(raw, i64_ty, true, "index")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?
            };
            let scaled = if stride == 1 {
                idx
            } else {
                let stride_val = self
                    .value_builder
                    .build_int_constant(i64_ty, stride, "stride");
                self.value_builder
                    .builder()
                    .build_int_mul(idx, stride_val, name)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?
            };
            offset = Some(match offset {
                None => scaled,
                Some(prev) => self
                    .value_builder
                    .builder()
                    .build_int_add(prev, scaled, name)
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?,
            });
        }
        // No subscript at all is offset 0, the first element. That is reachable from a
        // fixture body that names a tensor without indexing it.
        Ok(offset.unwrap_or_else(|| self.value_builder.build_int_constant(i64_ty, 0, name)))
    }

    /// Load one element out of a tensor. The read half of `t[i]`.
    fn load_tensor_element(
        &mut self,
        tensor: &crate::codegen::llvm::value_builder::TensorBinding<'ctx>,
        indices: &[BasicValueEnum<'ctx>],
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let gep = self.tensor_gep(tensor, indices, "elem_addr")?;
        self.value_builder
            .builder()
            .build_load(tensor.elem, gep, "elem")
            .map_err(|e| CodegenError::InstructionError(e.to_string()))
    }

    /// Emit a prelude math intrinsic, or return `None` if `name` is not one.
    ///
    /// The typechecker declares these with an empty body on the understanding that
    /// codegen emits them directly, so they are compiler intrinsics rather than calls.
    /// `None` means "not an intrinsic, carry on to the module lookup" -- it is NOT an
    /// error, because a program may legitimately call a function it declares itself.
    ///
    /// # Why `round` is not `llvm.round`
    ///
    /// `llvm.round.f64(double, i32)` does not verify on LLVM 17 without an `immarg`
    /// operand, and the two alternatives are both WRONG for Naso's `round`:
    /// `llvm.rint` rounds half-to-EVEN, while Naso (and WGSL, whose backend this
    /// mirrors) round half-AWAY-FROM-ZERO. `round(0.5)` must be `1`, and `rint` gives
    /// `0`. So `round` is spelled out of `floor`/`ceil` on the sign of the operand,
    /// which is half-away-from-zero by construction and verifiable by running it.
    ///
    /// # NaN behaviour
    ///
    /// `clamp` is `select` on ORDERED comparisons, so a NaN input falls through both
    /// tests and is returned unchanged. That is deliberate: the WGSL backend's `clamp`
    /// is the language's semantics, and an unordered comparison would instead return a
    /// bound, inventing a value the program never computed.
    fn build_prelude_math(
        &mut self,
        name: &str,
        args: &[BasicValueEnum<'ctx>],
    ) -> CodegenResult<Option<BasicValueEnum<'ctx>>> {
        // Arity is checked against the intrinsic, not assumed: `clamp` takes three
        // arguments and the unary intrinsics take one, and calling one with the wrong
        // count must be a diagnostic rather than an index panic or a silently
        // ignored extra operand.
        let want_arity = match name {
            "clamp" => 3,
            "round" | "abs" | "floor" | "ceil" | "sqrt" | "exp" => 1,
            _ => return Ok(None),
        };
        if args.len() != want_arity {
            return Err(CodegenError::UnsupportedFeature(format!(
                "prelude intrinsic `{name}` takes {want_arity} argument(s) but was \
                 called with {}",
                args.len()
            )));
        }
        // Every one of these is a FLOAT intrinsic in the prelude's own signature
        // (`TypeKind::Float`), so an integer argument is a typechecker gap. It is
        // refused rather than converted.
        let float_arg =
            |v: &BasicValueEnum<'ctx>| -> CodegenResult<inkwell::values::FloatValue<'ctx>> {
                match v.get_type() {
                    BasicTypeEnum::FloatType(_) => Ok((*v).into_float_value()),
                    other => Err(CodegenError::UnsupportedFeature(format!(
                        "prelude intrinsic `{name}` is declared on floats but got {other:?}. \
                     Converting it here would be an implicit conversion."
                    ))),
                }
            };

        match name {
            "clamp" => {
                let x = float_arg(&args[0])?;
                let lo = float_arg(&args[1])?;
                let hi = float_arg(&args[2])?;
                // min then max, as two ordered selects. Doing it in this order rather
                // than as a single select is what makes a reversed range (`lo > hi`)
                // return `hi`, matching WGSL's `clamp(x, lo, hi) = min(max(x, lo), hi)`.
                let builder = self.value_builder.builder();
                let above_lo = builder
                    .build_float_compare(inkwell::FloatPredicate::OLT, x, lo, "clamp_below_lo")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                // `build_select` needs a concrete `BasicValue`, and both arms must be the
                // same one: an `x` of one type against a bound of another would be a
                // select LLVM rejects, and one that "worked" would be storing a value the
                // program never computed.
                let raised: inkwell::values::FloatValue<'ctx> = builder
                    .build_select(
                        above_lo,
                        BasicValueEnum::FloatValue(lo),
                        BasicValueEnum::FloatValue(x),
                        "clamp_raised",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?
                    .into_float_value();
                let above_hi = builder
                    .build_float_compare(inkwell::FloatPredicate::OGT, raised, hi, "clamp_above_hi")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                let clamped = builder
                    .build_select(
                        above_hi,
                        BasicValueEnum::FloatValue(hi),
                        BasicValueEnum::FloatValue(raised),
                        "clamp",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                Ok(Some(clamped))
            }
            "round" => {
                let x = float_arg(&args[0])?;
                let ty = self
                    .value_builder
                    .type_lowering()
                    .float_type(crate::codegen::abi::FloatWidth::F64);
                let half = ty.const_float(0.5);
                // `x < 0 ? ceil(x - 0.5) : floor(x + 0.5)` is round-half-away-from-zero,
                // which is what Naso's `round` means and what `llvm.rint` does NOT mean.
                // The two branches are named for the direction they apply to: the
                // non-negative operand rounds UP by half and then takes the FLOOR, and
                // the negative operand rounds DOWN by half and then takes the CEILING.
                // Swapping floor and ceil between the two branches silently turns
                // round(-2.5) into -2, which no IR-text assertion would catch.
                //
                // Each step takes its own short borrow of the builder. `call_f64_intrinsic`
                // needs `&mut self`, so a single `builder` binding held across the whole
                // sequence would not borrow-check; the three scopes below keep every
                // borrow dead before the next one starts.
                let (is_neg, up, down) = {
                    let builder = self.value_builder.builder();
                    let is_neg = builder
                        .build_float_compare(
                            inkwell::FloatPredicate::OLT,
                            x,
                            ty.const_zero(),
                            "round_is_neg",
                        )
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                    let up = builder
                        .build_float_add(x, half, "round_up")
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                    let down = builder
                        .build_float_sub(x, half, "round_down")
                        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                    (is_neg, up, down)
                };
                // `floor`/`ceil` are the `llvm.floor.f64` / `llvm.ceil.f64` intrinsics:
                // inkwell 0.10 exposes no `build_float_to_int_floor`, and truncating
                // instead would make `round(2.7)` = 2 rather than 3.
                let (floor_up, ceil_down) = {
                    let floor_up = self
                        .call_f64_intrinsic("llvm.floor.f64", up, "floor")?
                        .into_float_value();
                    let ceil_down = self
                        .call_f64_intrinsic("llvm.ceil.f64", down, "ceil")?
                        .into_float_value();
                    (floor_up, ceil_down)
                };
                let rounded = self
                    .value_builder
                    .builder()
                    .build_select(
                        is_neg,
                        BasicValueEnum::FloatValue(ceil_down),
                        BasicValueEnum::FloatValue(floor_up),
                        "round",
                    )
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
                Ok(Some(rounded))
            }
            "floor" | "ceil" => {
                let x = float_arg(&args[0])?;
                let intrinsic = if name == "floor" {
                    "llvm.floor.f64"
                } else {
                    "llvm.ceil.f64"
                };
                Ok(Some(self.call_f64_intrinsic(intrinsic, x, name)?))
            }
            // `abs` on a float CLEARS THE SIGN BIT. It must not truncate: `abs(-0.5)`
            // is `0.5`, and an integer `abs` of the truncated value would be `0`. That
            // is a wrong answer rather than a rounding choice, so `build_float_abs` is
            // the only correct spelling and it is what is emitted.
            "abs" => {
                let x = float_arg(&args[0])?;
                Ok(Some(self.call_f64_intrinsic("llvm.fabs.f64", x, "abs")?))
            }
            // `sqrt` and `exp` are LLVM intrinsics and must be DECLARED with their exact
            // signature. The argument passed is `x` itself -- an earlier draft passed
            // `const_zero()`, which computed `sqrt(0.0)` and discarded `x` while
            // appearing to work.
            "sqrt" | "exp" => {
                let x = float_arg(&args[0])?;
                let ty = self
                    .value_builder
                    .type_lowering()
                    .float_type(crate::codegen::abi::FloatWidth::F64);
                let intrinsic = if name == "sqrt" {
                    "llvm.sqrt.f64"
                } else {
                    "llvm.exp.f64"
                };
                // The `double` type is what `call_f64_intrinsic` declares the intrinsic
                // for; `ty` is read here only to name the expectation.
                let _ = ty;
                Ok(Some(self.call_f64_intrinsic(intrinsic, x, name)?))
            }
            _ => Ok(None),
        }
    }

    /// Lower a call to a NASO FUNCTION in this module.
    ///
    /// # Argument marshalling, and why the callee's declaration is needed
    ///
    /// One source argument is not one LLVM argument. A tensor parameter occupies TWO:
    /// its buffer pointer, then the caller's `i64` element count. That count is not a
    /// detail -- it is what the callee's ABI guard compares against its own declared
    /// extent, and it is the only thing that makes the declared extent checkable at
    /// all. So a call forwards the length the CALLER was given, read off the caller's
    /// own `TensorBinding`, and never a number recomputed from a shape: recomputing it
    /// would make the callee's guard compare the caller's INVENTED length against the
    /// callee's declaration and pass, having checked nothing about the caller's
    /// actual allocation.
    ///
    /// ## Arity
    ///
    /// A mismatch between the number of source arguments and the number of declared
    /// parameters is REFUSED, naming both. Passing the wrong count to `build_call`
    /// produces a `call` whose operand list is the wrong length, which LLVM rejects
    /// with a verifier message that names neither the Naso function nor the parameter
    /// that is missing.
    ///
    /// ## Operand order
    ///
    /// inkwell 0.10 takes call arguments in a slice, ARGUMENTS FIRST, and appends the
    /// callee itself as the last operand. Passing them the other way round produces IR
    /// that parses, verifies, and computes the wrong thing; that failure is caught by
    /// executing the callee and checking the value, not by reading the IR.
    ///
    /// ## Void, value, and discarded results
    ///
    /// All three are handled by the same path. A void callee's `build_call` yields no
    /// basic value, and the `void_placeholder` stands in for it -- which is what a
    /// statement body wants, since a statement body's side effects are the point and
    /// its result is discarded. A value-returning callee's value flows out to
    /// whatever expression the call was embedded in. A call whose result is discarded
    /// reads as a statement body and takes the same path as a void one.
    fn build_naso_call(
        &mut self,
        name: &str,
        args: &[PirExpr],
        quantities: &HashMap<String, Quantity>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let params = self
            .callee_params
            .and_then(|p| p.get(name))
            .cloned()
            .unwrap_or_default();
        if params.len() != args.len() {
            return Err(CodegenError::FunctionBuildError(format!(
                "call to `{name}` passes {} argument(s) but the function declares {} \
                 parameter(s). Nothing is padded or dropped: an invented argument would \
                 be a value the caller never computed, and a dropped one would shift \
                 every later parameter.",
                args.len(),
                params.len()
            )));
        }

        let mut values: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(args.len());
        for (arg, param) in args.iter().zip(params.iter()) {
            match param {
                ParamKind::Tensor { .. } => {
                    // A tensor argument is a NAME, not a general expression. Accepting
                    // an arbitrary expression here would mean computing a fresh buffer
                    // and passing a pointer to it, which the callee would then fill --
                    // and whose length the caller could not state. Refusing keeps the
                    // one honest case (forwarding a buffer the caller was given) the
                    // only case.
                    let PirExpr::Var(tensor_name) = arg else {
                        return Err(CodegenError::UnsupportedFeature(format!(
                            "call to `{name}` passes a tensor parameter, so its argument \
                             must be a tensor NAME the caller holds. This one is not: \
                             `{arg:?}`. Passing a computed buffer would mean passing a \
                             length the caller cannot state, and the callee's guard would \
                             have nothing real to check."
                        )));
                    };
                    let Some(binding) = self.value_builder.tensor(tensor_name) else {
                        return Err(CodegenError::InstructionError(format!(
                            "call to `{name}` passes `{tensor_name}` as a tensor, but \
                             this function's scope has no tensor by that name. A name \
                             from another function's scope cannot be passed here: each \
                             function has its own, and none of them is visible."
                        )));
                    };
                    let Some(len) = binding.len else {
                        return Err(CodegenError::UnsupportedFeature(format!(
                            "call to `{name}` passes `{tensor_name}`, whose length the \
                             caller never received, so there is no count to forward. \
                             Passing a number computed from the declared shape instead \
                             would make the callee's guard check an invented length \
                             rather than the caller's buffer."
                        )));
                    };
                    // Pointer first, then the count: the ABI's own order, which is the
                    // order `build_entry_signature` recorded in `len_arg_index`.
                    values.push(binding.base.into());
                    values.push(len.into());
                }
                ParamKind::QRegister => {
                    // One argument, like the ABI slot: a quantum register is a
                    // caller-owned pointer with no element count. Nothing in this
                    // backend indexes one, so a read stays a diagnostic.
                    let PirExpr::Var(name) = arg else {
                        return Err(CodegenError::UnsupportedFeature(format!(
                            "call to `{name}` passes a quantum register, which is a \
                             caller-owned pointer, so its argument must be a NAME. \
                             `{arg:?}` is not."
                        )));
                    };
                    let Some(binding) = self.value_builder.tensor(name) else {
                        return Err(CodegenError::InstructionError(format!(
                            "call to `{name}` passes `{name}` as a quantum register, but \
                             this function's scope has none by that name."
                        )));
                    };
                    values.push(binding.base.into());
                }
                // A scalar is passed BY VALUE, evaluated as an ordinary expression.
                //
                // A WIDTH MISMATCH is refused rather than converted: the callee declared
                // this parameter's type, and converting to it would compute a value the
                // source never wrote. LLVM would also reject the call, but with a
                // message naming an operand index rather than the parameter.
                ParamKind::Scalar(_) => {
                    let v = self.build_expr(arg, quantities)?;
                    if v.get_type() != scalar_arg_type(self.value_builder, param)? {
                        return Err(CodegenError::UnsupportedFeature(format!(
                            "call to `{name}` passes an argument of type `{}` where the \
                             callee declares `{param:?}`. Nothing is converted: a cast \
                             here would be a value the source never wrote.",
                            v.get_type(),
                        )));
                    }
                    values.push(v);
                }
                ParamKind::Unsupported(ty) => {
                    return Err(CodegenError::UnsupportedFeature(format!(
                        "call to `{name}` passes a parameter of type `{ty}`, which has no \
                         slot in this ABI. Nothing is substituted for it."
                    )));
                }
            }
        }

        let symbol = crate::codegen::llvm::function_emission::symbol_for(name);
        let func = self.module.get_function(&symbol).ok_or_else(|| {
            CodegenError::FunctionBuildError(format!(
                "call to `{name}` looked for `{symbol}`, which is not in the module. \
                 Every function's signature is declared before any body is emitted, so \
                 this means the call names a function the lowering did not record."
            ))
        })?;

        let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> = values
            .into_iter()
            .map(BasicMetadataValueEnum::from)
            .collect();
        let call = self
            .value_builder
            .builder()
            .build_call(func, &arg_metadata, &format!("naso.call.{name}"))
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        // A void callee yields no basic value. `void_placeholder` is what a statement
        // body wants anyway: its side effects were emitted before this point, and its
        // result is discarded. A value-returning callee flows out to the enclosing
        // expression.
        Ok(call
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| self.void_placeholder()))
    }

    /// Call a `double -> double` LLVM intrinsic, declaring it on first use.
    ///
    /// A small helper because the declaration, the argument and the result all have to
    /// agree on the exact `double` signature; inlining that three times is how one of
    /// them ends up passing a `float` to a function declared for `double`, which LLVM
    /// rejects only at verification.
    fn call_f64_intrinsic(
        &mut self,
        intrinsic: &str,
        arg: inkwell::values::FloatValue<'ctx>,
        name: &str,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let ty = self
            .value_builder
            .type_lowering()
            .float_type(crate::codegen::abi::FloatWidth::F64);
        // The signature is taken from `arg` rather than assumed, so an intrinsic
        // declared for a different width is a type error here instead of invalid IR.
        debug_assert_eq!(
            arg.get_type(),
            ty,
            "f64 intrinsic `{intrinsic}` must take the type it is declared for"
        );
        let func = self.declare_or_get_intrinsic(intrinsic, &[ty.into()], ty.into())?;
        let call = self
            .value_builder
            .builder()
            .build_call(func, &[BasicMetadataValueEnum::from(arg)], name)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        call.try_as_basic_value().basic().ok_or_else(|| {
            CodegenError::UnsupportedFeature(format!("intrinsic `{intrinsic}` returned void"))
        })
    }

    /// Declare an LLVM intrinsic on first use, or return the existing declaration.
    ///
    /// LLVM requires `llvm.*` functions to be DECLARED, not defined, and requires the
    /// declaration to carry the exact signature -- so the types come from the call site
    /// that needs them rather than from a table that could drift out of step with it.
    fn declare_or_get_intrinsic(
        &mut self,
        intrinsic: &str,
        param_types: &[BasicTypeEnum<'ctx>],
        ret_type: BasicTypeEnum<'ctx>,
    ) -> CodegenResult<FunctionValue<'ctx>> {
        if let Some(f) = self.module.get_function(intrinsic) {
            return Ok(f);
        }
        let fn_type =
            self.value_builder
                .type_lowering()
                .fn_type(Some(ret_type), param_types, false);
        Ok(self.module.add_function(intrinsic, fn_type, None))
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

        // A FLOAT operand routes to the float arm. This dispatch is new and load-bearing:
        // `into_int_value` on a `double` PANICS, so `input[i] * scale` in a kernel body
        // used to abort the compiler rather than fail gracefully -- the int-only arm
        // below could not have produced the multiply even had it been reached.
        //
        // Mixing the two is refused rather than converted. `f64 * i64` has no single
        // correct answer: it differs on whether the integer is exactly representable, and
        // picking a conversion here is precisely the implicit widening this backend
        // must not insert.
        match (left.get_type(), right.get_type()) {
            (BasicTypeEnum::FloatType(_), BasicTypeEnum::FloatType(_)) => {
                return self.build_float_binary_op(
                    op,
                    left.into_float_value(),
                    right.into_float_value(),
                );
            }
            (BasicTypeEnum::IntType(_), BasicTypeEnum::IntType(_)) => {}
            (l, r) => {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "binary operation on operands of different types ({l:?} and {r:?}). \
                     Converting one to the other would be an implicit widening or \
                     narrowing, so it is refused."
                )));
            }
        }

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

    /// Float arithmetic and comparison.
    ///
    /// Comparisons use the FLOAT predicates, and this is not a detail: `slt` on an
    /// integer is a signed comparison, while a float has no sign bit, so `clamp(v, lo,
    /// hi)` implemented with the integer predicate would be meaningless. `Eq`/`Ne` use
    /// `oeq`/`one`, the ordered predicates, so a NaN compares unequal rather than
    /// unordered-equal.
    ///
    /// `Mod` has no float spelling in Naso and is refused: `frem` is defined but the
    /// language has no `%` on floats, so reaching here means the typechecker let
    /// something through and the honest answer is a diagnostic.
    fn build_float_binary_op(
        &mut self,
        op: BinaryOp,
        left: inkwell::values::FloatValue<'ctx>,
        right: inkwell::values::FloatValue<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        use inkwell::FloatPredicate;

        // Arithmetic yields a `double` and a comparison yields an `i1`, so the two groups
        // return separately rather than through one `?`-chained `Result`: a single arm
        // for both would have to convert the comparisons to a float, which is exactly
        // the implicit conversion this backend must not insert.
        let arithmetic: Option<inkwell::values::FloatValue<'ctx>> = match op {
            BinaryOp::Add => Some(
                self.value_builder
                    .builder()
                    .build_float_add(left, right, "fadd")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?,
            ),
            BinaryOp::Sub => Some(
                self.value_builder
                    .builder()
                    .build_float_sub(left, right, "fsub")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?,
            ),
            BinaryOp::Mul => Some(
                self.value_builder
                    .builder()
                    .build_float_mul(left, right, "fmul")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?,
            ),
            BinaryOp::Div => Some(
                self.value_builder
                    .builder()
                    .build_float_div(left, right, "fdiv")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?,
            ),
            _ => None,
        };
        if let Some(v) = arithmetic {
            return Ok(v.into());
        }

        // A float has no sign bit, so the ORDERED predicates are the only meaningful
        // ones: `olt` is "strictly less", not "signed less than", and `oeq`/`one` make
        // a NaN compare unequal rather than unordered-equal.
        let predicate = match op {
            BinaryOp::Eq => FloatPredicate::OEQ,
            BinaryOp::Ne => FloatPredicate::ONE,
            BinaryOp::Lt => FloatPredicate::OLT,
            BinaryOp::Le => FloatPredicate::OLE,
            BinaryOp::Gt => FloatPredicate::OGT,
            BinaryOp::Ge => FloatPredicate::OGE,
            other => {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "{other:?} is not defined on floating-point operands by the LLVM \
                     backend. A float has no bitwise or shift operators, and Naso has \
                     no `%` on floats."
                )));
            }
        };
        Ok(self
            .value_builder
            .builder()
            .build_float_compare(predicate, left, right, "fcmp")
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?
            .into())
    }

    /// Float negation, and the integer arm below.
    ///
    /// `into_int_value` panics on a `double`, so `-v` on a float had to route here
    /// first for the same reason binary multiplication does.
    fn build_unary_op(
        &mut self,
        op: UnaryOp,
        expr: BasicValueEnum<'ctx>,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        if let BasicTypeEnum::FloatType(_) = expr.get_type() {
            return match op {
                UnaryOp::Neg => Ok(self
                    .value_builder
                    .builder()
                    .build_float_neg(expr.into_float_value(), "fneg")
                    .map_err(|e| CodegenError::InstructionError(e.to_string()))?
                    .into()),
                UnaryOp::Not => Err(CodegenError::UnsupportedFeature(
                    "`!` (logical not) on a floating-point operand has no LLVM \
                     spelling; the typechecker should have rejected it."
                        .to_string(),
                )),
            };
        }
        let int_val = expr.into_int_value();
        let result = match op {
            UnaryOp::Neg => self.value_builder.builder().build_int_neg(int_val, "neg"),
            UnaryOp::Not => self.value_builder.builder().build_not(int_val, "not"),
        }
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        Ok(result.into())
    }
}
