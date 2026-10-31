//! Code Generation Error Types
//!
//! Comprehensive error taxonomy for codegen with diagnostic emission support.

use crate::ast::Span;
use std::path::PathBuf;
use thiserror::Error;

#[cfg(feature = "llvm")]
use inkwell::builder::BuilderError;

/// Result type for codegen operations
pub type CodegenResult<T> = Result<T, CodegenError>;

/// Errors that can occur during code generation
#[derive(Debug, Error)]
pub enum CodegenError {
    /// Target-related errors (invalid triple, unsupported target)
    #[error("Target error: {0}")]
    TargetError(String),

    /// Type lowering errors (unsupported type, quantity mismatch)
    #[error("Type lowering error: {0}")]
    TypeLoweringError(String),

    /// Module building errors (duplicate symbols, invalid IR)
    #[error("Module build error: {0}")]
    ModuleBuildError(String),

    /// Function building errors (signature mismatch, invalid body)
    #[error("Function build error: {0}")]
    FunctionBuildError(String),

    /// Instruction emission errors (invalid operands, type mismatch)
    #[error("Instruction emission error: {0}")]
    InstructionError(String),

    /// QIR-specific errors (invalid profile, missing intrinsics)
    #[error("QIR error: {0}")]
    QirError(String),

    /// Cranelift-specific errors
    #[error("Cranelift error: {0}")]
    CraneliftError(String),

    /// I/O errors during emission
    #[error("Emission error: {0}")]
    EmissionError(String),

    /// Verification errors (invalid IR after construction)
    #[error("Verification error: {0}")]
    VerificationError(String),

    /// Internal compiler errors (should not happen)
    #[error("Internal error: {0}")]
    InternalError(String),

    /// Unsupported feature for target
    #[error("Unsupported feature for target: {0}")]
    UnsupportedFeature(String),
}

#[cfg(feature = "llvm")]
impl From<BuilderError> for CodegenError {
    fn from(e: BuilderError) -> Self {
        CodegenError::InstructionError(e.to_string())
    }
}

impl CodegenError {
    /// Create a type lowering error with span
    pub fn type_lowering_with_span(msg: impl Into<String>, span: Span) -> Self {
        CodegenError::TypeLoweringError(format!("{} at {}:{}", msg.into(), span.line, span.column))
    }

    /// Create a module build error with span
    pub fn module_build_with_span(msg: impl Into<String>, span: Span) -> Self {
        CodegenError::ModuleBuildError(format!("{} at {}:{}", msg.into(), span.line, span.column))
    }

    /// Create an instruction error with span
    pub fn instruction_with_span(msg: impl Into<String>, span: Span) -> Self {
        CodegenError::InstructionError(format!("{} at {}:{}", msg.into(), span.line, span.column))
    }

    /// Create an unsupported feature error
    pub fn unsupported_feature(feature: impl Into<String>, target: impl Into<String>) -> Self {
        CodegenError::UnsupportedFeature(format!(
            "{} not supported on target {}",
            feature.into(),
            target.into()
        ))
    }
}

/// Trait for types that can emit diagnostics
pub trait Diagnostic {
    /// Emit the diagnostic to stderr
    fn emit(&self);

    /// Get the primary message
    fn message(&self) -> &str;

    /// Get optional span information
    fn span(&self) -> Option<Span>;

    /// Get optional source file path
    fn file(&self) -> Option<&PathBuf>;

    /// Get severity level
    fn severity(&self) -> Severity {
        Severity::Error
    }

    /// Get optional help text
    fn help(&self) -> Option<&str> {
        None
    }

    /// Get optional note text
    fn note(&self) -> Option<&str> {
        None
    }
}

/// Diagnostic severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Note,
    Warning,
    Error,
    Fatal,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Note => write!(f, "note"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
            Severity::Fatal => write!(f, "fatal"),
        }
    }
}

/// Trait for converting to diagnostic
pub trait IntoDiagnostic {
    fn into_diagnostic(self) -> Box<dyn Diagnostic>;
}

impl Diagnostic for CodegenError {
    fn emit(&self) {
        let severity = self.severity();
        eprintln!("{}: {}", severity, self.message());

        if let Some(span) = self.span() {
            eprintln!(
                "  --> {}:{}:{}",
                self.file()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                span.line,
                span.column
            );
        }

        if let Some(help) = self.help() {
            eprintln!("  = help: {}", help);
        }
        if let Some(note) = self.note() {
            eprintln!("  = note: {}", note);
        }
    }

    fn message(&self) -> &str {
        match self {
            CodegenError::TargetError(msg) => msg,
            CodegenError::TypeLoweringError(msg) => msg,
            CodegenError::ModuleBuildError(msg) => msg,
            CodegenError::FunctionBuildError(msg) => msg,
            CodegenError::InstructionError(msg) => msg,
            CodegenError::QirError(msg) => msg,
            CodegenError::CraneliftError(msg) => msg,
            CodegenError::EmissionError(msg) => msg,
            CodegenError::VerificationError(msg) => msg,
            CodegenError::InternalError(msg) => msg,
            CodegenError::UnsupportedFeature(msg) => msg,
        }
    }

    fn span(&self) -> Option<Span> {
        // Extract span from error message if embedded
        None
    }

    fn file(&self) -> Option<&PathBuf> {
        None
    }

    fn severity(&self) -> Severity {
        match self {
            CodegenError::InternalError(_) => Severity::Fatal,
            CodegenError::VerificationError(_) => Severity::Error,
            _ => Severity::Error,
        }
    }
}

/// Collection of diagnostics
#[allow(dead_code)]
pub struct DiagnosticEmitter {
    diagnostics: Vec<Box<dyn Diagnostic>>,
    source_map: Option<SourceMap>,
}

impl DiagnosticEmitter {
    pub fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            source_map: None,
        }
    }

    pub fn with_source_map(source_map: SourceMap) -> Self {
        Self {
            diagnostics: Vec::new(),
            source_map: Some(source_map),
        }
    }

    pub fn emit(&mut self, diagnostic: impl Diagnostic + 'static) {
        diagnostic.emit();
        self.diagnostics.push(Box::new(diagnostic));
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity() >= Severity::Error)
    }

    pub fn diagnostics(&self) -> &[Box<dyn Diagnostic>] {
        &self.diagnostics
    }
}

impl Default for DiagnosticEmitter {
    fn default() -> Self {
        Self::new()
    }
}

/// Source map for span-to-source mapping
#[derive(Debug, Clone)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    pub fn add_file(&mut self, path: PathBuf, content: String) -> usize {
        let id = self.files.len();
        self.files.push(SourceFile { path, content, id });
        id
    }

    pub fn get_file(&self, id: usize) -> Option<&SourceFile> {
        self.files.get(id)
    }

    pub fn span_to_location(&self, file_id: usize, span: Span) -> Option<SourceLocation> {
        let file = self.files.get(file_id)?;
        let content = &file.content;
        let start_byte = span.start as usize;
        let end_byte = span.end as usize;

        let line_start = content[..start_byte]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let line_end = content[..end_byte].rfind('\n').map(|i| i + 1).unwrap_or(0);

        let column_start = start_byte - line_start;
        let column_end = end_byte - line_end;

        Some(SourceLocation {
            file_id,
            line_start: span.line,
            column_start: column_start as u32,
            line_end: span.line,
            column_end: column_end as u32,
        })
    }
}

impl Default for SourceMap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub content: String,
    pub id: usize,
}

#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub file_id: usize,
    pub line_start: u32,
    pub column_start: u32,
    pub line_end: u32,
    pub column_end: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codegen_error_display() {
        let err = CodegenError::TypeLoweringError("invalid type".to_string());
        assert!(err.to_string().contains("Type lowering error"));
    }

    #[test]
    fn test_codegen_error_diagnostic() {
        let err = CodegenError::TypeLoweringError("test error".to_string());
        assert_eq!(err.message(), "test error");
        assert_eq!(err.severity(), Severity::Error);
    }

    #[test]
    fn test_diagnostic_emitter() {
        let mut emitter = DiagnosticEmitter::new();
        let err = CodegenError::TypeLoweringError("test".to_string());
        emitter.emit(err);
        assert!(emitter.has_errors());
    }

    #[test]
    fn test_source_map() {
        let mut map = SourceMap::new();
        let id = map.add_file("test.naso".into(), "fn main() {}\n".into());
        assert_eq!(id, 0);
    }
}
