//! TCK Test Categories

use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestCategory {
    Syntax,
    Types,
    Semantics,
    Stdlib,
    Quantum,
    Effects,
    Verification,
    Editions,
}

impl TestCategory {
    pub fn all() -> Vec<Self> {
        vec![
            TestCategory::Syntax,
            TestCategory::Types,
            TestCategory::Semantics,
            TestCategory::Stdlib,
            TestCategory::Quantum,
            TestCategory::Effects,
            TestCategory::Verification,
            TestCategory::Editions,
        ]
    }

    pub fn path(&self) -> &'static str {
        match self {
            TestCategory::Syntax => "syntax",
            TestCategory::Types => "types",
            TestCategory::Semantics => "semantics",
            TestCategory::Stdlib => "stdlib",
            TestCategory::Quantum => "quantum",
            TestCategory::Effects => "effects",
            TestCategory::Verification => "verification",
            TestCategory::Editions => "editions",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            TestCategory::Syntax => "Syntax conformance (valid/invalid programs)",
            TestCategory::Types => "Type system conformance (QTT, quantities, effects, dependent types)",
            TestCategory::Semantics => "Dynamic semantics (evaluation, quantum, concurrency, effects)",
            TestCategory::Stdlib => "Standard library conformance",
            TestCategory::Quantum => "Quantum operations and algorithms",
            TestCategory::Effects => "Algebraic effect system",
            TestCategory::Verification => "Formal verification (SMT, Lean 4)",
            TestCategory::Editions => "Edition system (2024, 2027, 2030)",
        }
    }

    pub fn print_all() {
        println!("{}", "TCK Test Categories".bold().cyan());
        for cat in Self::all() {
            println!("  {:<12} - {}", cat.path().bold(), cat.description());
        }
    }

    pub fn subcategories(&self) -> Vec<&'static str> {
        match self {
            TestCategory::Syntax => vec!["valid", "invalid"],
            TestCategory::Types => vec!["linear", "affine", "unrestricted", "quantities", "effects", "dependent", "erasure"],
            TestCategory::Semantics => vec!["basic", "quantum", "concurrency", "effects"],
            TestCategory::Stdlib => vec!["core", "quantum", "edition_gated"],
            TestCategory::Quantum => vec!["primitives", "variational", "qec", "hybrid", "benchmarks"],
            TestCategory::Effects => vec!["builtin", "handlers", "polymorphism", "stacking"],
            TestCategory::Verification => vec!["smt", "lean"],
            TestCategory::Editions => vec!["2024", "2027", "2030"],
        }
    }
}

impl fmt::Display for TestCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path())
    }
}

impl std::str::FromStr for TestCategory {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "syntax" => Ok(TestCategory::Syntax),
            "types" => Ok(TestCategory::Types),
            "semantics" => Ok(TestCategory::Semantics),
            "stdlib" => Ok(TestCategory::Stdlib),
            "quantum" => Ok(TestCategory::Quantum),
            "effects" => Ok(TestCategory::Effects),
            "verification" => Ok(TestCategory::Verification),
            "editions" => Ok(TestCategory::Editions),
            _ => Err(format!("Unknown test category: {}", s)),
        }
    }
}