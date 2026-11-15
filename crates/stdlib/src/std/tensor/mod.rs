//! Tensor module for Naso standard library
//!
//! Provides QTT-compliant tensor operations with linear type guarantees.

pub mod ops;
pub mod polyhedral_lowering;
#[allow(clippy::module_inception)]
pub mod tensor;

pub use ops::*;
pub use polyhedral_lowering::*;
pub use tensor::*;

use crate::core::prelude::*;

/// Marker trait for QTT quantity kinds
pub trait QttKind: Sized + 'static {
    const NAME: &'static str;
}

/// Proof-only zero-size shape marker
pub struct QttZero;
impl QttKind for QttZero {
    const NAME: &'static str = "QttZero";
}

/// Linear mutable buffer marker
pub struct QttLinear;
impl QttKind for QttLinear {
    const NAME: &'static str = "QttLinear";
}

/// Heap storage marker
pub struct QttHeap;
impl QttKind for QttHeap {
    const NAME: &'static str = "QttHeap";
}

/// Type-level quantity marker for tensor dimensions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Zero,
    Linear,
    Heap,
}

/// Trait for statically known tensor ranks
pub trait Rank: Sized + Copy + 'static {
    const VALUE: usize;
    type Prev: Rank;
}

impl Rank for () {
    const VALUE: usize = 0;
    type Prev = ();
}

#[derive(Copy, Clone)]
pub struct Succ<R: Rank>(std::marker::PhantomData<R>);
impl<R: Rank> Rank for Succ<R> {
    const VALUE: usize = R::VALUE + 1;
    type Prev = R;
}

pub type Rank0 = ();
pub type Rank1 = Succ<Rank0>;
pub type Rank2 = Succ<Rank1>;
pub type Rank3 = Succ<Rank2>;
pub type Rank4 = Succ<Rank3>;
pub type Rank5 = Succ<Rank4>;
pub type Rank6 = Succ<Rank5>;
pub type Rank7 = Succ<Rank6>;
pub type Rank8 = Succ<Rank7>;

/// Type-level dimension list
pub trait Dims: Sized + 'static {
    const RANK: usize;
    const SIZES: &'static [usize];
    type Head: Dim;
    type Tail: Dims;
}

impl Dims for () {
    const RANK: usize = 0;
    const SIZES: &'static [usize] = &[];
    type Head = DimConst<0>;
    type Tail = ();
}

pub struct DimCons<D: Dim, DS: Dims>(std::marker::PhantomData<(D, DS)>);
impl<D: Dim, DS: Dims> Dims for DimCons<D, DS> {
    const RANK: usize = DS::RANK + 1;
    const SIZES: &'static [usize] = &[];
    type Head = D;
    type Tail = DS;
}

/// Type-level dimension constant
pub trait Dim: Sized + 'static {
    const VALUE: usize;
}

#[derive(Copy, Clone)]
pub struct DimConst<const N: usize>;
impl<const N: usize> Dim for DimConst<N> {
    const VALUE: usize = N;
}

/// Quantity trait for QTT
pub trait QttQty: Sized + 'static {
    const QTY: Quantity;
}

#[derive(Copy, Clone)]
pub struct Q0;
impl QttQty for Q0 {
    const QTY: Quantity = Quantity::Zero;
}

#[derive(Copy, Clone)]
pub struct Q1;
impl QttQty for Q1 {
    const QTY: Quantity = Quantity::Linear;
}

#[derive(Copy, Clone)]
pub struct QStar;
impl QttQty for QStar {
    const QTY: Quantity = Quantity::Heap;
}

/// Shape trait for runtime shape validation
/// Note: Not dyn compatible due to Sized bound; use ConcreteShape directly for dynamic dispatch
pub trait Shape: Clone + Send + Sync + 'static {
    fn rank(&self) -> usize;
    fn dims(&self) -> &[usize];
    fn num_elements(&self) -> usize;
    fn strides(&self) -> Vec<usize> {
        let dims = self.dims();
        let mut strides = vec![0; dims.len()];
        if !dims.is_empty() {
            strides[dims.len() - 1] = 1;
            for i in (0..dims.len() - 1).rev() {
                strides[i] = strides[i + 1] * dims[i + 1];
            }
        }
        strides
    }
    fn can_broadcast(&self, other: &Self) -> bool {
        let self_dims = self.dims();
        let other_dims = other.dims();
        let max_rank = self_dims.len().max(other_dims.len());
        for i in 0..max_rank {
            let d1 = self_dims
                .get(self_dims.len().saturating_sub(max_rank - i))
                .copied()
                .unwrap_or(1);
            let d2 = other_dims
                .get(other_dims.len().saturating_sub(max_rank - i))
                .copied()
                .unwrap_or(1);
            if d1 != d2 && d1 != 1 && d2 != 1 {
                return false;
            }
        }
        true
    }
}

/// Concrete shape implementation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcreteShape {
    dims: Vec<usize>,
}

impl ConcreteShape {
    pub fn new(dims: Vec<usize>) -> Self {
        Self { dims }
    }
    pub fn scalar() -> Self {
        Self { dims: vec![] }
    }
    pub fn from_dims(dims: &[usize]) -> Self {
        Self {
            dims: dims.to_vec(),
        }
    }
}

impl Shape for ConcreteShape {
    fn rank(&self) -> usize {
        self.dims.len()
    }
    fn dims(&self) -> &[usize] {
        &self.dims
    }
    fn num_elements(&self) -> usize {
        self.dims.iter().product()
    }
}

/// Layout trait for memory layout
pub trait Layout: Sized + Copy + Clone + Send + Sync + 'static {
    fn element_offset(&self, shape: &ConcreteShape, indices: &[usize]) -> usize;
}

/// Row-major (C-style) layout
#[derive(Copy, Clone, Debug)]
pub struct RowMajor;
impl Layout for RowMajor {
    fn element_offset(&self, shape: &ConcreteShape, indices: &[usize]) -> usize {
        let strides = shape.strides();
        indices
            .iter()
            .zip(strides.iter())
            .map(|(&i, &s)| i * s)
            .sum()
    }
}

/// Column-major (Fortran-style) layout
#[derive(Copy, Clone, Debug)]
pub struct ColMajor;
impl Layout for ColMajor {
    fn element_offset(&self, shape: &ConcreteShape, indices: &[usize]) -> usize {
        let dims = shape.dims();
        let mut offset = 0;
        let mut stride = 1;
        for (i, &d) in dims.iter().enumerate() {
            offset += indices[i] * stride;
            stride *= d;
        }
        offset
    }
}
