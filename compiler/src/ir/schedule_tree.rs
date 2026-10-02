//! Schedule Tree Representation
//!
//! Hierarchical schedule representation following the ISL schedule tree model.
//! Nodes represent: bands (parallel loops), filters (domain restrictions),
//! sequences (sequential composition), contexts (parameter constraints),
//! and domains (statement instances).

use super::affine_domain::AffineDomain;
use super::affine_map::AffineMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a statement in the schedule
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StmtId(pub usize);

impl fmt::Display for StmtId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "S{}", self.0)
    }
}

/// Schedule tree node types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleNode {
    /// Band node: multi-dimensional affine loop nest
    /// Each member is a scheduling dimension (affine map from iteration domain to schedule time)
    Band {
        /// Scheduling dimensions (one per loop level in the band)
        members: Vec<AffineMap>,
        /// Coincident flags: true if iterations can be executed in parallel
        ///
        /// This is a DEPENDENCE claim, not a statement about shape. `true` on a level
        /// means the accesses covered by this band were found to carry no dependence
        /// that is serialised by that level. A level whose accesses were not examined,
        /// or were examined and found to conflict, is `false`.
        coincident: Vec<bool>,
        /// The SOURCE SPELLING of each level's induction variable, outermost first.
        ///
        /// One entry per level, the same shape and length as `members` and `coincident`.
        /// The loop body refers to its iterator by that name (`forall i in .. { i }`), so
        /// a backend that emits a real induction variable still has to bind this name to
        /// it or the body reads an unbound name.
        ///
        /// An EMPTY string means "this level's iterator is not declared". That is a real
        /// state, not a placeholder: a hand-built band with no source iterator emits a
        /// correct loop whose induction variable is simply not reachable by name, and a
        /// body that reads the name is then a diagnostic rather than a wrong value. An
        /// empty string never falls back to a neighbouring level's name.
        ///
        /// This field used to be smuggled through `AffineDomain::name` as
        /// `nest(i)` and recovered by string-parsing, which meant the debug label and the
        /// semantics were one field and a typo in a format string silently unbound every
        /// body.
        iterators: Vec<String>,
        /// Child node
        child: Box<ScheduleNode>,
    },
    /// Filter node: restrict domain to a subset
    Filter {
        domain: AffineDomain,
        child: Box<ScheduleNode>,
    },
    /// Sequence node: execute children sequentially
    Sequence { children: Vec<ScheduleNode> },
    /// Context node: parameter constraints
    Context {
        domain: AffineDomain,
        child: Box<ScheduleNode>,
    },
    /// Domain node: leaf representing statement instances
    Domain {
        stmt_id: StmtId,
        domain: AffineDomain,
    },
    /// Extension node: for tiling, unrolling, etc.
    Extension {
        /// The expansion factor / tile sizes
        sizes: Vec<usize>,
        child: Box<ScheduleNode>,
    },
    /// Empty node (no-op)
    Empty,
}

impl ScheduleNode {
    /// Create a band node with no declared iterators.
    ///
    /// Every level gets an EMPTY iterator name, meaning "this band does not say what its
    /// induction variables are called". A backend may still emit a real loop; a body that
    /// reads the iterator is then unbound, which is a diagnostic rather than a silently
    /// wrong value. Callers that DO know the source spelling use
    /// [`ScheduleNode::band_with_iterators`].
    pub fn band(members: Vec<AffineMap>, coincident: Vec<bool>, child: ScheduleNode) -> Self {
        let iterators = vec![String::new(); members.len()];
        Self::band_with_iterators(members, coincident, iterators, child)
    }

    /// Create a band node whose levels have known iterator names.
    ///
    /// `iterators` must be the same length as `members` and `coincident`: one entry per
    /// loop level, outermost first. An empty string is allowed and means "not declared".
    pub fn band_with_iterators(
        members: Vec<AffineMap>,
        coincident: Vec<bool>,
        iterators: Vec<String>,
        child: ScheduleNode,
    ) -> Self {
        assert_eq!(members.len(), coincident.len());
        assert_eq!(members.len(), iterators.len());
        ScheduleNode::Band {
            members,
            coincident,
            iterators,
            child: Box::new(child),
        }
    }

    /// The declared iterator name for `level`, or `None` when it is not declared.
    ///
    /// An empty name is reported as absent rather than returned as `Some("")`, so a
    /// caller can never bind a variable to the empty string.
    pub fn iterator_at(&self, level: usize) -> Option<&str> {
        let ScheduleNode::Band { iterators, .. } = self else {
            return None;
        };
        iterators
            .get(level)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Create a filter node
    pub fn filter(domain: AffineDomain, child: ScheduleNode) -> Self {
        ScheduleNode::Filter {
            domain,
            child: Box::new(child),
        }
    }

    /// Create a sequence node
    pub fn sequence(children: Vec<ScheduleNode>) -> Self {
        ScheduleNode::Sequence { children }
    }

    /// Create a context node
    pub fn context(domain: AffineDomain, child: ScheduleNode) -> Self {
        ScheduleNode::Context {
            domain,
            child: Box::new(child),
        }
    }

    /// Create a domain node (leaf)
    pub fn domain(stmt_id: StmtId, domain: AffineDomain) -> Self {
        ScheduleNode::Domain { stmt_id, domain }
    }

    /// Create an extension node
    pub fn extension(sizes: Vec<usize>, child: ScheduleNode) -> Self {
        ScheduleNode::Extension {
            sizes,
            child: Box::new(child),
        }
    }

    /// Create empty node
    pub fn empty() -> Self {
        ScheduleNode::Empty
    }

    /// Collect all domain nodes (statement instances) in this tree
    pub fn collect_domains(&self) -> Vec<(StmtId, AffineDomain)> {
        let mut result = Vec::new();
        self.collect_domains_rec(&mut result);
        result
    }

    fn collect_domains_rec(&self, result: &mut Vec<(StmtId, AffineDomain)>) {
        match self {
            ScheduleNode::Band { child, .. } => child.collect_domains_rec(result),
            ScheduleNode::Filter { child, .. } => child.collect_domains_rec(result),
            ScheduleNode::Sequence { children } => {
                for c in children {
                    c.collect_domains_rec(result);
                }
            }
            ScheduleNode::Context { child, .. } => child.collect_domains_rec(result),
            ScheduleNode::Domain { stmt_id, domain } => {
                result.push((*stmt_id, domain.clone()));
            }
            ScheduleNode::Extension { child, .. } => child.collect_domains_rec(result),
            ScheduleNode::Empty => {}
        }
    }

    /// Collect all band nodes with their scheduling dimensions
    pub fn collect_bands(&self) -> Vec<&Vec<AffineMap>> {
        let mut result = Vec::new();
        self.collect_bands_rec(&mut result);
        result
    }

    fn collect_bands_rec<'a>(&'a self, result: &mut Vec<&'a Vec<AffineMap>>) {
        match self {
            ScheduleNode::Band { members, child, .. } => {
                result.push(members);
                child.collect_bands_rec(result);
            }
            ScheduleNode::Filter { child, .. } => child.collect_bands_rec(result),
            ScheduleNode::Sequence { children } => {
                for c in children {
                    c.collect_bands_rec(result);
                }
            }
            ScheduleNode::Context { child, .. } => child.collect_bands_rec(result),
            ScheduleNode::Extension { child, .. } => child.collect_bands_rec(result),
            ScheduleNode::Domain { .. } | ScheduleNode::Empty => {}
        }
    }

    /// Check if the schedule tree is well-formed
    pub fn validate(&self) -> Result<(), ScheduleValidationError> {
        self.validate_rec(0)
    }

    fn validate_rec(&self, depth: usize) -> Result<(), ScheduleValidationError> {
        if depth > 100 {
            return Err(ScheduleValidationError::MaxDepthExceeded);
        }
        match self {
            ScheduleNode::Band {
                members,
                coincident,
                iterators,
                child,
            } => {
                if members.is_empty() {
                    return Err(ScheduleValidationError::EmptyBand);
                }
                if members.len() != coincident.len() {
                    return Err(ScheduleValidationError::CoincidentLengthMismatch);
                }
                // The same length rule for the iterator names. A mismatch here would mean
                // the band binds level `d`'s induction variable under a name that belongs
                // to a different level, so it is rejected rather than tolerated.
                if members.len() != iterators.len() {
                    return Err(ScheduleValidationError::IteratorLengthMismatch);
                }
                // Check all members have same input dimension
                let input_dims = members[0].pieces[0].domain.dims;
                for m in members {
                    if m.pieces[0].domain.dims != input_dims {
                        return Err(ScheduleValidationError::BandDimensionMismatch);
                    }
                }
                child.validate_rec(depth + 1)
            }
            ScheduleNode::Filter { domain, child } => {
                if domain.is_empty() {
                    return Err(ScheduleValidationError::EmptyFilterDomain);
                }
                child.validate_rec(depth + 1)
            }
            ScheduleNode::Sequence { children } => {
                if children.is_empty() {
                    return Err(ScheduleValidationError::EmptySequence);
                }
                for c in children {
                    c.validate_rec(depth + 1)?;
                }
                Ok(())
            }
            ScheduleNode::Context { domain, child } => {
                if domain.is_empty() {
                    return Err(ScheduleValidationError::EmptyContextDomain);
                }
                child.validate_rec(depth + 1)
            }
            ScheduleNode::Domain { domain, .. } => {
                if domain.is_empty() {
                    return Err(ScheduleValidationError::EmptyDomainNode);
                }
                Ok(())
            }
            ScheduleNode::Extension { child, .. } => child.validate_rec(depth + 1),
            ScheduleNode::Empty => Ok(()),
        }
    }

    /// Pretty print the schedule tree
    pub fn pretty_print(&self, indent: usize) -> String {
        let prefix = "  ".repeat(indent);
        match self {
            ScheduleNode::Band {
                members,
                coincident,
                iterators,
                child,
            } => {
                let mut s = format!("{}Band ({} dims):\n", prefix, members.len());
                for (i, (m, c)) in members.iter().zip(coincident.iter()).enumerate() {
                    s += &format!(
                        "{}  [{}] {} {}{}\n",
                        prefix,
                        i,
                        if *c { "coincident" } else { "sequential" },
                        m,
                        // An empty iterator name is shown as such rather than omitted, so
                        // a reader can tell "no iterator declared" from "name not printed".
                        match iterators.get(i).map(String::as_str) {
                            Some(name) if !name.is_empty() => format!("  iterator {name}"),
                            _ => "  iterator <undeclared>".to_string(),
                        }
                    );
                }
                s += &child.pretty_print(indent + 1);
                s
            }
            ScheduleNode::Filter { domain, child } => {
                format!(
                    "{}Filter: {}\n{}",
                    prefix,
                    domain.name.as_deref().unwrap_or(""),
                    child.pretty_print(indent + 1)
                )
            }
            ScheduleNode::Sequence { children } => {
                let mut s = format!("{}Sequence:\n", prefix);
                for c in children {
                    s += &c.pretty_print(indent + 1);
                }
                s
            }
            ScheduleNode::Context { domain, child } => {
                format!(
                    "{}Context: {}\n{}",
                    prefix,
                    domain.name.as_deref().unwrap_or(""),
                    child.pretty_print(indent + 1)
                )
            }
            ScheduleNode::Domain { stmt_id, domain } => {
                format!(
                    "{}Domain {}: {}\n",
                    prefix,
                    stmt_id,
                    domain.name.as_deref().unwrap_or("")
                )
            }
            ScheduleNode::Extension { sizes, child } => {
                format!(
                    "{}Extension (sizes={:?}):\n{}",
                    prefix,
                    sizes,
                    child.pretty_print(indent + 1)
                )
            }
            ScheduleNode::Empty => format!("{}Empty\n", prefix),
        }
    }
}

impl fmt::Display for ScheduleNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pretty_print(0))
    }
}

/// Complete schedule tree wrapper
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleTree {
    pub root: ScheduleNode,
    /// Parameters (symbolic constants) used in the schedule
    pub parameters: Vec<String>,
}

impl ScheduleTree {
    pub fn new(root: ScheduleNode, parameters: Vec<String>) -> Self {
        Self { root, parameters }
    }

    pub fn validate(&self) -> Result<(), ScheduleValidationError> {
        self.root.validate()
    }

    pub fn collect_domains(&self) -> Vec<(StmtId, AffineDomain)> {
        self.root.collect_domains()
    }

    pub fn pretty_print(&self) -> String {
        let mut s = format!("Parameters: {:?}\n", self.parameters);
        s += &self.root.pretty_print(0);
        s
    }
}

/// Schedule tree validation errors
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleValidationError {
    EmptyBand,
    CoincidentLengthMismatch,
    IteratorLengthMismatch,
    BandDimensionMismatch,
    EmptyFilterDomain,
    EmptySequence,
    EmptyContextDomain,
    EmptyDomainNode,
    MaxDepthExceeded,
}

impl fmt::Display for ScheduleValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScheduleValidationError::EmptyBand => write!(f, "Band node has no members"),
            ScheduleValidationError::CoincidentLengthMismatch => {
                write!(f, "Coincident flags length doesn't match band members")
            }
            ScheduleValidationError::IteratorLengthMismatch => {
                write!(
                    f,
                    "Iterator names length doesn't match band members, so a level's induction variable would be bound under another level's name"
                )
            }
            ScheduleValidationError::BandDimensionMismatch => {
                write!(f, "Band members have different input dimensions")
            }
            ScheduleValidationError::EmptyFilterDomain => write!(f, "Filter node has empty domain"),
            ScheduleValidationError::EmptySequence => write!(f, "Sequence node has no children"),
            ScheduleValidationError::EmptyContextDomain => {
                write!(f, "Context node has empty domain")
            }
            ScheduleValidationError::EmptyDomainNode => write!(f, "Domain node has empty domain"),
            ScheduleValidationError::MaxDepthExceeded => {
                write!(f, "Schedule tree exceeds maximum depth")
            }
        }
    }
}

impl std::error::Error for ScheduleValidationError {}

#[cfg(test)]
mod tests {
    use super::super::affine_domain::AffineDomain;
    use super::*;

    #[test]
    fn test_simple_schedule() {
        // Simple schedule: for i in 0..N { S(i) }
        let domain = AffineDomain::new(
            1,
            1,
            vec![
                super::super::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![-1, 1], 1),
            ],
        );

        let mut m = super::super::affine_map::Matrix::new(1, 2);
        m.set(0, 0, 1); // schedule time = i
        let schedule_map = super::super::affine_map::AffineMap::total(domain.clone(), m);

        let tree = ScheduleNode::band(
            vec![schedule_map],
            vec![false], // sequential
            ScheduleNode::domain(StmtId(0), domain),
        );

        let sched = ScheduleTree::new(tree, vec!["N".to_string()]);
        assert!(sched.validate().is_ok());

        let domains = sched.collect_domains();
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].0, StmtId(0));
    }

    #[test]
    fn test_nested_schedule() {
        // for i in 0..N { for j in 0..M { S(i,j) } }
        let domain = AffineDomain::new(
            2,
            2,
            vec![
                super::super::affine_domain::AffineConstraint::inequality(vec![1, 0, 0, 0], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![-1, 0, 1, 0], 1),
                super::super::affine_domain::AffineConstraint::inequality(vec![0, 1, 0, 0], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![0, -1, 0, 1], 1),
            ],
        );

        // Outer loop: i
        let mut m1 = super::super::affine_map::Matrix::new(1, 4);
        m1.set(0, 0, 1);
        let schedule_i = super::super::affine_map::AffineMap::total(domain.clone(), m1);

        // Inner loop: j
        let mut m2 = super::super::affine_map::Matrix::new(1, 4);
        m2.set(0, 1, 1);
        let schedule_j = super::super::affine_map::AffineMap::total(domain.clone(), m2);

        let inner_band = ScheduleNode::band(
            vec![schedule_j],
            vec![true], // j is parallel
            ScheduleNode::domain(StmtId(0), domain.clone()),
        );

        let outer_band = ScheduleNode::band(
            vec![schedule_i],
            vec![false], // i is sequential
            inner_band,
        );

        let sched = ScheduleTree::new(outer_band, vec!["N".to_string(), "M".to_string()]);
        assert!(sched.validate().is_ok());
    }

    #[test]
    fn test_sequence_schedule() {
        let domain1 = AffineDomain::new(
            1,
            0,
            vec![
                super::super::affine_domain::AffineConstraint::inequality(vec![1], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![-1], -5),
            ],
        );
        let domain2 = AffineDomain::new(
            1,
            0,
            vec![
                super::super::affine_domain::AffineConstraint::inequality(vec![1], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![-1], -3),
            ],
        );

        let m = super::super::affine_map::Matrix::identity(1);
        let map1 = super::super::affine_map::AffineMap::total(domain1.clone(), m.clone());
        let map2 = super::super::affine_map::AffineMap::total(domain2.clone(), m);

        let seq = ScheduleNode::sequence(vec![
            ScheduleNode::band(
                vec![map1],
                vec![false],
                ScheduleNode::domain(StmtId(0), domain1),
            ),
            ScheduleNode::band(
                vec![map2],
                vec![false],
                ScheduleNode::domain(StmtId(1), domain2),
            ),
        ]);

        let sched = ScheduleTree::new(seq, vec![]);
        assert!(sched.validate().is_ok());
    }

    #[test]
    fn test_validation_errors() {
        // Empty band
        let empty_band = ScheduleNode::band(vec![], vec![], ScheduleNode::empty());
        let tree = ScheduleTree::new(empty_band, vec![]);
        assert!(tree.validate().is_err());

        // Empty sequence
        let empty_seq = ScheduleNode::sequence(vec![]);
        let tree = ScheduleTree::new(empty_seq, vec![]);
        assert!(tree.validate().is_err());
    }
}
