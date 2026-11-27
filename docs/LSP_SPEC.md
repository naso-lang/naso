# Naso Language Server Protocol Specification

**Version**: 1.0.0  
**Status**: Draft  
**Authors**: Naso Language Team  
**Last Updated**: 2024

---

## Table of Contents

1. [Overview](#overview)
2. [Initialization](#initialization)
3. [Capabilities](#capabilities)
4. [Text Synchronization](#text-synchronization)
5. [Diagnostics](#diagnostics)
6. [Completion](#completion)
7. [Hover](#hover)
8. [Go to Definition](#go-to-definition)
9. [Find References](#find-references)
10. [Document Symbols](#document-symbols)
11. [Workspace Symbols](#workspace-symbols)
12. [Code Actions](#code-actions)
13. [Formatting](#formatting)
14. [Semantic Tokens](#semantic-tokens)
15. [Inlay Hints](#inlay-hints)
16. [Quantum-Specific Extensions](#quantum-specific-extensions)
17. [SMT Diagnostics Extension](#smt-diagnostics-extension)
18. [Error Codes](#error-codes)

---

## Overview

The Naso Language Server Protocol (naso-lsp) extends the standard Language Server Protocol (LSP) with Naso-specific features including:

- **Quantum Type Theory (QTT) support**: Specialized completions, hover, and diagnostics for quantum constructs
- **SMT-based type checking**: Real-time formal verification diagnostics
- **Quantum circuit analysis**: Visualization, simulation, and OpenQASM export
- **Package management integration**: Registry search, dependency resolution

This specification defines the protocol between Naso language clients (VS Code, IntelliJ, etc.) and the `naso-lsp` server.

---

## Initialization

### Initialize Request

```typescript
interface InitializeParams extends LSP.InitializeParams {
  initializationOptions?: {
    /** Path to naso compiler binary */
    nasoPath?: string;
    /** Enable quantum circuit analysis */
    enableQuantumAnalysis?: boolean;
    /** Enable SMT-based type checking */
    enableSMTDiagnostics?: boolean;
    /** Quantum simulator backend */
    quantumSimulator?: 'statevector' | 'stabilizer' | 'density_matrix';
  };
}
```

### Initialize Response

```typescript
interface InitializeResult extends LSP.InitializeResult {
  capabilities: NasoServerCapabilities;
  serverInfo: {
    name: 'naso-lsp';
    version: string;
  };
}
```

---

## Capabilities

### NasoServerCapabilities

```typescript
interface NasoServerCapabilities extends LSP.ServerCapabilities {
  /** Text document synchronization */
  textDocumentSync: TextDocumentSyncOptions | TextDocumentSyncKind;
  
  /** Completion provider with quantum-specific triggers */
  completionProvider: CompletionOptions;
  
  /** Hover provider with QTT type information */
  hoverProvider: boolean | HoverOptions;
  
  /** Definition provider */
  definitionProvider: boolean | DefinitionOptions;
  
  /** References provider */
  referencesProvider: boolean | ReferenceOptions;
  
  /** Document symbols for structure view */
  documentSymbolProvider: boolean | DocumentSymbolOptions;
  
  /** Workspace symbols for global search */
  workspaceSymbolProvider: boolean | WorkspaceSymbolOptions;
  
  /** Code actions including quantum-specific fixes */
  codeActionProvider: boolean | CodeActionOptions;
  
  /** Formatting support */
  documentFormattingProvider: boolean | DocumentFormattingOptions;
  documentRangeFormattingProvider: boolean;
  
  /** Semantic tokens for syntax highlighting */
  semanticTokensProvider: SemanticTokensOptions;
  
  /** Inlay hints for type annotations */
  inlayHintProvider: boolean | InlayHintOptions;
  
  /** Naso-specific extensions */
  naso?: {
    /** Quantum circuit analysis */
    quantumCircuitAnalysis: boolean;
    /** SMT diagnostics */
    smtDiagnostics: boolean;
    /** Package management */
    packageManagement: boolean;
  };
}
```

---

## Text Synchronization

Standard LSP text synchronization with `TextDocumentSyncKind.Incremental`.

---

## Diagnostics

### Standard Diagnostics

Published via `textDocument/publishDiagnostics` with standard LSP diagnostic format.

### Naso-Specific Diagnostic Extensions

```typescript
interface NasoDiagnostic extends LSP.Diagnostic {
  /** Naso-specific error code */
  code?: NasoErrorCode;
  
  /** Related quantum circuit information */
  quantumContext?: {
    circuitName?: string;
    qubitIndex?: number;
    gateName?: string;
  };
  
  /** SMT solver information */
  smtContext?: {
    solver: 'z3' | 'cvc5';
    assertion: string;
    model?: string;
  };
  
  /** Quick fix actions */
  quickFixes?: CodeAction[];
}
```

### NasoErrorCode

```typescript
enum NasoErrorCode {
  // Type errors
  TYPE_MISMATCH = 1001,
  TYPE_NOT_FOUND = 1002,
  GENERIC_CONSTRAINT_VIOLATION = 1003,
  LINEAR_TYPE_VIOLATION = 1004,
  
  // Quantum errors
  QUBIT_OUT_OF_BOUNDS = 2001,
  GATE_ARITY_MISMATCH = 2002,
  MEASUREMENT_COLLISION = 2003,
  CIRCUIT_UNITARY_VIOLATION = 2004,
  ENTANGLEMENT_ERROR = 2005,
  
  // SMT errors
  SMT_UNSATISFIABLE = 3001,
  SMT_TIMEOUT = 3002,
  SMT_UNKNOWN = 3003,
  
  // Package errors
  PACKAGE_NOT_FOUND = 4001,
  VERSION_CONFLICT = 4002,
  REGISTRY_UNAVAILABLE = 4003
}
```

---

## Completion

### Completion Request

Standard `textDocument/completion` with Naso-specific trigger characters: `.`, `:`, `::`, `@`, `|>`.

### Completion Item Extensions

```typescript
interface NasoCompletionItem extends LSP.CompletionItem {
  /** Quantum gate category */
  quantumGate?: {
    name: string;
    arity: number;
    params: number;
    description: string;
  };
  
  /** QTT type signature */
  qttType?: string;
  
  /** SMT verification status */
  smtVerified?: boolean;
}
```

---

## Hover

### Hover Request

Standard `textDocument/hover`.

### Hover Response Extensions

```typescript
interface NasoHover extends LSP.Hover {
  contents: LSP.MarkupContent | LSP.MarkedString | (LSP.MarkupContent | LSP.MarkedString)[];
  
  /** QTT type information */
  qttType?: {
    type: string;
    quantum: boolean;
    linear: boolean;
    description: string;
  };
  
  /** Circuit visualization (for quantum constructs) */
  circuitPreview?: string; // ASCII circuit diagram
}
```

---

## Go to Definition

Standard `textDocument/definition` with support for:
- Function/struct/enum/trait definitions
- Quantum gate definitions
- Package imports

---

## Find References

Standard `textDocument/references`.

---

## Document Symbols

Standard `textDocument/documentSymbol` with Naso-specific symbol kinds:

```typescript
enum NasoSymbolKind {
  Circuit = 100,
  Qubit = 101,
  Gate = 102,
  Measurement = 103,
  QAlloc = 104,
  QFree = 105
}
```

---

## Workspace Symbols

Standard `workspace/symbol` with query support for Naso constructs.

---

## Code Actions

### Standard Code Actions

- Import missing dependencies
- Add missing type annotations
- Fix formatting

### Naso-Specific Code Actions

```typescript
interface NasoCodeAction extends LSP.CodeAction {
  kind: NasoCodeActionKind;
  
  /** Quantum-specific fixes */
  quantumFix?: {
    type: 'add_qubit' | 'fix_gate_arity' | 'add_measurement' | 'fix_circuit';
    description: string;
  };
  
  /** SMT-specific fixes */
  smtFix?: {
    type: 'add_assertion' | 'relax_constraint' | 'provide_model';
    description: string;
  };
}
```

---

## Formatting

Standard `textDocument/formatting` and `textDocument/rangeFormatting` using `naso fmt` under the hood.

---

## Semantic Tokens

### Semantic Token Types

```typescript
enum NasoSemanticTokenType {
  quantumKeyword = 'quantumKeyword',
  quantumGate = 'quantumGate',
  qttType = 'qttType',
  linearType = 'linearType',
  smtAssertion = 'smtAssertion',
  circuit = 'circuit',
  qubit = 'qubit'
}
```

### Semantic Token Modifiers

```typescript
enum NasoSemanticTokenModifier {
  entangled = 'entangled',
  measured = 'measured',
  allocated = 'allocated',
  verified = 'verified'
}
```

---

## Inlay Hints

```typescript
interface NasoInlayHint extends LSP.InlayHint {
  /** Type annotation hint */
  typeHint?: {
    type: string;
    quantum: boolean;
  };
  
  /** Qubit index hint */
  qubitHint?: {
    index: number;
  };
}
```

---

## Quantum-Specific Extensions

### Circuit Analysis Request

```typescript
interface CircuitAnalysisParams {
  textDocument: TextDocumentIdentifier;
  circuitName?: string; // Analyze specific circuit, or all if omitted
}

interface CircuitAnalysisResult {
  circuits: CircuitInfo[];
}

interface CircuitInfo {
  name: string;
  qubits: number;
  depth: number;
  gates: GateInfo[];
  measurements: MeasurementInfo[];
  entanglement: EntanglementInfo;
  resources: ResourceEstimate;
}

interface GateInfo {
  name: string;
  qubits: number[];
  params?: number[];
  position: { line: number; character: number };
}

interface MeasurementInfo {
  qubit: number;
  classicalBit: number;
  position: { line: number; character: number };
}

interface EntanglementInfo {
  pairs: [number, number][];
  clusters: number[][];
}

interface ResourceEstimate {
  tCount: number;
  cliffordCount: number;
  depth: number;
  width: number;
}
```

### Simulation Request

```typescript
interface SimulationParams {
  textDocument: TextDocumentIdentifier;
  circuitName: string;
  simulator: 'statevector' | 'stabilizer' | 'density_matrix';
  shots?: number;
  initialState?: number[]; // Amplitude vector
}

interface SimulationResult {
  statevector?: Complex[];
  probabilities: { [bitstring: string]: number };
  expectationValues: { [observable: string]: number };
  executionTime: number;
}
```

### OpenQASM Export Request

```typescript
interface OpenQASMExportParams {
  textDocument: TextDocumentIdentifier;
  circuitName: string;
  version: '2.0' | '3.0';
  includeDeclarations: boolean;
}

interface OpenQASMExportResult {
  openqasm: string;
}
```

---

## SMT Diagnostics Extension

### Verification Request

```typescript
interface VerificationParams {
  textDocument: TextDocumentIdentifier;
  functionName?: string; // Verify specific function, or all if omitted
  timeout?: number; // milliseconds
}

interface VerificationResult {
  results: VerificationResultItem[];
}

interface VerificationResultItem {
  functionName: string;
  status: 'verified' | 'failed' | 'timeout' | 'error';
  assertions: AssertionResult[];
  counterexample?: string;
  executionTime: number;
}

interface AssertionResult {
  assertion: string;
  status: 'valid' | 'invalid' | 'unknown';
  model?: string;
}
```

---

## Error Codes

See [NasoErrorCode](#nasoerrorcode) enum above.

---

## Implementation Notes

### Server Implementation

The reference implementation is `naso-lsp` written in Rust, located in `crates/naso-lsp/`.

### Client Implementations

- **VS Code**: `vscode-naso` extension
- **IntelliJ Platform**: `intellij-naso` plugin
- **Neovim**: `naso.nvim` (community)
- **Helix**: `helix-naso` (community)

### Protocol Versioning

The protocol uses semantic versioning. Breaking changes increment the major version.

### Transport

Standard LSP transport: stdio (primary), TCP (optional for remote development).

---

## Appendix: Example Naso Code

```naso
// Function with QTT types
fn hadamard_test(qubit: Qubit) -> Result<bool> {
    H(qubit);
    let result = measure(qubit);
    Ok(result)
}

// Quantum circuit
circuit bell_state {
    qubits 2
    H(0)
    CNOT(0, 1)
    measure(0) -> c0
    measure(1) -> c1
}

// SMT-verified function
#[verify]
fn add_safe(a: u64, b: u64) -> u64
    requires a + b < u64::MAX
    ensures result == a + b
{
    a + b
}
```

---

*End of Specification*