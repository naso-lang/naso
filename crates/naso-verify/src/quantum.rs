//! Quantum uncomputation encoding for SMT-LIB2.
//!
//! This module translates Naso's quantum operations and uncomputation obligations
//! into SMT-LIB2 constraints using symbolic unitary matrices and bitvector reasoning.

use crate::error::VerifyError;
use crate::smtlib::{Sort, Term, builder::*};
use indexmap::IndexMap;
use naso_compiler::ast::Span;
use naso_compiler::ast::expr::{ExprKind, GateKind as AstGateKind};

/// Quantum gate kind for symbolic representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    RX,
    RY,
    RZ,
    Measure,
    Reset,
    Unitary,
}

impl GateKind {
    /// Convert from AST GateKind.
    pub fn from_ast(gate: &AstGateKind) -> Self {
        match gate {
            AstGateKind::H => GateKind::H,
            AstGateKind::X => GateKind::X,
            AstGateKind::Y => GateKind::Y,
            AstGateKind::Z => GateKind::Z,
            AstGateKind::S => GateKind::S,
            AstGateKind::T => GateKind::T,
            AstGateKind::CX => GateKind::CX,
            AstGateKind::CY => GateKind::CY,
            AstGateKind::CZ => GateKind::CZ,
            AstGateKind::RX(_) => GateKind::RX,
            AstGateKind::RY(_) => GateKind::RY,
            AstGateKind::RZ(_) => GateKind::RZ,
            AstGateKind::Custom(_) => GateKind::Unitary,
        }
    }

    /// Check if this gate is Clifford (efficiently simulable).
    pub fn is_clifford(&self) -> bool {
        matches!(
            self,
            GateKind::H
                | GateKind::CX
                | GateKind::CY
                | GateKind::CZ
                | GateKind::S
                | GateKind::X
                | GateKind::Y
                | GateKind::Z
                | GateKind::Measure
                | GateKind::Reset
        )
    }
}

/// Symbolic qubit with its state constraints.
#[derive(Debug, Clone)]
pub struct SymbolicQubit {
    pub id: String,
    /// Source-level binding this qubit was allocated under (`let q = qalloc(1)`).
    ///
    /// Gates are written against source names, so without this the lookup in
    /// `apply_gate` could never resolve and every gate silently did nothing.
    pub name: String,
    pub span: Span,
    /// Whether this qubit is a temporary (must be uncomputed)
    pub is_temp: bool,
    /// Current symbolic state (simplified: basis state index for computational basis)
    pub state_var: String,
    /// Unitary operations applied to this qubit (for uncomputation proof)
    pub operations: Vec<QubitOp>,
}

/// Operation applied to a qubit.
#[derive(Debug, Clone)]
pub struct QubitOp {
    pub gate: GateKind,
    pub target_qubits: Vec<String>,
    pub control_qubits: Vec<String>,
    pub span: Span,
}

/// Tracks quantum state during lowering.
pub struct QuantumTracker {
    /// All allocated qubits
    pub qubits: IndexMap<String, SymbolicQubit>,
    /// Temporary qubits that must be uncomputed
    pub temp_qubits: Vec<String>,
    /// Unitary matrix variables for symbolic reasoning
    pub unitary_vars: IndexMap<String, Term>,
    /// Next qubit ID
    pub next_qubit_id: u32,
    /// Source binding name for the next `qalloc`, set by the prover while
    /// encoding a `let` statement.
    pub pending_name: Option<String>,
    /// Current function for scoping
    pub current_function: Option<String>,
}

impl QuantumTracker {
    pub fn new() -> Self {
        Self {
            qubits: IndexMap::new(),
            temp_qubits: Vec::new(),
            unitary_vars: IndexMap::new(),
            next_qubit_id: 0,
            pending_name: None,
            current_function: None,
        }
    }

    /// Allocate a new qubit (qalloc).
    ///
    /// If the caller has set `pending_name` -- the prover does this when
    /// encoding `let q = qalloc(1)` -- the qubit is recorded under that
    /// source name so later gates can resolve it.
    pub fn allocate_qubit(&mut self, is_temp: bool, span: Span) -> String {
        let id = format!("q_{}", self.next_qubit_id);
        self.next_qubit_id += 1;

        let name = self.pending_name.take().unwrap_or_else(|| id.clone());
        let state_var = format!("state_{}", id);
        let qubit = SymbolicQubit {
            id: id.clone(),
            name,
            span,
            is_temp,
            state_var: state_var.clone(),
            operations: Vec::new(),
        };

        self.qubits.insert(id.clone(), qubit);
        if is_temp {
            self.temp_qubits.push(id.clone());
        }
        id
    }

    /// Drop every temporary qubit whose name is bound by the function's return
    /// value.
    ///
    /// A returned qubit is handed to the caller rather than discarded, so it is
    /// not a temporary that must be uncomputed to |0> -- returning a Bell pair
    /// in superposition is the whole point. Without this, every valid quantum
    /// routine would be reported as leaking.
    pub fn mark_returned(&mut self, names: &[String]) {
        for name in names {
            let returned: Vec<String> = self
                .qubits
                .values()
                .filter(|q| &q.name == name)
                .map(|q| q.id.clone())
                .collect();
            for id in returned {
                self.temp_qubits.retain(|t| t != &id);
                if let Some(q) = self.qubits.get_mut(&id) {
                    q.is_temp = false;
                }
            }
        }
    }

    /// Apply a gate to qubits.
    pub fn apply_gate(
        &mut self,
        gate: GateKind,
        targets: &[String],
        controls: &[String],
        span: Span,
    ) {
        for target in targets {
            // Gates are written against source names (`hadamard(q0)`), so
            // resolve by name first and fall back to the raw id.
            let id = match self.qubits.get(target) {
                Some(_) => Some(target.clone()),
                None => self
                    .qubits
                    .values()
                    .find(|q| &q.name == target)
                    .map(|q| q.id.clone()),
            };
            if let Some(id) = id
                && let Some(qubit) = self.qubits.get_mut(&id)
            {
                qubit.operations.push(QubitOp {
                    gate,
                    target_qubits: targets.to_vec(),
                    control_qubits: controls.to_vec(),
                    span,
                });
            }
        }
    }

    /// Get all temporary qubit IDs.
    pub fn temp_qubit_ids(&self) -> &[String] {
        &self.temp_qubits
    }

    /// Get qubit by ID.
    pub fn get_qubit(&self, id: &str) -> Option<&SymbolicQubit> {
        self.qubits.get(id)
    }

    /// Fold a qubit's operation list into a single symbolic state term.
    ///
    /// State encoding: `0` = |0>, `1` = |1>, `2` = superposition OR "not
    /// provably |0>". The third value is deliberately an over-approximation:
    /// anything this prover cannot demonstrate to be |0> becomes `2`, so an
    /// unmodelled gate makes the prover *less* permissive, never more. A
    /// verifier must never report "safe" on a gate it does not understand.
    fn folded_state(&self, qubit: &SymbolicQubit) -> Term {
        let mut state = var(&qubit.state_var, Sort::Int);

        for op in &qubit.operations {
            state = match op.gate {
                // Bit flip: |0> <-> |1>, superposition unchanged.
                GateKind::X => ite(
                    eq(state.clone(), int(0)),
                    int(1),
                    ite(eq(state.clone(), int(1)), int(0), int(2)),
                ),
                // Hadamard maps any basis state to a superposition, so it
                // always leaves the qubit provably not-|0>.
                GateKind::H => ite(
                    or(vec![eq(state.clone(), int(0)), eq(state.clone(), int(1))]),
                    int(2),
                    int(0),
                ),
                // Diagonal / phase gates. These fix |0> up to a global phase,
                // so they provably preserve an uncomputed qubit.
                GateKind::Y | GateKind::Z | GateKind::S | GateKind::T | GateKind::RZ => state,
                // A non-trivial rotation about X or Y takes |0> out of |0>.
                GateKind::RX | GateKind::RY => int(2),
                // Explicit reset returns the qubit to |0>.
                GateKind::Reset => int(0),
                // Measurement collapses to a basis state; it is not proof of
                // |0>, so the "superposition" value is the sound choice.
                GateKind::Measure => int(2),
                // Controlled gates: the target is only unchanged when the
                // control is provably |0>, which we do not model. Assume the
                // entangled case.
                GateKind::CX | GateKind::CY | GateKind::CZ => int(2),
                // An arbitrary unitary can map |0> anywhere.
                GateKind::Unitary => int(2),
            };
        }

        state
    }

    /// The uncomputation requirement for a single qubit: it must end in |0>.
    pub fn uncomputation_requirement(&self, qubit: &SymbolicQubit) -> Term {
        eq(var(&qubit.state_var, Sort::Int), int(0))
    }

    /// The gate-semantics fact for a single qubit: the state variable must
    /// equal the folded effect of the qubit's operation list.
    pub fn gate_semantics_for(&self, qubit: &SymbolicQubit) -> Term {
        let state_var = var(&qubit.state_var, Sort::Int);
        eq(state_var, self.folded_state(qubit))
    }

    /// Generate SMT declarations for a single qubit.
    pub fn declare_qubit(&self, script: &mut crate::smtlib::Script, qubit: &SymbolicQubit) {
        script.declare_const(&qubit.state_var, Sort::Int);
        script.assert(and(vec![
            ge(var(&qubit.state_var, Sort::Int), int(0)),
            le(var(&qubit.state_var, Sort::Int), int(2)),
        ]));
    }

    /// Generate SMT declarations for qubits.
    pub fn generate_declarations(&self, script: &mut crate::smtlib::Script) {
        for qubit in self.qubits.values() {
            self.declare_qubit(script, qubit);
        }

        for (name, _term) in &self.unitary_vars {
            script.declare_fun(name, vec![Sort::Int, Sort::Int], Sort::Int);
        }
    }

    /// Generate uncomputation constraints for all temporary qubits.
    pub fn generate_uncomputation_constraints(&self) -> Vec<Term> {
        self.temp_qubits
            .iter()
            .filter_map(|id| self.qubits.get(id))
            .map(|q| self.uncomputation_requirement(q))
            .collect()
    }

    /// Generate constraints for gate semantics.
    pub fn generate_gate_constraints(&self) -> Vec<Term> {
        self.qubits
            .values()
            .map(|q| self.gate_semantics_for(q))
            .collect()
    }
}

impl Default for QuantumTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode quantum operations for a function.
pub fn encode_quantum_expr(
    expr: &naso_compiler::ast::Expr,
    tracker: &mut QuantumTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    match &expr.kind {
        ExprKind::Call(func, args) => {
            if let ExprKind::Var(fname) = &func.kind {
                // Try to parse as a gate name
                if let Some(gate) = parse_gate_name(&fname.name) {
                    let mut targets = Vec::new();
                    let mut controls = Vec::new();

                    for arg in args {
                        if let ExprKind::Var(qname) = &arg.kind {
                            targets.push(qname.name.clone());
                        }
                    }

                    if gate == GateKind::CX && targets.len() >= 2 {
                        controls.push(targets[0].clone());
                        targets = vec![targets[1].clone()];
                    }

                    tracker.apply_gate(gate, &targets, &controls, expr.span);
                } else if fname.name == "qalloc" {
                    for arg in args.iter() {
                        if let ExprKind::Literal(naso_compiler::ast::Literal::Int(_n)) = &arg.kind {
                            for _ in 0..*_n as u32 {
                                tracker.allocate_qubit(true, expr.span);
                            }
                        }
                    }
                } else if fname.name == "qfree" {
                    for arg in args {
                        if let ExprKind::Var(_qname) = &arg.kind {
                            // Mark as freed (in real impl, check state)
                        }
                    }
                }
            }

            for arg in args {
                constraints.extend(encode_quantum_expr(arg, tracker)?);
            }
        }
        ExprKind::QuantumOp(qop) => {
            match qop {
                naso_compiler::ast::expr::QuantumOp::Alloc(_name) => {
                    tracker.allocate_qubit(true, expr.span);
                }
                naso_compiler::ast::expr::QuantumOp::Measure(target) => {
                    // Measurement must go through `apply_gate` like every other
                    // gate, so the qubit's symbolic state is folded to `2`
                    // ("not provably |0>") by `GateKind::Measure`'s transition.
                    //
                    // It did NOT. The arm looked the qubit up and then did nothing
                    // -- the comment "Measurement collapses state to basis" was an
                    // EMPTY BLOCK, with no statement under it -- and the state was
                    // only ever advanced by the `encode_quantum_expr(target, ...)`
                    // that read the variable. So a measured qubit kept whatever
                    // state it had, and a qubit measured while still |0> was
                    // reported CLEAN:
                    //
                    //     fn f() { let q = qalloc(1); let m = measure(q); }
                    //     -> 0 diagnostics (claimed uncomputed)
                    //
                    // which is unsound twice over. A measurement collapses to |0> or
                    // |1>, so it is never itself proof of |0>, and `folded_state`
                    // already encodes exactly that with `Measure => int(2)`. The
                    // transition existed; nothing called it.
                    if let ExprKind::Var(qname) = &target.kind {
                        tracker.apply_gate(
                            GateKind::Measure,
                            std::slice::from_ref(&qname.name),
                            &[],
                            expr.span,
                        );
                    }
                    constraints.extend(encode_quantum_expr(target, tracker)?);
                }
                naso_compiler::ast::expr::QuantumOp::ApplyGate(gate, args) => {
                    let gate_kind = GateKind::from_ast(gate);
                    let mut targets = Vec::new();
                    let mut controls = Vec::new();

                    for arg in args {
                        if let ExprKind::Var(qname) = &arg.kind {
                            targets.push(qname.name.clone());
                        }
                    }

                    if gate_kind == GateKind::CX && targets.len() >= 2 {
                        controls.push(targets[0].clone());
                        targets = vec![targets[1].clone()];
                    }

                    tracker.apply_gate(gate_kind, &targets, &controls, expr.span);
                }
                naso_compiler::ast::expr::QuantumOp::Entangle(args) => {
                    for arg in args {
                        constraints.extend(encode_quantum_expr(arg, tracker)?);
                    }
                }
                naso_compiler::ast::expr::QuantumOp::Phase(_, _) => {}
                naso_compiler::ast::expr::QuantumOp::Hamiltonian(_, _) => {}
            }
        }
        ExprKind::Let(binding) => {
            constraints.extend(encode_quantum_expr(&binding.value, tracker)?);
        }
        ExprKind::LetInOut(binding) => {
            constraints.extend(encode_quantum_expr(&binding.value, tracker)?);
        }
        ExprKind::LetConsume(binding) => {
            constraints.extend(encode_quantum_expr(&binding.value, tracker)?);
        }
        ExprKind::Block(block) => {
            if let Some(body_expr) = &block.expr {
                constraints.extend(encode_quantum_expr(body_expr, tracker)?);
            }
            for stmt in &block.stmts {
                if let naso_compiler::ast::StmtKind::Expr(stmt_expr) = &stmt.kind {
                    constraints.extend(encode_quantum_expr(stmt_expr, tracker)?);
                }
            }
        }
        ExprKind::If(cond, then_e, else_e) => {
            constraints.extend(encode_quantum_expr(cond, tracker)?);
            constraints.extend(encode_quantum_expr(then_e, tracker)?);
            if let Some(else_e) = else_e {
                constraints.extend(encode_quantum_expr(else_e, tracker)?);
            }
        }
        ExprKind::Binary(_, lhs, rhs) => {
            constraints.extend(encode_quantum_expr(lhs, tracker)?);
            constraints.extend(encode_quantum_expr(rhs, tracker)?);
        }
        ExprKind::Unary(_, operand) => {
            constraints.extend(encode_quantum_expr(operand, tracker)?);
        }
        ExprKind::MethodCall(receiver, _, args) => {
            constraints.extend(encode_quantum_expr(receiver, tracker)?);
            for arg in args {
                constraints.extend(encode_quantum_expr(arg, tracker)?);
            }
        }
        _ => {}
    }

    Ok(constraints)
}

/// Parse a gate name string to GateKind.
fn parse_gate_name(name: &str) -> Option<GateKind> {
    match name {
        "H" | "hadamard" => Some(GateKind::H),
        "X" => Some(GateKind::X),
        "Y" => Some(GateKind::Y),
        "Z" => Some(GateKind::Z),
        "S" => Some(GateKind::S),
        "T" => Some(GateKind::T),
        "CX" | "cnot" => Some(GateKind::CX),
        "CY" => Some(GateKind::CY),
        "CZ" => Some(GateKind::CZ),
        "RX" => Some(GateKind::RX),
        "RY" => Some(GateKind::RY),
        "RZ" => Some(GateKind::RZ),
        "measure" => Some(GateKind::Measure),
        "reset" => Some(GateKind::Reset),
        _ => None,
    }
}

/// Encode unitary matrix constraints for a gate (Clifford+T fragment).
pub fn encode_unitary_constraints(_gate: GateKind, _targets: &[String]) -> Vec<Term> {
    Vec::new()
}
