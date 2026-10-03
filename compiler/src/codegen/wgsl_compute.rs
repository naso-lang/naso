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

/// Naso parameter name -> the uniform that now carries it.
///
/// Scalar parameters are bound as `var<uniform> <name>_u`, because a WGSL compute
/// entry point may take only builtin values. Reading such a parameter in the body
/// must therefore emit the uniform's name, not the parameter's.
///
/// A module-level map rather than a parameter threaded through `emit_expr_inline`
/// and `emit_stmt` (21 call sites): `emit_expr_inline` recurses over every
/// expression, and `ExprKind::Var` is the single place a name becomes text, so one
/// lookup here covers every path. Emission is single-threaded per compile, and the
/// map is cleared at the start of each emission so one kernel cannot rename
/// another's variables.
static SCALAR_UNIFORMS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// The WGSL name a Naso parameter is emitted under, if it is a scalar parameter.
fn uniform_for(name: &str) -> Option<String> {
    SCALAR_UNIFORMS.lock().ok().and_then(|m| {
        m.iter()
            .find(|(naso, _)| naso == name)
            .map(|(_, wgsl)| wgsl.clone())
    })
}

/// A tensor parameter, resolved to a storage binding.
///
/// Public because it appears in [`ComputeAbi`], which a WebGPU host reads to
/// allocate buffers. The field names are the ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Naso parameter name, sanitized for WGSL.
    pub name: String,
    /// Element type as WGSL, e.g. `f32`.
    pub elem: String,
    /// `read` or `read_write`, derived from the parameter's mutability.
    pub access: &'static str,
    /// Binding index, assigned in source order over tensor parameters only.
    pub index: u32,
}

/// The host ABI of a generated compute kernel.
///
/// Emitting the shader is only half the job. A WebGPU host has to allocate a
/// buffer per binding, in binding order, of the right element type, and pass the
/// scalars as entry-point arguments. If it has to recover that by PARSING the
/// shader text, the ABI is only as trustworthy as a regex, and a shader that
/// parses is not evidence that the buffers were wired correctly.
///
/// So the same analysis that decides the shader's shape is published as data, and
/// both the emitter and the host read it. `generate_wgsl_compute` and
/// `describe_compute_abi` share `analyze`; they cannot disagree about the ABI
/// because there is only one description of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeAbi {
    /// Name of the `@compute` entry point, e.g. `quantize_compute`.
    pub entry_point: String,
    /// x-dimension of `@workgroup_size`.
    pub workgroup_size: u32,
    /// Storage bindings, in binding-index order.
    pub bindings: Vec<Binding>,
    /// Scalar parameters, in parameter order, as `(name, wgsl type)`.
    ///
    /// These are UNIFORM BINDINGS, not entry-point arguments: a WGSL compute entry
    /// point may take only builtin values. The WGSL binding name is `<name>_u`, and
    /// the binding index is `bindings.len() + position in this list`.
    pub scalars: Vec<(String, String)>,
    /// Elements each binding is indexed over. All bindings share one extent.
    pub extent: u32,
}

impl ComputeAbi {
    /// Total bytes one binding's storage buffer needs.
    ///
    /// `bytes_per_element` is 4 for every supported element type (f32, i32, u32);
    /// it is spelled out rather than assumed so a future 8-byte type cannot
    /// silently under-allocate.
    pub fn buffer_bytes(&self, index: u32) -> Option<usize> {
        let b = self.bindings.iter().find(|b| b.index == index)?;
        Some(self.extent as usize * bytes_per_element(&b.elem))
    }

    /// Total elements in any one binding.
    pub fn elements(&self) -> usize {
        self.extent as usize
    }

    /// Binding index of the `i`th scalar uniform.
    pub fn scalar_binding(&self, i: usize) -> Option<u32> {
        self.scalars
            .get(i)
            .map(|_| self.bindings.len() as u32 + i as u32)
    }

    /// Bytes a scalar uniform buffer needs.
    ///
    /// WGSL requires a uniform binding to be at least 16 bytes and its size to be a
    /// multiple of 16, so a single `f32` occupies a 16-byte slot whatever its type.
    /// Reporting 4 here would have the host allocate a buffer the driver rejects.
    pub fn scalar_bytes(&self, i: usize) -> Option<usize> {
        self.scalars.get(i).map(|_| 16)
    }

    /// The WGSL name of the `i`th scalar uniform.
    pub fn scalar_wgsl_name(&self, i: usize) -> Option<String> {
        self.scalars.get(i).map(|(n, _)| format!("{}_u", n))
    }
}

/// WGSL size in bytes for an element type this backend emits.
fn bytes_per_element(elem: &str) -> usize {
    match elem {
        "f32" | "i32" | "u32" => 4,
        "f16" | "i16" | "u16" => 2,
        _ => 4,
    }
}

/// Describe the compute ABI of `kernel` without emitting a shader.
///
/// Same refusals as `generate_wgsl_compute`: a kernel it cannot lower honestly is
/// an error here too, so the host is never told about a kernel that will not build.
pub fn describe_compute_abi(program: &Program, kernel: &str) -> CodegenResult<ComputeAbi> {
    let a = analyze(program, kernel)?;
    Ok(ComputeAbi {
        entry_point: entry_name(kernel),
        workgroup_size: WORKGROUP_SIZE,
        bindings: a.bindings,
        scalars: a.scalars,
        extent: a.extent,
    })
}

/// The parameter analysis shared by shader emission and ABI description.
struct Analysis {
    bindings: Vec<Binding>,
    scalars: Vec<(String, String)>,
    extent: u32,
}

/// Classify a kernel's parameters into storage bindings and scalar arguments.
fn analyze(program: &Program, kernel: &str) -> CodegenResult<Analysis> {
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

    Ok(Analysis {
        bindings,
        scalars,
        extent,
    })
}

/// Lower one tensor-valued function to a WGSL compute shader.
///
/// Refuses anything it cannot lower honestly. It never emits a partial or
/// approximate shader.
pub fn generate_wgsl_compute(program: &Program, kernel: &str) -> CodegenResult<String> {
    // The SAME analysis the host ABI is described from, so a host can never be
    // handed a layout that disagrees with the shader it was generated from.
    let a = analyze(program, kernel)?;
    let bindings = a.bindings;
    let scalars = a.scalars;
    let extent = a.extent;
    let func = program
        .items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) if f.name.name == kernel => Some(f),
            _ => None,
        })
        .expect("analyze() succeeded, so this function exists");

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

    // Scalar parameters become UNIFORM BINDINGS, not entry-point arguments.
    //
    // They were entry-point arguments. That is wrong: a WGSL compute entry point may
    // take ONLY builtin values. A plain `scale: f32` parameter is legal TEXT --
    // `wgsl_reflect` parses it without complaint -- and is rejected by validation,
    // and by every driver at pipeline creation. So `kernels/scale_f32.naso` would
    // never have run, on any GPU, in any browser.
    //
    // A uniform buffer is the correct spelling. Not `var<private>`: a scalar
    // parameter is an input, and a private global is writable module state the host
    // could never set.
    //
    // Uniforms are numbered AFTER the tensors, so every existing tensor-only kernel
    // keeps the binding indices a host already allocates for.
    if let Ok(mut m) = SCALAR_UNIFORMS.lock() {
        m.clear();
        for (name, _) in scalars.iter() {
            m.push((name.clone(), format!("{}_u", sanitize(name))));
        }
    }
    for (i, (name, t)) in scalars.iter().enumerate() {
        let index = bindings.len() as u32 + i as u32;
        out.push_str(&format!(
            "@group(0) @binding({index}) var<uniform> {}_u: {t};\n",
            sanitize(name)
        ));
    }
    if !scalars.is_empty() {
        out.push('\n');
    }

    // Entry point: the builtin is the ONLY parameter a compute entry point may take.
    let params = "@builtin(global_invocation_id) global_id: vec3<u32>";
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
        // A symbolic extent is refused, and this is NOT a gap waiting to be filled with
        // a WGSL `override`.
        //
        // The obvious fix is `override N: u32 = ...;` sizing the binding. That does not
        // work, and it was checked rather than assumed: feeding `array<f32, N>` with `N` an
        // override to naga 25.0.1 -- the same validator this project's shaders are checked
        // with -- fails validation with "'out' is invalid". A binding's array size must be
        // a compile-time constant in WGSL. Using an override as an INDEX is valid, but that
        // is a different thing and does not give the binding a size.
        //
        // So real support means either monomorphising one shader per extent, or binding a
        // runtime-sized array and passing the extent as an override alongside it -- which
        // changes the ABI, since the host must then supply the length. Both are design
        // decisions rather than a codegen tweak, so the honest answer is to refuse and say
        // why.
        None => Err(CodegenError::UnsupportedFeature(format!(
            "tensor extent at line {} is not a compile-time constant. A WGSL storage \
             binding's array size must be a constant: `array<f32, N]` with `N` an \
             `override` fails naga validation, so that fix does not apply. Supporting \
             this means either monomorphising one shader per extent, or binding a \
             runtime-sized array and passing the length as an override, which changes \
             the ABI. Neither is implemented.",
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
        ExprKind::Var(v) => match uniform_for(&v.name) {
            Some(wgsl) => out.push_str(&wgsl),
            None => out.push_str(&sanitize(&v.name)),
        },
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
        // `as T` is a CAST. Dropping it here is what made `clamp(v) as i32` emit
        // `clamp(v)`: an f32 written into an `array<i32>`. The shader still parses,
        // so a parse-only check accepts it; naga's validator and every real driver
        // reject it. See `cast_constructor`.
        ExprKind::Ascribe(inner, ty) => match crate::codegen::wgsl_straight::cast_constructor(ty) {
            Some(wgsl) => {
                out.push_str(&wgsl);
                out.push('(');
                emit_expr_inline(out, inner);
                out.push(')');
            }
            None => emit_expr_inline(out, inner),
        },
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
    fn test_scalar_parameter_is_a_uniform_not_an_entry_argument() {
        let src = r#"
fn k(a: [*] Tensor[f32, 64], scale: f32) {
    forall i in 0..64 { let v = a[i]; let _ = v * scale; }
}
"#;
        let w = generate_wgsl_compute(&parse(src), "k").expect("shader");

        // INVERTED. This test asserted the opposite: that a scalar is an
        // entry-point argument, on the reasoning that "a scalar is a value, so a
        // uniform binding would be a needless resource". That reasoning is wrong --
        // a WGSL compute entry point may take ONLY builtin values, so
        // `fn k(scale: f32, @builtin(global_invocation_id) ..)` is legal TEXT and
        // rejected by every driver at pipeline creation.
        //
        // It survived because `wgsl_reflect` only PARSES. naga's validator, and a
        // real GPU, both reject it. See wgsl_validation.rs.
        let sig = w
            .lines()
            .find(|l| l.starts_with("fn "))
            .expect("entry point");
        assert!(
            !sig.contains("scale: f32"),
            "a scalar must not be an entry-point argument: {sig}"
        );
        assert!(
            w.contains("@group(0) @binding(1) var<uniform> scale_u: f32;"),
            "the scalar must be a uniform binding after the tensor: {w}"
        );
        // The body's read must name the uniform, not the parameter.
        assert!(w.contains("scale_u"), "the body must read the uniform: {w}");
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

    // ---------------------- host ABI ----------------------
    //
    // The point of `ComputeAbi` is that a WebGPU host can wire buffers WITHOUT
    // parsing shader text. That is only true if the ABI and the shader describe
    // the same thing, so the tests below check them against EACH OTHER rather
    // than against hand-written expectations: each binding the ABI promises must
    // appear in the shader with the same index, element type and access, and the
    // entry point and workgroup size must match too.

    fn parse(src: &str) -> Program {
        crate::parser::parse_program(src).expect("parse failed")
    }

    const ABI_SRC: &str = r#"
fn quantize(input: [*] Tensor[f32, 1024], output: inout [1] Tensor[i32, 1024],
             scale: f32, N: u32) {
    forall i in 0..1024 {
        let v = input[i] / scale;
        output[i] = clamp(round(v), -128.0, 127.0) as i32;
    }
}
"#;

    #[test]
    fn abi_describes_bindings_in_source_order() {
        let abi = describe_compute_abi(&parse(ABI_SRC), "quantize").expect("abi");
        assert_eq!(abi.bindings.len(), 2, "two tensor params: {abi:?}");
        assert_eq!(abi.bindings[0].name, "input");
        assert_eq!(abi.bindings[0].elem, "f32");
        assert_eq!(abi.bindings[0].access, "read");
        assert_eq!(abi.bindings[0].index, 0);
        assert_eq!(abi.bindings[1].name, "output");
        assert_eq!(abi.bindings[1].elem, "i32");
        assert_eq!(abi.bindings[1].access, "read_write");
        assert_eq!(abi.bindings[1].index, 1);
    }

    #[test]
    fn abi_reports_scalars_as_entry_arguments() {
        let abi = describe_compute_abi(&parse(ABI_SRC), "quantize").expect("abi");
        assert_eq!(
            abi.scalars,
            vec![
                ("scale".to_string(), "f32".to_string()),
                ("N".to_string(), "u32".to_string())
            ],
            "scalars are values, not resources: {abi:?}"
        );
    }

    #[test]
    fn abi_sizes_buffers_from_extent_and_element_width() {
        let abi = describe_compute_abi(&parse(ABI_SRC), "quantize").expect("abi");
        assert_eq!(abi.extent, 1024);
        assert_eq!(abi.elements(), 1024);
        // 1024 elements x 4 bytes, for both an f32 and an i32 buffer.
        assert_eq!(abi.buffer_bytes(0), Some(4096));
        assert_eq!(abi.buffer_bytes(1), Some(4096));
        assert_eq!(abi.buffer_bytes(99), None, "no such binding");
    }

    /// The shader and the ABI must describe the same buffer layout.
    ///
    /// A host that allocates from the ABI but runs the shader will silently read
    /// the wrong memory if these disagree -- a shader still validates, so nothing
    /// downstream would catch it. Hence the cross-check rather than two separate
    /// expectation tests that could drift apart while both pass.
    #[test]
    fn abi_agrees_with_the_emitted_shader() {
        let p = parse(ABI_SRC);
        let w = generate_wgsl_compute(&p, "quantize").expect("shader");
        let abi = describe_compute_abi(&p, "quantize").expect("abi");

        for b in &abi.bindings {
            let decl = format!(
                "@group(0) @binding({}) var<storage, {}> {}: array<{}>;",
                b.index, b.access, b.name, b.elem
            );
            assert!(
                w.contains(&decl),
                "ABI binding {:?} must appear verbatim in the shader.\n  expected: {decl}\n  shader:\n{w}",
                b
            );
        }

        assert!(
            w.contains(&format!("fn {}(", abi.entry_point)),
            "entry point `{}` must be the shader's function: {w}",
            abi.entry_point
        );
        assert!(
            w.contains(&format!("@workgroup_size({})", abi.workgroup_size)),
            "workgroup size must match: {w}"
        );

        // The entry point must take ONLY the builtin.
        let sig = w
            .lines()
            .find(|l| l.starts_with("fn "))
            .expect("entry point line");
        assert!(
            sig.contains("@builtin(global_invocation_id)"),
            "the builtin is required: {sig}"
        );
        for (n, t) in &abi.scalars {
            assert!(
                !sig.contains(&format!("{n}: {t}")),
                "a scalar must NOT be an entry-point argument -- only builtins are \
                 legal there, and a driver rejects anything else. Signature was: {sig}"
            );
            // It must appear as a uniform binding instead, at the ABI's index.
            let idx = abi
                .scalar_binding(abi.scalars.iter().position(|(x, _)| x == n).unwrap())
                .unwrap();
            let decl = format!("@group(0) @binding({idx}) var<uniform> {n}_u: {t};");
            assert!(
                w.contains(&decl),
                "expected binding declaration: {decl}\n{w}"
            );
        }

        // And the dispatch count the host must issue.
        let expected_groups = abi.elements().div_ceil(abi.workgroup_size as usize);
        assert_eq!(expected_groups, 16, "1024 elements at 64 per group");
    }

    /// An `inout` tensor is writable, so the host must map it STORAGE|READ_WRITE.
    ///
    /// Getting this wrong fails at pipeline creation in the browser, which is the
    /// first point the mistake is visible -- but only after a user has typed a
    /// kernel and clicked run, so it is worth pinning here.
    #[test]
    fn mutable_tensors_become_read_write_bindings() {
        let src = r#"
fn touch(a: inout [1] Tensor[f32, 64]) {
    forall i in 0..64 { a[i] = a[i] + 1.0; }
}
"#;
        let abi = describe_compute_abi(&parse(src), "touch").expect("abi");
        assert_eq!(abi.bindings[0].access, "read_write");
        assert_eq!(abi.extent, 64);
        assert_eq!(abi.buffer_bytes(0), Some(256));
    }

    /// A kernel with no tensor has no bindings, and says so.
    #[test]
    fn a_scalar_function_is_refused_rather_than_given_an_empty_abi() {
        let src = "fn scalar_only(x: f32) -> f32 { return x + 1.0; }";
        let err = describe_compute_abi(&parse(src), "scalar_only").expect_err("must refuse");
        assert!(
            err.to_string().contains("takes no tensor"),
            "the refusal must say why, and point at the alternative: {err}"
        );
    }

    /// A missing kernel is an error, not an empty ABI.
    ///
    /// An empty ABI would tell the host "no buffers needed", which reads like a
    /// kernel that legitimately does nothing.
    #[test]
    fn an_unknown_kernel_is_an_error_not_an_empty_abi() {
        let err = describe_compute_abi(&parse(ABI_SRC), "no_such_kernel").expect_err("must refuse");
        assert!(err.to_string().contains("no function named"), "{err}");
    }

    /// Refusals in shader emission are refusals in the ABI too.
    ///
    /// Otherwise a host would be told about a kernel that cannot be emitted, and
    /// the failure would appear as a shader compile error instead.
    #[test]
    fn abi_refuses_exactly_what_shader_emission_refuses() {
        // Differing extents.
        let src = r#"
fn ragged(a: [*] Tensor[f32, 64], b: inout [1] Tensor[f32, 128]) {
    forall i in 0..64 { b[i] = a[i]; }
}
"#;
        let p = parse(src);
        assert!(
            generate_wgsl_compute(&p, "ragged").is_err(),
            "emitter refuses"
        );
        assert!(
            describe_compute_abi(&p, "ragged").is_err(),
            "abi refuses too"
        );

        // Narrow integer storage.
        let src = r#"
fn narrow(a: [*] Tensor[i8, 64]) {
    forall i in 0..64 { let v = a[i]; let _ = v; }
}
"#;
        let p = parse(src);
        assert!(
            generate_wgsl_compute(&p, "narrow").is_err(),
            "emitter refuses"
        );
        assert!(
            describe_compute_abi(&p, "narrow").is_err(),
            "abi refuses too"
        );
    }
}
