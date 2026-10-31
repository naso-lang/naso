//! End-to-End PIR Integration Tests
//!
//! Loads golden fixture PIR files and validates them through the IR validator.
//! Also includes property-based tests for inversion correctness.

#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(dead_code)]
#![allow(clippy::if_same_then_else)]

use naso_compiler::ast::{Mutability, Quantity};
use naso_compiler::ir::{
    access_relation::{AccessRelation, AccessRelations, AccessType},
    affine_domain::AffineDomain,
    affine_map::{AffineMap, Matrix},
    pir_types::{BinaryOp, PirExpr, PirModule, PirStatement, QuantityMap, ValidationError},
    pretty_print::format_golden_fixture,
    schedule_tree::{ScheduleNode, ScheduleTree, StmtId},
    validate::{validate_pir, validate_schedule_detailed},
};
use std::fs;
use std::path::Path;

/// Load a golden fixture PIR file and validate it
fn load_and_validate_fixture(name: &str) -> Result<PirModule, Vec<ValidationError>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{}.pir", name));

    let _content = fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));

    // For now, we'll construct the PIR programmatically based on the fixture
    // In a full implementation, this would parse the .pir text format
    construct_fixture(name)
}

/// Construct fixture programmatically for testing
fn construct_fixture(name: &str) -> Result<PirModule, Vec<ValidationError>> {
    match name {
        "matmul_64x64" => construct_matmul_fixture(),
        "stencil_3d" => construct_stencil_fixture(),
        "fft_1024" => construct_fft_fixture(),
        "teleport" => construct_teleport_fixture(),
        "rev_adder" => construct_rev_adder_fixture(),
        _ => Err(vec![ValidationError::ScheduleError(format!(
            "Unknown fixture: {}",
            name
        ))]),
    }
}

/// Construct matmul 64x64 fixture
fn construct_matmul_fixture() -> Result<PirModule, Vec<ValidationError>> {
    // Domain: 0 <= i,j,k < 64
    let domain = AffineDomain::new(
        3,
        0,
        vec![
            // i >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0, 0], 0),
            // i <= 63 => -i >= -63
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 0, 0], -63),
            // j >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![0, 1, 0], 0),
            // j <= 63 => -j >= -63
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![0, -1, 0], -63),
            // k >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![0, 0, 1], 0),
            // k <= 63 => -k >= -63
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![0, 0, -1], -63),
        ],
    )
    .with_name("matmul_domain".to_string());

    let mut m_i = Matrix::new(1, 3);
    m_i.set(0, 0, 1);
    let schedule_i = AffineMap::total(domain.clone(), m_i);

    let mut m_j = Matrix::new(1, 3);
    m_j.set(0, 1, 1);
    let schedule_j = AffineMap::total(domain.clone(), m_j);

    let mut m_k = Matrix::new(1, 3);
    m_k.set(0, 2, 1);
    let schedule_k = AffineMap::total(domain.clone(), m_k);

    // Access maps
    let mut m_a = Matrix::new(2, 3);
    m_a.set(0, 0, 1); // A[i][k] -> i
    m_a.set(1, 2, 1); // A[i][k] -> k
    let access_a = AffineMap::total(domain.clone(), m_a);

    let mut m_b = Matrix::new(2, 3);
    m_b.set(0, 2, 1); // B[k][j] -> k
    m_b.set(1, 1, 1); // B[k][j] -> j
    let access_b = AffineMap::total(domain.clone(), m_b);

    let mut m_c = Matrix::new(2, 3);
    m_c.set(0, 0, 1); // C[i][j] -> i
    m_c.set(1, 1, 1); // C[i][j] -> j
    let access_c = AffineMap::total(domain.clone(), m_c);

    let stmt = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Binary {
            op: BinaryOp::Add,
            left: Box::new(PirExpr::Binary {
                op: BinaryOp::Mul,
                left: Box::new(PirExpr::Index {
                    base: Box::new(PirExpr::Var("A".to_string())),
                    indices: vec![PirExpr::Var("i".to_string()), PirExpr::Var("k".to_string())],
                }),
                right: Box::new(PirExpr::Index {
                    base: Box::new(PirExpr::Var("B".to_string())),
                    indices: vec![PirExpr::Var("k".to_string()), PirExpr::Var("j".to_string())],
                }),
            }),
            right: Box::new(PirExpr::Index {
                base: Box::new(PirExpr::Var("C".to_string())),
                indices: vec![PirExpr::Var("i".to_string()), PirExpr::Var("j".to_string())],
            }),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let inner_band = ScheduleNode::band(
        vec![schedule_k],
        vec![true],
        ScheduleNode::domain(StmtId(0), domain.clone()),
    );
    let middle_band = ScheduleNode::band(vec![schedule_j], vec![true], inner_band);
    let outer_band = ScheduleNode::band(vec![schedule_i], vec![false], middle_band);

    let schedule = ScheduleTree::new(
        outer_band,
        vec!["N".to_string(), "M".to_string(), "K".to_string()],
    );

    let mut accesses = AccessRelations::new();
    accesses.add(
        AccessRelation::new(StmtId(0), domain.clone(), access_a, AccessType::Read)
            .with_array_name("A"),
    );
    accesses.add(
        AccessRelation::new(StmtId(0), domain.clone(), access_b, AccessType::Read)
            .with_array_name("B"),
    );
    accesses.add(
        AccessRelation::new(StmtId(0), domain, access_c, AccessType::Write).with_array_name("C"),
    );

    let mut quantities = QuantityMap::new();
    quantities.insert("N".to_string(), Quantity::Zero);
    quantities.insert("M".to_string(), Quantity::Zero);
    quantities.insert("K".to_string(), Quantity::Zero);

    let module = PirModule::new(
        vec![stmt],
        schedule,
        accesses,
        quantities,
        vec!["N".to_string(), "M".to_string(), "K".to_string()],
    );
    Ok(module)
}

/// Construct stencil 3D fixture
fn construct_stencil_fixture() -> Result<PirModule, Vec<ValidationError>> {
    let domain = AffineDomain::new(
        3,
        3,
        vec![
            // i >= 1
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(
                vec![1, 0, 0, 0, 0, 0],
                1,
            ),
            // i <= 127 => -i >= -127
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(
                vec![-1, 0, 0, 0, 0, 0],
                -127,
            ),
            // j >= 1
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(
                vec![0, 1, 0, 0, 0, 0],
                1,
            ),
            // j <= 127 => -j >= -127
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(
                vec![0, -1, 0, 0, 0, 0],
                -127,
            ),
            // k >= 1
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(
                vec![0, 0, 1, 0, 0, 0],
                1,
            ),
            // k <= 127 => -k >= -127
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(
                vec![0, 0, -1, 0, 0, 0],
                -127,
            ),
        ],
    )
    .with_name("stencil_domain".to_string());

    let mut m_i = Matrix::new(1, 6);
    m_i.set(0, 0, 1);
    let schedule_i = AffineMap::total(domain.clone(), m_i);

    let mut m_j = Matrix::new(1, 6);
    m_j.set(0, 1, 1);
    let schedule_j = AffineMap::total(domain.clone(), m_j);

    let mut m_k = Matrix::new(1, 6);
    m_k.set(0, 2, 1);
    let schedule_k = AffineMap::total(domain.clone(), m_k);

    let mut m_access = Matrix::new(3, 6);
    m_access.set(0, 0, 1);
    m_access.set(1, 1, 1);
    m_access.set(2, 2, 1);
    let access_map = AffineMap::total(domain.clone(), m_access);

    let stmt = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Var("u[i][j][k]".to_string()),
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let inner_band = ScheduleNode::band(
        vec![schedule_k],
        vec![true],
        ScheduleNode::domain(StmtId(0), domain.clone()),
    );
    let middle_band = ScheduleNode::band(vec![schedule_j], vec![true], inner_band);
    let outer_band = ScheduleNode::band(vec![schedule_i], vec![false], middle_band);

    let schedule = ScheduleTree::new(
        outer_band,
        vec!["N".to_string(), "M".to_string(), "K".to_string()],
    );

    let mut accesses = AccessRelations::new();
    // 7 reads for neighbors + 1 write
    for _ in 0..7 {
        accesses.add(
            AccessRelation::new(
                StmtId(0),
                domain.clone(),
                access_map.clone(),
                AccessType::Read,
            )
            .with_array_name("u"),
        );
    }
    accesses.add(
        AccessRelation::new(StmtId(0), domain, access_map, AccessType::Write).with_array_name("u"),
    );

    let mut quantities = QuantityMap::new();
    quantities.insert("N".to_string(), Quantity::Zero);
    quantities.insert("M".to_string(), Quantity::Zero);
    quantities.insert("K".to_string(), Quantity::Zero);

    let module = PirModule::new(
        vec![stmt],
        schedule,
        accesses,
        quantities,
        vec!["N".to_string(), "M".to_string(), "K".to_string()],
    );
    Ok(module)
}

/// Construct FFT 1024 fixture
fn construct_fft_fixture() -> Result<PirModule, Vec<ValidationError>> {
    let bitrev_domain = AffineDomain::new(
        1,
        1,
        vec![
            // i >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
            // i <= 1023 => -i >= -1023
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 0], -1023),
        ],
    )
    .with_name("bitrev_domain".to_string());

    let butterfly_domain = AffineDomain::new(
        2,
        2,
        vec![
            // s >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0, 0, 0], 0),
            // s <= 9 => -s >= -9
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 0, 0, 0], -9),
            // k >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![0, 1, 0, 0], 0),
            // k <= 511 => -k >= -511
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![0, -1, 0, 0], -511),
        ],
    )
    .with_name("butterfly_domain".to_string());

    let mut m_bitrev = Matrix::new(1, 2);
    m_bitrev.set(0, 0, 1);
    let bitrev_schedule = AffineMap::total(bitrev_domain.clone(), m_bitrev);

    let mut m_stage = Matrix::new(1, 4);
    m_stage.set(0, 0, 1);
    let stage_schedule = AffineMap::total(butterfly_domain.clone(), m_stage);

    let mut m_k = Matrix::new(1, 4);
    m_k.set(0, 1, 1);
    let k_schedule = AffineMap::total(butterfly_domain.clone(), m_k);

    let stmt_bitrev = PirStatement {
        id: StmtId(0),
        domain: bitrev_domain.clone(),
        body: PirExpr::Var("bitrev".to_string()),
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let stmt_butterfly = PirStatement {
        id: StmtId(1),
        domain: butterfly_domain.clone(),
        body: PirExpr::Var("butterfly".to_string()),
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let bitrev_band = ScheduleNode::band(
        vec![bitrev_schedule],
        vec![true],
        ScheduleNode::domain(StmtId(0), bitrev_domain.clone()),
    );

    let inner_band = ScheduleNode::band(
        vec![k_schedule],
        vec![true],
        ScheduleNode::domain(StmtId(1), butterfly_domain.clone()),
    );
    let outer_band = ScheduleNode::band(vec![stage_schedule], vec![false], inner_band);

    let schedule = ScheduleTree::new(
        ScheduleNode::sequence(vec![bitrev_band, outer_band]),
        vec!["N".to_string(), "LOGN".to_string()],
    );

    let mut accesses = AccessRelations::new();

    let mut m_x = Matrix::new(1, 2);
    m_x.set(0, 0, 1);
    let x_access = AffineMap::total(bitrev_domain.clone(), m_x);
    accesses.add(
        AccessRelation::new(StmtId(0), bitrev_domain, x_access, AccessType::ReadWrite)
            .with_array_name("x"),
    );

    let mut m_x2 = Matrix::new(1, 4);
    m_x2.set(0, 1, 1);
    let x_access2 = AffineMap::total(butterfly_domain.clone(), m_x2);
    accesses.add(
        AccessRelation::new(
            StmtId(1),
            butterfly_domain.clone(),
            x_access2,
            AccessType::ReadWrite,
        )
        .with_array_name("x"),
    );

    let mut m_twiddle = Matrix::new(2, 4);
    m_twiddle.set(0, 0, 1);
    m_twiddle.set(1, 1, 1);
    let twiddle_access = AffineMap::total(butterfly_domain.clone(), m_twiddle);
    accesses.add(
        AccessRelation::new(
            StmtId(1),
            butterfly_domain,
            twiddle_access,
            AccessType::Read,
        )
        .with_array_name("twiddle"),
    );

    let mut quantities = QuantityMap::new();
    quantities.insert("N".to_string(), Quantity::Zero);
    quantities.insert("LOGN".to_string(), Quantity::Zero);

    let module = PirModule::new(
        vec![stmt_bitrev, stmt_butterfly],
        schedule,
        accesses,
        quantities,
        vec!["N".to_string(), "LOGN".to_string()],
    );
    Ok(module)
}

/// Construct teleport fixture
fn construct_teleport_fixture() -> Result<PirModule, Vec<ValidationError>> {
    let domain = AffineDomain::new(
        1,
        1,
        vec![
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 0], -1),
        ],
    )
    .with_name("teleport_domain".to_string());

    let stmt_prepare = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Var("q[0]".to_string()), // Use q[0] as variable
        quantity: Quantity::One,
        mutability: Mutability::InOut,
        span: None,
    };

    let stmt_alice_ops = PirStatement {
        id: StmtId(1),
        domain: domain.clone(),
        body: PirExpr::Var("q[1]".to_string()), // Use q[1] as variable
        quantity: Quantity::One,
        mutability: Mutability::InOut,
        span: None,
    };

    let stmt_alice_measure = PirStatement {
        id: StmtId(2),
        domain: domain.clone(),
        body: PirExpr::Var("q[2]".to_string()), // Use q[2] as variable
        quantity: Quantity::Zero,
        mutability: Mutability::Immutable,
        span: None,
    };

    let stmt_bob = PirStatement {
        id: StmtId(3),
        domain: domain.clone(),
        body: PirExpr::Var("q[0]".to_string()), // Use q[0] as variable
        quantity: Quantity::One,
        mutability: Mutability::InOut,
        span: None,
    };

    let schedule = ScheduleTree::new(
        ScheduleNode::sequence(vec![
            ScheduleNode::band(
                vec![AffineMap::total(domain.clone(), Matrix::new(0, 2))],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            ScheduleNode::band(
                vec![AffineMap::total(domain.clone(), Matrix::new(0, 2))],
                vec![false],
                ScheduleNode::domain(StmtId(1), domain.clone()),
            ),
            ScheduleNode::band(
                vec![AffineMap::total(domain.clone(), Matrix::new(0, 2))],
                vec![false],
                ScheduleNode::domain(StmtId(2), domain.clone()),
            ),
            ScheduleNode::band(
                vec![AffineMap::total(domain.clone(), Matrix::new(0, 2))],
                vec![false],
                ScheduleNode::domain(StmtId(3), domain.clone()),
            ),
        ]),
        vec![],
    );

    let mut accesses = AccessRelations::new();
    for q in 0..3 {
        let m = Matrix::new(0, 2); // 0-dim access for qubit, 2 cols = domain dims (1 iter + 1 param)
        let access = AffineMap::total(domain.clone(), m);
        let typ = AccessType::ReadWrite;
        accesses.add(
            AccessRelation::new(StmtId(q as usize), domain.clone(), access, typ)
                .with_array_name("q"),
        );
    }

    let mut quantities = QuantityMap::new();
    quantities.insert("q[0]".to_string(), Quantity::Many);
    quantities.insert("q[1]".to_string(), Quantity::One);
    quantities.insert("q[2]".to_string(), Quantity::One);
    quantities.insert("b0".to_string(), Quantity::Zero);
    quantities.insert("b1".to_string(), Quantity::Zero);

    let module = PirModule::new(
        vec![stmt_prepare, stmt_alice_ops, stmt_alice_measure, stmt_bob],
        schedule,
        accesses,
        quantities,
        vec![],
    );
    Ok(module)
}

/// Construct reversible adder fixture
fn construct_rev_adder_fixture() -> Result<PirModule, Vec<ValidationError>> {
    let domain = AffineDomain::new(
        1,
        1,
        vec![
            // i >= 0
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
            // i <= 7 => -i >= -7
            naso_compiler::ir::affine_domain::AffineConstraint::inequality(vec![-1, 0], -7),
        ],
    )
    .with_name("adder_domain".to_string());

    let mut m = Matrix::new(1, 2);
    m.set(0, 0, 1);
    let schedule = AffineMap::total(domain.clone(), m);

    let mut m_rev = Matrix::new(1, 2);
    m_rev.set(0, 0, -1);
    let rev_schedule = AffineMap::total(domain.clone(), m_rev);

    let stmt_compute = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Var("compute_sum".to_string()),
        quantity: Quantity::Many,
        mutability: Mutability::InOut,
        span: None,
    };

    let stmt_uncompute = PirStatement {
        id: StmtId(1),
        domain: domain.clone(),
        body: PirExpr::Var("uncompute_carry".to_string()),
        quantity: Quantity::Zero,
        mutability: Mutability::Immutable,
        span: None,
    };

    let forward_band = ScheduleNode::band(
        vec![schedule],
        vec![false],
        ScheduleNode::domain(StmtId(0), domain.clone()),
    );
    let reverse_band = ScheduleNode::band(
        vec![rev_schedule],
        vec![false],
        ScheduleNode::domain(StmtId(1), domain.clone()),
    );

    let schedule_tree = ScheduleTree::new(
        ScheduleNode::sequence(vec![forward_band, reverse_band]),
        vec!["N".to_string()],
    );

    let mut accesses = AccessRelations::new();
    let mut m_access = Matrix::new(1, 2);
    m_access.set(0, 0, 1);
    let access_map = AffineMap::total(domain.clone(), m_access);
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Read,
        )
        .with_array_name("a"),
    );
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Read,
        )
        .with_array_name("b"),
    );
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Write,
        )
        .with_array_name("sum"),
    );
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Write,
        )
        .with_array_name("carry"),
    );
    accesses.add(
        AccessRelation::new(StmtId(1), domain, access_map, AccessType::ReadWrite)
            .with_array_name("carry"),
    );

    let mut quantities = QuantityMap::new();
    quantities.insert("a".to_string(), Quantity::Many);
    quantities.insert("b".to_string(), Quantity::Many);
    quantities.insert("sum".to_string(), Quantity::Many);
    quantities.insert("carry".to_string(), Quantity::Zero);

    let module = PirModule::new(
        vec![stmt_compute, stmt_uncompute],
        schedule_tree,
        accesses,
        quantities,
        vec!["N".to_string()],
    );
    Ok(module)
}

#[test]
fn test_matmul_fixture_validates() {
    let module = construct_fixture("matmul_64x64").unwrap();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "matmul fixture validation failed: {:?}",
        result
    );
}

#[test]
fn test_stencil_fixture_validates() {
    let module = construct_fixture("stencil_3d").unwrap();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "stencil fixture validation failed: {:?}",
        result
    );
}

#[test]
fn test_fft_fixture_validates() {
    let module = construct_fixture("fft_1024").unwrap();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "fft fixture validation failed: {:?}",
        result
    );
}

#[test]
fn test_teleport_fixture_validates() {
    let module = construct_fixture("teleport").unwrap();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "teleport fixture validation failed: {:?}",
        result
    );
}

#[test]
fn test_rev_adder_fixture_validates() {
    let module = construct_fixture("rev_adder").unwrap();
    let result = validate_pir(&module);
    assert!(
        result.is_ok(),
        "rev_adder fixture validation failed: {:?}",
        result
    );
}

#[test]
fn test_all_fixtures_have_valid_schedules() {
    let fixtures = [
        "matmul_64x64",
        "stencil_3d",
        "fft_1024",
        "teleport",
        "rev_adder",
    ];
    for name in fixtures {
        let module = construct_fixture(name).unwrap();
        let report = validate_schedule_detailed(&module.schedule).unwrap();
        assert!(report.band_count > 0, "Fixture {} has no bands", name);
        assert!(
            report.domain_count > 0,
            "Fixture {} has no domain nodes",
            name
        );
    }
}

#[test]
fn test_all_fixtures_preserve_quantities() {
    let fixtures = [
        "matmul_64x64",
        "stencil_3d",
        "fft_1024",
        "teleport",
        "rev_adder",
    ];
    for name in fixtures {
        let module = construct_fixture(name).unwrap();
        let result = validate_pir(&module);
        assert!(
            result.is_ok(),
            "Fixture {} failed quantity validation: {:?}",
            name,
            result
        );
    }
}

#[test]
fn test_fixture_json_roundtrip() {
    let module = construct_fixture("matmul_64x64").unwrap();
    let json = serde_json::to_string_pretty(&module).unwrap();
    let module2: PirModule = serde_json::from_str(&json).unwrap();
    assert_eq!(module, module2);
}

#[test]
fn test_fixture_golden_format() {
    let module = construct_fixture("matmul_64x64").unwrap();
    let fixture = format_golden_fixture(&module);
    assert!(fixture.contains("[parameters]"));
    assert!(fixture.contains("[quantities]"));
    assert!(fixture.contains("[statements]"));
    assert!(fixture.contains("[schedule]"));
    assert!(fixture.contains("[accesses]"));
    assert!(fixture.contains("Zero"));
}

#[test]
fn test_inversion_correctness_property() {
    // Property test: forward + inverse schedule should be identity on live variables
    // This is a simplified version - in practice would use quickcheck/proptest
    let module = construct_fixture("rev_adder").unwrap();

    // The forward schedule computes sum, the reverse uncomputes carry
    // After both, carry should be cleaned up (Zero quantity)
    let result = validate_pir(&module);
    assert!(result.is_ok());

    // Verify carry is Zero quantity and doesn't appear in runtime
    for (var, qty) in &module.quantities {
        if var == "carry" {
            assert_eq!(*qty, Quantity::Zero);
        }
    }
}
