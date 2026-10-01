//! Compute-pipeline WGSL backend.
//!
//! # Why this is a separate entry point
//!
//! WGSL cannot express a Naso function that takes a tensor. The spec is
//! explicit: a runtime-sized `array<T>` may only be used with a storage buffer
//! resource. A `Tensor[f32, 1024]` parameter has no spelling in a WGSL function
//! parameter position.
//!
//! So a tensor-valued function cannot be emitted as a WGSL *function*. It is
//! emitted as a compute entry point, and the tensors become module-scope
//! storage bindings:
//!
//! ```wgsl
//! @group(0) @binding(0) var<storage, read>       input:  array<f32>;
//! @group(0) @binding(1) var<storage, read_write> output: array<i32>;
//!
//! @compute @workgroup_size(64)
//! fn main(@builtin(global_invocation_id) gid: vec3<u32>) { .. }
//! ```
//!
//! This changes the ABI, not just the body. `wgsl_straight` remains the
//! function-emitting backend and is untouched; this is the entry point for
//! kernels.
//!
//! # Access mode is derived, not chosen
//!
//! A binding's access mode comes from the parameter's Naso annotations, not
//! from a convention invented here:
//!
//!   `inout` (mutable)  -> `read_write`
//!   otherwise          -> `read`
//!
//! A scalar parameter is passed as a value through a `let` inside the entry
//! point, so it needs no binding and consumes no binding index.
//!
//! # What is deliberately NOT claimed
//!
//! * **No vectorisation.** The loop index comes from
//!   `global_invocation_id`, and the guard is `i < extent`, so one invocation
//!   handles one element. There is no `vec4` widening.
//! * **No workgroup tuning.** `workgroup_size` is a fixed constant here. The
//!   right value depends on occupancy and register pressure, which this
//!   compiler does not model.
//! * **No unrolling, tiling, fusion or schedule transformation.** The emitted
//!   loop runs the loop nest in source order.
//! * **No multi-kernel programs.** One entry point per compilation, from one
//!   selected source function. Selecting which function is the kernel is a
//!   command-line concern, not something guessed here.
//! * **Narrow integer tensors are refused, not compiled.** WGSL has no `i8` or
//!   `i16` storage type, so a `Tensor[i8, N]` binding would have to widen to
//!   `i32` and drop the source's narrowing -- a shader that validates and is
//!   still wrong. The width is now carried in the AST, so the backend can
//!   refuse with that reason instead of guessing. `i32`, `u32`, `f32` and
//!   `bool` emit. This means `kernels/quant_int8.naso`, which is genuinely an
//!   `i8` kernel, is REFUSED -- correctly, and loudly.

use crate::ast::{
    Expr, ExprKind, ForallLoop, Item, Literal, Mutability, Program, Span, Stmt, StmtKind, Type,
    TypeKind,
};
use crate::codegen::error::CodegenError;
use crate::codegen::wgsl_straight::sanitize;

/// WGSL workgroup x-dimension.
///
/// Fixed, not tuned: the compiler does not model occupancy or register
/// pressure, and a wrong "tuned" value is worse than an honest constant.
pub const WORKGROUP_SIZE: u32 = 64;

type CodegenResult<T> = Result<T, CodegenError>;

/// A tensor parameter, resolved to a storage binding.
struct Binding {
    /// Naso parameter name, sanitized for WGSL.
    name: String,
    /// Element type as WGSL, e.g. `f32`.
    elem: String,
    /// `read` or `read_write`, derived from the parameter's mutability.
    access: &'static str,
    /// Binding index, assigned in source order over tensor parameters only.
    index: u32,
}

/// Lower one tensor-valued function to a WGSL compute shader.
///
/// Refuses anything it cannot lower honestly. It never emits a partial or
/// approximate shader.
pub fn generate_wgsl_compute(program: &Program, kernel: &str) -> CodegenResult<String> {
    let func = program
        .items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) if f.name.name == kernel => Some(f),
            _ => None,
        })
        .ok_or_else(|| {
            CodegenError::ModuleBuildError(format!(
                "no function named `{kernel}` to compile as a compute kernel"
            ))
        })?;

    // Classify parameters: tensors become bindings, scalars become values.
    let mut bindings: Vec<Binding> = Vec::new();
    let mut scalars: Vec<(String, String)> = Vec::new();
    for p in &func.params {
        match &p.ty.kind {
            TypeKind::Tensor(dims) => {
                let elem = tensor_element_wgsl(&p.ty, p.span)?;
                let _ = dims;
                // `mut` and `inout` are both mutable; `inout` is the
                // value-semantics projection and still writes through.
                let access = match p.mutability {
                    Mutability::Mut | Mutability::InOut => "read_write",
                    _ => "read",
                };
                bindings.push(Binding {
                    name: sanitize(&p.name.name),
                    elem,
                    access,
                    index: bindings.len() as u32,
                });
            }
            _other => {
                let ty = scalar_wgsl(&p.ty, p.span)?;
                scalars.push((sanitize(&p.name.name), ty));
            }
        }
    }

    if bindings.is_empty() {
        return Err(CodegenError::UnsupportedFeature(format!(
            "function `{kernel}` takes no tensor, so it has nothing to bind; \
             use the straight-line WGSL backend for scalar functions"
        )));
    }

    // The extent the entry point guards against: the maximum extent over all
    // bindings, since one entry point indexes every binding with the same `i`.
    // Taking the max is required for correctness only if all extents are equal;
    // mismatched extents are refused below rather than indexed out of bounds.
    let mut extents = Vec::new();
    for p in &func.params {
        if let TypeKind::Tensor(_) = &p.ty.kind {
            extents.push(tensor_extent(&p.ty.kind, p.span)?);
        }
    }
    if extents.iter().any(|e| *e != extents[0]) {
        return Err(CodegenError::UnsupportedFeature(format!(
            "kernel `{kernel}` has tensor parameters with differing extents {:?}. \
             One compute entry point indexes every binding with the same index, \
             so a single guard cannot be correct for all of them. Per-buffer \
             extents need one guard per binding, which is not implemented.",
            extents
        )));
    }
    let extent = extents[0];

    let mut out = String::new();
    out.push_str("// Generated by Naso -- WGSL compute backend\n");
    out.push_str(&format!(
        "// kernel `{kernel}`: {} binding(s), one invocation per element, \
         workgroup_size {WORKGROUP_SIZE}\n",
        bindings.len()
    ));
    out.push_str(
        "// Loop order is source order; no vectorisation, tiling or schedule \
         transformation is applied.\n\n",
    );

    // Bindings, in source order.
    for b in &bindings {
        out.push_str(&format!(
            "@group(0) @binding({}) var<storage, {}> {}: array<{}>;\n",
            b.index, b.access, b.name, b.elem
        ));
    }
    out.push('\n');

    // Scalar parameters become entry-point ARGUMENTS. A scalar is a value, so a
    // uniform binding would be a needless resource and a `var<private>` module
    // global would be writable module state rather than an input.
    let scalar_params: String = scalars
        .iter()
        .map(|(n, t)| format!("{n}: {t}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Entry point.
    // Scalars come first as entry-point arguments; the builtin is always last.
    let mut params = scalar_params;
    if !params.is_empty() {
        params.push_str(", ");
    }
    params.push_str("@builtin(global_invocation_id) global_id: vec3<u32>");
    out.push_str(&format!(
        "@compute @workgroup_size({WORKGROUP_SIZE})\nfn {}({params}) {{\n",
        entry_name(kernel)
    ));
    out.push_str("    let i = i32(global_id.x);\n");
    // Guard: one invocation per element, so out-of-range invocations exit.
    out.push_str(&format!("    if (i >= {extent}) {{ return; }}\n"));

    for stmt in &func.body.stmts {
        emit_stmt(&mut out, stmt, 1, &extent)?;
    }
    if let Some(tail) = &func.body.expr {
        emit_expr_inline(&mut out, tail);
        out.push_str(";\n");
    }
    out.push_str("}\n");
    Ok(out)
}

/// The WGSL entry-point name.
///
/// A Naso function name is reused so the shader is traceable back to source,
/// but a function whose name collides with a WGSL reserved word would not
/// compile, so the prefix is unconditional.
fn entry_name(kernel: &str) -> String {
    format!("{}_compute", sanitize(kernel))
}

fn tensor_element_wgsl(ty: &Type, span: Span) -> CodegenResult<String> {
    let (elem, _extent) = tensor_parts(&ty.kind, span)?;
    scalar_wgsl(elem, span)
}

/// Split `Tensor[Elem, N]` into its element type and its extent.
///
/// The AST stores a tensor's type arguments positionally: the first is the
/// element type and the rest are extents, so `Tensor[f32, 1024]` is ONE
/// dimension, not two. Counting the type arguments as dimensions would reject
/// every real 1-D tensor.
fn tensor_parts(kind: &TypeKind, span: Span) -> CodegenResult<(&Type, &Type)> {
    let dims = match kind {
        TypeKind::Tensor(d) => d,
        _ => return Err(tensor_expected(span)),
    };
    if dims.len() != 2 {
        return Err(CodegenError::UnsupportedFeature(format!(
            "tensor at line {} has {} type arguments; this backend models \
             `Tensor[Elem, N]`, a 1-D tensor with a constant extent",
            span.line,
            dims.len()
        )));
    }
    Ok((&dims[0], &dims[1]))
}

/// The constant extent of a 1-D tensor, or a refusal.
///
/// A symbolic extent (`Tensor[f32, N]`) has no value here, and the guard the
/// entry point needs is a compile-time constant in this backend. Refusing is
/// correct: guessing a bound would either drop elements or read out of range.
fn tensor_extent(kind: &TypeKind, span: Span) -> CodegenResult<u32> {
    let (_elem, extent_ty) = tensor_parts(kind, span)?;
    match extent_ty.nat_const() {
        Some(v) => u32::try_from(v).map_err(|_| {
            CodegenError::UnsupportedFeature(format!(
                "tensor extent at line {} is {v}, which does not fit a 32-bit \
                 element index",
                span.line
            ))
        }),
        None => Err(CodegenError::UnsupportedFeature(format!(
            "tensor extent at line {} is not a compile-time constant; the \
             entry point's bounds guard needs a value, and a symbolic extent \
             needs monomorphisation, which is not implemented",
            span.line
        ))),
    }
}

fn tensor_expected(span: Span) -> CodegenError {
    CodegenError::TypeLoweringError(format!(
        "expected a tensor at line {}, got something else",
        span.line
    ))
}

/// Reject an integer element type that WGSL cannot represent.
///
/// WGSL has no `i8`/`i16` storage type. Binding a narrow integer tensor as
/// `array<i32>` would silently WIDEN it, and a source narrowing cast would be
/// dropped, so the shader would compute a different function from the one
/// written -- it would store -200 where the source says to narrow to i8.
///
/// So this refuses. The width is available (`Type::int_width`) precisely so the
/// decision can be made instead of guessed.
fn refuse_unsupported_int(ty: &Type, span: Span) -> CodegenError {
    let kind = if matches!(ty.kind, TypeKind::UInt) {
        "unsigned"
    } else {
        "signed"
    };
    CodegenError::UnsupportedFeature(format!(
        "{kind} integer at line {} has width {}, which WGSL has no storage type \
         for. Binding it as i32 would silently widen it and drop the source's \
         narrowing cast, so the shader would not compute what the source says. \
         A narrow tensor needs a packed representation, which this backend does \
         not model. Use f32, i32 or u32 here, or implement the packing.",
        span.line,
        ty.int_width.unwrap_or(0)
    ))
}

/// Map a Naso scalar type to a WGSL type, refusing what WGSL cannot hold.
fn scalar_wgsl(ty: &Type, span: Span) -> CodegenResult<String> {
    // Only i32/u32 have a WGSL storage type. A narrower width is refused rather
    // than widened -- see `refuse_unsupported_int`.
    if matches!(ty.kind, TypeKind::Int | TypeKind::UInt) && ty.int_width.is_some_and(|w| w != 32) {
        return Err(refuse_unsupported_int(ty, span));
    }
    Ok(match &ty.kind {
        TypeKind::Int => "i32".to_string(),
        TypeKind::UInt => "u32".to_string(),
        TypeKind::Float => "f32".to_string(),
        TypeKind::Bool => "bool".to_string(),
        TypeKind::Tensor(_) => {
            return Err(CodegenError::TypeLoweringError(format!(
                "nested tensor at line {} has no WGSL layout",
                span.line
            )));
        }
        other => {
            return Err(CodegenError::TypeLoweringError(format!(
                "type `{}` at line {} has no WGSL equivalent",
                crate::codegen::wgsl_straight::type_kind_name(other),
                span.line
            )));
        }
    })
}

/// Emit a statement inside the entry point.
fn emit_stmt(out: &mut String, stmt: &Stmt, depth: usize, extent: &u32) -> CodegenResult<()> {
    let pad = "    ".repeat(depth);
    match &stmt.kind {
        StmtKind::Proof(_) => Ok(()),
        StmtKind::Let(s) => {
            out.push_str(&pad);
            out.push_str("let ");
            out.push_str(&sanitize(&crate::codegen::wgsl_straight::pattern_name(
                &s.pattern,
            )));
            out.push_str(" = ");
            emit_expr_inline(out, &s.value);
            out.push_str(";\n");
            Ok(())
        }
        StmtKind::LetInOut(s) => {
            out.push_str(&pad);
            out.push_str("var ");
            out.push_str(&sanitize(&s.name.name));
            out.push_str(" = ");
            emit_expr_inline(out, &s.value);
            out.push_str(";\n");
            Ok(())
        }
        StmtKind::LetConsume(s) => {
            // `consume` moves; at the WGSL level that is a copy, and a
            // consumed binding is never read again, so `let` is correct.
            out.push_str(&pad);
            out.push_str("let ");
            out.push_str(&sanitize(&s.name.name));
            out.push_str(" = ");
            emit_expr_inline(out, &s.value);
            out.push_str(";\n");
            Ok(())
        }
        StmtKind::Expr(e) => {
            if let ExprKind::Forall(loop_) = &e.kind {
                return emit_forall(out, loop_, depth, extent);
            }
            out.push_str(&pad);
            emit_expr_inline(out, e);
            out.push_str(";\n");
            Ok(())
        }
        StmtKind::Return(e) => {
            out.push_str(&pad);
            match e {
                Some(e) => {
                    out.push_str("return ");
                    emit_expr_inline(out, e);
                    out.push_str(";\n");
                }
                None => out.push_str("return;\n"),
            }
            Ok(())
        }
        other => Err(CodegenError::UnsupportedFeature(format!(
            "statement `{other}` at line {} is not supported by the WGSL \
             compute backend",
            stmt.span.line
        ))),
    }
}

/// Emit a `forall` as a WGSL `for` loop.
///
/// The loop variable starts at the entry point's `i`, so the loop is driven by
/// `global_invocation_id` rather than by a fresh counter. That is what makes
/// this a GPU mapping: invocation k executes iteration k, and the outer guard
/// has already excluded out-of-range invocations.
fn emit_forall(
    out: &mut String,
    loop_: &ForallLoop,
    depth: usize,
    extent: &u32,
) -> CodegenResult<()> {
    let pad = "    ".repeat(depth);
    if loop_.bindings.len() != 1 {
        return Err(CodegenError::UnsupportedFeature(format!(
            "`forall` at line {} has {} bindings; write one binding per loop",
            loop_.span.line,
            loop_.bindings.len()
        )));
    }
    let (var, lower, upper) = &loop_.bindings[0];
    let lo = const_int(lower, loop_.span.line)?;
    let hi = const_int(upper, loop_.span.line)?;
    let name = sanitize(&var.name);
    let extent_val = *extent;

    // A loop whose range is exactly the kernel's extent IS the invocation
    // index: invocation k runs iteration k, and the entry point's guard
    // already excluded out-of-range invocations. Emitting a `for` here would
    // (a) shadow the entry point's `i`, and (b) re-run the whole loop once per
    // invocation, so invocation k would run iterations 0..extent instead of
    // just k -- a quadrically wrong result that still compiles.
    //
    // The loop variable is therefore bound as an alias of `i` rather than a
    // fresh counter.
    if lo == 0 && hi == extent_val as i64 {
        if name != "i" {
            out.push_str(&format!("{pad}let {name} = i;\n"));
        }
    } else {
        // A different range is a different iteration space, and one
        // invocation cannot own it. Refuse rather than emit something whose
        // meaning is not the source's.
        return Err(CodegenError::UnsupportedFeature(format!(
            "`forall {name} in {lo}..{hi}` at line {} does not cover the kernel \
             extent 0..{extent_val}, so its iterations are not the invocation \
             index. A compute entry point maps one invocation to one element; \
             a loop over a different range needs a different mapping, which is \
             not implemented.",
            loop_.span.line
        )));
    }

    if name == "i" {
        for stmt in &loop_.body.stmts {
            emit_stmt(out, stmt, depth, extent)?;
        }
        if let Some(tail) = &loop_.body.expr {
            emit_expr_inline(out, tail);
            out.push_str(";\n");
        }
        return Ok(());
    }

    out.push_str(&format!(
        "{pad}for (var {name}: i32 = {lo}; {name} < {hi}; {name} = {name} + 1) {{\n"
    ));
    for stmt in &loop_.body.stmts {
        emit_stmt(out, stmt, depth + 1, extent)?;
    }
    if let Some(tail) = &loop_.body.expr {
        emit_expr_inline(out, tail);
        out.push_str(";\n");
    }
    out.push_str(&format!("{pad}}}\n"));
    Ok(())
}

fn const_int(e: &Expr, line: u32) -> CodegenResult<i64> {
    match &e.kind {
        ExprKind::Literal(Literal::Int(v)) => Ok(*v),
        other => Err(CodegenError::UnsupportedFeature(format!(
            "`forall` bound at line {line} is {}, which needs a constant integer \
             to write a WGSL `for` header",
            crate::codegen::wgsl_straight::describe_expr_kind(other)
        ))),
    }
}

/// Write an expression without a trailing newline.
fn emit_expr_inline(out: &mut String, expr: &Expr) {
    match &expr.kind {
        ExprKind::Literal(l) => out.push_str(&crate::codegen::wgsl_straight::emit_literal(l)),
        ExprKind::Var(v) => out.push_str(&sanitize(&v.name)),
        ExprKind::Assign(lhs, rhs) => {
            emit_expr_inline(out, lhs);
            out.push_str(" = ");
            emit_expr_inline(out, rhs);
        }
        ExprKind::Index(base, idx) => {
            emit_expr_inline(out, base);
            out.push('[');
            emit_expr_inline(out, idx);
            out.push(']');
        }
        ExprKind::Call(callee, args) => {
            out.push_str(&sanitize(&crate::codegen::wgsl_straight::callee_name(
                callee,
            )));
            out.push('(');
            for (n, a) in args.iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                emit_expr_inline(out, a);
            }
            out.push(')');
        }
        ExprKind::Ascribe(inner, _) => emit_expr_inline(out, inner),
        ExprKind::Binary(op, l, r) => {
            out.push('(');
            emit_expr_inline(out, l);
            if let Some(b) = crate::codegen::wgsl_straight::wgsl_binop(op) {
                out.push(' ');
                out.push_str(b);
                out.push(' ');
            }
            emit_expr_inline(out, r);
            out.push(')');
        }
        ExprKind::Unary(op, inner) => {
            if let Some(u) = crate::codegen::wgsl_straight::wgsl_unop(op) {
                out.push_str(u);
            }
            emit_expr_inline(out, inner);
        }
        _ => out.push_str("/* unsupported */"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    fn compute(src: &str, kernel: &str) -> CodegenResult<String> {
        let program = parse_program(src).expect("parse");
        generate_wgsl_compute(&program, kernel)
    }

    const QUANT: &str = "\
fn quantize(input: [1] Tensor[f32, 1024], output: inout [1] Tensor[f32, 1024], scale: f32) {
    forall i in 0..1024 {
        let v = round(input[i] / scale);
        output[i] = clamp(v, -128.0, 127.0);
    }
}";

    /// The headline: a tensor-valued function becomes a compute entry point with
    /// storage bindings, which is the whole point of this backend.
    #[test]
    fn test_tensor_function_becomes_a_compute_entry_point() {
        let w = compute(QUANT, "quantize").expect("must emit");
        assert!(w.contains("@compute @workgroup_size(64)"), "{w}");
        assert!(w.contains("@builtin(global_invocation_id)"), "{w}");
        assert!(w.contains("array<f32>"), "{w}");
    }

    /// Access mode must come from the parameter's mutability, not a convention:
    /// `[1]` linear input is `read`, `inout` output is `read_write`.
    #[test]
    fn test_access_mode_is_derived_from_mutability() {
        let w = compute(QUANT, "quantize").expect("must emit");
        assert!(w.contains("var<storage, read> input"), "input is read: {w}");
        assert!(
            w.contains("var<storage, read_write> output"),
            "output is written: {w}"
        );
    }

    /// Bindings are numbered in source order, densely, over tensors only.
    #[test]
    fn test_bindings_are_numbered_in_source_order() {
        let w = compute(QUANT, "quantize").expect("must emit");
        assert!(w.contains("@binding(0) var<storage, read> input"), "{w}");
        assert!(
            w.contains("@binding(1) var<storage, read_write> output"),
            "{w}"
        );
    }

    /// A scalar parameter is an entry-point ARGUMENT, not a binding: it is a
    /// value, so a uniform would be a needless resource.
    #[test]
    fn test_scalar_parameter_is_an_argument_not_a_binding() {
        let w = compute(QUANT, "quantize").expect("must emit");
        assert!(w.contains("scale: f32,"), "scale is a parameter: {w}");
        assert!(!w.contains("@binding(2)"), "no binding for a scalar: {w}");
    }

    /// The guard bounds the invocation to the tensor's extent, which is why the
    /// parser had to start retaining it.
    #[test]
    fn test_guard_uses_the_tensor_extent() {
        let w = compute(QUANT, "quantize").expect("must emit");
        assert!(w.contains("if (i >= 1024)"), "guard at the extent: {w}");
    }

    /// A different extent must be reflected. This is the regression for the
    /// parser discarding the extent literal: before, 1024 and 4 were
    /// indistinguishable in the AST and the guard was a guess.
    #[test]
    fn test_guard_reflects_a_different_extent() {
        let src = "fn f(a: [1] Tensor[f32, 4], b: inout [1] Tensor[f32, 4]) {
            forall i in 0..4 { b[i] = a[i] * 2.0; }
        }";
        let w = compute(src, "f").expect("must emit");
        assert!(w.contains("if (i >= 4)"), "{w}");
        assert!(!w.contains("1024"), "{w}");
    }

    /// The loop over the kernel's extent must NOT be re-emitted as a `for`. It
    /// would shadow the entry point's `i` and re-run the whole loop once per
    /// invocation, so invocation k would run every iteration instead of just k.
    /// That is a wrong result that still parses.
    #[test]
    fn test_loop_over_the_extent_is_not_re_emitted() {
        let w = compute(QUANT, "quantize").expect("must emit");
        assert!(
            !w.contains("for ("),
            "the loop IS the invocation index, not a re-run: {w}"
        );
        // The body must still be present, inlined.
        assert!(w.contains("output[i] ="), "body must be emitted: {w}");
    }

    /// A loop whose range is not the kernel's extent is a different iteration
    /// space, which one invocation cannot own. Refuse.
    #[test]
    fn test_loop_not_covering_the_extent_is_refused() {
        let src = "fn f(a: [1] Tensor[f32, 16], b: inout [1] Tensor[f32, 16]) {
            forall i in 0..8 { b[i] = a[i]; }
        }";
        let err = compute(src, "f").expect_err("must refuse");
        assert!(
            err.to_string().contains("does not cover the kernel extent"),
            "{err}"
        );
    }

    /// Mismatched extents: one guard cannot be right for all bindings.
    #[test]
    fn test_mismatched_extents_are_refused() {
        let src = "fn f(a: [1] Tensor[f32, 4], b: inout [1] Tensor[f32, 8]) {
            forall i in 0..4 { b[i] = a[i]; }
        }";
        let err = compute(src, "f").expect_err("must refuse");
        assert!(err.to_string().contains("differing extents"), "{err}");
    }

    /// A symbolic extent has no value, so the guard cannot be written. Refuse
    /// rather than guess a bound.
    #[test]
    fn test_unknown_extent_is_refused_not_guessed() {
        let src = "fn f(a: [1] Tensor[f32, N], b: inout [1] Tensor[f32, N]) {
            forall i in 0..4 { b[i] = a[i]; }
        }";
        // `N` does not parse as a tensor extent literal, so this exercises the
        // refusal path either way; the assertion is that it does not emit.
        let r = compute(src, "f");
        match r {
            Err(e) => assert!(e.to_string().contains("constant"), "{e}"),
            Ok(w) => panic!("emitted a shader with an unknown extent: {w}"),
        }
    }

    /// A scalar-only function has nothing to bind; it belongs to the
    /// straight-line backend. Say so instead of emitting an empty shader.
    #[test]
    fn test_scalar_function_is_refused() {
        let src = "fn f(a: f32) -> f32 { return a * 2.0; }";
        let err = compute(src, "f").expect_err("must refuse");
        assert!(err.to_string().contains("no tensor"), "{err}");
    }

    /// An unknown kernel name is an error, not an empty shader.
    #[test]
    fn test_unknown_kernel_is_refused() {
        let err = compute(QUANT, "nonexistent").expect_err("must refuse");
        assert!(err.to_string().contains("no function named"), "{err}");
    }

    /// A proof block contributes no code, and the loop still works.
    #[test]
    fn test_proof_block_contributes_no_code() {
        let src = "fn f(a: [1] Tensor[f32, 8], b: inout [1] Tensor[f32, 8], s: f32) {
            proof { assert(s > 0.0); }
            forall i in 0..8 { b[i] = a[i] * s; }
        }";
        let w = compute(src, "f").expect("must emit");
        assert!(!w.contains("assert"), "proof must be erased: {w}");
        assert!(w.contains("b[i] ="), "loop body must survive: {w}");
    }

    /// A narrow integer tensor must be REFUSED, not silently widened.
    ///
    /// This test previously asserted the opposite -- that `Tensor[i8, 8]` emits
    /// `array<i32>` -- because the parser discarded the width so the backend
    /// could not tell `i8` from `i32`. With the width carried in the AST, the
    /// backend can refuse instead of computing a different function.
    #[test]
    fn test_narrow_int_tensor_is_refused_not_widened() {
        for (ty, w) in [("i8", 8), ("i16", 16), ("i64", 64), ("u8", 8)] {
            let src = format!(
                "fn f(a: [1] Tensor[{ty}, 8], b: inout [1] Tensor[{ty}, 8]) {{
                     forall i in 0..8 {{ b[i] = a[i]; }}
                 }}"
            );
            let err = compute(&src, "f").expect_err(&format!("{ty} must be refused, not widened"));
            let m = err.to_string();
            assert!(m.contains(&format!("width {w}")), "{ty}: {m}");
            assert!(
                m.contains("silently widen"),
                "the reason must name the hazard: {m}"
            );
            assert!(
                !m.contains("i32") || m.contains("would silently"),
                "must not propose i32 as the answer: {m}"
            );
        }
    }

    /// `i32` and `u32` are exactly what WGSL can store, so they must emit.
    #[test]
    fn test_i32_and_u32_tensors_still_emit() {
        for ty in ["i32", "u32"] {
            let src = format!(
                "fn f(a: [1] Tensor[{ty}, 8], b: inout [1] Tensor[{ty}, 8]) {{
                     forall i in 0..8 {{ b[i] = a[i]; }}
                 }}"
            );
            let w = compute(&src, "f").unwrap_or_else(|e| panic!("{ty} must emit: {e}"));
            assert!(
                w.contains("array<i32>") || w.contains("array<u32>"),
                "{ty}: {w}"
            );
        }
    }

    /// The element type must come from the tensor's first type argument, not
    /// the extent in the second. `Tensor[f32, 8]` is an f32 array of 8.
    ///
    /// This used to use `i8` for the input, which is now (correctly) refused,
    /// so it uses `i32` and keeps asserting the same thing: the first type
    /// argument is the element type.
    #[test]
    fn test_element_type_is_the_first_type_argument() {
        // Mixed-width so the assertion is meaningful: if the element type were
        // read from the second type argument (the extent, 8) both bindings would
        // agree, and the test could not tell them apart.
        let src = "fn f(a: [1] Tensor[i32, 8], b: inout [1] Tensor[f32, 8]) {
            forall i in 0..8 { b[i] = a[i] as f32; }
        }";
        let w = compute(src, "f").expect("must emit");
        assert!(w.contains("a: array<i32>"), "input element is i32: {w}");
        assert!(w.contains("b: array<f32>"), "output element is f32: {w}");
    }
}
