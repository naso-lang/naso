//! The ABI boundary guard: what a tensor parameter's declared shape is FOR.
//!
//! # The gap this closes
//!
//! A `ParamKind::Tensor` reaches the entry function as a bare `ptr`. The declared
//! shape (`Tensor[f32, 4]` -> `[4]`) was carried through lowering, printed by the
//! backend's diagnostics, and then used for exactly one thing: rejecting a zero
//! extent. It was never compared against anything the caller supplied, because the
//! caller supplied nothing to compare. A C driver could hand a one-element buffer to
//! a kernel that reads four, get four elements back out of bounds, print nothing,
//! and exit 0.
//!
//! That is the whole QTT promise failing at the one place a caller can break it. The
//! typechecker genuinely enforces linearity inside a function body -- `naso check`
//! reports `linear variable 'a' used twice` -- but a body that is *internally*
//! correct still reads out of bounds if the buffer it was handed is the wrong size.
//!
//! # The ABI, and why it is this one
//!
//! Each `ParamKind::Tensor` with a known shape takes TWO arguments:
//!
//! ```text
//! ptr %input, i64 %input_len
//! ```
//!
//! Alternatives considered:
//!
//! - A pointer to a `{ptr, i64}` descriptor. Rejected: the caller must then build a
//!   struct and keep it alive, and under LLVM 17 opaque pointers the callee still
//!   loads both fields through a `getelementptr` it has to spell out. Two scalar
//!   arguments carry the same information, keep the data pointer a first-class named
//!   argument (so `ptr %input` remains a readable part of the signature), and make
//!   the length visible in the IR text where a test can assert it.
//! - Checking nothing and trusting the caller. That is the state this file exists
//!   to leave.
//!
//! The length is in ELEMENTS, not bytes: `Tensor[f32, 2, 3]` declares six elements
//! and a linearised subscript `t[i][j]` reads `i * 3 + j`, so six is the number the
//! check compares against. A byte count would be a second unit the caller could get
//! wrong in a different direction.
//!
//! # What the guard checks
//!
//! Two checks per tensor parameter, in this order:
//!
//! 1. `icmp eq ptr %p, null`. A null pointer is its own failure, not a length of
//!    zero, and it gets its own message and its own exit code so a caller can tell
//!    "you passed NULL" from "you passed too little".
//! 2. `icmp slt i64 %p_len, <declared element count>`. Short is a failure. The
//!    comparison is SIGNED so a negative length is rejected rather than read as
//!    `u64::MAX`.
//!
//! The comparison is `slt` against the declared count, NOT an equality test. A
//! caller whose buffer is LARGER than the declared extent is running a correct
//! program -- passing a 1024-element array as the `Tensor[f32, 4]` view of its first
//! four elements is memory-safe -- and rejecting it would be the guard inventing a
//! rule the type system never stated. The property being established is "every index
//! the body can form lies inside the buffer", which is exactly `len >= declared`.
//!
//! # Failure: two libc calls, never `unreachable` alone
//!
//! On failure the guard calls `fputs(msg, stderr)` and then `exit(code)`, with
//! [`NASO_ABI_EXIT_NULL`] for a null pointer and [`NASO_ABI_EXIT_SHORT`] for a short
//! buffer. The exit code is the observable signal a test asserts on; the message is
//! for a human, and is built at compile time because the parameter name and the
//! declared extent are both known then.
//!
//! The failure edge deliberately contains CALLS rather than a bare `unreachable`.
//! That is the elimination hazard this design exists to avoid: an optimiser may
//! delete a branch whose only consequence is `unreachable` once it can prove the
//! condition false, and in an inlined caller it proves exactly the wrong thing --
//! a caller that inlines the kernel with a constant short length would have the
//! check folded away and be left reading out of bounds. A call to an external
//! function has unknown side effects and cannot be deleted. Verified by EXECUTION
//! at `-O0` and `-O2` in `compiler/tests/llvm_abi_guard_execution_test.rs`; see
//! `the_guard_survives_at_O2` there for what is and is not claimed.
//!
//! # What CANNOT be enforced here, stated plainly
//!
//! Naso's central claim is that a `[1]` resource is consumed exactly once and does
//! not outlive its scope, with no garbage collector. Inside a function body the
//! typechecker delivers that. Across this ABI none of it survives, and no attribute
//! recovers it:
//!
//! - **The caller's buffer outliving the call.** A `ptr` carries no lifetime. The
//!   callee cannot observe when the caller's allocation is freed, and nothing in
//!   the signature says the buffer is borrowed rather than given away.
//! - **The callee not retaining the pointer.** Nothing prevents the generated code
//!   from storing `%input` into a global. The compiler does not emit such a store
//!   today, but "it does not today" is a property of today's emitter, not of the
//!   ABI.
//! - **Consumed exactly once.** Linearity counts how many times the BODY reads the
//!   name, which the typechecker already does statically. Counting it again at run
//!   time would require instrumenting every load and would prove nothing about the
//!   caller.
//! - **Read-only vs writable.** `inout` (the callee may write) and `[1]` (consumed
//!   by reading) are the SAME `ptr` here, and they must stay the same. See below.
//!
//! # Why `noalias` and `readonly` are deliberately NOT emitted
//!
//! It is tempting to mark a `[1]` tensor `noalias` and a read-only one `readonly`.
//! Both would be FALSE PROMISES, and both are worse than emitting nothing.
//!
//! `naso_entry(input, output, ...)` called as `naso_entry(buf, buf, ...)` -- the same
//! buffer as the read-only input and the writable output -- is an ordinary,
//! correct, memory-safe program; in-place elementwise scaling is the obvious one.
//! `noalias` on `%input` asserts no other pointer in the program reaches that memory,
//! which is false for that call. `readonly` on `%input` asserts the pointee is not
//! written during the call, which is also false: the callee writes it through
//! `%output`. Either attribute turns a legal program into undefined behaviour and
//! licenses the optimiser to produce a different answer.
//!
//! So the linear/non-linear distinction is NOT representable in this ABI, and this
//! module says so rather than emitting something that looks like enforcement. What
//! survives is the SHAPE: the declared extent is checked against the caller's buffer
//! at run time. Everything about the resource's lifetime is a compile-time property
//! of the callee body, checked by the typechecker, and it stops at this boundary.
//!
//! `compiler/tests/llvm_abi_guard_execution_test.rs` pins the aliasing case by
//! RUNNING it, so adding either attribute later fails a test rather than silently
//! corrupting a legitimate program.

use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::module_builder::EntryParam;
use crate::ir::pir_types::ParamKind;
use inkwell::AddressSpace;
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module as LlvmModule};
use inkwell::types::PointerType;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};

/// Exit code for a null tensor pointer.
///
/// Distinct from [`NASO_ABI_EXIT_SHORT`] so a caller -- and a test -- can tell the
/// two failures apart from the exit status alone, without parsing a message. That
/// matters because a message can be swallowed and an exit code cannot.
pub const NASO_ABI_EXIT_NULL: i64 = 90;

/// Exit code for a tensor buffer shorter than its declared extent.
pub const NASO_ABI_EXIT_SHORT: i64 = 91;

/// One tensor parameter's ABI arguments: the data pointer and its element count.
struct TensorSlot<'ctx> {
    name: String,
    ptr: PointerValue<'ctx>,
    len: IntValue<'ctx>,
    declared: u64,
}

/// Emit the boundary guard for every tensor parameter of `function`, in `abi` order.
///
/// The builder must be positioned in the block the guard starts from -- the entry
/// block -- with the arguments already bound. On return it is positioned in the last
/// block of the guard chain, which is where the body continues: each check ends in a
/// fresh open block that the next check branches out of, so the schedule lowering
/// that follows appends into an unterminated block.
///
/// A module with no tensor parameter emits NOTHING here: no blocks, no `exit`
/// declaration, no message strings. A scalar-only kernel's IR is unchanged.
pub fn emit_abi_guard<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    function: FunctionValue<'ctx>,
    builder: &LlvmBuilder<'ctx>,
    abi: &[EntryParam<'ctx>],
) -> CodegenResult<()> {
    let slots = collect_tensor_slots(function, abi)?;
    if slots.is_empty() {
        return Ok(());
    }
    declare_violation_runtime(context, module)?;

    for slot in &slots {
        let violation_null =
            append_block(context, function, &format!("naso.abi.null.{}", slot.name));
        let after_null = append_block(
            context,
            function,
            &format!("naso.abi.null.{}.cont", slot.name),
        );

        // 1. NULL, checked first and separately: a null pointer with a length of
        // zero would otherwise be reported as "too short", which sends the reader
        // looking for an allocation problem instead of the missing argument.
        let is_null = builder
            .build_is_null(slot.ptr, &format!("naso.is_null.{}", slot.name))
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        builder
            .build_conditional_branch(is_null, violation_null, after_null)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        builder.position_at_end(violation_null);
        emit_violation(
            context,
            module,
            builder,
            &format!("naso.abi.violation.null.{}", slot.name),
            &format!(
                "naso: ABI violation: parameter `{}` is a NULL pointer. A tensor \
                 parameter is a caller-owned buffer; the callee does not allocate one.\n",
                slot.name
            ),
            NASO_ABI_EXIT_NULL,
        )?;
        // Back into the continuation block: `emit_violation` left the builder in the
        // violation block, which is now terminated by `unreachable`. Leaving it there
        // would append the rest of the guard to a block that can never be reached, and
        // the branch that follows would be emitted after a terminator.
        builder.position_at_end(after_null);

        // 2. SHORT. `slt`, not `ne`: a caller whose buffer is LARGER than the
        // declared extent is running a correct program, and refusing it would be
        // the guard inventing a rule. See the module doc.
        let violation_short =
            append_block(context, function, &format!("naso.abi.short.{}", slot.name));
        let after_short = append_block(
            context,
            function,
            &format!("naso.abi.short.{}.cont", slot.name),
        );

        let declared_const = slot.len.get_type().const_int(slot.declared, false);
        let too_short = builder
            .build_int_compare(
                // SIGNED, so a caller passing a negative length is caught. Unsigned,
                // `i64 -1` reads as `u64::MAX`, the largest possible buffer, and
                // `ult` would wave it through -- a guard defeated by the value it
                // exists to reject.
                inkwell::IntPredicate::SLT,
                slot.len,
                declared_const,
                &format!("naso.too_short.{}", slot.name),
            )
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        builder
            .build_conditional_branch(too_short, violation_short, after_short)
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

        builder.position_at_end(violation_short);
        let last_index = slot.declared.saturating_sub(1);
        // `elements` is pluralised from the count: a `Tensor[f32, 1]` parameter
        // saying "declares 1 elements" reads like a machine-generated diagnostic,
        // and this message is the one a person debugging a wrong buffer sees.
        let plural = if slot.declared == 1 { "" } else { "s" };
        emit_violation(
            context,
            module,
            builder,
            &format!("naso.abi.violation.short.{}", slot.name),
            &format!(
                "naso: ABI violation: parameter `{}` declares {} element{}, but the \
                 caller supplied fewer. Indices 0..{} are read.\n",
                slot.name, slot.declared, plural, last_index
            ),
            NASO_ABI_EXIT_SHORT,
        )?;
        builder.position_at_end(after_short);
    }
    Ok(())
}

fn append_block<'ctx>(
    context: &'ctx Context,
    function: FunctionValue<'ctx>,
    name: &str,
) -> BasicBlock<'ctx> {
    context.append_basic_block(function, name)
}

/// The tensor parameters of `abi`, paired with their two LLVM arguments.
///
/// A symbolic extent cannot appear here: `build_entry_signature` refuses one, and a
/// parameter with no declared extent has no bound to check, so it is given no length
/// argument. An entry whose kind disagrees with its ABI shape is an inconsistency
/// between two functions documented to compute the signature identically, so it is
/// reported rather than skipped -- skipping would leave an unchecked pointer in a
/// function that believes it checked one.
fn collect_tensor_slots<'ctx>(
    function: FunctionValue<'ctx>,
    abi: &[EntryParam<'ctx>],
) -> CodegenResult<Vec<TensorSlot<'ctx>>> {
    let mut slots = Vec::new();
    for entry in abi {
        let ParamKind::Tensor { shape, .. } = &entry.kind else {
            continue;
        };
        let Some(shape) = shape else {
            return Err(CodegenError::InstructionError(format!(
                "tensor parameter `{}` reached the ABI guard with no declared extent. \
                 `build_entry_signature` refuses a symbolic extent, so either the \
                 signature and this guard disagree about which parameters are tensors, \
                 or a symbolic extent is no longer refused.",
                entry.name
            )));
        };
        let ptr_arg = function.get_nth_param(entry.arg_index).ok_or_else(|| {
            CodegenError::InstructionError(format!(
                "tensor parameter `{}` claims ABI argument {} but the function has \
                 fewer arguments",
                entry.name, entry.arg_index
            ))
        })?;
        let len_arg = entry.len_arg_index.ok_or_else(|| {
            CodegenError::InstructionError(format!(
                "tensor parameter `{}` has no length argument in its ABI slot. Every \
                 tensor parameter takes `(ptr, i64 len)`; see this module's docs.",
                entry.name
            ))
        })?;
        let len = function
            .get_nth_param(len_arg)
            .ok_or_else(|| {
                CodegenError::InstructionError(format!(
                    "tensor parameter `{}` claims length argument {len_arg} but the \
                     function has fewer arguments",
                    entry.name
                ))
            })?
            .into_int_value();
        slots.push(TensorSlot {
            name: entry.name.clone(),
            ptr: ptr_arg.into_pointer_value(),
            len,
            declared: element_count(&entry.name, shape)?,
        });
    }
    Ok(slots)
}

/// The element count a declared shape requires: the product of its extents.
///
/// `checked_product`, because a shape whose extents overflow `u64` cannot be
/// compared against an `i64` length without inventing a bound. That is refused with
/// the shape that caused it rather than wrapping to a small number -- a wrapped
/// product would be a bound the source never stated, and every index would then be
/// checked against the wrong number.
fn element_count(name: &str, shape: &[u64]) -> CodegenResult<u64> {
    shape
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| {
            CodegenError::UnsupportedFeature(format!(
                "tensor parameter `{name}` has shape {shape:?}, whose element count \
                 overflows a 64-bit integer. The ABI guard compares the caller's \
                 length against that count, and there is no representable count to \
                 compare against."
            ))
        })
}

/// Declare `exit`, `fputs` and `stderr` in `module`, once each.
///
/// `exit` is marked `noreturn` because it is: the guard emits `unreachable` after
/// the call, and without the attribute LLVM has no reason to believe the block
/// cannot fall through into the body. The attribute kind is looked up by NAME
/// because LLVM's enum attribute numbering is not stable across releases, and a
/// hard-coded id would attach whatever attribute happens to have that number.
fn declare_violation_runtime<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
) -> CodegenResult<()> {
    let ptr_type = context.ptr_type(AddressSpace::default());

    if module.get_function("exit").is_none() {
        let exit_type = context
            .void_type()
            .fn_type(&[context.i32_type().into()], false);
        let exit_fn = module.add_function("exit", exit_type, None);
        let noreturn_kind = Attribute::get_named_enum_kind_id("noreturn");
        if noreturn_kind == 0 {
            return Err(CodegenError::InternalError(
                "this LLVM build has no `noreturn` attribute, so the ABI guard's \
                 failure path cannot be marked as not returning. Refusing rather than \
                 emitting an `unreachable` after a call that might return: the \
                 generated code would continue executing the kernel body after the \
                 diagnostic, which is the silent wrong answer this guard exists to \
                 prevent."
                    .to_string(),
            ));
        }
        exit_fn.add_attribute(
            AttributeLoc::Function,
            context.create_enum_attribute(noreturn_kind, 0),
        );
    }

    if module.get_function("fputs").is_none() {
        let fputs_type = context
            .i32_type()
            .fn_type(&[ptr_type.into(), ptr_type.into()], false);
        module.add_function("fputs", fputs_type, None);
    }

    // `stderr` is a libc global of type `FILE *`, which under LLVM 17's opaque
    // pointers is exactly `ptr`, so the declaration needs no pointee type.
    if module.get_global("stderr").is_none() {
        let stderr = module.add_global(ptr_type, None, "stderr");
        stderr.set_linkage(Linkage::External);
    }
    Ok(())
}

/// Emit the failure path: print `message` to stderr, then exit with `code`.
///
/// These two calls are the whole reason this guard cannot be optimised away.
fn emit_violation<'ctx>(
    context: &'ctx Context,
    module: &LlvmModule<'ctx>,
    builder: &LlvmBuilder<'ctx>,
    global_name: &str,
    message: &str,
    code: i64,
) -> CodegenResult<()> {
    // A separate global per violation: a shared name would make the second message
    // reuse the first one's text, so a null pointer would be reported with the
    // short-buffer wording.
    let message_ptr: PointerValue<'ctx> = builder
        .build_global_string_ptr(message, global_name)
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?
        .as_pointer_value();

    let stderr_global = module
        .get_global("stderr")
        .ok_or_else(|| CodegenError::InternalError("stderr was not declared".to_string()))?;
    let stderr_ptr: PointerValue<'ctx> = builder
        .build_load(
            pointer_type(context),
            stderr_global.as_pointer_value(),
            "stderr",
        )
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?
        .into_pointer_value();

    let fputs = module
        .get_function("fputs")
        .ok_or_else(|| CodegenError::InternalError("fputs was not declared".to_string()))?;
    builder
        .build_call(fputs, &[message_ptr.into(), stderr_ptr.into()], "")
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

    let exit_fn = module
        .get_function("exit")
        .ok_or_else(|| CodegenError::InternalError("exit was not declared".to_string()))?;
    let code_value: BasicValueEnum<'ctx> = context.i32_type().const_int(code as u64, true).into();
    builder
        .build_call(exit_fn, &[code_value.into()], "")
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;

    // `exit` is `noreturn`, so this block genuinely has no successor. The
    // terminator is required for the block to be well formed.
    builder
        .build_unreachable()
        .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
    Ok(())
}

fn pointer_type<'ctx>(context: &'ctx Context) -> PointerType<'ctx> {
    context.ptr_type(AddressSpace::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_exit_codes_are_distinct_and_non_zero() {
        // A caller told "90" and a caller told "91" must be able to tell their
        // mistakes apart. If these ever became equal -- or either became 0 -- the
        // distinction would survive only in a message, which a caller can lose.
        assert_ne!(NASO_ABI_EXIT_NULL, NASO_ABI_EXIT_SHORT);
        assert_ne!(NASO_ABI_EXIT_NULL, 0);
        assert_ne!(NASO_ABI_EXIT_SHORT, 0);
    }

    #[test]
    fn element_count_is_the_product_of_the_extents() {
        assert_eq!(element_count("t", &[4]).unwrap(), 4);
        assert_eq!(element_count("t", &[2, 3]).unwrap(), 6);
        assert_eq!(element_count("t", &[2, 3, 5]).unwrap(), 30);
    }

    #[test]
    fn an_overflowing_shape_is_refused_rather_than_wrapped() {
        // Wrapping would turn [2^63+1, 4] into 4 and check every index against a
        // number the source never stated -- a guard that is confidently wrong.
        let err = element_count("t", &[u64::MAX, 4]).unwrap_err();
        assert!(
            err.to_string().contains("overflows"),
            "refusal must name the overflow, got: {err}"
        );
    }
}
