# Naso Technology Compatibility Kit (TCK)

**Version**: 1.0  
**Edition**: 2024  
**Repository**: https://github.com/naso-lang/naso/tree/main/tck

---

## 1. Overview

The Naso TCK is a comprehensive test suite that verifies any Naso implementation conforms to the [Language Specification](../spec/LANGUAGE_SPEC.md). It covers:

- **Syntax tests**: Valid/invalid programs per grammar
- **Type system tests**: QTT quantities, linearity, effects, dependent types
- **Semantic tests**: Operational behavior, quantum semantics, concurrency
- **Standard library tests**: Core and quantum modules
- **Edition tests**: 2024, 2027, 2030 compatibility
- **Verification tests**: SMT/Lean 4 integration
- **Interoperability tests**: Cross-implementation compatibility

---

## 2. Test Suite Structure

```
tck/
├── Cargo.toml                    # TCK runner crate
├── src/
│   ├── main.rs                   # Test runner entry point
│   ├── harness.rs                # Test harness infrastructure
│   ├── results.rs                # Test result reporting
│   └── categories/
│       ├── syntax.rs             # Syntax conformance
│       ├── types.rs              # Type system conformance
│       ├── semantics.rs          # Dynamic semantics
│       ├── stdlib.rs             # Standard library
│       ├── quantum.rs            # Quantum operations
│       ├── effects.rs            # Effect system
│       ├── verification.rs       # Verification integration
│       └── editions.rs           # Edition compliance
├── tests/
│   ├── syntax/
│   │   ├── valid/                # Programs that MUST compile
│   │   └── invalid/              # Programs that MUST reject
│   ├── types/
│   │   ├── linear/               # Linearity tests
│   │   ├── affine/               # Affine tests
│   │   ├── unrestricted/         # Unrestricted tests
│   │   ├── quantities/           # Quantity subtyping
│   │   ├── effects/              # Effect typing
│   │   ├── dependent/            # Dependent types
│   │   └── erasure/              # Erasure modes
│   ├── semantics/
│   │   ├── basic/                # Basic evaluation
│   │   ├── quantum/              # Quantum semantics
│   │   ├── concurrency/          # Async/await
│   │   └── effects/              # Effect handlers
│   ├── stdlib/
│   │   ├── core/                 # Core modules
│   │   ├── quantum/              # Quantum modules
│   │   └── edition_gated/        # Edition-gated modules
│   ├── verification/
│   │   ├── smt/                  # SMT-based verification
│   │   └── lean/                 # Lean 4 verification
│   └── editions/
│       ├── 2024/                 # 2024 edition tests
│       ├── 2027/                 # 2027 edition tests (future)
│       └── 2030/                 # 2030 edition tests (future)
├── expected/
│   └── *.expected                # Expected outputs for semantic tests
└── README.md                     # This file
```

---

## 3. Running the TCK

### 3.1 Quick Start

```bash
# Run all tests
cd /home/node/naso/tck
cargo run --release

# Run specific category
cargo run --release -- --category syntax
cargo run --release -- --category types
cargo run --release -- --category quantum

# Run with specific implementation
cargo run --release -- --impl-path /path/to/naso
```

### 3.2 Test Runner Options

```bash
naso-tck [OPTIONS]

Options:
  -c, --category <CAT>      Test category (syntax, types, semantics, stdlib, quantum, effects, verification, editions)
  -i, --impl-path <PATH>    Path to naso implementation under test
  -j, --jobs <N>            Parallel test jobs (default: CPU count)
  -v, --verbose             Verbose output
  -o, --output <FORMAT>     Output format: json, junit, tap, text (default: text)
  --filter <PATTERN>        Run tests matching pattern
  --list                    List all tests without running
  --help                    Show help
```

### 3.3 Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All tests passed |
| 1 | One or more tests failed |
| 2 | Test infrastructure error |
| 3 | Implementation not found/invalid |
| 4 | Configuration error |

---

## 4. Test Categories

### 4.1 Syntax Tests (`syntax/`)

**Valid programs** (`tests/syntax/valid/`) — Must compile successfully:
- `basic_fn.naso` — Simple function
- `quantum_circuit.naso` — Quantum circuit declaration
- `effect_handler.naso` — Effect handler syntax
- `dependent_types.naso` — Π/Σ types
- `edition_2024.naso` — Edition declaration
- `linear_types.naso` — Linear type annotations
- `generics.naso` — Generic parameters
- `pattern_matching.naso` — Complex patterns

**Invalid programs** (`tests/syntax/invalid/`) — Must produce compile errors:
- `missing_semicolon.naso` — Syntax error
- `invalid_quantity.naso` — Invalid quantity annotation
- `duplicate_binding.naso` — Shadowing error
- `unclosed_brace.naso` — Unmatched delimiter
- `invalid_effect_syntax.naso` — Malformed effect

### 4.2 Type System Tests (`types/`)

#### 4.2.1 Linear Types (`types/linear/`)
- `linear_consumed_once.naso` — Linear value used exactly once
- `linear_not_dropped.naso` — Linear value cannot be implicitly dropped
- `linear_not_copied.naso` — Linear value cannot be copied
- `linear_function_arg.naso` — Linear function parameter consumed
- `linear_return.naso` — Linear return value
- `linear_tuple.naso` — Linear tuple components
- `linear_match.naso` — Linear pattern matching

#### 4.2.2 Affine Types (`types/affine/`)
- `affine_can_drop.naso` — Affine value can be dropped
- `affine_not_copied.naso` — Affine value cannot be copied
- `affine_optional_use.naso` — Affine value used 0 or 1 times

#### 4.2.3 Unrestricted Types (`types/unrestricted/`)
- `unrestricted_copy.naso` — Unrestricted values freely copied
- `unrestricted_drop.naso` — Unrestricted values implicitly dropped
- `default_unrestricted.naso` — Default quantity is unrestricted

#### 4.2.4 Quantity Subtyping (`types/quantities/`)
- `subtyping_linear_to_affine.naso` — Linear ≤ Affine
- `subtyping_affine_to_unrestricted.naso` — Affine ≤ Unrestricted
- `subtyping_transitivity.naso` — Linear ≤ Affine ≤ Unrestricted
- `quantity_inference.naso` — Quantity inference

#### 4.2.5 Effect Typing (`types/effects/`)
- `effect_inference.naso` — Effect row inference
- `effect_polymorphism.naso` — Generic effects
- `effect_handling.naso` — Handler removes effects
- `unhandled_effect_error.naso` — Unhandled effect is error
- `builtin_async.naso` — Async effect
- `builtin_try.naso` — Try effect
- `builtin_quantum.naso` — Quantum effect

#### 4.2.6 Dependent Types (`types/dependent/`)
- `pi_type.naso` — Π-type formation
- `sigma_type.naso` — Σ-type formation
- `dependent_application.naso` — Dependent function application
- `relevance_inference.naso` — Relevance analysis
- `erasure_mode_erase.naso` — Erase mode
- `erasure_mode_check.naso` — Check mode
- `erasure_mode_retain.naso` — Retain mode

### 4.3 Semantic Tests (`semantics/`)

#### 4.3.1 Basic Evaluation (`semantics/basic/`)
- `arithmetic.naso` — Integer/float arithmetic
- `control_flow.naso` — If, match, loops
- `functions.naso` — Function calls, recursion
- `closures.naso` — Closure capture and call
- `mutability.naso` — Mutable bindings

#### 4.3.2 Quantum Semantics (`semantics/quantum/`)
- `qubit_allocation.naso` — Qubit allocation/initialization
- `single_qubit_gates.naso` — X, Y, Z, H, S, T gates
- `two_qubit_gates.naso` — CNOT, CZ, SWAP
- `measurement.naso` — Computational basis measurement
- `superposition.naso` — H|0⟩ = |+⟩
- `entanglement.naso` — Bell state preparation
- `no_cloning.naso` — Linear qubits prevent cloning
- `circuit_execution.naso` — Multi-gate circuits
- `density_matrix.naso` — Mixed state evolution
- `error_mitigation_zn.e.naso` — Zero-noise extrapolation

#### 4.3.3 Concurrency (`semantics/concurrency/`)
- `async_await.naso` — Basic async/await
- `task_spawning.naso` — Task creation
- `structured_concurrency.naso` — Task tree cancellation
- `channel_communication.naso` — Message passing
- `mutex_synchronization.naso` — Mutual exclusion

#### 4.3.4 Effect Handlers (`semantics/effects/`)
- `handler_basic.naso` — Simple effect handling
- `handler_resumption.naso` — Continuation resumption
- `handler_stacking.naso` — Multiple handlers
- `handler_polymorphic.naso` — Generic handlers
- `builtin_async_handler.naso` — Async runtime
- `builtin_try_handler.naso` — Error handling

### 4.4 Standard Library Tests (`stdlib/`)

#### 4.4.1 Core Modules (`stdlib/core/`)
- `prelude.naso` — Auto-imported items
- `alloc.naso` — Allocation API
- `vec.naso` — Vector operations
- `hashmap.naso` — Hash map operations
- `channel.naso` — Channel API
- `mutex.naso` — Mutex API

#### 4.4.2 Quantum Modules (`stdlib/quantum/`)
- `primitives.naso` — Qubit, Gate, Circuit types
- `variational_vqe.naso` — VQE algorithm
- `variational_qaoa.naso` — QAOA algorithm
- `qec_surface.naso` — Surface code
- `qec_repetition.naso` — Repetition code
- `qec_steane.naso` — Steane code
- `hybrid_hpc.naso` — Classical/quantum coupling

#### 4.4.3 Edition-Gated (`stdlib/edition_gated/`)
- `edition_2027_feature.naso` — 2027-only feature
- `edition_2030_feature.naso` — 2030-only feature

### 4.5 Verification Tests (`verification/`)

#### 4.5.1 SMT Verification (`verification/smt/`)
- `precondition_check.naso` — Requires clause
- `postcondition_check.naso` — Ensures clause
- `loop_invariant.naso` — Invariant checking
- `linearity_verification.naso` — Resource usage proof
- `effect_safety.naso` — Effect handling proof
- `quantum_unitarity.naso` — Unitary gate proof

#### 4.5.2 Lean 4 Verification (`verification/lean/`)
- `type_safety.lean` — Type safety theorem
- `linearity_proof.lean` — Linearity proof
- `compiler_correctness.lean` — Compilation correctness
- `quantum_semantics.lean` — Quantum semantics proof

### 4.6 Edition Tests (`editions/`)

#### 4.6.1 Edition 2024 (`editions/2024/`)
- `edition_declaration.naso` — Edition 2024 declaration
- `all_features.naso` — All 2024 features work
- `no_2027_features.naso` — 2027 features rejected

#### 4.6.2 Edition 2027 (`editions/2027/`) — Future
- `enhanced_dependent_types.naso`
- `new_stdlib_modules.naso`

#### 4.6.3 Edition 2030 (`editions/2030/`) — Future
- `ai_compilation.naso`
- `advanced_quantum.naso`

---

## 5. Test Format

### 5.1 Naso Test Files

Each `.naso` test file contains:

```naso
// TEST: test_name
// CATEGORY: types/linear
// EXPECT: compile_success | compile_error | runtime_success | runtime_error | verification_pass | verification_fail
// EDITION: 2024
// DESCRIPTION: Brief description of what this test verifies

edition 2024;

fn main() {
    // Test code here
    let x: [1] i32 = 42;
    consume(x);  // Must consume exactly once
}

fn consume(x: [1] i32) { }
```

### 5.2 Expected Output Files

For semantic tests, `.expected` files contain expected stdout/stderr:

```
# tests/semantics/basic/arithmetic.expected
stdout: "42\n"
stderr: ""
exit_code: 0
```

### 5.3 Verification Test Format

```naso
// TEST: precondition_check
// CATEGORY: verification/smt
// EXPECT: verification_pass
// EDITION: 2024

edition 2024;

fn add(x: i32, y: i32) -> i32
    verify {
        requires x >= 0;
        requires y >= 0;
        ensures result >= 0;
    }
{
    x + y
}
```

---

## 6. Conformance Levels

### 6.1 Level 0: Syntax Conformance
- Parses all valid programs
- Rejects all invalid programs with errors
- **Required for all implementations**

### 6.2 Level 1: Type System Conformance
- Level 0 +
- Correctly implements QTT typing rules
- Linear/affine/unrestricted quantities enforced
- Effect system sound
- **Required for "Naso Compatible" badge**

### 6.3 Level 2: Semantic Conformance
- Level 1 +
- Operational semantics match specification
- Quantum semantics correct (probabilities, entanglement)
- Concurrency model correct
- Effect handlers work
- **Required for "Naso Verified" badge**

### 6.4 Level 3: Verification Conformance
- Level 2 +
- SMT verification produces correct results
- Lean 4 proofs check (if supported)
- **Required for "Naso Certified" badge**

### 6.5 Level 4: Edition Conformance
- Level 3 +
- Correctly implements edition system
- Migration tool works
- Cross-edition interoperability
- **Required for "Naso Standard" badge**

---

## 7. Implementation Guide

### 7.1 Adding a New Implementation

1. Build your Naso implementation
2. Ensure `naso` binary is in PATH or specify `--impl-path`
3. Run TCK: `cargo run --release -- --impl-path /path/to/naso`
4. Address any failures
5. Submit conformance report

### 7.2 Interpreting Results

```
TCK Results Summary
===================
Categories: 8
Tests Run:  1,247
Passed:     1,234
Failed:     13
Skipped:    0

Failed Tests:
  types/linear/linear_not_dropped.naso     - Expected compile_error, got compile_success
  semantics/quantum/entanglement.naso      - Output mismatch: expected "0.707", got "0.7071"
  verification/smt/loop_invariant.naso     - Verification timeout
```

### 7.3 Reporting Conformance

Submit results to: https://github.com/naso-lang/naso/issues/new?template=conformance.yml

Include:
- Implementation name/version
- TCK version
- Conformance level achieved
- Full test output (JSON format)
- Known deviations (if any)

---

## 8. Continuous Integration

The TCK runs automatically on:
- Every commit to `naso-lang/naso` (GitHub Actions)
- Nightly against all known implementations
- Release candidates for new editions

### 8.1 GitHub Actions Workflow

```yaml
# .github/workflows/tck.yml
name: TCK Conformance
on: [push, pull_request, schedule]
jobs:
  tck:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build Naso
        run: cargo build --release --workspace
      - name: Run TCK
        run: cd tck && cargo run --release
      - name: Upload Results
        uses: actions/upload-artifact@v4
        with:
          name: tck-results
          path: tck/results/
```

---

## 9. Test Statistics (Target)

| Category | Test Count | Target Pass Rate |
|----------|------------|------------------|
| Syntax | 150 | 100% |
| Types | 420 | 100% |
| Semantics | 380 | 100% |
| Stdlib | 200 | 100% |
| Quantum | 180 | 100% |
| Effects | 120 | 100% |
| Verification | 90 | 100% |
| Editions | 60 | 100% |
| **Total** | **1,600** | **100%** |

---

## 10. Contributing

### 9.1 Adding Tests

1. Identify the correct category directory
2. Create `.naso` file with proper metadata header
3. Add `.expected` file if semantic test
4. Run TCK locally to verify
5. Submit PR

### 9.2 Test Design Principles

- **One assertion per test** — Each test verifies one specific behavior
- **Minimal reproduction** — Smallest code demonstrating the feature
- **Clear naming** — `feature_scenario_expected.naso`
- **Documentation** — DESCRIPTION explains *why* this test exists
- **Edition tagged** — EDITION specifies applicable editions

---

## 11. License

TCK tests are dual-licensed under MIT OR Apache-2.0, same as Naso.

---

*For the latest TCK version and results, see https://github.com/naso-lang/naso/tree/main/tck*