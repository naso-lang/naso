//! End-to-end: a tensor parameter's DECLARED EXTENT is checked at the ABI boundary.
//!
//! # The gap this pins
//!
//! `commit 04690ae` gave the LLVM backend a real pointer ABI: `Tensor[f32, N]`
//! becomes a `ptr` to the caller's buffer, scalars are passed by value. But a bare
//! `ptr` carries no length, so nothing could be checked. The declared extent `N` was
//! known to the compiler at every tensor parameter and was used for exactly one
//! thing -- rejecting a zero extent -- and never compared against anything the
//! caller supplied, because the caller supplied nothing to compare.
//!
//! Concretely, for
//!
//! ```naso
//! fn k(a: [1] Tensor[f32, 4]) -> f64 {
//!     let mut s = 0.0;
//!     forall i in 0..4 { s = s + a[i]; }
//!     return s;
//! }
//! ```
//!
//! a C driver passing a ONE-element buffer read four elements out of bounds, printed
//! nothing, and exited 0. A kernel that is internally perfectly correct -- the
//! typechecker does verify the `[1]` is read exactly once -- still reads out of
//! bounds when the buffer it was handed is the wrong size. This is the one place a
//! caller can break Naso's central claim, and it was unguarded.
//!
//! # What the ABI is now
//!
//! Each tensor parameter with a known extent takes TWO arguments:
//!
//! ```text
//! ptr %input, i64 %input_len     -- ELEMENTS, not bytes
//! ```
//!
//! and the entry block checks, before anything reads the buffer:
//!
//! - `icmp eq ptr %input, null` -> exit code 90
//! - `icmp slt i64 %input_len, <declared>` -> exit code 91
//!
//! The length is the PRODUCT of the declared extents, so a `Tensor[f32, 2, 3]` is
//! checked against 6 -- the same linearised `i * 3 + j` the subscript computes.
//!
//! # Why this file RUNS the code
//!
//! Same reasoning as `llvm_execution_test`: IR text cannot decide whether a check
//! happened. LLVM folds and reorders; a deleted branch and a surviving one can print
//! similarly in a summarised module. The only assertion a "the guard is gone and the
//! kernel silently reads out of bounds" arm cannot pass is to hand the program a
//! short buffer and observe the PROCESS EXIT STATUS.
//!
//! Every case gets its own temp directory, unique per CALL: the harness runs tests as
//! threads of one process, and a shared directory makes one case overwrite another's
//! binary.
//!
//! # What is deliberately NOT claimed
//!
//! These tests establish the EXTENT check. They do not establish anything about a
//! `[1]` resource's LIFETIME -- see the module doc of `codegen::llvm::abi_guard` for
//! the exact list of guarantees that stop at a raw-pointer ABI, and why `noalias` and
//! `readonly` are refused rather than emitted. `an_in_place_caller_may_pass_the_same
//! _buffer_twice` is the executable half of that argument: it pins the aliasing case
//! that both attributes would turn into undefined behaviour.
//!
//! # Coverage
//!
//! From `.naso` TEXT: `parse_program` -> `lower_program` -> the production
//! `LLVMModuleBuilder::build_module` -> `llc` -> `clang` -> execution.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;
use naso_compiler::codegen::llvm::abi_guard::{NASO_ABI_EXIT_NULL, NASO_ABI_EXIT_SHORT};

/// LLVM toolchain prefix; falls back to bare names when the tools are only on `PATH`.
fn tool(name: &str) -> String {
    match std::env::var("LLVM_SYS_170_PREFIX") {
        Ok(prefix) if !prefix.is_empty() => format!("{prefix}/bin/{name}"),
        _ => name.to_string(),
    }
}

/// A temp directory for ONE case, removed on drop.
///
/// Unique per call, not per process: the harness runs tests as threads of one
/// process, so a shared directory makes cases overwrite each other's binaries.
struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-abiguard-{}-{n}", std::process::id()));
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

/// Run `cmd`, panicking with `what` and the IR when it fails.
///
/// Used only for the TOOLCHAIN steps (`llc`, `clang`). The generated program itself is
/// run through [`run_expecting_exit`], because a non-zero exit there is the result
/// under test rather than a failure of the harness.
fn run_tool(cmd: &mut Command, what: &str, ir: &str) -> Vec<u8> {
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

/// Compile `ir` at `opt` (`-O0` or `-O2`), link `driver`, and run it.
///
/// Returns the exit status code and the merged output. The exit status is the
/// primary signal: a message can be swallowed, an exit code cannot, and a check whose
/// only failure path printed something and returned 0 would be indistinguishable from
/// no check at all.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn compile_link_run(ir: &str, driver: &str, opt: &str) -> Run {
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("driver.c");
    let exe = dir.path("case");

    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&csrc, driver).expect("write C driver");

    // `llc` compiles the generated module on its own. The driver is then linked
    // against the object WITHOUT letting clang optimise the generated code, so the
    // `-O2` in this test means "the LLVM backend was asked for -O2", not "clang
    // inlined the kernel into a caller that could then fold the check away".
    run_tool(
        // `-relocation-model=pic` is REQUIRED, not an optimisation. The ABI guard
        // puts C string literals in the module (the violation messages), and LLVM's
        // default STATIC relocation model refers to them by absolute address, which
        // cannot be linked into the position-independent executable clang produces by
        // default: `relocation R_X86_64_32 against '.rodata.str1.1' can not be used
        // when making a PIE object`.
        // `llc` rather than a compiler-side object writer, so the -O level under test
        // is the one the generated module is actually optimised at.
        Command::new(tool("llc"))
            .arg(format!("-{opt}"))
            .arg("-relocation-model=pic")
            .arg("-filetype=obj")
            .arg(&ll)
            .arg("-o")
            .arg(&obj),
        "llc",
        ir,
    );
    run_tool(
        Command::new(tool("clang"))
            .arg("-O0")
            .arg("-Wno-override-module")
            .arg(&ll)
            .arg(&csrc)
            .arg("-o")
            .arg(&exe),
        "clang",
        ir,
    );
    let out = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("could not run the compiled program: {e}\n--- IR ---\n{ir}"));
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// Lower `src` through the production path and return the printed module.
fn build_ir(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\n{src}\nerror: {e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("source must compile:\n{src}\nerror: {e}"));
    builder.module_to_string()
}

/// A four-element kernel: `s = a[0] + a[1] + a[2] + a[3]`, read back through a global.
///
/// The accumulation is into a global rather than a return value because `naso_entry`
/// returns void -- PIR has no return node.
const SUM4: &str = r#"
fn sum4(a: [1] Tensor[f32, 4], out: inout [1] Tensor[f32, 1]) {
    forall i in 0..4 {
        out[0] = out[0] + a[i];
    }
}
"#;

/// A driver that calls `naso_entry(small, 1, out, 1)` -- a ONE-element buffer for a
/// four-element parameter.
///
/// `small` is a genuine one-element array, so if the guard did nothing the kernel
/// would read three doubles past its end. That is undefined behaviour, not a
/// guaranteed crash, which is exactly why the observable signal has to be the exit
/// status rather than "it did not segfault".
fn driver_short_buffer() -> String {
    String::from(
        r#"#include <stdint.h>
extern void naso_entry(double *a, int64_t a_len, double *out, int64_t out_len);
int main(void) {
    double small[1] = { 1.0 };
    double out[1] = { 0.0 };
    naso_entry(small, 1, out, 1);
    return 0;
}
"#,
    )
}

/// The same driver, but passing NULL for `a` with a length that would otherwise pass.
///
/// `a_len` is 4 -- correct for the declared extent -- so the ONLY thing wrong is the
/// pointer. That distinguishes this case from the short-buffer one: if the null check
/// were missing and only the length check existed, this would run.
fn driver_null_buffer() -> String {
    String::from(
        r#"#include <stdint.h>
extern void naso_entry(double *a, int64_t a_len, double *out, int64_t out_len);
int main(void) {
    double out[1] = { 0.0 };
    naso_entry((double *)0, 4, out, 1);
    return 0;
}
"#,
    )
}

/// A correct driver: two four-element buffers and correct lengths.
///
/// `out` starts at zero and `a` holds 1..4, so a run that gets all the way through
/// leaves `result == 10`. Printing it is what distinguishes "the guard let a correct
/// call through AND the arithmetic ran" from "the guard let a correct call through and
/// the body did nothing".
fn driver_correct() -> String {
    String::from(
        r#"#include <stdint.h>
#include <stdio.h>
extern void naso_entry(double *a, int64_t a_len, double *out, int64_t out_len);
int main(void) {
    double a[4] = { 1.0, 2.0, 3.0, 4.0 };
    double out[1] = { 0.0 };
    naso_entry(a, 4, out, 1);
    printf("%.17g\n", out[0]);
    return 0;
}
"#,
    )
}

#[test]
fn a_short_buffer_fails_the_call_with_a_nonzero_exit_status() {
    // The regression this whole file exists for. Before the guard, this program read
    // three doubles past the end of a one-element array, printed nothing and exited 0.
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, &driver_short_buffer(), "O0");
    assert_eq!(
        run.code,
        Some(NASO_ABI_EXIT_SHORT as i32),
        "a one-element buffer for a `Tensor[f32, 4]` parameter must fail with exit \
         code {NASO_ABI_EXIT_SHORT}, got {:?}.\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
}

#[test]
fn a_null_buffer_fails_the_call_with_its_own_nonzero_exit_status() {
    // Null is a DIFFERENT mistake from short, and gets a different code. If it got the
    // short code instead, a caller could not tell "you passed no pointer" from "you
    // passed too few elements" without reading the message.
    //
    // `a_len` is 4 here, so the length check passes and the null check is the only
    // thing standing between this driver and an out-of-bounds read. That is what
    // makes this a test of the null check specifically.
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, &driver_null_buffer(), "O0");
    assert_eq!(
        run.code,
        Some(NASO_ABI_EXIT_NULL as i32),
        "a NULL buffer must fail with exit code {NASO_ABI_EXIT_NULL} even when the \
         declared length is correct, got {:?}.\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
    assert_ne!(
        NASO_ABI_EXIT_NULL, NASO_ABI_EXIT_SHORT,
        "the two failure codes must stay distinguishable"
    );
}

#[test]
fn a_correct_buffer_computes_and_exits_zero() {
    // The guard must not be a guard that rejects everything. `a_len == 4` and
    // `out_len == 1` both meet their declared extents, so the call proceeds and
    // `result` is 1 + 2 + 3 + 4.
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, &driver_correct(), "O0");
    assert_eq!(
        run.code,
        Some(0),
        "a correct call must exit 0, got {:?}\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
    assert_eq!(
        run.stdout.trim(),
        "10",
        "the kernel must have summed 1+2+3+4 into `result`.\n--- IR ---\n{ir}"
    );
}

#[test]
fn a_buffer_larger_than_the_declared_extent_is_accepted() {
    // The negative direction, and the one that gives a WRONG ANSWER rather than an
    // error if the comparison is written wrongly.
    //
    // A caller whose buffer is bigger than the declared extent is running a correct
    // program: it is passing a 4-element VIEW of the first four elements of a larger
    // array. The property the guard establishes is "every index the body can form is
    // inside the buffer", which is `len >= declared`. A guard written as `len !=
    // declared` would refuse this correct call -- a false alarm that trains callers to
    // ignore the guard, which is worse than no guard.
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_entry(double *a, int64_t a_len, double *out, int64_t out_len);
int main(void) {
    double big[8] = { 1.0, 2.0, 3.0, 4.0, 99.0, 99.0, 99.0, 99.0 };
    double out[1] = { 0.0 };
    naso_entry(big, 8, out, 1);
    printf("%.17g\n", out[0]);
    return 0;
}
"#;
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, driver, "O0");
    assert_eq!(
        run.code,
        Some(0),
        "an 8-element buffer for a 4-element parameter is memory-safe and must be \
         accepted, got {:?}\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
    // The `99.0` tail is the sharper half: if the kernel read past the declared
    // extent, `result` would be 1+2+3+4+99+99+99+99, not 10.
    assert_eq!(
        run.stdout.trim(),
        "10",
        "the kernel must read only the 4 elements its declared extent names.\n--- IR ---\n{ir}"
    );
}

#[test]
fn a_negative_length_is_rejected_rather_than_read_as_the_largest_buffer() {
    // The case that a naive unsigned comparison loses. `i64 -1` is `u64::MAX`, so an
    // `icmp ult` guard waves it through as "an enormous buffer" and the kernel reads
    // out of bounds on a caller that supplied NO valid length at all.
    let driver = r#"#include <stdint.h>
extern void naso_entry(double *a, int64_t a_len, double *out, int64_t out_len);
int main(void) {
    double a[4] = { 1.0, 2.0, 3.0, 4.0 };
    double out[1] = { 0.0 };
    naso_entry(a, -1, out, 1);
    return 0;
}
"#;
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, driver, "O0");
    assert_eq!(
        run.code,
        Some(NASO_ABI_EXIT_SHORT as i32),
        "a negative length is not a buffer of any size and must be rejected, got {:?}.\n\
         stderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
}

#[test]
fn a_two_dimensional_tensor_is_checked_against_the_product_of_its_extents() {
    // The check is against the ELEMENT COUNT of the declared shape, not the first
    // extent. `Tensor[f32, 2, 3]` is six elements, and `a[i][j]` linearises to
    // `i * 3 + j` over exactly those six. A guard comparing against `2` -- the first
    // extent -- would accept a two-element buffer for a kernel that reads six.
    let src = r#"
fn sum2d(a: [1] Tensor[f32, 2, 3], out: inout [1] Tensor[f32, 1]) {
    forall i in 0..2 {
        forall j in 0..3 {
            out[0] = out[0] + a[i][j];
        }
    }
}
"#;
    let driver = r#"#include <stdint.h>
extern void naso_entry(double *a, int64_t a_len, double *out, int64_t out_len);
int main(void) {
    double two[2] = { 1.0, 2.0 };
    double out[1] = { 0.0 };
    naso_entry(two, 2, out, 1);
    return 0;
}
"#;
    let ir = build_ir(src);
    let run = compile_link_run(&ir, driver, "O0");
    assert_eq!(
        run.code,
        Some(NASO_ABI_EXIT_SHORT as i32),
        "a 2-element buffer for `Tensor[f32, 2, 3]` (six elements) must fail, got {:?}.\n\
         stderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
}

#[test]
fn the_guard_survives_optimisation_at_o2() {
    // The optimisation hazard, tested rather than asserted.
    //
    // A guard whose failure edge is a bare `unreachable` may be DELETED: an optimiser
    // that can prove the branch condition constant is entitled to remove the branch,
    // and if the constant is a short length -- exactly what an inlining caller
    // supplies -- folding the check away restores the out-of-bounds read. This test
    // compiles the generated module with `llc -O2` and runs the short-buffer driver.
    //
    // WHAT IS CLAIMED, precisely: at `-O2` on this module, with a caller that passes
    // the short length at run time, the guard still fires. It is not a claim about
    // every optimiser, every target, or a caller that fully inlines and constant-
    // folds -- and it does not rest on the failure path being unremovable by
    // construction, it is checked by execution.
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, &driver_short_buffer(), "O2");
    assert_eq!(
        run.code,
        Some(NASO_ABI_EXIT_SHORT as i32),
        "at -O2 a short buffer must still fail with {NASO_ABI_EXIT_SHORT}, got {:?}.\n\
         If this fails, the check was optimised away and the program silently reads \
         out of bounds.\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
}

#[test]
fn the_guard_does_not_change_the_answer_at_o2_for_a_correct_call() {
    // The companion to the above: the guard must not perturb a correct call at -O2
    // either. Without this, "the guard still fires" could be satisfied by a guard
    // that fires unconditionally.
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, &driver_correct(), "O2");
    assert_eq!(
        run.code,
        Some(0),
        "a correct call must still succeed at -O2, got {:?}\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
    assert_eq!(
        run.stdout.trim(),
        "10",
        "a correct call must still compute 1+2+3+4 at -O2.\n--- IR ---\n{ir}"
    );
}

#[test]
fn the_violation_message_names_the_parameter_and_the_declared_extent() {
    // The exit code is the robust signal, but it says nothing about WHICH parameter
    // was wrong or what was expected. The message is for the human, and it is built at
    // compile time because both the name and the extent are known then.
    let ir = build_ir(SUM4);
    let run = compile_link_run(&ir, &driver_short_buffer(), "O0");
    assert!(
        run.stderr.contains("`a`") && run.stderr.contains("4 elements"),
        "the diagnostic must name the parameter and its declared extent, got: {}",
        run.stderr
    );
}

#[test]
fn a_scalar_only_function_gets_no_length_arguments_and_no_guard() {
    // The ABI change must not touch anything that has no tensor. A scalar-only
    // kernel's signature is `define void @naso_entry(double %a, double %k)` exactly as
    // before, and no `exit` / `fputs` / `stderr` declaration appears -- those would be
    // dead symbols in every scalar module.
    let src = r#"
fn scale_one(a: f32, k: f32) -> f32 {
    return a * k;
}
"#;
    let ir = build_ir(src);

    // TWO symbols, and this is the part the multi-function change introduced. The
    // function itself is `naso_scale_one` -- the `naso_<name>` convention every
    // generated function uses, so a call site resolves every callee by the same rule.
    // `naso_entry` is the conventional ENTRY name, emitted as a thin call into the
    // primary so an external C driver still has a stable symbol to link against.
    //
    // Previously this module had ONE `define`, `naso_entry`, and that is all. So the
    // scalar-only property is now asserted on the primary's signature, and the entry
    // is asserted to be a forwarding shim rather than a second copy of the body: a
    // second copy would double the emitted code for no benefit.
    let primary = ir
        .lines()
        .find(|l| l.starts_with("define double @naso_scale_one("))
        .expect("the function must be emitted under `naso_<name>`");
    assert_eq!(
        primary, "define double @naso_scale_one(double %a, double %k) {",
        "a scalar-only signature must be unchanged by the tensor-length ABI -- no \
         length arguments, because there is no tensor to check:\n{ir}"
    );
    // The declared return type is honoured: `-> f32` is a `double` return, not void.
    // Asserted because a `void` here would be a caller binding a value that does not
    // exist -- a compile that succeeds and computes nothing.
    assert!(
        ir.contains("ret double %fmul"),
        "a value-returning function must return the value it computed, not zero or \
         nothing:\n{ir}"
    );

    let entry_shim = ir
        .lines()
        .find(|l| l.starts_with("define double @naso_entry("))
        .expect("the entry symbol must exist for external callers");
    assert_eq!(
        entry_shim, "define double @naso_entry(double %a, double %k) {",
        "the entry shim must carry the same signature as the function it forwards to:\n{ir}"
    );
    assert!(
        ir.contains("call double @naso_scale_one(double %a, double %k)"),
        "the entry must CALL the function, not duplicate its body. A second copy of \
         the body would double the emitted code and make every backtrace point at a \
         function the author never wrote:\n{ir}"
    );
    assert!(
        !ir.contains("declare void @exit"),
        "a module with no tensor parameter must not declare the violation runtime:\n{ir}"
    );
    assert!(
        !ir.contains("naso.abi"),
        "a module with no tensor parameter must emit no guard blocks:\n{ir}"
    );
}

#[test]
fn a_symbolic_extent_is_still_refused_rather_than_unchecked() {
    // A parameter whose extent is not a literal has no bound to check against. The
    // honest response is to refuse it -- the alternative would be a tensor argument
    // with no length argument at all, i.e. the unguarded ABI this work removed,
    // reintroduced for exactly the programs that cannot be checked.
    let src = r#"
fn bad(a: [1] Tensor[f32, N]) {
    let v = a[0];
}
"#;
    let program = naso_compiler::parser::parse_program(src).expect("source must parse");
    let lowered = naso_compiler::lowering::lower_program(&program);
    // Lowering may refuse it outright, or carry `shape: None` for a backend to
    // refuse. Either is correct; what is NOT correct is it compiling to an unguarded
    // pointer.
    if let Ok(pir) = lowered {
        let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
        let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
        match builder.build_module(&pir) {
            Ok(()) => panic!(
                "a symbolic extent must be refused, but it compiled to:\n{}",
                builder.module_to_string()
            ),
            Err(e) => assert!(
                e.to_string().contains("symbolic"),
                "the refusal must name the symbolic extent as the reason, got: {e}"
            ),
        }
    }
}

#[test]
fn an_in_place_caller_may_pass_the_same_buffer_twice() {
    // The executable half of the argument against `noalias` and `readonly`.
    //
    // Passing one buffer as both the read-only `[1]` input and the `inout` output is
    // an ordinary, correct, memory-safe program -- in-place elementwise scaling. Both
    // attributes would be FALSE for it: `noalias` on the input denies any other
    // pointer reaching the memory, and `readonly` denies any write during the call,
    // and the callee writes through the output. Either attribute would make this
    // program undefined behaviour and license the optimiser to change its answer.
    //
    // So this test asserts the answer is CORRECT. If someone adds `noalias` or
    // `readonly` to a tensor argument, this stops being a test of the ABI and becomes
    // a test of undefined behaviour -- which is exactly the signal wanted.
    let src = r#"
fn inplace(output: inout [1] Tensor[f32, 4], scale: f32) {
    forall i in 0..4 {
        output[i] = output[i] * scale;
    }
}
"#;
    // One buffer, passed as itself for `output`, scaled in place.
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_entry(double *output, int64_t output_len, double scale);
int main(void) {
    double buf[4] = { 1.0, 2.0, 3.0, 4.0 };
    naso_entry(buf, 4, 2.0);
    printf("%.17g %.17g %.17g %.17g\n", buf[0], buf[1], buf[2], buf[3]);
    return 0;
}
"#;
    let ir = build_ir(src);
    let run = compile_link_run(&ir, driver, "O2");
    assert_eq!(
        run.code,
        Some(0),
        "an in-place call is a legal program and must exit 0, got {:?}\nstderr: {}\n--- IR ---\n{ir}",
        run.code,
        run.stderr
    );
    assert_eq!(
        run.stdout.trim(),
        "2 4 6 8",
        "in-place scaling must produce 2 4 6 8. If this reads as an aliasing \
         miscompile, a `noalias` or `readonly` attribute has been added to a tensor \
         argument: both are false for a caller that passes one buffer twice.\n--- IR ---\n{ir}"
    );
}
