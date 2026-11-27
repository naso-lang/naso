# Naso by Example

**Practical examples for learning Naso through code**

---

## Table of Contents

1. [Hello World Variations](#hello-world-variations)
2. [Data Structures](#data-structures)
3. [Algorithms](#algorithms)
4. [File I/O](#file-io)
5. [Concurrency](#concurrency)
6. [Error Handling Patterns](#error-handling-patterns)
7. [Quantum Examples](#quantum-examples)
8. [Verification Examples](#verification-examples)
9. [Package Examples](#package-examples)

---

## Hello World Variations

### Basic Hello World

```naso
fn main() {
    println!("Hello, World!");
}
```

### With Command Line Arguments

```naso
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    match args.len() {
        1 => println!("Hello, World!"),
        2 => println!("Hello, {}!", args[1]),
        _ => {
            println!("Hello, {}!", args[1]);
            println!("And {} others!", args.len() - 2);
        }
    }
}
```

### With Environment Variables

```naso
use std::env;

fn main() {
    let name = env::var("NASO_GREETING").unwrap_or_else(|_| "World".to_string());
    println!("Hello, {}!", name);
}
```

---

## Data Structures

### Linked List

```naso
enum List<T> {
    Nil,
    Cons(T, Box<List<T>>),
}

impl<T> List<T> {
    fn new() -> Self {
        List::Nil
    }
    
    fn prepend(self, elem: T) -> Self {
        List::Cons(elem, Box::new(self))
    }
    
    fn len(&self) -> usize {
        match self {
            List::Nil => 0,
            List::Cons(_, tail) => 1 + tail.len(),
        }
    }
    
    fn iter(&self) -> Iter<T> {
        Iter { next: self }
    }
}

struct Iter<'a, T> {
    next: &'a List<T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    
    fn next(&mut self) -> Option<Self::Item> {
        match self.next {
            List::Nil => None,
            List::Cons(ref elem, ref tail) => {
                self.next = tail;
                Some(elem)
            }
        }
    }
}

fn main() {
    let list = List::new()
        .prepend(3)
        .prepend(2)
        .prepend(1);
    
    println!("Length: {}", list.len());
    for x in list.iter() {
        println!("{}", x);
    }
}
```

### Binary Search Tree

```naso
struct TreeNode<T> {
    value: T,
    left: Option<Box<TreeNode<T>>>,
    right: Option<Box<TreeNode<T>>>,
}

impl<T: Ord> TreeNode<T> {
    fn new(value: T) -> Self {
        TreeNode { value, left: None, right: None }
    }
    
    fn insert(&mut self, value: T) {
        if value < self.value {
            match &mut self.left {
                Some(node) => node.insert(value),
                None => self.left = Some(Box::new(TreeNode::new(value))),
            }
        } else if value > self.value {
            match &mut self.right {
                Some(node) => node.insert(value),
                None => self.right = Some(Box::new(TreeNode::new(value))),
            }
        }
    }
    
    fn contains(&self, value: &T) -> bool {
        match value.cmp(&self.value) {
            std::cmp::Ordering::Less => self.left.as_ref().map_or(false, |n| n.contains(value)),
            std::cmp::Ordering::Greater => self.right.as_ref().map_or(false, |n| n.contains(value)),
            std::cmp::Ordering::Equal => true,
        }
    }
    
    fn inorder(&self) -> Vec<&T> {
        let mut result = Vec::new();
        if let Some(left) = &self.left {
            result.extend(left.inorder());
        }
        result.push(&self.value);
        if let Some(right) = &self.right {
            result.extend(right.inorder());
        }
        result
    }
}

fn main() {
    let mut root = TreeNode::new(5);
    root.insert(3);
    root.insert(7);
    root.insert(1);
    root.insert(9);
    
    println!("Inorder: {:?}", root.inorder());
    println!("Contains 7: {}", root.contains(&7));
    println!("Contains 4: {}", root.contains(&4));
}
```

---

## Algorithms

### Quick Sort

```naso
fn quicksort<T: Ord + Clone>(arr: &mut [T]) {
    if arr.len() <= 1 {
        return;
    }
    
    let pivot_index = partition(arr);
    let (left, right) = arr.split_at_mut(pivot_index);
    quicksort(left);
    quicksort(&mut right[1..]);
}

fn partition<T: Ord>(arr: &mut [T]) -> usize {
    let pivot_index = arr.len() - 1;
    let mut i = 0;
    
    for j in 0..pivot_index {
        if arr[j] <= arr[pivot_index] {
            arr.swap(i, j);
            i += 1;
        }
    }
    
    arr.swap(i, pivot_index);
    i
}

fn main() {
    let mut numbers = vec![64, 34, 25, 12, 22, 11, 90];
    println!("Before: {:?}", numbers);
    quicksort(&mut numbers);
    println!("After:  {:?}", numbers);
}
```

### Binary Search

```naso
fn binary_search<T: Ord>(arr: &[T], target: &T) -> Option<usize> {
    let mut left = 0;
    let mut right = arr.len();
    
    while left < right {
        let mid = left + (right - left) / 2;
        match arr[mid].cmp(target) {
            std::cmp::Ordering::Equal => return Some(mid),
            std::cmp::Ordering::Less => left = mid + 1,
            std::cmp::Ordering::Greater => right = mid,
        }
    }
    
    None
}

fn main() {
    let arr = [1, 3, 5, 7, 9, 11, 13, 15];
    
    println!("Index of 7: {:?}", binary_search(&arr, &7));
    println!("Index of 4: {:?}", binary_search(&arr, &4));
}
```

### Fibonacci with Memoization

```naso
use std::collections::HashMap;

fn fibonacci(n: u64, memo: &mut HashMap<u64, u64>) -> u64 {
    if let Some(&value) = memo.get(&n) {
        return value;
    }
    
    let result = if n <= 1 {
        n
    } else {
        fibonacci(n - 1, memo) + fibonacci(n - 2, memo)
    };
    
    memo.insert(n, result);
    result
}

fn main() {
    let mut memo = HashMap::new();
    for i in 0..20 {
        println!("fib({}) = {}", i, fibonacci(i, &mut memo));
    }
}
```

---

## File I/O

### Reading a File

```naso
use std::fs;
use std::io::{self, BufRead};

fn read_file_lines(path: &str) -> io::Result<Vec<String>> {
    let file = fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    reader.lines().collect()
}

fn main() -> io::Result<()> {
    let lines = read_file_lines("input.txt")?;
    for (i, line) in lines.iter().enumerate() {
        println!("{}: {}", i + 1, line);
    }
    Ok(())
}
```

### Writing a File

```naso
use std::fs;
use std::io::{self, Write};

fn write_file(path: &str, content: &str) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn main() -> io::Result<()> {
    let content = "Hello, Naso!\nThis is a test file.\n";
    write_file("output.txt", content)?;
    println!("File written successfully");
    Ok(())
}
```

### JSON Serialization

```naso
use serde::{Serialize, Deserialize};
use std::fs;

#[derive(Serialize, Deserialize, Debug)]
struct Person {
    name: String,
    age: u32,
    email: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let person = Person {
        name: "Alice".to_string(),
        age: 30,
        email: "alice@example.com".to_string(),
    };
    
    // Serialize to JSON
    let json = serde_json::to_string_pretty(&person)?;
    fs::write("person.json", json)?;
    
    // Deserialize from JSON
    let json = fs::read_to_string("person.json")?;
    let person: Person = serde_json::from_str(&json)?;
    
    println!("{:?}", person);
    Ok(())
}
```

---

## Concurrency

### Thread Spawning

```naso
use std::thread;
use std::time::Duration;

fn main() {
    let handle = thread::spawn(|| {
        for i in 1..=5 {
            println!("Thread: {}", i);
            thread::sleep(Duration::from_millis(100));
        }
    });
    
    for i in 1..=5 {
        println!("Main: {}", i);
        thread::sleep(Duration::from_millis(100));
    }
    
    handle.join().unwrap();
}
```

### Channel Communication

```naso
use std::sync::mpsc;
use std::thread;

fn main() {
    let (tx, rx) = mpsc::channel();
    
    thread::spawn(move || {
        let messages = vec!["Hello", "from", "the", "thread"];
        for msg in messages {
            tx.send(msg).unwrap();
            thread::sleep(Duration::from_millis(100));
        }
    });
    
    for received in rx {
        println!("Received: {}", received);
    }
}
```

### Shared State with Mutex

```naso
use std::sync::{Arc, Mutex};
use std::thread;

fn main() {
    let counter = Arc::new(Mutex::new(0));
    let mut handles = vec![];
    
    for _ in 0..10 {
        let counter = Arc::clone(&counter);
        let handle = thread::spawn(move || {
            let mut num = counter.lock().unwrap();
            *num += 1;
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.join().unwrap();
    }
    
    println!("Result: {}", *counter.lock().unwrap());
}
```

---

## Error Handling Patterns

### Custom Error Types

```naso
use std::fmt;
use std::error::Error;

#[derive(Debug)]
enum MathError {
    DivisionByZero,
    NegativeSquareRoot,
    Overflow,
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MathError::DivisionByZero => write!(f, "Division by zero"),
            MathError::NegativeSquareRoot => write!(f, "Cannot take square root of negative number"),
            MathError::Overflow => write!(f, "Arithmetic overflow"),
        }
    }
}

impl Error for MathError {}

fn divide(a: f64, b: f64) -> Result<f64, MathError> {
    if b == 0.0 {
        Err(MathError::DivisionByZero)
    } else {
        Ok(a / b)
    }
}

fn sqrt(x: f64) -> Result<f64, MathError> {
    if x < 0.0 {
        Err(MathError::NegativeSquareRoot)
    } else {
        Ok(x.sqrt())
    }
}

fn compute(a: f64, b: f64) -> Result<f64, MathError> {
    let div = divide(a, b)?;
    sqrt(div)
}

fn main() {
    match compute(10.0, 2.0) {
        Ok(result) => println!("Result: {}", result),
        Err(e) => println!("Error: {}", e),
    }
}
```

### Result Chaining

```naso
fn read_and_parse(path: &str) -> Result<i64, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let number = content.trim().parse()?;
    Ok(number)
}

fn main() {
    match read_and_parse("number.txt") {
        Ok(n) => println!("Number: {}", n),
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

---

## Quantum Examples

### Bell State Preparation

```naso
circuit bell_state {
    qubits 2
    
    // Create |Φ⁺⟩ = (|00⟩ + |11⟩)/√2
    H(0)
    CNOT(0, 1)
    
    measure(0) -> c0
    measure(1) -> c1
}

fn main() {
    // Run circuit and get results
    let results = naso::quantum::simulate(bell_state, 1000);
    
    for (state, count) in results {
        println!("{}: {} ({:.1}%)", state, count, count as f64 / 10.0);
    }
    // Expected: 00: ~50%, 11: ~50%
}
```

### Quantum Teleportation

```naso
circuit teleportation {
    qubits 3  // q0: message, q1,q2: Bell pair
    clbits 2  // Classical bits for measurement results
    
    // Prepare Bell pair between q1 and q2
    H(1)
    CNOT(1, 2)
    
    // Alice's operations
    CNOT(0, 1)
    H(0)
    
    // Measure Alice's qubits
    measure(0) -> c0
    measure(1) -> c1
    
    // Bob's corrections (conditional)
    // In hardware: classically controlled gates
    // In simulation: apply based on measurement
    if c1 { X(2); }
    if c0 { Z(2); }
    
    // Verify teleportation
    measure(2) -> c2
}
```

### Grover's Algorithm

```naso
circuit grover_3qubit {
    qubits 3
    clbits 3
    
    // Initialize superposition
    H(0); H(1); H(2);
    
    // Oracle: mark state |101⟩ (5)
    // For |101⟩, apply Z to q2 (controlled by q0=1, q1=0)
    X(1);  // Flip q1 so we control on |1⟩
    CCZ(0, 1, 2);  // Toffoli with Z on target
    X(1);  // Restore q1
    
    // Diffusion operator
    H(0); H(1); H(2);
    X(0); X(1); X(2);
    H(2);
    CCZ(0, 1, 2);
    H(2);
    X(0); X(1); X(2);
    H(0); H(1); H(2);
    
    measure(0) -> c0
    measure(1) -> c1
    measure(2) -> c2
}
```

### Quantum Fourier Transform

```naso
fn qft(circuit: &mut Circuit, qubits: &[Qubit]) {
    let n = qubits.len();
    
    for i in 0..n {
        H(qubits[i]);
        
        for j in (i+1)..n {
            let angle = PI / (2.0_f64.powi((j - i) as i32));
            CPHASE(angle, qubits[i], qubits[j]);
        }
    }
    
    // Swap for correct order
    for i in 0..n/2 {
        SWAP(qubits[i], qubits[n-1-i]);
    }
}

circuit qft_4qubit {
    qubits 4
    
    // QFT on qubits 0-3
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
    
    SWAP(0, 3)
    SWAP(1, 2)
}
```

### Variational Quantum Eigensolver (VQE)

```naso
fn vqe_ansatz(params: &[f64], qubits: &[Qubit]) {
    // Hardware-efficient ansatz
    let n = qubits.len();
    let mut param_idx = 0;
    
    // Rotation layer
    for i in 0..n {
        RY(params[param_idx], qubits[i]);
        param_idx += 1;
        RZ(params[param_idx], qubits[i]);
        param_idx += 1;
    }
    
    // Entanglement layer
    for i in 0..n-1 {
        CNOT(qubits[i], qubits[i+1]);
    }
    CNOT(qubits[n-1], qubits[0]);  // Circular
    
    // Another rotation layer
    for i in 0..n {
        RY(params[param_idx], qubits[i]);
        param_idx += 1;
        RZ(params[param_idx], qubits[i]);
        param_idx += 1;
    }
}

circuit vqe_h2 {
    qubits 4  // 4 qubits for H2 molecule
    
    // Parameters would be optimized classically
    // This shows the structure
    vqe_ansatz(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 
                 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6], 
               &[q0, q1, q2, q3]);
}
```

---

## Verification Examples

### Verified Arithmetic

```naso
#[verify]
fn add_checked(a: u32, b: u32) -> u32
    requires a + b < u32::MAX
    ensures result == a + b
{
    a + b
}

#[verify]
fn mul_checked(a: u32, b: u32) -> u32
    requires a * b < u32::MAX
    ensures result == a * b
{
    a * b
}

fn main() {
    // These will be verified at compile time
    let sum = add_checked(100, 200);
    let product = mul_checked(10, 20);
    println!("{} {}", sum, product);
}
```

### Verified Data Structure

```naso
#[verify]
fn binary_search_verified(arr: &[i32], target: i32) -> Option<usize>
    requires arr.is_sorted()
    ensures match result {
        Some(idx) => arr[idx] == target,
        None => !arr.contains(&target),
    }
{
    let mut left = 0;
    let mut right = arr.len();
    
    while left < right
        invariant left <= right
        invariant right <= arr.len()
        invariant forall i in 0..left: arr[i] < target
        invariant forall i in right..arr.len(): arr[i] >= target
    {
        let mid = left + (right - left) / 2;
        if arr[mid] < target {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    
    if left < arr.len() && arr[left] == target {
        Some(left)
    } else {
        None
    }
}
```

### Quantum Circuit Verification

```naso
#[verify_quantum]
circuit verified_bell {
    qubits 2
    H(0)
    CNOT(0, 1)
    // SMT proves: output state is (|00⟩ + |11⟩)/√2
}

#[verify_quantum(properties = ["unitarity", "entanglement"])]
circuit verified_ghz {
    qubits 3
    H(0)
    CNOT(0, 1)
    CNOT(0, 2)
    // Proves: creates GHZ state, is unitary, has genuine tripartite entanglement
}
```

---

## Package Examples

### Creating a Package

```bash
naso new my-quantum-lib
cd my-quantum-lib
```

### Package Structure

```
my-quantum-lib/
├── naso.toml
├── src/
│   ├── lib.naso
│   ├── circuits/
│   │   ├── mod.naso
│   │   ├── bell.naso
│   │   └── ghz.naso
│   └── algorithms/
│       ├── mod.naso
│       └── grover.naso
├── tests/
│   └── integration_test.naso
├── examples/
│   └── run_bell.naso
└── benches/
    └── benchmark.naso
```

### Publishing

```bash
# Build and test
naso build --release
naso test

# Publish to registry
naso pkg publish
```

### Using Dependencies

```toml
# naso.toml
[package]
name = "my-app"
version = "0.1.0"

[dependencies]
my-quantum-lib = "0.1"
naso-std = "0.1"
```

```naso
// src/main.naso
use my_quantum_lib::circuits::bell_state;
use my_quantum_lib::algorithms::grover;

fn main() {
    let results = naso::quantum::simulate(bell_state, 1000);
    // ...
}
```

---

*End of Naso by Example*