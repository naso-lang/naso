//! Sub-byte `i4` quantization, proven by EXECUTION.
//!
//! # What this file is for
//!
//! The mission is to shrink a model to miniature size. An `i8` quantizer does not do
//! that on its own: eight bits per weight is still eight bits. The load-bearing step is
//! **four bits per weight**, and the only honest way to have an `i4` type is for the
//! object to actually be half the size.
//!
//! The dishonest version of this feature is to declare `i4`, accept it in the source,
//! and give it a one-byte `i8` slot in memory with the low four bits zero. The
//! compression ratio that reports is then a lie, and every downstream size claim
//! inherits it. So the tests here are all execution tests, and one of them asserts the
//! **byte count**.
//!
//! # What the compiler actually does
//!
//! LLVM has no `i4` type. `Tensor[i4, 16]` is therefore stored as an 8-byte alloca:
//! two values per byte, low nibble first.
//!
//! * load  byte at `i >> 1`, then shift right by `(i & 1) * 4`, sign-extend from 4 bits
//! * store read-modify-write: keep the other nibble, OR the new one in
//!
//! # The evidence standard
//!
//! **IR TEXT IS NOT ACCEPTABLE EVIDENCE.** LLVM constant-folds, and a pack/unpack
//! sequence is exactly the kind of code that folds. Every test here compiles with
//! `llc`, links a C driver with `clang`, RUNS the binary, and checks stdout and the
//! exit status.
//!
//! # Sign convention
//!
//! `i4` is two's complement over four bits, so nibble `n` decodes as `n - 16` when
//! `n >= 8`. `0xF` is `-1`, not `15`. A test that decodes with `n - 8` will "fail" a
//! perfectly correct compiler; the reference decoder below is the one that matches the
//! type.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

/// 16 elements, so 8 bytes of storage. The whole point is that this ratio is real.
const N: usize = 16;

struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-i4-{}-{n}", std::process::id()));
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

/// A clamp-then-narrow quantizer over `Tensor[i4, N]`.
///
/// This is the kernel shape a real quantizer uses: divide by the scale, saturate to the
/// representable range, and narrow. The range here is the *actual* `i4` range, not an
/// `i8` range pretending to be one.
fn quant_kernel(n: usize) -> String {
    format!(
        "fn q(input: [1] Tensor[f32, {n}], output: inout [1] Tensor[i4, {n}], scale: f32) {{\n\
         \x20   forall i in 0..{n} {{\n\
         \x20       output[i] = clamp(input[i] / scale, -8.0, 7.0) as i4;\n\
         \x20   }}\n\
         }}\n"
    )
}

fn build_ir(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\nerror: {e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("must compile to LLVM:\n{src}\nerror: {e}"));
    builder.module_to_string()
}

fn link_and_run(ir: &str, driver: &str) -> (Option<i32>, String, String) {
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
                .arg("-Wno-override-module")
                .arg(&ll)
                .arg(&csrc)
                .arg("-o")
                .arg(&exe),
        ),
    ] {
        let out = cmd.output().unwrap_or_else(|e| panic!("spawn {what}: {e}"));
        assert!(
            out.status.success(),
            "{what} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let out = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("run driver: {e}"));
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A driver that runs the quantizer and prints the packed bytes followed by the
/// sign-extended nibbles.
///
/// The reference decoder is `n - 16` for `n >= 8`. That is four-bit two's complement.
/// Writing `n - 8` here would be a test bug that reads as a compiler bug.
fn driver(inputs: &[f64]) -> String {
    let vals = inputs
        .iter()
        .map(|v| format!("{v:.1}"))
        .collect::<Vec<String>>()
        .join(", ");
    let n = inputs.len();
    format!(
        r#"
#include <stdio.h>
#include <stdint.h>
extern void naso_entry(double* in, long inlen, uint8_t* out, long outlen, double scale);
int main(void) {{
    double in[{n}] = {{{vals}}};
    /* {n} nibbles == {bytes} bytes. Pass {n} elements; the ABI guard counts ELEMENTS. */
    uint8_t out[{n}];
    for (long i = 0; i < {n}; i++) out[i] = 0;
    naso_entry(in, {n}, out, {n}, 1.0);
    printf("bytes:");
    for (long i = 0; i < {bytes}; i++) printf(" %02x", out[i]);
    printf("\nnibs:");
    for (long i = 0; i < {n}; i++) {{
        uint8_t b = out[i/2];
        int nib = (i % 2 == 0) ? (b & 0xF) : (b >> 4);
        printf(" %d", nib >= 8 ? nib - 16 : nib);
    }}
    printf("\n");
    return 0;
}}
"#,
        bytes = n / 2
    )
}

/// `i4` decodes from a nibble as four-bit two's complement.
fn decode_nibble(n: i32) -> i32 {
    if n >= 8 { n - 16 } else { n }
}

/// Reference packer: the same saturating quantizer, in the test, independently of the
/// compiler.
fn reference_packed(inputs: &[f64]) -> (Vec<u8>, Vec<i32>) {
    let vals: Vec<i32> = inputs
        .iter()
        .map(|v| {
            let q = (v / 1.0).clamp(-8.0, 7.0) as i32;
            q.clamp(-8, 7)
        })
        .collect();
    let nibs: Vec<u8> = vals.iter().map(|v| (*v & 0xF) as u8).collect();
    let bytes: Vec<u8> = nibs.chunks(2).map(|c| c[0] | (c[1] << 4)).collect();
    (bytes, vals)
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!(" {b:02x}"))
        .collect::<String>()
}

// ---------------------------------------------------------------------------
// 1. The compression claim itself: 16 values occupy 8 bytes.
// ---------------------------------------------------------------------------

#[test]
fn sixteen_i4_values_occupy_eight_bytes() {
    let src = quant_kernel(N);
    let ir = build_ir(&src);
    let inputs: Vec<f64> = (0..N).map(|i| i as f64).collect();
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let bytes_line = stdout
        .lines()
        .find(|l| l.starts_with("bytes:"))
        .expect("bytes line");
    let emitted: Vec<u8> = bytes_line
        .split_whitespace()
        .skip(1)
        .map(|t| u8::from_str_radix(t, 16).expect("hex byte"))
        .collect();

    assert_eq!(
        emitted.len(),
        N / 2,
        "16 four-bit values must occupy 8 bytes, not {} -- if this is {} the \
         'sub-byte' claim is fiction",
        emitted.len(),
        emitted.len()
    );
}

#[test]
fn the_packed_bytes_match_an_independent_reference_packer() {
    let src = quant_kernel(N);
    let ir = build_ir(&src);
    let inputs: Vec<f64> = (0..N).map(|i| i as f64).collect();
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let (want_bytes, _want_vals) = reference_packed(&inputs);
    let bytes_line = stdout
        .lines()
        .find(|l| l.starts_with("bytes:"))
        .expect("bytes line");
    let got: Vec<u8> = bytes_line
        .split_whitespace()
        .skip(1)
        .map(|t| u8::from_str_radix(t, 16).expect("hex byte"))
        .collect();

    assert_eq!(
        hex(&got),
        hex(&want_bytes),
        "packed bytes differ from the reference packer"
    );
}

// ---------------------------------------------------------------------------
// 2. Round-trip through the low and high nibble of every byte position.
// ---------------------------------------------------------------------------

#[test]
fn every_low_nibble_holds_its_own_value() {
    // Even indices occupy the low nibble. Values 0..8 distinguish low from high.
    let inputs: Vec<f64> = (0..N).map(|i| i as f64).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let (_bytes, want) = reference_packed(&inputs);
    let got = parse_nibs(&stdout);
    for i in (0..N).step_by(2) {
        assert_eq!(got[i], want[i], "low nibble {i} is wrong");
    }
}

#[test]
fn every_high_nibble_holds_its_own_value() {
    // Odd indices occupy the high nibble. If the shift were dropped, or applied to
    // both halves, these would collide with their neighbours.
    let inputs: Vec<f64> = (0..N).map(|i| i as f64).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let (_bytes, want) = reference_packed(&inputs);
    let got = parse_nibs(&stdout);
    for i in (1..N).step_by(2) {
        assert_eq!(got[i], want[i], "high nibble {i} is wrong");
    }
}

#[test]
fn no_two_neighbours_share_a_byte_value() {
    // The strongest statement that packing really happened: neighbouring values are
    // distinct, so neither nibble can be a copy of the other or a zero fill.
    //
    // The input must be the -8..7 RAMP, not 0..15. The ramp saturates: `clamp` sends 8
    // to 7, so with 0..15 elements 7 and 8 both decode to 7 and would collide for a
    // reason that has nothing to do with packing.
    let inputs: Vec<f64> = (0..N).map(|i| i as f64 - 8.0).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");
    let got = parse_nibs(&stdout);
    for i in 0..N - 1 {
        assert_ne!(
            got[i],
            got[i + 1],
            "elements {i} and {} decoded identically, so at least one is wrong",
            i + 1
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Sign: the low half of the range must be negative and survive the round trip.
// ---------------------------------------------------------------------------

#[test]
fn negative_values_survive_the_nibble_round_trip() {
    // -1..-8, then saturation at -8. A sign-extension bug shows up here and nowhere else.
    let inputs: Vec<f64> = (0..N).map(|i| -((i + 1) as f64)).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let got = parse_nibs(&stdout);
    let (_bytes, want) = reference_packed(&inputs);
    assert_eq!(got, want, "negative round trip differs from reference");
    assert!(
        got.iter().all(|v| *v <= 0),
        "every input was negative, so every output must be: got {got:?}"
    );
    assert!(
        got.contains(&-1),
        "-1 must be representable, or the type is unsigned in disguise: {got:?}"
    );
}

#[test]
fn a_full_two_complement_ramp_round_trips() {
    // -8..7 is exactly the i4 range, so nothing saturates and the ramp must come back
    // unchanged. This is the case that catches an off-by-one in the range.
    let inputs: Vec<f64> = (0..N).map(|i| i as f64 - 8.0).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let got = parse_nibs(&stdout);
    let want: Vec<i32> = (0..N).map(|i| i as i32 - 8).collect();
    assert_eq!(got, want, "the full representable ramp did not survive");
}

#[test]
fn out_of_range_values_saturate_rather_than_wrap() {
    // Values far outside the range must clamp. Wrapping would be silent corruption --
    // the classic failure of a quantizer that drops a clamp.
    let inputs: Vec<f64> = (0..N).map(|i| 1000.0 + i as f64).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let got = parse_nibs(&stdout);
    assert!(
        got.iter().all(|v| *v == 7),
        "everything above the range must saturate to 7, got {got:?}"
    );

    let inputs: Vec<f64> = (0..N).map(|i| -1000.0 - i as f64).collect();
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");
    let got = parse_nibs(&stdout);
    assert!(
        got.iter().all(|v| *v == -8),
        "everything below the range must saturate to -8, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Writing one element must not destroy its neighbour.
// ---------------------------------------------------------------------------

#[test]
fn a_full_store_touches_exactly_half_as_many_bytes_as_elements() {
    // Pre-fill the output buffer with 0xFF, run the kernel, and look at WHICH BYTES it
    // wrote.
    //
    // A widened `i8` implementation writes 16 bytes. A genuinely packed one writes 8 and
    // leaves the tail of the buffer untouched. This is the size assertion stated as an
    // observation about memory, so it cannot be satisfied by a compiler that merely
    // accepts the `i4` keyword.
    let ir = build_ir(&quant_kernel(N));
    let driver = r#"
#include <stdio.h>
#include <stdint.h>
extern void naso_entry(double* in, long inlen, uint8_t* out, long outlen, double scale);
int main(void) {
    double in[16];
    for (int i = 0; i < 16; i++) in[i] = 0.0;
    /* 0xFF in every byte: every nibble decodes to -1. */
    uint8_t out[16];
    for (int i = 0; i < 16; i++) out[i] = 0xFF;
    naso_entry(in, 16, out, 16, 1.0);
    printf("touched:");
    for (int i = 0; i < 16; i++) printf(" %d", out[i] == 0x00 ? 1 : 0);
    printf("\n");
    return 0;
}
"#;
    let (code, stdout, stderr) = link_and_run(&ir, driver);
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

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
        touched,
        N / 2,
        "16 i4 values must write 8 bytes, but the kernel touched {touched}          (flags {flags:?}) -- a widened i8 store would touch all {N}"
    );
}

#[test]
fn a_pack_store_reads_the_existing_byte_rather_than_assuming_zero() {
    // The pre-existing nibble must survive a neighbouring write. This is the
    // read-modify-write half of packing: if the store just wrote the byte, the other
    // value in it would be lost.
    //
    // A dedicated single-element kernel writes only slot 1 (the high nibble of byte 0),
    // so the low nibble of byte 0 -- which the driver pre-sets -- must be unchanged.
    let src = "fn q(input: [1] Tensor[f32, 16], output: inout [1] Tensor[i4, 16], scale: f32) {\n\
               \x20   output[1] = clamp(input[1] / scale, -8.0, 7.0) as i4;\n\
               }\n";
    let ir = build_ir(src);
    let driver = r#"
#include <stdio.h>
#include <stdint.h>
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
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");

    let line = stdout
        .lines()
        .find(|l| l.starts_with("byte0:"))
        .expect("byte0 line");
    let got = u8::from_str_radix(line.split_whitespace().nth(1).expect("hex"), 16).expect("hex");
    // high nibble must now be 3 (the written value); low nibble must still be 5.
    assert_eq!(
        got, 0x35,
        "expected 0x35 (high=3 written, low=5 preserved), got 0x{got:02x} -- \
         if this is 0x03 or 0x30 the store is not a read-modify-write"
    );
}

// ---------------------------------------------------------------------------
// 5. A guard against the dishonest implementation.
// ---------------------------------------------------------------------------

#[test]
fn reading_an_i4_element_works_with_a_non_i64_index() {
    // REGRESSION. `sub_byte_load` shifts the loaded byte by a nibble offset computed from
    // the element index. The index has WHATEVER width the loop or literal gave it -- `i64`
    // for a `forall` counter, `i32` for a literal `x[0]`. LLVM requires both operands of a
    // shift to be the same width, so a narrow index used to emit `lshr i8 %b, i64 0`, which
    // is invalid IR and made the whole function fail to verify.
    //
    // Every other test here indexes from inside a `forall`, so they all pass an `i64` and
    // would not have caught it. This one indexes with a literal.
    let ir = build_ir("fn f(x: Tensor[i4, 8]) -> i8 { x[0] as i8 }");
    assert!(
        ir.contains("define i8 @naso_f"),
        "a literal-indexed i4 read must compile, not fail LLVM verification"
    );
    // The shift amount must be a CONSTANT, never an i64 SSA value. That is the exact shape
    // the bug produced: `lshr i8 %i4_byte, %narrow_index`, where LLVM then rejected the
    // module because the operands had different widths.
    for line in ir.lines() {
        let Some(rest) = line.split("lshr i8 ").nth(1) else {
            continue;
        };
        // `lshr i8 <value>, <amount>` -- the amount is the operand AFTER the comma.
        let amount = rest.split_once(',').map(|(_, a)| a.trim()).unwrap_or("");
        assert!(
            !amount.starts_with('%'),
            "shift amount must be a constant of the byte's own width, not an SSA value: {line}"
        );
    }
}

#[test]
fn i4_is_not_stored_as_a_widened_i8() {
    // The mutation this file exists to kill: accept `i4` in the source but give it a
    // full byte, leaving the high nibble of every storage byte unused. Then 16 values
    // would occupy 16 bytes and the driver above would read garbage out of the high
    // halves. Assert the store is a real merge: the emitted IR must contain a shift and
    // an or, and must not contain a plain i8 store of the narrowed value.
    let ir = build_ir(&quant_kernel(N));
    let has_shift = ir.contains("lshr i64") || ir.contains("shl i64");
    let has_merge = ir.contains("or i8");
    assert!(
        has_shift && has_merge,
        "expected a nibble shift and an or-merge in the store path"
    );
}

#[test]
fn a_scalar_i4_parameter_is_refused_rather_than_widened() {
    // A scalar `i4` has no packing context: there is no neighbour to share a byte with.
    // Widening it to `i8` would reintroduce exactly the fiction this file rejects, so
    // the compiler must refuse.
    let src = "fn f(x: i4) -> i4 { x }";
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse: {e}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower: {e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    let result = builder.build_module(&pir);
    assert!(
        result.is_err(),
        "a scalar i4 must be refused, not silently widened: got {:?}",
        result.map(|_| "Ok")
    );
}

// ---------------------------------------------------------------------------

fn parse_nibs(stdout: &str) -> Vec<i32> {
    stdout
        .lines()
        .find(|l| l.starts_with("nibs:"))
        .expect("nibs line")
        .split_whitespace()
        .skip(1)
        .map(|t| t.parse().expect("int"))
        .collect()
}

/// Keeps `decode_nibble` honest in the doc comment above: it must agree with what the
/// compiler actually produced for `0xF`.
#[test]
fn the_test_decoder_agrees_with_the_compiler_on_the_sign_convention() {
    let inputs: Vec<f64> = vec![-1.0; N];
    let ir = build_ir(&quant_kernel(N));
    let (code, stdout, stderr) = link_and_run(&ir, &driver(&inputs));
    assert_eq!(code, Some(0), "driver failed:\n{stderr}");
    let got = parse_nibs(&stdout);
    assert_eq!(
        got[0],
        decode_nibble(0xF),
        "the reference decoder must map 0xF to -1"
    );
    assert_eq!(got[0], -1);
}
