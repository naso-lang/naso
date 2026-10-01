//! Type Environment for Naso Type Checker
//!
//! Tracks variables with their types, quantities, and mutabilities.
//! Manages usage counting for linear variables, inout borrows, and consume moves.

#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]

use crate::ast::*;
use crate::typecheck::error::TypeError;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

/// Information about a variable binding
#[derive(Debug, Clone)]
pub struct VarInfo {
    /// The variable's type
    pub ty: Type,
    /// Quantity annotation (0, 1, N, *)
    pub quantity: Quantity,
    /// Mutability mode (imm, inout, consume)
    pub mutability: Mutability,
    /// Source span where defined
    pub defined_at: Span,
    /// Spans where this variable was used
    pub used_at: Vec<Span>,
    /// Whether this variable has been moved (for consume)
    pub moved: bool,
    /// Whether this variable is erased (quantity 0)
    pub erased: bool,
}

/// The use-state of a single variable, snapshotted across a proof block.
///
/// A proof block is erased: it reads values to state obligations but does not
/// consume them. Restoring this state afterwards means an obligation can
/// mention a `[1]` linear value without the enclosing runtime code seeing a
/// spurious second use.
#[derive(Debug, Clone)]
pub struct VarUseState {
    used_at: Vec<Span>,
    moved: bool,
}

impl VarUseState {
    fn from_info(info: &VarInfo) -> Self {
        Self {
            used_at: info.used_at.clone(),
            moved: info.moved,
        }
    }

    fn apply_to(&self, info: &mut VarInfo) {
        info.used_at = self.used_at.clone();
        info.moved = self.moved;
    }
}

impl VarInfo {
    pub fn new(ty: Type, quantity: Quantity, mutability: Mutability, defined_at: Span) -> Self {
        Self {
            ty,
            quantity,
            mutability,
            defined_at,
            used_at: Vec::new(),
            moved: false,
            erased: quantity == Quantity::Zero,
        }
    }

    /// Record a use of this variable
    pub fn record_use(&mut self, span: Span) {
        self.used_at.push(span);
        if self.quantity == Quantity::One {
            // Linear variable - track usage count
        }
    }

    /// Check if this variable can be used again
    pub fn can_use(&self) -> bool {
        if self.moved {
            return false;
        }
        match self.quantity {
            Quantity::Zero => false, // Erased variables cannot be used at runtime
            Quantity::One => !self.moved && self.used_at.is_empty(), // Exactly once - can use if not moved and not used yet
            Quantity::Bounded(n) => (self.used_at.len() as u32) < n,
            Quantity::Many => true,
        }
    }

    /// Mark as moved (for consume)
    pub fn mark_moved(&mut self, span: Span) {
        self.moved = true;
        self.record_use(span);
    }
}

/// Active inout borrow tracking
#[derive(Debug, Clone)]
pub struct InOutBorrow {
    /// The variable being borrowed
    pub var: Ident,
    /// The place expression borrowed (for alias checking)
    pub place: Place,
    /// Span where borrow started
    pub borrow_span: Span,
}

/// Place expression for alias tracking
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    /// Simple variable
    Var(Ident),
    /// Field projection
    Field(Box<Place>, Ident),
    /// Index projection (using a string key for simplicity)
    Index(Box<Place>, String),
    /// Deref projection
    Deref(Box<Place>),
}

/// Pattern bindings produced during pattern matching
#[derive(Debug, Clone, Default)]
pub struct PatternBindings {
    pub vars: IndexMap<Ident, VarInfo>,
}

impl PatternBindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, ident: Ident, info: VarInfo) {
        self.vars.insert(ident, info);
    }

    pub fn extend(&mut self, other: PatternBindings) {
        self.vars.extend(other.vars);
    }
}

/// Type environment with quantitative tracking
#[derive(Debug, Clone, Default)]
pub struct TypeEnv {
    /// Variable bindings in scope
    pub vars: IndexMap<Ident, VarInfo>,
    /// Type definitions (struct, enum, alias)
    pub types: IndexMap<Ident, TypeDef>,
    /// Function signatures
    pub functions: IndexMap<Ident, FunctionSig>,
    /// Constant definitions
    pub constants: IndexMap<Ident, ConstInfo>,
    /// Active inout borrows (for alias checking)
    pub inout_borrows: Vec<InOutBorrow>,
    /// Variables that have been moved (consumed)
    pub moved_vars: HashSet<Ident>,
    /// Variables marked as erasable (quantity 0)
    pub erasable_vars: HashSet<Ident>,
    /// Generic parameters in scope
    pub generics: IndexMap<Ident, GenericParam>,
    /// Quantity variables in scope (for dependent quantities)
    pub qty_vars: IndexMap<Ident, Quantity>,
}

/// Function signature for type checking
#[derive(Debug, Clone)]
pub struct FunctionSig {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret_ty: Option<Type>,
    pub quantity: Quantity,
    pub is_reversible: bool,
    pub span: Span,
}

/// Constant info
#[derive(Debug, Clone)]
pub struct ConstInfo {
    pub name: Ident,
    pub ty: Option<Type>,
    pub value: Expr,
    pub span: Span,
}

impl TypeEnv {
    /// Create a new empty type environment
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a variable binding
    pub fn bind_var(&mut self, name: Ident, ty: Type, quantity: Quantity, mutability: Mutability) {
        let info = VarInfo::new(ty, quantity, mutability, name.span);

        if quantity == Quantity::Zero {
            self.erasable_vars.insert(name.clone());
        }
        if mutability == Mutability::Consume {
            // Consume bindings are tracked specially
        }

        self.vars.insert(name, info);
    }

    /// Lookup a variable
    pub fn lookup_var(&self, name: &Ident) -> Option<&VarInfo> {
        self.vars.get(name)
    }

    /// Lookup a variable mutably
    pub fn lookup_var_mut(&mut self, name: &Ident) -> Option<&mut VarInfo> {
        self.vars.get_mut(name)
    }

    /// Snapshot the use-state of every currently bound variable.
    ///
    /// Used to typecheck a `proof { .. }` block. A proof block is erased, so
    /// reading a value in it observes the value without consuming it. Without
    /// this, a quantified obligation mentioning a `[1]` linear parameter would
    /// consume that parameter and make the surrounding runtime loop report a
    /// double use -- which would make every obligation about a linear value
    /// inexpressible.
    pub fn snapshot_uses(&self) -> HashMap<Ident, VarUseState> {
        self.vars
            .iter()
            .map(|(k, v)| (k.clone(), VarUseState::from_info(v)))
            .collect()
    }

    /// Restore use-state captured by [`Self::snapshot_uses`].
    ///
    /// Only use-state is restored, not bindings: variables bound inside the
    /// proof block leave with its scope, and restoring those would leak them
    /// into the enclosing code.
    pub fn restore_uses(&mut self, snapshot: &HashMap<Ident, VarUseState>) {
        for (name, state) in snapshot {
            if let Some(info) = self.vars.get_mut(name) {
                state.apply_to(info);
            }
        }
    }

    /// Record a use of a variable
    pub fn use_var(&mut self, name: &Ident, span: Span) -> Result<(), TypeError> {
        if let Some(info) = self.vars.get_mut(name) {
            if info.moved {
                return Err(TypeError::UseOfMovedValue {
                    name: name.clone(),
                    moved_at: info.used_at.last().cloned().unwrap_or(span),
                    used_at: span,
                });
            }

            // Check for Zero quantity - erased variables cannot be used at runtime
            if info.quantity == Quantity::Zero {
                return Err(TypeError::ErasedVariableUsedAtRuntime {
                    name: name.clone(),
                    span,
                });
            }

            // Check for linear variable used twice BEFORE recording the use
            if info.quantity == Quantity::One && !info.used_at.is_empty() {
                return Err(TypeError::LinearVariableUsedTwice {
                    name: name.clone(),
                    first_use: info.used_at[0],
                    second_use: span,
                });
            }

            // Record the use
            info.record_use(span);

            // Check quantity limits for bounded quantities AFTER recording the use
            if let Quantity::Bounded(n) = info.quantity {
                if (info.used_at.len() as u32) > n {
                    return Err(TypeError::VariableNotAvailable {
                        name: name.clone(),
                        reason: "quantity exhausted".to_string(),
                        span,
                    });
                }
            }

            // Check quantity limits for many quantities (should always pass)
            if matches!(info.quantity, Quantity::Many) && !info.can_use() {
                return Err(TypeError::VariableNotAvailable {
                    name: name.clone(),
                    reason: "quantity exhausted".to_string(),
                    span,
                });
            }
        }
        Ok(())
    }

    /// Mark a variable as moved (for consume)
    pub fn move_var(&mut self, name: &Ident, span: Span) -> Result<(), TypeError> {
        if let Some(info) = self.vars.get_mut(name) {
            if info.moved {
                return Err(TypeError::UseOfMovedValue {
                    name: name.clone(),
                    moved_at: info.used_at.last().cloned().unwrap_or(span),
                    used_at: span,
                });
            }
            info.mark_moved(span);
            self.moved_vars.insert(name.clone());
        }
        Ok(())
    }

    /// Start an inout borrow
    pub fn borrow_inout(&mut self, var: Ident, place: Place, span: Span) -> Result<(), TypeError> {
        // Check for aliasing with existing borrows
        for borrow in &self.inout_borrows {
            if places_overlap(&borrow.place, &place) {
                return Err(TypeError::InOutAliasing {
                    var: var.clone(),
                    existing_borrow: borrow.var.clone(),
                    existing_span: borrow.borrow_span,
                    new_span: span,
                });
            }
        }

        // Check for immutable borrows
        // (would need to track immutable borrows too)

        self.inout_borrows.push(InOutBorrow {
            var,
            place,
            borrow_span: span,
        });
        Ok(())
    }

    /// End an inout borrow (at scope exit)
    pub fn end_inout_borrow(&mut self, var: &Ident) {
        self.inout_borrows.retain(|b| &b.var != var);
    }

    /// Check if a place is currently borrowed inout
    pub fn is_borrowed_inout(&self, place: &Place) -> Option<&InOutBorrow> {
        self.inout_borrows
            .iter()
            .find(|b| places_overlap(&b.place, place))
    }

    /// Insert a type definition
    pub fn insert_type_def(&mut self, def: TypeDef) {
        self.types.insert(def.name.clone(), def);
    }

    /// Lookup a type definition
    pub fn lookup_type(&self, name: &Ident) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Insert a function signature and bind as callable variable
    pub fn insert_function(&mut self, func: Function) {
        let sig = FunctionSig {
            name: func.name.clone(),
            generics: func.generics.clone(),
            params: func.params.clone(),
            ret_ty: func.ret_ty.clone(),
            quantity: func.quantity,
            is_reversible: func.is_reversible,
            span: func.span,
        };
        self.functions.insert(func.name.clone(), sig);

        // Also bind as callable variable for function calls
        let param_tys: Vec<Type> = func.params.iter().map(|p| p.ty.clone()).collect();
        let ret_ty = func.ret_ty.clone().unwrap_or(Type::unit(func.span));
        let func_ty = Type::new(
            TypeKind::Function(param_tys, Box::new(ret_ty)),
            func.quantity,
            func.span,
        );
        self.bind_var(
            func.name.clone(),
            func_ty,
            func.quantity,
            Mutability::Immutable,
        );
    }

    /// Lookup a function signature
    pub fn lookup_function(&self, name: &Ident) -> Option<&FunctionSig> {
        self.functions.get(name)
    }

    /// Insert a constant
    pub fn insert_const(&mut self, c: ConstDef) {
        let info = ConstInfo {
            name: c.name.clone(),
            ty: c.ty.clone(),
            value: c.value.clone(),
            span: c.span,
        };
        self.constants.insert(c.name.clone(), info);
    }

    /// Lookup a constant
    pub fn lookup_const(&self, name: &Ident) -> Option<&ConstInfo> {
        self.constants.get(name)
    }

    /// Check if a variable is erasable (quantity 0)
    pub fn is_erasable(&self, name: &Ident) -> bool {
        self.erasable_vars.contains(name)
    }

    /// Get all linear variables that haven't been used
    pub fn unused_linear_vars(&self) -> Vec<&Ident> {
        self.vars
            .iter()
            .filter(|(_, info)| {
                info.quantity == Quantity::One && info.used_at.is_empty() && !info.moved
            })
            .map(|(name, _)| name)
            .collect()
    }
}

/// Scope guard for automatic cleanup
pub struct ScopeGuard {
    vars_initial_len: usize,
    inout_borrows_len: usize,
    moved_vars_snapshot: HashSet<Ident>,
    erasable_vars_snapshot: HashSet<Ident>,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        // Note: In a real implementation, we'd restore the snapshots
        // For now, this is a placeholder - the checker should explicitly
        // call exit_scope with the guard
    }
}

impl TypeEnv {
    /// Enter a new scope (for block scoping)
    pub fn enter_scope(&mut self) -> ScopeGuard {
        ScopeGuard {
            vars_initial_len: self.vars.len(),
            inout_borrows_len: self.inout_borrows.len(),
            moved_vars_snapshot: self.moved_vars.clone(),
            erasable_vars_snapshot: self.erasable_vars.clone(),
        }
    }

    /// Exit a scope, restoring state and checking for unused linear vars
    pub fn exit_scope(&mut self, guard: ScopeGuard) -> Result<(), TypeError> {
        // Check for unused linear variables bound in this scope
        for (name, info) in self.vars.iter().skip(guard.vars_initial_len) {
            // Skip consume bindings - they represent a consumption point
            // Skip inout bindings - they are borrowed for the scope duration
            if info.quantity == Quantity::One
                && info.used_at.is_empty()
                && !info.moved
                && info.mutability != Mutability::Consume
                && info.mutability != Mutability::InOut
            {
                return Err(TypeError::UnusedLinearVariable {
                    name: name.clone(),
                    defined_at: info.defined_at,
                });
            }
        }

        // Restore state - remove variables bound in this scope
        let keys_to_remove: Vec<Ident> = self
            .vars
            .keys()
            .skip(guard.vars_initial_len)
            .cloned()
            .collect();
        for key in keys_to_remove {
            self.vars.shift_remove(&key);
        }

        self.inout_borrows.truncate(guard.inout_borrows_len);
        self.moved_vars = guard.moved_vars_snapshot.clone();
        self.erasable_vars = guard.erasable_vars_snapshot.clone();

        Ok(())
    }
}

/// Check if two places overlap (for alias detection)
fn places_overlap(p1: &Place, p2: &Place) -> bool {
    match (p1, p2) {
        (Place::Var(v1), Place::Var(v2)) => v1.name == v2.name,
        (Place::Field(base1, field1), Place::Field(base2, field2)) => {
            field1.name == field2.name && places_overlap(base1, base2)
        }
        (Place::Index(base1, _), Place::Index(base2, _)) => places_overlap(base1, base2),
        (Place::Deref(base1), Place::Deref(base2)) => places_overlap(base1, base2),
        _ => false,
    }
}
