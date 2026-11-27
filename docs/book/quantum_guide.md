# Quantum Programming Guide

**A comprehensive guide to quantum programming in Naso**

---

## Table of Contents

1. [Quantum Computing Fundamentals](#quantum-computing-fundamentals)
2. [Naso Quantum Model](#naso-quantum-model)
3. [Qubit Management](#qubit-management)
4. [Quantum Gates](#quantum-gates)
5. [Quantum Circuits](#quantum-circuits)
6. [Measurement](#measurement)
7. [Quantum Algorithms](#quantum-algorithms)
8. [Error Correction](#error-correction)
9. [Simulation and Execution](#simulation-and-execution)
10. [Hardware Integration](#hardware-integration)
11. [Best Practices](#best-practices)
12. [Advanced Topics](#advanced-topics)

---

## Quantum Computing Fundamentals

### Qubits

A **qubit** (quantum bit) is the fundamental unit of quantum information. Unlike classical bits that are either 0 or 1, a qubit exists in a superposition:

```
|ψ⟩ = α|0⟩ + β|1⟩
```

where α, β ∈ ℂ and |α|² + |β|² = 1.

### Quantum States

- **|0⟩** = (1, 0)ᵀ - Ground state
- **|1⟩** = (0, 1)ᵀ - Excited state
- **|+⟩** = (|0⟩ + |1⟩)/√2 - Superposition
- **|−⟩** = (|0⟩ − |1⟩)/√2 - Superposition with phase

### Entanglement

Multiple qubits can be entangled, meaning their states cannot be described independently:

```
|Φ⁺⟩ = (|00⟩ + |11⟩)/√2  (Bell state)
```

Measuring one qubit instantly determines the state of its entangled partner.

### Quantum Gates

Quantum gates are unitary operations that evolve qubit states. They are reversible and preserve the norm of the state vector.

---

## Naso Quantum Model

### Quantum Type Theory (QTT)

Naso's QTT extends the type system with quantum-specific types:

```naso
// Qubit type - linear resource
type Qubit

// Classical bit (measurement result)
type Clbit

// Quantum register
type QubitRegister<const N: usize>

// Quantum circuit
type Circuit<const N: usize>
```

### Linear Types for Qubits

Qubits are **linear resources** - they must be used exactly once:

```naso
fn correct() {
    let q = qalloc();     // Allocate
    H(q);                 // Use
    measure(q);           // Consume (or qfree)
}                         // OK: q consumed

fn incorrect() {
    let q = qalloc();
    H(q);
    // ERROR: q not consumed before scope ends
}
```

The compiler enforces:
1. Every `qalloc()` must have a corresponding `measure()` or `qfree()`
2. Qubits cannot be copied (no `Clone` impl)
3. Qubits cannot be dropped without measurement

### Quantum Scopes

Qubit lifetimes are tied to their scope:

```naso
fn example() {
    let q1 = qalloc();  // Lives until end of function
    {
        let q2 = qalloc();  // Lives until end of block
        H(q2);
        measure(q2);        // Consumed here
    }  // q2 would be auto-freed if not consumed
    H(q1);
    measure(q1);
}
```

---

## Qubit Management

### Allocation

```naso
// Single qubit in |0⟩ state
let q = qalloc();

// Multiple qubits
let q1 = qalloc();
let q2 = qalloc();
let q3 = qalloc();

// Array of qubits
let qubits: [Qubit; 5] = qalloc(5);

// With explicit type
let q: Qubit = qalloc();
```

### Deallocation

```naso
// Explicit free (when not measuring)
let q = qalloc();
H(q);
qfree(q);  // Returns qubit to pool

// Automatic on measurement
let q = qalloc();
measure(q);  // q consumed, no qfree needed
```

### Qubit Pool

Naso maintains a qubit pool for efficient allocation:

```naso
// Check available qubits
let available = naso::quantum::available_qubits();

// Reserve qubits for critical section
let reserved = naso::quantum::reserve_qubits(10);
// ... use reserved qubits ...
naso::quantum::release_reserved(reserved);
```

---

## Quantum Gates

### Single-Qubit Gates

#### Pauli Gates

```naso
X(q);  // Bit flip: |0⟩↔|1⟩
Y(q);  // Bit+phase flip
Z(q);  // Phase flip: |1⟩→−|1⟩
```

#### Hadamard Gate

```naso
H(q);  // Creates superposition: |0⟩→|+⟩, |1⟩→|−⟩
```

#### Phase Gates

```naso
S(q);      // π/2 phase: |1⟩→i|1⟩
T(q);      // π/4 phase: |1⟩→e^(iπ/4)|1⟩
S†(q);     // S dagger
T†(q);     // T dagger
```

#### Rotation Gates

```naso
RX(theta, q);  // Rotation around X axis
RY(theta, q);  // Rotation around Y axis
RZ(theta, q);  // Rotation around Z axis

// Example: arbitrary single-qubit unitary
fn arbitrary_unitary(theta: f64, phi: f64, lambda: f64, q: Qubit) {
    RZ(phi, q);
    RX(theta, q);
    RZ(lambda, q);
}
```

#### Square Root Gates

```naso
sqrtX(q);   // √X gate
sqrtY(q);   // √Y gate
sqrtZ(q);   // √Z gate (same as S)
```

### Two-Qubit Gates

#### Controlled Gates

```naso
CNOT(control, target);  // CX gate
CZ(control, target);    // Controlled-Z
CY(control, target);    // Controlled-Y
```

#### Parametric Controlled Gates

```naso
CRX(theta, control, target);
CRY(theta, control, target);
CRZ(theta, control, target);
CPHASE(theta, control, target);  // Same as CRZ
```

#### SWAP and Variants

```naso
SWAP(q1, q2);        // Swap two qubits
CSWAP(c, q1, q2);    // Controlled-SWAP (Fredkin)
ISWAP(q1, q2);       // iSWAP gate
sqrtISWAP(q1, q2);   // √iSWAP
```

### Three-Qubit Gates

```naso
CCX(c1, c2, target);   // Toffoli (CCNOT)
CCZ(c1, c2, target);   // Controlled-Controlled-Z
CSWAP(c, q1, q2);      // Fredkin gate
```

### Multi-Qubit Gates

```naso
// Multi-controlled X
MCX(controls: &[Qubit], target: Qubit);

// Multi-controlled Z
MCZ(controls: &[Qubit], target: Qubit);

// Arbitrary unitary on n qubits
UNITARY(matrix: &[Complex], qubits: &[Qubit]);
```

---

## Quantum Circuits

### Circuit Definition

```naso
circuit bell_state {
    qubits 2
    clbits 2
    
    H(0)
    CNOT(0, 1)
    measure(0) -> c0
    measure(1) -> c1
}
```

### Circuit Parameters

```naso
circuit parametric_circuit(angle: f64) {
    qubits 2
    
    H(0)
    CRZ(angle, 0, 1)
    measure(0) -> c0
    measure(1) -> c1
}

// Usage
let circuit = parametric_circuit(PI/4);
```

### Subcircuits

```naso
// Define reusable subcircuit
circuit bell_pair {
    qubits 2
    H(0)
    CNOT(0, 1)
}

// Use in larger circuit
circuit teleportation {
    qubits 3
    clbits 2
    
    // Embed bell_pair on qubits 1,2
    bell_pair(1, 2)
    
    CNOT(0, 1)
    H(0)
    measure(0) -> c0
    measure(1) -> c1
}
```

### Circuit Composition

```naso
circuit composite {
    qubits 4
    
    // Parallel composition
    H(0) || H(2)  // Parallel execution hint
    
    // Sequential
    CNOT(0, 1);
    CNOT(2, 3);
    
    // Conditional (classical control)
    if c0 { X(1); }
    if c1 { Z(3); }
}
```

### Circuit Metadata

```naso
circuit documented_circuit {
    qubits 3
    depth 10
    t_count 15
    
    /// Creates a 3-qubit GHZ state
    /// @entanglement genuine_tripartite
    H(0)
    CNOT(0, 1)
    CNOT(0, 2)
}
```

---

## Measurement

### Computational Basis Measurement

```naso
// Measure to boolean
let result: bool = measure(q);

// Measure to classical bit
let c = Clbit::new();
measure(q) -> c;

// Measure multiple qubits
let q1 = qalloc();
let q2 = qalloc();
let (r1, r2) = measure([q1, q2]);
```

### Measurement in Other Bases

```naso
// X-basis measurement (|+⟩/|−⟩)
H(q);
let result = measure(q);

// Y-basis measurement
S†(q);
H(q);
let result = measure(q);

// Arbitrary basis
fn measure_basis(q: Qubit, theta: f64, phi: f64) -> bool {
    RZ(-phi, q);
    RY(-theta, q);
    measure(q)
}
```

### Partial Measurement

```naso
circuit partial_measure {
    qubits 3
    clbits 1
    
    H(0)
    CNOT(0, 1)
    CNOT(0, 2)
    
    // Measure only first qubit
    measure(0) -> c0
    
    // Remaining qubits still entangled
    // q1 and q2 now in mixed state depending on c0
}
```

### Measurement Statistics

```naso
fn measure_statistics(circuit: Circuit, shots: usize) -> MeasurementStats {
    let results = naso::quantum::simulate(circuit, shots);
    
    let counts = results.counts();
    let probabilities = results.probabilities();
    
    MeasurementStats {
        counts,
        probabilities,
        expectation_values: calculate_expectations(&probabilities),
    }
}
```

---

## Quantum Algorithms

### Deutsch-Jozsa Algorithm

```naso
circuit deutsch_jozsa<const N: usize>(oracle: Circuit<N+1>) {
    qubits N + 1
    
    // Initialize
    X(N);          // Ancilla in |1⟩
    repeat N { H(i); }  // Superposition on input
    H(N);          // Superposition on ancilla
    
    // Oracle
    oracle;
    
    // Interference
    repeat N { H(i); }
    
    // Measure input register
    repeat N { measure(i) -> c(i); }
}
```

### Grover's Search

```naso
fn grover_iteration<const N: usize>(
    oracle: Circuit<N>,
    diffuser: Circuit<N>
) -> Circuit<N> {
    circuit {
        qubits N
        oracle;
        diffuser;
    }
}

circuit grover_diffuser<const N: usize> {
    qubits N
    
    repeat N { H(i); }
    repeat N { X(i); }
    
    // Multi-controlled Z
    H(N-1);
    MCX(0..N-2, N-1);
    H(N-1);
    
    repeat N { X(i); }
    repeat N { H(i); }
}
```

### Quantum Phase Estimation

```naso
circuit qpe<const N: usize>(unitary: Circuit<N>, precision: usize) {
    qubits N + precision
    clbits precision
    
    // Initialize eigenstate on N qubits (assumed prepared)
    // Initialize precision qubits in superposition
    for i in 0..precision {
        H(i);
    }
    
    // Controlled unitary powers
    for i in 0..precision {
        controlled_unitary_power(unitary, 2^i, i, N..N+precision);
    }
    
    // Inverse QFT on precision qubits
    inverse_qft(0..precision);
    
    // Measure precision qubits
    for i in 0..precision {
        measure(i) -> c(i);
    }
}
```

### Variational Quantum Eigensolver (VQE)

```naso
struct VQEProblem {
    hamiltonian: PauliSum,
    ansatz: fn(&[f64], &[Qubit]) -> Circuit,
    initial_params: Vec<f64>,
}

fn vqe_solve(problem: VQEProblem) -> VQEResult {
    let mut params = problem.initial_params;
    let mut optimizer = COBYLA::new();
    
    loop {
        // Build circuit with current params
        let circuit = build_ansatz_circuit(&params, &problem.ansatz);
        
        // Estimate expectation value
        let energy = estimate_energy(&circuit, &problem.hamiltonian);
        
        // Optimize
        let new_params = optimizer.step(params, energy);
        
        if converged(params, new_params) {
            break VQEResult { energy, params: new_params };
        }
        params = new_params;
    }
}
```

### Quantum Approximate Optimization Algorithm (QAOA)

```naso
circuit qaoa<const N: usize>(problem: MaxCut, gamma: f64, beta: f64) {
    qubits N
    
    // Initial superposition
    repeat N { H(i); }
    
    // Problem unitary (cost Hamiltonian)
    for (i, j) in problem.edges {
        // exp(-i*gamma*Z_i*Z_j)
        CNOT(i, j);
        RZ(2*gamma, j);
        CNOT(i, j);
    }
    
    // Mixer unitary
    for i in 0..N {
        RX(2*beta, i);
    }
}
```

---

## Error Correction

### Quantum Error Correction Codes

```naso
// 3-qubit bit-flip code
circuit bit_flip_encode {
    qubits 3
    CNOT(0, 1)
    CNOT(0, 2)
}

circuit bit_flip_syndrome {
    qubits 5  // 3 data + 2 ancilla
    // Syndrome extraction
    CNOT(0, 3); CNOT(1, 3);
    CNOT(1, 4); CNOT(2, 4);
    measure(3) -> s0;
    measure(4) -> s1;
}

circuit bit_flip_correct {
    qubits 3
    clbits 2
    // Apply correction based on syndrome
    if s0 && !s1 { X(0); }
    if s0 && s1  { X(1); }
    if !s0 && s1 { X(2); }
}

// Shor's 9-qubit code
circuit shor_encode {
    qubits 9
    // Bit-flip encoding on 3 blocks
    // Phase-flip encoding across blocks
}

circuit shor_decode {
    qubits 9
    // Syndrome measurement
    // Correction
}
```

### Error Mitigation

```naso
// Zero-noise extrapolation
fn zero_noise_extrapolation(circuit: Circuit, noise_factors: &[f64]) -> f64 {
    let mut results = Vec::new();
    
    for &factor in noise_factors {
        let noisy_circuit = scale_noise(circuit, factor);
        let result = simulate(noisy_circuit);
        results.push(result);
    }
    
    // Extrapolate to zero noise
    extrapolate_to_zero(noise_factors, results)
}

// Readout error mitigation
fn readout_mitigation(results: MeasurementCounts, calibration: CalibrationMatrix) -> Counts {
    calibration.inverse() * results
}
```

---

## Simulation and Execution

### State Vector Simulation

```naso
use naso::quantum::simulator::StateVectorSimulator;

fn simulate_statevector(circuit: Circuit) -> StateVector {
    let mut sim = StateVectorSimulator::new(circuit.num_qubits());
    sim.run(&circuit);
    sim.statevector()
}

fn get_amplitudes(circuit: Circuit) -> Vec<Complex> {
    let sv = simulate_statevector(circuit);
    sv.amplitudes()
}
```

### Stabilizer Simulation

```naso
use naso::quantum::simulator::StabilizerSimulator;

fn simulate_stabilizer(circuit: Circuit) -> StabilizerState {
    let mut sim = StabilizerSimulator::new(circuit.num_qubits());
    sim.run(&circuit);
    sim.state()
}
```

### Density Matrix Simulation

```naso
use naso::quantum::simulator::DensityMatrixSimulator;

fn simulate_density_matrix(circuit: Circuit) -> DensityMatrix {
    let mut sim = DensityMatrixSimulator::new(circuit.num_qubits());
    sim.run(&circuit);
    sim.state()
}
```

### Sampling

```naso
fn sample(circuit: Circuit, shots: usize) -> Counts {
    let mut sim = StateVectorSimulator::new(circuit.num_qubits());
    sim.run(&circuit);
    sim.sample(shots)
}

fn sample_with_memory(circuit: Circuit, shots: usize) -> Vec<Bitstring> {
    let mut sim = StateVectorSimulator::new(circuit.num_qubits());
    sim.run(&circuit);
    sim.sample_memory(shots)
}
```

---

## Hardware Integration

### QIR Compilation

```bash
# Compile to QIR (Quantum Intermediate Representation)
naso build --target qir --release

# Output: circuit.qir (LLVM IR with quantum intrinsics)
```

### QIR Example

```llvm
; QIR for bell_state
define void @bell_state(%Qubit* %q0, %Qubit* %q1, %Result* %c0, %Result* %c1) {
entry:
  call void @__quantum__qis__h__body(%Qubit* %q0)
  call void @__quantum__qis__cnot__body(%Qubit* %q0, %Qubit* %q1)
  call void @__quantum__qis__mz__body(%Qubit* %q0, %Result* %c0)
  call void @__quantum__qis__mz__body(%Qubit* %q1, %Result* %c1)
  ret void
}
```

### Hardware Backends

```naso
// Configure hardware backend
#[cfg(target = "ibm")]
use naso::quantum::backend::IBMBackend;

#[cfg(target = "rigetti")]
use naso::quantum::backend::RigettiBackend;

#[cfg(target = "ionq")]
use naso::quantum::backend::IonQBackend;

fn run_on_hardware(circuit: Circuit, backend: &dyn QuantumBackend) -> JobResult {
    let job = backend.submit(circuit);
    job.wait()
}
```

### Pulse-Level Control

```naso
use naso::quantum::pulse::{Schedule, Pulse, Channel};

fn custom_gate_pulse(qubit: Qubit) -> Schedule {
    let mut sched = Schedule::new();
    
    // Gaussian pulse for X gate
    let gaussian = Pulse::gaussian(
        duration: 100,  // dt
        amp: 0.5,
        sigma: 20,
        channel: Channel::Drive(qubit)
    );
    
    sched.add_pulse(gaussian);
    sched
}
```

---

## Best Practices

### 1. Minimize Circuit Depth

```naso
// BAD: Sequential gates that could be parallel
H(0); H(1); H(2);
CNOT(0, 1);
CNOT(1, 2);

// GOOD: Explicit parallelism
H(0) || H(1) || H(2);  // Parallel hint
CNOT(0, 1);
CNOT(1, 2);
```

### 2. Reduce T-Count

```naso
// T gates are expensive on fault-tolerant hardware
// Use Clifford+T decomposition optimization

// BAD: Many T gates
T(q); T(q); T†(q); T(q);

// GOOD: Optimize
S(q);  // S = T²
```

### 3. Use Qubit Reuse

```naso
fn qubit_reuse_example() {
    let q = qalloc();
    
    // First computation
    H(q);
    let r1 = measure(q);  // q consumed
    
    // Reuse qubit for next computation
    let q = qalloc();  // New allocation (or from pool)
    X(q);
    let r2 = measure(q);
}
```

### 4. Verify Quantum Properties

```naso
#[verify_quantum(properties = ["unitarity", "no_cloning"])]
circuit verified_circuit {
    qubits 3
    // Compiler proves this circuit is unitary
    // and doesn't violate no-cloning
}
```

### 5. Profile Before Optimizing

```bash
# Analyze circuit resources
naso analyze circuit.naso --resources

# Output:
# Qubits: 10
# Depth: 45
# T-count: 120
# Clifford-count: 340
# 2-qubit gates: 85
```

---

## Advanced Topics

### Quantum Machine Learning

```naso
// Quantum Neural Network layer
fn qnn_layer(params: &[f64], qubits: &[Qubit]) {
    // Encoding layer
    for (i, &x) in data.iter().enumerate() {
        RY(x, qubits[i]);
    }
    
    // Variational layer
    let mut idx = 0;
    for i in 0..qubits.len() {
        RY(params[idx], qubits[i]); idx += 1;
        RZ(params[idx], qubits[i]); idx += 1;
    }
    
    // Entanglement
    for i in 0..qubits.len()-1 {
        CNOT(qubits[i], qubits[i+1]);
    }
}

// Training loop
fn train_qnn(data: &[Vec<f64>], labels: &[f64]) -> Vec<f64> {
    let mut params = random_params();
    let mut opt = Adam::new(0.01);
    
    for epoch in 0..100 {
        let grad = compute_gradient(&params, data, labels);
        params = opt.step(params, grad);
    }
    params
}
```

### Quantum Error Correction Integration

```naso
// Logical qubit abstraction
struct LogicalQubit {
    physical_qubits: [Qubit; 9],  // Shor code
    syndrome_history: Vec<Syndrome>,
}

impl LogicalQubit {
    fn new() -> Self {
        let physical = qalloc(9);
        shor_encode(physical);
        LogicalQubit { physical_qubits: physical, syndrome_history: Vec::new() }
    }
    
    fn logical_x(&mut self) {
        // Transversal X
        for q in &self.physical_qubits {
            X(*q);
        }
    }
    
    fn logical_measure(&mut self) -> bool {
        shor_decode(self.physical_qubits);
        measure(self.physical_qubits[0])
    }
    
    fn error_correct(&mut self) {
        let syndrome = extract_syndrome(self.physical_qubits);
        self.syndrome_history.push(syndrome);
        apply_correction(self.physical_qubits, syndrome);
    }
}
```

### Distributed Quantum Computing

```naso
// Network quantum operations
use naso::quantum::network::{EPRPair, Teleport};

fn distributed_cnot(control: Qubit, target: Qubit, epr: EPRPair) {
    // Teleport control to target location
    let (c_local, c_remote) = Teleport::create_pair();
    Teleport::send(control, c_local);
    
    // Remote CNOT
    CNOT(c_remote, target);
    
    // Teleport back if needed
}
```

---

## Appendix: Gate Reference

| Gate | Matrix | QASM | Description |
|------|--------|------|-------------|
| I | [[1,0],[0,1]] | `id` | Identity |
| X | [[0,1],[1,0]] | `x` | Pauli-X (NOT) |
| Y | [[0,-i],[i,0]] | `y` | Pauli-Y |
| Z | [[1,0],[0,-1]] | `z` | Pauli-Z |
| H | 1/√2[[1,1],[1,-1]] | `h` | Hadamard |
| S | [[1,0],[0,i]] | `s` | Phase (π/2) |
| T | [[1,0],[0,e^(iπ/4)]] | `t` | π/4 phase |
| RX(θ) | [[cos(θ/2),-isin(θ/2)],[-isin(θ/2),cos(θ/2)]] | `rx` | X rotation |
| RY(θ) | [[cos(θ/2),-sin(θ/2)],[sin(θ/2),cos(θ/2)]] | `ry` | Y rotation |
| RZ(θ) | [[e^(-iθ/2),0],[0,e^(iθ/2)]] | `rz` | Z rotation |
| CNOT | [[1,0,0,0],[0,1,0,0],[0,0,0,1],[0,0,1,0]] | `cx` | Controlled-X |
| CZ | diag(1,1,1,-1) | `cz` | Controlled-Z |
| SWAP | [[1,0,0,0],[0,0,1,0],[0,1,0,0],[0,0,0,1]] | `swap` | Swap |
| Toffoli | 8×8 permutation | `ccx` | CCX |

---

*End of Quantum Programming Guide*