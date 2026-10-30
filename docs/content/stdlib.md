---
title: "Standard Library"
weight: 30
---

# Naso Standard Library (`naso_std`)

## Overview

The Naso standard library (`naso_std`) provides foundational types and primitives integrated with QTT quantity tracking, MVS mutability, and formal verification.

## Modules

### `core`
Primitives, `Option`, `Result`, `Vec`, `String`, `Array`

### `std::quantum`
`qalloc`, `qfree`, `hadamard`, `cnot`, `measure`, `bell_pair`, `qft`

### `std::tensor`
`Tensor`, `matmul`, `add`, `sub`, `scale`, `transpose`, `contract`, `outer_product`

### `std::sys`
`sys_qalloc`, `sys_qfree`, `sys_barrier`, `sys_measure_trap`, `volatile_load/store`

### `std::alloc`
`LinearAllocator`, `QttArena`, `linear_alloc`, `linear_free`