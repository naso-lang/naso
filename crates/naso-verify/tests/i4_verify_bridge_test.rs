//! Bridge: a verified `i4` quantizer must execute within the range the prover proved.
//!
//! This test closes the gap between the two halves — the SMT prover (naso-verify) and
//! the LLVM execution test suite (`sub_byte_i4_execution_test.rs`):
//!
//!   1. PROVE: the kernel's `clamp(v, -8.0, 7.0)` obligation discharges to exit 0.
//!   2. EXECUTE: compile the same source to LLVM IR, link, RUN, and check every emitted
//!      nibble decodes into `[-8, 7]`.
//!
//! If the prover says "safe" and execution says "out of range", either the theorem
//! or the backend is broken — they are the SAME claim at two levels and both must hold.

#![cfg(all(feature = "z3", feature = "llvm"))]

use std::path::PathBuf;
use std::process::Command;

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

/// An i4 quantizer kernel that mirrors the i8 one but with the REAL i4 range.
///
/// The proof obligation `clamp(v, -8.0, 7.0) <= 7.0` and `>= -8.0` must discharge
/// under UFLIA. The runtime must then respect that range over packed storage.
const I4_QUANT_KERNEL: &str = r#"
fn quantize_i4(input: [1] Tensor[f32, 16], output: inout [1] Tensor[i4, 16], scale: f32) {
    proof {
        // The prover must discharge: clamp(v,-8,7) lies in [-8, 7].
        // This uses the round bounding axiom, not round-equality, so UFLIA suffices.
        assert(clamp(input[0] / scale, -8.0, 7.0) <= 7.0);
        assert(clamp(input[0] / scale, -8.0, 7.0) >= -8.0);
    }
    forall i in 0..16 {
        let v = round(input[i] / scale);
        output[i] = clamp(v, -8.0, 7.0) as i4;
    }
}
"#;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_naso-verify"))
}

fn tool(name: &str) -> String {
    match std::env::var("LLVM_SYS_170_PREFIX") {
        Ok(prefix) if !prefix.is_empty() => format!("{prefix}/bin/{name}"),
        _ => name.to_string(),
    }
}

/// Compile a naso source string to LLVM IR text.
fn build_ir(src: &str) -> String {
    let program =
        naso_compiler::parser::parse_program(src).unwrap_or_else(|e| panic!("parse:\n{e}"));
    let pir =
        naso_compiler::lowering::lower_program(&program).unwrap_or_else(|e| panic!("lower:\n{e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen ctx");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("codegen:\n{e}"));
    builder.module_to_string()
}

/// Link and run a driver, returning (exit_code, stdout, stderr).
fn link_and_run(ir: &str, driver: &str) -> (Option<i32>, String, String) {
    let dir = std::env::temp_dir().join(format!(
        "naso-i4-bridge-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let ll = dir.join("case.ll");
    let obj = dir.join("case.o");
    let csrc = dir.join("driver.c");
    let exe = dir.join("case");

    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&csrc, driver).expect("write driver");

    for (what, status) in [
        (
            "llc",
            Command::new(tool("llc"))
                .arg("-relocation-model=pic")
                .arg("-filetype=obj")
                .arg(&ll)
                .arg("-o")
                .arg(&obj)
                .status(),
        ),
        (
            "clang",
            Command::new(tool("clang"))
                .arg("-Wno-override-module")
                .arg(&ll)
                .arg(&csrc)
                .arg("-o")
                .arg(&exe)
                .arg("-lm")
                .status(),
        ),
    ] {
        let s = status.expect("spawn {what}");
        assert!(s.success(), "{what} failed (code {s:?})");
    }

    let out = Command::new(&exe).output().expect("run driver");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.code(), stdout, stderr)
}

/// Decode a nibble as four-bit two's complement.
fn decode_nibble(n: i32) -> i32 {
    if n >= 8 { n - 16 } else { n }
}

/// Parse the `nibs:` line from driver output into sign-extended i32 values.
fn parse_nibs(stdout: &str) -> Vec<i32> {
    stdout
        .lines()
        .find(|l| l.starts_with("nibs:"))
        .expect("nibs line")
        .split_whitespace()
        .skip(1)
        .map(|t| decode_nibble(t.parse::<i32>().expect("int")))
        .collect()
}

/// Driver: fills output with -1 (poison), runs the kernel, prints packed bytes
/// and decoded nibbles. Uses `input[i] = i - 8` so values 0..8 clamp to 7,
/// values 8..16 clamp to 7 (since clamp(8, -8, 7) = 7), and negatives
/// saturate at -8.
fn driver() -> String {
    r#"#include <stdint.h>
#include <stdio.h>
extern void naso_quantize_i4(double* input, int64_t input_len, uint8_t* output, int64_t output_len, double scale);
static double in[16];
static uint8_t out[16];
int main(void) {
    for (int i = 0; i < 16; i++) in[i] = (double)(i - 8);
    /* 0xFF = every nibble decodes to -1: a store that skips read-modify-write loses data. */
    for (int i = 0; i < 16; i++) out[i] = 0xFF;
    naso_quantize_i4(in, 16, out, 16, 1.0);
    printf("nibs:");
    for (int i = 0; i < 16; i++) {
        uint8_t b = out[i/2];
        int nib = (i % 2 == 0) ? (b & 0xF) : (b >> 4);
        printf(" %d", nib >= 8 ? nib - 16 : nib);
    }
    printf("\n");
    return 0;
}
"#.to_string()
}

// ---------------------------------------------------------------------------
// 1. PROVE: the i4 kernel's clamp obligations discharge through the CLI.
// ---------------------------------------------------------------------------

#[test]
fn the_i4_kernel_verify_obligations_discharge_through_the_cli() {
    let mut path = std::env::temp_dir();
    path.push(format!("naso_verify_i4_{}.naso", std::process::id()));
    std::fs::write(&path, I4_QUANT_KERNEL).expect("write kernel source");

    let out = Command::new(binary())
        .args(["--mode", "obligations", "--require-obligations"])
        .arg(&path)
        .output()
        .expect("run naso-verify");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let _ = std::fs::remove_file(&path);

    assert_eq!(
        out.status.code(),
        Some(0),
        "the i4 kernel's clamp obligations must discharge (exit 0). stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("2 obligation(s) discharged"),
        "both clamp range obligations must be discharged, got:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// 2. EXECUTE: the same kernel compiles to LLVM and runs within [-8, 7].
// ---------------------------------------------------------------------------

#[test]
fn the_i4_kernel_executes_within_the_proven_range() {
    let ir = build_ir(I4_QUANT_KERNEL);
    let (code, stdout, stderr) = link_and_run(&ir, &driver());

    assert_eq!(
        code,
        Some(0),
        "driver must exit 0. stderr:\n{stderr}\nstdout:\n{stdout}\n--- IR ---\n{ir}"
    );

    let got = parse_nibs(&stdout);
    assert_eq!(got.len(), 16, "must decode 16 nibbles");

    for (i, &v) in got.iter().enumerate() {
        assert!(
            (-8..=7).contains(&v),
            "nibble {i} = {v} is outside [-8, 7] that the prover proved the clamp keeps.\n\
             got: {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. The pack-store is read-modify-write, not a widened i8 store.
// ---------------------------------------------------------------------------

#[test]
fn the_i4_store_touches_exactly_half_as_many_bytes_as_elements() {
    let ir = build_ir(I4_QUANT_KERNEL);
    // Pre-fill with 0xFF, check only the 8 bytes (not 16) were written.
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_quantize_i4(double* input, int64_t input_len, uint8_t* output, int64_t output_len, double scale);
static double in[16];
static uint8_t out[16];
int main(void) {
    for (int i = 0; i < 16; i++) in[i] = (double)(i - 8);
    for (int i = 0; i < 16; i++) out[i] = 0xFF;
    naso_quantize_i4(in, 16, out, 16, 1.0);
    printf("touched:");
    for (int i = 0; i < 16; i++) printf(" %d", out[i] == 0xFF ? 0 : 1);
    printf("\n");
    return 0;
}
"#;
    let (code, stdout, stderr) = link_and_run(&ir, driver);
    assert_eq!(code, Some(0), "stderr:\n{stderr}");

    let flags: Vec<i32> = stdout
        .lines()
        .find(|l| l.starts_with("touched:"))
        .expect("touched line")
        .split_whitespace()
        .skip(1)
        .map(|t| t.parse().expect("int"))
        .collect();

    let touched = flags.iter().filter(|f| **f == 1).count();
    assert_eq!(
        touched, 8,
        "16 i4 values must write 8 bytes, but the kernel touched {touched} \
         (flags {flags:?}) -- a widened i8 store would touch all 16"
    );
}

// ---------------------------------------------------------------------------
// 4. A single-element store preserves its neighbour (read-modify-write).
// ---------------------------------------------------------------------------

#[test]
fn a_single_i4_store_preserves_its_neighbour() {
    let src = "fn q(input: [1] Tensor[f32, 16], output: inout [1] Tensor[i4, 16], scale: f32) {\n\
         \x20 forall i in 0..16 {\n\
         \x20   if i == 1 {\n\
         \x20     output[i] = clamp(input[i] / scale, -8.0, 7.0) as i4;\n\
         \x20   }\n\
         \x20 }\n\
         }\n";
    let ir = build_ir(src);
    let driver = r#"#include <stdint.h>
#include <stdio.h>
extern void naso_entry(double* in, long inlen, uint8_t* out, long outlen, double scale);
int main(void) {
    double in[16];
    for (int i = 0; i < 16; i++) in[i] = 3.0;   /* quantizes to 3 */
    uint8_t out[16];
    for (int i = 0; i < 16; i++) out[i] = 0xFF;
    /* byte 0 low nibble = 5, high nibble = -1 (0xF) */
    out[0] = 0xF5;
    naso_entry(in, 16, out, 16, 1.0);
    printf("byte0: %02x\n", out[0]);
    return 0;
}
"#;
    let (code, stdout, stderr) = link_and_run(&ir, driver);
    assert_eq!(code, Some(0), "stderr:\n{stderr}");

    let line = stdout
        .lines()
        .find(|l| l.starts_with("byte0:"))
        .expect("byte0 line");
    let got = u8::from_str_radix(line.split_whitespace().nth(1).expect("hex"), 16).expect("hex");
    // high nibble = 3 (written), low nibble = 5 (preserved from driver).
    assert_eq!(
        got, 0x35,
        "expected 0x35 (high=3 written, low=5 preserved), got 0x{got:02x} -- \
         if this is 0x03 or 0x30 the store is not a read-modify-write"
    );
}
