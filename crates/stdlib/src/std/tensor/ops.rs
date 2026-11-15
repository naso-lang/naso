//! Tensor operations: matmul, elementwise arithmetic, transpose, contraction, outer products

#![allow(unused_variables)]

use super::*;
use crate::core::prelude::*;
use std::ops::{Add, Deref, Div, Mul, Neg, Sub};

// Q1 operations
impl<D: Dims, T, L: Layout + Default> Tensor<Q1, D, T, L> {
    /// Element-wise addition
    pub fn add(&self, rhs: &Self) -> Self
    where
        T: Add<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for add");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() + b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise subtraction
    pub fn sub(&self, rhs: &Self) -> Self
    where
        T: Sub<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for sub");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() - b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise multiplication
    pub fn mul(&self, rhs: &Self) -> Self
    where
        T: Mul<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for mul");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() * b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise division
    pub fn div(&self, rhs: &Self) -> Self
    where
        T: Div<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for div");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() / b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise negation
    pub fn neg(&self) -> Self
    where
        T: Neg<Output = T> + Clone,
    {
        let result: Vec<T> = self.get_slice().iter().map(|a| -a.clone()).collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Scalar addition
    pub fn add_scalar(&self, scalar: T) -> Self
    where
        T: Add<Output = T> + Clone,
    {
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .map(|a| a.clone() + scalar.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Scalar multiplication
    pub fn mul_scalar(&self, scalar: T) -> Self
    where
        T: Mul<Output = T> + Clone,
    {
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .map(|a| a.clone() * scalar.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Matrix multiplication
    pub fn matmul(&self, rhs: &Self) -> Self
    where
        T: Add<Output = T> + Mul<Output = T> + Clone + Default + Copy,
    {
        assert_eq!(
            self.shape().dims()[1],
            rhs.shape().dims()[0],
            "Inner dimensions must match for matmul"
        );

        let m = self.shape().dims()[0];
        let k = self.shape().dims()[1];
        let n = rhs.shape().dims()[1];

        let mut result = Vec::with_capacity(m * n);
        for i in 0..m {
            for j in 0..n {
                let mut sum = T::default();
                for kk in 0..k {
                    sum = sum + self[i * k + kk] * rhs[kk * n + j];
                }
                result.push(sum);
            }
        }
        let shape = ConcreteShape::new(vec![m, n]);
        Self::from_vec(result, shape)
    }
}

// QStar operations
impl<D: Dims, T, L: Layout + Default> Tensor<QStar, D, T, L> {
    /// Element-wise addition
    pub fn add(&self, rhs: &Self) -> Self
    where
        T: Add<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for add");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() + b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise subtraction
    pub fn sub(&self, rhs: &Self) -> Self
    where
        T: Sub<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for sub");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() - b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise multiplication
    pub fn mul(&self, rhs: &Self) -> Self
    where
        T: Mul<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for mul");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() * b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise division
    pub fn div(&self, rhs: &Self) -> Self
    where
        T: Div<Output = T> + Clone,
    {
        assert_eq!(self.shape(), rhs.shape(), "Shape mismatch for div");
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .zip(rhs.get_slice().iter())
            .map(|(a, b)| a.clone() / b.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Element-wise negation
    pub fn neg(&self) -> Self
    where
        T: Neg<Output = T> + Clone,
    {
        let result: Vec<T> = self.get_slice().iter().map(|a| -a.clone()).collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Scalar addition
    pub fn add_scalar(&self, scalar: T) -> Self
    where
        T: Add<Output = T> + Clone,
    {
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .map(|a| a.clone() + scalar.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }

    /// Scalar multiplication
    pub fn mul_scalar(&self, scalar: T) -> Self
    where
        T: Mul<Output = T> + Clone,
    {
        let result: Vec<T> = self
            .get_slice()
            .iter()
            .map(|a| a.clone() * scalar.clone())
            .collect();
        Self::from_vec(result, self.shape().clone())
    }
}

// Generic functions working with both Q1 and QStar via deref
/// Element-wise maximum
pub fn maximum<Q, D: Dims, T, L: Layout + Default>(
    lhs: &Tensor<Q, D, T, L>,
    rhs: &Tensor<Q, D, T, L>,
) -> Tensor<Q, D, T, L>
where
    Q: QttQty,
    T: PartialOrd + Clone,
    Tensor<Q, D, T, L>: Deref<Target = [T]>,
{
    assert_eq!(lhs.shape(), rhs.shape(), "Shape mismatch for maximum");
    let result: Vec<T> = lhs
        .iter()
        .zip(rhs.iter())
        .map(|(a, b)| if a >= b { a.clone() } else { b.clone() })
        .collect();
    Tensor::<Q, D, T, L>::from_quantity_vec(result, lhs.shape().clone())
}

/// Element-wise minimum
pub fn minimum<Q, D: Dims, T, L: Layout + Default>(
    lhs: &Tensor<Q, D, T, L>,
    rhs: &Tensor<Q, D, T, L>,
) -> Tensor<Q, D, T, L>
where
    Q: QttQty,
    T: PartialOrd + Clone,
    Tensor<Q, D, T, L>: Deref<Target = [T]>,
{
    assert_eq!(lhs.shape(), rhs.shape(), "Shape mismatch for minimum");
    let result: Vec<T> = lhs
        .iter()
        .zip(rhs.iter())
        .map(|(a, b)| if a <= b { a.clone() } else { b.clone() })
        .collect();
    Tensor::<Q, D, T, L>::from_quantity_vec(result, lhs.shape().clone())
}

/// ReLU activation
pub fn relu<Q, D: Dims, T, L: Layout + Default>(tensor: &Tensor<Q, D, T, L>) -> Tensor<Q, D, T, L>
where
    Q: QttQty,
    T: PartialOrd + Clone + Default,
    Tensor<Q, D, T, L>: Deref<Target = [T]>,
{
    let result: Vec<T> = tensor
        .iter()
        .map(|a| {
            if a >= &T::default() {
                a.clone()
            } else {
                T::default()
            }
        })
        .collect();
    Tensor::<Q, D, T, L>::from_quantity_vec(result, tensor.shape().clone())
}

/// Sigmoid activation
pub fn sigmoid<Q, D: Dims, T, L: Layout + Default>(
    tensor: &Tensor<Q, D, T, L>,
) -> Tensor<Q, D, T, L>
where
    Q: QttQty,
    T: FloatOps + Clone + Neg<Output = T> + Div<Output = T> + Add<Output = T>,
    Tensor<Q, D, T, L>: Deref<Target = [T]>,
{
    let result: Vec<T> = tensor
        .iter()
        .map(|a| T::one() / (T::one() + (-a.clone()).exp()))
        .collect();
    Tensor::<Q, D, T, L>::from_quantity_vec(result, tensor.shape().clone())
}

/// Float operations trait for sigmoid
pub trait FloatOps: Clone {
    fn one() -> Self;
    fn exp(self) -> Self;
}

impl FloatOps for f32 {
    fn one() -> Self {
        1.0
    }
    fn exp(self) -> Self {
        self.exp()
    }
}

impl FloatOps for f64 {
    fn one() -> Self {
        1.0
    }
    fn exp(self) -> Self {
        self.exp()
    }
}
