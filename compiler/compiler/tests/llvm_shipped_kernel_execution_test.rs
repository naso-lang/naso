//! The shipped kernel `kernels/scale_f32.naso`, executed through the real LLVM ABI.
//!
//! The other guard tests use a four-element kernel written inline in the test. This
//! one compiles the repository's OWN kernel -- the file the WGSL validation suite and
//! `naso build` both use -- at its real 1024-element extent, so the new `(ptr, i64
//! len)` ABI is exercised against a shipped artifact rather than a fixture.
//!
//! The skill note applies: a kernel loop bound is not a driver array size. This
//! driver allocates 1024 elements because the kernel's loop runs 1024 times; a
//! smaller driver array would segfault and read like a compiler bug.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

/// The kernel this file compiles, verbatim from the repository.
const KERNEL: &str = include_str!("../../kernels/scale_f32.naso");

/// `Tensor[f32, 1024]` in `kernels/scale_f32.naso`.
const N: usize = 1024;

struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-scale-{}-{n}", std::process::id()));
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

/// The driver for the real kernel: `out[i] = clamp(in[i] * 3, -4, 10)`.
///
/// `input` spans `-512 .. +511`, so the product spans `-1536 .. +1533` and all three
/// clamp regimes are represented by hundreds of elements rather than one. `output` is
/// pre-filled with a poison value so an unwritten slot is visible as a mismatch rather
/// than as a coincidence.
fn driver_c() -> String {
    format!(
        r#"#include <stdint.h>
#include <stdio.h>
extern void naso_entry(double *input, int64_t input_len, double *output, int64_t output_len,
                       double scale, double lo, double hi);
static double in[{N}];
static double out[{N}];
int main(void) {{
    for (int i = 0; i < {N}; ++i) in[i] = (double)i - (double)({N} / 2);
    for (int i = 0; i < {N}; ++i) out[i] = -12345.0;
    naso_entry(in, {N}, out, {N}, 3.0, -4.0, 10.0);
    for (int i = 0; i < {N}; ++i) {{
        if (i) putchar(' ');
        printf("%.17g", out[i]);
    }}
    putchar('\n');
    return 0;
}}
"#
    )
}

#[test]
fn the_shipped_scale_f32_kernel_computes_all_1024_elements_under_the_new_abi() {
    let program =
        naso_compiler::parser::parse_program(KERNEL).expect("the shipped kernel must parse");
    let pir =
        naso_compiler::lowering::lower_program(&program).expect("the shipped kernel must lower");
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("the shipped kernel must compile to LLVM: {e}"));
    let ir = builder.module_to_string();

    // The new ABI is visible here, at the real extent.
    let sig = ir
        .lines()
        .find(|l| l.starts_with("define "))
        .expect("module must define an entry function");
    assert!(
        sig.contains("ptr %input, i64 %input_len") && sig.contains("ptr %output, i64 %output_len"),
        "the shipped kernel must take (ptr, i64 len) per tensor, got: {sig}"
    );
    assert!(
        ir.contains("icmp slt i64 %input_len, 1024"),
        "the guard must compare against the kernel's declared 1024 elements:\n{ir}"
    );

    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("driver.c");
    let exe = dir.path("case");
    std::fs::write(&ll, &ir).expect("write IR");
    std::fs::write(&csrc, driver_c()).expect("write driver");

    // `-relocation-model=pic` is required: the guard's violation strings are absolute
    // under LLVM's default static model and cannot be linked into a PIE.
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
                .arg("-O0")
                .arg("-Wno-override-module")
                .arg(&ll)
                .arg(&csrc)
                .arg("-o")
                .arg(&exe),
        ),
    ] {
        let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(
            out.status.success(),
            "{what} failed ({:?}):\n{}\n--- IR ---\n{ir}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = Command::new(&exe).output().expect("run");
    assert!(
        out.status.success(),
        "the compiled kernel failed ({:?}):\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(|f| f.parse::<f64>().expect("driver prints numbers"))
        .collect();
    assert_eq!(
        got.len(),
        N,
        "driver printed {} values, expected {N}",
        got.len()
    );

    // Every element, not a sample: a zero-filled or partially-written output would
    // agree with the expectation on the elements that happen to be right.
    let mut below = 0;
    let mut inside = 0;
    let mut above = 0;
    let mut mismatches = 0;
    for (i, &g) in got.iter().enumerate() {
        let product = (i as f64 - (N / 2) as f64) * 3.0;
        let want = product.clamp(-4.0, 10.0);
        if g != want {
            mismatches += 1;
            if mismatches <= 5 {
                eprintln!(
                    "element {i}: input {input} * 3.0 = {product}, want {want}, got {g}",
                    input = i as f64 - (N / 2) as f64
                );
            }
        }
        if product < -4.0 {
            below += 1;
        } else if product > 10.0 {
            above += 1;
        } else {
            inside += 1;
        }
    }
    assert_eq!(
        mismatches, 0,
        "{mismatches} of {N} elements disagree with clamp(input[i]*3, -4, 10)"
    );
    // If the input ever stops spanning all three regimes the assertion above would
    // start passing on a kernel that only computes part of the range, so pin it.
    assert!(
        below > 0 && inside > 0 && above > 0,
        "the driver input must span all three clamp regimes, got below={below} \
         inside={inside} above={above}"
    );
    println!(
        "scale_f32.naso: {N} elements, 0 mismatches (below={below} inside={inside} above={above})"
    );
}

#[test]
fn the_shipped_scale_f32_kernel_rejects_a_short_buffer() {
    // The same shipped kernel, handed a 16-element buffer for its declared 1024. This
    // is the caller bug the guard exists for, against the real artifact.
    let program = naso_compiler::parser::parse_program(KERNEL).expect("parse");
    let pir = naso_compiler::lowering::lower_program(&program).expect("lower");
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("ctx");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("builder");
    builder.build_module(&pir).expect("compile");
    let ir = builder.module_to_string();

    let driver = format!(
        r#"#include <stdint.h>
extern void naso_entry(double *input, int64_t input_len, double *output, int64_t output_len,
                       double scale, double lo, double hi);
int main(void) {{
    static double small[16];
    static double out[{N}];
    naso_entry(small, 16, out, {N}, 3.0, -4.0, 10.0);
    return 0;
}}
"#
    );
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("driver.c");
    let exe = dir.path("case");
    std::fs::write(&ll, &ir).expect("write IR");
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
                .arg("-O0")
                .arg("-Wno-override-module")
                .arg(&ll)
                .arg(&csrc)
                .arg("-o")
                .arg(&exe),
        ),
    ] {
        let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(out.status.success(), "{what} failed: {out:?}");
    }
    let out = Command::new(&exe).output().expect("run");
    assert_eq!(
        out.status.code(),
        Some(naso_compiler::codegen::llvm::abi_guard::NASO_ABI_EXIT_SHORT as i32),
        "a 16-element buffer for `Tensor[f32, 1024]` must fail with exit code 91, got {:?}\n\
         stderr: {}\n--- IR ---\n{ir}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    println!(
        "scale_f32.naso with a 16-element buffer: exit status {:?}",
        out.status.code()
    );
}
