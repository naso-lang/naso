// LLVM Module Builder
//
// High-level wrapper around inkwell::module::Module for building LLVM IR.

use crate::ast::{Span, Type, TypeKind};
use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::function_emission;
use crate::codegen::llvm::schedule_lowering::{
    lower_schedule_tree_into, schedule_parameters, statements_under,
};
use crate::codegen::llvm::type_lowering::LlvmTypeLowering;
use crate::codegen::llvm::value_builder::LlvmValueBuilder;
use crate::ir::pir_types::{ParamKind, PirModule};
use crate::ir::schedule_tree::StmtId;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder as LlvmBuilder;
use inkwell::module::Module as LlvmModule;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{AnyValue, BasicMetadataValueEnum, FunctionValue, PointerValue};
use std::collections::HashMap;

// LLVM Module Builder for constructing LLVM IR from PIR
pub struct LLVMModuleBuilder<'ctx> {
    context: &'ctx CodegenContext,
    module: LlvmModule<'ctx>,
    type_lowering: LlvmTypeLowering<'ctx>,
    // The single inkwell builder used for all instruction emission.
    // It lives inside [`LlvmValueBuilder`]; this type does not keep a second
    // copy because `inkwell::builder::Builder` is neither `Copy` nor `Clone`
    // (it owns an `LLVMBuilderRef` and implements `Drop`), so two builders
    // would silently drift apart in insertion position.
    value_builder: Option<LlvmValueBuilder<'ctx>>,
    // Current function being built
    current_function: Option<FunctionValue<'ctx>>,
    // Current basic block
    current_block: Option<BasicBlock<'ctx>>,
    // Each Naso function's declared parameter kinds, by name.
    //
    // Read by the expression lowering at a CALL SITE, which needs the CALLEE's
    // declaration to marshal arguments: one source argument is two LLVM arguments for
    // a tensor. Held here rather than recomputed at each call because a call inside a
    // loop body is lowered deep inside `ScheduleLowering`, which has no access to the
    // module's function list.
    callee_params: Option<HashMap<String, Vec<ParamKind>>>,
}

/// One DECLARED parameter's share of the entry function's signature.
///
/// A tensor occupies TWO LLVM arguments -- its pointer and its element count -- so
/// this is not one slot but a small run of them. `arg_index` is where the run starts
/// and `len_arg_index` is the second argument of a tensor's run. Keeping both on one
/// struct is what stops the signature builder and the guard emitter from disagreeing
/// about which argument is which: they read the same numbers from the same value
/// rather than each re-deriving an index from a position in a list.
///
/// # Why a tensor carries its length
///
/// `Tensor[f32, 4]` declares four elements, and this backend knew that and then did
/// nothing with it: a bare `ptr` cannot be checked against anything, because the
/// caller supplies no length to check against. The fix is to make the caller's
/// obligation part of the ABI rather than an assumption. See
/// [`crate::codegen::llvm::abi_guard`] for the guard, the failure codes, and an exact
/// statement of which QTT guarantees stop at this boundary.
pub struct EntryParam<'ctx> {
    /// The name the body reads this value by.
    pub name: String,
    /// The LLVM type of the FIRST argument of this parameter's run. A tensor's is
    /// `ptr`; a scalar's is its own type.
    pub ty: BasicTypeEnum<'ctx>,
    /// Index of this parameter's first LLVM argument.
    pub arg_index: u32,
    /// For a tensor with a known extent: index of the `i64` element count that
    /// follows its pointer. `None` for every other kind.
    ///
    /// `None` for a scalar (its value carries its own extent) and unreachable for a
    /// tensor with a symbolic extent, which `build_entry_signature` refuses before
    /// this is ever read.
    pub len_arg_index: Option<u32>,
    /// What the slot holds, for the binding step.
    ///
    /// A tensor's `elem` is the ELEMENT type, not `ptr`: it is what a `getelementptr`
    /// steps by and what a load returns. Storing `ptr` here would make `input[i]` a
    /// load of a pointer -- the exact confusion this ABI removed.
    pub kind: ParamKind,
}

/// Build the entry function's signature from `pir_module`.
///
/// The order is `PirModule::function_params` (declared parameters, source order)
/// followed by any schedule symbolic constant not among them. `lower_schedule_tree_into`
/// binds arguments by INDEX into this same list, so both sides must compute it
/// identically -- hence one function, exported, rather than two traversals.
///
/// Each entry records the LLVM arguments its declared parameter occupies: ONE for a
/// scalar, TWO for a tensor (pointer, then element count). The types are computed
/// together with the kind because a tensor's slot is a pointer while its kind carries
/// the element type; deriving one from the other separately is how they drift apart.
///
/// # The element-count argument
///
/// A tensor's second argument is the caller's obligation made explicit: the number of
/// ELEMENTS the buffer holds, which [`crate::codegen::llvm::abi_guard`] compares
/// against the product of the declared extents. Without it the declared extent is
/// knowledge the compiler has and cannot use -- a `Tensor[f32, 4]` kernel handed a
/// one-element buffer reads three elements past the end and says nothing.
///
/// The width is `i64` because that is what loop bounds are computed in throughout this
/// backend. The guard's comparison is SIGNED (`icmp slt`) for a reason worth stating
/// here, since this is where the argument's type is chosen: an unsigned `ult` against
/// a declared count treats `i64 -1` as `u64::MAX`, the largest possible buffer, so a
/// caller passing a negative length would sail past the check. Signed comparison
/// catches that for free, because `-1 < 4`.
pub fn build_entry_signature<'ctx>(
    type_lowering: &mut LlvmTypeLowering<'ctx>,
    pir_module: &PirModule,
) -> CodegenResult<Vec<EntryParam<'ctx>>> {
    let mut out: Vec<EntryParam<'ctx>> = Vec::new();
    // Every argument index handed out so far. A tensor consumes two, so the next
    // index cannot be `out.len()` -- which is exactly the bug a naive `enumerate`
    // over `out` would introduce: binding the second tensor's pointer to the first
    // tensor's element count, and reading a length where a buffer is expected.
    let mut next_arg: u32 = 0;
    for param in &pir_module.function_params {
        let (ty, kind) = param_slot(type_lowering, param)?;
        let takes_length = matches!(kind, ParamKind::Tensor { .. });
        out.push(EntryParam {
            name: param.name.clone(),
            ty,
            arg_index: next_arg,
            len_arg_index: takes_length.then_some(next_arg + 1),
            kind,
        });
        next_arg += if takes_length { 2 } else { 1 };
    }
    // A symbolic constant the schedule names but no function declared. It is an i64
    // because loop bounds are computed in i64 throughout this backend, and it is a real
    // argument rather than an invented constant: a trip count that came from anywhere
    // else would iterate a different number of times than the source says.
    let i64_type = type_lowering.int_type(crate::codegen::abi::IntWidth::I64);
    for name in schedule_parameters(&pir_module.schedule) {
        if out.iter().any(|p| p.name == name) {
            continue;
        }
        out.push(EntryParam {
            name,
            ty: i64_type.into(),
            arg_index: next_arg,
            len_arg_index: None,
            kind: ParamKind::Scalar(crate::ir::pir_types::ElemType::I64),
        });
        next_arg += 1;
    }
    Ok(out)
}

/// Flatten [`build_entry_signature`]'s entries into the entry function's LLVM
/// parameter types, in argument order.
///
/// A tensor contributes two types (`ptr`, then `i64`). This fills by the recorded
/// `arg_index` rather than assuming a fixed stride, so the type list and the argument
/// indices cannot come from two different assumptions about the same layout.
pub fn entry_arg_types<'ctx>(
    type_lowering: &LlvmTypeLowering<'ctx>,
    abi: &[EntryParam<'ctx>],
) -> CodegenResult<Vec<BasicTypeEnum<'ctx>>> {
    let i64_type: BasicTypeEnum<'ctx> = type_lowering
        .int_type(crate::codegen::abi::IntWidth::I64)
        .into();
    let total = abi
        .iter()
        .map(|e| e.arg_index + 1 + u32::from(e.len_arg_index.is_some()))
        .max()
        .unwrap_or(0);
    let mut types: Vec<Option<BasicTypeEnum<'ctx>>> = vec![None; total as usize];
    for entry in abi {
        types[entry.arg_index as usize] = Some(entry.ty);
        if let Some(len_index) = entry.len_arg_index {
            types[len_index as usize] = Some(i64_type);
        }
    }
    // A hole means two parameters claimed the same argument index, which is an
    // internal inconsistency between the two functions that read this layout.
    types
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            t.ok_or_else(|| {
                CodegenError::InstructionError(format!(
                    "entry argument {i} was claimed by no parameter. \
                     `build_entry_signature` and `entry_arg_types` disagree about the \
                     argument layout, so the generated function would have an unnamed \
                     or untyped argument."
                ))
            })
        })
        .collect()
}

/// The LLVM slot type and binding kind for one declared parameter.
fn param_slot<'ctx>(
    type_lowering: &mut LlvmTypeLowering<'ctx>,
    param: &crate::ir::pir_types::FunctionParam,
) -> CodegenResult<(BasicTypeEnum<'ctx>, ParamKind)> {
    use crate::ir::pir_types::ElemType;
    let ptr_ty = type_lowering
        .ptr_type(i64_placeholder(type_lowering).into(), 0)?
        .into();
    Ok(match &param.kind {
        // A tensor is a POINTER to the caller's buffer. It is never an alloca and never
        // zero-filled: a callee-allocated tensor is one the caller cannot supply, and a
        // function that reads it computes on data nobody provided -- building clean and
        // silently wrong.
        ParamKind::Tensor { elem, shape } => {
            let Some(shape) = shape else {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "parameter `{}` has a symbolic tensor extent. The entry signature \
                     needs the shape to linearise a subscript (`t[i][j]` is \
                     `i * cols + j`), and a symbolic extent needs monomorphisation, \
                     which is not implemented. Give the tensor literal extents.",
                    param.name
                )));
            };
            // The shape is recorded but not used to size anything: the caller owns the
            // allocation and knows its length. It is validated here only so a
            // zero-element dimension is refused rather than producing a stride of zero
            // that would alias every index onto one element.
            if shape.is_empty() || shape.contains(&0) {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "parameter `{}` is a tensor with shape {shape:?}, which has no \
                     elements to index or write",
                    param.name
                )));
            }
            (
                ptr_ty,
                ParamKind::Tensor {
                    elem: *elem,
                    shape: Some(shape.clone()),
                },
            )
        }
        // A scalar is passed BY VALUE. LLVM functions have no such restriction as WGSL
        // compute entry points do, so there is no reason to route it through memory.
        ParamKind::Scalar(elem) => {
            let ty: BasicTypeEnum<'ctx> = match elem {
                ElemType::F64 => type_lowering
                    .float_type(crate::codegen::abi::FloatWidth::F64)
                    .into(),
                // A SCALAR `i4` has no byte to live in. Packing is defined for a
                // tensor element, where the index says which nibble; a bare `i4` value
                // has no such context, so there is no honest slot for it. Returning an
                // `i8` here would widen silently -- the kernel would report 4-bit
                // quantization while running at 8 bits.
                ElemType::I4 => {
                    return Err(CodegenError::UnsupportedFeature(
                        "a scalar `i4` parameter has no storage: sub-byte values are defined \
                         only as TENSOR elements, where the index selects the nibble within \
                         a shared byte. A scalar `i4` is refused rather than widened to `i8`, \
                         which would report a compression ratio the code does not achieve"
                            .to_string(),
                    ));
                }
                ElemType::I8 => type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I8)
                    .into(),
                ElemType::I16 => type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I16)
                    .into(),
                ElemType::I32 => type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I32)
                    .into(),
                ElemType::I64 => type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I64)
                    .into(),
                ElemType::Bool => type_lowering
                    .int_type(crate::codegen::abi::IntWidth::I1)
                    .into(),
            };
            (ty, ParamKind::Scalar(*elem))
        }
        // A quantum register is a caller-owned pointer, but nothing in this backend
        // indexes one, so the binding exists while every read of it stays a diagnostic.
        ParamKind::QRegister => (ptr_ty, ParamKind::QRegister),
        ParamKind::Unsupported(ty) => {
            return Err(CodegenError::UnsupportedFeature(format!(
                "parameter `{}` has type `{ty}`, which has no slot in the LLVM function \
                 ABI. Nothing is substituted for it: an invented slot would let the \
                 body read a value the caller never passed.",
                param.name
            )));
        }
    })
}

/// Name a generated function's arguments from its ABI.
///
/// A tensor's element count is the argument AFTER its pointer, so both indices come
/// from the recorded `EntryParam` rather than from a position in the parameter list.
/// A mismatch here is not cosmetic: `lower_schedule_tree_into` binds argument `j` to
/// `EntryParam::arg_index == j`, so a parameter bound to the wrong argument is a
/// function that reads a length where a buffer is expected.
fn name_arguments<'ctx>(function: FunctionValue<'ctx>, abi: &[EntryParam<'ctx>]) {
    for entry in abi {
        if let Some(arg) = function.get_nth_param(entry.arg_index) {
            arg.set_name(&entry.name);
        }
        if let Some(arg) = entry.len_arg_index.and_then(|i| function.get_nth_param(i)) {
            arg.set_name(&format!("{}_len", entry.name));
        }
    }
}

/// The LLVM return type of one Naso function, or `None` for void.
///
/// `FnReturn::Unsupported` is REFUSED here rather than mapped to void: a void function
/// whose callers bind a value is a compile that succeeds and computes nothing, which is
/// the failure this compiler is built to avoid. The message names the type, because
/// "unsupported return type" without the type sends the reader looking through the
/// whole type system for it.
pub fn function_return_type<'ctx>(
    type_lowering: &mut LlvmTypeLowering<'ctx>,
    func: &crate::ir::pir_types::PirFunction,
) -> CodegenResult<Option<BasicTypeEnum<'ctx>>> {
    use crate::codegen::abi::{FloatWidth, IntWidth};
    use crate::ir::pir_types::{ElemType, FnReturn};
    Ok(Some(match &func.return_type {
        FnReturn::Void => return Ok(None),
        FnReturn::Scalar(ElemType::F64) => type_lowering.float_type(FloatWidth::F64).into(),
        // As with a scalar parameter: a returned `i4` has no byte to pack into, so it is
        // refused rather than widened. See the scalar-parameter arm for the reasoning.
        FnReturn::Scalar(ElemType::I4) => {
            return Err(CodegenError::UnsupportedFeature(
                "a scalar `i4` return has no storage: sub-byte values are defined only as \
                 TENSOR elements. Widening it to `i8` would report 4-bit quantization while \
                 running at 8 bits"
                    .to_string(),
            ));
        }
        FnReturn::Scalar(ElemType::I8) => type_lowering.int_type(IntWidth::I8).into(),
        FnReturn::Scalar(ElemType::I16) => type_lowering.int_type(IntWidth::I16).into(),
        FnReturn::Scalar(ElemType::I32) => type_lowering.int_type(IntWidth::I32).into(),
        FnReturn::Scalar(ElemType::I64) => type_lowering.int_type(IntWidth::I64).into(),
        FnReturn::Scalar(ElemType::Bool) => type_lowering.int_type(IntWidth::I1).into(),
        FnReturn::Unsupported(ty) => {
            return Err(CodegenError::UnsupportedFeature(format!(
                "function `{}` returns `{ty}`, which has no slot in this ABI. Nothing \
                 is substituted for it: an invented return type would let a caller bind \
                 a value the function never produces.",
                func.name
            )));
        }
    }))
}

/// A `PirModule` view holding exactly ONE function's state.
///
/// Built from the `PirFunction`, never by filtering the flat module. That direction
/// matters: a filter would have to decide which of several `input` bindings belongs to
/// this function, and getting it wrong reproduces exactly the bug that made
/// `kernels/quant_int8.naso` uncompilable -- silently binding one function's
/// `Tensor[i8, 1024] input` slot to another function's `Tensor[f32, 1024]`.
///
/// `extern_functions` is carried through unchanged: an `extern` declaration is visible
/// to every function, which is what an extern means.
pub fn function_view(
    func: &crate::ir::pir_types::PirFunction,
    pir_module: &PirModule,
) -> PirModule {
    PirModule {
        statements: func.statements.clone(),
        schedule: func.schedule.clone(),
        accesses: func.accesses.clone(),
        quantities: func.quantities.clone(),
        parameters: pir_module.parameters.clone(),
        function_params: func.params.clone(),
        extern_functions: pir_module.extern_functions.clone(),
        functions: vec![func.clone()],
    }
}

/// The one synthetic function a `PirFunction`-less module is lowered as.
///
/// # Why this exists
///
/// A module with no `functions` is a HAND-BUILT one: a `.pir` fixture parsed by
/// `compiler/tests/codegen_tests.rs`, or a test constructing a `PirModule` literal.
/// Those describe one flat statement list, which under the old backend was one LLVM
/// function. Making that explicit as a single `PirFunction` means the multi-function
/// path needs no special case beyond this, and the fixtures keep working with NO change
/// -- a `[statements]` fixture needs no function section because a fixture describes
/// one function.
///
/// The name is `PirModule`'s own, not `main`: `entry_function_index` picks the first
/// function when there is no `main`, and there is exactly one, so this is the entry
/// whatever it is called. Naming it `main` would be a lie about a fixture that never
/// mentioned `main`.
fn synthetic_single_function(pir_module: &PirModule) -> crate::ir::pir_types::PirFunction {
    crate::ir::pir_types::PirFunction {
        name: "entry".to_string(),
        params: pir_module.function_params.clone(),
        statements: pir_module.statements.clone(),
        schedule: pir_module.schedule.clone(),
        accesses: pir_module.accesses.clone(),
        quantities: pir_module.quantities.clone(),
        return_type: crate::ir::pir_types::FnReturn::Void,
        return_stmt: None,
        // A hand-built module has no `Block::expr`: it was written as a statement
        // list, so there is no trailing expression that could be an implicit return.
        // Fabricating one here would let a fixture return something its source never
        // wrote.
        tail_return_stmt: None,
        span: None,
    }
}

/// The expression a function RETURNS, taken from the statement lowering named.
///
/// `id` is [`crate::ir::pir_types::PirFunction::return_stmt`] for an explicit
/// `return e` and [`crate::ir::pir_types::PirFunction::tail_return_stmt`] for an
/// implicit tail. Both name a statement in the same list, so this lookup is shared;
/// WHICH one is being resolved is the caller's business, and it is decided in
/// `emit_one_function` by a rule that names both fields explicitly.
///
/// Refused rather than substituted if the id names no statement: a return id that
/// does not resolve means lowering and codegen disagree about the function's body, and
/// emitting `undef` there would return an arbitrary value that still passes every check
/// downstream.
fn return_expression(
    func: &crate::ir::pir_types::PirFunction,
    id: crate::ir::schedule_tree::StmtId,
) -> CodegenResult<&crate::ir::pir_types::PirExpr> {
    func.statements
        .iter()
        .find(|s| s.id == id)
        .map(|s| &s.body)
        .ok_or_else(|| {
            CodegenError::InstructionError(format!(
                "function `{}` records its return statement as {} but no statement with \
                 that id exists. Lowering and codegen disagree about the function's \
                 body, and returning an unspecified value would be a silent wrong \
                 answer.",
                func.name, id.0
            ))
        })
}

/// Which source construct supplies a function's return value.
///
/// Recorded rather than collapsed to a bare `Option<&PirExpr>` because the two are
/// NOT interchangeable downstream: the diagnostic for a type mismatch says "`return`
/// expression" for one and "trailing expression" for the other, and a caller reading
/// only the value could not tell which of the two programs it is looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnSource {
    /// `return e` written in the source.
    Explicit,
    /// The block's trailing expression, with no `return` in the body.
    ImplicitTail,
}

impl ReturnSource {
    /// How this source construct is named in a diagnostic.
    pub fn describe(self) -> &'static str {
        match self {
            ReturnSource::Explicit => "`return` expression",
            ReturnSource::ImplicitTail => "trailing expression",
        }
    }
}

/// A throwaway integer type used only to build an opaque pointer.
///
/// LLVM 17 pointers are opaque, so the pointee is erased in the resulting type and the
/// argument does not affect the pointer's identity. `LlvmTypeLowering::ptr_type` takes
/// one anyway to keep the caller's intent visible, so a real type has to be supplied.
fn i64_placeholder<'ctx>(type_lowering: &LlvmTypeLowering<'ctx>) -> inkwell::types::IntType<'ctx> {
    type_lowering.int_type(crate::codegen::abi::IntWidth::I64)
}

impl<'ctx> LLVMModuleBuilder<'ctx> {
    // Create a new module builder
    pub fn new(context: &'ctx CodegenContext) -> CodegenResult<Self> {
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("naso_module");
        let builder = llvm_context.create_builder();
        let type_lowering = LlvmTypeLowering::new(llvm_context);

        // `LlvmValueBuilder` owns the builder and its own type lowering, so it is
        // constructed before `Self` (the builder cannot be copied out of it
        // afterwards).
        let value_builder = LlvmValueBuilder::new(builder, LlvmTypeLowering::new(llvm_context));

        Ok(Self {
            context,
            module,
            type_lowering,
            value_builder: Some(value_builder),
            current_function: None,
            current_block: None,
            callee_params: None,
        })
    }

    // The inkwell builder shared by this type and its value builder
    fn llvm_builder(&self) -> &LlvmBuilder<'ctx> {
        self.value_builder
            .as_ref()
            .expect("value_builder not initialized")
            .builder()
    }

    // Get the underlying LLVM module
    pub fn module(&self) -> &LlvmModule<'ctx> {
        &self.module
    }

    // Get the LLVM context
    pub fn llvm_context(&self) -> &inkwell::context::Context {
        self.context.llvm_context()
    }

    // Get the type lowering context
    pub fn type_lowering(&mut self) -> &mut LlvmTypeLowering<'ctx> {
        &mut self.type_lowering
    }

    // Get the value builder
    pub fn value_builder(&mut self) -> &mut LlvmValueBuilder<'ctx> {
        self.value_builder
            .as_mut()
            .expect("value_builder not initialized")
    }

    // Set the current function
    pub fn set_current_function(&mut self, func: FunctionValue<'ctx>) {
        self.current_function = Some(func);
    }

    // Get the current function
    pub fn current_function(&self) -> Option<FunctionValue<'ctx>> {
        self.current_function
    }

    // Set the current basic block
    pub fn set_current_block(&mut self, block: BasicBlock<'ctx>) {
        self.current_block = Some(block);
        self.llvm_builder().position_at_end(block);
    }

    // Get the current basic block
    pub fn current_block(&self) -> Option<BasicBlock<'ctx>> {
        self.current_block
    }

    // Add a variable allocation
    // `ty` is the pointee type of the allocation; LLVM 17 opaque pointers do
    // not carry it, so it is recorded for the later `build_load`.
    //
    // This writes into the value builder's map, which is the SAME map the
    // schedule-lowering path reads: a name bound here is visible inside a
    // schedule band body, and vice versa.
    pub fn add_variable(&mut self, name: String, ptr: PointerValue<'ctx>, ty: BasicTypeEnum<'ctx>) {
        self.value_builder().add_variable(name, ptr, ty);
    }

    // Get a variable allocation
    pub fn get_variable(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.value_builder
            .as_ref()
            .expect("value_builder not initialized")
            .variable(name)
            .map(|(ptr, _)| ptr)
    }

    // Build the entire PIR module.
    //
    // # One LLVM symbol per Naso function
    //
    // This used to emit ONE `naso_entry` for the whole `PirModule`, because a
    // `PirModule` WAS one flat statement list with no function structure: `lower_item`
    // concatenated every function's body into it. Every function's parameters and
    // locals therefore shared one namespace in one LLVM body, and
    // `kernels/quant_int8.naso` could not compile at all -- its three functions each
    // declare `input`/`output`/`scale`, at `Tensor[f32, 1024]` in one and
    // `Tensor[i8, 1024]` in another, and one LLVM function has ONE slot per name.
    //
    // `PirModule::functions` now carries one `PirFunction` per Naso function, each with
    // its own parameters, statements, schedule, accesses and quantities, and each becomes
    // its own `define`. See `codegen::llvm::function_emission` for the naming
    // convention, the entry selection rule, and the recursion refusal.
    //
    // # The per-function PIR VIEW, and why it is built here
    //
    // `lower_schedule_tree_into` and `ScheduleLowering` read a `&PirModule`. Rather than
    // thread a second parallel set of `&PirFunction` parameters through both of them --
    // and change a signature that eight arguments already strain -- each function is
    // lowered against a SYNTHESIZED single-function `PirModule` view: the same flat
    // shape, holding only this function's statements, schedule, quantities, accesses
    // and parameters.
    //
    // That is not a lossy copy. Every field those two read is per-function anyway, and
    // the view is built FROM `PirFunction` rather than filtered out of the flat module,
    // so it cannot accidentally inherit a name from a sibling function. The very bug
    // that made `quant_int8.naso` uncompilable cannot survive here, because the flat
    // `function_params` union is never consulted on this path.
    //
    // # Why the SCHEDULE emits the body, not the statement list
    //
    // The schedule, not the statement list, decides execution order and loop structure.
    // Walking `statements` directly cannot express a loop at all: a `forall` appears as
    // ONE statement whose domain is `nest(i)`, so walking statements ran its body
    // exactly once -- arithmetically wrong, with no diagnostic, while the band that
    // recorded the iteration bounds sat in the schedule tree unread.
    // `lower_schedule_tree_into` emits a real loop nest (preheader / header with a phi /
    // body / latch / exit) and honours the bands. It also owns the function BODY: the
    // `entry` block, the ABI guard, the single return and the verification.
    pub fn build_module(&mut self, pir_module: &PirModule) -> CodegenResult<()> {
        // Declare external functions
        for extern_fn in &pir_module.extern_functions {
            self.declare_extern_function(extern_fn)?;
        }

        // The functions to emit.
        //
        // A module with an EMPTY `functions` list is a hand-built one: a `.pir` fixture
        // parsed by `compiler/tests/codegen_tests.rs`, or a test that constructs a
        // `PirModule` literal. Those have no `PirFunction`s and never will, so they are
        // lowered as ONE synthetic function carrying the module's flat fields -- exactly
        // the behaviour that was the only behaviour before. That is what keeps the
        // `.pir` fixture tests passing with no change to their fixtures: a `[statements]`
        // fixture describes one function, and a synthetic function is what it is.
        let synthesized: Vec<crate::ir::pir_types::PirFunction>;
        let functions: &[crate::ir::pir_types::PirFunction] = if pir_module.functions.is_empty() {
            synthesized = vec![synthetic_single_function(pir_module)];
            &synthesized
        } else {
            &pir_module.functions
        };

        // Each function's declared parameter kinds, for call-site marshalling.
        //
        // Built once, from the SAME `functions` slice emission walks, so a call cannot
        // marshal against one function's declaration while another function is emitted.
        self.callee_params = Some(
            functions
                .iter()
                .map(|f| {
                    (
                        f.name.clone(),
                        f.params.iter().map(|p| p.kind.clone()).collect(),
                    )
                })
                .collect(),
        );

        // Callee before caller, and recursion refused. Checked BEFORE anything is
        // emitted, so a recursive module produces a diagnostic naming the cycle rather
        // than a module whose `call`s silently become an infinite chain at run time.
        let order = function_emission::emission_order(functions)?;
        let entry = function_emission::entry_function_index(functions);

        // Every function's SIGNATURE, built and declared before any body is emitted.
        //
        // Two reasons this is a separate pass rather than emission order:
        //
        //  * A call site must be able to resolve a callee. `emission_order` already
        //    guarantees callee-before-caller, but declaring first means the resolution
        //    does not DEPEND on that ordering being right -- a bug in the order
        //    produces a wrong order, not a dangling symbol.
        //  * `add_function` is what creates the `define`; appending basic blocks to the
        //    returned `FunctionValue` fills in the body. The signature and the body are
        //    therefore built from the SAME `PirFunction` and cannot disagree.
        let mut declared: Vec<(FunctionValue<'ctx>, Vec<EntryParam<'ctx>>)> =
            Vec::with_capacity(functions.len());
        for (i, func) in functions.iter().enumerate() {
            let view = function_view(func, pir_module);
            let abi = build_entry_signature(&mut self.type_lowering, &view)?;
            let param_types = entry_arg_types(&self.type_lowering, &abi)?;
            let ret_type = function_return_type(&mut self.type_lowering, func)?;
            let fn_type = self.type_lowering.fn_type(ret_type, &param_types, false);
            let symbol = function_emission::primary_symbol(i, functions);
            let function = self.module.add_function(&symbol, fn_type, None);
            // Name the arguments in the IR so the printed module says where each value
            // came from -- which argument is a caller's buffer, which is its element
            // count, and which is a trip count. Named by the recorded index rather than
            // by position, because a tensor's count is the argument AFTER its pointer.
            name_arguments(function, &abi);
            declared.push((function, abi));
        }

        for i in order {
            let func = &functions[i];
            let (function, abi) = &declared[i];
            let view = function_view(func, pir_module);
            // The value builder is the ONE scope for this function, cleared before and
            // after, so a local bound in one function cannot be read in the next. That
            // separation is the entire point of emitting them as separate symbols.
            self.emit_one_function(func, *function, abi, &view)?;
            self.current_function = None;
            self.current_block = None;

            // Every statement must be REACHABLE from this function's schedule tree, or it
            // will not be emitted. Checked AFTER lowering, because the schedule tree is
            // the only thing that emits a statement: walking the statement list as well
            // would emit it twice.
            //
            // Silently accepting an uncovered statement would compile it to nothing --
            // the same class of silent wrong answer as the loop-once body. A
            // `[0]`-quantity statement IS in the tree and is deliberately skipped by
            // `lower_domain`, which is erasure rather than a coverage gap.
            let scheduled: Vec<StmtId> = statements_under(&func.schedule.root);
            for stmt in &func.statements {
                // The RETURN statement -- explicit or implicit tail -- is deliberately
                // not in the tree: it is evaluated in the function's exit block, not as
                // a scheduled statement. It is covered by construction, so it is not
                // reported here. BOTH ids are skipped, because a function can return a
                // value through either and a tail-only return would otherwise be
                // reported as an unreachable statement.
                if func.return_stmt == Some(stmt.id) || func.tail_return_stmt == Some(stmt.id) {
                    continue;
                }
                if !scheduled.contains(&stmt.id) {
                    return Err(CodegenError::UnsupportedFeature(format!(
                        "statement {} of function `{}` is in `statements` but no \
                         `Domain` node in its schedule tree covers it, so the LLVM \
                         backend would emit nothing for it. Add a schedule node naming \
                         it, or drop the statement.",
                        stmt.id, func.name
                    )));
                }
            }
        }

        // The entry's second symbol, for external callers: existing C drivers and the
        // shipped-kernel execution tests call `naso_entry`.
        //
        // A thin `tail call` into the primary rather than a second `define` of the same
        // body, because two defines would emit the code twice. And not an LLVM `alias`,
        // because an alias requires the two symbols to have IDENTICAL function types and
        // this shim is the one place a signature is deliberately restated -- which is
        // exactly the kind of restatement that should be visible in the IR.
        if let Some(entry_index) = entry {
            let (target, abi) = &declared[entry_index];
            self.emit_entry_shim(*target, abi)?;
        }

        // Verify the module
        self.module
            .verify()
            .map_err(|e| CodegenError::VerificationError(e.to_string()))?;

        Ok(())
    }

    /// Emit ONE Naso function's body into its already-declared LLVM function.
    ///
    /// `function` comes from the declaration pass, so its signature is already fixed and
    /// a `call` in another function can already resolve it. Here only the body is
    /// appended: the `entry` block, the ABI guard, the schedule, and the single return.
    ///
    /// The ABI guard is emitted by `lower_schedule_tree_into` for EVERY function, not
    /// just the entry. That is the point: a function reachable only by call would
    /// otherwise be an unguarded hole, and a short buffer passed to it would read past
    /// the end of the caller's allocation with nothing to say so.
    fn emit_one_function(
        &mut self,
        func: &crate::ir::pir_types::PirFunction,
        function: FunctionValue<'ctx>,
        abi: &[EntryParam<'ctx>],
        view: &PirModule,
    ) -> CodegenResult<()> {
        self.set_current_function(function);
        // The RETURN VALUE, evaluated in the function's exit block.
        //
        // Read out of the function's own statements by one of TWO ids, both recorded
        // by lowering and both removed from the schedule. Neither can be a scheduled
        // statement: that would compute the value in the middle of the body and return
        // whatever the last statement happened to leave behind.
        //
        // # The two sources, and why they are not one
        //
        // `return_stmt` is an explicit `return e` in the source. `tail_return_stmt` is
        // the block's trailing expression in a function that declares a return type and
        // contains no `return` at all -- `fn f() -> f32 { 3.0 * 4.0 }`, whose value is
        // 12.0. They are separate PIR fields so neither can be mistaken for the other:
        // a real `return` and an implicit tail are different source programs, and a
        // diagnostic or a test that cannot tell them apart cannot check either.
        //
        // # Why this was not always the answer
        //
        // `tail_return_stmt` did not exist before, and `fn f() -> f32 { 3.0 * 4.0 }`
        // lowered its tail into PIR and DISCARDED it while this arm refused the
        // function. The refusal was right, and is still right, for `fn f(x: f32) -> f32
        // { let mut y = x * 3.0; }` -- but wrong for a body whose last thing is an
        // expression whose value the programmer plainly means to return.
        //
        // Returning zero instead, which is what this backend did before `64ebf59`, is
        // the one answer that can never be right: `f(5.0)` evaluated to `0.0` and the
        // caller had no way to distinguish a missing computation from a genuine zero.
        //
        // # When BOTH ids are absent, there is genuinely no value
        //
        // The remaining refusal is not the tail case. It is a declared return type with
        // no `return` AND no trailing expression -- a body ending in `let`, or in
        // nothing at all. The parser folds a trailing *expression* into `Block::expr`
        // but routes every `let` to `Block::stmts`, so such a body lowers a statement
        // for the binding and has no expression to return. `f(5.0)` must not quietly
        // yield `0.0`, and it must not yield `y` either.
        let (return_value, return_source) = match (
            func.return_stmt,
            func.tail_return_stmt,
            func.return_type.clone(),
        ) {
            (None, None, crate::ir::pir_types::FnReturn::Void) => (None, None),
            (Some(id), _, _) => (
                Some(return_expression(func, id)?),
                Some(ReturnSource::Explicit),
            ),
            (None, Some(id), _) => (
                Some(return_expression(func, id)?),
                Some(ReturnSource::ImplicitTail),
            ),
            (None, None, crate::ir::pir_types::FnReturn::Scalar(_)) => {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "function `{}` declares a return type but has no `return` statement \
                     and no trailing expression whose value could be returned, so there \
                     is no value to return. Returning zero would be silently wrong: the \
                     caller cannot distinguish it from a computed zero. Add an explicit \
                     `return`, or end the body with the expression to return, or drop \
                     the return type to make the function `void`.",
                    func.name
                )));
            }
            // `FnReturn::Unsupported` is REFUSED just below, with a message naming the
            // type. Reaching this arm means the declared return type has no slot in
            // this ABI, so there is nothing to return in that slot either; the refusal
            // below is the one that says why.
            (None, None, _) => (None, None),
        };
        // `FnReturn::Unsupported` is checked here rather than in the declaration pass so
        // the refusal names the function whose return type could not be lowered.
        if let crate::ir::pir_types::FnReturn::Unsupported(ty) = &func.return_type {
            return Err(CodegenError::UnsupportedFeature(format!(
                "function `{}` returns `{ty}`, which has no slot in this ABI. Nothing \
                 is substituted for it: an invented return type would let a caller bind \
                 a value the function never produces.",
                func.name
            )));
        }

        // `self.module` and `self.value_builder` are borrowed separately because the
        // callee needs the module immutably and the value builder mutably, and both are
        // fields of the same `&mut self`.
        let module = &self.module;
        let value_builder = self
            .value_builder
            .as_mut()
            .expect("value_builder not initialized");
        lower_schedule_tree_into(
            self.context,
            module,
            function,
            value_builder,
            &view.schedule,
            view,
            &view.quantities,
            &view.accesses,
            abi,
            return_value,
            return_source,
            self.callee_params.as_ref(),
        )?;
        Ok(())
    }

    /// Emit `naso_entry` as a `tail call` into the entry function's primary symbol.
    ///
    /// The shim exists so an EXTERNAL caller has a stable name to link against while
    /// the Naso-to-Naso call sites use the `naso_<name>` convention. It is a call and
    /// not a jump so the target's own ABI guard still runs: `tail call` is not a jump
    /// that bypasses the callee's prologue, and every argument is forwarded unchanged,
    /// so the guard sees exactly the lengths the external caller supplied.
    fn emit_entry_shim(
        &mut self,
        target: FunctionValue<'ctx>,
        abi: &[EntryParam<'ctx>],
    ) -> CodegenResult<()> {
        // The shim's type IS the target's type, reused verbatim rather than rebuilt
        // from the ABI. A rebuilt signature is a second place the entry ABI is spelled,
        // and the two would be free to disagree about how many arguments a tensor
        // occupies.
        let fn_type = target.get_type();
        let param_count = fn_type.count_param_types();
        let shim = self
            .module
            .add_function(function_emission::ENTRY_SYMBOL, fn_type, None);
        name_arguments(shim, abi);
        let entry = self
            .context
            .llvm_context()
            .append_basic_block(shim, "entry");
        let builder = self.context.llvm_context().create_builder();
        builder.position_at_end(entry);
        let args: Vec<BasicMetadataValueEnum<'ctx>> = (0..param_count)
            .filter_map(|i| shim.get_nth_param(i))
            .map(BasicMetadataValueEnum::from)
            .collect();
        let call = builder
            .build_call(target, &args, "entry")
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        // `musttail` is NOT used: it requires the caller's signature to be IDENTICAL to
        // the callee's AND forbids intervening instructions. Both hold today, but a
        // `musttail` that stops holding is a hard verifier error, and an ordinary
        // `tail`-position call is correct in every case with one extra frame.
        // `.basic()` yields `None` for a void call and the value otherwise, which is
        // exactly the `Option` `build_return` takes. Boxed because `build_return` wants
        // `&dyn BasicValue` and `BasicValueEnum` is unsized-coercible into it.
        let returned = call
            .try_as_basic_value()
            .basic()
            .map(|v| Box::new(v) as Box<dyn inkwell::values::BasicValue<'ctx>>);
        builder
            .build_return(returned.as_deref())
            .map_err(|e| CodegenError::InstructionError(e.to_string()))?;
        if !shim.verify(true) {
            return Err(CodegenError::VerificationError(format!(
                "entry shim `{}` failed LLVM verification:\n{}",
                function_emission::ENTRY_SYMBOL,
                shim.print_to_string()
            )));
        }
        Ok(())
    }

    // Declare an external function
    fn declare_extern_function(
        &mut self,
        extern_fn: &crate::ir::pir_types::ExternFunction,
    ) -> CodegenResult<()> {
        let param_types: CodegenResult<Vec<BasicTypeEnum<'ctx>>> = extern_fn
            .params
            .iter()
            .map(|p| {
                // Extern params only carry a source-level type name plus their
                // quantity; the name is not resolvable here, so the declared
                // param type is the quantity-lowered `int` placeholder.
                let ty = Type::new(TypeKind::Int, p.quantity, Span::default());
                self.type_lowering
                    .lower_quantity_aware(&crate::codegen::abi::lower_pir_type(
                        &ty,
                        &HashMap::new(),
                    )?)
            })
            .collect();

        let param_types = param_types?;

        // `ExternFunction::return_type` is a source-level type name which cannot
        // be lowered to an LLVM type at declaration time, so the declaration
        // uses the void return signature (`None` == void for `fn_type`).
        let ret_type: Option<BasicTypeEnum<'ctx>> = None;

        let fn_type = self.type_lowering.fn_type(ret_type, &param_types, false);
        self.module.add_function(&extern_fn.name, fn_type, None);
        Ok(())
    }

    // Convert module to LLVM IR string
    pub fn module_to_string(&self) -> String {
        self.module.print_to_string().to_string()
    }

    // Write module to .ll file
    pub fn write_ll_file(&self, path: &std::path::Path) -> CodegenResult<()> {
        self.module
            .print_to_file(path)
            .map_err(|e| CodegenError::EmissionError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};

    #[test]
    fn test_module_builder_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let builder = LLVMModuleBuilder::new(&context);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_empty_module_emission() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let mut builder = LLVMModuleBuilder::new(&context).unwrap();

        // Create a minimal PIR module
        use crate::ast::{Mutability, Quantity};
        use crate::ir::access_relation::AccessRelations;
        use crate::ir::affine_domain::AffineDomain;
        use crate::ir::pir_types::{PirModule, PirStatement};
        use crate::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};
        use std::collections::HashMap;

        let domain = AffineDomain::universe(0, 0);
        let schedule = ScheduleTree::new(ScheduleNode::domain(StmtId(0), domain.clone()), vec![]);
        let accesses = AccessRelations::new();
        let quantities = HashMap::new();

        let stmt = PirStatement {
            id: StmtId(0),
            domain,
            body: crate::ir::pir_types::PirExpr::IntLit(42),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);

        let result = builder.build_module(&module);
        assert!(result.is_ok());

        let ir = builder.module_to_string();
        assert!(ir.contains("define"));
    }
}
