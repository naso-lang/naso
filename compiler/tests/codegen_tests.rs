//! Codegen Integration Tests
//!
//! End-to-end tests for the codegen pipeline including LLVM IR and QIR generation,
//! bitcode validation, and structural verification against golden fixtures.
//!
//! `parse_pir` below parses the real `.pir` fixtures in `tests/fixtures/` into
//! `PirModule`s so the pipeline is exercised against non-empty input.

use naso_compiler::ast::{Mutability, Quantity};
#[cfg(feature = "llvm")]
use naso_compiler::codegen::validate::BitcodeValidator;
#[cfg(feature = "llvm")]
use naso_compiler::codegen::validate::StructuralVerifier;
use naso_compiler::codegen::validate::{StructuralReport, ValidationReport};
use naso_compiler::ir::access_relation::AccessRelations;
use naso_compiler::ir::affine_domain::{AffineConstraint, AffineDomain};
use naso_compiler::ir::pir_types::{
    BinaryOp, ExternFunction, PirExpr, PirModule, PirStatement, QuantityMap,
};
use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};
use std::collections::HashMap;
use std::fs;

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

/// Resolve a fixture path relative to this crate's manifest directory.
///
/// `cargo test` runs with the *package* directory as the working directory, so
/// the repository-root-relative `compiler/tests/fixtures/...` paths this file
/// originally used never resolved at runtime.
fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Load a PIR fixture as a `PirModule`.
fn load_pir_fixture(name: &str) -> PirModule {
    let content = load_pir_text(name);
    parse_pir(&content).unwrap_or_else(|e| panic!("Failed to parse PIR fixture {}: {}", name, e))
}

/// Load the raw text of a PIR fixture.
fn load_pir_text(name: &str) -> String {
    let path = fixture_path(&format!("{}.pir", name));
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {}", path.display(), e))
}

/// Load an LLVM IR golden fixture.
#[cfg(feature = "llvm")]
fn load_llvm_fixture(name: &str) -> String {
    let path = fixture_path(&format!("{}.ll", name));
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read LLVM fixture {}: {}", path.display(), e))
}

/// Load a QIR golden fixture.
#[cfg(feature = "llvm")]
fn load_qir_fixture(name: &str) -> String {
    let path = fixture_path(&format!("{}.qir", name));
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read QIR fixture {}: {}", path.display(), e))
}

// ---------------------------------------------------------------------------
// Small text helpers
// ---------------------------------------------------------------------------

/// Split a fixture into `[header]` sections of raw lines.
fn sections(content: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            out.push((t[1..t.len() - 1].to_string(), Vec::new()));
        } else if let Some(cur) = out.last_mut() {
            cur.1.push(line.to_string());
        }
    }
    out
}

/// Split a `key = value` line, stripping comments and quotes from the value.
fn kv(line: &str) -> Option<(String, String)> {
    let t = line.split('#').next().unwrap_or(line).trim();
    let (k, v) = t.split_once('=')?;
    let k = k.trim().to_string();
    let v = v.trim().trim_matches('"').to_string();
    if k.is_empty() { None } else { Some((k, v)) }
}

fn bracket_delta(s: &str) -> i32 {
    let mut d = 0i32;
    for c in s.chars() {
        match c {
            '[' | '{' | '(' => d += 1,
            ']' | '}' | ')' => d -= 1,
            _ => {}
        }
    }
    d
}

/// Read the value of `key = ...`, continuing across lines until brackets balance.
fn field_block(lines: &[String], key: &str) -> Option<String> {
    let start = lines
        .iter()
        .position(|l| kv(l).map(|(k, _)| k == key).unwrap_or(false))?;
    let mut text = String::new();
    let mut depth = 0i32;
    for line in &lines[start..] {
        let raw = line.split('#').next().unwrap_or(line);
        if text.is_empty() {
            let (_, v) = kv(line)?;
            text = v;
        } else {
            text.push('\n');
            text.push_str(raw.trim());
        }
        depth += bracket_delta(raw);
        if depth <= 0 {
            break;
        }
    }
    Some(text.trim().to_string())
}

/// Parse integer rows out of `[[1, 0, 0], [0, 1, 0]]`; a flat `[1, 0]` is one row.
fn int_rows(s: &str) -> Vec<Vec<i64>> {
    // Constraint rows are followed by trailing comments (`[1, 0, 0, 0], # i >= 0`).
    // Without stripping them the row scan stops at the `#`.
    let stripped: String = s
        .lines()
        .map(|l| match l.find('#') {
            Some(p) => &l[..p],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = stripped.chars().collect();
    let mut i = match chars.iter().position(|c| *c == '[') {
        Some(p) => p + 1,
        None => return Vec::new(),
    };
    // Skip ahead to the first inner '[' (the outer bracket is just a wrapper).
    while i < chars.len() && chars[i] != '[' && chars[i] != ']' {
        i += 1;
    }
    if chars.get(i) != Some(&'[') {
        return Vec::new();
    }
    let mut rows = Vec::new();
    loop {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        match chars.get(i) {
            None | Some(']') => break,
            Some('[') => {
                i += 1;
                let start = i;
                let mut d = 1i32;
                while i < chars.len() && d > 0 {
                    match chars[i] {
                        '[' => d += 1,
                        ']' => d -= 1,
                        _ => {}
                    }
                    if d > 0 {
                        i += 1;
                    }
                }
                let body: String = chars[start..i].iter().collect();
                i += 1; // consume ']'
                rows.push(
                    body.split(',')
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .filter_map(|t| t.parse::<i64>().ok())
                        .collect(),
                );
            }
            Some(_) => break,
        }
    }
    rows
}

fn parse_quantity(s: &str) -> Result<Quantity, String> {
    match s {
        "Zero" => Ok(Quantity::Zero),
        "One" => Ok(Quantity::One),
        "Many" => Ok(Quantity::Many),
        other => other
            .strip_prefix("Bounded(")
            .and_then(|r| r.strip_suffix(')'))
            .and_then(|n| n.parse::<u32>().ok())
            .map(Quantity::Bounded)
            .ok_or_else(|| format!("unknown quantity {:?}", other)),
    }
}

/// The fixtures spell `Mutable`, which is `Mut` in the AST.
fn parse_mutability(s: &str) -> Result<Mutability, String> {
    match s {
        "Immutable" => Ok(Mutability::Immutable),
        "Mutable" | "Mut" => Ok(Mutability::Mut),
        "InOut" => Ok(Mutability::InOut),
        "Consume" => Ok(Mutability::Consume),
        other => Err(format!("unknown mutability {:?}", other)),
    }
}

/// A constraint row `[c0, .., cn]` reads as `sum(ci * xi) >= -cn`, the
/// convention every fixture uses: `[1,0,0,0]` is `i >= 0` and `[-1,0,0,63]`
/// is `i <= 63`.
fn parse_domain(name: &str, lines: &[String]) -> Result<AffineDomain, String> {
    let num = |key: &str| -> Result<usize, String> {
        field_block(lines, key)
            .and_then(|v| v.trim().parse::<usize>().ok())
            .ok_or_else(|| format!("domain {} missing {}", name, key))
    };
    let dims = num("dims")?;
    let n_iter = num("n_iter")?;
    let n_param = num("n_param")?;

    let joined = lines.join("\n");
    let mut constraints = Vec::new();
    for row in int_rows(&joined) {
        let (last, coeffs) = row.split_last().ok_or("empty constraint row")?;
        if coeffs.len() != dims {
            return Err(format!(
                "constraint row has {} coefficients but domain {} has dims {}",
                coeffs.len(),
                name,
                dims
            ));
        }
        constraints.push(AffineConstraint::inequality(coeffs.to_vec(), -*last));
    }

    Ok(AffineDomain {
        dims,
        n_iter,
        n_param,
        constraints,
        name: Some(name.to_string()),
    })
}

/// Split a `[statements]`-style section into per-entry blocks (`Name = { ... }`).
fn entry_blocks(lines: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in lines {
        let t = line.trim();
        if let Some(name) = t.strip_suffix("= {") {
            out.push((name.trim().to_string(), vec![line.clone()]));
        } else if let Some(b) = out.last_mut() {
            b.1.push(line.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Body lowering
// ---------------------------------------------------------------------------

/// A fresh qubit. QIR qubits are opaque pointers and a call to the allocator is
/// the only PIR-level way to obtain one, so every quantum operand in a fixture
/// body becomes a fresh `qir.qubit_alloc()`.
fn qubit_alloc() -> PirExpr {
    PirExpr::Call {
        name: "qir.qubit_alloc".to_string(),
        args: Vec::new(),
    }
}

/// Externs the lowered fixture bodies can call.
fn fixture_externs() -> Vec<ExternFunction> {
    vec![ExternFunction {
        name: "qir.qubit_alloc".to_string(),
        params: Vec::new(),
        return_type: None,
    }]
}

fn gate_name(callee: &str) -> Option<&'static str> {
    Some(match callee {
        "H" => "h",
        "X" => "x",
        "Y" => "y",
        "Z" => "z",
        "S" => "s",
        "T" => "t",
        "CNOT" | "CX" => "cx",
        "CZ" => "cz",
        "CCX" | "TOFFOLI" => "ccx",
        "SWAP" => "swap",
        "M" | "MZ" => "mz",
        _ => return None,
    })
}

/// Lower a statement body: `;`-separated ops, sequenced with nested `Let`s.
///
/// This is a structural lowering, not a semantic one. It exists so the fixtures
/// produce non-empty PIR that drives the LLVM and QIR builders; quantum
/// operands are always fresh qubit allocations, and a bare call whose callee is
/// not a gate becomes the gate of matching arity because neither builder can
/// emit a call to an undeclared function.
fn lower_body(body: &str) -> Result<PirExpr, String> {
    let ops: Vec<&str> = body
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if ops.is_empty() {
        return Err(format!("empty statement body {:?}", body));
    }
    let mut exprs = Vec::new();
    for op in &ops {
        exprs.push(lower_op(op)?);
    }
    let mut acc = PirExpr::BoolLit(false);
    for (i, e) in exprs.into_iter().enumerate().rev() {
        acc = PirExpr::Let {
            name: format!("_seq{}", i),
            qty: Quantity::Zero,
            mutability: Mutability::Immutable,
            value: Box::new(e),
            body: Box::new(acc),
        };
    }
    Ok(acc)
}

fn lower_op(op: &str) -> Result<PirExpr, String> {
    let t = op.trim();

    // `if NAME { ... }` -- the classical condition is modelled as a measurement
    // bound to a fresh name, which keeps the condition an `i1` (the type both
    // builders' `If` lowering needs) without depending on a binding made by a
    // different statement.
    if let Some(rest) = t.strip_prefix("if ") {
        let rest = rest.trim();
        let (cond_name, brace) = rest
            .split_once('{')
            .ok_or_else(|| format!("malformed if-statement {:?}", t))?;
        let inner = brace
            .trim_end()
            .strip_suffix('}')
            .ok_or_else(|| format!("malformed if-statement {:?}", t))?;
        let bound = format!("_cond_{}", cond_name.trim());
        return Ok(PirExpr::If {
            cond: Box::new(PirExpr::Let {
                name: bound.clone(),
                qty: Quantity::Zero,
                mutability: Mutability::Immutable,
                value: Box::new(PirExpr::QuantumOp {
                    op: "mz".to_string(),
                    args: Vec::new(),
                    qubits: vec![qubit_alloc()],
                }),
                body: Box::new(PirExpr::Var(bound)),
            }),
            then_branch: Box::new(lower_body(inner)?),
            else_branch: Box::new(PirExpr::BoolLit(false)),
        });
    }

    // `a <-> b` -- a two-element exchange.
    if t.contains("<->") {
        return Ok(PirExpr::QuantumOp {
            op: "swap".to_string(),
            args: Vec::new(),
            qubits: vec![qubit_alloc(), qubit_alloc()],
        });
    }

    // `uncompute X` -- X is its own inverse, so uncomputing a bit is X.
    if t.starts_with("uncompute ") {
        return Ok(PirExpr::QuantumOp {
            op: "x".to_string(),
            args: Vec::new(),
            qubits: vec![qubit_alloc()],
        });
    }

    // Assignments: `lhs = rhs` or `lhs += rhs`.
    if let Some((lhs, rhs, additive)) = split_assignment(t) {
        let lhs_expr = parse_expr(&lhs)?;
        let rhs_expr = lower_rhs(&rhs)?;
        let value = if additive {
            PirExpr::Binary {
                op: BinaryOp::Add,
                left: Box::new(lhs_expr),
                right: Box::new(rhs_expr),
            }
        } else {
            rhs_expr
        };
        return Ok(PirExpr::Let {
            name: base_name(&lhs).ok_or_else(|| format!("cannot bind {:?}", lhs))?,
            qty: Quantity::Zero,
            mutability: Mutability::Mut,
            value: Box::new(value),
            body: Box::new(PirExpr::BoolLit(false)),
        });
    }

    // `measure q[i]`
    if t.starts_with("measure ") {
        return Ok(measure_op());
    }

    // Space-separated gate application: `H q[1]`, `CNOT q[1], q[2]`.
    let head = t.split_whitespace().next().unwrap_or("");
    if let Some(qop) = gate_name(head) {
        let arity = t[head.len()..].split(',').count().max(1);
        return Ok(PirExpr::QuantumOp {
            op: qop.to_string(),
            args: Vec::new(),
            qubits: (0..arity).map(|_| qubit_alloc()).collect(),
        });
    }

    // Bare call: `butterfly(a, b, c)`, `majority(a, b, c)`.
    if t.contains('(') {
        let arity = t.split(',').count().max(1);
        let qop = match arity {
            1 => "mz",
            2 => "cx",
            _ => "ccx",
        };
        return Ok(PirExpr::QuantumOp {
            op: qop.to_string(),
            args: Vec::new(),
            qubits: (0..arity).map(|_| qubit_alloc()).collect(),
        });
    }

    parse_expr(t)
}

fn measure_op() -> PirExpr {
    PirExpr::QuantumOp {
        op: "mz".to_string(),
        args: Vec::new(),
        qubits: vec![qubit_alloc()],
    }
}

fn lower_rhs(rhs: &str) -> Result<PirExpr, String> {
    let t = rhs.trim();
    if t.starts_with("measure ") {
        return Ok(measure_op());
    }
    lower_op(t)
}

/// Split `lhs = rhs` / `lhs += rhs`, respecting bracket nesting.
fn split_assignment(t: &str) -> Option<(String, String, bool)> {
    let chars: Vec<char> = t.chars().collect();
    let mut depth = 0i32;
    for i in 0..chars.len() {
        match chars[i] {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            '=' if depth == 0 => {
                let prev = if i > 0 { chars[i - 1] } else { ' ' };
                if "=!<>-".contains(prev) {
                    continue;
                }
                let additive = prev == '+';
                let end = if additive { i - 1 } else { i };
                let lhs: String = chars[..end].iter().collect();
                let rhs: String = chars[i + 1..].iter().collect();
                let (lhs, rhs) = (lhs.trim().to_string(), rhs.trim().to_string());
                if lhs.is_empty() || rhs.is_empty() {
                    return None;
                }
                return Some((lhs, rhs, additive));
            }
            _ => {}
        }
    }
    None
}

fn base_name(lhs: &str) -> Option<String> {
    let name: String = lhs
        .trim()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

// --- expression parser: identifiers, indexing, integer arithmetic -----------

#[derive(Debug, Clone, PartialEq)]
enum ETok {
    Ident(String),
    Int(i64),
    Punct(char),
}

fn lex_expr(s: &str) -> Result<Vec<ETok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(ETok::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let tok: String = chars[start..i].iter().collect();
            out.push(ETok::Int(tok.parse::<i64>().map_err(|e| e.to_string())?));
        } else if "[]()[],+-*/^".contains(c) {
            out.push(ETok::Punct(c));
            i += 1;
        } else {
            return Err(format!(
                "unexpected character {:?} in expression {:?}",
                c, s
            ));
        }
    }
    Ok(out)
}

struct EParser {
    toks: Vec<ETok>,
    pos: usize,
}

impl EParser {
    fn peek(&self) -> Option<&ETok> {
        self.toks.get(self.pos)
    }
    fn bump(&mut self) -> Option<ETok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&ETok::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<PirExpr, String> {
        let mut lhs = self.term()?;
        while let Some(ETok::Punct(c @ ('+' | '-' | '^'))) = self.peek() {
            let op = *c;
            self.bump();
            let rhs = self.term()?;
            lhs = PirExpr::Binary {
                op: match op {
                    '+' => BinaryOp::Add,
                    '-' => BinaryOp::Sub,
                    _ => BinaryOp::Xor,
                },
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<PirExpr, String> {
        let mut lhs = self.unary()?;
        while let Some(ETok::Punct(c @ ('*' | '/'))) = self.peek() {
            let op = *c;
            self.bump();
            let rhs = self.unary()?;
            lhs = PirExpr::Binary {
                op: if op == '*' {
                    BinaryOp::Mul
                } else {
                    BinaryOp::Div
                },
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<PirExpr, String> {
        if self.eat('-') {
            let e = self.unary()?;
            return Ok(PirExpr::Binary {
                op: BinaryOp::Sub,
                left: Box::new(PirExpr::IntLit(0)),
                right: Box::new(e),
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<PirExpr, String> {
        let mut base = self.atom()?;
        while self.eat('[') {
            let idx = self.expr()?;
            if !self.eat(']') {
                return Err("missing ']' in index expression".into());
            }
            base = PirExpr::Index {
                base: Box::new(base),
                indices: vec![idx],
            };
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<PirExpr, String> {
        match self.bump() {
            Some(ETok::Int(n)) => Ok(PirExpr::IntLit(n)),
            Some(ETok::Ident(name)) => {
                if self.eat('(') {
                    // A call in index position (`x[bitrev(i)]`) stays an opaque
                    // index of `x`; neither builder can call a function that is
                    // not declared in the module.
                    let mut depth = 1usize;
                    while let Some(t) = self.bump() {
                        match t {
                            ETok::Punct('(') => depth += 1,
                            ETok::Punct(')') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    return Ok(PirExpr::IntLit(0));
                }
                Ok(PirExpr::Var(name))
            }
            Some(ETok::Punct('(')) => {
                let e = self.expr()?;
                if !self.eat(')') {
                    return Err("missing ')' in expression".into());
                }
                Ok(e)
            }
            other => Err(format!("unexpected token {:?} in expression", other)),
        }
    }
}

fn parse_expr(s: &str) -> Result<PirExpr, String> {
    let toks = lex_expr(s)?;
    if toks.is_empty() {
        return Err(format!("empty expression {:?}", s));
    }
    let mut p = EParser { toks, pos: 0 };
    let e = p.expr()?;
    if p.pos != p.toks.len() {
        return Err(format!("trailing tokens in expression {:?}", s));
    }
    Ok(e)
}

// ---------------------------------------------------------------------------
// parse_pir
// ---------------------------------------------------------------------------

/// Parse a `.pir` fixture into a `PirModule`.
fn parse_pir(content: &str) -> Result<PirModule, String> {
    let secs = sections(content);
    let section = |name: &str| -> Option<&Vec<String>> {
        secs.iter().find(|(h, _)| h == name).map(|(_, l)| l)
    };

    // [parameters] -- symbolic constant names.
    let mut parameters = Vec::new();
    for line in section("parameters").into_iter().flatten() {
        if let Some((k, _)) = kv(line) {
            parameters.push(k);
        }
    }

    // [domain NAME] blocks.
    let mut domains: Vec<(String, AffineDomain)> = Vec::new();
    for (header, lines) in &secs {
        if let Some(name) = header.strip_prefix("domain ") {
            domains.push((name.trim().to_string(), parse_domain(name.trim(), lines)?));
        }
    }
    let lookup_domain = |name: &str| -> Result<AffineDomain, String> {
        domains
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| format!("unknown domain {:?}", name))
    };

    // [quantities]
    let mut quantities: QuantityMap = HashMap::new();
    for line in section("quantities").into_iter().flatten() {
        if let Some((k, v)) = kv(line) {
            quantities.insert(k, parse_quantity(&v)?);
        }
    }

    // [statements] -- ids are assigned in file order.
    let blocks = entry_blocks(section("statements").map(|v| v.as_slice()).unwrap_or(&[]));
    if blocks.is_empty() {
        return Err("no [statements] entries found: not a PIR fixture".to_string());
    }
    let mut statements = Vec::new();
    for (i, (name, lines)) in blocks.iter().enumerate() {
        let get = |key: &str| -> Option<String> {
            lines.iter().skip(1).find_map(|l| match kv(l) {
                Some((k, v)) if k == key => Some(v),
                _ => None,
            })
        };
        let domain = lookup_domain(
            &get("domain").ok_or_else(|| format!("statement {} missing domain", name))?,
        )?;
        let body_text = get("body").ok_or_else(|| format!("statement {} missing body", name))?;
        let quantity = parse_quantity(
            &get("quantity").ok_or_else(|| format!("statement {} missing quantity", name))?,
        )?;
        let mutability = parse_mutability(
            &get("mutability").ok_or_else(|| format!("statement {} missing mutability", name))?,
        )?;
        statements.push(PirStatement {
            id: StmtId(i),
            domain,
            body: lower_body(&body_text)?,
            quantity,
            mutability,
            span: None,
        });
    }

    // Neither module builder reads the schedule tree or the access relations,
    // so they are left empty rather than guessed at; the statement domains
    // above are what codegen actually consumes.
    let schedule = ScheduleTree::new(
        ScheduleNode::Sequence {
            children: statements
                .iter()
                .map(|s| ScheduleNode::domain(s.id, s.domain.clone()))
                .collect(),
        },
        Vec::new(),
    );

    let mut module = PirModule::new(
        statements,
        schedule,
        AccessRelations::new(),
        quantities,
        parameters,
    );
    module.extern_functions = fixture_externs();
    Ok(module)
}
// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Generated-IR assertion helper.
///
/// Two checks are performed, and they answer different questions:
///
/// 1. **Authoritative**: the IR is re-parsed by LLVM and then accepted by
///    LLVM's own verifier (`Module::verify`). If this passes, the IR is
///    genuinely well-formed. `llvm-as` agrees with this result.
/// 2. **Structural report**: `BitcodeValidator::validate_module` also runs so
///    that the report is exercised, but its `errors` are *not* asserted to be
///    empty, because the validator has two known defects that fire on valid
///    IR (see `KNOWN_VALIDATOR_DEFECTS`). Asserting they are absent would be
///    asserting a bug; asserting they are exactly these two is the honest
///    check, and it still fails if any *new* kind of error appears.
#[cfg(feature = "llvm")]
mod ir_checks {
    use inkwell::context::Context;
    use inkwell::memory_buffer::MemoryBuffer;
    use inkwell::module::Module;
    use inkwell::targets::Target;
    use naso_compiler::codegen::validate::{BitcodeValidator, ValidationError, ValidationReport};

    /// Prefixes of `ValidationError::check` values that are known false
    /// positives emitted against valid LLVM IR:
    ///
    /// * `"Function signature: "` -- `verify_function_signature` treats
    ///   `fn_type.get_return_type().is_none()` as "no return type", but
    ///   inkwell returns `None` for `void` returns. Every generated function
    ///   is `void`, so every one is flagged.
    /// * `"Call signature: "` -- `verify_types` recovers call arguments via
    ///   `inst.get_operand(i)`, which is not reliable for LLVM 17 call sites
    ///   (operand layout is not `callee, args...`). It mis-reports valid
    ///   `call void @qir.h(i64 0)` against `declare void @qir.h(i64)`.
    pub const KNOWN_VALIDATOR_DEFECTS: [&str; 2] = ["Function signature: ", "Call signature: "];

    pub fn is_known_validator_defect(e: &ValidationError) -> bool {
        KNOWN_VALIDATOR_DEFECTS
            .iter()
            .any(|p| e.check.starts_with(p))
    }

    /// Parse `ir` with LLVM, assert LLVM's verifier accepts it, run
    /// `BitcodeValidator`, and assert every reported error is a known defect.
    pub fn check_ir(ir: &str, label: &str) -> ValidationReport {
        Target::initialize_all(&inkwell::targets::InitializationConfig::default());

        let context = Context::create();
        // Use the *copy* constructor: `create_from_memory_range` borrows the
        // slice and (with `RequiresNullTerminator = false`) fails
        // nondeterministically on the generated IR. Copying is also what
        // keeps the buffer valid independently of the `ir` String's lifetime.
        let buffer = MemoryBuffer::create_from_memory_range_copy(ir.as_bytes(), label);
        let module: Module = context
            .create_module_from_ir(buffer)
            .unwrap_or_else(|e| panic!("LLVM could not parse {}: {}", label, e));

        module.verify().unwrap_or_else(|e| {
            panic!(
                "LLVM's own verifier rejected {}: {}\n--- IR ---\n{}",
                label, e, ir
            )
        });

        let report = BitcodeValidator::new(&context)
            .validate_module(&module)
            .unwrap_or_else(|e| panic!("BitcodeValidator failed on {}: {}", label, e));

        let unexpected: Vec<&str> = report
            .errors
            .iter()
            .filter(|e| !is_known_validator_defect(e))
            .map(|e| e.check.as_str())
            .collect();
        assert!(
            unexpected.is_empty(),
            "{}: unexpected validator errors {:?} (known defects: {:?})",
            label,
            unexpected,
            report
                .errors
                .iter()
                .filter(|e| is_known_validator_defect(e))
                .map(|e| e.check.as_str())
                .collect::<Vec<_>>()
        );

        report
    }

    /// `BitcodeValidator`'s QIR path only looks for `__quantum__*` substrings,
    /// which the builder does not emit (it emits `qir.*`), so it reports no
    /// checks at all. Assert the QIR is non-empty and uses `qir.*` naming.
    pub fn check_qir(qir: &str, label: &str) {
        assert!(!qir.trim().is_empty(), "{} produced empty QIR", label);
        assert!(
            qir.contains("!qir.profile") || qir.contains("qir.qubit_alloc"),
            "{} QIR has no recognizable quantum runtime content:\n{}",
            label,
            qir
        );
        let report = BitcodeValidator::new(&Context::create())
            .validate_qir_module(qir)
            .unwrap_or_else(|e| panic!("QIR validation failed for {}: {}", label, e));
        println!("{} QIR validation: {}", label, report.summary());
    }
}

// ---------------------------------------------------------------------------
// LLVM codegen tests
// ---------------------------------------------------------------------------

#[cfg(feature = "llvm")]
mod llvm_codegen_tests {
    use super::ir_checks::check_ir;
    use super::*;
    use inkwell::context::Context;
    use inkwell::memory_buffer::MemoryBuffer;
    use inkwell::values::CallSiteValue;
    use naso_compiler::codegen::context::CodegenContext;
    use naso_compiler::codegen::{CodegenPipeline, CodegenTarget, OptLevel};

    fn pipeline(opt: OptLevel) -> CodegenPipeline {
        CodegenPipeline::new(
            CodegenContext::new(CodegenTarget::Host, opt)
                .expect("Failed to create codegen context"),
        )
    }

    #[test]
    fn test_matmul_64x64_llvm_codegen() {
        let pir = load_pir_fixture("matmul_64x64");
        assert_eq!(pir.statements.len(), 1, "matmul fixture has one statement");
        assert_eq!(pir.statements[0].id, StmtId(0));

        let llvm_ir = pipeline(OptLevel::Default)
            .emit_llvm(&pir)
            .expect("LLVM codegen failed");

        // The LLVM module builder emits one `void` function per statement.
        assert!(
            llvm_ir.contains("define void @stmt_0()"),
            "expected a stmt_0 function, got:\n{}",
            llvm_ir
        );
        // The fixture's parameters are parsed even though the LLVM module
        // builder does not currently emit them as function parameters.
        assert_eq!(pir.parameters, vec!["N", "M", "K"]);

        let report = check_ir(&llvm_ir, "matmul_64x64");
        println!("Matmul validation: {}", report.summary());
    }

    #[test]
    fn test_stencil_3d_llvm_codegen() {
        let pir = load_pir_fixture("stencil_3d");
        assert_eq!(pir.statements.len(), 1);
        assert_eq!(pir.statements[0].domain.dims, 3);

        let llvm_ir = pipeline(OptLevel::Default)
            .emit_llvm(&pir)
            .expect("LLVM codegen failed");
        assert!(
            llvm_ir.contains("define void @stmt_0()"),
            "expected a stmt_0 function, got:\n{}",
            llvm_ir
        );

        let report = check_ir(&llvm_ir, "stencil_3d");
        println!("Stencil validation: {}", report.summary());
    }

    #[test]
    fn test_fft_1024_llvm_codegen() {
        let pir = load_pir_fixture("fft_1024");
        assert_eq!(pir.statements.len(), 2, "fft fixture has two statements");

        let llvm_ir = pipeline(OptLevel::Default)
            .emit_llvm(&pir)
            .expect("LLVM codegen failed");

        // One function per statement, in fixture order.
        assert!(
            llvm_ir.contains("@stmt_0()"),
            "missing stmt_0:\n{}",
            llvm_ir
        );
        assert!(
            llvm_ir.contains("@stmt_1()"),
            "missing stmt_1:\n{}",
            llvm_ir
        );

        let report = check_ir(&llvm_ir, "fft_1024");
        println!("FFT validation: {}", report.summary());
    }

    #[test]
    fn test_matmul_64x64_golden_fixture() {
        let pir = load_pir_fixture("matmul_64x64");
        let llvm_ir = pipeline(OptLevel::Default)
            .emit_llvm(&pir)
            .expect("LLVM codegen failed");

        // The golden fixture is a hand-written reference for the kernel shape:
        // it is a *specification* of what the builder should eventually emit.
        let golden = load_llvm_fixture("matmul_64x64");
        assert!(
            golden.contains("define void @matmul_kernel"),
            "golden LLVM fixture should define matmul_kernel"
        );
        // i64 trip counters sized 64x64 in the reference.
        // `i64 64, i64 64, i64 64` bounds: a 64x64x64 problem over i64.
        assert_eq!(
            golden.matches("i64 64").count(),
            3,
            "golden kernel should take three i64 extents"
        );
        // The reference kernel is written in doubles.
        assert!(
            golden.contains("fmul double"),
            "golden fixture should contain the inner-product multiply"
        );
        assert!(
            golden.contains("fadd double"),
            "golden fixture should accumulate with fadd"
        );
        // Result C is returned by loading and storing through a pointer.
        assert!(
            golden.contains("ptr noalias %C"),
            "golden kernel should take C as its output"
        );

        // Structural verification compares the *parsed PIR* against the emitted
        // IR. It runs and produces a real report; a full match is not yet
        // expected because the builder emits `stmt_N` stubs rather than the
        // loop nest / per-array globals the golden fixture describes.
        let report =
            StructuralVerifier::verify_pir_to_llvm(&load_pir_text("matmul_64x64"), &llvm_ir)
                .expect("Structural verification failed");
        println!("Matmul structural: {}", report.summary());
        assert!(
            !report.all_matched(),
            "structural verifier unexpectedly matched every element"
        );
        // The verifier checks every statement, every array, and the loop
        // structure. The fixture has 1 statement (S0) plus array accesses
        // (A, B, C) and a Band schedule, so all of them must be accounted for
        // as either matched or mismatched -- none may be silently dropped.
        let checked = report.matched_elements.len() + report.mismatched_elements.len();
        assert!(
            checked >= 5,
            "structural verifier only checked {} elements, expected statements + arrays + loops",
            checked
        );
        // The statement itself is specifically reported as missing its function,
        // because the builder names functions `stmt_0`, not `fn_s0`.
        // `parse_pir_structure` reads every `NAME = {` line that starts with
        // `S` as a "statement", so the `[accesses]` entries (S0_read_A, ...)
        // are counted alongside the real statement S0. Each of them lacks a
        // matching function, because the builder emits one function per
        // statement named `stmt_N`.
        for expected in [
            "Missing function for statement: S0",
            "Missing function for statement: S0_read_A",
        ] {
            assert!(
                report.mismatched_elements.iter().any(|m| m == expected),
                "expected {:?} in {:?}",
                expected,
                report.mismatched_elements
            );
        }
        // Arrays A and B are read by the kernel; the builder does not yet emit
        // per-array globals.
        assert!(
            report
                .mismatched_elements
                .iter()
                .any(|m| m == "Missing allocation for array: A")
        );
    }

    #[test]
    fn test_bitcode_validator_on_valid_module() {
        let context = Context::create();
        let module = context.create_module("test_module");

        let i32_type = context.i32_type();
        let fn_type = i32_type.fn_type(&[], false);
        let function = module.add_function("test_fn", fn_type, None);
        let entry = context.append_basic_block(function, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);
        builder
            .build_return(Some(&i32_type.const_int(42, false)))
            .unwrap();

        let report = BitcodeValidator::new(&context)
            .validate_module(&module)
            .expect("Validation failed");

        // A non-void, properly terminated function produces no errors at all.
        assert!(!report.has_errors(), "Errors: {:?}", report.errors);
        assert!(
            report
                .passed_checks
                .iter()
                .any(|c| c.contains("Module integrity"))
        );
        assert!(
            report
                .passed_checks
                .iter()
                .any(|c| c.contains("Function body present"))
        );
        assert!(
            report
                .passed_checks
                .iter()
                .any(|c| c.contains("Function signature valid: test_fn"))
        );
    }

    #[test]
    fn test_bitcode_validator_catches_missing_terminator() {
        let context = Context::create();
        let module = context.create_module("test_module");

        let i32_type = context.i32_type();
        let fn_type = i32_type.fn_type(&[], false);
        let function = module.add_function("bad_fn", fn_type, None);
        // Intentionally NOT adding a terminator.
        let _entry = context.append_basic_block(function, "entry");

        let report = BitcodeValidator::new(&context)
            .validate_module(&module)
            .expect("Validation failed");

        assert!(report.has_errors());
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.check.contains("Block terminator"))
        );
    }

    /// A WELL-TYPED call must not produce a "Call signature" error.
    ///
    /// `verify_types` used to treat operand 0 as the callee and operands `1..n` as the
    /// arguments. Under LLVM 17 the layout is the opposite -- the callee is the LAST
    /// operand -- so a correct `call i32 @callee(i32 3)` had its `[ptr]` callee type
    /// compared against the declared `[i32]`, and every well-typed call was reported as
    /// a signature mismatch.
    ///
    /// Measured on LLVM 17 for `%r = call i32 @callee(i32 3)`: operand 0 is `i32`,
    /// operand 1 is `ptr`. See the comment in `verify_types`.
    #[test]
    fn a_well_typed_call_is_not_reported_as_a_signature_mismatch() {
        let context = Context::create();
        let module = context.create_module("calls");

        let i32t = context.i32_type();
        let callee = module.add_function("callee", i32t.fn_type(&[i32t.into()], false), None);

        let caller = module.add_function("caller", i32t.fn_type(&[], false), None);
        let entry = context.append_basic_block(caller, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);
        let arg = i32t.const_int(3, false);
        let call = builder.build_call(callee, &[arg.into()], "r").unwrap();
        // inkwell 0.10's `build_return` takes `&dyn BasicValue`, and a `CallSiteValue`
        // is not one; recover the `BasicValueEnum` from the instruction.
        let returned = call
            .try_as_basic_value()
            .basic()
            .expect("the call yields a value");
        builder.build_return(Some(&returned)).unwrap();

        // Ground truth: LLVM accepts this module.
        module.verify().expect("LLVM must accept a well-typed call");

        let report = BitcodeValidator::new(&context)
            .validate_module(&module)
            .expect("Validation failed");

        assert!(
            !report
                .errors
                .iter()
                .any(|e| e.check.contains("Call signature")),
            "a well-typed call must not be a signature mismatch. Errors: {:?}",
            report.errors
        );
        assert!(
            report
                .warnings
                .iter()
                .all(|w| !w.check.contains("Call operands")),
            "the argument count must agree: {:?}",
            report.warnings
        );
        assert!(
            report
                .passed_checks
                .iter()
                .any(|c| c.contains("Call signature match: callee")),
            "the call should be recognised as matching. passed: {:?}",
            report.passed_checks
        );
    }

    /// A call whose argument type does NOT match must still be reported.
    ///
    /// The counterpart to the test above: fixing the operand order must not have
    /// turned the check into one that always passes.
    #[test]
    fn a_mismatched_call_argument_is_still_reported() {
        let context = Context::create();
        let module = context.create_module("bad_calls");

        let i32t = context.i32_type();
        let f32t = context.f32_type();
        let voidt = context.void_type();

        // The callee takes an f32...
        let callee = module.add_function("callee", f32t.fn_type(&[f32t.into()], false), None);
        let caller = module.add_function("caller", voidt.fn_type(&[], false), None);
        let entry = context.append_basic_block(caller, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);

        // ...but we pass an i32.
        let bad = i32t.const_int(3, false);
        let _call = builder.build_call(callee, &[bad.into()], "r").unwrap();
        builder.build_return(None).unwrap();

        // LLVM itself rejects this module, which is what makes it the right input: a
        // test the real verifier passes would not be testing a mismatch at all.
        assert!(
            module.verify().is_err(),
            "the test premise is wrong: LLVM accepts this call, so it is not a mismatch"
        );

        let report = BitcodeValidator::new(&context)
            .validate_module(&module)
            .expect("Validation failed");
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.check.contains("Call signature")),
            "a mismatched argument type must be reported. Errors: {:?}",
            report.errors
        );
    }

    /// A `void` function is valid IR and must NOT be reported as malformed.
    ///
    /// INVERTED. This test previously asserted the opposite -- that the validator
    /// flags `void_fn` with "Function signature: void_fn / no return type". That
    /// pinned a real defect: `FunctionType::get_return_type()` returns `None` to mean
    /// VOID, and the check read `None` as "no return type". Every void function was
    /// an error, including the ones this compiler emits.
    ///
    /// `module.verify()` below is the authority: LLVM's own verifier accepts the
    /// module, so if the validator disagrees it is the validator that is wrong.
    #[test]
    fn a_void_function_is_not_reported_as_missing_a_return_type() {
        let context = Context::create();
        let module = context.create_module("test_module");

        let void_type = context.void_type();
        let fn_type = void_type.fn_type(&[], false);
        let function = module.add_function("void_fn", fn_type, None);
        let entry = context.append_basic_block(function, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);
        builder.build_return(None).unwrap();

        // The ground truth: LLVM itself accepts this.
        module.verify().expect("LLVM must accept a void function");

        let report = BitcodeValidator::new(&context)
            .validate_module(&module)
            .expect("Validation failed");

        assert!(
            !report
                .errors
                .iter()
                .any(|e| e.check == "Function signature: void_fn"),
            "a void return must not be an error; LLVM accepts this module. Errors: {:?}",
            report.errors
        );
        assert!(
            report
                .passed_checks
                .iter()
                .any(|c| c.contains("void_fn") && c.contains("void")),
            "the signature check should record that void_fn returns void: {:?}",
            report.passed_checks
        );
    }

    #[test]
    fn test_codegen_opt_levels() {
        let pir = load_pir_fixture("matmul_64x64");

        for opt_level in [OptLevel::None, OptLevel::Default, OptLevel::Aggressive] {
            let llvm_ir = pipeline(opt_level)
                .emit_llvm(&pir)
                .expect("LLVM codegen failed");

            let report = check_ir(&llvm_ir, &format!("matmul_{:?}", opt_level));
            println!("{:?} validation: {}", opt_level, report.summary());
        }
    }

    #[test]
    fn test_codegen_pipeline_end_to_end() {
        for fixture in [
            "matmul_64x64",
            "stencil_3d",
            "fft_1024",
            "teleport",
            "rev_adder",
        ] {
            let pir = load_pir_fixture(fixture);
            let p = pipeline(OptLevel::Default);

            let llvm_ir = p
                .emit_llvm(&pir)
                .unwrap_or_else(|e| panic!("LLVM codegen failed for {}: {}", fixture, e));
            let report = check_ir(&llvm_ir, fixture);
            println!("{} validation: {}", fixture, report.summary());

            if fixture == "teleport" || fixture == "rev_adder" {
                let qir = p
                    .emit_qir(&pir)
                    .unwrap_or_else(|e| panic!("QIR codegen failed for {}: {}", fixture, e));
                super::ir_checks::check_qir(&qir, fixture);
            }
        }
    }

    /// Every gate named in a fixture body must appear as a call in the
    /// generated IR -- this is what ties the fixture text to the emitted code.
    #[test]
    fn test_fixture_gates_appear_in_generated_ir() {
        let cases: [(&str, &[&str]); 2] = [
            (
                "teleport",
                &["@qir.h(", "@qir.cx(", "@qir.mz(", "@qir.x(", "@qir.z("],
            ),
            ("rev_adder", &["@qir.ccx(", "@qir.x("]),
        ];
        for (fixture, gates) in cases {
            let ir = pipeline(OptLevel::Default)
                .emit_llvm(&load_pir_fixture(fixture))
                .expect("LLVM codegen failed");
            for gate in gates {
                assert!(
                    ir.contains(gate),
                    "{}: expected a call to {}, got:\n{}",
                    fixture,
                    gate,
                    ir
                );
            }
            check_ir(&ir, fixture);
        }
    }

    /// The call sites the builder emits really do target the functions it
    /// declares, checked directly rather than via the buggy `verify_types`.
    #[test]
    fn test_call_sites_match_declarations() {
        let context = Context::create();
        for fixture in ["teleport", "rev_adder", "stencil_3d", "fft_1024"] {
            let ir = pipeline(OptLevel::Default)
                .emit_llvm(&load_pir_fixture(fixture))
                .expect("LLVM codegen failed");
            let buffer = MemoryBuffer::create_from_memory_range_copy(ir.as_bytes(), fixture);
            let module = context
                .create_module_from_ir(buffer)
                .unwrap_or_else(|e| panic!("parse failed for {}: {}", fixture, e));

            for function in module.get_functions() {
                for block in function.get_basic_blocks() {
                    for inst in block.get_instructions() {
                        let call = match CallSiteValue::try_from(inst) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        let callee = match call.get_called_fn_value() {
                            Some(c) => c,
                            None => continue,
                        };
                        let decl_params = callee.get_type().get_param_types().len();
                        assert_eq!(
                            call.count_arguments() as usize,
                            decl_params,
                            "{}: call to {} passes {} args but declares {}",
                            fixture,
                            callee.get_name().to_string_lossy(),
                            call.count_arguments(),
                            decl_params
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// QIR codegen tests
// ---------------------------------------------------------------------------

#[cfg(feature = "llvm")]
mod qir_codegen_tests {
    use super::ir_checks::check_qir;
    use super::*;
    use naso_compiler::codegen::context::CodegenContext;
    use naso_compiler::codegen::{CodegenPipeline, CodegenTarget, OptLevel};

    fn pipeline() -> CodegenPipeline {
        CodegenPipeline::new(
            CodegenContext::new(CodegenTarget::Host, OptLevel::Default)
                .expect("Failed to create codegen context"),
        )
    }

    #[test]
    fn test_teleport_qir_codegen() {
        let pir = load_pir_fixture("teleport");
        assert_eq!(pir.statements.len(), 4, "teleport has four statements");

        let qir = pipeline().emit_qir(&pir).expect("QIR codegen failed");

        // The QIR builder emits `qir.`-prefixed intrinsics and one function
        // per statement, named `qir_stmt_<id>`.
        assert!(
            qir.contains("call void @qir.h("),
            "missing H gate:\n{}",
            qir
        );
        assert!(qir.contains("call void @qir.cx("), "missing CNOT:\n{}", qir);
        assert!(
            qir.contains("call ptr @qir.qubit_alloc()"),
            "missing qubit allocation:\n{}",
            qir
        );
        for i in 0..4 {
            assert!(
                qir.contains(&format!("define void @qir_stmt_{}()", i)),
                "missing qir_stmt_{}:\n{}",
                i,
                qir
            );
        }
        check_qir(&qir, "teleport");
    }

    #[test]
    fn test_rev_adder_qir_codegen() {
        let pir = load_pir_fixture("rev_adder");
        assert_eq!(pir.statements.len(), 2, "rev_adder has two statements");

        let qir = pipeline().emit_qir(&pir).expect("QIR codegen failed");

        // `majority(a, b, c)` lowers to the three-qubit Toffoli.
        assert!(
            qir.contains("call void @qir.ccx("),
            "missing Toffoli:\n{}",
            qir
        );
        assert!(qir.contains("define void @qir_stmt_0()"));
        assert!(qir.contains("define void @qir_stmt_1()"));
        check_qir(&qir, "rev_adder");
    }

    #[test]
    fn test_teleport_golden_fixture() {
        let pir = load_pir_fixture("teleport");
        let qir = pipeline().emit_qir(&pir).expect("QIR codegen failed");

        // The golden fixture is the hand-written reference using the
        // `__quantum__*` runtime naming, which the builder does not emit.
        let golden = load_qir_fixture("teleport");
        assert!(
            golden.contains("__quantum__rt__initialize"),
            "golden QIR fixture should declare the runtime initializer"
        );
        assert!(golden.contains("__quantum__qis__cnot"));

        let report = StructuralVerifier::verify_pir_to_qir(&load_pir_text("teleport"), &qir)
            .expect("Structural verification failed");
        println!("Teleport QIR structural: {}", report.summary());
        assert!(
            !report.all_matched(),
            "structural verifier unexpectedly matched every element"
        );
        // All four teleport statements are checked.
        assert_eq!(
            report.matched_elements.len() + report.mismatched_elements.len(),
            4
        );
    }

    #[test]
    fn test_qir_contains_quantum_intrinsics() {
        let p = pipeline();
        for fixture in ["teleport", "rev_adder"] {
            let qir = p
                .emit_qir(&load_pir_fixture(fixture))
                .unwrap_or_else(|e| panic!("QIR codegen failed for {}: {}", fixture, e));

            // Quantum runtime: allocation is emitted as a real call.
            assert!(
                qir.contains("call ptr @qir.qubit_alloc()"),
                "{} missing qubit allocation call",
                fixture
            );
            // Gates.
            assert!(
                qir.contains("call void @qir."),
                "{} missing quantum gate calls",
                fixture
            );
            // Metadata identifying the QIR profile.
            assert!(
                qir.contains("!qir.profile"),
                "{} missing qir.profile metadata",
                fixture
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline / report tests that do not need LLVM
// ---------------------------------------------------------------------------

/// `CodegenPipeline::new(())` and the "LLVM backend not enabled" errors only
/// exist in the non-LLVM build, so this module is gated on `not(llvm)`.
#[cfg(not(feature = "llvm"))]
mod no_llvm_tests {
    use super::*;
    use naso_compiler::codegen::CodegenPipeline;

    #[test]
    fn test_codegen_pipeline_creation_without_llvm() {
        let pipeline = CodegenPipeline::new(());
        let pir = load_pir_fixture("matmul_64x64");

        let err = pipeline
            .emit_llvm(&pir)
            .expect_err("emit_llvm must fail without the llvm feature")
            .to_string();
        assert!(
            err.contains("LLVM backend not enabled"),
            "unexpected error: {}",
            err
        );

        let err = pipeline
            .emit_qir(&pir)
            .expect_err("emit_qir must fail without the llvm feature")
            .to_string();
        assert!(
            err.contains("QIR backend requires LLVM"),
            "unexpected error: {}",
            err
        );
    }
}

/// Report bookkeeping, which is independent of the LLVM feature.
mod report_tests {
    use super::*;

    #[test]
    fn test_validation_report_creation() {
        let mut report = ValidationReport::new();
        report.passed_checks.push("test".to_string());

        assert!(!report.has_errors());
        assert!(!report.has_warnings());
        assert_eq!(report.passed_checks.len(), 1);
        assert!(report.summary().contains("1"));
    }

    #[test]
    fn test_structural_report() {
        let mut report = StructuralReport::new();
        assert!(report.all_matched());

        report.matched_elements.push("test".to_string());
        assert!(report.all_matched());
        assert_eq!(report.matched_elements.len(), 1);

        report.mismatched_elements.push("bad".to_string());
        assert!(!report.all_matched());
    }
}

/// The fixture parser itself is worth testing directly.
mod parser_tests {
    use super::*;

    #[test]
    fn test_parse_pir_matmul() {
        let pir = parse_pir(&load_pir_text("matmul_64x64")).unwrap();
        assert_eq!(pir.statements.len(), 1);
        assert_eq!(pir.statements[0].id, StmtId(0));
        assert_eq!(pir.parameters, vec!["N", "M", "K"]);
        assert_eq!(pir.statements[0].domain.dims, 3);
        assert_eq!(pir.statements[0].domain.n_iter, 3);
        assert_eq!(pir.statements[0].domain.constraints.len(), 6);
        assert_eq!(pir.statements[0].quantity, Quantity::Many);
        assert_eq!(pir.statements[0].mutability, Mutability::Immutable);
    }

    #[test]
    fn test_parse_pir_teleport() {
        let pir = parse_pir(&load_pir_text("teleport")).unwrap();
        assert_eq!(pir.statements.len(), 4);
        assert_eq!(pir.parameters, vec!["N"]);
        assert_eq!(pir.quantities.get("q[0]"), Some(&Quantity::One));
        assert_eq!(pir.quantities.get("b0"), Some(&Quantity::Zero));
        assert_eq!(pir.quantities.get("b1"), Some(&Quantity::Zero));
        // Statement ids are assigned in fixture order.
        let ids: Vec<usize> = pir.statements.iter().map(|s| s.id.0).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_parse_pir_rev_adder() {
        let pir = parse_pir(&load_pir_text("rev_adder")).unwrap();
        assert_eq!(pir.statements.len(), 2);
        assert_eq!(pir.statements[0].domain.dims, 1);
        assert_eq!(
            pir.statements[0].mutability,
            Mutability::Mut,
            "fixture says Mutable, which parses to Mut"
        );
    }

    #[test]
    fn test_parse_pir_fft_two_statements() {
        let pir = parse_pir(&load_pir_text("fft_1024")).unwrap();
        assert_eq!(pir.statements.len(), 2);
        assert_eq!(pir.parameters, vec!["N", "LOGN"]);
    }

    /// The fixtures write upper bounds as `[-1, 0, 0, 63]` meaning `i <= 63`.
    /// That is an inequality on the negated coefficients with a negated bound,
    /// so the parser must produce `-i >= -63`.
    #[test]
    fn test_domain_constraint_sign_convention() {
        let pir = parse_pir(&load_pir_text("matmul_64x64")).unwrap();
        let cs = &pir.statements[0].domain.constraints;
        assert_eq!(cs.len(), 6);
        // `[1, 0, 0, 0]` -> i >= 0
        assert_eq!(cs[0].coefficients, vec![1, 0, 0]);
        assert_eq!(cs[0].constant, 0);
        // `[-1, 0, 0, 63]` -> i <= 63
        assert_eq!(cs[1].coefficients, vec![-1, 0, 0]);
        assert_eq!(cs[1].constant, -63);
        // `-i >= -63` is `i <= 63`: 63 is inside, 64 is outside.
        assert!(cs[1].contains(&[63, 0, 0]), "i = 63 should be in bounds");
        assert!(
            !cs[1].contains(&[64, 0, 0]),
            "i = 64 should be out of bounds"
        );
        // `contains` tests one constraint in isolation, so this upper bound
        // still admits negative values; the lower bound cs[0] is what rejects
        // them. Together the two give exactly 0 <= i <= 63.
        assert!(cs[1].contains(&[-1, 0, 0]), "upper bound alone admits -1");
        // The lower bound `[1, 0, 0, 0]` is `i >= 0`.
        assert!(cs[0].contains(&[0, 0, 0]), "i = 0 should be in bounds");
        assert!(
            !cs[0].contains(&[-1, 0, 0]),
            "i = -1 should be out of bounds"
        );
    }

    #[test]
    fn test_parse_pir_rejects_garbage() {
        assert!(parse_pir("not a pir fixture at all").is_err());
        assert!(parse_pir("[statements]\nnot a block").is_err());
    }
}
