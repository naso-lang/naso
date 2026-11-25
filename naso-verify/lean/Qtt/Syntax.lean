/-- QTT (Quantity-aware Type Theory) Formalization in Lean 4

This file defines the abstract syntax of QTT, including:
- Quantity kinds (0, 1, ω)
- Types with quantity annotations
- Terms with linear/affine/relevant usage
- Quantum-specific constructs (qubits, gates, circuits)
- -/

namespace Qtt

/-- Quantity kinds in QTT -/
inductive QtyKind : Type where
  | zero : QtyKind      -- 0: erased/unused
  | one  : QtyKind      -- 1: linear (used exactly once)
  | many : QtyKind      -- ω: unrestricted (used any number of times)

/-/
  Quantity ordering: 0 ≤ 1 ≤ ω
  This is used for subtyping and quantity polymorphism
/-/
instance : LE QtyKind :=
  ⟨fun q1 q2 =>
    match q1, q2 with
    | QtyKind.zero, _ => true
    | QtyKind.one, QtyKind.zero => false
    | QtyKind.one, _ => true
    | QtyKind.many, QtyKind.many => true
    | QtyKind.many, _ => false⟩

instance : PartialOrder QtyKind :=
  ⟨by
    intro q
    simp [QtyKind]
    <;> aesop,
   by
    intro q1 q2 h1 h2
    simp [QtyKind, LE] at h1 h2 ⊢
    <;> aesop,
   by
    intro q1 q2 q3 h1 h2
    simp [QtyKind, LE] at h1 h2 ⊢
    <;> aesop⟩

/-/
  Quantity addition (for combining usage):
  0 + q = q
  1 + 1 = ω
  1 + ω = ω
  ω + _ = ω
  _ + ω = ω
/-/
def qtyAdd : QtyKind → QtyKind → QtyKind
  | QtyKind.zero, q => q
  | q, QtyKind.zero => q
  | QtyKind.one, QtyKind.one => QtyKind.many
  | QtyKind.one, QtyKind.many => QtyKind.many
  | QtyKind.many, _ => QtyKind.many

/-/
  Quantity multiplication (for nesting):
  0 * _ = 0
  1 * q = q
  ω * 1 = ω
  ω * ω = ω
/-/
def qtyMul : QtyKind → QtyKind → QtyKind
  | QtyKind.zero, _ => QtyKind.zero
  | _, QtyKind.zero => QtyKind.zero
  | QtyKind.one, q => q
  | q, QtyKind.one => q
  | QtyKind.many, QtyKind.many => QtyKind.many
  | QtyKind.many, _ => QtyKind.many
  | _, QtyKind.many => QtyKind.many

/-/
  Quantity lattice operations
/-/
def qtyJoin : QtyKind → QtyKind → QtyKind
  | QtyKind.zero, q => q
  | q, QtyKind.zero => q
  | QtyKind.one, QtyKind.one => QtyKind.one
  | QtyKind.one, QtyKind.many => QtyKind.many
  | QtyKind.many, QtyKind.one => QtyKind.many
  | QtyKind.many, QtyKind.many => QtyKind.many

def qtyMeet : QtyKind → QtyKind → QtyKind
  | QtyKind.zero, _ => QtyKind.zero
  | _, QtyKind.zero => QtyKind.zero
  | QtyKind.one, QtyKind.one => QtyKind.one
  | QtyKind.one, QtyKind.many => QtyKind.one
  | QtyKind.many, QtyKind.one => QtyKind.one
  | QtyKind.many, QtyKind.many => QtyKind.many

/-/
  Nat kinds for const generics
/-/
inductive NatKind : Type where
  | zero : NatKind
  | succ : NatKind → NatKind
  | var  : String → NatKind  -- Type-level variable

/-/
  Base types in QTT
/-/
inductive BaseType : Type where
  | unit   : BaseType
  | bool   : BaseType
  | int    : BaseType
  | float  : BaseType
  | qubit  : BaseType  -- Quantum qubit type
  | bit    : BaseType  -- Classical bit (measurement result)
  | string : BaseType
  | char   : BaseType

/-/
  Type constructors with quantity annotations
/-/
inductive Type : Type where
  | base      : BaseType → Type
  | arrow     : QtyKind → Type → Type → Type  -- q → A → B (function with argument usage q)
  | tensor    : Type → Type → Type            -- A ⊗ B (tensor product, both linear)
  | sum       : Type → Type → Type            -- A ⊕ B (sum type)
  | forall    : String → QtyKind → Type → Type → Type  -- ∀(x:q A). B (dependent function)
  | exists    : String → QtyKind → Type → Type → Type  -- ∃(x:q A). B (dependent pair)
  | array     : Type → NatKind → Type         -- Array A n
  | ref       : QtyKind → Type → Type         -- Ref q A (reference with usage q)
  | quantum   : Type → Type                   -- Quantum circuit type
  | classical : Type → Type                   -- Classical data (erased at quantum boundary)
  | nat       : NatKind → Type                -- Type-level natural
  | var       : String → Type                 -- Type variable

/-/
  Terms in QTT
/-/
inductive Term : Type where
  | var       : String → Term                    -- Variable
  | unit      : Term                             -- ()
  | bool      : Bool → Term                      -- true/false
  | int       : ℤ → Term                         -- Integer literal
  | float     : Float → Term                     -- Float literal
  | string    : String → Term                    -- String literal
  | char      : Char → Term                      -- Char literal
  | lam       : String → QtyKind → Type → Term → Term  -- λ(x:q A). t
  | app       : Term → Term → Term               -- t u
  | pair      : Term → Term → Term               -- (t, u)
  | fst       : Term → Term                      -- fst t
  | snd       : Term → Term                      -- snd t
  | inl       : Term → Type → Term               -- inl t : A ⊕ B
  | inr       : Term → Type → Term               -- inr t : A ⊕ B
  | case      : Term → String → Term → String → Term → Term  -- case t of inl x => u | inr y => v
  | let_      : String → QtyKind → Term → Term → Term  -- let x:q = t in u
  | ann       : Term → Type → Term               -- (t : A)
  | arrayLit  : List Term → Term                 -- [t1, t2, ...]
  | arrayGet  : Term → Term → Term               -- a[i]
  | arraySet  : Term → Term → Term → Term        -- a[i] = v
  | ref       : Term → Term                      -- ref t
  | deref     : Term → Term                      -- !t
  | assign    : Term → Term → Term               -- t := u
  -- Quantum constructs
  | qalloc    : Term → Term                      -- qalloc n (allocate n qubits)
  | qfree     : Term → Term                      -- qfree q (free qubit)
  | gate      : String → List Term → Term        -- gate "H" [q1, q2]
  | measure   : Term → Term                      -- measure q
  | cnot      : Term → Term → Term               -- cnot control target
  | hadamard  : Term → Term                      -- H q
  | phase     : Float → Term → Term              -- Phase θ q
  | tGate     : Term → Term                      -- T q
  | tdagGate  : Term → Term                      -- T† q
  -- Classical control
  | if_       : Term → Term → Term → Term        -- if c then t else u
  | while     : Term → Term → Term               -- while c do t
  -- Dependent types
  | typeLam   : String → QtyKind → Type → Term → Term  -- Λ(x:q A). t
  | typeApp   : Term → Type → Term               -- t [A]
  | pack      : Type → Term → Type → Term        -- pack [A, t] as ∃x:q A. B
  | unpack    : Term → String → String → Term → Term  -- unpack [x, y] = t in u
  -- Proof terms (erased)
  | proof     : Term                             -- Proof term (erased at runtime)
  | trivial   : Term                             -- Trivial proof

/-/
  Typing context with quantity annotations
/-/
structure ContextEntry where
  name : String
  qty  : QtyKind
  ty   : Type

def Context := List ContextEntry

/-/
  Typing judgment: Γ ⊢ t : A
/-/
inductive HasType : Context → Term → Type → Prop where
  | var : ∀ {Γ x q A}, (ContextEntry.mk x q A) ∈ Γ → HasType Γ (Term.var x) A
  | unit : ∀ {Γ}, HasType Γ Term.unit (Type.base BaseType.unit)
  | bool : ∀ {Γ b}, HasType Γ (Term.bool b) (Type.base BaseType.bool)
  | int : ∀ {Γ n}, HasType Γ (Term.int n) (Type.base BaseType.int)
  | float : ∀ {Γ f}, HasType Γ (Term.float f) (Type.base BaseType.float)
  | string : ∀ {Γ s}, HasType Γ (Term.string s) (Type.base BaseType.string)
  | char : ∀ {Γ c}, HasType Γ (Term.char c) (Type.base BaseType.char)
  | lam : ∀ {Γ x q A B t},
      HasType (ContextEntry.mk x q A :: Γ) t B →
      HasType Γ (Term.lam x q A t) (Type.arrow q A B)
  | app : ∀ {Γ t u q A B},
      HasType Γ t (Type.arrow q A B) →
      HasType Γ u A →
      HasType Γ (Term.app t u) B
  | pair : ∀ {Γ t u A B},
      HasType Γ t A →
      HasType Γ u B →
      HasType Γ (Term.pair t u) (Type.tensor A B)
  | fst : ∀ {Γ t A B},
      HasType Γ t (Type.tensor A B) →
      HasType Γ (Term.fst t) A
  | snd : ∀ {Γ t A B},
      HasType Γ t (Type.tensor A B) →
      HasType Γ (Term.snd t) B
  | inl : ∀ {Γ t A B},
      HasType Γ t A →
      HasType Γ (Term.inl t B) (Type.sum A B)
  | inr : ∀ {Γ t A B},
      HasType Γ t B →
      HasType Γ (Term.inr t A) (Type.sum A B)
  | case : ∀ {Γ t A B C x u y v},
      HasType Γ t (Type.sum A B) →
      HasType (ContextEntry.mk x QtyKind.one A :: Γ) u C →
      HasType (ContextEntry.mk y QtyKind.one B :: Γ) v C →
      HasType Γ (Term.case t x u y v) C
  | let_ : ∀ {Γ x q t u A B},
      HasType Γ t A →
      HasType (ContextEntry.mk x q A :: Γ) u B →
      HasType Γ (Term.let_ x q t u) B
  | ann : ∀ {Γ t A},
      HasType Γ t A →
      HasType Γ (Term.ann t A) A
  | arrayLit : ∀ {Γ ts A},
      (∀ t ∈ ts, HasType Γ t A) →
      HasType Γ (Term.arrayLit ts) (Type.array A (NatKind.zero))
  | arrayGet : ∀ {Γ a i A n},
      HasType Γ a (Type.array A n) →
      HasType Γ i (Type.base BaseType.int) →
      HasType Γ (Term.arrayGet a i) A
  | arraySet : ∀ {Γ a i v A n},
      HasType Γ a (Type.array A n) →
      HasType Γ i (Type.base BaseType.int) →
      HasType Γ v A →
      HasType Γ (Term.arraySet a i v) (Type.base BaseType.unit)
  | ref : ∀ {Γ t q A},
      HasType Γ t A →
      HasType Γ (Term.ref t) (Type.ref q A)
  | deref : ∀ {Γ t q A},
      HasType Γ t (Type.ref q A) →
      HasType Γ (Term.deref t) A
  | assign : ∀ {Γ t u q A},
      HasType Γ t (Type.ref q A) →
      HasType Γ u A →
      HasType Γ (Term.assign t u) (Type.base BaseType.unit)
  -- Quantum typing rules
  | qalloc : ∀ {Γ n},
      HasType Γ n (Type.base BaseType.int) →
      HasType Γ (Term.qalloc n) (Type.quantum (Type.array (Type.base BaseType.qubit) (NatKind.zero)))
  | qfree : ∀ {Γ q},
      HasType Γ q (Type.base BaseType.qubit) →
      HasType Γ (Term.qfree q) (Type.base BaseType.unit)
  | gate : ∀ {Γ name args},
      HasType Γ (Term.gate name args) (Type.base BaseType.unit)  -- Simplified
  | measure : ∀ {Γ q},
      HasType Γ q (Type.base BaseType.qubit) →
      HasType Γ (Term.measure q) (Type.base BaseType.bit)
  | cnot : ∀ {Γ c t},
      HasType Γ c (Type.base BaseType.qubit) →
      HasType Γ t (Type.base BaseType.qubit) →
      HasType Γ (Term.cnot c t) (Type.base BaseType.unit)
  | hadamard : ∀ {Γ q},
      HasType Γ q (Type.base BaseType.qubit) →
      HasType Γ (Term.hadamard q) (Type.base BaseType.unit)
  | phase : ∀ {Γ θ q},
      HasType Γ θ (Type.base BaseType.float) →
      HasType Γ q (Type.base BaseType.qubit) →
      HasType Γ (Term.phase θ q) (Type.base BaseType.unit)
  | tGate : ∀ {Γ q},
      HasType Γ q (Type.base BaseType.qubit) →
      HasType Γ (Term.tGate q) (Type.base BaseType.unit)
  | tdagGate : ∀ {Γ q},
      HasType Γ q (Type.base BaseType.qubit) →
      HasType Γ (Term.tdagGate q) (Type.base BaseType.unit)
  -- Classical control
  | if_ : ∀ {Γ c t u A},
      HasType Γ c (Type.base BaseType.bool) →
      HasType Γ t A →
      HasType Γ u A →
      HasType Γ (Term.if_ c t u) A
  | while : ∀ {Γ c t},
      HasType Γ c (Type.base BaseType.bool) →
      HasType Γ t (Type.base BaseType.unit) →
      HasType Γ (Term.while c t) (Type.base BaseType.unit)
  -- Dependent types
  | typeLam : ∀ {Γ x q A B t},
      HasType (ContextEntry.mk x q A :: Γ) t B →
      HasType Γ (Term.typeLam x q A t) (Type.forall x q A B)
  | typeApp : ∀ {Γ t x q A B A'},
      HasType Γ t (Type.forall x q A B) →
      HasType Γ (Term.typeApp t A') (B[A' / x])
  | pack : ∀ {Γ A t B x q A'},
      HasType Γ t A' →
      HasType Γ (Term.pack A t B) (Type.exists x q A' B)
  | unpack : ∀ {Γ t x y u C A q A' B},
      HasType Γ t (Type.exists x q A' B) →
      HasType (ContextEntry.mk x q A' :: ContextEntry.mk y QtyKind.one (B[A' / x]) :: Γ) u C →
      HasType Γ (Term.unpack t x y u) C
  -- Proof terms
  | proof : ∀ {Γ t A},
      HasType Γ t A →
      HasType Γ (Term.proof t) A
  | trivial : ∀ {Γ A},
      HasType Γ Term.trivial A

/-/
  Subtyping with quantity awareness
/-/
inductive Subtype : Type → Type → Prop where
  | refl : ∀ A, Subtype A A
  | trans : ∀ A B C, Subtype A B → Subtype B C → Subtype A C
  | qty : ∀ {q1 q2 : QtyKind} {A B : Type},
      q1 ≤ q2 → Subtype A B → Subtype (Type.arrow q1 A B) (Type.arrow q2 A B)
  | quantum_classical : ∀ A, Subtype (Type.quantum A) (Type.classical A)
  | array : ∀ {A B n}, Subtype A B → Subtype (Type.array A n) (Type.array B n)

/-/
  Quantity erasure: types with q=0 are erased at runtime
/-/
def eraseQty : Type → Type
  | Type.base b => Type.base b
  | Type.arrow q A B => if q = QtyKind.zero then eraseQty B else Type.arrow q (eraseQty A) (eraseQty B)
  | Type.tensor A B => Type.tensor (eraseQty A) (eraseQty B)
  | Type.sum A B => Type.sum (eraseQty A) (eraseQty B)
  | Type.forall x q A B => if q = QtyKind.zero then eraseQty B else Type.forall x q (eraseQty A) (eraseQty B)
  | Type.exists x q A B => if q = QtyKind.zero then eraseQty B else Type.exists x q (eraseQty A) (eraseQty B)
  | Type.array A n => Type.array (eraseQty A) n
  | Type.ref q A => Type.ref q (eraseQty A)
  | Type.quantum A => Type.quantum (eraseQty A)
  | Type.classical A => Type.classical (eraseQty A)
  | Type.nat n => Type.nat n
  | Type.var x => Type.var x

/-/
  Linearity checking: ensures linear variables are used exactly once
/-/
def linearVars : Term → List String
  | Term.var x => [x]
  | Term.lam x q A t => linearVars t.filter (· ≠ x)
  | Term.app t u => linearVars t ++ linearVars u
  | Term.pair t u => linearVars t ++ linearVars u
  | Term.fst t => linearVars t
  | Term.snd t => linearVars t
  | Term.inl t _ => linearVars t
  | Term.inr t _ => linearVars t
  | Term.case t x u y v => (linearVars t) ++ (linearVars u.filter (· ≠ x)) ++ (linearVars v.filter (· ≠ y))
  | Term.let_ x q t u => (linearVars t) ++ (linearVars u.filter (· ≠ x))
  | Term.ann t _ => linearVars t
  | Term.arrayLit ts => ts.bind linearVars
  | Term.arrayGet a i => linearVars a ++ linearVars i
  | Term.arraySet a i v => linearVars a ++ linearVars i ++ linearVars v
  | Term.ref t => linearVars t
  | Term.deref t => linearVars t
  | Term.assign t u => linearVars t ++ linearVars u
  | Term.qalloc n => linearVars n
  | Term.qfree q => linearVars q
  | Term.gate _ args => args.bind linearVars
  | Term.measure q => linearVars q
  | Term.cnot c t => linearVars c ++ linearVars t
  | Term.hadamard q => linearVars q
  | Term.phase θ q => linearVars θ ++ linearVars q
  | Term.tGate q => linearVars q
  | Term.tdagGate q => linearVars q
  | Term.if_ c t u => linearVars c ++ linearVars t ++ linearVars u
  | Term.while c t => linearVars c ++ linearVars t
  | Term.typeLam x q A t => linearVars t.filter (· ≠ x)
  | Term.typeApp t _ => linearVars t
  | Term.pack _ t _ => linearVars t
  | Term.unpack t x y u => linearVars t ++ linearVars u.filter (· ≠ x).filter (· ≠ y)
  | Term.proof t => linearVars t
  | Term.trivial => []
  | _ => []

def checkLinearity : Context → Term → Prop
  | Γ, t =>
    let vars := linearVars t
    let linearCtx := Γ.filter (·.qty = QtyKind.one).map (·.name)
    vars.count = linearCtx.length ∧ vars.toSet = linearCtx.toSet

end Qtt