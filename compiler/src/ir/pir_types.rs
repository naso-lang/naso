//! PIR Type Definitions
//!
//! Top-level Polyhedral IR module container integrating all IR components
//! with QTT quantity tracking from the type checker.

#![allow(clippy::useless_format)]

use super::access_relation::AccessRelations;
use super::affine_domain::AffineDomain;
use super::schedule_tree::{ScheduleTree, StmtId};
use crate::ast::{Mutability, Quantity};
use crate::ir::schedule_tree::ScheduleNode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Simplified expression for PIR (lowered from AST)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PirExpr {
    /// Integer literal
    IntLit(i64),
    /// Float literal (uses string to avoid Eq issue with f64)
    FloatLit(String),
    /// Boolean literal
    BoolLit(bool),
    /// Variable reference
    Var(String),
    /// Binary operation
    Binary {
        op: BinaryOp,
        left: Box<PirExpr>,
        right: Box<PirExpr>,
    },
    /// Unary operation
    Unary { op: UnaryOp, expr: Box<PirExpr> },
    /// Function call
    Call { name: String, args: Vec<PirExpr> },
    /// Array access
    Index {
        base: Box<PirExpr>,
        indices: Vec<PirExpr>,
    },
    /// Struct field access
    Field { base: Box<PirExpr>, field: String },
    /// Let binding (for SSA form)
    Let {
        name: String,
        qty: Quantity,
        mutability: Mutability,
        value: Box<PirExpr>,
        body: Box<PirExpr>,
    },
    /// If expression
    If {
        cond: Box<PirExpr>,
        then_branch: Box<PirExpr>,
        else_branch: Box<PirExpr>,
    },
    /// Reversible block
    Reversible {
        body: Box<PirExpr>,
        inverse: Box<PirExpr>,
    },
    /// Quantum operation (qalloc, gates, measure, etc.)
    QuantumOp {
        op: String,
        args: Vec<PirExpr>,
        qubits: Vec<PirExpr>,
    },
    /// Store `value` into the lvalue `target`, evaluating to the stored value.
    ///
    /// Assignment was previously not representable, so `output[i] = ...` failed to
    /// lower with `Unsupported(Assign(..))`. That is the correct failure -- it is a
    /// diagnostic, not a wrong answer -- but it meant no loop kernel with a store
    /// could reach a backend at all.
    Assign {
        target: Box<PirExpr>,
        value: Box<PirExpr>,
    },
    /// A numeric conversion of `expr` to type `ty`.
    ///
    /// `e as T` was previously unrepresentable, so lowering refused with
    /// `Unsupported(Ascribe(..))` -- or, when it sat inside a loop body, silently
    /// discarded the whole body first (the body became `IntLit(0)`).
    ///
    /// The target type is carried rather than assumed: a backend that cannot perform
    /// the conversion must say so instead of dropping it, because a dropped cast
    /// stores the wrong type into the slot.
    Cast {
        expr: Box<PirExpr>,
        /// Declared bit width of the cast target, when the target is an integer.
        ///
        /// Only the width is carried, not the whole `ast::Type`, because `PirExpr`
        /// derives `Eq` and `Type` does not. The width is what a backend needs to
        /// build the right LLVM integer type or WGSL constructor, and it is what was
        /// being lost: `q[..] as i8` lowered with the `i8` simply absent.
        width: Option<u8>,
        /// Whether the TARGET is a signed integer.
        ///
        /// Carried explicitly because LLVM integer types are SIGNLESS -- `i32` is the
        /// same type whether it holds a signed or an unsigned value, and the difference
        /// is entirely in whether you emit `sext` or `zext`. Nothing downstream can
        /// recover it, so guessing here is a wrong answer rather than a default: `zext`
        /// of a negative value wraps, and `sext` of a large unsigned value goes negative.
        signed: bool,
        /// The cast target is a FLOAT, not an integer.
        ///
        /// `width` alone cannot express `x as f32`: a float target has no bit width in
        /// the integer sense, and `width` is `None` for it, which previously read as
        /// "an i32" -- so `input[i] as f32` on an `i8` buffer emitted a SIGN-EXTENDING
        /// INTEGER conversion and then multiplied an `i32` by a `double`. That is a
        /// silent wrong answer: the program compiles, verifies, and computes nonsense.
        ///
        /// So the target's SORT is recorded explicitly rather than inferred from a
        /// `None` width. `None` keeps the original integer behaviour byte for byte;
        /// `Some(_)` means `sitofp`/`uitofp`, with the sign flag still carried because
        /// LLVM integer types are signless and nothing downstream can recover it.
        ///
        /// No float WIDTH is carried, because this backend has one: `f32` and `f64` are
        /// already the same type in the IR (`ElemType` documents why), so recording a
        /// width would record a distinction that does not exist.
        float_target: Option<ElemType>,
    },
    /// A sequence of statements evaluated for effect, yielding no value.
    ///
    /// This exists because a loop body is usually statements and no tail
    /// expression. Before this variant, `forall i in 0..1024 { output[i] = ...; }`
    /// lowered its body to `IntLit(0)` -- the ENTIRE body was discarded and the
    /// kernel compiled to `define void @stmt_0() { ret void }`. `IntLit(0)` is a
    /// value, so a value-less body had nowhere to live and the placeholder looked
    /// plausible at the type level while throwing away the program's meaning.
    ///
    /// Order matters and is preserved: this is a sequence, not a set.
    Stmts(Vec<PirExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// The element type of a tensor or scalar, as the LLVM ABI needs it.
///
/// This is deliberately coarser than `ast::Type`. It carries only the
/// distinctions a backend cannot recover on its own, and it derives `Eq` so
/// `PirExpr` (and therefore `PirModule`) keeps deriving it.
///
/// # Why floats have no width
///
/// `ast::Type` records `int_width` but has no float counterpart: the parser
/// turns BOTH `f32` and `f64` into `TypeKind::Float` with nothing to tell them
/// apart, and `PirExpr::FloatLit` lowers to LLVM `double`. So `ElemType::F64`
/// is the only spelling available for a Naso float. Emitting `float` for a
/// tensor while literals are `double` would require an `fptrunc` at every
/// store, and inserting that conversion silently is exactly what this IR
/// exists to prevent. `TypeKind::Float` is therefore `double` throughout, and
/// a program needing genuine single-precision storage is not representable
/// yet -- that gap belongs in the AST, not in a guess here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElemType {
    F64,
    I8,
    I16,
    I32,
    I64,
    Bool,
}

/// How a function parameter reaches the generated function.
///
/// A parameter used to produce NO PIR at all: `PirModule::parameters` carried
/// only the NAMES of `main`'s parameters, and only as module-level symbolic
/// constants. A body that read a tensor parameter therefore had no allocation
/// for it. This enum is what a backend needs to give each parameter a real ABI
/// slot, and it is carried from the AST rather than re-derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamKind {
    /// A caller-owned buffer of `elem`, of `extent` elements.
    ///
    /// Becomes an LLVM POINTER, never an alloca. Allocating and zero-filling a
    /// tensor parameter would produce a function that computes on data the
    /// caller cannot supply -- it builds clean and is silently wrong, which is
    /// the specific failure this whole change exists to remove.
    ///
    /// `shape` is the extent of each dimension, outermost first. It is `None`
    /// for a symbolic extent (`Tensor[f32, N]`), which no backend here can
    /// monomorphise; such a parameter is refused with that reason rather than
    /// given a guessed bound.
    ///
    /// A shape rather than one extent, because `C[i][j]` needs both: a single
    /// pointer has one stride, so a 2-D subscript can only be linearised
    /// (`i * cols + j`) when the column count is known. Carrying the whole
    /// shape is what makes that a GEP instead of a refusal, and it is why a
    /// matrix kernel and a vector kernel share one ABI.
    ///
    /// # The shape is a CHECK, not just a stride
    ///
    /// The LLVM backend passes a tensor parameter as `(ptr, i64 len)` and compares
    /// `len` against the product of `shape` before the body runs; see
    /// [`crate::codegen::llvm::abi_guard`]. That is the only part of a `[1]`
    /// resource's contract that survives at a raw-pointer ABI, and it is worth
    /// being exact about the rest:
    ///
    /// - CONSUMED EXACTLY ONCE is a property of how many times the function BODY
    ///   reads the name. The typechecker counts that statically and refuses a
    ///   double use. It is not re-checked at run time, and at this boundary it
    ///   could not be: a `ptr` does not know how many times it was read.
    /// - NOT OUTLIVING ITS SCOPE cannot be checked at all. A raw pointer carries
    ///   no lifetime, so nothing here can observe whether the caller's buffer is
    ///   still alive, and nothing prevents the callee from retaining the pointer.
    /// - LINEAR vs UNRESTRICTED (`inout` vs `[1]`) are the SAME `ptr` in this ABI
    ///   and are deliberately not distinguished. They could only be distinguished
    ///   with `noalias` / `readonly`, both of which would be false: a caller may
    ///   legitimately pass the SAME buffer as the read-only input and the writable
    ///   output (in-place elementwise scaling), and either attribute turns that
    ///   correct program into undefined behaviour.
    ///
    /// So the guarantee this type carries is its EXTENT, enforced against the
    /// caller's buffer at run time. The linearity is enforced by the typechecker
    /// over the body, and stops at the call boundary.
    Tensor {
        elem: ElemType,
        shape: Option<Vec<u64>>,
    },
    /// A scalar, passed BY VALUE.
    Scalar(ElemType),
    /// A quantum register: a caller-owned pointer, like a tensor but with no
    /// element type the LLVM backend indexes.
    QRegister,
    /// A parameter whose type no ABI rule covers.
    ///
    /// Recorded rather than dropped, so a backend refuses it naming the type
    /// instead of inventing a slot for it.
    Unsupported(String),
}

/// One function parameter, with everything an ABI needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionParam {
    pub name: String,
    pub kind: ParamKind,
    pub quantity: Quantity,
    pub mutability: Mutability,
}

/// PIR Statement: a computational unit with iteration domain
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirStatement {
    pub id: StmtId,
    /// Iteration domain for this statement
    pub domain: AffineDomain,
    /// Statement body (lowered expression)
    pub body: PirExpr,
    /// Quantity annotation from QTT type checker
    pub quantity: Quantity,
    /// Mutability annotation
    pub mutability: Mutability,
    /// Source location for debugging
    pub span: Option<crate::ast::Span>,
}

/// Quantity map: variable name -> Quantity (from type checker)
pub type QuantityMap = HashMap<String, Quantity>;

/// What a Naso function returns.
///
/// `None` in the AST (`fn f() { .. }`) and `Some(f64)` are both representable, and the
/// difference is exactly what a call site needs to know: whether the call yields a
/// value or not. Carrying it is what lets a backend emit a typed `ret` and refuse a
/// call that uses a void result as a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FnReturn {
    /// No return type declared. The function yields nothing.
    Void,
    /// A scalar, returned by value.
    Scalar(ElemType),
    /// A declared return type with no slot in this ABI.
    ///
    /// Recorded rather than dropped, so a backend refuses it NAMING the type instead
    /// of silently emitting a void function whose callers then bind a value that does
    /// not exist. That is the same rule [`ParamKind::Unsupported`] follows.
    Unsupported(String),
}

/// One Naso function, with its own parameters, statements, schedule and names.
///
/// # Why this exists
///
/// A `PirModule` used to have NO function structure: `lower_item` walked every
/// `Item::Function` and concatenated all their bodies into one flat `statements` list,
/// and `LLVMModuleBuilder::build_module` emitted that list as a single `naso_entry`.
/// Every function's parameters therefore shared one namespace in one LLVM function
/// body, which made `kernels/quant_int8.naso` uncompilable: its three functions each
/// declare `input`/`output`/`scale`, and `Tensor[f32, 1024]` is not
/// `Tensor[i8, 1024]`. Lowering refused with "declared with two different types".
///
/// LLVM has exact `i8` storage and `as i8` narrows correctly, so the obstacle was
/// never the width -- it was the flattening. This type is the flattening's replacement:
/// each function owns its parameters, its statements, its schedule tree and its
/// quantity map, so two functions may bind the same name at different types.
///
/// # Linearity is per function, and that is the point
///
/// `quantities` and `statements` are per function rather than shared because a `[1]`
/// resource is consumed exactly once *by the body that owns it*. `input` is `[1]` in
/// all three functions of `quant_int8.naso`, and each consumes its own once; counted
/// across a flattened module that is three uses of one name and PIR validation rejects
/// the file. Scoping the count to the function is not a loosening -- it is the same
/// rule applied at the right granularity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirFunction {
    /// The name as written in the source.
    pub name: String,
    /// This function's declared parameters, in source order.
    pub params: Vec<FunctionParam>,
    /// This function's statements. `StmtId`s are unique across the whole module.
    pub statements: Vec<PirStatement>,
    /// The schedule for THIS function's statements.
    pub schedule: ScheduleTree,
    /// Access relations for this function's statements.
    pub accesses: AccessRelations,
    /// Quantity annotations for the names THIS function binds.
    pub quantities: QuantityMap,
    /// What the function returns.
    pub return_type: FnReturn,
    /// The statement holding this function's RETURN VALUE, if it returns one.
    ///
    /// `lower_stmt` turns `return e` into an ordinary statement whose body is the
    /// lowered `e`. That is enough for a `void` function, where the value is
    /// computed and discarded. It is NOT enough for a function that returns a value:
    /// the backend has to `ret` the computed value, and the flat statement list gives
    /// it no way to know which statement is the return rather than a discarded
    /// expression. This field names it.
    ///
    /// The statement is REMOVED from `schedule` when this is set, so the return value
    /// is evaluated once, in the function's exit block, and not also as a scheduled
    /// side-effecting statement.
    ///
    /// `None` for a `void` function, and for a value-returning function whose return
    /// statement is not the function's last statement -- which is REFUSED at lowering
    /// rather than lowered to a silently zero return.
    pub return_stmt: Option<crate::ir::schedule_tree::StmtId>,
    /// The statement holding this function's IMPLICIT return value: the value of the
    /// block's trailing expression, for a function that declares a return type and
    /// contains no `return` at all.
    ///
    /// # This is NOT the explicit return
    ///
    /// [`Self::return_stmt`] is `Some` only when the source contains `return e`. A
    /// backend must not conclude "this function returns a value" from that field
    /// alone: a function can return a value with `return_stmt == None`, and conflating
    /// the two is how a tail return gets re-emitted as a scheduled statement, or
    /// refused as a missing return.
    ///
    /// # Why a second field rather than a synthetic `return_stmt`
    ///
    /// Recording the tail as if the programmer had written `return <tail>` makes the
    /// two indistinguishable everywhere downstream: `return_stmt == None` stops meaning
    /// "no return statement", a diagnostic cannot say which form the source used, and a
    /// test cannot assert that a real `return` took a different path from a tail. Two
    /// fields keep the source's own distinction. They are never both `Some`: lowering
    /// fills this one only when `return_stmt` is `None`.
    ///
    /// # Which AST shape sets it
    ///
    /// `Block::expr` -- the trailing expression the PARSER folded off the end of the
    /// body -- and only when its kind yields a value. `let mut y = x * 3.0;` is NOT a
    /// trailing expression: the parser routes every `let` to `Block::stmts`, so that
    /// program has no `Block::expr` at all and therefore no tail value, even though
    /// lowering does emit a statement for the binding. "A statement exists in PIR" and
    /// "the body ends in an expression whose value is the function's result" are
    /// different facts, and only the second one may be returned. Lowering checks the
    /// AST kind against an explicit allow-list for exactly that reason.
    ///
    /// # Scheduled?
    ///
    /// No. As with `return_stmt`, the `Domain` node naming this statement is removed
    /// from `schedule`, so the value is evaluated once in the function's exit block
    /// rather than also as a statement in the middle of the body.
    pub tail_return_stmt: Option<crate::ir::schedule_tree::StmtId>,
    /// Source location, for a diagnostic that can point at the function.
    pub span: Option<crate::ast::Span>,
}

/// Complete PIR Module
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirModule {
    /// All statements in the module
    pub statements: Vec<PirStatement>,
    /// Schedule tree defining execution order
    pub schedule: ScheduleTree,
    /// Memory access relations
    pub accesses: AccessRelations,
    /// Quantity tracking from QTT type checker
    pub quantities: QuantityMap,
    /// Module-level parameters (symbolic constants)
    pub parameters: Vec<String>,
    /// Every function parameter in the program, with its ABI shape.
    ///
    /// `parameters` above is a NAME list for module-level symbolic constants
    /// (`n` in `forall i in 0..n`) and cannot express a type, an extent, or a
    /// parameter of any function other than `main`. This field is the one a
    /// backend reads to build a function signature, so it carries every
    /// parameter of every function in the program, in source order.
    ///
    /// Parameters from DIFFERENT functions are merged into one list, deduplicated by
    /// name. That union is the compatibility surface for a backend that still emits
    /// ONE function for the whole module; a backend that emits one symbol per function
    /// reads [`PirModule::functions`] and each function's own `params` instead, which
    /// is why two functions may now bind the same name at different types without a
    /// refusal. When two declarations of one name disagree, the FIRST wins here and
    /// the per-function lists carry both types correctly.
    pub function_params: Vec<FunctionParam>,
    /// Function signatures for external calls
    pub extern_functions: Vec<ExternFunction>,
    /// The program's functions, each with its own scope.
    ///
    /// # Relationship to the flat fields above
    ///
    /// `statements`, `schedule`, `accesses`, `quantities` and `function_params` are the
    /// CONCATENATION of every function's, in source order. They are kept because a
    /// great deal of code reads them -- `ir::validate`, `codegen::wgsl`, the
    /// `pretty_print` dump, and every hand-built module in the test suite, including
    /// the `.pir` fixtures in `compiler/tests/fixtures/` which have no function
    /// sections at all.
    ///
    /// So the flat list is the COMPATIBILITY SURFACE and `functions` is the STRUCTURE.
    /// A backend that emits one symbol per function reads `functions`; a backend that
    /// still emits one function for the whole module keeps reading the flat list and
    /// is unaffected by this field being populated.
    ///
    /// `function_params` is the union of every function's parameters. Two functions
    /// binding the same name at DIFFERENT types is no longer a conflict, because each
    /// function reads its own `params`; the union keeps the first declaration so a
    /// backend still reading it gets one entry per name rather than duplicates.
    pub functions: Vec<PirFunction>,
}

impl Default for PirModule {
    fn default() -> Self {
        // An EMPTY module, not an empty schedule tree around a dummy statement: the
        // latter invents structure the IR never had. Tests and backends that need an
        // empty module reach for `..Default::default()` constantly, and before this
        // existed they each had to spell out six fields.
        Self {
            statements: Vec::new(),
            schedule: ScheduleTree::new(ScheduleNode::Empty, Vec::new()),
            accesses: AccessRelations::new(),
            quantities: QuantityMap::new(),
            parameters: Vec::new(),
            function_params: Vec::new(),
            extern_functions: Vec::new(),
            functions: Vec::new(),
        }
    }
}

/// External function declaration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternFunction {
    pub name: String,
    pub params: Vec<ExternParam>,
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternParam {
    pub name: String,
    pub ty: String,
    pub quantity: Quantity,
    pub mutability: Mutability,
}

impl PirModule {
    pub fn new(
        statements: Vec<PirStatement>,
        schedule: ScheduleTree,
        accesses: AccessRelations,
        quantities: QuantityMap,
        parameters: Vec<String>,
    ) -> Self {
        Self {
            statements,
            schedule,
            accesses,
            quantities,
            parameters,
            // Filled in by the lowering pass from the AST signatures; a
            // hand-constructed module genuinely has no function parameters.
            function_params: Vec::new(),
            extern_functions: Vec::new(),
            // Likewise: a hand-constructed module (every `.pir` fixture, and every
            // test that builds one by hand) has one implicit entry body and no
            // function structure. Populating this is the lowering pass's job.
            functions: Vec::new(),
        }
    }

    /// Validate the entire PIR module
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        // Validate schedule tree
        if let Err(e) = self.schedule.validate() {
            errors.push(ValidationError::ScheduleError(e.to_string()));
        }

        // Check all statements have corresponding domain nodes in schedule
        let scheduled_domains = self.schedule.collect_domains();
        let scheduled_ids: std::collections::HashSet<_> =
            scheduled_domains.iter().map(|(id, _)| *id).collect();

        for stmt in &self.statements {
            if !scheduled_ids.contains(&stmt.id) {
                errors.push(ValidationError::UnscheduledStatement(stmt.id));
            }
        }

        // Check quantity consistency: [0] vars should not appear in runtime schedule
        for (var, qty) in &self.quantities {
            if qty == &Quantity::Zero {
                // Check if var appears in any statement body
                for stmt in &self.statements {
                    if self.expr_contains_var(&stmt.body, var) {
                        errors.push(ValidationError::ZeroQuantityInRuntime(var.clone(), stmt.id));
                    }
                }
            }
        }

        // Check [1] vars appear exactly once in schedule (linearity)
        // This is a simplified check - full linearity requires dataflow analysis
        //
        // Run once per FUNCTION, not once per module: a `[1]` binding is consumed by
        // the body that owns it, so `input` being `[1]` and used once in each of three
        // functions is three correct single uses, not a triple use of one name.
        // Counting across the flattened statement list rejected `kernels/
        // quant_int8.naso`, which is exactly the flattening this type removes.
        for (statements, quantities) in self.quantity_scopes() {
            for (var, qty) in quantities {
                if qty == &Quantity::One {
                    // Only a CONSUMING operation counts; gates borrow. See
                    // `ir::validate` step 6 for why `count == 0` is not an error here:
                    // "must be consumed" is a source-level property enforced by the
                    // typechecker, which can see branches and returns and PIR cannot.
                    let count = statements
                        .iter()
                        .map(|s| self.count_in_expr(&s.body, var))
                        .sum();
                    if count > 1 {
                        errors.push(ValidationError::LinearVarUsedMultipleTimes(
                            var.clone(),
                            count,
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn expr_contains_var(&self, expr: &PirExpr, var: &str) -> bool {
        match expr {
            PirExpr::Var(v) => v == var,
            PirExpr::Binary { left, right, .. } => {
                self.expr_contains_var(left, var) || self.expr_contains_var(right, var)
            }
            PirExpr::Let { value, body, .. } => {
                self.expr_contains_var(value, var) || self.expr_contains_var(body, var)
            }
            PirExpr::Unary { expr, .. } => self.expr_contains_var(expr, var),
            PirExpr::Call { args, .. } => args.iter().any(|a| self.expr_contains_var(a, var)),
            PirExpr::Index { base, indices } => {
                self.expr_contains_var(base, var)
                    || indices.iter().any(|i| self.expr_contains_var(i, var))
            }
            PirExpr::Field { base, .. } => self.expr_contains_var(base, var),
            // A sequence's contents must be searched. Returning `false` here would
            // make every statement inside a loop body invisible to linearity
            // checking, so a linear value used only inside a loop would pass -- a
            // soundness hole, not a cosmetic one.
            PirExpr::Stmts(parts) => parts.iter().any(|p| self.expr_contains_var(p, var)),
            // BOTH sides: a linear value appearing in the target is being bound to a
            // location, and one in the value is being consumed. Both are uses.
            PirExpr::Assign { target, value } => {
                self.expr_contains_var(target, var) || self.expr_contains_var(value, var)
            }
            PirExpr::Cast { expr, .. } => self.expr_contains_var(expr, var),
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr_contains_var(cond, var)
                    || self.expr_contains_var(then_branch, var)
                    || self.expr_contains_var(else_branch, var)
            }
            PirExpr::Reversible { body, inverse } => {
                self.expr_contains_var(body, var) || self.expr_contains_var(inverse, var)
            }
            PirExpr::QuantumOp {
                op: _,
                args,
                qubits,
            } => {
                args.iter().any(|a| self.expr_contains_var(a, var))
                    || qubits.iter().any(|q| self.expr_contains_var(q, var))
            }
            PirExpr::IntLit(_) | PirExpr::FloatLit(_) | PirExpr::BoolLit(_) => false,
        }
    }

    /// The (statements, quantities) pairs a quantity-dependent check must run over.
    ///
    /// One pair per [`PirFunction`] when the module has function structure, and a
    /// single pair over the flat fields otherwise -- which is what every hand-built
    /// module and every `.pir` fixture has, so their behaviour is unchanged.
    ///
    /// The fallback is the whole point of keeping the flat fields: a module with no
    /// `functions` still gets its quantity checks, over exactly the statements it has.
    pub fn quantity_scopes(&self) -> Vec<(&[PirStatement], &QuantityMap)> {
        if self.functions.is_empty() {
            vec![(self.statements.as_slice(), &self.quantities)]
        } else {
            self.functions
                .iter()
                .map(|f| (f.statements.as_slice(), &f.quantities))
                .collect()
        }
    }

    /// The name of the function that becomes the module's entry point.
    ///
    /// `main` if the program declares one, otherwise the FIRST function in source
    /// order. A module with no `functions` at all -- a hand-built one, a `.pir`
    /// fixture -- has no entry name and returns `None`, and its single body is the
    /// entry by construction.
    ///
    /// The rule is stated rather than inferred so that a multi-function program has a
    /// DEFINED entry instead of several equally plausible candidates. `main` wins
    /// because it is what the source says; first-in-source-order is the fallback
    /// because it is the order the reader wrote them in, and it is deterministic.
    pub fn entry_function_name(&self) -> Option<&str> {
        if self.functions.is_empty() {
            return None;
        }
        if let Some(main) = self.functions.iter().find(|f| f.name == "main") {
            return Some(&main.name);
        }
        self.functions.first().map(|f| f.name.as_str())
    }

    fn count_in_expr(&self, expr: &PirExpr, var: &str) -> usize {
        match expr {
            PirExpr::Var(v) if v == var => 1,
            PirExpr::Binary { left, right, .. } => {
                self.count_in_expr(left, var) + self.count_in_expr(right, var)
            }
            PirExpr::Let { value, body, .. } => {
                self.count_in_expr(value, var) + self.count_in_expr(body, var)
            }
            PirExpr::Unary { expr, .. } => self.count_in_expr(expr, var),
            PirExpr::Call { args, .. } => args.iter().map(|a| self.count_in_expr(a, var)).sum(),
            PirExpr::Index { base, indices } => {
                self.count_in_expr(base, var)
                    + indices
                        .iter()
                        .map(|i| self.count_in_expr(i, var))
                        .sum::<usize>()
            }
            PirExpr::Field { base, .. } => self.count_in_expr(base, var),
            PirExpr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.count_in_expr(cond, var)
                    + self.count_in_expr(then_branch, var)
                    + self.count_in_expr(else_branch, var)
            }
            PirExpr::Reversible { body, inverse } => {
                self.count_in_expr(body, var) + self.count_in_expr(inverse, var)
            }
            PirExpr::QuantumOp { op, args, qubits } => {
                // Which quantum operations CONSUME their qubit, as opposed to
                // borrowing it?
                //
                // Every qubit reference used to be counted as one linear use, so a
                // second gate on the same qubit failed PIR validation:
                //
                //     fn f() { let [1] q: Qubit = qalloc(1); hadamard(q); hadamard(q); }
                //     -> IR validation failed: LinearVarUsedMultipleTimes("q", 2)
                //
                // That is not a linearity violation -- it is the opposite. A qubit is
                // linear, so it may be BORROWED any number of times; only being
                // CONSUMED spends it. `hadamard` and friends mutate the qubit and
                // leave the binding usable, which is exactly what `Mutability::InOut`
                // on the `hadamard` prelude signature already says. Counting a borrow
                // as a use made every real circuit -- which applies several gates in
                // sequence -- fail to lower, so no multi-gate quantum program could be
                // compiled at all.
                //
                // Only `measure` consumes: it collapses the qubit and the binding is
                // gone afterwards (the typechecker enforces the use-after-move). So a
                // qubit may appear under any number of gates, and at most once as the
                // target of a `measure`.
                //
                // A `measure` nested inside another op's arguments is not special
                // here: the `args` walk below still counts it, so the at-most-once
                // rule is enforced for every position.
                let consuming = crate::ir::validate::is_consuming_quantum_op(op);
                args.iter()
                    .map(|a| self.count_in_expr(a, var))
                    .sum::<usize>()
                    + if consuming {
                        qubits
                            .iter()
                            .map(|q| self.count_in_expr(q, var))
                            .sum::<usize>()
                    } else {
                        0
                    }
            }
            _ => 0,
        }
    }

    /// Pretty print the entire module
    pub fn pretty_print(&self) -> String {
        let mut s = String::new();
        s += "=== PIR Module ===\n";
        s += &format!("Parameters: {:?}\n", self.parameters);
        s += &format!("Quantities: {:?}\n", self.quantities);
        s += "\n--- Statements ---\n";
        for stmt in &self.statements {
            s += &format!(
                "  {}: qty={:?}, mut={:?}\n",
                stmt.id, stmt.quantity, stmt.mutability
            );
            s += &format!(
                "    Domain: {}\n",
                stmt.domain.name.as_deref().unwrap_or("")
            );
            s += &format!("    Body: {}\n", pir_expr_to_string(&stmt.body, 0));
        }
        s += "\n--- Schedule ---\n";
        s += &self.schedule.pretty_print();
        s += "\n--- Accesses ---\n";
        for access in &self.accesses.relations {
            s += &format!("  {}\n", access);
        }
        s
    }
}

/// Validation errors for PIR module
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationError {
    ScheduleError(String),
    UnscheduledStatement(StmtId),
    ZeroQuantityInRuntime(String, StmtId),
    LinearVarNotUsed(String),
    LinearVarUsedMultipleTimes(String, usize),
    AccessDomainMismatch(StmtId),
    DuplicateStatementId(StmtId),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::ScheduleError(e) => write!(f, "Schedule error: {}", e),
            ValidationError::UnscheduledStatement(id) => {
                write!(f, "Statement {} not in schedule", id)
            }
            ValidationError::ZeroQuantityInRuntime(var, id) => write!(
                f,
                "[0] variable '{}' appears in runtime statement {}",
                var, id
            ),
            ValidationError::LinearVarNotUsed(var) => write!(f, "[1] variable '{}' not used", var),
            ValidationError::LinearVarUsedMultipleTimes(var, count) => {
                write!(f, "[1] variable '{}' used {} times", var, count)
            }
            ValidationError::AccessDomainMismatch(id) => {
                write!(f, "Access domain mismatch for statement {}", id)
            }
            ValidationError::DuplicateStatementId(id) => {
                write!(f, "Duplicate statement ID: {}", id)
            }
        }
    }
}

impl std::error::Error for ValidationError {}

fn pir_expr_to_string(expr: &PirExpr, _indent: usize) -> String {
    match expr {
        PirExpr::IntLit(v) => format!("{}", v),
        PirExpr::FloatLit(v) => format!("{}", v),
        PirExpr::BoolLit(v) => format!("{}", v),
        PirExpr::Var(v) => v.clone(),
        PirExpr::Assign { target, value } => {
            format!(
                "{} = {}",
                pir_expr_to_string(target, 0),
                pir_expr_to_string(value, 0)
            )
        }
        PirExpr::Cast {
            expr,
            width,
            signed,
            float_target,
        } => match float_target {
            // A float target is printed as a float. Printing it as `i{width}` would be
            // a lie about the conversion the dump claims to be showing.
            Some(_) => format!("{} as f64", pir_expr_to_string(expr, 0)),
            None => format!(
                "{} as {}i{}",
                pir_expr_to_string(expr, 0),
                if *signed { "" } else { "u" },
                width.unwrap_or(32)
            ),
        },
        PirExpr::Stmts(parts) => {
            let inner: Vec<String> = parts.iter().map(|p| pir_expr_to_string(p, 0)).collect();
            if inner.is_empty() {
                "{}".to_string()
            } else {
                format!("{{ {}; }}", inner.join("; "))
            }
        }
        PirExpr::Binary { op, left, right } => {
            format!(
                "({} {} {})",
                pir_expr_to_string(left, 0),
                binary_op_to_str(*op),
                pir_expr_to_string(right, 0)
            )
        }
        PirExpr::Unary { op, expr } => {
            format!("{}{}", unary_op_to_str(*op), pir_expr_to_string(expr, 0))
        }
        PirExpr::Call { name, args } => {
            format!(
                "{}({})",
                name,
                args.iter()
                    .map(|a| pir_expr_to_string(a, 0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        PirExpr::Index { base, indices } => {
            format!(
                "{}[{}]",
                pir_expr_to_string(base, 0),
                indices
                    .iter()
                    .map(|i| pir_expr_to_string(i, 0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        PirExpr::Field { base, field } => {
            format!("{}.{}", pir_expr_to_string(base, 0), field)
        }
        PirExpr::Let {
            name,
            qty,
            mutability,
            value,
            body,
        } => {
            format!(
                "let {:?} {:?} {} = {}; {}",
                qty,
                mutability,
                name,
                pir_expr_to_string(value, 0),
                pir_expr_to_string(body, 0)
            )
        }
        PirExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            format!(
                "if {} then {} else {}",
                pir_expr_to_string(cond, 0),
                pir_expr_to_string(then_branch, 0),
                pir_expr_to_string(else_branch, 0)
            )
        }
        PirExpr::Reversible { body, inverse } => {
            format!(
                "reversible {} inv {}",
                pir_expr_to_string(body, 0),
                pir_expr_to_string(inverse, 0)
            )
        }
        PirExpr::QuantumOp { op, args, qubits } => {
            format!(
                "quantum {}({}, qubits={})",
                op,
                args.iter()
                    .map(|a| pir_expr_to_string(a, 0))
                    .collect::<Vec<_>>()
                    .join(", "),
                qubits
                    .iter()
                    .map(|q| pir_expr_to_string(q, 0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn binary_op_to_str(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::Xor => "^",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
    }
}

fn unary_op_to_str(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

#[cfg(test)]
mod tests {
    use super::super::access_relation::{AccessRelation, AccessRelations, AccessType};
    use super::super::affine_domain::AffineDomain;
    use super::super::affine_map::{AffineMap, Matrix};
    use super::super::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};
    use super::*;
    use crate::ast::{Mutability, Quantity};

    #[test]
    fn test_pir_module_creation() {
        let domain = AffineDomain::universe(1, 0);
        let mut m = Matrix::new(1, 1);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        let stmt = PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::IntLit(42),
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let schedule = ScheduleTree::new(
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            vec![],
        );

        let mut accesses = AccessRelations::new();
        accesses.add(AccessRelation::new(
            StmtId(0),
            domain.clone(),
            map,
            AccessType::Write,
        ));

        let mut quantities = QuantityMap::new();
        quantities.insert("x".to_string(), Quantity::Many);

        let module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);
        assert!(module.validate().is_ok());
    }

    #[test]
    fn test_zero_quantity_validation() {
        let domain = AffineDomain::universe(1, 0);
        let mut m = Matrix::new(1, 1);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        let stmt = PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::Var("x".to_string()), // Uses zero-quantity var
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let schedule = ScheduleTree::new(
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            vec![],
        );

        let mut accesses = AccessRelations::new();
        accesses.add(AccessRelation::new(
            StmtId(0),
            domain.clone(),
            map,
            AccessType::Write,
        ));

        let mut quantities = QuantityMap::new();
        quantities.insert("x".to_string(), Quantity::Zero); // [0] quantity

        let module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);
        let result = module.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ZeroQuantityInRuntime(_, _)))
        );
    }

    #[test]
    fn test_linear_var_validation() {
        let domain = AffineDomain::universe(1, 0);
        let mut m = Matrix::new(1, 1);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        // Statement uses linear var twice
        let stmt = PirStatement {
            id: StmtId(0),
            domain: domain.clone(),
            body: PirExpr::Binary {
                op: BinaryOp::Add,
                left: Box::new(PirExpr::Var("x".to_string())),
                right: Box::new(PirExpr::Var("x".to_string())),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };

        let schedule = ScheduleTree::new(
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            vec![],
        );

        let mut accesses = AccessRelations::new();
        accesses.add(AccessRelation::new(
            StmtId(0),
            domain.clone(),
            map,
            AccessType::Write,
        ));

        let mut quantities = QuantityMap::new();
        quantities.insert("x".to_string(), Quantity::One); // [1] quantity

        let module = PirModule::new(vec![stmt], schedule, accesses, quantities, vec![]);
        let result = module.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::LinearVarUsedMultipleTimes(_, 2)))
        );
    }
}
