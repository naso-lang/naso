//! Domain transformations for early exits inside an affine `forall` band.
//!
//! # The problem this replaces
//!
//! A `forall` band has a statically known trip count and its body is emitted as
//! straight-line code inside that iteration space. There is no runtime exit to branch to,
//! so `break` inside one was refused:
//!
//! ```ignore
//! for i in 10 { if i == 3 { break; } }   // refused: "`break` inside an affine `forall` loop"
//! ```
//!
//! That refusal was honest but stopped one step short of the real answer. Leaving early is
//! not impossible in a polyhedral loop -- it is a question about the ITERATION DOMAIN, and
//! that is exactly what this module computes.
//!
//! # The transformation
//!
//! `forall i in lo..hi { ... if i >= k { break } ... }` executes iteration `i` only if no
//! earlier iteration broke. The surviving set is a PREFIX of the original iteration space:
//!
//! ```text
//!   original domain:   lo <= i <  hi
//!   break at i >= k:   lo <= i <  min(hi, k)
//! ```
//!
//! So the transformation is an intersection with a new upper bound, which stays inside the
//! affine representation. `continue` is the dual: the body is skipped but the iteration
//! still happens, so the DOMAIN is unchanged and only the BODY is predicated.
//!
//! # What this does not do
//!
//! It handles a `break`/`continue` guard that is **affine and monotone in the band's
//! iteration variable**, because only then does the surviving set remain a single affine
//! range. A guard over several iterators, or one that is not monotone in the induction
//! variable, produces a domain that is a UNION of ranges or not a range at all, and is
//! refused with the reason.
//!
//! This is the polyhedral argument, not a workaround: the transformation below is only
//! applied where the result is provably still affine, and refused otherwise rather than
//! approximated.

use crate::ir::affine_domain::{AffineConstraint, AffineDomain, AffineExpr};

/// Why a domain transformation could not be applied.
///
/// Every variant names the specific reason, because "cannot transform this domain" is not
/// actionable and the user needs to know which guard to rewrite.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TransformError {
    /// The band's iteration domain has no upper bound in the iterator.
    ///
    /// Without one there is no range to shorten.
    NoIteratorBound {
        /// The iteration variable's name as written in source.
        iterator: String,
    },
    /// The guard's upper bound is not a compile-time constant.
    ///
    /// A symbolic bound would make the new trip count depend on a runtime value, which is
    /// exactly the data-dependent early exit an affine band cannot express. Use a `while`
    /// loop, whose trip count is decided at runtime.
    SymbolicGuardBound {
        /// The bound as written, e.g. `n - 1`.
        bound: String,
    },
    /// The break cannot be reduced to "stop at this point in the iteration".
    ///
    /// Raised when the first iteration satisfying the guard cannot be identified, because
    /// the guard depends on runtime data rather than on the iteration variable. The
    /// surviving set is then not a prefix of the original domain and has no affine form.
    NotAPrefixOfTheIteration {
        /// The guard as written, e.g. `i > n / 2`.
        guard: String,
    },
    /// The resulting domain admits no iteration at all.
    ///
    /// A real answer, not a failure: the loop would execute zero times. Callers emit an
    /// empty loop rather than the original trip count, because running iterations that the
    /// program breaks out of immediately is the wrong computation.
    EmptyAfterTransformation {
        /// The resulting lower bound.
        lower: i64,
        /// The resulting upper bound, exclusive.
        upper: i64,
    },
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformError::NoIteratorBound { iterator } => write!(
                f,
                "the iteration domain for `{iterator}` has no computable upper bound, so \
                 there is no range to shorten"
            ),
            TransformError::SymbolicGuardBound { bound } => write!(
                f,
                "the break point `{bound}` is not a compile-time constant, so the trip \
                 count would depend on a runtime value. That is a data-dependent early \
                 exit, which an affine `forall` band cannot express -- use a `while` loop"
            ),
            TransformError::NotAPrefixOfTheIteration { guard } => write!(
                f,
                "the guard `{guard}` does not identify a point in the iteration, so the \
                 surviving iterations are not a prefix of the original domain and have no \
                 affine form. A `break` inside a `forall` must depend only on the iteration \
                 variable; anything else needs a `while` loop"
            ),
            TransformError::EmptyAfterTransformation { lower, upper } => write!(
                f,
                "the break point makes the range [{lower}, {upper}) empty, so the loop \
                 executes zero times"
            ),
        }
    }
}

/// What an early exit does to the iteration domain.
///
/// `Break` shrinks the domain. `Continue` leaves it alone and predicates the body, which is
/// why they are separate variants rather than one "skip" flag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EarlyExit {
    /// Stop before this iteration.
    Break,
    /// Skip the body of this iteration but keep going.
    Continue,
}

/// A `forall` band whose iteration space is a single affine range.
///
/// # Both bounds are INCLUSIVE
///
/// `AffineDomain::iterator_bounds` returns the largest value a domain admits, and the
/// front end encodes a half-open source range `lo..hi` as `i <= hi - 1`
/// (`lowering::loop_extraction::domain_from_nest`). Mixing that inclusive convention with
/// an exclusive one is how `forall i in 0..4` once ran three iterations and summed 0+1+2
/// instead of 0+1+2+3 -- a loop that looked right and computed a wrong answer.
///
/// So `AffineBand` uses inclusive bounds on BOTH ends, matching the library it is built
/// from. A `while`-style loop is half-open in source but is not a `forall` band and does
/// not come through here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AffineBand {
    /// The iterator's name as written, e.g. `i`.
    pub iterator: String,
    /// First iteration executed, INCLUSIVE.
    pub lower: AffineExpr,
    /// Last iteration executed, INCLUSIVE.
    pub upper: AffineExpr,
}

impl AffineBand {
    /// Build a band from an iteration domain's bounds.
    ///
    /// Returns `None` when the domain has no computable bounds for that iterator, which is
    /// a real state (a domain with a coupling constraint) and not a placeholder.
    pub fn from_domain(
        domain: &AffineDomain,
        iter_dim: usize,
        iterator: &str,
    ) -> Result<Self, TransformError> {
        let (lower, upper) =
            domain
                .iterator_bounds(iter_dim)
                .ok_or_else(|| TransformError::NoIteratorBound {
                    iterator: iterator.to_string(),
                })?;
        Ok(Self {
            iterator: iterator.to_string(),
            lower,
            upper,
        })
    }

    /// The trip count, when both bounds are constants.
    ///
    /// `None` for a symbolic band, which is not an error -- the band is real, its trip
    /// count is just not a number this pass can compute.
    pub fn trip_count(&self, point: &[i64]) -> Option<i64> {
        // Inclusive on both ends, so the count is `hi - lo + 1`. With the exclusive form
        // this was `hi - lo`, which under-counts every band by exactly one -- the same
        // off-by-one that dropped the final iteration of a counted loop.
        Some(self.upper.evaluate(point) - self.lower.evaluate(point) + 1)
    }

    /// The band as an `AffineDomain`, for a band with constant bounds.
    ///
    /// A domain over one iteration variable and no parameters, so `contains` is exact and
    /// cheap. Used by the tests to check a transformation rather than trust its arithmetic.
    /// `lower` and `upper` are INCLUSIVE, matching the fields.
    pub fn to_constant_domain(&self, lower: i64, upper: i64) -> AffineDomain {
        let mut domain = AffineDomain::universe(1, 0);
        // `AffineConstraint::inequality` means `sum(coeffs) >= constant`.
        domain.add_constraint(AffineConstraint::inequality(vec![1], lower)); //  i >= lower
        domain.add_constraint(AffineConstraint::inequality(vec![-1], -upper)); // -i >= -upper
        domain
    }

    /// The largest constant that is still strictly below the current upper bound.
    ///
    /// This is where a symbolic bound is rejected. A bound that is a plain constant
    /// evaluates the same for every point, so the shortened domain stays affine in the
    /// parameters. Anything else would make the trip count runtime-dependent.
    fn constant_upper_bound(&self, point: &[i64]) -> Result<i64, TransformError> {
        // `AffineExpr` has public `coefficients` and `constant`, so constness is read
        // directly rather than through a new helper. Every non-zero coefficient makes the
        // bound depend on a value, which is what makes the trip count runtime-dependent.
        if self.upper.coefficients.iter().any(|&c| c != 0) {
            let terms: Vec<String> = self
                .upper
                .coefficients
                .iter()
                .enumerate()
                .filter(|&(_, &c)| c != 0)
                .map(|(dim, &c)| format!("{c}*x{dim}"))
                .collect();
            return Err(TransformError::SymbolicGuardBound {
                bound: format!("{} + {}", terms.join(" + "), self.upper.constant),
            });
        }
        Ok(self.upper.evaluate(point))
    }

    /// Apply a `break` whose guard is "stop when `i >= at`".
    ///
    /// The transformation: `hi` becomes `min(hi, at)`. `min` of two affine bounds is affine,
    /// so the result is still a band -- which is the whole reason this is expressible and a
    /// general early exit is not.
    ///
    /// # What the caller must have established
    ///
    /// That `at` is reached by ascending iteration and by no earlier one. For a guard of the
    /// form `i >= k` that holds by construction; the caller checks the guard's SHAPE, and
    /// this function is only the domain algebra. `early_exit_is_prefix_guard` is the shape
    /// check.
    pub fn break_before(&self, at: i64, point: &[i64]) -> Result<Self, TransformError> {
        let upper = self.constant_upper_bound(point)?;
        let lower = self.lower.evaluate(point);

        // A break point STRICTLY BELOW the band skips nothing: the band never reaches it.
        // Clamping with `at - 1` in that case would give `at - 1 < lower` and report the
        // band as empty, the opposite of the truth. An earlier version did exactly that, and
        // a band starting at 5 with a break at 2 came back EmptyAfterTransformation.
        //
        // Strictly below, not at-or-below: `at == lower` DOES skip the whole band, since the
        // first iteration is the one that breaks. That is a genuine empty domain.
        //
        // So the clamp only applies when the break point is actually inside the band.
        let new_upper = if at < lower { upper } else { upper.min(at - 1) };

        if new_upper < lower {
            return Err(TransformError::EmptyAfterTransformation {
                lower,
                upper: new_upper,
            });
        }
        Ok(Self {
            iterator: self.iterator.clone(),
            lower: self.lower.clone(),
            // `min` of two constants is a constant, so the shortened band stays affine.
            upper: AffineExpr::constant(new_upper),
        })
    }

    /// The domain a `continue` leaves behind: UNCHANGED.
    ///
    /// A `continue` skips the body of one iteration but the iteration still happens, so the
    /// iteration space is untouched and only the body needs predication. Returning the band
    /// unchanged is the transformation; the method exists so the caller cannot accidentally
    /// treat `continue` as `break`.
    pub fn continue_predicate(&self, point: &[i64]) -> Result<Self, TransformError> {
        // Validating the bounds keeps the failure mode identical to `break_before`, so a
        // band that cannot be transformed is reported the same way whichever exit it uses.
        let upper = self.constant_upper_bound(point)?;
        if upper < self.lower.evaluate(point) {
            return Err(TransformError::EmptyAfterTransformation {
                lower: self.lower.evaluate(point),
                upper,
            });
        }
        Ok(self.clone())
    }

    /// The iteration at which execution would stop, given concrete parameter values.
    ///
    /// `None` when the band runs to completion.
    pub fn stop_point(&self, break_at: Option<i64>, point: &[i64]) -> Option<i64> {
        let lower = self.lower.evaluate(point);
        let upper = self.upper.evaluate(point);
        // `at` is the first SKIPPED iteration, so it is a real stop point only when it
        // actually falls inside the band. Both ends are needed: a break at `at < lower`
        // skips nothing (the band never reaches it), and `at > upper` skips nothing either.
        //
        // Reporting a stop point outside the band would tell a backend to emit an exit at an
        // iteration that does not exist, which is how an off-by-one becomes a wrong answer
        // rather than a visible mistake.
        match break_at {
            Some(at) if at >= lower && at <= upper => Some(at),
            _ => None,
        }
    }
}

/// Whether a `break` guard is one this transformation can express.
///
/// # What it accepts
///
/// A guard that compares the band's iterator against a compile-time constant:
/// `i >= k`, `i > k`, `i == k`, `i < k` and their reflections. Each of those identifies a
/// point in the ascending iteration, so the surviving set is a prefix.
///
/// # What it refuses, and why
///
/// Anything mentioning another identifier. `i >= n / 2` depends on a runtime value, so the
/// first iteration that breaks is not known until runtime and the surviving set is not a
/// prefix the compiler can name. A `while` loop is the right construct for that, and this
/// error says so.
pub fn early_exit_is_prefix_guard(guard: &str, iterator: &str) -> Result<(), TransformError> {
    let trimmed = guard.trim();

    // Split the comparison at the operator, whichever side the iterator is on.
    //
    // An earlier version used `strip_prefix(iterator)` for the right-hand side and
    // `rsplit(iterator)` for the left. That handles `i >= 3` and fails on `3 <= i`, which
    // names the same point -- the operator's direction has to be read, not assumed, or a
    // reflected guard is reported as "data-dependent" when it is perfectly static.
    const OPS: [&str; 6] = ["<=", ">=", "==", "!=", "<", ">"];
    let Some((left, _op, right)) = OPS.iter().find_map(|op| {
        trimmed
            .split_once(op)
            .map(|(l, r)| (l.trim(), *op, r.trim()))
    }) else {
        return Err(TransformError::NotAPrefixOfTheIteration {
            guard: guard.to_string(),
        });
    };

    /// A plain integer literal, with an optional leading `-`.
    fn is_integer_literal(side: &str) -> bool {
        let digits = side.strip_prefix('-').unwrap_or(side);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    }

    // Exactly one side must be the iterator and the other a literal. Both sides being the
    // iterator (`i >= i`) or neither (`x >= y`) does not identify a point in the iteration.
    let ok = match (left == iterator, right == iterator) {
        (true, false) => is_integer_literal(right),
        (false, true) => is_integer_literal(left),
        _ => false,
    };

    if ok {
        Ok(())
    } else {
        Err(TransformError::NotAPrefixOfTheIteration {
            guard: guard.to_string(),
        })
    }
}

/// The kind of transformation an early exit performs, for a diagnostic that names it.
///
/// `Continue` is separate because the two differ in what a backend must do: `Break` shrinks
/// the domain, `Continue` predicates the body. A backend that treated them alike would emit
/// the wrong number of iterations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransformationKind {
    /// The iteration domain is intersected with a shorter range.
    ShrinkDomain,
    /// The domain is unchanged; the body is predicated per iteration.
    PredicateBody,
}

impl std::fmt::Display for TransformationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformationKind::ShrinkDomain => f.write_str("shrink the iteration domain"),
            TransformationKind::PredicateBody => f.write_str("predicate the loop body"),
        }
    }
}

/// What an early exit does to a band, for a backend to act on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Transformation {
    /// Which kind of transformation was applied.
    pub kind: TransformationKind,
    /// The band AFTER the transformation.
    pub band: AffineBand,
    /// The iteration at which the body stops running, when `kind` is `ShrinkDomain`.
    ///
    /// `None` for `PredicateBody`, where no iteration is skipped -- the body merely does not
    /// execute its statements.
    pub stop_at: Option<i64>,
}

impl Transformation {
    /// Transform a band for an early exit.
    ///
    /// `break_at` is the constant at which a `break` fires, and must already have passed
    /// [`early_exit_is_prefix_guard`]. For a `Continue` it is ignored, because a `continue`
    /// does not change the domain.
    pub fn apply(
        band: &AffineBand,
        exit: EarlyExit,
        break_at: Option<i64>,
        point: &[i64],
    ) -> Result<Self, TransformError> {
        match exit {
            EarlyExit::Break => {
                let at = break_at.ok_or(TransformError::NotAPrefixOfTheIteration {
                    guard: "<no constant break point>".to_string(),
                })?;
                let transformed = band.break_before(at, point)?;
                Ok(Self {
                    kind: TransformationKind::ShrinkDomain,
                    band: transformed,
                    stop_at: band.stop_point(Some(at), point),
                })
            }
            EarlyExit::Continue => Ok(Self {
                kind: TransformationKind::PredicateBody,
                band: band.continue_predicate(point)?,
                stop_at: None,
            }),
        }
    }
}
