//! Naso Expression AST
//!
//! Defines expressions including reversible blocks, inout projections,
//! quantum operations, and quantitative annotations.

use crate::ast::{Ident, Literal, Mutability, NodeId, Quantity, Span};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Expression with metadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Option<crate::ast::ty::Type>,
    pub quantity: Quantity,
    pub span: Span,
    pub id: NodeId,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span, id: NodeId) -> Self {
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

/// Expression kinds
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExprKind {
    /// Literal value
    Literal(Literal),
    /// Variable reference
    Var(Ident),
    /// Binary operation
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Unary operation
    Unary(UnOp, Box<Expr>),
    /// Function call
    Call(Box<Expr>, Vec<Expr>),
    /// Method call
    MethodCall(Box<Expr>, Ident, Vec<Expr>),
    /// Field access
    Field(Box<Expr>, Ident),
    /// Index access
    Index(Box<Expr>, Box<Expr>),
    /// Struct construction
    Struct(Ident, Vec<FieldExpr>),
    /// Enum variant construction
    Variant(Ident, Ident, Vec<Expr>),
    /// Tuple construction
    Tuple(Vec<Expr>),
    /// Array/list construction
    Array(Vec<Expr>),
    /// Block expression
    Block(Box<crate::ast::Block>),
    /// If expression
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    /// Match expression
    Match(Box<Expr>, Vec<MatchArm>),
    /// Let binding
    Let(Box<LetBinding>),
    /// Mutable binding (inout projection)
    LetInOut(Box<LetInOutBinding>),
    /// Consume binding (linear move)
    LetConsume(Box<LetConsumeBinding>),
    /// Reversible block
    Reversible(Box<ReversibleBlock>),
    /// Lambda/closure
    Lambda(Box<LambdaExpr>),
    /// For loop
    For(Box<ForLoop>),
    /// Forall loop (parallel polyhedral loop)
    Forall(Box<ForallLoop>),
    /// While loop
    While(Box<Expr>, Box<Expr>),
    /// Return expression
    Return(Option<Box<Expr>>),
    /// Break expression
    Break(Option<Box<Expr>>),
    /// Continue expression
    Continue,
    /// Assignment
    Assign(Box<Expr>, Box<Expr>),
    /// Inout projection creation
    Projection(Box<Expr>),
    /// Quantum operations
    QuantumOp(QuantumOp),
    /// Type ascription
    Ascribe(Box<Expr>, crate::ast::ty::Type),
    /// Error placeholder
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldExpr {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: crate::ast::pattern::Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetBinding {
    pub name: Ident,
    pub ty: Option<crate::ast::ty::Type>,
    pub quantity: Quantity,
    pub mutability: Mutability,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetInOutBinding {
    pub name: Ident,
    pub ty: Option<crate::ast::ty::Type>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetConsumeBinding {
    pub name: Ident,
    pub ty: Option<crate::ast::ty::Type>,
    pub value: Expr,
    pub span: Span,
}

/// Reversible computation block
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReversibleBlock {
    pub body: crate::ast::Block,
    pub uncomputes: Vec<UncomputeStep>,
    pub span: Span,
}

/// A proof block: `proof { .. }`.
///
/// Holds verification obligations. The body is a normal block, so `assert(..)`
/// and quantified expressions in it are typechecked exactly as they would be
/// at runtime. That is deliberate: an obligation that cannot be stated in the
/// language's own type system would be unenforceable.
///
/// Nothing in a proof block reaches codegen. The block is erased, and the
/// `naso-verify` crate reads the obligations instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofBlock {
    pub body: crate::ast::Block,
    pub span: Span,
}

/// Step in the uncomputation DAG
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UncomputeStep {
    pub target: Ident,
    pub inverse_expr: Expr,
    pub dependencies: Vec<Ident>,
    pub span: Span,
}

/// Lambda expression
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LambdaExpr {
    pub params: Vec<crate::ast::Param>,
    pub ret_ty: Option<crate::ast::ty::Type>,
    pub body: Box<Expr>,
    pub span: Span,
    pub captures: Vec<Capture>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub name: Ident,
    pub mode: CaptureMode,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMode {
    ByValue,
    ByInOut,
    ByConsume,
}

/// For loop
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForLoop {
    pub var: Ident,
    pub iter: Expr,
    pub body: crate::ast::Block,
    pub span: Span,
}

/// Forall loop (parallel polyhedral loop)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForallLoop {
    /// Multiple loop bindings: (var, lower, upper)
    pub bindings: Vec<(Ident, Expr, Expr)>,
    pub body: crate::ast::Block,
    pub span: Span,
}

/// Binary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Assign,
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinOp::Add => write!(f, "+"),
            BinOp::Sub => write!(f, "-"),
            BinOp::Mul => write!(f, "*"),
            BinOp::Div => write!(f, "/"),
            BinOp::Rem => write!(f, "%"),
            BinOp::Eq => write!(f, "=="),
            BinOp::Ne => write!(f, "!="),
            BinOp::Lt => write!(f, "<"),
            BinOp::Le => write!(f, "<="),
            BinOp::Gt => write!(f, ">"),
            BinOp::Ge => write!(f, ">="),
            BinOp::And => write!(f, "&&"),
            BinOp::Or => write!(f, "||"),
            BinOp::BitAnd => write!(f, "&"),
            BinOp::BitOr => write!(f, "|"),
            BinOp::BitXor => write!(f, "^"),
            BinOp::Shl => write!(f, "<<"),
            BinOp::Shr => write!(f, ">>"),
            BinOp::Assign => write!(f, "="),
        }
    }
}

/// Unary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    Deref,
    /// Inout borrow
    InOut,
    /// Consume move
    Consume,
}

impl fmt::Display for UnOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnOp::Neg => write!(f, "-"),
            UnOp::Not => write!(f, "!"),
            UnOp::BitNot => write!(f, "~"),
            UnOp::Deref => write!(f, "*"),
            UnOp::InOut => write!(f, "&mut"),
            UnOp::Consume => write!(f, "consume"),
        }
    }
}

/// Quantum operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuantumOp {
    /// Allocate new qubit
    Alloc(Ident),
    /// Measure qubit
    Measure(Box<Expr>),
    /// Apply gate
    ApplyGate(GateKind, Vec<Expr>),
    /// Entangle qubits
    Entangle(Vec<Expr>),
    /// Quantum phase
    Phase(Box<Expr>, Box<Expr>),
    /// Hamiltonian evolution
    Hamiltonian(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GateKind {
    H,
    X,
    Y,
    Z,
    S,
    T,
    CX,
    CY,
    CZ,
    RX(Box<Expr>),
    RY(Box<Expr>),
    RZ(Box<Expr>),
    Custom(Ident),
}

impl fmt::Display for GateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateKind::H => write!(f, "H"),
            GateKind::X => write!(f, "X"),
            GateKind::Y => write!(f, "Y"),
            GateKind::Z => write!(f, "Z"),
            GateKind::S => write!(f, "S"),
            GateKind::T => write!(f, "T"),
            GateKind::CX => write!(f, "CX"),
            GateKind::CY => write!(f, "CY"),
            GateKind::CZ => write!(f, "CZ"),
            GateKind::RX(_) => write!(f, "RX"),
            GateKind::RY(_) => write!(f, "RY"),
            GateKind::RZ(_) => write!(f, "RZ"),
            GateKind::Custom(ident) => write!(f, "{}", ident),
        }
    }
}

/// Pretty printing for expressions (basic)
impl fmt::Display for ExprKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprKind::Literal(l) => write!(f, "{}", l),
            ExprKind::Var(v) => write!(f, "{}", v),
            ExprKind::Binary(op, l, r) => write!(f, "({:?} {} {:?})", l, op, r),
            ExprKind::Unary(op, e) => write!(f, "({}{:?})", op, e),
            ExprKind::Call(fun, _args) => write!(f, "{:?}(...)", fun),
            ExprKind::Block(_) => write!(f, "{{ ... }}"),
            ExprKind::If(c, t, e) => {
                write!(f, "if {:?} then {:?} else {:?}", c, t, e)
            }
            ExprKind::Let(b) => write!(f, "let {:?} = {:?}", b.name, b.value),
            ExprKind::Reversible(_) => write!(f, "reversible {{ ... }}"),
            ExprKind::QuantumOp(q) => write!(f, "quantum {:?}", q),
            ExprKind::Error => write!(f, "<error>"),
            _ => write!(f, "<expr>"),
        }
    }
}
