//! Schedule-tree consumer for codegen backends.
//!
//! The straight-line WGSL backend refuses to emit loops, so the natural
//! question is where the loop information should go. It belongs in the
//! diagnostic: a rejection that names the loop's actual iteration domain is
//! strictly more useful than a bare "unsupported", and producing it requires
//! the real schedule tree rather than the AST.
//!
//! That is the point. A backend that renders a band's domain cannot be written
//! against a stub domain, because the stub's `dims` is 0 and there is nothing
//! to render. Consuming the tree here means `lower_program` -- which produces
//! it -- is on the path of `naso build --target wgsl` in a DEFAULT build, with
//! no LLVM feature required.
//!
//! # What this does and does not do
//!
//! It READS the schedule tree. It does not yet transform anything: no
//! permutation, no tiling, no parallel distribution. The bands are identity
//! maps translated by each loop's lower bound (see `loop_nest_to_bands`), so
//! the schedule time is a faithful, source-order schedule time and nothing
//! more. Anyone extending this to actually reorder loops must first read
//! `LoopNest`, which carries loop structure but NOT the body's memory
//! accesses; a reorder chosen without them would be unfounded.

use crate::ir::PirModule;
use crate::ir::schedule_tree::ScheduleNode;

/// One band found in the schedule tree, summarised for a consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandSummary {
    /// Number of scheduling dimensions, i.e. nesting depth.
    pub depth: usize,
    /// The leaf statement this band ultimately schedules.
    pub stmt_id: String,
    /// Per-dimension rendered constraints, e.g. `["i >= 0", "i <= 7"]`.
    pub constraints: Vec<Vec<String>>,
    /// Coincident flags, one per dimension.
    pub coincident: Vec<bool>,
}

impl BandSummary {
    /// A short human-readable description of the iteration domain.
    ///
    /// Renders the constraints rather than a dimension count, because
    /// "depth 1" says nothing about how many iterations run and "0 <= i < 8"
    /// says exactly that.
    pub fn describe(&self) -> String {
        let depth = self.depth;
        let space = match depth {
            0 => "0 dimensions".to_string(),
            1 => "1 dimension".to_string(),
            n => format!("{n} dimensions"),
        };
        let body = if self.constraints.is_empty() {
            "unconstrained".to_string()
        } else {
            let per_dim: Vec<String> = self
                .constraints
                .iter()
                .map(|cs| {
                    if cs.is_empty() {
                        "any".to_string()
                    } else {
                        cs.join(" and ")
                    }
                })
                .collect();
            per_dim.join("; ")
        };
        let par = if self.coincident.iter().filter(|c| **c).count() > 1 {
            ", parallelisable along "
        } else if self.coincident.first().copied().unwrap_or(false) {
            ", parallelisable"
        } else {
            ""
        };
        format!("{space}, domain {body}{par}")
    }
}

/// Render one affine constraint as a readable inequality.
fn render_constraint(c: &crate::ir::affine_domain::AffineConstraint) -> String {
    // A constraint is sum(coeff[i] * x[i]) + constant REL c.constant.
    // Render the linear part symbolically where possible, so the message
    // survives without knowing the iterator's name.
    let mut terms: Vec<String> = Vec::new();
    for (i, coeff) in c.coefficients.iter().enumerate() {
        if *coeff == 0 {
            continue;
        }
        let mag = coeff.abs();
        let var = format!("x{i}");
        terms.push(if mag == 1 {
            var
        } else {
            format!("{mag}*{var}")
        });
    }
    let lhs = if terms.is_empty() {
        "0".to_string()
    } else {
        terms.join(" + ")
    };
    let rel = match c.ctype {
        crate::ir::affine_domain::ConstraintType::Equality => "=",
        crate::ir::affine_domain::ConstraintType::Inequality => ">=",
    };
    if c.constant == 0 {
        format!("{lhs} {rel} 0")
    } else if c.constant > 0 {
        format!("{lhs} {rel} {}", c.constant)
    } else {
        format!("{lhs} {rel} -{}", -c.constant)
    }
}

/// Collect every band in the tree, in execution order.
pub fn summarize_bands(root: &ScheduleNode) -> Vec<BandSummary> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn walk(node: &ScheduleNode, out: &mut Vec<BandSummary>) {
    match node {
        ScheduleNode::Band {
            members,
            coincident,
            child,
            ..
        } => {
            let mut stmt_id = String::from("none");
            let mut leaf_domain = None;
            collect_leaf(child, &mut stmt_id, &mut leaf_domain);

            let constraints = members
                .iter()
                .map(|m| {
                    m.pieces
                        .first()
                        .map(|p| p.domain.constraints.iter().map(render_constraint).collect())
                        .unwrap_or_default()
                })
                .collect();

            out.push(BandSummary {
                depth: members.len(),
                stmt_id,
                constraints,
                coincident: coincident.clone(),
            });
            walk(child, out);
        }
        ScheduleNode::Sequence { children } => {
            for c in children {
                walk(c, out);
            }
        }
        _ => {}
    }
}

fn collect_leaf(
    node: &ScheduleNode,
    stmt_id: &mut String,
    domain: &mut Option<crate::ir::affine_domain::AffineDomain>,
) {
    match node {
        ScheduleNode::Domain {
            stmt_id: id,
            domain: d,
        } => {
            *stmt_id = id.to_string();
            *domain = Some(d.clone());
        }
        ScheduleNode::Band { child, .. } => collect_leaf(child, stmt_id, domain),
        _ => {}
    }
}

/// Summarise every band in a lowered module.
///
/// Returns an empty vector when the module has no schedule tree, which is the
/// normal case for loop-free code. Callers must not treat that as an error.
pub fn module_bands(module: &PirModule) -> Vec<BandSummary> {
    summarize_bands(&module.schedule.root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lowering::lower_program;
    use crate::parser::parse_program;

    fn bands_for(src: &str) -> Vec<BandSummary> {
        let program = parse_program(src).expect("parse");
        let module = lower_program(&program).expect("lower");
        module_bands(&module)
    }

    /// The point of the whole module: a band must report a NON-zero depth and a
    /// domain that admits the loop's iterations. Before the lowering fix every
    /// loop produced a 0-dimensional domain, so there was nothing to render.
    #[test]
    fn test_single_loop_band_has_depth_one_and_a_real_domain() {
        let bands = bands_for("fn f(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }");
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].depth, 1, "a loop has one scheduling dimension");
        assert!(
            bands[0].constraints[0].len() >= 2,
            "both bounds must appear as constraints, got {:?}",
            bands[0].constraints[0]
        );
    }

    /// `0..8` renders as `x0 >= -7 and x0 >= 0`, i.e. 0 <= i < 8. Pin the
    /// rendered constants so an off-by-one in the encoding is visible in the
    /// message a user actually sees.
    #[test]
    fn test_exclusive_upper_bound_appears_in_the_rendered_domain() {
        let bands = bands_for("fn f(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }");
        let joined = bands[0].constraints[0].join(" and ");
        assert!(
            joined.contains("x0 >= -7"),
            "exclusive upper bound: {joined}"
        );
        assert!(joined.contains("x0 >= 0"), "lower bound: {joined}");
    }

    /// A non-zero lower bound must be reflected, not normalised away.
    #[test]
    fn test_nonzero_lower_bound_is_rendered() {
        let bands = bands_for("fn f(t: Tensor[f32,8]) { forall i in 4..9 { t[i] = 1.0; } }");
        let joined = bands[0].constraints[0].join(" and ");
        assert!(joined.contains("x0 >= 4"), "lower bound 4: {joined}");
        assert!(joined.contains("x0 >= -8"), "exclusive upper 9: {joined}");
    }

    /// A nested loop is one band of depth 2, with a constraint set per level.
    #[test]
    fn test_nested_loop_is_depth_two_with_per_dimension_constraints() {
        let bands = bands_for(
            "fn f(t: Tensor[f32,4]) { forall i in 0..4 { forall j in 0..3 { t[i] = 1.0; } } }",
        );
        assert_eq!(bands.len(), 1, "one band, two levels");
        assert_eq!(bands[0].depth, 2);
        assert_eq!(
            bands[0].constraints.len(),
            2,
            "one constraint set per level"
        );
        assert!(bands[0].constraints[0].iter().any(|c| c.contains("-3")));
        assert!(bands[0].constraints[1].iter().any(|c| c.contains("-2")));
    }

    /// The 1024-element kernel reports its real extent, so the number 1023 is
    /// visible -- proof the extent came from the source, not a placeholder.
    #[test]
    fn test_real_kernel_extent_appears_in_the_domain() {
        let bands = bands_for(
            "fn q(input: [1] Tensor[f32,1024], output: inout [1] Tensor[i8,1024], scale: f32) {
                 proof { assert(scale > 0.0); }
                 forall i in 0..1024 { let v = round(input[i] / scale); output[i] = clamp(v, -128.0, 127.0) as i8; }
             }",
        );
        assert_eq!(bands.len(), 1);
        let joined = bands[0].constraints[0].join(" and ");
        assert!(
            joined.contains("x0 >= -1023"),
            "1024 is exclusive: {joined}"
        );
    }

    /// A proof block contributes no band: it is erased before the schedule is
    /// built, so it must not appear as a scheduling dimension.
    #[test]
    fn test_proof_block_contributes_no_band() {
        let with_proof = bands_for("fn f(x: f32) -> f32 { proof { assert(x > 0.0); } return x; }");
        assert!(
            with_proof.is_empty(),
            "a proof block is not runtime code, so it has no band"
        );
    }

    /// Loop-free code has no bands, and that is not an error.
    #[test]
    fn test_loop_free_program_has_no_bands() {
        assert!(bands_for("fn f(a: f32) -> f32 { return a * 2.0; }").is_empty());
    }

    /// Each function's loop produces its own band, so a multi-function kernel
    /// reports every extent rather than only the first.
    #[test]
    fn test_each_function_contributes_its_own_band() {
        // The second function's parameter is named `u`, not `t`. Both functions
        // declaring `t` at different extents is now a REFUSAL, not a band: a `PirModule`
        // is one flat statement list with no function structure, so the generated entry
        // has one slot per NAME and cannot give `t` both `Tensor[f32,8]` and
        // `Tensor[f32,4]`. That refusal is pinned by
        // `lowering::lowering_tests::test_two_functions_may_not_reuse_a_name_at_two_types`.
        // What this test is about is one band per FUNCTION, which needs two distinct
        // names to say at all.
        let bands = bands_for(
            "fn a(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }
             fn b(u: Tensor[f32,4]) { forall i in 0..4 { u[i] = 2.0; } }",
        );
        assert_eq!(bands.len(), 2, "one band per function");
        assert!(
            bands[0].constraints[0].join(" ").contains("-7"),
            "8 elements"
        );
        assert!(
            bands[1].constraints[0].join(" ").contains("-3"),
            "4 elements"
        );
    }

    /// `describe` must convey the extent, not merely a dimension count.
    #[test]
    fn test_describe_mentions_the_domain() {
        let bands = bands_for("fn f(t: Tensor[f32,8]) { forall i in 0..8 { t[i] = 1.0; } }");
        let d = bands[0].describe();
        assert!(d.contains("1 dimension"), "{d}");
        assert!(d.contains("x0 >= -7"), "describe must carry the bound: {d}");
    }

    /// An equality constraint renders with `=`, not `>=`.
    #[test]
    fn test_equality_constraint_renders_correctly() {
        use crate::ir::affine_domain::{AffineConstraint, ConstraintType};
        let c = AffineConstraint {
            coefficients: vec![2],
            constant: 4,
            ctype: ConstraintType::Equality,
        };
        assert_eq!(render_constraint(&c), "2*x0 = 4");
    }
}
