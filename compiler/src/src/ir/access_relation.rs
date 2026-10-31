//! Access Relation Representation
//!
//! Maps statement instances to memory locations with read/write annotations.
//! Used for dependence analysis, memory footprint computation, and code generation.

#![allow(clippy::collapsible_if)]

use super::affine_domain::AffineDomain;
use super::affine_map::AffineMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Type of memory access
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessType {
    /// Read-only access
    Read,
    /// Write-only access
    Write,
    /// Read-write access (read then write)
    ReadWrite,
    /// Reduction access (accumulator)
    Reduction,
}

impl fmt::Display for AccessType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccessType::Read => write!(f, "Read"),
            AccessType::Write => write!(f, "Write"),
            AccessType::ReadWrite => write!(f, "ReadWrite"),
            AccessType::Reduction => write!(f, "Reduction"),
        }
    }
}

/// Access relation: statement instance -> memory location
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRelation {
    /// Statement identifier
    pub stmt_id: super::schedule_tree::StmtId,
    /// Domain of statement instances this access applies to
    pub stmt_domain: AffineDomain,
    /// Affine map from statement instance to memory location
    pub access_map: AffineMap,
    /// Type of access
    pub access_type: AccessType,
    /// Optional array/memory region name for debugging
    pub array_name: Option<String>,
}

impl AccessRelation {
    pub fn new(
        stmt_id: super::schedule_tree::StmtId,
        stmt_domain: AffineDomain,
        access_map: AffineMap,
        access_type: AccessType,
    ) -> Self {
        assert_eq!(stmt_domain.dims, access_map.pieces[0].domain.dims);
        Self {
            stmt_id,
            stmt_domain,
            access_map,
            access_type,
            array_name: None,
        }
    }

    pub fn with_array_name(mut self, name: impl Into<String>) -> Self {
        self.array_name = Some(name.into());
        self
    }

    /// Apply access relation to a statement instance point
    /// Returns the memory location accessed
    pub fn apply(&self, stmt_point: &[i64]) -> Option<Vec<i64>> {
        if self.stmt_domain.contains(stmt_point) {
            self.access_map.apply(stmt_point)
        } else {
            None
        }
    }

    /// Get the memory domain (image of stmt_domain under access_map)
    pub fn memory_domain(&self) -> AffineDomain {
        self.access_map.image(&self.stmt_domain)
    }

    /// Check if two access relations may alias (overlap in memory)
    pub fn may_alias(&self, other: &AccessRelation) -> bool {
        // Quick check: different arrays can't alias
        if let (Some(a), Some(b)) = (&self.array_name, &other.array_name) {
            if a != b {
                return false;
            }
        }

        // Check if memory domains intersect
        let dom1 = self.memory_domain();
        let dom2 = other.memory_domain();
        let inter = dom1.intersection(&dom2);
        !inter.is_empty()
    }

    /// Check if this is a read access
    pub fn is_read(&self) -> bool {
        matches!(self.access_type, AccessType::Read | AccessType::ReadWrite)
    }

    /// Check if this is a write access
    pub fn is_write(&self) -> bool {
        matches!(self.access_type, AccessType::Write | AccessType::ReadWrite)
    }
}

impl fmt::Display for AccessRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.array_name.as_deref().unwrap_or("mem");
        write!(
            f,
            "{} {}: {} -> {} ({})",
            self.access_type,
            name,
            self.stmt_id,
            self.access_map,
            self.stmt_domain.name.as_deref().unwrap_or("")
        )
    }
}

/// Collection of access relations for a module
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRelations {
    pub relations: Vec<AccessRelation>,
}

impl AccessRelations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, relation: AccessRelation) {
        self.relations.push(relation);
    }

    /// Get all accesses for a given statement
    pub fn for_stmt(&self, stmt_id: super::schedule_tree::StmtId) -> Vec<&AccessRelation> {
        self.relations
            .iter()
            .filter(|r| r.stmt_id == stmt_id)
            .collect()
    }

    /// Get all read accesses
    pub fn reads(&self) -> Vec<&AccessRelation> {
        self.relations.iter().filter(|r| r.is_read()).collect()
    }

    /// Get all write accesses
    pub fn writes(&self) -> Vec<&AccessRelation> {
        self.relations.iter().filter(|r| r.is_write()).collect()
    }

    /// Check for potential read-after-write (RAW) dependences
    /// RAW: write must execute BEFORE read (write.stmt_id < read.stmt_id)
    pub fn raw_dependences(&self) -> Vec<(&AccessRelation, &AccessRelation)> {
        let mut deps = Vec::new();
        for write in self.writes() {
            for read in self.reads() {
                if write.stmt_id < read.stmt_id && write.may_alias(read) {
                    deps.push((write, read));
                }
            }
        }
        deps
    }

    /// Check for potential write-after-read (WAR) dependences
    /// WAR: read must execute BEFORE write (read.stmt_id < write.stmt_id)
    pub fn war_dependences(&self) -> Vec<(&AccessRelation, &AccessRelation)> {
        let mut deps = Vec::new();
        for read in self.reads() {
            for write in self.writes() {
                if read.stmt_id < write.stmt_id && read.may_alias(write) {
                    deps.push((read, write));
                }
            }
        }
        deps
    }

    /// Check for potential write-after-write (WAW) dependences
    pub fn waw_dependences(&self) -> Vec<(&AccessRelation, &AccessRelation)> {
        let writes = self.writes();
        let mut deps = Vec::new();
        for i in 0..writes.len() {
            for j in i + 1..writes.len() {
                if writes[i].may_alias(writes[j]) {
                    deps.push((writes[i], writes[j]));
                }
            }
        }
        deps
    }
}

#[cfg(test)]
mod tests {
    use super::super::affine_domain::AffineDomain;
    use super::super::affine_map::{AffineMap, Matrix};
    use super::super::schedule_tree::StmtId;
    use super::*;

    #[test]
    fn test_access_relation() {
        // Statement S(i) accessing A[i]
        let stmt_domain = AffineDomain::new(
            1,
            1,
            vec![
                super::super::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![-1, 1], 1),
            ],
        );

        let mut m = Matrix::new(1, 2);
        m.set(0, 0, 1); // memory index = i
        let access_map = AffineMap::total(stmt_domain.clone(), m);

        let access = AccessRelation::new(StmtId(0), stmt_domain, access_map, AccessType::ReadWrite)
            .with_array_name("A");

        assert_eq!(access.apply(&[5, 10]), Some(vec![5]));
        assert_eq!(access.apply(&[10, 10]), None); // out of bounds
    }

    #[test]
    fn test_alias_detection() {
        let domain = AffineDomain::new(
            1,
            1,
            vec![
                super::super::affine_domain::AffineConstraint::inequality(vec![1, 0], 0),
                super::super::affine_domain::AffineConstraint::inequality(vec![-1, 1], 1),
            ],
        );

        let mut m = Matrix::new(1, 2);
        m.set(0, 0, 1);
        let map = AffineMap::total(domain.clone(), m);

        let read = AccessRelation::new(StmtId(0), domain.clone(), map.clone(), AccessType::Read)
            .with_array_name("A");
        let write = AccessRelation::new(StmtId(1), domain.clone(), map.clone(), AccessType::Write)
            .with_array_name("A");

        assert!(read.may_alias(&write));

        // Different arrays
        let write_b =
            AccessRelation::new(StmtId(1), domain, map, AccessType::Write).with_array_name("B");
        assert!(!read.may_alias(&write_b));
    }

    #[test]
    fn test_dependence_analysis() {
        let domain = AffineDomain::universe(1, 0);
        let mut m = Matrix::new(1, 1);
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
        assert_eq!(raw.len(), 1); // Only S0 -> S1

        let waw = relations.waw_dependences();
        // WAW: write i before write j where i < j
        // S0 -> S2: 0 < 2 = TRUE
        assert_eq!(waw.len(), 1); // S0 -> S2
    }
}
