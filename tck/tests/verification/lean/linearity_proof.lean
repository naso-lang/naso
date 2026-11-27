-- TEST: linearity_proof
-- CATEGORY: verification/lean
-- EXPECT: verification_pass
-- EDITION: 2024
-- DESCRIPTION: Linearity proof

theorem linearity : ∀ (Γ : Context) (e : Expr) (τ : Type),
    Γ ⊢ e : [1] τ →
    (∃ v, e ↝* v ∧ Γ ⊢ v : [1] τ) := by sorry
