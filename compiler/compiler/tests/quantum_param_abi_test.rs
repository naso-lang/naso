//! Quantum parameter types must have an ABI, and a quantity must not be mistaken for a type.
//!
//! # What was wrong
//!
//! `LoweringContext::param_kind` mapped `TypeKind::QRegister` to `ParamKind::QRegister`
//! but not `TypeKind::Qubit`, which fell through to the catch-all. Measured:
//!
//! ```text
//! fn f(q: Qubit)    -> i64   ->  param=Unsupported("Qubit")
//! fn f(q: [0] Qubit)-> i64   ->  param=Unsupported("[0] Qubit")
//! fn f(q: Many)     -> i64   ->  param=Unsupported("Many")
//! ```
//!
//! So NO quantum parameter type had an ABI -- not even the single one the language
//! exists for. Separately, `Many` and `One` are quantities, but they parse as ordinary
//! identifiers in type position, so `fn f(q: Many)` reported a diagnostic naming a
//! quantity as though it were the author's intended type.

#![cfg(feature = "llvm")]

use naso_compiler::ir::pir_types::ParamKind;

/// Lower a one-parameter function and return the parameter's ABI kind.
fn param_kind(src: &str) -> ParamKind {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("test source must parse: {e}"));
    let module = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("test source must lower: {e}"));
    module.functions[0].params[0].kind.clone()
}

/// `Qubit` must get a real ABI slot.
///
/// THE REGRESSION THIS FILE EXISTS FOR. `Qubit` previously became
/// `Unsupported("Qubit")`, so the one type the whole language exists for had no ABI.
///
/// A `Qubit` parameter is a caller-owned pointer, exactly like a `QRegister`: the
/// backend indexes neither, it only passes the address through.
#[test]
fn a_qubit_parameter_gets_a_real_abi_slot() {
    assert_eq!(
        param_kind("fn f(q: Qubit) -> i64 { 0 }"),
        ParamKind::QRegister,
        "`Qubit` must map to the same pointer ABI as `QRegister`, not to Unsupported"
    );
}

/// A quantity annotation on a qubit must not defeat the mapping.
///
/// `[0] Qubit` is the borrow spelling. Recording the annotation in the *type* meant the
/// combined spelling missed the `QRegister` arm entirely, so the borrowed form and the
/// bare form behaved differently for no ABI reason.
#[test]
fn a_borrowed_qubit_parameter_gets_a_real_abi_slot() {
    assert_eq!(
        param_kind("fn f(q: [0] Qubit) -> i64 { 0 }"),
        ParamKind::QRegister,
        "`[0] Qubit` must map to the pointer ABI like `Qubit` does"
    );
}

/// A consumed qubit must also map, since `[1]` is the default annotation.
#[test]
fn a_consumed_qubit_parameter_gets_a_real_abi_slot() {
    assert_eq!(
        param_kind("fn f(q: [1] Qubit) -> i64 { 0 }"),
        ParamKind::QRegister
    );
}

/// `QRegister` must get a pointer slot too -- including the BARE spelling.
///
/// This is a second unreachable arm, found while testing the first fix. `param_kind`
/// matched `TypeKind::QRegister(_)`, but the parser resolves `QRegister` to
/// `Named("QRegister", args)`, so that arm could never match. Measured, before the fix:
///
/// ```text
/// fn f(q: QRegister)          -> Unsupported("QRegister")
/// fn f(q: QRegister[f32, 4])  -> Unsupported("QRegister<Float, 4>")
/// ```
///
/// So `Qubit` missed for want of an arm and `QRegister` for want of a reachable one:
/// between them, NO quantum parameter type had an ABI.
#[test]
fn a_bare_qregister_parameter_gets_a_pointer_slot() {
    assert_eq!(
        param_kind("fn f(q: QRegister) -> i64 { 0 }"),
        ParamKind::QRegister,
        "bare `QRegister` must not fall through the catch-all"
    );
}

/// With generic arguments, which is the spelling that carries the register width.
#[test]
fn a_sized_qregister_parameter_gets_a_pointer_slot() {
    assert_eq!(
        param_kind("fn f(q: QRegister[f32, 4]) -> i64 { 0 }"),
        ParamKind::QRegister
    );
}

/// `Many` in type position must be diagnosed as the quantity mistake it is.
///
/// The point is not that it is refused -- the catch-all already refused it. The point
/// is that the message must not name a quantity as though the author had asked for a
/// type called `Many`, because that sends them looking for a missing type instead of
/// toward the `[0]` / `[1]` annotation they actually meant to write.
#[test]
fn many_in_type_position_is_diagnosed_as_a_quantity_not_a_type() {
    match param_kind("fn f(q: Many) -> i64 { 0 }") {
        ParamKind::Unsupported(msg) => {
            assert!(
                msg.contains("quantity"),
                "the diagnostic must say `Many` is a quantity: {msg}"
            );
            assert!(
                msg.contains("[1] Qubit") && msg.contains("[0] Qubit"),
                "the diagnostic must show the annotation spelling that was meant: {msg}"
            );
        }
        other => panic!("`Many` must not become a usable ABI slot, got {other:?}"),
    }
}

/// The same for `One`.
#[test]
fn one_in_type_position_is_diagnosed_as_a_quantity_not_a_type() {
    match param_kind("fn f(q: One) -> i64 { 0 }") {
        ParamKind::Unsupported(msg) => assert!(
            msg.contains("quantity"),
            "the diagnostic must say `One` is a quantity: {msg}"
        ),
        other => panic!("`One` must not become a usable ABI slot, got {other:?}"),
    }
}

/// A genuinely unknown type keeps the plain catch-all message.
///
/// The quantity arm is narrow on purpose. Over-broadening it would swallow every typo
/// into a message about `[0]`/`[1]` annotations, which would be its own kind of
/// misleading diagnostic.
#[test]
fn an_unknown_type_name_keeps_the_plain_catch_all_message() {
    match param_kind("fn f(q: Blah) -> i64 { 0 }") {
        ParamKind::Unsupported(msg) => assert_eq!(
            msg, "Blah",
            "an unknown type must report itself, not be redirected to the quantity \
             diagnostic: {msg}"
        ),
        other => panic!("`Blah` must not become a usable ABI slot, got {other:?}"),
    }
}

/// Ordinary scalar parameters must be unaffected by the new arms.
#[test]
fn ordinary_scalar_parameters_are_unaffected() {
    assert_eq!(
        param_kind("fn f(a: f32) -> i64 { 0 }"),
        ParamKind::Scalar(naso_compiler::ir::pir_types::ElemType::F64)
    );
    assert_eq!(
        param_kind("fn f(a: i64) -> i64 { 0 }"),
        ParamKind::Scalar(naso_compiler::ir::pir_types::ElemType::I64)
    );
}
