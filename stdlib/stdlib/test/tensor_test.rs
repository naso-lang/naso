//! Tensor unit tests verifying operations and QTT linearity invariants

use std::marker::PhantomData;
use naso_std::prelude::*;
use naso_std::std::tensor::*;

#[test]
fn test_q0_proof_tensor() {
    let shape = ConcreteShape::scalar();
    let tensor: Tensor<Q0, (), f32, RowMajor> = Tensor::proof(shape.clone());
    assert_eq!(tensor.rank(), 0);
    assert_eq!(tensor.num_elements(), 0);
    assert!(tensor.is_empty());
}

#[test]
fn test_q1_linear_tensor() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::vector(4);
    let tensor: Tensor<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor> = Tensor::from_vec(data.clone(), shape.clone());
    
    assert_eq!(tensor.rank(), 1);
    assert_eq!(tensor.num_elements(), 4);
    assert_eq!(tensor.as_slice(), &data[..]);
}

#[test]
fn test_q1_linear_tensor_mut() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::vector(4);
    let mut tensor: Tensor<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    tensor[0] = 10.0;
    tensor[1] = 20.0;
    assert_eq!(tensor[0], 10.0);
    assert_eq!(tensor[1], 20.0);
}

#[test]
fn test_qstar_heap_tensor() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::vector(4);
    let tensor: Tensor<QStar, DimCons<DimConst<4>, ()>, f32, RowMajor> = Tensor::from_vec(data.clone(), shape.clone());
    
    assert_eq!(tensor.rank(), 1);
    assert_eq!(tensor.num_elements(), 4);
    assert_eq!(tensor.as_slice(), &data[..]);
}

#[test]
fn test_qstar_shared_ownership() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::vector(4);
    let tensor1: Tensor<QStar, DimCons<DimConst<4>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    let tensor2 = tensor1.clone();
    
    assert_eq!(tensor1.as_slice(), tensor2.as_slice());
    assert!(std::sync::Arc::strong_count(&tensor1.data) >= 2);
}

#[test]
fn test_q1_to_qstar_conversion() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::vector(4);
    let linear: Tensor<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let heap: Tensor<QStar, DimCons<DimConst<4>, ()>, f32, RowMajor> = linear.into();
    assert_eq!(heap.as_slice(), &[1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn test_matrix_creation() {
    let data: Vec<f32> = (0..6).map(|x| x as f32).collect();
    let shape = ConcreteShape::matrix(2, 3);
    let tensor: Tensor<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    assert_eq!(tensor.rank(), 2);
    assert_eq!(tensor.shape().dims(), &[2, 3]);
    assert_eq!(tensor.num_elements(), 6);
}

#[test]
fn test_matrix_transpose() {
    let data: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let shape = ConcreteShape::matrix(2, 3);
    let tensor: Tensor<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let transposed = tensor.transpose();
    assert_eq!(transposed.shape().dims(), &[3, 2]);
}

#[test]
fn test_elementwise_add() {
    let a = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0],
        ConcreteShape::vector(4)
    );
    let b = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![10.0, 20.0, 30.0, 40.0],
        ConcreteShape::vector(4)
    );
    
    let result = add(&a, &b);
    assert_eq!(result.as_slice(), &[11.0, 22.0, 33.0, 44.0]);
}

#[test]
fn test_elementwise_sub() {
    let a = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![10.0, 20.0, 30.0, 40.0],
        ConcreteShape::vector(4)
    );
    let b = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0],
        ConcreteShape::vector(4)
    );
    
    let result = sub(&a, &b);
    assert_eq!(result.as_slice(), &[9.0, 18.0, 27.0, 36.0]);
}

#[test]
fn test_elementwise_mul() {
    let a = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0],
        ConcreteShape::vector(4)
    );
    let b = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![2.0, 3.0, 4.0, 5.0],
        ConcreteShape::vector(4)
    );
    
    let result = mul(&a, &b);
    assert_eq!(result.as_slice(), &[2.0, 6.0, 12.0, 20.0]);
}

#[test]
fn test_elementwise_div() {
    let a = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![10.0, 20.0, 30.0, 40.0],
        ConcreteShape::vector(4)
    );
    let b = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![2.0, 4.0, 5.0, 8.0],
        ConcreteShape::vector(4)
    );
    
    let result = div(&a, &b);
    assert_eq!(result.as_slice(), &[5.0, 5.0, 6.0, 5.0]);
}

#[test]
fn test_scalar_operations() {
    let a = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0],
        ConcreteShape::vector(4)
    );
    
    let result_add = add_scalar(&a, 10.0);
    assert_eq!(result_add.as_slice(), &[11.0, 12.0, 13.0, 14.0]);
    
    let result_mul = mul_scalar(&a, 2.0);
    assert_eq!(result_mul.as_slice(), &[2.0, 4.0, 6.0, 8.0]);
}

#[test]
fn test_matmul() {
    let a = Tensor::<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        ConcreteShape::matrix(2, 3)
    );
    let b = Tensor::<Q1, DimCons<DimConst<3>, DimCons<DimConst<2>, ()>>, f32, RowMajor>::from_vec(
        vec![7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
        ConcreteShape::matrix(3, 2)
    );
    
    let result = matmul(&a, &b);
    assert_eq!(result.shape().dims(), &[2, 2]);
    
    // [1 2 3] * [7 8] = [58 64]
    // [4 5 6]   [9 10]  [139 154]
    //             [11 12]
    assert_eq!(result[0], 58.0);
    assert_eq!(result[1], 64.0);
    assert_eq!(result[2], 139.0);
    assert_eq!(result[3], 154.0);
}

#[test]
fn test_dot_product() {
    let a = Tensor::<Q1, DimCons<DimConst<3>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0],
        ConcreteShape::vector(3)
    );
    let b = Tensor::<Q1, DimCons<DimConst<3>, ()>, f32, RowMajor>::from_vec(
        vec![4.0, 5.0, 6.0],
        ConcreteShape::vector(3)
    );
    
    let result = dot(&a, &b);
    assert_eq!(result, 32.0); // 1*4 + 2*5 + 3*6 = 4 + 10 + 18 = 32
}

#[test]
fn test_matvec() {
    let mat = Tensor::<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        ConcreteShape::matrix(2, 3)
    );
    let vec = Tensor::<Q1, DimCons<DimConst<3>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0],
        ConcreteShape::vector(3)
    );
    
    let result = matvec(&mat, &vec);
    assert_eq!(result.shape().dims(), &[2]);
    // [1 2 3] * [1] = [14]
    // [4 5 6]   [2]   [32]
    //           [3]
    assert_eq!(result[0], 14.0);
    assert_eq!(result[1], 32.0);
}

#[test]
fn test_outer_product() {
    let a = Tensor::<Q1, DimCons<DimConst<2>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0],
        ConcreteShape::vector(2)
    );
    let b = Tensor::<Q1, DimCons<DimConst<3>, ()>, f32, RowMajor>::from_vec(
        vec![3.0, 4.0, 5.0],
        ConcreteShape::vector(3)
    );
    
    let result = outer(&a, &b);
    assert_eq!(result.shape().dims(), &[2, 3]);
    // [1*3, 1*4, 1*5] = [3, 4, 5]
    // [2*3, 2*4, 2*5]   [6, 8, 10]
    assert_eq!(result[0], 3.0);
    assert_eq!(result[1], 4.0);
    assert_eq!(result[2], 5.0);
    assert_eq!(result[3], 6.0);
    assert_eq!(result[4], 8.0);
    assert_eq!(result[5], 10.0);
}

#[test]
fn test_contraction() {
    let a = Tensor::<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        ConcreteShape::matrix(2, 3)
    );
    let b = Tensor::<Q1, DimCons<DimConst<3>, DimCons<DimConst<2>, ()>>, f32, RowMajor>::from_vec(
        vec![7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
        ConcreteShape::matrix(3, 2)
    );
    
    let result = contract(&a, &b, 1, 0);
    assert_eq!(result.shape().dims(), &[2, 2]);
    // Same as matmul
    assert_eq!(result[0], 58.0);
    assert_eq!(result[1], 64.0);
    assert_eq!(result[2], 139.0);
    assert_eq!(result[3], 154.0);
}

#[test]
fn test_reshape() {
    let data: Vec<f32> = (0..12).map(|x| x as f32).collect();
    let shape = ConcreteShape::new(vec![3, 4]);
    let tensor: Tensor<Q1, DimCons<DimConst<3>, DimCons<DimConst<4>, ()>>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let reshaped = tensor.reshape(ConcreteShape::new(vec![2, 6]));
    assert_eq!(reshaped.shape().dims(), &[2, 6]);
    assert_eq!(reshaped.num_elements(), 12);
}

#[test]
fn test_squeeze_unsqueeze() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::new(vec![1, 4, 1]);
    let tensor: Tensor<Q1, DimCons<DimConst<1>, DimCons<DimConst<4>, DimCons<DimConst<1>, ()>>>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let squeezed = squeeze(tensor, &[0, 2]);
    assert_eq!(squeezed.shape().dims(), &[4]);
    
    let unsqueezed = unsqueeze(squeezed, 0);
    assert_eq!(unsqueezed.shape().dims(), &[1, 4]);
}

#[test]
fn test_reduction_sum() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let shape = ConcreteShape::matrix(2, 3);
    let tensor: Tensor<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let sum_all = sum(&tensor, None);
    assert_eq!(sum_all.num_elements(), 1);
    assert_eq!(sum_all[0], 21.0);
    
    let sum_dim0 = sum(&tensor, Some(0));
    assert_eq!(sum_dim0.shape().dims(), &[3]);
    assert_eq!(sum_dim0.as_slice(), &[5.0, 7.0, 9.0]); // [1+4, 2+5, 3+6]
    
    let sum_dim1 = sum(&tensor, Some(1));
    assert_eq!(sum_dim1.shape().dims(), &[2]);
    assert_eq!(sum_dim1.as_slice(), &[6.0, 15.0]); // [1+2+3, 4+5+6]
}

#[test]
fn test_relu() {
    let data = vec![-2.0f32, -1.0, 0.0, 1.0, 2.0];
    let shape = ConcreteShape::vector(5);
    let tensor: Tensor<Q1, DimCons<DimConst<5>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let result = relu(&tensor);
    assert_eq!(result.as_slice(), &[0.0, 0.0, 0.0, 1.0, 2.0]);
}

#[test]
fn test_maximum_minimum() {
    let a = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 5.0, 3.0, 7.0],
        ConcreteShape::vector(4)
    );
    let b = Tensor::<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor>::from_vec(
        vec![4.0, 2.0, 6.0, 3.0],
        ConcreteShape::vector(4)
    );
    
    let max_result = maximum(&a, &b);
    assert_eq!(max_result.as_slice(), &[4.0, 5.0, 6.0, 7.0]);
    
    let min_result = minimum(&a, &b);
    assert_eq!(min_result.as_slice(), &[1.0, 2.0, 3.0, 3.0]);
}

#[test]
fn test_broadcast() {
    let a = Tensor::<Q1, DimCons<DimConst<3>, ()>, f32, RowMajor>::from_vec(
        vec![1.0, 2.0, 3.0],
        ConcreteShape::vector(3)
    );
    
    let target = ConcreteShape::new(vec![2, 3]);
    let result = broadcast(&a, &target);
    assert_eq!(result.shape().dims(), &[2, 3]);
}

#[test]
fn test_qtt_linearity_q1_unique_ownership() {
    // Q1 tensors enforce unique ownership - cannot be cloned
    let data = vec![1.0f32, 2.0, 3.0];
    let shape = ConcreteShape::vector(3);
    let tensor: Tensor<Q1, DimCons<DimConst<3>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    // Move semantics - ownership transferred
    let moved = tensor;
    // tensor is now moved, cannot be used
    
    // This would fail to compile:
    // let _ = tensor[0]; // error: use of moved value
    
    // But moved works fine
    assert_eq!(moved[0], 1.0);
}

#[test]
fn test_qtt_linearity_qstar_shared_ownership() {
    // QStar tensors allow shared ownership
    let data = vec![1.0f32, 2.0, 3.0];
    let shape = ConcreteShape::vector(3);
    let tensor: Tensor<QStar, DimCons<DimConst<3>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let cloned = tensor.clone();
    let cloned2 = tensor.clone();
    
    // All three can be used
    assert_eq!(tensor[0], 1.0);
    assert_eq!(cloned[0], 1.0);
    assert_eq!(cloned2[0], 1.0);
}

#[test]
fn test_tensor_view() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let shape = ConcreteShape::matrix(2, 3);
    let tensor: Tensor<Q1, DimCons<DimConst<2>, DimCons<DimConst<3>, ()>>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    let view = TensorView::new(tensor.as_slice(), tensor.shape().clone());
    assert_eq!(view.shape().dims(), &[2, 3]);
    assert_eq!(view.as_slice(), &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
}

#[test]
fn test_tensor_view_mut() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::vector(4);
    let mut tensor: Tensor<Q1, DimCons<DimConst<4>, ()>, f32, RowMajor> = Tensor::from_vec(data, shape);
    
    {
        let mut view = TensorViewMut::new(tensor.as_mut_slice(), tensor.shape().clone());
        view[0] = 10.0;
        view[1] = 20.0;
    }
    
    assert_eq!(tensor[0], 10.0);
    assert_eq!(tensor[1], 20.0);
}

#[test]
fn test_shape_operations() {
    let shape = ConcreteShape::new(vec![2, 3, 4]);
    assert_eq!(shape.rank(), 3);
    assert_eq!(shape.num_elements(), 24);
    assert_eq!(shape.dims(), &[2, 3, 4]);
    assert_eq!(shape.strides(), &[12, 4, 1]);
    assert!(shape.is_contiguous());
}

#[test]
fn test_broadcastable() {
    let shape1 = ConcreteShape::new(vec![3, 1, 5]);
    let shape2 = ConcreteShape::new(vec![1, 4, 5]);
    
    assert!(shape1.can_broadcast(&shape2));
    let broadcasted = shape1.broadcast_shape(&shape2).unwrap();
    assert_eq!(broadcasted.dims(), &[3, 4, 5]);
}

#[test]
fn test_broadcastable_incompatible() {
    let shape1 = ConcreteShape::new(vec![3, 2, 5]);
    let shape2 = ConcreteShape::new(vec![3, 4, 5]);
    
    assert!(!shape1.can_broadcast(&shape2));
    assert!(shape1.broadcast_shape(&shape2).is_none());
}

#[test]
fn test_polyhedral_lowering_matmul() {
    let mut ctx = LoweringContext::new();
    let op = TensorOp::MatMul {
        lhs_shape: ConcreteShape::matrix(32, 32),
        rhs_shape: ConcreteShape::matrix(32, 32),
        out_shape: ConcreteShape::matrix(32, 32),
    };
    
    let schedule_id = ctx.create_schedule(&op);
    let schedule = ctx.get_schedule(schedule_id).unwrap();
    
    assert_eq!(schedule.domain.constraints.len(), 6);
    assert!(!schedule.transforms.is_empty());
}

#[test]
fn test_polyhedral_lowering_elementwise() {
    let mut ctx = LoweringContext::new();
    let op = TensorOp::ElementWise {
        op: ElementWiseOp::Add,
        shape: ConcreteShape::vector(1024),
    };
    
    let schedule_id = ctx.create_schedule(&op);
    let schedule = ctx.get_schedule(schedule_id).unwrap();
    
    assert_eq!(schedule.domain.constraints.len(), 2); // 1 dim * 2 constraints
    assert!(schedule.transforms.iter().any(|t| matches!(t, ScheduleTransform::Parallelize { .. })));
}

#[test]
fn test_polyhedral_lowering_contraction() {
    let mut ctx = LoweringContext::new();
    let op = TensorOp::Contraction {
        lhs_shape: ConcreteShape::matrix(16, 16),
        rhs_shape: ConcreteShape::matrix(16, 16),
        lhs_dim: 1,
        rhs_dim: 0,
        out_shape: ConcreteShape::matrix(16, 16),
    };
    
    let schedule_id = ctx.create_schedule(&op);
    let schedule = ctx.get_schedule(schedule_id).unwrap();
    
    // 2 output dims + 1 contraction dim = 3 dims, 2 constraints each = 6 + 2 = 8
    assert_eq!(schedule.domain.constraints.len(), 8);
}

#[test]
fn test_schedule_transforms() {
    let mut ctx = LoweringContext::new();
    let op = TensorOp::MatMul {
        lhs_shape: ConcreteShape::matrix(64, 64),
        rhs_shape: ConcreteShape::matrix(64, 64),
        out_shape: ConcreteShape::matrix(64, 64),
    };
    
    let schedule_id = ctx.create_schedule(&op);
    
    ctx.apply_transforms(schedule_id, vec![
        ScheduleTransform::Tile {
            loop_depth: 0,
            tile_sizes: vec![16, 16, 16],
        },
        ScheduleTransform::Vectorize {
            loop_depth: 2,
            vector_width: 8,
        },
        ScheduleTransform::Parallelize {
            loop_depth: 0,
        },
    ]);
    
    let schedule = ctx.get_schedule(schedule_id).unwrap();
    assert_eq!(schedule.transforms.len(), 5); // 2 default + 3 applied
}

#[test]
fn test_pir_lowering() {
    let mut ctx = LoweringContext::new();
    let op = TensorOp::ElementWise {
        op: ElementWiseOp::Mul,
        shape: ConcreteShape::vector(4),
    };
    
    let schedule_id = ctx.create_schedule(&op);
    let schedule = ctx.get_schedule(schedule_id).unwrap();
    
    let pir = pir_lowering::lower_to_pir(schedule);
    assert!(pir.contains("domain"));
    assert!(pir.contains("schedule"));
    assert!(pir.contains("transforms"));
    assert!(pir.contains("parallelize"));
}

#[test]
fn test_autotune_candidates() {
    let mut ctx = LoweringContext::new();
    let op = TensorOp::MatMul {
        lhs_shape: ConcreteShape::matrix(32, 32),
        rhs_shape: ConcreteShape::matrix(32, 32),
        out_shape: ConcreteShape::matrix(32, 32),
    };
    
    let schedule_id = ctx.create_schedule(&op);
    let schedule = ctx.get_schedule(schedule_id).unwrap();
    
    let config = autotune::TuningConfig::default();
    let candidates = autotune::generate_candidates(schedule, &config);
    
    assert!(!candidates.is_empty());
    assert_eq!(candidates.len(), 3 * 4 * 3); // tile_sizes * vector_widths * unroll_factors
}

#[test]
fn test_naso_ir_conversion() {
    let op = TensorOp::MatMul {
        lhs_shape: ConcreteShape::matrix(16, 16),
        rhs_shape: ConcreteShape::matrix(16, 16),
        out_shape: ConcreteShape::matrix(16, 16),
    };
    
    let ir_nodes = naso_ir::tensor_op_to_naso_ir(&op);
    assert_eq!(ir_nodes.len(), 1);
    
    match &ir_nodes[0] {
        naso_ir::NasoIrNode::MatMul { lhs_shape, rhs_shape, out_shape } => {
            assert_eq!(lhs_shape.dims(), &[16, 16]);
            assert_eq!(rhs_shape.dims(), &[16, 16]);
            assert_eq!(out_shape.dims(), &[16, 16]);
        }
        _ => panic!("Expected MatMul IR node"),
    }
}

#[test]
fn test_col_major_layout() {
    let data = vec![1.0f32, 2.0, 3.0, 4.0];
    let shape = ConcreteShape::matrix(2, 2);
    let tensor: Tensor<Q1, DimCons<DimConst<2>, DimCons<DimConst<2>, ()>>, f32, ColMajor> = Tensor::from_vec(data, shape);
    
    assert_eq!(tensor.layout(), ColMajor);
    // In col-major: [0,0]=0, [1,0]=1, [0,1]=2, [1,1]=3
    assert_eq!(tensor[0], 1.0); // [0,0]
    assert_eq!(tensor[1], 3.0); // [0,1]
    assert_eq!(tensor[2], 2.0); // [1,0]
    assert_eq!(tensor[3], 4.0); // [1,1]
}

#[test]
fn test_rank_types() {
    // Verify rank type definitions
    let _r0: Rank0 = ();
    let _r1: Rank1 = Succ(PhantomData);
    let _r2: Rank2 = Succ(PhantomData);
    
    assert_eq!(Rank0::VALUE, 0);
    assert_eq!(Rank1::VALUE, 1);
    assert_eq!(Rank2::VALUE, 2);
}

#[test]
fn test_dimension_types() {
    // Verify dimension type definitions
    assert_eq!(Dim1::VALUE, 1);
    assert_eq!(Dim2::VALUE, 2);
    assert_eq!(Dim4::VALUE, 4);
    assert_eq!(Dim8::VALUE, 8);
    assert_eq!(Dim16::VALUE, 16);
    assert_eq!(Dim32::VALUE, 32);
    assert_eq!(Dim64::VALUE, 64);
    assert_eq!(Dim128::VALUE, 128);
    assert_eq!(Dim256::VALUE, 256);
    assert_eq!(Dim512::VALUE, 512);
    assert_eq!(Dim1024::VALUE, 1024);
}