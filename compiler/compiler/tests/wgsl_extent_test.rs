//! What the WGSL backends do with extents, and why the symbolic case is refused rather
//! than approximated.
//!
//! # The finding
//!
//! The compute WGSL backend refuses a symbolic tensor extent. The obvious fix is a WGSL
//! `override` constant sizing the binding, and it does not work:
//!
//! ```wgsl
//! override N: u32 = 8u;
//! @group(0) @binding(0) var<storage, read_write> out: array<f32, N>;
//! ```
//!
//! Fed to naga 25.0.1 -- the same validator this project's shaders are checked with --
//! that fails validation with `'out' is invalid`. A storage binding's array size must be
//! a compile-time constant in WGSL. An override used as an *index* is valid, but that is a
//! different thing and gives the binding no size.
//!
//! So supporting symbolic extents means monomorphising one shader per extent, or binding a
//! runtime-sized array and passing the length as an override -- which changes the ABI,
//! because the host must then supply it. Both are design decisions, not codegen tweaks.
//! The refusal is correct, and this file pins it so it stays correct for the stated reason.
//!
//! # Honesty constraints
//!
//! - `naga` validation is NOT GPU execution. There is no `/dev/dri`, no Vulkan ICD and no
//!   `navigator.gpu` in this environment. Nothing here runs a shader.
//! - The straight-line backend refuses ALL tensor parameters, not just symbolic ones. That
//!   is a separate, deliberate limitation (see `wgsl_type`), covered here so the two are
//!   not conflated.

#![cfg(feature = "llvm")]

/// Typecheck a program, panicking with the diagnostic if it does not compile.
fn typechecked(src: &str) -> naso_compiler::ast::Program {
    let mut program =
        naso_compiler::parser::parse_program(src).unwrap_or_else(|e| panic!("must parse: {e}"));
    let checked = naso_compiler::typecheck::check_program(&mut program);
    assert!(
        checked.errors.is_empty(),
        "source must typecheck: {:?}",
        checked.errors.first().map(|e| e.to_string())
    );
    checked.program
}

/// Generate a compute shader, or return the refusal message.
fn compute(src: &str, kernel: &str) -> Result<String, String> {
    let program = typechecked(src);
    naso_compiler::codegen::wgsl_compute::generate_wgsl_compute(&program, kernel)
        .map_err(|e| e.to_string())
}

/// Generate a straight-line shader, or return the refusal message.
fn straight(src: &str) -> Result<String, String> {
    let program = typechecked(src);
    naso_compiler::codegen::wgsl_straight::generate_wgsl_straight_line(&program)
        .map_err(|e| e.to_string())
}

/// A literal extent is the supported case, and must keep working.
#[test]
fn a_literal_extent_still_emits_a_compute_kernel() {
    let shader = compute("fn compute(a: Tensor[f32, 8]) { }", "compute")
        .expect("a literal extent must still emit");
    assert!(
        shader.contains("@compute"),
        "the emitted shader must be a compute entry point.\n{shader}"
    );
    assert!(
        shader.contains("array<f32, 8>") || shader.contains("array<f32>"),
        "the emitted shader must declare its storage array.\n{shader}"
    );
}

/// A symbolic extent must be refused, and the message must give the REASON.
///
/// The reason matters here more than usual: someone reading "not implemented" would
/// reasonably try `override`, which does not work. The message has to say why the obvious
/// approach is unavailable, or the next person repeats the investigation.
#[test]
fn a_symbolic_extent_is_refused_and_explains_why_override_does_not_help() {
    let msg = compute("fn compute(a: Tensor[f32, N]) { }", "compute")
        .expect_err("a symbolic extent must be refused");

    let lower = msg.to_lowercase();

    // Asserted in PHRASES, not single words.
    //
    // A mutation that replaced "is not a compile-time constant" with "is unavailable"
    // survived an earlier version of this test that only checked for the word "constant",
    // because the surrounding sentence still contained it. A single-word assertion over a
    // long diagnostic cannot survive an edit to the part that carries the meaning.
    for phrase in ["compile-time constant", "naga validation"] {
        assert!(
            lower.contains(phrase),
            "the refusal must contain the phrase `{phrase}`: {msg}"
        );
    }
    // The two routes that would genuinely work. Without this, the reader is sent to try
    // `override` and discovers for themselves that it does not validate.
    assert!(
        lower.contains("override") && lower.contains("monomorph"),
        "the refusal must name both real routes -- monomorphisation and an \
         override-sized ABI -- so nobody re-derives why the obvious fix is unavailable: {msg}"
    );
}

/// The straight-line backend refuses tensors entirely -- a DIFFERENT limitation.
///
/// It has no WGSL type for a tensor in a parameter position at all, which is about buffer
/// layout rather than about extents. Asserted separately so the two refusals are not
/// conflated, and so a fix to one is not assumed to fix the other.
#[test]
fn the_straight_line_backend_refuses_tensors_regardless_of_extent() {
    for (tag, src) in [
        ("literal", "fn f(x: Tensor[f32, 4]) -> f32 { x[0] }"),
        ("symbolic", "fn f(x: Tensor[f32, N]) -> f32 { x[0] }"),
    ] {
        let msg = straight(src).expect_err("straight-line WGSL has no tensor parameter type");
        assert!(
            msg.to_lowercase().contains("tensor"),
            "the {tag} refusal must name the tensor: {msg}"
        );
    }
}

/// A multi-dimensional tensor is refused for a different reason again.
///
/// The compute backend models `Tensor[Elem, N]` only, so 2-D hits the arity check before
/// the extent check ever runs. Asserted so the diagnostic cannot drift to blaming the
/// extent when the real cause is the rank.
#[test]
fn a_two_dimensional_tensor_is_refused_for_its_rank_not_its_extent() {
    let msg = compute("fn compute(a: Tensor[f32, 2, 3]) { }", "compute")
        .expect_err("2-D tensors are not modelled by the compute backend");
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("3") || lower.contains("argument"),
        "the refusal must point at the type ARITY, since that is the actual cause: {msg}"
    );
}

/// A symbolic extent must not produce a shader that merely *looks* right.
///
/// The failure mode this guards is emitting `array<f32, N>` into the output with a bare
/// `N` the driver cannot resolve. The test asserts no shader comes back at all.
#[test]
fn no_shader_is_produced_for_a_symbolic_extent() {
    if let Ok(shader) = compute("fn compute(a: Tensor[f32, N]) { }", "compute") {
        panic!(
            "a symbolic extent produced a shader. If it contains an unresolved identifier \
             in an array size, it will fail on a real driver with nothing pointing back \
             here.\n{shader}"
        );
    }
}
