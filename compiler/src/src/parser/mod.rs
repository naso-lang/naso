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

    /// The first syntax error, once one has been recorded.
    ///
    /// Sticky on purpose. The parser is a recursive-descent loop over a token
    /// stream with no backtracking, so after a syntax error the remaining tokens
    /// have no coherent interpretation -- continuing would produce cascades of
    /// nonsense diagnostics from the one real mistake. Recording the FIRST error
    /// and stopping is both simpler and more useful to a user.
    ///
    /// `is_eof` reports true whenever this is set, which is what stops the loops.
    error: Option<ParseError>,
}

/// A syntax error, with the span needed to point at it.
///
/// The parser used to `panic!` for every input it did not expect -- about 90 sites.
/// That is survivable for a CLI, and fatal for a browser: an `unwind` in
/// WebAssembly raises a trap that `catch_unwind` cannot intercept (that target has
/// no unwinder), so a user typing `fn f( {` into a playground would not get an error
/// message, they would lose the whole compiler module.
///
/// This is the honest replacement: a value, returned through `Result`, with a span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Human-readable description, e.g. "expected `)`, found `{`".
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// A value the parser can return when it has already recorded an error.
///
/// Once `Parser::error` is set, parsing STOPS: `is_eof` reports end-of-input, so
/// every loop in the parser halts at its next condition check and the partial AST is
/// discarded by `parse_program`. These sentinels are therefore never read as real
/// values -- they exist only so the parser can unwind through ordinary `return`
/// statements instead of threading `Result` through every function signature.
///
/// Returning a wrong value here would be a lie only if it could be observed. It
/// cannot: `parse_program` returns `Err` whenever `error` is set, so no caller ever
/// sees the tree. `sentinel_invalid_value_reached` pins that.
pub trait Sentinel {
    fn sentinel(span: Span) -> Self;
}

impl Sentinel for Token {
    fn sentinel(span: Span) -> Self {
        Token {
            // Any kind works: the token is never inspected, because once an error
            // is recorded `is_eof` short-circuits and `parse_program` returns Err.
            kind: TK::Error,
            span: crate::lexer::TokenSpan {
                start: span.start as usize,
                end: span.end as usize,
                line: span.line,
                column: span.column,
            },
        }
    }
}

impl Sentinel for Ident {
    fn sentinel(span: Span) -> Self {
        Ident::new("<error>", span)
    }
}

impl Sentinel for bool {
    fn sentinel(_span: Span) -> Self {
        false
    }
}

/// Sentinels deliberately reuse EXISTING variants rather than adding an `Error`
/// variant to `ExprKind` / `PatternKind` / `TypeKind`.
///
/// An `Error` variant would be a better-looking sentinel, and the wrong trade: it
/// adds a case to enums matched exhaustively in ~28 places across typechecking,
/// lowering and both codegen backends, none of which can ever see it. Every one of
/// those matches would need an arm for an unreachable state, or -- worse -- a
/// catch-all that silently maps it to some real behaviour.
///
/// `Var("<error>")` is indistinguishable from a real expression only if you
/// somehow obtain the discarded tree, which `parse_program` makes impossible. The
/// alternative spreads a phantom case through the whole compiler.
impl Sentinel for Expr {
    fn sentinel(span: Span) -> Self {
        Expr::new(
            ExprKind::Var(Ident::new("<error>", span)),
            span,
            NodeId::new(0),
        )
    }
}

impl Sentinel for Pattern {
    fn sentinel(span: Span) -> Self {
        Pattern::new(
            PatternKind::Ident(Ident::new("<error>", span)),
            span,
            NodeId::new(0),
        )
    }
}

impl Sentinel for Type {
    fn sentinel(span: Span) -> Self {
        Type::new(
            TypeKind::Named(Ident::new("<error>", span), Vec::new()),
            Quantity::Many,
            span,
        )
    }
}

impl Sentinel for Item {
    fn sentinel(span: Span) -> Self {
        Item::Const(ConstDef {
            name: Ident::new("<error>", span),
            ty: None,
            value: Expr::sentinel(span),
            span,
        })
    }
}

impl Sentinel for GenericKind {
    fn sentinel(_span: Span) -> Self {
        GenericKind::Type
    }
}

impl Sentinel for () {
    fn sentinel(_span: Span) -> Self {}
}

impl<'a> Parser<'a> {
    /// Create a parser over `tokens`.
    pub fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            no_block_expr: false,
            error: None,
        }
    }

    /// Record the first syntax error, and return a sentinel to unwind through.
    ///
    /// Keeps only the first: a user fixing `fn f( {` should see that, not the
    /// twenty follow-on complaints from parsing `{` as an item.
    fn fail<T: Sentinel>(&mut self, message: String, span: Span) -> T {
        if self.error.is_none() {
            self.error = Some(ParseError { message, span });
        }
        T::sentinel(span)
    }

    /// Describe the token at the cursor, or end of input.
    fn found_description(&self) -> String {
        match self.peek_token() {
            Some(t) => format!("`{}`", t.kind),
            None => "end of input".to_string(),
        }
    }

    /// The span to blame for an error at the cursor.
    fn error_span(&self) -> Span {
        match self.peek_token() {
            Some(t) => Span::new(
                t.span.start as u32,
                t.span.end as u32,
                t.span.line,
                t.span.column,
            ),
            None => Span::default(),
        }
    }

    /// Record an error: "expected `expected`, found `found`".
    fn error_expected(&mut self, expected: &str) -> ParseError {
        ParseError {
            message: format!("expected {expected}, found {}", self.found_description()),
            span: self.error_span(),
        }
    }

    /// The recorded error, if any. Read by `parse_program`.
    pub fn error(&self) -> Option<&ParseError> {
        self.error.as_ref()
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
        // Once an error is recorded, report end-of-input. Every `while !is_eof()`
        // and every `match self.peek()` in the parser then takes its
        // end-of-input path, so the recursive descent unwinds immediately instead
        // of chewing through the rest of a malformed program.
        if self.error.is_some() {
            return true;
        }
        self.pos >= self.tokens.len()
    }

    /// Look at the current token kind without consuming it.
    /// The token kind at the cursor.
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

    /// Consume the current token if its kind matches `kind`, otherwise record a
    /// syntax error describing what was expected.
    ///
    /// The sentinel token is never consumed by anything: once an error is set,
    /// `is_eof` short-circuits and `parse_program` returns `Err`.
    pub fn expect(&mut self, kind: TK) -> Token {
        match self.peek() {
            Some(k) if *k == kind => match self.bump() {
                Some(t) => t,
                // `peek` returned Some, so `bump` cannot return None unless the
                // token stream is empty, which the guard above already excludes.
                None => self.fail(
                    format!("expected `{}`, found end of input", kind.display_name()),
                    self.error_span(),
                ),
            },
            _ => {
                let err = self.error_expected(&format!("`{}`", kind.display_name()));
                self.fail(err.message, err.span)
            }
        }
    }

    /// Consume the current token if its kind matches `kind`, returning whether
    /// it did.
    ///
    /// Returns false once an error is recorded, so `while self.eat(..)` halts.
    pub fn eat(&mut self, kind: TK) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Whether the parser has already recorded a syntax error.
    ///
    /// Exists for the `loop { ... }` sites whose exit condition is a `match
    /// self.peek()` arm. Those loops call `at()`, which is already
    /// error-aware -- but an `at()`-only loop cannot distinguish "error" from
    /// "end of input", so with every predicate false it spins forever consuming
    /// nothing. Each such loop needs this check; see `loop_until_error`.
    pub fn has_errored(&self) -> bool {
        self.error.is_some()
    }

    /// The `loop { ... }` exit test: stop when the cursor no longer matches.
    ///
    /// Every loop that advances the cursor must break on this, not only on a
    /// specific token. The alternative -- trusting each loop to check for an error
    /// -- is how `parse_type_args` came to spin forever on `Tensor[1,` : with the
    /// error recorded, `at(TK::RBracket)` is false and `expect(TK::Comma)` consumes
    /// nothing, so the loop made no progress and never terminated.
    ///
    /// A `loop` becomes `while !self.loop_should_stop()`.
    pub fn loop_should_stop(&self) -> bool {
        self.error.is_some() || self.pos >= self.tokens.len()
    }

    /// Whether the current token kind is `kind`, without consuming it.
    pub fn at(&self, kind: TK) -> bool {
        self.peek().map(|k| *k == kind).unwrap_or(false)
    }

    /// Whether the current token is an identifier spelled `name`.
    fn at_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Some(TK::Ident(s)) if s == name)
    }

    /// Record a syntax error describing the token found at the cursor.
    fn unexpected<T: Sentinel>(&mut self, expected: &str) -> T {
        let err = self.error_expected(expected);
        self.fail(err.message, err.span)
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
                    let msg = format!("expected identifier, found `{other}`");
                    return self.fail(msg, self.span_from(start));
                }
            },
            None => {
                let msg = "expected identifier, found end of input".to_string();
                return self.fail(msg, self.span_from(start));
            }
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

        // `requires { .. }` sits between the signature and the body. It is parsed as an
        // ordinary block and then reduced to its `assert(..)` calls, so a precondition is
        // written exactly like an obligation and cannot silently mean something else.
        let requires = if self.at(TK::Requires) {
            self.parse_requires_block()
        } else {
            Vec::new()
        };
        debug_log(&format!("parse_fn: after requires, peek={:?}", self.peek()));

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
            requires,
        }
    }

    /// Parse a list of comma-separated parameters, stopping at the closing
    /// `)` which the caller consumes.
    pub fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        while !self.loop_should_stop() {
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
        while !self.loop_should_stop() {
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
        while !self.loop_should_stop() {
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
            while !self.loop_should_stop() {
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
        while !self.loop_should_stop() {
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
            while !self.loop_should_stop() {
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
    // The partial AST is discarded on error. The sentinel values the parser
    // returned while unwinding are never observable, because nothing else can
    // reach them once this returns Err.
    match parser.error() {
        Some(e) => Err(e.to_string()),
        None => Ok(Program::new(items, span)),
    }
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
    match parser.error() {
        Some(e) => Err(e.to_string()),
        None => Ok(Program::new(items, span)),
    }
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

    // ===== Syntax errors are VALUES, not aborts =====
    //
    // The parser used to `panic!` on unexpected input (~90 sites). In WebAssembly
    // that is a TRAP, not a catchable unwind: the target has no unwinder, so a user
    // typing `fn f( {` into a playground lost the whole compiler module. These assert
    // the replacement: a `Result` carrying a message and a span.

    /// Malformed input returns `Err` with a message and a span.
    #[test]
    fn malformed_input_is_an_err_not_a_panic() {
        for (src, needle) in [
            ("fn f( {", "identifier"),
            ("fn", "identifier"),
            ("fn f() {", "expected `'}'`"),
            ("fn f() { let", "pattern"),
            ("fn f(x: ) {}", "type"),
            ("fn f() -> {", "type"),
            ("fn f() { let a = }", "expression"),
            ("struct", "identifier"),
            ("let x = 1;", "top-level item"),
            ("}}}", "top-level item"),
            ("@@@", "top-level item"),
        ] {
            let err = parse_program(src).expect_err(&format!(
                "`{src}` must not parse, but must not abort either"
            ));
            assert!(
                err.contains(needle),
                "`{src}`: expected a message naming `{needle}`, got: {err}"
            );
        }
    }

    /// The error names the token it actually found.
    ///
    /// Without this a user cannot tell WHICH of twenty mistakes they made, and a
    /// message that silently omitted the found-token would still pass the check
    /// above.
    #[test]
    fn the_error_names_the_offending_token() {
        let err = parse_program("fn f(x: ) {}").expect_err("must not parse");
        // `found ` + backtick + `)` + backtick -- matched loosely, so the assertion
        // is about NAMING the token, not about the exact quoting style.
        assert!(
            err.contains("found") && err.contains(")"),
            "must name the token found so the user knows where to look: {err}"
        );
    }

    /// No malformed input may loop forever.
    ///
    /// This is the regression that matters most and is the least obvious. Replacing
    /// `panic!` with "record an error and return a sentinel" makes `at()` and
    /// `peek()` report "nothing here" -- but a `loop { }` whose exit test is only
    /// `at(TK::RBracket)` then cannot tell "error" from "not this token", so every
    /// predicate is false, `expect` consumes nothing, and the loop spins.
    ///
    /// It did. `naso check` on `Tensor[1,` was killed by the OOM killer (exit 137)
    /// rather than returning an error, which is strictly worse than the panic it
    /// replaced. Hence `loop_should_stop` on every cursor-advancing loop, and hence
    /// this test: each case is a shape that hung.
    #[test]
    fn no_malformed_input_loops_forever() {
        // These all spun at some point, or are the shapes that made them spin:
        // unterminated lists, a bare keyword, and a delimiter with nothing after it.
        for src in [
            "fn f( {",
            "fn",
            "struct",
            "enum",
            "mod",
            "import",
            "const",
            "type",
            "fn f(x: ) {}",
            "fn f(x: Tensor[1, ) {}",
            "fn f(x: Tensor[) {}",
            "fn f() -> {",
            "fn f() { let",
            "fn f() { let a = ",
            "fn f() { let a: ",
            "fn f() { for i in ",
            "fn f() { for i in 0.. ",
            "fn f() { match x {",
            "fn f() { (1,",
            "fn f() { [1,",
            "fn f() { a.",
            "fn f() { x[",
            "fn f(a: [1] , ) {}",
            "fn f[nat N]() {}",
            "fn f[ ]() {}",
            "fn f() { let a = if }",
            "}}",
            "()",
            "",
            "   ",
            "fn f() { return }",
        ] {
            // Must terminate and must not panic.
            let result = std::panic::catch_unwind(|| parse_program(src).is_ok());
            assert!(
                result.is_ok(),
                "`{src}` unwound: a panic in wasm is a trap that kills the module"
            );
        }
    }

    /// Valid programs still parse.
    ///
    /// The counterweight to the two tests above. Making `at()` and `peek()`
    /// error-aware touched the hot path of every loop in the parser, so "it no
    /// longer panics" is only half the claim; the other half is that valid input is
    /// unaffected.
    #[test]
    fn valid_programs_still_parse() {
        for src in [
            "fn f() {}",
            "fn f(x: [1] i32) -> i32 { return x; }",
            "fn f() { forall i in 0..10 { let x = i; } }",
            "fn f(x: Tensor[f32, 1024]) { forall i in 0..1024 { let v = x[i]; } }",
            "fn f(a: [*] Tensor[f32, 8], b: inout [1] Tensor[f32, 8]) { forall i in 0..8 { b[i] = a[i]; } }",
            "fn f() { let (a, b) = (1, 2); }",
            "fn f(x: (i32, i32)) { let (a, _) = x; }",
            "struct S { a: i32, b: f32 }",
            "enum E { A, B }",
            "enum E { A, B(x: i32) }",
            "mod m { fn inner() {} }",
            "import foo.bar;",
            "import foo.bar { baz, qux };",
            "const K: i32 = 1;",
            "type Alias = i32;",
            "fn f() { if true { let x = 1; } else { let y = 2; } }",
            "fn f() { match 1 { 1 => { let x = 1; } _ => { let y = 2; } } }",
            "fn f(q: Qubit) { hadamard(q); reset(q); }",
            "fn f() { let x = [1, 2, 3]; }",
            "fn f[T: type](x: T) -> T { return x; }",
            "fn f[N: nat]() {}",
        ] {
            parse_program(src).unwrap_or_else(|e| panic!("`{src}` should parse: {e}"));
        }
    }

    /// An empty program is valid, and an empty ERROR is not.
    ///
    /// `parse_program("")` must be `Ok` with no items. If error detection were
    /// implemented as "ran out of tokens", an empty file would become a syntax
    /// error and every new project would need a dummy declaration to parse.
    #[test]
    fn an_empty_program_is_not_an_error() {
        let prog = parse_program("").expect("an empty program is valid");
        assert!(prog.items.is_empty());
    }

    /// Only the FIRST error is reported.
    ///
    /// The parser is recursive descent with no backtracking, so after a syntax error
    /// the remaining tokens have no coherent interpretation. Reporting all of them
    /// would bury the one real mistake under a cascade.
    #[test]
    fn only_the_first_error_is_reported() {
        let err = parse_program("fn f(x: ) { let y: = 1; let z: = 2; }").expect_err("must fail");
        assert_eq!(
            err.matches("expected ").count(),
            1,
            "exactly one error must be reported, not a cascade: {err}"
        );
    }

    /// A valid program records no error, which is what `parse_program` relies on to
    /// decide between `Ok` and `Err`.
    ///
    /// `parse_program` returns `Err` iff `Parser::error` is set. If a path recorded
    /// an error for input that actually parses, the program would be silently
    /// rejected -- a false positive in the other direction, and one that would not
    /// show up as a crash.
    #[test]
    fn a_parsed_program_has_no_recorded_error() {
        for src in ["fn f() {}", "fn f(x: [1] i32) -> i32 { return x; }"] {
            let tokens = Lexer::lex(src)
                .into_iter()
                .filter(|t| !matches!(&t.kind, TK::Comment | TK::Newline))
                .collect::<Vec<_>>();
            let mut p = Parser::new(&tokens);
            let _ = p.parse_program_items();
            assert!(
                p.error().is_none(),
                "`{src}` parsed but recorded: {:?}",
                p.error()
            );
        }
    }
}
