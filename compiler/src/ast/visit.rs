//! AST visitor trait
//!
//! Provides a default treeless visitor over the Naso AST. The default
//! implementations walk each node's children; overrides can be used to
//! implement passes (e.g. type checking, reversible uncomputation).

#![allow(clippy::collapsible_if)]

use crate::ast::{
    Block, ConstDef, Expr, Function, Item, Module, Param, Program, Stmt, Type, TypeDef,
};

/// A visitor over the AST.
///
/// Each method returns a [`VisitOutcome`] which allows the traversal to
/// short-circuit (e.g. after encountering an error).
pub trait Visitor: Sized {
    type Err;

    fn visit_program(&mut self, program: &Program) -> Result<VisitOutcome, Self::Err> {
        walk::walk_program(self, program)
    }

    fn visit_item(&mut self, item: &Item) -> Result<VisitOutcome, Self::Err> {
        walk::walk_item(self, item)
    }

    fn visit_function(&mut self, function: &Function) -> Result<VisitOutcome, Self::Err> {
        walk::walk_function(self, function)
    }

    fn visit_type_def(&mut self, type_def: &TypeDef) -> Result<VisitOutcome, Self::Err> {
        walk::walk_type_def(self, type_def)
    }

    fn visit_module(&mut self, module: &Module) -> Result<VisitOutcome, Self::Err> {
        walk::walk_module(self, module)
    }

    fn visit_const(&mut self, constant: &ConstDef) -> Result<VisitOutcome, Self::Err> {
        walk::walk_const(self, constant)
    }

    fn visit_param(&mut self, param: &Param) -> Result<VisitOutcome, Self::Err> {
        walk::walk_param(self, param)
    }

    fn visit_block(&mut self, block: &Block) -> Result<VisitOutcome, Self::Err> {
        walk::walk_block(self, block)
    }

    fn visit_stmt(&mut self, stmt: &Stmt) -> Result<VisitOutcome, Self::Err> {
        walk::walk_stmt(self, stmt)
    }

    fn visit_expr(&mut self, expr: &Expr) -> Result<VisitOutcome, Self::Err> {
        walk::walk_expr(self, expr)
    }

    fn visit_type(&mut self, ty: &Type) -> Result<VisitOutcome, Self::Err> {
        walk::walk_type(self, ty)
    }
}

/// Control flow signal for the visitor traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisitOutcome {
    /// Continue traversing children.
    Continue,
    /// Continue to the next sibling (skip this node's children).
    Skip,
    /// Stop the entire traversal.
    Stop,
}

/// A visitor with no failures; convenience for simple passes.
pub trait SimpleVisitor {
    fn visit_program(&mut self, program: &Program) {
        let _ = program;
    }
    fn visit_item(&mut self, item: &Item) {
        let _ = item;
    }
    fn visit_function(&mut self, function: &Function) {
        let _ = function;
    }
    fn visit_type_def(&mut self, type_def: &TypeDef) {
        let _ = type_def;
    }
    fn visit_module(&mut self, module: &Module) {
        let _ = module;
    }
    fn visit_const(&mut self, constant: &ConstDef) {
        let _ = constant;
    }
    fn visit_param(&mut self, param: &Param) {
        let _ = param;
    }
    fn visit_block(&mut self, block: &Block) {
        let _ = block;
    }
    fn visit_stmt(&mut self, stmt: &Stmt) {
        let _ = stmt;
    }
    fn visit_expr(&mut self, expr: &Expr) {
        let _ = expr;
    }
    fn visit_type(&mut self, ty: &Type) {
        let _ = ty;
    }
}

/// Default traversal helpers. These are re-implementations of the default
/// method bodies so they can also be invoked directly.
pub mod walk {
    use super::*;
    use crate::ast::Visitor;
    use crate::ast::*;

    pub fn walk_program<V: Visitor>(
        visitor: &mut V,
        program: &Program,
    ) -> Result<VisitOutcome, V::Err> {
        for item in &program.items {
            if visitor.visit_item(item)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        Ok(VisitOutcome::Continue)
    }

    pub fn walk_item<V: Visitor>(visitor: &mut V, item: &Item) -> Result<VisitOutcome, V::Err> {
        match item {
            Item::Function(f) => visitor.visit_function(f),
            Item::TypeDef(t) => visitor.visit_type_def(t),
            Item::Module(m) => visitor.visit_module(m),
            Item::Import(_) => Ok(VisitOutcome::Continue),
            Item::Const(c) => visitor.visit_const(c),
        }
    }

    pub fn walk_function<V: Visitor>(
        visitor: &mut V,
        function: &Function,
    ) -> Result<VisitOutcome, V::Err> {
        for param in &function.params {
            if visitor.visit_param(param)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        if let Some(ret) = &function.ret_ty {
            if visitor.visit_type(ret)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        visitor.visit_block(&function.body)
    }

    pub fn walk_type_def<V: Visitor>(
        visitor: &mut V,
        type_def: &TypeDef,
    ) -> Result<VisitOutcome, V::Err> {
        match &type_def.kind {
            crate::ast::TypeDefKind::Struct(fields) => {
                for field in fields {
                    if visitor.visit_type(&field.ty)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
            }
            crate::ast::TypeDefKind::Enum(variants) => {
                for variant in variants {
                    for field in &variant.fields {
                        if visitor.visit_type(&field.ty)? == VisitOutcome::Stop {
                            return Ok(VisitOutcome::Stop);
                        }
                    }
                }
            }
            crate::ast::TypeDefKind::Alias(t) => {
                if visitor.visit_type(t)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
            }
        }
        Ok(VisitOutcome::Continue)
    }

    pub fn walk_module<V: Visitor>(
        visitor: &mut V,
        module: &Module,
    ) -> Result<VisitOutcome, V::Err> {
        for item in &module.items {
            if visitor.visit_item(item)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        Ok(VisitOutcome::Continue)
    }

    pub fn walk_const<V: Visitor>(
        visitor: &mut V,
        constant: &ConstDef,
    ) -> Result<VisitOutcome, V::Err> {
        if let Some(ty) = &constant.ty {
            if visitor.visit_type(ty)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        visitor.visit_expr(&constant.value)
    }

    pub fn walk_param<V: Visitor>(visitor: &mut V, param: &Param) -> Result<VisitOutcome, V::Err> {
        visitor.visit_type(&param.ty)
    }

    pub fn walk_block<V: Visitor>(visitor: &mut V, block: &Block) -> Result<VisitOutcome, V::Err> {
        for stmt in &block.stmts {
            if visitor.visit_stmt(stmt)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        if let Some(expr) = &block.expr {
            if visitor.visit_expr(expr)? == VisitOutcome::Stop {
                return Ok(VisitOutcome::Stop);
            }
        }
        Ok(VisitOutcome::Continue)
    }

    pub fn walk_stmt<V: Visitor>(visitor: &mut V, stmt: &Stmt) -> Result<VisitOutcome, V::Err> {
        match &stmt.kind {
            StmtKind::Let(l) => visitor.visit_expr(&l.value),
            StmtKind::LetInOut(l) => visitor.visit_expr(&l.value),
            StmtKind::LetConsume(l) => visitor.visit_expr(&l.value),
            StmtKind::Expr(e) => visitor.visit_expr(e),
            StmtKind::Reversible(r) => {
                walk_block(visitor, &r.body)?;
                for step in &r.uncomputes {
                    if visitor.visit_expr(&step.inverse_expr)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            StmtKind::Proof(p) => {
                walk_block(visitor, &p.body)?;
                Ok(VisitOutcome::Continue)
            }
            StmtKind::Item(i) => visitor.visit_item(i),
            StmtKind::Return(opt) => opt
                .as_ref()
                .map_or(Ok(VisitOutcome::Continue), |e| visitor.visit_expr(e)),
            StmtKind::Break(opt) => opt
                .as_ref()
                .map_or(Ok(VisitOutcome::Continue), |e| visitor.visit_expr(e)),
            StmtKind::Continue => Ok(VisitOutcome::Continue),
            StmtKind::Empty | StmtKind::Error => Ok(VisitOutcome::Continue),
        }
    }

    pub fn walk_expr<V: Visitor>(visitor: &mut V, expr: &Expr) -> Result<VisitOutcome, V::Err> {
        use crate::ast::ExprKind;
        match &expr.kind {
            ExprKind::Literal(_) | ExprKind::Var(_) | ExprKind::Error => Ok(VisitOutcome::Continue),
            ExprKind::Binary(_, l, r) => {
                if visitor.visit_expr(l)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_expr(r)
            }
            ExprKind::Unary(_, e) => visitor.visit_expr(e),
            ExprKind::Call(fun, args) => {
                if visitor.visit_expr(fun)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                for arg in args {
                    if visitor.visit_expr(arg)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::MethodCall(recv, _, args) => {
                if visitor.visit_expr(recv)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                for arg in args {
                    if visitor.visit_expr(arg)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Field(recv, _) => visitor.visit_expr(recv),
            ExprKind::Index(base, index) => {
                if visitor.visit_expr(base)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_expr(index)
            }
            ExprKind::Struct(_, fields) => {
                for field in fields {
                    if visitor.visit_expr(&field.value)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Variant(_, _, args) => {
                for arg in args {
                    if visitor.visit_expr(arg)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Tuple(items) | ExprKind::Array(items) => {
                for item in items {
                    if visitor.visit_expr(item)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Block(b) => walk_block(visitor, b),
            ExprKind::If(cond, then, else_) => {
                if visitor.visit_expr(cond)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                if visitor.visit_expr(then)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                if let Some(e) = else_ {
                    if visitor.visit_expr(e)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Match(scrutinee, arms) => {
                if visitor.visit_expr(scrutinee)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                for arm in arms {
                    if visitor.visit_expr(&arm.body)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Let(l) => visitor.visit_expr(&l.value),
            ExprKind::LetInOut(l) => visitor.visit_expr(&l.value),
            ExprKind::LetConsume(l) => visitor.visit_expr(&l.value),
            ExprKind::Reversible(r) => {
                walk_block(visitor, &r.body)?;
                for step in &r.uncomputes {
                    if visitor.visit_expr(&step.inverse_expr)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Lambda(l) => {
                for param in &l.params {
                    if visitor.visit_param(param)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                visitor.visit_expr(&l.body)
            }
            ExprKind::For(loop_) => {
                if visitor.visit_expr(&loop_.iter)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                walk_block(visitor, &loop_.body)
            }
            ExprKind::Range(lo, hi) => {
                if visitor.visit_expr(lo)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_expr(hi)
            }
            ExprKind::Forall(loop_) => {
                for (_, lower, upper) in &loop_.bindings {
                    if visitor.visit_expr(lower)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                    if visitor.visit_expr(upper)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                walk_block(visitor, &loop_.body)
            }
            ExprKind::Quantified(quant) => {
                for (_, lower, upper) in &quant.bindings {
                    if visitor.visit_expr(lower)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                    if visitor.visit_expr(upper)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                walk_block(visitor, &quant.body)
            }
            ExprKind::While(cond, body) => {
                if visitor.visit_expr(cond)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_expr(body)
            }
            ExprKind::Return(e) | ExprKind::Break(e) => {
                if let Some(inner) = e {
                    visitor.visit_expr(inner)
                } else {
                    Ok(VisitOutcome::Continue)
                }
            }
            ExprKind::Continue => Ok(VisitOutcome::Continue),
            ExprKind::Assign(l, r) => {
                if visitor.visit_expr(l)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_expr(r)
            }
            ExprKind::Projection(e) => visitor.visit_expr(e),
            ExprKind::QuantumOp(op) => {
                use crate::ast::QuantumOp;
                match op {
                    QuantumOp::Alloc(_) => {}
                    QuantumOp::Measure(e) => {
                        if visitor.visit_expr(e)? == VisitOutcome::Stop {
                            return Ok(VisitOutcome::Stop);
                        }
                    }
                    QuantumOp::ApplyGate(_, args) => {
                        for arg in args {
                            if visitor.visit_expr(arg)? == VisitOutcome::Stop {
                                return Ok(VisitOutcome::Stop);
                            }
                        }
                    }
                    QuantumOp::Entangle(qubits) => {
                        for q in qubits {
                            if visitor.visit_expr(q)? == VisitOutcome::Stop {
                                return Ok(VisitOutcome::Stop);
                            }
                        }
                    }
                    QuantumOp::Phase(a, b) | QuantumOp::Hamiltonian(a, b) => {
                        if visitor.visit_expr(a)? == VisitOutcome::Stop {
                            return Ok(VisitOutcome::Stop);
                        }
                        if visitor.visit_expr(b)? == VisitOutcome::Stop {
                            return Ok(VisitOutcome::Stop);
                        }
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            ExprKind::Ascribe(e, _) => visitor.visit_expr(e),
        }
    }

    pub fn walk_type<V: Visitor>(visitor: &mut V, ty: &Type) -> Result<VisitOutcome, V::Err> {
        match &ty.kind {
            TypeKind::Projection(inner) | TypeKind::Reversible(inner) => visitor.visit_type(inner),
            TypeKind::Function(params, ret) => {
                for param in params {
                    if visitor.visit_type(param)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                visitor.visit_type(ret)
            }
            TypeKind::Named(_name, args) => {
                for arg in args {
                    if let TypeArg::Type(t) = arg {
                        if visitor.visit_type(t)? == VisitOutcome::Stop {
                            return Ok(VisitOutcome::Stop);
                        }
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            TypeKind::QRegister(dims) | TypeKind::Tensor(dims) => {
                for dim in dims {
                    if visitor.visit_type(dim)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            TypeKind::Pi(_name, domain, codomain) => {
                if visitor.visit_type(domain)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_type(codomain)
            }
            TypeKind::Sigma(_name, fst, snd) => {
                if visitor.visit_type(fst)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_type(snd)
            }
            TypeKind::Lambda(_param, body) => visitor.visit_type(body),
            TypeKind::App(fun, arg) => {
                if visitor.visit_type(fun)? == VisitOutcome::Stop {
                    return Ok(VisitOutcome::Stop);
                }
                visitor.visit_type(arg)
            }
            TypeKind::Array(elem, _) => visitor.visit_type(elem),
            TypeKind::Tuple(elems) => {
                for elem in elems {
                    if visitor.visit_type(elem)? == VisitOutcome::Stop {
                        return Ok(VisitOutcome::Stop);
                    }
                }
                Ok(VisitOutcome::Continue)
            }
            TypeKind::Unit
            | TypeKind::Bool
            | TypeKind::Int
            | TypeKind::UInt
            | TypeKind::Float
            | TypeKind::String
            | TypeKind::Char
            | TypeKind::Nat
            | TypeKind::Quint8
            | TypeKind::Qubit
            | TypeKind::Universe(_)
            | TypeKind::Var(_)
            | TypeKind::Meta(_)
            | TypeKind::Error => Ok(VisitOutcome::Continue),
        }
    }
}
