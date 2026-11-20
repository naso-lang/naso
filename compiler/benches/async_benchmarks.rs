//! Async/await benchmarks
//!
//! These benchmarks measure the overhead of async/await state machine generation
//! and await point suspension/resumption.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use naso_compiler::codegen::llvm::async_lowering::AsyncLowerer;
use naso_compiler::codegen::llvm::module_builder::LLVMModuleBuilder;
use naso_compiler::codegen::llvm::value_builder::LlvmValueBuilder;
use naso_compiler::ir::pir_types::PirExpr;
use naso_compiler::codegen::{CodegenContext, CodegenTarget, OptLevel};
use inkwell::context::Context;
use std::collections::HashMap;
use std::time::Duration;

fn bench_async_state_machine_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("async_state_machine");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(10);

    let context = Context::create();
    let codegen_context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("Failed to create codegen context");
    let module = context.create_module("async_bench");
    let builder = context.create_builder();
    let type_lowering = naso_compiler::codegen::llvm::type_lowering::LlvmTypeLowering::new(&context);
    let mut module_builder = LLVMModuleBuilder::new(&codegen_context).expect("Failed to create module builder");
    let mut value_builder = module_builder.take_value_builder();
    let quantities: HashMap<String, naso_compiler::ast::Quantity> = HashMap::new();

    // Create a simple async function body: async { 42 }
    let async_body = PirExpr::IntLit(42);

    group.bench_function("async_simple_creation", |b| {
        b.iter(|| {
            let mut async_lowerer = AsyncLowerer::new(&mut module_builder, &quantities);
            let fn_type = context.void_type().fn_type(&[], false);
            let current_function = module.add_function("test_async", fn_type, None);
            let entry = context.append_basic_block(current_function, "entry");
            value_builder.builder().position_at_end(entry);

            let _ = async_lowerer.build_async(
                &async_body,
                &mut value_builder,
                &module,
                current_function,
                None,
                true,
                &mut Vec::new(),
            );
            black_box(());
        })
    });

    group.finish();
}

fn bench_await_point_suspension(c: &mut Criterion) {
    let mut group = c.benchmark_group("await_suspension");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(10);

    let context = Context::create();
    let codegen_context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("Failed to create codegen context");
    let module = context.create_module("await_bench");
    let builder = context.create_builder();
    let type_lowering = naso_compiler::codegen::llvm::type_lowering::LlvmTypeLowering::new(&context);
    let mut module_builder = LLVMModuleBuilder::new(&codegen_context).expect("Failed to create module builder");
    let mut value_builder = module_builder.take_value_builder();
    let quantities: HashMap<String, naso_compiler::ast::Quantity> = HashMap::new();

    // Create a future expression to await
    let future_expr = PirExpr::IntLit(42); // Simplified - would be a future pointer

    group.bench_function("await_point", |b| {
        b.iter(|| {
            let mut async_lowerer = AsyncLowerer::new(&mut module_builder, &quantities);
            let fn_type = context.void_type().fn_type(&[], false);
            let current_function = module.add_function("test_await", fn_type, None);
            let entry = context.append_basic_block(current_function, "entry");
            value_builder.builder().position_at_end(entry);

            let _ = async_lowerer.build_await(
                &future_expr,
                &mut value_builder,
                &module,
                current_function,
                None,
                true,
                &mut Vec::new(),
            );
            black_box(());
        })
    });

    group.finish();
}

fn bench_multiple_await_points(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiple_await_points");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(10);

    let context = Context::create();
    let codegen_context = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("Failed to create codegen context");
    let module = context.create_module("multi_await_bench");
    let builder = context.create_builder();
    let type_lowering = naso_compiler::codegen::llvm::type_lowering::LlvmTypeLowering::new(&context);
    let mut module_builder = LLVMModuleBuilder::new(&codegen_context).expect("Failed to create module builder");
    let mut value_builder = module_builder.take_value_builder();
    let quantities: HashMap<String, naso_compiler::ast::Quantity> = HashMap::new();

    for num_awaits in [1, 2, 5, 10] {
        group.bench_with_input(BenchmarkId::from_parameter(num_awaits), &num_awaits, |b, &num_awaits| {
            b.iter(|| {
                let mut async_lowerer = AsyncLowerer::new(&mut module_builder, &quantities);
                let fn_type = context.void_type().fn_type(&[], false);
                let current_function = module.add_function("test_multi_await", fn_type, None);
                let entry = context.append_basic_block(current_function, "entry");
                value_builder.builder().position_at_end(entry);

                // Build multiple await points
                for i in 0..num_awaits {
                    let future_expr = PirExpr::IntLit(i);
                    let _ = async_lowerer.build_await(
                        &future_expr,
                        &mut value_builder,
                        &module,
                        current_function,
                        None,
                        true,
                        &mut Vec::new(),
                    );
                }
                black_box(());
            })
        });
    }

    group.finish();
}

criterion_group!(
    async_benches,
    bench_async_state_machine_creation,
    bench_await_point_suspension,
    bench_multiple_await_points
);
criterion_main!(async_benches);