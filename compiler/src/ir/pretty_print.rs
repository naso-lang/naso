//! Pretty Printing for PIR
//!
//! Human-readable text format for debugging and golden fixtures.

#![allow(clippy::useless_format)]

use super::access_relation::AccessRelations;
use super::affine_domain::AffineDomain;
use super::affine_map::{AffineMap, Matrix};
use super::pir_types::{BinaryOp, PirExpr, PirModule, UnaryOp};
use super::schedule_tree::ScheduleTree;

/// Convert PIR module to human-readable string
pub fn pir_to_string(module: &PirModule) -> String {
    module.pretty_print()
}

/// Convert PIR module to JSON
pub fn pir_to_json(module: &PirModule) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(module)
}

/// Parse PIR from JSON
pub fn pir_from_json(json: &str) -> Result<PirModule, serde_json::Error> {
    serde_json::from_str(json)
}

/// Format an affine domain as string
pub fn format_domain(domain: &AffineDomain) -> String {
    let mut s = format!("{{ {} ", domain.name.as_deref().unwrap_or(""));
    if domain.n_iter > 0 {
        s += &format!("iterators={}, ", domain.n_iter);
    }
    if domain.n_param > 0 {
        s += &format!("params={}, ", domain.n_param);
    }
    s += &format!("constraints={} }}", domain.constraints.len());
    s
}

/// Format an affine map as string
pub fn format_map(map: &AffineMap) -> String {
    format!("AffineMap({} pieces)", map.pieces.len())
}

/// Format a matrix as string
pub fn format_matrix(m: &Matrix) -> String {
    format!("{}", m)
}

/// Format schedule tree as string
pub fn format_schedule(tree: &ScheduleTree) -> String {
    tree.pretty_print()
}

/// Format access relations as string
pub fn format_accesses(accesses: &AccessRelations) -> String {
    let mut s = String::new();
    for rel in &accesses.relations {
        s += &format!("  {}\n", rel);
    }
    s
}

/// Format PIR expression as string
pub fn format_pir_expr(expr: &PirExpr) -> String {
    pir_expr_to_string(expr, 0)
}

fn pir_expr_to_string(expr: &PirExpr, _indent: usize) -> String {
    match expr {
        PirExpr::IntLit(v) => format!("{}", v),
        PirExpr::FloatLit(v) => format!("{}", v),
        PirExpr::BoolLit(v) => format!("{}", v),
        PirExpr::Var(v) => v.clone(),
        PirExpr::Assign { target, value } => {
            format!(
                "{} = {}",
                pir_expr_to_string(target, 0),
                pir_expr_to_string(value, 0)
            )
        }
        PirExpr::Cast {
            expr,
            width,
            signed,
        } => format!(
            "{} as {}i{}",
            pir_expr_to_string(expr, 0),
            if *signed { "" } else { "u" },
            width.unwrap_or(32)
        ),
        PirExpr::Stmts(parts) => {
            let inner: Vec<String> = parts.iter().map(|p| pir_expr_to_string(p, 0)).collect();
            if inner.is_empty() {
                "{}".to_string()
            } else {
                format!("{{ {}; }}", inner.join("; "))
            }
        }
        PirExpr::Binary { op, left, right } => {
            format!(
                "({} {} {})",
                pir_expr_to_string(left, 0),
                binary_op_to_str(*op),
                pir_expr_to_string(right, 0)
            )
        }
        PirExpr::Unary { op, expr } => {
            format!("{}{}", unary_op_to_str(*op), pir_expr_to_string(expr, 0))
        }
        PirExpr::Call { name, args } => {
            format!(
                "{}({})",
                name,
                args.iter()
                    .map(|a| pir_expr_to_string(a, 0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        PirExpr::Index { base, indices } => {
            format!(
                "{}[{}]",
                pir_expr_to_string(base, 0),
                indices
                    .iter()
                    .map(|i| pir_expr_to_string(i, 0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        PirExpr::Field { base, field } => {
            format!("{}.{}", pir_expr_to_string(base, 0), field)
        }
        PirExpr::Let {
            name,
            qty,
            mutability,
            value,
            body,
        } => {
            format!(
                "let {:?} {:?} {} = {}; {}",
                qty,
                mutability,
                name,
                pir_expr_to_string(value, 0),
                pir_expr_to_string(body, 0)
            )
        }
        PirExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            format!(
                "if {} then {} else {}",
                pir_expr_to_string(cond, 0),
                pir_expr_to_string(then_branch, 0),
                pir_expr_to_string(else_branch, 0)
            )
        }
        PirExpr::Reversible { body, inverse } => {
            format!(
                "reversible {} inv {}",
                pir_expr_to_string(body, 0),
                pir_expr_to_string(inverse, 0)
            )
        }
        PirExpr::QuantumOp { op, args, qubits } => {
            format!(
                "quantum {}({}, qubits={})",
                op,
                args.iter()
                    .map(|a| pir_expr_to_string(a, 0))
                    .collect::<Vec<_>>()
                    .join(", "),
                qubits
                    .iter()
                    .map(|q| pir_expr_to_string(q, 0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn binary_op_to_str(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::Xor => "^",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
    }
}

fn unary_op_to_str(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

/// Format for golden fixture files (structured text)
pub fn format_golden_fixture(module: &PirModule) -> String {
    let mut s = String::new();
    s += "# Naso PIR Golden Fixture\n";
    s += "# Generated by Naso Compiler\n\n";

    s += "[parameters]\n";
    for p in &module.parameters {
        s += &format!("  {}\n", p);
    }
    s += "\n";

    s += "[quantities]\n";
    for (var, qty) in &module.quantities {
        s += &format!("  {} = {:?}\n", var, qty);
    }
    s += "\n";

    s += "[statements]\n";
    for stmt in &module.statements {
        s += &format!("  {} {{\n", stmt.id);
        s += &format!("    quantity = {:?}\n", stmt.quantity);
        s += &format!("    mutability = {:?}\n", stmt.mutability);
        s += &format!("    domain = {}\n", format_domain(&stmt.domain));
        s += &format!("    body = {}\n", format_pir_expr(&stmt.body));
        s += "  }\n";
    }
    s += "\n";

    s += "[schedule]\n";
    s += &format_schedule(&module.schedule);
    s += "\n";

    s += "[accesses]\n";
    s += &format_accesses(&module.accesses);
    s += "\n";

    if !module.extern_functions.is_empty() {
        s += "[extern_functions]\n";
        for ext in &module.extern_functions {
            s += &format!("  {}(", ext.name);
            for (i, p) in ext.params.iter().enumerate() {
                if i > 0 {
                    s += ", ";
                }
                s += &format!("{}: {} [{:?} {:?}]", p.name, p.ty, p.quantity, p.mutability);
            }
            s += ")";
            if let Some(ret) = &ext.return_type {
                s += &format!(" -> {}", ret);
            }
            s += "\n";
        }
        s += "\n";
    }

    s
}

#[cfg(test)]
mod tests {
    use super::super::access_relation::{AccessRelation, AccessRelations, AccessType};
    use super::super::affine_domain::AffineDomain;
    use super::super::affine_map::{AffineMap, Matrix};
    use super::super::pir_types::{PirExpr, PirModule, PirStatement, QuantityMap};
    use super::super::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};
    use super::*;
    use crate::ast::{Mutability, Quantity};

    #[test]
    fn test_pir_to_string() {
        let domain = AffineDomain::universe(1, 0);
        let mut m = Matrix::new(1, 1);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        let stmt = PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::IntLit(42),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let schedule = ScheduleTree::new(
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            vec![],
        );

        let mut accesses = AccessRelations::new();
        accesses.add(AccessRelation::new(
            StmtId(0),
            domain.clone(),
            map,
            AccessType::Write,
        ));

        let mut quantities = QuantityMap::new();
        quantities.insert("x".to_string(), Quantity::Many);

        let module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);

        let s = pir_to_string(&module);
        assert!(s.contains("PIR Module"));
        assert!(s.contains("S0"));
    }

    #[test]
    fn test_format_golden_fixture() {
        let domain = AffineDomain::universe(1, 1);
        let mut m = Matrix::new(1, 2);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        let stmt = PirStatement {
            id: StmtId(0),
            domain: domain.clone().with_name("S_domain".to_string()),
            body: PirExpr::IntLit(42),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let schedule = ScheduleTree::new(
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            vec!["N".to_string()],
        );

        let mut accesses = AccessRelations::new();
        accesses.add(
            AccessRelation::new(StmtId(0), domain, map, AccessType::Write).with_array_name("A"),
        );

        let mut quantities = QuantityMap::new();
        quantities.insert("n".to_string(), Quantity::Zero);

        let module = PirModule::new(
            vec![stmt],
            schedule,
            accesses,
            quantities,
            vec!["N".to_string()],
        );

        let fixture = format_golden_fixture(&module);
        assert!(fixture.contains("[parameters]"));
        assert!(fixture.contains("N"));
        assert!(fixture.contains("[quantities]"));
        assert!(fixture.contains("n = Zero"));
        assert!(fixture.contains("[statements]"));
        assert!(fixture.contains("S0"));
        assert!(fixture.contains("[schedule]"));
        assert!(fixture.contains("[accesses]"));
        assert!(fixture.contains("A"));
    }
}
