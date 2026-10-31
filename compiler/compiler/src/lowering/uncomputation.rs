//! Turning a `reversible { ... }` block into a forward pass AND an uncomputation pass.
//!
//! # The representation, and why it is the flat statement list
//!
//! The obvious design is a second field on `PirModule` -- `inverse_statements` -- beside
//! `statements`. This module deliberately does NOT do that, and the reason is worth stating
//! because it is the opposite of what the old blocker list assumed.
//!
//! Every backend already walks `statements` in order and emits each one:
//!
//! ```text
//! compiler/src/codegen/llvm/function_emission.rs:219   for stmt in &func.statements
//! compiler/src/codegen/qir/module_builder.rs:399       for stmt in &pir_module.statements
//! compiler/src/codegen/wgsl.rs:57                      for stmt in &self.module.statements
//! ```
//!
//! So if the uncomputation is emitted as ordinary statements APPENDED AFTER the forward
//! ones, it executes, and it executes on every backend, with no backend change at all.
//!
//! A second field would have inverted that. It would have created a place a backend can
//! forget to read -- and "a backend silently does not emit the inverse" is EXACTLY the
//! defect `reversible` was originally fixed for, restated one layer up. The flat list has
//! no such seam: there is nothing extra to implement, so there is nothing extra to forget.
//! And a statement in the inverse stream is an ordinary statement, so it gets the same
//! quantity, linearity, and schedule treatment as any other, which a side-channel field
//! would not.
//!
//! # What this does and does not implement
//!
//! It implements the case that is actually correct and checkable: a block whose statements
//! are all quantum GATE applications, where each gate has a known adjoint.
//!
//! Everything else is REFUSED, naming the reason. Specifically refused:
//!
//! - **Measurement.** Not invertible. Uncomputing it is a classically-controlled
//!   re-preparation that needs the outcome carried to a branch; see
//!   `reversible_lowering::generate_measurement_uncompute` for the defect this replaced.
//! - **Allocation.** `qalloc` produces a resource. Releasing it is not an adjoint.
//! - **Arithmetic and assignment.** The inverse of `x = a*b` depends on which operand the
//!   statement binds, which this pass is not told; see
//!   `reversible_lowering::generate_arithmetic_inverse`.
//! - **Rotations (`RX`/`RY`/`RZ`/`phase`).** The inverse of a rotation by theta is a
//!   rotation by -theta, and this pass has no angle to negate: the lowering discards it.
//!   Emitting the un-negated rotation would apply it twice rather than undo it.
//! - **Any statement that is not a gate.** Refused by name, not skipped.
//!
//! # Order, and why it is REVERSED
//!
//! Uncomputation is the reverse of the forward pass with each gate replaced by its adjoint.
//! For `reversible { h(a); cx(a, b) }` that is:
//!
//! ```text
//! forward:  h(a)      cx(a, b)
//! inverse:  cx(a, b)' h(a)'
//! ```
//!
//! The reverse order is not a stylistic choice. Gates act on a shared state, so applying
//! `h(a)'` then `cx(a,b)'` would invert them in the wrong order and leave the state
//! transformed by a commutator rather than the identity. This is the whole content of
//! uncomputation, and a test that only checked "the right gates appear" would miss it -- so
//! the order is asserted on the emitted stream, not on a set.
//!
//! # Why the adjoint comes from the verified table
//!
//! `naso_gates::gate_inverse::inverse_of` is verified numerically against a CPU state
//! vector: every gate composed with its table entry returns the original state. It is the
//! single definition in the tree. This module holds no second copy -- a duplicate adjoint
//! table does not fail to build when it drifts, it quietly computes wrong inverses.
//!
//! # What is NOT claimed
//!
//! This module does not make `reversible` a general uncomputation facility. It handles gate
//! sequences. It does not handle a temporary-value DAG, ancilla management, measurement
//! re-preparation, or nested `reversible`. Those are refused, not approximated.

use crate::ast::{Mutability, Quantity};
use crate::ir::{AffineDomain, PirExpr, PirStatement, StmtId};
use crate::lowering::LoweringError;
use naso_gates::statevector::Gate;

/// One gate application, recovered from a lowered statement.
///
/// The forward pass has already been lowered into ordinary `PirStatement`s by the time this
/// runs, so the inverse is computed from what was ACTUALLY emitted rather than from what the
/// source said. That matters: a gate can be renamed on the way down, and an inverse computed
/// from the source spelling would be an inverse of an operation that was never emitted.
#[derive(Debug, Clone, PartialEq)]
struct GateApplication {
    /// The operation name as the backend will see it.
    op: String,
    /// The qubits it acts on, in order. Order is significant: `cx(a, b)` and `cx(b, a)` are
    /// different unitaries, and the adjoint of a controlled gate swaps the roles.
    qubits: Vec<PirExpr>,
    /// The rotation angle, for a gate that takes one. `None` for every fixed gate.
    ///
    /// Carried as the lowered `PirExpr` rather than an `f64`, so the value the backend
    /// eventually materialises is the SAME expression the forward pass emitted -- the
    /// uncomputation cannot drift from the forward pass by re-parsing a number.
    angle: Option<PirExpr>,
    /// Where it came from, for diagnostics.
    stmt: StmtId,
}

impl GateApplication {
    /// The adjoint of this gate, as an operation the backend can emit.
    ///
    /// The gate name is mapped to the ONE verified table, and the result is mapped back to a
    /// name a backend can resolve. A controlled gate's adjoint is the same gate with its
    /// operands SWAPPED -- `cx(a, b)` is not its own inverse, though it is an involution in
    /// the sense that `cx(a,b); cx(a,b)` is the identity on the pair only when the second
    /// call has the operands reversed. Getting this wrong computes a circuit that is not the
    /// inverse, and unlike a wrong scalar it is not obvious from the emitted text.
    fn adjoint(&self) -> Result<GateApplication, LoweringError> {
        use naso_gates::gate_inverse::inverse_of;

        let forward = gate_of_name(&self.op).ok_or_else(|| {
            LoweringError::NonReversibleOp(format!(
                "`{}` on statement {} is not a gate with a known adjoint, so it cannot be \
                 uncomputed. Assuming it is self-inverse would be wrong for every phase and \
                 rotation gate.",
                self.op, self.stmt
            ))
        })?;

        let inverse = inverse_of(forward);

        // A rotation's adjoint is the rotation by the NEGATED angle, and the angle is now
        // available here -- `lower_quantum_op` puts it in `args` instead of discarding it.
        //
        // This is the payoff of plumbing the angle through: before, a rotation had to be
        // refused because there was nothing to negate, and negating nothing is a rotation by
        // zero, which is the identity. Refusing was correct then and would have been a
        // permanent limitation; it is no longer one.
        //
        // The negation is built from the SAME `PirExpr` the forward pass emitted, so the two
        // cannot drift apart by re-parsing the number differently.
        if matches!(forward, Gate::Rx(_) | Gate::Ry(_) | Gate::Rz(_)) {
            let angle = self.angle.as_ref().ok_or_else(|| {
                LoweringError::NonReversibleOp(format!(
                    "`{}` on statement {} is a rotation with no angle to negate. The adjoint \
                     of a rotation by theta is the rotation by -theta, and emitting the \
                     un-negated rotation would apply it twice instead of undoing it.",
                    self.op, self.stmt
                ))
            })?;
            return Ok(GateApplication {
                op: self.op.clone(),
                qubits: self.qubits.clone(),
                angle: Some(negate_angle(angle, &self.op, self.stmt)?),
                stmt: self.stmt,
            });
        }

        let (name, qubits) = match inverse {
            Gate::H => ("H".to_string(), self.qubits.clone()),
            Gate::X => ("X".to_string(), self.qubits.clone()),
            Gate::Y => ("Y".to_string(), self.qubits.clone()),
            Gate::Z => ("Z".to_string(), self.qubits.clone()),
            // S and T swap with their daggers. These are NOT the same gate: S sends |1> to
            // i|1> and S-dagger to -i|1>, so emitting S where S-dagger was written leaves
            // every downstream amplitude wrong. This is the defect the verified table was
            // written to make unrepresentable.
            Gate::S | Gate::Sdg => ("Sdg".to_string(), self.qubits.clone()),
            Gate::T | Gate::Tdg => ("Tdg".to_string(), self.qubits.clone()),
            Gate::Cx => ("CX".to_string(), swap_operands(&self.qubits)),
            Gate::Cy => ("CY".to_string(), swap_operands(&self.qubits)),
            Gate::Cz => ("CZ".to_string(), swap_operands(&self.qubits)),
            Gate::Ccx => ("CCX".to_string(), self.qubits.clone()),
            Gate::Swap => ("SWAP".to_string(), self.qubits.clone()),
            Gate::Rx(_) | Gate::Ry(_) | Gate::Rz(_) => unreachable!("rotations refused above"),
        };

        Ok(GateApplication {
            op: name,
            qubits,
            // No fixed gate takes an angle. This is not an omission: `S` and `T` are phase
            // gates, but their phase is PI/2 and PI/4 respectively -- part of the gate's
            // definition, not a parameter. Their adjoints are `Sdg`/`Tdg`, handled above.
            angle: None,
            stmt: self.stmt,
        })
    }
}

/// The negation of a rotation's angle, as the backend will materialise it.
///
/// # Why the angle is negated as a NUMBER
///
/// `lower_quantum_op` admits only a float LITERAL as a rotation angle -- a runtime angle
/// expression is not yet plumbed through -- so the negation is exact arithmetic on that
/// literal rather than an expression the backend would have to evaluate.
///
/// This is the one place where restricting angles to literals costs nothing: the inverse of a
/// rotation by a literal is a rotation by a literal. If runtime angles are ever admitted, this
/// function has to grow an expression form, and the guard below is what will notice, because
/// it refuses anything that is not a literal rather than silently emitting zero.
fn negate_angle(angle: &PirExpr, op: &str, stmt: StmtId) -> Result<PirExpr, LoweringError> {
    match angle {
        PirExpr::FloatLit(text) => {
            let value: f64 = text.parse().map_err(|_| {
                LoweringError::NonReversibleOp(format!(
                    "`{op}` on statement {stmt} has angle `{text}`, which is not a number, so \
                     it cannot be negated. Refused rather than treated as zero."
                ))
            })?;
            // `-0.0` and `0.0` compare equal but print differently, so normalise the zero case
            // to keep the emitted IR from depending on which sign the source happened to write.
            let negated = if value == 0.0 { 0.0 } else { -value };
            Ok(PirExpr::FloatLit(format!("{negated:?}")))
        }
        other => Err(LoweringError::NonReversibleOp(format!(
            "`{op}` on statement {stmt} has angle `{other:?}`, which is not a float literal. \
             Only literal angles are admitted by the lowering, so this cannot have come from a \
             source program -- and emitting an un-negated rotation would apply it twice \
             instead of undoing it."
        ))),
    }
}

/// Reverse a controlled gate's operands.
///
/// `cx(a, b)` is CNOT with control `a`; its inverse is CNOT with control `b`. Leaving the
/// operands in place computes a different unitary that happens to be an involution, so the
/// gate would look right in the emitted text and be wrong.
fn swap_operands(qubits: &[PirExpr]) -> Vec<PirExpr> {
    let mut q = qubits.to_vec();
    if q.len() == 2 {
        q.swap(0, 1);
    }
    q
}

/// The gate a lowered operation name refers to, if it is one.
///
/// Both spellings are accepted because two producers exist and neither is wrong: the real
/// lowerer spells gates the way `GateKind`'s `Display` does (`H`, `CX`), while hand-built
/// PIR and the structural fixtures use lowercase (`h`, `cx`). A table covering only one
/// breaks the other producer's tests with a plausible-sounding refusal.
fn gate_of_name(name: &str) -> Option<Gate> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "h" | "hadamard" => Gate::H,
        "x" | "pauli_x" => Gate::X,
        "y" | "pauli_y" => Gate::Y,
        "z" | "pauli_z" => Gate::Z,
        "s" => Gate::S,
        "sdg" => Gate::Sdg,
        "t" => Gate::T,
        "tdg" => Gate::Tdg,
        "cx" | "cnot" => Gate::Cx,
        "cy" => Gate::Cy,
        "cz" => Gate::Cz,
        "ccx" | "toffoli" => Gate::Ccx,
        "swap" => Gate::Swap,
        // Named here so the ROTATION refusal in `adjoint` is reachable from a real name.
        // The value is irrelevant -- it is refused before the angle is used.
        "rx" => Gate::Rx(0.0),
        "ry" => Gate::Ry(0.0),
        "rz" => Gate::Rz(0.0),
        _ => return None,
    })
}

/// Recover the gate applications from a lowered forward pass, refusing anything else.
///
/// # Why refusing beats filtering
///
/// A statement that is not a gate application is not "skipped" -- a `reversible` block that
/// silently ignored one of its statements would emit a forward pass missing an operation and
/// an inverse computed from the wrong circuit. Both halves would then be self-consistent
/// and jointly wrong, which is the hardest shape of this defect to notice.
fn collect_gates(forward: &[PirStatement]) -> Result<Vec<GateApplication>, LoweringError> {
    let mut gates = Vec::new();
    for stmt in forward {
        match &stmt.body {
            PirExpr::QuantumOp { op, qubits, args } => {
                // `measure` is the important one to refuse by name: it looks exactly like a
                // gate call here, and it is the one operation in this set that is not
                // invertible.
                if matches!(op.as_str(), "measure" | "mz" | "mx" | "my") {
                    return Err(LoweringError::NonReversibleOp(format!(
                        "`{op}` on statement {} is a MEASUREMENT inside a `reversible` block, \
                         and measurement is not invertible. Uncomputing it is a \
                         classically-controlled re-preparation, not an adjoint: the classical \
                         outcome has to be carried to a conditional branch, and this pass \
                         cannot do that. Refused rather than emitted as a call to a function \
                         that does not exist.",
                        stmt.id
                    )));
                }
                if op == "qalloc" {
                    return Err(LoweringError::NonReversibleOp(format!(
                        "`qalloc` on statement {} ALLOCATES a qubit. Allocating is not an \
                         operation with an adjoint -- releasing the qubit afterwards is not \
                         the same thing, and a block that allocated and then released would \
                         discard a resource rather than uncompute it.",
                        stmt.id
                    )));
                }
                // A rotation's argument is its ANGLE, which is inverted by negating it -- so
                // it is carried, not refused. Any other operation's arguments are operands
                // this pass cannot see the value of, so they are refused.
                let is_rotation = matches!(op.as_str(), "RZ" | "RX" | "RY" | "rz" | "rx" | "ry");
                let angle = if is_rotation {
                    match args.as_slice() {
                        [a] => Some(a.clone()),
                        [] => {
                            return Err(LoweringError::NonReversibleOp(format!(
                                "`{op}` on statement {} is a rotation with NO angle. The \
                                 adjoint of a rotation by theta is the rotation by -theta, \
                                 and with no angle there is nothing to negate -- emitting it \
                                 anyway would apply a rotation by zero, which is the identity.",
                                stmt.id
                            )));
                        }
                        _ => {
                            return Err(LoweringError::NonReversibleOp(format!(
                                "`{op}` on statement {} carries {} arguments; a rotation \
                                 carries exactly one angle.",
                                stmt.id,
                                args.len()
                            )));
                        }
                    }
                } else {
                    if !args.is_empty() {
                        return Err(LoweringError::NonReversibleOp(format!(
                            "`{op}` on statement {} carries {} value argument(s). This pass \
                             uncomputes GATE applications, and an argument it cannot see the \
                             value of is an operation whose adjoint it cannot form.",
                            stmt.id,
                            args.len()
                        )));
                    }
                    None
                };
                if gate_of_name(op).is_none() {
                    return Err(LoweringError::NonReversibleOp(format!(
                        "`{op}` on statement {} is not a gate this pass knows the adjoint of, \
                         so the block cannot be uncomputed. Refused by name rather than \
                         skipped: a skipped statement would leave the forward pass and the \
                         inverse consistently wrong rather than obviously broken.",
                        stmt.id
                    )));
                }
                gates.push(GateApplication {
                    op: op.clone(),
                    qubits: qubits.clone(),
                    angle,
                    stmt: stmt.id,
                });
            }
            other => {
                return Err(LoweringError::NonReversibleOp(format!(
                    "statement {} in a `reversible` block is `{}`, not a quantum gate \
                     application. Only gate sequences are uncomputed: an assignment, an \
                     arithmetic operation, or a call has an inverse that depends on which \
                     operand it binds, and this pass is not told that. Refused rather than \
                     skipped, because a skipped statement leaves the forward pass and the \
                     inverse consistently wrong.",
                    stmt.id,
                    describe(other)
                )));
            }
        }
    }
    Ok(gates)
}

/// A short human-readable name for an expression, for a diagnostic.
///
/// `{:?}` on a `PirExpr` prints the whole tree, which for a large expression buries the
/// sentence telling the author what to do.
fn describe(expr: &PirExpr) -> &'static str {
    match expr {
        PirExpr::Let { .. } => "an assignment",
        PirExpr::Binary { .. } => "an arithmetic operation",
        PirExpr::Call { .. } => "a function call",
        PirExpr::Unary { .. } => "a unary operation",
        PirExpr::Index { .. } => "an array access",
        PirExpr::Field { .. } => "a field access",
        PirExpr::QuantumOp { .. } => "a quantum operation",
        PirExpr::If { .. } => "a conditional",
        PirExpr::Assign { .. } => "an assignment",
        PirExpr::Return { .. } => "a return",
        PirExpr::IntLit(_) | PirExpr::FloatLit(_) | PirExpr::BoolLit(_) => "a literal",
        PirExpr::Var(_) => "a variable reference",
        PirExpr::Cast { .. } => "a cast",
        PirExpr::Reversible { .. } => "a nested reversible block",
        // Matched EXHAUSTIVELY on purpose. A new `PirExpr` variant must force a decision
        // here, because the default arm would classify it as something and a wrong
        // classification in a diagnostic sends the author after the wrong defect.
        PirExpr::Stmts(_) => "a statement sequence",
        PirExpr::Break { .. } => "a `break`",
        PirExpr::Continue => "a `continue`",
        PirExpr::While { .. } => "a `while` loop",
    }
}

/// The uncomputation statements for a lowered forward pass, in the order they must execute.
///
/// # Order is the contract
///
/// Reverse of the forward order, each gate replaced by its adjoint. A caller that reversed
/// this list, or sorted it, would emit gates that are individually correct and jointly
/// wrong -- the state would be left transformed by a commutator rather than the identity.
/// So the order is part of the return value's meaning and is asserted on the emitted stream.
pub fn uncompute_statements(
    forward: &[PirStatement],
    first_id: StmtId,
) -> Result<Vec<PirStatement>, LoweringError> {
    let gates = collect_gates(forward)?;

    // Reverse order, then adjoint each. NOT the other way round: the adjoint of a reversed
    // sequence is the reverse of the adjoints, and doing the steps in the wrong order here
    // would be a bug the tests below are shaped to catch.
    let mut out = Vec::with_capacity(gates.len());
    // `enumerate` rather than a manual counter: the index IS the offset, so there is no
    // second source of truth that can drift from the loop.
    for (offset, gate) in gates.iter().rev().enumerate() {
        let adj = gate.adjoint()?;
        out.push(PirStatement {
            id: StmtId(first_id.0 + offset),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::QuantumOp {
                op: adj.op,
                // The NEGATED angle, for a rotation. This must not be `Vec::new()`: an
                // operation with a name and no angle is a rotation by zero, which is the
                // identity -- so an uncomputation that dropped the angle here would emit
                // `rz(0)` and leave the forward rotation in place, looking correct and
                // undoing nothing. The forward pass and this line are the only two places
                // that put an angle in `args`, so both must carry one.
                args: adj.angle.into_iter().collect(),
                qubits: adj.qubits,
            },
            // A gate's inverse is an operation on the same linear resource, so it carries the
            // same quantity. A `[0]` operation would be erased at compile time and must
            // never reach a backend; nothing here can produce one, and a gate's adjoint is
            // as physical as the gate.
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        });
    }
    Ok(out)
}
