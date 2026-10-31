//! Naso Type System AST
//!
//! Defines the type language including quantitative annotations,
//! dependent types (Nat), function types, and quantum types.

use crate::ast::{Ident, Quantity, Span};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Type expression in the AST
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Type {
    pub kind: TypeKind,
    pub quantity: Quantity,
    pub span: Span,
    /// Bit width of an integer type, when the source spelled one out.
    ///
    /// `i8`, `i16`, `i32`, `i64` and `isize` ALL parse to `TypeKind::Int`, so
    /// without this the width is discarded and a backend cannot tell an `i8`
    /// tensor from an `i32` one. WGSL has no `i8` storage type, which made this
    /// a live wrong-answer bug: a narrowing cast was silently dropped.
    ///
    /// `None` means the source did not name a width (a bare `int` literal
    /// type). `isize`/`usize` are recorded as 64, the width this target uses;
    /// a cross-target backend must consult the data layout rather than assume.
    pub int_width: Option<u8>,
    /// The constant value of a `TypeKind::Nat` used as a tensor extent, e.g. the
    /// `1024` in `Tensor[f32, 1024]`.
    ///
    /// `TypeKind::Nat` is a unit variant, so a bare `Nat` carries no value, and
    /// the parser used to read the extent literal and DISCARD it -- which made
    /// `Tensor[f32, 1024]` and `Tensor[f32, 4]` indistinguishable in the AST and
    /// left any backend with no way to know the element count.
    ///
    /// Adding a variant to `TypeKind` instead would be the tidier shape, but that
    /// is an exhaustive match in 11 places, and this field is what lets a codegen
    /// backend emit a correct bounds guard. `None` means "not a known constant",
    /// which is what a generic `N` or a bare `Nat` is.
    pub nat_value: Option<u64>,
    /// If true, the tensor is a sparse tensor. Only applies to `TypeKind::Tensor`.
    /// A sparse tensor allocates one `u32` mask word per 32 elements (1 bit set
    /// if the element is non-zero) plus the dense allocation for non-zero elements.
    pub sparse: bool,
}

impl Type {
    pub fn new(kind: TypeKind, quantity: Quantity, span: Span) -> Self {
        Self {
            kind,
            quantity,
            span,
            nat_value: None,
            int_width: None,
            sparse: false,
        }
    }

    /// An integer type of a specific bit width, as written in the source.
    pub fn int_width(width: u8, span: Span) -> Self {
        Self {
            kind: TypeKind::Int,
            quantity: Quantity::Many,
            span,
            nat_value: None,
            int_width: Some(width),
            sparse: false,
        }
    }

    /// An unsigned integer type of a specific bit width.
    pub fn uint_width(width: u8, span: Span) -> Self {
        Self {
            kind: TypeKind::UInt,
            quantity: Quantity::Many,
            span,
            nat_value: None,
            int_width: Some(width),
            sparse: false,
        }
    }

    /// A `Nat` type carrying a known constant value, as a tensor extent.
    pub fn nat_lit(value: u64, span: Span) -> Self {
        Self {
            kind: TypeKind::Nat,
            quantity: Quantity::Many,
            span,
            nat_value: Some(value),
            int_width: None,
            sparse: false,
        }
    }

    /// The constant value of this type, if it is a `Nat` with a known value.
    pub fn nat_const(&self) -> Option<u64> {
        self.nat_value
    }

    pub fn unit(span: Span) -> Self {
        Self::new(TypeKind::Unit, Quantity::Many, span)
    }

    pub fn bool(span: Span) -> Self {
        Self::new(TypeKind::Bool, Quantity::Many, span)
    }

    pub fn int(span: Span) -> Self {
        Self::new(TypeKind::Int, Quantity::Many, span)
    }

    pub fn nat(span: Span) -> Self {
        Self::new(TypeKind::Nat, Quantity::Many, span)
    }

    /// `quint8` — 8-bit unsigned quantized integer, stored packed as u32.
    pub fn quint8(span: Span) -> Self {
        Self::new(TypeKind::Quint8, Quantity::Many, span)
    }

    pub fn qubit(span: Span) -> Self {
        Self::new(TypeKind::Qubit, Quantity::One, span)
    }

    pub fn qregister(dims: Vec<Type>, span: Span) -> Self {
        Self::new(TypeKind::QRegister(dims), Quantity::One, span)
    }

    pub fn tensor(dims: Vec<Type>, span: Span) -> Self {
        Self {
            kind: TypeKind::Tensor(dims),
            quantity: Quantity::Many,
            span,
            nat_value: None,
            int_width: None,
            sparse: false,
        }
    }

    pub fn function(params: Vec<Type>, ret: Box<Type>, span: Span) -> Self {
        Self {
            kind: TypeKind::Function(params, ret),
            quantity: Quantity::Many,
            span,
            nat_value: None,
            int_width: None,
            sparse: false,
        }
    }

    pub fn with_quantity(mut self, qty: Quantity) -> Self {
        self.quantity = qty;
        self
    }

    pub fn never(span: Span) -> Self {
        Self::new(TypeKind::Error, Quantity::Many, span)
    }

    pub fn to_ident(&self) -> Ident {
        // Simplified for now - would need proper implementation
        Ident::new("tmp", self.span)
    }
}

/// Core type kinds
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeKind {
    /// Unit type ()
    Unit,
    /// Boolean
    Bool,
    /// Signed integer
    Int,
    /// Unsigned integer
    UInt,
    /// 8-bit unsigned quantized integer (stored as u32, 4 per word)
    /// Used for trillion-parameter model compression: 1T params × 1 byte = 1TB raw,
    /// packed as u32 storage gives 4× density, dequantized on-demand to f32.
    Quint8,
    /// Float
    Float,
    /// String
    String,
    /// Char
    Char,
    /// Natural number (for dependent types)
    Nat,
    /// Quantum bit - linear resource
    Qubit,
    /// Quantum register with dimensions
    QRegister(Vec<Type>),
    /// Tensor with shape dimensions
    Tensor(Vec<Type>),
    /// Array type (homogeneous, fixed size if NatExpr provided)
    Array(Box<Type>, Option<NatExpr>),
    /// Tuple type
    Tuple(Vec<Type>),
    /// User-defined type (struct, enum, alias)
    Named(Ident, Vec<TypeArg>),
    /// Function type
    Function(Vec<Type>, Box<Type>),
    /// Reference/projection type (for inout)
    Projection(Box<Type>),
    /// Reversible computation type
    Reversible(Box<Type>),
    /// Dependent function type (Pi type)
    Pi(Ident, Box<Type>, Box<Type>),
    /// Dependent pair type (Sigma type)
    Sigma(Ident, Box<Type>, Box<Type>),
    /// Type-level lambda
    Lambda(Ident, Box<Type>),
    /// Type application
    App(Box<Type>, Box<Type>),
    /// Universe level
    Universe(u32),
    /// Type variable (for inference)
    Var(TypeVar),
    /// Metavariable (unsolved during inference)
    Meta(MetaVar),
    /// Error type (for recovery)
    Error,
}

/// Type arguments for generic instantiation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeArg {
    Type(Type),
    Nat(NatExpr),
    Quantity(Quantity),
}

/// Type variable (rigid, from user annotation)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeVar(pub u32);

impl fmt::Display for TypeVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?T{}", self.0)
    }
}

/// Metavariable (flexible, created during inference)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetaVar(pub u32);

impl fmt::Display for MetaVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?M{}", self.0)
    }
}

/// Natural number expressions (for dependent types)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NatExpr {
    /// A literal constant, carried as its value.
    ///
    /// Encoding a literal as a chain of `Succ` nodes makes it O(n) in memory
    /// and O(n) to read back, so `Tensor[f32, 1024]` cost ~1024 allocations for
    /// a number that fits in a `u64`.
    Lit(u64),
    Zero,
    Succ(Box<NatExpr>),
    Var(Ident),
    Add(Box<NatExpr>, Box<NatExpr>),
    Mul(Box<NatExpr>, Box<NatExpr>),
    /// Type-level if
    If(Box<Type>, Box<NatExpr>, Box<NatExpr>),
}

impl NatExpr {
    pub fn from_u64(n: u64) -> Self {
        NatExpr::Lit(n)
    }

    pub fn to_u64(&self) -> Option<u64> {
        match self {
            NatExpr::Lit(n) => Some(*n),
            NatExpr::Zero => Some(0),
            NatExpr::Succ(inner) => inner.to_u64().map(|n| n + 1),
            _ => None,
        }
    }
}

impl fmt::Display for NatExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NatExpr::Lit(n) => write!(f, "{n}"),
            NatExpr::Zero => write!(f, "0"),
            NatExpr::Succ(inner) => write!(f, "{} + 1", inner),
            NatExpr::Var(v) => write!(f, "{}", v),
            NatExpr::Add(a, b) => write!(f, "{} + {}", a, b),
            NatExpr::Mul(a, b) => write!(f, "{} * {}", a, b),
            NatExpr::If(cond, t, e) => write!(f, "if {} then {} else {}", cond, t, e),
        }
    }
}

/// Pretty printing for types
impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.quantity != Quantity::Many {
            write!(f, "{} ", self.quantity)?;
        }
        write!(f, "{}", self.kind)
    }
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeKind::Unit => write!(f, "()"),
            TypeKind::Bool => write!(f, "Bool"),
            TypeKind::Int => write!(f, "Int"),
            TypeKind::UInt => write!(f, "UInt"),
            TypeKind::Float => write!(f, "Float"),
            TypeKind::String => write!(f, "String"),
            TypeKind::Char => write!(f, "Char"),
            TypeKind::Nat => write!(f, "Nat"),
            TypeKind::Quint8 => write!(f, "quint8"),
            TypeKind::Qubit => write!(f, "Qubit"),
            TypeKind::QRegister(dims) => write!(
                f,
                "QRegister[{}]",
                dims.iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeKind::Tensor(dims) => write!(
                f,
                "Tensor[{}]",
                dims.iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeKind::Array(elem, size) => {
                if let Some(n) = size {
                    write!(f, "[{}; {}]", elem, n)
                } else {
                    write!(f, "[{}]", elem)
                }
            }
            TypeKind::Tuple(elems) => write!(
                f,
                "({})",
                elems
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeKind::Named(name, args) => {
                if args.is_empty() {
                    write!(f, "{}", name)
                } else {
                    write!(
                        f,
                        "{}<{}>",
                        name,
                        args.iter()
                            .map(|a| a.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            TypeKind::Function(params, ret) => {
                write!(f, "fn(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            TypeKind::Projection(inner) => write!(f, "&mut {}", inner),
            TypeKind::Reversible(inner) => write!(f, "reversible {}", inner),
            TypeKind::Pi(name, domain, codomain) => {
                write!(f, "Π({}: {}). {}", name, domain, codomain)
            }
            TypeKind::Sigma(name, fst, snd) => write!(f, "Σ({}: {}). {}", name, fst, snd),
            TypeKind::Lambda(param, body) => write!(f, "λ{}. {}", param, body),
            TypeKind::App(fun, arg) => write!(f, "{} {}", fun, arg),
            TypeKind::Universe(level) => write!(f, "Type{}", level),
            TypeKind::Var(v) => write!(f, "{}", v),
            TypeKind::Meta(m) => write!(f, "{}", m),
            TypeKind::Error => write!(f, "<error>"),
        }
    }
}

impl fmt::Display for TypeArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeArg::Type(t) => write!(f, "{}", t),
            TypeArg::Nat(n) => write!(f, "{}", n),
            TypeArg::Quantity(q) => write!(f, "{}", q),
        }
    }
}
