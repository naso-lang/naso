//! Expression parsing for the Naso parser.
//!
//! Implements a precedence-climbing expression parser producing
//! [`crate::ast::Expr`] nodes, including calls, field/index access, control
//! flow, blocks, and quantum operations (`measure`, `entangle`).

#![allow(clippy::while_let_loop)]

use crate::ast::*;
use crate::lexer::TokenKind as TK;
use crate::parser::{Parser, next_id};

use super::token_span;

/// Whether a parsed expression can stand alone as a block statement without a
/// trailing semicolon. Control-flow block constructs (`if`, `match`, `for`,
/// `while`) are closed by a `}` and read naturally as statements.
pub(crate) fn is_control_flow_stmt(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::If(..)
            | ExprKind::Match(..)
            | ExprKind::For(..)
            | ExprKind::While(..)
            | ExprKind::Forall(..)
    )
}

fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
        BinOp::BitOr => 4,
        BinOp::BitXor => 5,
        BinOp::BitAnd => 6,
        BinOp::Shl | BinOp::Shr => 7,
        BinOp::Add | BinOp::Sub => 8,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 9,
        BinOp::Assign => 0,
    }
}

impl<'a> Parser<'a> {
    // ===== Expressions =====

    /// Parse a full expression, handling right-associative assignment.
    pub fn parse_expr(&mut self) -> Expr {
        let mut lhs = self.parse_expr_precedence(0);
        if self.at(TK::Assign) {
            self.bump();
            let rhs = self.parse_expr();
            let span = lhs.span.merge(rhs.span);
            lhs = Expr::new(
                ExprKind::Assign(Box::new(lhs), Box::new(rhs)),
                span,
                next_id(),
            );
        }
        lhs
    }

    /// Parse an expression using precedence climbing (left-associative).
    pub fn parse_expr_precedence(&mut self, min_prec: u8) -> Expr {
        let mut lhs = self.parse_unary();
        loop {
            let op = match self.peek_binary_op() {
                Some(op) => op,
                None => break,
            };
            let prec = binop_prec(op);
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.parse_expr_precedence(prec + 1);
            let span = lhs.span.merge(rhs.span);
            lhs = Expr::new(
                ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
                next_id(),
            );
        }
        lhs
    }

    fn peek_binary_op(&self) -> Option<BinOp> {
        match self.peek() {
            Some(TK::OrOr) => Some(BinOp::Or),
            Some(TK::AndAnd) => Some(BinOp::And),
            Some(TK::Eq) => Some(BinOp::Eq),
            Some(TK::Ne) => Some(BinOp::Ne),
            Some(TK::Lt) => Some(BinOp::Lt),
            Some(TK::Le) => Some(BinOp::Le),
            Some(TK::Gt) => Some(BinOp::Gt),
            Some(TK::Ge) => Some(BinOp::Ge),
            Some(TK::Pipe) => Some(BinOp::BitOr),
            Some(TK::Caret) => Some(BinOp::BitXor),
            Some(TK::Amp) => Some(BinOp::BitAnd),
            Some(TK::Shl) => Some(BinOp::Shl),
            Some(TK::Shr) => Some(BinOp::Shr),
            Some(TK::Plus) => Some(BinOp::Add),
            Some(TK::Minus) => Some(BinOp::Sub),
            Some(TK::Star) => Some(BinOp::Mul),
            Some(TK::Slash) => Some(BinOp::Div),
            Some(TK::Percent) => Some(BinOp::Rem),
            _ => None,
        }
    }

    fn parse_unary(&mut self) -> Expr {
        if self.eat(TK::Minus) {
            let e = self.parse_unary();
            let span = e.span;
            return Expr::new(ExprKind::Unary(UnOp::Neg, Box::new(e)), span, next_id());
        }
        if self.eat(TK::Not) {
            let e = self.parse_unary();
            let span = e.span;
            return Expr::new(ExprKind::Unary(UnOp::Not, Box::new(e)), span, next_id());
        }
        if self.eat(TK::Star) {
            let e = self.parse_unary();
            let span = e.span;
            return Expr::new(ExprKind::Unary(UnOp::Deref, Box::new(e)), span, next_id());
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut expr = self.parse_primary();
        loop {
            if self.at(TK::LParen) {
                self.bump();
                let args = self.parse_args_until(TK::RParen);
                self.expect(TK::RParen);
                let span = expr.span;
                expr = Expr::new(ExprKind::Call(Box::new(expr), args), span, next_id());
            } else if self.at(TK::Dot) {
                self.bump();
                let name = self.parse_ident();
                if self.at(TK::LParen) {
                    self.bump();
                    let args = self.parse_args_until(TK::RParen);
                    self.expect(TK::RParen);
                    let span = expr.span;
                    expr = Expr::new(
                        ExprKind::MethodCall(Box::new(expr), name, args),
                        span,
                        next_id(),
                    );
                } else {
                    let span = expr.span;
                    expr = Expr::new(ExprKind::Field(Box::new(expr), name), span, next_id());
                }
            } else if self.at(TK::LBracket) {
                self.bump();
                // Parse comma-separated indices (supporting multi-dimensional indexing)
                let indices = self.parse_args_until(TK::RBracket);
                self.expect(TK::RBracket);
                let span = expr.span;
                // If multiple indices, wrap in a tuple
                let idx_expr = if indices.len() == 1 {
                    indices.into_iter().next().unwrap()
                } else {
                    Expr::new(ExprKind::Tuple(indices), span, next_id())
                };
                expr = Expr::new(
                    ExprKind::Index(Box::new(expr), Box::new(idx_expr)),
                    span,
                    next_id(),
                );
            } else if self.at(TK::As) {
                // Numeric cast: `expr as i8`. Reuses ExprKind::Ascribe, which
                // already existed in the AST and was wired into visiting and
                // inference but never parsed.
                self.bump();
                let ty = self.parse_type();
                let span = expr.span;
                expr = Expr::new(ExprKind::Ascribe(Box::new(expr), ty), span, next_id());
            } else {
                break;
            }
        }
        expr
    }

    /// Parse a comma-separated argument/field list, stopping at `closer`.
    fn parse_args_until(&mut self, closer: TK) -> Vec<Expr> {
        let mut args = Vec::new();
        loop {
            if self.at(closer.clone()) {
                break;
            }
            args.push(self.parse_expr());
            if self.at(closer.clone()) {
                break;
            }
            self.expect(TK::Comma);
            if self.at(closer.clone()) {
                break;
            }
        }
        args
    }

    // ===== Primary =====

    fn parse_primary(&mut self) -> Expr {
        match self.peek() {
            Some(TK::If) => self.parse_if(),
            Some(TK::Match) => self.parse_match(),
            Some(TK::For) => self.parse_for(),
            Some(TK::Forall) => self.parse_quantified(),
            Some(TK::While) => self.parse_while(),
            Some(TK::Return) => self.parse_return(),
            Some(TK::Reversible) => self.parse_reversible_expr(),
            Some(TK::Measure) => self.parse_measure(),
            Some(TK::Entangle) => self.parse_entangle(),
            Some(TK::QAlloc) => self.parse_qalloc(),
            Some(TK::Hadamard) => self.parse_hadamard(),
            Some(TK::Reset) => self.parse_reset(),
            Some(TK::CNot) => self.parse_cnot(),
            Some(TK::Int(_)) | Some(TK::Float(_)) | Some(TK::Bool(_)) | Some(TK::Str(_))
            | Some(TK::Char(_)) => self.parse_literal(),
            // `{` may be a trailing block on an expression, or the body of a
            // construct whose bound is being parsed. See Parser::no_block_expr.
            Some(TK::LBrace) if !self.no_block_expr => self.parse_block_expr(),
            Some(TK::LParen) => self.parse_tuple_or_paren(),
            Some(TK::LBracket) => self.parse_array(),
            Some(TK::Ident(_)) => self.parse_var(),
            Some(TK::TypeIdent(_)) => self.parse_struct_literal_or_var(),
            None => self.unexpected("an expression"),
            Some(k) => self.unexpected(&format!("an expression, found `{k}`")),
        }
    }

    fn parse_literal(&mut self) -> Expr {
        let tok = self.bump().expect("literal token");
        let span = token_span(&tok);
        let lit = match &tok.kind {
            TK::Int(n) => Literal::Int(*n),
            TK::Float(f) => Literal::Float(*f),
            TK::Bool(b) => Literal::Bool(*b),
            TK::Str(s) => Literal::String(s.clone()),
            TK::Char(c) => Literal::Char(*c),
            _ => unreachable!("parse_literal on {:?}", tok.kind),
        };
        Expr::new(ExprKind::Literal(lit), span, next_id())
    }

    fn parse_var(&mut self) -> Expr {
        let start = self.pos;
        let name = self.parse_ident();
        Expr::new(ExprKind::Var(name), self.span_from(start), next_id())
    }

    /// Parse either a struct literal `TypeName { ... }` or a plain variable
    /// reference to a type name.
    fn parse_struct_literal_or_var(&mut self) -> Expr {
        let start = self.pos;
        let name = self.parse_ident();
        // Same rule as the `{` primary: inside a range bound, the brace belongs
        // to the enclosing construct, not to a struct literal.
        if self.at(TK::LBrace) && !self.no_block_expr {
            self.bump();
            let mut fields = Vec::new();
            loop {
                if self.at(TK::RBrace) {
                    break;
                }
                fields.push(self.parse_field_expr());
                if self.at(TK::RBrace) {
                    break;
                }
                self.expect(TK::Comma);
                if self.at(TK::RBrace) {
                    break;
                }
            }
            self.expect(TK::RBrace);
            let span = self.span_from(start);
            Expr::new(ExprKind::Struct(name, fields), span, next_id())
        } else {
            let span = self.span_from(start);
            Expr::new(ExprKind::Var(name), span, next_id())
        }
    }

    fn parse_field_expr(&mut self) -> FieldExpr {
        let start = self.pos;
        let name = self.parse_ident();
        self.expect(TK::Colon);
        let value = self.parse_expr();
        FieldExpr {
            name,
            value,
            span: self.span_from(start),
        }
    }

    // ===== Tuples / arrays / blocks =====

    fn parse_tuple_or_paren(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::LParen);
        if self.at(TK::RParen) {
            self.bump();
            let span = self.span_from(start);
            return Expr::new(ExprKind::Literal(Literal::Unit), span, next_id());
        }
        let first = self.parse_expr();
        if !self.at(TK::Comma) {
            self.expect(TK::RParen);
            return first;
        }
        let mut items = vec![first];
        while self.at(TK::Comma) {
            self.bump();
            if self.at(TK::RParen) {
                break;
            }
            items.push(self.parse_expr());
        }
        self.expect(TK::RParen);
        let span = self.span_from(start);
        Expr::new(ExprKind::Tuple(items), span, next_id())
    }

    fn parse_array(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::LBracket);
        let mut items = Vec::new();
        loop {
            if self.at(TK::RBracket) {
                break;
            }
            items.push(self.parse_expr());
            if self.at(TK::RBracket) {
                break;
            }
            self.expect(TK::Comma);
            if self.at(TK::RBracket) {
                break;
            }
        }
        self.expect(TK::RBracket);
        let span = self.span_from(start);
        Expr::new(ExprKind::Array(items), span, next_id())
    }

    fn parse_block_expr(&mut self) -> Expr {
        let b = self.parse_block();
        let span = b.span;
        Expr::new(ExprKind::Block(Box::new(b)), span, next_id())
    }

    // ===== Control flow =====

    fn parse_if(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::If);
        let cond = self.parse_expr();
        let then = self.parse_block_expr();
        let else_ = if self.at(TK::Else) {
            self.bump();
            if self.at(TK::If) {
                Some(Box::new(self.parse_if()))
            } else {
                Some(Box::new(self.parse_block_expr()))
            }
        } else {
            None
        };
        let span = self.span_from(start);
        Expr::new(
            ExprKind::If(Box::new(cond), Box::new(then), else_),
            span,
            next_id(),
        )
    }

    fn parse_match(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Match);
        let scrutinee = self.parse_expr();
        self.expect(TK::LBrace);
        let mut arms = Vec::new();
        loop {
            if self.at(TK::RBrace) {
                break;
            }
            arms.push(self.parse_match_arm());
            self.eat(TK::Comma);
            if self.at(TK::RBrace) {
                break;
            }
        }
        self.expect(TK::RBrace);
        let span = self.span_from(start);
        Expr::new(ExprKind::Match(Box::new(scrutinee), arms), span, next_id())
    }

    fn parse_match_arm(&mut self) -> MatchArm {
        let start = self.pos;
        let pattern = self.parse_pattern();
        let guard = if self.at(TK::If) {
            self.bump();
            Some(self.parse_expr())
        } else {
            None
        };
        self.expect(TK::FatArrow);
        let body = self.parse_expr();
        MatchArm {
            pattern,
            guard,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_for(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::For);
        let var = self.parse_ident();
        if !self.at(TK::In) {
            self.unexpected::<()>(r"`in`");
        }
        self.bump();
        let iter = self.parse_expr();
        let body = self.parse_block_expr();
        let body = match body.kind {
            ExprKind::Block(b) => *b,
            _ => unreachable!("for loop body is always a block"),
        };
        let span = self.span_from(start);
        Expr::new(
            ExprKind::For(Box::new(ForLoop {
                var,
                iter,
                body,
                span,
            })),
            span,
            next_id(),
        )
    }

    pub(crate) fn parse_forall(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Forall);

        // Parse one or more comma-separated bindings
        let mut bindings = Vec::new();
        loop {
            let var = self.parse_ident();
            self.expect(TK::In);
            let lower = self.parse_range_bound();
            self.expect(TK::DotDot);
            let upper = self.parse_range_bound();
            bindings.push((var, lower, upper));

            if !self.at(TK::Comma) {
                break;
            }
            self.bump(); // consume comma
        }

        let body = self.parse_block_expr();
        let body = match body.kind {
            ExprKind::Block(b) => *b,
            _ => unreachable!("forall loop body is always a block"),
        };

        let span = self.span_from(start);
        Expr::new(
            ExprKind::Forall(Box::new(ForallLoop {
                bindings,
                body,
                span,
            })),
            span,
            next_id(),
        )
    }

    /// Parse a quantified proposition: `forall i in 0..N { <bool expr> }`.
    ///
    /// Same syntax as the loop form, but reached only from expression
    /// position -- see ExprKind::Quantified. The body block must end in a
    /// tail expression, because that tail is the predicate; a proposition
    /// written as a statement sequence has no meaning.
    fn parse_quantified(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Forall);

        let mut bindings = Vec::new();
        loop {
            let var = self.parse_ident();
            self.expect(TK::In);
            let lower = self.parse_range_bound();
            self.expect(TK::DotDot);
            let upper = self.parse_range_bound();
            bindings.push((var, lower, upper));

            if !self.at(TK::Comma) {
                break;
            }
            self.bump(); // consume comma
        }

        let body = self.parse_block_expr();
        let body = match body.kind {
            ExprKind::Block(b) => *b,
            _ => unreachable!("quantified body is always a block"),
        };

        let span = self.span_from(start);
        Expr::new(
            ExprKind::Quantified(Box::new(ForallLoop {
                bindings,
                body,
                span,
            })),
            span,
            next_id(),
        )
    }

    fn parse_while(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::While);
        let cond = self.parse_expr();
        let body = self.parse_block_expr();
        let span = self.span_from(start);
        Expr::new(
            ExprKind::While(Box::new(cond), Box::new(body)),
            span,
            next_id(),
        )
    }

    fn parse_return(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Return);
        let value = match self.peek() {
            Some(TK::Semicolon) | Some(TK::RBrace) | Some(TK::RParen) | None => None,
            _ => Some(Box::new(self.parse_expr())),
        };
        let span = self.span_from(start);
        Expr::new(ExprKind::Return(value), span, next_id())
    }

    // ===== Quantum operations =====

    fn parse_measure(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Measure);
        let target = self.parse_unary();
        let span = self.span_from(start);
        Expr::new(
            ExprKind::QuantumOp(QuantumOp::Measure(Box::new(target))),
            span,
            next_id(),
        )
    }

    fn parse_entangle(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Entangle);
        let args = if self.at(TK::LParen) {
            self.bump();
            let args = self.parse_args_until(TK::RParen);
            self.expect(TK::RParen);
            args
        } else {
            Vec::new()
        };
        let span = self.span_from(start);
        Expr::new(
            ExprKind::QuantumOp(QuantumOp::Entangle(args)),
            span,
            next_id(),
        )
    }

    fn parse_qalloc(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::QAlloc);
        // qalloc() or qalloc(n) - consume parentheses
        self.expect(TK::LParen);
        // Parse optional size argument
        let _size = if self.at(TK::RParen) {
            None
        } else {
            let expr = self.parse_expr();
            Some(expr)
        };
        self.expect(TK::RParen);
        // qalloc returns a qubit (or qubit array if size specified)
        let span = self.span_from(start);
        Expr::new(
            ExprKind::QuantumOp(QuantumOp::Alloc(Ident::new("qalloc", span))),
            span,
            next_id(),
        )
    }

    fn parse_hadamard(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Hadamard);
        self.expect(TK::LParen);
        let args = self.parse_args_until(TK::RParen);
        self.expect(TK::RParen);
        let span = self.span_from(start);
        // hadamard takes 1 qubit argument
        Expr::new(
            ExprKind::QuantumOp(QuantumOp::ApplyGate(GateKind::H, args)),
            span,
            next_id(),
        )
    }

    /// `reset(q)` -- returns the qubit to |0> in place.
    ///
    /// Parsed as a `GateKind::Reset` apply-gate rather than a dedicated
    /// `QuantumOp` variant: reset IS a gate application, it just has an exact
    /// state effect. Routing it through `ApplyGate` means the verifier's existing
    /// `GateKind::Reset => int(0)` transition and the runtime exporters' reset
    /// emission both apply with no special case at either end.
    fn parse_reset(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::Reset);
        self.expect(TK::LParen);
        let args = self.parse_args_until(TK::RParen);
        self.expect(TK::RParen);
        let span = self.span_from(start);
        // reset takes 1 qubit argument
        Expr::new(
            ExprKind::QuantumOp(QuantumOp::ApplyGate(GateKind::Reset, args)),
            span,
            next_id(),
        )
    }

    fn parse_cnot(&mut self) -> Expr {
        let start = self.pos;
        self.expect(TK::CNot);
        self.expect(TK::LParen);
        let args = self.parse_args_until(TK::RParen);
        self.expect(TK::RParen);
        let span = self.span_from(start);
        // cnot takes 2 qubit arguments (control, target)
        Expr::new(
            ExprKind::QuantumOp(QuantumOp::ApplyGate(GateKind::CX, args)),
            span,
            next_id(),
        )
    }

    // ===== Reversible blocks =====

    /// Parse a `reversible { ... }` block expression.
    pub(crate) fn parse_reversible_expr(&mut self) -> Expr {
        let start = self.pos;
        let rb = self.parse_reversible_block();
        let span = self.span_from(start);
        Expr::new(ExprKind::Reversible(Box::new(rb)), span, next_id())
    }

    // ===== Patterns (used by match arms and let bindings) =====

    pub fn parse_pattern(&mut self) -> Pattern {
        match self.peek() {
            Some(TK::Int(_)) | Some(TK::Float(_)) | Some(TK::Bool(_)) | Some(TK::Str(_))
            | Some(TK::Char(_)) => self.parse_literal_pattern(),
            Some(TK::LParen) => self.parse_tuple_pattern(),
            Some(TK::Ident(_)) | Some(TK::TypeIdent(_)) => self.parse_ident_pattern(),
            None => self.unexpected("a pattern"),
            Some(k) => self.unexpected(&format!("a pattern, found `{k}`")),
        }
    }

    fn parse_literal_pattern(&mut self) -> Pattern {
        let tok = self.bump().expect("pattern literal token");
        let span = token_span(&tok);
        let lit = match &tok.kind {
            TK::Int(n) => Literal::Int(*n),
            TK::Float(f) => Literal::Float(*f),
            TK::Bool(b) => Literal::Bool(*b),
            TK::Str(s) => Literal::String(s.clone()),
            TK::Char(c) => Literal::Char(*c),
            _ => unreachable!("parse_literal_pattern on {:?}", tok.kind),
        };
        Pattern::new(PatternKind::Literal(lit), span, next_id())
    }

    fn parse_tuple_pattern(&mut self) -> Pattern {
        let start = self.pos;
        self.expect(TK::LParen);
        let mut items = Vec::new();
        loop {
            if self.at(TK::RParen) {
                break;
            }
            items.push(self.parse_pattern());
            if self.at(TK::RParen) {
                break;
            }
            self.expect(TK::Comma);
            if self.at(TK::RParen) {
                break;
            }
        }
        self.expect(TK::RParen);
        Pattern::new(PatternKind::Tuple(items), self.span_from(start), next_id())
    }

    fn parse_ident_pattern(&mut self) -> Pattern {
        let tok = self.bump().expect("pattern identifier");
        let span = token_span(&tok);
        let (name, is_wildcard) = match &tok.kind {
            TK::Ident(s) => (Ident::new(s.clone(), span), s == "_"),
            TK::TypeIdent(s) => (Ident::new(s.clone(), span), false),
            other => panic!("expected pattern identifier, found `{other}`"),
        };
        if is_wildcard {
            return Pattern::new(PatternKind::Wildcard, span, next_id());
        }
        if self.at(TK::LBrace) {
            self.bump();
            let mut fields = Vec::new();
            loop {
                if self.at(TK::RBrace) {
                    break;
                }
                let field = self.parse_field_pattern();
                fields.push(field);
                if self.at(TK::RBrace) {
                    break;
                }
                self.expect(TK::Comma);
                if self.at(TK::RBrace) {
                    break;
                }
            }
            self.expect(TK::RBrace);
            return Pattern::new(PatternKind::Struct(name, fields), span, next_id());
        }
        if self.at(TK::LParen) {
            self.bump();
            let mut args = Vec::new();
            loop {
                if self.at(TK::RParen) {
                    break;
                }
                args.push(self.parse_pattern());
                if self.at(TK::RParen) {
                    break;
                }
                self.expect(TK::Comma);
                if self.at(TK::RParen) {
                    break;
                }
            }
            self.expect(TK::RParen);
            // Enum variant arguments are dropped for now.
            let _ = args;
            return Pattern::new(PatternKind::Ident(name), span, next_id());
        }
        Pattern::new(PatternKind::Ident(name), span, next_id())
    }

    fn parse_field_pattern(&mut self) -> FieldPattern {
        let start = self.pos;
        let name = self.parse_ident();
        self.expect(TK::Colon);
        let pattern = self.parse_pattern();
        FieldPattern {
            name,
            pattern,
            span: self.span_from(start),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    #[test]
    fn assigns_like_an_expression() {
        let prog = parse_program("fn f() { let x = 1; x = 2; }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 2);
        match &func.body.stmts[1].kind {
            StmtKind::Expr(e) => assert!(matches!(e.kind, ExprKind::Assign(..))),
            other => panic!("expected assign stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_struct_literal() {
        let prog =
            parse_program("fn f() { let p = Point { x: 1.0, y: 2.0 }; }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        match &func.body.stmts[0].kind {
            StmtKind::Let(l) => match &l.value.kind {
                ExprKind::Struct(name, fields) => {
                    assert_eq!(name.name, "Point");
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected struct, got {other:?}"),
            },
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_measure_and_entangle() {
        let prog = parse_program("fn f(q: Qubit) { let r = measure q; let e = entangle(q, q); }")
            .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 2);
    }

    /// `reset(q)` parses as a `GateKind::Reset` apply-gate.
    ///
    /// `reset` had no lexer token, no parser production, and no AST variant, while
    /// the verifier modelled it and the runtime exporters emitted it -- so it read
    /// as a language feature and was not one. It is routed through `ApplyGate`
    /// rather than a dedicated `QuantumOp` because reset IS a gate application; it
    /// just has an exact state effect, which is what lets the verifier's existing
    /// `GateKind::Reset => int(0)` transition apply with no special case.
    #[test]
    fn parses_reset() {
        let prog = parse_program("fn f(q: Qubit) { reset(q); }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::QuantumOp(QuantumOp::ApplyGate(GateKind::Reset, args)) => {
                    assert_eq!(args.len(), 1, "reset takes exactly one qubit");
                }
                other => panic!("expected ApplyGate(Reset), got {other:?}"),
            },
            other => panic!("expected expression stmt, got {other:?}"),
        }
    }

    /// `reset` is case-insensitive, like the other quantum keywords.
    #[test]
    fn parses_reset_case_insensitively() {
        for src in [
            "fn f(q: Qubit) { reset(q); }",
            "fn f(q: Qubit) { Reset(q); }",
            "fn f(q: Qubit) { RESET(q); }",
        ] {
            parse_program(src).unwrap_or_else(|e| panic!("{src} should parse: {e}"));
        }
    }

    /// `reset` takes exactly one argument.
    ///
    /// Two arguments is a user error, not a two-qubit reset, so it must not parse
    /// as one silently.
    #[test]
    fn reset_with_two_arguments_is_rejected() {
        let result = std::panic::catch_unwind(|| {
            parse_program("fn f(a: Qubit, b: Qubit) { reset(a, b); }").is_ok()
        });
        // Either it fails to parse or it parses and then fails to typecheck; what
        // must not happen is it parsing as a valid single-qubit reset.
        if result.unwrap_or(false) {
            let mut prog =
                parse_program("fn f(a: Qubit, b: Qubit) { reset(a, b); }").expect("parse failed");
            let errors = crate::typecheck::check_program(&mut prog).errors;
            assert!(
                !errors.is_empty(),
                "reset(a, b) must not be accepted as a valid statement"
            );
        }
    }

    #[test]
    fn parses_hadamard_and_cnot() {
        let prog = parse_program("fn f(q0: Qubit, q1: Qubit) { hadamard(q0); cnot(q0, q1); }")
            .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 2);
        // First stmt: hadamard(q0)
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::QuantumOp(QuantumOp::ApplyGate(GateKind::H, args)) => {
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected QuantumOp::ApplyGate(H), got {:?}", other),
            },
            other => panic!("expected expr stmt, got {:?}", other),
        }
        // Second stmt: cnot(q0, q1)
        match &func.body.stmts[1].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::QuantumOp(QuantumOp::ApplyGate(GateKind::CX, args)) => {
                    assert_eq!(args.len(), 2);
                }
                other => panic!("expected QuantumOp::ApplyGate(CX), got {:?}", other),
            },
            other => panic!("expected expr stmt, got {:?}", other),
        }
    }

    #[test]
    fn parses_qalloc() {
        let prog = parse_program("fn f() { let q = qalloc(); }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 1);
        match &func.body.stmts[0].kind {
            StmtKind::Let(l) => match &l.value.kind {
                ExprKind::QuantumOp(QuantumOp::Alloc(_)) => {}
                other => panic!("expected QuantumOp::Alloc, got {:?}", other),
            },
            other => panic!("expected let stmt, got {:?}", other),
        }
    }

    #[test]
    fn parses_bell_pair_tuple_return() {
        let prog = parse_program(
            "fn bell_pair() -> (Qubit, Qubit) { let q0 = qalloc(); let q1 = qalloc(); (q0, q1) }",
        )
        .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        // Check return type is tuple
        match &func.ret_ty.as_ref().unwrap().kind {
            TypeKind::Tuple(elems) => {
                assert_eq!(elems.len(), 2);
                match (&elems[0].kind, &elems[1].kind) {
                    (TypeKind::Qubit, TypeKind::Qubit) => {}
                    other => panic!("expected (Qubit, Qubit), got {:?}", other),
                }
            }
            other => panic!("expected tuple type, got {:?}", other),
        }
        // Check body has two qalloc calls and a tuple tail expression
        assert_eq!(func.body.stmts.len(), 2);
        assert!(func.body.expr.is_some());
        match &func.body.expr.as_ref().unwrap().kind {
            ExprKind::Tuple(items) => assert_eq!(items.len(), 2),
            other => panic!("expected tuple tail expression, got {:?}", other),
        }
    }

    #[test]
    fn parses_forall_simple() {
        let prog =
            parse_program("fn f() { forall i in 0..10 { let x = i; } }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 1);
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Forall(forall_loop) => {
                    assert_eq!(forall_loop.bindings.len(), 1);
                    assert_eq!(forall_loop.bindings[0].0.name, "i");
                }
                other => panic!("expected Forall, got {:?}", other),
            },
            other => panic!("expected expr stmt, got {:?}", other),
        }
    }

    #[test]
    fn parses_forall_multiple_bindings() {
        let prog = parse_program("fn f() { forall i in 0..10, j in 0..20 { let x = i + j; } }")
            .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 1);
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Forall(forall_loop) => {
                    assert_eq!(forall_loop.bindings.len(), 2);
                    assert_eq!(forall_loop.bindings[0].0.name, "i");
                    assert_eq!(forall_loop.bindings[1].0.name, "j");
                }
                other => panic!("expected Forall, got {:?}", other),
            },
            other => panic!("expected expr stmt, got {:?}", other),
        }
    }

    #[test]
    fn parses_nested_forall() {
        let prog =
            parse_program("fn f() { forall i in 0..10 { forall j in 0..5 { let x = i * j; } } }")
                .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 1);
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Forall(outer) => {
                    assert_eq!(outer.bindings.len(), 1);
                    // Check nested forall in body
                    match &outer.body.stmts[0].kind {
                        StmtKind::Expr(inner_e) => match &inner_e.kind {
                            ExprKind::Forall(inner) => {
                                assert_eq!(inner.bindings.len(), 1);
                                assert_eq!(inner.bindings[0].0.name, "j");
                            }
                            other => panic!("expected nested Forall, got {:?}", other),
                        },
                        other => panic!("expected expr stmt in outer body, got {:?}", other),
                    }
                }
                other => panic!("expected Forall, got {:?}", other),
            },
            other => panic!("expected expr stmt, got {:?}", other),
        }
    }

    #[test]
    fn parses_forall_with_expr_bounds() {
        let prog = parse_program("fn f(n: Int) { forall i in 0..n { let x = i + 1; } }")
            .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.body.stmts.len(), 1);
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Forall(forall_loop) => {
                    assert_eq!(forall_loop.bindings.len(), 1);
                    // Upper bound is variable n
                }
                other => panic!("expected Forall, got {:?}", other),
            },
            other => panic!("expected expr stmt, got {:?}", other),
        }
    }

    #[test]
    fn is_control_flow_stmt_recognizes_blocks() {
        let then = Expr::new(
            ExprKind::Block(Box::new(Block::new(Vec::new(), None, Span::default()))),
            Span::default(),
            NodeId::new(2),
        );
        assert!(is_control_flow_stmt(&ExprKind::If(
            Box::new(Expr::new(
                ExprKind::Literal(Literal::Bool(true)),
                Span::default(),
                NodeId::new(1)
            )),
            Box::new(then),
            None,
        )));
        assert!(!is_control_flow_stmt(&ExprKind::Var(Ident::new(
            "x",
            Span::default()
        ))));
    }
}
