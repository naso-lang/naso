//! Tests for the WGSL bridge: the browser's path from Naso source to a shader.
//!
//! `compile_naso_wasm` stopped at PIR, so a page could parse and typecheck Naso but
//! could not obtain a shader. That gap is why the shipped client carried
//! hand-written WGSL: nothing in the pipeline could produce the real thing, so the
//! duplicate was the only option available.
//!
//! These assert the bridge emits a shader AND an ABI that describe the same buffer
//! layout. A host that allocates from the ABI and runs the shader reads the wrong
//! memory otherwise -- and the shader still compiles, so nothing downstream catches
//! it.

use nasoc_wasm::compile_naso_wgsl;

const SRC: &str = r#"
fn quantize(input: [*] Tensor[f32, 1024], output: inout [1] Tensor[i32, 1024],
             scale: f32, N: u32) {
    forall i in 0..1024 {
        let v = input[i] / scale;
        output[i] = clamp(round(v), -128.0, 127.0) as i32;
    }
}
"#;

/// A real kernel produces a shader and its ABI.
#[test]
fn wgsl_bridge_emits_a_shader_and_an_abi() {
    let r = compile_naso_wgsl(SRC, "quantize");
    assert!(r.success(), "expected success, got {:?}", r.diagnostics());
    let w = r.wgsl().expect("shader text");

    assert!(w.contains("@compute"), "no compute attribute:\n{w}");
    assert!(
        w.contains("fn quantize_compute("),
        "wrong entry point:\n{w}"
    );
    assert_eq!(r.entry_point(), "quantize_compute");
    assert_eq!(r.workgroup_size(), 64);
    // 1024 elements at 64 per workgroup.
    assert_eq!(r.dispatch_groups(), 16);

    assert!(r.diagnostics().is_empty(), "unexpected diagnostics");
}

/// The ABI's bindings appear verbatim in the shader.
///
/// This is the cross-check that makes the ABI usable: the host allocates from the
/// ABI, so every `(index, name, access, elem)` it is told about must be what the
/// shader actually declares.
#[test]
fn abi_bindings_appear_verbatim_in_the_shader() {
    let r = compile_naso_wgsl(SRC, "quantize");
    assert!(r.success());
    let w = r.wgsl().expect("shader");
    let bindings = r.bindings();

    assert_eq!(bindings.len(), 2, "two tensor parameters: {bindings:?}");
    for raw in &bindings {
        let b: serde_json::Value =
            serde_json::from_str(raw).expect("each binding is a JSON object");
        let expected = format!(
            "@group(0) @binding({}) var<storage, {}> {}: array<{}>;",
            b["index"].as_u64().unwrap(),
            b["access"].as_str().unwrap(),
            b["name"].as_str().unwrap(),
            b["elem"].as_str().unwrap(),
        );
        assert!(
            w.contains(&expected),
            "ABI binding {} must appear in the shader.\n  expected: {expected}\n  shader:\n{w}",
            b
        );
    }
}

/// Buffer sizes are consistent with the extent and element width.
#[test]
fn abi_reports_buffer_sizes() {
    let r = compile_naso_wgsl(SRC, "quantize");
    assert!(r.success());
    let b0: serde_json::Value = serde_json::from_str(&r.bindings()[0]).unwrap();
    let b1: serde_json::Value = serde_json::from_str(&r.bindings()[1]).unwrap();

    // 1024 elements x 4 bytes for both f32 and i32.
    assert_eq!(b0["bytes"], 4096);
    assert_eq!(b1["bytes"], 4096);
    assert_eq!(b0["access"], "read");
    assert_eq!(b1["access"], "read_write");
}

/// Scalars are reported as entry-point arguments, in order.
#[test]
fn abi_reports_scalar_arguments() {
    let r = compile_naso_wgsl(SRC, "quantize");
    assert!(r.success());
    assert_eq!(r.scalars(), vec!["scale: f32", "N: u32"]);

    let w = r.wgsl().unwrap();
    let sig = w
        .lines()
        .find(|l| l.starts_with("fn "))
        .expect("entry point");
    for s in r.scalars() {
        assert!(
            sig.contains(&s),
            "scalar `{s}` missing from signature: {sig}"
        );
    }
}

/// `toJson` describes the same result the getters return.
///
/// It exists so the JS side can take the whole result in one call; if it were built
/// from separate fields it could quietly disagree with the getters.
#[test]
fn to_json_agrees_with_the_getters() {
    let r = compile_naso_wgsl(SRC, "quantize");
    assert!(r.success());
    let v: serde_json::Value = serde_json::from_str(&r.to_json()).expect("json");

    assert_eq!(v["success"], r.success());
    assert_eq!(v["wgsl"], r.wgsl().unwrap());
    assert_eq!(v["entryPoint"], r.entry_point());
    assert_eq!(v["workgroupSize"], r.workgroup_size());
    assert_eq!(v["dispatchGroups"], r.dispatch_groups());
    assert_eq!(v["bindings"].as_array().unwrap().len(), r.bindings().len());
    assert_eq!(v["scalars"].as_array().unwrap().len(), r.scalars().len());
}

/// A program that does not typecheck yields no shader.
///
/// The prover work already showed the hazard: a stage that runs on parsed-but-not-
/// typechecked input will happily report success on a program that cannot compile.
/// The browser must never receive a shader for a program the compiler rejects.
#[test]
fn a_program_that_does_not_typecheck_yields_no_shader() {
    let bad = "fn f() { let x: f32 = \"not a float\"; }";
    let r = compile_naso_wgsl(bad, "f");
    assert!(!r.success(), "must not succeed");
    assert!(
        r.wgsl().is_none(),
        "a shader was emitted for a program that does not typecheck:\n{:?}",
        r.wgsl()
    );
    assert!(r.bindings().is_empty(), "no ABI for a failed compile");
    assert!(!r.diagnostics().is_empty(), "must explain the failure");
}

/// Malformed input is a parse ERROR, not a trap.
///
/// This was the gap. The parser had ~90 `panic!` / `unreachable!` sites for input it
/// did not expect, and in WebAssembly an unwind RAISES A TRAP that `catch_unwind`
/// cannot intercept -- verified directly, a wasm32 binary whose whole body is
/// `catch_unwind(|| panic!())` compiles cleanly and then throws
/// `RuntimeError: unreachable`. So a user typing `fn f( {` into a playground lost
/// the entire compiler module, not just the error message.
///
/// A `catch_unwind` guard was written here first. It passed `cargo test` on x86 and
/// did nothing in a browser, so it was removed rather than shipped.
///
/// The parser now records a `ParseError` and returns `Err`. Asserted here through
/// the bridge because that is the path a browser takes, and a native `#[test]`
/// cannot reproduce wasm's behaviour.
#[test]
fn malformed_input_is_a_parse_error_not_a_trap() {
    for src in [
        "fn f( {",
        "fn",
        "fn f() {",
        "fn f() { let",
        "fn f() { for i in }",
        "fn f(x: ) {}",
        "struct",
        "}}}",
        "@@@",
    ] {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| compile_naso_wgsl(src, "f"));
        std::panic::set_hook(prev);

        assert!(
            outcome.is_ok(),
            "`{src}` must not unwind: an unwind is a trap in wasm, which takes the \
             whole compiler module with it"
        );
        let r = outcome.unwrap();
        assert!(!r.success(), "`{src}` must report failure");
        assert!(r.wgsl().is_none(), "`{src}` must not produce a shader");
        let msgs: Vec<String> = r
            .diagnostics()
            .iter()
            .map(|d| d.message().to_string())
            .collect();
        assert!(
            msgs.iter().any(|m| m.contains("arse error")),
            "`{src}` must be reported as a parse error, got: {msgs:?}"
        );
    }
}

/// An unknown kernel is an error with a shader-free result.
#[test]
fn an_unknown_kernel_fails_with_a_reason() {
    let r = compile_naso_wgsl(SRC, "no_such_kernel");
    assert!(!r.success());
    assert!(r.wgsl().is_none());
    let msgs: Vec<String> = r
        .diagnostics()
        .iter()
        .map(|d| d.message().to_string())
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("no function named")),
        "the diagnostic must name the problem: {msgs:?}"
    );
}

/// A kernel the backend refuses is refused here too, not emitted partially.
#[test]
fn a_refused_kernel_produces_no_shader() {
    // A scalar function has no bindings, so there is nothing to dispatch over.
    let src = "fn scalar_only(x: f32) -> f32 { return x + 1.0; }";
    let r = compile_naso_wgsl(src, "scalar_only");
    assert!(!r.success());
    assert!(r.wgsl().is_none());
}

/// A narrow-integer tensor is refused, not silently widened to i32.
///
/// The same wrong-answer bug the compute backend was written to prevent: binding
/// `i8` as `array<i32>` computes something the source did not say.
#[test]
fn a_narrow_integer_tensor_is_refused_not_widened() {
    let src = r#"
fn narrow(a: [*] Tensor[i8, 64]) {
    forall i in 0..64 { let v = a[i]; let _ = v; }
}
"#;
    let r = compile_naso_wgsl(src, "narrow");
    assert!(!r.success(), "i8 storage must be refused");
    assert!(r.wgsl().is_none());
    let msgs: Vec<String> = r
        .diagnostics()
        .iter()
        .map(|d| d.message().to_string())
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("width 8")),
        "the diagnostic must say why: {msgs:?}"
    );
}

/// A scalar `inout` tensor is writable, so the host must map it READ_WRITE.
#[test]
fn a_mutable_tensor_is_reported_read_write() {
    let src = r#"
// `mut`, not `inout`: `inout [1]` demands unique ownership and so rejects the
    // two indexing operations below as a double use, while `inout [*]` is rejected
    // outright for not being [1]. `mut` is the local-binding spelling, and the
    // backend maps it to the same `read_write` storage access.
    fn touch(a: mut Tensor[f32, 64]) {
    forall i in 0..64 { a[i] = a[i] + 1.0; }
}
"#;
    let r = compile_naso_wgsl(src, "touch");
    assert!(r.success(), "{:?}", r.diagnostics());
    let b: serde_json::Value = serde_json::from_str(&r.bindings()[0]).unwrap();
    assert_eq!(b["access"], "read_write");
    assert_eq!(b["bytes"], 256);
}
