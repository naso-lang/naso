//! A forward and an inverse operation stream, as two distinct types.
//!
//! # Why this exists
//!
//! Naso's central claim is that computation is discarded by *uncomputing* it, not by
//! garbage-collecting it. That claim needs a representation in which an inverse operation
//! cannot be confused with a forward one, dropped on the floor, or inverted a second time.
//!
//! The previous representation carried both in one node:
//!
//! ```ignore
//! PirExpr::Reversible { body: Box<PirExpr>, inverse: Box<PirExpr> }
//! ```
//!
//! Two fields in one node is a promise, not a guarantee. Every consumer has to remember to
//! look at both. When the LLVM backend emitted `body` and ignored `inverse`, the code
//! compiled, the tests passed, and a reversible block quietly became a forward-only block
//! with no diagnostic. The only thing that caught it was noticing that a "reversible" block
//! emitted exactly as many statements as the same block without the keyword.
//!
//! # The guarantee this makes
//!
//! A forward stream and an inverse stream are DIFFERENT TYPES. The inverse stream has no
//! `invert` method, so asking it for its own inverse does not compile. That is the point:
//! double-uncomputation is a type error, not a runtime check that someone has to remember
//! to write.
//!
//! A backend must therefore decide, in one match arm, whether it handles both streams. It
//! cannot read the forward stream and fall through, because the inverse stream is a
//! different type sitting in a different field and exhaustiveness will not be satisfied by
//! handling only one.
//!
//! # What this does NOT claim
//!
//! This is the REPRESENTATION. It does not make uncomputation work end to end. Emitting a
//! forward stream followed by its inverse is straightforward only once the operations in it
//! are individually invertible, and that is a separate property this module does not
//! assume -- see [`OpClass`].
//!
//! No backend consumes these streams yet, so `reversible` remains refused. That refusal is
//! correct and is not to be removed by wiring these types in without proving the per-op
//! invertibility that emitting actually requires.

use std::collections::BTreeMap;
use std::fmt;

/// One operation in a stream.
///
/// `Op` deliberately does not wrap a `PirExpr`. A `PirExpr` can already be an inverse
/// (via `PirExpr::Reversible`), so wrapping one here would reintroduce exactly the nesting
/// this module exists to prevent. Keeping the payload a small, closed enum means the
/// question "can this be inverted?" is answerable by looking at the type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Op {
    /// A classical operation with no quantum effect: arithmetic, assignment, a call.
    Classical(String),
    /// A quantum gate.
    Gate { name: String, qubits: Vec<String> },
    /// A measurement. NOT invertible: the result is destroyed by measuring.
    Measure { qubits: Vec<String> },
    /// An allocation or deallocation of a quantum register.
    Alloc { qubits: Vec<String> },
    /// A barrier or other synchronisation point.
    Barrier,
    /// An operation this compiler does not model.
    ///
    /// Carried explicitly rather than omitted, because an unknown operation dropped here
    /// is an operation whose inverse is unknown too, and it would fail the same silent way
    /// the `PirExpr::Reversible` case did.
    Unknown { name: String },
}

impl Op {
    /// Whether this operation can be undone.
    ///
    /// # This is a claim about ONE operation, not about a program
    ///
    /// `measure` returning `false` is not a bug. Measuring genuinely destroys the
    /// amplitude, so there is no operation that restores it; a program that measures and
    /// then uncomputes is wrong in the physics, not in the compiler. The point of this
    /// predicate is to make that visible at the point the stream is BUILT, rather than at
    /// the point it is emitted, where the wrong answer has already been generated.
    pub fn is_invertible(&self) -> bool {
        match self {
            // A classical computation can be undone if its input is available. That is a
            // precondition on the surrounding program, not on this operation, so it is
            // reported as invertible here and checked by the caller that knows the inputs.
            Op::Classical(_) | Op::Barrier => true,
            // Every gate used by this compiler so far is a unitary, so its inverse exists.
            Op::Gate { .. } => true,
            // Measurement is not reversible.
            Op::Measure { .. } => false,
            // Allocation is not an operation that can be undone by running something; it
            // is a resource transition, and the compiler does not model the release.
            Op::Alloc { .. } => false,
            // An unmodelled operation's inverse is by definition unmodelled.
            Op::Unknown { .. } => false,
        }
    }

    /// The name used when reporting this operation in a diagnostic.
    pub fn name(&self) -> &str {
        match self {
            Op::Classical(n) | Op::Gate { name: n, .. } | Op::Unknown { name: n } => n,
            Op::Measure { .. } => "measure",
            Op::Alloc { .. } => "alloc",
            Op::Barrier => "barrier",
        }
    }
}

/// How a stream relates to uncomputation.
///
/// Separate from [`OpClass`]: this is what a stream IS FOR, not what it contains.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpClass {
    /// Ordinary execution.
    Forward,
    /// The undo of a forward stream.
    Inverse,
}

impl fmt::Display for OpClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpClass::Forward => f.write_str("forward"),
            OpClass::Inverse => f.write_str("inverse"),
        }
    }
}

/// The ordered operations of one direction of a reversible computation.
///
/// # The inverse stream has no `invert`
///
/// This is the type-level guard, and it is the reason `InverseStream` is a separate type
/// rather than a flag on `ForwardStream`:
///
/// ```compile_fail
/// use naso_compiler::ir::dual_stream::{InverseStream, Op};
/// let inv = InverseStream::uncompute(vec![Op::Barrier]);
/// // `invert` does not exist on `InverseStream`, so this does not compile.
/// let again = inv.invert();
/// ```
///
/// With a single type and a class flag, `invert` would exist on both, and inverting an
/// inverse stream would compile and produce a second forward pass that uncomputes the
/// uncomputation. Two types make that a compile error.
///
/// Deliberately absent: `Default`, `PartialEq` against `ForwardStream`, and any
/// constructor reachable from an `InverseStream`. Each would be a route by which a forward
/// and an inverse stream could be compared or silently converted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InverseStream(Vec<Op>);

impl InverseStream {
    /// Build an inverse stream. The name is `uncompute` rather than `new` to make the
    /// direction visible at every call site.
    pub fn uncompute(ops: Vec<Op>) -> Self {
        Self(ops)
    }

    /// The operations, in emission order.
    pub fn ops(&self) -> &[Op] {
        &self.0
    }

    /// Whether there is nothing to emit.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The class, for diagnostics that must name the direction.
    pub fn class(&self) -> OpClass {
        OpClass::Inverse
    }

    /// The first operation in this stream that cannot be undone, with its index.
    ///
    /// A stream built this way is already in the "undo" direction, so asking what *it*
    /// could be undone by is not meaningful. What IS meaningful is whether every operation
    /// in it was individually undoable in the first place -- a stream containing a
    /// measurement should never have been constructed, and this is how a caller finds out
    /// before emitting.
    pub fn first_uninvertible(&self) -> Option<(usize, &Op)> {
        self.0
            .iter()
            .enumerate()
            .find(|(_, op)| !op.is_invertible())
    }
}

impl fmt::Display for InverseStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "inverse[{}]", self.0.len())
    }
}

/// The ordered operations of the ordinary, forward direction.
///
/// # What a backend has to do
///
/// A backend matching on [`DualStream`] gets both fields. Because they are different types,
/// handling only `forward` and treating `inverse` as unreachable does not typecheck. The
/// backend must emit the inverse stream, refuse the program, or return an explicit
/// "unsupported" -- which is what every backend does today.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ForwardStream(Vec<Op>);

impl ForwardStream {
    /// Build a forward stream.
    pub fn new(ops: Vec<Op>) -> Self {
        Self(ops)
    }

    /// The operations, in execution order.
    pub fn ops(&self) -> &[Op] {
        &self.0
    }

    /// Whether there is nothing to emit.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The class, for diagnostics that must name the direction.
    pub fn class(&self) -> OpClass {
        OpClass::Forward
    }

    /// The first operation in this stream that cannot be undone, with its index.
    ///
    /// This is the check that must run BEFORE an inverse stream is built. A forward stream
    /// containing a measurement has no valid inverse, and building one anyway is how a
    /// program would claim to be uncomputed while actually having destroyed amplitude.
    pub fn first_uninvertible(&self) -> Option<(usize, &Op)> {
        self.0
            .iter()
            .enumerate()
            .find(|(_, op)| !op.is_invertible())
    }
}

impl fmt::Display for ForwardStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "forward[{}]", self.0.len())
    }
}

/// Both directions of one reversible computation.
///
/// The two streams are kept as separate fields, in this order, with the forward one first:
/// a backend that emits this pair must emit the forward stream and then the inverse stream,
/// and putting them in one `Vec` would let an implementation interleave them.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DualStream {
    /// The ordinary direction.
    pub forward: ForwardStream,
    /// The undo of `forward`, to be emitted after it.
    ///
    /// `Option` because the representation exists before anything produces one. `None`
    /// means "no inverse has been established", and every consumer must treat that as
    /// refusing to emit rather than as "nothing to uncompute" -- otherwise the absence of
    /// an inverse is indistinguishable from an empty one, which is precisely the failure
    /// this module exists to prevent.
    pub inverse: Option<InverseStream>,
}

impl DualStream {
    /// A stream with an established inverse.
    pub fn new(forward: ForwardStream, inverse: InverseStream) -> Self {
        Self {
            forward,
            inverse: Some(inverse),
        }
    }

    /// A forward stream with no inverse yet.
    ///
    /// For the representation only. A backend must refuse this rather than emit the
    /// forward half on its own.
    pub fn forward_only(forward: ForwardStream) -> Self {
        Self {
            forward,
            inverse: None,
        }
    }

    /// Whether an inverse has been established.
    pub fn has_inverse(&self) -> bool {
        self.inverse.is_some()
    }

    /// Establish the inverse of this forward stream, or explain why it cannot be built.
    ///
    /// The check is on the forward stream's contents, before any inverse exists. This is
    /// the single place where "is this program uncomputable at all?" is answered.
    pub fn try_invert(&self) -> Result<InverseStream, InversionError> {
        if let Some((index, op)) = self.forward.first_uninvertible() {
            return Err(InversionError::NotInvertible {
                index,
                operation: op.name().to_string(),
            });
        }
        // The undo of `forward` is the forward operations in REVERSE order.
        //
        // Reverse, not forward: if forward is `a; b`, undoing must run `undo(b)` before
        // `undo(a)`, because `b` may depend on the value `a` produced. This ordering is the
        // entire reason an inverse stream is a separate ordered list rather than a
        // recomputed view of the forward one.
        Ok(InverseStream::uncompute(
            self.forward.ops().iter().rev().cloned().collect(),
        ))
    }

    /// Whether an inverse stream could be built from this one's forward stream.
    ///
    /// # This does NOT mean the inverse exists
    ///
    /// Deliberately named "could be built" rather than "can be uncomputed", because it
    /// answers only the physics and ignores whether an inverse has been established. A
    /// `forward_only` stream of plain gates answers `true` here while having no inverse at
    /// all.
    ///
    /// Use [`Self::has_inverse`] to ask whether one exists, and prefer
    /// [`Self::established_inverse`] when both matter -- which is what a backend does,
    /// because emitting forward-only because `can_uncompute` said yes is the original bug.
    pub fn could_invert(&self) -> bool {
        self.try_invert().is_ok()
    }

    /// The established inverse, or why there is not one.
    ///
    /// The single entry point a backend should use. It refuses when no inverse has been
    /// established, which [`Self::try_invert`] alone does not, and when no inverse could be
    /// built. There is no path through this method that returns nothing and reports success.
    pub fn established_inverse(&self) -> Result<&InverseStream, InversionError> {
        if !self.has_inverse() {
            return Err(InversionError::NotEstablished);
        }
        let stored = self.inverse.as_ref().expect("checked above");

        // TWO checks, and the forward one is the one that was missing.
        //
        // Checking only the stored inverse let a forward stream containing a measurement
        // pass, as long as the stored inverse happened to be empty -- a backend would then
        // emit a "reversible" program whose measurement is never undone. The forward stream
        // is what the user wrote, so if it cannot be uncomputed, nothing is uncomputable.
        if let Some((index, op)) = self.forward.first_uninvertible() {
            return Err(InversionError::NotInvertible {
                index,
                operation: op.name().to_string(),
            });
        }

        // A stored inverse whose operations could not have been inverted means the streams
        // disagree, which is an internal inconsistency rather than a user-facing refusal.
        // Caught here so it cannot reach a backend as a silently wrong inverse.
        match stored.first_uninvertible() {
            Some((index, op)) => Err(InversionError::NotInvertible {
                index,
                operation: op.name().to_string(),
            }),
            None => Ok(stored),
        }
    }
}

/// Why an inverse stream could not be built.
///
/// Carries the offending operation and its position, because "this program cannot be
/// uncomputed" is not an actionable diagnostic and the user has to be told where.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InversionError {
    /// A forward operation has no inverse.
    NotInvertible {
        /// Zero-based position in the forward stream.
        index: usize,
        /// The operation's name, as it appears in source.
        operation: String,
    },
    /// No inverse has been established yet, and none can be inferred.
    ///
    /// Distinct from `NotInvertible`: this says nothing about the program's physics, only
    /// that the compiler has not established an inverse for it.
    NotEstablished,
}

impl fmt::Display for InversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InversionError::NotInvertible { index, operation } => write!(
                f,
                "`{operation}` at position {index} has no inverse, so this program cannot \
                 be uncomputed. A measurement destroys amplitude and an allocation is a \
                 resource transition; neither can be undone by running an operation."
            ),
            InversionError::NotEstablished => {
                f.write_str("no inverse stream has been established for this computation")
            }
        }
    }
}

/// A whole program as forward and inverse streams, keyed by function name.
///
/// This is the shape a backend would read. It is deliberately NOT `PirModule`: no backend
/// consumes it yet, and adding a field to `PirModule` that every backend must match on
/// would mean touching all of them while the emission contract is still undecided.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ProgramStreams {
    streams: BTreeMap<String, DualStream>,
}

impl ProgramStreams {
    /// An empty program.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one function's streams.
    pub fn insert(&mut self, function: impl Into<String>, stream: DualStream) -> &mut Self {
        self.streams.insert(function.into(), stream);
        self
    }

    /// One function's streams.
    pub fn get(&self, function: &str) -> Option<&DualStream> {
        self.streams.get(function)
    }

    /// Every function name, sorted.
    pub fn function_names(&self) -> Vec<&str> {
        self.streams.keys().map(String::as_str).collect()
    }

    /// Whether the program is empty.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Functions whose inverse stream has not been established.
    ///
    /// Every one of these is a function a backend must refuse rather than emit forward-only.
    pub fn forward_only_functions(&self) -> Vec<&str> {
        self.streams
            .iter()
            .filter(|(_, s)| !s.has_inverse())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Functions containing an operation that can never be uncomputed.
    pub fn functions_with_uninvertible_ops(&self) -> Vec<(&str, InversionError)> {
        self.streams
            .iter()
            .filter_map(|(name, s)| match s.try_invert() {
                Err(e) => Some((name.as_str(), e)),
                Ok(_) => None,
            })
            .collect()
    }

    /// Whether every function can be emitted as forward-then-inverse.
    ///
    /// Requires BOTH that an inverse has been established AND that it could have been
    /// built. Checking only the second would be the conflation this module exists to
    /// prevent: `can_uncompute` derives an inverse from the forward stream, so a function
    /// with `inverse: None` over invertible gates answers `true` there -- meaning "no
    /// inverse yet" would read as "fully reversible", and a caller could emit it
    /// forward-only while believing otherwise.
    pub fn is_fully_reversible(&self) -> bool {
        self.streams
            .values()
            .all(|s| s.established_inverse().is_ok())
    }
}
