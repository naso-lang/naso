//! Polyhedral Scheduling for Hybrid Classical/Quantum Loops
//!
//! This module provides polyhedral scheduling optimizations for loops
//! that contain both classical and quantum operations. It identifies
//! hybrid loops and applies transformations to optimize the schedule.

use crate::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId, ScheduleValidationError};
use crate::ir::affine_domain::{AffineDomain, AffineConstraint, ConstraintType};
use crate::ir::affine_map::{AffineMap, Matrix};
use std::collections::HashMap;

/// Hybrid loop classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    /// Purely classical loop
    Classical,
    /// Purely quantum loop
    Quantum,
    /// Mixed classical and quantum operations
    Hybrid,
}

/// Result of hybrid loop analysis
#[derive(Debug, Clone)]
pub struct HybridLoopAnalysis {
    pub kind: LoopKind,
    pub classical_ops: usize,
    pub quantum_ops: usize,
    pub parallel_dimensions: Vec<usize>,
    pub sequential_dimensions: Vec<usize>,
}

/// Polyhedral scheduler for hybrid loops
pub struct PolyhedralScheduler {
    /// Maximum number of scheduling dimensions to consider
    max_dims: usize,
    /// Enable tiling optimization
    enable_tiling: bool,
    /// Enable loop fusion
    enable_fusion: bool,
}

impl PolyhedralScheduler {
    pub fn new() -> Self {
        Self {
            max_dims: 4,
            enable_tiling: true,
            enable_fusion: true,
        }
    }

    pub fn with_max_dims(mut self, max_dims: usize) -> Self {
        self.max_dims = max_dims;
        self
    }

    pub fn with_tiling(mut self, enable: bool) -> Self {
        self.enable_tiling = enable;
        self
    }

    pub fn with_fusion(mut self, enable: bool) -> Self {
        self.enable_fusion = enable;
        self
    }

    /// Analyze a schedule tree for hybrid loops
    pub fn analyze(&self, tree: &ScheduleTree) -> Result<Vec<HybridLoopAnalysis>, ScheduleValidationError> {
        tree.validate()?;
        let mut analyses = Vec::new();
        self.analyze_node(&tree.root, &mut analyses);
        Ok(analyses)
    }

    fn analyze_node(&self, node: &ScheduleNode, analyses: &mut Vec<HybridLoopAnalysis>) {
        match node {
            ScheduleNode::Band { members, coincident, child, .. } => {
                // Analyze this band for hybrid operations
                let analysis = self.analyze_band(members, coincident);
                if analysis.kind != LoopKind::Classical {
                    analyses.push(analysis);
                }
                self.analyze_node(child, analyses);
            }
            ScheduleNode::Sequence { children } => {
                for c in children {
                    self.analyze_node(c, analyses);
                }
            }
            ScheduleNode::Filter { child, .. } => {
                self.analyze_node(child, analyses);
            }
            ScheduleNode::Context { child, .. } => {
                self.analyze_node(child, analyses);
            }
            ScheduleNode::Extension { child, .. } => {
                self.analyze_node(child, analyses);
            }
            ScheduleNode::Domain { .. } | ScheduleNode::Empty => {}
        }
    }

    fn analyze_band(&self, members: &[AffineMap], coincident: &[bool]) -> HybridLoopAnalysis {
        let mut parallel_dims = Vec::new();
        let mut sequential_dims = Vec::new();

        for (i, &c) in coincident.iter().enumerate() {
            if c {
                parallel_dims.push(i);
            } else {
                sequential_dims.push(i);
            }
        }

        // In a real implementation, this would inspect the loop body
        // to count classical vs quantum operations
        HybridLoopAnalysis {
            kind: LoopKind::Hybrid, // Conservative assumption
            classical_ops: 0,
            quantum_ops: 0,
            parallel_dimensions: parallel_dims,
            sequential_dimensions: sequential_dims,
        }
    }

    /// Apply polyhedral optimizations to a schedule tree
    pub fn optimize(&self, tree: &ScheduleTree) -> Result<ScheduleTree, ScheduleValidationError> {
        let analyses = self.analyze(tree)?;
        
        if analyses.is_empty() {
            return Ok(tree.clone());
        }

        let mut optimized_root = self.optimize_node(&tree.root, &analyses)?;
        let mut optimized_tree = ScheduleTree::new(optimized_root, tree.parameters.clone());
        optimized_tree.validate()?;
        Ok(optimized_tree)
    }

    fn optimize_node(&self, node: &ScheduleNode, analyses: &[HybridLoopAnalysis]) -> Result<ScheduleNode, ScheduleValidationError> {
        match node {
            ScheduleNode::Band { members, coincident, iterators, child } => {
                // Apply tiling to hybrid bands
                let optimized_child = self.optimize_node(child, analyses)?;
                
                // Find the analysis for this band (if any)
                let parallel_dims: Vec<usize> = analyses.iter()
                    .find(|a| a.kind != LoopKind::Classical)
                    .map(|a| a.parallel_dimensions.clone())
                    .unwrap_or_default();

                if self.enable_tiling && !parallel_dims.is_empty() {
                    // Add tiling extension for parallel dimensions
                    let tile_sizes = vec![32; parallel_dims.len()];
                    let tiled_child = ScheduleNode::Extension {
                        sizes: tile_sizes,
                        child: Box::new(optimized_child),
                    };
                    Ok(ScheduleNode::band_with_iterators(
                        members.clone(),
                        coincident.clone(),
                        iterators.clone(),
                        tiled_child,
                    ))
                } else {
                    Ok(ScheduleNode::band_with_iterators(
                        members.clone(),
                        coincident.clone(),
                        iterators.clone(),
                        optimized_child,
                    ))
                }
            }
            ScheduleNode::Sequence { children } => {
                if self.enable_fusion && children.len() > 1 {
                    // Try to fuse adjacent compatible loops
                    let fused_children = self.try_fuse_loops(children, analyses)?;
                    let optimized: Result<Vec<_>, _> = fused_children
                        .into_iter()
                        .map(|c| self.optimize_node(&c, analyses))
                        .collect();
                    Ok(ScheduleNode::Sequence { children: optimized? })
                } else {
                    let optimized: Result<Vec<_>, _> = children
                        .into_iter()
                        .map(|c| self.optimize_node(c, analyses))
                        .collect();
                    Ok(ScheduleNode::Sequence { children: optimized? })
                }
            }
            ScheduleNode::Filter { domain, child } => {
                let optimized_child = self.optimize_node(child, analyses)?;
                Ok(ScheduleNode::Filter {
                    domain: domain.clone(),
                    child: Box::new(optimized_child),
                })
            }
            ScheduleNode::Context { domain, child } => {
                let optimized_child = self.optimize_node(child, analyses)?;
                Ok(ScheduleNode::Context {
                    domain: domain.clone(),
                    child: Box::new(optimized_child),
                })
            }
            ScheduleNode::Extension { sizes, child } => {
                let optimized_child = self.optimize_node(child, analyses)?;
                Ok(ScheduleNode::Extension {
                    sizes: sizes.clone(),
                    child: Box::new(optimized_child),
                })
            }
            ScheduleNode::Domain { stmt_id, domain } => Ok(ScheduleNode::Domain {
                stmt_id: *stmt_id,
                domain: domain.clone(),
            }),
            ScheduleNode::Empty => Ok(ScheduleNode::Empty),
        }
    }

    fn try_fuse_loops(&self, children: &[ScheduleNode], _analyses: &[HybridLoopAnalysis]) -> Result<Vec<ScheduleNode>, ScheduleValidationError> {
        // Simplified: just return children as-is for now
        // A real implementation would check for compatible iteration domains
        // and fuse them when beneficial
        Ok(children.to_vec())
    }

    /// Create an optimized schedule for a quantum kernel with classical pre/post processing
    pub fn create_hybrid_schedule(
        &self,
        classical_pre_ops: usize,
        quantum_ops: usize,
        classical_post_ops: usize,
        qubit_count: usize,
    ) -> Result<ScheduleTree, ScheduleValidationError> {
        // Create a schedule with three phases:
        // 1. Classical pre-processing (parallel)
        // 2. Quantum execution (sequential, requires coherence)
        // 3. Classical post-processing (parallel)

        let mut children = Vec::new();

        // Classical pre-processing band
        if classical_pre_ops > 0 {
            let pre_domain = self.create_domain(1, classical_pre_ops);
            let pre_map = self.create_identity_map(&pre_domain, 0);
            let pre_band = ScheduleNode::band(
                vec![pre_map],
                vec![true], // Parallel
                ScheduleNode::domain(StmtId(0), pre_domain),
            );
            children.push(pre_band);
        }

        // Quantum execution band
        if quantum_ops > 0 {
            let quantum_domain = self.create_domain(1, quantum_ops);
            let quantum_map = self.create_identity_map(&quantum_domain, 0);
            let quantum_band = ScheduleNode::band(
                vec![quantum_map],
                vec![false], // Sequential - quantum ops must execute in order
                ScheduleNode::domain(StmtId(1), quantum_domain),
            );
            children.push(quantum_band);
        }

        // Classical post-processing band
        if classical_post_ops > 0 {
            let post_domain = self.create_domain(1, classical_post_ops);
            let post_map = self.create_identity_map(&post_domain, 0);
            let post_band = ScheduleNode::band(
                vec![post_map],
                vec![true], // Parallel
                ScheduleNode::domain(StmtId(2), post_domain),
            );
            children.push(post_band);
        }

        let root = if children.len() == 1 {
            children.into_iter().next().unwrap()
        } else {
            ScheduleNode::Sequence { children }
        };

        let tree = ScheduleTree::new(root, vec!["N".to_string()]);
        tree.validate()?;
        Ok(tree)
    }

    fn create_domain(&self, dims: usize, extent: usize) -> AffineDomain {
        let mut constraints = Vec::new();
        for d in 0..dims {
            // Lower bound: x_d >= 0
            let mut lower = vec![0; dims + 1];
            lower[d] = 1;
            constraints.push(AffineConstraint::inequality(lower, 0));

            // Upper bound: x_d < extent => -x_d >= -extent + 1
            let mut upper = vec![0; dims + 1];
            upper[d] = -1;
            constraints.push(AffineConstraint::inequality(upper, -(extent as i64 - 1)));
        }
        AffineDomain::new(dims, 1, constraints)
    }

    fn create_identity_map(&self, domain: &AffineDomain, dim: usize) -> AffineMap {
        let mut m = Matrix::new(1, domain.dims);
        m.set(0, dim, 1);
        AffineMap::total(domain.clone(), m)
    }
}

impl Default for PolyhedralScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Schedule transformation utilities
pub mod transformations {
    use super::*;
    use crate::ir::affine_map::AffineMap;

    /// Apply loop tiling to a band
    pub fn tile_band(
        members: &[AffineMap],
        coincident: &[bool],
        tile_sizes: &[usize],
    ) -> Result<Vec<AffineMap>, ScheduleValidationError> {
        if members.len() != tile_sizes.len() {
            return Err(ScheduleValidationError::BandDimensionMismatch);
        }

        let mut tiled = Vec::new();
        for (i, (member, &size)) in members.iter().zip(tile_sizes.iter()).enumerate() {
            // Create tile map: t = floor(i / size)
            let tile_map = AffineMap::total(
                member.pieces[0].domain.clone(),
                // Simplified: just clone for now
                member.pieces[0].matrix.clone(),
            );
            tiled.push(tile_map);
        }
        Ok(tiled)
    }

    /// Check if two bands can be fused
    pub fn can_fuse(band1: &ScheduleNode, band2: &ScheduleNode) -> bool {
        match (band1, band2) {
            (ScheduleNode::Band { .. }, ScheduleNode::Band { .. }) => {
                // Check if iteration domains are compatible
                // Simplified: always return false for safety
                false
            }
            _ => false,
        }
    }

    /// Create a parallel schedule for independent quantum gates
    pub fn create_parallel_gate_schedule(
        gate_count: usize,
        max_parallel: usize,
    ) -> Result<ScheduleTree, ScheduleValidationError> {
        let domain = AffineDomain::new(
            1,
            1,
            vec![
                AffineConstraint::inequality(vec![1, 0], 0),                    // i >= 0
                AffineConstraint::inequality(vec![-1, 1], gate_count as i64),  // -i + N >= N  =>  i <= N-1 where N=gate_count
            ],
        );

        let mut m = Matrix::new(1, 2);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        let band = ScheduleNode::band(
            vec![map],
            vec![gate_count <= max_parallel], // Parallel if within limit
            ScheduleNode::domain(StmtId(0), domain),
        );

        let tree = ScheduleTree::new(band, vec!["N".to_string()]);
        tree.validate()?;
        Ok(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polyhedral_scheduler_creation() {
        let scheduler = PolyhedralScheduler::new();
        assert_eq!(scheduler.max_dims, 4);
        assert!(scheduler.enable_tiling);
        assert!(scheduler.enable_fusion);
    }

    #[test]
    fn test_create_hybrid_schedule() {
        let scheduler = PolyhedralScheduler::new();
        let tree = scheduler.create_hybrid_schedule(10, 5, 8, 4).unwrap();
        assert!(tree.validate().is_ok());
        
        // Check that tree has bands for each phase
        let bands = tree.root.collect_bands();
        // Should have 3 bands (pre, quantum, post)
        assert_eq!(bands.len(), 3);
    }

    #[test]
    fn test_create_parallel_gate_schedule() {
        let tree = transformations::create_parallel_gate_schedule(8, 4).unwrap();
        assert!(tree.validate().is_ok());
        
        let bands = tree.root.collect_bands();
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].len(), 1);
    }

    #[test]
    fn test_analyze_hybrid_loop() {
        let scheduler = PolyhedralScheduler::new();
        
        // Create a simple hybrid schedule
        // 1 iterator (i), 1 parameter (N)
        let domain = AffineDomain::new(
            1,
            1,
            vec![
                AffineConstraint::inequality(vec![1, 0], 0),       // i >= 0
                AffineConstraint::inequality(vec![-1, 1], 1),      // -i + N >= 1  =>  i <= N-1
            ],
        );
        
        let mut m = Matrix::new(1, 2);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);
        
        let band = ScheduleNode::band(
            vec![map],
            vec![true],
            ScheduleNode::domain(StmtId(0), domain),
        );
        
        let tree = ScheduleTree::new(band, vec!["N".to_string()]);
        let analyses = scheduler.analyze(&tree).unwrap();
        
        // Should detect as hybrid
        assert_eq!(analyses.len(), 1);
        assert_eq!(analyses[0].kind, LoopKind::Hybrid);
    }
}