-- TEST: type_safety
-- CATEGORY: verification/lean
-- EXPECT: verification_pass
-- EDITION: 2024
-- DESCRIPTION: Type safety theorem

theorem type_safety : ∀ (e : Expr) (τ : Type),
    ⊢ e : τ →
    (∃ v, e ↝* v ∧ ⊢ v : τ) ∨ (∃ e', e ↝ e') := by sorry
