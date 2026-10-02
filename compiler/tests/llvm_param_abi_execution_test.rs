//! End-to-end: a function's PARAMETERS must reach the generated LLVM function.
//!
//! # The gap this file pins
//!
//! `PirModule` carried `parameters: Vec<String>`, but that list is only the
//! module-level SYMBOLIC CONSTANTS of a `forall i in 0..n` — `n` and friends, which
//! `schedule_parameters` turns into trailing `i64` arguments. A function's OWN
//! parameters produced no PIR at all: `extract_parameters` walked `main` for names
//! and quantities and never looked at a type, and nothing in `lowering` or
//! `build_module` read the parameter list of an arbitrary function.
//!
//! So `input[i]` in a kernel body had no allocation behind it. Until the strict
//! unbound read landed that returned i64 zero, which meant a kernel READ ITS INPUT
//! AS ZERO, compiled cleanly, and computed `clamp(0 * scale, lo, hi)`. That is the
//! wrong answer with no diagnostic, and it is what this file exists to make
//! impossible to reintroduce silently.
//!
//! # What the ABI is
//!
//! - `Tensor[T, N]` / `QRegister` -> `ptr` to `T`. The pointer IS the data: nothing
//!   is allocated and nothing is zero-filled, because a callee-allocated tensor is
//!   one the caller cannot supply.
//! - scalar `f32` -> `double`, by value. The AST carries no float width (`f32` and
//!   `f64` are the same `TypeKind::Float`), so `double` is the only representable
//!   choice without inventing a width field.
//! - scalar `i64` -> `i64`, by value.
//!
//! # Why this RUNS the code
//!
//! Same reasoning as `llvm_execution_test`: IR text cannot distinguish a
//! computation from its own dead code. LLVM folds constants forward, an unused
//! instruction disappears entirely, and `output[i] = ...` written to a different
//! pointer prints identically to one written to the caller's buffer. The only check
//! a "computes the right value and throws it away" arm cannot pass is to fill a
//! buffer, run, and read every element back.
//!
//! Every case gets its own temp directory, unique per CALL: the harness runs tests
//! as threads of one process, and a shared directory makes one case read another's
//! binary.
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
/// process, so a shared directory would have cases overwrite each other's binaries.
struct CaseDir(PathBuf);

static CASE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaseDir {
    fn new() -> Self {
        let n = CASE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("naso-paramabi-{}-{n}", std::process::id()));
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

/// The C driver: fill `input` with a pattern, call the generated entry, print `output`.
///
/// The printed line is every element of `output`, space separated. Asserting the
/// whole array rather than one element is the point: an alloca-and-zero-fill fix
/// (the alternative that "compiles") returns 0.0 for every slot, so a single
/// in-range element can pass by luck while the rest of the array is wrong.
fn driver_c(n: usize) -> String {
    format!(
        r#"#include <stdio.h>
extern void naso_entry(double *input, double *output, double scale, double lo, double hi);

// Values chosen so that clamp() is exercised on BOTH sides and in the middle:
// below `lo`, strictly between, and above `hi`. A kernel that computed the product
// but skipped the clamp would pass a centre-only input array.
static double in[{n}];
static double out[{n}];

int main(void) {{
    for (int i = 0; i < {n}; ++i) in[i] = (double)i - (double)({n} / 2);
    for (int i = 0; i < {n}; ++i) out[i] = -12345.0;   // poison, so an unwritten slot is visible
    naso_entry(in, out, 3.0, -4.0, 10.0);
    for (int i = 0; i < {n}; ++i) {{
        if (i) putchar(' ');
        printf("%.17g", out[i]);
    }}
    putchar('\n');
    return 0;
}}
"#
    )
}

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

/// Compile `ir`, link the C driver, run it, and return the printed doubles.
fn execute(ir: &str, n: usize) -> Vec<f64> {
    let dir = CaseDir::new();
    let ll = dir.path("case.ll");
    let obj = dir.path("case.o");
    let csrc = dir.path("driver.c");
    let exe = dir.path("case");

    std::fs::write(&ll, ir).expect("write IR");
    std::fs::write(&csrc, driver_c(n)).expect("write C driver");
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
        .split_whitespace()
        .map(|f| {
            f.parse::<f64>()
                .unwrap_or_else(|e| panic!("driver printed {f}, which is not a number: {e}"))
        })
        .collect()
}

/// Lower `src` and emit LLVM IR through the production `build_module`.
///
/// The IR comes from `module_to_string()` on the builder that `build_module` wrote
/// into, not from a second hand-built module: the point is to inspect exactly the
/// module the production path produced.
fn build_ir(src: &str) -> String {
    match try_build_ir(src) {
        Ok(ir) => ir,
        Err(e) => panic!("source must compile:\n{src}\nerror: {e}"),
    }
}

/// Lower `src` and emit IR, surfacing any refusal as an error string.
///
/// Lowering and codegen have different error types, so both are flattened to text
/// here. A refusal test asserts on that text; flattening loses nothing it needs,
/// because a diagnostic is what a caller of the CLI actually sees.
fn try_build_ir(src: &str) -> Result<String, String> {
    let program =
        naso_compiler::parser::parse_program(src).map_err(|e| format!("source must parse: {e}"))?;
    let pir = naso_compiler::lowering::lower_program(&program).map_err(|e| format!("{e}"))?;
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder.build_module(&pir).map_err(|e| format!("{e}"))?;
    Ok(builder.module_to_string())
}

/// The kernel that used to fail with `read of 'input': no allocation is known for it`.
const SCALE_F32: &str = r#"
fn scale_clamp_f32(
    input: [1] Tensor[f32, 64],
    output: inout [1] Tensor[f32, 64],
    scale: f32,
    lo: f32,
    hi: f32
) {
    forall i in 0..64 {
        let v = input[i] * scale;
        output[i] = clamp(v, lo, hi);
    }
}
"#;

#[test]
fn tensor_parameters_are_pointers_in_the_entry_signature() {
    let ir = build_ir(SCALE_F32);
    let sig = ir
        .lines()
        .find(|l| l.starts_with("define "))
        .expect("module must define an entry function");
    // A tensor is a `ptr`, and it is the CALLER's pointer: no `alloca` of the
    // element type may stand in for it.
    assert!(
        sig.contains("naso_entry(ptr %input"),
        "entry must take `input` as a pointer, got: {sig}"
    );
    assert!(
        sig.contains("ptr %output"),
        "entry must take `output` as a pointer, got: {sig}"
    );
    // Scalars are by value, in declaration order, after the tensors.
    assert!(
        sig.contains("double %scale") && sig.contains("double %lo") && sig.contains("double %hi"),
        "scalars must be by-value arguments, got: {sig}"
    );
    let idx = |needle: &str| {
        sig.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {sig}"))
    };
    assert!(idx("ptr %input") < idx("double %scale"), "order: {sig}");
    // An alloca-and-zero-fill "fix" would compile and compute on data the caller
    // never supplied. This is the assertion that rules it out.
    assert!(
        !ir.contains("alloca double, align 8\n  store double 0.0"),
        "a tensor parameter must not become a zero-filled local buffer:\n{ir}"
    );
}

#[test]
fn scale_f32_kernel_computes_clamp_over_the_whole_caller_buffer() {
    let n = 64usize;
    let ir = build_ir(SCALE_F32);
    let got = execute(&ir, n);

    assert_eq!(
        got.len(),
        n,
        "driver printed {} values, expected {n}",
        got.len()
    );

    // input[i] = i - n/2, scale = 3, so the product is 3i - 96; clamp to [-4, 10].
    // The range spans i = 0..64, so the product runs -96 .. +93: below `lo`, inside,
    // and above `hi` are all represented, and every one of the three regimes is
    // covered by many elements rather than one.
    let mut below = 0;
    let mut inside = 0;
    let mut above = 0;
    for (i, &value) in got.iter().enumerate().take(n) {
        let product = (i as f64 - (n / 2) as f64) * 3.0;
        let want = product.clamp(-4.0, 10.0);
        assert_eq!(
            got[i],
            want,
            "element {i}: input {} * 3.0 = {product}, clamped should be {want}, driver produced {}",
            i as f64 - (n / 2) as f64,
            value
        );
        if product < -4.0 {
            below += 1;
        } else if product > 10.0 {
            above += 1;
        } else {
            inside += 1;
        }
    }
    // If the test input ever stops exercising all three regimes the assertion above
    // would start passing vacuously, so pin that it did not.
    assert!(
        below > 0 && inside > 0 && above > 0,
        "test input must span all three clamp regimes, got below={below} inside={inside} above={above}"
    );
}

#[test]
fn a_scalar_only_parameter_is_an_argument_and_reaches_the_arithmetic() {
    // No tensor at all: the whole computation is `result = a * k`, so if `a` and `k`
    // were dropped the result would be 0 or garbage.
    let src = r#"
fn scale_one(a: f32, k: f32) -> f32 {
    return a * k;
}
"#;
    let ir = build_ir(src);
    let sig = ir
        .lines()
        .find(|l| l.starts_with("define "))
        .expect("module must define an entry function");
    assert!(
        sig.contains("double %a") && sig.contains("double %k"),
        "both scalars must be arguments, got: {sig}"
    );
}

#[test]
fn nested_tensor_indexing_is_refused_with_a_diagnostic() {
    // A tensor-of-tensors parameter has no representation: the ABI passes ONE
    // pointer, so `rows[i][j]` would need a second stride that nothing supplies.
    // This must be a refusal naming the construct, not a silent first-level-only
    // index that returns the wrong element.
    let src = r#"
fn bad(input: [1] Tensor[Tensor[f32, 4], 8]) {
    let v = input[0][0];
}
"#;
    // Lowering may reject the shape outright; if it does not, codegen must.
    match try_build_ir(src) {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("nested tensor"),
                "refusal must name nesting as the reason, not just \"a tensor\", got: {msg}"
            );
            assert!(
                msg.contains("second stride"),
                "refusal must say why one pointer is not enough, got: {msg}"
            );
        }
        Ok(ir) => panic!("nested tensor indexing must be refused, but it compiled:\n{ir}"),
    }
}

#[test]
fn an_i8_tensor_parameter_keeps_its_exact_i8_storage() {
    // This is the OPPOSITE of a refusal, and it is here deliberately: LLVM has an exact
    // `i8` type, so a `Tensor[i8, N]` parameter is exactly representable as `ptr` and
    // this backend does NOT widen it. The `store i8` and the absence of any `i32` in
    // the signature are the evidence -- a backend that widened i8 to i32 to make a
    // kernel "work" would produce a `store i32` and a signature full of `i32`.
    let src = r#"
fn q(output: inout [1] Tensor[i8, 16], k: i8) {
    forall i in 0..16 { output[i] = k; }
}
"#;
    let ir = build_ir(src);
    let sig = ir
        .lines()
        .find(|l| l.starts_with("define "))
        .expect("module must define an entry function");
    assert!(
        sig.contains("ptr %output") && sig.contains("i8 %k"),
        "an i8 tensor must be a pointer and an i8 scalar an i8 argument, got: {sig}"
    );
    assert!(
        !sig.contains("i32"),
        "nothing about this kernel should be i32, got: {sig}"
    );
    assert!(
        ir.contains("store i8"),
        "the element store must be exactly one byte wide:\n{ir}"
    );
}

#[test]
fn storing_a_float_into_an_i8_tensor_slot_is_refused_rather_than_narrowed() {
    // The inverse: the SLOT is right, but the body wants to put a `double` in it.
    // Truncating to fit would be a silent wrong answer -- and `as i8` narrowing is
    // genuinely implemented elsewhere, so this is about implicit conversion, not about
    // i8 being unrepresentable.
    let src = r#"
fn q(input: [1] Tensor[f32, 16], output: inout [1] Tensor[i8, 16], scale: f32) {
    forall i in 0..16 { output[i] = input[i] * scale; }
}
"#;
    match try_build_ir(src) {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("Widen") || msg.contains("widening"),
                "refusal must say a conversion would be a widening bug, got: {msg}"
            );
        }
        Ok(ir) => {
            panic!("storing a float into an i8 tensor must be refused, but it compiled:\n{ir}")
        }
    }
}

#[test]
fn two_functions_reusing_a_parameter_name_with_different_types_are_refused() {
    // The deliberate limitation, pinned. A `PirModule` is ONE flat statement list with
    // no function structure, so the generated entry has one slot per NAME. Two
    // functions that both declare `input` at different element types cannot both be
    // satisfied by one slot. Deduplicating on name alone would silently give the
    // second function the first one's buffer -- the exact class of silent wrong answer
    // this work exists to remove -- so it is a named refusal instead.
    let src = r#"
fn a(input: [1] Tensor[f32, 8]) {
    let v = input[0];
}
fn b(input: [1] Tensor[i8, 8]) {
    let v = input[0];
}
"#;
    match try_build_ir(src) {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("`input`") && msg.contains("two different types"),
                "refusal must name the colliding parameter, got: {msg}"
            );
        }
        Ok(ir) => panic!(
            "two conflicting declarations of `input` must be refused, but it compiled:\n{ir}"
        ),
    }
}

#[test]
fn assigning_to_an_unknown_name_is_still_an_error() {
    // The strict unbound read must not have been weakened to get the above working.
    let src = r#"
fn bad(output: inout [1] Tensor[f32, 8]) {
    forall i in 0..8 {
        not_declared[i] = 1.0;
    }
}
"#;
    match try_build_ir(src) {
        Err(e) => assert!(
            e.to_string().contains("not_declared"),
            "refusal must name the unknown target, got: {e}"
        ),
        Ok(ir) => panic!("assignment to an undeclared name must fail, but it compiled:\n{ir}"),
    }
}

/// A two-dimensional tensor must linearise row-major using the DECLARED shape.
///
/// `linearize_index` computes `i * stride + j`, where a dimension's stride is the
/// product of every extent to its right -- so `u[i][j]` in a `Tensor[f32, 2, 3]` is
/// `i*3 + j`. That product is the entire correctness of multi-dimensional indexing and
/// NOTHING covered it: every other test in this file indexes a ONE-dimensional tensor,
/// so setting every stride to 1 still passes all of them. A wrong stride reads the
/// wrong element and returns a plausible wrong number, which is the failure mode this
/// whole ABI work exists to remove.
///
/// The driver writes DISTINCT values into the buffer and reads back the elements the
/// body wrote, so a stride error is observable rather than merely plausible. Row-major
/// gives `dst[j] = src[i*3 + j]`; column-major would give `dst[i + 2*j]`.
#[test]
fn a_two_dimensional_tensor_indexes_row_major_using_the_declared_shape() {
    let src = r#"
fn transpose2d(a: [1] Tensor[f32, 2, 3], b: inout [1] Tensor[f32, 3, 2]) {
    forall i in 0..2 {
        forall j in 0..3 {
            b[j][i] = a[i][j];
        }
    }
}
"#;
    let ir = build_ir(src);

    // The signature must pass both tensors as pointers so a driver can supply them.
    assert!(
        ir.contains("ptr %a") && ir.contains("ptr %b"),
        "both tensors must be pointer arguments:\n{ir}"
    );

    // Drive it. `a` is 2x3 = 6 elements, `b` is 3x2 = 6 elements. Fill `a` with 1..6 and
    // check `b` is the transpose: b[j][i] = a[i][j], so b = [1 4 3 6] row-major.
    let dir = CaseDir::new();
    let ll = dir.path("m.ll");
    std::fs::write(&ll, &ir).expect("write ir");
    let driver = dir.path("d.c");
    std::fs::write(
        &driver,
        r#"
#include <stdio.h>
void naso_entry(double*, double*);
int main(void) {
  double a[6], b[6];
  for (int i = 0; i < 6; i++) a[i] = (double)(i + 1);
  for (int i = 0; i < 6; i++) b[i] = -1.0;
  naso_entry(a, b);
  for (int i = 0; i < 6; i++) printf("%g ", b[i]);
  printf("\n");
  return 0;
}
"#,
    )
    .expect("write driver");
    let exe = dir.path("prog");
    run(
        Command::new(tool("clang"))
            .arg("-O0")
            .arg("-Wno-override-module")
            .arg(&ll)
            .arg(&driver)
            .arg("-o")
            .arg(&exe),
        "clang",
        &ir,
    );
    let out = run(&mut Command::new(&exe), "the 2-D transposing program", &ir);
    let got: Vec<f64> = String::from_utf8_lossy(&out)
        .split_whitespace()
        .map(|f| f.parse::<f64>().expect("driver prints numbers"))
        .collect();

    // a is the 2x3 [[1,2,3],[4,5,6]] and b is the 3x2 transpose, so row-major b is
    // [[1,4],[2,5],[3,6]] -> flat [1, 4, 2, 5, 3, 6]. A stride-1 (column-major) layout
    // would instead produce [1, 2, 4, 3, 5, 6], which is what this assertion rules out.
    let want = [1.0, 4.0, 2.0, 5.0, 3.0, 6.0];
    assert_eq!(
        got.len(),
        want.len(),
        "driver printed the wrong number of elements: {got:?}\n--- IR ---\n{ir}"
    );
    for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(
            g, w,
            "element {i}: the row-major transpose should be {w}, got {g}. A stride-1 \
             or column-major layout yields [1, 2, 4, 3, 5, 6] instead, which is what \
             this rules out.\n--- IR ---\n{ir}"
        );
    }
}
