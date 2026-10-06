//! Configuration types for the SMT solver.

#![allow(non_camel_case_types)]

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Supported SMT logics for Z3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Logic {
    /// Quantifier-free uninterpreted functions + linear integer arithmetic
    #[default]
    QF_UFLIA,
    /// Quantifier-free bitvectors
    QF_BV,
    /// Quantifier-free arrays + linear integer arithmetic
    QF_AUFLIA,
    /// Full first-order with linear integer arithmetic (with quantifiers)
    AUFLIA,
    /// Full first-order with mixed real/integer arithmetic + arrays + UF (quantifiers).
    /// Needed for `to_int`/`to_real` reasoning over bounded reals.
    AUFLIRA,
    /// Non-linear arithmetic
    QF_NRA,
}

impl Logic {
    /// SMT-LIB2 logic string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Logic::QF_UFLIA => "QF_UFLIA",
            Logic::QF_BV => "QF_BV",
            Logic::QF_AUFLIA => "QF_AUFLIA",
            Logic::AUFLIA => "AUFLIA",
            Logic::AUFLIRA => "AUFLIRA",
            Logic::QF_NRA => "QF_NRA",
        }
    }

    /// Parse logic from string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_uppercase().as_str() {
            "QF_UFLIA" => Ok(Logic::QF_UFLIA),
            "QF_BV" => Ok(Logic::QF_BV),
            "QF_AUFLIA" => Ok(Logic::QF_AUFLIA),
            "AUFLIA" => Ok(Logic::AUFLIA),
            "AUFLIRA" => Ok(Logic::AUFLIRA),
            "QF_NRA" => Ok(Logic::QF_NRA),
            other => Err(format!(
                "Unknown SMT logic: {}. Use QF_UFLIA, QF_BV, QF_AUFLIA, AUFLIA, AUFLIRA, or QF_NRA",
                other
            )),
        }
    }
}

/// Solver configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverConfig {
    /// SMT logic to use.
    pub logic: Logic,
    /// Maximum solving time per query.
    pub timeout: Duration,
    /// Maximum memory in MB (advisory; enforced via process limits where possible).
    pub memory_mb: usize,
    /// Enable incremental solving (push/pop).
    pub incremental: bool,
    /// Request model generation for SAT results.
    pub produce_models: bool,
    /// Request unsat core generation for UNSAT results.
    pub produce_unsat_cores: bool,
    /// Random seed for Z3 (for reproducibility).
    pub seed: Option<u64>,
    /// Number of parallel solver threads (0 = auto).
    pub threads: usize,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            logic: Logic::default(),
            timeout: Duration::from_secs(30),
            memory_mb: 4096,
            incremental: true,
            produce_models: true,
            produce_unsat_cores: true,
            seed: Some(0xDEADBEEF), // Fixed seed for reproducibility
            threads: 0,
        }
    }
}

impl SolverConfig {
    /// Create a fast config for quick checks (short timeout, no models).
    pub fn fast() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            produce_models: false,
            produce_unsat_cores: false,
            ..Default::default()
        }
    }

    /// Create a thorough config for deep verification (long timeout, all features).
    pub fn thorough() -> Self {
        Self {
            timeout: Duration::from_secs(300),
            memory_mb: 16384,
            ..Default::default()
        }
    }

    /// Create config for quantifier-heavy problems (AUFLIA logic).
    pub fn with_quantifiers() -> Self {
        Self {
            logic: Logic::AUFLIA,
            incremental: false, // Quantifiers often don't work well with incremental
            ..Default::default()
        }
    }

    /// Create config for bitvector problems (QF_BV logic).
    pub fn with_bitvectors() -> Self {
        Self {
            logic: Logic::QF_BV,
            ..Default::default()
        }
    }
}
