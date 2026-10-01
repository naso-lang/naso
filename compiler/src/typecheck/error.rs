//! Error types for the Naso type checker

use crate::ast::*;
use thiserror::Error;

/// Type checker errors with span information
#[derive(Debug, Clone, Error)]
pub enum TypeError {
    #[error("variable `{name}` not available: {reason}")]
    VariableNotAvailable {
        name: Ident,
        reason: String,
        span: Span,
    },

    #[error(
        "linear variable `{name}` used twice (first at {first_use:?}, second at {second_use:?})"
    )]
    LinearVariableUsedTwice {
        name: Ident,
        first_use: Span,
        second_use: Span,
    },

    #[error("use of moved value `{name}` (moved at {moved_at:?}, used at {used_at:?})")]
    UseOfMovedValue {
        name: Ident,
        moved_at: Span,
        used_at: Span,
    },

    #[error("inout aliasing: `{var}` aliases with existing inout borrow of `{existing_borrow}`")]
    InOutAliasing {
        var: Ident,
        existing_borrow: Ident,
        existing_span: Span,
        new_span: Span,
    },

    #[error("unused linear variable `{name}` (defined at {defined_at:?})")]
    UnusedLinearVariable { name: Ident, defined_at: Span },

    #[error("type mismatch: expected `{expected}`, found `{found}`")]
    TypeMismatch {
        expected: Type,
        found: Type,
        span: Span,
    },

    #[error("quantity mismatch: expected `{expected}`, found `{found}`")]
    QuantityMismatch {
        expected: Quantity,
        found: Quantity,
        span: Span,
    },

    #[error("argument count mismatch: expected {expected}, found {found}")]
    ArgumentCountMismatch {
        expected: usize,
        found: usize,
        span: Span,
    },

    #[error("type argument count mismatch for `{name}`: expected {expected}, found {found}")]
    TypeArgumentCountMismatch {
        name: Ident,
        expected: usize,
        found: usize,
        span: Span,
    },

    #[error("a quantified proposition must have a boolean body: `forall i in a..b {{ <expr> }}`")]
    QuantifiedBodyNotBool { span: Span },

    #[error("`assert` may only be used inside a `proof {{ .. }}` block")]
    AssertOutsideProof { span: Span },

    #[error("cannot cast `{from}` to `{to}`: casts are only defined between numeric types")]
    InvalidCast { from: Type, to: Type, span: Span },

    #[error("field `{field}` not found in type `{ty}`")]
    FieldNotFound { field: Ident, ty: Type, span: Span },

    #[error("variant `{variant_name}` not found in enum `{enum_name}`")]
    VariantNotFound {
        enum_name: Ident,
        variant_name: Ident,
        span: Span,
    },

    #[error("type `{name}` is not a struct")]
    NotAStruct { name: Ident, span: Span },

    #[error("type `{ty}` is not a function")]
    NotAFunction { ty: Type, span: Span },

    #[error("type `{ty}` is not indexable")]
    NotIndexable { ty: Type, span: Span },

    #[error("inout binding requires unique ownership (quantity 1), found `{found_qty}`")]
    InOutRequiresUnique { found_qty: Quantity, span: Span },

    #[error("occurs check failed: metavariable `{meta_var}` occurs in type `{ty}`")]
    OccursCheck {
        meta_var: MetaVar,
        ty: Type,
        span: Span,
    },

    #[error("undefined type `{name}`")]
    UndefinedType { name: Ident, span: Span },

    #[error(
        "erased variable `{name}` used at runtime (quantity 0 variables cannot appear in runtime positions)"
    )]
    ErasedVariableUsedAtRuntime { name: Ident, span: Span },

    #[error("impure operation in reversible block: `{operation}`")]
    ImpureInReversible { operation: String, span: Span },

    #[error("missing uncomputation step for variable `{name}` in reversible block")]
    MissingUncompute { name: Ident, span: Span },

    #[error("cyclic uncomputation dependency involving `{name}`")]
    CyclicUncompute { name: Ident, span: Span },

    #[error("qubit must have quantity 1 (linear)")]
    QubitQuantityMismatch { found: Quantity, span: Span },

    #[error("tuple pattern arity mismatch: expected {tuple_len} elements, found {pattern_len}")]
    PatternTupleArityMismatch {
        pattern_len: usize,
        tuple_len: usize,
        span: Span,
    },

    #[error("measurement requires qubit with quantity 1 (consume)")]
    MeasureRequiresConsumeQubit { span: Span },

    #[error("entangle requires qubits with quantity 1 (consume)")]
    EntangleRequiresConsumeQubits { span: Span },

    #[error("generic type parameter mismatch")]
    GenericMismatch { message: String, span: Span },

    #[error("dependent type evaluation failed: {message}")]
    DependentTypeError { message: String, span: Span },

    #[error("pattern match is non-exhaustive")]
    NonExhaustiveMatch { span: Span },

    #[error("break/continue outside of loop")]
    ControlFlowOutsideLoop { span: Span },

    #[error("return outside of function")]
    ReturnOutsideFunction { span: Span },

    #[error("inference failed: {message}")]
    InferenceError { message: String, span: Span },
}

/// Result type for type checking operations
pub type TypeResult<T> = Result<T, TypeError>;
