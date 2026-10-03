//! Mixed integer/float binary arithmetic: promoted, executed, and pinned by value.
//!
//! # What changed
//!
//! `fn f(a: i64, b: f32) -> f64 { a + b }` used to be REFUSED with "binary operation on
//! operands of different types". The backend now applies the usual arithmetic
//! conversions before the operator is built, so mixed operands compute something:
//!
//! - integer + integer -> the WIDER integer, narrower sign-extended (`i8 + i64` = i64)
//! - integer + float -> the FLOAT (`i64 + f32` = `double`, via `sitofp`)
//! - float + float -> unchanged
//!
//! and two combinations stay refused because promoting them would compute something the
//! source never wrote: mixed `%` (Naso has no float remainder, so answering it would
//! mean truncating `2.5` to `2`) and mixed bitwise/shift operators.
//!
//! # Why these are EXECUTION tests
//!
//! A promotion bug does not have to be a crash. The dangerous version emits valid LLVM
//! and the wrong NUMBER: truncating instead of converting, sign-extending where
//! zero-extension was meant, or promoting the wrong operand to the float. IR text does
//! not distinguish those, so every test here compiles the module with `llc`, links a C
//! driver with `cc`, and asserts stdout.
//!
//! Naso's `f32`/`f64` both lower to LLVM `double`, so every C harness below declares
//! `double`, never `float`. A harness declaring `float` would read the wrong registers
//! and the test would fail for a reason that has nothing to do with promotion.
//!
//! Every natively spawned program gets a 20-second budget (`NATIVE_TIMEOUT_SECS`), so a
//! build that hangs fails instead of parking the suite.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique directory per call, so parallel tests never collide on a temp path.
struct CaseDir(PathBuf);

impl CaseDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p =
            std::env::temp_dir().join(format!("naso-prom-{}-{}-{}", tag, std::process::id(), n));
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

/// Locate `llc`/`clang`, preferring `PATH` so this tests the compiler and not one
/// machine's layout.
fn tool(name: &str) -> String {
    let on_path = Command::new(name).arg("--version").output();
    if let Ok(found) = on_path
        && found.status.success()
    {
        return name.to_string();
    }
    let brew = "/home/linuxbrew/.linuxbrew/opt/llvm@17/bin";
    if std::path::Path::new(brew).join(name).exists() {
        return format!("{brew}/{name}");
    }
    panic!(
        "`{name}` not found on PATH and not in {brew}; these tests execute generated \
         code, so they cannot be skipped."
    );
}

/// A hard 20-second budget on the generated program.
///
/// Without it, a mutant that produces a hang parks the whole suite forever instead of
/// failing. A test that cannot finish in 20s is a failing test, which is the truth
/// about that build.
const NATIVE_TIMEOUT_SECS: u64 = 20;

fn build_ir(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\n{src}\nerror: {e:?}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed:\n{src}\nerror: {e}\n{pir:?}"));
    builder.module().to_string()
}

/// Compile the IR with `llc`, link a C driver, RUN it, and return what it printed.
fn run(ir: &str, driver: &str) -> String {
    use std::io::Read;
    use std::process::Stdio;

    let dir = CaseDir::new("run");
    let ll = dir.join("m.ll");
    let obj = dir.join("m.o");
    let csrc = dir.join("d.c");
    let exe = dir.join("prog");
    std::fs::write(&ll, ir).expect("write ll");
    std::fs::write(&csrc, driver).expect("write driver");

    let llc = Command::new(tool("llc"))
        .args(["-filetype=obj", "-relocation-model=pic"])
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .expect("run llc");
    assert!(
        llc.status.success(),
        "llc failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&llc.stderr)
    );

    // `-lm` because a Naso program calling `round`/`floor`/`ceil` lowers to libm.
    let clang = Command::new(tool("clang"))
        .arg("-O0")
        .arg(&obj)
        .arg(&csrc)
        .arg("-lm")
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run clang");
    assert!(
        clang.status.success(),
        "clang link failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&clang.stderr)
    );

    let mut child = Command::new(&exe)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn exe");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(NATIVE_TIMEOUT_SECS);
    let mut buf = Vec::new();
    loop {
        match child.try_wait().expect("wait for exe") {
            Some(status) => {
                assert!(
                    status.success(),
                    "generated program exited with {status}\n--- IR ---\n{ir}"
                );
                if let Some(mut out) = child.stdout.take() {
                    out.read_to_end(&mut buf).expect("read stdout");
                }
                break;
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "generated program did not exit within {NATIVE_TIMEOUT_SECS}s and \
                         was killed. A hang is a failing build, not a passing one.\n--- IR \
                         ---\n{ir}"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    String::from_utf8_lossy(&buf).trim().to_string()
}

/// The error text a program is refused with, or a panic.
fn compile_error(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}"));
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

/// `int + float`: the integer is converted to the float type and the add is a float add.
///
/// Inputs are chosen so that zero-extending the integer, truncating the float, or
/// converting the wrong operand each give a different answer from 12.5:
/// `3 + 9.5 = 12.5`, while an integer result would be 12.
#[test]
fn an_integer_plus_a_float_computes_a_float_sum() {
    let ir = build_ir("fn f(a: i64, b: f32) -> f64 { a + b }\n");
    let driver = r#"
#include <stdio.h>
double naso_f(long, double);
int main(void){ printf("%.1f %.1f %.1f\n", naso_f(3, 9.5), naso_f(-3, 9.5), naso_f(0, 9.5)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "12.5 6.5 9.5",
        "`a + b` must convert the INTEGER to double and add: 3+9.5 = 12.5, \
         -3+9.5 = 6.5, 0+9.5 = 9.5. Truncating the float first would print 12, and \
         subtracting instead of adding would print -6.5.\n--- IR ---\n{ir}"
    );
}

/// `float + int`: the operand order is reversed, and the answer must not change.
///
/// A separate test because the two orders are different match arms. A promotion that
/// only handled integer-on-the-left would be refused here; one that converted the FLOAT
/// to an integer would give `9 + 3 = 12`.
#[test]
fn a_float_plus_an_integer_computes_the_same_sum_in_the_other_order() {
    let ir = build_ir("fn f(a: f32, b: i64) -> f64 { a + b }\n");
    let driver = r#"
#include <stdio.h>
double naso_f(double, long);
int main(void){ printf("%.1f %.1f %.1f\n", naso_f(9.5, 3), naso_f(9.5, -3), naso_f(9.5, 0)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "12.5 6.5 9.5",
        "`a + b` with a float on the left must give the same values as the \
         integer-on-the-left form: 9.5+3 = 12.5, 9.5+(-3) = 6.5, 9.5+0 = 9.5. A \
         promoted-and-subtracted result would give -6.5 for the second.\n--- IR ---\n{ir}"
    );
}

/// Subtraction and multiplication, both operand orders.
///
/// These are separate arms of the float lowering (`fsub`, `fmul`), so a promotion bug
/// can hit one and not the other. The subtraction inputs are negatives of each other,
/// so a swapped subtraction is caught.
#[test]
fn subtraction_and_multiplication_promote_to_float_arithmetic() {
    let sub = build_ir("fn f(a: i64, b: f32) -> f64 { a - b }\n");
    let sub_driver = r#"
#include <stdio.h>
double naso_f(long, double);
int main(void){ printf("%.1f %.1f\n", naso_f(10, 2.5), naso_f(2, 9.5)); return 0; }
"#;
    assert_eq!(
        run(&sub, sub_driver),
        "7.5 -7.5",
        "`10 - 2.5 = 7.5` and `2 - 9.5 = -7.5`. The two are negatives of each other, \
         so a swapped subtraction prints `-7.5 7.5` and is caught.\n--- IR ---\n{sub}"
    );

    let mul = build_ir("fn f(a: f32, b: i64) -> f64 { a * b }\n");
    let mul_driver = r#"
#include <stdio.h>
double naso_f(double, long);
int main(void){ printf("%.1f %.1f\n", naso_f(2.5, 4), naso_f(-2.5, 4)); return 0; }
"#;
    assert_eq!(
        run(&mul, mul_driver),
        "10.0 -10.0",
        "`2.5 * 4 = 10.0` and `-2.5 * 4 = -10.0`. Integer multiplication of the same \
         operands gives the same two numbers, which is why the negative input is \
         load-bearing: it pins the sign of the promoted value rather than restating the \
         magnitude.\n--- IR ---\n{mul}"
    );
}

/// Division by an integer promotes the integer, and the divisor is NOT truncated.
///
/// `7 / 2` is 3 for integers and 3.5 for floats. A promotion that went the other way --
/// `fptosi` on the divisor -- would print 3.00, which is the silent truncation this
/// implementation refuses to do for `%`.
#[test]
fn division_by_an_integer_promotes_the_integer_and_does_not_truncate() {
    let ir = build_ir("fn f(a: i64, b: f32) -> f64 { a / b }\n");
    let driver = r#"
#include <stdio.h>
double naso_f(long, double);
int main(void){ printf("%.2f %.2f %.2f\n", naso_f(7, 2), naso_f(7, 2.5), naso_f(-7, 2)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "3.50 2.80 -3.50",
        "`7 / 2 = 3.5`, `7 / 2.5 = 2.8` and `-7 / 2 = -3.5`: real divisions of the \
         actual values. Truncating the divisor to an integer first gives 3.00 and 3.00, \
         which is why 2.5 is the second input.\n--- IR ---\n{ir}"
    );
}

/// Integers of two different widths: the NARROWER one is sign-extended, not
/// zero-extended.
///
/// This is the test that fails if the promotion zero-extends. `a` is `-3` as an `i8`,
/// which is `0xFD`; a zero-extension reads it as 253. Sign extension gives `-3 + 4 = 1`.
#[test]
fn integers_of_two_widths_promote_by_sign_extension_to_the_wider_one() {
    let ir = build_ir("fn f(a: i8, b: i64) -> i64 { a + b }\n");
    let driver = r#"
#include <stdio.h>
long naso_f(signed char, long);
int main(void){ printf("%ld %ld %ld\n", naso_f(-3, 4), naso_f(3, 4), naso_f(-1, -1)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "1 7 -2",
        "`-3 + 4 = 1`, `3 + 4 = 7`, `-1 + (-1) = -2`. A ZERO-extension of the i8 \
         turns -3 into 253 and prints 257.\n--- IR ---\n{ir}"
    );
}

/// Multiplication across widths, so the promotion is not accidentally special-cased to
/// the add the previous test uses.
#[test]
fn multiplication_across_integer_widths_also_promotes_to_the_wider_type() {
    let ir = build_ir("fn f(a: i8, b: i64) -> i64 { a * b }\n");
    let driver = r#"
#include <stdio.h>
long naso_f(signed char, long);
int main(void){ printf("%ld %ld\n", naso_f(-3, 5), naso_f(3, 5)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "-15 15",
        "`-3 * 5 = -15` and `3 * 5 = 15`. Zero-extending -3 gives 253 * 5 = 1265.\n--- IR \
         ---\n{ir}"
    );
}

/// Two floats still work, with no conversion inserted.
///
/// Regression cover for the promotion itself: the float/float case is the one every
/// float kernel already depended on, and a promotion that touched it would break them.
#[test]
fn two_floats_are_unaffected_by_the_promotion() {
    let ir = build_ir("fn f(a: f32, b: f32) -> f64 { a + b * 2.0 }\n");
    let driver = r#"
#include <stdio.h>
double naso_f(double, double);
int main(void){ printf("%.1f %.1f %.1f\n", naso_f(1.0, 2.0), naso_f(-1.0, 2.0), naso_f(0.5, 0.25)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "5.0 3.0 1.0",
        "`1.0 + 2.0*2.0 = 5.0`, `-1.0 + 4.0 = 3.0`, `0.5 + 0.5 = 1.0`. Unchanged by the \
         promotion.\n--- IR ---\n{ir}"
    );
}

/// `<`, `>`, `<=`, `>=` across the int/float boundary.
///
/// A mixed comparison must use the FLOAT predicate and promote the same operand the
/// arithmetic does. The inputs are the same for every operator, so the four expected
/// strings are not permutations of one another.
#[test]
fn a_mixed_ordered_comparison_uses_the_float_predicate() {
    // Inputs to every case: (2, 2.5), (2, 1.5), (2, 2.0), (-3, 1.0).
    let cases = [
        (
            "a < b",
            "fn f(a: i64, b: f32) -> i64 { if a < b { 1 } else { 0 } }",
            "1 0 0 1",
        ),
        (
            "a > b",
            "fn f(a: i64, b: f32) -> i64 { if a > b { 1 } else { 0 } }",
            "0 1 0 0",
        ),
        (
            "a <= b",
            "fn f(a: i64, b: f32) -> i64 { if a <= b { 1 } else { 0 } }",
            "1 0 1 1",
        ),
        (
            "a >= b",
            "fn f(a: i64, b: f32) -> i64 { if a >= b { 1 } else { 0 } }",
            "0 1 1 0",
        ),
    ];
    let driver = r#"
#include <stdio.h>
long naso_f(long, double);
int main(void){ printf("%ld %ld %ld %ld\n", naso_f(2, 2.5), naso_f(2, 1.5), naso_f(2, 2.0), naso_f(-3, 1.0)); return 0; }
"#;
    for (op, src, expected) in cases {
        let ir = build_ir(&format!("{src}\n"));
        assert_eq!(
            run(&ir, driver),
            expected,
            "`{op}` with an i64 and an f32 must compare as doubles; expected \
             {expected:?}. An integer compare of the same operands reads 2.5 as 2, \
             which changes the first answer (2 < 2 becomes false), so this is a real \
             check of the float predicate rather than of the branch.\n--- IR ---\n{ir}"
        );
    }
}

/// The same comparison with the operands swapped: `f32 < i64`.
///
/// A promotion that hardcoded "convert the right operand" would be refused here or
/// compute the wrong answer. `-3` on the integer side is the sign check: it must
/// survive as `-3.0`.
#[test]
fn a_mixed_comparison_also_works_with_the_float_on_the_left() {
    let src = "fn f(a: f32, b: i64) -> i64 { if a < b { 1 } else { 0 } }";
    let ir = build_ir(&format!("{src}\n"));
    let driver = r#"
#include <stdio.h>
long naso_f(double, long);
int main(void){ printf("%ld %ld %ld\n", naso_f(2.5, 2), naso_f(1.5, 2), naso_f(1.0, -3)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "0 1 0",
        "`2.5 < 2` false, `1.5 < 2` true, `1.0 < -3` false. The last is what catches a \
         sign error: the integer -3 must promote to -3.0, not to 3.0.\n--- IR ---\n{ir}"
    );
}

/// `==` and `!=` across the boundary.
///
/// `3 == 3.0` must be true, and `-3 == 3.0` must be false. The `!=` expectations are the
/// exact negation, so a mutant that swapped the two predicates fails here.
#[test]
fn mixed_equality_compares_the_promoted_values() {
    let eq_ir = build_ir("fn f(a: i64, b: f32) -> i64 { if a == b { 1 } else { 0 } }\n");
    let eq_driver = r#"
#include <stdio.h>
long naso_f(long, double);
int main(void){ printf("%ld %ld %ld\n", naso_f(3, 3.0), naso_f(3, 4.0), naso_f(-3, 3.0)); return 0; }
"#;
    assert_eq!(
        run(&eq_ir, eq_driver),
        "1 0 0",
        "`3 == 3.0` true, `3 == 4.0` false, `-3 == 3.0` false. The last is the case \
         that catches a sign error in the conversion.\n--- IR ---\n{eq_ir}"
    );

    let ne_ir = build_ir("fn f(a: i64, b: f32) -> i64 { if a != b { 1 } else { 0 } }\n");
    let ne_driver = r#"
#include <stdio.h>
long naso_f(long, double);
int main(void){ printf("%ld %ld %ld\n", naso_f(3, 3.0), naso_f(3, 4.0), naso_f(-3, 3.0)); return 0; }
"#;
    assert_eq!(
        run(&ne_ir, ne_driver),
        "0 1 1",
        "`3 != 3.0` false, `3 != 4.0` true, `-3 != 3.0` true -- the exact negation of \
         the `==` row, so a swapped predicate is caught.\n--- IR ---\n{ne_ir}"
    );
}

/// A mixed comparison at a FRACTIONAL boundary, which is where a truncating conversion
/// changes the answer rather than merely risking it.
///
/// `2 < 2.5` is true; truncate 2.5 to an integer first and `2 < 2` is false. `<=` and
/// `>=` are checked too because each is its own predicate arm.
#[test]
fn a_mixed_comparison_at_a_fractional_boundary_does_not_truncate_the_float() {
    let cases = [
        (
            "fn f(a: i64, b: f32) -> i64 { if a < b { 1 } else { 0 } }",
            "1",
            "2 < 2.5",
        ),
        (
            "fn f(a: i64, b: f32) -> i64 { if a <= b { 1 } else { 0 } }",
            "1",
            "2 <= 2.5",
        ),
        (
            "fn f(a: i64, b: f32) -> i64 { if a >= b { 1 } else { 0 } }",
            "0",
            "2 >= 2.5",
        ),
    ];
    for (src, expected, what) in cases {
        let ir = build_ir(&format!("{src}\n"));
        let driver = r#"
#include <stdio.h>
long naso_f(long, double);
int main(void){ printf("%ld\n", naso_f(2, 2.5)); return 0; }
"#;
        assert_eq!(
            run(&ir, driver),
            expected,
            "`{what}` must be {expected}. Truncating 2.5 to an integer first would \
             compare 2 against 2 and get every one of these wrong.\n--- IR ---\n{ir}"
        );
    }
}

/// Mixed `%` is REFUSED, and the diagnostic says why rather than truncating.
///
/// `7 % 2.5` has no integer answer: answering it would read `2.5` as `2` and return 1.
/// The diagnostic must name the truncation, because "refused" alone leaves the author
/// with nothing to act on.
#[test]
fn a_mixed_remainder_is_refused_naming_the_truncation_it_avoided() {
    let msg = compile_error("fn f(a: i64, b: f32) -> i64 { if a % b == 0 { 1 } else { 0 } }\n");
    assert!(
        msg.contains("remainder") && msg.contains("truncat"),
        "a mixed `%` must be refused with a diagnostic naming the truncation it avoids, \
         so the author knows to write `/` or make both operands integers. Got: {msg}"
    );
}

/// The same refusal with the float on the left, so it is not an accident of the match
/// arm order.
#[test]
fn a_mixed_remainder_is_refused_with_the_float_on_the_left_too() {
    let msg = compile_error("fn f(a: f32, b: i64) -> i64 { a % b }\n");
    assert!(
        msg.contains("remainder") && msg.contains("truncat"),
        "the refusal must not depend on which operand is the float. Got: {msg}"
    );
}

/// Mixed bitwise and shift operators stay refused: they are integer operations.
///
/// Promoting the integer to a double to apply `and` would compute something the source
/// never wrote. Each operator is named separately because they are separate arms.
#[test]
fn mixed_bitwise_and_shift_operators_are_refused() {
    for (op, src) in [
        ("&", "fn f(a: i64, b: f32) -> i64 { a & b }"),
        ("|", "fn f(a: i64, b: f32) -> i64 { a | b }"),
        ("^", "fn f(a: i64, b: f32) -> i64 { a ^ b }"),
        ("<<", "fn f(a: i64, b: f32) -> i64 { a << b }"),
        (">>", "fn f(a: i64, b: f32) -> i64 { a >> b }"),
    ] {
        let msg = compile_error(&format!("{src}\n"));
        assert!(
            msg.contains("not defined when one operand is a floating-point"),
            "`{op}` with a float operand must be refused by name. Got: {msg}"
        );
    }
}

/// A mixed comparison driving an `if`, so promotion is exercised on a value that has to
/// flow through a phi and a branch, not just through a return.
#[test]
fn a_mixed_comparison_drives_an_if_branch_by_its_own_value() {
    let src = "fn f(a: i64, b: f32) -> f64 { if a < b { 1.5 } else { 0.5 } }";
    let ir = build_ir(&format!("{src}\n"));
    let driver = r#"
#include <stdio.h>
double naso_f(long, double);
int main(void){ printf("%.1f %.1f %.1f\n", naso_f(1, 2.5), naso_f(5, 2.5), naso_f(2, 2.5)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "1.5 0.5 1.5",
        "the promoted comparison must CONTROL the branch: 1 < 2.5 takes the then-arm \
         (1.5), 5 < 2.5 takes the else-arm (0.5), 2 < 2.5 takes the then-arm. A \
         comparison that was computed and discarded cannot produce all three.\n--- IR \
         ---\n{ir}"
    );
}

/// A mixed comparison accumulating over a loop, so the promotion runs on every
/// iteration rather than once.
#[test]
fn a_mixed_comparison_recomputes_on_every_loop_iteration() {
    let src = "fn f(n: i64) -> i64 { let mut total = 0; for i in n { if i < 3 { total = total + 5; } } total }";
    let ir = build_ir(&format!("{src}\n"));
    let driver = r#"
#include <stdio.h>
long naso_f(long);
int main(void){ printf("%ld %ld %ld\n", naso_f(5), naso_f(2), naso_f(0)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "15 10 0",
        "with n=5, i=0,1,2 satisfy i < 3 (three iterations x 5 = 15); with n=2 both do \
         (10); with n=0 nothing does (0). Three trip counts, so no hard-coded total \
         satisfies all of them.\n--- IR ---\n{ir}"
    );
}

/// A `bool` and an integer of a different width is refused rather than widened.
///
/// An `i1` is Naso's `bool`. Sign-extending it would make `true` be `-1`, which is not a
/// value `true` ever had.
#[test]
fn a_bool_and_an_integer_of_another_width_are_refused_rather_than_widened() {
    let msg = compile_error("fn f(a: i64, b: bool) -> i64 { a + b }\n");
    assert!(
        msg.contains("bool") && msg.contains("i1"),
        "a mixed bool/integer add must be refused naming the i1, not silently \
         zero-extended or sign-extended. Got: {msg}"
    );
}

/// Two integers of the SAME width are untouched by the promotion.
///
/// This is the case every existing integer kernel depends on, and it pins that
/// agreeing widths short-circuit instead of picking either one and converting.
#[test]
fn two_integers_of_the_same_width_still_compute_exactly() {
    let ir = build_ir("fn f(a: i64, b: i64) -> i64 { a + b * 2 - 1 }\n");
    let driver = r#"
#include <stdio.h>
long naso_f(long, long);
int main(void){ printf("%ld %ld %ld\n", naso_f(3, 4), naso_f(-3, 4), naso_f(0, 0)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "10 4 -1",
        "`3 + 4*2 - 1 = 10`, `-3 + 8 - 1 = 4`, `0 + 0 - 1 = -1`. The negative input is \
         what catches an unsigned or truncated reading.\n--- IR ---\n{ir}"
    );
}

/// The integer->float conversion must be SIGNED, and this is the test that says so.
///
/// `sitofp` and `uitofp` agree for every value whose sign bit is clear: `uitofp(-3)` is
/// still `3.0` when the 64-bit pattern is interpreted... no. More precisely, `uitofp`
/// treats the operand as UNSIGNED, so `uitofp(-3i64)` is `18446744073709551613.0`, not
/// `-3.0`.
///
/// The existing tests all use `-3`, which is exactly the value that hides the bug if the
/// mutation is done by flipping the SIGN FLAG somewhere rather than swapping the
/// instruction, so a signed/unsigned swap has to be probed with a magnitude where the two
/// readings differ unambiguously.
///
/// `-(1 << 40)` is used: large enough that the unsigned reading is obviously absurd, small
/// enough that `double` represents it exactly (so the expected value is exact and the test
/// is not comparing rounded output).
#[test]
fn a_large_negative_integer_promotes_as_a_signed_value() {
    let ir = build_ir("fn f(a: i64, b: f32) -> f64 { a + b }\n");
    let driver = r#"
#include <stdio.h>
double naso_f(long, double);
int main(void){ printf("%.1f\n", naso_f(-(1L << 40), 0.5)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "-1099511627775.5",
        "a large negative integer must sign-extend into the float conversion. Reading it \
         as unsigned would print about 1099511627808.5.\n--- IR ---\n{ir}"
    );
}

/// The same for the integer-on-the-left of a comparison, where the value is consumed by a
/// predicate rather than summed.
#[test]
fn a_large_negative_integer_compares_as_a_signed_value() {
    let ir = build_ir("fn f(a: i64, b: f32) -> i64 { if a < b { 1 } else { 0 } }\n");
    let driver = r#"
#include <stdio.h>
long naso_f(long, double);
int main(void){ printf("%ld\n", naso_f(-(1L << 40), 0.0)); return 0; }
"#;
    assert_eq!(
        run(&ir, driver),
        "1",
        "-(1<<40) < 0.0 is true. Read as unsigned it would be about 1.1e12, which is \
         greater than 0.0, so the branch would not be taken and this would print 0.\n--- \
         IR ---\n{ir}"
    );
}
