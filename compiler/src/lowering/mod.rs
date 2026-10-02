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
        // asks "what ABI does the generated function need?". It runs first so a
        // parameter declared with two conflicting types is reported before any
        // body is lowered against a signature that cannot represent it.
        self.extract_function_params(program)?;

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
    /// The list is the UNION over all functions because a `PirModule` holds one
    /// statement list with no function structure and the backend emits one entry
    /// function. A name bound by two functions with DIFFERENT types is recorded
    /// and the backend refuses it naming both, rather than one of them silently
    /// winning.
    fn extract_function_params(&mut self, program: &Program) -> Result<(), LoweringError> {
        for item in &program.items {
            let crate::ast::Item::Function(func) = item else {
                continue;
            };
            for param in &func.params {
                let kind = self.param_kind(&param.ty);
                let entry = crate::ir::pir_types::FunctionParam {
                    name: param.name.name.clone(),
                    kind,
                    quantity: param.quantity,
                    mutability: param.mutability,
                };
                if let Some(existing) = self.function_params.iter().find(|p| p.name == entry.name) {
                    if existing.kind != entry.kind {
                        return Err(LoweringError::Unsupported(format!(
                            "parameter `{}` is declared with two different types \
                             ({} and {}). A PIR module is one flat statement list \
                             with no function structure, so the generated entry \
                             function has ONE slot per name and cannot give this \
                             name both types.",
                            entry.name,
                            describe_param_kind(&existing.kind),
                            describe_param_kind(&entry.kind),
                        )));
                    }
                    continue;
                }
                self.function_params.push(entry);
            }
        }
        Ok(())
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
            crate::ast::ty::TypeKind::QRegister(_) => ParamKind::QRegister,
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
                for stmt in &func.body.stmts {
                    self.lower_stmt(stmt)?;
                }
                // A trailing expression is held on the block, NOT in `stmts`.
                //
                // The parser folds the last statement of a function body into
                // `Block::expr` when it is an expression, so
                // `fn f() { .. return true; }` puts that `return` in the tail
                // slot. Only iterating `stmts` skipped it, and because the tail
                // was simply never visited its `return` was never reported
                // either -- the function body lowered as if it ended earlier.
                if let Some(tail) = &func.body.expr {
                    self.lower_expr(tail)?;
                }
            }
            _ => {
                // Skip other items for now
            }
        }
        Ok(())
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

    fn lower_reversible_block(
        &mut self,
        block: &crate::ast::expr::ReversibleBlock,
    ) -> Result<(), LoweringError> {
        // Lower forward block body
        for stmt in &block.body.stmts {
            self.lower_stmt(stmt)?;
        }

        // Create schedule tree for reversible block
        // (simplified - full implementation in reversible_lowering.rs)
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
            ExprKind::Reversible(block) => {
                // Lower reversible block expression
                let b = self.lower_reversible_expr(block)?;
                // For now just return a placeholder
                Ok(PirExpr::Reversible {
                    body: Box::new(b),
                    inverse: Box::new(PirExpr::IntLit(0)),
                })
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

    fn lower_reversible_expr(
        &mut self,
        block: &crate::ast::expr::ReversibleBlock,
    ) -> Result<crate::ir::PirExpr, LoweringError> {
        // Lower the body of the reversible block
        for stmt in &block.body.stmts {
            self.lower_stmt(stmt)?;
        }
        Ok(crate::ir::PirExpr::IntLit(0)) // Placeholder
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
                // Quantum gates like H, CNOT, etc.
                let qubits = args
                    .iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(PirExpr::QuantumOp {
                    op: gate.to_string(),
                    args: vec![],
                    qubits,
                })
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
        match &pattern.kind {
            crate::ast::PatternKind::Ident(ident) => vec![ident.name.clone()],
            crate::ast::PatternKind::Tuple(patterns) => {
                let mut names = Vec::new();
                for p in patterns {
                    names.extend(self.extract_pattern_names(p));
                }
                names
            }
            crate::ast::PatternKind::Wildcard => vec![],
            crate::ast::PatternKind::Struct(_, fields) => {
                let mut names = Vec::new();
                for f in fields {
                    names.extend(self.extract_pattern_names(&f.pattern));
                }
                names
            }
            crate::ast::PatternKind::Variant(_, _, patterns) => {
                let mut names = Vec::new();
                for p in patterns {
                    names.extend(self.extract_pattern_names(p));
                }
                names
            }
            crate::ast::PatternKind::Array(patterns) => {
                let mut names = Vec::new();
                for p in patterns {
                    names.extend(self.extract_pattern_names(p));
                }
                names
            }
            crate::ast::PatternKind::Or(a, b) => {
                let mut names = self.extract_pattern_names(a);
                names.extend(self.extract_pattern_names(b));
                names
            }
            crate::ast::PatternKind::Ref(p)
            | crate::ast::PatternKind::InOut(p)
            | crate::ast::PatternKind::Consume(p) => self.extract_pattern_names(p),
            crate::ast::PatternKind::Guard(p, _) => self.extract_pattern_names(p),
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

    /// Two functions may NOT reuse one parameter name at two different types.
    ///
    /// A `PirModule` is one flat statement list with no function structure, so the
    /// generated entry function has ONE slot per name. Deduplicating on name alone
    /// would hand the second function the first one's buffer -- the caller fills
    /// `Tensor[f32,8]` elements and the second function reads them as
    /// `Tensor[f32,4]` or as `i8`, which is a silent wrong answer with no
    /// diagnostic. So it is refused, naming both types.
    ///
    /// The SAME name at the SAME type is fine, and must stay fine: that is the
    /// ordinary "two kernels with the same signature" case.
    #[test]
    fn test_two_functions_may_not_reuse_a_name_at_two_types() {
        let program = parse_program(
            "fn a(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn b(t: Tensor[f32,4]) { forall i in 0..4 { t[i] = 2.0; } }",
        )
        .expect("parse");
        let err = lower_program(&program).expect_err("must refuse the collision");
        let msg = format!("{err}");
        assert!(
            msg.contains("`t`") && msg.contains("two different types"),
            "the refusal must name the parameter and the conflict: {msg}"
        );

        // The same name at the same type is ONE slot, not a refusal.
        let same = lower(
            "fn a(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn b(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 2.0; } }",
        );
        assert_eq!(
            same.statements.len(),
            2,
            "identical signatures must still lower, one slot shared by name"
        );
        assert_eq!(
            same.function_params.len(),
            1,
            "`t` is one entry slot, deduplicated because the types agree"
        );
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
