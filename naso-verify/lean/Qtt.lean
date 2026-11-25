/-- QTT (Quantity-aware Type Theory) Formalization

Main module exporting all QTT components.
-/

namespace Qtt

@[doc "QTT Syntax: quantities, types, terms, contexts"]
include Syntax

@[doc "QTT Typing Rules: typing judgments, substitution, preservation, progress"]
include Typing

@[doc "QTT Semantics: operational semantics, quantum state, no-cloning"]
include Semantics

@[doc "QTT Compiler Correctness: CompCert-style forward/backward simulation"]
include Compiler

@[doc "QTT Quantum Verification: QIR, unitary proofs, error correction"]
include Quantum

@[doc "QTT Polyhedral Optimizer: verified loop transformations"]
include Polyhedral

@[doc "QTT Self-Verifying Compiler: bootstrap, certified compilation"]
include Bootstrap

end Qtt