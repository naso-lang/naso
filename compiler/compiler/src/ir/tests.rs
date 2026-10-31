//! IR Module Tests
//!
//! Comprehensive unit tests for all IR components.

// Re-export all submodules for testing
use super::access_relation::*;
use super::affine_domain::*;
use super::affine_map::*;
use super::pir_types::*;
use super::pretty_print::*;
use super::schedule_tree::*;
use super::validate::*;
use crate::ast::{Mutability, Quantity};

// Additional integration tests

#[test]
fn test_full_pir_roundtrip() {
    // Build a complete PIR module
    let domain = AffineDomain::new(
        2,
        2,
        vec![
            AffineConstraint::inequality(vec![1, 0, 0, 0], 0),
            AffineConstraint::inequality(vec![-1, 0, 1, 0], 1),
            AffineConstraint::inequality(vec![0, 1, 0, 0], 0),
            AffineConstraint::inequality(vec![0, -1, 0, 1], 1),
        ],
    )
    .with_name("matmul_domain".to_string());

    let mut m1 = Matrix::new(1, 4);
    m1.set(0, 0, 1);
    let schedule_i = AffineMap::total(domain.clone(), m1);

    let mut m2 = Matrix::new(1, 4);
    m2.set(0, 1, 1);
    let schedule_j = AffineMap::total(domain.clone(), m2);

    let mut m3 = Matrix::new(1, 4);
    m3.set(0, 0, 1);
    let access_map = AffineMap::total(domain.clone(), m3);

    let stmt = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Binary {
            op: BinaryOp::Mul,
            left: Box::new(PirExpr::Index {
                base: Box::new(PirExpr::Var("A".to_string())),
                indices: vec![PirExpr::Var("i".to_string()), PirExpr::Var("k".to_string())],
            }),
            right: Box::new(PirExpr::Index {
                base: Box::new(PirExpr::Var("B".to_string())),
                indices: vec![PirExpr::Var("k".to_string()), PirExpr::Var("j".to_string())],
            }),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let inner_band = ScheduleNode::band(
        vec![schedule_j],
        vec![true],
        ScheduleNode::domain(StmtId(0), domain.clone()),
    );

    let outer_band = ScheduleNode::band(vec![schedule_i], vec![false], inner_band);

    let schedule = ScheduleTree::new(outer_band, vec!["N".to_string(), "M".to_string()]);

    let mut accesses = AccessRelations::new();
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Read,
        )
        .with_array_name("A"),
    );
    accesses.add(
        AccessRelation::new(
            StmtId(0),
            domain.clone(),
            access_map.clone(),
            AccessType::Read,
        )
        .with_array_name("B"),
    );
    accesses.add(
        AccessRelation::new(StmtId(0), domain, access_map, AccessType::Write).with_array_name("C"),
    );

    let mut quantities = QuantityMap::new();
    quantities.insert("N".to_string(), Quantity::Zero);
    quantities.insert("M".to_string(), Quantity::Zero);

    let module = PirModule::new(
        vec![stmt],
        schedule,
        accesses,
        quantities,
        vec!["N".to_string(), "M".to_string()],
    );

    // Validate
    assert!(validate_pir(&module).is_ok());

    // Serialize to JSON
    let json = pir_to_json(&module).unwrap();
    assert!(!json.is_empty());

    // Deserialize
    let module2: PirModule = pir_from_json(&json).unwrap();
    assert_eq!(module, module2);

    // Format as golden fixture
    let fixture = format_golden_fixture(&module);
    assert!(fixture.contains("[parameters]"));
    assert!(fixture.contains("matmul_domain"));
}

#[test]
fn test_affine_domain_operations() {
    // Test intersection
    let d1 = AffineDomain::new(
        1,
        0,
        vec![
            AffineConstraint::inequality(vec![1], 0),
            AffineConstraint::inequality(vec![-1], -10),
        ],
    );
    let d2 = AffineDomain::new(
        1,
        0,
        vec![
            AffineConstraint::inequality(vec![1], 5),
            AffineConstraint::inequality(vec![-1], -15),
        ],
    );
    let inter = d1.intersection(&d2);
    assert!(inter.contains(&[5]));
    assert!(inter.contains(&[10]));
    assert!(!inter.contains(&[4]));
    assert!(!inter.contains(&[11]));

    // Test projection
    let d3 = AffineDomain::new(
        2,
        1,
        vec![
            AffineConstraint::inequality(vec![1, 0, 0], 0),
            AffineConstraint::inequality(vec![-1, 0, 1], 1),
            AffineConstraint::inequality(vec![0, 1, 0], 0),
            AffineConstraint::inequality(vec![0, -1, 1], 1),
        ],
    );
    let proj = d3.project(&[1]); // Project out j
    assert_eq!(proj.n_iter, 1);
    assert_eq!(proj.n_param, 1);
    assert!(proj.contains(&[5, 10]));
}

#[test]
fn test_affine_map_composition() {
    // Map 1: (i, j) -> (i)
    let dom1 = AffineDomain::universe(2, 0);
    let mut m1 = Matrix::new(1, 2);
    m1.set(0, 0, 1);
    let _map1 = AffineMap::total(dom1.clone(), m1);

    // Map 2: (i) -> (i * 2)
    let dom2 = AffineDomain::universe(1, 0);
    let mut m2 = Matrix::new(1, 1);
    m2.set(0, 0, 2);
    let _map2 = AffineMap::total(dom2, m2);

    // Can't compose directly due to dimension mismatch
    // But we can test same-dimension composition
    let mut m3 = Matrix::new(2, 2);
    m3.set(0, 0, 1);
    m3.set(1, 1, 1);
    let map3 = AffineMap::total(dom1.clone(), m3);

    let mut m4 = Matrix::new(2, 2);
    m4.set(0, 0, 2);
    m4.set(1, 1, 3);
    let map4 = AffineMap::total(dom1, m4);

    let composed = map3.compose(&map4);
    assert_eq!(composed.pieces.len(), 1);
    assert_eq!(composed.pieces[0].matrix.get(0, 0), 2);
    assert_eq!(composed.pieces[0].matrix.get(1, 1), 3);
}

#[test]
fn test_schedule_tree_construction() {
    // Build a 3-level nested loop schedule
    let domain = AffineDomain::new(
        3,
        0,
        vec![
            AffineConstraint::inequality(vec![1, 0, 0], 0),
            AffineConstraint::inequality(vec![-1, 0, 0], -10),
            AffineConstraint::inequality(vec![0, 1, 0], 0),
            AffineConstraint::inequality(vec![0, -1, 0], -10),
            AffineConstraint::inequality(vec![0, 0, 1], 0),
            AffineConstraint::inequality(vec![0, 0, -1], -10),
        ],
    );

    let mut m1 = Matrix::new(1, 3);
    m1.set(0, 0, 1);
    let schedule_i = AffineMap::total(domain.clone(), m1);

    let mut m2 = Matrix::new(1, 3);
    m2.set(0, 1, 1);
    let schedule_j = AffineMap::total(domain.clone(), m2);

    let mut m3 = Matrix::new(1, 3);
    m3.set(0, 2, 1);
    let schedule_k = AffineMap::total(domain.clone(), m3);

    let inner = ScheduleNode::band(
        vec![schedule_k],
        vec![true],
        ScheduleNode::domain(StmtId(0), domain.clone()),
    );
    let middle = ScheduleNode::band(vec![schedule_j], vec![true], inner);
    let outer = ScheduleNode::band(vec![schedule_i], vec![false], middle);

    let tree = ScheduleTree::new(outer, vec![]);
    assert!(tree.validate().is_ok());

    let report = validate_schedule_detailed(&tree).unwrap();
    assert_eq!(report.band_count, 3);
    assert_eq!(report.domain_count, 1);
    assert_eq!(report.max_depth, 3);
}

#[test]
fn test_access_relation_dependence() {
    let domain = AffineDomain::new(
        1,
        1,
        vec![
            AffineConstraint::inequality(vec![1, 0], 0),
            AffineConstraint::inequality(vec![-1, 1], 1),
        ],
    );

    let mut m = Matrix::new(1, 2);
    m.set(0, 0, 1);
    let map = AffineMap::total(domain.clone(), m);

    let mut relations = AccessRelations::new();
    relations.add(
        AccessRelation::new(StmtId(0), domain.clone(), map.clone(), AccessType::Write)
            .with_array_name("A"),
    );
    relations.add(
        AccessRelation::new(StmtId(1), domain.clone(), map.clone(), AccessType::Read)
            .with_array_name("A"),
    );
    relations.add(
        AccessRelation::new(StmtId(2), domain.clone(), map.clone(), AccessType::Write)
            .with_array_name("A"),
    );

    let raw = relations.raw_dependences();
    // RAW: write must execute BEFORE read (write.stmt_id < read.stmt_id)
    // S0 (write) -> S1 (read): 0 < 1 = TRUE
    // S2 (write) -> S1 (read): 2 < 1 = FALSE
    assert_eq!(raw.len(), 1);

    let waw = relations.waw_dependences();
    // WAW: write i before write j where i < j
    // S0 -> S2: 0 < 2 = TRUE
    assert_eq!(waw.len(), 1);

    let war = relations.war_dependences();
    // WAR: read must execute BEFORE write (read.stmt_id < write.stmt_id)
    // S1 (read) -> S2 (write): 1 < 2 = TRUE
    // S1 (read) -> S0 (write): 1 < 0 = FALSE
    assert_eq!(war.len(), 1);
}

#[test]
fn test_matrix_inverse() {
    // Test integer matrix inverse (only works for unimodular matrices)
    let mut m = Matrix::new(2, 2);
    m.set(0, 0, 1);
    m.set(0, 1, 1);
    m.set(1, 0, 0);
    m.set(1, 1, 1);
    // det = 1, invertible over integers

    let inv = m.inverse();
    assert!(inv.is_some());
    let inv_m = inv.unwrap();
    // [1 1; 0 1] inverse = [1 -1; 0 1]
    assert_eq!(inv_m.get(0, 0), 1);
    assert_eq!(inv_m.get(0, 1), -1);
    assert_eq!(inv_m.get(1, 0), 0);
    assert_eq!(inv_m.get(1, 1), 1);
}

#[test]
fn test_quantity_validation_comprehensive() {
    // Test all quantity types
    let domain = AffineDomain::universe(1, 0);
    let mut m = Matrix::new(1, 1);
    m.set(0, 0, 1);
    let map = AffineMap::total(domain.clone(), m);

    // Zero quantity - should not appear in runtime
    let stmt_zero = PirStatement {
        id: StmtId(0),
        domain: domain.clone(),
        body: PirExpr::Var("z".to_string()),
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    // One quantity - should appear exactly once
    let stmt_one = PirStatement {
        id: StmtId(1),
        domain: domain.clone(),
        body: PirExpr::Var("o".to_string()),
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    // Many quantity - unrestricted
    let stmt_many = PirStatement {
        id: StmtId(2),
        domain: domain.clone(),
        body: PirExpr::Binary {
            op: BinaryOp::Add,
            left: Box::new(PirExpr::Var("m".to_string())),
            right: Box::new(PirExpr::Var("m".to_string())),
        },
        quantity: Quantity::Many,
        mutability: Mutability::Immutable,
        span: None,
    };

    let schedule = ScheduleTree::new(
        ScheduleNode::sequence(vec![
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain.clone()),
            ),
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(1), domain.clone()),
            ),
            ScheduleNode::band(
                vec![map.clone()],
                vec![false],
                ScheduleNode::domain(StmtId(2), domain.clone()),
            ),
        ]),
        vec![],
    );

    let mut accesses = AccessRelations::new();
    accesses.add(AccessRelation::new(
        StmtId(0),
        domain.clone(),
        map.clone(),
        AccessType::Write,
    ));
    accesses.add(AccessRelation::new(
        StmtId(1),
        domain.clone(),
        map.clone(),
        AccessType::Write,
    ));
    accesses.add(AccessRelation::new(
        StmtId(2),
        domain,
        map,
        AccessType::Write,
    ));

    let mut quantities = QuantityMap::new();
    quantities.insert("z".to_string(), Quantity::Zero);
    quantities.insert("o".to_string(), Quantity::One);
    quantities.insert("m".to_string(), Quantity::Many);

    let module = PirModule::new(
        vec![stmt_zero, stmt_one, stmt_many],
        schedule,
        accesses,
        quantities,
        vec![],
    );

    let result = validate_pir(&module);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    // Should have: zero in runtime, one used once (OK), many used twice (OK)
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValidationError::ZeroQuantityInRuntime(_, _)))
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, ValidationError::LinearVarNotUsed(_)))
    );
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, ValidationError::LinearVarUsedMultipleTimes(_, _)))
    );
}
