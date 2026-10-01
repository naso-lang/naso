//! End-to-end execution of compiler-generated LLVM IR on the CPU.
//!
//! # Why this file exists
//!
//! Every other LLVM test in this repo asserts on IR TEXT. Text is weak evidence: a
//! `fptosi` on a constant is constant-folded into the following store before the module
//! is printed, so a correctly-emitted conversion is INVISIBLE in the IR string, and a
//! dropped conversion looks identical. That is not hypothetical -- an earlier version
//! of the cast test asserted `ir.contains("fptosi")` and passed or failed for reasons
//! that had nothing to do with the code under test.
//!
//! So these tests COMPILE and RUN the module natively, and read the value the machine
//! produced. That is the only check which cannot be satisfied by an arm that computes
//! the right value and discards it, or by one that never converted at all.
//!
//! # How it runs
//!
//! `llc` compiles the generated `.ll` to an x86-64 object, a tiny C `main` calls the
//! generated `stmt_0` and prints the result global, and the test parses that output.
//! `lli` was tried first and rejected: it reports only an 8-bit exit status, so an i64
//! result cannot be read from it, and inkwell 0.10 no longer exposes a JIT constructor.
//!
//! The generated statements are `define void @stmt_N()`, so there is nothing to return;
//! the value is observed through an i64 global the statement stores into.
//!
//! # Storage
//!
//! Each case writes to a per-process temp directory which is removed immediately after
//! the run, including on panic paths, so a test run leaves no build artifacts behind.
//!
//! # What is still not covered
//!
//! No GPU. There is no `/dev/dri`, no Vulkan ICD and no `navigator.gpu` in this
//! environment, so nothing here can speak for generated WGSL on real hardware.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;
use naso_compiler::ir::affine_domain::AffineDomain;
use naso_compiler::ir::pir_types::{PirExpr, PirModule, PirStatement};
use naso_compiler::ir::schedule_tree::StmtId;

/// The global the generated statement stores its result into.
const RESULT_GLOBAL: &str = "result";

/// LLVM toolchain prefix. The suite is run with `LLVM_SYS_170_PREFIX` set; falling back
/// to bare names lets it work when the tools are simply on `PATH`.
fn tool(name: &str) -> String {
    match std::env::var("LLVM_SYS_170_PREFIX") {
        Ok(prefix) if !prefix.is_empty() => format!("{prefix}/bin/{name}"),
        _ => name.to_string(),
    }
}

/// A temp directory for ONE case, removed on drop.
///
/// The name is unique per call, not per process: the test harness runs tests as
/// threads of a single process, so a per-process directory would have concurrent cases
/// overwrite each other's `case.ll` and `case` binary. That produced tests reading each
/// other's results -- 300 where 5 was expected -- which looked like a backend bug.
struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-llvm-exec-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        CaseDir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The C driver: call the generated statement, then print the result global.
const HARNESS_C: &str = r#"
#include <stdint.h>
#include <stdio.h>
extern void stmt_0(void);
extern int64_t result;
int main(void) {
    stmt_0();
    printf("%lld\n", (long long)result);
    return 0;
}
"#;

/// Same driver, but reading an i8 destination.
const HARNESS_C_I8: &str = r#"
#include <stdint.h>
#include <stdio.h>
extern void stmt_0(void);
extern int8_t result;
int main(void) {
    stmt_0();
    printf("%d\n", (int)result);
    return 0;
}
"#;

/// Same driver, but reading an i32 destination.
const HARNESS_C_I32: &str = r#"
#include <stdint.h>
#include <stdio.h>
extern void stmt_0(void);
extern int32_t result;
int main(void) {
    stmt_0();
    printf("%d\n", (int)result);
    return 0;
}
"#;

/// The C driver matching a destination width.
fn harness_for(width: naso_compiler::codegen::abi::IntWidth) -> &'static str {
    use naso_compiler::codegen::abi::IntWidth as W;
    match width {
        W::I8 => HARNESS_C_I8,
        W::I32 => HARNESS_C_I32,
        // Everything else in this file uses the i64 driver.
        _ => HARNESS_C,
    }
}

/// Build the compiler's IR for one statement storing into `result`, and return the text.
///
/// `width` is the destination's bit width. It is a parameter rather than a fixed i64 so
/// that a cast which CHANGES the width is representable: with a fixed i64 destination
/// and an i64 `IntLit` source, every int->int cast in this file was a no-op, the
/// conversion arm short-circuited, and three separate mutations of that arm
/// (`fptosi`->`fptoui`, and forcing `sext` or `zext`) all SURVIVED the suite.
fn build_storing_ir_at(value: PirExpr, width: naso_compiler::codegen::abi::IntWidth) -> String {
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");

    // The destination width MATCHES the value's width. The Assign arm deliberately
    // refuses a mismatch rather than inserting a conversion of its own: silently
    // widening there would be exactly the class of wrong-answer bug this backend had
    // when whole loop bodies were dropped.
    let dest = builder.type_lowering().int_type(width);
    let global = builder.module().add_global(dest, None, RESULT_GLOBAL);
    global.set_initializer(&dest.const_zero());
    builder.add_variable(
        RESULT_GLOBAL.to_string(),
        global.as_pointer_value(),
        dest.into(),
    );

    let stmt = PirStatement {
        id: StmtId(0),
        domain: AffineDomain::universe(0, 0),
        body: PirExpr::Assign {
            target: Box::new(PirExpr::Var(RESULT_GLOBAL.to_string())),
            value: Box::new(value),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };
    let module = PirModule {
        statements: vec![stmt],
        ..Default::default()
    };
    builder
        .build_module(&module)
        .unwrap_or_else(|e| panic!("lowering to IR failed: {e}"));
    builder.module_to_string()
}

/// Build, compile, run, and return `(value, ir)` for a store of `value` into an i64
/// destination.
fn build_storing_ir(value: PirExpr) -> String {
    build_storing_ir_at(value, naso_compiler::codegen::abi::IntWidth::I64)
}

/// Compile `ir` to x86-64, link it with the C harness, run it, and return the printed
/// value. This is a genuine native execution, not an inspection of the IR.
///
/// `width` must match the destination global, since the C harness reads the global with
/// that type.
fn execute(ir: &str, width: naso_compiler::codegen::abi::IntWidth) -> i64 {
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("harness.c");
    let exe = dir.path("case");

    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&csrc, harness_for(width)).expect("write C harness");

    run(
        Command::new(tool("llc"))
            .arg("-filetype=obj")
            .arg(&ll)
            .arg("-o")
            .arg(&obj),
        "llc",
        ir,
    );
    run(
        Command::new(tool("clang"))
            .arg(&csrc)
            .arg(&obj)
            .arg("-o")
            .arg(&exe),
        "clang",
        ir,
    );
    let out = run(&mut Command::new(&exe), "the compiled program", ir);

    String::from_utf8_lossy(&out)
        .trim()
        .parse::<i64>()
        .unwrap_or_else(|e| panic!("harness printed {e}; expected an integer. IR:\n{ir}"))
}

/// Run a command, panicking with the compiler's IR attached if it fails.
fn run(cmd: &mut Command, what: &str, ir: &str) -> Vec<u8> {
    let out = cmd.output().unwrap_or_else(|e| {
        panic!("could not start `{what}`: {e}. Is the LLVM 17 toolchain on PATH?")
    });
    assert!(
        out.status.success(),
        "{what} failed ({:?}):\n{}\n--- compiler-generated IR ---\n{ir}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Build, compile, run, and return `(value, ir)` for a store of `value` into an i64
/// destination.
fn eval(value: PirExpr) -> (i64, String) {
    eval_at(value, naso_compiler::codegen::abi::IntWidth::I64)
}

/// Build, compile, run, and return `(value, ir)` for a store into a `width` destination.
fn eval_at(value: PirExpr, width: naso_compiler::codegen::abi::IntWidth) -> (i64, String) {
    let ir = build_storing_ir_at(value, width);
    let got = execute(&ir, width);
    (got, ir)
}

/// `5.7 as i64` must be 5, on the CPU.
///
/// This is the check a text assertion cannot make. LLVM folds `fptosi` of a constant into
/// the following store, so the conversion instruction does not appear in the IR at all
/// even when the conversion happened. Only running it distinguishes "converted
/// correctly" from "converted to poison" from "never converted".
#[test]
fn a_float_to_i64_cast_produces_the_truncated_integer_on_the_cpu() {
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(PirExpr::FloatLit("5.7".into())),
        width: Some(64),
        signed: true,
    });
    assert_eq!(got, 5, "5.7 as i64 must be 5. Got {got}.\n--- IR ---\n{ir}");
}

#[test]
fn a_float_cast_truncates_toward_zero_rather_than_rounding() {
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(PirExpr::FloatLit("300.7".into())),
        width: Some(64),
        signed: true,
    });
    assert_eq!(
        got, 300,
        "300.7 as i64 must truncate toward zero to 300, not round to 301. Got {got}.\n--- IR ---\n{ir}"
    );
}

#[test]
fn a_widening_cast_preserves_the_value() {
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(PirExpr::IntLit(700)),
        width: Some(64),
        signed: true,
    });
    assert_eq!(
        got, 700,
        "a widening cast must preserve 700. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// -5 widened to i64 must be -5. `zext` would give 4294967291, so executing this
/// distinguishes sign-extension from zero-extension without reading IR text.
#[test]
fn a_negative_source_widens_by_sign_extension() {
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(PirExpr::IntLit(-5)),
        width: Some(64),
        signed: true,
    });
    assert_eq!(
        got, -5,
        "a signed widening cast must sign-extend; zext would give {}. Got {got}.\n--- IR ---\n{ir}",
        4294967291u32 as i64
    );
}

/// The control for every test above.
///
/// If this fails, the harness is wrong rather than the cast logic, so it is a real
/// assertion and not filler: without it a harness that always printed 0 would pass the
/// other four.
#[test]
fn the_harness_reports_an_uncast_value_unchanged() {
    let (got, ir) = eval(PirExpr::IntLit(42));
    assert_eq!(
        got, 42,
        "an uncast 42 must reach the CPU as 42. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// A second control with a different magnitude, so a harness printing a fixed small
/// value cannot satisfy both controls.
#[test]
fn the_harness_reports_a_large_uncast_value_unchanged() {
    let (got, ir) = eval(PirExpr::IntLit(1234567));
    assert_eq!(
        got, 1234567,
        "a large uncast value must survive the round trip. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// 700 truncated to 8 bits is -68 on two's complement: 700 = 2*256 + 188, and 188 - 256
/// = -68.
///
/// This is the test that makes the int->int conversion arm OBSERVABLE. With an i64
/// destination and an i64 `IntLit` source, every int->int cast is a no-op, the arm
/// short-circuits, and mutations of the sign flag and the trunc/sext/zext choice all
/// survived. Storing into an i8 destination forces a real conversion to happen.
#[test]
fn a_narrowing_integer_cast_drops_the_high_bits_on_the_cpu() {
    let (got, ir) = eval_at(
        PirExpr::Cast {
            expr: Box::new(PirExpr::IntLit(700)),
            width: Some(8),
            signed: true,
        },
        naso_compiler::codegen::abi::IntWidth::I8,
    );
    assert_eq!(
        got, -68,
        "700 truncated to 8 bits is -68 (two's complement). Got {got}.\n--- IR ---\n{ir}"
    );
}

/// 300 as i8 is 300 - 256 = 44. Positive, so it distinguishes `trunc` from an
/// extension in the other direction as well.
#[test]
fn a_narrowing_cast_of_a_positive_value_truncates_to_the_low_byte() {
    let (got, ir) = eval_at(
        PirExpr::Cast {
            expr: Box::new(PirExpr::IntLit(300)),
            width: Some(8),
            signed: true,
        },
        naso_compiler::codegen::abi::IntWidth::I8,
    );
    assert_eq!(
        got, 44,
        "300 truncated to 8 bits is 44. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// A float cast to a NARROW integer: 300.7 as i32 is 300, stored in an i32 slot.
///
/// This separates `fptosi` from `fptoui`: `fptoui` of a negative source is poison, so
/// the positive value here would still pass under `fptoui` -- but the negative test
/// below would not.
#[test]
fn a_float_to_i32_cast_truncates_into_the_narrow_slot() {
    let (got, ir) = eval_at(
        PirExpr::Cast {
            expr: Box::new(PirExpr::FloatLit("300.7".into())),
            width: Some(32),
            signed: true,
        },
        naso_compiler::codegen::abi::IntWidth::I32,
    );
    assert_eq!(
        got, 300,
        "300.7 as i32 must be 300. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// The float-to-int signedness discriminator.
///
/// `fptosi` of -5.7 is -5. `fptoui` of a negative value is POISON, so under `fptoui`
/// this program produces an undefined value and this assertion fails. That is what
/// makes the signed conversion observable, which an all-positive test suite could not
/// do.
#[test]
fn a_negative_float_to_int_cast_is_signed() {
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(PirExpr::FloatLit("-5.7".into())),
        width: Some(64),
        signed: true,
    });
    assert_eq!(
        got, -5,
        "-5.7 as i64 must be -5; a negative result is only possible via fptosi, \
         since fptoui of a negative value is poison. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// A negative integer narrowed to i8 keeps its sign: -300 fits in i8 as -44.
///
/// `zext` of the low byte (0xD4 = 212) would give 212, so this is the int->int
/// sign-handling discriminator that `a_narrowing_integer_cast_drops_the_high_bits`
/// cannot provide.
#[test]
fn a_negative_narrowing_cast_sign_extends_rather_than_zero_extends() {
    let (got, ir) = eval_at(
        PirExpr::Cast {
            expr: Box::new(PirExpr::IntLit(-300)),
            width: Some(8),
            signed: true,
        },
        naso_compiler::codegen::abi::IntWidth::I8,
    );
    assert_eq!(
        got, -44,
        "-300 truncated to 8 bits is -44. zext of the low byte would give 212. \
         Got {got}.\n--- IR ---\n{ir}"
    );
}

/// The int->int WIDENING discriminator, via a nested cast.
///
/// LLVM's sign flag on an integer cast only affects WIDENING; a narrowing `trunc` is
/// the same instruction either way. So every narrowing test above passed even with the
/// sign flag forced to `true` or forced to `false` -- two mutants survived until this
/// test existed.
///
/// A nested cast is the way to get a narrow source: `IntLit` always lowers to i64, so
/// the inner cast produces the i8 and the outer cast widens it back to i64, where
/// `sext` gives -44 and `zext` gives 212.
#[test]
fn a_narrow_then_widen_cast_round_trips_a_negative_value() {
    let inner = PirExpr::Cast {
        expr: Box::new(PirExpr::IntLit(-300)),
        width: Some(8),
        signed: true,
    };
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(inner),
        width: Some(64),
        signed: true,
    });
    assert_eq!(
        got, -44,
        "-300 narrowed to i8 then widened back to i64 must be -44. A zext widening \
         would give 212. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// The same round trip with a positive value, so a harness that always produced a
/// negative answer cannot pass both.
#[test]
fn a_narrow_then_widen_cast_round_trips_a_positive_value() {
    let inner = PirExpr::Cast {
        expr: Box::new(PirExpr::IntLit(300)),
        width: Some(8),
        signed: true,
    };
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(inner),
        width: Some(64),
        signed: true,
    });
    assert_eq!(
        got, 44,
        "300 narrowed to i8 then widened back to i64 must be 44. Got {got}.\n--- IR ---\n{ir}"
    );
}

/// The UNSIGNED widening discriminator.
///
/// 200 truncated to 8 bits is 0xC8, which is -56 read as signed. Widening it as UNSIGNED
/// must recover 200 (`zext`); widening it as signed would give -56 (`sext`).
///
/// This is the last mutation that survived: every earlier test declared `signed: true`,
/// and for a signed widening `sext` and the correct answer agree, so forcing `sext`
/// everywhere passed. Only an unsigned widening of a bit pattern whose sign bit is set
/// separates them.
#[test]
fn an_unsigned_widening_cast_uses_zero_extension() {
    let inner = PirExpr::Cast {
        expr: Box::new(PirExpr::IntLit(200)),
        width: Some(8),
        // Narrowing: the bit pattern is all that matters here.
        signed: false,
    };
    let (got, ir) = eval(PirExpr::Cast {
        expr: Box::new(inner),
        width: Some(64),
        signed: false,
    });
    assert_eq!(
        got, 200,
        "200 as u8 widened to u64 must be 200; a sext would give -56. \
         Got {got}.\n--- IR ---
{ir}"
    );
}

/// The `let mut total = 0; total = total + i;` pattern a `forall` loop body needs is
/// NOT yet lowerable, and this test pins WHY rather than leaving it to a comment.
///
/// Two separate structural problems, both now producing honest diagnostics instead of
/// silently wrong code:
///
/// 1. `build_module` emits ONE FUNCTION PER PIR STATEMENT (`define void @stmt_N()`).
///    An alloca created for statement 0 lives in `stmt_0`, so statement 1 cannot refer
///    to it. The Assign arm therefore reports "no allocation is known for it".
/// 2. `ExprKind::Let` lowers to `PirExpr::Let { body: IntLit(0) }` -- the binding's body
///    is a fabricated literal, because `LetBinding` carries no body. The binding is
///    scoped to its own statement for the same reason.
///
/// This is a real gap, not a passing configuration: a `forall` whose body accumulates
/// into an outer variable does not compile to LLVM yet. The test asserts the DIAGNOSTIC,
/// so that if the gap is ever closed the test fails and has to be rewritten into a
/// numeric execution check -- which is the check that would actually prove the loop runs.
#[test]
fn a_loop_body_that_writes_an_outer_binding_is_refused_not_silently_dropped() {
    let ir = {
        let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("context");
        let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");
        // Statement 0: bind `total` (as `let mut total = 0` does).
        let bind = PirStatement {
            id: StmtId(0),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Let {
                name: "total".into(),
                qty: Quantity::Many,
                mutability: Mutability::Mut,
                value: Box::new(PirExpr::IntLit(0)),
                body: Box::new(PirExpr::IntLit(0)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };
        // Statement 1: `total = total + 1`, as a loop body would.
        let write = PirStatement {
            id: StmtId(1),
            domain: AffineDomain::universe(0, 0),
            body: PirExpr::Assign {
                target: Box::new(PirExpr::Var("total".into())),
                value: Box::new(PirExpr::IntLit(1)),
            },
            quantity: Quantity::Many,
            mutability: Mutability::Immutable,
            span: None,
        };
        let module = PirModule {
            statements: vec![bind, write],
            ..Default::default()
        };
        let err = builder
            .build_module(&module)
            .expect_err("a cross-statement write has no destination allocation");
        err.to_string()
    };

    assert!(
        ir.contains("no allocation is known"),
        "the diagnostic must explain the missing destination, not silently drop the \
         write: {ir}"
    );
}
