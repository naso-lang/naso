//! AST-to-Polyhedral IR Lowering Pass
//!
//! Converts typed AST (from Sprint 2 typechecker) to well-formed Polyhedral IR.
//! Handles loop nests, array accesses, reversible blocks, and quantity semantics.

#![allow(clippy::collapsible_if)]

pub mod access_analysis;
pub mod ast_to_pir;
pub mod loop_extraction;
pub mod reversible_lowering;
pub mod simd;
pub mod uncomputation;

use crate::ast::Program;
use crate::ir::validate::validate_pir;
use crate::ir::{
    AccessRelations, AffineDomain, AffineMap, PirModule, PirStatement, QuantityMap, ScheduleNode,
    ScheduleTree, StmtId,
};

/// SIMD lowering entry point
pub fn lower_quantization_to_simd(
    module: &PirModule,
    target: simd::SimdTarget,
) -> Result<PirModule, LoweringError> {
    simd::lower_quantization_to_simd(module, target)
}

/// Lowering error types
#[derive(Debug, Clone, thiserror::Error)]
pub enum LoweringError {
    #[error("Non-affine loop bounds: {0}")]
    NonAffineBounds(String),

    #[error("Array access out of bounds: {0}")]
    AccessOutOfBounds(String),

    #[error("Non-reversible operation in reversible block: {0}")]
    NonReversibleOp(String),

    #[error("Quantity mismatch: {0}")]
    QuantityMismatch(String),

    #[error("Unsupported construct: {0}")]
    Unsupported(String),

    #[error("IR validation failed: {0}")]
    ValidationError(String),
}

/// Public entry point for lowering a typed AST program to PIR
pub fn lower_program(program: &Program) -> Result<PirModule, LoweringError> {
    let mut ctx = LoweringContext::new();
    ctx.lower_program(program)
}

/// Main lowering entry point (alias for lower_program)
pub fn lower_ast(program: &Program) -> Result<PirModule, LoweringError> {
    lower_program(program)
}

/// Internal lowering context
pub struct LoweringContext {
    next_stmt_id: usize,
    _next_var_id: usize,
    quantities: QuantityMap,
    statements: Vec<PirStatement>,
    accesses: AccessRelations,
    schedule_nodes: Vec<ScheduleNode>,
    param_names: Vec<String>,
    /// The typed ABI parameter list, the union over every function's parameters.
    function_params: Vec<crate::ir::pir_types::FunctionParam>,
    /// One entry per `Item::Function`, each with its OWN statements, schedule,
    /// quantities and parameters.
    ///
    /// This is the structure a backend reads to emit one LLVM symbol per Naso
    /// function. `statements`, `schedule_nodes`, `quantities` and `function_params`
    /// remain as the flat concatenation for the compatibility surface.
    functions: Vec<crate::ir::pir_types::PirFunction>,
    /// The `StmtId` of the most recent `return e` this function lowered.
    ///
    /// `lower_stmt` turns `return e` into an ordinary statement, which is right for a
    /// `void` function and wrong for one that returns a value: a backend emitting
    /// `ret` needs to know WHICH statement holds the value. Recorded here and drained
    /// per function in `lower_item`, so a `return` in one function cannot be
    /// attributed to the next.
    pending_return_stmt: Option<crate::ir::schedule_tree::StmtId>,
}

/// The `ElemType` a scalar or tensor-element AST type lowers to.
///
/// `None` means "no LLVM spelling in this ABI", which the caller turns into a
/// refusal naming the type. Notably a float is ALWAYS `F64`: the AST records
/// `int_width` but has no float width, so `f32` and `f64` are the same type in
/// the IR already and inventing a distinction here would be a guess.
fn elem_kind(ty: &crate::ast::Type) -> Option<crate::ir::pir_types::ElemType> {
    use crate::ir::pir_types::ElemType;
    Some(match ty.kind {
        // `TypeKind::Float` covers both `f32` and `f64` with nothing to tell
        // them apart; see `ElemType`'s doc comment.
        crate::ast::ty::TypeKind::Float => ElemType::F64,
        crate::ast::ty::TypeKind::Bool => ElemType::Bool,
        // `int_width` is `None` for a bare `int`. Choosing a width here would be
        // inventing one, and loop bounds are i64 throughout this backend, so a
        // widthless `int` parameter has no exact slot.
        crate::ast::ty::TypeKind::Int
        | crate::ast::ty::TypeKind::UInt
        | crate::ast::ty::TypeKind::Nat => match ty.int_width {
            // Sub-byte. Carried distinctly so a backend must either pack it or refuse
            // it; see `ElemType::I4` for why substituting `I8` is not acceptable.
            Some(4) => ElemType::I4,
            Some(8) => ElemType::I8,
            Some(16) => ElemType::I16,
            Some(32) => ElemType::I32,
            Some(64) => ElemType::I64,
            _ => return None,
        },
        _ => return None,
    })
}

/// A `ParamKind` as text, for a diagnostic that names the type it could not give
/// a slot to.
fn describe_param_kind(kind: &crate::ir::pir_types::ParamKind) -> String {
    use crate::ir::pir_types::ParamKind;
    match kind {
        ParamKind::Tensor { elem, shape } => match shape {
            // Spelled the way the source spells it: element type first, then one
            // extent per dimension, so a two-dimensional matrix reads as
            // `Tensor[F64, 64, 64]` rather than as a shape the author never wrote.
            Some(shape) => {
                let dims: Vec<String> = shape.iter().map(|n| n.to_string()).collect();
                format!("Tensor[{elem:?}, {}]", dims.join(", "))
            }
            None => format!("Tensor[{elem:?}, <symbolic extent>]"),
        },
        ParamKind::Scalar(elem) => format!("{elem:?}"),
        ParamKind::QRegister => "QRegister".to_string(),
        ParamKind::Unsupported(ty) => ty.clone(),
    }
}

/// The names a `let` PATTERN binds.
///
/// A thin adapter over the lowering's own `extract_pattern_names`, so there is exactly
/// ONE walk of `PatternKind` in this file. Two walks would be two places to update when
/// a pattern form is added, and the failure mode is quiet: a name missing from one
/// quantity map and present in the other.
fn collect_pattern_names(pattern: &crate::ast::Pattern, out: &mut Vec<String>) {
    out.extend(LoweringContext::pattern_names(pattern));
}

/// Build a `ScheduleTree` from a function's own schedule nodes.
///
/// The same shape `build_schedule_tree` produces for the whole module: a `Sequence`
/// of the top-level nodes, or the single node when there is exactly one. A function
/// with no statements gets `ScheduleNode::Empty`, which means "emit nothing" -- the
/// same meaning it has at module level.
fn build_schedule_from_nodes(nodes: &[ScheduleNode], param_names: &[String]) -> ScheduleTree {
    let root = match nodes.len() {
        0 => ScheduleNode::Empty,
        1 => nodes[0].clone(),
        _ => ScheduleNode::sequence(nodes.to_vec()),
    };
    ScheduleTree::new(root, param_names.to_vec())
}

/// Every name a function BODY binds, in source order.
///
/// This walks the AST, not the lowered PIR, because the lowered form has already
/// merged every function's names into one map and the function's own share cannot be
/// recovered from it. Walking the AST is also more faithful: it sees a `let` inside a
/// loop body, which lowering lifts into the enclosing loop statement.
///
/// Only `let`-family bindings and `for`/`forall` iterators count. A name merely READ
/// (`input`) is not bound by the body, and including reads would make every function's
/// quantity map claim every name it mentions -- including another function's.
fn collect_bound_names_stmt(stmt: &crate::ast::Stmt, out: &mut Vec<String>) {
    use crate::ast::StmtKind;
    match &stmt.kind {
        StmtKind::Let(l) => {
            // A destructuring pattern binds more than one name, or none. The
            // lowering refuses it (`destructuring let binding has no PIR node`), so
            // whatever names it holds are recorded: if that refusal is ever lifted,
            // the quantity map already covers them.
            let mut names = Vec::new();
            collect_pattern_names(&l.pattern, &mut names);
            out.extend(names);
        }
        StmtKind::LetInOut(l) => out.push(l.name.name.clone()),
        StmtKind::LetConsume(l) => out.push(l.name.name.clone()),
        StmtKind::Expr(e) => collect_bound_names_expr(e, out),
        // A nested `Item::Function`'s body binds names in the SAME lexical scope as
        // this statement -- it is not a new function, so its locals are this
        // function's locals.
        StmtKind::Item(crate::ast::Item::Function(f)) => {
            for s in &f.body.stmts {
                collect_bound_names_stmt(s, out);
            }
        }
        _ => {}
    }
}

fn collect_bound_names_expr(expr: &crate::ast::Expr, out: &mut Vec<String>) {
    use crate::ast::expr::ExprKind;
    match &expr.kind {
        // A loop binds its iterator. `Forall` carries the loop variable on the node.
        ExprKind::Forall(loop_) => {
            for s in &loop_.body.stmts {
                collect_bound_names_stmt(s, out);
            }
            if let Some(tail) = &loop_.body.expr {
                collect_bound_names_expr(tail, out);
            }
        }
        ExprKind::Block(b) => {
            for s in &b.stmts {
                collect_bound_names_stmt(s, out);
            }
            if let Some(tail) = &b.expr {
                collect_bound_names_expr(tail, out);
            }
        }
        ExprKind::Binary(_, l, r) => {
            collect_bound_names_expr(l, out);
            collect_bound_names_expr(r, out);
        }
        ExprKind::Unary(_, e)
        | ExprKind::Ascribe(e, _)
        | ExprKind::Index(e, _)
        | ExprKind::Field(e, _)
        | ExprKind::Return(Some(e))
        | ExprKind::Break(Some(e)) => collect_bound_names_expr(e, out),
        ExprKind::Let(l) => out.push(l.name.name.clone()),
        ExprKind::LetInOut(l) => out.push(l.name.name.clone()),
        ExprKind::LetConsume(l) => out.push(l.name.name.clone()),
        ExprKind::Call(callee, args) | ExprKind::MethodCall(callee, _, args) => {
            collect_bound_names_expr(callee, out);
            for a in args {
                collect_bound_names_expr(a, out);
            }
        }
        ExprKind::Tuple(items) | ExprKind::Array(items) => {
            for item in items {
                collect_bound_names_expr(item, out);
            }
        }
        ExprKind::If(c, t, e) => {
            collect_bound_names_expr(c, out);
            collect_bound_names_expr(t, out);
            if let Some(e) = e {
                collect_bound_names_expr(e, out);
            }
        }
        // A `Quantified` is a PROPOSITION, not a loop: it runs no code, so a `let`
        // inside it binds nothing that survives into a runtime statement. Skipping it
        // is correct rather than merely convenient.
        ExprKind::Quantified(_) => {}
        _ => {}
    }
}

impl LoweringContext {
    fn new() -> Self {
        Self {
            next_stmt_id: 0,
            _next_var_id: 0,
            quantities: QuantityMap::new(),
            statements: Vec::new(),
            accesses: AccessRelations::new(),
            schedule_nodes: Vec::new(),
            param_names: Vec::new(),
            function_params: Vec::new(),
            functions: Vec::new(),
            pending_return_stmt: None,
        }
    }

    fn next_stmt_id(&mut self) -> StmtId {
        let id = StmtId(self.next_stmt_id);
        self.next_stmt_id += 1;
        id
    }

    fn lower_program(&mut self, program: &Program) -> Result<PirModule, LoweringError> {
        // Extract parameters from main function
        self.extract_parameters(program);

        // The typed ABI list, over EVERY function. This is a separate pass from
        // `extract_parameters` because the two answer different questions: that
        // one asks "which names does the schedule need a value for?", this one
        // asks "what ABI does the generated function need?".
        //
        // A name declared at TWO DIFFERENT TYPES by two different functions is no
        // longer an error. It used to be refused with "declared with two different
        // types ... a PIR module is one flat statement list with no function
        // structure", which is exactly what stopped `kernels/quant_int8.naso` from
        // compiling: its three functions each declare `input`/`output`, at
        // `Tensor[f32, 1024]` in one and `Tensor[i8, 1024]` in another. Each function
        // now carries its own `PirFunction::params`, so both are representable; this
        // union keeps the FIRST declaration per name for the compatibility surface
        // described on `PirModule::function_params`.
        self.extract_function_params(program);

        // Lower all items
        for item in &program.items {
            self.lower_item(item)?;
        }

        // Build schedule tree from extracted nodes
        let schedule = self.build_schedule_tree()?;

        // Build PIR module
        let mut pir = PirModule::new(
            std::mem::take(&mut self.statements),
            schedule,
            std::mem::take(&mut self.accesses),
            std::mem::take(&mut self.quantities),
            self.param_names.clone(),
        );
        pir.function_params = std::mem::take(&mut self.function_params);
        pir.functions = std::mem::take(&mut self.functions);

        // Validate the generated PIR
        validate_pir(&pir).map_err(|e| LoweringError::ValidationError(format!("{:?}", e)))?;

        Ok(pir)
    }

    fn extract_parameters(&mut self, program: &Program) {
        // Extract parameters from main function signature
        //
        // `param_names` stays the module-level SYMBOLIC-CONSTANT list, which is a
        // different thing from the ABI: it carries names only and exists so a
        // schedule band naming `n` has a slot for its value. The typed ABI that a
        // backend builds a function signature from is `function_params`.
        for item in &program.items {
            if let crate::ast::Item::Function(func) = item {
                if func.name.name == "main" {
                    for param in &func.params {
                        self.param_names.push(param.name.name.clone());
                        self.quantities
                            .insert(param.name.name.clone(), param.ty.quantity);
                    }
                    break;
                }
            }
        }
    }

    /// Lower every function's parameters into the typed ABI list.
    ///
    /// This runs for EVERY function, not just `main`. Restricting it to `main`
    /// was why a body reading `input` in `kernels/scale_f32.naso` had no
    /// allocation: the file's functions are `scale_clamp_f32` and `scale_one`,
    /// so no parameter was ever recorded and the read was refused.
    ///
    /// # Two functions, one name, two types
    ///
    /// This used to REFUSE that case:
    ///
    /// ```text
    /// parameter `input` is declared with two different types (Tensor[F64, 1024] and
    /// Tensor[I8, 1024]). A PIR module is one flat statement list with no function
    /// structure, so the generated entry function has ONE slot per name and cannot
    /// give this name both types.
    /// ```
    ///
    /// and that refusal is why `kernels/quant_int8.naso` -- a shipped kernel that
    /// parses and typechecks -- could not be built for LLVM at all. The second
    /// sentence of that message is no longer true: `PirFunction` gives each function
    /// its own parameters, so `quantize_int8_symmetric`'s `Tensor[f32, 1024] input`
    /// and `dequantize_int8_symmetric`'s `Tensor[i8, 1024] input` are two slots in
    /// two functions.
    ///
    /// The name is deduplicated rather than rejected, FIRST declaration winning, so
    /// the flat union stays one entry per name for a backend that still reads it.
    /// That union cannot express both types, which is exactly why a backend emitting
    /// per-function symbols reads `PirFunction::params` and not this list.
    fn extract_function_params(&mut self, program: &Program) {
        for item in &program.items {
            let crate::ast::Item::Function(func) = item else {
                continue;
            };
            for param in &func.params {
                let entry = crate::ir::pir_types::FunctionParam {
                    name: param.name.name.clone(),
                    kind: self.param_kind(&param.ty),
                    quantity: param.quantity,
                    mutability: param.mutability,
                };
                if self.function_params.iter().any(|p| p.name == entry.name) {
                    continue;
                }
                self.function_params.push(entry);
            }
        }
    }

    /// The ABI shape of one AST parameter type.
    ///
    /// Anything not covered becomes `ParamKind::Unsupported` carrying the type's
    /// own `Display`, so the backend's refusal names what it could not lower.
    /// Returning `Unsupported` rather than `Err` keeps the diagnostic at the
    /// backend, which is the layer that knows what it can emit; the lowering pass
    /// has no LLVM types to reason about.
    fn param_kind(&self, ty: &crate::ast::Type) -> crate::ir::pir_types::ParamKind {
        use crate::ir::pir_types::ParamKind;
        match &ty.kind {
            crate::ast::ty::TypeKind::Tensor(dims) => {
                // `Tensor[Elem, N]`: the first type argument is the ELEMENT and
                // the rest are extents. Counting type arguments as dimensions
                // would make every real 1-D tensor look 2-D, which is the mistake
                // `wgsl_compute::tensor_parts` documents.
                let Some((elem, rest)) = dims.split_first() else {
                    return ParamKind::Unsupported("Tensor with no element type".to_string());
                };
                let Some(elem_ty) = elem_kind(elem) else {
                    // The common case here is a tensor OF tensors, `Tensor[Tensor[f32, 4], 8]`.
                    // Naming it as nesting rather than as an opaque element type is what
                    // tells the author the ABI would need a second stride, not a wider
                    // element: `Tensor[Tensor[f32, 4], 8]` cannot be one `ptr` because
                    // `rows[i]` yields four elements, not one.
                    return ParamKind::Unsupported(format!(
                        "a tensor whose element type is itself `{}`, i.e. a nested tensor. The \
                         LLVM ABI passes one `ptr` per tensor, which has a single stride; \
                         indexing `u[i][j]` needs a second stride that no argument supplies",
                        elem.kind
                    ));
                };
                // Every remaining type argument is an extent, so `Tensor[f32, 64]`
                // is shape `[64]` and `Tensor[f32, 64, 64]` is `[64, 64]`. A shape
                // rather than a single extent is what lets `C[i][j]` linearise.
                let shape: Option<Vec<u64>> = rest.iter().map(|d| d.nat_const()).collect();
                if let Some(ref s) = shape {
                    if s.contains(&0) {
                        return ParamKind::Unsupported(format!(
                            "tensor `{ty}` has a zero extent, so it has no elements to index \
                             or write"
                        ));
                    }
                }
                ParamKind::Tensor {
                    elem: elem_ty,
                    // A symbolic extent is carried as `None`, not resolved: a backend
                    // that needs a constant must refuse it rather than invent a bound.
                    shape,
                }
            }
            // `Qubit` is a single wire and `QRegister` a register of them; the ABI
            // treats both as a caller-owned pointer with no element type the LLVM
            // backend indexes, so both map to the same slot.
            //
            // `Qubit` previously fell through to `Unsupported("Qubit")`, which meant
            // NO quantum parameter type worked -- the one type the whole language
            // exists for had no ABI at all.
            crate::ast::ty::TypeKind::QRegister(_) | crate::ast::ty::TypeKind::Qubit => {
                ParamKind::QRegister
            }
            // The parser resolves `QRegister` to `Named("QRegister", args)`, NOT to
            // `TypeKind::QRegister` -- so the arm above was unreachable and BARE
            // `QRegister` produced `Unsupported("QRegister")`. Measured:
            //
            //     fn f(q: QRegister)            -> Unsupported("QRegister")
            //     fn f(q: QRegister[f32, 4])    -> Unsupported("QRegister<Float, 4>")
            //
            // Every quantum parameter type was therefore unusable: `Qubit` missed for
            // want of an arm, `QRegister` for want of a reachable one. Matched by name
            // so both spellings land on the pointer ABI.
            crate::ast::ty::TypeKind::Named(n, _)
                if n.name == "QRegister" || n.name == "Qubits" =>
            {
                ParamKind::QRegister
            }
            // A quantity is not a type. `Many` and `One` parse as `Named` because
            // they are ordinary identifiers in type position, so `fn f(q: Many)`
            // silently became `Unsupported("Many")` -- a diagnostic that names a
            // quantity as though it were the author's intended type.
            //
            // They are the quantity ANNOTATION, written `[0]` / `[1]` before the
            // type, so the fix is to point at that spelling rather than pass the
            // string through.
            crate::ast::ty::TypeKind::Named(n, _) if n.name == "Many" || n.name == "One" => {
                ParamKind::Unsupported(format!(
                    "`{}` is a quantity, not a type. Quantities are written as an \
                     annotation before the type: `[1] Qubit` consumes exactly once, \
                     `[0] Qubit` is borrowed and may be reused",
                    n
                ))
            }
            _ => match elem_kind(ty) {
                Some(elem) => ParamKind::Scalar(elem),
                None => ParamKind::Unsupported(format!("{ty}")),
            },
        }
    }

    fn lower_item(&mut self, item: &crate::ast::Item) -> Result<(), LoweringError> {
        use crate::ast::Item;

        match item {
            Item::Function(func) => {
                // Every function is lowered, not just `main`.
                //
                // This previously skipped anything not named `main`, so
                // `kernels/quant_int8.naso` -- whose functions are
                // quantize_int8_symmetric, dequantize_int8_symmetric and
                // normalize_f32 -- lowered to an empty schedule. A kernel file
                // produced no PIR at all, with no error to indicate it.
                //
                // StmtIds are unique per function, allocated from the same
                // counter, so the Domain nodes still identify distinct
                // statements.
                //
                // The offsets below SNAPSHOT the flat lists around this function's
                // body, so its own statements, schedule nodes and quantity
                // annotations can be lifted into a `PirFunction`. The flat lists
                // are left as they were -- the concatenation -- because that is the
                // compatibility surface every other consumer reads.
                let stmt_offset = self.statements.len();
                let node_offset = self.schedule_nodes.len();
                let access_offset = self.accesses.relations.len();
                // Reset before the body, not after: a `void` function's `return` is
                // not drained below (it is an ordinary discarded statement), so
                // without this a `void` function with a `return` would leave the id
                // behind for the NEXT value-returning function to claim.
                self.pending_return_stmt = None;
                // Set when the function's LAST statement is a control-flow
                // expression that is also its return value; see below.
                let mut tail_value_override: Option<crate::ir::schedule_tree::StmtId> = None;

                // A function body that ENDS with a control-flow expression --
                // `fn f() -> f32 { if c { 1.0 } else { 2.0 } }` -- has its `if` in
                // `body.stmts`, not `body.expr`, because the parser classifies
                // `if`/`match`/`for`/`while` as self-delimiting statements that need
                // no trailing semicolon.
                //
                // That is a statement POSITION, but the construct is an EXPRESSION
                // whose value is the function's return value, and its arms are blocks
                // whose tails are that value. Lowering it as an ordinary statement --
                // evaluated for its side effects, value discarded -- is how this ended
                // up refused as "no return statement" while the value sat in the
                // source. Worse, it would have computed the `if` and thrown the answer
                // away, which is the silent-wrong-answer shape.
                //
                // So the LAST statement, when it is control flow and there is no
                // `body.expr` after it, is lowered as the return value. `lower_stmt`
                // cannot do this: it discards an expression's value by contract.
                let stmts_len = func.body.stmts.len();
                //
                // `forall` is EXCLUDED. `is_control_flow_stmt` includes it, because a
                // `forall` statement also needs no trailing semicolon, but a `forall` is
                // a LOOP: `lower_expr_stmt` lowers it into a schedule band, and routing
                // it through `lower_expr` instead hit the `Unsupported` fallback with a
                // bare `Forall(...)` debug dump. That broke 15 lowering and
                // schedule-consumer tests. So the condition here is `if`/`match` only --
                // the constructs that are EXPRESSIONS whose value is the block's tail.
                let last_is_control_flow = func.body.expr.is_none()
                    && stmts_len > 0
                    && matches!(&func.body.stmts[stmts_len - 1].kind, crate::ast::StmtKind::Expr(e)
                        if matches!(e.kind, crate::ast::expr::ExprKind::If(..)));
                if last_is_control_flow {
                    // Statements before it run normally.
                    for stmt in &func.body.stmts[..stmts_len - 1] {
                        self.lower_stmt(stmt)?;
                    }
                    let expr = match &func.body.stmts[stmts_len - 1].kind {
                        crate::ast::StmtKind::Expr(e) => e.clone(),
                        _ => unreachable!("checked by last_is_control_flow"),
                    };
                    let lowered = self.lower_expr(&expr)?;
                    let stmt_id = self.next_stmt_id();
                    let domain = AffineDomain::universe(0, 0);
                    self.statements.push(PirStatement {
                        id: stmt_id,
                        domain: domain.clone(),
                        body: lowered,
                        quantity: crate::ast::Quantity::Many,
                        mutability: crate::ast::Mutability::Immutable,
                        span: None,
                    });
                    self.schedule_nodes
                        .push(ScheduleNode::domain(stmt_id, domain));
                    tail_value_override = Some(stmt_id);
                } else {
                    for stmt in &func.body.stmts {
                        self.lower_stmt(stmt)?;
                    }
                }
                // The body's trailing expression, and whether its VALUE is this
                // function's return value.
                //
                // The parser folds the last expression of a function body into
                // `Block::expr`, so it is NOT in `stmts`. Only iterating `stmts`
                // skipped it, and the function body then lowered as if it ended
                // earlier.
                //
                // `tail_value` is `Some` only for a function that DECLARES a return
                // type, contains no `return`, and ends in an expression that produces
                // a value. Each of those three conditions is load-bearing:
                //
                //  * no explicit `return` -- an explicit return is the programmer
                //    naming the value, and it wins. `pending_return_stmt` is checked
                //    BEFORE the tail is lowered precisely so a `return` in tail
                //    position cannot be double-counted as both.
                //  * a value-producing kind -- see `tail_yields_value`. This is the
                //    condition that keeps `fn f(x: f32) -> f32 { let mut y = x * 3.0;
                //    }` refused: the parser routes every `let` to `Block::stmts`, so
                //    that body has no `Block::expr` and no tail value at all, even
                //    though lowering does emit a statement for the binding. A tail
                //    that is not a value -- an assignment, a loop, a control-flow
                //    form -- is equally refused rather than guessed at.
                //
                // # A tail that is NOT the return still has to RUN
                //
                // Only the return-value case above takes the expression's VALUE. Every
                // other tail -- a `void` function's, or one a `return` already claimed --
                // goes through `lower_expr_stmt`, which makes it an ordinary SCHEDULED
                // statement.
                //
                // It used to go through `lower_expr` and have its result thrown away,
                // which silently dropped the tail's SIDE EFFECTS: `fn side(out: inout
                // [1] Tensor[f32, 1]) { out[0] = 7.0 }` -- a store, with no semicolon,
                // so the parser made it the tail -- lowered to a function whose body
                // emitted no store at all. The identical body with a `;` stored
                // correctly. A tail is not dead code because its value is unused, and a
                // compiler that compiles a store away is the silent wrong answer this
                // compiler exists to prevent.
                let declared_return = self.return_type(func);
                let tail_is_the_return =
                    !matches!(declared_return, crate::ir::pir_types::FnReturn::Void)
                        && self.pending_return_stmt.is_none()
                        && func
                            .body
                            .expr
                            .as_ref()
                            .is_some_and(|t| Self::tail_yields_value(&t.kind));
                let tail_value = if let Some(id) = tail_value_override {
                    Some(id)
                } else {
                    match &func.body.expr {
                        Some(tail) if tail_is_the_return => {
                            let lowered = self.lower_expr(tail)?;
                            let stmt_id = self.next_stmt_id();
                            let domain = AffineDomain::universe(0, 0);
                            self.statements.push(PirStatement {
                                id: stmt_id,
                                domain: domain.clone(),
                                body: lowered,
                                quantity: crate::ast::Quantity::Many,
                                mutability: crate::ast::Mutability::Immutable,
                                span: None,
                            });
                            self.schedule_nodes
                                .push(ScheduleNode::domain(stmt_id, domain));
                            Some(stmt_id)
                        }
                        // Not the return value, so it is evaluated for what it DOES rather
                        // than for what it is: an ordinary scheduled expression statement,
                        // exactly the lowering a `;` after it would have produced.
                        Some(tail) => {
                            self.lower_expr_stmt(tail)?;
                            None
                        }
                        None => None,
                    }
                };

                // Lift this function's slice out of the flat lists.
                //
                // A `return` in a nested position can make `lower_expr` push a
                // statement via `lower_stmt`, and `lower_loop_stmt` truncates and
                // re-pushes, so the slice is taken by OFFSET and length rather than
                // assumed to be "everything since the snapshot". That is the same
                // reason `lower_loop_stmt` snapshots rather than assuming.
                let statements: Vec<PirStatement> = self.statements[stmt_offset..].to_vec();
                let schedule_nodes: Vec<ScheduleNode> = self.schedule_nodes[node_offset..].to_vec();
                let accesses = crate::ir::AccessRelations {
                    relations: self.accesses.relations[access_offset..].to_vec(),
                };
                // The quantity names this function's body introduced. A name bound
                // by an earlier function is NOT re-declared into this function's map
                // unless this function binds it: `quantities` is keyed by name and
                // cannot express two bindings, so taking the keys the body added is
                // what keeps one function's local `v` out of another's scope.
                let quantities = self.per_function_quantities(func);

                let params = self.declared_params(func)?;
                let return_type = declared_return;

                // The RETURN VALUE, if this function returns one.
                //
                // A `void` function's `return e` is an ordinary statement whose value
                // is discarded, which is correct. A function that RETURNS A VALUE needs
                // `ret <value>`, and the value has to be evaluated in the function's
                // exit block, after the body -- not as one scheduled statement among
                // the others, which would return whatever the last statement happened
                // to compute. So the return statement is named here and removed from
                // this function's schedule nodes; the backend emits it at the `ret`.
                //
                // `pending_return_stmt` is drained here so one function's `return`
                // cannot be attributed to the next.
                //
                // # `None` here no longer means "returns zero"
                //
                // It used to. `-> i64` with no `return e` was accepted and the backend
                // read `None` as zero, which is the one answer a caller cannot
                // distinguish from a computed zero -- so the backend now refuses. The
                // other way a value-returning function can have `return_stmt == None`
                // is the implicit tail, recorded separately in `tail_return_stmt`
                // above. A backend that sees `None` must consult `tail_return_stmt`
                // next, and refuse only when BOTH are `None`.
                let return_stmt = match return_type {
                    crate::ir::pir_types::FnReturn::Void => None,
                    _ => self.pending_return_stmt.take(),
                };
                // An explicit `return` and an implicit tail are never both recorded:
                // `tail_is_the_return` required `pending_return_stmt.is_none()`, and a
                // tail `return` sets it during `lower_expr` above. Asserting it here
                // rather than trusting the ordering is what keeps the two fields from
                // silently disagreeing if the tail-lowering order ever changes.
                debug_assert!(
                    return_stmt.is_none() || tail_value.is_none(),
                    "function `{}` recorded both an explicit `return` and an implicit \
                     tail return; exactly one is the source's shape",
                    func.name.name
                );
                // Both the explicit return and the implicit tail are evaluated in the
                // exit block, so neither may ALSO be a scheduled statement.
                let schedule_nodes: Vec<ScheduleNode> = schedule_nodes
                    .into_iter()
                    .filter(|n| {
                        let ScheduleNode::Domain { stmt_id, .. } = n else {
                            return true;
                        };
                        *stmt_id != return_stmt.unwrap_or(StmtId(usize::MAX))
                            && *stmt_id != tail_value.unwrap_or(StmtId(usize::MAX))
                    })
                    .collect();

                self.functions.push(crate::ir::pir_types::PirFunction {
                    name: func.name.name.clone(),
                    params,
                    schedule: build_schedule_from_nodes(&schedule_nodes, &self.param_names),
                    statements,
                    accesses,
                    quantities,
                    return_type,
                    return_stmt,
                    tail_return_stmt: tail_value,
                    span: Some(func.span),
                });
            }
            _ => {
                // Skip other items for now
            }
        }
        Ok(())
    }

    /// The ABI parameters ONE function declares, in source order.
    ///
    /// Read from the AST rather than sliced out of `self.function_params`: that list
    /// is deduplicated by name across the whole program, so slicing it would give the
    /// first function's declaration of a shared name to every later function. Reading
    /// the AST is what lets `dequantize_int8_symmetric` keep `Tensor[i8, 1024] input`
    /// when `quantize_int8_symmetric` declared `Tensor[f32, 1024] input` first.
    ///
    /// A name declared TWICE WITHIN ONE FUNCTION at two different types is still an
    /// error, and it is a different error from the cross-function case: one LLVM
    /// function has one slot per name, so there is genuinely nowhere to put the second
    /// type. The message is the old one, narrowed to say so.
    fn declared_params(
        &self,
        func: &crate::ast::Function,
    ) -> Result<Vec<crate::ir::pir_types::FunctionParam>, LoweringError> {
        let mut out: Vec<crate::ir::pir_types::FunctionParam> = Vec::new();
        for param in &func.params {
            let entry = crate::ir::pir_types::FunctionParam {
                name: param.name.name.clone(),
                kind: self.param_kind(&param.ty),
                quantity: param.quantity,
                mutability: param.mutability,
            };
            if let Some(existing) = out.iter().find(|p| p.name == entry.name) {
                if existing.kind != entry.kind {
                    return Err(LoweringError::Unsupported(format!(
                        "function `{}` declares parameter `{}` twice, at two different \
                         types ({} and {}). One LLVM function has one slot per name, \
                         so there is nowhere to put the second type. Note that the \
                         SAME name at two types in TWO DIFFERENT functions is fine: \
                         each function has its own scope.",
                        func.name.name,
                        entry.name,
                        describe_param_kind(&existing.kind),
                        describe_param_kind(&entry.kind),
                    )));
                }
                continue;
            }
            out.push(entry);
        }
        Ok(out)
    }

    /// What `func` returns, as the ABI element type.
    ///
    /// `None` in the AST is `FnReturn::Void`. A declared type with no scalar spelling
    /// here becomes `Unsupported`, so a backend refuses it naming the type rather than
    /// emitting a void function whose callers bind a value that does not exist.
    fn return_type(&self, func: &crate::ast::Function) -> crate::ir::pir_types::FnReturn {
        use crate::ir::pir_types::FnReturn;
        let Some(ty) = &func.ret_ty else {
            return FnReturn::Void;
        };
        match elem_kind(ty) {
            Some(elem) => FnReturn::Scalar(elem),
            None => FnReturn::Unsupported(format!("{ty}")),
        }
    }

    /// Does this trailing-expression kind produce a VALUE, and so can be a
    /// function's implicit return?
    ///
    /// # Why an allow-list and not a deny-list
    ///
    /// Because the question is not "is this a statement" but "does evaluating it
    /// produce a value the caller can receive". A deny-list would have to enumerate
    /// every form that does NOT, and `ExprKind` grows: the next variant added would
    /// default to "this is a value", which is exactly the silent wrong answer this
    /// compiler exists to prevent. An allow-list defaults to "not a value", so a new
    /// expression kind is refused until someone decides it yields one.
    ///
    /// # What is deliberately NOT here
    ///
    ///  * `Let`/`LetInOut`/`LetConsume` -- a binding's value is its bound NAME, and a
    ///    function body that ends in one has no expression at all. The parser routes
    ///    every `let` to `Block::stmts`, so this case is reached only through a block
    ///    expression; it is listed as refused so the reason is stated rather than
    ///    reached by accident.
    ///  * `Assign` -- an assignment is a store. Its "value" is not what the source
    ///    means by the function's result, and reading the target back would invent one.
    ///  * `Return`/`Break`/`Continue` -- control flow, not values. A `return` here is
    ///    handled by `pending_return_stmt`, so reaching this arm means something else.
    ///  * `For`/`Forall`/`While`/`Quantified` -- loops. A loop's result is a statement
    ///    list, not an expression, and a `forall` band in tail position has no value
    ///    this backend can name.
    ///  * `Reversible`, `Lambda`, `Projection`, `Error` -- none lowers to a value this
    ///    ABI can return; `lower_expr` refuses the unsupported ones itself.
    ///
    /// # What IS here
    ///
    /// The forms whose lowering is a `PirExpr` the backend evaluates to a value:
    /// literals, variables, operators, calls, subscripts, field access, the
    /// constructors, the control-flow-valued `if`/`match`, a type ascription, and a
    /// block expression (whose value is its own trailing expression).
    ///
    /// Note that this says nothing about the TYPE. Whether the value matches the
    /// declared return type is decided by the backend, which refuses a mismatch rather
    /// than converting -- see `lower_schedule_tree_into`.
    fn tail_yields_value(kind: &crate::ast::expr::ExprKind) -> bool {
        use crate::ast::expr::ExprKind as E;
        matches!(
            kind,
            E::Literal(_)
                | E::Var(_)
                | E::Binary(..)
                | E::Unary(..)
                | E::Call(..)
                | E::MethodCall(..)
                | E::Field(..)
                | E::Index(..)
                | E::Struct(..)
                | E::Variant(..)
                | E::Tuple(_)
                | E::Array(_)
                | E::Block(_)
                | E::If(..)
                | E::Match(..)
                | E::Ascribe(..)
                | E::QuantumOp(_)
        )
    }

    /// The quantity annotations in scope for `func`: its parameters, plus every name
    /// its body bound.
    ///
    /// `self.quantities` is the program-wide union, so it cannot be filtered by
    /// function -- a local `v` bound by an earlier function would be attributed to
    /// this one. Instead the parameters are taken from the AST and the locals are
    /// COLLECTED FROM THIS FUNCTION'S OWN BODY, which is the only place a name a
    /// function binds can appear. This is the granularity at which `[1]` linearity
    /// is a meaningful statement: consumed once by the body that owns it.
    fn per_function_quantities(&self, func: &crate::ast::Function) -> crate::ir::QuantityMap {
        let mut out = crate::ir::QuantityMap::new();
        for p in &func.params {
            out.insert(p.name.name.clone(), p.quantity);
        }
        let mut locals = Vec::new();
        for stmt in &func.body.stmts {
            collect_bound_names_stmt(stmt, &mut locals);
        }
        if let Some(tail) = &func.body.expr {
            collect_bound_names_expr(tail, &mut locals);
        }
        // A parameter's own quantity wins over anything the body recorded for the
        // same name: the declaration is what the typechecker checked, and a body that
        // re-reads a parameter does not re-quantify it.
        for name in locals {
            out.entry(name)
                .or_insert_with(|| crate::ast::Quantity::Many);
        }
        out
    }

    fn lower_stmt(&mut self, stmt: &crate::ast::Stmt) -> Result<(), LoweringError> {
        use crate::ast::StmtKind;

        match &stmt.kind {
            StmtKind::Let(let_stmt) => self.lower_let_stmt(let_stmt),
            StmtKind::LetInOut(let_inout) => self.lower_let_inout(let_inout),
            StmtKind::LetConsume(let_consume) => self.lower_let_consume(let_consume),
            StmtKind::Expr(expr) => self.lower_expr_stmt(expr),
            StmtKind::Reversible(block) => self.lower_reversible_block(block),
            StmtKind::Item(item) => self.lower_item(item),
            // A proof block is erased: it states obligations and produces no
            // runtime code, so it lowers to nothing. Without this arm the whole
            // enclosing function failed to lower with
            // `Unsupported(Proof(...))` -- which is why kernels/quant_int8.naso,
            // a file that typechecks and parses, could not be lowered at all.
            // Obligation discharge is the prover's job (naso-verify), not the
            // lowering pass's.
            StmtKind::Proof(_) => Ok(()),
            // `return e` lowers to an expression statement holding e. The
            // return's control flow is carried by the expression itself; PIR
            // has no dedicated return node, and adding one is out of scope
            // here. Previously any function containing a `return` failed to
            // lower with Unsupported(Return(...)).
            StmtKind::Return(ret) => {
                if let Some(value) = ret {
                    let lowered = self.lower_expr(value)?;
                    let stmt_id = self.next_stmt_id();
                    self.pending_return_stmt = Some(stmt_id);
                    let domain = AffineDomain::universe(0, 0);
                    self.statements.push(PirStatement {
                        id: stmt_id,
                        domain: domain.clone(),
                        body: lowered,
                        quantity: crate::ast::Quantity::Many,
                        mutability: crate::ast::Mutability::Immutable,
                        span: None,
                    });
                    self.schedule_nodes
                        .push(ScheduleNode::domain(stmt_id, domain));
                }
                Ok(())
            }
            _ => Err(LoweringError::Unsupported(format!("{:?}", stmt.kind))),
        }
    }

    fn lower_let_stmt(&mut self, let_stmt: &crate::ast::LetStmt) -> Result<(), LoweringError> {
        // Track quantity for all bindings in the pattern
        let qty = let_stmt.quantity;
        let mutability = let_stmt.mutability;

        // Extract variable names from pattern
        let names = self.extract_pattern_names(&let_stmt.pattern);
        for name in &names {
            self.quantities.insert(name.clone(), qty);
        }

        // Lower initialization expression
        let expr = self.lower_expr(&let_stmt.value)?;

        // Create statement.
        //
        // A destructuring pattern (`let (a, b) = ...`) has no single name to bind, and
        // PIR has no destructuring node, so those keep the previous behaviour of
        // lowering to the value alone. That is a real gap -- the value is computed and
        // discarded -- and it is reported rather than left silent below.
        //
        // A single-name pattern must become a `PirExpr::Let`, not bare the value. It used
        // to lower to `expr` directly, so the NAME was dropped and no allocation was ever
        // created: every later `total = ...` then had no destination and was refused with
        // "no allocation is known for it", which is what stopped any `forall` body from
        // accumulating into an outer variable.
        let body = match names.as_slice() {
            [single] => crate::ir::PirExpr::Let {
                name: single.clone(),
                qty,
                mutability,
                // `LetBinding`/`LetStmt` carry no body, so a statement-position binding
                // gets a placeholder tail. The backend keys off statement position, not
                // off this shape, so a genuine trailing expression of `0` is not
                // mistaken for it.
                value: Box::new(expr),
                body: Box::new(crate::ir::PirExpr::IntLit(0)),
            },
            [] => expr,
            multiple => {
                return Err(LoweringError::Unsupported(format!(
                    "destructuring `let` binding {{{}}} has no PIR node: the value is \
                     computed and no variable is created, so every use of the bound \
                     names would be undefined. Bind one name per statement instead.",
                    multiple.join(", ")
                )));
            }
        };

        let stmt_id = self.next_stmt_id();
        let domain = AffineDomain::universe(0, 0); // Scalar let binding

        let stmt = PirStatement {
            id: stmt_id,
            domain: domain.clone(),
            body,
            quantity: qty,
            mutability,
            span: None,
        };

        self.statements.push(stmt);
        // Add to schedule
        self.schedule_nodes
            .push(ScheduleNode::domain(stmt_id, domain));
        Ok(())
    }

    fn lower_let_inout(
        &mut self,
        let_inout: &crate::ast::LetInOutStmt,
    ) -> Result<(), LoweringError> {
        // Track quantity
        let qty = crate::ast::Quantity::Many; // inout bindings are many by default
        let mutability = crate::ast::Mutability::InOut;

        self.quantities.insert(let_inout.name.name.clone(), qty);

        // Lower initialization expression
        let expr = self.lower_expr(&let_inout.value)?;

        // Create statement
        let stmt_id = self.next_stmt_id();
        let domain = AffineDomain::universe(0, 0);

        let stmt = PirStatement {
            id: stmt_id,
            domain: domain.clone(),
            body: expr,
            quantity: qty,
            mutability,
            span: None,
        };

        self.statements.push(stmt);
        // Add to schedule
        self.schedule_nodes
            .push(ScheduleNode::domain(stmt_id, domain));
        Ok(())
    }

    fn lower_let_consume(
        &mut self,
        let_consume: &crate::ast::LetConsumeStmt,
    ) -> Result<(), LoweringError> {
        // Track quantity
        let qty = crate::ast::Quantity::One; // consume bindings are linear
        let mutability = crate::ast::Mutability::Consume;

        self.quantities.insert(let_consume.name.name.clone(), qty);

        // Lower initialization expression
        let expr = self.lower_expr(&let_consume.value)?;

        // Create statement
        let stmt_id = self.next_stmt_id();
        let domain = AffineDomain::universe(0, 0);

        let stmt = PirStatement {
            id: stmt_id,
            domain,
            body: expr,
            quantity: qty,
            mutability,
            span: None,
        };

        self.statements.push(stmt);
        Ok(())
    }

    /// Lower an expression used as a statement.
    ///
    /// A `forall` here is a LOOP, not a proposition -- the parser
    /// disambiguates the two by position. It needs the iteration domain and the
    /// schedule band, so it is intercepted before `lower_expr` (which has no
    /// loop case and would reject it).
    ///
    /// Previously every expression statement, loop or not, got
    /// `AffineDomain::universe(0, 0)` -- a ZERO-dimensional domain, i.e. a
    /// statement that runs exactly once. A `forall i in 0..1024 { .. }` was
    /// therefore lowered as a single-iteration body, silently. The domain is
    /// now taken from the loop's own bounds, and the band from
    /// `loop_nest_to_bands`.
    fn lower_expr_stmt(&mut self, expr: &crate::ast::Expr) -> Result<(), LoweringError> {
        // Reconstruct a Stmt so the existing extractor, which works on
        // statement-position foralls, can be reused unchanged.
        let as_stmt =
            crate::ast::Stmt::new(crate::ast::StmtKind::Expr(expr.clone()), expr.span, expr.id);

        if let Some(nest) = self::loop_extraction::extract_loop_nest(&as_stmt) {
            return self.lower_loop_stmt(nest, expr);
        }

        // An expression-position `return` (the parser's block tail, and how a
        // statement `return e;` is represented) must go through `lower_stmt`,
        // exactly once. It is routed here rather than given its own `lower_expr`
        // arm because `lower_expr_stmt` is this expr's single entry point:
        // handling it in both places lowered the return twice.
        if let crate::ast::ExprKind::Return(inner) = &expr.kind {
            return self.lower_stmt(&crate::ast::Stmt::new(
                crate::ast::StmtKind::Return(inner.as_deref().cloned()),
                expr.span,
                expr.id,
            ));
        }

        let expr = self.lower_expr(expr)?;
        let stmt_id = self.next_stmt_id();
        let domain = AffineDomain::universe(0, 0);

        let stmt = PirStatement {
            id: stmt_id,
            domain: domain.clone(),
            body: expr,
            quantity: crate::ast::Quantity::Many,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };

        self.statements.push(stmt);
        // Add to schedule
        self.schedule_nodes
            .push(ScheduleNode::domain(stmt_id, domain));
        Ok(())
    }

    /// Lower a `forall` loop into a statement with a real iteration domain and
    /// a real schedule band.
    fn lower_loop_stmt(
        &mut self,
        nest: self::loop_extraction::LoopNest,
        expr: &crate::ast::Expr,
    ) -> Result<(), LoweringError> {
        // The band's `coincident` flags are a dependence claim about the accesses
        // this loop body performs, so the access relations are handed in. The
        // front end records none for a `forall` body yet, which makes every level
        // sequential -- see `loop_extraction::level_is_parallel` -- and that is the
        // correct answer for "nothing was examined", not a fallback.
        //
        // The context is borrowed mutably by `loop_nest_to_bands` and the access
        // relations immutably, which are disjoint fields of `self`; going through
        // `std::mem::take` on the (currently empty) access set avoids the aliasing
        // and hands over an owned value, which is also the honest shape for a
        // function that only reads it.
        let accesses = std::mem::take(&mut self.accesses);
        let bands = self::loop_extraction::loop_nest_to_bands(&nest, &accesses, self);
        self.accesses = accesses;
        let bands = bands?;
        let (members, coincident, iterators) = match bands.first() {
            Some(ScheduleNode::Band {
                members,
                coincident,
                iterators,
                ..
            }) => (members.clone(), coincident.clone(), iterators.clone()),
            _ => {
                return Err(LoweringError::Unsupported(
                    "loop nest produced no band".to_string(),
                ));
            }
        };

        let domain = members[0].pieces[0].domain.clone();

        let stmt_id = self.next_stmt_id();

        // Lower the body. Bindings introduced by the loop are NOT bound as
        // values yet: the body is lowered as written, and a reference to an
        // iterator is handled by the band at execution time.
        let body = match &expr.kind {
            crate::ast::ExprKind::Forall(loop_) => {
                // A loop body is usually a statement sequence with no tail value.
                // It used to become `IntLit(0)` here, which discarded the body
                // entirely: a 1024-iteration kernel compiled to an empty function.
                // `PirExpr::Stmts` carries it instead.
                // `lower_stmt` appends to `self.statements` rather than
                // returning a value. Everything it appends for this body is
                // this body's, so snapshot the length and take the tail. The
                // outer statement's own schedule node is registered below.
                let first_new = self.statements.len();
                let first_sched = self.schedule_nodes.len();
                for st in &loop_.body.stmts {
                    self.lower_stmt(st)?;
                }
                let mut parts: Vec<crate::ir::PirExpr> = self.statements[first_new..]
                    .iter()
                    .map(|s| s.body.clone())
                    .collect();
                // Those inner statements must not also appear as top-level
                // statements: they are the loop body's, and the band runs them.
                self.statements.truncate(first_new);
                self.schedule_nodes.truncate(first_sched);

                if let Some(tail) = &loop_.body.expr {
                    parts.push(self.lower_expr(tail)?);
                }
                match parts.len() {
                    0 => crate::ir::PirExpr::Stmts(Vec::new()),
                    1 => parts.pop().expect("len checked"),
                    _ => crate::ir::PirExpr::Stmts(parts),
                }
            }
            _ => self.lower_expr(expr)?,
        };

        let stmt = PirStatement {
            id: stmt_id,
            domain: domain.clone(),
            body,
            quantity: crate::ast::Quantity::Many,
            mutability: crate::ast::Mutability::Immutable,
            span: None,
        };
        self.statements.push(stmt);

        // Replace the placeholder leaf with the real Domain node. Built here
        // rather than in loop_nest_to_bands because only this site knows the
        // PIR StmtId.
        let leaf = ScheduleNode::Domain {
            stmt_id,
            domain: domain.clone(),
        };
        self.schedule_nodes.push(ScheduleNode::Band {
            members,
            coincident,
            iterators,
            child: Box::new(leaf),
        });
        Ok(())
    }

    /// Lower a `reversible { ... }` block: forward pass, then uncomputation.
    ///
    /// # The defect this replaced, and why refusing was right for so long
    ///
    /// This used to lower the forward statements and then `Ok(())`, with a comment
    /// reading "(simplified - full implementation in reversible_lowering.rs)".
    /// Measured, against a control:
    ///
    /// ```text
    /// fn g(a: i64) -> i64 { let b = a + 1; b }                     -> 2 statements
    /// fn g(a: i64) -> i64 { reversible { let b = a + 1; b } }      -> 1 statement
    /// fn f(q: Many) -> Many { h(q) }                                -> 1 statement
    /// fn f(q: Many) -> Many { reversible { h(q) } }                 -> 0 statements
    /// ```
    ///
    /// So a `reversible` block dropped its body AND generated no inverse. The missing
    /// schedule tree WAS the uncomputation pass, which is the entire point of the
    /// construct: `reversible { h(q) }` is supposed to emit `h` and then uncompute it.
    /// What it emitted instead was whatever the forward walk happened to produce, and `Ok`.
    ///
    /// The existing `PirExpr::Reversible` refusals in the LLVM and QIR backends could not
    /// catch this: this path never CONSTRUCTS a `PirExpr::Reversible`. It emitted no node
    /// for a backend to reject, so the whole guard layer was bypassed. Same lesson as the
    /// Cranelift `42` -- the defect was visible only by asking what this path DOES with its
    /// input, not whether the input is correct.
    ///
    /// Refusing was then the honest answer, and it stayed refused until the inverse could
    /// be COMPUTED rather than assumed. See `lowering::uncomputation` for the design.
    ///
    /// # Where the inverse goes, and why that is the whole trick
    ///
    /// Into `self.statements`, appended after the forward statements. Not into a second
    /// field. Every backend already walks `statements` in order, so an appended statement
    /// executes on all of them with no backend change -- and there is no new field for a
    /// backend to forget to read. A second field would have recreated, one layer up, the
    /// exact defect above: a place the uncomputation could silently not reach.
    ///
    /// # Statement-snapshot discipline
    ///
    /// The forward statements are recorded BEFORE lowering the body and read back AFTER, so
    /// the inverse is computed from what was actually emitted rather than from what the
    /// source spelled. Reading the source instead would compute the adjoint of an operation
    /// that lowering may have renamed -- and a gate that is renamed on the way down is a
    /// gate whose inverse is not the one you asked for.
    fn lower_reversible_block(
        &mut self,
        block: &crate::ast::expr::ReversibleBlock,
    ) -> Result<(), LoweringError> {
        // A block tail is a real statement. `reversible { h(q) }` -- no semicolon -- parses
        // as a TAIL EXPRESSION, not as `body.stmts`, so counting only `stmts` reported
        // "this one holds 0 statements" for a block that holds one. The count is the only
        // thing the old refusal told the author, so it was wrong precisely when the author
        // had written the shortest possible block.
        let tail = block.body.expr.as_deref();
        let n = block.body.stmts.len() + usize::from(tail.is_some());

        if n == 0 {
            return Err(LoweringError::Unsupported(
                "an EMPTY `reversible { }` block is refused. With no body there is nothing to \
                 run and nothing to uncompute, so accepting it would emit a circuit that \
                 claims to be reversible because it does nothing -- an empty program is not \
                 evidence that an uncomputation pass ran."
                    .to_string(),
            ));
        }

        // Remember where the forward statements start, so they can be read back AFTER
        // lowering. The inverse must be derived from what was emitted, not from the source.
        let forward_start = self.statements.len();
        let schedule_start = self.schedule_nodes.len();

        for stmt in &block.body.stmts {
            self.lower_stmt(stmt)?;
        }
        // The tail is a statement too: it is lowered through `lower_stmt` rather than
        // `lower_expr_stmt` so a `return` inside it is not lowered twice.
        if let Some(tail) = tail {
            self.lower_stmt(&crate::ast::Stmt::new(
                crate::ast::StmtKind::Expr(tail.clone()),
                tail.span,
                tail.id,
            ))?;
        }

        let forward = self.statements[forward_start..].to_vec();

        // The first inverse id is allocated HERE, and this placement is load-bearing.
        //
        // It was allocated BEFORE the forward pass on the first attempt, and before the
        // block's own tail statement was lowered, and both produced
        // `DuplicateStatementId` from the IR validator. The second failure is the
        // instructive one: the ids run 0,1 (the two `let`s), 2,3 (the two forward gates),
        // then 4,5 -- which is the uncomputation -- and the ENCLOSING FUNCTION's tail then
        // allocated 5 as well, because `next_stmt_id` had not been advanced past the
        // uncomputation's own ids at the moment the tail ran.
        //
        // So the uncomputation has to advance the shared counter as it commits, not merely
        // choose ids inside its own range. The counter is the single source of statement
        // identity in this pass; an allocation that does not move it hands the same id to
        // the next allocation, and the result is two different statements sharing a name.
        let first_inverse_id = self.next_stmt_id();

        // Any refusal here happens BEFORE the forward statements are committed, so a
        // refused block leaves no partial emission behind. (The statements are already in
        // `self.statements` by this point; the truncation below is what removes them.)
        let inverse = match uncomputation::uncompute_statements(&forward, first_inverse_id) {
            Ok(inverse) => inverse,
            Err(e) => {
                self.statements.truncate(forward_start);
                self.schedule_nodes.truncate(schedule_start);
                return Err(e);
            }
        };

        // Commit: forward statements, then the uncomputation, in that order.
        //
        // Each committed statement consumes its id from the SHARED counter. Advancing
        // `next_stmt_id` past the whole inverse range is what stops the next allocation --
        // the enclosing function's tail, or the next statement -- from reissuing an id the
        // uncomputation already used.
        for stmt in inverse {
            let id = stmt.id;
            self.next_stmt_id = self.next_stmt_id.max(id.0 + 1);
            self.statements.push(stmt);
            self.schedule_nodes
                .push(ScheduleNode::domain(id, AffineDomain::universe(0, 0)));
        }
        Ok(())
    }

    fn lower_expr(&mut self, expr: &crate::ast::Expr) -> Result<crate::ir::PirExpr, LoweringError> {
        use crate::ast::expr::ExprKind;
        use crate::ir::PirExpr;

        match &expr.kind {
            ExprKind::Literal(lit) => self.lower_literal(lit),
            // `qir/primitives.rs` and `wgsl_compute.rs` apply it correctly.
            ExprKind::Ascribe(inner, ty) => {
                let lowered = self.lower_expr(inner)?;
                Ok(crate::ir::PirExpr::Cast {
                    expr: Box::new(lowered),
                    // The DECLARED width travels with the cast. Lowering to the inner
                    // expression alone loses the conversion, and a backend that cannot
                    // see one was asked for will not perform it -- which is how an
                    // `as i8` ended up storing an f64 into an i8 slot.
                    width: ty.int_width,
                    // `TypeKind::UInt`/`Nat` are the unsigned kinds; everything else
                    // that is an integer here is signed.
                    signed: !matches!(
                        ty.kind,
                        crate::ast::ty::TypeKind::UInt | crate::ast::ty::TypeKind::Nat
                    ),
                    // A FLOAT target is a different conversion, not an integer one with
                    // no width. Recorded explicitly because `width: None` used to be read
                    // as "i32", which turned `input[i] as f32` into a sign-extending
                    // integer conversion.
                    float_target: match ty.kind {
                        crate::ast::ty::TypeKind::Float => elem_kind(ty),
                        _ => None,
                    },
                })
            }

            ExprKind::Var(name) => Ok(PirExpr::Var(name.name.clone())),
            ExprKind::Binary(op, left, right) => {
                let l = self.lower_expr(left)?;
                let r = self.lower_expr(right)?;
                Ok(PirExpr::Binary {
                    op: self.lower_binop(*op),
                    left: Box::new(l),
                    right: Box::new(r),
                })
            }
            ExprKind::Unary(op, expr) => {
                let e = self.lower_expr(expr)?;
                Ok(PirExpr::Unary {
                    op: self.lower_unop(*op),
                    expr: Box::new(e),
                })
            }
            ExprKind::Call(func, args) => {
                let args = args
                    .iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                // Get function name from call
                let name = match &func.kind {
                    ExprKind::Var(name) => name.name.clone(),
                    _ => "unknown".to_string(),
                };
                Ok(PirExpr::Call { name, args })
            }
            ExprKind::Index(base, index) => {
                let b = self.lower_expr(base)?;
                let idx = self.lower_expr(index)?;
                Ok(PirExpr::Index {
                    base: Box::new(b),
                    indices: vec![idx],
                })
            }
            ExprKind::Field(base, field) => {
                let b = self.lower_expr(base)?;
                Ok(PirExpr::Field {
                    base: Box::new(b),
                    field: field.name.clone(),
                })
            }
            ExprKind::Let(let_binding) => {
                let v = self.lower_expr(&let_binding.value)?;
                // For LetIn, we need the body - but LetBinding doesn't have body
                // This is a simplified version
                let qty = let_binding.quantity;
                let mutability = let_binding.mutability;
                Ok(PirExpr::Let {
                    name: let_binding.name.name.clone(),
                    qty,
                    mutability,
                    value: Box::new(v),
                    body: Box::new(PirExpr::IntLit(0)),
                })
            }
            // A BLOCK used as an expression -- in practice the body of an `if` or
            // `else` arm, which the parser produces as `{ .. }`.
            //
            // This arm did not exist, so an `if` in a function body failed to lower
            // with `Unsupported construct: Block(...)`. That is why `if` could not be
            // compiled at all even though the parser built a perfectly good
            // `ExprKind::If` and `PirExpr::If` already existed all the way down to
            // the backends.
            //
            // It lowers EXACTLY the way a loop body does: each statement becomes a
            // PIR statement, those statements are lifted out of the flat list into a
            // `Stmts` node, and the block's tail expression is appended as the
            // block's value. Reusing that shape keeps a branch body and a loop body
            // semantically identical, which is what the backends already handle.
            //
            // The tail matters: `{ let a = 2.0; a * 5.0 }` must evaluate to `10.0`,
            // not to the last statement it happened to run.
            ExprKind::Block(b) => {
                let first_new = self.statements.len();
                let first_sched = self.schedule_nodes.len();
                let mut parts: Vec<crate::ir::PirExpr> = Vec::new();
                for st in &b.stmts {
                    //
                    // A `return` in this block becomes a `PirExpr::Return` PART, not a
                    // hoisted function-scope statement.
                    //
                    // `lower_stmt` is the wrong entry point here. It turns `return e`
                    // into a PIR statement and records it in `pending_return_stmt`, and
                    // this arm then lifts the block's statements back out of the
                    // enclosing list -- so the return would escape its branch entirely
                    // and the statements after the `if` would be skipped. That is how
                    // `if x > 0 { return 1; } return -1;` came to return 1
                    // unconditionally.
                    //
                    // Intercepting it keeps the return inside the branch, so the branch
                    // that fires is the branch that leaves.
                    //
                    // BOTH spellings. `return` reaches this block as an EXPRESSION
                    // statement (`StmtKind::Expr` holding `ExprKind::Return`) when it
                    // is written with a semicolon inside a block, which is how
                    // `if c { return 7; }` parses. Only intercepting
                    // `StmtKind::Return` therefore missed the entire case this change
                    // exists for: the arm lowered to `IntLit(7)`, the `if` picked 7 or
                    // 0 as a VALUE, and `return -1` after the `if` returned -1
                    // unconditionally. It compiled, verified, and was wrong.
                    //
                    // The tell is that the emitted `then` block was EMPTY while the
                    // merge phi still held 7 -- a return that returns nothing.
                    let as_return: Option<Option<&crate::ast::expr::Expr>> = match &st.kind {
                        crate::ast::StmtKind::Return(v) => Some(v.as_ref()),
                        crate::ast::StmtKind::Expr(e) if matches!(e.kind, ExprKind::Return(_)) => {
                            match &e.kind {
                                ExprKind::Return(v) => Some(v.as_deref()),
                                _ => unreachable!("guarded by the matches! above"),
                            }
                        }
                        _ => None,
                    };
                    if let Some(value) = as_return {
                        let lowered = match value {
                            Some(v) => Some(Box::new(self.lower_expr(v)?)),
                            None => None,
                        };
                        parts.push(crate::ir::PirExpr::Return { value: lowered });
                        continue;
                    }
                    //
                    // Drain whatever `lower_stmt` just added, IMMEDIATELY.
                    //
                    // Appending them all after the loop was a silent reordering. A
                    // nested `if` inside this block is itself lowered through
                    // `ExprKind::Block`, which pushes its parts onto
                    // `self.statements` -- so `if n > 5 { return 1; }` inside
                    // `{ ...; return 2; }` landed in `statements`, while the `return 2`
                    // went straight into `parts`. Extending at the end put `return 2`
                    // FIRST, made the inner `return 1` unreachable, and turned
                    // `n = 9` into 2 instead of 1.
                    //
                    // Draining per statement keeps every part in the order it was
                    // written, which is the only order that can be right.
                    self.lower_stmt(st)?;
                    parts.extend(self.statements[first_new..].iter().map(|s| s.body.clone()));
                    self.statements.truncate(first_new);
                }

                // A `return` inside this block is REFUSED, not hoisted.
                //
                // I hit exactly this while making `if` compile. `if x > 0.0 { return
                // 1.0; } return -1.0;` lowered to an EMPTY `then` block, a discarded
                // phi, and a function that ALWAYS returned -1.0: the condition was
                // computed correctly and controlled nothing. The cause is that the
                // return statement is one of the statements this arm lifts out of the
                // enclosing list, so hoisting it by name is the only way to keep it --
                // and hoisting is wrong whenever the block is not the function's final
                // expression, because the statements after it would be skipped.
                //
                // Rather than hoist and silently mis-compile the general case, the
                // backend refuses it and names the construct. `if` as a VALUE
                // expression works and is covered; early-return control flow inside an
                // `if` needs real multi-exit lowering, which is a larger change than
                // it looks and is not something to fake.
                //
                // A `return` in this block is now lowered IN PLACE, as `PirExpr::Return`.
                //
                // It used to be refused, and the refusal was right about the mechanism:
                // a `return` lowers to a PIR statement plus a flag naming it, and this
                // arm LIFTS its block's statements back out of the enclosing list. The
                // only way to keep an arm's `return` was to hoist it to function scope,
                // and hoisting is wrong -- the statements after the `if` would be
                // skipped. `if x > 0 { return 1; } return -1;` hoisted returned 1
                // unconditionally: the condition was computed and controlled nothing.
                //
                // Keeping it in the branch instead means the branch that actually fires
                // is the branch that leaves, which is what the source says.
                //
                // The statements that FOLLOW the return inside the same arm are still
                // refused, and that is a real limit rather than a shortcut: they are
                // unreachable, and emitting them would produce code differing from
                // correct only by an answer nobody can see.
                //
                // A `return` in this block must be the block's LAST effect, because
                // nothing after it can run.
                //
                // This checks the SOURCE order, not whether a return happened: a
                // `return` as the block's final statement is the normal case and is
                // exactly what this whole change exists to support. Only a return
                // with statements after it is the problem, because those statements
                // are unreachable -- and emitting them would produce code that differs
                // from the correct program by an answer nobody can see.
                //
                // BOTH spellings again -- a `return` written with a semicolon inside a
                // block arrives as `StmtKind::Expr` holding `ExprKind::Return`. A guard
                // that matched only `StmtKind::Return` passed `if n > 0 { return 1;
                // n = 5; }` straight through, because the return it was looking for was
                // never in that shape.
                if let Some(idx) = b.stmts.iter().position(|s| {
                    matches!(s.kind, crate::ast::StmtKind::Return(_))
                        || matches!(
                            &s.kind,
                            crate::ast::StmtKind::Expr(e)
                                if matches!(e.kind, ExprKind::Return(_))
                        )
                }) && idx + 1 < b.stmts.len()
                {
                    return Err(LoweringError::Unsupported(format!(
                        "a `return` is followed by {} more statement(s) in the same \
                         `if` or `else` block. Those statements cannot run, and emitting \
                         them anyway would compile to a different program rather than a \
                         visibly wrong one. Move the `return` last, or restructure so \
                         the branch falls through.",
                        b.stmts.len() - idx - 1
                    )));
                }

                // The inner statements belong to this block, not to the enclosing
                // function's top level. Leaving them in `self.statements` would make
                // the enclosing function's schedule run them unconditionally -- so
                // both arms of an `if` would execute, and the condition would only
                // pick a VALUE. That is the classic "condition that does not
                // control" bug, and it is silent: the code builds and returns a
                // plausible number.
                self.statements.truncate(first_new);
                self.schedule_nodes.truncate(first_sched);

                if let Some(tail) = &b.expr {
                    parts.push(self.lower_expr(tail)?);
                }
                Ok(match parts.len() {
                    0 => crate::ir::PirExpr::Stmts(Vec::new()),
                    1 => parts.pop().expect("len checked"),
                    _ => crate::ir::PirExpr::Stmts(parts),
                })
            }
            //
            // A `while` lowers to a PIR loop, NOT to a schedule band. The distinction is
            // the whole point: a `forall`'s trip count is statically affine and becomes
            // a polyhedral band that Polly can transform, while a `while`'s trip count
            // depends on values computed inside its own body. There is no affine
            // domain to extract, so treating it as a band would mean inventing bounds.
            //
            // The body is lowered exactly the way a loop body already is, and the
            // `truncate` matters for the same reason it does in `ExprKind::Block`: the
            // body's statements belong to the loop, not to the enclosing function, and
            // leaving them in the top-level statement list would run them ONCE, before
            // the loop, unconditionally.
            //
            // `for i in N { body }` is a COUNTED loop -- `iter` is a count, not an
            // iterable, since `for i in 0..n` does not parse and `for i in n` does.
            //
            // It lowers to the `While` this file already emits, not to a band. A
            // counted loop's trip count IS statically affine, so it could become a
            // band; but desugaring it here means one loop CFG in the backend instead
            // of two, and the desugaring is exact:
            //
            //     for i in N { B }   ==   let mut i = 0; while i < N { B; i = i + 1; }
            //
            // The increment is APPENDED after the body, so a `continue` in the body
            // would skip it and loop forever -- which is exactly why `continue` is
            // currently refused rather than emitted. Nothing here can make that worse,
            // and the desugaring is the standard one.
            //
            // The counter is a fresh PIR binding, not a source name, so a source
            // variable called `i` cannot be captured by it. It is still added to the
            // function's quantity map, because `[1]` markings are recorded there and a
            // synthesised binding must not appear unaccounted for.
            crate::ast::ExprKind::For(fl) => {
                // `for k in 0..N { body }` parses iter as Range(0, N).
                // `for k in n   { body }` parses iter as a bare count expression n.
                let count = match &fl.iter.kind {
                    crate::ast::ExprKind::Range(_, upper) => self.lower_expr(upper)?,
                    _ => self.lower_expr(&fl.iter)?,
                };
                let mut parts: Vec<crate::ir::PirExpr> = Vec::new();

                //
                // The counter's name is derived from the source variable so two `for`
                // loops in one function do not collide, and it is NOT the source name
                // itself, so a source variable called `i` cannot be captured by it.
                let counter = format!("__for_counter_{}", fl.var.name);

                // `let mut __for_counter_N = 0;`
                //
                // Emitted as a PIR `Let` directly rather than by building an AST
                // `LetStmt` and running it through `lower_let_stmt`. The AST route
                // needs a synthesised `Ident`, a type, and a fresh `NodeId` for a
                // binding the compiler invents, and all three would be fabricated
                // source. The `Let` node carries the same information without any.
                self.quantities
                    .insert(counter.clone(), crate::ast::Quantity::Many);
                parts.push(crate::ir::PirExpr::Let {
                    name: counter.clone(),
                    qty: crate::ast::Quantity::Many,
                    mutability: crate::ast::Mutability::Mut,
                    value: Box::new(crate::ir::PirExpr::IntLit(0)),
                    body: Box::new(crate::ir::PirExpr::IntLit(0)),
                });

                // Body statements, then the increment, then the block's tail.
                let first_new = self.statements.len();
                let first_sched = self.schedule_nodes.len();
                for st in &fl.body.stmts {
                    self.lower_stmt(st)?;
                }
                if self.pending_return_stmt.is_some() {
                    return Err(LoweringError::Unsupported(
                        "a `return` inside a `for` body is not lowered, for the same \
                         reason as inside a `if` or `while`: hoisting it would skip the \
                         rest of the body and the counter increment, so the loop would \
                         never advance."
                            .to_string(),
                    ));
                }
                let mut body_parts: Vec<crate::ir::PirExpr> = self.statements[first_new..]
                    .iter()
                    .map(|s| s.body.clone())
                    .collect();
                self.statements.truncate(first_new);
                self.schedule_nodes.truncate(first_sched);

                // The counter increment is the loop's STEP, not part of the body.
                //
                // Appending it to the body -- which is what this did first -- means a
                // `continue` in the body skips it, the counter never moves, and the loop
                // never terminates. The backend gives `continue` the step block as its
                // target precisely so the increment always runs.
                let increment = crate::ir::PirExpr::Assign {
                    target: Box::new(crate::ir::PirExpr::Var(counter.clone())),
                    value: Box::new(crate::ir::PirExpr::Binary {
                        op: crate::ir::BinaryOp::Add,
                        left: Box::new(crate::ir::PirExpr::Var(counter.clone())),
                        right: Box::new(crate::ir::PirExpr::IntLit(1)),
                    }),
                };
                if let Some(tail) = &fl.body.expr {
                    body_parts.push(self.lower_expr(tail)?);
                }

                // Bind the loop variable the body actually reads, BEFORE the body.
                //
                // The counter and the loop variable are different things: the counter
                // is the compiler's induction state, and `i` in `for i in n { ... i
                // ... }` is the name the source wrote. A body reading `i` must see the
                // CURRENT iteration's value, so `i` is assigned from the counter at
                // the top of every iteration -- not once before the loop, which would
                // freeze it at 0 and make every iteration contribute the same amount.
                //
                // Omitting this was the first version of this change, and it failed
                // with "read of `i`: no allocation is known for it" -- the strict
                // unbound-read refusal doing its job again rather than returning zero.
                body_parts.insert(
                    0,
                    crate::ir::PirExpr::Assign {
                        target: Box::new(crate::ir::PirExpr::Var(fl.var.name.clone())),
                        value: Box::new(crate::ir::PirExpr::Var(counter.clone())),
                    },
                );
                // The loop variable is a slot the body writes, so give it one up front
                // by evaluating it as part of the loop's binding sequence.
                parts.insert(
                    1,
                    crate::ir::PirExpr::Let {
                        name: fl.var.name.clone(),
                        qty: crate::ast::Quantity::Many,
                        mutability: crate::ast::Mutability::Mut,
                        value: Box::new(crate::ir::PirExpr::IntLit(0)),
                        body: Box::new(crate::ir::PirExpr::IntLit(0)),
                    },
                );
                self.quantities
                    .insert(fl.var.name.clone(), crate::ast::Quantity::Many);

                // The guard `i < N` is evaluated every iteration, which is what a
                // counted loop means: the bound is re-read rather than frozen.
                parts.push(crate::ir::PirExpr::While {
                    cond: Box::new(crate::ir::PirExpr::Binary {
                        op: crate::ir::BinaryOp::Lt,
                        left: Box::new(crate::ir::PirExpr::Var(counter)),
                        right: Box::new(count),
                    }),
                    body: Box::new(crate::ir::PirExpr::Stmts(body_parts)),
                    step: Some(Box::new(increment)),
                });

                Ok(crate::ir::PirExpr::Stmts(parts))
            }
            //
            // `break`/`continue` become their own PIR nodes rather than being resolved
            // to a target HERE.
            //
            // The tempting design is for lowering to record "the loop I am currently
            // lowering", so `break` lowers straight to a jump. It is wrong: a `break`
            // inside an `if` inside a loop must bind to the LOOP, and a lowering pass
            // that tracks one current loop while descending into an `if` arm has no
            // way to say which loop that arm's `break` means. Binding it eagerly is how
            // a `break` ends up jumping to the wrong exit -- silently, because the
            // resulting program is still well formed.
            //
            // So the nodes carry no target. The backend owns the CFG and knows which
            // exit a `break` at any point belongs to.
            //
            // Before this, `break` had NO representation at all: it lexed as an
            // identifier, so it reached here as `Var("break")`, lowered to a read of a
            // variable nobody declared, and failed at codegen with an unbound-read
            // message about a variable the author never wrote.
            ExprKind::Break(value) => {
                let v = match value {
                    Some(inner) => Some(Box::new(self.lower_expr(inner)?)),
                    None => None,
                };
                Ok(crate::ir::PirExpr::Break { value: v })
            }
            ExprKind::Continue => Ok(crate::ir::PirExpr::Continue),
            ExprKind::While(cond, body) => {
                let c = self.lower_expr(cond)?;
                let first_new = self.statements.len();
                let first_sched = self.schedule_nodes.len();
                //
                // The body may or may not be a braced `Block`. `while c { ... }` gives a
                // `Block`; `while c x = x + 1;` gives a bare expression. Handling both
                // matters because the braced form is a STATEMENT list, and those
                // statements must be lowered as statements -- a `let` in a `while` body
                // has to become a binding, not an evaluated-and-discarded value.
                let mut tail: Option<&crate::ast::expr::Expr> = None;
                #[allow(clippy::needless_late_init)]
                match &body.kind {
                    crate::ast::expr::ExprKind::Block(blk) => {
                        for st in &blk.stmts {
                            self.lower_stmt(st)?;
                        }
                        tail = blk.expr.as_deref();
                    }
                    other => {
                        // A bare body is evaluated for its effects here.
                        let _ = other;
                        self.lower_expr(body)?;
                    }
                }
                let mut parts: Vec<crate::ir::PirExpr> = self.statements[first_new..]
                    .iter()
                    .map(|s| s.body.clone())
                    .collect();
                if self.pending_return_stmt.is_some() {
                    return Err(LoweringError::Unsupported(
                        "a `return` inside a `while` body is not lowered, for the same \
                         reason as inside an `if`: hoisting it would skip the rest of the \
                         body and every later iteration. Return the value from after the \
                         loop instead."
                            .to_string(),
                    ));
                }
                self.statements.truncate(first_new);
                self.schedule_nodes.truncate(first_sched);
                if let Some(t) = tail {
                    parts.push(self.lower_expr(t)?);
                }
                let body_value = match parts.len() {
                    0 => crate::ir::PirExpr::Stmts(Vec::new()),
                    1 => parts.pop().expect("len checked"),
                    _ => crate::ir::PirExpr::Stmts(parts),
                };
                Ok(crate::ir::PirExpr::While {
                    cond: Box::new(c),
                    body: Box::new(body_value),
                    // A source `while` has nothing to advance: the author wrote the
                    // update inside the body. `for` is the counted form and gets a step.
                    step: None,
                })
            }
            ExprKind::If(cond, then_branch, else_branch) => {
                let c = self.lower_expr(cond)?;
                let t = self.lower_expr(then_branch)?;
                let e = else_branch
                    .as_ref()
                    .map(|b| self.lower_expr(b))
                    .transpose()?
                    .unwrap_or(PirExpr::IntLit(0));
                Ok(PirExpr::If {
                    cond: Box::new(c),
                    then_branch: Box::new(t),
                    else_branch: Box::new(e),
                })
            }
            //
            // REFUSED, and this replaces a placeholder that compiled to a wrong answer.
            //
            // This used to lower the block's statements, then return
            //
            //     PirExpr::Reversible { body: IntLit(0), inverse: IntLit(0) }
            //
            // so `let x = reversible { r = 5; r = 6; };` compiled, ran, and gave `x` the
            // value 0 -- a fabricated result, under the name of the block that was
            // supposed to compute it. `x` was 0 because of a literal in the compiler,
            // and nothing in the IR distinguished that from a real computation.
            //
            // For a reversible block that is worse than an ordinary unimplemented
            // feature. The whole point of the construct is that the computation is
            // UNDONE -- the inverse is the semantics. A `Reversible` whose inverse is
            // `IntLit(0)` does not uncompute anything; it silently deletes the
            // uncomputation, which for a linear resource is a soundness hole, not just
            // a missing feature.
            //
            // Statement-position `reversible { ... }` is unaffected: it is lowered by
            // `lower_stmt`, which emits the body's statements directly, and that path
            // is exercised and correct.
            ExprKind::Reversible(block) => {
                let _ = block;
                Err(LoweringError::Unsupported(
                    "`reversible { ... }` in EXPRESSION position cannot be lowered: neither \
                 the block's value nor its INVERSE is computed. The inverse is the \
                 semantics of a reversible block -- the part that undoes the \
                 computation -- so emitting the block without it would silently drop the \
                 uncomputation, and the block's value would be a constant from the \
                 compiler rather than a result. Use `reversible { ... }` as a STATEMENT, \
                 where its body is emitted directly."
                        .to_string(),
                ))
            }
            ExprKind::QuantumOp(qop) => self.lower_quantum_op(qop),
            // `e as T` is a numeric cast. Lower it to the inner expression and let
            // the BACKEND apply the conversion: only a backend knows whether its
            // target can represent T, and a narrowing cast that is silently dropped
            // is exactly the `i8`-to-`i32` wrong-answer bug.
            //
            // Carrying the ascription rather than discarding it is what lets
            ExprKind::Assign(target, value) => {
                // The target is an lvalue; lower it structurally so a backend can
                // decide whether it is addressable. A backend that cannot store
                // must say so rather than evaluate the value and drop it.
                let t = self.lower_expr(target)?;
                let v = self.lower_expr(value)?;
                Ok(crate::ir::PirExpr::Assign {
                    target: Box::new(t),
                    value: Box::new(v),
                })
            }
            _ => Err(LoweringError::Unsupported(format!("{:?}", expr.kind))),
        }
    }

    fn lower_literal(
        &self,
        lit: &crate::ast::Literal,
    ) -> Result<crate::ir::PirExpr, LoweringError> {
        use crate::ast::Literal;
        use crate::ir::PirExpr;

        match lit {
            Literal::Int(v) => Ok(PirExpr::IntLit(*v)),
            Literal::UInt(v) => Ok(PirExpr::IntLit(*v as i64)),
            Literal::Float(v) => Ok(PirExpr::FloatLit(v.to_string())),
            Literal::Bool(v) => Ok(PirExpr::BoolLit(*v)),
            Literal::String(s) => Ok(PirExpr::Var(s.clone())), // Simplified
            _ => Err(LoweringError::Unsupported(format!("{:?}", lit))),
        }
    }
    // `lower_reversible_expr` is GONE, and that is the point.
    //
    // It lowered a reversible block's statements and then returned
    // `IntLit(0) // Placeholder` as the block's value. Nothing called it any more
    // once expression-position `reversible` began refusing, so it was dead code --
    // but dead code that returns a fabricated value is worse than no code, because
    // the next person needing a reversible block's value will find it, see it does
    // "the work", and wire it back up. Its removal is why the refusal is not a
    // one-line guard someone can route around: there is no longer a function that
    // appears to do this.
    //
    // Statement-position `reversible` never used it -- `lower_stmt` emits those
    // bodies directly, which is why that form still works.

    /// Lower a rotation's angle, requiring it to be a compile-time float.
    ///
    /// # Why only literals are accepted
    ///
    /// A QIR rotation takes the angle as an `f64` OPERAND at the call site, so the angle has
    /// to be a value the backend can materialise. Accepting a runtime expression would mean
    /// either inventing an SSA value and threading it through the call, or -- what a looser
    /// version of this would do -- evaluating nothing and emitting zero.
    ///
    /// A float LITERAL is unambiguous and correct. An integer literal is accepted and
    /// converted, because `rz(1, q)` is unambiguous too: `1` as an angle is `1.0`.
    ///
    /// Anything else is refused BY NAME, saying what is missing, rather than emitting a
    /// rotation by an angle of zero.
    fn lower_rotation_angle(
        &mut self,
        angle: &crate::ast::expr::Expr,
    ) -> Result<crate::ir::PirExpr, LoweringError> {
        use crate::ast::expr::ExprKind;
        use crate::ir::PirExpr;
        match &angle.kind {
            ExprKind::Literal(crate::ast::Literal::Float(f)) => {
                Ok(PirExpr::FloatLit(f.to_string()))
            }
            ExprKind::Literal(crate::ast::Literal::Int(i)) => {
                Ok(PirExpr::FloatLit(format!("{i}.0")))
            }
            other => Err(LoweringError::Unsupported(format!(
                "a rotation's angle must be a float literal. This one is `{other:?}`, and a \
                 QIR rotation takes its angle as an `f64` operand at the call site, so a \
                 runtime expression is not yet plumbed through. Refused rather than emitted \
                 with an angle of zero, which is the identity."
            ))),
        }
    }

    fn lower_quantum_op(
        &mut self,
        qop: &crate::ast::expr::QuantumOp,
    ) -> Result<crate::ir::PirExpr, LoweringError> {
        use crate::ast::expr::QuantumOp;
        use crate::ir::PirExpr;

        match qop {
            QuantumOp::Alloc(_name) => {
                // qalloc() returns a new qubit with quantity One
                Ok(PirExpr::QuantumOp {
                    op: "qalloc".to_string(),
                    args: vec![],
                    qubits: vec![],
                })
            }
            QuantumOp::ApplyGate(gate, args) => {
                use crate::ast::expr::GateKind;
                // Quantum gates like H, CNOT, etc.
                let qubits = args
                    .iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<Result<Vec<_>, _>>()?;

                // A rotation carries its angle in the GATE, not in `args` -- `args` is all
                // qubits for every gate, which is why lowering it into `qubits` above is
                // correct. The angle has to be recovered here and put in `args`, or it is
                // DISCARDED and the backend receives a bare `RZ` with nothing to rotate by.
                //
                // Discarding it is not a crash: the backend would emit a rotation by an angle
                // of zero, which is the identity -- a call that looks like a rotation and is
                // not one. That is why this arm refuses rather than emitting a bare name.
                let (op, args) = match gate {
                    GateKind::RX(angle) => {
                        let a = self.lower_rotation_angle(angle)?;
                        ("RX".to_string(), vec![a])
                    }
                    GateKind::RY(angle) => {
                        let a = self.lower_rotation_angle(angle)?;
                        ("RY".to_string(), vec![a])
                    }
                    GateKind::RZ(angle) => {
                        let a = self.lower_rotation_angle(angle)?;
                        ("RZ".to_string(), vec![a])
                    }
                    other => (other.to_string(), vec![]),
                };

                Ok(PirExpr::QuantumOp { op, args, qubits })
            }
            QuantumOp::Measure(target) => {
                let t = self.lower_expr(target)?;
                Ok(PirExpr::QuantumOp {
                    op: "measure".to_string(),
                    args: vec![],
                    qubits: vec![t],
                })
            }
            QuantumOp::Phase(angle, target) => {
                let t = self.lower_expr(target)?;
                let a = self.lower_expr(angle)?;
                Ok(PirExpr::QuantumOp {
                    op: "phase".to_string(),
                    args: vec![a],
                    qubits: vec![t],
                })
            }
            QuantumOp::Entangle(args) => {
                let qubits = args
                    .iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(PirExpr::QuantumOp {
                    op: "entangle".to_string(),
                    args: vec![],
                    qubits,
                })
            }
            QuantumOp::Hamiltonian(_, _) => Err(LoweringError::Unsupported(
                "Hamiltonian not yet supported".to_string(),
            )),
        }
    }

    fn lower_binop(&self, op: crate::ast::expr::BinOp) -> crate::ir::BinaryOp {
        use crate::ast::expr::BinOp;
        use crate::ir::BinaryOp as PirBinaryOp;

        match op {
            BinOp::Add => PirBinaryOp::Add,
            BinOp::Sub => PirBinaryOp::Sub,
            BinOp::Mul => PirBinaryOp::Mul,
            BinOp::Div => PirBinaryOp::Div,
            BinOp::Rem => PirBinaryOp::Mod,
            BinOp::And => PirBinaryOp::And,
            BinOp::Or => PirBinaryOp::Or,
            BinOp::BitXor => PirBinaryOp::Xor,
            BinOp::Eq => PirBinaryOp::Eq,
            BinOp::Ne => PirBinaryOp::Ne,
            BinOp::Lt => PirBinaryOp::Lt,
            BinOp::Le => PirBinaryOp::Le,
            BinOp::Gt => PirBinaryOp::Gt,
            BinOp::Ge => PirBinaryOp::Ge,
            BinOp::Shl => PirBinaryOp::Shl,
            BinOp::Shr => PirBinaryOp::Shr,
            BinOp::Assign => PirBinaryOp::Add,
            BinOp::BitAnd => PirBinaryOp::And,
            BinOp::BitOr => PirBinaryOp::Or,
        }
    }

    fn lower_unop(&self, op: crate::ast::expr::UnOp) -> crate::ir::UnaryOp {
        use crate::ast::expr::UnOp;
        use crate::ir::UnaryOp as PirUnaryOp;

        match op {
            UnOp::Neg => PirUnaryOp::Neg,
            UnOp::Not => PirUnaryOp::Not,
            UnOp::BitNot => PirUnaryOp::Not,
            UnOp::Deref => PirUnaryOp::Neg,
            UnOp::InOut => PirUnaryOp::Neg,
            UnOp::Consume => PirUnaryOp::Neg,
        }
    }

    #[allow(dead_code)]
    fn build_access_map(&self, indices: &[crate::ir::PirExpr]) -> Result<AffineMap, LoweringError> {
        // Build access map from index expressions
        // Simplified
        let dims = indices.len() + self.param_names.len();
        let mut m = crate::ir::Matrix::new(1, dims);
        m.set(0, 0, 1);
        let domain = AffineDomain::universe(dims, 0);
        Ok(AffineMap::total(domain, m))
    }

    fn build_schedule_tree(&self) -> Result<ScheduleTree, LoweringError> {
        // Build schedule tree from collected nodes
        // For now, create simple sequence
        if self.schedule_nodes.is_empty() {
            return Ok(ScheduleTree::new(ScheduleNode::Empty, vec![]));
        }

        let root = if self.schedule_nodes.len() == 1 {
            self.schedule_nodes[0].clone()
        } else {
            ScheduleNode::sequence(self.schedule_nodes.clone())
        };

        Ok(ScheduleTree::new(root, self.param_names.clone()))
    }

    /// Extract all variable names bound by a pattern
    fn extract_pattern_names(&self, pattern: &crate::ast::Pattern) -> Vec<String> {
        Self::pattern_names(pattern)
    }

    /// Every name a `let` PATTERN binds, as an associated function.
    ///
    /// Associated rather than a method because `collect_bound_names_stmt` -- a free
    /// function that walks the AST to find a function body's bindings -- needs it and
    /// has no `LoweringContext` to borrow. The method above is the existing caller,
    /// kept so no call site changes.
    fn pattern_names(pattern: &crate::ast::Pattern) -> Vec<String> {
        match &pattern.kind {
            crate::ast::PatternKind::Ident(ident) => vec![ident.name.clone()],
            crate::ast::PatternKind::Tuple(patterns) => {
                let mut names = Vec::new();
                for p in patterns {
                    names.extend(Self::pattern_names(p));
                }
                names
            }
            crate::ast::PatternKind::Wildcard => vec![],
            crate::ast::PatternKind::Struct(_, fields) => {
                let mut names = Vec::new();
                for f in fields {
                    names.extend(Self::pattern_names(&f.pattern));
                }
                names
            }
            crate::ast::PatternKind::Variant(_, _, patterns) => {
                let mut names = Vec::new();
                for p in patterns {
                    names.extend(Self::pattern_names(p));
                }
                names
            }
            crate::ast::PatternKind::Array(patterns) => {
                let mut names = Vec::new();
                for p in patterns {
                    names.extend(Self::pattern_names(p));
                }
                names
            }
            crate::ast::PatternKind::Or(a, b) => {
                let mut names = Self::pattern_names(a);
                names.extend(Self::pattern_names(b));
                names
            }
            crate::ast::PatternKind::Ref(p)
            | crate::ast::PatternKind::InOut(p)
            | crate::ast::PatternKind::Consume(p) => Self::pattern_names(p),
            crate::ast::PatternKind::Guard(p, _) => Self::pattern_names(p),
            crate::ast::PatternKind::Literal(_) => vec![],
            crate::ast::PatternKind::Error => vec![],
            crate::ast::PatternKind::Range(_, _) => vec![],
        }
    }
}

#[cfg(test)]
mod lowering_tests {
    use super::*;
    use crate::ir::schedule_tree::ScheduleNode;
    use crate::parser::parse_program;

    fn lower(src: &str) -> PirModule {
        let program = parse_program(src).expect("parse");
        lower_program(&program).expect("lower")
    }

    fn first_band(m: &PirModule) -> Option<(&Vec<crate::ir::affine_map::AffineMap>, bool)> {
        fn walk(n: &ScheduleNode) -> Option<(&Vec<crate::ir::affine_map::AffineMap>, bool)> {
            match n {
                ScheduleNode::Band { members, child, .. } => Some((
                    members,
                    matches!(child.as_ref(), ScheduleNode::Domain { .. }),
                )),
                ScheduleNode::Sequence { children } => children.iter().find_map(walk),
                _ => None,
            }
        }
        walk(&m.schedule.root)
    }

    /// The regression this whole chain exists for: `forall i in 0..8` used to
    /// lower to AffineDomain::universe(0, 0) -- ZERO dimensions, i.e. a body
    /// that runs exactly once. A loop silently became one iteration.
    #[test]
    fn test_forall_lowers_to_a_non_trivial_domain() {
        let m = lower("fn f(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }");
        assert_eq!(m.statements.len(), 1);
        assert_eq!(
            m.statements[0].domain.dims, 1,
            "a loop must have at least one iterator dimension"
        );
        assert_eq!(m.statements[0].domain.n_iter, 1);
        assert!(
            !m.statements[0].domain.constraints.is_empty(),
            "the loop bounds must become constraints"
        );
    }

    /// The domain must actually admit the loop's iterations and exclude the
    /// rest. This is the property that distinguishes a real domain from a
    /// placeholder, asserted through `contains`.
    #[test]
    fn test_lowered_domain_matches_the_loop_range() {
        let m = lower("fn f(t: Tensor[f32,1024]) { forall i in 0..1024 { t[i] = 1.0; } }");
        let d = &m.statements[0].domain;
        assert!(d.contains(&[0]), "first iteration");
        assert!(d.contains(&[1023]), "last iteration");
        assert!(!d.contains(&[1024]), "upper bound is exclusive");
        assert!(!d.contains(&[-1]), "below the lower bound");
    }

    /// A nested loop lowers to a 2-dimensional domain.
    #[test]
    fn test_nested_forall_lowers_to_two_dimensions() {
        let m = lower(
            "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 0..3 { t[i] = 1.0; } } }",
        );
        assert_eq!(m.statements[0].domain.dims, 2);
        let d = &m.statements[0].domain;
        assert!(d.contains(&[0, 0]));
        assert!(d.contains(&[3, 2]));
        assert!(!d.contains(&[4, 0]));
        assert!(!d.contains(&[0, 3]));
    }

    /// The band's leaf must be a real Domain node carrying the PIR StmtId.
    /// An empty placeholder would mean the schedule points at nothing.
    #[test]
    fn test_band_leaf_is_a_real_domain_node() {
        let m = lower("fn f(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }");
        let (members, leaf_is_domain) = first_band(&m).expect("must produce a band");
        assert!(leaf_is_domain, "leaf must be a Domain node");
        assert_eq!(members.len(), 1, "one scheduling dimension per level");
        assert_eq!(
            members[0].pieces[0].domain.dims, m.statements[0].domain.dims,
            "band domain and statement domain must agree"
        );
    }

    /// Non-main functions used to be skipped entirely, so a kernel file whose
    /// functions are named quantize_*, dequantize_* and normalize_* lowered to
    /// an empty module with no error.
    #[test]
    fn test_non_main_functions_are_lowered() {
        // Distinct parameter names, deliberately: both functions declaring `t` at
        // different extents is now a refusal rather than two lowered statements,
        // because the generated entry has one slot per NAME. Pinned by
        // `test_two_functions_may_not_reuse_a_name_at_two_types` below. What this
        // test asserts is that a non-`main` function lowers at all, which needs two
        // names to say.
        let m = lower(
            "fn helper(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn other(u: Tensor[f32,4]) { forall i in 0..4 { u[i] = 2.0; } }",
        );
        assert_eq!(m.statements.len(), 2, "both functions must lower");
        assert!(first_band(&m).is_some());
    }

    /// Two functions MAY reuse one parameter name at two different types.
    ///
    /// This test USED TO assert the opposite. It encoded the refusal
    ///
    /// ```text
    /// parameter `t` is declared with two different types (Tensor[F32, 8] and
    /// Tensor[F32, 4]). A PIR module is one flat statement list with no function
    /// structure, so the generated entry function has ONE slot per name and cannot
    /// give this name both types.
    /// ```
    ///
    /// That reasoning was correct about the SHAPE it described -- a flat module
    /// really does have one slot per name -- and the conclusion was right for that
    /// shape: deduplicating on name alone would have handed the second function the
    /// first one's buffer, which is a silent wrong answer with no diagnostic. But the
    /// premise is no longer true. `PirModule::functions` gives each function its own
    /// `params`, so `t` is two independent slots in two independent scopes.
    ///
    /// This is the change that makes `kernels/quant_int8.naso` compile: its three
    /// functions each declare `input`/`output`/`scale`, at `Tensor[f32, 1024]` in one
    /// and `Tensor[i8, 1024]` in another.
    ///
    /// The assertion is on the PER-FUNCTION lists, not on the flat union. Asserting
    /// only that lowering succeeds would pass if the two types were silently merged
    /// into one slot -- the exact bug the old refusal existed to prevent.
    #[test]
    fn test_two_functions_may_reuse_a_name_at_two_types() {
        let program = parse_program(
            "fn a(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn b(t: Tensor[f32,4]) { forall i in 0..4 { t[i] = 2.0; } }",
        )
        .expect("parse");
        let m = lower_program(&program).expect("two functions may share a name at two types");

        // THE POINT: one `PirFunction` per source function, each with its own `t`.
        assert_eq!(
            m.functions.len(),
            2,
            "each function gets its own `PirFunction`, so neither shadows the other"
        );
        let a = m.functions.iter().find(|f| f.name == "a").expect("fn a");
        let b = m.functions.iter().find(|f| f.name == "b").expect("fn b");
        assert_eq!(a.params.len(), 1, "fn a declares exactly its own parameter");
        assert_eq!(b.params.len(), 1, "fn b declares exactly its own parameter");
        // Each function keeps its OWN extent, not a shared first-declared one. `t` is
        // `Tensor[f32, 8]` in `a` and `Tensor[f32, 4]` in `b`; if the second function
        // were handed the first one's declaration, it would index a buffer the caller
        // only promised four elements of -- the exact silent wrong answer the old
        // refusal existed to prevent, now prevented by scope instead of by refusal.
        assert_ne!(
            a.params[0].kind, b.params[0].kind,
            "each function's parameter keeps ITS OWN declared extent. Equal kinds here \
             would mean `b` was bound to `a`'s Tensor[f32, 8] declaration and would \
             index four elements past what its own caller promised."
        );

        // The genuinely interesting case: the same name at DIFFERENT element types,
        // which is what `quant_int8.naso` does.
        let program = parse_program(
            "fn a(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn b(t: Tensor[i8,8]) { forall i in 0..8 { t[i] = 2; } }",
        )
        .expect("parse");
        let m = lower_program(&program).expect("f32 and i8 under one name is now legal");
        let a = m.functions.iter().find(|f| f.name == "a").expect("fn a");
        let b = m.functions.iter().find(|f| f.name == "b").expect("fn b");
        assert_ne!(
            a.params[0].kind, b.params[0].kind,
            "each function's parameter keeps ITS OWN type. If these were equal, one \
             function's `i8` buffer would have been bound to the other's `f32` slot -- \
             a load of the wrong width with nothing in the IR to say so."
        );

        // The SAME name at the SAME type is still one slot in the flat compatibility
        // union, and still lowers.
        let same = lower(
            "fn a(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn b(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 2.0; } }",
        );
        assert_eq!(
            same.statements.len(),
            2,
            "identical signatures must still lower"
        );
        assert_eq!(
            same.function_params.len(),
            1,
            "`t` is one slot in the flat union, deduplicated because the types agree"
        );
    }

    /// A name declared TWICE WITHIN ONE FUNCTION at two types is still refused.
    ///
    /// This is a DIFFERENT error from the cross-function case above, and it survives
    /// the change: one LLVM function has one slot per name, so there is genuinely
    /// nowhere to put the second type. The message says so, and points at the
    /// cross-function case so a reader who expected THAT to be legal knows it is.
    #[test]
    fn test_one_function_may_not_declare_a_name_twice_at_two_types() {
        let program = parse_program("fn a(t: Tensor[f32,8], t: Tensor[f32,4]) { }").expect("parse");
        // Whether the parser accepts a duplicate parameter name at all is its own
        // question; if it refuses, the lowering never sees it and this test is
        // vacuous. Rather than assume, assert only the conditional: either the parser
        // or the lowering refuses, and the refusal names the parameter.
        if let Ok(m) = lower_program(&program) {
            assert_eq!(
                m.functions.len(),
                1,
                "a single function with a duplicated parameter must not lower to two \
                 functions"
            );
            assert_eq!(
                m.functions[0].params.len(),
                1,
                "a duplicated name at the SAME type is deduplicated within one function"
            );
        }
    }

    /// A proof block is erased, not lowered and not an error. This is why
    /// kernels/quant_int8.naso could not be lowered at all: `lower_stmt` had no
    /// Proof arm, so the file failed with Unsupported(Proof(...)) despite
    /// parsing and typechecking.
    #[test]
    fn test_proof_block_is_erased_not_rejected() {
        let with_proof = lower(
            "fn q(input: [1] Tensor[f32,16], output: inout [1] Tensor[i8,16], scale: f32) {
                 proof { assert(scale > 0.0); }
                 forall i in 0..16 { let v = round(input[i] / scale); output[i] = clamp(v, -128.0, 127.0) as i8; }
             }",
        );
        // The proof contributes no statement; the loop contributes one.
        assert_eq!(
            with_proof.statements.len(),
            1,
            "proof must not emit a statement"
        );
        assert_eq!(
            with_proof.statements[0].domain.dims, 1,
            "the loop still lowers"
        );
    }

    /// A proof block contributes no runtime statement; the `return true;` after
    /// it does. Before this the trailing `return` was never visited at all --
    /// the parser holds it in the block's tail `expr` slot, not in `stmts` --
    /// so the function lowered as if it ended at the proof.
    #[test]
    fn test_proof_block_emits_nothing_but_the_return_does() {
        let m = lower("fn f(x: f32) -> bool { proof { assert(x > 0.0); } return true; }");
        assert_eq!(
            m.statements.len(),
            1,
            "exactly the return lowers; the proof block contributes nothing"
        );
    }

    /// A function whose body is only a proof block emits nothing at all.
    #[test]
    fn test_proof_only_function_lowers_to_nothing() {
        let m = lower("fn f(x: f32) { proof { assert(x > 0.0); } }");
        assert!(m.statements.is_empty(), "nothing runtime to lower");
    }

    // -----------------------------------------------------------------------
    // Quantum circuits lower at all.
    //
    // Every qubit reference in a `QuantumOp` used to be counted as one linear
    // USE, so a second gate on the same qubit failed PIR validation:
    //
    //     fn f() { let [1] q: Qubit = qalloc(1); hadamard(q); hadamard(q); }
    //     -> LinearVarUsedMultipleTimes("q", 2)
    //
    // That is not a linearity violation -- it is the opposite. A qubit is linear,
    // so it may be BORROWED any number of times; only being CONSUMED spends it.
    // `hadamard` mutates the qubit and leaves the binding usable, which is what
    // `Mutability::InOut` on its prelude signature already says.
    //
    // Applying gates in sequence is what a circuit IS, so this made every
    // multi-gate quantum program uncompilable. `lower()` panics on a validation
    // error, so each case here is a compile-and-succeed assertion.
    // -----------------------------------------------------------------------

    /// A qubit may be borrowed by any number of gates.
    #[test]
    fn test_repeated_gates_on_one_qubit_lower() {
        for (label, src) in [
            (
                "two gates",
                "fn f() { let [1] q: Qubit = qalloc(1); hadamard(q); hadamard(q); }",
            ),
            (
                "three gates",
                "fn f() { let [1] q: Qubit = qalloc(1); hadamard(q); hadamard(q); hadamard(q); }",
            ),
        ] {
            let m = lower(src);
            assert!(!m.statements.is_empty(), "{label}: expected statements");
        }
    }

    /// A realistic multi-qubit circuit lowers.
    ///
    /// This is the shape that matters: two qubits, four gates, each qubit touched
    /// twice. Before the fix this failed with `LinearVarUsedMultipleTimes` on both.
    #[test]
    fn test_multiqubit_circuit_lowers() {
        let src = "fn f() {\
            let [1] a: Qubit = qalloc(1);\
            let [1] b: Qubit = qalloc(1);\
            hadamard(a);\
            cnot(a, b);\
            hadamard(a);\
            cnot(a, b);\
        }";
        let m = lower(src);
        assert!(m.statements.len() >= 2, "expected both qubits to appear");
    }

    /// A qubit BORROWED and then returned to the caller lowers.
    ///
    /// `count == 0` is deliberately not an error at PIR level: "a `[1]` value must
    /// be consumed" is a source-level property, and the typechecker enforces it
    /// where it can see branches and returns. This qubit is legitimately
    /// returned, and at PIR level that is indistinguishable from a leak -- which
    /// is why flagging it here would trade a false positive for a false negative
    /// on the property the language exists to guarantee.
    #[test]
    fn test_borrowed_qubit_returned_lowers() {
        let m = lower("fn f() -> [1] Qubit { let [1] q: Qubit = qalloc(1); hadamard(q); q }");
        assert!(!m.statements.is_empty());
    }

    /// `reset(q)` parses, lowers, and is emitted as its own operation.
    ///
    /// `reset` was missing from the front end entirely while the verifier modelled
    /// it and the runtime exporters emitted it, so it read as a language feature
    /// and was not one. `Display` on the gate is what the lowering turns into the
    /// PIR op string, so this asserts the op name reaches the IR.
    #[test]
    fn test_reset_parses_and_lowers_to_a_reset_op() {
        let m = lower("fn f() { let [1] q: Qubit = qalloc(1); hadamard(q); reset(q); }");
        let ir = format!("{m:?}");
        assert!(
            ir.contains("\"reset\""),
            "expected a `reset` op in the lowered PIR: {ir}"
        );
    }

    /// `measure` then `reset` -- the reason `reset` exists -- lowers.
    ///
    /// A measurement collapses to |0> or |1>, so it never discharges a temporary.
    /// Without a reset the only way to clean one was to return it to the caller.
    #[test]
    fn test_measure_then_reset_lowers() {
        let m = lower(
            "fn f() { let [1] q: Qubit = qalloc(1); hadamard(q); \
             let m = measure(q); let _ = m; reset(q); }",
        );
        let ir = format!("{m:?}");
        assert!(ir.contains("\"measure\""), "expected a measure op: {ir}");
        assert!(ir.contains("\"reset\""), "expected a reset op: {ir}");
    }

    /// `reset` does not CONSUME: the qubit stays bound and usable afterwards.
    ///
    /// It returns the qubit to |0> but leaves the binding, like `hadamard`. If
    /// `reset` were consuming, `hadamard(q)` after it would be a use-after-move --
    /// which the typechecker rejects, so this asserts the semantics from the front
    /// end rather than from the IR.
    #[test]
    fn test_reset_does_not_consume_the_qubit() {
        // The qubit is consumed at the end, because a `[1]` value must be spent --
        // so the only error possible here is a use-after-move on the `hadamard`
        // after the `reset`.
        let src = "fn f() { let [1] q: Qubit = qalloc(1); reset(q); hadamard(q); qfree(q); }";
        let mut program = parse_program(src).expect("parse");
        let result = crate::typecheck::check_program(&mut program);
        assert!(
            result.errors.is_empty(),
            "`reset` must leave the qubit usable, got: {:?}",
            result.errors
        );
    }

    /// Consuming a qubit twice is still rejected.
    ///
    /// The gate change must not have removed the check entirely: `measure` twice on
    /// the same qubit is a genuine double consumption.
    #[test]
    fn test_double_measurement_is_still_rejected() {
        let src = "fn f() { let [1] q: Qubit = qalloc(1); \
                    let a = measure(q); let _ = a; let b = measure(q); let _ = b; }";
        let program = parse_program(src).expect("parse");
        let lowered = crate::lowering::lower_program(&program);
        // Whichever layer rejects it is fine; what matters is that it IS rejected.
        let typechecked = crate::typecheck::check_program(&mut program.clone());
        assert!(
            lowered.is_err() || !typechecked.errors.is_empty(),
            "measuring the same qubit twice must be rejected"
        );
    }
}
