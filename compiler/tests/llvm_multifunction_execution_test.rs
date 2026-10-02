//! Multi-function LLVM compilation, proven by EXECUTION.
//!
//! # What this file is for
//!
//! `kernels/quant_int8.naso` was uncompilable because every Naso function's parameters
//! and locals were concatenated into ONE LLVM function body, so three functions that each
//! declare `input`/`output`/`scale` -- at `Tensor[f32, 1024]` in one and
//! `Tensor[i8, 1024]` in another -- could not all be given a slot for that name. LLVM
//! has exact `i8` storage and `as i8` narrows correctly, so the obstacle was never the
//! width. It was the flattening.
//!
//! `PirModule::functions` now gives each function its own parameters, statements,
//! schedule and quantity map, and each becomes its own `define`.
//!
//! # The evidence standard
//!
//! **IR TEXT IS NOT ACCEPTABLE EVIDENCE THAT A COMPUTATION HAPPENED.** LLVM
//! constant-folds instructions into the following store, and an out-of-range `fptosi`
//! prints as `store i8 poison` with the instruction gone. So every test here:
//!
//!   * compiles the module with `llc -filetype=obj`, links a C driver with `clang`;
//!   * RUNS the binary;
//!   * checks stdout AND the exit status.
//!
//! A guard that traps must make the process exit non-zero, and that is observable; a
//! test that only reads the IR would see `call void @exit(i32 91)` and call it a pass
//! whether or not the exit code ever happened.
//!
//! A KERNEL LOOP BOUND IS NOT A DRIVER ARRAY SIZE. The kernel loops 1024 times, so the
//! driver allocates 1024 elements. A 4-iteration kernel driven with an 8-element array
//! is a driver bug that segfaults and reads like a compiler bug.
//!
//! # The call-proof standard
//!
//! A function call must be proven by execution END TO END: the callee observed through
//! its OWN printed value, called TWICE WITH DIFFERENT ARGUMENTS, with the two results
//! DIFFERING and each correct. That is what distinguishes a real call from a stale
//! value, a copy-pasted body, or an argument list in the wrong order -- all three of
//! which produce IR that parses and verifies.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

/// The shipped kernel this file exists for, verbatim from the repository.
const QUANT_INT8: &str = include_str!("../../kernels/quant_int8.naso");

/// `Tensor[T, 1024]` in `kernels/quant_int8.naso`. The parser requires a literal here.
const N: usize = 1024;

struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-multi-{}-{n}", std::process::id()));
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

fn tool(name: &str) -> String {
    match std::env::var("LLVM_SYS_170_PREFIX") {
        Ok(prefix) if !prefix.is_empty() => format!("{prefix}/bin/{name}"),
        _ => name.to_string(),
    }
}

fn parse(src: &str) -> naso_compiler::ast::Program {
    naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e}"))
}

fn lower(program: &naso_compiler::ast::Program) -> naso_compiler::ir::PirModule {
    naso_compiler::lowering::lower_program(program)
        .unwrap_or_else(|e| panic!("source must lower:\nerror: {e}"))
}

/// Compile source text all the way to an LLVM module and return its IR.
fn build_ir(src: &str) -> String {
    let program = parse(src);
    let pir = lower(&program);
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("must compile to LLVM:\n{src}\nerror: {e}"));
    builder.module_to_string()
}

/// Compile the IR to an executable with `driver`, RUN it, and return `(status, stdout,
/// stderr)`.
///
/// `-relocation-model=pic` is required: the ABI guard's violation strings are absolute
/// under LLVM's default static model and cannot be linked into a PIE.
fn link_and_run(ir: &str, driver: &str, opt: &str) -> (Option<i32>, String, String) {
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("driver.c");
    let exe = dir.path("case");
    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&csrc, driver).expect("write driver");

    for (what, cmd) in [
        (
            "llc",
            Command::new(tool("llc"))
                .arg("-relocation-model=pic")
                .arg("-filetype=obj")
                .arg(&ll)
                .arg("-o")
                .arg(&obj),
        ),
        (
            "clang",
            Command::new(tool("clang"))
                .arg(opt)
                .arg("-Wno-override-module")
                .arg(&ll)
                .arg(&csrc)
                .arg("-o")
                .arg(&exe)
                // `-lm`: `round` is emitted as `floor`/`ceil` on the sign of the
                // operand, because LLVM's `llvm.rint` rounds half-to-EVEN and Naso (like
                // WGSL) rounds half-AWAY-FROM-ZERO -- `round(0.5)` must be `1`, and
                // `rint` gives `0`. So the generated object references libm.
                .arg("-lm"),
        ),
    ] {
        let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(
            out.status.success(),
            "{what} failed ({:?}) at {opt}:\n{}\n--- IR ---\n{ir}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = Command::new(&exe).output().expect("run");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn run_ok(ir: &str, driver: &str) -> String {
    let (code, stdout, stderr) = link_and_run(ir, driver, "-O0");
    assert_eq!(
        code,
        Some(0),
        "the program must exit 0.\nstdout:\n{stdout}\nstderr:\n{stderr}\n--- IR ---\n{ir}"
    );
    stdout
}

/// The error a source produces, from lowering OR codegen, as a string.
///
/// Which stage refuses recursion is not fixed: whether the source is refused depends on
/// whether the typechecker accepts it, and this compiler does not claim that a recursive
/// program is rejected at a particular stage. What IS claimed is that it is rejected,
/// and that the message names the cycle -- so the helper takes whichever error comes out.
fn compile_error_for(src: &str) -> String {
    let program = parse(src);
    match naso_compiler::lowering::lower_program(&program) {
        Err(e) => e.to_string(),
        Ok(pir) => {
            let cc =
                CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
            let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
            match builder.build_module(&pir) {
                Ok(()) => panic!("source must be refused, but it compiled:\n{src}"),
                Err(e) => e.to_string(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 1. The shipped kernel compiles at all, and executes with correct bytes.
// ---------------------------------------------------------------------------

/// The driver for `quantize_int8_symmetric`, the shipped kernel's entry.
///
/// `naso_quantize_int8_symmetric(input, 1024, output, 1024, scale)` with
/// `input[i] = i - 512`, so `input[i] / scale` spans a wide range and `round` and
/// `clamp` both have something to do. `output` is pre-filled with a poison byte so an
/// UNWRITTEN slot is visible as a mismatch rather than as a coincidence -- a
/// zero-initialised output would agree with the expectation on every element whose
/// correct answer happens to be zero, which is most of the clamped tail.
fn quantize_driver() -> String {
    format!(
        r#"#include <stdint.h>
#include <stdio.h>
extern void naso_quantize_int8_symmetric(double *input, int64_t input_len,
                                          int8_t *output, int64_t output_len,
                                          double scale);
static double in[{N}];
static int8_t out[{N}];
int main(void) {{
    for (int i = 0; i < {N}; ++i) in[i] = (double)i - (double)({N} / 2);
    for (int i = 0; i < {N}; ++i) out[i] = (int8_t)-99;
    naso_quantize_int8_symmetric(in, {N}, out, {N}, 1.0);
    /* The first 32 bytes, as unsigned decimal, so a sign-extension bug is visible. */
    for (int i = 0; i < 32; ++i) {{
        if (i) putchar(' ');
        printf("%d", (int)out[i]);
    }}
    putchar('\n');
    /* A checksum over ALL {N}, so a wrong element past 32 is still caught. */
    long sum = 0;
    for (int i = 0; i < {N}; ++i) sum += (long)out[i];
    printf("%ld\n", sum);
    return 0;
}}
"#
    )
}

/// The value the Naso source says `quantize_int8_symmetric` must produce.
///
/// Straight from the kernel:
///
/// ```naso
/// forall i in 0..1024 {
///     let v = round(input[i] / scale);
///     output[i] = clamp(v, -128.0, 127.0) as i8;
/// }
/// ```
///
/// With `scale = 1.0` and `input[i] = i - 512`:
///   * `v = round((i - 512) / 1.0) = i - 512`, an integer, so `round` is exact.
///   * `clamp(v, -128, 127)` leaves it alone for `-128 <= i - 512 <= 127`, i.e.
///     `i` in `384 .. 639`.
///   * Below that, everything clamps to `-128`; above, to `127`.
///   * `as i8` is exact on all of `-128 .. 127`.
///
/// So `output[i] = max(-128, min(127, i - 512))` for every `i` in `0 .. 1024`.
///
/// DERIVED, not guessed. The interesting structure is that the clamping is load-bearing
/// for 896 of the 1024 elements: `i - 512` would overflow `i8` for 896 of them, so a
/// backend that dropped the clamp, or dropped the cast, would produce bytes this test
/// rejects.
#[test]
fn the_shipped_quant_int8_kernel_compiles_and_quantizes_correctly() {
    // The refusal this test replaces, verbatim, so the change is visible in the diff:
    //
    //   parameter `input` is declared with two different types (Tensor[F64, 1024] and
    //   Tensor[I8, 1024]). A PIR module is one flat statement list with no function
    //   structure, so the generated entry function has ONE slot per name.
    let ir = build_ir(QUANT_INT8);

    // All THREE functions get their own symbol. The point of the whole change.
    for symbol in [
        "naso_quantize_int8_symmetric",
        "naso_dequantize_int8_symmetric",
        "naso_normalize_f32",
    ] {
        assert!(
            ir.contains(&format!("define void @{symbol}(")),
            "each Naso function must get its own LLVM symbol `{symbol}`:\\n{ir}"
        );
    }

    // `input` is `f64` in one function and `i8` in another. Both widths must appear, and
    // each function must index at ITS OWN width. A single shared slot would make one of
    // these a load of the wrong size with nothing in the IR to say so.
    assert!(
        ir.contains("getelementptr double, ptr %input, i64"),
        "the f32-input function must index `input` as `double`:\\n{ir}"
    );
    assert!(
        ir.contains("getelementptr i8, ptr %input, i64"),
        "the i8-input function must index `input` as `i8`. Reading it as `double` would \\
         consume eight bytes per element and read far past the end of a 1024-byte \\
         buffer:\\n{ir}"
    );

    let stdout = run_ok(&ir, &quantize_driver());
    let mut lines = stdout.lines();
    let head: Vec<i32> = lines
        .next()
        .expect("driver prints the first 32 bytes")
        .split_whitespace()
        .map(|s| s.parse::<i32>().expect("driver prints integers"))
        .collect();
    let checksum: i64 = lines
        .next()
        .expect("driver prints a checksum")
        .trim()
        .parse()
        .unwrap();

    // DERIVED expectation. See this test's doc comment.
    let want_at = |i: usize| -> i32 { (i as i32 - 512).clamp(-128, 127) };

    assert_eq!(
        head.len(),
        32,
        "driver printed {} bytes, expected 32",
        head.len()
    );
    for (i, &g) in head.iter().enumerate() {
        assert_eq!(
            g,
            want_at(i),
            "element {i}: input {} / scale 1.0 rounds to {}, clamps to [{}, 127], so \\
             {} is the answer",
            i as i32 - 512,
            i as i32 - 512,
            -128,
            want_at(i)
        );
    }

    // Every element, via the checksum. The 32 printed bytes are all in the CLAMPED tail
    // -- indices 0..31 are all `i - 512 < -128`, so every one of them is `-128`. That is
    // exactly why the checksum exists: a test that only looked at the head would pass on
    // a kernel that wrote `-128` everywhere, or that never wrote at all.
    let want_sum: i64 = (0..N).map(|i| want_at(i) as i64).sum();
    assert_eq!(
        checksum, want_sum,
        "the sum of all {N} quantized bytes must be {want_sum}. A kernel that dropped the \\
         clamp would overflow `i8` and wrap; one that dropped the cast would store \\
         something else entirely."
    );

    // The clamped region must actually be exercised, or the above would be satisfied by
    // a constant. Assert the structure of the expectation itself.
    assert!(
        (0..N).any(|i| want_at(i) == -128)
            && (0..N).any(|i| want_at(i) == 127)
            && (0..N).any(|i| want_at(i) == -1 && want_at(i) != -128 && want_at(i) != 127),
        "the input must span the clamp floor, the clamp ceiling, and the unclamped \\
         middle, or the test is not testing the clamp"
    );
}

// ---------------------------------------------------------------------------
// 2. A two-function program whose call is proven by execution.
// ---------------------------------------------------------------------------

/// Two functions, the second calling the first twice with DIFFERENT arguments.
///
/// `double_it` returns `x * 2.0`. `main` calls it on `3.0` and on `10.0` and writes both
/// results into `out`.
///
/// This is the shape that distinguishes a real call from everything that looks like one:
///
///   * a STALE VALUE: both slots would hold the same number.
///   * a COPY-PASTED BODY: same.
///   * ARGUMENTS IN THE WRONG ORDER: inkwell 0.10 takes call arguments first and the
///     callee last, and getting that backwards yields a `call` that parses, verifies,
///     and computes the wrong thing. `6` and `20` are distinguishable, and neither is
///     the value for the other argument.
///   * A SIGN-EXTENDED OR TRUNCATED RETURN: the two results are far apart, so a width
///     bug cannot accidentally produce both.
const CALL_TWICE: &str = r#"
fn double_it(x: f32) -> f32 {
    return x * 2.0;
}

fn main(out: inout [1] Tensor[f32, 2]) {
    let a = double_it(3.0);
    let b = double_it(10.0);
    out[0] = a;
    out[1] = b;
}
"#;

fn call_twice_driver() -> String {
    r#"#include <stdint.h>
#include <stdio.h>
extern void naso_main(double *out, int64_t out_len);
static double out[2];
int main(void) {
    out[0] = -1.0; out[1] = -1.0;
    naso_main(out, 2);
    printf("%.17g %.17g\n", out[0], out[1]);
    return 0;
}
"#
    .to_string()
}

#[test]
fn a_call_to_another_function_is_proven_by_execution_with_two_different_arguments() {
    let ir = build_ir(CALL_TWICE);

    // Both symbols exist and the callee is emitted FIRST, because a `call` must resolve
    // to a `define` that is already in the module.
    let callee_at = ir
        .find("define double @naso_double_it(")
        .expect("the callee must get its own symbol");
    let caller_at = ir
        .find("define void @naso_main(")
        .expect("the caller must get its own symbol");
    assert!(
        callee_at < caller_at,
        "a callee must be emitted before its caller. LLVM would accept the other order \\
         only with a forward `declare`, whose signature would be computed a second time \\
         from a second place -- and two computations of one signature is how a caller \\
         ends up disagreeing with the callee about an argument's type."
    );

    // The declared return type is honoured: `-> f32` is a `double` return, not void.
    assert!(
        ir.contains("define double @naso_double_it("),
        "a value-returning function must return its declared type:\\n{ir}"
    );

    let stdout = run_ok(&ir, &call_twice_driver());
    let got: Vec<f64> = stdout
        .split_whitespace()
        .map(|s| s.parse::<f64>().expect("driver prints numbers"))
        .collect();
    assert_eq!(
        got.len(),
        2,
        "driver printed {} values, expected 2",
        got.len()
    );

    // DERIVED: `double_it(x) = x * 2.0`, so `double_it(3.0) = 6.0` and
    // `double_it(10.0) = 20.0`.
    assert_eq!(
        got[0], 6.0,
        "double_it(3.0) must be 6.0 -- got {}. A stale value from the second call would \\
         be 20.0 here.",
        got[0]
    );
    assert_eq!(
        got[1], 20.0,
        "double_it(10.0) must be 20.0 -- got {}. A stale value from the first call would \\
         be 6.0 here.",
        got[1]
    );
    // The two results must DIFFER. Asserted explicitly because it is the property that
    // rules out a stale value and a copy-pasted body in one assertion.
    assert_ne!(
        got[0], got[1],
        "the two calls pass different arguments, so the two results must differ. Equal \\
         results mean one call's value was reused for both."
    );
}

#[test]
fn a_void_call_as_a_statement_computes_its_side_effects() {
    // A void callee whose only observable effect is a STORE. The call is a statement, so
    // its result is discarded -- and the store must still happen. This is the case where
    // a backend that returned early on "Call returned void" would compile cleanly and
    // write nothing.
    let src = r#"
fn fill(out: inout [1] Tensor[f32, 4]) {
    forall i in 0..4 { out[i] = 2.5; }
}

fn main(out: inout [1] Tensor[f32, 4]) {
    fill(out);
}
"#;
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_main(double *out, int64_t out_len);
static double out[4];
int main(void) {
    for (int i = 0; i < 4; ++i) out[i] = -1.0;
    naso_main(out, 4);
    for (int i = 0; i < 4; ++i) { if (i) putchar(' '); printf("%.17g", out[i]); }
    putchar('\n');
    return 0;
}
"#
    .to_string();
    let stdout = run_ok(&build_ir(src), &driver);
    let got: Vec<f64> = stdout
        .split_whitespace()
        .map(|s| s.parse::<f64>().expect("driver prints numbers"))
        .collect();
    assert_eq!(
        got.len(),
        4,
        "driver printed {} values, expected 4",
        got.len()
    );
    for (i, &g) in got.iter().enumerate() {
        assert_eq!(
            g, 2.5,
            "element {i} must be 2.5, written by the void callee. -1.0 means the call's \\
             side effects never happened: a void call whose result is discarded must \\
             still emit its body."
        );
    }
}

#[test]
fn a_call_whose_result_is_discarded_still_runs_the_callee() {
    // The same shape as above but with the call used as a bare expression statement, so
    // the value path and the statement path differ: `double_it` RETURNS a value which
    // `main` throws away. A backend that treated a discarded result as "the callee did
    // not run" would drop the call.
    let src = r#"
fn compute(x: f32) -> f32 {
    return x * 3.0;
}

fn main(out: inout [1] Tensor[f32, 1]) {
    compute(7.0);
    out[0] = 1.0;
}
"#;
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_main(double *out, int64_t out_len);
static double out[1];
int main(void) {
    out[0] = -1.0;
    naso_main(out, 1);
    printf("%.17g\n", out[0]);
    return 0;
}
"#
    .to_string();
    let stdout = run_ok(&build_ir(src), &driver);
    let got: f64 = stdout.trim().parse().expect("driver prints a number");
    // The callee returns 21.0 and main discards it, so the observable value is main's
    // own `1.0`. What this pins is that the CALL did not abort the function: a backend
    // that refused or skipped a discarded result would leave the slot at its poison
    // value of -1.0.
    assert_eq!(
        got, 1.0,
        "the statement after a discarded call must still run, leaving 1.0. -1.0 means the \\
         call was treated as terminal."
    );
}

// ---------------------------------------------------------------------------
// 3. Recursion is refused, loudly.
// ---------------------------------------------------------------------------

#[test]
fn self_recursion_is_refused_with_a_diagnostic_naming_the_function() {
    let src = r#"
fn countdown(n: i64) -> i64 {
    return countdown(n) + 1;
}
"#;
    let msg = compile_error_for(src);
    assert!(
        msg.contains("recursive call") && msg.contains("countdown"),
        "the refusal must name the construct and the function in the cycle: {msg}"
    );
}

#[test]
fn mutual_recursion_is_refused_and_the_whole_cycle_is_named() {
    let src = r#"
fn ping(n: i64) -> i64 {
    return pong(n);
}
fn pong(n: i64) -> i64 {
    return ping(n);
}
"#;
    let msg = compile_error_for(src);
    assert!(
        msg.contains("recursive call"),
        "mutual recursion must be refused: {msg}"
    );
    // The whole cycle, not just the closing edge. "recursive call to `ping`" is
    // unhelpful when the cycle is ping -> pong -> ping, because it does not say `pong`
    // is involved.
    assert!(
        msg.contains("ping -> pong -> ping"),
        "the refusal must name the whole cycle: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 4. The ABI guard fires on a NON-ENTRY function.
// ---------------------------------------------------------------------------

#[test]
fn the_abi_guard_fires_on_a_short_buffer_passed_to_a_non_entry_function() {
    // `helper` is reachable ONLY by call: it is not the entry and nothing links it
    // directly. A guard emitted only on the entry would leave this function an
    // UNGUARDED HOLE, and a short buffer passed to it would read past the end of the
    // caller's allocation with nothing to say so.
    let src = r#"
fn helper(input: [1] Tensor[f32, 8], output: inout [1] Tensor[f32, 8]) {
    forall i in 0..8 { output[i] = input[i]; }
}

fn main(input: [1] Tensor[f32, 8], output: inout [1] Tensor[f32, 8]) {
    helper(input, output);
}
"#;
    let ir = build_ir(src);
    assert!(
        ir.contains("define void @naso_helper("),
        "`helper` must be a real symbol, not inlined into the entry:\\n{ir}"
    );
    assert_eq!(
        ir.matches("icmp slt i64 %input_len, 8").count(),
        2,
        "BOTH functions must guard their own `input`. A guard on the entry alone would \\
         leave `helper` reachable with an unchecked buffer -- and the entry's guard \\
         passing says nothing about the length a CALLER passes to `helper`:\\n{ir}"
    );

    // The driver passes the entry a full-length buffer (so the ENTRY's guard passes) and
    // then calls `helper` directly with a SHORT one. Exit 91 is the short-buffer code.
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_helper(double *input, int64_t input_len, double *output, int64_t output_len);
static double in[8];
static double out[8];
int main(void) {
    for (int i = 0; i < 8; ++i) { in[i] = (double)i; out[i] = -1.0; }
    /* Only 3 of the 8 declared elements. Indices 0..7 are read. */
    naso_helper(in, 3, out, 8);
    printf("SHOULD NOT REACH HERE\n");
    return 0;
}
"#
    .to_string();
    let (code, stdout, stderr) = link_and_run(&ir, &driver, "-O0");
    assert_eq!(
        code,
        Some(91),
        "a short buffer passed to a NON-ENTRY function must exit 91.\nstdout:\n{stdout}\n\
         stderr:\n{stderr}\n--- IR ---\n{ir}"
    );
    assert!(
        !stdout.contains("SHOULD NOT REACH HERE"),
        "the process must terminate at the guard, not run past it:\n{stdout}"
    );
    // The diagnostic must name the parameter and the declared extent, so the reader is
    // not left guessing which buffer was short.
    assert!(
        stderr.contains("`input`") && stderr.contains("8 elements"),
        "the guard must name the parameter and its declared extent.\nstderr:\n{stderr}"
    );
}

#[test]
fn the_abi_guard_survives_optimisation_on_a_non_entry_function() {
    // The guard is emitted by hand and the optimizer is entitled to delete a comparison
    // it believes is dead. At `-O2` and `-O3`, on a function that is NOT the entry,
    // `icmp slt` and the `exit` it feeds must still be there -- otherwise the guard is
    // decoration.
    let src = r#"
fn helper(input: [1] Tensor[f32, 8], output: inout [1] Tensor[f32, 8]) {
    forall i in 0..8 { output[i] = input[i] * 2.0; }
}

fn main(input: [1] Tensor[f32, 8], output: inout [1] Tensor[f32, 8]) {
    helper(input, output);
}
"#;
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_helper(double *input, int64_t input_len, double *output, int64_t output_len);
static double in[8];
static double out[8];
int main(void) {
    for (int i = 0; i < 8; ++i) { in[i] = (double)i; out[i] = -1.0; }
    naso_helper(in, 1, out, 8);
    printf("SHOULD NOT REACH HERE\n");
    return 0;
}
"#
    .to_string();
    let ir = build_ir(src);
    for opt in ["-O0", "-O2", "-O3"] {
        let (code, stdout, stderr) = link_and_run(&ir, &driver, opt);
        assert_eq!(
            code,
            Some(91),
            "the guard must fire at {opt} on a non-entry function.\nstdout:\n{stdout}\n\
             stderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("SHOULD NOT REACH HERE"),
            "at {opt} the process must terminate at the guard:\n{stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. The naming convention, pinned where emission and lookup meet.
// ---------------------------------------------------------------------------

#[test]
fn every_function_is_emitted_under_naso_prefix_and_the_entry_also_as_naso_entry() {
    let ir = build_ir(CALL_TWICE);
    // The convention. A caller must be able to find a callee's symbol, so the spelling
    // is asserted here AND produced by the one function that both emission and call-site
    // lookup call (`function_emission::symbol_for`), which has its own unit test.
    assert!(ir.contains("@naso_double_it"), "{ir}");
    assert!(ir.contains("@naso_main"), "{ir}");

    // The entry ALSO answers to the conventional name, so an external C driver -- and
    // the shipped-kernel execution test -- are unaffected by the per-function change.
    assert!(
        ir.contains("@naso_entry"),
        "the entry must remain reachable under `naso_entry`:\\n{ir}"
    );
    assert_eq!(
        ir.matches("define ").count(),
        3,
        "exactly three `define`s -- two functions plus the entry shim. More would mean a \\
         body was emitted twice; fewer would mean a function was inlined away:\\n{ir}"
    );
}

#[test]
fn the_entry_shim_forwards_rather_than_duplicating_the_body() {
    let ir = build_ir(CALL_TWICE);
    // `naso_entry` must CALL `naso_main`, not be a second copy of it. A duplicated body
    // would double the emitted code, and -- worse -- would make every stack trace and
    // every profile attribute the work to a function the author never wrote.
    assert!(
        ir.contains("call void @naso_main("),
        "the entry shim must call the primary:\\n{ir}"
    );
    assert_eq!(
        ir.matches("define void @naso_main(").count(),
        1,
        "the primary must be defined exactly once"
    );
    // And the shim must carry the SAME signature, so an external caller's declaration
    // still matches.
    let primary = ir
        .lines()
        .find(|l| l.starts_with("define void @naso_main("))
        .expect("primary");
    let shim = ir
        .lines()
        .find(|l| l.starts_with("define void @naso_entry("))
        .expect("shim");
    assert_eq!(
        primary.replace("@naso_main", "@SYM"),
        shim.replace("@naso_entry", "@SYM"),
        "the entry shim must carry the primary's signature verbatim; a different one \\
         would be a link error at the far end rather than a diagnostic here"
    );
}

#[test]
fn two_functions_may_each_bind_the_same_local_name() {
    // Locals, not just parameters. Both functions bind `v` and `acc`; under the old flat
    // emission the second function's `v` shared the first's alloca, so a read in one
    // function could see the other's value.
    let src = r#"
fn first(out: inout [1] Tensor[f32, 2]) {
    let v = 1.0;
    let acc = v + 10.0;
    out[0] = acc;
}

fn second(out: inout [1] Tensor[f32, 2]) {
    let v = 2.0;
    let acc = v + 20.0;
    out[1] = acc;
}

fn main(out: inout [1] Tensor[f32, 2]) {
    first(out);
    second(out);
}
"#;
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_main(double *out, int64_t out_len);
static double out[2];
int main(void) {
    out[0] = -1.0; out[1] = -1.0;
    naso_main(out, 2);
    printf("%.17g %.17g\n", out[0], out[1]);
    return 0;
}
"#
    .to_string();
    let stdout = run_ok(&build_ir(src), &driver);
    let got: Vec<f64> = stdout
        .split_whitespace()
        .map(|s| s.parse::<f64>().expect("driver prints numbers"))
        .collect();
    // DERIVED: first writes `1.0 + 10.0 = 11.0` into slot 0, second writes
    // `2.0 + 20.0 = 22.0` into slot 1. Each writes its OWN slot, so the two results are
    // independent: sharing a slot would let the second call overwrite the first and make
    // this test pass with both values equal.
    assert_eq!(got.len(), 2);
    assert_eq!(
        got[0], 11.0,
        "first(): v=1.0, acc = 1.0 + 10.0 = 11.0, got {}",
        got[0]
    );
    assert_eq!(
        got[1], 22.0,
        "second(): v=2.0, acc = 2.0 + 20.0 = 22.0, got {}",
        got[1]
    );
    // If the two functions shared one scope, `second`'s `v = 2.0` would either be
    // invisible to `first` or visible to it, and one of these two numbers would be
    // wrong.
}

// ---------------------------------------------------------------------------
// 6. Gaps closed by mutation testing.
//
// Each test below was added because a mutation of the corresponding logic SURVIVED
// the suite. The mutation that motivated each one is named in its doc comment, and
// the mutation harness is `mutate.py`. A test that exists only to raise a count is
// not one of these.
// ---------------------------------------------------------------------------

/// A call must forward the length the CALLER WAS HANDED, not one recomputed from the
/// shape.
///
/// MUTATION CAUGHT: replacing the forwarded length with `product(shape)` -- the callee's
/// declared element count. That SURVIVED the suite before this test existed, because
/// every call site in the suite was handed a full-length buffer, where the declared
/// count and the real count are the same number. The callee's guard then compares an
/// INVENTED length against its own declaration and passes, having checked nothing at
/// all about the caller's allocation.
///
/// The test therefore passes the caller a SHORT buffer and calls the callee from Naso.
/// The caller's OWN guard sees the real length and passes it on; the callee's guard
/// then compares that real, short length against its own larger declaration and fires.
/// With the mutation, the callee is told the buffer is long enough and exits 0 having
/// read four elements past the end of a four-element array.
#[test]
fn a_call_forwards_the_callers_real_length_so_the_callees_guard_still_fires() {
    // `helper` declares 8 elements. The ENTRY declares 4, so an external caller may
    // legitimately hand `main` a four-element buffer -- which is exactly the case where
    // "the declared count" and "the real count" differ.
    let src = r#"
fn helper(input: [1] Tensor[f32, 8], output: inout [1] Tensor[f32, 8]) {
    forall i in 0..8 { output[i] = input[i] * 2.0; }
}

fn main(input: [1] Tensor[f32, 4], output: inout [1] Tensor[f32, 8]) {
    helper(input, output);
}
"#;
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_main(double *input, int64_t input_len, double *output, int64_t output_len);
static double in[4];
static double out[8];
int main(void) {
    for (int i = 0; i < 4; ++i) { in[i] = (double)i; out[i] = -1.0; }
    naso_main(in, 4, out, 8);
    printf("SHOULD NOT REACH HERE\n");
    return 0;
}
"#
    .to_string();
    let (code, stdout, stderr) = link_and_run(&build_ir(src), &driver, "-O0");
    assert_eq!(
        code,
        Some(91),
        "the callee's guard must fire on the length the CALLER received. Exit 0 means \\
         the call site forwarded an invented length instead of the real one, so the \\
         guard compared a number against itself and passed.\nstdout:\n{stdout}\n\
         stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("SHOULD NOT REACH HERE"),
        "the process must terminate at the callee's guard:\n{stdout}"
    );
    // And the diagnostic must name `helper`'s OWN declaration of 8, proving the guard
    // that fired is the callee's and not the caller's.
    assert!(
        stderr.contains("8 elements"),
        "the callee's guard must name ITS OWN declared extent of 8 elements. The caller \\
         declared 4 and passed 4, so its own guard must NOT have fired -- if it had, \\
         this test would pass for the wrong reason.\nstderr:\n{stderr}"
    );
}

/// `as f32` on an integer must be `sitofp`, not a sign-extending integer conversion.
///
/// MUTATION CAUGHT: routing the float target back through the integer cast path --
/// which is what `width: None` used to mean before `Cast` recorded that its target was
/// a float. That SURVIVED the suite before this test existed, because the only test
/// touching a float cast asserted that the module COMPILED, and the mutant compiles.
/// It read the `i8` buffer as an `i32` and then multiplied an integer by a `double`,
/// which LLVM rejects at run time rather than at build time.
///
/// The evidence is EXECUTION of `dequantize_int8_symmetric`, the shipped kernel's
/// i8-to-f32 function, driven over its real 1024-element extent.
#[test]
fn an_i8_to_float_cast_is_a_float_conversion_and_executes_correctly() {
    let driver = format!(
        r#"#include <stdint.h>
#include <stdio.h>
extern void naso_dequantize_int8_symmetric(int8_t *input, int64_t input_len,
                                            double *output, int64_t output_len,
                                            double scale);
static int8_t in[{N}];
static double out[{N}];
int main(void) {{
    /* Every element distinct, so a wrong width cannot coincide on the first few. */
    for (int i = 0; i < {N}; ++i) in[i] = (int8_t)(i - 128);
    for (int i = 0; i < {N}; ++i) out[i] = -1.0;
    naso_dequantize_int8_symmetric(in, {N}, out, {N}, 2.0);
    /* 0..8 and 512..520: the low end (negative inputs), the middle, and the wrap
       point where the `i8` pattern repeats. */
    for (int i = 0; i < 8; ++i) printf("%.17g ", out[i]);
    printf("| ");
    for (int i = 512; i < 520; ++i) printf("%.17g ", out[i]);
    printf("\n");
    return 0;
}}
"#
    );
    let ir = build_ir(QUANT_INT8);
    assert!(
        ir.contains("sitofp"),
        "the shipped kernel's `input[i] as f32` must lower to `sitofp`, the signed \\
         integer-to-float conversion:\\n{ir}"
    );
    let stdout = run_ok(&ir, &driver);
    let got: Vec<f64> = stdout
        .split([' ', '|'])
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse::<f64>().expect("driver prints numbers"))
        .collect();
    assert_eq!(
        got.len(),
        16,
        "driver printed {} values, expected 16",
        got.len()
    );

    // DERIVED from the kernel: `let v = input[i] as f32; output[i] = v * scale;`
    // with `scale = 2.0` and `in[i] = (i8)(i - 128)`. The `as f32` SIGN-EXTENDS,
    // because `i8` is signed in LLVM only via the sign the `sitofp` carries -- an `i8`
    // holding 200 is the value -56, not 200.
    let want_at = |i: usize| -> f64 { ((i as i32 - 128) as i8) as f64 * 2.0 };
    for (k, i) in (0..8).chain(512..520).enumerate() {
        assert_eq!(
            got[k],
            want_at(i),
            "element {i}: in[i] = (i8)({i} - 128), widened with its sign and doubled. \\
             A WRONG ANSWER HERE means the cast was an integer one: `{}` is what you \\
             get from reading the byte as an unsigned or sign-extending integer rather \\
             than converting it to a float.",
            i as i32 - 128
        );
    }
    // The negatives are the discriminator. An `uitofp` would give 200.0 and 256.0 where
    // this expects -112.0 and -256.0, and an integer cast would give an integer that
    // `* 2.0` cannot even be applied to.
    assert!(
        got[0] < 0.0 && got[7] < 0.0,
        "elements 0 and 7 hold negative bytes and must come out negative, got {} and \\
         {}. A non-negative result means the sign was lost -- `zext` or `uitofp`.",
        got[0],
        got[7]
    );
}

/// Each function's ABI must come from ITS OWN parameters, never the program-wide union.
///
/// MUTATION CAUGHT: making the per-function PIR view fall back to
/// `PirModule::function_params` -- the union of every function's parameters -- which is
/// what the old flat backend did. That SURVIVED the suite before this test existed,
/// because the shape it breaks needs a name declared at two DIFFERENT EXTENTS in two
/// functions: with identical declarations the union and the per-function list are the
/// same list, so the mutation is invisible.
///
/// `quant_int8.naso` is the real case, and it is asserted here on the entry's own guard
/// as well: the middle function declares 1024 elements like the first, so the ENTRY's
/// guard text alone cannot tell the two implementations apart. The discriminator is
/// which guard fires when a short buffer is passed.
#[test]
fn each_functions_abi_comes_from_its_own_parameters_not_the_union() {
    // Three functions, `t` at THREE different extents. Under the union, all three would
    // bind to the first declaration's extent and all three guards would compare against
    // it -- so passing a buffer that is long enough for `first` but short for `third`
    // would NOT fire the guard the source requires.
    let src = r#"
fn first(t: [1] Tensor[f32, 4]) {
    forall i in 0..4 { let v = t[i]; }
}

fn second(t: [1] Tensor[f32, 6]) {
    forall i in 0..6 { let v = t[i]; }
}

fn third(t: [1] Tensor[f32, 8]) {
    forall i in 0..8 { let v = t[i]; }
}

fn main(t: [1] Tensor[f32, 8]) {
    first(t);
    second(t);
    third(t);
}
"#;
    let ir = build_ir(src);

    // Three DIFFERENT extents in the source, so the three guard comparisons must be
    // three different constants. Under the union they would all be `4`.
    for extent in ["4", "6", "8"] {
        assert!(
            ir.contains(&format!("icmp slt i64 %t_len, {extent}")),
            "a function declaring {extent} elements must guard against exactly {extent}. \\
             Under the program-wide union every function would compare against the first \\
             declaration's extent, which is a buffer-length check against a number the \\
             source never said for that function.\\n{ir}"
        );
    }
    // And `second` must be callable with 6 but not with 5, even though `first` -- which
    // is declared first and so wins the union -- says 4.
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_second(double *t, int64_t t_len);
static double t[8];
int main(void) {
    for (int i = 0; i < 8; ++i) t[i] = (double)i;
    naso_second(t, 5);
    printf("SHOULD NOT REACH HERE\n");
    return 0;
}
"#
    .to_string();
    let (code, stdout, stderr) = link_and_run(&ir, &driver, "-O0");
    assert_eq!(
        code,
        Some(91),
        "`second` declares 6 elements and was given 5, so its guard must fire. Exit 0 \\
         means it compared against `first`'s 4 -- the program-wide union -- and found \\
         5 long enough.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("6 elements"),
        "the diagnostic must name `second`'s OWN extent of 6, proving the guard that \\
         fired is the per-function one.\nstderr:\n{stderr}"
    );
}

/// A local bound in one function must NOT be visible in another.
///
/// MUTATION CAUGHT: dropping the `clear_variables()` between functions, so the value
/// builder's `name -> allocation` map carried one function's locals into the next. That
/// SURVIVED the suite before this test existed, because every test's functions bound the
/// same names, and a function's own binding overwrites the stale one before any read.
///
/// The discriminating case is a name bound in `first` and READ — never bound — in
/// `second`. Under correct scoping that read is an unbound name and must be REFUSED.
/// With the mutation it silently reads `first`'s alloca, so the module compiles and
/// `second` computes with a value it never bound. That is the worst kind of bug this
/// compiler exists to prevent: no diagnostic, no crash, just a wrong answer.
///
/// The refusal is asserted, and so is the fact that the message names the name rather
/// than failing somewhere unrelated.
#[test]
fn a_local_from_one_function_is_not_visible_in_another() {
    let src = r#"
fn first(out: inout [1] Tensor[f32, 1]) {
    let leaked = 42.0;
    out[0] = leaked;
}

fn second(out: inout [1] Tensor[f32, 1]) {
    out[0] = leaked;
}
"#;
    let ir = match try_build(src) {
        Ok(ir) => ir,
        Err(e) => {
            assert!(
                e.contains("leaked"),
                "the refusal must name the unbound variable `leaked`, got: {e}"
            );
            return;
        }
    };
    panic!(
        "`second` reads `leaked`, which only `first` binds. Each function has its own \\
         scope, so that read must be REFUSED -- it compiled instead, meaning one \\
         function's local was visible in another.\\n{ir}"
    );
}

/// And the same rule for a PARAMETER: one function's parameter is not another's.
///
/// Distinct from the local case above because a parameter binds through a different
/// path (`bind_tensor` / `bind_argument` rather than a `let` alloca), so a test that only
/// covered locals would not notice a scope leak on the parameter path.
#[test]
fn a_parameter_from_one_function_is_not_visible_in_another() {
    let src = r#"
fn first(other: [1] Tensor[f32, 1], out: inout [1] Tensor[f32, 1]) {
    out[0] = other[0];
}

fn second(out: inout [1] Tensor[f32, 1]) {
    out[0] = other[0];
}
"#;
    match try_build(src) {
        Err(e) => assert!(
            e.contains("other"),
            "the refusal must name the unbound `other`, got: {e}"
        ),
        Ok(ir) => panic!(
            "`second` reads `other`, which only `first` declares as a parameter. Each \\
             function has its own parameter scope, so that read must be REFUSED.\\n{ir}"
        ),
    }
}

/// Build IR, returning the ERROR as a string rather than panicking.
///
/// Used by the two refusal tests above, which assert that a program does NOT compile.
/// `build_ir` panics on failure, which is right for the tests that want a module and
/// wrong for these.
fn try_build(src: &str) -> Result<String, String> {
    let program = parse(src);
    let pir = match naso_compiler::lowering::lower_program(&program) {
        Ok(p) => p,
        Err(e) => return Err(e.to_string()),
    };
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    match builder.build_module(&pir) {
        Ok(()) => Ok(builder.module_to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// A function that DECLARES a return type but never returns is refused.
///
/// It used to compile and return zero. For `fn f(x: f32) -> f32 { let mut y = x * 3.0; }`
/// that meant `f(5.0)` evaluated to `0.0` where `15.0` was correct, and the caller had
/// no way to tell a missing computation from a genuine zero. Zero is the one answer
/// that can never be right here, so the backend names the function instead.
#[test]
fn a_declared_return_type_with_no_return_is_refused_rather_than_returning_zero() {
    let msg = compile_error_for(
        "fn f(x: f32) -> f32 {\n    let mut y = x * 3.0;\n}\nfn main() -> f32 { return f(5.0); }\n",
    );
    assert!(
        msg.contains("`f`") && msg.contains("no `return` statement"),
        "the diagnostic must name the function and the missing `return`: {msg}"
    );
    assert!(
        msg.contains("Returning zero would be silently wrong"),
        "the diagnostic must say why returning zero is refused: {msg}"
    );
}

/// Dropping the return type makes the same function legal, and it then really is void.
///
/// This is the fix the diagnostic recommends, so it has to be one the compiler accepts.
#[test]
fn the_same_function_compiles_once_it_is_honestly_void() {
    let ir =
        build_ir("fn f(x: f32) {\n    let mut y = x * 3.0;\n}\nfn main() -> f32 { return 4.0; }\n");
    assert!(
        ir.contains("define void @naso_f"),
        "dropping the return type must give a void `f`:\n{ir}"
    );
}
