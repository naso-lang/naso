//! Naso Compiler WASM Bindings
//!
//! Exposes the Naso compiler pipeline to WebAssembly for browser-based IDEs and playgrounds.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use naso_compiler::ast::Span;
use naso_compiler::lexer::Lexer;
use naso_compiler::lowering::{LoweringError, lower_program};
use naso_compiler::parser::parse_program;
use naso_compiler::typecheck::{CheckResult, TypeError, check_program};

#[cfg(feature = "console_error_panic_hook")]
use console_error_panic_hook::set_once as set_panic_hook;

#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    set_panic_hook();
}

/// Result of parsing Naso source code
#[derive(Serialize, Deserialize, Debug, Clone)]
#[wasm_bindgen]
pub struct ParseResult {
    /// Whether parsing succeeded
    success: bool,
    /// Pretty-printed AST JSON (if successful)
    ast_json: Option<String>,
    /// Parse error message (if failed)
    error: Option<String>,
}

#[wasm_bindgen]
impl ParseResult {
    #[wasm_bindgen(getter)]
    pub fn success(&self) -> bool {
        self.success
    }

    #[wasm_bindgen(getter)]
    pub fn ast_json(&self) -> Option<String> {
        self.ast_json.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
}

/// Diagnostic information for a single error/warning
#[derive(Serialize, Deserialize, Debug, Clone)]
#[wasm_bindgen]
pub struct Diagnostic {
    /// Severity: "error" | "warning" | "info"
    severity: String,
    /// Human-readable message
    message: String,
    /// Source location
    line: u32,
    column: u32,
    /// End position
    end_line: u32,
    end_column: u32,
    /// Optional error code
    code: Option<String>,
}

#[wasm_bindgen]
impl Diagnostic {
    #[wasm_bindgen(getter)]
    pub fn severity(&self) -> String {
        self.severity.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn line(&self) -> u32 {
        self.line
    }

    #[wasm_bindgen(getter)]
    pub fn column(&self) -> u32 {
        self.column
    }

    #[wasm_bindgen(getter)]
    pub fn end_line(&self) -> u32 {
        self.end_line
    }

    #[wasm_bindgen(getter)]
    pub fn end_column(&self) -> u32 {
        self.end_column
    }

    #[wasm_bindgen(getter)]
    pub fn code(&self) -> Option<String> {
        self.code.clone()
    }
}

/// Complete compilation result
#[derive(Serialize, Deserialize, Debug, Clone)]
#[wasm_bindgen]
pub struct CompileResult {
    /// Whether compilation succeeded (no errors)
    success: bool,
    /// Pretty-printed AST JSON
    ast_json: Option<String>,
    /// Inverse DAG JSON (for reversible blocks)
    inverse_dag_json: Option<String>,
    /// All diagnostics (errors, warnings, info)
    diagnostics: Vec<Diagnostic>,
}

#[wasm_bindgen]
impl CompileResult {
    #[wasm_bindgen(getter)]
    pub fn success(&self) -> bool {
        self.success
    }

    #[wasm_bindgen(getter)]
    pub fn ast_json(&self) -> Option<String> {
        self.ast_json.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn inverse_dag_json(&self) -> Option<String> {
        self.inverse_dag_json.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics.clone()
    }
}

/// Token for syntax highlighting
#[derive(Serialize, Deserialize, Debug, Clone)]
#[wasm_bindgen]
pub struct WasmToken {
    kind: String,
    text: String,
    start: u32,
    end: u32,
    line: u32,
    column: u32,
}

#[wasm_bindgen]
impl WasmToken {
    #[wasm_bindgen(getter)]
    pub fn kind(&self) -> String {
        self.kind.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn text(&self) -> String {
        self.text.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn start(&self) -> u32 {
        self.start
    }

    #[wasm_bindgen(getter)]
    pub fn end(&self) -> u32 {
        self.end
    }

    #[wasm_bindgen(getter)]
    pub fn line(&self) -> u32 {
        self.line
    }

    #[wasm_bindgen(getter)]
    pub fn column(&self) -> u32 {
        self.column
    }
}

/// Extract span from TypeError
fn extract_span_from_error(e: &TypeError) -> Option<Span> {
    use naso_compiler::typecheck::error::TypeError::*;
    match e {
        VariableNotAvailable { span, .. } => Some(*span),
        LinearVariableUsedTwice { second_use, .. } => Some(*second_use),
        UseOfMovedValue { used_at, .. } => Some(*used_at),
        InOutAliasing { new_span, .. } => Some(*new_span),
        UnusedLinearVariable { defined_at, .. } => Some(*defined_at),
        TypeMismatch { span, .. } => Some(*span),
        QuantityMismatch { span, .. } => Some(*span),
        ArgumentCountMismatch { span, .. } => Some(*span),
        TypeArgumentCountMismatch { span, .. } => Some(*span),
        InvalidCast { span, .. } => Some(*span),
        FieldNotFound { span, .. } => Some(*span),
        VariantNotFound { span, .. } => Some(*span),
        NotAStruct { span, .. } => Some(*span),
        NotAFunction { span, .. } => Some(*span),
        NotIndexable { span, .. } => Some(*span),
        InOutRequiresUnique { span, .. } => Some(*span),
        OccursCheck { span, .. } => Some(*span),
        UndefinedType { span, .. } => Some(*span),
        ErasedVariableUsedAtRuntime { span, .. } => Some(*span),
        ImpureInReversible { span, .. } => Some(*span),
        MissingUncompute { span, .. } => Some(*span),
        CyclicUncompute { span, .. } => Some(*span),
        QubitQuantityMismatch { span, .. } => Some(*span),
        PatternTupleArityMismatch { span, .. } => Some(*span),
        MeasureRequiresConsumeQubit { span, .. } => Some(*span),
        EntangleRequiresConsumeQubits { span, .. } => Some(*span),
        GenericMismatch { span, .. } => Some(*span),
        DependentTypeError { span, .. } => Some(*span),
        NonExhaustiveMatch { span, .. } => Some(*span),
        ControlFlowOutsideLoop { span, .. } => Some(*span),
        ReturnOutsideFunction { span, .. } => Some(*span),
        InferenceError { span, .. } => Some(*span),
    }
}

/// Convert typechecker errors to diagnostics
fn typecheck_errors_to_diagnostics(errors: &[TypeError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| {
            let span = extract_span_from_error(e);
            Diagnostic {
                severity: "error".to_string(),
                message: e.to_string(),
                line: span.map(|s| s.line).unwrap_or(1),
                column: span.map(|s| s.column).unwrap_or(1),
                end_line: span.map(|s| s.line).unwrap_or(1),
                end_column: span.map(|s| s.column).unwrap_or(1) + 10,
                code: None,
            }
        })
        .collect()
}

/// Convert lowering errors to diagnostics
fn lowering_error_to_diagnostics(err: &LoweringError) -> Vec<Diagnostic> {
    vec![Diagnostic {
        severity: "error".to_string(),
        message: err.to_string(),
        line: 1,
        column: 1,
        end_line: 1,
        end_column: 10,
        code: Some("LOWERING_ERROR".to_string()),
    }]
}

/// Tokenize Naso source code
#[wasm_bindgen]
pub fn tokenize_naso(source: &str) -> Vec<WasmToken> {
    let tokens = Lexer::lex(source);
    tokens
        .into_iter()
        .map(|t| WasmToken {
            kind: format!("{:?}", t.kind),
            text: source[t.span.start..t.span.end].to_string(),
            start: t.span.start as u32,
            end: t.span.end as u32,
            line: t.span.line,
            column: t.span.column,
        })
        .collect()
}

/// Parse Naso source code to AST
#[wasm_bindgen]
pub fn parse_naso(source: &str) -> ParseResult {
    match parse_program(source) {
        Ok(program) => {
            let ast_json = serde_json::to_string_pretty(&program).ok();
            ParseResult {
                success: true,
                ast_json,
                error: None,
            }
        }
        Err(e) => ParseResult {
            success: false,
            ast_json: None,
            error: Some(e),
        },
    }
}

/// Compile Naso source code (parse + typecheck + lower)
#[wasm_bindgen]
pub fn compile_naso_wasm(source: &str) -> CompileResult {
    // Step 1: Parse
    let mut program = match parse_program(source) {
        Ok(p) => p,
        Err(e) => {
            return CompileResult {
                success: false,
                ast_json: None,
                inverse_dag_json: None,
                diagnostics: vec![Diagnostic {
                    severity: "error".to_string(),
                    message: format!("Parse error: {}", e),
                    line: 1,
                    column: 1,
                    end_line: 1,
                    end_column: 10,
                    code: Some("PARSE_ERROR".to_string()),
                }],
            };
        }
    };

    // Step 2: Type check
    let check_result: CheckResult = check_program(&mut program);
    let mut diagnostics = typecheck_errors_to_diagnostics(&check_result.errors);

    let ast_json = serde_json::to_string_pretty(&check_result.program).ok();

    // Step 3: Lower to PIR (if type checking succeeded)
    let inverse_dag_json = if check_result.errors.is_empty() {
        match lower_program(&check_result.program) {
            Ok(pir) => {
                // For now, serialize the PIR module as the "inverse DAG"
                // In the future, this would be the actual reversible schedule pair
                serde_json::to_string_pretty(&pir).ok()
            }
            Err(e) => {
                diagnostics.extend(lowering_error_to_diagnostics(&e));
                None
            }
        }
    } else {
        None
    };

    CompileResult {
        success: check_result.errors.is_empty(),
        ast_json,
        inverse_dag_json,
        diagnostics,
    }
}

/// Create a default Naso program for the playground
#[wasm_bindgen]
pub fn default_naso_program() -> String {
    r#"// Naso Playground - Reversible Quantum Adder Example

// Full adder using reversible computation
// fn reversible makes the ENTIRE function body a reversible block (no nested reversible { })
fn reversible full_adder(inout a: [1] Qubit, inout b: [1] Qubit, inout cin: [1] Qubit) -> ([1] Qubit, [1] Qubit) {
    // Sum = a ^ b ^ cin
    // Carry = (a & b) | (a & cin) | (b & cin)
    
    // Toffoli for carry computation (using cnot)
    hadamard(a);
    cnot(a, b);
    hadamard(a);
    
    hadamard(a);
    cnot(a, cin);
    hadamard(a);
    
    hadamard(b);
    cnot(b, cin);
    hadamard(b);
    
    // Sum computation (reversible XOR chain using cnot)
    let [1] sum: Qubit = qalloc();
    cnot(a, sum);
    cnot(b, sum);
    cnot(cin, sum);

    // Carry computation
    let [1] carry: Qubit = qalloc();
    cnot(a, carry);
    cnot(b, carry);
    cnot(cin, carry);
    
    return (sum, carry);
}

// Quantum teleportation protocol
fn reversible teleport(msg: [1] Qubit, inout alice: [1] Qubit, inout bob: [1] Qubit) -> [1] Qubit {
    // Create Bell pair between Alice and Bob
    hadamard(alice);
    cnot(alice, bob);
    
    // Bell basis measurement on msg + alice
    cnot(msg, alice);
    hadamard(msg);
    
    let m1 = measure(msg);
    let m2 = measure(alice);
    
    // Conditional corrections on Bob's qubit
    // Note: X and Z gates not in prelude, using cnot+hadamard for demo
    if m1 { cnot(bob, bob); }  // placeholder for X(bob)
    if m2 { hadamard(bob); cnot(bob, bob); hadamard(bob); }  // placeholder for Z(bob)
    
    // msg and alice are consumed (measured)
    // bob now holds the teleported state
    return bob;
}

// Simple reversible function: swap two values using arithmetic
// Int is copyable (Quantity::Many), so use 'mut' not 'inout [1]'
fn reversible swap(mut x: Int, mut y: Int) {
    x = x + y;
    y = x - y;
    x = x - y;
}

// Main entry point
fn main() -> Int {
    // Allocate qubits
    let [1] q1: Qubit = qalloc();
    let [1] q2: Qubit = qalloc();
    let [1] q3: Qubit = qalloc();
    
    // Run full adder
    let (sum, carry) = full_adder(q1, q2, q3);
    
    // Measure results
    let s = measure(sum);
    let c = measure(carry);
    
    // Return integer encoding of results
    if s { 1 } else { 0 } + if c { 2 } else { 0 }
}
"#
    .to_string()
}
