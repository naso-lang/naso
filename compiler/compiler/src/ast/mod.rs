//! Naso Compiler - Core AST Definitions
//!
//! This module defines the Abstract Syntax Tree (AST) for the Naso language,
//! including quantitative type annotations, reversible blocks, and mutable value semantics.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::Hash;

pub mod expr;
pub mod pattern;
pub mod stmt;
pub mod ty;
pub mod visit;

pub use expr::*;
pub use pattern::*;
pub use stmt::*;
pub use ty::*;
pub use visit::*;

/// Unique identifier for AST nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Source location information
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(start: u32, end: u32, line: u32, column: u32) -> Self {
        Self {
            start,
            end,
            line,
            column,
        }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            line: self.line.min(other.line),
            column: self.column.min(other.column),
        }
    }
}

impl Default for Span {
    fn default() -> Self {
        Self::new(0, 0, 1, 1)
    }
}

/// Quantitative usage annotation: [0], [1], [*], [N]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Quantity {
    /// Erased at compile time - proofs, invariants
    Zero,
    /// Linear - must be consumed exactly once
    One,
    /// Bounded reuse - exactly N times (stored separately)
    Bounded(u32),
    /// Unrestricted - can be used any number of times
    #[default]
    Many,
}

impl Quantity {
    pub fn is_linear(&self) -> bool {
        matches!(self, Quantity::One)
    }

    pub fn is_erased(&self) -> bool {
        matches!(self, Quantity::Zero)
    }

    pub fn is_unrestricted(&self) -> bool {
        matches!(self, Quantity::Many)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quantity::Zero => write!(f, "[0]"),
            Quantity::One => write!(f, "[1]"),
            Quantity::Bounded(n) => write!(f, "[{}]", n),
            Quantity::Many => write!(f, "[*]"),
        }
    }
}

/// Mutability mode for bindings and parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mutability {
    /// Immutable binding (default)
    Immutable,
    /// Mutable binding (mut)
    Mut,
    /// Mutable via inout projection (no aliasing)
    InOut,
    /// Consumed exactly once (linear move)
    Consume,
}

impl fmt::Display for Mutability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mutability::Immutable => write!(f, ""),
            Mutability::Mut => write!(f, "mut "),
            Mutability::InOut => write!(f, "inout "),
            Mutability::Consume => write!(f, "consume "),
        }
    }
}

/// Top-level program AST
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub items: Vec<Item>,
    pub span: Span,
}

impl Program {
    pub fn new(items: Vec<Item>, span: Span) -> Self {
        Self { items, span }
    }
}

/// Top-level items (functions, types, modules, etc.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Item {
    /// Function definition
    Function(Function),
    /// Type definition (struct, enum, alias)
    TypeDef(TypeDef),
    /// Module declaration
    Module(Module),
    /// Import declaration
    Import(Import),
    /// Constant definition
    Const(ConstDef),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Function(f) => f.span,
            Item::TypeDef(t) => t.span,
            Item::Module(m) => m.span,
            Item::Import(i) => i.span,
            Item::Const(c) => c.span,
        }
    }
}

/// Function definition with quantitative parameters
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret_ty: Option<Type>,
    pub body: Block,
    pub span: Span,
    pub attributes: Vec<Attribute>,
    pub is_reversible: bool,
    pub quantity: Quantity,
    /// Claims the CALLER must satisfy, written `requires { assert(..); .. }`.
    ///
    /// These are axioms the prover may assume while discharging the body's proof block,
    /// and they are never themselves discharged as obligations of this function -- a
    /// precondition is not something the function proves about itself. Keeping the two
    /// lists separate is what stops a function from discharging its obligations by
    /// assuming them.
    pub requires: Vec<Expr>,
}

/// Generic type parameter
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericParam {
    pub name: Ident,
    pub kind: GenericKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GenericKind {
    /// Type parameter
    Type,
    /// Natural number parameter (for dependent types)
    Nat,
    /// Quantity parameter
    Quantity,
}

/// Function parameter with quantity and mutability
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: Ident,
    pub ty: Type,
    pub quantity: Quantity,
    pub mutability: Mutability,
    pub span: Span,
}

/// Function body (block of statements)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub expr: Option<Box<Expr>>,
    pub span: Span,
}

impl Block {
    pub fn new(stmts: Vec<Stmt>, expr: Option<Expr>, span: Span) -> Self {
        Self {
            stmts,
            expr: expr.map(Box::new),
            span,
        }
    }
}

/// Type definitions (struct, enum, type alias)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub kind: TypeDefKind,
    pub span: Span,
    pub attributes: Vec<Attribute>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeDefKind {
    /// struct Name { fields }
    Struct(Vec<Field>),
    /// enum Name { variants }
    Enum(Vec<Variant>),
    /// type Name = Type;
    Alias(Type),
}

/// Struct/enum field
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: Ident,
    pub ty: Type,
    pub quantity: Quantity,
    pub span: Span,
    pub attributes: Vec<Attribute>,
}

/// Enum variant
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub name: Ident,
    pub fields: Vec<Field>,
    pub span: Span,
    pub attributes: Vec<Attribute>,
}

/// Module declaration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: Ident,
    pub items: Vec<Item>,
    pub span: Span,
}

/// Import declaration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Import {
    pub path: Vec<Ident>,
    pub items: ImportItems,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportItems {
    /// import path::*;
    All,
    /// import path::{item1, item2}
    Specific(Vec<Ident>),
}

/// Constant definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstDef {
    pub name: Ident,
    pub ty: Option<Type>,
    pub value: Expr,
    pub span: Span,
}

/// Attribute (e.g., #[inline], #[test])
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribute {
    pub name: Ident,
    pub args: Vec<AttributeArg>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttributeArg {
    Ident(Ident),
    Literal(Literal),
    Nested(Attribute),
}

/// Identifier with span
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl PartialEq for Ident {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Ident {}

impl std::hash::Hash for Ident {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Self {
            name: name.into(),
            span,
        }
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Literal values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Unit,
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Int(v) => write!(f, "{}", v),
            Literal::UInt(v) => write!(f, "{}u", v),
            Literal::Float(v) => write!(f, "{}", v),
            Literal::Bool(v) => write!(f, "{}", v),
            Literal::String(v) => write!(f, "\"{}\"", v.escape_debug()),
            Literal::Char(v) => write!(f, "'{}'", v.escape_debug()),
            Literal::Unit => write!(f, "()"),
        }
    }
}
