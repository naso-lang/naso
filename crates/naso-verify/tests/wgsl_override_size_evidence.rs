//! The evidence behind the symbolic-extent refusal.
//!
//! `compiler/src/codegen/wgsl_compute.rs` refuses a symbolic tensor extent, and its comment
//! claims the obvious workaround -- a WGSL `override` sizing the binding -- does not work.
//! This file is that claim, executed, so the comment cannot rot into folklore.
//!
//! # NOT GPU execution
//!
//! naga is a validator. Passing here means "this is well-formed WGSL", NOT "this computes
//! the right answer on a GPU". There is no `/dev/dri`, no Vulkan ICD, no usable software
//! driver and no browser `navigator.gpu` in this environment. Nothing below runs a shader,
//! and a green result here must never be reported as GPU execution.

/// Parse and validate WGSL the way the project's own shader checks do.
fn validate(label: &str, wgsl: &str) -> Result<(), String> {
    let module = match naga::front::wgsl::parse_str(wgsl) {
        Ok(m) => m,
        Err(e) => return Err(format!("{label}: parse failed: {e}")),
    };
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator
        .validate(&module)
        .map(|_| ())
        .map_err(|e| format!("{label}: validation failed: {}", e.emit_to_string(wgsl)))
}

/// The control: a constant-sized binding array must validate.
///
/// Without this, the rejection test below could pass for the wrong reason -- if constant
/// arrays did not validate either, then the "override does not help" conclusion would be
/// true but uninformative.
#[test]
fn a_constant_sized_binding_array_validates() {
    validate(
        "constant array",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32, 8>;
"#,
    )
    .unwrap_or_else(|e| panic!("a constant array size must be valid WGSL: {e}"));
}

/// THE CLAIM: a binding array sized by an `override` is invalid WGSL.
///
/// This is why the symbolic extent cannot be fixed with `override N: u32; array<f32, N>`.
/// The validator rejects it, and this test is what the comment in `wgsl_compute.rs`
/// refers to. If a future naga version accepts this, the test fails and the refusal has to
/// be reconsidered -- which is the correct outcome, since it would mean the workaround
/// became available.
#[test]
fn an_override_sized_binding_array_is_invalid() {
    let result = validate(
        "override-sized array",
        r#"
override N: u32 = 8u;
@group(0) @binding(0) var<storage, read_write> out: array<f32, N>;
"#,
    );
    assert!(
        result.is_err(),
        "an `override`-sized storage array now VALIDATES. If naga changed, then the \\
         symbolic-extent refusal in wgsl_compute.rs no longer needs its stated reason and \\
         the feature may be implementable via an override. Update the comment and revisit."
    );
    // Name the actual diagnostic in the failure output, so a change here is legible.
    if let Err(e) = &result {
        assert!(
            e.contains("invalid"),
            "expected an 'invalid' diagnostic naming the binding, got: {e}"
        );
    }
}

/// An override IS valid as an index -- and that is a different thing.
///
/// Worth pinning because it is the near-miss: someone who sees "override works as an index"
/// might conclude overrides solve the binding-size problem. They do not. This test records
/// both halves so the distinction is explicit.
#[test]
fn an_override_is_valid_as_an_index_but_does_not_size_the_binding() {
    validate(
        "override as index",
        r#"
override N: u32 = 8u;
@group(0) @binding(0) var<storage, read_write> out: array<f32>;
@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  out[gid.x] = f32(gid.x) * 2.0 + f32(N);
}
"#,
    )
    .unwrap_or_else(|e| panic!("an override used as an index is valid WGSL: {e}"));
}
