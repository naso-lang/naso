-- TEST: compiler_correctness
-- CATEGORY: verification/lean
-- EXPECT: verification_pass
-- EDITION: 2024
-- DESCRIPTION: Compilation correctness

theorem compiler_correctness : ∀ (e : Expr),
    compile e = Some asm →
    semantics e = semantics_asm asm := by sorry
