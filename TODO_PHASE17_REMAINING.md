# Phase 17 Remaining Tasks

## 1. Stack-allocated state machines (17.3)
- [ ] Rewrite async lowering to use LLVM coroutines/statepoints for true stack allocation
- [ ] Replace `build_alloca` with coroutine frame allocation
- [ ] Implement `llvm.coro.begin`, `llvm.coro.alloc`, `llvm.coro.free`, `llvm.coro.suspend`, `llvm.coro.resume`, `llvm.coro.destroy`
- [ ] Update FFI to work with coroutine-based state machines

## 2. True callback completion (17.4)
- [ ] Replace polling `naso_yield()` with io_uring CQE → direct callback trampoline
- [ ] Implement `naso_uring_submit` + completion queue polling in background thread
- [ ] Create callback trampolines that directly resume suspended coroutines
- [ ] Remove `naso_check_*_completion` polling functions
- [ ] Wire io_uring CQE to invoke callback with state machine pointer

## 3. Binary size optimization (17.7)
- [ ] Add LTO (Link Time Optimization) to LLVM codegen
- [ ] Add DCE (Dead Code Elimination) passes
- [ ] Enable `-C lto=yes` and `-C opt-level=z` for size optimization
- [ ] Add `-C codegen-units=1` for better optimization
- [ ] Strip symbols in release builds

## 4. Verification
- [ ] Build compiler with all changes
- [ ] Run benchmarks to verify performance
- [ ] Check binary size reduction