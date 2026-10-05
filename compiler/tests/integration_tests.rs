#![allow(unused_variables)]
#![allow(clippy::approx_constant)]

use naso_compiler::ast::*;
use naso_compiler::lexer::{Lexer, TokenKind as TK};
use naso_compiler::parser::parse_program;

#[test]
fn lexer_tokenizes_quantity_markers() {
    let kinds = Lexer::tokenize("[0] [1] [*] [N]");
    assert_eq!(
        kinds,
        vec![
            TK::LBracket,
            TK::Int(0),
            TK::RBracket,
            TK::LBracket,
            TK::Int(1),
            TK::RBracket,
            TK::QtyStar,
            TK::LBracket,
            TK::TypeIdent("N".to_string()),
            TK::RBracket,
        ]
    );
}

#[test]
fn lexer_tokenizes_all_keywords() {
    let src = "fn let inout consume reversible return if else match for while struct enum type mod import const";
    let kinds = Lexer::tokenize(src);
    assert_eq!(
        kinds,
        vec![
            TK::Fn,
            TK::Let,
            TK::InOut,
            TK::Consume,
            TK::Reversible,
            TK::Return,
            TK::If,
            TK::Else,
            TK::Match,
            TK::For,
            TK::While,
            TK::Struct,
            TK::Enum,
            TK::Type,
            TK::Mod,
            TK::Import,
            TK::Const,
        ]
    );
}

#[test]
fn lexer_tokenizes_quantum_keywords() {
    let kinds = Lexer::tokenize("qubit qregister measure gate entangle");
    assert_eq!(
        kinds,
        vec![
            TK::Qubit,
            TK::QRegister,
            TK::Measure,
            TK::Gate,
            TK::Entangle,
        ]
    );
}

#[test]
fn lexer_tokenizes_literals_and_operators() {
    let kinds = Lexer::tokenize("42 3.14 true \"hello\" 'x' + - => == <=");
    assert_eq!(
        kinds,
        vec![
            TK::Int(42),
            TK::Float(3.14),
            TK::Bool(true),
            TK::Str("hello".to_string()),
            TK::Char('x'),
            TK::Plus,
            TK::Minus,
            TK::FatArrow,
            TK::Eq,
            TK::Le,
        ]
    );
}

#[test]
fn parser_simple_function() {
    let src = r#"
        fn main() -> Int {
            return 0;
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    assert_eq!(prog.items.len(), 1);

    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function, got {other:?}"),
    };
    assert_eq!(func.name.name, "main");
    assert!(func.params.is_empty());
    assert!(func.ret_ty.is_some());
}

#[test]
fn parser_function_with_params() {
    let src = r#"
        fn add(x: Int, y: Int) -> Int {
            return x + y;
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function, got {other:?}"),
    };
    assert_eq!(func.name.name, "add");
    assert_eq!(func.params.len(), 2);
    assert_eq!(func.params[0].name.name, "x");
    assert_eq!(func.params[1].name.name, "y");
    assert!(func.ret_ty.is_some());
}

#[test]
fn parser_function_with_quantity_markers() {
    let src = r#"
        fn process(data: [0] Matrix[Rows, Cols], inout acc: Int) -> Int {
            return acc;
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function, got {other:?}"),
    };
    assert_eq!(func.params.len(), 2);

    // First param: [0] quantity marker
    let p0 = &func.params[0];
    assert_eq!(p0.name.name, "data");
    assert_eq!(p0.ty.quantity, Quantity::Zero);

    // Second param: inout mutability
    let p1 = &func.params[1];
    assert_eq!(p1.name.name, "acc");
    assert_eq!(p1.mutability, Mutability::InOut);
}

#[test]
fn parser_struct_definition() {
    let src = r#"
        struct Point {
            x: Float,
            y: Float,
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    assert_eq!(prog.items.len(), 1);

    let td = match &prog.items[0] {
        Item::TypeDef(t) => t,
        other => panic!("expected type def, got {other:?}"),
    };
    assert_eq!(td.name.name, "Point");
    let fields = match &td.kind {
        TypeDefKind::Struct(f) => f,
        other => panic!("expected struct, got {other:?}"),
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name.name, "x");
    assert_eq!(fields[1].name.name, "y");
}

#[test]
fn parser_enum_definition() {
    let src = r#"
        enum Color {
            Red,
            Green,
            Blue,
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let td = match &prog.items[0] {
        Item::TypeDef(t) => t,
        other => panic!("expected type def, got {other:?}"),
    };
    assert_eq!(td.name.name, "Color");
    let variants = match &td.kind {
        TypeDefKind::Enum(v) => v,
        other => panic!("expected enum, got {other:?}"),
    };
    assert_eq!(variants.len(), 3);
    assert_eq!(variants[0].name.name, "Red");
}

#[test]
fn parser_reversible_block() {
    let src = r#"
        fn demo() {
            reversible {
                let a = 1;
                let b = 2;
            }
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function"),
    };
    assert_eq!(func.body.stmts.len(), 1);
    match &func.body.stmts[0].kind {
        StmtKind::Reversible(_) => {}
        other => panic!("expected reversible stmt, got {other:?}"),
    }
}

#[test]
fn parser_if_else_expression() {
    let src = r#"
        fn check(x: Int) -> Int {
            if x > 0 {
                return 1;
            } else {
                return 0;
            }
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function"),
    };
    // The function body should contain the if statement
    assert!(!func.body.stmts.is_empty());
}

#[test]
fn parser_binary_operators() {
    let src = r#"
        fn calc(a: Int, b: Int) -> Int {
            return a + b * 2;
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function"),
    };
    // The body has one statement: return a + b * 2;
    match &func.body.stmts[0].kind {
        StmtKind::Expr(e) => {
            match &e.kind {
                ExprKind::Return(Some(inner)) => {
                    // Should be (a + (b * 2))
                    match &inner.kind {
                        ExprKind::Binary(BinOp::Add, lhs, rhs) => {
                            assert!(matches!(lhs.kind, ExprKind::Var(_)));
                            match &rhs.kind {
                                ExprKind::Binary(BinOp::Mul, _, _) => {}
                                other => panic!("expected Mul, got {other:?}"),
                            }
                        }
                        other => panic!("expected Add, got {other:?}"),
                    }
                }
                other => panic!("expected return, got {other:?}"),
            }
        }
        other => panic!("expected expr stmt, got {other:?}"),
    }
}

#[test]
fn parser_call_expression() {
    let src = r#"
        fn main() {
            foo(1, 2, 3);
        }
    "#;
    let prog = parse_program(src).expect("parse failed");
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function"),
    };
    match &func.body.stmts[0].kind {
        StmtKind::Expr(e) => match &e.kind {
            ExprKind::Call(callee, args) => {
                assert!(matches!(callee.kind, ExprKind::Var(_)));
                assert_eq!(args.len(), 3);
            }
            other => panic!("expected call, got {other:?}"),
        },
        other => panic!("expected expr stmt, got {other:?}"),
    }
}

#[test]
fn parser_entangle_transform_from_discussion() {
    let src = r#"
        fn entangle_transform[Rows: type, Cols: type](
            matrix: [0] Matrix[Rows, Cols],
            inout state: QRegister[Cols],
            ancilla: consume Qubit,
        ) -> QRegister[Rows] {
            reversible {
                let phase_shift = compute_phase(state, ancilla);
                apply_hamiltonian(state, phase_shift);
            }
            return project_register(state);
        }
    "#;
    let prog = parse_program(src).expect("parse of DISCUSSION.md example failed");

    assert_eq!(prog.items.len(), 1);
    let func = match &prog.items[0] {
        Item::Function(f) => f,
        other => panic!("expected function, got {other:?}"),
    };

    assert_eq!(func.name.name, "entangle_transform");
    assert_eq!(func.generics.len(), 2);
    assert_eq!(func.generics[0].name.name, "Rows");
    assert_eq!(func.generics[1].name.name, "Cols");
    assert_eq!(func.params.len(), 3);

    // matrix: [0] Matrix[Rows, Cols]  -- quantity Zero
    let p0 = &func.params[0];
    assert_eq!(p0.name.name, "matrix");
    assert_eq!(p0.ty.quantity, Quantity::Zero);

    // inout state: QRegister[Cols]
    let p1 = &func.params[1];
    assert_eq!(p1.name.name, "state");
    assert_eq!(p1.mutability, Mutability::InOut);

    // consume ancilla: Qubit
    let p2 = &func.params[2];
    assert_eq!(p2.name.name, "ancilla");
    assert_eq!(p2.mutability, Mutability::Consume);

    // return type: QRegister[Rows]
    let ret = func.ret_ty.as_ref().expect("expected return type");
    assert_eq!(ret.quantity, Quantity::Many);

    // body contains at least one reversible stmt + one return stmt
    assert!(!func.body.stmts.is_empty());

    // Check that one statement is a reversible block
    let has_reversible = func
        .body
        .stmts
        .iter()
        .any(|s| matches!(s.kind, StmtKind::Reversible(_)));
    assert!(has_reversible, "expected a reversible block in the body");
}

#[test]
fn parser_const_definition() {
    let src = r#"
        const MAX_SIZE = 1024;
    "#;
    let prog = parse_program(src).expect("parse failed");
    assert_eq!(prog.items.len(), 1);
    match &prog.items[0] {
        Item::Const(c) => {
            assert_eq!(c.name.name, "MAX_SIZE");
            assert!(c.ty.is_none());
        }
        other => panic!("expected const, got {other:?}"),
    }
}

#[test]
fn parser_type_alias() {
    let src = r#"
        type Length = Int;
    "#;
    let prog = parse_program(src).expect("parse failed");
    match &prog.items[0] {
        Item::TypeDef(td) => {
            assert_eq!(td.name.name, "Length");
            assert!(matches!(td.kind, TypeDefKind::Alias(_)));
        }
        other => panic!("expected type def, got {other:?}"),
    }
}

#[test]
// PIN (Step 8): a `forall` over a float range binds a Float variable, so
// `round(t)` (signature `f32 -> f32`) typechecks. This is the *typecheck-side*
// half of the Real-typed quantifier change -- the encoder-side sort inference
// is pinned separately in naso-verify's `real_typed_forall_round_bound_discharges`.
// Reverting `type_of_range_bound` to `TypeKind::Int` makes `round(t)` a type
// error (argument Int != domain f32), so this test FAILS on that regression.
fn typecheck_forall_float_range_binds_float_var() {
    use naso_compiler::typecheck::check_program;
    let src = r#"
        fn k() -> Bool {
            proof {
                forall t in 0.0..1.0 {
                    assert(round(t) <= 0.5);
                }
            }
            return true;
        }
    "#;
    let mut prog = parse_program(src).expect("parse failed");
    let result = check_program(&mut prog);
    assert!(
        result.errors.is_empty(),
        "float-range `forall` should typecheck with t: Float; errors: {:?}",
        result.errors
    );
}
