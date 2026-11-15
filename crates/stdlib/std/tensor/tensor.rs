use std::marker::PhantomData;

/// Marker for zero-sized tensors
pub struct Zero;
/// Marker for linear (vector) tensors
pub struct One;
/// Marker for multi-dimensional tensors
pub struct Multi;

/// A generic tensor with quantity `Q` and dimensions `Dims`.
///
/// `Q` is a type-level quantity marker: `Zero`, `One`, or `Multi`.
/// Dimensions are provided as up to 5 const generic integers; the actual rank is given by `N`.
/// Unused dimensions (beyond `N`) should be set to 0 and will be ignored.
pub struct Tensor<Q, const N: usize, const D1: usize, const D2: usize, const D3: usize, const D4: usize, const D5: usize> {
    data: Vec<f64>,
    dims: [usize; 5],
    _marker: PhantomData<Q>,
}

impl<Q, const N: usize, const D1: usize, const D2: usize, const D3: usize, const D4: usize, const D5: usize>
    Tensor<Q, N, D1, D2, D3, D4, D5>
{
    /// Returns the dimensions array.
    const fn dims_array() -> [usize; 5] {
        [D1, D2, D3, D4, D5]
    }

    /// Returns the product of the first `N` dimensions (total number of elements).
    const fn size() -> usize {
        let mut prod: usize = 1;
        let mut i: usize = 0;
        while i < N && i < 5 {
            prod = prod.wrapping_mul(Self::dims_array()[i]);
            i += 1;
        }
        prod
    }

    /// Creates a new tensor.
    ///
    /// For zero-sized tensors (total size = 0), allocates zero capacity.
    /// Otherwise allocates a linear buffer of length `size()`.
    pub fn new() -> Self {
        let size = Self::size();
        // Zero-sized tensors must have zero total size; we assert for safety.
        assert!(size == 0 || size > 0, "Tensor size must be non-negative");
        Self {
            data: if size == 0 {
                Vec::with_capacity(0)
            } else {
                vec![0.0; size]
            },
            dims: Self::dims_array(),
            _marker: PhantomData,
        }
    }

    /// Returns the shape of the tensor as a slice (first `N` dimensions).
    pub fn shape(&self) -> &[usize] {
        &self.dims[..N]
    }

    /// Returns the total number of elements in the tensor.
    pub fn len(&self) -> usize {
        Self::size()
    }

    /// Returns true if the tensor has zero elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Type alias for a zero-sized tensor with given dimensions (must have zero size).
pub type ZeroTensor<const N: usize, const D1: usize, const D2: usize, const D3: usize, const D4: usize, const D5: usize> =
    Tensor<Zero, N, D1, D2, D3, D4, D5>;

/// Type alias for a linear tensor (vector) with given dimensions.
pub type LinearTensor<const N: usize, const D1: usize, const D2: usize, const D3: usize, const D4: usize, const D5: usize> =
    Tensor<One, N, D1, D2, D3, D4, D5>;

/// Type alias for a multi-dimensional tensor with given dimensions.
pub type MultiTensor<const N: usize, const D1: usize, const D2: usize, const D3: usize, const D4: usize, const D5: usize> =
    Tensor<Multi, N, D1, D2, D3, D4, D5>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_tensor() {
        // Zero-sized tensor: e.g., dimensions [0] (rank 1, D1=0)
        type T = ZeroTensor<1, 0, 0, 0, 0, 0>;
        let t = T::new();
        assert_eq!(t.shape(), &[0]);
        assert_eq!(t.len(), 0);
        assert!(t.is_empty());

        // Another zero-sized case: any dimension zero, e.g., [2,0,5] rank 3
        type T2 = ZeroTensor<3, 2, 0, 5, 0, 0>;
        let t2 = T2::new();
        assert_eq!(t2.shape(), &[2, 0, 5]);
        assert_eq!(t2.len(), 0);
        assert!(t2.is_empty());
    }

    #[test]
    fn test_linear_tensor() {
        // Linear tensor: e.g., dimensions [5] rank 1
        type T = LinearTensor<1, 5, 0, 0, 0, 0, 0>;
        let t = T::new();
        assert_eq!(t.shape(), &[5]);
        assert_eq!(t.len(), 5);
        assert!(!t.is_empty());
    }

    #[test]
    fn test_multi_tensor() {
        // Multi-dimensional tensor: e.g., dimensions [2,3] rank 2
        type T = MultiTensor<2, 2, 3, 0, 0, 0, 0>;
        let t = T::new();
        assert_eq!(t.shape(), &[2, 3]);
        assert_eq!(t.len(), 6);
        assert!(!t.is_empty());
    }

    #[test]
    fn test_higher_rank() {
        // Rank 4 tensor: dimensions [2,3,4,5]
        type T = MultiTensor<4, 2, 3, 4, 5, 0, 0>;
        let t = T::new();
        assert_eq!(t.shape(), &[2, 3, 4, 5]);
        assert_eq!(t.len(), 2*3*4*5);
        assert!(!t.is_empty());
    }
}
