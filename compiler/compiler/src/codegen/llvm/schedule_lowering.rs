//! Schedule Tree Lowering to LLVM Control Flow
//!
//! This module lowers PIR ScheduleTree nodes to LLVM basic blocks with
//! proper induction variables, phi nodes, and control flow structure.
//!
//! Key features:
//! - Band nodes -> LLVM loops with affine bounds
//! - Filter nodes -> conditional branches with predicate computation
//! - Sequence/Context nodes -> block chaining and scoping
//! - QTT quantity constraints: [0]-erased loops stripped entirely
//! - AccessRelation -> LLVM GEP with alias.scope metadata

use crate::codegen::context::CodegenContext;
use crate::codegen::error::{CodegenError, CodegenResult};
use crate::codegen::llvm::module_builder::{EntryParam, ReturnSource, build_entry_signature};
use crate::codegen::llvm::value_builder::TensorBinding;
use crate::codegen::llvm::{
    access_emission::AccessEmitter,
    expr_lowering::PirExprLowerer,
    loop_emission::{LoopBounds as SequentialLoopBounds, LoopEmitter},
    parallel::{LoopBounds as ParallelLoopBounds, ParallelEmitter},
    polyhedral_opts::PolyhedralOptimizer,
    type_lowering::LlvmTypeLowering,
    value_builder::LlvmValueBuilder,
};
use crate::ir::pir_types::ElemType;
use crate::ir::pir_types::ParamKind;
use crate::ir::{
    access_relation::AccessRelations,
    affine_domain::AffineDomain,
    affine_map::AffineMap,
    pir_types::{PirExpr, PirModule, PirStatement, QuantityMap},
    schedule_tree::{ScheduleNode, ScheduleTree, StmtId},
};
use inkwell::basic_block::BasicBlock;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{AnyValue, BasicValueEnum, FunctionValue};
use std::collections::HashMap;

/// Main entry point for lowering a ScheduleTree to LLVM IR
///
/// `module` is passed in rather than recovered from `function`. inkwell 0.10 exposes
/// no safe "which module does this function belong to" accessor, and the only way to
/// reach the `Module` from a `FunctionValue` is `unsafe { Module::new(raw) }` --
/// which installs a `Drop` that calls `LLVMDisposeModule`, disposing a module the
/// caller still owns. The body lowering needs the module to resolve a callee and to
/// declare a quantum intrinsic, so it is threaded through explicitly.
pub fn lower_schedule_tree<'ctx>(
    ctx: &'ctx CodegenContext,
    module: &inkwell::module::Module<'ctx>,
    function: FunctionValue<'ctx>,
    schedule: &ScheduleTree,
    pir_module: &PirModule,
    quantities: &QuantityMap,
    access_relations: &AccessRelations,
) -> CodegenResult<()> {
    let llvm_context = ctx.llvm_context();
    let builder = llvm_context.create_builder();
    // `LlvmValueLowering` is not `Clone` (it owns a named-struct cache), so the
    // value builder takes sole ownership of it here.
    let type_lowering = LlvmTypeLowering::new(llvm_context);
    let mut value_builder = LlvmValueBuilder::new(builder, type_lowering);

    let abi = build_entry_signature(&mut LlvmTypeLowering::new(llvm_context), pir_module)?;
    lower_schedule_tree_into(
        ctx,
        module,
        function,
        &mut value_builder,
        schedule,
        pir_module,
        quantities,
        access_relations,
        abi.as_slice(),
        None,
        None,
        None,
    )
}

/// Lower a schedule tree into `function` through a CALLER-SUPPLIED value builder.
///
/// This is the same lowering [`lower_schedule_tree`] performs, split so the
/// caller keeps ownership of the single `LlvmValueBuilder`. The reason is scope:
/// `LlvmValueBuilder` owns the `name -> allocation` map, and a `let` bound before
/// a `forall` must still be writable inside the loop body. A caller that built its
/// own `LlvmValueBuilder` (as `lower_schedule_tree` does) and passed in a
/// different one would silently lose every binding made before the call, so
/// there is exactly ONE builder and it is threaded through.
///
/// `function` must have NO basic blocks yet: this function appends the `entry`
/// block, emits the schedule, appends exactly one return, and verifies. A caller
/// that has already added blocks or its own terminator will produce a
/// double-terminated or unreachable-IR module, so ownership of the body is
/// deliberately not split.
// The parameter list mirrors `lower_schedule_tree` plus the value builder, and every
// one of them is load-bearing: `ctx` for the LLVM context, `module` for callee and
// intrinsic resolution, `function` for the block being built, the value builder for
// the single insertion point and variable scope, and the four PIR arguments because
// `PirModule` is borrowed rather than decomposed. Bundling them into a struct would
// only move the same fields one level down without removing any of them.
#[allow(clippy::too_many_arguments)]
pub fn lower_schedule_tree_into<'ctx>(
    ctx: &'ctx CodegenContext,
    module: &inkwell::module::Module<'ctx>,
    function: FunctionValue<'ctx>,
    value_builder: &mut LlvmValueBuilder<'ctx>,
    schedule: &ScheduleTree,
    pir_module: &PirModule,
    quantities: &QuantityMap,
    access_relations: &AccessRelations,
    abi: &[EntryParam<'ctx>],
    return_value: Option<&PirExpr>,
    return_source: Option<ReturnSource>,
    callee_params: Option<&HashMap<String, Vec<ParamKind>>>,
) -> CodegenResult<()> {
    let llvm_context = ctx.llvm_context();

    // A schedule tree has no `let` step of its own, so a band body's only nameable
    // storage is what already exists in the module. Bind the module's globals by name
    // before lowering, so `counter = counter + 1` has somewhere to write. A name that
    // is not a global stays unbound and an assignment to it is still refused.
    value_builder.register_module_globals(module)?;

    // Create entry block
    let entry = llvm_context.append_basic_block(function, "entry");
    value_builder.builder().position_at_end(entry);

    // Bind the function's arguments to their names BEFORE the schedule is lowered,
    // because a symbolic loop bound and every tensor subscript read them.
    //
    // The list comes from `build_entry_signature`, the SAME function that built the
    // signature in `LLVMModuleBuilder::build_module`, so argument `j` is exactly the
    // `j`-th entry there. Deriving the two independently is how they drift apart and
    // produce a function whose first argument is silently read as the second.
    //
    // A tensor binds to the argument POINTER ITSELF, with no alloca and no store: the
    // pointer is the caller's buffer. Allocating a local and storing the pointer into it
    // would still work for reading, but it re-introduces the "tensor is a slot holding a
    // value" model that makes `input[i]` ambiguous between a load of the pointer and a
    // load of the element.
    // The arguments are bound from the ABI the CALLER built for this function.
    //
    // Previously this rebuilt the ABI from `pir_module.function_params`, which was
    // correct when there was exactly one function per module and is wrong now: that
    // field is the program-wide UNION of every function's parameters, so rebuilding
    // from it would bind argument `j` to whichever function declared that name first.
    // For `kernels/quant_int8.naso` that means binding `input` -- declared
    // `Tensor[f32, 1024]` by the first function -- to a function whose `input` is
    // `Tensor[i8, 1024]`: a load of the wrong width, with nothing in the IR to say so.
    // The ABI is now passed in, so the signature and the binding cannot disagree.
    for entry in abi.iter() {
        let Some(arg) = function.get_nth_param(entry.arg_index) else {
            // Fewer arguments than the ABI names. A hand-built module may legitimately
            // do this; a body that actually reads the missing name is refused by the
            // `Var` arm rather than given a substitute value.
            break;
        };
        match entry.kind {
            ParamKind::Tensor { elem, ref shape } => {
                let elem_ty = scalar_slot_type(value_builder, &elem);
                // The length argument, recorded on the binding so a CALL can forward
                // the caller's real element count to the callee. Forwarding the count
                // the caller was given -- rather than one recomputed from the shape --
                // is what makes the callee's own guard a real check on the caller's
                // buffer instead of a comparison of two invented numbers.
                let len = entry
                    .len_arg_index
                    .and_then(|i| function.get_nth_param(i))
                    .map(|v| v.into_int_value());
                value_builder.bind_tensor(
                    &entry.name,
                    TensorBinding {
                        base: arg.into_pointer_value(),
                        elem: elem_ty,
                        shape: shape.clone(),
                        len,
                        // Declared sub-byte element => packed storage. Recorded here,
                        // where the `ElemType` is still known; downstream the i8 storage
                        // makes an i8 tensor and a packed i4 tensor look alike.
                        sub_byte: elem == ElemType::I4,
                    },
                );
            }
            // A quantum register binds as a pointer like a tensor, but with no element
            // type, because nothing here indexes one. A read stays a diagnostic.
            ParamKind::QRegister => {
                value_builder.bind_tensor(
                    &entry.name,
                    TensorBinding {
                        base: arg.into_pointer_value(),
                        // The element type is recorded but never used for indexing: a
                        // `QRegister` subscript has no lowering and reports that. Using
                        // `i8` is a placeholder that must never be mistaken for a
                        // working one.
                        elem: value_builder
                            .type_lowering()
                            .int_type(crate::codegen::abi::IntWidth::I8)
                            .into(),
                        shape: None,
                        // A `QRegister` occupies ONE argument, not the `(ptr, len)`
                        // pair a tensor does, so there is no length to forward.
                        len: None,
                        sub_byte: false,
                    },
                );
            }
            // A scalar is stored into an alloca so the body reads it through the same
            // name-resolution path as every other binding; a loop bound lowers as a
            // load of exactly this slot.
            // `Unsupported` has no slot to bind to. `build_entry_signature` refuses
            // that kind before this loop runs, so reaching it would mean the signature
            // and this loop disagree; it is reported rather than given a substitute
            // value, which is exactly the substitution this work refuses to make.
            ParamKind::Unsupported(ref reason) => {
                return Err(CodegenError::UnsupportedFeature(format!(
                    "parameter `{}` has no bindable ABI slot: {reason}",
                    entry.name
                )));
            }
            ParamKind::Scalar(_) => {
                value_builder.bind_argument(&entry.name, arg)?;
            }
        }
    }

    // The ABI BOUNDARY CHECK, emitted after the arguments are bound and before any
    // tensor element is loaded or stored.
    //
    // It sits here because this is the last point that is provably before every read:
    // binding a tensor is a name lookup and binding a scalar is an alloca plus a store
    // of the scalar's OWN value, so neither touches a caller's buffer. Everything the
    // schedule lowering emits afterwards may.
    //
    // The builder is left positioned in the guard's last continuation block, which is
    // open and unterminated, so `ScheduleLowering` appends the body into it. A module
    // with no tensor parameter emits nothing here and the builder stays in `entry`, so
    // a scalar-only kernel's block structure is unchanged.
    crate::codegen::llvm::abi_guard::emit_abi_guard(
        llvm_context,
        module,
        function,
        value_builder.builder(),
        abi,
    )?;

    // Create schedule lowering context
    let mut lowering = ScheduleLowering::new(
        value_builder,
        module,
        function,
        pir_module,
        quantities,
        access_relations,
        callee_params,
    )?;

    // Lower the root schedule node
    lowering.lower_node(&schedule.root)?;

    // Build return. inkwell 0.10's `build_return` takes
    // `Option<&dyn BasicValue>`; a void function returns `None`, while a typed
    // function returns the value the function computes.
    //
    // This is the ONE return for the function. A band emits its own `ret`-free
    // exit block and leaves the builder positioned there, so appending here
    // terminates the exit rather than an already-terminated block.
    //
    // # The RETURN VALUE, not a zero
    //
    // This used to return `build_zero(return_type)`. That is correct ONLY for a
    // function whose return value is never read, and it is a silent wrong answer for
    // every other one: a caller would receive 0 for `fn f(x) { x + 1 }` and nothing in
    // the IR would say the value was invented. So `return_value` -- the expression the
    // function's `return` statement OR its trailing expression lowered to, evaluated
    // HERE, in the exit block -- is what is returned. Which of the two it was is
    // `return_source`, and nothing downstream may tell them apart by value alone.
    //
    // # A function with a return TYPE and no value at all
    //
    // Unreachable from `emit_one_function`, which REFUSES a value-returning function
    // whose `return_stmt` and `tail_return_stmt` are both `None` before calling here.
    // That refusal is the fix the comment above used to call for: the zero fallback
    // below used to be the ONLY answer such a function got, and it was the one answer
    // a caller cannot distinguish from a computed zero.
    //
    // The arm survives because this function is `pub` and is also called by
    // `lower_schedule_tree` -- the flat single-function entry point that has no
    // `PirFunction` and therefore no return bookkeeping at all. Rather than delete the
    // arm and leave that caller to emit an unterminated function, the fallback stays
    // and says what it is. It is a compatibility path, NOT a claim that a Naso
    // function may return zero.
    let return_type = function.get_type().get_return_type();
    let returned = match (return_type, return_value) {
        (None, _) => None,
        (Some(ty), Some(expr)) => {
            let mut lowerer = PirExprLowerer {
                value_builder: lowering.value_builder,
                module,
                current_function: function,
                callee_params: lowering.callee_params,
                in_statement_position: false,
                // Empty: no loop is being lowered at the point this builder is created.
                // The loop arms push and pop as they go.
                loop_stack: Vec::new(),
            };
            let value = lowerer.build_expr(expr, quantities)?;
            // A type mismatch here is REFUSED rather than converted: the return type
            // came from the function's declaration and the value from its body, and if
            // they disagree the source does not say what the function returns. Casting
            // one to the other would invent an answer.
            //
            // `return_source` names WHICH source construct produced this expression, so
            // the diagnostic says "trailing expression" for `fn f() -> f32 { 3.0 * 4.0 }`
            // and "`return` expression" for `fn f() -> f32 { return 3.0 * 4.0; }`. The
            // two are different programs and a reader chasing the bug needs to know
            // which one they are looking at.
            if value.get_type() != ty {
                let source = return_source.map_or("return", ReturnSource::describe);
                return Err(CodegenError::UnsupportedFeature(format!(
                    "function `{}` declares it returns `{}` but its {source} evaluates \
                     to `{}`. Nothing is converted between them: a cast here would be \
                     a value the source never wrote.",
                    function.get_name().to_string_lossy(),
                    ty,
                    value.get_type(),
                )));
            }
            Some(value)
        }
        // No value was supplied at all. Reached only through the flat
        // `lower_schedule_tree` entry point -- see the comment above. A `PirFunction`
        // path cannot get here: `emit_one_function` refuses first.
        (Some(ty), None) => Some(lowering.value_builder.build_zero(ty)),
    };
    lowering.value_builder.build_return(returned)?;

    // Verify function. inkwell 0.10's `verify` returns a `bool` rather than a
    // `Result`, printing diagnostics to stderr when `print` is true; on failure
    // surface the offending function body as the error message.
    if !function.verify(true) {
        return Err(CodegenError::VerificationError(format!(
            "scheduled function `{}` failed LLVM verification:\n{}",
            function.get_name().to_string_lossy(),
            function.print_to_string()
        )));
    }

    Ok(())
}

/// The LLVM type a scalar element type occupies in a tensor buffer.
///
/// One function, because the signature builder and the argument binder must agree: a
/// tensor whose slot says `f64` but whose binding loads `f32` is a load of the wrong
/// width, and nothing about the emitted IR makes that obvious.
fn scalar_slot_type<'ctx>(
    value_builder: &LlvmValueBuilder<'ctx>,
    elem: &crate::ir::pir_types::ElemType,
) -> BasicTypeEnum<'ctx> {
    use crate::codegen::abi::{FloatWidth, IntWidth};
    use crate::ir::pir_types::ElemType;
    let tl = value_builder.type_lowering();
    match elem {
        ElemType::F64 => tl.float_type(FloatWidth::F64).into(),
        // SUB-BYTE storage: an `i4` element occupies half a byte, but the byte is the
        // addressable unit, so the SLOT is `i8`. Two elements share it and
        // `TensorBinding::sub_byte` says so; without that flag this tensor and an
        // `i8` one would be indistinguishable, which is the ambiguity a widening bug
        // hides in.
        ElemType::I4 => tl.int_type(IntWidth::I8).into(),
        ElemType::I8 => tl.int_type(IntWidth::I8).into(),
        ElemType::I16 => tl.int_type(IntWidth::I16).into(),
        ElemType::I32 => tl.int_type(IntWidth::I32).into(),
        ElemType::I64 => tl.int_type(IntWidth::I64).into(),
        ElemType::Bool => tl.int_type(IntWidth::I1).into(),
    }
}

/// Every `Domain` statement id in a schedule subtree, in tree order.
pub fn statements_under(node: &ScheduleNode) -> Vec<StmtId> {
    let mut ids = Vec::new();
    collect_statements(node, &mut ids);
    ids
}

/// The symbolic constants the schedule's domains name, in a fixed order.
///
/// This is the entry function's parameter list. It is derived from the SCHEDULE
/// rather than from `PirModule::parameters` because the schedule is what actually
/// needs a value: a `forall i in 0..n` encodes `n` as a named dimension of its
/// band's domain, and that domain is the only place the need is recorded.
///
/// Names come out in tree order, first appearance first occurrence, deduplicated.
/// The order is the ABI: `lower_schedule_tree_into` binds argument `j` to element
/// `j` of this list, so both sides must compute it the same way -- hence one
/// function rather than two traversals.
pub fn schedule_parameters(schedule: &ScheduleTree) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut domains = Vec::new();
    collect_domains(&schedule.root, &mut domains);
    for domain in domains {
        for name in &domain.parameter_names {
            if !name.is_empty() && !out.iter().any(|q| q == name) {
                out.push(name.clone());
            }
        }
    }
    out
}

fn collect_domains(node: &ScheduleNode, out: &mut Vec<AffineDomain>) {
    match node {
        ScheduleNode::Domain { domain, .. } => out.push(domain.clone()),
        ScheduleNode::Band { child, .. }
        | ScheduleNode::Filter { child, .. }
        | ScheduleNode::Context { child, .. }
        | ScheduleNode::Extension { child, .. } => collect_domains(child, out),
        ScheduleNode::Sequence { children } => {
            for child in children {
                collect_domains(child, out);
            }
        }
        ScheduleNode::Empty => {}
    }
}

fn collect_statements(node: &ScheduleNode, out: &mut Vec<StmtId>) {
    match node {
        ScheduleNode::Domain { stmt_id, .. } => out.push(*stmt_id),
        ScheduleNode::Band { child, .. }
        | ScheduleNode::Filter { child, .. }
        | ScheduleNode::Context { child, .. }
        | ScheduleNode::Extension { child, .. } => collect_statements(child, out),
        ScheduleNode::Sequence { children } => {
            for child in children {
                collect_statements(child, out);
            }
        }
        ScheduleNode::Empty => {}
    }
}

/// Schedule lowering context
pub struct ScheduleLowering<'ctx, 'a> {
    value_builder: &'a mut LlvmValueBuilder<'ctx>,
    /// The module under construction, needed by the body lowering to resolve a callee
    /// and to declare a quantum intrinsic on first use.
    module: &'a inkwell::module::Module<'ctx>,
    function: FunctionValue<'ctx>,
    pir_module: &'a PirModule,
    quantities: &'a QuantityMap,
    access_relations: &'a AccessRelations,
    /// Each Naso function's declared parameter kinds, so a `Call` inside a loop body
    /// marshals its arguments the same way one at statement level does.
    ///
    /// Taken from the whole program rather than from this function alone: a loop body
    /// can call any function in the module, not only a sibling. `None` for a
    /// hand-built module with no `functions`, where every callee is an `extern`.
    callee_params: Option<&'a HashMap<String, Vec<ParamKind>>>,

    // Sub-emitters
    loop_emitter: LoopEmitter<'ctx>,
    access_emitter: AccessEmitter<'ctx>,
    optimizer: PolyhedralOptimizer,
    parallel_emitter: ParallelEmitter<'ctx>,
    /// How many loop bands are currently being emitted around this statement.
    ///
    /// This is what decides whether a statement's body is inside an affine loop.
    /// It cannot be inferred from "is there a band above me", because EVERY statement
    /// sits under a Domain node, including a function's top-level statements -- so a
    /// top-level `break` was reported as "`break` inside an affine `forall` loop",
    /// which is worse than the original wrong message: the program has no loop at all
    /// and now claims to have one.
    band_depth: usize,
}

impl<'ctx, 'a> ScheduleLowering<'ctx, 'a> {
    pub fn new(
        value_builder: &'a mut LlvmValueBuilder<'ctx>,
        module: &'a inkwell::module::Module<'ctx>,
        function: FunctionValue<'ctx>,
        pir_module: &'a PirModule,
        quantities: &'a QuantityMap,
        access_relations: &'a AccessRelations,
        callee_params: Option<&'a HashMap<String, Vec<ParamKind>>>,
    ) -> CodegenResult<Self> {
        let loop_emitter = LoopEmitter::new(value_builder.type_lowering().context())?;
        let access_emitter = AccessEmitter::new(value_builder.type_lowering().context())?;
        let optimizer = PolyhedralOptimizer::new();
        let parallel_emitter = ParallelEmitter::new(value_builder.type_lowering().context())?;

        Ok(Self {
            value_builder,
            module,
            function,
            pir_module,
            quantities,
            access_relations,
            callee_params,
            loop_emitter,
            access_emitter,
            optimizer,
            parallel_emitter,
            // No band is being emitted for a statement that has not been reached
            // through `lower_band`.
            band_depth: 0,
        })
    }

    fn set_current_block(&mut self, block: BasicBlock<'ctx>) {
        self.value_builder.builder().position_at_end(block);
    }

    /// Lower a schedule node recursively
    pub fn lower_node(&mut self, node: &ScheduleNode) -> CodegenResult<()> {
        match node {
            ScheduleNode::Band {
                members,
                coincident,
                iterators,
                child,
            } => self.lower_band(members, coincident, iterators, child),
            ScheduleNode::Filter { domain, child } => self.lower_filter(domain, child),
            ScheduleNode::Sequence { children } => self.lower_sequence(children),
            ScheduleNode::Context { domain, child } => self.lower_context(domain, child),
            ScheduleNode::Domain { stmt_id, domain } => self.lower_domain(*stmt_id, domain),
            ScheduleNode::Extension { sizes, child } => self.lower_extension(sizes, child),
            ScheduleNode::Empty => Ok(()),
        }
    }

    /// Lower a band node (affine loop nest)
    fn lower_band(
        &mut self,
        members: &[AffineMap],
        coincident: &[bool],
        iterators: &[String],
        child: &ScheduleNode,
    ) -> CodegenResult<()> {
        // Check if this band should be erased ([0] quantity)
        if self.is_band_erased(child) {
            // [0]-quantity band: skip code generation entirely
            return self.lower_node(child);
        }

        // Extract loop bounds from scheduling maps
        let bounds = self.extract_bounds(members, iterators)?;

        // A band whose domain pins no iterator to a constant range has NO bounds,
        // and `emit_sequential_band` emits one loop PER bound -- so with no bounds it
        // emits no loop and the callback that lowers the child is never invoked. The
        // band body would then vanish from the module entirely: the statements are in
        // the schedule tree, so the coverage check passes, and the program compiles to
        // something that simply does less than it was told to.
        //
        // That is the same silent wrong answer as running a loop body once, so it is
        // refused here instead. The common cause is a symbolic bound:
        // `forall i in 0..n` records only `i >= 0`, and `iterator_bounds` needs both
        // ends of the range. Lowering a symbolic bound needs the parameter to reach
        // codegen as a value, which the IR does not yet carry.
        if bounds.is_empty() && !self.is_band_erased(child) {
            return Err(CodegenError::UnsupportedFeature(format!(
                "cannot lower a band with no constant loop bounds: its domain {:?} \
                 constrains no iterator to a range with both a lower and an upper \
                 bound, so no trip count can be computed. A symbolic bound such as \
                 `forall i in 0..n` is the usual cause -- it records only `i >= 0`. \
                 The band's body is NOT emitted as straight-line code, because \
                 running it once is a different program.",
                members
                    .first()
                    .map(|m| m.pieces[0].domain.name.clone().unwrap_or_default())
                    .unwrap_or_else(|| "<unnamed>".to_string())
            )));
        }

        // Is the band parallel?
        //
        // `coincident` is a PER-LEVEL flag, and the parallel emitter here
        // parallelises the WHOLE band: it attaches `llvm.loop.parallel_accesses` /
        // `omp parallel for` to every loop it emits. So a band may only take the
        // parallel path when EVERY level it would parallelise is genuinely parallel.
        //
        // This used to be `coincident.iter().any(|&c| c)`. That is wrong in the
        // unsound direction: ONE true flag -- say the inner level of a 2-D band --
        // parallelised the outer level too, which the dependence analysis never
        // cleared. `.all()` is the conjunction the emitter's behaviour actually
        // requires. A band with mixed flags falls to the sequential emitter, which
        // is slower and correct; `coincident` mixing true and false is honest about
        // the band and is not something this emitter can yet express.
        let is_parallel = !coincident.is_empty() && coincident.iter().all(|&c| c);

        // The emitters hand the child-lowering callback a *mock* lowering that
        // only exposes the `LlvmValueBuilder` they positioned inside the loop
        // body. To lower the real child we rebuild a `ScheduleLowering` over
        // that builder; the immutable pieces (function, PIR module, quantities,
        // access relations) are `Copy` handles into `self`, so no borrow of
        // `self` is captured by the closure.
        let module = self.module;
        let function = self.function;
        let pir_module = self.pir_module;
        let quantities = self.quantities;
        let access_relations = self.access_relations;
        let callee_params = self.callee_params;

        // Emit the loop nest
        if is_parallel {
            let par_bounds: Vec<ParallelLoopBounds<'ctx>> = bounds
                .iter()
                .map(|b| ParallelLoopBounds {
                    iterator_dim: b.iterator_dim,
                    lower: b.lower,
                    upper: b.upper,
                    step: b.step,
                    iterator_name: b.iterator_name.clone(),
                })
                .collect();
            self.parallel_emitter.emit_parallel_band(
                self.value_builder,
                &par_bounds,
                members,
                child,
                |lowering| {
                    let mut inner = ScheduleLowering::new(
                        lowering.value_builder(),
                        module,
                        function,
                        pir_module,
                        quantities,
                        access_relations,
                        callee_params,
                    )?;
                    // Statements lowered from here ARE inside the loop nest this band
                    // emits, so their bodies carry the affine frame. Without this,
                    // `emit_statement_body` cannot tell a band body from a top-level
                    // statement -- both sit under a Domain node.
                    inner.band_depth = self.band_depth + 1;
                    inner.lower_node(child)
                },
            )?;
        } else {
            let seq_bounds: Vec<SequentialLoopBounds<'ctx>> = bounds
                .iter()
                .map(|b| SequentialLoopBounds {
                    iterator_dim: b.iterator_dim,
                    lower: b.lower,
                    upper: b.upper,
                    step: b.step,
                    iterator_name: b.iterator_name.clone(),
                })
                .collect();
            self.loop_emitter.emit_sequential_band(
                self.value_builder,
                &seq_bounds,
                members,
                child,
                |lowering| {
                    let mut inner = ScheduleLowering::new(
                        lowering.value_builder(),
                        module,
                        function,
                        pir_module,
                        quantities,
                        access_relations,
                        callee_params,
                    )?;
                    // Statements lowered from here ARE inside the loop nest this band
                    // emits, so their bodies carry the affine frame. Without this,
                    // `emit_statement_body` cannot tell a band body from a top-level
                    // statement -- both sit under a Domain node.
                    inner.band_depth = self.band_depth + 1;
                    inner.lower_node(child)
                },
            )?;
        }

        Ok(())
    }

    /// Check if a band is [0]-quantity (erased)
    ///
    /// KNOWN BUG (pre-existing, not fixed here): this is **not** scoped to the
    /// band. `QuantityMap` is a flat `name -> Quantity` map with no link back to
    /// statements or schedule-tree nodes, so a single `[0]`-quantity variable
    /// *anywhere in the module* makes this return `true` for *every* band --
    /// silently deleting all loops in the module. Correct behaviour requires
    /// per-statement quantities (available as `PirStatement::quantity`) plus a
    /// way to enumerate the statements covered by `members`; neither exists on
    /// `AffineMap` today. Left as-is pending that IR support.
    /// Whether THIS band should be erased because its statements are `[0]`.
    ///
    /// This used to ignore its argument and test `self.quantities` as a whole: if ANY
    /// binding anywhere in the program had `[0]` quantity, EVERY band was erased and
    /// every loop became straight-line code. That is a silent wrong answer -- the loop
    /// simply ran once -- rather than a diagnostic, and it applied to bands that had
    /// nothing to do with the `[0]` statement.
    ///
    /// Now scoped to the statements actually in this band: a band is erased only when
    /// every statement it covers is `[0]`. A band containing a `[0]` statement alongside
    /// a `[1]` one is NOT erased, because dropping it would discard the `[1]` work.
    fn is_band_erased(&self, child: &ScheduleNode) -> bool {
        // A band with no statements at all has nothing to run; treating it as erased
        // avoids emitting a loop whose body is empty.
        let stmt_ids = statements_under(child);
        if stmt_ids.is_empty() {
            return true;
        }
        // A statement's quantity is the authority. The quantity MAP is keyed by variable
        // name, not statement, so it cannot answer this question: two statements can
        // touch the same name with different quantities, and a `[0]` binding in one
        // statement says nothing about another.
        stmt_ids.iter().all(|id| {
            self.pir_module
                .statements
                .iter()
                .find(|s| s.id == *id)
                .is_some_and(|s| matches!(s.quantity, crate::ast::Quantity::Zero))
        })
    }

    /// Extract loop bounds from affine scheduling maps
    //
    // This can return FEWER bounds than the band has dimensions, or none at all,
    // when the band's domain does not pin an iterator to a range it can express:
    // an unrepresentable bound such as `n + 1` encodes no constraint at all.
    // `lower_band` treats an empty result as an error rather than emitting a body
    // once, because emitting the body once is a silent wrong answer -- precisely the
    // bug this schedule path exists to fix.
    //
    // A SYMBOLIC bound (`0..n`) is no longer in that category: the domain encodes
    // it as `-i + n >= 1` over a named parameter dimension, so the bound comes back
    // as an `AffineExpr` with a coefficient on `n` and `lower_affine_expr` turns it
    // into a real LLVM value.
    //
    // `iterators` is the band's own `ScheduleNode::Band::iterators`: the source
    // spelling of each level's induction variable, one entry per level, outermost
    // first. It used to be recovered by parsing `AffineDomain::name` for a
    // `nest(i)` prefix, which coupled a debug label to program meaning.
    fn extract_bounds(
        &mut self,
        members: &[AffineMap],
        iterators: &[String],
    ) -> CodegenResult<Vec<LoopBounds<'ctx>>> {
        let mut bounds = Vec::new();

        // A band's `members` are its SCHEDULING maps -- one per loop level. Each
        // member's piece describes ONE iterator, so the iteration space has exactly
        // `members.len()` dimensions.
        //
        // This used to iterate `0..domain.n_iter` for every member, taking the loop
        // bounds from a member's domain rather than from the member itself. For a
        // 2-dimensional band that produced 2 x 2 = 4 bounds instead of 2, so the
        // emitter built a 4-deep nest: a 3x2 loop accumulated 36 rather than 6. The IR
        // looked like an ordinary nest, so nothing about it was legible as wrong.
        for (level, member) in members.iter().enumerate() {
            // Get the domain of the schedule map
            let domain = &member.pieces[0].domain;

            // The bound for THIS level, which is the member's own index. Iterating the
            // full `n_iter` would repeat earlier levels once per member.
            let iter_dim = level;
            if let Some((lower, upper)) = domain.iterator_bounds(iter_dim) {
                // The band's own iterator list is where the source spelling of this
                // level's induction variable lives. An empty or absent entry means the
                // band does not declare one, and the induction variable is then left
                // unbound -- a body that reads it is a diagnostic, never a silent zero.
                let iterator_name = match iterators.get(level) {
                    Some(name) if !name.is_empty() => Some(name.clone()),
                    _ => None,
                };
                bounds.push(LoopBounds {
                    iterator_dim: iter_dim,
                    lower: self.lower_affine_expr(&domain.parameter_names, &lower)?,
                    iterator_name,
                    upper: self.lower_affine_expr(&domain.parameter_names, &upper)?,
                    step: 1, // Default step of 1
                });
            }
        }

        Ok(bounds)
    }

    /// Lower an affine expression in the domain's parameters to an LLVM value.
    ///
    /// `expr` is `sum_j expr.coefficients[j] * P_j + expr.constant`, where `P_j` is
    /// the domain's parameter dimension `j` and `parameter_names[j]` is its name.
    ///
    /// # Where a parameter's value comes from
    ///
    /// From the generated function's own argument of that name, and nowhere else.
    /// `LLVMModuleBuilder::build_module` gives `naso_entry` one `i64` argument per
    /// symbolic constant the schedule's domains name, and
    /// `lower_schedule_tree_into` stores each argument into an alloca bound to that
    /// name before the schedule is lowered. So a `forall i in 0..n` bound reads the
    /// `n` the caller passed, and the same binding makes `n` readable in the loop
    /// body.
    ///
    /// Nothing here invents a value. A parameter dimension with no name, or a name
    /// with no binding, is a diagnostic: substituting zero for a trip count is a
    /// program that runs the wrong number of iterations and says nothing.
    fn lower_affine_expr(
        &mut self,
        parameter_names: &[String],
        expr: &crate::ir::affine_domain::AffineExpr,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        let int_type = self
            .value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);

        // An expression with no parameter term is a constant, which is the common
        // case for a literal bound.
        let mut acc: Option<inkwell::values::IntValue<'ctx>> = None;
        for (j, &coeff) in expr.coefficients.iter().enumerate() {
            if coeff == 0 {
                continue;
            }
            let name = parameter_names.get(j).map(String::as_str).unwrap_or("");
            let value = self.load_parameter_value(name, j)?;
            let term = if coeff == 1 {
                value
            } else {
                // `coeff as u64` is the two's-complement bit pattern, which is what
                // `const_int` wants: `mul` by `2^64 - c` is `-c` in wrapping i64
                // arithmetic, so a negative coefficient needs no special case and no
                // unsigned/signed reinterpretation of a value.
                self.value_builder.build_int_mul(
                    value,
                    int_type.const_int(coeff as u64, false),
                    "bound_scale",
                )?
            };
            acc = Some(match acc {
                None => term,
                Some(prev) => self.value_builder.build_int_add(prev, term, "bound_sum")?,
            });
        }

        let constant =
            self.value_builder
                .build_int_constant(int_type, expr.constant as u64, "bound");
        Ok(match acc {
            None => constant.into(),
            Some(sum) => self
                .value_builder
                .build_int_add(sum, constant, "bound")?
                .into(),
        })
    }

    /// Load the i64 value of the parameter named `name` (domain parameter `index`).
    fn load_parameter_value(
        &mut self,
        name: &str,
        index: usize,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        if name.is_empty() {
            return Err(CodegenError::UnsupportedFeature(format!(
                "a loop bound depends on parameter dimension {index}, which this domain \
                 does not name. A symbolic bound can only be lowered when the \
                 parameter's name is known, because the name is how its value is \
                 found; the value is NOT invented."
            )));
        }
        let (ptr, ty) = self.value_builder.variable(name).ok_or_else(|| {
            CodegenError::UnsupportedFeature(format!(
                "a loop bound depends on the symbolic constant `{name}`, which has no \
                 value in the generated function. `naso_entry` takes one argument per \
                 symbolic constant the schedule names, so this means the band refers \
                 to a parameter the entry signature does not carry."
            ))
        })?;
        // The width is checked, not assumed: loading an i32 slot as i64 would read
        // past it, and silently widening a parameter is the kind of implicit
        // conversion this backend must not insert.
        match ty {
            inkwell::types::BasicTypeEnum::IntType(t) if t.get_bit_width() == 64 => {
                Ok(self.value_builder.build_load(ptr, name)?.into_int_value())
            }
            other => Err(CodegenError::UnsupportedFeature(format!(
                "symbolic constant `{name}` is bound to {other:?}, but loop bounds are \
                 computed in i64. Widening it here would change the value."
            ))),
        }
    }

    /// Lower a filter node (conditional domain restriction)
    fn lower_filter(&mut self, domain: &AffineDomain, child: &ScheduleNode) -> CodegenResult<()> {
        // Compute predicate from filter domain constraints
        let predicate = self.compute_filter_predicate(domain)?;

        // Create basic blocks for then/else
        let func = self.function;
        let then_block = self
            .value_builder
            .type_lowering()
            .context()
            .append_basic_block(func, "filter_then");
        let else_block = self
            .value_builder
            .type_lowering()
            .context()
            .append_basic_block(func, "filter_else");
        let merge_block = self
            .value_builder
            .type_lowering()
            .context()
            .append_basic_block(func, "filter_merge");

        // Branch on predicate
        self.value_builder
            .build_conditional_branch(predicate, then_block, else_block)?;

        // Then branch: lower child
        self.set_current_block(then_block);
        self.lower_node(child)?;
        self.value_builder.build_unconditional_branch(merge_block)?;

        // Else branch: skip child
        self.set_current_block(else_block);
        self.value_builder.build_unconditional_branch(merge_block)?;

        // Merge
        self.set_current_block(merge_block);
        Ok(())
    }

    /// Compute filter predicate from domain constraints
    fn compute_filter_predicate(
        &mut self,
        domain: &AffineDomain,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        let bool_type = self
            .value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I1);
        let _zero = bool_type.const_zero();

        // For now, combine all constraints with AND
        let mut predicate = bool_type.const_int(1, false);

        for constraint in &domain.constraints {
            let constraint_val = self.lower_constraint(constraint)?;
            predicate = self
                .value_builder
                .build_and(predicate, constraint_val, "filter_and")?;
        }

        Ok(predicate)
    }

    /// Lower a single constraint to a boolean value
    fn lower_constraint(
        &mut self,
        _constraint: &crate::ir::affine_domain::AffineConstraint,
    ) -> CodegenResult<inkwell::values::IntValue<'ctx>> {
        // Simplified implementation
        let bool_type = self
            .value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I1);
        Ok(bool_type.const_int(1, false))
    }

    /// Lower a sequence node (sequential composition)
    fn lower_sequence(&mut self, children: &[ScheduleNode]) -> CodegenResult<()> {
        for child in children {
            self.lower_node(child)?;
        }
        Ok(())
    }

    /// Lower a context node (parameter constraints)
    fn lower_context(&mut self, _domain: &AffineDomain, child: &ScheduleNode) -> CodegenResult<()> {
        // Context nodes impose constraints on parameters
        // For codegen, we can emit assertions or just lower the child
        self.lower_node(child)
    }

    /// Lower a domain node (statement instance)
    fn lower_domain(&mut self, stmt_id: StmtId, _domain: &AffineDomain) -> CodegenResult<()> {
        // Find the statement
        let stmt = self
            .pir_module
            .statements
            .iter()
            .find(|s| s.id == stmt_id)
            .ok_or_else(|| {
                CodegenError::FunctionBuildError(format!("Statement {:?} not found", stmt_id))
            })?;

        // Check if statement is [0]-quantity (erased)
        if stmt.quantity == crate::ast::Quantity::Zero {
            return Ok(()); // Skip erased statements
        }

        // Get access relations for this statement
        let accesses = self.access_relations.for_stmt(stmt_id);

        // Emit access instructions (loads/stores/GEPs)
        for access in accesses {
            self.access_emitter.emit_access(
                self.value_builder,
                access,
                &stmt.body,
                self.quantities,
            )?;
        }

        // Emit the statement body expression
        self.emit_statement_body(stmt)?;

        Ok(())
    }

    /// Emit the body of a statement: lower its `PirExpr` into real instructions.
    ///
    /// This used to be a stub returning `Ok(())`, on the theory that "this is handled
    /// by the access emitter". It is not, and not partly: the access emitter only
    /// handles `AccessRelation`s, which a plain expression body has none of. So a
    /// `forall` lowered to a correct loop -- preheader, header, phi induction variable,
    /// latch, exit -- with an EMPTY body. The loop ran the right number of times and
    /// computed nothing, which is a silent wrong answer rather than a diagnostic.
    ///
    /// The expression logic itself lives in `PirExprLowerer`, shared with
    /// `LLVMModuleBuilder`, so there is exactly one implementation of `Cast`, `Assign`
    /// and the rest. Two copies would drift, and the copy that drifted would be the
    /// one no existing test exercised -- which is exactly how the stub survived.
    ///
    /// `in_statement_position` is true: a `let` in a statement body is a binding, and
    /// it must outlive its own expression.
    fn emit_statement_body(&mut self, stmt: &PirStatement) -> CodegenResult<()> {
        let quantities = self.quantities;
        //
        // An AFFINE frame, not an empty stack.
        //
        // This used to be `Vec::new()` with a comment saying no loop was being lowered
        // -- which was wrong. The body IS inside a band: `forall i in 0..4 { ... }`
        // emits straight-line code for each point of a known iteration space, and this
        // function builds that code. With an empty stack, a `break` in the body was
        // reported as being "outside a loop", telling the author their program had no
        // loop when it plainly has one.
        //
        // The two targets are never branched to -- `affine_band` is checked before
        // either is read -- but they must be real block values.
        //
        // The entry block stands in for both targets. They are never branched to --
        // `affine_band` is checked first and refuses -- but `LoopTargets` holds real
        // block values, and the function's entry block is one that certainly exists.
        // A `for` is chosen over inventing two unreachable blocks so that a future
        // refactor which DID branch here would produce a visible wrong answer rather
        // than a dangling block reference.
        let band_exit = self
            .function
            .get_first_basic_block()
            .expect("a function under construction always has an entry block");
        let mut lowerer = PirExprLowerer {
            value_builder: self.value_builder,
            module: self.module,
            current_function: self.function,
            callee_params: self.callee_params,
            in_statement_position: true,
            loop_stack: if self.band_depth > 0 {
                vec![crate::codegen::llvm::expr_lowering::LoopTargets {
                    continue_target: band_exit,
                    break_target: band_exit,
                    affine_band: true,
                }]
            } else {
                // Not inside a loop. An empty stack is correct HERE and only here, and
                // it makes `break` say "outside a loop", which is the truth.
                Vec::new()
            },
        };
        // The value is discarded because a statement body's result is not the point --
        // its SIDE EFFECTS are. Every arm of `build_expr` that has a side effect emits
        // it before returning, and an error still propagates, so a body that cannot be
        // lowered fails the build rather than compiling to nothing.
        let _ = lowerer.build_expr(&stmt.body, quantities)?;
        Ok(())
    }

    /// Lower an extension node (tiling, unrolling)
    fn lower_extension(&mut self, sizes: &[usize], child: &ScheduleNode) -> CodegenResult<()> {
        // Apply polyhedral optimization. `apply_extension` takes a
        // `&mut dyn FnMut(&ScheduleNode) -> CodegenResult<()>>` -- the callback it
        // hands to the tiling pass is a *function*, not a schedule lowering, so
        // it is invoked as `f(child)`.
        //
        // NOTE: `PolyhedralOptimizer::apply_tiling` is currently a placeholder
        // that never invokes the callback, so relying on it alone would silently
        // drop the entire extension subtree. We therefore lower the child
        // ourselves as well; whoever implements real tiling must remove this
        // direct lowering to avoid emitting the child twice.
        self.optimizer
            .apply_extension(sizes, child, |lower_child| lower_child(child))?;
        self.lower_node(child)
    }
}

/// Loop bounds representation
#[derive(Debug, Clone)]
struct LoopBounds<'ctx> {
    iterator_dim: usize,
    lower: BasicValueEnum<'ctx>,
    /// The iterator this bound drives, when the band's domain names it.
    ///
    /// `AffineDomain::name` is `nest(i)` for a domain derived from `forall i in
    /// ..`, which is the only channel carrying the source spelling of the
    /// iterator. It is `None` for a hand-built domain, in which case the
    /// induction variable is still emitted (the loop is real) but is not bound
    /// to a name, and a body reading it stays a diagnostic rather than a guess.
    iterator_name: Option<String>,
    upper: BasicValueEnum<'ctx>,
    step: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Mutability, Quantity};
    use crate::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
    use crate::ir::{
        affine_domain::AffineDomain,
        pir_types::{PirModule, PirStatement},
        schedule_tree::{ScheduleNode, ScheduleTree, StmtId},
    };
    use std::collections::HashMap;

    #[test]
    fn test_schedule_lowering_creation() {
        let context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        let llvm_context = context.llvm_context();
        let module = llvm_context.create_module("test");
        let void_type = llvm_context.void_type();
        let fn_type = void_type.fn_type(&[], false);
        let function = module.add_function("test_fn", fn_type, None);

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

        let pir_module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);

        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let builder = llvm_context.create_builder();
        let mut value_builder =
            crate::codegen::llvm::value_builder::LlvmValueBuilder::new(builder, type_lowering);

        let lowering = ScheduleLowering::new(
            &mut value_builder,
            &module,
            function,
            &pir_module,
            &pir_module.quantities,
            &pir_module.accesses,
            None,
        );
        assert!(lowering.is_ok());
    }

    /// Build a `ScheduleLowering` over a module with the given statement quantities.
    fn lowering_with_quantities(quantities: Vec<Quantity>) -> ScheduleLowering<'static, 'static> {
        // The context must outlive the lowering, which borrows it, so it is leaked
        // deliberately and released when the test process exits.
        let context: &'static CodegenContext = Box::leak(Box::new(
            CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap(),
        ));
        let llvm_context = context.llvm_context();
        // The module is leaked for the same reason as the context above: `Module<'ctx>`
        // is invariant over `'ctx`, so `ScheduleLowering`'s borrow of it must be the
        // same region as the borrow of the context, which a stack-local pair cannot
        // satisfy (drop order is the reverse of declaration order).
        let module: &'static inkwell::module::Module<'static> =
            Box::leak(Box::new(llvm_context.create_module("test")));
        let void_type = llvm_context.void_type();
        let function = module.add_function("f", void_type.fn_type(&[], false), None);

        let statements: Vec<PirStatement> = quantities
            .iter()
            .enumerate()
            .map(|(i, q)| PirStatement {
                id: StmtId(i),
                domain: AffineDomain::universe(0, 0),
                body: crate::ir::pir_types::PirExpr::IntLit(i as i64),
                quantity: *q,
                mutability: Mutability::Immutable,
                span: None,
            })
            .collect();
        let schedule = ScheduleTree::new(ScheduleNode::Empty, vec![]);
        let pir_module = Box::leak(Box::new(PirModule::new(
            statements,
            schedule,
            AccessRelations::new(),
            HashMap::new(),
            vec![],
        )));

        let type_lowering =
            crate::codegen::llvm::type_lowering::LlvmTypeLowering::new(llvm_context);
        let builder = llvm_context.create_builder();
        let value_builder: &'static mut crate::codegen::llvm::value_builder::LlvmValueBuilder<
            'static,
        > = Box::leak(Box::new(
            crate::codegen::llvm::value_builder::LlvmValueBuilder::new(builder, type_lowering),
        ));
        ScheduleLowering::new(
            value_builder,
            module,
            function,
            pir_module,
            &pir_module.quantities,
            &pir_module.accesses,
            None,
        )
        .expect("lowering construction")
    }

    /// A band whose statements are all `[0]` is erased: the loop is dropped entirely.
    #[test]
    fn a_band_of_only_zero_quantity_statements_is_erased() {
        let lowering = lowering_with_quantities(vec![Quantity::Zero, Quantity::Zero]);
        let child = ScheduleNode::Sequence {
            children: vec![
                ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
                ScheduleNode::domain(StmtId(1), AffineDomain::universe(0, 0)),
            ],
        };
        assert!(
            lowering.is_band_erased(&child),
            "an all-[0] band must be erased"
        );
    }

    /// A band containing a `[1]` statement is NOT erased, even if some OTHER statement
    /// in the program is `[0]`.
    ///
    /// This is the regression for the old whole-quantity-map test, which erased EVERY
    /// band as soon as ANY binding anywhere had `[0]` quantity. The loop then ran once
    /// instead of iterating -- a silent wrong answer with no diagnostic.
    #[test]
    fn a_band_containing_a_linear_statement_survives_another_zero_quantity_statement() {
        // Statement 0 is [0], statement 1 is [1]. The band covers only statement 1.
        let lowering = lowering_with_quantities(vec![Quantity::Zero, Quantity::One]);
        let child = ScheduleNode::domain(StmtId(1), AffineDomain::universe(0, 0));
        assert!(
            !lowering.is_band_erased(&child),
            "a [1] statement's band must NOT be erased because another statement is [0]"
        );
    }

    /// A band covering BOTH a [0] and a [1] statement is not erased: dropping it would
    /// discard the linear work.
    #[test]
    fn a_band_mixing_zero_and_linear_statements_is_not_erased() {
        let lowering = lowering_with_quantities(vec![Quantity::Zero, Quantity::One]);
        let child = ScheduleNode::Sequence {
            children: vec![
                ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
                ScheduleNode::domain(StmtId(1), AffineDomain::universe(0, 0)),
            ],
        };
        assert!(
            !lowering.is_band_erased(&child),
            "erasing this band would silently drop the [1] statement"
        );
    }

    /// A band with no statements has nothing to run, so it is erased.
    #[test]
    fn a_band_with_no_statements_is_erased() {
        let lowering = lowering_with_quantities(vec![Quantity::One]);
        assert!(
            lowering.is_band_erased(&ScheduleNode::Empty),
            "a band with no statements must be erased rather than emit an empty loop"
        );
    }
}
