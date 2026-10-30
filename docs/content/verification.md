---
title: "Formal Verification"
weight: 40
---

# Formal Verification (`naso-verify`)

## Overview

`naso-verify` is Naso's embedded SMT-based verification engine. It uses Z3 to prove compile-time guarantees about linear resource usage, quantum uncomputation safety, array bounds, and polyhedral loop invariants.

## Verification Targets

| Target | Check | Error Code |
|--------|-------|------------|
| Linear resources | Every `[1]` consumed exactly once | `NASO-LIN-*` |
| Erased quantities | `[0]` values eliminated | `NASO-ERASE-*` |
| Quantum uncomputation | Ancilla returned to $|0\rangle$ | `NASO-UNC-*` |
| Array bounds | All accesses in-bounds | `NASO-BND-*` |
| Polyhedral schedules | Dependences preserved | `NASO-POL-*` |