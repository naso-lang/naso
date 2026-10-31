//! Naso Pattern Matching AST
//!
//! Patterns for match expressions and let bindings with quantitative annotations.

use crate::ast::{Ident, Literal, NodeId, Quantity, Span};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Pattern for matching
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub kind: PatternKind,
    pub ty: Option<crate::ast::ty::Type>,
    pub quantity: Quantity,
    pub span: Span,
    pub id: NodeId,
}

impl Pattern {
    pub fn new(kind: PatternKind, span: Span, id: NodeId) -> Self {
        Self {
            kind,
            ty: None,
            quantity: Quantity::Many,
            span,
            id,
        }
    }

    pub fn with_ty(mut self, ty: crate::ast::ty::Type) -> Self {
        self.ty = Some(ty);
        self
    }

    pub fn with_qty(mut self, qty: Quantity) -> Self {
        self.quantity = qty;
        self
    }
}

/// Pattern kinds
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PatternKind {
    /// Wildcard pattern _
    Wildcard,
    /// Identifier binding
    Ident(Ident),
    /// Literal pattern
    Literal(Literal),
    /// Tuple pattern
    Tuple(Vec<Pattern>),
    /// Struct pattern
    Struct(Ident, Vec<FieldPattern>),
    /// Enum variant pattern
    Variant(Ident, Ident, Vec<Pattern>),
    /// Array/slice pattern
    Array(Vec<Pattern>),
    /// Range pattern (start..end)
    Range(Box<Pattern>, Box<Pattern>),
    /// Or pattern (a | b)
    Or(Box<Pattern>, Box<Pattern>),
    /// Reference pattern (&pattern)
    Ref(Box<Pattern>),
    /// Inout pattern (&mut pattern)
    InOut(Box<Pattern>),
    /// Consume pattern (consume pattern)
    Consume(Box<Pattern>),
    /// Guard pattern (pattern if expr)
    Guard(Box<Pattern>, Box<crate::ast::expr::Expr>),
    /// Error pattern
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldPattern {
    pub name: Ident,
    pub pattern: Pattern,
    pub span: Span,
}

impl fmt::Display for PatternKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternKind::Wildcard => write!(f, "_"),
            PatternKind::Ident(i) => write!(f, "{}", i),
            PatternKind::Literal(l) => write!(f, "{}", l),
            PatternKind::Tuple(ps) => write!(
                f,
                "({})",
                ps.iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            PatternKind::Struct(name, fields) => {
                write!(
                    f,
                    "{} {{ {} }}",
                    name,
                    fields
                        .iter()
                        .map(|f| f.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            PatternKind::Variant(enum_name, variant, fields) => {
                write!(f, "{}::{}", enum_name, variant)?;
                if !fields.is_empty() {
                    write!(
                        f,
                        "({})",
                        fields
                            .iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                Ok(())
            }
            PatternKind::Array(ps) => write!(
                f,
                "[{}]",
                ps.iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            PatternKind::Range(s, e) => write!(f, "{}..{}", s, e),
            PatternKind::Or(a, b) => write!(f, "{} | {}", a, b),
            PatternKind::Ref(p) => write!(f, "&{}", p),
            PatternKind::InOut(p) => write!(f, "&mut {}", p),
            PatternKind::Consume(p) => write!(f, "consume {}", p),
            PatternKind::Guard(p, g) => write!(f, "{:?} if {:?}", p, g),
            PatternKind::Error => write!(f, "<error>"),
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.quantity != Quantity::Many {
            write!(f, "{} ", self.quantity)?;
        }
        write!(f, "{}", self.kind)
    }
}

impl fmt::Display for FieldPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.pattern)
    }
}
