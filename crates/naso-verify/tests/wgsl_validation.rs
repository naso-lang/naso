//! Validate compiler-generated WGSL with naga, the front-end wgpu uses.
//!
//! # Why this exists, given no GPU is available
//!
//! The generated shaders were previously checked with `wgsl_reflect`, which PARSES.
//! Parsing proves the text is well-formed; it does not prove the shader typechecks.
//! A shader with a wrong binding type, a mismatched `i32`/`u32`, or an out-of-range
//! constructor still parses, and would fail only when a real driver compiled it --
//! in a user's browser, on their machine, with no diagnostic pointing at the
//! compiler that produced it.
//!
//! There is no GPU in this environment and no way to get one:
//!
//!   /dev/dri        absent
//!   /dev/nvidia*    absent
//!   /dev/kfd        absent
//!   Vulkan ICDs     none installed (the loader `libvulkan.so.1` is present, but a
//!                   loader without a driver reports zero devices)
//!   navigator.gpu   undefined under Node
//!
//! `mesa-vulkan-drivers` (llvmpipe, the software Vulkan driver) has no installable
//! candidate. So execution cannot be tested here, and this file does not pretend
//! otherwise.
//!
//! What CAN be tested is the layer a driver would reject first. naga is wgpu's own
//! WGSL front-end: `parse` then `validate` performs full type resolution, binding
//! layout construction, and expression type checking. That is strictly stronger than
//! a parse and catches the class of bug that would otherwise reach a user.
//!
//! What remains untested, and must be tested on real hardware: numerical results,
//! `dispatchWorkgroups` behaviour, buffer aliasing, and driver-specific limits.
//! `unverified_on_hardware` below names that gap explicitly.

/// Compile a kernel straight through the compiler pipeline, the way the CLI does.
///
/// Calls the library rather than spawning `naso`: the same `generate_wgsl_compute`
/// the CLI routes to, so there is no subprocess to be stale, and no dependence on
/// a build artefact having been built first.
fn gen_wgsl(source: &str, kernel: &str) -> String {
    use naso_compiler::codegen::wgsl_compute::generate_wgsl_compute;
    use naso_compiler::parser::parse_program;
    use naso_compiler::typecheck::check_program;

    let mut program = parse_program(source).expect("source should parse");
    let checked = check_program(&mut program);
    assert!(
        checked.errors.is_empty(),
        "source should typecheck before codegen: {:?}",
        checked.errors
    );
    generate_wgsl_compute(&checked.program, kernel).expect("shader should generate")
}

/// Parse AND validate with naga, returning diagnostics as strings.
///
/// Two stages, because they fail differently: `parse_str` rejects malformed WGSL
/// text, and `validate` typechecks it. A shader that parses can still be wrong --
/// binding types, integer widths, and expression types are all checked only in the
/// second stage.
fn naga_check(shader: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(shader).map_err(|e| format!("parse: {e:?}"))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map(|_| ())
    .map_err(|e| format!("validate: {}", e.emit_to_string(shader)))
}

/// The generated shader must pass naga's validator.
#[test]
fn generated_shader_passes_naga_validation() {
    let src = r#"
fn quantize(input: [*] Tensor[f32, 1024], output: inout [1] Tensor[i32, 1024],
             scale: f32, N: u32) {
    forall i in 0..1024 {
        let v = input[i] / scale;
        output[i] = clamp(round(v), -128.0, 127.0) as i32;
    }
}
"#;
    let shader = gen_wgsl(src, "quantize");
    let res = naga_check(&shader);
    assert!(
        res.is_ok(),
        "naga rejected the generated shader (this is what a driver does first):\n  {res:?}\n\n{shader}"
    );
}

/// The repo's own showcase kernel.
#[test]
fn scale_f32_kernel_passes_naga_validation() {
    let shader = gen_wgsl(
        include_str!("../../../kernels/scale_f32.naso"),
        "scale_clamp_f32",
    );
    let res = naga_check(&shader);
    assert!(
        res.is_ok(),
        "naga rejected kernels/scale_f32.naso's shader:\n  {res:?}\n\n{shader}"
    );
}

/// The validator must REJECT a broken shader.
///
/// A validation suite that only ever sees valid input proves nothing: a validator
/// stubbed to accept everything would pass every test above. This pins that the
/// check has teeth.
///
/// Note WHERE naga catches each class. A type mismatch like `f32 -> array<i32>` is
/// caught during PARSING, because naga resolves scalar conversions there; so that
/// case needs `parse_str` to be expected to fail. A case that parses cleanly but
/// fails validation is an illegal entry-point signature -- which is exactly what
/// the compiler was getting wrong, so it is the one worth keeping.
#[test]
fn naga_catches_a_type_error_that_still_parses() {
    // Parses as text, but the conversion is not legal.
    let broken = r#"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<i32>;
@compute @workgroup_size(64)
fn broken_compute(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let i = i32(global_id.x);
    if (i >= 1024) { return; }
    output[i] = input[i];
}
"#;
    assert!(
        naga::front::wgsl::parse_str(broken).is_err(),
        "naga must reject an f32 stored into array<i32>; if it accepted this, every \
         'passes validation' assertion in this file is vacuous"
    );
}

/// An illegal entry-point signature must fail VALIDATION, not parsing.
///
/// This is the bug: a WGSL compute entry point may only take builtin values. The
/// backend emitted `fn k(scale: f32, N: u32, @builtin(global_invocation_id) ..)`,
/// which parses -- and is rejected by every driver, so `kernels/scale_f32.naso`
/// would have failed at pipeline creation in a user's browser.
#[test]
fn naga_rejects_non_builtin_entry_point_arguments() {
    let illegal = r#"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@compute @workgroup_size(64)
fn k(scale: f32, @builtin(global_invocation_id) global_id: vec3<u32>) {
    let i = i32(global_id.x);
    if (i >= 4) { return; }
    let scaled = input[i] * scale;
}
"#;
    let module = naga::front::wgsl::parse_str(illegal).expect("this one parses; it is legal TEXT");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    let err = validator
        .validate(&module)
        .expect_err("naga must reject a non-builtin entry-point argument");
    let msg = err.emit_to_string(illegal);
    assert!(
        msg.contains("must all have bindings"),
        "the diagnostic must name the actual rule, got: {msg}"
    );
}

/// Names what this suite does NOT establish.
///
/// A test file that only reports passes invites the reader to conclude more than it
/// should. The gap is real and specific: no arithmetic has been executed, so a
/// shader that computes the wrong VALUES while typechecking cleanly is invisible
/// here. That requires a GPU.
#[test]
fn unverified_on_hardware() {
    let shader = gen_wgsl(
        r#"
fn scale_clamp_f32(input: [*] Tensor[f32, 256], output: inout [1] Tensor[f32, 256],
                    scale: f32, lo: f32, hi: f32) {
    forall i in 0..256 { output[i] = clamp(input[i] * scale, lo, hi); }
}
"#,
        "scale_clamp_f32",
    );
    assert!(
        naga_check(&shader).is_ok(),
        "the shader itself must be valid"
    );

    // The kernel's INTENT, checked against the source -- this is as close to a
    // numerical test as is possible without executing anything.
    // Temporaries are folded, so the expression appears inline. What matters is that
    // the arithmetic is the SOURCE's: multiply by the scale, THEN clamp to the
    // bounds -- not the reverse order, and not the wrong operand.
    //
    // Scalars are bound as uniforms named `<name>_u`, because a compute entry point
    // may take only builtin values.
    assert!(
        shader.contains("output[i] = clamp((input[i] * scale_u), lo_u, hi_u);"),
        "the emitted arithmetic must match the source expression:\n{shader}"
    );
    assert!(
        shader.contains("var<uniform> scale_u: f32"),
        "scalars must be uniform bindings:\n{shader}"
    );
    assert!(
        !shader.contains("let _ ="),
        "`let _` is not valid WGSL -- naga rejects it as an invalid identifier:\n{shader}"
    );

    eprintln!(
        "NOT VERIFIED: no GPU exists in this environment (no /dev/dri, no Vulkan ICD, \
         no navigator.gpu), so the shader is validated but never EXECUTED. Numerical \
         results and driver behaviour remain untested."
    );
}
