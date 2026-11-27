//! Mutable Value Semantics (MVS) encoding for SMT-LIB2.
//!
//! This module translates Naso's `inout` parameters and mutable value semantics
//! into frame conditions and disjointness guarantees in SMT-LIB2.

#[cfg(feature = "z3")]
use crate::error::{LoweringError, VerifyError};
#[cfg(feature = "z3")]
use crate::smtlib::{Sort, Term, builder::*};
#[cfg(feature = "z3")]
use indexmap::IndexMap;
#[cfg(feature = "z3")]
use naso_compiler::ast::expr::ExprKind;
#[cfg(feature = "z3")]
use naso_compiler::ast::{Mutability, Span};

#[cfg(feature = "z3")]
/// Represents an `inout` parameter with its frame condition.
#[derive(Debug, Clone)]
pub struct InoutParam {
    pub name: String,
    pub span: Span,
    pub ty: naso_compiler::ast::Type,
    /// The frame: set of memory locations this inout may modify
    pub frame: Frame,
}

#[cfg(feature = "z3")]
/// Frame condition: a set of disjoint memory regions that may be modified.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    /// Named regions (e.g., "array_data", "struct_field_x")
    pub regions: IndexMap<String, Region>,
    /// Whether the frame is open (may modify unknown regions)
    pub is_open: bool,
}

#[cfg(feature = "z3")]
impl Frame {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_region(&mut self, name: String, region: Region) {
        self.regions.insert(name, region);
    }

    pub fn is_disjoint_from(&self, other: &Frame) -> bool {
        if self.is_open || other.is_open {
            return false; // Conservative: open frames may overlap
        }
        // Check all region pairs for overlap
        for (_, r1) in &self.regions {
            for (_, r2) in &other.regions {
                if r1.overlaps(r2) {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(feature = "z3")]
/// A memory region with base and bounds.
#[derive(Debug, Clone)]
pub struct Region {
    pub base: Term,         // Base address/offset
    pub size: Term,         // Size in bytes/elements
    pub element_sort: Sort, // Element type
}

#[cfg(feature = "z3")]
impl Region {
    pub fn new(base: Term, size: Term, element_sort: Sort) -> Self {
        Self {
            base,
            size,
            element_sort,
        }
    }

    /// Check if this region overlaps with another.
    pub fn overlaps(&self, _other: &Region) -> bool {
        // Conservative: if we can't prove disjoint, assume overlap
        // In SMT, this would be a constraint: (overlap r1 r2) = (r1.base < r2.base + r2.size) && (r2.base < r1.base + r1.size)
        // For now, return true (assume overlap) unless we have concrete disjointness info
        true
    }

    /// Generate SMT constraint for disjointness from another region.
    pub fn disjoint_from(&self, other: &Region) -> Term {
        // disjoint = (self.base + self.size <= other.base) OR (other.base + other.size <= self.base)
        let self_end = add(vec![self.base.clone(), self.size.clone()]);
        let other_end = add(vec![other.base.clone(), other.size.clone()]);

        or(vec![
            le(self_end, other.base.clone()),
            le(other_end, self.base.clone()),
        ])
    }
}

#[cfg(feature = "z3")]
/// Tracks MVS state during lowering.
pub struct MvsTracker {
    /// Active inout parameters in current scope
    inout_params: IndexMap<String, InoutParam>,
    /// Frame conditions for each function
    function_frames: IndexMap<String, Frame>,
    /// Current function being processed
    current_function: Option<String>,
}

#[cfg(feature = "z3")]
impl MvsTracker {
    pub fn new() -> Self {
        Self {
            inout_params: IndexMap::new(),
            function_frames: IndexMap::new(),
            current_function: None,
        }
    }

    /// Enter a function scope.
    pub fn enter_function(&mut self, name: String) {
        self.current_function = Some(name.clone());
        self.function_frames.insert(name, Frame::new());
    }

    /// Exit current function scope.
    pub fn exit_function(&mut self) -> Option<Frame> {
        let name = self.current_function.take()?;
        self.function_frames.shift_remove(&name)
    }

    /// Register an inout parameter.
    pub fn register_inout(&mut self, param: InoutParam) {
        let frame = param.frame.clone();
        self.inout_params.insert(param.name.clone(), param);

        // Add to function frame
        #[allow(clippy::collapsible_if)]
        if let Some(func) = &self.current_function {
            if let Some(func_frame) = self.function_frames.get_mut(func) {
                for (name, region) in frame.regions {
                    func_frame.add_region(name, region);
                }
            }
        }
    }

    /// Get current function's frame.
    pub fn current_frame(&self) -> Option<&Frame> {
        self.current_function
            .as_ref()
            .and_then(|f| self.function_frames.get(f))
    }

    /// Check if two inout parameters have disjoint frames.
    pub fn check_disjoint(&self, param1: &str, param2: &str) -> Result<bool, VerifyError> {
        let p1 = self.inout_params.get(param1).ok_or_else(|| {
            VerifyError::Lowering(LoweringError::MvsError {
                span: Span::new(0, 0, 1, 1),
                msg: format!("Unknown inout parameter: {}", param1),
            })
        })?;
        let p2 = self.inout_params.get(param2).ok_or_else(|| {
            VerifyError::Lowering(LoweringError::MvsError {
                span: Span::new(0, 0, 1, 1),
                msg: format!("Unknown inout parameter: {}", param2),
            })
        })?;

        Ok(p1.frame.is_disjoint_from(&p2.frame))
    }

    /// Generate frame condition axioms for current function.
    pub fn generate_frame_axioms(&self) -> Vec<Term> {
        let mut axioms = Vec::new();

        if let Some(frame) = self.current_frame() {
            // For each pair of regions in the frame, assert disjointness
            let region_names: Vec<&String> = frame.regions.keys().collect();
            for i in 0..region_names.len() {
                for j in (i + 1)..region_names.len() {
                    let r1 = &frame.regions[region_names[i]];
                    let r2 = &frame.regions[region_names[j]];
                    axioms.push(r1.disjoint_from(r2));
                }
            }
        }

        axioms
    }

    /// Generate SMT declarations for inout parameters.
    pub fn generate_declarations(&self, script: &mut crate::smtlib::Script) {
        for (name, param) in &self.inout_params {
            // Declare the inout parameter as an array (memory model)
            let array_sort = Sort::Array(Box::new(Sort::Int), Box::new(Sort::Int)); // Simplified
            script.declare_const(&format!("inout_{}", name), array_sort);

            // Declare frame regions
            for (rname, _region) in &param.frame.regions {
                script.declare_const(&format!("frame_{}_{}", name, rname), Sort::Int); // base
                script.declare_const(&format!("frame_{}_{}_size", name, rname), Sort::Int); // size
            }
        }
    }
}

#[cfg(feature = "z3")]
impl Default for MvsTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "z3")]
/// Encode MVS constraints for a function with inout parameters.
pub fn encode_mvs_function(
    func: &naso_compiler::ast::Function,
    tracker: &mut MvsTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    // Enter function scope
    tracker.enter_function(func.name.name.clone());

    // Register inout parameters
    for param in &func.params {
        if param.mutability == Mutability::InOut {
            let region_name = format!("param_{}", param.name.name);
            let base = var(&format!("inout_{}_base", param.name.name), Sort::Int);
            let size = var(&format!("inout_{}_size", param.name.name), Sort::Int);
            let region = Region::new(base, size, Sort::Int);

            let mut frame = Frame::new();
            frame.add_region(region_name, region);

            tracker.register_inout(InoutParam {
                name: param.name.name.clone(),
                span: param.span,
                ty: param.ty.clone(),
                frame,
            });
        }
    }

    // Generate frame axioms (disjointness of inout regions)
    constraints.extend(tracker.generate_frame_axioms());

    // Process function body
    if let Some(body_expr) = &func.body.expr {
        constraints.extend(encode_mvs_expr(body_expr.as_ref(), tracker)?);
    }
    for stmt in &func.body.stmts {
        if let naso_compiler::ast::StmtKind::Expr(expr) = &stmt.kind {
            constraints.extend(encode_mvs_expr(expr, tracker)?);
        }
    }

    // Exit function scope
    tracker.exit_function();

    Ok(constraints)
}

#[cfg(feature = "z3")]
/// Encode MVS constraints for an expression.
pub fn encode_mvs_expr(
    expr: &naso_compiler::ast::Expr,
    _tracker: &mut MvsTracker,
) -> Result<Vec<Term>, VerifyError> {
    let mut constraints = Vec::new();

    match &expr.kind {
        ExprKind::Call(func, args) => {
            // Check if calling a function with inout params
            if let ExprKind::Var(_fname) = &func.kind {
                // Would need to look up callee's frame and check disjointness
            }
            for arg in args {
                constraints.extend(encode_mvs_expr(arg, _tracker)?);
            }
        }
        ExprKind::Let(binding) => {
            constraints.extend(encode_mvs_expr(&binding.value, _tracker)?);
        }
        ExprKind::LetInOut(binding) => {
            constraints.extend(encode_mvs_expr(&binding.value, _tracker)?);
        }
        ExprKind::LetConsume(binding) => {
            constraints.extend(encode_mvs_expr(&binding.value, _tracker)?);
        }
        ExprKind::Block(block) => {
            if let Some(body_expr) = &block.expr {
                constraints.extend(encode_mvs_expr(body_expr.as_ref(), _tracker)?);
            }
            for stmt in &block.stmts {
                if let naso_compiler::ast::StmtKind::Expr(stmt_expr) = &stmt.kind {
                    constraints.extend(encode_mvs_expr(stmt_expr, _tracker)?);
                }
            }
        }
        ExprKind::If(cond, then_e, else_e) => {
            constraints.extend(encode_mvs_expr(cond, _tracker)?);
            constraints.extend(encode_mvs_expr(then_e, _tracker)?);
            if let Some(else_e) = else_e {
                constraints.extend(encode_mvs_expr(else_e, _tracker)?);
            }
        }
        ExprKind::Binary(_, lhs, rhs) => {
            constraints.extend(encode_mvs_expr(lhs, _tracker)?);
            constraints.extend(encode_mvs_expr(rhs, _tracker)?);
        }
        ExprKind::Unary(_, operand) => {
            constraints.extend(encode_mvs_expr(operand, _tracker)?);
        }
        ExprKind::MethodCall(receiver, _, args) => {
            constraints.extend(encode_mvs_expr(receiver, _tracker)?);
            for arg in args {
                constraints.extend(encode_mvs_expr(arg, _tracker)?);
            }
        }
        ExprKind::QuantumOp(_) => {}
        _ => {}
    }

    Ok(constraints)
}

#[cfg(feature = "z3")]
/// Helper to get current function name (placeholder).
#[allow(dead_code)]
fn func_name() -> String {
    "current".to_string()
}
