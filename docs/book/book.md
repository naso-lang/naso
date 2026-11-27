# The Naso Book

**Version**: 0.1.0  
**Language Version**: Naso 0.1  
**Authors**: The Naso Language Team

---

## Table of Contents

1. [Introduction](#introduction)
2. [Getting Started](#getting-started)
3. [Basic Syntax](#basic-syntax)
4. [Variables and Mutability](#variables-and-mutability)
5. [Functions](#functions)
6. [Control Flow](#control-flow)
7. [Data Types](#data-types)
8. [Structs and Enums](#structs-and-enums)
9. [Traits and Generics](#traits-and-generics)
10. [Error Handling](#error-handling)
11. [Modules and Packages](#modules-and-packages)
12. [Testing](#testing)
13. [Quantum Programming](#quantum-programming)
14. [Quantum Type Theory (QTT)](#quantum-type-theory-qtt)
15. [Formal Verification](#formal-verification)
16. [Advanced Topics](#advanced-topics)
17. [Appendix: Keywords](#appendix-keywords)
18. [Appendix: Operators](#appendix-operators)

---

## Introduction

### What is Naso?

Naso is a systems programming language that combines:
- **Memory safety** without garbage collection (like Rust)
- **Quantum computing primitives** as first-class citizens
- **Formal verification** via SMT solvers (Z3, CVC5)
- **Quantum Type Theory (QTT)** for quantum program correctness
- **High performance** with LLVM, Cranelift, and QIR backends

### Design Goals

1. **Correctness by construction**: Quantum programs that are verified at compile time
2. **Zero-cost abstractions**: No runtime overhead for quantum operations
3. **Interoperability**: Seamless integration with classical code
4. **Developer experience**: Excellent tooling, diagnostics, and documentation

### Who Should Read This Book?

- Systems programmers interested in quantum computing
- Quantum algorithm researchers wanting verified implementations
- Rust developers exploring quantum extensions
- Anyone building quantum-classical hybrid applications

---

## Getting Started

### Installation

```bash
# Using the installer script
curl -fsSL https://nasolang.org/install.sh | sh

# Or from source
git clone https://github.com/naso-lang/naso
cd naso
cargo build --release
```

### Hello, World!

```naso
fn main() {
    println!("Hello, Naso!");
}
```

Run it:
```bash
naso run hello.naso
```

### Your First Quantum Program

```naso
fn main() {
    // Allocate a qubit in |0⟩ state
    let q = qalloc();
    
    // Apply Hadamard gate: |0⟩ → (|0⟩ + |1⟩)/√2
    H(q);
    
    // Measure in computational basis
    let result = measure(q);
    
    println!("Measurement: {}", result);
    
    // Free the qubit
    qfree(q);
}
```

---

## Basic Syntax

### Comments

```naso
// Line comment

/*
 * Block comment
 * Can span multiple lines
 */

/// Documentation comment
/// Used for generating docs
```

### Literals

```naso
// Integers
42          // i64 (default)
42i32       // i32
42u64       // u64

// Floats
3.14        // f64 (default)
3.14f32     // f32

// Boolean
true
false

// Characters
'a'
'🚀'

// Strings
"Hello, Naso!"
"Line 1\nLine 2"

// Raw strings
r#"No escaping needed: \n\t"#

// Byte strings
b"bytes"
```

---

## Variables and Mutability

### Immutable by Default

```naso
let x = 42;        // Immutable
let y = 3.14;      // Type inferred

// x = 10;         // Error: cannot assign to immutable variable
```

### Mutable Variables

```naso
let mut x = 42;
x = 10;            // OK

let mut y = 3.14;
y += 1.0;          // OK
```

### Constants

```naso
const PI: f64 = 3.14159265359;
const MAX_SIZE: usize = 1024;

// Must be annotated with type
// Can be used in const contexts
```

### Shadowing

```naso
let x = 5;
let x = x + 1;     // New binding, shadows previous
let x = x * 2;     // Shadows again

// x is now 12
```

---

## Functions

### Basic Functions

```naso
fn add(a: i64, b: i64) -> i64 {
    a + b  // Implicit return (no semicolon)
}

fn greet(name: &str) {
    println!("Hello, {}!", name);  // Statement (with semicolon)
}
```

### Function Parameters

```naso
// By value (move semantics for non-Copy types)
fn take_ownership(s: String) { }

// By immutable reference
fn borrow(s: &String) { }

// By mutable reference
fn modify(s: &mut String) { }

// Generic parameters
fn identity<T>(x: T) -> T { x }

// Multiple parameters
fn compute(a: i64, b: i64, c: f64) -> f64 { a as f64 + b as f64 * c }
```

### Return Values

```naso
// Implicit return (expression without semicolon)
fn add(a: i64, b: i64) -> i64 {
    a + b
}

// Explicit return
fn div(a: f64, b: f64) -> Result<f64, String> {
    if b == 0.0 {
        return Err("Division by zero".to_string());
    }
    Ok(a / b)
}

// Unit type (implicit return)
fn print_sum(a: i64, b: i64) {
    println!("Sum: {}", a + b);
    // Returns ()
}
```

### Diverging Functions

```naso
fn panic(msg: &str) -> ! {
    // Never returns
    std::process::abort(msg)
}
```

---

## Control Flow

### If Expressions

```naso
// If as expression
let max = if a > b { a } else { b };

// If as statement
if condition {
    // block
} else if other_condition {
    // block
} else {
    // block
}

// Pattern matching in if
if let Some(value) = optional {
    // use value
}
```

### Loops

```naso
// Infinite loop
loop {
    if condition { break; }
}

// While loop
while condition {
    // body
}

// For loop over ranges
for i in 0..10 {
    println!("{}", i);
}

for i in 0..=10 {  // Inclusive
    println!("{}", i);
}

// For loop over iterators
for item in collection.iter() {
    // use item
}

// Loop labels
'outer: for i in 0..10 {
    for j in 0..10 {
        if i * j > 50 {
            break 'outer;
        }
    }
}
```

### Match Expressions

```naso
match value {
    1 => println!("One"),
    2 | 3 => println!("Two or three"),
    4..=10 => println!("Four to ten"),
    n @ 11..=20 => println!("Eleven to twenty: {}", n),
    _ => println!("Something else"),
}

// Match as expression
let result = match x {
    0 => "zero",
    1 => "one",
    _ => "many",
};

// Match with guards
match x {
    n if n > 0 => "positive",
    n if n < 0 => "negative",
    _ => "zero",
}
```

---

## Data Types

### Primitive Types

```naso
// Signed integers
i8, i16, i32, i64, i128, isize

// Unsigned integers
u8, u16, u32, u64, u128, usize

// Floating point
f32, f64

// Boolean
bool

// Character (Unicode scalar value)
char

// Never type (diverging functions)
!
```

### Compound Types

```naso
// Tuple
let tup: (i64, f64, bool) = (42, 3.14, true);
let (x, y, z) = tup;  // Destructuring
let first = tup.0;    // Index access

// Array (fixed size)
let arr: [i64; 5] = [1, 2, 3, 4, 5];
let first = arr[0];

// Slice (dynamic view)
let slice: &[i64] = &arr[1..3];

// String
let s: String = String::from("hello");
let s: &str = "hello";  // String slice
```

### Pointers and References

```naso
// Immutable reference
let x = 42;
let r: &i64 = &x;

// Mutable reference
let mut y = 10;
let r: &mut i64 = &mut y;

// Raw pointers (unsafe)
let ptr: *const i64 = &x as *const i64;
let mut_ptr: *mut i64 = &mut y as *mut i64;
```

---

## Structs and Enums

### Structs

```naso
// Classic struct
struct Point {
    x: f64,
    y: f64,
}

// Tuple struct
struct Color(u8, u8, u8);

// Unit struct
struct Marker;

// With generics
struct Pair<T, U> {
    first: T,
    second: U,
}

// Methods
impl Point {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    
    fn distance(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
    
    fn translate(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
    }
}

// Usage
let p = Point::new(0.0, 0.0);
let d = p.distance(&Point::new(3.0, 4.0));
```

### Enums

```naso
// Simple enum
enum Direction {
    North,
    South,
    East,
    West,
}

// Enum with data
enum Option<T> {
    None,
    Some(T),
}

// Enum with multiple variants
enum Result<T, E> {
    Ok(T),
    Err(E),
}

// Complex enum
enum Message {
    Quit,
    Move { x: i32, y: i32 },
    Write(String),
    ChangeColor(u8, u8, u8),
}

// Pattern matching
match msg {
    Message::Quit => println!("Quit"),
    Message::Move { x, y } => println!("Move to ({}, {})", x, y),
    Message::Write(text) => println!("Write: {}", text),
    Message::ChangeColor(r, g, b) => println!("Color: {} {} {}", r, g, b),
}
```

---

## Traits and Generics

### Defining Traits

```naso
trait Drawable {
    fn draw(&self);
    
    fn draw_twice(&self) {
        self.draw();
        self.draw();
    }
}

trait Serializable {
    fn serialize(&self) -> Vec<u8>;
    fn deserialize(data: &[u8]) -> Self where Self: Sized;
}
```

### Implementing Traits

```naso
impl Drawable for Point {
    fn draw(&self) {
        println!("Drawing point at ({}, {})", self.x, self.y);
    }
}

impl<T: Drawable> Drawable for Vec<T> {
    fn draw(&self) {
        for item in self {
            item.draw();
        }
    }
}
```

### Trait Bounds

```naso
// Single bound
fn print_debug<T: std::fmt::Debug>(value: &T) {
    println!("{:?}", value);
}

// Multiple bounds
fn process<T: Clone + std::fmt::Debug>(value: T) -> T {
    println!("{:?}", value);
    value.clone()
}

// Where clause
fn complex<T, U>(t: T, u: U) 
where 
    T: Clone + std::fmt::Debug,
    U: std::fmt::Display 
{
    // ...
}
```

### Generics

```naso
// Generic struct
struct Container<T> {
    value: T,
}

// Generic enum
enum Either<L, R> {
    Left(L),
    Right(R),
}

// Generic function
fn swap<T>(a: &mut T, b: &mut T) {
    std::mem::swap(a, b);
}

// Const generics
struct Array<T, const N: usize> {
    data: [T; N],
}

fn take_array<const N: usize>(arr: [i32; N]) { }
```

---

## Error Handling

### Result Type

```naso
enum Result<T, E> {
    Ok(T),
    Err(E),
}

// Using Result
fn divide(a: f64, b: f64) -> Result<f64, String> {
    if b == 0.0 {
        Err("Division by zero".to_string())
    } else {
        Ok(a / b)
    }
}

// Handling Result
match divide(10.0, 2.0) {
    Ok(result) => println!("Result: {}", result),
    Err(e) => println!("Error: {}", e),
}

// Using ? operator
fn compute() -> Result<f64, String> {
    let x = divide(10.0, 2.0)?;
    let y = divide(x, 5.0)?;
    Ok(y)
}
```

### Option Type

```naso
enum Option<T> {
    None,
    Some(T),
}

// Using Option
fn find_user(id: u64) -> Option<User> {
    if id == 0 { None } else { Some(User::new(id)) }
}

// Handling Option
match find_user(42) {
    Some(user) => println!("Found: {}", user.name),
    None => println!("Not found"),
}

// Using ? with Option
fn get_name(id: u64) -> Option<String> {
    let user = find_user(id)?;
    Some(user.name)
}
```

---

## Modules and Packages

### Module System

```naso
// math.naso
pub fn add(a: i64, b: i64) -> i64 { a + b }
pub fn sub(a: i64, b: i64) -> i64 { a - b }

// main.naso
mod math;  // Loads math.naso or math/mod.naso

fn main() {
    println!("{}", math::add(1, 2));
}
```

### Module Hierarchy

```
src/
├── main.naso
├── math.naso
├── physics/
│   ├── mod.naso
│   ├── quantum.naso
│   └── classical.naso
```

### Visibility

```naso
mod internal {
    pub fn public_function() { }  // Visible outside
    fn private_function() { }     // Module-private
    
    pub mod nested {
        pub fn deeply_nested() { }
    }
}

// Re-export
pub use internal::public_function;
```

### Package Configuration (naso.toml)

```toml
[package]
name = "my-project"
version = "0.1.0"
authors = ["Your Name <you@example.com>"]
edition = "2024"
description = "A Naso project"
license = "MIT"
repository = "https://github.com/user/my-project"
homepage = "https://github.com/user/my-project"
keywords = ["quantum", "naso"]
categories = ["science", "quantum-computing"]

[dependencies]
naso-std = "0.1"
quantum-gates = "0.1"

[dev-dependencies]
naso-test = "0.1"

[build-dependencies]
naso-build = "0.1"

[features]
default = ["std"]
std = ["naso-std"]
no-std = []
```

---

## Testing

### Unit Tests

```naso
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
    
    #[test]
    fn test_divide() {
        assert!(divide(10.0, 2.0).is_ok());
        assert!(divide(10.0, 0.0).is_err());
    }
    
    #[test]
    #[should_panic(expected = "Division by zero")]
    fn test_divide_panic() {
        divide(10.0, 0.0).unwrap();
    }
}
```

### Integration Tests

```naso
// tests/integration_test.naso
use my_project::*;

#[test]
fn test_full_workflow() {
    let result = compute();
    assert!(result.is_ok());
}
```

### Running Tests

```bash
naso test              # Run all tests
naso test -- --nocapture  # Show println output
naso test test_add     # Run specific test
```

---

## Quantum Programming

### Qubit Management

```naso
fn quantum_example() {
    // Allocate qubits
    let q1 = qalloc();           // Single qubit
    let q2 = qalloc();           // Another qubit
    let qubits = qalloc(5);      // Array of 5 qubits
    
    // Use qubits...
    
    // Free qubits (automatic on scope exit, but explicit is better)
    qfree(q1);
    qfree(q2);
    qfree(qubits);
}
```

### Quantum Gates

```naso
fn apply_gates(q: Qubit) {
    // Single-qubit gates
    H(q);      // Hadamard
    X(q);      // Pauli-X (NOT)
    Y(q);      // Pauli-Y
    Z(q);      // Pauli-Z
    S(q);      // Phase (π/2)
    T(q);      // π/4 phase
    
    // Parameterized gates
    RX(PI/2, q);   // Rotation around X
    RY(PI/4, q);   // Rotation around Y
    RZ(PI, q);     // Rotation around Z
}

fn apply_two_qubit_gates(q1: Qubit, q2: Qubit) {
    CNOT(q1, q2);  // Controlled-X
    CZ(q1, q2);    // Controlled-Z
    SWAP(q1, q2);  // Swap qubits
}
```

### Measurement

```naso
fn measure_examples(q: Qubit) {
    // Measure in computational basis
    let result: bool = measure(q);
    
    // Measure to classical bit
    let cbit = Clbit::new();
    measure(q) -> cbit;
    
    // Measure multiple qubits
    let q1 = qalloc();
    let q2 = qalloc();
    let results = measure([q1, q2]);  // Returns (bool, bool)
}
```

### Quantum Circuits

```naso
circuit bell_state {
    qubits 2
    
    H(0)           // Create superposition on qubit 0
    CNOT(0, 1)     // Entangle qubits 0 and 1
    measure(0) -> c0
    measure(1) -> c1
}

circuit ghz_state {
    qubits 3
    
    H(0)
    CNOT(0, 1)
    CNOT(0, 2)
    measure(0) -> c0
    measure(1) -> c1
    measure(2) -> c2
}

circuit quantum_fourier_transform {
    qubits 4
    
    H(0)
    CPHASE(PI/2, 0, 1)
    CPHASE(PI/4, 0, 2)
    CPHASE(PI/8, 0, 3)
    
    H(1)
    CPHASE(PI/2, 1, 2)
    CPHASE(PI/4, 1, 3)
    
    H(2)
    CPHASE(PI/2, 2, 3)
    
    H(3)
    
    // Swap for correct output order
    SWAP(0, 3)
    SWAP(1, 2)
}
```

### Quantum Subroutines

```naso
// Reusable quantum function
fn create_bell_pair(q1: Qubit, q2: Qubit) {
    H(q1);
    CNOT(q1, q2);
}

// Using subroutines in circuits
circuit teleportation {
    qubits 3
    
    // Prepare Bell pair between qubits 1 and 2
    create_bell_pair(1, 2)
    
    // Alice's operations on qubit 0 and 1
    CNOT(0, 1)
    H(0)
    
    // Measurements
    measure(0) -> c0
    measure(1) -> c1
    
    // Bob's corrections
    if c1 { X(2); }
    if c0 { Z(2); }
    
    // Verify teleportation
    measure(2) -> c2
}
```

---

## Quantum Type Theory (QTT)

### Linear Types for Qubits

```naso
// Qubits are linear resources - must be used exactly once
fn linear_example() {
    let q = qalloc();
    H(q);
    // q must be measured or freed before going out of scope
    measure(q);  // Consumes q
    // q is no longer accessible here
}

// This would be a compile error:
// fn bad_example() {
//     let q = qalloc();
//     H(q);
//     // q not consumed - ERROR!
// }
```

### Quantum Type Annotations

```naso
// Explicit quantum types
let q: Qubit = qalloc();
let c: Clbit = Clbit::new();

// Entangled qubit pairs
type BellPair = (Qubit, Qubit) @entangled;

// Superposition types
type Superposition = Qubit @superposition;

// Measured qubit (classical result)
type Measured = Clbit @measured;

// Circuit types
circuit my_circuit: Circuit<2> { ... }
```

### Quantum Lifetimes

```naso
// Qubit lifetime tied to circuit scope
circuit my_circuit {
    qubits 2
    
    // q1 and q2 live for entire circuit
    H(0)
    CNOT(0, 1)
    measure(0) -> c0  // q0 consumed
    measure(1) -> c1  // q1 consumed
}
// All qubits automatically freed here
```

---

## Formal Verification

### Contracts

```naso
// Preconditions and postconditions
fn add_safe(a: u64, b: u64) -> u64
    requires a + b < u64::MAX
    ensures result == a + b
{
    a + b
}

// Loop invariants
fn factorial(n: u64) -> u64
    requires n <= 20
    ensures result == factorial_spec(n)
{
    let mut result = 1;
    let mut i = 1;
    while i <= n
        invariant result == factorial_spec(i - 1)
        invariant i <= n + 1
    {
        result *= i;
        i += 1;
    }
    result
}

fn factorial_spec(n: u64) -> u64 {
    if n == 0 { 1 } else { n * factorial_spec(n - 1) }
}
```

### Verification Attributes

```naso
// Verify this function
#[verify]
fn verified_function(x: i64) -> i64 {
    x + 1
}

// Verify with specific solver
#[verify(solver = "z3", timeout = 5000)]
fn complex_verification(x: i64, y: i64) -> i64 {
    x * y
}

// Skip verification (for debugging)
#[verify(skip)]
fn skip_verification() { }
```

### Quantum Verification

```naso
// Verify quantum circuit unitarity
#[verify_quantum]
circuit verified_circuit {
    qubits 2
    H(0)
    CNOT(0, 1)
    // SMT proves this is unitary
}

// Verify quantum algorithm properties
#[verify_quantum(properties = ["entanglement", "no_cloning"])]
circuit algorithm_circuit {
    qubits 3
    // ...
}
```

---

## Advanced Topics

### Async/Await

```naso
async fn fetch_data() -> Result<Data, Error> {
    // Async operation
    let response = http::get("https://api.example.com/data").await?;
    Ok(response.json().await?)
}

fn main() {
    // Run async function
    let data = naso::runtime::block_on(fetch_data());
}
```

### Unsafe Code

```naso
unsafe fn raw_pointer_demo() {
    let mut x = 42;
    let ptr = &mut x as *mut i64;
    
    unsafe {
        *ptr = 10;  // Dereference raw pointer
    }
}

// FFI
extern "C" {
    fn external_function(x: i32) -> i32;
}

fn call_external() {
    unsafe {
        external_function(42);
    }
}
```

### Macros

```naso
// Declarative macro
macro_rules! vec {
    ($($x:expr),*) => {{
        let mut v = Vec::new();
        $(v.push($x);)*
        v
    }};
}

// Usage
let v = vec![1, 2, 3, 4, 5];

// Procedural macros (compile-time code generation)
#[derive(Debug, Clone)]
struct MyStruct {
    field: i64,
}
```

### Const Generics and Compile-Time Computation

```naso
// Compile-time evaluation
const FIB_10: u64 = {
    fn fib(n: u64) -> u64 {
        if n <= 1 { n } else { fib(n-1) + fib(n-2) }
    }
    fib(10)
};

// Array with const generic size
struct Buffer<const N: usize> {
    data: [u8; N],
}

fn process_buffer<const N: usize>(buf: &Buffer<N>) {
    // N is known at compile time
}
```

---

## Appendix: Keywords

```
as         break      const      continue   circuit
else       enum       extern     false      fn
for        if         impl       in         let
loop       match      mod        move       mut
pub        qalloc     qfree      return     struct
trait      true       type       unsafe     use
where      while      async      await      dyn
```

### Reserved Keywords

```
abstract   become     box        do         final
macro      override   priv       ref        try
typeof     unsized    virtual    yield
```

### Quantum Keywords

```
circuit    qubits     gate       measure    qalloc
qfree      H          X          Y          Z
CNOT       CZ         RX         RY         RZ
S          T          SWAP       CPHASE
```

---

## Appendix: Operators

### Arithmetic
```
+  -  *  /  %  **
+= -= *= /= %=
```

### Comparison
```
== != < > <= >=
```

### Logical
```
&& || !
```

### Bitwise
```
& | ^ << >> ~
&= |= ^= <<= >>=
```

### Quantum
```
->  (measurement)
|>  (pipeline)
@   (type annotation)
```

### Other
```
=  .  ..  ...  ::  :  ;  ,  =>  ->
?  ??  #  $  @
```

---

*End of The Naso Book*