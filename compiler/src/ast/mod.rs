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

/// Typed integer literal value
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedInt {
    pub value: i64,
    pub ty: Option<IntTy>,
}

/// Integer type suffix for typed literals (e.g., u8, u16, i32, i64)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntTy {
    U8, U16, U32, U64, USize,
    I8, I16, I32, I64, ISize,
}

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
    /// Trait definition
    Trait(Trait),
    /// Trait implementation
    TraitImpl(TraitImpl),
    /// Module declaration
    Module(Module),
    /// Import declaration
    Import(Import),
    /// Constant definition
    Const(ConstDef),
    /// External function block (extern "C" { ... })
    Extern(ExternBlock),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Function(f) => f.span,
            Item::TypeDef(t) => t.span,
            Item::Trait(t) => t.span,
            Item::TraitImpl(t) => t.span,
            Item::Module(m) => m.span,
            Item::Import(i) => i.span,
            Item::Const(c) => c.span,
            Item::Extern(e) => e.span,
        }
    }
}

/// External function block: extern "C" { fn name(...); ... }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternBlock {
    pub abi: String,
    pub functions: Vec<ExternFn>,
    pub span: Span,
}

/// External function declaration within extern block
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternFn {
    pub name: Ident,
    pub params: Vec<ExternParam>,
    pub ret_ty: Option<Type>,
    pub link_name: Option<String>,
    pub span: Span,
    pub is_async: bool,
}

/// Parameter of an external function
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternParam {
    pub name: Ident,
    pub ty: Type,
    pub quantity: Quantity,
    pub mutability: Mutability,
    pub span: Span,
}

/// Trait definition: trait Name<Params> { associated types; methods }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trait {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub bounds: Vec<TraitBound>, // Supertraits
    pub items: Vec<TraitItem>,
    pub span: Span,
}

/// Trait item: associated type or method signature
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TraitItem {
    /// Associated type: type Name<Params>: Bounds;
    AssociatedType(AssociatedType),
    /// Method signature: fn name(...) -> Ret;
    Method(TraitMethod),
}

/// Associated type definition in a trait
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssociatedType {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub bounds: Vec<TraitBound>, // where Self: Trait bounds
    pub span: Span,
}

/// Method signature in a trait (no body)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraitMethod {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret_ty: Option<Type>,
    pub span: Span,
    pub is_async: bool,
}

/// Trait bound: Trait<Args> + Send + ...
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraitBound {
    pub path: Vec<Ident>,
    pub args: Vec<TypeArg>,
    pub span: Span,
}

/// Trait implementation: impl<Params> Trait<Args> for Type { items }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraitImpl {
    pub generics: Vec<GenericParam>,
    pub trait_path: Vec<Ident>,
    pub trait_args: Vec<TypeArg>,
    pub for_ty: Type,
    pub where_clauses: Vec<WhereClause>,
    pub items: Vec<ImplItem>,
    pub span: Span,
}

/// Implementation item: associated type definition or method
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImplItem {
    /// Associated type definition: type Name = Type;
    AssociatedTypeDef(AssociatedTypeDef),
    /// Method implementation
    Method(Function),
}

/// Associated type definition in an impl block
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssociatedTypeDef {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub ty: Type,
    pub span: Span,
}

/// Where clause: where Type: Trait<Args>, Type2: Trait2
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhereClause {
    pub subject: Type,
    pub bounds: Vec<TraitBound>,
    pub span: Span,
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
    pub is_async: bool,
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
    /// Trait bounds for this generic parameter (e.g., `T: Iterator + Clone`)
    pub bounds: Vec<TraitBound>,
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
    Int(i64, Option<IntTy>),
    UInt(u64, Option<IntTy>),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Unit,
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Int(v, ty) => {
                write!(f, "{}", v)?;
                if let Some(ty) = ty {
                    write!(f, "{}", ty)?;
                }
                Ok(())
            }
            Literal::UInt(v, ty) => {
                write!(f, "{}", v)?;
                if let Some(ty) = ty {
                    write!(f, "{}", ty)?;
                }
                Ok(())
            }
            Literal::Float(v) => write!(f, "{}", v),
            Literal::Bool(v) => write!(f, "{}", v),
            Literal::String(v) => write!(f, "\"{}\"", v.escape_debug()),
            Literal::Char(v) => write!(f, "'{}'", v.escape_debug()),
            Literal::Unit => write!(f, "()"),
        }
    }
}

impl fmt::Display for IntTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntTy::U8 => write!(f, "u8"),
            IntTy::U16 => write!(f, "u16"),
            IntTy::U32 => write!(f, "u32"),
            IntTy::U64 => write!(f, "u64"),
            IntTy::USize => write!(f, "usize"),
            IntTy::I8 => write!(f, "i8"),
            IntTy::I16 => write!(f, "i16"),
            IntTy::I32 => write!(f, "i32"),
            IntTy::I64 => write!(f, "i64"),
            IntTy::ISize => write!(f, "isize"),
        }
    }
}

impl fmt::Display for TraitBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, ident) in self.path.iter().enumerate() {
            if i > 0 {
                write!(f, "::")?;
            }
            write!(f, "{}", ident)?;
        }
        if !self.args.is_empty() {
            write!(f, "[")?;
            for (i, arg) in self.args.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", arg)?;
            }
            write!(f, "]")?;
        }
        Ok(())
    }
}
