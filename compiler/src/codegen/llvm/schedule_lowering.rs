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
use crate::codegen::llvm::{
    access_emission::AccessEmitter,
    expr_lowering::PirExprLowerer,
    loop_emission::{LoopBounds as SequentialLoopBounds, LoopEmitter},
    parallel::{LoopBounds as ParallelLoopBounds, ParallelEmitter},
    polyhedral_opts::PolyhedralOptimizer,
    type_lowering::LlvmTypeLowering,
    value_builder::LlvmValueBuilder,
};
use crate::ir::{
    access_relation::AccessRelations,
    affine_domain::AffineDomain,
    affine_map::AffineMap,
    pir_types::{PirModule, PirStatement, QuantityMap},
    schedule_tree::{ScheduleNode, ScheduleTree, StmtId},
};
use inkwell::basic_block::BasicBlock;
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

    lower_schedule_tree_into(
        ctx,
        module,
        function,
        &mut value_builder,
        schedule,
        pir_module,
        quantities,
        access_relations,
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

    // Create schedule lowering context
    let mut lowering = ScheduleLowering::new(
        value_builder,
        module,
        function,
        pir_module,
        quantities,
        access_relations,
    )?;

    // Lower the root schedule node
    lowering.lower_node(&schedule.root)?;

    // Build return. inkwell 0.10's `build_return` takes
    // `Option<&dyn BasicValue>`; a void function returns `None`, while a typed
    // function returns a zero value of its return type (`FunctionType` has no
    // `Void` variant -- `None` *is* void).
    //
    // This is the ONE return for the function. A band emits its own `ret`-free
    // exit block and leaves the builder positioned there, so appending here
    // terminates the exit rather than an already-terminated block.
    let return_type = function.get_type().get_return_type();
    let return_value = return_type.map(|ty| lowering.value_builder.build_zero(ty));
    lowering.value_builder.build_return(return_value)?;

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

/// Every `Domain` statement id in a schedule subtree, in tree order.
pub fn statements_under(node: &ScheduleNode) -> Vec<StmtId> {
    let mut ids = Vec::new();
    collect_statements(node, &mut ids);
    ids
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

    /// Current basic block
    current_block: Option<BasicBlock<'ctx>>,
    /// Statement -> block mapping
    stmt_blocks: HashMap<StmtId, BasicBlock<'ctx>>,
    /// Induction variable phi nodes
    induction_vars: HashMap<String, inkwell::values::PhiValue<'ctx>>,
    /// Loop metadata
    loop_metadata: HashMap<String, inkwell::values::MetadataValue<'ctx>>,

    // Sub-emitters
    loop_emitter: LoopEmitter<'ctx>,
    access_emitter: AccessEmitter<'ctx>,
    optimizer: PolyhedralOptimizer,
    parallel_emitter: ParallelEmitter<'ctx>,
}

impl<'ctx, 'a> ScheduleLowering<'ctx, 'a> {
    pub fn new(
        value_builder: &'a mut LlvmValueBuilder<'ctx>,
        module: &'a inkwell::module::Module<'ctx>,
        function: FunctionValue<'ctx>,
        pir_module: &'a PirModule,
        quantities: &'a QuantityMap,
        access_relations: &'a AccessRelations,
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
            current_block: None,
            stmt_blocks: HashMap::new(),
            induction_vars: HashMap::new(),
            loop_metadata: HashMap::new(),
            loop_emitter,
            access_emitter,
            optimizer,
            parallel_emitter,
        })
    }

    fn set_current_block(&mut self, block: BasicBlock<'ctx>) {
        self.current_block = Some(block);
        self.value_builder.builder().position_at_end(block);
    }

    fn current_block(&self) -> BasicBlock<'ctx> {
        self.current_block.expect("No current block set")
    }

    /// Lower a schedule node recursively
    pub fn lower_node(&mut self, node: &ScheduleNode) -> CodegenResult<()> {
        match node {
            ScheduleNode::Band {
                members,
                coincident,
                child,
            } => self.lower_band(members, coincident, child),
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
        child: &ScheduleNode,
    ) -> CodegenResult<()> {
        // Check if this band should be erased ([0] quantity)
        if self.is_band_erased(child) {
            // [0]-quantity band: skip code generation entirely
            return self.lower_node(child);
        }

        // Extract loop bounds from scheduling maps
        let bounds = self.extract_bounds(members)?;

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

        // Check if this band is parallelizable
        let is_parallel = coincident.iter().any(|&c| c);

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
                &mut self.value_builder,
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
                    )?;
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
                &mut self.value_builder,
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
                    )?;
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
    // when the band's domain does not pin an iterator to a constant range: a
    // symbolic `forall i in 0..n` encodes only `i >= 0`, and
    // `AffineDomain::iterator_bounds` requires BOTH a lower and an upper
    // constraint to answer. `lower_band` treats an empty result as an error
    // rather than emitting a body once, because emitting the body once is a
    // silent wrong answer -- precisely the bug this schedule path exists to fix.
    fn extract_bounds(&mut self, members: &[AffineMap]) -> CodegenResult<Vec<LoopBounds<'ctx>>> {
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
            for iter_dim in [level] {
                if let Some((lower, upper)) = domain.iterator_bounds(iter_dim) {
                    bounds.push(LoopBounds {
                        iterator_dim: iter_dim,
                        lower: self.lower_affine_expr(&lower)?,
                        // The domain's own name is where the source spelling of the
                        // iterator lives. It is read here and nowhere else, so the
                        // binding decision is made once, next to the loop bound that
                        // it describes.
                        iterator_name: domain.nest_iterator().map(str::to_string),
                        upper: self.lower_affine_expr(&upper)?,
                        step: 1, // Default step of 1
                    });
                }
            }
        }

        Ok(bounds)
    }

    /// Lower an affine expression to LLVM value
    fn lower_affine_expr(
        &mut self,
        expr: &crate::ir::affine_domain::AffineExpr,
    ) -> CodegenResult<BasicValueEnum<'ctx>> {
        // In a full implementation this would evaluate parameters and induction
        // variables; `build_int_constant` returns the `IntValue` directly.
        let int_type = self
            .value_builder
            .type_lowering()
            .int_type(crate::codegen::abi::IntWidth::I64);
        Ok(self
            .value_builder
            .build_int_constant(int_type, expr.constant as u64, "bound")
            .into())
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
                &mut self.value_builder,
                access,
                &stmt.body,
                &self.quantities,
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
        let mut lowerer = PirExprLowerer {
            value_builder: self.value_builder,
            module: self.module,
            current_function: self.function,
            in_statement_position: true,
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
        let lowering = ScheduleLowering::new(
            value_builder,
            module,
            function,
            pir_module,
            &pir_module.quantities,
            &pir_module.accesses,
        )
        .expect("lowering construction");
        lowering
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
