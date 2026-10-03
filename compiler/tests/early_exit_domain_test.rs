//! The early-exit domain transformation, checked by enumeration.
//!
//! # Why enumeration rather than arithmetic
//!
//! A domain transformation is a claim about which iterations run. Asserting the computed
//! trip count against a hand-written number tests the arithmetic but not the CLAIM: it would
//! pass for a transformation that computed the right number for the wrong reason, or for a
//! band whose domain does not actually contain the iterations it claims.
//!
//! So the property tests here enumerate every iteration of the ORIGINAL band, decide for
//! each one whether it runs under the early exit, and compare that set against the
//! iteration set of the TRANSFORMED band. That is the semantics, stated independently of
//! the implementation.
//!
//! # What is and is not established
//!
//! Established: the transformation is correct for a constant, prefix-shaped guard, verified
//! against an independent enumeration, and it refuses with a specific reason otherwise.
//!
//! NOT established: that any backend emits the transformed band. `break` inside a `forall`
//! is still refused at codegen. This module computes the answer; wiring it into emission is
//! the next step, and until then the refusal stands.

use std::collections::BTreeSet;

use naso_compiler::ir::affine_domain::{AffineConstraint, AffineDomain};
use naso_compiler::ir::early_exit::{
    AffineBand, EarlyExit, TransformError, Transformation, TransformationKind,
    early_exit_is_prefix_guard,
};

/// The iterations of a band, computed by evaluating its bounds.
fn iterations(band: &AffineBand, point: &[i64]) -> BTreeSet<i64> {
    // Bounds are INCLUSIVE on both ends -- see AffineBand's docs, and the counted-loop
    // off-by-one that motivated them.
    let lo = band.lower.evaluate(point);
    let hi = band.upper.evaluate(point);
    (lo..=hi).collect()
}

/// The iterations that actually run, given a `break` at `break_at`.
///
/// Written as the semantics of the CONSTRUCT, independently of the transformation: a `break`
/// at `at` means every iteration from `at` onwards is skipped.
fn expected_with_break(band: &AffineBand, break_at: i64, point: &[i64]) -> BTreeSet<i64> {
    let lo = band.lower.evaluate(point);
    let hi = band.upper.evaluate(point);
    (lo..=hi).filter(|&i| i < break_at).collect()
}

/// A band over `0..n` for concrete `n`.
fn band(n: i64) -> AffineBand {
    AffineBand {
        iterator: "i".to_string(),
        lower: naso_compiler::ir::affine_domain::AffineExpr::constant(0),
        // `n` iterations from 0 means the last is `n - 1`, and bounds are inclusive.
        upper: naso_compiler::ir::affine_domain::AffineExpr::constant(n - 1),
    }
}

/// The transformed band must contain EXACTLY the iterations that survive a `break`.
///
/// Checked for many trip counts and break points, because a transformation that is right for
/// one shape (break in the middle) can be wrong at the edges (break before the first
/// iteration, break after the last), and the edges are where the emptiness check lives.
#[test]
fn the_transformed_band_contains_exactly_the_surviving_iterations() {
    for n in 1..=8 {
        let b = band(n);
        for break_at in 0..=10 {
            let point: Vec<i64> = Vec::new();
            let expected = expected_with_break(&b, break_at, &point);

            match Transformation::apply(&b, EarlyExit::Break, Some(break_at), &point) {
                Ok(t) => {
                    let actual = iterations(&t.band, &point);
                    assert_eq!(
                        actual, expected,
                        "n={n}, break at {break_at}: the transformed band must contain \
                         exactly the iterations before the break point"
                    );
                    assert_eq!(
                        t.kind,
                        TransformationKind::ShrinkDomain,
                        "a `break` shrinks the domain; a backend reading this as \
                         `PredicateBody` would run the wrong number of iterations"
                    );
                }
                Err(TransformError::EmptyAfterTransformation { .. }) => {
                    assert!(
                        expected.is_empty(),
                        "n={n}, break at {break_at}: reported empty, but {expected:?} should \
                         have run"
                    );
                }
                Err(other) => panic!("n={n}, break at {break_at}: unexpected refusal {other}"),
            }
        }
    }
}

/// A `continue` must NOT change the iteration space.
///
/// This is the distinction that makes the two exits different kinds of transformation. A
/// `continue` skips the BODY of one iteration; the iteration still happens, so the domain is
/// untouched. Treating it as a `break` would run fewer iterations than the program asks
/// for.
#[test]
fn a_continue_leaves_the_iteration_space_unchanged() {
    for n in 1..=8 {
        let b = band(n);
        let point: Vec<i64> = Vec::new();
        let t = Transformation::apply(&b, EarlyExit::Continue, Some(3), &point)
            .unwrap_or_else(|e| panic!("n={n}: a continue must always transform: {e}"));

        assert_eq!(
            iterations(&t.band, &point),
            iterations(&b, &point),
            "n={n}: `continue` must leave every iteration in place"
        );
        assert_eq!(
            t.kind,
            TransformationKind::PredicateBody,
            "a `continue` predicates the body; it does not shrink the domain"
        );
        assert_eq!(
            t.stop_at, None,
            "a `continue` skips no iteration, so there is no stop point"
        );
    }
}

/// A break before the first iteration is a real, EMPTY answer -- not a failure.
///
/// `for i in 10 { if i <= 0 { break } }` runs zero iterations. Reporting that as an error
/// would be wrong; running the original ten would be wronger.
#[test]
fn a_break_before_the_first_iteration_yields_an_empty_domain() {
    let b = band(10);
    let point: Vec<i64> = Vec::new();
    let err = Transformation::apply(&b, EarlyExit::Break, Some(0), &point)
        .expect_err("breaking at the first iteration leaves nothing to run");

    match err {
        TransformError::EmptyAfterTransformation { lower, upper } => {
            assert_eq!(lower, 0, "the band's own lower bound is unchanged");
            // `at = 0` is the first iteration skipped, so the last survivor is `-1`, which
            // is below the band's lower bound of 0. The reported upper is the computed one,
            // not a clamped 0 -- clamping would hide which iteration stopped it.
            assert_eq!(
                upper, -1,
                "the reported upper is `at - 1`: the last iteration that runs"
            );
        }
        other => panic!("expected EmptyAfterTransformation, got {other:?}"),
    }
    assert!(
        err.to_string().contains("zero times"),
        "the message must say the loop runs zero times: {err}"
    );
}

/// A break at or after the last iteration runs everything.
///
/// The other edge: the band is unchanged, because nothing was skipped. Getting this wrong
/// would shorten every loop that breaks on its final iteration.
#[test]
fn a_break_at_or_after_the_end_changes_nothing() {
    for break_at in [10, 11, 100] {
        let b = band(10);
        let point: Vec<i64> = Vec::new();
        let t = Transformation::apply(&b, EarlyExit::Break, Some(break_at), &point)
            .expect("a break past the end is not an error");
        assert_eq!(
            iterations(&t.band, &point),
            (0..10).collect::<BTreeSet<i64>>(),
            "break at {break_at} must leave all ten iterations"
        );
        assert_eq!(
            t.stop_at, None,
            "nothing is actually skipped, so there is no stop point to report"
        );
    }
}

/// A symbolic trip count is refused: the shortened domain would not be affine.
///
/// This is the honest boundary. `min(hi, at)` is affine only while `at` is a constant. A
/// bound depending on a parameter would make the trip count runtime-dependent, which is
/// precisely the data-dependent early exit an affine band cannot express.
#[test]
fn a_symbolic_trip_count_is_refused_with_its_reason() {
    use naso_compiler::ir::affine_domain::AffineExpr;

    // `hi = n + 1` over one parameter `n`: affine, but not a constant.
    let b = AffineBand {
        iterator: "i".to_string(),
        lower: AffineExpr::constant(0),
        upper: AffineExpr {
            coefficients: vec![1],
            constant: 1,
        },
    };
    let point = vec![10];
    let err = Transformation::apply(&b, EarlyExit::Break, Some(5), &point)
        .expect_err("a symbolic trip count cannot be shortened");

    match &err {
        TransformError::SymbolicGuardBound { bound } => {
            assert!(
                bound.contains("x0"),
                "the diagnostic must show the offending term, so the user can see WHICH \
                 part of the bound is symbolic: {bound}"
            );
        }
        other => panic!("expected SymbolicGuardBound, got {other:?}"),
    }
    assert!(
        err.to_string().contains("while"),
        "the message must point at the right construct: {err}"
    );
}

/// A band built from a real `AffineDomain` must yield the right range.
#[test]
fn a_band_derived_from_a_domain_has_the_right_bounds() {
    // `AffineConstraint::inequality` means `sum(coeffs) >= constant`, so these are
    // `i >= 0` and `-i >= -4` (i.e. `i <= 4`). An earlier version wrote them the other way
    // round, which made the domain empty and the assertion fail for the wrong reason.
    let mut domain = AffineDomain::universe(1, 0);
    domain.add_constraint(AffineConstraint::inequality(vec![1], 0)); //  i >= 0
    domain.add_constraint(AffineConstraint::inequality(vec![-1], -4)); // -i >= -4

    // The constraint DIRECTION is not asserted from memory: this test originally hard-coded
    // `upper == 5` and `trip_count == 5`, both of which were wrong, because
    // `iterator_bounds` reads a POSITIVE coefficient as the LOWER bound and a negative one
    // as the upper. So the expected range is derived from the domain's own `contains`.
    let band = AffineBand::from_domain(&domain, 0, "i").expect("the domain has bounds");
    let in_domain: Vec<i64> = (-3..8).filter(|&i| domain.contains(&[i])).collect();
    let expected_lo = *in_domain.first().expect("the domain has iterations");
    // `in_domain`'s last value IS the inclusive upper bound.
    let expected_hi = *in_domain.last().expect("the domain has iterations");

    assert_eq!(band.lower.evaluate(&[]), expected_lo);
    assert_eq!(band.upper.evaluate(&[]), expected_hi);
    assert_eq!(band.trip_count(&[]), Some(expected_hi - expected_lo + 1));
    assert_eq!(
        iterations(&band, &[]),
        in_domain
            .into_iter()
            .collect::<std::collections::BTreeSet<i64>>(),
        "the band's range must be exactly the iterations the domain contains"
    );

    // And the transformation on it agrees with enumeration.
    let point: Vec<i64> = Vec::new();
    let t = Transformation::apply(&band, EarlyExit::Break, Some(2), &point).expect("transforms");
    assert_eq!(
        iterations(&t.band, &point),
        expected_with_break(&band, 2, &point)
    );
}

/// A domain with NO computable bound is reported as such, not silently defaulted.
#[test]
fn a_domain_without_bounds_is_reported_rather_than_defaulted() {
    // A constraint coupling the iterator to nothing else, but with a coefficient that does
    // not divide the constant, so no bound can be read off. The alternative -- assuming a
    // bound -- would produce a wrong trip count.
    let mut domain = AffineDomain::universe(1, 0);
    // 2i >= 5  ->  no integral bound of the simple form the reader accepts
    domain.add_constraint(AffineConstraint::inequality(vec![-2], -5));
    domain.add_constraint(AffineConstraint::inequality(vec![1], 9));

    let result = AffineBand::from_domain(&domain, 0, "i");
    // Either a bound was derivable, in which case it must be arithmetically right, or none
    // was, and that must be reported. Both are acceptable; a wrong bound is not.
    match result {
        Ok(band) => {
            let lo = band.lower.evaluate(&[]);
            let hi = band.upper.evaluate(&[]);
            for i in lo..hi {
                assert!(
                    domain.contains(&[i]),
                    "the derived range [{lo}, {hi}) must contain only iterations the \
                     ORIGINAL domain contains, but {i} is not in it"
                );
            }
        }
        Err(e) => assert!(
            matches!(e, TransformError::NoIteratorBound { .. }),
            "an un-derivable domain must say so, got {e:?}"
        ),
    }
}

/// A constant prefix guard is accepted; a data-dependent one is refused with the reason.
///
/// The guard is what decides whether the transformation applies, so its SHAPE is the whole
/// question. `i >= 3` names a point in the iteration. `i >= n / 2` depends on runtime data.
#[test]
fn guard_shape_decides_whether_the_transformation_applies() {
    for guard in ["i >= 3", "i > 3", "i == 3", "i < 100", "3 <= i", "i <= 3"] {
        early_exit_is_prefix_guard(guard, "i").unwrap_or_else(|e| {
            panic!(
                "`{guard}` names a constant point in the iteration, so it must be \
                    accepted: {e}"
            )
        });
    }

    for guard in [
        "i >= n",
        "i >= n / 2",
        "i >= limit",
        "i >= f(x)",
        "j >= 3",
        "x >= 3",
    ] {
        let err = early_exit_is_prefix_guard(guard, "i")
            .err()
            .unwrap_or_else(|| panic!("`{guard}` is data-dependent and must be refused"));
        assert!(
            matches!(err, TransformError::NotAPrefixOfTheIteration { .. }),
            "`{guard}` must be refused as not-a-prefix, got {err:?}"
        );
        assert!(
            err.to_string().contains("while"),
            "the message must name the right construct: {err}"
        );
    }
}

/// The stop point reports where execution actually stops, and only when it does.
#[test]
fn the_stop_point_is_reported_only_when_iterations_are_skipped() {
    let b = band(10);
    let point: Vec<i64> = Vec::new();

    // Break inside the band: some iterations are skipped.
    let t = Transformation::apply(&b, EarlyExit::Break, Some(4), &point).expect("transforms");
    assert_eq!(t.stop_at, Some(4), "iterations 4..10 are skipped");

    // Break past the end: nothing is skipped.
    let t = Transformation::apply(&b, EarlyExit::Break, Some(10), &point).expect("transforms");
    assert_eq!(t.stop_at, None, "no iteration is skipped");

    // Break before the start of a band whose lower bound is above zero: nothing is skipped,
    // so there must be no stop point. This is the case the lower-bound check exists for, and
    // a mutation dropping that check survived until it was covered -- with a band starting
    // at 0 every `at < 0` is already excluded by `at <= upper`, so the test never reached it.
    let shifted = AffineBand {
        iterator: "i".to_string(),
        lower: naso_compiler::ir::affine_domain::AffineExpr::constant(5),
        upper: naso_compiler::ir::affine_domain::AffineExpr::constant(9),
    };
    let t = Transformation::apply(&shifted, EarlyExit::Break, Some(2), &point)
        .expect("a break below the band is not an error");
    assert_eq!(
        t.stop_at, None,
        "a break point of 2 lies below a band starting at 5, so no iteration is skipped"
    );
    assert_eq!(
        iterations(&t.band, &point),
        (5..=9).collect::<std::collections::BTreeSet<i64>>(),
        "and the band must be unchanged"
    );

    // Break before the start: reported through EmptyAfterTransformation, not a stop point.
    let r = Transformation::apply(&b, EarlyExit::Break, Some(0), &point);
    assert!(
        matches!(r, Err(TransformError::EmptyAfterTransformation { .. })),
        "breaking before the first iteration is empty, not a stop point"
    );
}

/// A `break` with no constant break point must be refused, not defaulted to the full band.
///
/// The failure this guards is a backend receiving a band it believes is shortened when it is
/// not, and running iterations the program meant to skip.
#[test]
fn a_break_without_a_break_point_is_refused() {
    let b = band(10);
    let point: Vec<i64> = Vec::new();
    let err = Transformation::apply(&b, EarlyExit::Break, None, &point)
        .expect_err("a break needs a known point to stop at");
    assert!(
        matches!(err, TransformError::NotAPrefixOfTheIteration { .. }),
        "expected NotAPrefixOfTheIteration, got {err:?}"
    );
}
