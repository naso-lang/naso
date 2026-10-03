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
//! ## Why it is not simply wired up
//!
//! Investigated before deciding. Three independent blockers, any one of which is fatal:
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
    /// Inverse operation to uncompute this temp (if invertible)
    inverse_op: Option<InverseOperation>,
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
                    inverse_op: None,
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
                        inverse_op: None,
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
            _ => {
                // Default: try affine map inversion for affine ops
                generate_affine_inverse(&def_expr, node.def_stmt)?
            }
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

/// Generate adjoint for quantum gate
fn generate_quantum_adjoint(
    gate: &str,
    args: &[PirExpr],
    _stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    let adjoint_gate = match gate {
        "H" => "H",    // Self-adjoint
        "X" => "X",    // Self-adjoint
        "Y" => "Y",    // Self-adjoint
        "Z" => "Z",    // Self-adjoint
        "S" => "S†",   // S† = S^3
        "T" => "T†",   // T† = T^7
        "CX" => "CX",  // Self-adjoint
        "CY" => "CY",  // Self-adjoint
        "CZ" => "CZ",  // Self-adjoint
        "RX" => "RX†", // RX(θ)† = RX(-θ)
        "RY" => "RY†", // RY(θ)† = RY(-θ)
        "RZ" => "RZ†", // RZ(θ)† = RZ(-θ)
        _ => "UNKNOWN",
    };

    // Create inverse call with negated angles for rotation gates
    let inv_args = args
        .iter()
        .map(|arg| match arg {
            PirExpr::Call { name, args } if matches!(name.as_str(), "RX" | "RY" | "RZ") => {
                if let Some(PirExpr::Var(angle)) = args.first() {
                    PirExpr::Unary {
                        op: crate::ir::UnaryOp::Neg,
                        expr: Box::new(PirExpr::Var(angle.clone())),
                    }
                } else {
                    arg.clone()
                }
            }
            _ => arg.clone(),
        })
        .collect();

    Ok(InverseOperation {
        expr: PirExpr::Call {
            name: adjoint_gate.to_string(),
            args: inv_args,
        },
        schedule_map: None,
        is_adjoint: true,
        required_ancilla: Vec::new(),
    })
}

/// Generate uncompute for measurement (requires ancilla)
fn generate_measurement_uncompute(
    _name: &str,
    args: &[PirExpr],
    _stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    // Measurement is not invertible - requires ancilla qubit to record outcome
    // The uncompute would need to reverse the measurement basis
    let qubit = args.first().cloned().unwrap_or(PirExpr::IntLit(0));

    Ok(InverseOperation {
        expr: PirExpr::Call {
            name: "unmeasure".to_string(),
            args: vec![qubit],
        },
        schedule_map: None,
        is_adjoint: false,
        required_ancilla: vec!["measurement_ancilla".to_string()],
    })
}

/// Generate uncompute for RNG (requires ancilla to store entropy)
fn generate_rng_uncompute(
    _name: &str,
    _args: &[PirExpr],
    _stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    // RNG is not invertible - requires ancilla to store random bits
    Ok(InverseOperation {
        expr: PirExpr::Call {
            name: "unrng".to_string(),
            args: vec![],
        },
        schedule_map: None,
        is_adjoint: false,
        required_ancilla: vec!["rng_ancilla".to_string()],
    })
}

/// Generate inverse for arithmetic operations
fn generate_arithmetic_inverse(
    op: crate::ir::BinaryOp,
    left: &PirExpr,
    right: &PirExpr,
    _stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    // For reversible arithmetic: a + b = c, inverse is c - b = a or c - a = b
    // This is simplified - real implementation would track which operand is the output
    let inv_op = match op {
        crate::ir::BinaryOp::Add => crate::ir::BinaryOp::Sub,
        crate::ir::BinaryOp::Sub => crate::ir::BinaryOp::Add,
        crate::ir::BinaryOp::Mul => crate::ir::BinaryOp::Div,
        crate::ir::BinaryOp::Div => crate::ir::BinaryOp::Mul,
        crate::ir::BinaryOp::Xor => crate::ir::BinaryOp::Xor, // Self-inverse
        _ => crate::ir::BinaryOp::Add,                        // Default
    };

    Ok(InverseOperation {
        expr: PirExpr::Binary {
            op: inv_op,
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
        },
        schedule_map: None,
        is_adjoint: false,
        required_ancilla: Vec::new(),
    })
}

/// Generate inverse for assignment (reversible copy/uncompute)
fn generate_assignment_inverse(
    value: &PirExpr,
    _stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    // For `let x = y`, inverse is just discarding x (if y is still live)
    // or copying back if needed
    Ok(InverseOperation {
        expr: PirExpr::Call {
            name: "discard".to_string(),
            args: vec![value.clone()],
        },
        schedule_map: None,
        is_adjoint: false,
        required_ancilla: Vec::new(),
    })
}

/// Generate inverse using affine map inversion
fn generate_affine_inverse(
    expr: &PirExpr,
    _stmt_id: StmtId,
) -> Result<InverseOperation, LoweringError> {
    // Try to extract affine map from expression and invert it
    // This is a simplified version
    Ok(InverseOperation {
        expr: PirExpr::Call {
            name: "affine_inverse".to_string(),
            args: vec![expr.clone()],
        },
        schedule_map: None,
        is_adjoint: false,
        required_ancilla: Vec::new(),
    })
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
    verify_ancilla_zeroing(&requirements, dag)?;

    Ok(requirements)
}

/// Verify that all ancilla qubits can be returned to |0⟩ state
fn verify_ancilla_zeroing(
    requirements: &[AncillaRequirement],
    dag: &DataflowDAG,
) -> Result<(), LoweringError> {
    for req in requirements {
        if req.is_quantum {
            // Check that there's an inverse operation that zeros this ancilla
            let node = dag.nodes.get(&req.name);
            if node.is_none() {
                return Err(LoweringError::NonReversibleOp(format!(
                    "Quantum ancilla {} not found in DAG",
                    req.name
                )));
            }

            // The ancilla must have an inverse operation
            if node.unwrap().inverse_op.is_none() {
                return Err(LoweringError::NonReversibleOp(format!(
                    "No inverse operation for quantum ancilla {}",
                    req.name
                )));
            }
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

    #[test]
    fn test_quantum_adjoint_generation() {
        // Test H gate (self-adjoint)
        let inv = generate_quantum_adjoint("H", &[], StmtId(0)).unwrap();
        assert_eq!(
            inv.expr,
            PirExpr::Call {
                name: "H".to_string(),
                args: vec![]
            }
        );
        assert!(inv.is_adjoint);

        // Test S gate (adjoint is S†)
        let inv = generate_quantum_adjoint("S", &[], StmtId(0)).unwrap();
        assert_eq!(
            inv.expr,
            PirExpr::Call {
                name: "S†".to_string(),
                args: vec![]
            }
        );

        // Test RX gate (angle negation)
        let angle_expr = PirExpr::Var("theta".to_string());
        let args = vec![PirExpr::Call {
            name: "RX".to_string(),
            args: vec![angle_expr.clone()],
        }];
        let inv = generate_quantum_adjoint("RX", &args, StmtId(0)).unwrap();
        if let PirExpr::Call { name, args } = inv.expr {
            assert_eq!(name, "RX†");
            assert_eq!(args.len(), 1);
            // Check angle is negated
            if let PirExpr::Unary {
                op: crate::ir::UnaryOp::Neg,
                expr,
            } = &args[0]
            {
                if let PirExpr::Var(v) = expr.as_ref() {
                    assert_eq!(v, "theta");
                }
            } else {
                panic!("Expected negated angle");
            }
        } else {
            panic!("Expected call");
        }
    }

    #[test]
    fn test_arithmetic_inverse() {
        let left = PirExpr::Var("a".to_string());
        let right = PirExpr::Var("b".to_string());

        // Addition -> Subtraction
        let inv = generate_arithmetic_inverse(crate::ir::BinaryOp::Add, &left, &right, StmtId(0))
            .unwrap();
        if let PirExpr::Binary { op, .. } = inv.expr {
            assert_eq!(op, crate::ir::BinaryOp::Sub);
        } else {
            panic!("Expected binary op");
        }

        // Multiplication -> Division
        let inv = generate_arithmetic_inverse(crate::ir::BinaryOp::Mul, &left, &right, StmtId(0))
            .unwrap();
        if let PirExpr::Binary { op, .. } = inv.expr {
            assert_eq!(op, crate::ir::BinaryOp::Div);
        } else {
            panic!("Expected binary op");
        }

        // XOR is self-inverse
        let inv = generate_arithmetic_inverse(crate::ir::BinaryOp::Xor, &left, &right, StmtId(0))
            .unwrap();
        if let PirExpr::Binary { op, .. } = inv.expr {
            assert_eq!(op, crate::ir::BinaryOp::Xor);
        } else {
            panic!("Expected binary op");
        }
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

    #[test]
    fn test_measurement_uncompute_requires_ancilla() {
        let inv = generate_measurement_uncompute("measure", &[], StmtId(0)).unwrap();
        assert!(!inv.is_adjoint);
        assert!(!inv.required_ancilla.is_empty());
        assert_eq!(inv.required_ancilla[0], "measurement_ancilla");
    }

    #[test]
    fn test_rng_uncompute_requires_ancilla() {
        let inv = generate_rng_uncompute("rng", &[], StmtId(0)).unwrap();
        assert!(!inv.is_adjoint);
        assert!(!inv.required_ancilla.is_empty());
        assert_eq!(inv.required_ancilla[0], "rng_ancilla");
    }
}
