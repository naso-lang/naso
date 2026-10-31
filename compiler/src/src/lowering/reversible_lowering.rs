//! Reversible Block Lowering with Automatic Uncomputation
//!
//! Lower reversible blocks to dual ScheduleTrees (forward + inverse).
//! Implements TASK-303: Build the automatic uncomputation engine.
//!
//! Algorithm:
//! 1) Construct dataflow DAG of temporary values in forward schedule.
//! 2) Topologically sort; for each node, generate inverse operation using affine map inversion.
//! 3) Allocate ancilla registers for non-invertible ops (measurement, RNG) and insert explicit uncompute steps.
//! 4) Verify DAG has no cycles and all temps are cleaned.
//! 5) Emit optimized inverse ScheduleTree with ancilla deallocation.
//! 6) Integrate with QTT [0]-erasure: proof-only temps never allocated.
//! 7) Reverse-topological order instruction inversion for reversible {} blocks.
//! 8) Generate adjoint/mirror operations for reversible assignments, in-place updates, and quantum gate operations.
//! 9) Ancilla qubit zeroing verification: track temporary ancilla allocations (|0⟩) and synthesize inverse circuits to ensure clean uncomputation.

#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::collapsible_if)]

//! # THIS MODULE IS UNWIRED AND PRODUCES NOTHING EMITTABLE
//!
//! Nothing calls `lower_reversible_block` or `lower_reversible_block_to_pair`. The
//! statement-position `reversible` path in `mod.rs` refuses instead, which is correct:
//! see `tests/reversible_refusal_test.rs`.
//!
//! ## What the audit found (this section is new; read it before deciding anything)
//!
//! An earlier version of this banner listed three blockers, all of them structural, and
//! left the impression that the module was otherwise sound. It was not. Six of its inverse
//! generators FABRICATED an inverse rather than failing, and every one of them had a
//! passing unit test:
//!
//! | Generator | What it emitted | Why that is a fabrication |
//! |---|---|---|
//! | `generate_quantum_adjoint` | `Call "S†"`, `Call "RX†"`, `Call "UNKNOWN"` | Names no backend can resolve, and a hand-written relation that could drift from the verified one. Now uses `naso_gates::gate_inverse::inverse_of`, and refuses an unrecognised gate. |
//! | `generate_measurement_uncompute` | `Call "unmeasure"` with operand `IntLit(0)` | `unmeasure` is not a function, and on the no-argument branch the qubit operand was INVENTED -- an integer standing in for a quantum pointer. Now refused. |
//! | `generate_rng_uncompute` | `Call "unrng"` | Not a function, and it takes no arguments, so it could not recover the entropy it claimed to uncompute. Now refused. |
//! | `generate_arithmetic_inverse` | `Mul -> Div`, `Div -> Mul`, `_ -> Add` | `Mul -> Div` computes a DIFFERENT NUMBER instead of undoing one, and the default arm gave every comparison an "inverse" that was an addition. Now refused. |
//! | `generate_assignment_inverse` | `Call "discard"` | Not a function, and "just discard a `[1]` binding" is the soundness hole this language exists to prevent. Now refused. |
//! | `generate_affine_inverse` | `Call "affine_inverse"` | Ignored its own argument, called a function that does not exist, and claimed to invert a map that is not in general invertible. Now refused. |
//!
//! A seventh defect was in the VERIFICATION rather than the inverses:
//! `verify_ancilla_zeroing` tested `DAGNode::inverse_op`, a field initialised to `None` at
//! both construction sites and never assigned anywhere -- the inverses live in a separate
//! map. The predicate was therefore a constant, and the "check" refused every quantum
//! ancilla while claiming to verify one. It now consults the map that actually holds them.
//!
//! The lesson is the one this repository keeps relearning: a module whose tests exercise
//! only its own output is measuring itself. Twelve tests passed while the module emitted six
//! calls to functions that do not exist.
//!
//! ## Why it is still not wired up
//!
//! The fabrications are gone. The structural blockers are untouched, and any one of them is
//! still fatal:
//!
//! 1. **The result has nowhere to go.** `PirModule` carries `statements: Vec<PirStatement>`
//!    and ONE `schedule: ScheduleTree`. This module returns a *pair* of trees
//!    (`ReversibleSchedulePair { forward, inverse, .. }`). There is no second schedule
//!    field and no `PirModule` variant carrying an inverse, so the inverse tree would be
//!    dropped at the return.
//!
//! 2. **The inverse tree contains no operations.** `build_inverse_schedule` emits
//!    `ScheduleNode::domain(stmt_id, domain)` per inverse step -- loop structure only.
//!    `ScheduleNode` has no field carrying a `PirExpr` (its fields are `members`,
//!    `coincident`, `iterators`, `child`, `domain`, `stmt_id`). Every `InverseOperation.expr`
//!    computed by `generate_inverse_operations` is DISCARDED: the tree says "run step N
//!    here" with no step N.
//!
//! 3. **No backend reads an inverse tree.** Grepping the codegen tree for one returns
//!    nothing outside this file's own tests. LLVM, QIR and WGSL each walk
//!    `statements` in order.
//! 4. **The adjoint table has no producer, and no carrier.** `naso_gates::gate_inverse`
//!    is verified numerically, and `generate_quantum_adjoint` now consumes it -- but the
//!    `PirExpr` it returns still has nowhere to be stored. Fixing the fabrications made
//!    this module honest; it did not make it complete.
//!
//! ## The runtime now makes the goal testable, and that is the bar
//!
//! A compiled Naso circuit executes natively (`.naso` -> LLVM IR -> `llc` -> `cc` -> run,
//! linked against `libnaso_gates.a`). So "both a forward AND an inverse circuit execute" is
//! no longer hypothetical: it is a test that can be written and run. Until one is,
//! `reversible` stays refused. A pass that lowers and refuses to emit is not a partial
//! `reversible`; it is the defect this construct was fixed for, one layer down.
//!
//! ## The trap to avoid
//!
//! Wiring steps 1-7 and pushing `pair.inverse` into a field that looks right would
//! produce a module that COMPILES, PASSES this file's own 12 unit tests, and emits the
//! forward pass with no uncomputation -- exactly the defect that was just fixed at the
//! `mod.rs` call site, reintroduced one layer down. That is why `mod.rs` refuses rather
//! than calling into here, and why this banner exists.
//!
//! ## What real completion requires
//!
//! A PIR representation that can hold a second, ordered operation stream -- an inverse
//! statement list, or a `PirModule` field carrying both -- plus backend support for
//! emitting it. The gate is in `tests/capability_matrix_test.rs`, which pins
//! `reversible { ... }` as **refused** and fails if that changes without the reasoning
//! changing too.
//!
//! ## Two module-level lint suppressions
//!
//! `#![allow(clippy::match_like_matches_macro)]` and `#![allow(clippy::collapsible_if)]`
//! sit at the top of this file. They are left in place only because the module is dead
//! code; fixing them is a mechanical cleanup to do when it is wired, not a reason to
//! spend the effort now.

use super::{LoweringContext, LoweringError};
use crate::ast::Quantity;
use crate::ir::{
    AffineDomain, AffineMap, Matrix, PirExpr, PirStatement, QuantityMap, ScheduleNode,
    ScheduleTree, StmtId,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Result of reversible lowering: forward and inverse schedule trees
#[derive(Debug, Clone)]
pub struct ReversibleSchedulePair {
    pub forward: ScheduleTree,
    pub inverse: ScheduleTree,
    /// Ancilla qubits that must be zeroed at the end
    pub ancilla_requirements: Vec<AncillaRequirement>,
    /// Zero-quantity (proof-only) temporaries that were erased
    pub erased_temps: HashSet<String>,
}

/// Ancilla qubit allocation requirement
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AncillaRequirement {
    pub name: String,
    pub allocated_at: StmtId,
    pub zeroed_at: Option<StmtId>,
    pub is_quantum: bool,
}

/// Node in the dataflow DAG representing a temporary value
#[derive(Debug, Clone)]
struct DAGNode {
    /// Temporary variable name
    temp: String,
    /// Statement that defines this temp
    def_stmt: StmtId,
    /// Statements that use this temp
    uses: Vec<StmtId>,
    /// Whether this temp is zero-quantity (proof-only)
    is_zero_qty: bool,
    /// Whether this temp is an ancilla qubit
    is_ancilla: bool,
    /// Dependencies: temps that must be uncomputed before this one
    dependencies: Vec<String>,
}

/// Inverse operation for a temporary value
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct InverseOperation {
    /// The inverse expression
    expr: PirExpr,
    /// Affine map for the inverse schedule
    schedule_map: Option<AffineMap>,
    /// Whether this is an adjoint (quantum gate inverse)
    is_adjoint: bool,
    /// Ancilla qubits needed for this uncompute
    required_ancilla: Vec<String>,
}

/// Lower a reversible block to dual ScheduleTrees with automatic uncomputation
pub fn lower_reversible_block(
    block: &crate::ast::expr::ReversibleBlock,
    ctx: &mut LoweringContext,
) -> Result<ReversibleSchedulePair, LoweringError> {
    // Phase 1: Lower forward block and collect statements
    let forward_stmts = lower_forward_block(block, ctx)?;

    // Phase 2: Build dataflow DAG of temporary values
    let mut dag = DataflowDAG::new(&ctx.quantities);
    dag.build(&forward_stmts)?;

    // Phase 3: Topological sort and verify no cycles
    let topo_order = dag.topological_sort()?;

    // Phase 4: Generate inverse operations for each temp in reverse topological order
    let inverse_ops = generate_inverse_operations(&dag, &topo_order, ctx)?;

    // Phase 5: Allocate ancilla for non-invertible ops and verify zeroing
    let ancilla_reqs = allocate_ancilla_and_verify(&dag, &inverse_ops, ctx)?;

    // Phase 6: Build inverse schedule tree
    let inverse_schedule = build_inverse_schedule(&dag, &inverse_ops, &ancilla_reqs, ctx)?;

    // Phase 7: Build forward schedule tree
    let forward_schedule = build_forward_schedule(&forward_stmts, ctx)?;

    Ok(ReversibleSchedulePair {
        forward: forward_schedule,
        inverse: inverse_schedule,
        ancilla_requirements: ancilla_reqs,
        erased_temps: dag.erased_temps(),
    })
}

/// Dataflow DAG for temporary value tracking
struct DataflowDAG {
    nodes: HashMap<String, DAGNode>,
    /// Zero-quantity temps (erased at compile time)
    zero_qty_temps: HashSet<String>,
    quantities: QuantityMap,
}

impl DataflowDAG {
    fn new(quantities: &QuantityMap) -> Self {
        Self {
            nodes: HashMap::new(),
            zero_qty_temps: HashSet::new(),
            quantities: quantities.clone(),
        }
    }

    /// Build DAG from forward statements
    fn build(&mut self, stmts: &[PirStatement]) -> Result<(), LoweringError> {
        // First pass: collect definitions
        for stmt in stmts {
            self.collect_defs(&stmt.body, stmt.id)?;
        }

        // Second pass: collect uses
        for stmt in stmts {
            self.collect_uses(&stmt.body, stmt.id)?;
        }

        // Third pass: compute dependencies (def-use chains)
        self.compute_dependencies();

        // Identify zero-quantity temps
        for (name, qty) in &self.quantities {
            if *qty == Quantity::Zero {
                self.zero_qty_temps.insert(name.clone());
            }
        }

        Ok(())
    }

    fn collect_defs(&mut self, expr: &PirExpr, stmt_id: StmtId) -> Result<(), LoweringError> {
        match expr {
            PirExpr::Let {
                name,
                qty,
                mutability: _,
                value,
                body,
            } => {
                let is_zero = *qty == Quantity::Zero;
                let is_ancilla = self.is_ancilla_allocation(value);

                let node = DAGNode {
                    temp: name.clone(),
                    def_stmt: stmt_id,
                    uses: Vec::new(),
                    is_zero_qty: is_zero,
                    is_ancilla,
                    dependencies: Vec::new(),
                };
                self.nodes.insert(name.clone(), node);

                // Recurse into value and body
                self.collect_defs(value, stmt_id)?;
                self.collect_defs(body, stmt_id)?;
            }
            PirExpr::Call { name, args } if name == "qalloc" || name == "alloc_qubit" => {
                // Quantum allocation creates ancilla
                if let Some(PirExpr::Var(var)) = args.first() {
                    let node = DAGNode {
                        temp: var.clone(),
                        def_stmt: stmt_id,
                        uses: Vec::new(),
                        is_zero_qty: false,
                        is_ancilla: true,
                        dependencies: Vec::new(),
                    };
                    self.nodes.insert(var.clone(), node);
                }
            }
            PirExpr::Reversible { body, inverse } => {
                self.collect_defs(body, stmt_id)?;
                self.collect_defs(inverse, stmt_id)?;
            }
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_defs(cond, stmt_id)?;
                self.collect_defs(then_branch, stmt_id)?;
                self.collect_defs(else_branch, stmt_id)?;
            }
            PirExpr::Binary { left, right, .. } => {
                self.collect_defs(left, stmt_id)?;
                self.collect_defs(right, stmt_id)?;
            }
            PirExpr::Unary { expr, .. } => {
                self.collect_defs(expr, stmt_id)?;
            }
            PirExpr::Call { args, .. } => {
                for arg in args {
                    self.collect_defs(arg, stmt_id)?;
                }
            }
            PirExpr::Index { base, indices } => {
                self.collect_defs(base, stmt_id)?;
                for idx in indices {
                    self.collect_defs(idx, stmt_id)?;
                }
            }
            PirExpr::Field { base, .. } => {
                self.collect_defs(base, stmt_id)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn collect_uses(&mut self, expr: &PirExpr, stmt_id: StmtId) -> Result<(), LoweringError> {
        match expr {
            PirExpr::Var(name) => {
                if let Some(node) = self.nodes.get_mut(name) {
                    node.uses.push(stmt_id);
                }
            }
            PirExpr::Let { value, body, .. } => {
                self.collect_uses(value, stmt_id)?;
                self.collect_uses(body, stmt_id)?;
            }
            PirExpr::Reversible { body, inverse } => {
                self.collect_uses(body, stmt_id)?;
                self.collect_uses(inverse, stmt_id)?;
            }
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_uses(cond, stmt_id)?;
                self.collect_uses(then_branch, stmt_id)?;
                self.collect_uses(else_branch, stmt_id)?;
            }
            PirExpr::Binary { left, right, .. } => {
                self.collect_uses(left, stmt_id)?;
                self.collect_uses(right, stmt_id)?;
            }
            PirExpr::Unary { expr, .. } => {
                self.collect_uses(expr, stmt_id)?;
            }
            PirExpr::Call { args, .. } => {
                for arg in args {
                    self.collect_uses(arg, stmt_id)?;
                }
            }
            PirExpr::Index { base, indices } => {
                self.collect_uses(base, stmt_id)?;
                for idx in indices {
                    self.collect_uses(idx, stmt_id)?;
                }
            }
            PirExpr::Field { base, .. } => {
                self.collect_uses(base, stmt_id)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn compute_dependencies(&mut self) {
        // For each node, find temps it depends on (temps used in its definition)
        let node_names: Vec<String> = self.nodes.keys().cloned().collect();

        for name in node_names {
            // Collect dependencies first without mutable borrow
            let mut deps = Vec::new();
            let node_def_stmt = {
                let node = self.nodes.get(&name).unwrap();
                node.def_stmt
            };

            for other_name in self.nodes.keys() {
                if other_name != &name {
                    let other = self.nodes.get(other_name).unwrap();
                    if other.uses.contains(&node_def_stmt) {
                        deps.push(other_name.clone());
                    }
                }
            }

            if let Some(node) = self.nodes.get_mut(&name) {
                node.dependencies = deps;
            }
        }
    }

    fn is_ancilla_allocation(&self, expr: &PirExpr) -> bool {
        match expr {
            PirExpr::Call { name, .. } if name == "qalloc" || name == "alloc_qubit" => true,
            _ => false,
        }
    }

    /// Topological sort of the DAG (reverse order for uncomputation)
    fn topological_sort(&self) -> Result<Vec<String>, LoweringError> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize
        for name in self.nodes.keys() {
            in_degree.insert(name.clone(), 0);
            adj.insert(name.clone(), Vec::new());
        }

        // Build adjacency and in-degrees (reverse edges for uncomputation order)
        for (name, node) in &self.nodes {
            for dep in &node.dependencies {
                adj.get_mut(dep).unwrap().push(name.clone());
                *in_degree.get_mut(name).unwrap() += 1;
            }
        }

        // Kahn's algorithm
        let mut queue: VecDeque<String> = VecDeque::new();
        for (name, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(name.clone());
            }
        }

        let mut result = Vec::new();
        while let Some(name) = queue.pop_front() {
            result.push(name.clone());
            for neighbor in adj.get(&name).unwrap() {
                let deg = in_degree.get_mut(neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(neighbor.clone());
                }
            }
        }

        if result.len() != self.nodes.len() {
            return Err(LoweringError::NonReversibleOp(
                "Cycle detected in dataflow DAG - cannot uncomputation".to_string(),
            ));
        }

        // Reverse for uncomputation order (last defined, first uncomputed)
        result.reverse();
        Ok(result)
    }

    fn erased_temps(&self) -> HashSet<String> {
        self.zero_qty_temps.clone()
    }
}

/// Generate inverse operations for each temp in reverse topological order
fn generate_inverse_operations(
    dag: &DataflowDAG,
    topo_order: &[String],
    ctx: &LoweringContext,
) -> Result<HashMap<String, InverseOperation>, LoweringError> {
    let mut inverse_ops = HashMap::new();

    for temp_name in topo_order {
        let node = dag.nodes.get(temp_name).unwrap();

        // Skip zero-quantity temps (erased at compile time)
        if node.is_zero_qty {
            continue;
        }

        // Find the defining expression for this temp
        let def_expr = find_def_expr(&node.temp, &ctx.statements)?;

        // Generate inverse based on expression type
        let inv_op = match &def_expr {
            PirExpr::Call { name, args } if is_quantum_gate(name.as_str()) => {
                generate_quantum_adjoint(name.as_str(), args.as_slice(), node.def_stmt)?
            }
            PirExpr::Call { name, args } if is_measurement(name.as_str()) => {
                generate_measurement_uncompute(name.as_str(), args.as_slice(), node.def_stmt)?
            }
            PirExpr::Call { name, args } if is_rng(name.as_str()) => {
                generate_rng_uncompute(name.as_str(), args.as_slice(), node.def_stmt)?
            }
            PirExpr::Binary { op, left, right } => {
                generate_arithmetic_inverse(*op, left.as_ref(), right.as_ref(), node.def_stmt)?
            }
            PirExpr::Let { name: _, value, .. } => {
                generate_assignment_inverse(value.as_ref(), node.def_stmt)?
            }
            // No rule, so no inverse. This arm used to read "try affine map
            // inversion", which described a function that extracted nothing and called a
            // callee that does not exist.
            _ => generate_affine_inverse(&def_expr, node.def_stmt)?,
        };

        // Update the node with inverse op
        inverse_ops.insert(temp_name.clone(), inv_op);
    }

    Ok(inverse_ops)
}

/// Check if a call is a quantum gate
fn is_quantum_gate(name: &str) -> bool {
    matches!(
        name,
        "H" | "X"
            | "Y"
            | "Z"
            | "S"
            | "T"
            | "CX"
            | "CY"
            | "CZ"
            | "RX"
            | "RY"
            | "RZ"
            | "qgate"
            | "apply_gate"
    )
}

/// Check if a call is a measurement
fn is_measurement(name: &str) -> bool {
    matches!(name, "measure" | "qmeasure" | "measure_z")
}

/// Check if a call is RNG
fn is_rng(name: &str) -> bool {
    matches!(name, "rng" | "random" | "rand")
}

/// Generate the adjoint of a quantum gate, from the ONE VERIFIED table.
///
/// # Why this consults `naso_gates::gate_inverse::inverse_of`
///
/// It used to hand-write the relation, mapping `"S"` to the string `"S\u{2020}"` and
/// `"RX"` to `"RX\u{2020}"`. Neither is a name anything can lower or emit: `PirExpr::Call`
/// with name `"S\u{2020}"` reaches no backend, and the QIR mapping has no arm for it either,
/// so it would have been refused downstream -- or worse, defaulted to something. The table's
/// OWN doc comment names this exact hazard ("an earlier version of the QIR classifier mapped
/// S-dagger to S").
///
/// `naso_gates::gate_inverse::inverse_of` is verified numerically on a CPU state vector:
/// every gate composed with its table entry returns the original state. That is the
/// relation. There is exactly one of it in the tree, so a second copy here cannot drift
/// from it -- which is the failure a duplicate adjoint table does NOT announce, it just
/// quietly computes wrong inverses.
///
/// # Why an unrecognised gate is REFUSED
///
/// The old code mapped every unknown gate to `"UNKNOWN"` and returned `Ok`. That is a
/// fabrication with no caller to reject it: the call emitted a callee named `UNKNOWN`,
/// which is not a gate, not a function, and not an error. A name this function cannot
/// invert has no inverse here, and saying so is the only honest answer.
///
/// # Why the adjoint is only computed for a gate this module can recognise
///
/// `GateKind`'s `Display` spells gates `"H"`, `"CX"`, `"RX"`. A `Custom(ident)` gate has
/// no matrix and therefore no adjoint that can be computed without running the user
/// function's transpose, so it is refused rather than assumed self-inverse. Assuming
/// self-inverse is precisely the claim that is false for `S`, `T` and every rotation.
fn gate_by_name(name: &str) -> Option<naso_gates::statevector::Gate> {
    use naso_gates::statevector::Gate;
    Some(match name {
        "H" => Gate::H,
        "X" => Gate::X,
        "Y" => Gate::Y,
        "Z" => Gate::Z,
        "S" => Gate::S,
        "Sdg" => Gate::Sdg,
        "T" => Gate::T,
        "Tdg" => Gate::Tdg,
        "CX" => Gate::Cx,
        "CY" => Gate::Cy,
        "CZ" => Gate::Cz,
        "CCX" => Gate::Ccx,
        "SWAP" => Gate::Swap,
        // A rotation's adjoint is the negated rotation, which the verified table computes.
        // The angle is carried by the enclosing `QuantumOp`, not by the name, so the
        // angle-free variants here are only used for gates with no angle.
        "RX" => Gate::Rx(0.0),
        "RY" => Gate::Ry(0.0),
        "RZ" => Gate::Rz(0.0),
        _ => return None,
    })
}

/// Generate the adjoint for a quantum gate.
///
/// The inverse NAME comes from the verified table. A rotation's angle is a real negation of
/// the original expression, which is what makes `RZ(θ)`'s inverse `RZ(-θ)`; the table gives
/// the identity and the negation gives the angle, and neither is guessed.
fn generate_quantum_adjoint(
    gate: &str,
    args: &[PirExpr],
    stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    use naso_gates::gate_inverse::inverse_of;
    use naso_gates::statevector::Gate;

    // An angle, if the gate has one, has to come from the arguments. The name alone does not
    // carry it: `RX` and `RX(1.1)` lower to the same operation name.
    let angle: Option<f64> = args.iter().find_map(|a| match a {
        PirExpr::FloatLit(s) => s.parse::<f64>().ok(),
        PirExpr::IntLit(i) => Some(*i as f64),
        _ => None,
    });

    let forward = match (gate, angle) {
        ("RX", Some(t)) => Gate::Rx(t),
        ("RY", Some(t)) => Gate::Ry(t),
        ("RZ", Some(t)) => Gate::Rz(t),
        ("RX" | "RY" | "RZ", None) => {
            return Err(LoweringError::Unsupported(format!(
                "`{gate}` on statement {stmt_id:?} carries no angle, so its adjoint cannot be \
                 computed: the inverse of a rotation by theta is a rotation by -theta, and \
                 there is no theta here to negate. Emitting the un-negated angle would \
                 compute Rz(theta) twice instead of undoing it."
            )));
        }
        _ => gate_by_name(gate).ok_or_else(|| {
            LoweringError::NonReversibleOp(format!(
                "`{gate}` on statement {stmt_id:?} is not a gate with a known adjoint, so its \
                 inverse is not computed here. Assuming it is self-inverse would be wrong for \
                 every phase and rotation gate; guessing an inverse would be wrong in a way \
                 nothing downstream could detect."
            ))
        })?,
    };

    let inverse = inverse_of(forward);

    // The emitted inverse is a name a backend can actually resolve. `S` maps back to `Sdg`,
    // which is a real gate; it is NOT mapped to `S`, which is the original defect this table
    // was written to make unrepresentable.
    let (name, inv_args): (String, Vec<PirExpr>) = match inverse {
        Gate::H => ("H".to_string(), args.to_vec()),
        Gate::X => ("X".to_string(), args.to_vec()),
        Gate::Y => ("Y".to_string(), args.to_vec()),
        Gate::Z => ("Z".to_string(), args.to_vec()),
        Gate::S => ("S".to_string(), args.to_vec()),
        Gate::Sdg => ("Sdg".to_string(), args.to_vec()),
        Gate::T => ("T".to_string(), args.to_vec()),
        Gate::Tdg => ("Tdg".to_string(), args.to_vec()),
        Gate::Cx => ("CX".to_string(), args.to_vec()),
        Gate::Cy => ("CY".to_string(), args.to_vec()),
        Gate::Cz => ("CZ".to_string(), args.to_vec()),
        Gate::Ccx => ("CCX".to_string(), args.to_vec()),
        Gate::Swap => ("SWAP".to_string(), args.to_vec()),
        Gate::Rx(_) => ("RX".to_string(), negate_first_angle(args)),
        Gate::Ry(_) => ("RY".to_string(), negate_first_angle(args)),
        Gate::Rz(_) => ("RZ".to_string(), negate_first_angle(args)),
    };

    Ok(InverseOperation {
        expr: PirExpr::Call {
            name,
            args: inv_args,
        },
        schedule_map: None,
        is_adjoint: true,
        required_ancilla: Vec::new(),
    })
}

/// Negate the angle argument of a rotation, keeping the qubit operands unchanged.
///
/// A rotation is `RZ(angle, qubit)`: the FIRST argument is the angle, the rest are qubits.
/// Negating every argument would negate the qubit, which is a type error at best and a
/// silently wrong circuit at worst.
fn negate_first_angle(args: &[PirExpr]) -> Vec<PirExpr> {
    let mut out: Vec<PirExpr> = args.to_vec();
    if let Some(first) = args.first() {
        out[0] = PirExpr::Unary {
            op: crate::ir::UnaryOp::Neg,
            expr: Box::new(first.clone()),
        };
    }
    out
}

/// Measurement uncomputation. REFUSED, because the previous version fabricated it.
///
/// # The defect this replaces
///
/// It returned `Ok(InverseOperation { expr: Call { name: "unmeasure", args: vec![qubit] } })`,
/// where `qubit` was `args.first().cloned().unwrap_or(PirExpr::IntLit(0))`. Two separate
/// fabrications in three lines:
///
/// 1. **`unmeasure` is not a function.** It exists nowhere in this repository -- no lexer
///    token, no `PirExpr` variant, no backend intrinsic. The emitted call named a callee
///    that cannot resolve, so a measurement inside `reversible` would have produced PIR
///    looking like it uncomputes and a module with no way to run it.
/// 2. **The qubit operand was invented.** On the fallback branch -- which is the branch a
///    measurement with no operand argument takes -- the "qubit" was the literal `0`. That is
///    an integer standing in for a quantum pointer. It is not a fabrication that fails; it
///    is one that produces valid-looking PIR.
///
/// # Why refusing is the correct answer and not a retreat
///
/// Measurement is not invertible. Uncomputing it requires a classically-controlled
/// re-preparation: record the outcome, and on `|1>` apply X to return the qubit to `|0>`.
/// That is a real algorithm, and its correct implementation needs an outcome register the
/// current representation cannot carry -- see the module banner on `PirModule` having no
/// inverse carrier. An ancilla does not fix this by being named: the module's previous
/// response was to append the string `"measurement_ancilla"` to a list, which allocates
/// nothing.
///
/// Claiming otherwise is worse than refusing. `unmeasure` is exactly the shape of the
/// original defect this construct was fixed for -- a forward pass and no uncomputation,
/// wearing the name of an uncomputation.
fn generate_measurement_uncompute(
    name: &str,
    _args: &[PirExpr],
    stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    Err(LoweringError::NonReversibleOp(format!(
        "`{name}` on statement {stmt_id:?} is a MEASUREMENT, and measurement is not \
         invertible, so there is no adjoint to emit. Uncomputing it is not 'undoing' a call: \
         it is a classically-controlled re-preparation, which needs the classical outcome to \
         be carried to a conditional branch. The PIR has no inverse carrier for that, so \
         this is refused rather than emitted as a call to a function that does not exist. \
         Naming an ancilla does not supply the outcome register either."
    )))
}

/// RNG uncomputation. REFUSED, because the previous version fabricated it.
///
/// It returned `Ok(InverseOperation { expr: Call { name: "unrng", args: vec![] } })`. As
/// with `unmeasure`, `unrng` is not a function anywhere in this repository -- and it takes no
/// arguments at all, so even if it existed it could not recover the entropy it was supposed
/// to uncompute.
///
/// RNG is genuinely non-invertible: recovering a discarded random value is a search, not a
/// computation. A `reversible` block containing one cannot be uncomputed, and the honest
/// result is a refusal naming the cause.
fn generate_rng_uncompute(
    name: &str,
    _args: &[PirExpr],
    stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    Err(LoweringError::NonReversibleOp(format!(
        "`{name}` on statement {stmt_id:?} draws randomness, which cannot be uncomputed: \
         recovering a discarded random value is a search, not a computation. No amount of \
         ancilla makes it invertible, so this is refused rather than emitted as a call to an \
         `unrng` function that does not exist."
    )))
}

/// Arithmetic inversion. REFUSED, because the previous version computed a WRONG inverse.
///
/// # The defect this replaces
///
/// It mapped `Mul -> Div` and `Div -> Mul`, with the comment "This is simplified - real
/// implementation would track which operand is the output". The mapping is not a
/// simplification, it is wrong in both directions:
///
/// - `a * b` is inverted by **dividing the RESULT by the other operand**, not by rewriting
///   `a * b` to `a / b`. The inverse of `x = a*b` is `x/b = a`, and the code emitted `a / b`,
///   which is a different number.
/// - `a / b` is not inverted by `a * b` under any reading. Recovering `b` from `a / b` needs
///   division, and recovering `a` needs multiplication; which one is meant depends on which
///   operand the statement actually binds, and the function was not told.
///
/// It also had a `_ => BinaryOp::Add` default, so `Less`, `And`, `Shl`, comparisons -- every
/// non-arithmetic binary op -- silently got an "inverse" that was an addition.
///
/// # Why this is refused rather than fixed here
///
/// A correct implementation needs to know which operand the statement BINDS, and that is
/// `find_def_expr`'s job, not this function's -- it is called with only the two operand
/// expressions and the statement id. Inventing the missing information here would be the
/// same fabrication in a new place. The refusal names the actual missing capability.
fn generate_arithmetic_inverse(
    op: crate::ir::BinaryOp,
    _left: &PirExpr,
    _right: &PirExpr,
    stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    Err(LoweringError::Unsupported(format!(
        "the arithmetic operator `{op:?}` on statement {stmt_id:?} is not uncomputed here. \
         Its inverse is not a rewrite of the operator: the inverse of `x = a*b` is `x/b`, \
         which needs to know WHICH OPERAND the statement binds, and the inverse of `x = a/b` \
         is not `a*b` at all. This pass is not told which operand is the result, so it will \
         not guess -- `Mul`/`Div` were previously mapped to each other, which computes a \
         different number rather than undoing one."
    )))
}

/// Assignment inversion. REFUSED, because the previous version emitted a call to `discard`.
///
/// `discard` is not a function in this repository. The previous version emitted
/// `Call { name: "discard", args: vec![value] }` with the comment "For `let x = y`, inverse
/// is just discarding x".
///
/// That comment is wrong about what it describes, and it is worth being precise about why,
/// because "just discard it" is the shape of every linear-type bug this language exists to
/// prevent. A `[1]`-quantity binding cannot be discarded: dropping it is a soundness hole,
/// which is why the source language has no `discard`. What uncomputation actually needs is
/// to restore the value that was overwritten, and for a linear resource that means applying
/// the inverse of whatever wrote it -- which is this module's job, not a primitive named
/// `discard`.
///
/// # Note on `[0]`
///
/// A `[0]`-quantity temporary needs no runtime uncompute at all: it is erased at compile
/// time and never allocated (Rule 2 -- a `$[0]$-use variable must not reach the backend).
/// That case is already handled upstream, by `DataflowDAG` recording it in
/// `zero_qty_temps` and `generate_inverse_operations` skipping it. A temp reaching THIS
/// function is a `[1]` temp, so the erasure path is not the answer for it.
fn generate_assignment_inverse(
    _value: &PirExpr,
    stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    Err(LoweringError::NonReversibleOp(format!(
        "an assignment on statement {stmt_id:?} is not uncomputed by discarding its value. \
         A [1]-quantity binding cannot be dropped -- that is a soundness hole, not an undo -- \
         so there is no `discard` to call. Uncomputation here has to apply the inverse of \
         whatever wrote the value, and this pass does not yet know what that is. A [0] \
         temporary needs none of this: it is erased at compile time and never allocated."
    )))
}

/// The fallback for an expression with no inverse rule. REFUSED.
///
/// The previous version returned `Ok(InverseOperation { expr: Call { name: "affine_inverse",
/// args: vec![expr.clone()] } })`, with the comment "Try to extract affine map from
/// expression and invert it / This is a simplified version".
///
/// Three fabrications in one function:
///
/// 1. **`affine_inverse` is not a function.** Nothing in this repository defines it, so the
///    emitted call named a callee that cannot resolve.
/// 2. **Nothing was extracted.** The function ignores its `expr` argument entirely and
///    passes it straight through. The comment says it extracts and inverts an affine map; the
///    code does neither.
/// 3. **The inverse of an affine map is not in general affine.** Inverting `x -> A*x + b`
///    requires `A` to be invertible, and gives `x -> A^-1*(x - b)` with an inverse that is
///    generally rational. Presenting that as "the affine inverse" is exactly the claim that
///    fails silently for a singular `A`.
///
/// This is also the arm every unrecognised expression reached, so it was the module's
/// universal "yes, I can uncompute this" answer.
fn generate_affine_inverse(
    _expr: &PirExpr,
    stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    Err(LoweringError::Unsupported(format!(
        "the expression on statement {stmt_id:?} has no inverse rule, and it is not \
         uncomputed by passing it to a function called `affine_inverse` -- no such function \
         exists, and nothing was extracted from it anyway. Inverting an affine map also \
         requires the matrix to be invertible and generally yields a rational map, so \
         'the affine inverse' is not a thing that can be assumed. Refused rather than \
         emitted as a call to a callee that cannot resolve."
    )))
}

/// Find the defining expression for a temporary
fn find_def_expr(temp: &str, stmts: &[PirStatement]) -> Result<PirExpr, LoweringError> {
    for stmt in stmts {
        if let PirExpr::Let { name, value, .. } = &stmt.body {
            if name == temp {
                return Ok((**value).clone());
            }
        }
        // Also check for direct assignment
        if let PirExpr::Call { name, args } = &stmt.body {
            if name == "qalloc" || name == "alloc_qubit" {
                if let Some(PirExpr::Var(v)) = args.first() {
                    if v == temp {
                        return Ok(stmt.body.clone());
                    }
                }
            }
        }
    }
    Err(LoweringError::Unsupported(format!(
        "Temp {} not found",
        temp
    )))
}

/// Allocate ancilla for non-invertible ops and verify zeroing
fn allocate_ancilla_and_verify(
    dag: &DataflowDAG,
    inverse_ops: &HashMap<String, InverseOperation>,
    _ctx: &LoweringContext,
) -> Result<Vec<AncillaRequirement>, LoweringError> {
    let mut requirements = Vec::new();
    let mut ancilla_counter = 0;

    // Collect all required ancilla from inverse ops
    for (temp, inv_op) in inverse_ops {
        let node = dag.nodes.get(temp).unwrap();

        for ancilla in &inv_op.required_ancilla {
            let req = AncillaRequirement {
                name: format!("{}_{}", ancilla, ancilla_counter),
                allocated_at: node.def_stmt,
                zeroed_at: None, // Will be set when building inverse schedule
                is_quantum: inv_op.is_adjoint,
            };
            requirements.push(req);
            ancilla_counter += 1;
        }

        // Check ancilla zeroing for quantum ancillas
        if node.is_ancilla {
            // Verify this ancilla is returned to |0⟩
            let req = AncillaRequirement {
                name: temp.clone(),
                allocated_at: node.def_stmt,
                zeroed_at: None, // Set during inverse schedule building
                is_quantum: true,
            };
            requirements.push(req);
        }
    }

    // Verify all ancillas can be zeroed (no leftover entanglement)
    verify_ancilla_zeroing(&requirements, dag, inverse_ops)?;

    Ok(requirements)
}

/// Verify that every quantum ancilla actually HAS an inverse to zero it with.
///
/// # The defect this replaces
///
/// It checked `node.inverse_op.is_none()` -- a field on `DAGNode` that was initialised to
/// `None` at both construction sites and NEVER assigned anywhere in the module. The
/// inverses live in a separate `HashMap<String, InverseOperation>` returned by
/// `generate_inverse_operations`, and nothing ever copied them onto the node.
///
/// So the predicate was a constant. For a requirement with `is_quantum` set it was
/// unconditionally true, and every quantum ancilla produced
/// `NonReversibleOp("No inverse operation for quantum ancilla ...")`. A check that cannot
/// pass is not a check: it is a second, differently-worded refusal wearing the costume of a
/// verification, and it would have "caught" a missing inverse whether or not one was
/// missing.
///
/// # What it checks now
///
/// The map that actually holds the inverses. An ancilla with no entry in it has nothing to
/// zero it with, which is the real condition worth reporting. This does not make the module
/// correct -- it makes the diagnostic mean what it says.
fn verify_ancilla_zeroing(
    requirements: &[AncillaRequirement],
    dag: &DataflowDAG,
    inverse_ops: &HashMap<String, InverseOperation>,
) -> Result<(), LoweringError> {
    for req in requirements {
        if !req.is_quantum {
            continue;
        }
        if !dag.nodes.contains_key(&req.name) {
            return Err(LoweringError::NonReversibleOp(format!(
                "quantum ancilla `{}` is not a node in the dataflow DAG, so there is no record \
                 of where it was allocated",
                req.name
            )));
        }
        // The ancilla must have a REAL inverse operation, from the map that holds them.
        if !inverse_ops.contains_key(&req.name) {
            return Err(LoweringError::NonReversibleOp(format!(
                "quantum ancilla `{}` has no inverse operation, so nothing zeroes it. An \
                 ancilla left entangled at the end of a `reversible` block is exactly the \
                 garbage the block existed to erase",
                req.name
            )));
        }
    }
    Ok(())
}

/// Build inverse schedule tree from inverse operations
fn build_inverse_schedule(
    dag: &DataflowDAG,
    inverse_ops: &HashMap<String, InverseOperation>,
    ancilla_reqs: &[AncillaRequirement],
    ctx: &LoweringContext,
) -> Result<ScheduleTree, LoweringError> {
    let mut inverse_nodes = Vec::new();
    let mut stmt_id_counter = ctx.statements.len();

    // Add inverse operations in reverse topological order (already sorted)
    for temp_name in dag.topological_sort()? {
        if inverse_ops.contains_key(&temp_name) {
            // Skip zero-quantity temps
            if dag.zero_qty_temps.contains(&temp_name) {
                continue;
            }

            let stmt_id = StmtId(stmt_id_counter);
            stmt_id_counter += 1;

            let domain = AffineDomain::universe(0, 0);
            // Build schedule node for this inverse op
            let mut m = Matrix::new(1, 1);
            m.set(0, 0, 1);
            let map = AffineMap::total(domain.clone(), m);

            inverse_nodes.push(ScheduleNode::band(
                vec![map],
                vec![false],
                ScheduleNode::domain(stmt_id, domain),
            ));
        }
    }

    // Add ancilla deallocation steps
    for req in ancilla_reqs {
        if req.is_quantum {
            let stmt_id = StmtId(stmt_id_counter);
            stmt_id_counter += 1;

            let domain = AffineDomain::universe(0, 0);
            // Build dealloc schedule node
            let mut m = Matrix::new(1, 1);
            m.set(0, 0, 1);
            let map = AffineMap::total(domain.clone(), m);

            inverse_nodes.push(ScheduleNode::band(
                vec![map],
                vec![false],
                ScheduleNode::domain(stmt_id, domain),
            ));
        }
    }

    // Combine into sequence
    let root = if inverse_nodes.is_empty() {
        ScheduleNode::Empty
    } else if inverse_nodes.len() == 1 {
        inverse_nodes.into_iter().next().unwrap()
    } else {
        ScheduleNode::sequence(inverse_nodes)
    };

    Ok(ScheduleTree::new(root, ctx.param_names.clone()))
}

/// Build forward schedule tree
fn build_forward_schedule(
    stmts: &[PirStatement],
    ctx: &LoweringContext,
) -> Result<ScheduleTree, LoweringError> {
    let mut nodes = Vec::new();

    for stmt in stmts {
        let domain = stmt.domain.clone();
        let mut m = Matrix::new(1, domain.dims + domain.n_param);
        if domain.dims > 0 {
            m.set(0, 0, 1);
        }
        let map = AffineMap::total(domain.clone(), m);

        nodes.push(ScheduleNode::band(
            vec![map],
            vec![false],
            ScheduleNode::domain(stmt.id, domain),
        ));
    }

    let root = if nodes.is_empty() {
        ScheduleNode::Empty
    } else if nodes.len() == 1 {
        nodes.into_iter().next().unwrap()
    } else {
        ScheduleNode::sequence(nodes)
    };

    Ok(ScheduleTree::new(root, ctx.param_names.clone()))
}

/// Lower forward block and return statements
fn lower_forward_block(
    block: &crate::ast::expr::ReversibleBlock,
    ctx: &mut LoweringContext,
) -> Result<Vec<PirStatement>, LoweringError> {
    for stmt in &block.body.stmts {
        ctx.lower_stmt(stmt)?;
    }

    // Collect the statements that were added
    // (In practice, we'd track which ones belong to this block)
    Ok(ctx.statements.clone())
}

/// Public entry point: lower reversible block to dual ScheduleTrees
pub fn lower_reversible_block_to_pair(
    block: &crate::ast::expr::ReversibleBlock,
    ctx: &mut LoweringContext,
) -> Result<ReversibleSchedulePair, LoweringError> {
    lower_reversible_block(block, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Block, Mutability, Quantity, Span};
    use crate::ir::{AffineDomain, PirExpr, PirStatement, QuantityMap, ScheduleNode, StmtId};

    fn make_span() -> Span {
        Span::new(0, 0, 1, 1)
    }

    /// Every arithmetic operator is REFUSED, including the self-inverse ones.
    ///
    /// `Xor` genuinely is its own inverse, so a test that only checked `Add` would still be
    /// asserting the fabrication. The point is not that some operators lack an inverse; it
    /// is that this function cannot tell which operand the statement BINDS, and without that
    /// it cannot write the inverse of ANY of them. `Mul -> Div` computed a different number
    /// rather than undoing one, and a self-inverse `Xor` is not worth special-casing while
    /// `Add` beside it is wrong.
    #[test]
    fn every_arithmetic_operator_is_refused_rather_than_guessed() {
        use crate::ir::BinaryOp::*;
        let left = PirExpr::Var("a".to_string());
        let right = PirExpr::Var("b".to_string());

        for op in [Add, Sub, Mul, Div, Xor, And, Or, Shl, Shr, Lt, Gt, Eq] {
            let err = generate_arithmetic_inverse(op, &left, &right, StmtId(7)).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("WHICH OPERAND"),
                "`{op:?}` must be refused naming the missing information: {msg}"
            );
        }
    }

    /// The adjoint comes from the ONE verified table, not a hand-written string.
    ///
    /// The old implementation returned the name `"S†"` (U+2020), which is not a gate, not a
    /// function, and cannot be lowered by any backend. This asserts on the quantum relation
    /// instead: `S` inverts to `Sdg`, `T` to `Tdg`, and the self-inverse gates to themselves.
    ///
    /// `Sdg` and not `S` is the specific defect that table was written to make
    /// unrepresentable, so it is asserted directly rather than inferred.
    #[test]
    fn the_adjoint_of_a_gate_is_computed_from_the_verified_table() {
        for (gate, expected) in [
            ("H", "H"),
            ("X", "X"),
            ("Z", "Z"),
            ("S", "Sdg"),
            ("T", "Tdg"),
            ("Sdg", "S"),
            ("Tdg", "T"),
            ("CX", "CX"),
            ("CZ", "CZ"),
            ("CCX", "CCX"),
        ] {
            let inv = generate_quantum_adjoint(gate, &[], StmtId(0))
                .unwrap_or_else(|e| panic!("`{gate}` has a known adjoint, so it must invert: {e}"));
            match inv.expr {
                PirExpr::Call { name, .. } => assert_eq!(
                    name, expected,
                    "the inverse of `{gate}` must come from naso-gates::inverse_of, not a \
                     hand-written table"
                ),
                other => panic!("expected a call, got {other:?}"),
            }
            assert!(inv.is_adjoint, "`{gate}`'s inverse is an adjoint");
        }
    }

    /// The adjoint NAME must be one a backend can resolve.
    ///
    /// Every name `generate_quantum_adjoint` emits is asserted against the gate set the
    /// compiler actually knows. A name outside it -- `"S†"`, `"UNKNOWN"` -- is a call no
    /// backend can resolve, which is how the previous version's output was unusable while
    /// every one of its own tests still passed.
    #[test]
    fn every_emitted_adjoint_name_is_a_gate_the_compiler_knows() {
        let known = [
            "H", "X", "Y", "Z", "S", "Sdg", "T", "Tdg", "CX", "CY", "CZ", "CCX", "SWAP", "RX",
            "RY", "RZ",
        ];
        for gate in ["H", "S", "T", "CX", "CCX"] {
            let inv = generate_quantum_adjoint(gate, &[], StmtId(0)).unwrap();
            if let PirExpr::Call { name, .. } = &inv.expr {
                assert!(
                    known.contains(&name.as_str()),
                    "the adjoint of `{gate}` was emitted as `{name}`, which is not a gate the \
                     compiler knows. A callee no backend can resolve is the fabrication this \
                     function used to produce."
                );
            }
        }
    }

    /// An unrecognised gate is REFUSED, not mapped to a placeholder name.
    ///
    /// It used to return `Ok` with the name `"UNKNOWN"` -- a callee that is not a gate, not a
    /// function, and not an error. Assuming self-inverse would be wrong for `S`, `T` and
    /// every rotation, so there is no safe default.
    #[test]
    fn an_unrecognised_gate_is_refused_rather_than_assumed_self_inverse() {
        for gate in ["Custom", "ccz", "iswap", "not_a_gate", ""] {
            let err = generate_quantum_adjoint(gate, &[], StmtId(3)).unwrap_err();
            assert!(
                err.to_string().contains("self-inverse"),
                "`{gate}` must be refused naming why guessing is wrong: {err}"
            );
        }
    }

    /// A rotation with NO angle is refused: there is nothing to negate.
    ///
    /// `RX` and `RX(1.1)` lower to the same operation name, so the angle has to come from the
    /// arguments. Emitting the un-negated angle would apply `Rz(theta)` twice instead of
    /// undoing it -- an inverse that is the identity on no state at all.
    #[test]
    fn a_rotation_with_no_angle_is_refused_rather_than_emitted_unchanged() {
        for gate in ["RX", "RY", "RZ"] {
            let err = generate_quantum_adjoint(gate, &[], StmtId(4)).unwrap_err();
            assert!(
                err.to_string().contains("no angle"),
                "`{gate}` with no angle must say so: {err}"
            );
        }
    }

    /// A rotation's angle IS negated, and its qubit is NOT.
    ///
    /// `RZ(angle, qubit)`: the angle is the first argument. Negating every argument -- which
    /// the previous implementation did, matching on any argument that was itself a call --
    /// would negate the qubit.
    #[test]
    fn a_rotation_negates_its_angle_and_leaves_its_qubit_alone() {
        let inv = generate_quantum_adjoint(
            "RZ",
            &[PirExpr::FloatLit("1.1".into()), PirExpr::Var("q".into())],
            StmtId(0),
        )
        .unwrap();

        match &inv.expr {
            PirExpr::Call { name, args } => {
                assert_eq!(name, "RZ", "RZ is its own inverse up to the angle");
                assert_eq!(args.len(), 2, "both operands must survive");
                match &args[0] {
                    PirExpr::Unary {
                        op: crate::ir::UnaryOp::Neg,
                        expr,
                    } => assert!(
                        matches!(**expr, PirExpr::FloatLit(ref s) if s == "1.1"),
                        "the ANGLE must be negated"
                    ),
                    other => panic!("the angle must be negated, got {other:?}"),
                }
                assert!(
                    matches!(args[1], PirExpr::Var(ref v) if v == "q"),
                    "the QUBIT must be passed through unchanged, not negated"
                );
            }
            other => panic!("expected a call, got {other:?}"),
        }
    }

    /// An assignment is REFUSED, and the refusal says why `discard` is not the answer.
    ///
    /// The old implementation emitted `Call { name: "discard", ... }` -- a function that does
    /// not exist -- on the reasoning that "the inverse is just discarding x". For a `[1]`
    /// binding that is not an undo, it is the soundness hole this language exists to prevent,
    /// so the refusal has to name that rather than just report an error.
    #[test]
    fn an_assignment_is_refused_and_discard_is_not_presented_as_an_undo() {
        let err = generate_assignment_inverse(&PirExpr::Var("x".into()), StmtId(5)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("discard"),
            "the refusal must address the `discard` idea directly: {msg}"
        );
        assert!(
            msg.contains("soundness hole"),
            "and say why dropping a [1] binding is not an undo: {msg}"
        );
    }

    /// The no-rule fallback is REFUSED, not a call to `affine_inverse`.
    ///
    /// That function ignored its `expr` argument entirely and passed it through to a callee
    /// that does not exist, under a comment claiming it extracted and inverted an affine map.
    /// It did neither. It is also the arm every unrecognised expression reached, so it was
    /// this module's universal "yes" answer.
    #[test]
    fn an_expression_with_no_inverse_rule_is_refused_not_passed_to_affine_inverse() {
        let err = generate_affine_inverse(&PirExpr::IntLit(7), StmtId(6)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("affine_inverse"),
            "the refusal must name the fabricated callee it replaces: {msg}"
        );
        assert!(
            msg.contains("invertible"),
            "and say why 'the affine inverse' cannot be assumed: {msg}"
        );
    }

    #[test]
    fn test_dataflow_dag_build() {
        let mut quantities = QuantityMap::new();
        quantities.insert("x".to_string(), Quantity::Many);
        quantities.insert("y".to_string(), Quantity::Many);
        quantities.insert("proof".to_string(), Quantity::Zero);

        let mut dag = DataflowDAG::new(&quantities);

        // Create test statements: let x = 1; let y = x + 2;
        let stmt1 = PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "x".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::IntLit(1)),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let stmt2 = PirStatement {
            id: StmtId(1),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "y".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::Binary {
                    op: crate::ir::BinaryOp::Add,
                    left: Box::new(PirExpr::Var("x".to_string())),
                    right: Box::new(PirExpr::IntLit(2)),
                }),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        dag.build(&[stmt1, stmt2]).unwrap();

        // Check nodes exist
        assert!(dag.nodes.contains_key("x"));
        assert!(dag.nodes.contains_key("y"));

        // Check zero-qty tracking
        assert!(dag.zero_qty_temps.contains("proof"));
        assert!(!dag.zero_qty_temps.contains("x"));

        // Check dependencies: y depends on x
        let y_node = dag.nodes.get("y").unwrap();
        assert!(y_node.dependencies.contains(&"x".to_string()));
    }

    #[test]
    fn test_topological_sort() {
        let mut quantities = QuantityMap::new();
        quantities.insert("a".to_string(), Quantity::Many);
        quantities.insert("b".to_string(), Quantity::Many);
        quantities.insert("c".to_string(), Quantity::Many);

        let mut dag = DataflowDAG::new(&quantities);

        // a -> b -> c (a used to compute b, b used to compute c)
        let stmt_a = PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "a".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::IntLit(1)),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let stmt_b = PirStatement {
            id: StmtId(1),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "b".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::Binary {
                    op: crate::ir::BinaryOp::Add,
                    left: Box::new(PirExpr::Var("a".to_string())),
                    right: Box::new(PirExpr::IntLit(1)),
                }),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let stmt_c = PirStatement {
            id: StmtId(2),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "c".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::Binary {
                    op: crate::ir::BinaryOp::Add,
                    left: Box::new(PirExpr::Var("b".to_string())),
                    right: Box::new(PirExpr::IntLit(1)),
                }),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        dag.build(&[stmt_a, stmt_b, stmt_c]).unwrap();
        let topo = dag.topological_sort().unwrap();

        // Reverse topological order: c, b, a (uncompute c first, then b, then a)
        assert_eq!(topo[0], "c");
        assert_eq!(topo[1], "b");
        assert_eq!(topo[2], "a");
    }

    #[test]
    fn test_cycle_detection() {
        let mut quantities = QuantityMap::new();
        quantities.insert("x".to_string(), Quantity::Many);
        quantities.insert("y".to_string(), Quantity::Many);

        let mut dag = DataflowDAG::new(&quantities);

        // Create cycle: x = y + 1; y = x + 1
        let stmt_x = PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "x".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::Binary {
                    op: crate::ir::BinaryOp::Add,
                    left: Box::new(PirExpr::Var("y".to_string())),
                    right: Box::new(PirExpr::IntLit(1)),
                }),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let stmt_y = PirStatement {
            id: StmtId(1),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "y".to_string(),
                qty: Quantity::Many,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::Binary {
                    op: crate::ir::BinaryOp::Add,
                    left: Box::new(PirExpr::Var("x".to_string())),
                    right: Box::new(PirExpr::IntLit(1)),
                }),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        dag.build(&[stmt_x, stmt_y]).unwrap();
        let result = dag.topological_sort();

        // Should detect cycle
        assert!(result.is_err());
        if let Err(LoweringError::NonReversibleOp(msg)) = result {
            assert!(msg.contains("Cycle"));
        } else {
            panic!("Expected cycle detection error");
        }
    }

    #[test]
    fn test_zero_quantity_erasure() {
        let mut quantities = QuantityMap::new();
        quantities.insert("runtime_var".to_string(), Quantity::Many);
        quantities.insert("proof_var".to_string(), Quantity::Zero);

        let mut dag = DataflowDAG::new(&quantities);

        let stmt = PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "proof_var".to_string(),
                qty: Quantity::Zero,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::IntLit(42)),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Zero,
            mutability: Mutability::Immutable,
            span: None,
        };

        dag.build(&[stmt]).unwrap();

        // proof_var should be in erased_temps
        assert!(dag.erased_temps().contains("proof_var"));
        assert!(!dag.erased_temps().contains("runtime_var"));

        // In topological sort, zero-qty temps should be skipped
        let _topo = dag.topological_sort().unwrap();
        // proof_var has no uses, so it might not appear in DAG at all
        // or it should be filtered out
    }

    #[test]
    fn test_ancilla_allocation() {
        let mut quantities = QuantityMap::new();
        quantities.insert("q".to_string(), Quantity::Many);

        let mut dag = DataflowDAG::new(&quantities);

        // qalloc creates ancilla
        let stmt = PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Call {
                name: "qalloc".to_string(),
                args: vec![PirExpr::Var("q".to_string())],
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        dag.build(&[stmt]).unwrap();

        let node = dag.nodes.get("q").unwrap();
        assert!(node.is_ancilla);
    }

    #[test]
    fn test_reversible_schedule_pair() {
        // This test verifies the overall structure compiles and runs
        let mut ctx = LoweringContext::new();

        let block = crate::ast::expr::ReversibleBlock {
            body: Block::new(vec![], None, make_span()),
            uncomputes: vec![],
            span: make_span(),
        };

        let result = lower_reversible_block(&block, &mut ctx);
        assert!(result.is_ok());

        let pair = result.unwrap();
        // Should have empty forward and inverse schedules
        assert!(matches!(pair.forward.root, ScheduleNode::Empty));
        assert!(matches!(pair.inverse.root, ScheduleNode::Empty));
    }

    /// Measurement uncomputation is REFUSED -- and specifically not `unmeasure`.
    ///
    /// The old version returned `Ok` with a call named `unmeasure`, a function that exists
    /// nowhere in this repository, whose operand fell back to the literal `0` when no qubit
    /// argument was supplied. An integer standing in for a quantum pointer is not a
    /// fabrication that fails; it is one that produces valid-looking PIR.
    ///
    /// Every operand shape is tested, because the fallback branch -- the one that invented
    /// the `0` -- is the one a single-argument test would miss.
    #[test]
    fn measurement_uncomputation_is_refused_and_invents_no_qubit_operand() {
        for args in [
            vec![],
            vec![PirExpr::Var("q".to_string())],
            vec![PirExpr::IntLit(0)],
        ] {
            let err = generate_measurement_uncompute("measure", &args, StmtId(8)).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("MEASUREMENT"),
                "the refusal must name the non-invertibility: {msg}"
            );
            assert!(
                msg.contains("outcome"),
                "and say what is actually required -- carrying the classical outcome: {msg}"
            );
        }
    }

    /// RNG uncomputation is REFUSED -- and specifically not `unrng`.
    ///
    /// The old version returned `Ok` with a call named `unrng` taking NO arguments, so even
    /// if the callee existed it could not have recovered the entropy it claimed to uncompute.
    #[test]
    fn rng_uncomputation_is_refused_rather_than_emitting_an_unrng_call() {
        let err = generate_rng_uncompute("rng", &[], StmtId(9)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("search, not a computation"),
            "the refusal must say why randomness cannot be undone: {msg}"
        );
        assert!(
            msg.contains("unrng"),
            "and name the fabricated callee it replaces: {msg}"
        );
    }
}
