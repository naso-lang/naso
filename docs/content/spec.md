---
title: "Language Specification"
weight: 20
---

# Naso Language Specification v1.0-alpha
This document specifies the syntax and semantics of the Naso programming language, a statically-typed systems language integrating Quantitative Type Theory (QTT), Mutable Value Semantics (MVS), and quantum state conservation through automatic uncomputation.

## 1. Lexical & Syntactic Grammar

### 1.1 Tokens
Naso source code consists of the following tokens:

- **Keywords**: `fn`, `let`, `inout`, `if`, `else`, `for`, `while`, `match`, `return`, `uncompute`, `qalloc`, `qfree`, `hadamard`, `cnot`, `measure`, `x`, `y`, `z`, `proof`, `Vec`, `Qubit`, `bool`, `int`, `f64`, `f32`, `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `usize`, `isize`
- **Quantity Annotations**: `[0]`, `[1]`, `[*]`, `[N]` where `N` is a positive integer or affine expression
- **Operators**: `+`, `-`, `*`, `/`, `%`, `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, `&`, `|`, `^`, `<<`, `>>`, `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`, `->`, `=>`, `..`, `...`
- **Delimiters**: `(`, `)`, `{`, `}`, `[`, `]`, `,`, `;`, `:`, `.`, `->`, `=>`
- **Literals**: Integer (`123`), floating-point (`3.14`), boolean (`true`, `false`), character (`'a'`), string (`"hello"`), quantum state (`|0>`, `|1>`)
- **Identifiers**: Letter or underscore followed by letters, digits, underscores (`_name`, `value1`, `q0`)
### 1.2 Grammar (EBNF)

```
program        = { declaration } ;
declaration    = function_decl | variable_decl | constant_decl | type_alias ;
function_decl  = "fn" identifier "(" [ parameter_list ] ")" [ "->" type ] block ;
parameter_list = parameter { "," parameter } ;
parameter      = [ "inout" ] identifier ":" quantity type ;
quantity       = "[" ( "0" | "1" | "*" | N ) "]" ;
N              = integer | affine_expression ;
affine_expression = integer | identifier | affine_expression ( "+" | "-" ) integer | integer ( "*" | "/" ) identifier ;
type           = primitive_type | array_type | vector_type | proof_type | qubit_type | tuple_type ;
primitive_type = "bool" | "int" | "f64" | "f32" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "usize" | "isize" ;
array_type     = type "[" integer "]" ;
vector_type    = "Vec" "<" type ">" ;
proof_type     = "[0] Proof" ;
qubit_type     = "Qubit" ;
tuple_type     = "(" [ type { "," type } ] ")" ;
block          = "{" { statement } "}" ;
statement      = let_stmt | inout_stmt | expr_stmt | if_stmt | for_stmt | while_stmt | match_stmt | return_stmt | uncompute_stmt | quantum_stmt ;
let_stmt       = "let" identifier ":" quantity type [ "=" expression ] ";" ;
inout_stmt     = "inout" identifier ":" quantity type ";" ;
expr_stmt      = expression ";" ;
if_stmt        = "if" "(" expression ")" block [ "else" ( if_stmt | block ) ] ;
for_stmt       = "for" identifier "in" range_expression block ;
range_expression = integer ".." integer | integer "..=" integer ;
while_stmt     = "while" "(" expression ")" block ;
match_stmt     = "match" "(" expression ")" "{" { match_arm } "}" ;
match_arm      = pattern "=>" block | pattern "=>" expression "," ;
pattern        = identifier | literal | "_" | "(" pattern { "," pattern } ")" ;
return_stmt    = "return" [ expression ] ";" ;
uncompute_stmt = "uncompute" block ;
quantum_stmt   = ( "hadamard" | "cnot" | "measure" | "x" | "y" | "z" ) "(" [ "inout" ] expression { "," [ "inout" ] expression } ")" ;
expression     = literal | identifier | unary_op expression | expression binary_op expression | function_call | array_access | field_access | cast ;
unary_op       = "+" | "-" | "!" | "~" ;
binary_op      = "+" | "-" | "*" | "/" | "%" | "==" | "!=" | "<" | "\leq" | ">" | ">=" | "&&" | "||" | "&" | "|" | "^" | "<<" | ">>" ;
function_call  = identifier "(" [ expression { "," expression } ] ")" ;
array_access   = expression "[" expression "]" ;
field_access   = expression "." identifier ;
cast           = "as" type ;
```

### 1.3 Concrete Code Snippets

#### Quantum Primitives: Bell State Preparation

```naso
fn create_bell_pair(inout q0: [1] Qubit, inout q1: [1] Qubit) -> [0] Proof {
    hadamard(inout q0);
    cnot(inout q0, inout q1);
}
```