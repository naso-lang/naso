//! Polyhedral IR lowering bindings for affine loop tiling, fusion, and vectorization

use super::*;
use crate::core::prelude::*;

/// Polyhedral schedule representation
#[derive(Debug, Clone)]
pub struct Schedule {
    pub domain: AffineDomain,
    pub schedule: AffineMap,
    pub transforms: Vec<ScheduleTransform>,
}

/// Affine domain for polyhedral representation
#[derive(Debug, Clone)]
pub struct AffineDomain {
    pub constraints: Vec<AffineConstraint>,
    pub symbols: Vec<Symbol>,
}

/// Affine constraint: a_1*x_1 + ... + a_n*x_n + b_1*s_1 + ... + b_m*s_m + c >= 0
#[derive(Debug, Clone)]
pub struct AffineConstraint {
    pub coeffs: Vec<i64>,
    pub symbol_coeffs: Vec<i64>,
    pub constant: i64,
}

/// Symbolic parameter
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Symbol {
    pub name: String,
    pub lower_bound: Option<i64>,
    pub upper_bound: Option<i64>,
}

/// Affine map: y = A*x + B*s + c
#[derive(Debug, Clone)]
pub struct AffineMap {
    pub matrix: Vec<Vec<i64>>,
    pub symbol_matrix: Vec<Vec<i64>>,
    pub constant: Vec<i64>,
}

/// Schedule transformation
#[derive(Debug, Clone)]
pub enum ScheduleTransform {
    /// Tile a loop nest
    Tile {
        loop_depth: usize,
        tile_sizes: Vec<usize>,
    },
    /// Fuse two loops
    Fuse {
        loop_depth1: usize,
        loop_depth2: usize,
    },
    /// Vectorize a loop
    Vectorize {
        loop_depth: usize,
        vector_width: usize,
    },
    /// Parallelize a loop
    Parallelize { loop_depth: usize },
    /// Interchange two loops
    Interchange {
        loop_depth1: usize,
        loop_depth2: usize,
    },
    /// Unroll a loop
    Unroll { loop_depth: usize, factor: usize },
    /// Shift a loop
    Shift { loop_depth: usize, offset: i64 },
    /// Skew a loop
    Skew { loop_depth: usize, factor: i64 },
}

/// Tensor operation for polyhedral lowering
#[derive(Debug, Clone)]
pub enum TensorOp {
    MatMul {
        lhs_shape: ConcreteShape,
        rhs_shape: ConcreteShape,
        out_shape: ConcreteShape,
    },
    ElementWise {
        op: ElementWiseOp,
        shape: ConcreteShape,
    },
    Contraction {
        lhs_shape: ConcreteShape,
        rhs_shape: ConcreteShape,
        lhs_dim: usize,
        rhs_dim: usize,
        out_shape: ConcreteShape,
    },
    OuterProduct {
        lhs_shape: ConcreteShape,
        rhs_shape: ConcreteShape,
        out_shape: ConcreteShape,
    },
    Transpose {
        shape: ConcreteShape,
        perm: Vec<usize>,
    },
    Broadcast {
        shape: ConcreteShape,
        target_shape: ConcreteShape,
    },
    Reduction {
        op: ReductionOp,
        shape: ConcreteShape,
        dim: Option<usize>,
    },
}

/// Element-wise operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementWiseOp {
    Add,
    Sub,
    Mul,
    Div,
    Maximum,
    Minimum,
    Relu,
    Sigmoid,
}

/// Reduction operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionOp {
    Sum,
    Mean,
    Max,
    Min,
    Prod,
}

/// Polyhedral lowering context
#[derive(Default)]
pub struct LoweringContext {
    pub schedules: Vec<Schedule>,
    pub next_schedule_id: usize,
}

impl LoweringContext {
    pub fn new() -> Self {
        Self {
            schedules: Vec::new(),
            next_schedule_id: 0,
        }
    }

    /// Create a schedule for a tensor operation
    pub fn create_schedule(&mut self, op: &TensorOp) -> ScheduleId {
        let schedule = match op {
            TensorOp::MatMul {
                lhs_shape,
                rhs_shape,
                out_shape,
            } => self.lower_matmul(lhs_shape, rhs_shape, out_shape),
            TensorOp::ElementWise { op, shape } => self.lower_elementwise(*op, shape),
            TensorOp::Contraction {
                lhs_shape,
                rhs_shape,
                lhs_dim,
                rhs_dim,
                out_shape,
            } => self.lower_contraction(lhs_shape, rhs_shape, *lhs_dim, *rhs_dim, out_shape),
            TensorOp::OuterProduct {
                lhs_shape,
                rhs_shape,
                out_shape,
            } => self.lower_outer(lhs_shape, rhs_shape, out_shape),
            TensorOp::Transpose { shape, perm } => self.lower_transpose(shape, perm),
            TensorOp::Broadcast {
                shape,
                target_shape,
            } => self.lower_broadcast(shape, target_shape),
            TensorOp::Reduction { op, shape, dim } => self.lower_reduction(*op, shape, *dim),
        };

        let id = ScheduleId(self.next_schedule_id);
        self.schedules.push(schedule);
        self.next_schedule_id += 1;
        id
    }

    /// Get a schedule by ID
    pub fn get_schedule(&self, id: ScheduleId) -> Option<&Schedule> {
        self.schedules.get(id.0)
    }

    /// Apply transformations to a schedule
    pub fn apply_transforms(&mut self, id: ScheduleId, transforms: Vec<ScheduleTransform>) {
        if let Some(schedule) = self.schedules.get_mut(id.0) {
            schedule.transforms.extend(transforms);
        }
    }

    /// Lower matrix multiplication to polyhedral representation
    fn lower_matmul(
        &self,
        lhs_shape: &ConcreteShape,
        rhs_shape: &ConcreteShape,
        _out_shape: &ConcreteShape,
    ) -> Schedule {
        let m = lhs_shape.dims()[0];
        let k = lhs_shape.dims()[1];
        let n = rhs_shape.dims()[1];

        // Domain: 0 <= i < m, 0 <= j < n, 0 <= k < k
        let constraints = vec![
            // i >= 0
            AffineConstraint {
                coeffs: vec![1, 0, 0],
                symbol_coeffs: vec![],
                constant: 0,
            },
            // i < m
            AffineConstraint {
                coeffs: vec![-1, 0, 0],
                symbol_coeffs: vec![],
                constant: m as i64 - 1,
            },
            // j >= 0
            AffineConstraint {
                coeffs: vec![0, 1, 0],
                symbol_coeffs: vec![],
                constant: 0,
            },
            // j < n
            AffineConstraint {
                coeffs: vec![0, -1, 0],
                symbol_coeffs: vec![],
                constant: n as i64 - 1,
            },
            // k >= 0
            AffineConstraint {
                coeffs: vec![0, 0, 1],
                symbol_coeffs: vec![],
                constant: 0,
            },
            // k < k
            AffineConstraint {
                coeffs: vec![0, 0, -1],
                symbol_coeffs: vec![],
                constant: k as i64 - 1,
            },
        ];

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        // Schedule: (i, j, k) -> (i, j, k) - identity for now
        let schedule = AffineMap {
            matrix: vec![vec![1, 0, 0], vec![0, 1, 0], vec![0, 0, 1]],
            symbol_matrix: vec![],
            constant: vec![0, 0, 0],
        };

        Schedule {
            domain,
            schedule,
            transforms: vec![
                ScheduleTransform::Tile {
                    loop_depth: 0,
                    tile_sizes: vec![32, 32, 32],
                },
                ScheduleTransform::Vectorize {
                    loop_depth: 2,
                    vector_width: 4,
                },
            ],
        }
    }

    /// Lower element-wise operation
    fn lower_elementwise(&self, _op: ElementWiseOp, shape: &ConcreteShape) -> Schedule {
        let rank = shape.rank();
        let mut constraints = Vec::new();

        for i in 0..rank {
            let dim = shape.dims()[i];
            // idx_i >= 0
            let mut coeffs = vec![0; rank];
            coeffs[i] = 1;
            constraints.push(AffineConstraint {
                coeffs: coeffs.clone(),
                symbol_coeffs: vec![],
                constant: 0,
            });
            // idx_i < dim
            coeffs[i] = -1;
            constraints.push(AffineConstraint {
                coeffs,
                symbol_coeffs: vec![],
                constant: dim as i64 - 1,
            });
        }

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        let schedule = AffineMap {
            matrix: (0..rank)
                .map(|i| {
                    let mut row = vec![0; rank];
                    row[i] = 1;
                    row
                })
                .collect(),
            symbol_matrix: vec![],
            constant: vec![0; rank],
        };

        let mut transforms = vec![ScheduleTransform::Parallelize { loop_depth: 0 }];

        // Add vectorization hint for last dimension if large enough
        if let Some(&last_dim) = shape.dims().last() {
            if last_dim >= 4 {
                transforms.push(ScheduleTransform::Vectorize {
                    loop_depth: rank - 1,
                    vector_width: 4,
                });
            }
        }

        Schedule {
            domain,
            schedule,
            transforms,
        }
    }

    /// Lower tensor contraction
    fn lower_contraction(
        &self,
        lhs_shape: &ConcreteShape,
        _rhs_shape: &ConcreteShape,
        lhs_dim: usize,
        _rhs_dim: usize,
        out_shape: &ConcreteShape,
    ) -> Schedule {
        let rank = out_shape.rank();
        let contract_size = lhs_shape.dims()[lhs_dim];

        let mut constraints = Vec::new();

        // Output domain constraints
        for i in 0..rank {
            let dim = out_shape.dims()[i];
            let mut coeffs = vec![0; rank + 1]; // +1 for contraction dimension
            coeffs[i] = 1;
            constraints.push(AffineConstraint {
                coeffs: coeffs.clone(),
                symbol_coeffs: vec![],
                constant: 0,
            });
            coeffs[i] = -1;
            constraints.push(AffineConstraint {
                coeffs,
                symbol_coeffs: vec![],
                constant: dim as i64 - 1,
            });
        }

        // Contraction dimension constraint
        let mut k_coeffs = vec![0; rank + 1];
        k_coeffs[rank] = 1;
        constraints.push(AffineConstraint {
            coeffs: k_coeffs.clone(),
            symbol_coeffs: vec![],
            constant: 0,
        });
        k_coeffs[rank] = -1;
        constraints.push(AffineConstraint {
            coeffs: k_coeffs,
            symbol_coeffs: vec![],
            constant: contract_size as i64 - 1,
        });

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        let schedule = AffineMap {
            matrix: (0..=rank)
                .map(|i| {
                    let mut row = vec![0; rank + 1];
                    row[i] = 1;
                    row
                })
                .collect(),
            symbol_matrix: vec![],
            constant: vec![0; rank + 1],
        };

        Schedule {
            domain,
            schedule,
            transforms: vec![
                ScheduleTransform::Tile {
                    loop_depth: 0,
                    tile_sizes: vec![32; rank + 1],
                },
                ScheduleTransform::Vectorize {
                    loop_depth: rank,
                    vector_width: 4,
                },
            ],
        }
    }

    /// Lower outer product
    fn lower_outer(
        &self,
        _lhs_shape: &ConcreteShape,
        _rhs_shape: &ConcreteShape,
        out_shape: &ConcreteShape,
    ) -> Schedule {
        let rank = out_shape.rank();
        let mut constraints = Vec::new();

        for i in 0..rank {
            let dim = out_shape.dims()[i];
            let mut coeffs = vec![0; rank];
            coeffs[i] = 1;
            constraints.push(AffineConstraint {
                coeffs: coeffs.clone(),
                symbol_coeffs: vec![],
                constant: 0,
            });
            coeffs[i] = -1;
            constraints.push(AffineConstraint {
                coeffs,
                symbol_coeffs: vec![],
                constant: dim as i64 - 1,
            });
        }

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        let schedule = AffineMap {
            matrix: (0..rank)
                .map(|i| {
                    let mut row = vec![0; rank];
                    row[i] = 1;
                    row
                })
                .collect(),
            symbol_matrix: vec![],
            constant: vec![0; rank],
        };

        Schedule {
            domain,
            schedule,
            transforms: vec![ScheduleTransform::Parallelize { loop_depth: 0 }],
        }
    }

    /// Lower transpose
    fn lower_transpose(&self, shape: &ConcreteShape, perm: &[usize]) -> Schedule {
        let rank = shape.rank();
        let mut constraints = Vec::new();

        for i in 0..rank {
            let dim = shape.dims()[perm[i]];
            let mut coeffs = vec![0; rank];
            coeffs[i] = 1;
            constraints.push(AffineConstraint {
                coeffs: coeffs.clone(),
                symbol_coeffs: vec![],
                constant: 0,
            });
            coeffs[i] = -1;
            constraints.push(AffineConstraint {
                coeffs,
                symbol_coeffs: vec![],
                constant: dim as i64 - 1,
            });
        }

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        let schedule = AffineMap {
            matrix: (0..rank)
                .map(|i| {
                    let mut row = vec![0; rank];
                    row[perm[i]] = 1;
                    row
                })
                .collect(),
            symbol_matrix: vec![],
            constant: vec![0; rank],
        };

        Schedule {
            domain,
            schedule,
            transforms: vec![],
        }
    }

    /// Lower broadcast
    fn lower_broadcast(&self, _shape: &ConcreteShape, target_shape: &ConcreteShape) -> Schedule {
        let rank = target_shape.rank();
        let mut constraints = Vec::new();

        for i in 0..rank {
            let dim = target_shape.dims()[i];
            let mut coeffs = vec![0; rank];
            coeffs[i] = 1;
            constraints.push(AffineConstraint {
                coeffs: coeffs.clone(),
                symbol_coeffs: vec![],
                constant: 0,
            });
            coeffs[i] = -1;
            constraints.push(AffineConstraint {
                coeffs,
                symbol_coeffs: vec![],
                constant: dim as i64 - 1,
            });
        }

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        let schedule = AffineMap {
            matrix: (0..rank)
                .map(|i| {
                    let mut row = vec![0; rank];
                    row[i] = 1;
                    row
                })
                .collect(),
            symbol_matrix: vec![],
            constant: vec![0; rank],
        };

        Schedule {
            domain,
            schedule,
            transforms: vec![ScheduleTransform::Parallelize { loop_depth: 0 }],
        }
    }

    /// Lower reduction
    fn lower_reduction(
        &self,
        _op: ReductionOp,
        shape: &ConcreteShape,
        dim: Option<usize>,
    ) -> Schedule {
        let rank = shape.rank();
        let mut constraints = Vec::new();

        if let Some(reduce_dim) = dim {
            // Output domain (excluding reduced dimension)
            let out_rank = rank - 1;
            for i in 0..out_rank {
                let src_i = if i < reduce_dim { i } else { i + 1 };
                let dim = shape.dims()[src_i];
                let mut coeffs = vec![0; rank];
                coeffs[src_i] = 1;
                constraints.push(AffineConstraint {
                    coeffs: coeffs.clone(),
                    symbol_coeffs: vec![],
                    constant: 0,
                });
                coeffs[src_i] = -1;
                constraints.push(AffineConstraint {
                    coeffs,
                    symbol_coeffs: vec![],
                    constant: dim as i64 - 1,
                });
            }

            // Reduction dimension
            let reduce_size = shape.dims()[reduce_dim];
            let mut k_coeffs = vec![0; rank];
            k_coeffs[reduce_dim] = 1;
            constraints.push(AffineConstraint {
                coeffs: k_coeffs.clone(),
                symbol_coeffs: vec![],
                constant: 0,
            });
            k_coeffs[reduce_dim] = -1;
            constraints.push(AffineConstraint {
                coeffs: k_coeffs,
                symbol_coeffs: vec![],
                constant: reduce_size as i64 - 1,
            });
        } else {
            // Full reduction
            for i in 0..rank {
                let dim = shape.dims()[i];
                let mut coeffs = vec![0; rank];
                coeffs[i] = 1;
                constraints.push(AffineConstraint {
                    coeffs: coeffs.clone(),
                    symbol_coeffs: vec![],
                    constant: 0,
                });
                coeffs[i] = -1;
                constraints.push(AffineConstraint {
                    coeffs,
                    symbol_coeffs: vec![],
                    constant: dim as i64 - 1,
                });
            }
        }

        let domain = AffineDomain {
            constraints,
            symbols: vec![],
        };

        let schedule = AffineMap {
            matrix: (0..rank)
                .map(|i| {
                    let mut row = vec![0; rank];
                    row[i] = 1;
                    row
                })
                .collect(),
            symbol_matrix: vec![],
            constant: vec![0; rank],
        };

        Schedule {
            domain,
            schedule,
            transforms: vec![ScheduleTransform::Parallelize { loop_depth: 0 }],
        }
    }
}

/// Schedule identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScheduleId(pub usize);

/// Lowering to Naso's Polyhedral IR
pub mod pir_lowering {
    use super::*;

    /// Lower a schedule to Polyhedral IR
    pub fn lower_to_pir(schedule: &Schedule) -> String {
        let mut pir = String::new();
        pir.push_str("// Polyhedral IR\n");
        pir.push_str("domain {\n");

        for constraint in &schedule.domain.constraints {
            pir.push_str(&format!("  {};\n", format_constraint(constraint)));
        }

        pir.push_str("}\n\n");
        pir.push_str("schedule {\n");
        pir.push_str(&format_schedule(&schedule.schedule));
        pir.push_str("}\n\n");

        pir.push_str("transforms {\n");
        for transform in &schedule.transforms {
            pir.push_str(&format!("  {};\n", format_transform(transform)));
        }
        pir.push_str("}\n");

        pir
    }

    fn format_constraint(c: &AffineConstraint) -> String {
        let mut parts = Vec::new();
        for (i, &coeff) in c.coeffs.iter().enumerate() {
            if coeff != 0 {
                parts.push(format!("{}*x{}", coeff, i));
            }
        }
        for (i, &coeff) in c.symbol_coeffs.iter().enumerate() {
            if coeff != 0 {
                parts.push(format!("{}*s{}", coeff, i));
            }
        }
        if c.constant != 0 {
            parts.push(c.constant.to_string());
        }
        format!("{} >= 0", parts.join(" + "))
    }

    fn format_schedule(map: &AffineMap) -> String {
        let mut result = String::new();
        for (i, row) in map.matrix.iter().enumerate() {
            let mut parts = Vec::new();
            for (j, &coeff) in row.iter().enumerate() {
                if coeff != 0 {
                    parts.push(format!("{}*x{}", coeff, j));
                }
            }
            for (j, &coeff) in map
                .symbol_matrix
                .get(i)
                .unwrap_or(&vec![])
                .iter()
                .enumerate()
            {
                if coeff != 0 {
                    parts.push(format!("{}*s{}", coeff, j));
                }
            }
            if map.constant.get(i).copied().unwrap_or(0) != 0 {
                parts.push(map.constant[i].to_string());
            }
            result.push_str(&format!("  t{} = {};\n", i, parts.join(" + ")));
        }
        result
    }

    fn format_transform(t: &ScheduleTransform) -> String {
        match t {
            ScheduleTransform::Tile {
                loop_depth,
                tile_sizes,
            } => {
                format!("tile({}, {:?})", loop_depth, tile_sizes)
            }
            ScheduleTransform::Fuse {
                loop_depth1,
                loop_depth2,
            } => {
                format!("fuse({}, {})", loop_depth1, loop_depth2)
            }
            ScheduleTransform::Vectorize {
                loop_depth,
                vector_width,
            } => {
                format!("vectorize({}, {})", loop_depth, vector_width)
            }
            ScheduleTransform::Parallelize { loop_depth } => {
                format!("parallelize({})", loop_depth)
            }
            ScheduleTransform::Interchange {
                loop_depth1,
                loop_depth2,
            } => {
                format!("interchange({}, {})", loop_depth1, loop_depth2)
            }
            ScheduleTransform::Unroll { loop_depth, factor } => {
                format!("unroll({}, {})", loop_depth, factor)
            }
            ScheduleTransform::Shift { loop_depth, offset } => {
                format!("shift({}, {})", loop_depth, offset)
            }
            ScheduleTransform::Skew { loop_depth, factor } => {
                format!("skew({}, {})", loop_depth, factor)
            }
        }
    }
}

/// Auto-tuning hints for polyhedral optimization
pub mod autotune {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct TuningConfig {
        pub tile_sizes: Vec<Vec<usize>>,
        pub vector_widths: Vec<usize>,
        pub unroll_factors: Vec<usize>,
    }

    impl Default for TuningConfig {
        fn default() -> Self {
            Self {
                tile_sizes: vec![vec![32, 32, 32], vec![64, 64, 64], vec![16, 16, 16]],
                vector_widths: vec![2, 4, 8, 16],
                unroll_factors: vec![2, 4, 8],
            }
        }
    }

    /// Generate tuning candidates for a schedule
    pub fn generate_candidates(schedule: &Schedule, config: &TuningConfig) -> Vec<Schedule> {
        let mut candidates = Vec::new();

        for tile_sizes in &config.tile_sizes {
            for &vector_width in &config.vector_widths {
                for &unroll_factor in &config.unroll_factors {
                    let mut candidate = schedule.clone();
                    candidate.transforms = vec![
                        ScheduleTransform::Tile {
                            loop_depth: 0,
                            tile_sizes: tile_sizes.clone(),
                        },
                        ScheduleTransform::Vectorize {
                            loop_depth: schedule.domain.constraints.len().saturating_sub(1),
                            vector_width,
                        },
                        ScheduleTransform::Unroll {
                            loop_depth: 0,
                            factor: unroll_factor,
                        },
                    ];
                    candidates.push(candidate);
                }
            }
        }

        candidates
    }
}

/// Integration with Naso compiler's IR
pub mod naso_ir {
    use super::*;

    /// Convert tensor operation to Naso IR nodes
    pub fn tensor_op_to_naso_ir(op: &TensorOp) -> Vec<NasoIrNode> {
        match op {
            TensorOp::MatMul {
                lhs_shape,
                rhs_shape,
                out_shape,
            } => {
                vec![NasoIrNode::MatMul {
                    lhs_shape: lhs_shape.clone(),
                    rhs_shape: rhs_shape.clone(),
                    out_shape: out_shape.clone(),
                }]
            }
            TensorOp::ElementWise { op, shape } => {
                vec![NasoIrNode::ElementWise {
                    op: *op,
                    shape: shape.clone(),
                }]
            }
            TensorOp::Contraction {
                lhs_shape,
                rhs_shape,
                lhs_dim,
                rhs_dim,
                out_shape,
            } => {
                vec![NasoIrNode::Contraction {
                    lhs_shape: lhs_shape.clone(),
                    rhs_shape: rhs_shape.clone(),
                    lhs_dim: *lhs_dim,
                    rhs_dim: *rhs_dim,
                    out_shape: out_shape.clone(),
                }]
            }
            _ => vec![],
        }
    }

    #[derive(Debug, Clone)]
    pub enum NasoIrNode {
        MatMul {
            lhs_shape: ConcreteShape,
            rhs_shape: ConcreteShape,
            out_shape: ConcreteShape,
        },
        ElementWise {
            op: ElementWiseOp,
            shape: ConcreteShape,
        },
        Contraction {
            lhs_shape: ConcreteShape,
            rhs_shape: ConcreteShape,
            lhs_dim: usize,
            rhs_dim: usize,
            out_shape: ConcreteShape,
        },
    }
}
