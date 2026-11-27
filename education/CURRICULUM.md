# Naso University Curriculum

**Version**: 1.0  
**Target**: CS undergraduate and graduate programs  
**Language**: Naso 2024 Edition  
**License**: MIT OR Apache-2.0

---

## Overview

A complete university curriculum for teaching Naso as a first systems programming language. The curriculum covers:

- Introductory programming with Naso
- Systems programming (memory, concurrency, quantum)
- Formal verification
- Compiler construction
- Quantum computing with Naso
- Capstone projects

---

## Course Catalog

### CS101: Introduction to Programming with Naso

**Level**: Freshman / Introductory  
**Prerequisites**: None  
**Duration**: 1 semester (14 weeks)

#### Learning Objectives
- Write basic Naso programs with variables, functions, and control flow
- Understand linear/affine type quantities
- Use async/await for concurrent programming
- Debug and test Naso code

#### Weekly Topics

| Week | Topic | Key Concepts |
|------|-------|--------------|
| 1 | Naso basics | Variables, types, functions |
| 2 | Control flow | If, match, loops |
| 3 | Data structures | Arrays, vectors, hashmaps |
| 4 | Ownership and quantities | Linear, affine, unrestricted |
| 5 | Error handling | Try effect, result types |
| 6 | Modules and crates | Module system, packages |
| 7 | Concurrency | Async/await, tasks |
| 8 | Channels and communication | Message passing |
| 9 | Testing | Unit tests, integration tests |
| 10 | File I/O | Reading/writing files |
| 11 | Network basics | HTTP client |
| 12 | Project 1 | Personal todo application |
| 13 | Project 1 continued | Refinement and testing |
| 14 | Final project presentations | Demo and code review |

#### Assignments
- Lab 1: Hello Naso (basic syntax)
- Lab 2: Calculator (functions, control flow)
- Lab 3: Contact list (data structures)
- Lab 4: Bank account (ownership, quantities)
- Lab 5: Chat client (concurrency, channels)
- Project 1: Personal task manager (full application)

#### Required Reading
- [The Naso Book](https://naso-lang.org/book)
- [Naso by Example](https://naso-lang.org/by-example)

---

### CS201: Systems Programming with Naso

**Level**: Sophomore / Intermediate  
**Prerequisites**: CS101  
**Duration**: 1 semester (14 weeks)

#### Learning Objectives
- Understand memory management without garbage collection
- Write unsafe code safely with linear types
- Implement data structures from scratch
- Use the HAL for hardware interaction
- Build embedded applications

#### Weekly Topics

| Week | Topic | Key Concepts |
|------|-------|--------------|
| 1 | Memory model | Stack, heap, regions |
| 2 | Pointers and references | Raw pointers, references |
| 3 | Custom data structures | Linked lists, trees, graphs |
| 4 | Unsafe code | Unsafe blocks, FFI |
| 5 | Hardware abstraction | HAL, GPIO, I2C, SPI |
| 6 | Embedded development | No-std, bare metal |
| 7 | Real-time systems | RTIC, interrupts |
| 8 | Power management | Sleep modes, wakeup |
| 9 | Device drivers | Writing drivers |
| 10 | Networking embedded | LwIP, MQTT |
| 11 | Security | Secure boot, encryption |
| 12 | Project 2 | Embedded sensor node |
| 13 | Project 2 continued | Integration and testing |
| 14 | Final presentations | Demo and review |

#### Assignments
- Lab 1: Memory allocator
- Lab 2: Binary search tree
- Lab 3: GPIO blink (embedded)
- Lab 4: I2C temperature sensor
- Lab 5: UART communication
- Project 2: Environmental monitoring station

---

### CS301: Formal Verification with Naso

**Level**: Junior / Advanced  
**Prerequisites**: CS201, Discrete Mathematics  
**Duration**: 1 semester (14 weeks)

#### Learning Objectives
- Write verified code with pre/post conditions
- Use SMT solvers for automated proving
- Understand Lean 4 for mechanized proofs
- Prove linear type safety
- Verify quantum algorithms

#### Weekly Topics

| Week | Topic | Key Concepts |
|------|-------|--------------|
| 1 | Verification basics | Requires, ensures, invariants |
| 2 | SMT solving | Z3, SMT-LIB |
| 3 | Loop invariants | Induction, termination |
| 4 | Linear type proofs | Resource usage |
| 5 | Effect system verification | Handler correctness |
| 6 | Lean 4 basics | Tactics, proofs |
| 7 | Lean 4 advanced | Inductive types, quotients |
| 8 | Compiler correctness | Simulation proofs |
| 9 | Quantum verification | Unitary proofs |
| 10 | QEC verification | Error correction proofs |
| 11 | Self-hosting verification | Bootstrap proofs |
| 12 | Project 3 | Verified data structure |
| 13 | Project 3 continued | Proof completion |
| 14 | Final presentations | Demo and review |

#### Assignments
- Lab 1: Verified sorting algorithm
- Lab 2: Verified binary search
- Lab 3: Verified linked list
- Lab 4: Lean 4 exercises
- Lab 5: Quantum gate verification
- Project 3: Verified red-black tree

---

### CS401: Quantum Computing with Naso

**Level**: Senior / Advanced  
**Prerequisites**: CS301, Linear Algebra  
**Duration**: 1 semester (14 weeks)

#### Learning Objectives
- Write quantum circuits in Naso
- Implement VQE and QAOA algorithms
- Use quantum error correction
- Build hybrid classical/quantum applications
- Deploy on real quantum hardware

#### Weekly Topics

| Week | Topic | Key Concepts |
|------|-------|--------------|
| 1 | Quantum basics | Qubits, gates, circuits |
| 2 | Quantum algorithms | Deutsch-Jozsa, Grover |
| 3 | Variational algorithms | VQE, QAOA |
| 4 | Error mitigation | ZNE, PEC, CDR |
| 5 | Quantum error correction | Surface, repetition, Steane |
| 6 | Hybrid workflows | Classical + quantum |
| 7 | HPC integration | MPI-offload coupling |
| 8 | Hardware backends | IBM, IonQ, Rigetti, Quantinuum |
| 9 | Quantum ML | QNN, quantum kernels |
| 10 | Quantum optimization | Portfolio, MaxCut |
| 11 | Quantum chemistry | Molecular simulation |
| 12 | Project 4 | Quantum application |
| 13 | Project 4 continued | Hardware deployment |
| 14 | Final presentations | Demo and review |

#### Assignments
- Lab 1: Bell state preparation
- Lab 2: Grover's algorithm
- Lab 3: VQE for H2 molecule
- Lab 4: Surface code QEC
- Lab 5: Quantum ML classifier
- Project 4: End-to-end quantum application

---

### CS402: Compiler Construction with Naso

**Level**: Senior / Advanced  
**Prerequisites**: CS301  
**Duration**: 1 semester (14 weeks)

#### Learning Objectives
- Build a compiler frontend (lexer, parser, type checker)
- Implement IR generation and optimization
- Write code generation for LLVM/GCC/MSVC
- Implement effects and quantum backends
- Add formal verification to compiler

#### Weekly Topics

| Week | Topic | Key Concepts |
|------|-------|--------------|
| 1 | Compiler overview | Pipeline, phases |
| 2 | Lexing | Tokenization, regex |
| 3 | Parsing | Recursive descent, precedence |
| 4 | AST | Abstract syntax trees |
| 5 | Type checking | QTT, quantities, effects |
| 6 | IR design | PIR, schedule trees |
| 7 | Optimization | Polyhedral, inlining |
| 8 | Code generation | LLVM, QIR, WASM |
| 9 | Effects backend | Handler compilation |
| 10 | Quantum backend | QIR emission |
| 11 | Verification integration | SMT, Lean 4 |
| 12 | Project 5 | Mini compiler |
| 13 | Project 5 continued | Optimization passes |
| 14 | Final presentations | Demo and review |

#### Assignments
- Lab 1: Lexer for Naso subset
- Lab 2: Parser for expressions
- Lab 3: Type checker
- Lab 4: IR generation
- Lab 5: LLVM code generation
- Project 5: Complete compiler for Naso subset

---

## Teaching Resources

### Lecture Slides
Available at: https://github.com/naso-lang/education/tree/main/slides

### Lab Exercises
Available at: https://github.com/naso-lang/education/tree/main/labs

### Sample Solutions
Available at: https://github.com/naso-lang/education/tree/main/solutions (instructor access required)

### Autograder
Available at: https://github.com/naso-lang/naso-autograder

### Docker Images
- `naso-lang/edu:2024` - Full development environment
- `naso-lang/edu:2024-minimal` - Compiler only

---

## Adoption Program

Universities adopting Naso receive:
- Free instructor training
- Guest lectures from Naso team
- Curriculum consulting
- Student hackathon support
- Research collaboration opportunities

Contact: education@naso-lang.org

---

## Participating Universities

| University | Course | Semester | Students |
|------------|--------|----------|----------|
| MIT | 6.1000 | Fall 2026 | 150 |
| Stanford | CS143 | Winter 2027 | 120 |
| ETH Zurich | 252-0030 | Spring 2027 | 90 |
| Carnegie Mellon | 15-418 | Fall 2026 | 110 |
| UC Berkeley | CS162 | Spring 2027 | 140 |

*Curriculum available under CC BY-SA 4.0*