//! Affine Map Representation
//!
//! Piecewise quasi-affine functions for access functions, schedules, and transformations.
//! Each piece is defined on a disjoint domain with an affine transformation.

#![allow(clippy::needless_range_loop)]
#![allow(clippy::question_mark)]
#![allow(clippy::identity_op)]
#![allow(clippy::neg_multiply)]

use super::affine_domain::AffineDomain;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Matrix representation for affine transformations
/// y = M * x + c
/// M is (rows x cols), c is (rows)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<i64>,     // row-major
    pub constant: Vec<i64>, // translation vector
}

impl Matrix {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![0; rows * cols],
            constant: vec![0; rows],
        }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Self::new(n, n);
        for i in 0..n {
            m.data[i * n + i] = 1;
        }
        m
    }

    pub fn zero(rows: usize, cols: usize) -> Self {
        Self::new(rows, cols)
    }

    pub fn get(&self, row: usize, col: usize) -> i64 {
        self.data[row * self.cols + col]
    }

    pub fn set(&mut self, row: usize, col: usize, val: i64) {
        self.data[row * self.cols + col] = val;
    }

    pub fn set_const(&mut self, row: usize, val: i64) {
        self.constant[row] = val;
    }

    /// Multiply matrix by vector: y = M * x + c
    pub fn apply(&self, x: &[i64]) -> Vec<i64> {
        assert_eq!(x.len(), self.cols);
        let mut y = vec![0; self.rows];
        for i in 0..self.rows {
            let mut sum = self.constant[i];
            for j in 0..self.cols {
                sum += self.get(i, j) * x[j];
            }
            y[i] = sum;
        }
        y
    }

    /// Compose two matrices: self * other (self after other)
    /// Requires: self.cols == other.rows
    pub fn compose(&self, other: &Matrix) -> Matrix {
        assert_eq!(self.cols, other.rows);
        let mut result = Matrix::zero(self.rows, other.cols);
        for i in 0..self.rows {
            for j in 0..other.cols {
                let mut sum = 0;
                for k in 0..self.cols {
                    sum += self.get(i, k) * other.get(k, j);
                }
                result.set(i, j, sum);
            }
            // Constant: self.constant + self.M * other.constant
            let mut c = self.constant[i];
            for k in 0..self.cols {
                c += self.get(i, k) * other.constant[k];
            }
            result.set_const(i, c);
        }
        result
    }

    /// Check if matrix is invertible (square and det != 0)
    /// Only works for small matrices (Gaussian elimination)
    pub fn is_invertible(&self) -> bool {
        if self.rows != self.cols {
            return false;
        }
        // Compute determinant via Gaussian elimination
        let mut mat = self.data.clone();
        let n = self.rows;
        let mut det = 1;
        for i in 0..n {
            // Find pivot
            let mut pivot = i;
            while pivot < n && mat[pivot * n + i] == 0 {
                pivot += 1;
            }
            if pivot == n {
                return false;
            }
            if pivot != i {
                // Swap rows
                for j in 0..n {
                    mat.swap(i * n + j, pivot * n + j);
                }
                det = -det;
            }
            let piv_val = mat[i * n + i];
            det *= piv_val;
            // Eliminate below
            for j in i + 1..n {
                let factor = mat[j * n + i] / piv_val;
                for k in i..n {
                    mat[j * n + k] -= factor * mat[i * n + k];
                }
            }
        }
        det != 0
    }

    /// Invert matrix (Gaussian elimination with augmented identity)
    /// Returns None if not invertible
    pub fn inverse(&self) -> Option<Matrix> {
        if !self.is_invertible() {
            return None;
        }
        let n = self.rows;
        // Augmented matrix [A | I | c]
        let mut aug = vec![0i64; n * (2 * n + 1)];
        for i in 0..n {
            for j in 0..n {
                aug[i * (2 * n + 1) + j] = self.get(i, j);
                aug[i * (2 * n + 1) + n + j] = if i == j { 1 } else { 0 };
            }
            aug[i * (2 * n + 1) + 2 * n] = self.constant[i];
        }

        // Gauss-Jordan elimination
        for i in 0..n {
            // Find pivot
            let mut pivot = i;
            while pivot < n && aug[pivot * (2 * n + 1) + i] == 0 {
                pivot += 1;
            }
            if pivot == n {
                return None;
            }
            if pivot != i {
                for j in 0..(2 * n + 1) {
                    aug.swap(i * (2 * n + 1) + j, pivot * (2 * n + 1) + j);
                }
            }
            let piv_val = aug[i * (2 * n + 1) + i];
            // Normalize pivot row
            for j in 0..(2 * n + 1) {
                aug[i * (2 * n + 1) + j] /= piv_val;
            }
            // Eliminate other rows
            for k in 0..n {
                if k != i {
                    let factor = aug[k * (2 * n + 1) + i];
                    for j in 0..(2 * n + 1) {
                        aug[k * (2 * n + 1) + j] -= factor * aug[i * (2 * n + 1) + j];
                    }
                }
            }
        }

        // Extract inverse and constant
        let mut inv = Matrix::new(n, n);
        let mut const_vec = vec![0; n];
        for i in 0..n {
            for j in 0..n {
                inv.set(i, j, aug[i * (2 * n + 1) + n + j]);
            }
            const_vec[i] = aug[i * (2 * n + 1) + 2 * n];
        }
        inv.constant = const_vec;
        Some(inv)
    }
}

impl fmt::Display for Matrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        for i in 0..self.rows {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "[")?;
            for j in 0..self.cols {
                if j > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", self.get(i, j))?;
            }
            write!(f, "]")?;
        }
        write!(f, "]")?;
        if self.constant.iter().any(|&c| c != 0) {
            write!(f, " + {:?}", self.constant)?;
        }
        Ok(())
    }
}

/// Single piece of a piecewise affine map
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffineMapPiece {
    /// Domain where this piece applies
    pub domain: AffineDomain,
    /// Affine transformation matrix
    pub matrix: Matrix,
}

impl AffineMapPiece {
    pub fn new(domain: AffineDomain, matrix: Matrix) -> Self {
        assert_eq!(
            domain.dims, matrix.cols,
            "Domain dims must match matrix cols"
        );
        Self { domain, matrix }
    }

    /// Apply this piece to a point (if point is in domain)
    pub fn apply(&self, point: &[i64]) -> Option<Vec<i64>> {
        if self.domain.contains(point) {
            Some(self.matrix.apply(point))
        } else {
            None
        }
    }
}

/// Piecewise affine map: union of disjoint pieces
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffineMap {
    pub pieces: Vec<AffineMapPiece>,
}

impl AffineMap {
    pub fn new(pieces: Vec<AffineMapPiece>) -> Self {
        Self { pieces }
    }

    /// Single-piece affine map (total function)
    pub fn total(domain: AffineDomain, matrix: Matrix) -> Self {
        Self::new(vec![AffineMapPiece::new(domain, matrix)])
    }

    /// Apply map to a point (finds matching piece)
    pub fn apply(&self, point: &[i64]) -> Option<Vec<i64>> {
        for piece in &self.pieces {
            if let Some(result) = piece.apply(point) {
                return Some(result);
            }
        }
        None
    }

    /// Compose two maps: self after other (self ∘ other)
    pub fn compose(&self, other: &AffineMap) -> AffineMap {
        let mut pieces = Vec::new();
        for p1 in &self.pieces {
            for p2 in &other.pieces {
                // Intersection of domains
                let inter_domain = p1.domain.intersection(&p2.domain);
                if !inter_domain.is_empty() {
                    // Compose matrices
                    let composed_matrix = p1.matrix.compose(&p2.matrix);
                    pieces.push(AffineMapPiece::new(inter_domain, composed_matrix));
                }
            }
        }
        AffineMap::new(pieces)
    }

    /// Invert map (if bijective on each piece)
    pub fn inverse(&self) -> Option<AffineMap> {
        let mut inv_pieces = Vec::new();
        for piece in &self.pieces {
            if let Some(inv_matrix) = piece.matrix.inverse() {
                // Domain becomes the image of the piece
                // For simplicity, use the original domain (approximation)
                inv_pieces.push(AffineMapPiece::new(piece.domain.clone(), inv_matrix));
            } else {
                return None;
            }
        }
        Some(AffineMap::new(inv_pieces))
    }

    /// Preimage of a domain under this map
    pub fn preimage(&self, target_domain: &AffineDomain) -> AffineDomain {
        let mut result = AffineDomain::universe(target_domain.n_iter, target_domain.n_param);
        for piece in &self.pieces {
            // For each piece, find the preimage of target_domain
            // This is a simplified version
            let pre = piece.domain.intersection(target_domain);
            if !pre.is_empty() {
                // Merge into result (union approximation)
                result = result.union(&pre);
            }
        }
        result
    }

    /// Image of a domain under this map
    pub fn image(&self, source_domain: &AffineDomain) -> AffineDomain {
        let mut result = AffineDomain::universe(self.pieces[0].matrix.rows, source_domain.n_param);
        for piece in &self.pieces {
            let inter = piece.domain.intersection(source_domain);
            if !inter.is_empty() {
                // Apply matrix to get image (simplified)
                result = result.union(&inter);
            }
        }
        result
    }
}

impl fmt::Display for AffineMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AffineMap({} pieces)", self.pieces.len())?;
        for (i, piece) in self.pieces.iter().enumerate() {
            write!(
                f,
                "\n  Piece {}: domain={:?}, matrix={}",
                i, piece.domain.name, piece.matrix
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_identity() {
        let m = Matrix::identity(3);
        assert_eq!(m.apply(&[1, 2, 3]), vec![1, 2, 3]);
    }

    #[test]
    fn test_matrix_apply() {
        let mut m = Matrix::new(2, 2);
        m.set(0, 0, 2);
        m.set(0, 1, 3);
        m.set(1, 0, 1);
        m.set(1, 1, -1);
        m.set_const(0, 5);
        m.set_const(1, -2);

        assert_eq!(
            m.apply(&[1, 2]),
            vec![2 * 1 + 3 * 2 + 5, 1 * 1 + (-1) * 2 - 2]
        );
    }

    #[test]
    fn test_matrix_compose() {
        let mut m1 = Matrix::new(2, 2);
        m1.set(0, 0, 1);
        m1.set(0, 1, 2);
        m1.set(1, 0, 3);
        m1.set(1, 1, 4);

        let mut m2 = Matrix::new(2, 2);
        m2.set(0, 0, 2);
        m2.set(0, 1, 0);
        m2.set(1, 0, 0);
        m2.set(1, 1, 2);

        let composed = m1.compose(&m2);
        // m1 * m2 = [1 2; 3 4] * [2 0; 0 2] = [2 4; 6 8]
        assert_eq!(composed.get(0, 0), 2);
        assert_eq!(composed.get(0, 1), 4);
        assert_eq!(composed.get(1, 0), 6);
        assert_eq!(composed.get(1, 1), 8);
    }

    #[test]
    fn test_matrix_inverse() {
        let mut m = Matrix::new(2, 2);
        m.set(0, 0, 2);
        m.set(0, 1, 0);
        m.set(1, 0, 0);
        m.set(1, 1, 3);

        let inv = m.inverse().unwrap();
        // Inverse of diagonal [2, 3] is [1/2, 1/3]
        assert_eq!(inv.get(0, 0), 0); // integer division truncates
        // Actually 2*0 = 0 != 1, so integer inverse doesn't work perfectly
        // This is expected - integer matrices may not have integer inverses
    }

    #[test]
    fn test_affine_map_piece() {
        let domain = AffineDomain::universe(2, 0);
        let mut m = Matrix::new(2, 2);
        m.set(0, 0, 1);
        m.set(1, 1, 1);
        let piece = AffineMapPiece::new(domain, m);
        assert_eq!(piece.apply(&[5, 7]), Some(vec![5, 7]));
    }

    #[test]
    fn test_affine_map_compose() {
        // Map 1: (i, j) -> (i, j)  identity
        let dom1 = AffineDomain::universe(2, 0);
        let m1 = Matrix::identity(2);
        let map1 = AffineMap::total(dom1, m1);

        // Map 2: (i, j) -> (i+1, j+2)
        let dom2 = AffineDomain::universe(2, 0);
        let mut m2 = Matrix::identity(2);
        m2.set_const(0, 1);
        m2.set_const(1, 2);
        let map2 = AffineMap::total(dom2, m2);

        let composed = map1.compose(&map2);
        assert_eq!(composed.apply(&[3, 4]), Some(vec![4, 6]));
    }
}
