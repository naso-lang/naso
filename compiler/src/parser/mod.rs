//! Recursive-descent parser for the Naso language.
//!
//! The parser consumes a token stream (produced by [`crate::lexer`]) and
//! produces the AST types defined in [`crate::ast`]. It replaces the earlier
//! `winnow`-combinator implementation with a hand-written recursive-descent
//! parser over a [`Token`] slice so that quantitative-type constructs like
//! `[0] Matrix[Rows, Cols]` are handled without combinator lifetime issues.

use crate::ast::*;
use crate::lexer::{Lexer, Token, TokenKind as TK};
use crate::typecheck::debug_log;

pub mod expr;
pub mod stmt;
pub mod ty;

/// Monotonic node-id counter for AST nodes created during parsing.
use std::sync::atomic::{AtomicU32, Ordering};
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

pub(crate) fn next_id() -> NodeId {
    NodeId::new(NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// Hand-written recursive-descent parser over a token slice.
///
/// The parser owns an immutable borrow of the token stream and a cursor
/// position. Comments and newlines are expected to have been filtered out
/// before construction (see [`parse_program`]).
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// While set, `{` does not begin a block expression.
    ///
    /// Needed for range bounds. In `forall i in 0..N { ... }` the `{` is the
    /// loop body, not a block attached to the expression `N`, but `parse_expr`
    /// has no way to know that and consumes the brace as a trailing block --
    /// which then parses the loop body's first statement as the block of `N`
    /// and panics. Only an *identifier* bound (`N`) exposed this; a literal
    /// bound (`10`) happened to produce an empty block and silently worked.
    no_block_expr: bool,
}

impl<'a> Parser<'a> {
    /// Create a parser over `tokens`.
    pub fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            no_block_expr: false,
        }
    }

    /// Parse a range bound, where `{` starts the loop body rather than a block
    /// expression. Restores the previous setting afterwards.
    pub fn parse_range_bound(&mut self) -> Expr {
        let prev = self.no_block_expr;
        self.no_block_expr = true;
        let expr = self.parse_expr();
        self.no_block_expr = prev;
        expr
    }

    // ===== Token-stream helpers =====

    /// Whether the cursor is past the last token.
    pub fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// Look at the current token kind without consuming it.
    pub fn peek(&self) -> Option<&TK> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    /// Look at the current token without consuming it.
    fn peek_token(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    /// Consume and return the current token, or `None` at end of input.
    pub fn bump(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    /// Consume the current token if its kind matches `kind`, otherwise raise
    /// a parse panic describing what was expected.
    pub fn expect(&mut self, kind: TK) -> Token {
        match self.peek() {
            Some(k) if *k == kind => self.bump().expect("peeked token disappeared"),
            other => panic!(
                "parse error: expected `{}`, found {} at token {}",
                kind.display_name(),
                other
                    .map(|k| k.to_string())
                    .unwrap_or_else(|| "end of input".to_string()),
                self.pos,
            ),
        }
    }

    /// Consume the current token if its kind matches `kind`, returning whether
    /// it did.
    pub fn eat(&mut self, kind: TK) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Whether the current token kind is `kind`, without consuming it.
    pub fn at(&self, kind: TK) -> bool {
        self.peek().map(|k| *k == kind).unwrap_or(false)
    }

    /// Whether the current token is an identifier spelled `name`.
    fn at_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Some(TK::Ident(s)) if s == name)
    }

    /// Abort with a `panic!` describing the token found at the cursor.
    fn unexpected<T>(&self, expected: &str) -> T {
        match self.peek_token() {
            Some(t) => panic!(
                "parse error: expected {expected}, found `{}` at line {}, col {}",
                t.kind, t.span.line, t.span.column
            ),
            None => panic!("parse error: expected {expected}, found end of input"),
        }
    }

    /// Create a [`Span`] covering the tokens consumed since position `start`.
    pub fn span_from(&self, start: usize) -> Span {
        match (
            self.tokens.get(start),
            self.tokens.get(self.pos.saturating_sub(1)),
        ) {
            (Some(f), Some(l)) => Span::new(
                f.span.start as u32,
                l.span.end as u32,
                f.span.line,
                f.span.column,
            ),
            _ => Span::default(),
        }
    }

    // ===== Quantities =====

    /// Parse an optional single-token quantity marker `[0]`, `[1]`, `[*]` or
    /// `[N]` (where `N` is any identifier / type name).
    ///
    /// Symbolic quantity markers like `[N]` (a Nat variable) have no dedicated
    /// [`Quantity`] variant, so they are stored as `Quantity::Bounded(u32::MAX)`
    /// as a placeholder.
    pub fn parse_quantity(&mut self) -> Option<Quantity> {
        // Handle `[*]` as a single token
        if self.at(TK::QtyStar) {
            self.bump();
            return Some(Quantity::Many);
        }
        if !self.at(TK::LBracket) {
            return None;
        }
        let start = self.pos;
        self.bump();
        let qty = match self.peek() {
            Some(TK::QtyStar) => {
                self.bump();
                Quantity::Many
            }
            Some(TK::Int(n)) => {
                let n = *n;
                self.bump();
                match n {
                    0 => Quantity::Zero,
                    1 => Quantity::One,
                    _ => Quantity::Bounded(n as u32),
                }
            }
            Some(TK::Ident(_)) | Some(TK::TypeIdent(_)) | Some(TK::FloatKw) => {
                self.bump();
                Quantity::Bounded(u32::MAX)
            }
            _ => {
                self.pos = start;
                return None;
            }
        };
        if self.at(TK::RBracket) {
            self.bump();
            Some(qty)
        } else {
            self.pos = start;
            None
        }
    }

    // ===== Identifiers =====

    /// Parse an identifier or type-name token into an [`Ident`].
    pub fn parse_ident(&mut self) -> Ident {
        let start = self.pos;
        let name = match self.bump() {
            Some(t) => match &t.kind {
                TK::Ident(s) | TK::TypeIdent(s) => s.clone(),
                TK::LParen => {
                    // Recovery: if we unexpectedly hit a '(', return a dummy identifier
                    // to allow parsing to continue and report more errors
                    "dummy_ident".to_string()
                }
                other => {
                    panic!("parse error: expected identifier, found `{other}` at token {start}")
                }
            },
            None => panic!("parse error: expected identifier, found end of input"),
        };
        Ident::new(name, self.span_from(start))
    }

    // ===== Program =====

    /// Parse the top-level item list.
    pub fn parse_program_items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while !self.is_eof() {
            items.push(self.parse_item());
        }
        items
    }

    /// Parse a single top-level item.
    pub fn parse_item(&mut self) -> Item {
        match self.peek() {
            Some(TK::Fn) => Item::Function(self.parse_fn()),
            Some(TK::Struct) => Item::TypeDef(self.parse_struct()),
            Some(TK::Enum) => Item::TypeDef(self.parse_enum()),
            Some(TK::Type) => Item::TypeDef(self.parse_type_alias()),
            Some(TK::Const) => Item::Const(self.parse_const()),
            Some(TK::Import) => Item::Import(self.parse_import()),
            Some(TK::Mod) => Item::Module(self.parse_module()),
            _ => self.unexpected("a top-level item"),
        }
    }

    // ===== Functions =====

    /// Parse a `fn` item.
    pub fn parse_fn(&mut self) -> Function {
        let start = self.pos;
        self.expect(TK::Fn);

        // Check for reversible modifier
        let is_reversible = self.eat(TK::Reversible);

        let quantity = self.parse_quantity().unwrap_or(Quantity::Many);
        debug_log(&format!("parse_fn: after quantity, peek={:?}", self.peek()));
        let name = self.parse_ident();
        debug_log(&format!(
            "parse_fn: name={}, peek={:?}",
            name.name,
            self.peek()
        ));
        let generics = if self.at(TK::LBracket) {
            self.parse_generic_params()
        } else {
            Vec::new()
        };
        debug_log(&format!("parse_fn: after generics, peek={:?}", self.peek()));
        self.expect(TK::LParen);
        debug_log(&format!("parse_fn: after LParen, peek={:?}", self.peek()));
        let params = if self.at(TK::RParen) {
            Vec::new()
        } else {
            self.parse_params()
        };
        debug_log(&format!("parse_fn: after params, peek={:?}", self.peek()));
        self.expect(TK::RParen);
        debug_log(&format!("parse_fn: after RParen, peek={:?}", self.peek()));
        let ret_ty = if self.at(TK::Arrow) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        debug_log(&format!("parse_fn: after ret_ty, peek={:?}", self.peek()));
        let body = self.parse_block();
        debug_log(&format!("parse_fn: after body, peek={:?}", self.peek()));
        let span = self.span_from(start);

        Function {
            name,
            generics,
            params,
            ret_ty,
            body,
            span,
            attributes: Vec::new(),
            is_reversible,
            quantity,
        }
    }

    /// Parse a list of comma-separated parameters, stopping at the closing
    /// `)` which the caller consumes.
    pub fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        loop {
            params.push(self.parse_param());
            if self.at(TK::RParen) {
                break;
            }
            self.expect(TK::Comma);
            if self.at(TK::RParen) {
                break;
            }
        }
        params
    }

    fn parse_param(&mut self) -> Param {
        let start = self.pos;
        let pre_mut = if self.eat(TK::InOut) {
            Mutability::InOut
        } else if self.eat(TK::Consume) {
            Mutability::Consume
        } else if self.eat(TK::Mut) {
            Mutability::Mut
        } else {
            Mutability::Immutable
        };
        let quantity = self.parse_quantity().unwrap_or(Quantity::Many);
        let name = self.parse_ident();
        self.expect(TK::Colon);
        let mutability = if self.eat(TK::Consume) {
            Mutability::Consume
        } else if self.eat(TK::InOut) {
            Mutability::InOut
        } else if self.eat(TK::Mut) {
            Mutability::Mut
        } else {
            pre_mut
        };
        let ty = self.parse_type();
        Param {
            name,
            ty,
            quantity,
            mutability,
            span: self.span_from(start),
        }
    }

    /// Parse generic parameters in `[ ... ]`, independent of quantity markers.
    pub fn parse_generic_params(&mut self) -> Vec<GenericParam> {
        self.expect(TK::LBracket);
        let mut params = Vec::new();
        loop {
            params.push(self.parse_generic_param());
            if self.at(TK::RBracket) {
                break;
            }
            self.expect(TK::Comma);
            if self.at(TK::RBracket) {
                break;
            }
        }
        self.expect(TK::RBracket);
        params
    }

    fn parse_generic_param(&mut self) -> GenericParam {
        let start = self.pos;
        let _qty = self.parse_quantity();
        let name = self.parse_ident();
        let kind = if self.at(TK::Colon) {
            self.bump();
            self.parse_generic_kind()
        } else {
            GenericKind::Type
        };
        GenericParam {
            name,
            kind,
            span: self.span_from(start),
        }
    }

    fn parse_generic_kind(&mut self) -> GenericKind {
        match self.peek() {
            Some(TK::Type) => {
                self.bump();
                GenericKind::Type
            }
            Some(TK::Nat) => {
                self.bump();
                GenericKind::Nat
            }
            Some(TK::QtyStar) => {
                self.bump();
                GenericKind::Quantity
            }
            Some(TK::Ident(_)) if self.at_ident("nat") => {
                self.bump();
                GenericKind::Nat
            }
            Some(TK::Ident(_)) if self.at_ident("type") => {
                self.bump();
                GenericKind::Type
            }
            _ => self.unexpected("a generic kind (`type`, `nat`, or `[*]`)"),
        }
    }

    // ===== Type definitions =====

    fn parse_struct(&mut self) -> TypeDef {
        let start = self.pos;
        self.expect(TK::Struct);
        let name = self.parse_ident();
        let generics = if self.at(TK::LBracket) {
            self.parse_generic_params()
        } else {
            Vec::new()
        };
        self.expect(TK::LBrace);
        let fields = self.parse_fields();
        self.expect(TK::RBrace);
        let span = self.span_from(start);
        TypeDef {
            name,
            generics,
            kind: TypeDefKind::Struct(fields),
            span,
            attributes: Vec::new(),
        }
    }

    fn parse_enum(&mut self) -> TypeDef {
        let start = self.pos;
        self.expect(TK::Enum);
        let name = self.parse_ident();
        let generics = if self.at(TK::LBracket) {
            self.parse_generic_params()
        } else {
            Vec::new()
        };
        self.expect(TK::LBrace);
        let mut variants = Vec::new();
        loop {
            if self.at(TK::RBrace) {
                break;
            }
            variants.push(self.parse_variant());
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
        TypeDef {
            name,
            generics,
            kind: TypeDefKind::Enum(variants),
            span,
            attributes: Vec::new(),
        }
    }

    fn parse_variant(&mut self) -> Variant {
        let start = self.pos;
        let name = self.parse_ident();
        let fields = if self.at(TK::LParen) {
            self.bump();
            let mut fields = Vec::new();
            loop {
                if self.at(TK::RParen) {
                    break;
                }
                fields.push(self.parse_field());
                if self.at(TK::RParen) {
                    break;
                }
                self.expect(TK::Comma);
                if self.at(TK::RParen) {
                    break;
                }
            }
            self.expect(TK::RParen);
            fields
        } else {
            Vec::new()
        };
        Variant {
            name,
            fields,
            span: self.span_from(start),
            attributes: Vec::new(),
        }
    }

    /// Parse a brace-delimited list of struct/enum fields (the `{` `}` are
    /// consumed by the caller).
    pub fn parse_fields(&mut self) -> Vec<Field> {
        let mut fields = Vec::new();
        loop {
            if self.at(TK::RBrace) {
                break;
            }
            fields.push(self.parse_field());
            if self.at(TK::RBrace) {
                break;
            }
            self.expect(TK::Comma);
            if self.at(TK::RBrace) {
                break;
            }
        }
        fields
    }

    fn parse_field(&mut self) -> Field {
        let start = self.pos;
        let quantity = self.parse_quantity().unwrap_or(Quantity::Many);
        let name = self.parse_ident();
        self.expect(TK::Colon);
        let ty = self.parse_type();
        Field {
            name,
            ty,
            quantity,
            span: self.span_from(start),
            attributes: Vec::new(),
        }
    }

    fn parse_type_alias(&mut self) -> TypeDef {
        let start = self.pos;
        self.expect(TK::Type);
        let name = self.parse_ident();
        self.expect(TK::Assign);
        let ty = self.parse_type();
        self.eat(TK::Semicolon);
        let span = self.span_from(start);
        TypeDef {
            name,
            generics: Vec::new(),
            kind: TypeDefKind::Alias(ty),
            span,
            attributes: Vec::new(),
        }
    }

    // ===== Const, imports, modules =====

    fn parse_const(&mut self) -> ConstDef {
        let start = self.pos;
        self.expect(TK::Const);
        let name = self.parse_ident();
        let ty = if self.at(TK::Colon) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(TK::Assign);
        let value = self.parse_expr();
        self.eat(TK::Semicolon);
        let span = self.span_from(start);
        ConstDef {
            name,
            ty,
            value,
            span,
        }
    }

    fn parse_import(&mut self) -> Import {
        let start = self.pos;
        self.expect(TK::Import);
        let first = self.parse_ident();
        let mut path = vec![first];
        while self.eat(TK::Dot) {
            path.push(self.parse_ident());
        }
        let items = if self.at(TK::LBrace) {
            self.bump();
            let mut list = Vec::new();
            loop {
                if self.at(TK::RBrace) {
                    break;
                }
                list.push(self.parse_ident());
                if self.at(TK::RBrace) {
                    break;
                }
                self.expect(TK::Comma);
                if self.at(TK::RBrace) {
                    break;
                }
            }
            self.expect(TK::RBrace);
            ImportItems::Specific(list)
        } else {
            ImportItems::All
        };
        self.eat(TK::Semicolon);
        Import {
            path,
            items,
            span: self.span_from(start),
        }
    }

    fn parse_module(&mut self) -> Module {
        let start = self.pos;
        self.expect(TK::Mod);
        let name = self.parse_ident();
        let items = if self.at(TK::LBrace) {
            self.bump();
            let mut items = Vec::new();
            while !self.is_eof() && !self.at(TK::RBrace) {
                items.push(self.parse_item());
            }
            self.expect(TK::RBrace);
            items
        } else {
            self.eat(TK::Semicolon);
            Vec::new()
        };
        Module {
            name,
            items,
            span: self.span_from(start),
        }
    }
}

/// Convert a lexer token span into an AST [`Span`].
pub(crate) fn token_span(t: &Token) -> Span {
    Span::new(
        t.span.start as u32,
        t.span.end as u32,
        t.span.line,
        t.span.column,
    )
}

/// Parse a Naso source string into a [`Program`].
pub fn parse_program(source: &str) -> Result<Program, String> {
    let tokens = Lexer::lex(source)
        .into_iter()
        .filter(|t| !matches!(&t.kind, TK::Comment | TK::Newline))
        .collect::<Vec<_>>();
    let span = match (tokens.first(), tokens.last()) {
        (Some(f), Some(l)) => span_from_tokens(f, l),
        _ => Span::default(),
    };
    let mut parser = Parser::new(&tokens);
    let items = parser.parse_program_items();
    Ok(Program::new(items, span))
}

/// Parse a token stream (comments and newlines already filtered) into a
/// [`Program`].
pub fn parse_program_tokens(tokens: &[Token]) -> Result<Program, String> {
    let span = match (tokens.first(), tokens.last()) {
        (Some(f), Some(l)) => span_from_tokens(f, l),
        _ => Span::default(),
    };
    let mut parser = Parser::new(tokens);
    let items = parser.parse_program_items();
    Ok(Program::new(items, span))
}

fn span_from_tokens(first: &Token, last: &Token) -> Span {
    Span::new(
        first.span.start as u32,
        last.span.end as u32,
        first.span.line,
        first.span.column,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_program() {
        let prog = parse_program("").expect("parse failed");
        assert!(prog.items.is_empty());
    }

    #[test]
    fn parses_function_with_quantity_markers() {
        let prog =
            parse_program("fn f(x: [1] Qubit) -> Qubit { return x; }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        assert_eq!(func.params.len(), 1);
        assert_eq!(func.params[0].ty.quantity, Quantity::One);
    }

    #[test]
    fn parses_precedence_climbing() {
        let prog = parse_program("fn f() -> Int { return 1 + 2 * 3; }").expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        match &func.body.stmts[0].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Return(Some(inner)) => match &inner.kind {
                    ExprKind::Binary(BinOp::Add, _, rhs) => {
                        assert!(matches!(rhs.kind, ExprKind::Binary(BinOp::Mul, _, _)))
                    }
                    other => panic!("expected Add, got {other:?}"),
                },
                other => panic!("expected return, got {other:?}"),
            },
            other => panic!("expected expr stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_let_mut_binding() {
        let prog = parse_program(
            r#"
            fn f() {
                let x = 1;
                let mut sum = 0.0;
                sum = sum + 1.0;
            }
        "#,
        )
        .expect("parse failed");
        let func = match &prog.items[0] {
            Item::Function(f) => f,
            other => panic!("expected function, got {other:?}"),
        };
        // Check first let binding (immutable)
        match &func.body.stmts[0].kind {
            StmtKind::Let(LetStmt {
                pattern,
                mutability,
                ..
            }) => {
                // pattern should be Ident("x")
                if let PatternKind::Ident(ident) = &pattern.kind {
                    assert_eq!(ident.name, "x");
                } else {
                    panic!("expected ident pattern");
                }
                assert_eq!(*mutability, Mutability::Immutable);
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
        // Check second let binding (mut)
        match &func.body.stmts[1].kind {
            StmtKind::Let(LetStmt {
                pattern,
                mutability,
                ..
            }) => {
                // pattern should be Ident("sum")
                if let PatternKind::Ident(ident) = &pattern.kind {
                    assert_eq!(ident.name, "sum");
                } else {
                    panic!("expected ident pattern");
                }
                assert_eq!(*mutability, Mutability::Mut);
            }
            other => panic!("expected let mut stmt, got {other:?}"),
        }
        // Check assignment statement
        match &func.body.stmts[2].kind {
            StmtKind::Expr(e) => match &e.kind {
                ExprKind::Assign(lhs, _rhs) => {
                    assert!(matches!(lhs.kind, ExprKind::Var(_)));
                }
                other => panic!("expected assign, got {other:?}"),
            },
            other => panic!("expected expr stmt, got {other:?}"),
        }
    }
}
