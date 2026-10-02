//! Affine Domain Representation
//!
//! Represents Z-polyhedra (integer sets defined by affine constraints)
//! with support for parameters (symbolic constants).
//!
//! This is a native Rust implementation avoiding external C dependencies.
//! For production use, the `isl` crate can be swapped in when it compiles on Windows.

#![allow(clippy::identity_op)]

use serde::{Deserialize, Serialize};

/// Type of affine constraint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintType {
    /// Equality constraint: a·x = b
    Equality,
    /// Inequality constraint: a·x >= b
    Inequality,
}

/// Single affine constraint: a·x >= b or a·x = b
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffineConstraint {
    /// Coefficients for each dimension (iterators + parameters)
    pub coefficients: Vec<i64>,
    /// Constant term
    pub constant: i64,
    /// Constraint type
    pub ctype: ConstraintType,
}

impl AffineConstraint {
    /// Create a new inequality constraint: sum(coeff[i] * x[i]) >= constant
    pub fn inequality(coefficients: Vec<i64>, constant: i64) -> Self {
        Self {
            coefficients,
            constant,
            ctype: ConstraintType::Inequality,
        }
    }

    /// Create a new equality constraint: sum(coeff[i] * x[i]) = constant
    pub fn equality(coefficients: Vec<i64>, constant: i64) -> Self {
        Self {
            coefficients,
            constant,
            ctype: ConstraintType::Equality,
        }
    }

    /// Number of dimensions this constraint operates on
    pub fn dims(&self) -> usize {
        self.coefficients.len()
    }

    /// Check if a point satisfies this constraint
    pub fn contains(&self, point: &[i64]) -> bool {
        let sum: i64 = self
            .coefficients
            .iter()
            .zip(point.iter())
            .map(|(c, x)| c * x)
            .sum();
        match self.ctype {
            ConstraintType::Inequality => sum >= self.constant,
            ConstraintType::Equality => sum == self.constant,
        }
    }
}

/// Affine domain: a set of integer points defined by affine constraints
///
/// Represents iteration domains in polyhedral compilation.
/// Dimensions are split into:
/// - Iterator dimensions (loop indices): 0..n_iter
/// - Parameter dimensions (symbolic constants): n_iter..n_iter+n_param
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffineDomain {
    /// Total number of dimensions (iterators + parameters)
    pub dims: usize,
    /// Number of iterator dimensions
    pub n_iter: usize,
    /// Number of parameter dimensions
    pub n_param: usize,
    /// Affine constraints defining the domain
    pub constraints: Vec<AffineConstraint>,
    /// The NAMES of this domain's parameter dimensions, in dimension order.
    ///
    /// A domain with `n_param > 0` has constraints that mention dimensions
    /// `n_iter..dims`, and without a name for each one nothing downstream can say
    /// what those dimensions MEAN. Codegen needs the name to find the value: the
    /// `-i + N >= 1` constraint for `forall i in 0..N` is only lowerable to a real
    /// trip count if dimension 1 is known to be `N`.
    ///
    /// An empty list means "these parameter dimensions are unnamed". That is a real
    /// state -- hand-built domains carry parameter dimensions with no names -- and a
    /// backend that meets it must refuse rather than substitute a value.
    ///
    /// This is deliberately not a debug label: it is the index-to-name mapping that
    /// makes a symbolic bound lowerable, and it is checked for length against
    /// `n_param` by `validate_domain`.
    pub parameter_names: Vec<String>,
    /// A debug LABEL for this domain, shown by `pretty_print` and diagnostics.
    ///
    /// It carries no semantics. It used to double as the channel by which a loop's
    /// iterator name reached codegen (as `nest(i)`, recovered by
    /// `AffineDomain::nest_iterator`), which coupled a format string to program
    /// meaning. That is now `ScheduleNode::Band::iterators`, a real field.
    pub name: Option<String>,
}

impl AffineDomain {
    /// Create a new empty domain (universe) with given dimensions
    pub fn universe(n_iter: usize, n_param: usize) -> Self {
        Self {
            dims: n_iter + n_param,
            n_iter,
            n_param,
            constraints: Vec::new(),
            parameter_names: Vec::new(),
            name: None,
        }
    }

    /// Create a domain from explicit constraints
    pub fn new(n_iter: usize, n_param: usize, constraints: Vec<AffineConstraint>) -> Self {
        let dims = n_iter + n_param;
        for c in &constraints {
            assert_eq!(c.dims(), dims, "Constraint dimension mismatch");
        }
        Self {
            dims,
            n_iter,
            n_param,
            constraints,
            parameter_names: Vec::new(),
            name: None,
        }
    }

    /// Name this domain's parameter dimensions, in dimension order.
    ///
    /// `names` may be empty (the parameter dimensions stay unnamed) or at most
    /// `n_param` long. A shorter list names the FIRST few parameter dimensions and
    /// leaves the rest unnamed, which is the honest reading: an unnamed trailing
    /// dimension is not a dimension whose value may be guessed.
    pub fn with_parameter_names(mut self, names: Vec<String>) -> Self {
        assert!(
            names.len() <= self.n_param,
            "a domain with {} parameter dimensions cannot name {} of them",
            self.n_param,
            names.len()
        );
        self.parameter_names = names;
        self
    }

    /// Create a named domain
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add a constraint to the domain
    pub fn add_constraint(&mut self, constraint: AffineConstraint) {
        assert_eq!(
            constraint.dims(),
            self.dims,
            "Constraint dimension mismatch"
        );
        self.constraints.push(constraint);
    }

    /// Intersection of two domains (both must have same dimension structure)
    pub fn intersection(&self, other: &AffineDomain) -> Self {
        assert_eq!(self.dims, other.dims, "Dimension mismatch in intersection");
        assert_eq!(self.n_iter, other.n_iter, "Iterator count mismatch");
        assert_eq!(self.n_param, other.n_param, "Parameter count mismatch");

        let mut constraints = self.constraints.clone();
        constraints.extend(other.constraints.clone());
        Self {
            dims: self.dims,
            n_iter: self.n_iter,
            n_param: self.n_param,
            constraints,
            // The parameter dimensions are inherited unchanged, so their names
            // are too. Dropping them here would leave a domain that constrains
            // dimensions nobody can name.
            parameter_names: self.parameter_names.clone(),
            name: None,
        }
    }

    /// Union of two domains (approximated as convex hull for polyhedral domains)
    /// Note: True union of polyhedra is not a polyhedron; this returns convex hull
    pub fn union(&self, other: &AffineDomain) -> Self {
        // For true polyhedral union, we'd need disjunctive normal form.
        // This is a placeholder that returns the convex hull via constraint merging.
        // In practice, schedule trees use Filter nodes for conditional domains.
        self.intersection(other)
    }

    /// Project out dimensions (existential quantification)
    /// Returns a new domain with the specified dimensions removed
    pub fn project(&self, dims_to_remove: &[usize]) -> Self {
        let mut keep = vec![true; self.dims];
        for &d in dims_to_remove {
            if d < self.dims {
                keep[d] = false;
            }
        }

        let new_n_iter = (0..self.n_iter).filter(|&i| keep[i]).count();
        let new_n_param = (self.n_iter..self.dims).filter(|&i| keep[i]).count();
        let new_dims = new_n_iter + new_n_param;

        // Build mapping from old dimension index to new dimension index
        let mut old_to_new = vec![None; self.dims];
        let mut new_idx = 0;
        for (old_idx, &keep_dim) in keep.iter().enumerate() {
            if keep_dim {
                old_to_new[old_idx] = Some(new_idx);
                new_idx += 1;
            }
        }

        // Fourier-Motzkin elimination for projection
        // Simplified: drop constraints involving removed dims and remap kept coefficients
        let constraints: Vec<AffineConstraint> = self
            .constraints
            .iter()
            .filter(|c| {
                c.coefficients
                    .iter()
                    .enumerate()
                    .all(|(i, &coeff)| coeff == 0 || keep[i])
            })
            .map(|c| {
                // Remap coefficients to new dimension indices
                let new_coeffs: Vec<i64> = c
                    .coefficients
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &coeff)| old_to_new[i].map(|new_i| (new_i, coeff)))
                    .fold(vec![0; new_dims], |mut acc, (new_i, coeff)| {
                        acc[new_i] = coeff;
                        acc
                    });
                AffineConstraint {
                    coefficients: new_coeffs,
                    constant: c.constant,
                    ctype: c.ctype,
                }
            })
            .collect();

        // The kept parameter dimensions keep their names, in their new order. A
        // projected-away parameter's name is dropped with it, so the list can end
        // up shorter than `new_n_param`; the remaining dimensions are then unnamed,
        // which is what they are.
        let parameter_names: Vec<String> = (self.n_iter..self.dims)
            .filter(|&i| keep[i])
            .enumerate()
            .filter_map(|(p, _)| self.parameter_names.get(p).cloned())
            .collect();

        Self {
            dims: new_dims,
            n_iter: new_n_iter,
            n_param: new_n_param,
            constraints,
            parameter_names,
            name: None,
        }
    }

    /// Check if domain is definitely empty (sound but incomplete)
    pub fn is_empty(&self) -> bool {
        // Simple check: look for contradictory constraints like x >= 1, x <= 0
        // Full emptiness requires ILP solving (Fourier-Motzkin or simplex)
        for i in 0..self.constraints.len() {
            for j in i + 1..self.constraints.len() {
                if self.are_contradictory(&self.constraints[i], &self.constraints[j]) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if two constraints are directly contradictory
    fn are_contradictory(&self, c1: &AffineConstraint, c2: &AffineConstraint) -> bool {
        // Check for x >= a and x <= b where a > b
        if c1.coefficients == c2.coefficients {
            match (c1.ctype, c2.ctype) {
                (ConstraintType::Inequality, ConstraintType::Inequality) => {
                    // Both >= : c1: sum >= a, c2: sum >= b -> not contradictory
                    false
                }
                (ConstraintType::Equality, ConstraintType::Equality) => c1.constant != c2.constant,
                (ConstraintType::Equality, ConstraintType::Inequality)
                | (ConstraintType::Inequality, ConstraintType::Equality) => {
                    // x = a and x >= b: contradictory if a < b
                    // x = a and x <= b: contradictory if a > b
                    let (eq, ineq) = if c1.ctype == ConstraintType::Equality {
                        (c1, c2)
                    } else {
                        (c2, c1)
                    };
                    // For inequality, we'd need to know direction (<= vs >=)
                    // Our representation only has >=, so x = a, x >= b contradicts if a < b
                    eq.constant < ineq.constant
                }
            }
        } else if Self::are_negated(&c1.coefficients, &c2.coefficients) {
            // One constraint is the negation of the other (e.g., x >= 5 and -x >= -3)
            // This means we have x >= a and -x >= b => x >= a and x <= -b
            // Contradiction if a > -b
            // Both must be inequalities
            if c1.ctype == ConstraintType::Inequality && c2.ctype == ConstraintType::Inequality {
                // Check if c1 is negated version of c2
                let (pos, neg) = if Self::is_negated_version(&c1.coefficients, &c2.coefficients) {
                    (c1, c2)
                } else if Self::is_negated_version(&c2.coefficients, &c1.coefficients) {
                    (c2, c1)
                } else {
                    return false;
                };
                // pos: sum >= a, neg: -sum >= b => sum <= -b
                // Contradiction if a > -b
                pos.constant > -neg.constant
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Check if one coefficient vector is the negation of another
    fn is_negated_version(a: &[i64], b: &[i64]) -> bool {
        a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| *x == -*y)
    }

    /// Check if two coefficient vectors are negations of each other
    fn are_negated(a: &[i64], b: &[i64]) -> bool {
        Self::is_negated_version(a, b) || Self::is_negated_version(b, a)
    }

    /// Check if a point is in the domain
    pub fn contains(&self, point: &[i64]) -> bool {
        if point.len() != self.dims {
            return false;
        }
        self.constraints.iter().all(|c| c.contains(point))
    }

    /// Get iterator bounds for a specific dimension (if bounded)
    ///
    /// Returns `(lower_bound, upper_bound)` as affine expressions in this domain's
    /// PARAMETERS. A constraint that mentions only `iter_dim` and parameter
    /// dimensions yields a bound that depends on those parameters; one that mentions
    /// another iterator does not bound this dimension on its own and is ignored.
    ///
    /// The returned expressions' `coefficients` are indexed by the domain's
    /// PARAMETER index (`0` is `parameter_names[0]`), NOT by the domain dimension.
    /// That is the only indexing under which an expression makes sense: a loop bound
    /// cannot depend on the induction variable it bounds, so the iterator dimensions
    /// are already resolved out. `evaluate` on such an expression therefore takes the
    /// parameter values as its point.
    ///
    /// # Divisibility
    ///
    /// A bound is only produced when the dividing coefficient divides the constant
    /// and every parameter coefficient EXACTLY. Truncating division would floor a
    /// bound -- `3i >= 8` would become `i >= 2` instead of admitting the domain is
    /// not a simple range -- and a wrong bound is a wrong trip count, so the
    /// constraint is skipped instead and the caller ends up with no bound to lower.
    pub fn iterator_bounds(&self, iter_dim: usize) -> Option<(AffineExpr, AffineExpr)> {
        if iter_dim >= self.n_iter {
            return None;
        }

        let mut lower = None;
        let mut upper = None;

        for c in &self.constraints {
            if c.ctype != ConstraintType::Inequality {
                continue;
            }
            // Constraint must involve this iterator and no OTHER iterator. A
            // constraint coupling two iterators says nothing about either one alone.
            if c.coefficients[iter_dim] == 0
                || c.coefficients
                    .iter()
                    .enumerate()
                    .any(|(i, &coeff)| i < self.n_iter && i != iter_dim && coeff != 0)
            {
                continue;
            }
            let coeff = c.coefficients[iter_dim];
            // Skip a constraint whose division is not exact rather than aborting: the
            // point is that this constraint does not yield a usable bound. If that
            // leaves `lower` or `upper` unset, `lower.zip(upper)` below yields `None`
            // and the caller refuses the band rather than inventing a trip count.
            let Some(expr) = exact_bound(c, self.n_iter, coeff) else {
                continue;
            };
            if coeff > 0 {
                lower = Some(expr);
            } else {
                upper = Some(expr);
            }
        }

        lower.zip(upper)
    }
}

/// Solve `coeff * x + sum_j b_j p_j >= C` for `x` as an affine expression in the
/// parameters, or `None` when the division is not exact.
///
/// The caller guarantees `coeff != 0` and that the constraint mentions no iterator
/// other than the one being solved for.
fn exact_bound(c: &AffineConstraint, n_iter: usize, coeff: i64) -> Option<AffineExpr> {
    debug_assert_ne!(coeff, 0);
    // `a*x >= K`  ==>  `x >= K/a` for a > 0, and `x <= K/a` for a < 0. Both
    // directions are the same division; only the side of the inequality differs,
    // which the caller reads off the sign of `coeff`. The division by a negative
    // coefficient is what flips the direction, and negating `C` as well would flip
    // it a second time.
    let mut expr = AffineExpr {
        coefficients: Vec::new(),
        constant: c.constant,
    };
    for (p, &b) in c.coefficients[n_iter..].iter().enumerate() {
        let q = -b;
        if q % coeff != 0 {
            // Not exact: refuse rather than truncate.
            return None;
        }
        // Keep the slot so the vector is indexed by parameter position even when
        // this parameter's coefficient happens to be zero.
        while expr.coefficients.len() <= p {
            expr.coefficients.push(0);
        }
        expr.coefficients[p] = q / coeff;
    }
    if c.constant % coeff != 0 {
        return None;
    }
    expr.constant = c.constant / coeff;
    Some(expr)
}

/// Affine expression: sum(a_i * x_i) + c
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffineExpr {
    pub coefficients: Vec<i64>,
    pub constant: i64,
}

impl AffineExpr {
    pub fn constant(c: i64) -> Self {
        Self {
            coefficients: Vec::new(),
            constant: c,
        }
    }

    pub fn var(dim: usize, coeff: i64) -> Self {
        let mut coefficients = vec![0; dim + 1];
        coefficients[dim] = coeff;
        Self {
            coefficients,
            constant: 0,
        }
    }

    pub fn evaluate(&self, point: &[i64]) -> i64 {
        self.coefficients
            .iter()
            .zip(point.iter())
            .map(|(c, x)| c * x)
            .sum::<i64>()
            + self.constant
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_universe_domain() {
        let d = AffineDomain::universe(2, 1);
        assert_eq!(d.dims, 3);
        assert_eq!(d.n_iter, 2);
        assert_eq!(d.n_param, 1);
        assert!(d.constraints.is_empty());
    }

    #[test]
    fn test_domain_with_constraints() {
        // { [i, j] : 0 <= i < N, 0 <= j < M }
        let mut d = AffineDomain::universe(2, 2);
        // i >= 0
        d.add_constraint(AffineConstraint::inequality(vec![1, 0, 0, 0], 0));
        // i <= N-1  =>  -i >= -N+1  =>  -i + N >= 1
        d.add_constraint(AffineConstraint::inequality(vec![-1, 0, 1, 0], 1));
        // j >= 0
        d.add_constraint(AffineConstraint::inequality(vec![0, 1, 0, 0], 0));
        // j <= M-1
        d.add_constraint(AffineConstraint::inequality(vec![0, -1, 0, 1], 1));

        assert!(d.contains(&[0, 0, 10, 20]));
        assert!(d.contains(&[5, 10, 10, 20]));
        assert!(!d.contains(&[10, 0, 10, 20])); // i >= N
        assert!(!d.contains(&[0, 20, 10, 20])); // j >= M
    }

    #[test]
    fn test_intersection() {
        let d1 = AffineDomain::new(
            1,
            0,
            vec![
                AffineConstraint::inequality(vec![1], 0),   // x >= 0
                AffineConstraint::inequality(vec![-1], -5), // x <= 5
            ],
        );
        let d2 = AffineDomain::new(
            1,
            0,
            vec![
                AffineConstraint::inequality(vec![1], 3),    // x >= 3
                AffineConstraint::inequality(vec![-1], -10), // x <= 10
            ],
        );
        let inter = d1.intersection(&d2);
        // Should be 3 <= x <= 5
        assert!(inter.contains(&[3]));
        assert!(inter.contains(&[5]));
        assert!(!inter.contains(&[2]));
        assert!(!inter.contains(&[6]));
    }

    #[test]
    fn test_projection() {
        // { [i, j] : 0 <= i < N, 0 <= j < M }
        let mut d = AffineDomain::universe(2, 2);
        d.add_constraint(AffineConstraint::inequality(vec![1, 0, 0, 0], 0));
        d.add_constraint(AffineConstraint::inequality(vec![-1, 0, 1, 0], 1));
        d.add_constraint(AffineConstraint::inequality(vec![0, 1, 0, 0], 0));
        d.add_constraint(AffineConstraint::inequality(vec![0, -1, 0, 1], 1));

        // Project out j (dim 1)
        let proj = d.project(&[1]);
        assert_eq!(proj.n_iter, 1);
        assert_eq!(proj.n_param, 2);
        assert!(proj.contains(&[0, 10, 20]));
        assert!(proj.contains(&[5, 10, 20]));
    }

    #[test]
    fn test_contradiction_detection() {
        // x >= 5 and x <= 3 (impossible)
        let mut d = AffineDomain::universe(1, 0);
        d.add_constraint(AffineConstraint::inequality(vec![1], 5)); // x >= 5
        d.add_constraint(AffineConstraint::inequality(vec![-1], -3)); // -x >= -3 => x <= 3

        assert!(d.is_empty());
    }

    #[test]
    fn test_affine_expr() {
        let e = AffineExpr {
            coefficients: vec![2, 3],
            constant: 5,
        };
        assert_eq!(e.evaluate(&[1, 2]), 2 * 1 + 3 * 2 + 5);
        assert_eq!(e.evaluate(&[0, 0]), 5);
    }

    /// `iterator_bounds` must recover the upper bound with the correct sign.
    ///
    /// `-i >= -3` means `i <= 3`. This returned `-3 / -1 = -3` because it negated the
    /// constant as well as dividing by the negative coefficient, inverting the bound a
    /// second time. Every generated loop then compared `iv < -3`, never ran its body,
    /// and returned a silently wrong answer.
    ///
    /// The single test that exercised this path wrote the constraint with a POSITIVE
    /// constant to work around the bug, so it passed against broken arithmetic.
    #[test]
    fn an_upper_bound_constraint_yields_the_correct_signed_bound() {
        // 0 <= i, and i <= 3.
        let domain = AffineDomain::new(
            1,
            0,
            vec![
                AffineConstraint::inequality(vec![1], 0),
                AffineConstraint::inequality(vec![-1], -3),
            ],
        );
        let (lower, upper) = domain
            .iterator_bounds(0)
            .expect("a bounded iteration space has bounds");
        assert_eq!(lower.constant, 0, "the lower bound comes from `i >= 0`");
        assert_eq!(
            upper.constant, 3,
            "`-i >= -3` means `i <= 3`, not `i <= -3`"
        );
    }

    /// The same bound read with the coefficients scaled, which must not change the
    /// result: `-2i >= -6` still means `i <= 3`.
    ///
    /// This pins the DIVISION, not just a negation. The buggy version returned
    /// `-(-6) / -2 = -3`.
    #[test]
    fn an_upper_bound_is_invariant_under_scaling_the_constraint() {
        let domain = AffineDomain::new(
            1,
            0,
            vec![
                AffineConstraint::inequality(vec![1], 0),
                AffineConstraint::inequality(vec![-2], -6),
            ],
        );
        let (_, upper) = domain.iterator_bounds(0).expect("bounds");
        assert_eq!(upper.constant, 3, "`-2i >= -6` means `i <= 3`");
    }

    /// A lower bound with a negative constant is genuinely negative, not flipped.
    #[test]
    fn a_negative_lower_bound_keeps_its_sign() {
        let domain = AffineDomain::new(
            1,
            0,
            vec![
                AffineConstraint::inequality(vec![1], -5),
                AffineConstraint::inequality(vec![-1], -3),
            ],
        );
        let (lower, _) = domain.iterator_bounds(0).expect("bounds");
        assert_eq!(lower.constant, -5, "`i >= -5`");
    }
}
