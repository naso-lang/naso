//! Model extraction and unsat core handling for Z3.
//!
//! This module provides model extraction and unsat core handling for the
//! Z3 solver backend. All Z3-dependent types are gated behind the `z3` feature.

#![allow(unused_imports)]

use naso_compiler::ast::Span;
#[cfg(feature = "z3")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "z3")]
use std::collections::HashMap;
#[cfg(feature = "z3")]
use z3::ast::Ast;

/// Verification diagnostic for LSP integration.
/// This type is always available for LSP integration regardless of Z3 feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyDiagnostic {
    pub code: String,
    pub message: String,
    pub span: Span,
    pub severity: DiagnosticSeverity,
    pub related: Vec<RelatedInfo>,
    pub fix: Option<CodeFix>,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Related diagnostic information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedInfo {
    pub span: Span,
    pub message: String,
}

/// Suggested code fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeFix {
    pub title: String,
    pub edits: Vec<TextEdit>,
}

/// Text edit for a code fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    pub span: Span,
    pub new_text: String,
}

impl VerifyDiagnostic {
    /// Create from a verification result (requires Z3 feature).
    #[cfg(feature = "z3")]
    pub fn from_verify_result(
        result: &crate::solver::VerifyResult,
        code: &str,
        span: Span,
    ) -> Option<Self> {
        match result {
            crate::solver::VerifyResult::Unsat(Some(core)) => Some(Self {
                code: code.to_string(),
                message: core.explanation.clone(),
                span,
                severity: DiagnosticSeverity::Error,
                related: core
                    .assertions
                    .iter()
                    .map(|a| RelatedInfo {
                        span: Span::new(0, 0, 1, 1),
                        message: format!("Conflicting assertion: {}", a),
                    })
                    .collect(),
                fix: None,
            }),
            crate::solver::VerifyResult::Sat(model) => Some(Self {
                code: code.to_string(),
                message: "Property violated (counterexample found)".to_string(),
                span,
                severity: DiagnosticSeverity::Error,
                related: model
                    .to_counterexample(&HashMap::new())
                    .locations
                    .iter()
                    .map(|l| RelatedInfo {
                        span: l.span,
                        message: format!("{} = {}", l.variable, l.value),
                    })
                    .collect(),
                fix: None,
            }),
            _ => None,
        }
    }
}

impl Default for VerifyDiagnostic {
    fn default() -> Self {
        Self {
            code: String::new(),
            message: String::new(),
            span: Span::new(0, 0, 1, 1),
            severity: DiagnosticSeverity::Error,
            related: Vec::new(),
            fix: None,
        }
    }
}

// Z3-dependent types and implementations
#[cfg(feature = "z3")]
mod z3_models {
    use super::*;
    use crate::solver::VerifyResult;
    use z3::FuncDecl;
    use z3::Sort;
    use z3::ast::{Ast, BV, Bool, Dynamic, Int};
    use z3_sys::Z3_get_bv_sort_size;

    /// Extracted model from a SAT result.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Model {
        /// Variable assignments: name -> value
        pub assignments: HashMap<String, ModelValue>,
        /// Function interpretations (for uninterpreted functions)
        pub functions: HashMap<String, FuncInterpretation>,
    }

    impl Model {
        /// Create an empty model.
        pub fn empty() -> Self {
            Self::default()
        }

        /// Create a model from Z3's model.
        pub fn from_z3(z3_model: Option<z3::Model>) -> Result<Self, String> {
            let mut model = Self::default();

            if let Some(m) = z3_model {
                for decl in &m {
                    let name = decl.name();
                    let _sort_kind = decl.range();

                    if decl.arity() == 0 {
                        // For constants (arity 0), apply with no args to get the Ast
                        let const_ast = decl.apply(&[]);
                        if let Some(value) = m.get_const_interp(&const_ast) {
                            model.assignments.insert(name, ModelValue::from_z3(&value)?);
                        }
                    } else {
                        let interpretation = FuncInterpretation::from_z3(&m, &decl)?;
                        model.functions.insert(name, interpretation);
                    }
                }
            }

            Ok(model)
        }

        /// Get an integer value by name.
        pub fn get_int(&self, name: &str) -> Option<i64> {
            self.assignments.get(name).and_then(|v| v.as_int())
        }

        /// Get a boolean value by name.
        pub fn get_bool(&self, name: &str) -> Option<bool> {
            self.assignments.get(name).and_then(|v| v.as_bool())
        }

        /// Get a bitvector value by name.
        pub fn get_bv(&self, name: &str) -> Option<(u32, u64)> {
            self.assignments.get(name).and_then(|v| v.as_bv())
        }

        /// Map model values back to Naso source locations.
        pub fn to_counterexample(&self, var_spans: &HashMap<String, Span>) -> Counterexample {
            let mut locations = Vec::new();

            for (name, value) in &self.assignments {
                if let Some(span) = var_spans.get(name) {
                    locations.push(CounterexampleLocation {
                        variable: name.clone(),
                        value: value.clone(),
                        span: *span,
                    });
                }
            }

            Counterexample { locations }
        }
    }

    /// A value in the model.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub enum ModelValue {
        Int(i64),
        Bool(bool),
        BitVec(u32, u64),
        Real(String),
        Array(Vec<(ModelValue, ModelValue)>),
        Uninterpreted(String),
    }

    impl ModelValue {
        fn from_z3(value: &Dynamic) -> Result<Self, String> {
            let sort = value.get_sort();
            let kind = sort.kind();

            match kind {
                z3::SortKind::Int => value
                    .as_int()
                    .ok_or_else(|| "Failed to extract int value".to_string())
                    .and_then(|v| {
                        v.as_i64()
                            .ok_or_else(|| "Failed to convert int".to_string())
                    })
                    .map(ModelValue::Int),
                z3::SortKind::Bool => value
                    .as_bool()
                    .ok_or_else(|| "Failed to extract bool value".to_string())
                    .and_then(|v| {
                        v.as_bool()
                            .ok_or_else(|| "Failed to convert bool".to_string())
                    })
                    .map(ModelValue::Bool),
                z3::SortKind::BV => {
                    // Use z3_sys to get bitvector size since Sort doesn't have a public method for this
                    let ctx = z3::Context::thread_local();
                    let width =
                        unsafe { Z3_get_bv_sort_size(ctx.get_z3_context(), sort.get_z3_sort()) };
                    if width == 0 {
                        return Err("Invalid BV sort".to_string());
                    }
                    value
                        .as_bv()
                        .ok_or_else(|| "Failed to extract BV value".to_string())
                        .and_then(|v| v.as_u64().ok_or_else(|| "Failed to convert BV".to_string()))
                        .map(|v| ModelValue::BitVec(width, v))
                }
                z3::SortKind::Real => Ok(ModelValue::Real(value.to_string())),
                z3::SortKind::Array => Ok(ModelValue::Array(Vec::new())),
                _ => Ok(ModelValue::Uninterpreted(value.to_string())),
            }
        }

        pub fn as_int(&self) -> Option<i64> {
            match self {
                ModelValue::Int(i) => Some(*i),
                _ => None,
            }
        }

        pub fn as_bool(&self) -> Option<bool> {
            match self {
                ModelValue::Bool(b) => Some(*b),
                _ => None,
            }
        }

        pub fn as_bv(&self) -> Option<(u32, u64)> {
            match self {
                ModelValue::BitVec(w, v) => Some((*w, *v)),
                _ => None,
            }
        }
    }

    impl std::fmt::Display for ModelValue {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ModelValue::Int(i) => write!(f, "{}", i),
                ModelValue::Bool(b) => write!(f, "{}", b),
                ModelValue::BitVec(w, v) => write!(f, "(_ bv{} {})", v, w),
                ModelValue::Real(s) => write!(f, "{}", s),
                ModelValue::Array(_) => write!(f, "[array]"),
                ModelValue::Uninterpreted(s) => write!(f, "{}", s),
            }
        }
    }

    /// Function interpretation in a model.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FuncInterpretation {
        pub entries: Vec<FuncEntry>,
        pub else_branch: Option<ModelValue>,
    }

    /// Single entry in a function interpretation.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FuncEntry {
        pub args: Vec<ModelValue>,
        pub value: ModelValue,
    }

    impl FuncInterpretation {
        fn from_z3(_model: &z3::Model, _decl: &z3::FuncDecl) -> Result<Self, String> {
            Ok(Self {
                entries: Vec::new(),
                else_branch: None,
            })
        }
    }

    /// Counterexample for reporting verification failures.
    #[derive(Debug, Clone)]
    pub struct Counterexample {
        pub locations: Vec<CounterexampleLocation>,
    }

    /// A single location in a counterexample.
    #[derive(Debug, Clone)]
    pub struct CounterexampleLocation {
        pub variable: String,
        pub value: ModelValue,
        pub span: Span,
    }

    impl Counterexample {
        /// Format as human-readable string.
        pub fn format(&self) -> String {
            let mut out = String::new();
            out.push_str("Counterexample:\n");
            for loc in &self.locations {
                out.push_str(&format!(
                    "  {} at <unknown>:{}:{} = {}\n",
                    loc.variable, loc.span.line, loc.span.column, loc.value
                ));
            }
            out
        }
    }

    /// Unsatisfiable core from Z3.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct UnsatCore {
        /// Assertion names that form the unsat core
        pub assertions: Vec<String>,
        /// Human-readable explanation
        pub explanation: String,
    }

    impl UnsatCore {
        /// Create from Z3's unsat core.
        pub fn from_z3(
            z3_core: Vec<z3::ast::Bool>,
            _assertion_ids: &HashMap<String, usize>,
        ) -> Result<Self, String> {
            let mut assertions = Vec::new();

            for ast in z3_core {
                assertions.push(ast.to_string());
            }

            let explanation = if assertions.is_empty() {
                "Unsat core not available".to_string()
            } else {
                format!("Conflicting assertions: {}", assertions.join(", "))
            };

            Ok(Self {
                assertions,
                explanation,
            })
        }

        /// Format as human-readable string.
        pub fn format(&self) -> String {
            let mut out = String::new();
            out.push_str("Unsatisfiable Core:\n");
            for a in &self.assertions {
                out.push_str(&format!("  - {}\n", a));
            }
            out.push_str(&self.explanation);
            out
        }
    }
}

// Re-export Z3-dependent types when z3 feature is enabled
#[cfg(feature = "z3")]
pub use z3_models::*;
