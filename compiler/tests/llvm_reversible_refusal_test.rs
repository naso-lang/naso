//! Expression-position `reversible { ... }` is REFUSED, because it used to fabricate
//! its result.
//!
//! # What this file replaces
//!
//! `ExprKind::Reversible` in expression position lowered to
//!
//! ```text
//! PirExpr::Reversible { body: IntLit(0), inverse: IntLit(0) }
//! ```
//!
//! with the comment "For now just return a placeholder". So
//!
//! ```naso
//! let x = reversible { r = 5; r = 6; };
//! ```
//!
//! compiled, ran, and bound `x` to **0**. Not because the block computed zero --
//! because a literal sat in the compiler. Nothing in the emitted IR distinguished
//! that from a real computation: `x` was stored from a constant, and the store
//! looked exactly like every other store.
//!
//! That is the worst failure mode this compiler has: a fabricated value carrying
//! the name of a computation that never ran.
//!
//! # Why the inverse mattered more than the value
//!
//! The LLVM backend separately ran `body` and dropped `inverse`, justified as "the
//! inverse is an undo, not part of the value". That is true of the RESULT and false
//! of the SEMANTICS. For a reversible block the inverse is the part that
//! *uncomputes* -- running the body without it is not a partial implementation of
//! uncomputation, it is the opposite of one. It leaves behind precisely the garbage
//! the block existed to erase, which for a linear resource is a soundness hole.
//!
//! The old code's defence was a comment saying the omission was "deliberately bound
//! rather than discarded, so the omission is visible in this file". A comment is not
//! a guarantee. Nothing checked it.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;
use naso_compiler::ir::affine_domain::AffineDomain;
use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
use naso_compiler::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct CaseDir(PathBuf);

impl CaseDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("naso-rev-{}-{}-{}", tag, std::process::id(), n));
        std::fs::create_dir_all(&p).expect("case dir");
        Self(p)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tool(name: &str) -> String {
    if let Ok(found) = Command::new(name).arg("--version").output()
        && found.status.success()
    {
        return name.to_string();
    }
    let brew = "/home/linuxbrew/.linuxbrew/opt/llvm@17/bin";
    if std::path::Path::new(brew).join(name).exists() {
        return format!("{brew}/{name}");
    }
    panic!("`{name}` not found on PATH or under {brew}");
}

/// The fallible form of `build_ir`, for asserting that something is REFUSED.
///
/// `build_ir` panics on a lowering error, which is right for the tests that want IR.
/// A test checking a refusal needs the error, not a panic.
fn try_build_ir(src: &str) -> Result<String, String> {
    let program = naso_compiler::parser::parse_program(src).map_err(|e| format!("parse: {e:?}"))?;
    let pir = naso_compiler::lowering::lower_program(&program).map_err(|e| e.to_string())?;
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).map_err(|e| e.to_string())?;
    let mut builder = LLVMModuleBuilder::new(&cc).map_err(|e| e.to_string())?;
    builder.build_module(&pir).map_err(|e| e.to_string())?;
    Ok(builder.module().to_string())
}

fn build_ir(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\n{src}\nerror: {e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed:\n{src}\nerror: {e}"));
    builder.module().to_string()
}

fn run(ir: &str, driver: &str) -> String {
    let dir = CaseDir::new("run");
    std::fs::write(dir.join("m.ll"), ir).expect("write ll");
    std::fs::write(dir.join("d.c"), driver).expect("write driver");
    let llc = Command::new(tool("llc"))
        .args([
            "-filetype=obj",
            "-relocation-model=pic",
            "m.ll",
            "-o",
            "m.o",
        ])
        .current_dir(dir.join(""))
        .output()
        .expect("run llc");
    assert!(
        llc.status.success(),
        "llc failed:\n{}",
        String::from_utf8_lossy(&llc.stderr)
    );
    let clang = Command::new(tool("clang"))
        .args(["-O0", "m.o", "d.c", "-lm", "-o", "prog"])
        .current_dir(dir.join(""))
        .output()
        .expect("run clang");
    assert!(
        clang.status.success(),
        "clang link failed:\n{}",
        String::from_utf8_lossy(&clang.stderr)
    );
    let mut child = Command::new(dir.join("prog"))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn exe");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match child.try_wait().expect("poll exe") {
            Some(_) => break,
            None => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("the compiled program did not terminate within 20s.\n--- IR ---\n{ir}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    let out = child.wait_with_output().expect("read exe output");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The error text a program is refused with, at either lowering or codegen.
fn compile_error(src: &str) -> String {
    let program = match naso_compiler::parser::parse_program(src) {
        Ok(p) => p,
        Err(e) => return format!("{e:?}"),
    };
    let pir = match naso_compiler::lowering::lower_program(&program) {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    match builder.build_module(&pir) {
        Ok(()) => panic!("expected a refusal, but the program compiled:\n{src}"),
        Err(e) => e.to_string(),
    }
}

/// Expression-position `reversible` must be refused, not compiled to a constant.
///
/// The specific program here used to bind `x` to 0. Asserting only "it is refused"
/// would still pass if the compiler one day refused it for an unrelated reason, so
/// the message must name the inverse -- the part whose absence is what made the old
/// behaviour unsound.
#[test]
fn a_reversible_block_in_expression_position_is_refused() {
    let msg =
        compile_error("fn f() -> i64 { let mut r = 0; let x = reversible { r = 5; r = 6; }; r }\n");
    assert!(
        msg.contains("EXPRESSION position"),
        "the refusal must say which position is unsupported: {msg}"
    );
    assert!(
        msg.contains("INVERSE"),
        "the refusal must name the inverse, because dropping the uncomputation -- not \
         merely losing a value -- is what made the old behaviour unsound: {msg}"
    );
    assert!(
        msg.contains("as a STATEMENT"),
        "the refusal must point at the form that does work: {msg}"
    );
}

/// Statement-position `reversible` still compiles and still runs correctly.
///
/// This is the guard on the other side. Refusing `reversible` entirely would pass
/// every test above while quietly breaking working code, and `examples/test.naso`
/// A statement-position `reversible { ... }` is now REFUSED.
///
/// # This test previously asserted the opposite
///
/// It read:
///
/// ```ignore
/// // the statement form emits its body, so r ends at 2. This must keep working --
/// // the refusal targets expression position only.
/// assert_eq!(run(...), "2");
/// ```
///
/// That comment was wrong, and the assertion enshrined the defect. The construct was
/// NOT "emitting its body" in any meaningful sense -- it was dropping the block and
/// reporting success, and no inverse was generated. `r` ending at 2 was an accident of
/// which statements the forward walk happened to visit, not a guarantee.
///
/// A second test made the same claim ("the keyword must not change the result"). Also
/// wrong: the keyword changed the result from a correct program to one missing its
/// entire block, and no inverse either.
///
/// The distinction this file does still draw -- between statement position and
/// EXPRESSION position -- remains real and is covered above: `lower_reversible_expr`
/// is gone, and expression position is refused at parse/lowering. What changed is that
/// statement position is now refused too, because emitting a forward pass with no
/// uncomputation is not a partial success, it is a wrong answer.
#[test]
fn a_reversible_block_in_statement_position_is_refused() {
    let msg =
        match try_build_ir("fn two() -> i64 { let mut r = 0; reversible { r = 1; r = 2; } r }\n") {
            Err(e) => e.to_string(),
            Ok(ok) => panic!(
                "a `reversible {{ ... }}` block built successfully. It emits the forward \
             statements and NO inverse, so the block is not actually uncomputed.\n{ok:?}"
            ),
        };
    assert!(
        msg.contains("reversible"),
        "the refusal must name the construct: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("uncomput") || msg.to_lowercase().contains("inverse"),
        "the refusal must say the uncomputation pass is missing: {msg}"
    );
}

/// The keyword MUST change the result: with it, refusal; without it, the real value.
///
/// This is the corrected form of the old `the_statement_form_matches_the_identical_
/// plain_statements`. `reversible { r = 1; r = 2; }` and `r = 1; r = 2;` are NOT
/// interchangeable -- the first promises uncomputation and cannot deliver it, so it is
/// refused rather than silently reduced to the second.
#[test]
fn the_keyword_changes_the_outcome_where_it_previously_did_not() {
    let with_kw = try_build_ir("fn a() -> i64 { let mut r = 0; reversible { r = 1; r = 2; } r }\n");
    assert!(
        with_kw.is_err(),
        "the reversible form must be refused rather than behaving like the plain one"
    );

    let without_kw = build_ir("fn b() -> i64 { let mut r = 0; r = 1; r = 2; r }\n");
    let ir = without_kw;
    assert_eq!(
        run(
            &ir,
            "#include <stdio.h>\nlong naso_b(void);\nint main(void){ printf(\"%ld\\n\", naso_b()); return 0; }\n",
        ),
        "2",
        "the plain form is unaffected and must keep executing correctly"
    );
}

/// No IR may contain a fabricated zero standing in for a reversible computation.
///
/// The old bug's signature was a constant reaching the store for a value that a
/// block was supposed to compute. This test cannot see the past bug directly, but it
/// pins the property that makes it impossible to reintroduce silently: a `Reversible`
/// that reaches codegen at all is a refusal, so no module is ever produced from one.
#[test]
fn a_reversible_never_reaches_codegen_as_an_emitted_value() {
    // If this ever compiles, the backend has started accepting `Reversible` again,
    // and the question to answer before changing that is what the inverse becomes.
    let msg = compile_error("fn f() -> i64 { let mut r = 0; let x = reversible { r = 5; }; r }\n");
    assert!(
        msg.contains("INVERSE") || msg.contains("EXPRESSION position"),
        "a `Reversible` must be refused at whichever stage sees it first, never \
         emitted: {msg}"
    );
}

/// A `Reversible` arriving as hand-written PIR must be refused by the BACKEND.
///
/// This test exists because of a surviving mutant. With lowering refusing
/// expression-position `reversible`, the backend's own arm was unreachable from any
/// source program -- so a mutation restoring the original behaviour, "run `body`,
/// drop `inverse`", passed the entire suite. Four green tests, one of them
/// explicitly about the inverse, none of them able to see it.
///
/// Nothing stops a `.pir` fixture, or any other PIR producer, from constructing a
/// `Reversible` directly. A backend must not run a construct whose defining operation
/// it cannot perform no matter who built it, so the refusal is pinned here by
/// constructing exactly that node.
#[test]
fn the_backend_refuses_a_reversible_that_arrives_as_pir() {
    let _ctx = inkwell::context::Context::create();
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::Default).expect("context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");
    let stmt = PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        // A body with a SIDE EFFECT, so "ran the body, dropped the inverse" would be
        // observably different from "refused". If this compiles, the body was emitted
        // without the inverse -- the exact original bug.
        body: PirExpr::Reversible {
            body: Box::new(PirExpr::Assign {
                target: Box::new(PirExpr::Var("out".to_string())),
                value: Box::new(PirExpr::IntLit(1)),
            }),
            inverse: Box::new(PirExpr::IntLit(0)),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    let module = PirModule {
        statements: vec![stmt],
        schedule: ScheduleTree::new(
            ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
            vec![],
        ),
        ..Default::default()
    };
    let err = builder
        .build_module(&module)
        .expect_err("a `Reversible` must be refused: this backend cannot uncompute");
    let msg = err.to_string();
    assert!(
        msg.contains("INVERSE"),
        "the refusal must name the inverse as the missing operation: {msg}"
    );
    assert!(
        msg.contains("uncomputation"),
        "the refusal must say the missing capability is uncomputation, so a reader \
         knows this is a backend gap and not a bad program: {msg}"
    );
}

/// A `Reversible` must also be refused by the QIR backend.
///
/// QIR had the identical defect: it ran `body` and dropped `inverse`, justified
/// by the comment "The inverse would be handled by quantum compiler". Nothing
/// handled it. (`ancilla_emission.rs` was the file this comment pointed at, but it had
/// no `mod` declaration in `qir/mod.rs`, so it was never compiled and never read
/// anything. It has been deleted, not wired.)
///
/// This is the more damaging half of the bug in a QIR backend. QIR's contract is
/// that the emitted circuit is reversible; a `Reversible` with no adjoint emits
/// the forward operation alone, under a type signature asserting a reversibility
/// the output does not have. Nothing downstream executes on a QPU to catch it.
///
/// Like the LLVM case, the node is built directly as PIR: lowering refuses
/// expression-position `reversible` first, so no source program can reach here,
/// and a mutation restoring the old behaviour would survive otherwise.
#[test]
fn the_qir_backend_refuses_a_reversible_that_arrives_as_pir() {
    use naso_compiler::codegen::qir::QIRModuleBuilder;
    let _ctx = inkwell::context::Context::create();
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::Default).expect("context");
    let mut builder = QIRModuleBuilder::new(&cc).expect("builder");
    let stmt = PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::Reversible {
            body: Box::new(PirExpr::IntLit(1)),
            inverse: Box::new(PirExpr::IntLit(0)),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    let module = PirModule {
        statements: vec![stmt],
        schedule: ScheduleTree::new(
            ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
            vec![],
        ),
        ..Default::default()
    };
    let err = builder
        .build_module(&module)
        .expect_err("QIR must not emit a `Reversible` with no adjoint");
    let msg = err.to_string();
    assert!(
        msg.contains("INVERSE"),
        "the refusal must name the inverse as the missing operation: {msg}"
    );
    assert!(
        msg.contains("reversible") || msg.contains("reversibility"),
        "the refusal must say what QIR's contract is, since that is why this is \
         worse here than in a classical backend: {msg}"
    );
}

/// Both backends must refuse it, for the same reason.
///
/// If one backend accepted a `Reversible` the other refused, the same program
/// would mean two different things depending on `--target`, and a quantum kernel
/// that is safe on one target would silently drop its uncomputation on another.
/// The two refusals are checked for the shared claim rather than compared for
/// equality, because each names its own backend's consequence.
#[test]
fn llvm_and_qir_agree_that_a_reversible_is_refused() {
    use naso_compiler::codegen::qir::QIRModuleBuilder;
    let _ctx = inkwell::context::Context::create();
    let make = || PirModule {
        statements: vec![PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Reversible {
                body: Box::new(PirExpr::IntLit(1)),
                inverse: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        }],
        schedule: ScheduleTree::new(
            ScheduleNode::domain(StmtId(0), AffineDomain::universe(0, 0)),
            vec![],
        ),
        ..Default::default()
    };
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::Default).expect("context");

    let mut llvm_b = LLVMModuleBuilder::new(&cc).expect("llvm builder");
    let llvm_msg = llvm_b
        .build_module(&make())
        .expect_err("LLVM must refuse")
        .to_string();

    let mut qir_b = QIRModuleBuilder::new(&cc).expect("qir builder");
    let qir_msg = qir_b
        .build_module(&make())
        .expect_err("QIR must refuse")
        .to_string();

    for (name, msg) in [("LLVM", &llvm_msg), ("QIR", &qir_msg)] {
        assert!(
            msg.contains("INVERSE"),
            "{name} must name the inverse as the missing operation: {msg}"
        );
    }
}
