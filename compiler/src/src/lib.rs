//! Naso Compiler library crate.
//!
//! Exposes the lexer, parser, typechecker, IR, lowering, AST, codegen, and runtime modules
//! used by the `naso` binary and by integration tests.

pub mod ast;
pub mod codegen;
pub mod ir;
pub mod lexer;
pub mod lowering;
pub mod parser;
pub mod runtime;
pub mod typecheck;

pub use ast::Program;
pub use ast::*;
