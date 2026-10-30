---
title: "Quantitative Type Theory (QTT)"
weight: 10
---

Naso integrates **Quantitative Type Theory (QTT)** into its core type-checker to govern memory safety, quantum state conservation, and zero-cost compile-time proofs without a garbage collector.

## Quantity Semantics

...

$$\frac{\Gamma \vdash e : [q] \tau \quad q = q_1 + q_2)}{\Gamma_1, \Gamma_2 \vdash e : [q_1] \tau \otimes [q_2] \tau}$$
where $\Gamma = \Gamma_1 \oplus \Gamma_2$ (disjoint split).

### Linear Consumption
$$\frac{\Gamma, x:[1]\tau \vdash e : \sigma)}{\Gamma \vdash \text{let } _ = x \text{ in } e : \sigma}$$
Variable $x$ must be used exactly once in $e$.

### Erased Elimination
$$\frac{\Gamma \vdash e : [0] \tau)}{\Gamma \vdash \text{erase}(e) : \text{void}}$$
Any `[0]`-qualified value is erased to `void` during code generation.

### Bounded Loop Scaling
$$\frac{\Gamma \vdash e : [N] \tau \quad \text{loop\_bound}(L) = M)}{\Gamma \vdash \text{for } i \text{ in } 0..L \{ e \} : [N \cdot M] \tau}$$
Quantity scales linearly with loop iterations.

### Subtyping
$$\frac{q_1 \leq q_2}{[q_1]\tau \leq [q_2]\tau}$$
where $0 \leq 1 \leq N \leq *$.

---

## Code Lowering Comparison

...

// uncompute block automatically generates adjoint
    uncompute {
        // This block is inverted and appended
        // Result: ancilla returned to |0>
    }
}
// After function: ancilla is |0>, x holds result
```