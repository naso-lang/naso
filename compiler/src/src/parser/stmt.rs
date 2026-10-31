//! Statement and block parsing for the Naso parser.

use crate::ast::*;
use crate::lexer::TokenKind as TK;
use crate::parser::{Parser, next_id};
use crate::typecheck::debug_log;

use super::expr::is_control_flow_stmt;

impl<'a> Parser<'a> {
    // ===== Blocks =====

    /// Parse a `{ ... }` block.
    pub fn parse_block(&mut self) -> Block {
        let start = self.pos;
        self.expect(TK::LBrace);
        let (stmts, tail) = self.parse_stmt_list();
        self.expect(TK::RBrace);
        Block::new(stmts, tail, self.span_from(start))
    }

    /// Parse a sequence of statements terminated by `}` or EOF.
    ///
    /// Returns the statements plus an optional trailing expression (a
    /// statement without a closing `;` is the block's tail expression, unless
    /// it is a control-flow block construct that reads naturally without one).
    fn parse_stmt_list(&mut self) -> (Vec<Stmt>, Option<Expr>) {
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.loop_should_stop() {
            debug_log(&format!("parse_stmt_list: peek={:?}", self.peek()));
            match self.peek() {
                Some(TK::RBrace) | None => break,
                Some(TK::Semicolon) => {
                    self.bump();
                    let span = Span::default();
                    stmts.push(Stmt::new(StmtKind::Empty, span, next_id()));
                }
                Some(TK::Let) => {
                    debug_log("parse_stmt_list: dispatching to parse_let_stmt");
                    stmts.push(self.parse_let_stmt());
                }
                Some(TK::Reversible) => {
                    let start = self.pos;
                    let rb = self.parse_reversible_block();
                    self.eat(TK::Semicolon);
                    let span = self.span_from(start);
                    stmts.push(Stmt::new(StmtKind::Reversible(rb), span, next_id()))
                }
                Some(TK::Proof) => {
                    let start = self.pos;
                    let pb = self.parse_proof_block();
                    self.eat(TK::Semicolon);
                    let span = self.span_from(start);
                    stmts.push(Stmt::new(StmtKind::Proof(pb), span, next_id()))
                }
                // Statement-position `forall` is a loop. In expression
                // position it is a proposition (parse_quantified), so the two
                // forms are distinguished by where the keyword appears.
                Some(TK::Forall) => {
                    let expr = self.parse_forall();
                    self.eat(TK::Semicolon);
                    let span = expr.span;
                    stmts.push(Stmt::new(StmtKind::Expr(expr), span, next_id()))
                }
                _ => {
                    debug_log("parse_stmt_list: dispatching to parse_expr");
                    let expr = self.parse_expr();
                    if self.at(TK::Semicolon) {
                        self.bump();
                        let span = expr.span;
                        stmts.push(Stmt::new(StmtKind::Expr(expr), span, next_id()))
                    } else if is_control_flow_stmt(&expr.kind) || is_block_expr(&expr.kind) {
                        // `if`/`match`/`for`/`while` are closed by `}` and need no
                        // trailing semicolon.
                        //
                        // A BLOCK is the same: `{ .. }` is self-delimiting, so a
                        // block is a complete statement with or without a `;`.
                        // Without this a block was classified as the block's TAIL
                        // expression, `parse_stmt_list` broke out early, and the
                        // statement AFTER the block was orphaned -- so the parser
                        // panicked with `expected '}', found 'let'`. A block
                        // followed by anything could not be parsed at all, and
                        // even `{} let z = 2;` failed.
                        let span = expr.span;
                        stmts.push(Stmt::new(StmtKind::Expr(expr), span, next_id()))
                    } else {
                        tail = Some(expr);
                        break;
                    }
                }
            }
        }

        (stmts, tail)
    }

    // ===== Let statements =====

    /// Apply a quantity to a pattern and all its sub-patterns (for tuples).
    fn apply_quantity_to_pattern(pattern: &mut Pattern, qty: Quantity) {
        pattern.quantity = qty;
        match &mut pattern.kind {
            PatternKind::Tuple(items) => {
                for item in items {
                    Self::apply_quantity_to_pattern(item, qty);
                }
            }
            PatternKind::Struct(_, fields) => {
                for field in fields {
                    Self::apply_quantity_to_pattern(&mut field.pattern, qty);
                }
            }
            PatternKind::Variant(_, _, items) => {
                for item in items {
                    Self::apply_quantity_to_pattern(item, qty);
                }
            }
            PatternKind::Array(items) => {
                for item in items {
                    Self::apply_quantity_to_pattern(item, qty);
                }
            }
            PatternKind::Or(a, b) => {
                Self::apply_quantity_to_pattern(a, qty);
                Self::apply_quantity_to_pattern(b, qty);
            }
            PatternKind::Ref(p) | PatternKind::InOut(p) | PatternKind::Consume(p) => {
                Self::apply_quantity_to_pattern(p, qty);
            }
            PatternKind::Guard(p, _) => {
                Self::apply_quantity_to_pattern(p, qty);
            }
            _ => {} // Ident, Wildcard, Literal, Range, Error - no sub-patterns
        }
    }

    /// Apply a quantity to a type and all its sub-types.
    fn apply_quantity_to_type(ty: &mut Type, qty: Quantity) {
        ty.quantity = qty;
        match &mut ty.kind {
            TypeKind::Tuple(elems) => {
                for elem in elems {
                    Self::apply_quantity_to_type(elem, qty);
                }
            }
            TypeKind::Array(elem, _) => {
                Self::apply_quantity_to_type(elem, qty);
            }
            TypeKind::Named(_, args) => {
                for arg in args {
                    if let TypeArg::Type(t) = arg {
                        Self::apply_quantity_to_type(t, qty);
                    }
                }
            }
            TypeKind::Function(params, ret) => {
                for p in params {
                    Self::apply_quantity_to_type(p, qty);
                }
                Self::apply_quantity_to_type(ret, qty);
            }
            TypeKind::Projection(inner) | TypeKind::Reversible(inner) => {
                Self::apply_quantity_to_type(inner, qty);
            }
            _ => {}
        }
    }

    fn parse_let_stmt(&mut self) -> Stmt {
        let start = self.pos;
        debug_log(&format!("parse_let_stmt: start, peek={:?}", self.peek()));
        self.expect(TK::Let);
        debug_log(&format!(
            "parse_let_stmt: after Let, peek={:?}",
            self.peek()
        ));

        // Check for `let inout` or `let consume` keywords
        if self.eat(TK::InOut) {
            // let inout name = value;
            debug_log(&format!("parse_let_stmt: InOut, peek={:?}", self.peek()));
            let name = self.parse_ident();
            debug_log(&format!(
                "parse_let_stmt: name={}, peek={:?}",
                name.name,
                self.peek()
            ));
            let ty = self.parse_optional_type_annotation();
            debug_log(&format!("parse_let_stmt: after ty, peek={:?}", self.peek()));
            self.expect(TK::Assign);
            debug_log(&format!(
                "parse_let_stmt: after Assign, peek={:?}",
                self.peek()
            ));
            let value = self.parse_expr();
            debug_log(&format!(
                "parse_let_stmt: after expr, peek={:?}",
                self.peek()
            ));
            self.eat(TK::Semicolon);
            let span = self.span_from(start);
            Stmt::new(
                StmtKind::LetInOut(LetInOutStmt {
                    name,
                    ty,
                    value,
                    span,
                }),
                span,
                next_id(),
            )
        } else if self.eat(TK::Consume) {
            // let consume name = value;
            debug_log(&format!("parse_let_stmt: Consume, peek={:?}", self.peek()));
            let name = self.parse_ident();
            debug_log(&format!(
                "parse_let_stmt: name={}, peek={:?}",
                name.name,
                self.peek()
            ));
            let ty = self.parse_optional_type_annotation();
            self.expect(TK::Assign);
            let value = self.parse_expr();
            self.eat(TK::Semicolon);
            let span = self.span_from(start);
            Stmt::new(
                StmtKind::LetConsume(LetConsumeStmt {
                    name,
                    ty,
                    value,
                    span,
                }),
                span,
                next_id(),
            )
        } else {
            // let [qty] mut? pattern = value;
            debug_log(&format!(
                "parse_let_stmt: regular let, peek={:?}",
                self.peek()
            ));
            let quantity = self.parse_quantity().unwrap_or(Quantity::Many);
            debug_log(&format!(
                "parse_let_stmt: quantity={:?}, peek={:?}",
                quantity,
                self.peek()
            ));
            let mutability = if self.eat(TK::Mut) {
                Mutability::Mut
            } else if self.eat(TK::InOut) {
                Mutability::InOut
            } else if self.eat(TK::Consume) {
                Mutability::Consume
            } else {
                Mutability::Immutable
            };
            debug_log(&format!(
                "parse_let_stmt: mutability={:?}, peek={:?}",
                mutability,
                self.peek()
            ));
            let mut pattern = self.parse_pattern();
            debug_log(&format!(
                "parse_let_stmt: pattern={:?}, peek={:?}",
                pattern,
                self.peek()
            ));
            // Apply the binding's quantity to the pattern (and sub-patterns for tuples)
            Self::apply_quantity_to_pattern(&mut pattern, quantity);
            let mut ty = self.parse_optional_type_annotation();
            // Apply the binding's quantity to the explicit type annotation
            if let Some(ref mut ty) = ty {
                Self::apply_quantity_to_type(ty, quantity);
            }
            debug_log(&format!("parse_let_stmt: after ty, peek={:?}", self.peek()));
            self.expect(TK::Assign);
            debug_log(&format!(
                "parse_let_stmt: after Assign, peek={:?}",
                self.peek()
            ));
            let value = self.parse_expr();
            debug_log(&format!(
                "parse_let_stmt: after expr, peek={:?}",
                self.peek()
            ));
            self.eat(TK::Semicolon);
            let span = self.span_from(start);
            Stmt::new(
                StmtKind::Let(LetStmt {
                    pattern,
                    ty,
                    quantity,
                    mutability,
                    value,
                    span,
                }),
                span,
                next_id(),
            )
        }
    }

    fn parse_optional_type_annotation(&mut self) -> Option<Type> {
        if self.at(TK::Colon) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        }
    }

    // ===== Reversible blocks =====

    /// Parse a `reversible { ... }` block (shared by statements and
    /// expressions).
    pub(crate) fn parse_reversible_block(&mut self) -> ReversibleBlock {
        let start = self.pos;
        self.expect(TK::Reversible);
        let body = self.parse_block();
        let span = self.span_from(start);
        ReversibleBlock {
            body,
            uncomputes: Vec::new(),
            span,
        }
    }

    /// Parse a `proof { ... }` block.
    ///
    /// The body is an ordinary block, so obligations inside it are typechecked
    /// by the normal path. `uncomputes` is not modelled: proof obligations are
    /// not a statement kind of their own yet, so they arrive as `assert(..)`
    /// expressions in the block body.
    pub(crate) fn parse_proof_block(&mut self) -> ProofBlock {
        let start = self.pos;
        self.expect(TK::Proof);
        let body = self.parse_block();
        let span = self.span_from(start);
        ProofBlock { body, span }
    }

    /// Parse `requires { assert(..); .. }` into the list of preconditions.
    ///
    /// Only `assert` statements are accepted. Anything else is a parse error rather than a
    /// silently dropped statement: a `requires` block that quietly ignored a line would let
    /// a precondition be written, not registered, and the body would then be proved under a
    /// weaker context than the author believes -- the exact kind of gap that makes a proof
    /// mean less than it appears to.
    pub(crate) fn parse_requires_block(&mut self) -> Vec<Expr> {
        self.expect(TK::Requires);
        let block = self.parse_block();
        let mut requires = Vec::new();
        for stmt in &block.stmts {
            let expr = match &stmt.kind {
                StmtKind::Expr(e) => e,
                _ => {
                    let msg =
                        "a `requires` block may only contain `assert(..);` statements".to_string();
                    self.fail::<crate::lexer::Token>(msg, stmt.span);
                    return requires;
                }
            };
            match &expr.kind {
                ExprKind::Call(callee, args) if is_assert_callee(callee) => {
                    let Some(first) = args.first() else {
                        self.fail::<crate::lexer::Token>(
                            "`assert` requires one argument".to_string(),
                            stmt.span,
                        );
                        return requires;
                    };
                    requires.push(first.clone());
                }
                // A quantified precondition is exactly the shape a quantizer needs, and it
                // is kept WHOLE -- binding and domain included. Flattening it to its inner
                // `assert` would drop the index domain, and the premise would silently
                // become weaker than the one the author wrote.
                ExprKind::Forall(_) | ExprKind::Quantified(_) => requires.push(expr.clone()),
                _ => {
                    let msg =
                        "a `requires` block may only contain `assert(..);` statements".to_string();
                    self.fail::<crate::lexer::Token>(msg, stmt.span);
                    return requires;
                }
            }
        }
        requires
    }
}

/// Whether an expression is a call to the pseudo-function `assert`.
pub(crate) fn is_assert_callee(callee: &Expr) -> bool {
    matches!(&callee.kind, ExprKind::Var(ident) if ident.name == "assert")
}

/// Whether an expression is a self-delimiting BLOCK, `{ .. }`.
///
/// Like `if`/`while`, a block is closed by its own `}` and needs no trailing
/// semicolon, so it is a statement rather than a block's tail expression.
pub(crate) fn is_block_expr(kind: &crate::ast::ExprKind) -> bool {
    matches!(kind, crate::ast::ExprKind::Block(_))
}
