require mathlib from git
  "https://github.com/leanprover-community/mathlib4.git" @ "v4.13.0-rc1"

@[default_target]
lean_lib Qtt where
  @[default_target]
  lean_lib Syntax where
    -- QTT syntax definitions
  lean_lib Typing where
    -- QTT typing rules and judgments
  lean_lib Semantics where
    -- Operational and denotational semantics
  lean_lib Compiler where
    -- Compiler correctness proofs
  lean_lib Quantum where
    -- Quantum semantics and QIR verification
  lean_lib Polyhedral where
    -- Verified polyhedral optimizer
  lean_lib Bootstrap where
    -- Self-verifying compiler

lean_lib QttTests where
  lean_lib Syntax where
  lean_lib Typing where
  lean_lib Semantics where
  lean_lib Compiler where
  lean_lib Quantum where
  lean_lib Polyhedral where
  lean_lib Bootstrap where