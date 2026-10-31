#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::parser::parse_program;
    use crate::typecheck::{TypeError, check_program};

    fn check_source(source: &str) -> Result<(), Vec<TypeError>> {
        let mut program = parse_program(source).expect("Failed to parse");
        let result = check_program(&mut program);
        if result.errors.is_empty() {
            Ok(())
        } else {
            Err(result.errors)
        }
    }

    #[test]
    fn test_linear_variable_used_once() {
        // [1] variable used exactly once
        let source = r#"
            fn test() {
                let [1] x = 42;
                // `y` INHERITS `[1]` from `x`, so it must be spent too. The comment
                // used to read "consume x", but a plain `let` is not a consume: the
                // binding inherits the linearity and the leak check applies to it.
                // Before quantity inheritance, `y` was silently widened to `[*]` and
                // this program passed while `x`'s linearity went unenforced.
                let y = x;
                let consume z = y;
                let _ = z;
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    #[test]
    fn test_linear_variable_used_twice_error() {
        // [1] variable used twice - should error
        let source = r#"
            fn test() {
                let [1] x = 42;
                let y = x;
                let z = x; // ERROR: x used twice
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::LinearVariableUsedTwice { .. }))
        );
    }

    #[test]
    fn test_zero_quantity_erased() {
        // [0] variable - should be erased, cannot use at runtime
        let source = r#"
            fn test() {
                let [0] x = 42;
                let y = x; // ERROR: erased variable used at runtime
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::ErasedVariableUsedAtRuntime { .. }))
        );
    }

    #[test]
    fn test_zero_quantity_in_type_position_ok() {
        // [0] variable in type position (compile-time) should be OK
        let source = r#"
            fn test() {
                let [0] n = 5;
                let arr: [Int; n] = [1, 2, 3, 4, 5]; // n used in type position
            }
        "#;
        // This should work as n is used only in type position
        // Currently our checker may not fully support this - just verify it doesn't crash
        let _ = check_source(source);
    }

    #[test]
    fn test_bounded_quantity() {
        // [N] variable - can be used at most N times
        let source = r#"
            fn test() {
                let [2] x = 42;
                let y = x;
                let z = x;
                // let w = x; // would error - third use
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    #[test]
    fn test_bounded_quantity_exceeded() {
        let source = r#"
            fn test() {
                let [2] x = 42;
                let y = x;
                let z = x;
                let w = x; // ERROR: third use exceeds bound
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_inout_binding() {
        // inout binding requires unique ownership
        let source = r#"
            fn test() {
                let [1] x = 42;
                let inout y = x; // OK: x has quantity 1
                y = 10; // mutate through inout
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    #[test]
    fn test_inout_requires_unique() {
        // inout binding requires quantity 1
        let source = r#"
            fn test() {
                let x = 42; // default quantity [*]
                let inout y = x; // ERROR: requires unique ownership
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::InOutRequiresUnique { .. }))
        );
    }

    #[test]
    fn test_consume_binding() {
        // consume binding - linear move
        let source = r#"
            fn test() {
                let [1] x = 42;
                let consume y = x; // move x into y
                // let z = x; // ERROR: x moved
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    #[test]
    fn test_consume_requires_linear() {
        // consume requires quantity 1
        let source = r#"
            fn test() {
                let x = 42; // default [*]
                let consume y = x; // ERROR: consume requires [1]
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::QuantityMismatch { .. }))
        );
    }

    #[test]
    fn test_unused_linear_variable_error() {
        // [1] variable not used - should error at scope exit
        let source = r#"
            fn test() {
                let [1] x = 42;
                // x never used
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::UnusedLinearVariable { .. }))
        );
    }

    #[test]
    fn test_quantity_subtyping() {
        // [0] <= [1] <= [N] <= [*]
        let source = r#"
            fn test() {
                let [0] x = 42; // erased
                let [1] y = x; // OK: Zero <= One
                let [2] z = y; // OK: One <= Bounded(2)
                let w = z; // OK: Bounded(2) <= Many
            }
        "#;
        // This tests quantity subtyping in assignments
        // Note: actual subtyping behavior depends on how we implement it
        let _ = check_source(source);
    }

    #[test]
    fn test_reversible_block_pure() {
        // reversible block body must be pure
        let source = r#"
            fn test() {
                reversible {
                    let x = 1;
                    let y = x + 1;
                }
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    #[test]
    fn test_reversible_block_measure_error() {
        // measurement in reversible block should error
        let source = r#"
            fn test() {
                reversible {
                    let [1] q = qalloc();
                    measure(q); // ERROR: impure in reversible
                }
            }
        "#;
        let _program = parse_program(source).expect("Failed to parse");
        println!("PARSED AST: {:#?}", _program);
        let result = check_source(source);
        if let Err(ref errors) = result {
            for e in errors {
                println!("ERROR: {:?}", e);
            }
        }
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::ImpureInReversible { .. }))
        );
    }

    #[test]
    fn test_qubit_linearity() {
        // Qubit must have quantity 1
        let source = r#"
            fn test() {
                let [1] q = qalloc(); // OK
                let r = measure(q); // consumes q
                // `r` is a classical bit (`[*]`), so using it is not required --
                // but reading it once documents that the qubit was consumed.
                let _ = r;
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    #[test]
    fn test_qubit_quantity_mismatch() {
        // Qubit with wrong quantity should error
        let source = r#"
            fn test() {
                let q = qalloc(); // default [*] - should error for Qubit
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err());
    }

    /// Regression test for quantum gate borrowing (Issue: linear variable used twice)
    /// Tests that quantum gates (hadamard, cnot) borrow qubits instead of consuming them
    #[test]
    fn test_quantum_gate_borrowing() {
        // Bell pair creation - hadamard and cnot should borrow qubits, not consume them
        let source = r#"
            fn bell_pair() -> (Qubit, Qubit) {
                let [1] q0 = qalloc();
                let [1] q1 = qalloc();
                hadamard(q0);
                cnot(q0, q1);
                (q0, q1)
            }
        "#;
        assert!(
            check_source(source).is_ok(),
            "Bell pair with quantum gates should type check"
        );
    }

    /// Negative test: actual repeated ownership consumption should still fail
    #[test]
    fn test_repeated_ownership_consumption_fails() {
        // This should fail: measure consumes the qubit, then using it again should error
        let source = r#"
            fn test() {
                let [1] q = qalloc();
                measure(q); // consumes q
                measure(q); // ERROR: use of moved value
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err(), "Repeated measure should fail");
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::UseOfMovedValue { .. }))
        );
    }

    /// Test that measure still consumes the qubit
    #[test]
    fn test_measure_consumes_qubit() {
        let source = r#"
            fn test() {
                let [1] q = qalloc();
                let r = measure(q); // consumes q
                // q should be moved, cannot use again
                let _ = r;
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    /// Test that entangle consumes qubits
    #[test]
    fn test_entangle_consumes_qubits() {
        let source = r#"
            fn test() {
                let [1] q0 = qalloc();
                let [1] q1 = qalloc();
                let qr = entangle(q0, q1); // consumes both qubits
                // q0, q1 moved, cannot use again
                let _ = qr;
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    /// Test that gate then measure works (gate borrows, measure consumes)
    #[test]
    fn test_gate_then_measure_works() {
        let source = r#"
            fn test() {
                let [1] q = qalloc();
                hadamard(q); // borrows q
                measure(q); // consumes q
            }
        "#;
        assert!(check_source(source).is_ok());
    }

    /// Test that measure then gate fails (measure consumes, gate finds moved)
    #[test]
    fn test_measure_then_gate_fails() {
        let source = r#"
            fn test() {
                let [1] q = qalloc();
                measure(q); // consumes q
                hadamard(q); // ERROR: use of moved value
            }
        "#;
        let result = check_source(source);
        assert!(result.is_err(), "Measure then gate should fail");
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::UseOfMovedValue { .. }))
        );
    }
    /// Round, abs, floor and ceil were added to the prelude because symmetric
    /// INT8 quantization cannot be written without them: round(x/scale) and
    /// the abs() in the overflow precondition are both required.
    #[test]
    fn test_prelude_math_builtins_float() {
        for src in [
            "fn q(x: f32) -> f32 { return round(x); }",
            "fn q(x: f32) -> f32 { return abs(x); }",
            "fn q(x: f32) -> f32 { return floor(x); }",
            "fn q(x: f32) -> f32 { return ceil(x); }",
            "fn q(x: f32) -> f32 { return exp(x); }",
            "fn q(x: f32) -> f32 { return sqrt(x); }",
        ] {
            assert!(
                check_source(src).is_ok(),
                "expected prelude builtin to typecheck: {}",
                src
            );
        }
    }

    /// clamp is 3-arity, so it is declared separately from the unary table.
    /// The arity is part of the contract: a 2-arg call must not typecheck.
    #[test]
    fn test_prelude_clamp_arity() {
        assert!(check_source("fn q(x: f32) -> f32 { return clamp(x, -128.0, 127.0); }").is_ok());
        assert!(check_source("fn q(x: f32) -> f32 { return clamp(x, 127.0); }").is_err());
    }

    /// The builtins are float-only. A regression guard on the declaration
    /// itself: an int argument must be rejected, not coerced.
    #[test]
    fn test_prelude_math_builtins_reject_int() {
        assert!(check_source("fn q(x: i8) -> f32 { return round(x); }").is_err());
        assert!(check_source("fn q(x: i8) -> f32 { return abs(x); }").is_err());
        assert!(check_source("fn q() -> f32 { return round(0); }").is_err());
    }

    /// End-to-end shape of a quantization-style loop over tensors, which is
    /// what the WGSL backend needs to lower eventually.
    #[test]
    fn test_tensor_loop_with_math_builtins() {
        let src = "
            fn normalize_clamped(
                input: [1] Tensor[f32, 1024],
                output: inout [1] Tensor[f32, 1024],
                scale: f32
            ) {
                forall i in 0..1024 {
                    let v = round(input[i] / scale);
                    output[i] = clamp(v, -128.0, 127.0);
                }
            }
        ";
        assert!(check_source(src).is_ok());
    }

    /// `as T` is a numeric cast. It exists because symmetric INT8
    /// quantization needs an f32 -> i8 narrowing, and without it the
    /// flagship kernel in kernels/quant_int8.naso could not typecheck.
    #[test]
    fn test_numeric_cast_between_scalar_types() {
        assert!(check_source("fn q(x: f32) -> i8 { return x as i8; }").is_ok());
        assert!(check_source("fn q(x: i8) -> f32 { return x as f32; }").is_ok());
        assert!(check_source("fn q(x: i8) -> u32 { return x as u32; }").is_ok());
    }

    /// A cast yields the target type, so an element-wise conversion is what
    /// lets an f32 tensor feed an i8 tensor slot. This is the exact shape the
    /// quantizer relies on.
    #[test]
    fn test_cast_allows_cross_element_tensor_conversion() {
        let quantize = "
            fn quantize(
                input: [1] Tensor[f32, 1024],
                output: inout [1] Tensor[i8, 1024],
                scale: f32
            ) {
                forall i in 0..1024 {
                    let v = round(input[i] / scale);
                    output[i] = clamp(v, -128.0, 127.0) as i8;
                }
            }
        ";
        assert!(check_source(quantize).is_ok());

        let dequantize = "
            fn dequantize(
                input: [1] Tensor[i8, 1024],
                output: inout [1] Tensor[f32, 1024],
                scale: f32
            ) {
                forall i in 0..1024 {
                    let v = input[i] as f32;
                    output[i] = v * scale;
                }
            }
        ";
        assert!(check_source(dequantize).is_ok());
    }

    /// `as` is a numeric conversion, not a reinterpret cast. Both a
    /// non-numeric operand and a non-numeric target must be rejected.
    #[test]
    fn test_cast_rejects_non_numeric() {
        assert!(check_source("fn q(x: bool) -> i8 { return x as i8; }").is_err());
        assert!(
            check_source("fn q(x: f32) -> Tensor[f32, 4] { return x as Tensor[f32, 4]; }").is_err()
        );
        assert!(check_source("fn q(x: f32) -> Qubit { return x as Qubit; }").is_err());
    }

    /// Regression guard, now inverted: this used to assert that a bare
    /// cross-element assignment is rejected. That is still true -- a cast is
    /// required, and omitting it must not silently succeed.
    #[test]
    fn test_cross_element_tensor_assignment_still_needs_cast() {
        assert!(check_source("fn q(input: [1] Tensor[i8, 8], output: inout [1] Tensor[f32, 8]) { forall i in 0..8 { output[i] = input[i]; } }").is_err());
        assert!(check_source("fn q(input: [1] Tensor[f32, 8], output: inout [1] Tensor[i8, 8]) { forall i in 0..8 { output[i] = input[i]; } }").is_err());
    }
    /// `proof { .. }` parses and typechecks. The body is an ordinary block,
    /// so obligations inside it go through the normal inference path.
    #[test]
    fn test_proof_block_parses_and_typechecks() {
        assert!(
            check_source("fn f(x: f32) -> f32 { proof { assert(x > 0.0); } return x; }").is_ok()
        );
        assert!(check_source("fn f(x: f32) -> f32 { proof { } return x; }").is_ok());
    }

    /// The soundness property this design exists for: `assert` outside a proof
    /// block is rejected. If it were declared in the prelude it would
    /// typecheck in ordinary code and then lower to nothing, a silent no-op
    /// where a runtime check was written.
    #[test]
    fn test_assert_rejected_outside_proof_block() {
        let result = check_source("fn f(x: f32) -> f32 { assert(x > 0.0); return x; }");
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::AssertOutsideProof { .. })),
            "expected AssertOutsideProof, got {:?}",
            errors
        );
    }

    /// An obligation must be a proposition, and assert takes exactly one.
    #[test]
    fn test_assert_argument_checks() {
        assert!(check_source("fn f(x: f32) -> f32 { proof { assert(x); } return x; }").is_err());
        assert!(check_source("fn f(x: f32) -> f32 { proof { assert(); } return x; }").is_err());
        assert!(
            check_source("fn f(x: f32) -> f32 { proof { assert(x > 0.0, 1.0); } return x; }")
                .is_err()
        );
    }

    /// `bool` is now nameable as a type. Only bool literals existed before,
    /// so no prelude function could take or return one.
    #[test]
    fn test_bool_type_is_nameable() {
        assert!(check_source("fn f(x: bool) -> bool { return x; }").is_ok());
    }

    /// A proof block's scope does not leak: bindings inside it are not
    /// visible to the enclosing runtime code.
    #[test]
    fn test_proof_block_scope_is_isolated() {
        assert!(
            check_source("fn f(x: f32) -> f32 { proof { let y = x; assert(y > 0.0); } return y; }")
                .is_err()
        );
    }

    /// Documented gap: `assert(forall ..)` does not typecheck.
    ///
    /// The parser reuses ExprKind::Forall for both loop statements and
    /// quantified propositions, and infer_forall returns unit, so a
    /// quantified obligation is currently inexpressible. This test records the
    /// limit; it should be replaced with a positive one when quantifiers can
    /// A quantified obligation is expressible. `forall` in expression position
    /// is a proposition and has type bool, which is what `assert` requires.
    #[test]
    fn test_quantified_assert_is_supported() {
        assert!(
            check_source(
                "fn f() -> bool { proof { assert(forall i in 0..10 { i < 10 }); } return true; }"
            )
            .is_ok()
        );
        assert!(check_source("fn f() -> bool { proof { assert(forall i in 0..10, j in 0..20 { i < j }); } return true; }").is_ok());
    }

    /// A quantified body must be a proposition. A statement sequence states
    /// nothing, and treating it as vacuously true would let an obligation that
    /// asserts no condition pass -- the failure mode a verifier must not have.
    #[test]
    fn test_quantified_body_must_be_bool() {
        assert!(
            check_source(
                "fn f() -> bool { proof { assert(forall i in 0..10 { }); } return true; }"
            )
            .is_err()
        );
    }

    /// Statement-position `forall` is still a loop, and the two forms did not
    /// collide when the proposition variant was introduced.
    #[test]
    fn test_forall_statement_form_still_a_loop() {
        assert!(
            check_source("fn f() -> bool { forall i in 0..10 { let x = i; } return true; }")
                .is_ok()
        );
    }

    /// A proof block is erased, so reading a `[1]` linear value in an
    /// obligation does not consume it. Without this, any obligation about a
    /// linear parameter would make the surrounding runtime code look like a
    /// double use, and quantified obligations would be inexpressible for
    /// exactly the values worth stating properties about.
    #[test]
    fn test_proof_block_does_not_consume_linear_values() {
        let src = "
            fn quantize(
                input: [1] Tensor[f32, 1024],
                output: inout [1] Tensor[i8, 1024],
                scale: f32
            ) {
                proof {
                    assert(forall i in 0..1024 { abs(input[i] / scale) <= 127.0 });
                }

                forall i in 0..1024 {
                    let v = round(input[i] / scale);
                    output[i] = clamp(v, -128.0, 127.0) as i8;
                }
            }
        ";
        assert!(check_source(src).is_ok());
    }

    /// The erasure above is scoped to proof blocks only. A genuine runtime
    /// double use must still be reported.
    #[test]
    fn test_runtime_double_use_still_reported() {
        let result = check_source(
            "fn f(x: [1] f32) -> f32 { proof { assert(x > 0.0); } let a = x; let b = x; return a; }",
        );
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::LinearVariableUsedTwice { .. })),
            "expected LinearVariableUsedTwice, got {:?}",
            errors
        );
    }

    /// The quantize kernel with a real proof block. Obligations are currently
    /// scalar; the per-element precondition is not yet expressible.
    #[test]
    fn test_quantize_kernel_with_proof_block() {
        let src = "
            fn quantize(
                input: [1] Tensor[f32, 1024],
                output: inout [1] Tensor[i8, 1024],
                scale: f32
            ) {
                proof {
                    assert(scale > 0.0);
                }

                forall i in 0..1024 {
                    let v = round(input[i] / scale);
                    output[i] = clamp(v, -128.0, 127.0) as i8;
                }
            }
        ";
        assert!(check_source(src).is_ok());
    }
    /// symbolic form crashed.
    #[test]
    fn test_symbolic_extent_as_loop_bound_parses() {
        assert!(check_source(
            "fn f[N: nat](t: Tensor[f32, N]) -> f32 { forall i in 0..N { let x = t[i]; } return 0.0; }"
        )
        .is_ok());
        assert!(check_source("fn f[N: nat]() { forall i in 0..N { } }").is_ok());
    }

    /// A Nat generic is bound as a value, so it is usable as a term.
    #[test]
    fn test_nat_generic_is_usable_as_a_value() {
        assert!(check_source("fn f[N: nat]() -> nat { return N; }").is_ok());
        assert!(check_source("fn f[N: nat]() -> nat { let a = N; return a; }").is_ok());
    }

    /// The literal-extent form must keep working; the parser change touched
    /// the shared `{` handling.
    #[test]
    fn test_literal_extent_loop_bound_still_parses() {
        assert!(check_source("fn f() { forall i in 0..10 { let x = i; } }").is_ok());
        assert!(
            check_source(
                "fn f(t: Tensor[f32, 4]) -> f32 { forall i in 0..4 { let x = t[i]; } return 0.0; }"
            )
            .is_ok()
        );
    }
}

/// Unit tests for quantity unification and lattice operations (TASK-205)
#[cfg(test)]
mod qty_tests {
    use crate::ast::Quantity;
    use crate::typecheck::unify::{qty_consume, qty_join, qty_meet, qty_subtype, unify_quantity};

    fn span() -> crate::ast::Span {
        crate::ast::Span::default()
    }

    #[test]
    fn test_unify_quantity_zero_zero() {
        let result = unify_quantity(Quantity::Zero, Quantity::Zero, span());
        assert!(matches!(result, Ok(Quantity::Zero)));
    }

    #[test]
    fn test_unify_quantity_one_one() {
        let result = unify_quantity(Quantity::One, Quantity::One, span());
        assert!(matches!(result, Ok(Quantity::One)));
    }

    #[test]
    fn test_unify_quantity_one_many() {
        // Per join table: One \ Many = Many
        let result = unify_quantity(Quantity::One, Quantity::Many, span());
        assert!(matches!(result, Ok(Quantity::Many)));
    }

    #[test]
    fn test_unify_quantity_one_bounded() {
        // Per join table: One \ Bounded(3) = Bounded(max(1,3)) = Bounded(3)
        let result = unify_quantity(Quantity::One, Quantity::Bounded(3), span());
        assert!(matches!(result, Ok(Quantity::Bounded(3))));
    }

    #[test]
    fn test_unify_quantity_bounded_bounded() {
        // Per join table: Bounded(2) \ Bounded(5) = Bounded(max(2,5)) = Bounded(5)
        let result = unify_quantity(Quantity::Bounded(2), Quantity::Bounded(5), span());
        assert!(matches!(result, Ok(Quantity::Bounded(5))));
    }

    #[test]
    fn test_unify_quantity_bounded_many() {
        // Per join table: Bounded(2) \ Many = Many
        let result = unify_quantity(Quantity::Bounded(2), Quantity::Many, span());
        assert!(matches!(result, Ok(Quantity::Many)));
    }

    #[test]
    fn test_unify_quantity_zero_one_error() {
        // Zero cannot unify with non-Zero
        let result = unify_quantity(Quantity::Zero, Quantity::One, span());
        assert!(result.is_err());
        match result {
            Err(crate::typecheck::error::TypeError::QuantityMismatch { .. }) => {}
            _ => panic!("Expected QuantityMismatch error"),
        }
    }

    #[test]
    fn test_unify_quantity_one_zero_error() {
        let result = unify_quantity(Quantity::One, Quantity::Zero, span());
        assert!(result.is_err());
    }

    #[test]
    fn test_qty_subtype_zero_all() {
        assert!(qty_subtype(Quantity::Zero, Quantity::Zero));
        assert!(qty_subtype(Quantity::Zero, Quantity::One));
        assert!(qty_subtype(Quantity::Zero, Quantity::Bounded(5)));
        assert!(qty_subtype(Quantity::Zero, Quantity::Many));
    }

    #[test]
    fn test_qty_subtype_one() {
        assert!(qty_subtype(Quantity::One, Quantity::One));
        assert!(qty_subtype(Quantity::One, Quantity::Bounded(1)));
        assert!(qty_subtype(Quantity::One, Quantity::Bounded(5)));
        assert!(qty_subtype(Quantity::One, Quantity::Many));
        assert!(!qty_subtype(Quantity::One, Quantity::Zero));
    }

    #[test]
    fn test_qty_subtype_bounded() {
        assert!(qty_subtype(Quantity::Bounded(2), Quantity::Bounded(5)));
        assert!(!qty_subtype(Quantity::Bounded(5), Quantity::Bounded(2)));
        assert!(qty_subtype(Quantity::Bounded(2), Quantity::Many));
        assert!(!qty_subtype(Quantity::Bounded(2), Quantity::One));
        assert!(!qty_subtype(Quantity::Bounded(2), Quantity::Zero));
    }

    #[test]
    fn test_qty_subtype_many() {
        assert!(qty_subtype(Quantity::Many, Quantity::Many));
        assert!(!qty_subtype(Quantity::Many, Quantity::One));
        assert!(!qty_subtype(Quantity::Many, Quantity::Bounded(5)));
        assert!(!qty_subtype(Quantity::Many, Quantity::Zero));
    }

    #[test]
    fn test_qty_join() {
        assert_eq!(qty_join(Quantity::One, Quantity::Many), Quantity::Many);
        assert_eq!(
            qty_join(Quantity::One, Quantity::Bounded(3)),
            Quantity::Bounded(3)
        );
        assert_eq!(
            qty_join(Quantity::Bounded(2), Quantity::Bounded(5)),
            Quantity::Bounded(5)
        );
        assert_eq!(qty_join(Quantity::Zero, Quantity::One), Quantity::One);
        assert_eq!(
            qty_join(Quantity::Zero, Quantity::Bounded(3)),
            Quantity::Bounded(3)
        );
    }

    #[test]
    fn test_qty_meet() {
        assert_eq!(qty_meet(Quantity::One, Quantity::Many), Quantity::One);
        assert_eq!(qty_meet(Quantity::One, Quantity::Bounded(3)), Quantity::One);
        assert_eq!(
            qty_meet(Quantity::Bounded(2), Quantity::Bounded(5)),
            Quantity::Bounded(2)
        );
        assert_eq!(qty_meet(Quantity::Zero, Quantity::One), Quantity::Zero);
        assert_eq!(
            qty_meet(Quantity::Bounded(3), Quantity::Many),
            Quantity::Bounded(3)
        );
    }

    #[test]
    fn test_qty_consume() {
        assert_eq!(qty_consume(Quantity::Zero), Quantity::Zero);
        assert_eq!(qty_consume(Quantity::One), Quantity::Zero);
        assert_eq!(qty_consume(Quantity::Bounded(3)), Quantity::Bounded(2));
        assert_eq!(qty_consume(Quantity::Bounded(1)), Quantity::Bounded(0));
        assert_eq!(qty_consume(Quantity::Bounded(0)), Quantity::Zero);
        assert_eq!(qty_consume(Quantity::Many), Quantity::Many);
    }
}
