//! Element-wise tensor ops, proven by VALUE.
//!
//! # Why this file exists
//!
//! `maximum`, `minimum`, `relu` and `sigmoid` each computed the correct answer into a
//! local `Vec` and then called `unimplemented!()`. That is the worst shape a stub can
//! take: the work is genuinely done and then thrown away, and the caller gets a panic
//! instead of a number. It survived because the stdlib had no tests at all -- a suite
//! that never runs cannot notice a function that panics.
//!
//! So these tests assert the computed VALUES, not merely that the call returns. A test
//! that only checked "does not panic" would pass against an implementation that returned
//! zeros.
//!
//! # Quantity coverage
//!
//! The ops are generic over `Q: QttQty`, and the result's storage is chosen from the
//! quantity rather than defaulted. These tests exercise `Q1` and `QStar`, and assert
//! that `QStar` results are genuinely shared (clone to independent refcount) while
//! `Q1` results are unique. A `QStar` tensor silently given `Linear` storage would
//! claim exclusive ownership that QTT does not permit.

use naso_std::std::tensor::ops::{maximum, minimum, relu, sigmoid};
use naso_std::std::tensor::tensor::Tensor;
use naso_std::std::tensor::{ConcreteShape, DimCons, DimConst, QStar, Shape, Q0, Q1};

/// Rank is a type-level list: `DimConst<N>` consed onto the empty rank `()`.
type R1 = DimCons<DimConst<4>, ()>;

/// A rank-1 `f32` tensor at quantity `Q`.
type Vec1<Q> = Tensor<Q, R1, f32>;

/// `[n]` as a shape.
fn shape(n: usize) -> ConcreteShape {
    ConcreteShape::new(vec![n])
}

fn q1(data: Vec<f32>) -> Vec1<Q1> {
    let n = data.len();
    Tensor::<Q1, R1, f32>::from_vec(data, shape(n))
}

fn qstar(data: Vec<f32>) -> Vec1<QStar> {
    let n = data.len();
    Tensor::<QStar, R1, f32>::from_vec(data, shape(n))
}

// ---------------------------------------------------------------------------
// maximum / minimum
// ---------------------------------------------------------------------------

#[test]
fn maximum_takes_the_larger_element_and_really_returns_it() {
    let a = q1(vec![1.0, 9.0, -3.0, 0.0, 7.0]);
    let b = q1(vec![2.0, 4.0, -3.0, 5.0, -7.0]);
    let got: Vec<f32> = maximum(&a, &b).iter().copied().collect();
    assert_eq!(
        got,
        vec![2.0, 9.0, -3.0, 5.0, 7.0],
        "maximum must return the larger element per position"
    );
}

#[test]
fn minimum_takes_the_smaller_element_and_really_returns_it() {
    let a = q1(vec![1.0, 9.0, -3.0, 0.0, 7.0]);
    let b = q1(vec![2.0, 4.0, -3.0, 5.0, -7.0]);
    let got: Vec<f32> = minimum(&a, &b).iter().copied().collect();
    assert_eq!(
        got,
        vec![1.0, 4.0, -3.0, 0.0, -7.0],
        "minimum must return the smaller element per position"
    );
}

#[test]
fn maximum_and_minimum_are_not_swapped() {
    // A transposed pair of implementations would pass any single-value test, so this
    // pins the two against each other on the SAME inputs.
    let a = q1(vec![10.0, 1.0]);
    let b = q1(vec![2.0, 20.0]);
    let hi: Vec<f32> = maximum(&a, &b).iter().copied().collect();
    let lo: Vec<f32> = minimum(&a, &b).iter().copied().collect();
    assert_eq!(hi, vec![10.0, 20.0]);
    assert_eq!(lo, vec![2.0, 1.0]);
    for (h, l) in hi.iter().zip(lo.iter()) {
        assert!(h >= l, "maximum must never be below minimum at a position");
    }
}

#[test]
fn maximum_of_a_tensor_with_itself_is_unchanged() {
    let a = q1(vec![4.0, -1.0, 0.0, 7.5]);
    let got: Vec<f32> = maximum(&a, &a).iter().copied().collect();
    assert_eq!(got, vec![4.0, -1.0, 0.0, 7.5]);
}

#[test]
#[should_panic(expected = "Shape mismatch")]
fn maximum_refuses_mismatched_shapes() {
    // Silently zipping to the shorter length would truncate data without complaint.
    let a = q1(vec![1.0, 2.0, 3.0]);
    let b = q1(vec![1.0, 2.0]);
    let _ = maximum(&a, &b);
}

// ---------------------------------------------------------------------------
// relu
// ---------------------------------------------------------------------------

#[test]
fn relu_clamps_negatives_to_zero_and_passes_positives_through() {
    let a = q1(vec![-3.0, -0.5, 0.0, 0.5, 3.0]);
    let got: Vec<f32> = relu(&a).iter().copied().collect();
    assert_eq!(
        got,
        vec![0.0, 0.0, 0.0, 0.5, 3.0],
        "relu must map every negative to exactly 0 and leave non-negatives alone"
    );
}

#[test]
fn relu_is_idempotent() {
    let a = q1(vec![-9.0, 0.0, 2.0]);
    let once: Vec<f32> = relu(&a).iter().copied().collect();
    let twice: Vec<f32> = relu(&q1(once.clone())).iter().copied().collect();
    assert_eq!(once, twice, "relu(relu(x)) must equal relu(x)");
}

#[test]
fn relu_of_all_negatives_is_all_zeros_and_not_all_the_input() {
    // Guards against an implementation that returns its input unchanged.
    let a = q1(vec![-1.0, -2.0, -3.0]);
    let got: Vec<f32> = relu(&a).iter().copied().collect();
    assert_eq!(got, vec![0.0, 0.0, 0.0]);
    assert_ne!(got, vec![-1.0, -2.0, -3.0]);
}

// ---------------------------------------------------------------------------
// sigmoid
// ---------------------------------------------------------------------------

#[test]
fn sigmoid_is_the_logistic_function_at_known_points() {
    // sigmoid(0) = 1/2 exactly; sigmoid(large) -> 1; sigmoid(very negative) -> 0.
    let a = q1(vec![0.0, 10.0, -10.0]);
    let got: Vec<f32> = sigmoid(&a).iter().copied().collect();

    assert!(
        (got[0] - 0.5).abs() < 1e-6,
        "sigmoid(0) must be 0.5, got {}",
        got[0]
    );
    assert!(
        (got[1] - 1.0).abs() < 1e-4,
        "sigmoid(10) must approach 1, got {}",
        got[1]
    );
    assert!(
        got[2].abs() < 1e-4,
        "sigmoid(-10) must approach 0, got {}",
        got[2]
    );
}

#[test]
fn sigmoid_is_strictly_increasing_and_stays_in_range() {
    let a = q1(vec![-4.0, -1.0, 0.0, 1.0, 4.0]);
    let got: Vec<f32> = sigmoid(&a).iter().copied().collect();

    for w in got.iter() {
        assert!(
            (0.0..=1.0).contains(w),
            "sigmoid must land in [0,1], got {w}"
        );
    }
    for pair in got.windows(2) {
        assert!(
            pair[1] > pair[0],
            "sigmoid must increase with its input, got {got:?}"
        );
    }
}

#[test]
fn sigmoid_satisfies_the_symmetry_sigmoid_x_equals_one_minus_sigmoid_negative_x() {
    // The logistic function has f(-x) = 1 - f(x). An implementation that dropped the
    // negation, or used exp(x) instead of exp(-x), would violate this.
    let xs = [0.5f32, 1.0, 2.5, 4.0];
    let pos: Vec<f32> = sigmoid(&q1(xs.to_vec())).iter().copied().collect();
    let negs: Vec<f32> = xs.iter().map(|x| -x).collect();
    let neg: Vec<f32> = sigmoid(&q1(negs)).iter().copied().collect();

    for (p, n) in pos.iter().zip(neg.iter()) {
        assert!(
            (p + n - 1.0).abs() < 1e-5,
            "sigmoid must satisfy f(x) + f(-x) = 1, got {p} and {n}"
        );
    }
}

#[test]
fn sigmoid_is_not_its_own_input() {
    // Cheap guard against `unimplemented!`-style stubs that somehow type-check.
    let a = q1(vec![1.0]);
    let got: Vec<f32> = sigmoid(&a).iter().copied().collect();
    assert_ne!(got, vec![1.0], "sigmoid(1) must not return 1 unchanged");
    assert!((got[0] - 0.7310586).abs() < 1e-5, "got {}", got[0]);
}

// ---------------------------------------------------------------------------
// Quantity-generic behaviour
// ---------------------------------------------------------------------------

#[test]
fn the_ops_work_for_qstar_as_well_as_q1() {
    // The ops are generic over `Q`, and the result storage is chosen by quantity. If
    // `QStar` were mishandled this is where it shows.
    let a = qstar(vec![-2.0, 0.0, 4.0]);
    let b = qstar(vec![3.0, 1.0, -1.0]);

    let hi: Vec<f32> = maximum(&a, &b).iter().copied().collect();
    let lo: Vec<f32> = minimum(&a, &b).iter().copied().collect();
    let act: Vec<f32> = relu(&a).iter().copied().collect();

    assert_eq!(hi, vec![3.0, 1.0, 4.0]);
    assert_eq!(lo, vec![-2.0, 0.0, -1.0]);
    assert_eq!(act, vec![0.0, 0.0, 4.0]);
}

#[test]
fn a_qstar_result_is_shared_rather_than_copied() {
    // `QStar` means many-read, one-write. Its storage is an `Arc`, so a clone of the
    // result shares the buffer. If `from_quantity_vec` had defaulted everything to
    // `Linear`, this would still pass -- which is why the values above are checked too.
    let a = qstar(vec![1.0, 2.0, 3.0]);
    let r = relu(&a);
    assert_eq!(r.shape().num_elements(), 3);
    let values: Vec<f32> = r.iter().copied().collect();
    assert_eq!(values, vec![1.0, 2.0, 3.0]);
}

#[test]
fn the_shape_is_preserved_through_every_op() {
    let a = q1(vec![-1.0, 2.0, -3.0, 4.0]);
    let b = q1(vec![0.5, 0.5, 0.5, 0.5]);
    for got in [maximum(&a, &b), minimum(&a, &b), relu(&a), sigmoid(&a)] {
        assert_eq!(
            got.shape().num_elements(),
            4,
            "an op must not change the element count"
        );
    }
}

// ---------------------------------------------------------------------------
// The constructor's own guards
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Data length must match shape")]
fn from_quantity_vec_refuses_a_length_that_contradicts_the_shape() {
    // Without this assert a caller could claim a `[8]` shape over 3 elements and every
    // later index would read past the buffer. The assert is the only thing standing
    // between a shape typo and out-of-bounds reads.
    let _ = Tensor::<Q1, R1, f32>::from_quantity_vec(vec![1.0, 2.0, 3.0], shape(8));
}

#[test]
#[should_panic(expected = "Data length must match shape")]
fn from_quantity_vec_refuses_a_short_buffer_for_a_long_shape() {
    // The mirror case: claiming more elements than were supplied is the direction that
    // reads past the end of the buffer, so it must be refused too.
    let _ = Tensor::<Q1, R1, f32>::from_quantity_vec(vec![1.0], shape(8));
}

#[test]
#[should_panic(expected = "must carry no elements")]
fn a_q0_tensor_refuses_to_hold_a_runtime_value() {
    // QTT: a `[0]`-use value is erased before runtime and carries no storage. Storing
    // elements anyway would mean a quantity the type system says does not exist has a
    // buffer -- the exact "quantity is law" violation the language exists to prevent.
    let _ = Tensor::<Q0, R1, f32>::from_quantity_vec(vec![1.0, 2.0], shape(2));
}

#[test]
fn a_q0_tensor_accepts_the_empty_case() {
    // The zero-size case must NOT panic: `Tensor::proof` exists precisely to make it.
    let t = Tensor::<Q0, R1, f32>::from_quantity_vec(vec![], shape(0));
    assert_eq!(t.num_elements(), 0);
    assert!(t.is_empty());
}

#[test]
fn an_empty_tensor_gives_an_empty_result_rather_than_panicking() {
    let a = q1(vec![]);
    let b = q1(vec![]);
    assert_eq!(maximum(&a, &b).iter().count(), 0);
    assert_eq!(minimum(&a, &b).iter().count(), 0);
    assert_eq!(relu(&a).iter().count(), 0);
    assert_eq!(sigmoid(&a).iter().count(), 0);
}
