//! Naso Statement AST
//!
//! Top-level statements and declarations.

use crate::ast::expr::Expr;
use crate::ast::ty::Type as AstType;
use crate::ast::{Ident, Mutability, NodeId, Pattern, Quantity, Span};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Statement
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
    pub id: NodeId,
}

impl Stmt {
    pub fn new(kind: StmtKind, span: Span, id: NodeId) -> Self {
        Self { kind, span, id }
    }
}

/// Statement kinds
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// `StmtKind` variants differ in size by a wide margin because several wrap
// large inline structs. Recording a tensor extent on `Type` (which every
// statement's pattern carries) pushed the largest variant past clippy's
// `large_enum_variant` threshold. Boxing the large variants is the alternative
// and touches many construction sites; the allow keeps that refactor separate
// from the extent fix. Remove this allow when the variants are boxed.
#[allow(clippy::large_enum_variant)]
pub enum StmtKind {
    /// Let binding
    Let(LetStmt),
    /// Inout let binding
    LetInOut(LetInOutStmt),
    /// Consume let binding
    LetConsume(LetConsumeStmt),
    /// Expression statement
    Expr(Expr),
    /// Item declaration (function, type, etc.)
    Item(crate::ast::Item),
    /// Reversible block statement
    Reversible(crate::ast::expr::ReversibleBlock),
    /// Proof block: obligations the verifier must discharge. The body is
    /// typechecked like any other block, but nothing in it reaches codegen.
    Proof(crate::ast::expr::ProofBlock),
    /// Return statement
    Return(Option<Expr>),
    /// Break statement
    Break(Option<Expr>),
    /// Continue statement
    Continue,
    /// Empty statement
    Empty,
    /// Error statement
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetStmt {
    pub pattern: Pattern,
    pub ty: Option<AstType>,
    pub quantity: Quantity,
    pub mutability: Mutability,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetInOutStmt {
    pub name: Ident,
    pub ty: Option<AstType>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetConsumeStmt {
    pub name: Ident,
    pub ty: Option<AstType>,
    pub value: Expr,
    pub span: Span,
}

impl fmt::Display for StmtKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StmtKind::Let(l) => write!(f, "let {}{:?} = {:?};", l.mutability, l.pattern, l.value),
            StmtKind::LetInOut(l) => write!(f, "let inout {:?} = {:?};", l.name, l.value),
            StmtKind::LetConsume(l) => write!(f, "let consume {:?} = {:?};", l.name, l.value),
            StmtKind::Expr(e) => write!(f, "{:?};", e),
            StmtKind::Item(i) => write!(f, "{:?}", i),
            StmtKind::Reversible(_r) => write!(f, "reversible {{ ... }}"),
            StmtKind::Proof(_p) => write!(f, "proof {{ ... }}"),
            StmtKind::Return(opt) => {
                if let Some(e) = opt {
                    write!(f, "return {:?};", e)
                } else {
                    write!(f, "return;")
                }
            }
            StmtKind::Break(opt) => {
                if let Some(e) = opt {
                    write!(f, "break {:?};", e)
                } else {
                    write!(f, "break;")
                }
            }
            StmtKind::Continue => write!(f, "continue;"),
            StmtKind::Empty => write!(f, ";"),
            StmtKind::Error => write!(f, "<error>"),
        }
    }
}
