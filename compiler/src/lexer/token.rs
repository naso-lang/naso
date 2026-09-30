//! Token definitions for the Naso lexer.
//!
//! The lexer is built with [`logos`]. Keywords are matched case-insensitively
//! by declaring each keyword as an explicit per-character class regex
//! (e.g. `[fF][nN]`), given a higher `priority` than the generic identifier
//! rules so that exact keyword spellings win on equal-length matches while
//! longer identifiers keep their full extent. Quantity markers `[0]`, `[1]`,
//! `[N]` are produced as `[`, literal/ident, `]` token sequences (the parser
//! composes them into [`crate::ast::Quantity`] so that type-argument brackets
//! like `QRegister[Cols]` stay unambiguous). `[*]` is lexed as a single
//! dedicated token.

use logos::Logos;
use std::fmt;

/// A single lexical token produced by the [`super::Lexer`].
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: TokenSpan,
}

/// Location of a token within the source.
///
/// `start`/`end` are byte offsets; `line`/`column` are 1-based and describe
/// the start of the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSpan {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
}

impl TokenSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            line: 1,
            column: 1,
        }
    }

    pub fn with_position(start: usize, end: usize, line: u32, column: u32) -> Self {
        Self {
            start,
            end,
            line,
            column,
        }
    }

    pub fn merge(self, other: TokenSpan) -> TokenSpan {
        TokenSpan {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            line: self.line.min(other.line),
            column: self.column.min(other.column),
        }
    }
}

/// The kinds of lexical tokens: quantity markers, keywords, quantum keywords,
/// operators, punctuation, identifiers, and literals.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r]+")]
pub enum TokenKind {
    // ===== Comments & newlines =====
    #[regex(r"//[^\n]*")]
    Comment,
    #[regex(r"\n")]
    Newline,

    // ===== Quantity marker [*] =====
    #[regex(r"\[\*\]")]
    QtyStar,

    // ===== Keywords =====
    // Case-insensitive (per-character classes) with `priority = 3` so they
    // beat the `priority = 2` identifier rules on exact-length matches.
    #[regex("[fF][nN]", priority = 3)]
    Fn,
    #[regex("[lL][eE][tT]", priority = 3)]
    Let,
    #[regex("[iI][nN][oO][uU][tT]", priority = 3)]
    InOut,
    #[regex("[cC][oO][nN][sS][uU][mM][eE]", priority = 3)]
    Consume,
    #[regex("[mM][uU][tT]", priority = 3)]
    Mut,
    #[regex("[rR][eE][vV][eE][rR][sS][iI][bB][lL][eE]", priority = 3)]
    Reversible,
    #[regex("[rR][eE][tT][uU][rR][nN]", priority = 3)]
    Return,
    #[regex("[iI][fF]", priority = 3)]
    If,
    #[regex("[eE][lL][sS][eE]", priority = 3)]
    Else,
    #[regex("[mM][aA][tT][cC][hH]", priority = 3)]
    Match,
    #[regex("[fF][oO][rR]", priority = 3)]
    For,
    #[regex("[wW][hH][iI][lL][eE]", priority = 3)]
    While,
    #[regex("[fF][oO][rR][aA][lL][lL]", priority = 3)]
    Forall,
    #[regex("[iI][nN]", priority = 3)]
    In,
    #[regex("[sS][tT][rR][uU][cC][tT]", priority = 3)]
    Struct,
    #[regex("[eE][nN][uU][mM]", priority = 3)]
    Enum,
    #[regex("[tT][yY][pP][eE]", priority = 3)]
    Type,
    #[regex("[mM][oO][dD]", priority = 3)]
    Mod,
    #[regex("[iI][mM][pP][oO][rR][tT]", priority = 3)]
    Import,
    #[regex("[cC][oO][nN][sS][tT]", priority = 3)]
    Const,
    // `as` introduces a numeric cast: `expr as i8`. Numeric types only --
    // the typechecker rejects casts to tensor, qubit or pointer-shaped types,
    // so this is not a general reinterpret-cast operator.
    #[regex("[aA][sS]", priority = 3)]
    As,
    #[regex("[iI]8", priority = 3)]
    Int8,
    #[regex("[iI]16", priority = 3)]
    Int16,
    #[regex("[iI]32", priority = 3)]
    Int32,
    #[regex("[iI]64", priority = 3)]
    Int64,
    #[regex("[iI][sS][iI][zZ][eE]", priority = 3)]
    ISize,
    #[regex("[uU]8", priority = 3)]
    UInt8,
    #[regex("[uU]16", priority = 3)]
    UInt16,
    #[regex("[uU]32", priority = 3)]
    UInt32,
    #[regex("[uU]64", priority = 3)]
    UInt64,
    #[regex("[uU][sS][iI][zZ][eE]", priority = 3)]
    USize,
    #[regex("[fF]32", priority = 3)]
    Float32,
    #[regex("[fF]64", priority = 3)]
    Float64,
    #[regex("[qQ][uU][bB][iI][tT]", priority = 3)]
    Qubit,
    #[regex("[qQ][rR][eE][gG][iI][sS][tT][eE][rR]", priority = 3)]
    QRegister,
    #[regex("[mM][eE][aA][sS][uU][rR][eE]", priority = 3)]
    Measure,
    #[regex("[gG][aA][tT][eE]", priority = 3)]
    Gate,
    #[regex("[eE][nN][tT][aA][nN][gG][lL][eE]", priority = 3)]
    Entangle,
    #[regex("[hH][aA][dD][aA][mM][aA][rR][dD]", priority = 3)]
    Hadamard,
    #[regex("[cC][nN][oO][tT]", priority = 3)]
    CNot,
    #[regex("[qQ][aA][lL][lL][oO][cC]", priority = 3)]
    QAlloc,
    #[regex("[nN][aA][tT]", priority = 3)]
    Nat,
    #[regex("[fF][lL][oO][aA][tT]", priority = 3)]
    FloatKw,
    #[regex("[iI][nN][tT]", priority = 3)]
    IntKw,
    #[regex("[tT][rR][uU][eE]", |lex| Some(lex.slice().eq_ignore_ascii_case("true")), priority = 3)]
    #[regex("[fF][aA][lL][sS][eE]", |lex| Some(lex.slice().eq_ignore_ascii_case("true")), priority = 3)]
    Bool(bool),
    #[regex("[tT][eE][nN][sS][oO][rR]", priority = 3)]
    Tensor,

    // ===== Literals =====
    #[regex(r"[0-9][0-9_]*", |lex| parse_int(lex.slice()))]
    Int(i64),
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*((e|E)[+-]?[0-9]+)?", |lex| parse_float(lex.slice()))]
    #[regex(r"\.[0-9][0-9_]*((e|E)[+-]?[0-9]+)?", |lex| parse_float(lex.slice()))]
    Float(f64),
    #[regex(r#""([^"\\\n]|\\.)*""#, |lex| parse_string(lex.slice()))]
    Str(String),
    #[regex(r"'([^'\\\n]|\\.)'", |lex| parse_char(lex.slice()))]
    Char(char),

    // ===== Identifiers =====
    #[regex(r"[A-Z][A-Za-z0-9_]*", |lex| lex.slice().to_string())]
    TypeIdent(String),
    #[regex(r"[a-z_][A-Za-z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    // ===== Operators =====
    #[token("!")]
    Not,
    #[token("==")]
    Eq,
    #[token("!=")]
    Ne,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("&&")]
    AndAnd,
    #[token("||")]
    OrOr,
    #[token("<<")]
    Shl,
    #[token(">>")]
    Shr,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("..")]
    DotDot,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("=")]
    Assign,
    #[token("@")]
    At,

    // ===== Punctuation =====
    #[token(":")]
    Colon,
    #[token(";")]
    Semicolon,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,

    // ===== Error (unrecognized character) =====
    // In logos 0.13 lexical errors are surfaced through `Result::Err` from the
    // lexer iterator; this variant is produced manually by `crate::lexer`.
    Error,
}

impl TokenKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            TokenKind::Fn => "'fn'",
            TokenKind::Let => "'let'",
            TokenKind::InOut => "'inout'",
            TokenKind::Consume => "'consume'",
            TokenKind::Reversible => "'reversible'",
            TokenKind::Return => "'return'",
            TokenKind::If => "'if'",
            TokenKind::Else => "'else'",
            TokenKind::Match => "'match'",
            TokenKind::For => "'for'",
            TokenKind::While => "'while'",
            TokenKind::Forall => "'forall'",
            TokenKind::In => "'in'",
            TokenKind::Struct => "'struct'",
            TokenKind::Enum => "'enum'",
            TokenKind::Type => "'type'",
            TokenKind::Mod => "'mod'",
            TokenKind::Import => "'import'",
            TokenKind::Const => "'const'",
            TokenKind::As => "'as'",
            TokenKind::Mut => "'mut'",
            TokenKind::Int8 => "'i8'",
            TokenKind::Int16 => "'i16'",
            TokenKind::Int32 => "'i32'",
            TokenKind::Int64 => "'i64'",
            TokenKind::ISize => "'isize'",
            TokenKind::UInt8 => "'u8'",
            TokenKind::UInt16 => "'u16'",
            TokenKind::UInt32 => "'u32'",
            TokenKind::UInt64 => "'u64'",
            TokenKind::USize => "'usize'",
            TokenKind::Float32 => "'f32'",
            TokenKind::Float64 => "'f64'",
            TokenKind::Qubit => "'qubit'",
            TokenKind::QRegister => "'qregister'",
            TokenKind::Measure => "'measure'",
            TokenKind::Gate => "'gate'",
            TokenKind::Entangle => "'entangle'",
            TokenKind::Hadamard => "'hadamard'",
            TokenKind::CNot => "'cnot'",
            TokenKind::QAlloc => "'qalloc'",
            TokenKind::Nat => "'nat'",
            TokenKind::FloatKw => "'float'",
            TokenKind::Tensor => "'tensor'",
            TokenKind::QtyStar => "'[*]'",
            TokenKind::Int(_) => "integer literal",
            TokenKind::Float(_) => "float literal",
            TokenKind::Bool(_) => "boolean literal",
            TokenKind::Str(_) => "string literal",
            TokenKind::Char(_) => "char literal",
            TokenKind::Ident(_) => "identifier",
            TokenKind::TypeIdent(_) => "type name",
            TokenKind::Plus => "'+'",
            TokenKind::Not => "'!'",
            TokenKind::Minus => "'-'",
            TokenKind::Star => "'*'",
            TokenKind::Slash => "'/'",
            TokenKind::Percent => "'%'",
            TokenKind::Eq => "'=='",
            TokenKind::Ne => "'!='",
            TokenKind::Lt => "'<'",
            TokenKind::Le => "'<='",
            TokenKind::Gt => "'>'",
            TokenKind::Ge => "'>='",
            TokenKind::AndAnd => "'&&'",
            TokenKind::OrOr => "'||'",
            TokenKind::Amp => "'&'",
            TokenKind::Pipe => "'|'",
            TokenKind::Caret => "'^'",
            TokenKind::Shl => "'<<'",
            TokenKind::Shr => "'>>'",
            TokenKind::Assign => "'='",
            TokenKind::Arrow => "'->'",
            TokenKind::FatArrow => "'=>'",
            TokenKind::Colon => "':'",
            TokenKind::Semicolon => "';'",
            TokenKind::Comma => "','",
            TokenKind::Dot => "'.'",
            TokenKind::DotDot => "'..'",
            TokenKind::LParen => "'('",
            TokenKind::RParen => "')'",
            TokenKind::LBracket => "'['",
            TokenKind::RBracket => "']'",
            TokenKind::LBrace => "'{'",
            TokenKind::RBrace => "'}'",
            TokenKind::At => "'@'",
            TokenKind::Newline => "newline",
            TokenKind::Comment => "comment",
            TokenKind::IntKw => "'int'",
            TokenKind::Error => "<lex error>",
        }
    }
}

/// Whether a token is pure trivia that can be skipped between grammar tokens.
pub fn is_trivia(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Newline | TokenKind::Comment)
}

fn parse_int(s: &str) -> i64 {
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    cleaned.parse().unwrap_or(0)
}

fn parse_float(s: &str) -> f64 {
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    cleaned.parse().unwrap_or(0.0)
}

fn parse_string(s: &str) -> String {
    let inner = &s[1..s.len() - 1];
    unescape(inner)
}

fn parse_char(s: &str) -> char {
    let inner = &s[1..s.len() - 1];
    unescape(inner).chars().next().unwrap_or('\0')
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('"') => out.push('"'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Int(v) => write!(f, "{}", v),
            TokenKind::Float(v) => write!(f, "{}", v),
            TokenKind::Bool(v) => write!(f, "{}", v),
            TokenKind::Str(v) => write!(f, "\"{}\"", v),
            TokenKind::Char(v) => write!(f, "'{}'", v),
            TokenKind::Ident(s) | TokenKind::TypeIdent(s) => write!(f, "{}", s),
            _ => write!(f, "{}", self.display_name()),
        }
    }
}
