//! Generic Tensor structure with QTT quantity enforcement

use super::*;
use crate::core::prelude::*;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

/// Generic tensor with QTT quantity parameter
///
/// Tensor<[Q]; Dims> where:
/// - Q0 = [0]: Proof-only, zero-size shapes (compile-time only, no runtime data)
/// - Q1 = [1]: Linear mutable buffers (unique ownership, move semantics)
/// - QStar = [*]: Heap storage (shared ownership, reference counted)
pub struct Tensor<Q: QttQty, D: Dims, T, L: Layout = RowMajor> {
    data: TensorStorage<T>,
    shape: ConcreteShape,
    layout: L,
    _dims: PhantomData<D>,
    _qty: PhantomData<Q>,
}

/// Storage backend based on QTT quantity
enum TensorStorage<T> {
    Zero(PhantomData<T>),      // Q0: no storage
    Linear(Box<[T]>),          // Q1: unique owned buffer
    Heap(std::sync::Arc<[T]>), // QStar: shared reference counted
}

impl<T> TensorStorage<T> {
    #[allow(dead_code)]
    fn as_ptr(&self) -> *const T {
        match self {
            TensorStorage::Zero(_) => std::ptr::null(),
            TensorStorage::Linear(buf) => buf.as_ptr(),
            TensorStorage::Heap(buf) => buf.as_ptr(),
        }
    }

    #[allow(dead_code)]
    fn as_mut_ptr(&mut self) -> *mut T {
        match self {
            TensorStorage::Zero(_) => std::ptr::null_mut(),
            TensorStorage::Linear(buf) => buf.as_mut_ptr(),
            TensorStorage::Heap(buf) => std::sync::Arc::get_mut(buf)
                .map(|b| b.as_mut_ptr())
                .unwrap_or(std::ptr::null_mut()),
        }
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        match self {
            TensorStorage::Zero(_) => 0,
            TensorStorage::Linear(buf) => buf.len(),
            TensorStorage::Heap(buf) => buf.len(),
        }
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Clone> Clone for TensorStorage<T> {
    fn clone(&self) -> Self {
        match self {
            TensorStorage::Zero(_) => TensorStorage::Zero(PhantomData),
            TensorStorage::Linear(buf) => TensorStorage::Linear(buf.clone()),
            TensorStorage::Heap(buf) => TensorStorage::Heap(buf.clone()),
        }
    }
}

// Common methods for all quantities
impl<Q: QttQty, D: Dims, T, L: Layout + Default> Tensor<Q, D, T, L> {
    /// Get the shape
    pub fn shape(&self) -> &ConcreteShape {
        &self.shape
    }

    /// Get the rank
    pub fn rank(&self) -> usize {
        self.shape.rank()
    }

    /// Get number of elements
    pub fn num_elements(&self) -> usize {
        self.shape.num_elements()
    }

    /// Get the layout
    pub fn layout(&self) -> L {
        self.layout
    }

    /// Check if tensor is empty
    pub fn is_empty(&self) -> bool {
        self.num_elements() == 0
    }

    /// Build a tensor from a vector, choosing the storage the QUANTITY demands.
    ///
    /// This exists because `Q1::from_vec` and `QStar::from_vec` were the only
    /// constructors, so any function generic over `Q` had no way to build its result.
    /// Four element-wise ops (`maximum`, `minimum`, `relu`, `sigmoid`) computed the
    /// correct answer into a local `Vec` and then hit `unimplemented!`, which is the worst
    /// of both worlds: the work is done and thrown away, and the caller gets a panic
    /// instead of a number.
    ///
    /// The quantity decides the storage rather than a default:
    ///
    /// * `Q0` has no runtime value, so it must have no elements. A non-empty vector here
    ///   is a `[0]`-use violation and is refused rather than silently stored.
    /// * `Q1` owns a unique mutable buffer -- one owner, as QTT requires.
    /// * `QStar` shares an `Arc`, which is what makes `[1]`-use semantics expressible.
    ///
    /// Choosing by quantity rather than by convention is the point. Defaulting everything
    /// to `Linear` would let a `QStar` tensor claim exclusive ownership, and the aliasing
    /// that follows is invisible at the type level.
    pub fn from_quantity_vec(vec: Vec<T>, shape: ConcreteShape) -> Self {
        assert_eq!(
            vec.len(),
            shape.num_elements(),
            "Data length must match shape"
        );
        let data = match Q::QTY {
            Quantity::Zero => {
                assert!(
                    vec.is_empty(),
                    "a Q0 tensor is erased before runtime and must carry no elements, \
                     but {} were supplied",
                    vec.len()
                );
                TensorStorage::Zero(PhantomData)
            }
            Quantity::Linear => TensorStorage::Linear(vec.into_boxed_slice()),
            Quantity::Heap => TensorStorage::Heap(vec.into_boxed_slice().into()),
        };
        Tensor {
            data,
            shape,
            layout: L::default(),
            _dims: PhantomData,
            _qty: PhantomData,
        }
    }

    /// Reshape the tensor
    pub fn reshape<D2: Dims>(self, new_shape: ConcreteShape) -> Tensor<Q, D2, T, L> {
        assert_eq!(self.shape.num_elements(), new_shape.num_elements());
        Tensor {
            data: self.data,
            shape: new_shape,
            layout: self.layout,
            _dims: PhantomData,
            _qty: PhantomData,
        }
    }
}

// Q0-specific: proof-only tensors
impl<D: Dims, T, L: Layout + Default> Tensor<Q0, D, T, L> {
    /// Create a proof-only tensor (zero-size, compile-time only)
    pub fn proof(shape: ConcreteShape) -> Self {
        assert_eq!(shape.num_elements(), 0, "Q0 tensor must have zero elements");
        Self {
            data: TensorStorage::Zero(PhantomData),
            shape,
            layout: L::default(),
            _dims: PhantomData,
            _qty: PhantomData,
        }
    }
}

// Q1-specific: unique mutable access
impl<D: Dims, T, L: Layout + Default> Tensor<Q1, D, T, L> {
    /// Create a linear tensor with unique ownership
    pub fn linear(data: Box<[T]>, shape: ConcreteShape) -> Self {
        assert_eq!(
            data.len(),
            shape.num_elements(),
            "Data length must match shape"
        );
        Self {
            data: TensorStorage::Linear(data),
            shape,
            layout: L::default(),
            _dims: PhantomData,
            _qty: PhantomData,
        }
    }

    /// Create a linear tensor from a vector
    pub fn from_vec(vec: Vec<T>, shape: ConcreteShape) -> Self {
        Self::linear(vec.into_boxed_slice(), shape)
    }

    /// Create an uninitialized linear tensor
    pub fn uninitialized(shape: ConcreteShape) -> Self
    where
        T: Default,
    {
        let len = shape.num_elements();
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(T::default());
        }
        Self::from_vec(vec, shape)
    }

    /// Get immutable slice access
    pub fn get_slice(&self) -> &[T] {
        match &self.data {
            TensorStorage::Linear(buf) => buf.as_ref(),
            _ => unreachable!(),
        }
    }

    /// Get mutable slice access (linear, unique)
    pub fn get_mut_slice(&mut self) -> &mut [T] {
        match &mut self.data {
            TensorStorage::Linear(buf) => buf.as_mut(),
            _ => unreachable!(),
        }
    }

    /// Consume and return the underlying buffer
    pub fn into_inner(self) -> Box<[T]> {
        match self.data {
            TensorStorage::Linear(buf) => buf,
            _ => unreachable!(),
        }
    }

    /// Transpose last two dimensions (for matrices)
    pub fn transpose(mut self) -> Self
    where
        D: Dims,
    {
        let mut dims = self.shape.dims().to_vec();
        let len = dims.len();
        if len >= 2 {
            dims.swap(len - 2, len - 1);
        }
        let new_shape = ConcreteShape::new(dims);
        self.shape = new_shape;
        self
    }
}

impl<D: Dims, T, L: Layout + Default> Deref for Tensor<Q1, D, T, L> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.get_slice()
    }
}

impl<D: Dims, T, L: Layout + Default> DerefMut for Tensor<Q1, D, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut_slice()
    }
}

// QStar-specific: shared immutable access
impl<D: Dims, T, L: Layout + Default> Tensor<QStar, D, T, L> {
    /// Create a heap tensor with shared ownership
    pub fn heap(data: std::sync::Arc<[T]>, shape: ConcreteShape) -> Self {
        assert_eq!(
            data.len(),
            shape.num_elements(),
            "Data length must match shape"
        );
        Self {
            data: TensorStorage::Heap(data),
            shape,
            layout: L::default(),
            _dims: PhantomData,
            _qty: PhantomData,
        }
    }

    /// Create a heap tensor from a vector
    pub fn from_vec(vec: Vec<T>, shape: ConcreteShape) -> Self {
        Self::heap(vec.into_boxed_slice().into(), shape)
    }

    /// Create a heap tensor by cloning a linear tensor
    pub fn from_linear(linear: Tensor<Q1, D, T, L>) -> Self {
        let shape = linear.shape().clone();
        let data = linear.into_inner();
        Self::heap(data.into(), shape)
    }

    /// Get immutable slice access (shared)
    pub fn get_slice(&self) -> &[T] {
        match &self.data {
            TensorStorage::Heap(buf) => buf.as_ref(),
            _ => unreachable!(),
        }
    }

    /// Try to get mutable access (fails if shared)
    pub fn try_get_mut_slice(&mut self) -> Option<&mut [T]> {
        match &mut self.data {
            TensorStorage::Heap(buf) => std::sync::Arc::get_mut(buf),
            _ => None,
        }
    }

    /// Transpose last two dimensions (for matrices)
    pub fn transpose(self) -> Self {
        let mut dims = self.shape.dims().to_vec();
        let len = dims.len();
        if len >= 2 {
            dims.swap(len - 2, len - 1);
        }
        let new_shape = ConcreteShape::new(dims);
        Tensor {
            data: self.data,
            shape: new_shape,
            layout: self.layout,
            _dims: PhantomData,
            _qty: PhantomData,
        }
    }
}

impl<D: Dims, T, L: Layout + Default> Deref for Tensor<QStar, D, T, L> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.get_slice()
    }
}

// Indexing support
impl<Q: QttQty, D: Dims, T, L: Layout> std::ops::Index<usize> for Tensor<Q, D, T, L>
where
    Tensor<Q, D, T, L>: Deref<Target = [T]>,
{
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.deref()[index]
    }
}

impl<D: Dims, T, L: Layout + Default> std::ops::IndexMut<usize> for Tensor<Q1, D, T, L> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.deref_mut()[index]
    }
}

// Conversion traits
impl<D: Dims, T: Clone, L: Layout + Default> From<Tensor<Q1, D, T, L>> for Tensor<QStar, D, T, L> {
    fn from(linear: Tensor<Q1, D, T, L>) -> Self {
        Tensor::from_linear(linear)
    }
}

impl<D: Dims, T: Clone, L: Layout + Default> From<Tensor<Q0, D, T, L>> for Tensor<Q1, D, T, L>
where
    T: Default,
{
    fn from(_proof: Tensor<Q0, D, T, L>) -> Self {
        let shape = _proof.shape().clone();
        Tensor::<Q1, D, T, L>::uninitialized(shape)
    }
}

impl<D: Dims, T: Clone, L: Layout + Default> From<Tensor<Q0, D, T, L>> for Tensor<QStar, D, T, L>
where
    T: Default,
{
    fn from(_proof: Tensor<Q0, D, T, L>) -> Self {
        let shape = _proof.shape().clone();
        let len = shape.num_elements();
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(T::default());
        }
        Tensor::<QStar, D, T, L>::from_vec(vec, shape)
    }
}

// Display for debugging
impl<D: Dims, T: std::fmt::Debug, L: Layout + Default> std::fmt::Debug for Tensor<Q1, D, T, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tensor")
            .field("qty", &Q1::QTY)
            .field("shape", &self.shape)
            .field("layout", &std::any::type_name::<L>())
            .field("data", &self.get_slice())
            .finish()
    }
}

impl<D: Dims, T: std::fmt::Debug, L: Layout + Default> std::fmt::Debug for Tensor<QStar, D, T, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tensor")
            .field("qty", &QStar::QTY)
            .field("shape", &self.shape)
            .field("layout", &std::any::type_name::<L>())
            .field("data", &self.get_slice())
            .finish()
    }
}

// Default layout
impl Default for RowMajor {
    fn default() -> Self {
        RowMajor
    }
}

impl Default for ColMajor {
    fn default() -> Self {
        ColMajor
    }
}

/// Tensor view for borrowing without ownership transfer
pub struct TensorView<'a, T, L: Layout + Default = RowMajor> {
    data: &'a [T],
    shape: ConcreteShape,
    #[allow(dead_code)]
    layout: L,
}

impl<'a, T, L: Layout + Default> TensorView<'a, T, L> {
    pub fn new(data: &'a [T], shape: ConcreteShape) -> Self {
        assert_eq!(data.len(), shape.num_elements());
        Self {
            data,
            shape,
            layout: L::default(),
        }
    }

    pub fn shape(&self) -> &ConcreteShape {
        &self.shape
    }

    pub fn as_slice(&self) -> &[T] {
        self.data
    }
}

impl<'a, T, L: Layout + Default> Deref for TensorView<'a, T, L> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

/// Mutable tensor view for unique mutable borrowing
pub struct TensorViewMut<'a, T, L: Layout + Default = RowMajor> {
    data: &'a mut [T],
    shape: ConcreteShape,
    #[allow(dead_code)]
    layout: L,
}

impl<'a, T, L: Layout + Default> TensorViewMut<'a, T, L> {
    pub fn new(data: &'a mut [T], shape: ConcreteShape) -> Self {
        assert_eq!(data.len(), shape.num_elements());
        Self {
            data,
            shape,
            layout: L::default(),
        }
    }

    pub fn shape(&self) -> &ConcreteShape {
        &self.shape
    }

    pub fn as_slice(&self) -> &[T] {
        self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.data
    }
}

impl<'a, T, L: Layout + Default> Deref for TensorViewMut<'a, T, L> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<'a, T, L: Layout + Default> DerefMut for TensorViewMut<'a, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}
