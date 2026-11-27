# Naso Language Specification

**Version**: 1.0 (2024 Edition)  
**Status**: Draft for Standardization  
**Repository**: https://github.com/naso-lang/naso

---

## 1. Introduction

Naso is a quantum-first systems programming language with native formal verification, linear/affine types, and Quantity-aware Type Theory (QTT) at its core. This specification defines the complete syntax, static semantics, and dynamic semantics of the Naso programming language.

### 1.1 Design Goals

- **Quantum-First**: Native quantum computing abstractions with classical unification
- **Verified by Default**: Formal verification integrated into the compilation pipeline
- **Performance Dominance**: Zero-cost abstractions matching or exceeding Rust/C++
- **Memory Safety**: Linear/affine types eliminate use-after-free, double-free, data races
- **Standardization Ready**: Multiple implementations, conformance testing, edition system

### 1.2 Scope

This specification covers:
- Lexical structure and grammar (formal BNF)
- Type system (QTT with quantities, linear/affine/unrestricted)
- Static semantics (typing rules, effect system, verification conditions)
- Dynamic semantics (operational semantics, memory model, quantum semantics)
- Standard library core modules
- Edition system (2024, 2027, 2030)

---

## 2. Lexical Structure

### 2.1 Character Set

Naso source files are UTF-8 encoded. The lexical grammar operates on Unicode code points.

### 2.2 Tokens

```
token ::= keyword | identifier | literal | operator | punctuation

keyword ::= 
    'fn' | 'let' | 'mut' | 'const' | 'struct' | 'enum' | 'trait' | 'impl'
  | 'if' | 'else' | 'match' | 'loop' | 'while' | 'for' | 'break' | 'continue' | 'return'
  | 'async' | 'await' | 'try' | 'catch' | 'throw'
  | 'pub' | 'priv' | 'use' | 'mod' | 'as' | 'where'
  | 'type' | 'alias' | 'extern' | 'unsafe' | 'verify'
  | 'quantum' | 'qubit' | 'gate' | 'circuit' | 'measure'
  | 'linear' | 'affine' | 'unrestricted' | 'move' | 'copy' | 'drop'
  | 'effect' | 'handle' | 'with' | 'perform' | 'resume'
  | 'edition' | '2024' | '2027' | '2030'

identifier ::= XID_Start XID_Continue*
  | '_' XID_Continue+

literal ::= integer_literal | float_literal | bool_literal | string_literal | char_literal
  | qubit_literal | circuit_literal

integer_literal ::= decimal_literal | hex_literal | octal_literal | binary_literal
decimal_literal ::= [0-9]+ ( '_' [0-9]+ )* ( 'i8' | 'i16' | 'i32' | 'i64' | 'i128' | 'isize'
  | 'u8' | 'u16' | 'u32' | 'u64' | 'u128' | 'usize' | 'nat' )?

float_literal ::= [0-9]+ '.' [0-9]* ( 'f32' | 'f64' )?
  | [0-9]+ ( 'f32' | 'f64' )

bool_literal ::= 'true' | 'false'

string_literal ::= '"' ( string_char | escape_sequence )* '"'
char_literal ::= '\'' ( char_char | escape_sequence )* '\''

qubit_literal ::= '|' basis_state '⟩'
basis_state ::= '0' | '1' | '+' | '-' | 'i' | '-i'

circuit_literal ::= 'circuit' '{' gate_sequence '}'

operator ::= '+' | '-' | '*' | '/' | '%' | '**'
  | '==' | '!=' | '<' | '>' | '<=' | '>='
  | '&&' | '||' | '!'
  | '&' | '|' | '^' | '<<' | '>>' | '~'
  | '+=' | '-=' | '*=' | '/=' | '%='
  | '->' | '=>' | '::' | '.' | '..' | '...'
  | '|>' | '<|' | '>>=' | '=<<'

punctuation ::= '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':' | '::'
  | '@' | '#' | '$' | '?' | '|'
```

### 2.3 Comments

```
line_comment ::= '//' .* '\n'
block_comment ::= '/*' ( . | '\n' )* '*/'
doc_comment ::= '///' .* '\n' | '/**' ( . | '\n' )* '*/'
```

Comments are whitespace and do not nest (except block comments).

### 2.4 Whitespace

Whitespace includes space (U+0020), tab (U+0009), newline (U+000A), carriage return (U+000D). Whitespace separates tokens but is otherwise insignificant.

---

## 3. Grammar (Formal BNF)

### 3.1 Program Structure

```
program ::= edition_decl? item*

edition_decl ::= 'edition' edition_version ';'
edition_version ::= '2024' | '2027' | '2030'

item ::= extern_crate | use_decl | module_decl | function_decl | type_decl
  | trait_decl | impl_decl | const_decl | static_decl
  | verification_decl | effect_decl | quantum_decl

extern_crate ::= 'extern' 'crate' identifier ';'
use_decl ::= 'use' use_tree ';'
use_tree ::= identifier ( '::' use_tree )? | '{' use_tree ( ',' use_tree )* '}' | 'self' | '*'

module_decl ::= ( 'pub' )? 'mod' identifier ( '{' item* '}' | ';' )
```

### 3.2 Types

```
type ::= primitive_type | compound_type | generic_type | function_type
  | quantum_type | linear_type | effect_type | dependent_type

primitive_type ::= 
    'bool' | 'char' | 'str'
  | 'i8' | 'i16' | 'i32' | 'i64' | 'i128' | 'isize'
  | 'u8' | 'u16' | 'u32' | 'u64' | 'u128' | 'usize'
  | 'f32' | 'f64'
  | 'nat' | 'nat8' | 'nat16' | 'nat32' | 'nat64'

compound_type ::= tuple_type | array_type | slice_type | reference_type | pointer_type

tuple_type ::= '(' type ( ',' type )+ ')' | '(' ')'
array_type ::= '[' type ';' const_expr ']'
slice_type ::= '[' type ']'
reference_type ::= '&' ( 'mut' )? ( lifetime )? type
pointer_type ::= '*' ( 'mut' | 'const' )? type

generic_type ::= identifier ( '<' type_args '>' )?
type_args ::= type ( ',' type )*

function_type ::= ( 'unsafe' )? ( 'extern' abi )? 'fn' ( '<' generics '>' )?
  '(' param_types ')' ( '->' type )?
  ( 'effect' effect_row )?

param_types ::= ( param_type ( ',' param_type )* )?
param_type ::= ( 'mut' )? type

quantum_type ::= 'qubit' | '[' quantity ']' 'Qubit' | 'Circuit' '<' gate_set '>'
  | 'QEC' '<' code_type ',' distance '>'

quantity ::= '1' | 'ω' | '?'  // Linear, Unrestricted, Affine

linear_type ::= '[' quantity ']' type

effect_type ::= 'effect' effect_row
effect_row ::= '{' effect ( ',' effect )* '}'
effect ::= identifier ( '<' type_args '>' )?

dependent_type ::= 'Π' identifier ':' type '.' type  // Dependent function
  | 'Σ' identifier ':' type '.' type  // Dependent pair
  | '{' prop '}'  // Proposition as type
```

### 3.3 Expressions

```
expr ::= literal | path_expr | block_expr | call_expr | method_call
  | field_expr | index_expr | unary_expr | binary_expr | cast_expr
  | if_expr | match_expr | loop_expr | while_expr | for_expr
  | async_expr | await_expr | try_expr | throw_expr
  | quantum_expr | linear_expr | effect_expr | verify_expr
  | closure_expr | macro_expr

path_expr ::= identifier ( '::' identifier )* ( '<' type_args '>' )?

block_expr ::= '{' stmt* ( expr )? '}'

call_expr ::= expr '(' arg_list? ')'
arg_list ::= expr ( ',' expr )* ','?

method_call ::= expr '.' identifier ( '<' type_args '>' )? '(' arg_list? ')'

field_expr ::= expr '.' ( identifier | tuple_index )
tuple_index ::= [0-9]+

index_expr ::= expr '[' expr ']'

unary_expr ::= ( '-' | '!' | '*' | '&' | '&mut' ) expr

binary_expr ::= expr binary_op expr
binary_op ::= '+' | '-' | '*' | '/' | '%' | '**'
  | '==' | '!=' | '<' | '>' | '<=' | '>='
  | '&&' | '||'
  | '&' | '|' | '^' | '<<' | '>>'

cast_expr ::= expr 'as' type

if_expr ::= 'if' expr block_expr ( 'else' ( block_expr | if_expr ) )?

match_expr ::= 'match' expr '{' match_arm* '}'
match_arm ::= pattern ( 'if' expr )? '=>' ( expr | block_expr ) ','

loop_expr ::= 'loop' block_expr
while_expr ::= 'while' expr block_expr
for_expr ::= 'for' pattern 'in' expr block_expr

async_expr ::= 'async' ( move )? block_expr
await_expr ::= expr '.await'

try_expr ::= 'try' block_expr ( 'catch' pattern '=>' block_expr )?
throw_expr ::= 'throw' expr

quantum_expr ::= 'quantum' '{' quantum_stmt* '}'
  | 'qubit' ( '[' quantity ']' )? expr?
  | 'gate' identifier ( '<' type_args '>' )? '(' arg_list? ')'
  | 'circuit' '{' gate_sequence '}'
  | 'measure' expr ( 'in' basis )?

quantum_stmt ::= gate_application | measurement | qubit_allocation | classical_computation
gate_application ::= identifier ( '<' type_args '>' )? '(' arg_list? ) ';'?
measurement ::= 'measure' expr ( 'in' basis )? ';'
qubit_allocation ::= 'qubit' ( '[' quantity ']' )? identifier ( '=' expr )? ';'

linear_expr ::= 'move' expr | 'copy' expr | 'drop' expr

effect_expr ::= 'perform' identifier ( '<' type_args '>' )? '(' arg_list? ')'
  | 'handle' expr 'with' effect_handler
  | 'with' effect_handler 'in' expr

effect_handler ::= '{' handler_clause* '}'
handler_clause ::= identifier ( '<' type_args '>' )? '(' pattern ')' '=>' block_expr

verify_expr ::= 'verify' '{' verification_condition* '}'
verification_condition ::= 'requires' expr ';' | 'ensures' expr ';' | 'invariant' expr ';'

closure_expr ::= ( 'move' )? '|' param_list? '|' ( '->' type )? ( 'effect' effect_row )? block_expr
param_list ::= param ( ',' param )*
param ::= ( 'mut' )? pattern ( ':' type )?

macro_expr ::= identifier '!' ( '(' token_tree* ')' | '[' token_tree* ']' | '{' token_tree* '}' )
```

### 3.4 Statements

```
stmt ::= let_stmt | expr_stmt | item_decl | verify_stmt

let_stmt ::= 'let' ( 'mut' )? pattern ( ':' type )? ( '=' expr )? ';'
expr_stmt ::= expr ';'
item_decl ::= item  // Item declarations allowed in blocks
verify_stmt ::= 'verify' '{' verification_condition* '}' ';'
```

### 3.5 Patterns

```
pattern ::= literal_pattern | identifier_pattern | wildcard_pattern
  | tuple_pattern | struct_pattern | enum_pattern | slice_pattern
  | reference_pattern | or_pattern | guard_pattern

literal_pattern ::= literal
identifier_pattern ::= identifier
wildcard_pattern ::= '_'
tuple_pattern ::= '(' pattern ( ',' pattern )* ')' | '(' ')'
struct_pattern ::= identifier '{' pattern_fields? '}'
pattern_fields ::= pattern_field ( ',' pattern_field )* ','?
pattern_field ::= identifier ( ':' pattern )?
enum_pattern ::= identifier '::' identifier ( '(' pattern_list? ')' )?
slice_pattern ::= '[' pattern ( ',' pattern )* ']'
reference_pattern ::= '&' ( 'mut' )? pattern
or_pattern ::= pattern '|' pattern
guard_pattern ::= pattern 'if' expr
```

### 3.6 Declarations

```
function_decl ::= ( 'pub' )? ( 'async' )? ( 'unsafe' )? ( 'extern' abi )?
  'fn' identifier ( '<' generics '>' )?
  '(' param_list? ')' ( '->' type )?
  ( 'where' where_clauses )?
  ( 'effect' effect_row )?
  ( 'verify' '{' verification_condition* '}' )?
  block_expr

generics ::= generic_param ( ',' generic_param )*
generic_param ::= identifier ( ':' trait_bounds )? ( '=' type )?
  | const_generic_param
const_generic_param ::= 'const' identifier ':' type

where_clauses ::= where_clause ( ',' where_clause )*
where_clause ::= type ':' trait_bounds

trait_bounds ::= trait_bound ( '+' trait_bound )*
trait_bound ::= identifier ( '<' type_args '>' )?

type_decl ::= ( 'pub' )? 'type' identifier ( '<' generics '>' )? '=' type ';'
  | ( 'pub' )? 'struct' identifier ( '<' generics '>' )?
    ( '{' struct_fields '}' | '(' tuple_fields ')' | ';' )
    ( 'where' where_clauses )?
  | ( 'pub' )? 'enum' identifier ( '<' generics '>' )?
    '{' enum_variants '}' ( 'where' where_clauses )?
  | ( 'pub' )? 'union' identifier ( '<' generics '>' )?
    '{' struct_fields '}'

struct_fields ::= struct_field ( ',' struct_field )* ','?
struct_field ::= ( 'pub' )? identifier ':' type
tuple_fields ::= type ( ',' type )* ','?
enum_variants ::= enum_variant ( ',' enum_variant )* ','?
enum_variant ::= identifier ( '(' tuple_fields? ')' | '{' struct_fields '}' )?

trait_decl ::= ( 'pub' )? ( 'unsafe' )? 'trait' identifier ( '<' generics '>' )?
  ( ':' trait_bounds )?
  '{' trait_item* '}'

trait_item ::= 
    ( 'pub' )? 'fn' identifier ( '<' generics '>' )?
      '(' param_list? ')' ( '->' type )? ( 'where' where_clauses )? ';'
  | ( 'pub' )? 'type' identifier ( '<' generics '>' )? ( ':' trait_bounds )? ';'
  | ( 'pub' )? 'const' identifier ':' type ';'

impl_decl ::= ( 'pub' )? 'impl' ( '<' generics '>' )?
  ( identifier ( '<' type_args '>' )? )? 'for' type
  ( 'where' where_clauses )?
  '{' impl_item* '}'

impl_item ::= 
    ( 'pub' )? ( 'async' )? ( 'unsafe' )? 'fn' identifier ( '<' generics '>' )?
      '(' param_list? ')' ( '->' type )? ( 'where' where_clauses )? block_expr
  | ( 'pub' )? 'type' identifier '=' type ';'
  | ( 'pub' )? 'const' identifier ':' type '=' expr ';'

const_decl ::= ( 'pub' )? 'const' identifier ':' type '=' expr ';'
static_decl ::= ( 'pub' )? 'static' ( 'mut' )? identifier ':' type '=' expr ';'

verification_decl ::= 'verify' '{' verification_condition* '}' ';'

effect_decl ::= 'effect' identifier ( '<' generics '>' )?
  '{' effect_operation* '}'

effect_operation ::= 'fn' identifier ( '<' generics '>' )?
  '(' param_list? ')' ( '->' type )? ( 'where' where_clauses )? ';'

quantum_decl ::= 'quantum' 'mod' identifier '{' quantum_item* '}'

quantum_item ::= 
    'gate' identifier ( '<' generics '>' )? '(' param_list? ')' '->' 'Gate'
  | 'circuit' identifier ( '<' generics '>' )? '(' param_list? ')' '->' 'Circuit'
  | 'qec' identifier ( '<' generics '>' )? '(' param_list? ')' '->' 'QEC'
```

### 3.7 Attributes

```
attribute ::= '#' '[' meta_item ']'
meta_item ::= identifier ( '=' expr )? | identifier '(' meta_seq? ')'
meta_seq ::= meta_item ( ',' meta_item )* ','?
```

Standard attributes:
- `#[verify]` - Enable verification for item
- `#[effect]` - Declare effect
- `#[quantum]` - Quantum item
- `#[linear]` / `#[affine]` / `#[unrestricted]` - Quantity annotation
- `#[inline]` / `#[no_inline]` - Inlining hints
- `#[cfg(...)]` - Conditional compilation
- `#[deprecated]` - Deprecation notice

---

## 4. Static Semantics (Type System)

### 4.1 Quantity-aware Type Theory (QTT)

Naso's type system is based on **Quantity-aware Type Theory (QTT)**, extending linear/affine logic with quantitative resource tracking.

#### 4.1.1 Quantities

```
Quantity q ::= 1 | ω | ?

1    : Linear (exactly once) — consumed on use, cannot be duplicated or dropped
ω    : Unrestricted (any number) — freely copyable, droppable
?    : Affine (at most once) — can be dropped but not duplicated
```

#### 4.1.2 Quantity Annotations on Types

Every type carries an implicit or explicit quantity:
- `T` ≡ `[ω] T` (unrestricted by default)
- `[1] T` — Linear type
- `[?] T` — Affine type
- `[ω] T` — Explicit unrestricted

#### 4.1.3 Quantity Subtyping

```
q₁ ≤ q₂  iff:
  1 ≤ 1
  ? ≤ 1, ? ≤ ω
  ω ≤ ω
```

Subtyping allows using a value where fewer uses are required.

#### 4.1.4 Typing Judgment

```
Γ ⊢ e : τ | Δ
```

Where:
- `Γ` — Typing context (variable → (type, quantity))
- `e` — Expression
- `τ` — Result type
- `Δ` — Effect row (set of effects performed)

### 4.2 Linear/Affine Typing Rules

#### 4.2.1 Variable Usage

```
(LINEAR-VAR)
Γ, x : [1] τ ⊢ x : τ | ∅
  consumes x from Γ

(AFFINE-VAR)
Γ, x : [?] τ ⊢ x : τ | ∅
  consumes x from Γ (may be dropped)

(UNRESTRICTED-VAR)
Γ, x : [ω] τ ⊢ x : τ | ∅
  x remains in Γ
```

#### 4.2.2 Function Application

```
(APP)
Γ₁ ⊢ f : [q] (Π(x:τ₁).τ₂) | Δ₁
Γ₂ ⊢ a : τ₁ | Δ₂
q ≤ 1
──────────────────────────────
Γ₁ ⊎ Γ₂ ⊢ f(a) : τ₂[a/x] | Δ₁ ∪ Δ₂
```

The function's quantity must be at least linear (consumed on call).

#### 4.2.3 Tensor Product (Linear Pair)

```
(TENSOR-INTRO)
Γ₁ ⊢ e₁ : τ₁ | Δ₁
Γ₂ ⊢ e₂ : τ₂ | Δ₂
─────────────────────────────
Γ₁ ⊎ Γ₂ ⊢ (e₁, e₂) : τ₁ ⊗ τ₂ | Δ₁ ∪ Δ₂

(TENSOR-ELIM)
Γ ⊢ e : τ₁ ⊗ τ₂ | Δ
Γ, x : [1] τ₁, y : [1] τ₂ ⊢ e' : τ | Δ'
──────────────────────────────────────────
Γ ⊢ let (x, y) = e; e' : τ | Δ ∪ Δ'
```

### 4.3 Effect System

Naso uses **Algebraic Effects** as first-class language features.

#### 4.3.1 Effect Declarations

```
effect Async { fn yield(); fn sleep(duration: Duration); }
effect Try<E> { fn throw(e: E); }
effect IO { fn read(fd: Fd, buf: &mut [u8]) -> usize; fn write(fd: Fd, buf: &[u8]) -> usize; }
effect Quantum { fn allocate() -> Qubit; fn apply(gate: Gate, qs: &[Qubit]); fn measure(q: Qubit) -> bool; }
```

#### 4.3.2 Effect Typing

```
(EFFECT-PERFORM)
Γ ⊢ e : τ | Δ
op ∈ Σ  (operation signature)
──────────────────────────────────────
Γ ⊢ perform op(e) : τ' | Δ ∪ {op}

(EFFECT-HANDLE)
Γ ⊢ e : τ | Δ ∪ {op₁, ..., opₙ}
Γ, h : Handler(op₁, ..., opₙ) ⊢ h : τ' | Δ'
──────────────────────────────────────────────────
Γ ⊢ handle e with h : τ' | Δ ∪ Δ' \ {op₁, ..., opₙ}
```

#### 4.3.3 Built-in Effects

| Effect | Operations | Description |
|--------|------------|-------------|
| `Async` | `yield`, `sleep`, `spawn` | Asynchronous computation |
| `Try<E>` | `throw` | Error handling |
| `Gen<T>` | `yield` | Generators/coroutines |
| `IO` | `read`, `write`, `open`, `close` | I/O operations |
| `Quantum` | `allocate`, `apply`, `measure` | Quantum operations |
| `State<S>` | `get`, `set` | Mutable state |
| `NonDet` | `choose` | Non-determinism |

### 4.4 Dependent Types & Erasure

#### 4.4.1 Dependent Function Types (Π-types)

```
Π(x:τ).σ  —  Function where return type depends on input value
```

#### 4.4.2 Erasure Modes

```
ErasureMode ::= Erase | Check | Retain

Erase   : Proof terms erased at runtime (default)
Check   : Proof terms become runtime assertions
Retain  : Proof terms retained for reflection
```

#### 4.4.3 Relevance Analysis

Types are classified as **relevant** (computational content) or **irrelevant** (proof-only):
- `Prop`, `Proof<T>`, `Σ(x:τ).Prop` are irrelevant
- All other types are relevant

Irrelevant arguments are erased during compilation.

### 4.5 Verification Conditions

Verification conditions are generated from:
- `requires` / `ensures` / `invariant` clauses
- `#[verify]` attribute on functions
- Linear/affine resource usage
- Effect safety (no unhandled effects)
- Quantum unitarity constraints

VCs are discharged by the built-in SMT solver (Z3 backend) or exported to Lean 4 for full mechanized verification.

---

## 5. Dynamic Semantics

### 5.1 Memory Model

Naso uses a **region-based memory model** with linear/affine ownership:

- Each allocation belongs to a region
- Regions have lifetimes tracked by the type system
- Linear values own their region exclusively
- Affine values may share regions (with borrow checking)
- Quantum states have special memory semantics (no-cloning enforced)

### 5.2 Operational Semantics (Small-step)

#### 5.2.1 Configuration

```
Configuration ⟨σ, e⟩
σ : Store (location → value)
e : Expression
```

#### 5.2.2 Reduction Rules (Selected)

```
(β-reduction)
⟨σ, (λx.e) v⟩ → ⟨σ, e[v/x]⟩

(LET)
⟨σ, let x = v; e⟩ → ⟨σ, e[v/x]⟩

(IF-TRUE)
⟨σ, if true { e₁ } else { e₂ }⟩ → ⟨σ, e₁⟩

(IF-FALSE)
⟨σ, if false { e₁ } else { e₂ }⟩ → ⟨σ, e₂⟩

(QUANTUM-ALLOC)
⟨σ, qubit⟩ → ⟨σ[ℓ ↦ |0⟩], ℓ⟩  where ℓ fresh

(QUANTUM-GATE)
⟨σ, gate G(q₁,...,qₙ)⟩ → ⟨σ[G(q₁,...,qₙ)], ()⟩

(QUANTUM-MEASURE)
⟨σ, measure q in Z⟩ → ⟨σ[q ↦ |b⟩], b⟩  where b ∈ {0,1} with prob |α|², |β|²
```

#### 5.2.3 Quantum Semantics

Quantum operations follow the **density matrix formalism** with support for:
- Pure state evolution (unitary gates)
- Mixed states (noise channels)
- Measurement (POVMs)
- Error mitigation (ZNE, PEC, CDR)

### 5.3 Concurrency Model

Naso uses **structured concurrency** with async/await:
- Tasks are spawned with `async { ... }`
- `await` suspends current task
- Task tree ensures no orphaned tasks
- Cancellation propagates down the task tree

### 5.4 Effect Runtime

Effects are implemented via **handler-based interpretation**:
- `perform op(v)` captures continuation
- Handler matches operation and provides implementation
- Resumption continues with handler's result
- Multiple handlers stack (innermost handles first)

---

## 6. Standard Library Core

### 6.1 Core Modules

| Module | Description |
|--------|-------------|
| `std::prelude` | Auto-imported types/traits |
| `std::alloc` | Memory allocation |
| `std::async` | Async runtime primitives |
| `std::channel` | Message passing |
| `std::hashmap` | Hash maps |
| `std::http` | HTTP types |
| `std::mutex` | Synchronization |
| `std::vec` | Dynamic arrays |

### 6.2 Quantum Modules

| Module | Description |
|--------|-------------|
| `std::quantum::primitives` | Qubit, Gate, Circuit types |
| `std::quantum::variational` | VQE, QAOA algorithms |
| `std::quantum::qec` | Quantum error correction |
| `std::quantum::hybrid` | Classical/quantum hybrid |

### 6.3 Edition-Gated Modules

| Module | Edition | Description |
|--------|---------|-------------|
| `std::edition::2027` | 2027+ | New standard library additions |
| `std::edition::2030` | 2030+ | Future additions |

---

## 7. Edition System

### 7.1 Editions

| Edition | Status | Key Features |
|---------|--------|--------------|
| 2024 | Stable | Baseline language (QTT, effects, quantum, verification) |
| 2027 | Planned | Enhanced dependent types, better ergonomics, new stdlib |
| 2030 | Planned | AI-assisted compilation, advanced quantum, formal stdlib |

### 7.2 Edition Declaration

```naso
edition 2024;
```

Must be the first non-comment token in a crate root.

### 7.3 Migration

- Editions are opt-in via `edition` declaration
- `naso fix --edition 2027` automates migration
- No breaking changes within an edition
- Crates of different editions interoperate seamlessly

### 7.4 Edition-Gated Features

```naso
#[cfg(edition = "2027")]
mod new_feature;
```

---

## 8. Conformance Requirements

### 8.1 Implementation Requirements

A conforming Naso implementation must:
1. Accept all programs valid per this specification
2. Reject all programs invalid per this specification
3. Produce observably equivalent behavior for conforming programs
4. Pass the Naso Technology Compatibility Kit (TCK)

### 8.2 Permitted Extensions

Implementations may add:
- Target-specific intrinsics (behind `#[target]` attribute)
- Additional optimization passes
- Extended diagnostics
- Non-standard library modules (prefixed with vendor name)

Extensions must not change the semantics of standard programs.

### 8.3 Diagnostics

Implementations must emit errors for:
- Type errors (with location)
- Linearity/affinity violations
- Effect safety violations
- Verification failures (when `#[verify]` present)
- Edition incompatibilities

Warnings are encouraged for:
- Dead code
- Unused imports
- Deprecated APIs
- Performance hints

---

## 9. Grammar Summary (Complete)

<details>
<summary>Full BNF Grammar (click to expand)</summary>

```
program        ::= edition_decl? item*
edition_decl   ::= 'edition' edition_version ';'
edition_version::= '2024' | '2027' | '2030'

item           ::= extern_crate | use_decl | module_decl | function_decl
               |   type_decl | trait_decl | impl_decl | const_decl
               |   static_decl | verification_decl | effect_decl | quantum_decl

extern_crate   ::= 'extern' 'crate' identifier ';'
use_decl       ::= 'use' use_tree ';'
use_tree       ::= identifier ( '::' use_tree )? | '{' use_tree ( ',' use_tree )* '}' | 'self' | '*'
module_decl    ::= ( 'pub' )? 'mod' identifier ( '{' item* '}' | ';' )

type           ::= primitive_type | compound_type | generic_type | function_type
               |   quantum_type | linear_type | effect_type | dependent_type

primitive_type ::= 'bool' | 'char' | 'str'
               |   'i8' | 'i16' | 'i32' | 'i64' | 'i128' | 'isize'
               |   'u8' | 'u16' | 'u32' | 'u64' | 'u128' | 'usize'
               |   'f32' | 'f64' | 'nat' | 'nat8' | 'nat16' | 'nat32' | 'nat64'

compound_type  ::= tuple_type | array_type | slice_type | reference_type | pointer_type
tuple_type     ::= '(' type ( ',' type )+ ')' | '(' ')'
array_type     ::= '[' type ';' const_expr ']'
slice_type     ::= '[' type ']'
reference_type ::= '&' ( 'mut' )? ( lifetime )? type
pointer_type   ::= '*' ( 'mut' | 'const' )? type

generic_type   ::= identifier ( '<' type_args '>' )?
type_args      ::= type ( ',' type )*

function_type  ::= ( 'unsafe' )? ( 'extern' abi )? 'fn' ( '<' generics '>' )?
               '(' param_types ')' ( '->' type )? ( 'effect' effect_row )?

quantum_type   ::= 'qubit' | '[' quantity ']' 'Qubit' | 'Circuit' '<' gate_set '>'
               |   'QEC' '<' code_type ',' distance '>'

quantity       ::= '1' | 'ω' | '?'
linear_type    ::= '[' quantity ']' type

effect_type    ::= 'effect' effect_row
effect_row     ::= '{' effect ( ',' effect )* '}'
effect         ::= identifier ( '<' type_args '>' )?

dependent_type ::= 'Π' identifier ':' type '.' type
               |   'Σ' identifier ':' type '.' type
               |   '{' prop '}'

expr           ::= literal | path_expr | block_expr | call_expr | method_call
               |   field_expr | index_expr | unary_expr | binary_expr | cast_expr
               |   if_expr | match_expr | loop_expr | while_expr | for_expr
               |   async_expr | await_expr | try_expr | throw_expr
               |   quantum_expr | linear_expr | effect_expr | verify_expr
               |   closure_expr | macro_expr

... (remaining grammar rules as defined in sections 3.3-3.7)
```
</details>

---

## 10. References

1. **QTT Foundation**: "Quantity-aware Type Theory for Quantum Programming" — Naso Team
2. **Effect System**: "Algebraic Effects in Naso" — Naso Team
3. **Verification**: "Proof-Carrying Quantization" — Phase 16 Documentation
4. **Quantum Semantics**: "QIR + Unitary Proofs" — Phase 25/29 Documentation
5. **Self-Hosting**: "CompCert-Style Compiler Correctness" — Phase 27 Documentation

---

## Appendix A: Edition 2024 Feature Set

The 2024 Edition includes the complete feature set as of specification version 1.0:

- ✅ QTT with linear/affine/unrestricted quantities
- ✅ Algebraic effects (Async, Try, Gen, IO, Quantum, State, NonDet)
- ✅ Dependent types with erasure (Π, Σ, Prop)
- ✅ Quantum primitives (qubit, gate, circuit, measure)
- ✅ QEC types (surface, repetition, Steane, color codes)
- ✅ Variational algorithms (VQE, QAOA)
- ✅ Hybrid HPC workflows
- ✅ Formal verification (SMT + Lean 4)
- ✅ Self-hosting verified compiler
- ✅ Distributed compilation
- ✅ WASM GC + Component Model
- ✅ ML-guided optimization
- ✅ Edition system framework

---

*This specification is a living document. For the latest version, see https://github.com/naso-lang/naso/blob/main/spec/LANGUAGE_SPEC.md*