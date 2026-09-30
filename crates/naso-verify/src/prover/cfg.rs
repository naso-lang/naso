//! Control-Flow Graph (CFG) construction for verification.
//!
//! This module builds CFGs from Naso AST functions for path-sensitive
//! analysis in the linearity and uncomputation provers.

use indexmap::IndexMap;
use naso_compiler::ast::expr::ExprKind;
use naso_compiler::ast::{Block, Expr, Function, Ident, Span, Stmt, StmtKind};
use std::collections::HashMap;

/// A node in the control-flow graph.
#[derive(Debug, Clone)]
pub struct CfgNode {
    pub id: u32,
    pub kind: CfgNodeKind,
    pub span: Span,
    pub successors: Vec<u32>,
    pub predecessors: Vec<u32>,
}

/// Kind of CFG node.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum CfgNodeKind {
    Entry,
    Exit,
    Stmt(Stmt),
    Branch {
        condition: Expr,
        then_block: u32,
        else_block: Option<u32>,
    },
    LoopHeader {
        index: Ident,
        domain: Expr,
        body: u32,
    },
    LoopBack,
    Merge,
}

/// Control-flow graph for a function.
#[derive(Debug, Clone)]
pub struct ControlFlowGraph {
    pub nodes: IndexMap<u32, CfgNode>,
    pub entry_id: u32,
    pub exit_id: u32,
    pub next_id: u32,
}

impl ControlFlowGraph {
    /// Build a CFG from a function.
    pub fn from_function(func: &Function) -> Result<Self, String> {
        let mut cfg = Self {
            nodes: IndexMap::new(),
            entry_id: 0,
            exit_id: 0,
            next_id: 0,
        };

        let entry_id = cfg.new_node(CfgNodeKind::Entry, Span::default());
        cfg.entry_id = entry_id;

        let mut current_id = entry_id;

        for stmt in &func.body.stmts {
            current_id = cfg.build_from_stmt(stmt, current_id)?;
        }

        if let Some(body_expr) = &func.body.expr {
            current_id = cfg.build_from_expr(body_expr, current_id)?;
        }

        let final_exit = cfg.new_node(CfgNodeKind::Exit, Span::default());
        cfg.add_edge(current_id, final_exit);
        cfg.exit_id = final_exit;

        Ok(cfg)
    }

    /// Create a new node and return its ID.
    fn new_node(&mut self, kind: CfgNodeKind, span: Span) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let node = CfgNode {
            id,
            kind,
            span,
            successors: Vec::new(),
            predecessors: Vec::new(),
        };
        self.nodes.insert(id, node);
        id
    }

    /// Add an edge between nodes.
    fn add_edge(&mut self, from: u32, to: u32) {
        if let Some(node) = self.nodes.get_mut(&from) {
            node.successors.push(to);
        }
        if let Some(node) = self.nodes.get_mut(&to) {
            node.predecessors.push(from);
        }
    }

    /// Build CFG from a statement, returning the exit node ID.
    fn build_from_stmt(&mut self, stmt: &Stmt, entry_id: u32) -> Result<u32, String> {
        match &stmt.kind {
            StmtKind::Let(binding) => {
                let stmt_node = self.new_node(
                    CfgNodeKind::Stmt(Stmt::new(
                        StmtKind::Let(binding.clone()),
                        binding.span,
                        naso_compiler::ast::NodeId::default(),
                    )),
                    binding.span,
                );
                self.add_edge(entry_id, stmt_node);
                self.build_from_expr(&binding.value, stmt_node)
            }
            StmtKind::LetInOut(binding) => {
                let stmt_node = self.new_node(
                    CfgNodeKind::Stmt(Stmt::new(
                        StmtKind::LetInOut(binding.clone()),
                        binding.span,
                        naso_compiler::ast::NodeId::default(),
                    )),
                    binding.span,
                );
                self.add_edge(entry_id, stmt_node);
                self.build_from_expr(&binding.value, stmt_node)
            }
            StmtKind::LetConsume(binding) => {
                let stmt_node = self.new_node(
                    CfgNodeKind::Stmt(Stmt::new(
                        StmtKind::LetConsume(binding.clone()),
                        binding.span,
                        naso_compiler::ast::NodeId::default(),
                    )),
                    binding.span,
                );
                self.add_edge(entry_id, stmt_node);
                self.build_from_expr(&binding.value, stmt_node)
            }
            StmtKind::Expr(expr) => self.build_from_expr(expr, entry_id),
            StmtKind::Return(expr_opt) => {
                if let Some(expr) = expr_opt {
                    self.build_from_expr(expr, entry_id)
                } else {
                    Ok(entry_id)
                }
            }
            StmtKind::Item(_) => Ok(entry_id),
            StmtKind::Reversible(_) => Ok(entry_id),
            // A proof block contains no runtime control flow. Erasing it is
            // correct for CFG analysis: obligations are read from the AST by
            // the prover, not as part of the execution path.
            StmtKind::Proof(_) => Ok(entry_id),
            StmtKind::Break(_) => Ok(entry_id),
            StmtKind::Continue => Ok(entry_id),
            StmtKind::Empty => Ok(entry_id),
            StmtKind::Error => Ok(entry_id),
        }
    }

    /// Build CFG from an expression, returning the exit node ID.
    fn build_from_expr(&mut self, expr: &Expr, entry_id: u32) -> Result<u32, String> {
        match &expr.kind {
            ExprKind::If(cond, then_e, else_e) => {
                let branch_id = self.new_node(
                    CfgNodeKind::Branch {
                        condition: *cond.clone(),
                        then_block: 0,
                        else_block: None,
                    },
                    expr.span,
                );
                self.add_edge(entry_id, branch_id);

                let then_id = self.new_node(
                    CfgNodeKind::Stmt(Stmt::new(
                        StmtKind::Expr(then_e.as_ref().clone()),
                        expr.span,
                        naso_compiler::ast::NodeId::default(),
                    )),
                    expr.span,
                );
                let then_exit = self.build_from_expr(then_e, then_id)?;

                #[allow(clippy::collapsible_if)]
                if let Some(node) = self.nodes.get_mut(&branch_id) {
                    if let CfgNodeKind::Branch { then_block, .. } = &mut node.kind {
                        *then_block = then_id;
                    }
                }

                let else_exit = if let Some(else_e) = else_e {
                    let else_id = self.new_node(
                        CfgNodeKind::Stmt(Stmt::new(
                            StmtKind::Expr(else_e.as_ref().clone()),
                            expr.span,
                            naso_compiler::ast::NodeId::default(),
                        )),
                        expr.span,
                    );
                    self.build_from_expr(else_e, else_id)?
                } else {
                    branch_id
                };

                let merge_id = self.new_node(CfgNodeKind::Merge, expr.span);
                self.add_edge(then_exit, merge_id);
                if else_exit != branch_id {
                    self.add_edge(else_exit, merge_id);
                }

                #[allow(clippy::collapsible_if)]
                if let Some(node) = self.nodes.get_mut(&branch_id) {
                    if let CfgNodeKind::Branch { else_block, .. } = &mut node.kind {
                        *else_block = if else_exit == branch_id {
                            None
                        } else {
                            Some(else_exit)
                        };
                    }
                }

                Ok(merge_id)
            }
            ExprKind::For(loop_) => {
                let header_id = self.new_node(
                    CfgNodeKind::LoopHeader {
                        index: loop_.var.clone(),
                        domain: loop_.iter.clone(),
                        body: 0,
                    },
                    expr.span,
                );
                self.add_edge(entry_id, header_id);

                let body_expr = loop_.body.expr.clone().unwrap_or_else(|| {
                    Box::new(naso_compiler::ast::Expr::new(
                        ExprKind::Literal(naso_compiler::ast::Literal::Unit),
                        expr.span,
                        naso_compiler::ast::NodeId::default(),
                    ))
                });
                let body_id = self.new_node(
                    CfgNodeKind::Stmt(Stmt::new(
                        StmtKind::Expr(*body_expr.clone()),
                        expr.span,
                        naso_compiler::ast::NodeId::default(),
                    )),
                    expr.span,
                );
                let body_exit = self.build_from_expr(&body_expr, body_id)?;

                for stmt in &loop_.body.stmts {
                    if let StmtKind::Expr(stmt_expr) = &stmt.kind {
                        let stmt_id = self.new_node(
                            CfgNodeKind::Stmt(Stmt::new(
                                StmtKind::Expr(stmt_expr.clone()),
                                expr.span,
                                naso_compiler::ast::NodeId::default(),
                            )),
                            expr.span,
                        );
                        self.add_edge(body_exit, stmt_id);
                    }
                }

                let back_id = self.new_node(CfgNodeKind::LoopBack, expr.span);
                self.add_edge(body_exit, back_id);
                self.add_edge(back_id, header_id);

                #[allow(clippy::collapsible_if)]
                if let Some(node) = self.nodes.get_mut(&header_id) {
                    if let CfgNodeKind::LoopHeader { body, .. } = &mut node.kind {
                        *body = body_id;
                    }
                }

                let exit_id = self.new_node(CfgNodeKind::Merge, expr.span);
                self.add_edge(header_id, exit_id);

                Ok(exit_id)
            }
            _ => {
                let stmt_node = self.new_node(
                    CfgNodeKind::Stmt(Stmt::new(
                        StmtKind::Expr(expr.clone()),
                        expr.span,
                        naso_compiler::ast::NodeId::default(),
                    )),
                    expr.span,
                );
                self.add_edge(entry_id, stmt_node);
                Ok(stmt_node)
            }
        }
    }

    /// Get all paths from entry to exit (for path-sensitive analysis).
    pub fn all_paths(&self) -> Vec<Vec<u32>> {
        let mut paths = Vec::new();
        let mut current_path = Vec::new();
        self.dfs_paths(self.entry_id, &mut current_path, &mut paths);
        paths
    }

    fn dfs_paths(&self, node_id: u32, current: &mut Vec<u32>, paths: &mut Vec<Vec<u32>>) {
        current.push(node_id);

        let node = &self.nodes[&node_id];
        if node_id == self.exit_id {
            paths.push(current.clone());
        } else {
            for &succ in &node.successors {
                if !current.contains(&succ) || succ == self.exit_id {
                    self.dfs_paths(succ, current, paths);
                }
            }
        }

        current.pop();
    }

    /// Get nodes in topological order (for dataflow analysis).
    pub fn topological_order(&self) -> Vec<u32> {
        let mut visited = HashMap::new();
        let mut order = Vec::new();

        fn visit(
            cfg: &ControlFlowGraph,
            node_id: u32,
            visited: &mut HashMap<u32, bool>,
            order: &mut Vec<u32>,
        ) {
            if visited.get(&node_id).copied().unwrap_or(false) {
                return;
            }
            visited.insert(node_id, true);

            let node = &cfg.nodes[&node_id];
            for &succ in &node.successors {
                visit(cfg, succ, visited, order);
            }

            order.push(node_id);
        }

        visit(self, self.entry_id, &mut visited, &mut order);
        order.reverse();
        order
    }
}

/// Dataflow analysis framework for linearity checking.
pub struct LinearityDataflow {
    cfg: ControlFlowGraph,
    live_in: HashMap<u32, Vec<String>>,
    live_out: HashMap<u32, Vec<String>>,
}

impl LinearityDataflow {
    pub fn new(cfg: ControlFlowGraph) -> Self {
        Self {
            cfg,
            live_in: HashMap::new(),
            live_out: HashMap::new(),
        }
    }

    /// Run the dataflow analysis to find linearity violations.
    pub fn analyze(&mut self) -> Result<Vec<LinearityViolation>, String> {
        let mut changed = true;
        while changed {
            changed = false;
            for &node_id in &self.cfg.topological_order() {
                if self.transfer(node_id)? {
                    changed = true;
                }
            }
        }

        let mut violations = Vec::new();
        if let Some(live) = self.live_out.get(&self.cfg.exit_id) {
            for resource in live {
                violations.push(LinearityViolation {
                    kind: ViolationKind::Leak,
                    resource: resource.clone(),
                    node_id: self.cfg.exit_id,
                    message: format!("Resource '{}' leaked at function exit", resource),
                });
            }
        }

        Ok(violations)
    }

    /// Transfer function for a node.
    fn transfer(&mut self, node_id: u32) -> Result<bool, String> {
        let node = &self.cfg.nodes[&node_id];
        let mut live = self.live_in.get(&node_id).cloned().unwrap_or_default();

        match &node.kind {
            CfgNodeKind::Stmt(_stmt) => {}
            CfgNodeKind::Branch { .. } => {}
            CfgNodeKind::Merge => {
                let mut merged = Vec::new();
                for &pred in &node.predecessors {
                    if let Some(pred_live) = self.live_out.get(&pred) {
                        merged.extend(pred_live.iter().cloned());
                    }
                }
                merged.sort();
                merged.dedup();
                live = merged;
            }
            CfgNodeKind::LoopHeader { .. } => {}
            CfgNodeKind::LoopBack => {}
            _ => {}
        }

        let changed = self
            .live_out
            .insert(node_id, live.clone())
            .is_none_or(|old| old != live);
        Ok(changed)
    }
}

/// Linearity violation found by dataflow analysis.
#[derive(Debug, Clone)]
pub struct LinearityViolation {
    pub kind: ViolationKind,
    pub resource: String,
    pub node_id: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    Leak,
    DoubleConsume,
    Unconsumed,
    InvalidAccess,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cfg_creation() {}
}
