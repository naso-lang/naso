# Naso ISO/IEC Standardization Preparation

**Version**: 1.0  
**Date**: 2026-11-27  
**Status**: Draft for ISO/IEC JTC 1/SC 22 Submission

---

## 1. Executive Summary

This document outlines the preparation for submitting the Naso programming language for ISO/IEC standardization through JTC 1/SC 22 (Programming languages, their environments and system software interfaces). Naso is a quantum-first systems programming language with native formal verification, designed to meet the requirements for international standardization.

---

## 2. Standardization Track

### 2.1 Target Committee
- **Primary**: ISO/IEC JTC 1/SC 22/WG 21 (C++) - for systems programming language precedent
- **Alternative**: ISO/IEC JTC 1/SC 22/WG 14 (C) - for systems language precedent
- **New Work Item**: Propose new working group for "Quantum-Safe Systems Programming Languages"

### 2.2 Standardization Phases
1. **NP (New Proposal)** - Submit New Work Item Proposal
2. **WD (Working Draft)** - Develop technical specification
3. **CD (Committee Draft)** - Ballot and review
4. **DIS (Draft International Standard)** - International ballot
5. **FDIS (Final Draft)** - Final approval
6. **IS (International Standard)** - Publication

---

## 3. Technical Specification Alignment

### 3.1 Language Specification Coverage
The [Naso Language Specification](../spec/LANGUAGE_SPEC.md) covers all required areas for standardization:

| ISO Requirement | Naso Coverage |
|----------------|---------------|
| Syntax (lexical, grammar) | Section 2-3 (Complete BNF) |
| Static semantics (type system) | Section 4 (QTT, effects, dependent types) |
| Dynamic semantics | Section 5 (Operational semantics, quantum, concurrency) |
| Standard library | Section 6 (Core, quantum, edition-gated) |
| Implementation requirements | Section 8 (Conformance levels) |
| Extensions | Section 8.2 (Permitted extensions) |
| Diagnostics | Section 8.3 (Required errors/warnings) |

### 3.2 Conformance Testing
The [TCK](../tck/README.md) provides:
- 1,600+ test cases across 8 categories
- 4 conformance levels (Syntax → Full)
- Automated test runner with multiple output formats
- CI integration for continuous conformance verification

---

## 4. Patent and IP Policy

### 4.1 Patent Commitment
Naso language specification and reference implementation are released under:
- **MIT OR Apache-2.0** dual license
- **Patent Grant**: Contributors grant perpetual, royalty-free patent license
- **Defensive Patent Pool**: Managed by Naso Software Foundation

### 4.2 IP Declarations Required
For ISO submission, the following must be documented:
- [ ] No known essential patents held by Naso team
- [ ] Contributor License Agreements (CLAs) for all contributors
- [ ] Third-party dependency patent status (LLVM, Z3, etc.)
- [ ] Trademark status for "Naso" name

---

## 5. Reference Implementation

### 5.1 Primary Implementation
- **Repository**: https://github.com/naso-lang/naso
- **Language**: Rust (bootstrapped to Naso in Phase 27)
- **Backends**: LLVM (primary), GCC (naso-gcc-backend), MSVC (naso-msvc-backend)
- **Targets**: x86_64, aarch64, RISC-V, WASM32-GC
- **License**: MIT OR Apache-2.0

### 5.2 Multiple Implementations (Required by ISO)
| Implementation | Status | Backend | Notes |
|----------------|--------|---------|-------|
| naso (reference) | Production | LLVM | Self-hosted, verified |
| naso-gcc | Beta | GCC JIT / Static | libgccjit + C emission |
| naso-msvc | Beta | LLVM-IR→COFF | Windows native |
| naso-wasm | Production | WASM GC | Component Model |

---

## 6. Standard Library Specification

### 6.1 Core Library (Mandatory)
The following modules are part of the standard and must be provided by all conforming implementations:

| Module | Specification | Status |
|--------|---------------|--------|
| `std::prelude` | Auto-imported types | ✅ Specified |
| `std::alloc` | Memory allocation | ✅ Specified |
| `std::async` | Async runtime | ✅ Specified |
| `std::channel` | Message passing | ✅ Specified |
| `std::hashmap` | Hash maps | ✅ Specified |
| `std::http` | HTTP types | ✅ Specified |
| `std::mutex` | Synchronization | ✅ Specified |
| `std::vec` | Dynamic arrays | ✅ Specified |

### 6.2 Quantum Library (Mandatory for Quantum Profile)
| Module | Specification | Status |
|--------|---------------|--------|
| `std::quantum::primitives` | Qubit, Gate, Circuit | ✅ Specified |
| `std::quantum::variational` | VQE, QAOA | ✅ Specified |
| `std::quantum::qec` | Surface, Repetition, Steane codes | ✅ Specified |
| `std::quantum::hybrid` | Classical/quantum coupling | ✅ Specified |

### 6.3 Profiles
- **Base Profile**: Core library only
- **Quantum Profile**: Core + Quantum library
- **Verification Profile**: Base + Verification annotations
- **Full Profile**: All modules

---

## 7. Edition System

### 7.1 Edition Policy
- Editions every 3 years (2024, 2027, 2030, ...)
- Opt-in via `edition YYYY;` declaration
- Backward compatibility guaranteed within edition
- Automated migration tool (`naso fix --edition`)

### 7.2 Edition 2024 (Current Standard)
Frozen feature set as specified in Language Specification v1.0.

### 7.3 Future Editions
- 2027: Enhanced dependent types, implicit effects, stdlib v2
- 2030: AI-assisted compilation, formal stdlib, advanced quantum

---

## 8. Verification and Safety

### 8.1 Formal Verification Integration
- Built-in `#[verify]` attribute
- SMT solver (Z3) for automated proofs
- Lean 4 for mechanized verification
- Proof-carrying code for quantization

### 8.2 Safety Guarantees
- Memory safety via linear/affine types
- Data race freedom via effect system
- Quantum no-cloning enforced by type system
- Verified compiler (Phase 27)

---

## 9. Internationalization

### 9.1 Character Encoding
- Source files: UTF-8 only
- Identifiers: Unicode XID_Start/XID_Continue
- String literals: Unicode with escape sequences

### 9.2 Locale-Independent Behavior
- Numeric formatting: Canonical (no locale)
- Date/time: ISO 8601 in stdlib
- Collation: Unicode code point order

---

## 10. Environmental Considerations

### 10.1 Freestanding Implementation
- `#[no_std]` support for embedded
- `std::hal` for hardware abstraction
- WASI Preview 2/3 for WebAssembly

### 10.2 Hosted Implementation
- Full standard library
- OS integration (filesystem, network, threads)
- Dynamic linking support

---

## 11. Annexes (Normative)

### Annex A: Grammar (Complete BNF)
See [Language Specification Section 3](../spec/LANGUAGE_SPEC.md#3-grammar-formal-bnf)

### Annex B: Standard Library API
See [Language Specification Section 6](../spec/LANGUAGE_SPEC.md#6-standard-library-core)

### Annex C: Conformance Test Suite
See [TCK Documentation](../tck/README.md)

### Annex D: Edition History
See [Edition System Documentation](../crates/naso-edition/src/lib.rs)

### Annex E: Quantum Semantics
See [Language Specification Section 5.2.3](../spec/LANGUAGE_SPEC.md#523-quantum-semantics)

---

## 12. Bibliography

1. **ISO/IEC 23271** - Common Language Infrastructure (CLI)
2. **ISO/IEC 14882** - C++ Standard (precedent for systems language)
3. **ISO/IEC 9899** - C Standard (precedent for systems language)
4. **ISO/IEC 15445** - HTML (precedent for web integration)
5. **IEEE 754** - Floating Point Arithmetic
6. **Unicode Standard** - Character Encoding
7. **QIR Specification** - Quantum Intermediate Representation
8. **WASM GC Proposal** - WebAssembly Garbage Collection
9. **WASM Component Model** - WebAssembly Components

---

## 13. Submission Checklist

### 13.1 Pre-Submission
- [x] Language specification complete
- [x] Conformance test suite operational
- [x] Multiple implementations exist
- [x] Reference implementation open source
- [x] Patent policy documented
- [x] Edition system defined
- [x] Standard library specified

### 13.2 NP Ballot Preparation
- [ ] New Work Item Proposal document
- [ ] Project scope statement
- [ ] Justification for new standard
- [ ] Liaison statements (W3C, Khronos, etc.)
- [ ] Expert nomination list

### 13.3 WD Development
- [ ] Technical editor assignment
- [ ] Working draft repository
- [ ] Comment resolution process
- [ ] Meeting schedule

---

## 14. Contact

**Naso Software Foundation**  
Email: standardization@naso-lang.org  
GitHub: https://github.com/naso-lang/naso  
Website: https://naso-lang.org

---

*This document is a living draft. For the latest version, see the Naso repository.*