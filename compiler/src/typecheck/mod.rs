//! Naso Type Checker
//!
//! Bidirectional type checker with Quantitative Type Theory (QTT) support:
//! - Quantity tracking: [0] erased, [1] linear, [N] bounded, [*] unrestricted
//! - Mutable value semantics: inout (unique mutable projection), consume (linear move)
//! - Dependent types: Pi/Sigma, Nat expressions
//! - Reversible computation with uncomputation verification
//! - Quantum linearity: Qubit/QRegister are always [1]

#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]

pub mod check;
pub mod constraints;
pub mod error;
pub mod inference;
pub mod type_env;
pub mod unify;

#[cfg(test)]
pub mod tests;

pub use error::TypeError;
pub use error::TypeResult;

use crate::ast::*;
use indexmap::IndexMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Metavariable supply for type inference
static META_VAR_SUPPLY: AtomicU32 = AtomicU32::new(0);

/// Generate a fresh metavariable
pub fn fresh_meta_var() -> MetaVar {
    MetaVar(META_VAR_SUPPLY.fetch_add(1, Ordering::Relaxed))
}

/// Generate a fresh type variable
pub fn fresh_type_var() -> TypeVar {
    TypeVar(META_VAR_SUPPLY.fetch_add(1, Ordering::Relaxed))
}

/// Bidirectional typing mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Check mode: expression checks against expected type
    Check,
    /// Infer mode: expression synthesizes its type
    Infer,
}

/// Type checker state
pub struct TypeChecker {
    /// Type environment with quantity/mutability tracking
    pub env: type_env::TypeEnv,
    /// Metavariable store
    pub meta_vars: IndexMap<MetaVar, Option<Type>>,
    /// Quantity constraints to solve
    pub qty_constraints: constraints::ConstraintSet,
    /// Current typing mode
    pub mode: Mode,
    /// Errors accumulated during checking
    pub errors: Vec<error::TypeError>,
    /// Whether we're inside a reversible block (affects purity checks)
    pub in_reversible: bool,
    /// Whether we're inside a `proof { .. }` block.
    ///
    /// Gates `assert`, which exists only to state an obligation. Making it
    /// proof-only is what stops `assert(x)` in ordinary code from typechecking
    /// and then being a silent no-op in codegen.
    pub in_proof: bool,
    /// Current function signature being checked (for return type)
    pub current_fn_ret: Option<Type>,
}

impl TypeChecker {
    /// Create a new type checker with empty environment
    pub fn new() -> Self {
        Self {
            env: type_env::TypeEnv::new(),
            meta_vars: IndexMap::new(),
            qty_constraints: constraints::ConstraintSet::new(),
            mode: Mode::Infer,
            errors: Vec::new(),
            in_reversible: false,
            in_proof: false,
            current_fn_ret: None,
        }
    }

    /// Create a type checker with a pre-populated environment
    pub fn with_env(env: type_env::TypeEnv) -> Self {
        Self {
            env,
            meta_vars: IndexMap::new(),
            qty_constraints: constraints::ConstraintSet::new(),
            mode: Mode::Infer,
            errors: Vec::new(),
            in_reversible: false,
            in_proof: false,
            current_fn_ret: None,
        }
    }

    /// Enter check mode with expected type
    pub fn check_expr(&mut self, expr: &Expr, expected: &Type) -> Result<(), error::TypeError> {
        let prev_mode = self.mode;
        self.mode = Mode::Check;
        let result = check::check_expr(self, expr, expected);
        self.mode = prev_mode;
        result
    }

    /// Enter infer mode to synthesize type
    pub fn infer_expr(&mut self, expr: &Expr) -> Result<Type, error::TypeError> {
        let prev_mode = self.mode;
        self.mode = Mode::Infer;
        let result = inference::infer_expr(self, expr);
        self.mode = prev_mode;
        result
    }

    /// Check a statement
    pub fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), error::TypeError> {
        check::check_stmt(self, stmt)
    }

    /// Check a block
    pub fn check_block(&mut self, block: &Block) -> Result<(), error::TypeError> {
        check::check_block(self, block)
    }

    /// Check a function definition
    pub fn check_function(&mut self, func: &Function) -> Result<(), error::TypeError> {
        check::check_function(self, func)
    }

    /// Check a pattern against a scrutinee type
    pub fn check_pattern(
        &mut self,
        pattern: &Pattern,
        scrutinee_ty: &Type,
    ) -> Result<type_env::PatternBindings, error::TypeError> {
        check::check_pattern(self, pattern, scrutinee_ty)
    }

    /// Add a quantity constraint
    pub fn add_qty_constraint(&mut self, constraint: constraints::QtyConstraint) {
        self.qty_constraints.add(constraint);
    }

    /// Solve all accumulated constraints
    pub fn solve_constraints(&mut self) -> Result<(), error::TypeError> {
        constraints::solve(
            &mut self.qty_constraints,
            &mut self.env,
            &mut self.meta_vars,
        )
    }

    /// Register a metavariable with optional solution
    pub fn register_meta(&mut self, mv: MetaVar, solution: Option<Type>) {
        self.meta_vars.insert(mv, solution);
    }

    /// Lookup a metavariable solution
    pub fn lookup_meta(&self, mv: &MetaVar) -> Option<&Option<Type>> {
        self.meta_vars.get(mv)
    }

    /// Report an error
    pub fn error(&mut self, err: error::TypeError) {
        self.errors.push(err);
    }

    /// Check if any errors have been reported
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Take all errors
    pub fn take_errors(&mut self) -> Vec<error::TypeError> {
        std::mem::take(&mut self.errors)
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Load the standard library prelude into the type environment
fn load_prelude(env: &mut type_env::TypeEnv) {
    use crate::ast::{
        Block, Function, Ident, Param, Quantity, Span, Type, TypeDef, TypeDefKind, TypeKind,
    };

    /// Configuration for a prelude function
    struct PreludeFn {
        name: &'static str,
        params: Vec<Param>,
        ret_ty: Option<Type>,
        quantity: Quantity,
        is_reversible: bool,
    }

    impl PreludeFn {
        fn insert(self, env: &mut type_env::TypeEnv) {
            let fname = Ident::new(self.name, Span::default());
            let func = Function {
                name: fname.clone(),
                generics: Vec::new(),
                params: self.params,
                ret_ty: self.ret_ty.clone(),
                body: Block::new(Vec::new(), None, Span::default()),
                span: Span::default(),
                attributes: Vec::new(),
                is_reversible: self.is_reversible,
                quantity: self.quantity,
            };
            env.insert_function(func.clone());
            // Also bind as a variable so it can be used as a callee
            let fn_type = Type::new(
                TypeKind::Function(
                    func.params.iter().map(|p| p.ty.clone()).collect(),
                    Box::new(self.ret_ty.clone().unwrap_or(Type::unit(Span::default()))),
                ),
                Quantity::Many,
                Span::default(),
            );
            env.bind_var(fname, fn_type, Quantity::Many, Mutability::Immutable);
        }
    }

    // Quantum primitives
    PreludeFn {
        name: "qalloc",
        params: vec![],
        ret_ty: Some(Type::qubit(Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    PreludeFn {
        name: "hadamard",
        params: vec![Param {
            name: Ident::new("q", Span::default()),
            ty: Type::qubit(Span::default()),
            quantity: Quantity::One,
            mutability: Mutability::InOut,
            span: Span::default(),
        }],
        ret_ty: Some(Type::unit(Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    //
    // REACHABILITY WARNING: this entry is currently SHADOWED. The parser builds a
    // dedicated `QuantumOp::Measure` node (parser/expr.rs), so `measure(q)` is
    // typed by `infer_quantum_op`, never by this signature. The quantity here is
    // kept correct so the two agree, but changing ONLY this line changes no
    // behaviour -- which is what happened during this work, and why the real fix
    // was in `infer_quantum_op`. If the dedicated node is ever removed, this
    // signature becomes load-bearing.
    //
    // A builtin reachable only from a prelude table is NOT verified by a test that
    // goes through the dedicated node. Grep for the AST construction before
    // assuming a prelude entry is the live one.
    PreludeFn {
        name: "cnot",
        params: vec![
            Param {
                name: Ident::new("ctrl", Span::default()),
                ty: Type::qubit(Span::default()),
                quantity: Quantity::One,
                mutability: Mutability::InOut,
                span: Span::default(),
            },
            Param {
                name: Ident::new("target", Span::default()),
                ty: Type::qubit(Span::default()),
                quantity: Quantity::One,
                mutability: Mutability::InOut,
                span: Span::default(),
            },
        ],
        ret_ty: Some(Type::unit(Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    // `measure` CONSUMES its qubit argument -- the `[1]` param with
    // `Mutability::Consume` below, which is correct. But the value it hands
    // back is a classical readout of a collapsed qubit: copying it,
    // branching on it, and discarding it are all fine. Typing the return
    // `[1]` made every `let r = measure(q);` bind a linear `r`, so an unused
    // `r` was reported as a linear leak:
    //
    //     fn f() { let [1] q: Qubit = qalloc(1); let r = measure(q); }
    //     -> unused linear variable `r`
    //
    // That is a WRONG diagnostic on a correct program: the qubit WAS
    // properly consumed, and the leftover classical bit is not a resource.
    // The return is `[*]`.
    PreludeFn {
        name: "measure",
        params: vec![Param {
            name: Ident::new("q", Span::default()),
            ty: Type::qubit(Span::default()),
            quantity: Quantity::One,
            mutability: Mutability::Consume,
            span: Span::default(),
        }],
        ret_ty: Some(Type::new(TypeKind::Bool, Quantity::Many, Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    // `linear_free` CONSUMES its argument and releases it, so no linear value
    // comes back -- the result is unit. Returning `Int [1]` would hand out
    // a fresh linear value the caller must consume, from a function whose
    // entire purpose is to get rid of one.
    PreludeFn {
        name: "linear_free",
        params: vec![Param {
            name: Ident::new("x", Span::default()),
            ty: Type::new(TypeKind::Int, Quantity::One, Span::default()),
            quantity: Quantity::One,
            mutability: Mutability::Consume,
            span: Span::default(),
        }],
        ret_ty: Some(Type::unit(Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    PreludeFn {
        name: "linear_alloc",
        params: vec![Param {
            name: Ident::new("value", Span::default()),
            ty: Type::new(TypeKind::Int, Quantity::Many, Span::default()),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: Span::default(),
        }],
        ret_ty: Some(Type::new(TypeKind::Int, Quantity::One, Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    PreludeFn {
        name: "qfree",
        params: vec![Param {
            name: Ident::new("q", Span::default()),
            ty: Type::qubit(Span::default()),
            quantity: Quantity::One,
            mutability: Mutability::Consume,
            span: Span::default(),
        }],
        ret_ty: Some(Type::unit(Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    // Core types
    env.insert_type_def(TypeDef {
        name: Ident::new("Int", Span::default()),
        generics: Vec::new(),
        kind: TypeDefKind::Alias(Type::new(TypeKind::Int, Quantity::Many, Span::default())),
        span: Span::default(),
        attributes: Vec::new(),
    });
    env.insert_type_def(TypeDef {
        name: Ident::new("Bool", Span::default()),
        generics: Vec::new(),
        kind: TypeDefKind::Alias(Type::new(TypeKind::Bool, Quantity::Many, Span::default())),
        span: Span::default(),
        attributes: Vec::new(),
    });
    env.insert_type_def(TypeDef {
        name: Ident::new("Qubit", Span::default()),
        generics: Vec::new(),
        kind: TypeDefKind::Alias(Type::qubit(Span::default())),
        span: Span::default(),
        attributes: Vec::new(),
    });

    // Tensor type and operations
    env.insert_type_def(TypeDef {
        name: Ident::new("Tensor", Span::default()),
        generics: Vec::new(),
        kind: TypeDefKind::Alias(Type::new(
            TypeKind::Named(Ident::new("Tensor", Span::default()), Vec::new()),
            Quantity::Many,
            Span::default(),
        )),
        span: Span::default(),
        attributes: Vec::new(),
    });

    // alloc_tensor function
    PreludeFn {
        name: "alloc_tensor",
        params: vec![Param {
            name: Ident::new("shape", Span::default()),
            ty: Type::new(TypeKind::Int, Quantity::Many, Span::default()),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: Span::default(),
        }],
        ret_ty: Some(Type::new(
            TypeKind::Named(Ident::new("Tensor", Span::default()), Vec::new()),
            Quantity::One,
            Span::default(),
        )),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);

    // Math functions.
    //
    // These are compiler intrinsics: the body is empty because codegen emits
    // them directly. Any new entry here needs a matching arm in the WGSL
    // backend's expression lowering, or it will compile and then fail to
    // lower.
    let math_funcs = [
        ("exp", TypeKind::Float, TypeKind::Float),
        ("sqrt", TypeKind::Float, TypeKind::Float),
        // Required by symmetric INT8 quantization: round(x/scale) and
        // clamp(.., -128.0, 127.0) are what make the i8 cast sound. Without
        // these a quantizer cannot be written in naso at all.
        ("round", TypeKind::Float, TypeKind::Float),
        ("abs", TypeKind::Float, TypeKind::Float),
        ("floor", TypeKind::Float, TypeKind::Float),
        ("ceil", TypeKind::Float, TypeKind::Float),
    ];
    for (name, arg_ty, ret_ty) in math_funcs {
        PreludeFn {
            name,
            params: vec![Param {
                name: Ident::new("x", Span::default()),
                ty: Type::new(arg_ty, Quantity::Many, Span::default()),
                quantity: Quantity::Many,
                mutability: Mutability::Immutable,
                span: Span::default(),
            }],
            ret_ty: Some(Type::new(ret_ty, Quantity::Many, Span::default())),
            quantity: Quantity::Many,
            is_reversible: false,
        }
        .insert(env);
    }

    // clamp(value, lo, hi). Not a uniform arity like the table above, so it
    // is declared explicitly. QTT does not model the bound on lo/hi: they
    // are float literals in every valid use, and threading a quantity here
    // would let a caller pass a linear value into a numeric bound.
    PreludeFn {
        name: "clamp",
        params: vec![
            Param {
                name: Ident::new("x", Span::default()),
                ty: Type::new(TypeKind::Float, Quantity::Many, Span::default()),
                quantity: Quantity::Many,
                mutability: Mutability::Immutable,
                span: Span::default(),
            },
            Param {
                name: Ident::new("lo", Span::default()),
                ty: Type::new(TypeKind::Float, Quantity::Many, Span::default()),
                quantity: Quantity::Many,
                mutability: Mutability::Immutable,
                span: Span::default(),
            },
            Param {
                name: Ident::new("hi", Span::default()),
                ty: Type::new(TypeKind::Float, Quantity::Many, Span::default()),
                quantity: Quantity::Many,
                mutability: Mutability::Immutable,
                span: Span::default(),
            },
        ],
        ret_ty: Some(Type::new(TypeKind::Float, Quantity::Many, Span::default())),
        quantity: Quantity::Many,
        is_reversible: false,
    }
    .insert(env);
}

/// Result of type checking a program
pub struct CheckResult {
    /// The checked program (with types filled in)
    pub program: Program,
    /// Any errors encountered
    pub errors: Vec<error::TypeError>,
}

/// Type check a full program
pub fn check_program(program: &mut Program) -> CheckResult {
    let mut checker = TypeChecker::new();

    // Load prelude (stdlib) into the type environment
    load_prelude(&mut checker.env);

    // First pass: register all top-level types and functions
    for item in &program.items {
        match item {
            Item::Function(f) => {
                checker.env.insert_function(f.clone());
            }
            Item::TypeDef(t) => {
                checker.env.insert_type_def(t.clone());
            }
            Item::Const(c) => {
                checker.env.insert_const(c.clone());
            }
            _ => {}
        }
    }

    // Second pass: check function bodies
    for item in &mut program.items {
        if let Item::Function(f) = item {
            if let Err(e) = checker.check_function(f) {
                checker.error(e);
            }
        }
    }

    // Solve constraints
    if let Err(e) = checker.solve_constraints() {
        checker.error(e);
    }

    CheckResult {
        program: program.clone(),
        errors: checker.take_errors(),
    }
}

/// Log a debug message (uses web_sys console in WASM, stderr otherwise)
#[cfg(feature = "wasm-debug")]
pub fn debug_log(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::console::log_1(&format!("[TYPECHECK DEBUG] {}", msg).into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        eprintln!("[TYPECHECK DEBUG] {}", msg);
    }
}

/// No-op debug log when wasm-debug feature is not enabled
#[cfg(not(feature = "wasm-debug"))]
pub fn debug_log(_msg: &str) {
    // No-op
}
