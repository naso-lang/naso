//! Lexer for the Naso language.
//!
//! The token definitions live in [`token`], implemented with [`logos`]. This
//! module wraps them in a [`Lexer`] that produces a flat [`Vec<Token>`] and
//! tracks line/column information for diagnostics.

#![allow(clippy::approx_constant)]

pub mod token;

use logos::Logos;
pub use token::{Token, TokenKind, TokenSpan};

/// A lexical error with the byte offset at which the lexer failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lex error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for LexError {}

/// The result of lexing a source string.
#[derive(Debug, Clone, PartialEq)]
pub struct LexResult {
    /// Tokens, including `Newline` and `Comment` markers.
    pub tokens: Vec<Token>,
    /// The first lexical error encountered, if any.
    pub error: Option<LexError>,
}

/// A wrapper around the [`logos`] lexer that yields all tokens up front and
/// tracks newlines.
pub struct Lexer;

impl Lexer {
    /// Lex `source`, returning all tokens (comment and newline tokens included).
    pub fn lex(source: &str) -> Vec<Token> {
        Self::lex_with_error(source).tokens
    }

    /// Lex `source`, returning the tokens and the first error encountered.
    pub fn lex_with_error(source: &str) -> LexResult {
        let mut tokens = Vec::new();
        let mut error = None;

        let mut lex = TokenKind::lexer(source);
        while let Some(result) = lex.next() {
            let span = TokenSpan::new(lex.span().start, lex.span().end);
            match result {
                Ok(kind) => {
                    tokens.push(Token { kind, span });
                }
                Err(_) => {
                    if error.is_none() {
                        let ch = source[lex.span().clone()].chars().next().unwrap_or(' ');
                        error = Some(LexError {
                            offset: lex.span().start,
                            message: format!("unexpected character {:?}", ch),
                        });
                    }
                    // Surface an explicit error token and stop lexing.
                    tokens.push(Token {
                        kind: TokenKind::Error,
                        span,
                    });
                    break;
                }
            }
        }

        LexResult { tokens, error }
    }

    /// Lex `source`, skipping comments and newlines. Convenient for tests.
    pub fn tokenize(source: &str) -> Vec<TokenKind> {
        Lexer::lex(source)
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Comment | TokenKind::Newline))
            .map(|t| t.kind)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_quantity_markers() {
        let kinds = Lexer::tokenize("[0] [1] [*] [N]");
        assert_eq!(
            kinds,
            vec![
                TokenKind::LBracket,
                TokenKind::Int(0),
                TokenKind::RBracket,
                TokenKind::LBracket,
                TokenKind::Int(1),
                TokenKind::RBracket,
                TokenKind::QtyStar,
                TokenKind::LBracket,
                TokenKind::TypeIdent("N".to_string()),
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn lexes_keywords_and_quantum_keywords() {
        let kinds = Lexer::tokenize("fn let inout consume reversible qubit measure entangle");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Fn,
                TokenKind::Let,
                TokenKind::InOut,
                TokenKind::Consume,
                TokenKind::Reversible,
                TokenKind::Qubit,
                TokenKind::Measure,
                TokenKind::Entangle,
            ]
        );
    }

    #[test]
    fn lexes_qalloc() {
        let kinds = Lexer::tokenize("qalloc()");
        assert_eq!(
            kinds,
            vec![TokenKind::QAlloc, TokenKind::LParen, TokenKind::RParen,]
        );
    }

    #[test]
    fn lexes_literals_and_operators() {
        let kinds = Lexer::tokenize("42 3.14 true \"hi\" 'a' + - => == <=");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Int(42),
                TokenKind::Float(3.14),
                TokenKind::Bool(true),
                TokenKind::Str("hi".to_string()),
                TokenKind::Char('a'),
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::FatArrow,
                TokenKind::Eq,
                TokenKind::Le,
            ]
        );
    }
}
