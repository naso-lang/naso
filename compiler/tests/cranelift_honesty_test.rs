//! Cranelift must refuse, not return a stub constant.
//!
//! # What it did
//!
//! `compile_and_execute` took a `&PirModule` and ignored it. It JIT-compiled a
//! function returning the literal `42` and returned that as the program's result.
//! Reached from the CLI, that was worse than wrong, because it was indistinguishable
//! from success:
//!
//! ```text
//! $ naso build --target cranelift kernels/sum_1_to_10.naso   # LLVM: 45
//! JIT execution result: 42
//! $ naso build --target cranelift kernels/returns_7.naso      # LLVM: 7
//! JIT execution result: 42
//! ```
//!
//! Two different programs, the same output, exit status 0. `42` is the JIT stub's own
//! constant, not a value anything computed. Nothing downstream could detect it: the
//! number is a plausible `i32` and the call succeeded.
//!
//! This is the single worst defect found in the backend audit, and it is the one the
//! other audits could not have found. Every earlier hole was found by asking "what
//! does this backend do with X". This one was found by asking whether the backend
//! works at all.
//!
//! # Why it survived
//!
//! **The `cranelift` feature is never built or tested in CI.** Every workflow line
//! passes `--features llvm`, so this file had not been compiled by a single check.
//! That is why two fixes are needed and not one: refuse here, and build the feature in
//! CI so the next gap in it cannot hide behind a feature flag nobody enables.
//!
//! # What is kept
//!
//! `CraneliftJit` still works as a JIT harness and its constant is honestly named
//! `trivial_fn`. It takes no PIR, so it cannot be reachable from the pipeline. The
//! tests below pin that separation.

#[cfg(all(feature = "llvm", feature = "cranelift"))]
mod suite {

    use naso_compiler::ast::{Mutability, Quantity};
    #[cfg(not(feature = "llvm"))]
    use naso_compiler::codegen::context::CodegenContext;
    #[cfg(feature = "llvm")]
    use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
    use naso_compiler::codegen::cranelift::jit::compile_and_execute;
    use naso_compiler::ir::affine_domain::AffineDomain;
    use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
    use naso_compiler::ir::schedule_tree::StmtId;

    /// A module whose single statement is `body`.
    ///
    /// `body` differs per test: the point is that the result is identical regardless.
    fn module_with(body: PirExpr) -> PirModule {
        let mut m = PirModule::default();
        m.quantities.insert("q".to_string(), Quantity::One);
        m.statements.push(PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body,
            quantity: Quantity::One,
            mutability: Mutability::Immutable,
            span: None,
        });
        m
    }

    /// The value the old stub always returned.
    const STUB: i32 = 42;

    /// Call `compile_and_execute` with a real `CodegenContext`.
    ///
    /// The signature takes one even though the refusal ignores it, and building one needs
    /// the `llvm` feature -- which is why this file is gated on `llvm` as well as
    /// `cranelift`. That is not a dodge: CI separately proves that `--features cranelift`
    /// ALONE builds and links, which is the property that was actually broken. Asserting
    /// behaviour through a `cranelift`-only build would mean constructing a context that
    /// build cannot have.
    fn call(module: &PirModule) -> Result<i32, String> {
        let ctx = CodegenContext::new(CodegenTarget::Host, OptLevel::None).unwrap();
        compile_and_execute(module, &ctx).map_err(|e| e.to_string())
    }

    /// The module entry point must REFUSE, not return a constant.
    ///
    /// This is the regression. It returned `Ok(42)` for every input.
    #[test]
    fn the_module_entry_point_refuses() {
        let err = call(&module_with(PirExpr::IntLit(1)))
            .expect_err("Cranelift must refuse rather than return a stub constant");
        let msg = err.to_string();
        assert!(
            msg.contains("not") && msg.to_lowercase().contains("cranelift"),
            "the refusal must say what is not implemented: {msg}"
        );
    }

    /// The refusal must not be `Ok(STUB)` for a trivial body.
    ///
    /// The exact shape of the bug: a one-integer module whose JIT "answer" was 42 and
    /// not 1. If this passes, the entry point is still routing through the stub.
    #[test]
    fn it_does_not_return_the_stub_constant() {
        if let Ok(v) = call(&module_with(PirExpr::IntLit(1))) {
            panic!(
                "returned Ok({v}) for a module that computes nothing meaningful. \
                 The stub constant is {STUB}; returning it is a fabricated answer."
            );
        }
    }

    /// Two structurally different modules must not produce the same "result".
    ///
    /// This is the property that made the bug invisible. One integer and a block of two
    /// are different programs, so a backend that returns one fixed `i32` for both is
    /// demonstrably not executing either.
    #[test]
    fn different_modules_do_not_yield_the_same_answer() {
        let a = module_with(PirExpr::IntLit(1));
        let b = module_with(PirExpr::Stmts(vec![PirExpr::IntLit(1), PirExpr::IntLit(2)]));
        let ra = call(&a);
        let rb = call(&b);
        if let (Ok(x), Ok(y)) = (ra, rb) {
            panic!(
                "both modules returned Ok: {x} and {y}. Identical answers for different \
                 programs means nothing was executed."
            );
        }
    }

    /// A module containing a linear value must not be silently executed either.
    ///
    /// Linearity is checked at the QIR and WGSL boundaries. Cranelift refuses everything,
    /// so this is about the refusal being unconditional rather than conditional on the
    /// module happening to be well-formed.
    #[test]
    fn a_module_with_a_linear_value_also_refuses() {
        let body = PirExpr::Stmts(vec![
            PirExpr::Var(String::from("q")),
            PirExpr::Var(String::from("q")),
        ]);
        assert!(
            call(&module_with(body)).is_err(),
            "a doubled [1] value must not produce an Ok result from a stub"
        );
    }

    /// The refusal must be an error, not a panic, and not a silent zero.
    ///
    /// A panic would be visible but wrong as a contract; a silent zero would repeat the
    /// exact defect this exists to remove.
    #[test]
    fn the_refusal_is_a_clean_error_not_a_panic() {
        // A panic here would unwind the test rather than return; reaching the assert means
        // the refusal was a `Result::Err`.
        let r = call(&module_with(PirExpr::IntLit(0)));
        match r {
            Err(e) => assert!(
                !e.to_string().is_empty(),
                "an error with no message is as unhelpful as no error"
            ),
            Ok(v) => panic!("returned Ok({v})"),
        }
    }
}
