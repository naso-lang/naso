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

        // Lower all items
        for item in &program.items {
            self.lower_item(item)?;
        }

        // Build schedule tree from extracted nodes
        let schedule = self.build_schedule_tree()?;

        // Build PIR module
        let pir = PirModule::new(
            std::mem::take(&mut self.statements),
            schedule,
            std::mem::take(&mut self.accesses),
            std::mem::take(&mut self.quantities),
            self.param_names.clone(),
        );

        // Validate the generated PIR
        validate_pir(&pir).map_err(|e| LoweringError::ValidationError(format!("{:?}", e)))?;

        Ok(pir)
    }

    fn extract_parameters(&mut self, program: &Program) {
        // Extract parameters from main function signature
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

    fn lower_item(&mut self, item: &crate::ast::Item) -> Result<(), LoweringError> {
        use crate::ast::Item;

        match item {
            Item::Function(func) => {
                if func.name.name != "main" {
                    // Skip non-main functions for now
                    return Ok(());
                }
                // Lower function body
                for stmt in &func.body.stmts {
                    self.lower_stmt(stmt)?;
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

        // Create statement
        let stmt_id = self.next_stmt_id();
        let domain = AffineDomain::universe(0, 0); // Scalar let binding

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

    fn lower_expr_stmt(&mut self, expr: &crate::ast::Expr) -> Result<(), LoweringError> {
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
