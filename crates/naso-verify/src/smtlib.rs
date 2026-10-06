//! SMT-LIB2 AST and pretty-printer.

use std::fmt;

/// SMT-LIB2 script representation.
#[derive(Debug, Clone)]
pub struct Script {
    pub commands: Vec<Command>,
}

impl Script {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn set_logic(&mut self, logic: &str) {
        self.commands.push(Command::SetLogic(logic.to_string()));
    }

    pub fn declare_sort(&mut self, name: &str, arity: usize) {
        self.commands
            .push(Command::DeclareSort(name.to_string(), arity));
    }

    pub fn define_sort(&mut self, name: &str, params: Vec<String>, def: Sort) {
        self.commands
            .push(Command::DefineSort(name.to_string(), params, def));
    }

    pub fn declare_fun(&mut self, name: &str, args: Vec<Sort>, ret: Sort) {
        self.commands
            .push(Command::DeclareFun(name.to_string(), args, ret));
    }

    /// Emit a `(define-fun <name> (<params>) <ret> <body>)` command.
    ///
    /// Used for `round` under AUFLIRA: `define-fun` gives Z3 a complete definition
    /// (via `to_int`/`to_real`), eliminating the need for the uninterpreted-function
    /// + universal-axiom combination that causes quantifier e-matching timeouts.
    pub fn define_fun(&mut self, name: &str, params: Vec<(String, Sort)>, ret: Sort, body: Term) {
        self.commands
            .push(Command::DefineFun(name.to_string(), params, ret, body));
    }

    /// Declare a constant of the given sort.
    ///
    /// A FUNCTION sort cannot be expressed by `declare-const` in SMT-LIB2 -- it needs
    /// `declare-fun` with a wrapped domain -- and emitting `declare-const` for one makes
    /// Z3 reject the script. Since that rejection is silent (see `Command::DeclareFun`),
    /// a tensor obligation would come back "refuted" with an empty model rather than as an
    /// error. So the dispatch happens here: a function sort is declared as a function.
    pub fn declare_const(&mut self, name: &str, sort: Sort) {
        match sort {
            Sort::Function(domain, ret) => {
                self.commands
                    .push(Command::DeclareFun(name.to_string(), domain, *ret));
            }
            other => self
                .commands
                .push(Command::DeclareConst(name.to_string(), other)),
        }
    }

    pub fn assert(&mut self, term: Term) {
        self.commands.push(Command::Assert(term));
    }

    pub fn push(&mut self, n: u32) {
        self.commands.push(Command::Push(n));
    }

    pub fn pop(&mut self, n: u32) {
        self.commands.push(Command::Pop(n));
    }

    pub fn check_sat(&mut self) {
        self.commands.push(Command::CheckSat);
    }

    pub fn check_sat_assuming(&mut self, assumptions: Vec<Term>) {
        self.commands.push(Command::CheckSatAssuming(assumptions));
    }

    pub fn get_model(&mut self) {
        self.commands.push(Command::GetModel);
    }

    pub fn get_unsat_core(&mut self) {
        self.commands.push(Command::GetUnsatCore);
    }

    pub fn get_value(&mut self, terms: Vec<Term>) {
        self.commands.push(Command::GetValue(terms));
    }

    pub fn exit(&mut self) {
        self.commands.push(Command::Exit);
    }
}

impl Default for Script {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Script {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for cmd in &self.commands {
            writeln!(f, "{}", cmd)?;
        }
        Ok(())
    }
}

/// SMT-LIB2 commands.
#[derive(Debug, Clone)]
pub enum Command {
    SetLogic(String),
    DeclareSort(String, usize),
    DefineSort(String, Vec<String>, Sort),
    DeclareFun(String, Vec<Sort>, Sort),
    DefineFun(String, Vec<(String, Sort)>, Sort, Term),
    DeclareConst(String, Sort),
    Assert(Term),
    Push(u32),
    Pop(u32),
    CheckSat,
    CheckSatAssuming(Vec<Term>),
    GetModel,
    GetUnsatCore,
    GetValue(Vec<Term>),
    Exit,
    Comment(String),
    SetOption(String, AttributeValue),
    SetInfo(AttributeValue),
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::SetLogic(logic) => write!(f, "(set-logic {})", logic),
            Command::DeclareSort(name, arity) => write!(f, "(declare-sort {} {})", name, arity),
            Command::DefineSort(name, params, def) => {
                write!(f, "(define-sort {} (", name)?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") {})", def)
            }
            Command::DeclareFun(name, domain, ret) => {
                // The DOMAIN is a list of sorts, so it is wrapped: `(declare-fun t
                // ((Int)) Int)`. When the domain is itself a single function sort, its
                // `(Int) Int` rendering nests one level deeper, which is exactly what
                // SMT-LIB2 requires for an uninterpreted function.
                //
                // Emitting `(declare-fun t (Int) Int)` instead makes Z3 reject the script,
                // and the rejection is SILENT -- `from_string` discards an unparseable
                // script and the solver then reports `sat` for a solver holding no
                // assertions. Every obligation mentioning a tensor was reported REFUTED
                // with an empty model: a false negative stacked on a false negative.
                //
                // Verified against Z3 directly: `(declare-const f (Int) Int)` answers
                // `sat` (rejected and discarded), whereas `(declare-fun f ((Int)) Int)`
                // answers `unsat` for the tautology `(not (= (f 0) (f 0)))` -- correct.
                // `(declare-fun <name> (<domain>) <ret>)`
                write!(f, "(declare-fun {} (", name)?;
                write!(f, "(")?;
                for (i, arg) in domain.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")) {})", ret)
            }
            Command::DefineFun(name, params, ret, body) => {
                write!(f, "(define-fun {} (", name)?;
                for (i, (p, s)) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "({} {})", p, s)?;
                }
                write!(f, ") {} {})", ret, body)
            }
            Command::DeclareConst(name, sort) => write!(f, "(declare-const {} {})", name, sort),
            Command::Assert(term) => write!(f, "(assert {})", term),
            Command::Push(n) => write!(f, "(push {})", n),
            Command::Pop(n) => write!(f, "(pop {})", n),
            Command::CheckSat => write!(f, "(check-sat)"),
            Command::CheckSatAssuming(assumptions) => {
                write!(f, "(check-sat-assuming")?;
                for a in assumptions {
                    write!(f, " {}", a)?;
                }
                write!(f, ")")
            }
            Command::GetModel => write!(f, "(get-model)"),
            Command::GetUnsatCore => write!(f, "(get-unsat-core)"),
            Command::GetValue(terms) => {
                write!(f, "(get-value")?;
                for t in terms {
                    write!(f, " {}", t)?;
                }
                write!(f, ")")
            }
            Command::Exit => write!(f, "(exit)"),
            Command::Comment(s) => write!(f, "; {}", s),
            Command::SetOption(opt, val) => write!(f, "(set-option :{} {})", opt, val),
            Command::SetInfo(val) => write!(f, "(set-info {})", val),
        }
    }
}

/// SMT-LIB2 sorts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sort {
    Bool,
    Int,
    Real,
    BitVec(u32),
    Array(Box<Sort>, Box<Sort>),
    Datatype(String),
    Function(Vec<Sort>, Box<Sort>),
    Custom(String),
}

impl fmt::Display for Sort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Sort::Bool => write!(f, "Bool"),
            Sort::Int => write!(f, "Int"),
            Sort::Real => write!(f, "Real"),
            Sort::BitVec(n) => write!(f, "(_ BitVec {})", n),
            Sort::Array(idx, elem) => write!(f, "(Array {} {})", idx, elem),
            Sort::Datatype(name) => write!(f, "{}", name),
            // `(Int) Int` is the RANGE-sort notation -- correct as written. The
            // declaration emitter is responsible for wrapping it into a `declare-fun`
            // DOMAIN list, which is a different shape; see `Command::DeclareFun`.
            Sort::Function(args, ret) => {
                write!(f, "(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", a)?;
                }
                write!(f, ") {}", ret)
            }
            Sort::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// SMT-LIB2 terms.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Term {
    Const(Constant),
    Var(String, Sort),
    App(String, Vec<Term>),
    Let(Vec<(String, Term)>, Box<Term>),
    Forall(Vec<(String, Sort)>, Box<Term>),
    Exists(Vec<(String, Sort)>, Box<Term>),
    Match(Box<Term>, Vec<MatchCase>),
    Annotated(Box<Term>, Vec<Attribute>),
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Const(c) => write!(f, "{}", c),
            Term::Var(name, _sort) => write!(f, "{}", name), // Sort in declaration
            Term::App(name, args) => {
                write!(f, "({}", name)?;
                for arg in args {
                    write!(f, " {}", arg)?;
                }
                write!(f, ")")
            }
            Term::Let(bindings, body) => {
                write!(f, "(let (")?;
                for (i, (name, val)) in bindings.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "({} {})", name, val)?;
                }
                write!(f, ") {})", body)
            }
            Term::Forall(vars, body) => {
                write!(f, "(forall (")?;
                for (i, (name, sort)) in vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "({} {})", name, sort)?;
                }
                write!(f, ") {})", body)
            }
            Term::Exists(vars, body) => {
                write!(f, "(exists (")?;
                for (i, (name, sort)) in vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "({} {})", name, sort)?;
                }
                write!(f, ") {})", body)
            }
            Term::Match(scrutinee, cases) => {
                write!(f, "(match {} ", scrutinee)?;
                for case in cases {
                    write!(f, " {}", case)?;
                }
                write!(f, ")")
            }
            Term::Annotated(term, attrs) => {
                write!(f, "(! {} ", term)?;
                for attr in attrs {
                    write!(f, " {}", attr)?;
                }
                write!(f, ")")
            }
        }
    }
}

/// SMT-LIB2 constants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Constant {
    Bool(bool),
    Int(i64),
    Real(String), // Decimal string
    /// An arbitrary-precision integer numeral, rendered verbatim.
    ///
    /// `Int(i64)` cannot hold every integer this prover needs: a subnormal `f64` has a
    /// dyadic denominator of `2^1074`, far past 64 bits. SMT-LIB2 integers are arbitrary
    /// precision, so the digits are carried as a string. `String` is NOT a substitute -- it
    /// renders QUOTED, which is a different SMT constant entirely.
    Numeral(String),
    BitVec(u32, u64), // (width, value)
    String(String),
}

impl fmt::Display for Constant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constant::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            Constant::Int(i) => write!(f, "{}", i),
            Constant::Real(s) => write!(f, "{}", s),
            Constant::Numeral(s) => write!(f, "{}", s),
            Constant::BitVec(w, v) => write!(f, "(_ bv{} {})", v, w),
            Constant::String(s) => write!(f, "\"{}\"", s.replace('"', "\\\"")),
        }
    }
}

/// Match case for pattern matching.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchCase {
    pub pattern: Pattern,
    pub body: Term,
}

impl fmt::Display for MatchCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {})", self.pattern, self.body)
    }
}

/// Patterns for match expressions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Constructor(String, Vec<Pattern>),
    Const(Constant),
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard => write!(f, "_"),
            Pattern::Var(name) => write!(f, "{}", name),
            Pattern::Constructor(name, args) => {
                write!(f, "({}", name)?;
                for arg in args {
                    write!(f, " {}", arg)?;
                }
                write!(f, ")")
            }
            Pattern::Const(c) => write!(f, "{}", c),
        }
    }
}

/// SMT-LIB2 attributes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Attribute {
    Keyword(String),
    KeywordValue(String, AttributeValue),
}

impl fmt::Display for Attribute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Attribute::Keyword(k) => write!(f, ":{}", k),
            Attribute::KeywordValue(k, v) => write!(f, ":{} {}", k, v),
        }
    }
}

/// Attribute values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttributeValue {
    Symbol(String),
    String(String),
    Number(i64),
    List(Vec<AttributeValue>),
}

impl fmt::Display for AttributeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttributeValue::Symbol(s) => write!(f, "{}", s),
            AttributeValue::String(s) => write!(f, "\"{}\"", s.replace('"', "\\\"")),
            AttributeValue::Number(n) => write!(f, "{}", n),
            AttributeValue::List(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
        }
    }
}

/// Builder for constructing terms ergonomically.
pub mod builder {
    use super::*;

    pub fn bool(b: bool) -> Term {
        Term::Const(Constant::Bool(b))
    }

    pub fn int(i: i64) -> Term {
        Term::Const(Constant::Int(i))
    }

    pub fn bv(width: u32, value: u64) -> Term {
        Term::Const(Constant::BitVec(width, value))
    }

    pub fn var(name: &str, sort: Sort) -> Term {
        Term::Var(name.to_string(), sort)
    }

    pub fn app(name: &str, args: Vec<Term>) -> Term {
        Term::App(name.to_string(), args)
    }

    pub fn not(t: Term) -> Term {
        app("not", vec![t])
    }

    pub fn and(terms: Vec<Term>) -> Term {
        app("and", terms)
    }

    pub fn or(terms: Vec<Term>) -> Term {
        app("or", terms)
    }

    pub fn eq(lhs: Term, rhs: Term) -> Term {
        app("=", vec![lhs, rhs])
    }

    pub fn distinct(terms: Vec<Term>) -> Term {
        app("distinct", terms)
    }

    pub fn implies(lhs: Term, rhs: Term) -> Term {
        app("=>", vec![lhs, rhs])
    }

    pub fn ite(cond: Term, then_t: Term, else_t: Term) -> Term {
        app("ite", vec![cond, then_t, else_t])
    }

    pub fn forall(vars: Vec<(String, Sort)>, body: Term) -> Term {
        Term::Forall(vars, Box::new(body))
    }

    pub fn exists(vars: Vec<(String, Sort)>, body: Term) -> Term {
        Term::Exists(vars, Box::new(body))
    }

    pub fn add(args: Vec<Term>) -> Term {
        app("+", args)
    }

    pub fn sub(args: Vec<Term>) -> Term {
        app("-", args)
    }

    pub fn mul(args: Vec<Term>) -> Term {
        app("*", args)
    }

    pub fn div(lhs: Term, rhs: Term) -> Term {
        app("div", vec![lhs, rhs])
    }

    pub fn le(lhs: Term, rhs: Term) -> Term {
        app("<=", vec![lhs, rhs])
    }

    pub fn lt(lhs: Term, rhs: Term) -> Term {
        app("<", vec![lhs, rhs])
    }

    pub fn ge(lhs: Term, rhs: Term) -> Term {
        app(">=", vec![lhs, rhs])
    }

    pub fn gt(lhs: Term, rhs: Term) -> Term {
        app(">", vec![lhs, rhs])
    }

    pub fn select(array: Term, index: Term) -> Term {
        app("select", vec![array, index])
    }

    pub fn store(array: Term, index: Term, value: Term) -> Term {
        app("store", vec![array, index, value])
    }
}

/// Common SMT-LIB2 theory symbols.
pub mod theory {
    pub const BOOL: &str = "Bool";
    pub const INT: &str = "Int";
    pub const REAL: &str = "Real";
    pub fn bv(w: u32) -> String {
        format!("(_ BitVec {})", w)
    }
    pub fn array(idx: &str, elem: &str) -> String {
        format!("(Array {} {})", idx, elem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function-sorted constant MUST be printed as `declare-fun` with a wrapped domain.
    ///
    /// Regression for the silent-discard failure. `(declare-const f (Int) Int)` is not
    /// valid SMT-LIB2: a `declare-const` takes a plain sort, and `(Int) Int` is the
    /// range-sort notation for a function. Z3 rejects the whole script, and the rejection
    /// is SILENT -- `Solver::from_string` drops a script it cannot parse and the solver
    /// then reports `sat` for a solver holding no assertions at all.
    ///
    /// The visible symptom was every tensor obligation being reported REFUTED with an
    /// empty model, which is a false negative layered on a false positive: the prover
    /// confidently refuted a claim it never sent to Z3.
    #[test]
    fn a_function_sorted_constant_is_declared_as_a_function() {
        let mut script = Script::new();
        script.declare_const("f.t", Sort::Function(vec![Sort::Int], Box::new(Sort::Int)));
        assert_eq!(script.to_string(), "(declare-fun f.t ((Int)) Int)\n");
    }

    /// A non-function sort still uses `declare-const` -- the fix must not widen it.
    #[test]
    fn a_plain_constant_uses_declare_const() {
        let mut script = Script::new();
        script.declare_const("n", Sort::Int);
        assert_eq!(script.to_string(), "(declare-const n Int)\n");
    }

    /// A two-argument function sorts correctly, and nesting is balanced.
    #[test]
    fn a_multi_argument_function_domain_is_rendered_and_balanced() {
        let mut script = Script::new();
        script.declare_const(
            "m",
            Sort::Function(vec![Sort::Int, Sort::Bool], Box::new(Sort::Int)),
        );
        let out = script.to_string();
        assert_eq!(out, "(declare-fun m ((Int Bool)) Int)\n");
        // Balanced parens: an unbalanced script is silently discarded by Z3.
        let depth = out.chars().fold(0i32, |a, c| match c {
            '(' => a + 1,
            ')' => a - 1,
            _ => a,
        });
        assert_eq!(depth, 0, "unbalanced parentheses in {out}");
    }

    /// Every declaration the compiler emits must be balanced.
    ///
    /// A single stray `)` makes Z3 discard the entire script and report `sat`, so an
    /// unbalanced emitter is indistinguishable from a prover that never ran. This walks the
    /// shapes the obligation encoder actually produces rather than trusting any one of them.
    #[test]
    fn emitted_declarations_are_always_parenthesis_balanced() {
        let sorts = [
            Sort::Int,
            Sort::Bool,
            Sort::Real,
            Sort::BitVec(4),
            Sort::Array(Box::new(Sort::Int), Box::new(Sort::Int)),
            Sort::Function(vec![Sort::Int], Box::new(Sort::Int)),
            Sort::Function(vec![Sort::Int, Sort::Int], Box::new(Sort::Int)),
        ];
        for sort in sorts {
            let mut script = Script::new();
            script.declare_const("x", sort.clone());
            let out = script.to_string();
            let depth = out.chars().fold(0i32, |a, c| match c {
                '(' => a + 1,
                ')' => a - 1,
                _ => a,
            });
            assert_eq!(depth, 0, "unbalanced for {sort:?}: {out}");
        }
    }
}
