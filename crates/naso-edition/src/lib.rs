//! Edition System for Naso Language
//!
//! This crate implements the edition system for Naso, managing language editions
//! (2024, 2027, 2030) and providing migration tooling.
//!
//! This is part of Phase 30: Language Stabilization & Standardization.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use colored::Colorize;
use clap::Parser;
use walkdir::WalkDir;

#[derive(Error, Debug)]
pub enum EditionError {
    #[error("Unknown edition: {0}")]
    UnknownEdition(String),
    #[error("Edition not supported: {0}")]
    NotSupported(String),
    #[error("Migration failed: {0}")]
    MigrationFailed(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("TOML error: {0}")]
    TomlError(#[from] toml::de::Error),
    #[error("TOML serialize error: {0}")]
    TomlSerError(#[from] toml::ser::Error),
    #[error("Walkdir error: {0}")]
    WalkdirError(#[from] walkdir::Error),
}

/// Edition metadata and feature gates
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EditionInfo {
    pub year: u32,
    pub name: String,
    pub status: EditionStatus,
    pub features: Vec<EditionFeature>,
    pub deprecated: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EditionStatus {
    Stable,
    Preview,
    Planned,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EditionFeature {
    pub name: String,
    pub description: String,
    pub gate: String,  // cfg flag name
    pub since: u32,
    pub unstable: bool,
}

/// Edition manager
pub struct EditionManager {
    editions: HashMap<u32, EditionInfo>,
    current_edition: u32,
}

impl EditionManager {
    pub fn new() -> Self {
        let mut manager = Self {
            editions: HashMap::new(),
            current_edition: 2024,
        };
        manager.register_standard_editions();
        manager
    }

    fn register_standard_editions(&mut self) {
        // Edition 2024 - Baseline
        self.editions.insert(2024, EditionInfo {
            year: 2024,
            name: "2024".to_string(),
            status: EditionStatus::Stable,
            features: vec![
                EditionFeature {
                    name: "qtt".to_string(),
                    description: "Quantity-aware Type Theory with linear/affine/unrestricted quantities".to_string(),
                    gate: "feature=qtt".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "effects".to_string(),
                    description: "Algebraic effects (Async, Try, Gen, IO, Quantum, State, NonDet)".to_string(),
                    gate: "feature=effects".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "quantum".to_string(),
                    description: "Native quantum primitives (qubit, gate, circuit, measure)".to_string(),
                    gate: "feature=quantum".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "qec".to_string(),
                    description: "Quantum error correction types (Surface, Repetition, Steane, Color codes)".to_string(),
                    gate: "feature=qec".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "variational".to_string(),
                    description: "Variational quantum algorithms (VQE, QAOA)".to_string(),
                    gate: "feature=variational".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "verification".to_string(),
                    description: "Formal verification (SMT + Lean 4)".to_string(),
                    gate: "feature=verification".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "dependent_types".to_string(),
                    description: "Dependent types with erasure (Π, Σ, Prop)".to_string(),
                    gate: "feature=dependent-types".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "distributed_compilation".to_string(),
                    description: "Distributed build infrastructure".to_string(),
                    gate: "feature=distributed".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "wasm_gc".to_string(),
                    description: "WebAssembly GC and Component Model".to_string(),
                    gate: "feature=wasm-gc".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "ml_optimization".to_string(),
                    description: "ML-guided compiler optimization".to_string(),
                    gate: "feature=ml-opt".to_string(),
                    since: 2024,
                    unstable: false,
                },
                EditionFeature {
                    name: "self_hosting".to_string(),
                    description: "Self-hosting verified compiler".to_string(),
                    gate: "feature=self-hosting".to_string(),
                    since: 2024,
                    unstable: false,
                },
            ],
            deprecated: vec![],
            removed: vec![],
        });

        // Edition 2027 - Enhanced
        self.editions.insert(2027, EditionInfo {
            year: 2027,
            name: "2027".to_string(),
            status: EditionStatus::Planned,
            features: vec![
                EditionFeature {
                    name: "enhanced_dependent_types".to_string(),
                    description: "Improved dependent type ergonomics, better inference".to_string(),
                    gate: "feature=enhanced-dependent-types".to_string(),
                    since: 2027,
                    unstable: true,
                },
                EditionFeature {
                    name: "implicit_effects".to_string(),
                    description: "Effect inference without explicit annotations".to_string(),
                    gate: "feature=implicit-effects".to_string(),
                    since: 2027,
                    unstable: true,
                },
                EditionFeature {
                    name: "pattern_types".to_string(),
                    description: "Type patterns in match expressions".to_string(),
                    gate: "feature=pattern-types".to_string(),
                    since: 2027,
                    unstable: true,
                },
                EditionFeature {
                    name: "const_generics_v2".to_string(),
                    description: "Enhanced const generics with arbitrary expressions".to_string(),
                    gate: "feature=const-generics-v2".to_string(),
                    since: 2027,
                    unstable: true,
                },
                EditionFeature {
                    name: "async_trait".to_string(),
                    description: "Native async trait support".to_string(),
                    gate: "feature=async-trait".to_string(),
                    since: 2027,
                    unstable: true,
                },
                EditionFeature {
                    name: "stdlib_v2".to_string(),
                    description: "Redesigned standard library with better ergonomics".to_string(),
                    gate: "feature=stdlib-v2".to_string(),
                    since: 2027,
                    unstable: true,
                },
                EditionFeature {
                    name: "macro_rules_v2".to_string(),
                    description: "Improved macro system with better hygiene".to_string(),
                    gate: "feature=macro-rules-v2".to_string(),
                    since: 2027,
                    unstable: true,
                },
            ],
            deprecated: vec![
                "explicit_effect_annotations".to_string(),
                "legacy_const_generics".to_string(),
            ],
            removed: vec![],
        });

        // Edition 2030 - Future
        self.editions.insert(2030, EditionInfo {
            year: 2030,
            name: "2030".to_string(),
            status: EditionStatus::Planned,
            features: vec![
                EditionFeature {
                    name: "ai_assisted_compilation".to_string(),
                    description: "AI-guided optimization and code generation".to_string(),
                    gate: "feature=ai-compilation".to_string(),
                    since: 2030,
                    unstable: true,
                },
                EditionFeature {
                    name: "advanced_quantum".to_string(),
                    description: "Fault-tolerant quantum computing primitives".to_string(),
                    gate: "feature=advanced-quantum".to_string(),
                    since: 2030,
                    unstable: true,
                },
                EditionFeature {
                    name: "formal_stdlib".to_string(),
                    description: "Fully verified standard library in Lean 4".to_string(),
                    gate: "feature=formal-stdlib".to_string(),
                    since: 2030,
                    unstable: true,
                },
                EditionFeature {
                    name: "effect_rows_in_types".to_string(),
                    description: "Effect rows as first-class type components".to_string(),
                    gate: "feature=effect-rows-in-types".to_string(),
                    since: 2030,
                    unstable: true,
                },
                EditionFeature {
                    name: "linear_logic_v2".to_string(),
                    description: "Enhanced linear logic with session types".to_string(),
                    gate: "feature=linear-logic-v2".to_string(),
                    since: 2030,
                    unstable: true,
                },
            ],
            deprecated: vec![],
            removed: vec![],
        });
    }

    /// Get edition info
    pub fn get_edition(&self, year: u32) -> Option<&EditionInfo> {
        self.editions.get(&year)
    }

    /// List all editions
    pub fn list_editions(&self) -> Vec<&EditionInfo> {
        let mut editions: Vec<_> = self.editions.values().collect();
        editions.sort_by_key(|e| e.year);
        editions
    }

    /// Check if edition is supported
    pub fn is_supported(&self, year: u32) -> bool {
        self.editions.get(&year).map(|e| e.status != EditionStatus::Planned).unwrap_or(false)
    }

    /// Get current edition
    pub fn current_edition(&self) -> u32 {
        self.current_edition
    }

    /// Set current edition
    pub fn set_current_edition(&mut self, year: u32) -> Result<(), EditionError> {
        if !self.editions.contains_key(&year) {
            return Err(EditionError::UnknownEdition(year.to_string()));
        }
        self.current_edition = year;
        Ok(())
    }

    /// Check if feature is available in edition
    pub fn has_feature(&self, year: u32, feature: &str) -> bool {
        self.editions.get(&year)
            .map(|e| e.features.iter().any(|f| f.name == feature))
            .unwrap_or(false)
    }

    /// Get feature gate for edition
    pub fn feature_gate(&self, year: u32, feature: &str) -> Option<String> {
        self.editions.get(&year)
            .and_then(|e| e.features.iter().find(|f| f.name == feature))
            .map(|f| f.gate.clone())
    }

    /// Get deprecated features in edition
    pub fn deprecated_features(&self, year: u32) -> Vec<String> {
        self.editions.get(&year)
            .map(|e| e.deprecated.clone())
            .unwrap_or_default()
    }

    /// Get removed features in edition
    pub fn removed_features(&self, year: u32) -> Vec<String> {
        self.editions.get(&year)
            .map(|e| e.removed.clone())
            .unwrap_or_default()
    }

    /// Generate Cargo.toml edition configuration
    pub fn generate_cargo_config(&self, year: u32) -> Result<String, EditionError> {
        let edition = self.get_edition(year)
            .ok_or_else(|| EditionError::UnknownEdition(year.to_string()))?;

        let mut config = String::new();
        config.push_str(&format!("# Naso Edition {} Configuration\n", year));
        config.push_str(&format!("edition = \"{}\"\n\n", year));

        config.push_str("[features]\n");
        for feature in &edition.features {
            if !feature.unstable {
                config.push_str(&format!("{} = []\n", feature.gate));
            }
        }

        config.push_str("\n# Unstable features (require explicit opt-in)\n");
        for feature in &edition.features {
            if feature.unstable {
                config.push_str(&format!("# {} = []  # {}\n", feature.gate, feature.description));
            }
        }

        if !edition.deprecated.is_empty() {
            config.push_str("\n# Deprecated features\n");
            for dep in &edition.deprecated {
                config.push_str(&format!("# deprecated: {}\n", dep));
            }
        }

        Ok(config)
    }
}

/// Migration tool for edition upgrades
pub struct MigrationTool {
    manager: EditionManager,
}

impl MigrationTool {
    pub fn new() -> Self {
        Self {
            manager: EditionManager::new(),
        }
    }

    /// Migrate a crate from one edition to another
    pub fn migrate_crate(
        &self,
        crate_path: &Path,
        from: u32,
        to: u32,
    ) -> Result<MigrationReport, EditionError> {
        if from >= to {
            return Err(EditionError::MigrationFailed("Target edition must be newer".into()));
        }

        let from_edition = self.manager.get_edition(from)
            .ok_or_else(|| EditionError::UnknownEdition(from.to_string()))?;
        let to_edition = self.manager.get_edition(to)
            .ok_or_else(|| EditionError::UnknownEdition(to.to_string()))?;

        let mut report = MigrationReport {
            crate_path: crate_path.to_path_buf(),
            from_edition: from,
            to_edition: to,
            changes: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };

        // Read Cargo.toml
        let cargo_toml = crate_path.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml)?;
            let mut cargo: toml::Value = content.parse()
                .map_err(|e: toml::de::Error| EditionError::ConfigError(e.to_string()))?;

            // Update edition field
            if let Some(package) = cargo.get_mut("package") {
                if let Some(edition_field) = package.get_mut("edition") {
                    *edition_field = toml::Value::String(to.to_string());
                    report.changes.push(MigrationChange {
                        file: "Cargo.toml".to_string(),
                        change_type: ChangeType::EditionUpdate,
                        description: format!("Updated edition from {} to {}", from, to),
                        old_value: Some(from.to_string()),
                        new_value: Some(to.to_string()),
                    });
                }
            }

            // Handle feature migrations
            self.migrate_features(&mut cargo, from_edition, to_edition, &mut report)?;

            // Write back
            let new_content = toml::to_string_pretty(&cargo)
                .map_err(|e| EditionError::ConfigError(e.to_string()))?;
            std::fs::write(&cargo_toml, new_content)?;
        }

        // Migrate source files
        self.migrate_sources(crate_path, from_edition, to_edition, &mut report)?;

        Ok(report)
    }

    fn migrate_features(
        &self,
        cargo: &mut toml::Value,
        from: &EditionInfo,
        to: &EditionInfo,
        report: &mut MigrationReport,
    ) -> Result<(), EditionError> {
        // Check for deprecated features
        for dep in &from.deprecated {
            if let Some(features) = cargo.get_mut("features") {
                if let Some(table) = features.as_table_mut() {
                    if table.remove(dep).is_some() {
                        report.changes.push(MigrationChange {
                            file: "Cargo.toml".to_string(),
                            change_type: ChangeType::FeatureRemoved,
                            description: format!("Removed deprecated feature: {}", dep),
                            old_value: Some(dep.clone()),
                            new_value: None,
                        });
                        report.warnings.push(format!("Feature '{}' was deprecated in edition {}", dep, to.year));
                    }
                }
            }
        }

        // Check for removed features
        for removed in &from.removed {
            if let Some(features) = cargo.get_mut("features") {
                if let Some(table) = features.as_table_mut() {
                    if table.remove(removed).is_some() {
                        report.errors.push(format!("Feature '{}' was removed in edition {}", removed, to.year));
                    }
                }
            }
        }

        Ok(())
    }

    fn migrate_sources(
        &self,
        crate_path: &Path,
        from: &EditionInfo,
        to: &EditionInfo,
        report: &mut MigrationReport,
    ) -> Result<(), EditionError> {
        // Walk source files
        for entry in walkdir::WalkDir::new(crate_path.join("src")) {
            let entry = entry?;
            if entry.path().extension().map_or(false, |ext| ext == "naso" || ext == "rs") {
                let content = std::fs::read_to_string(entry.path())?;
                let mut new_content = content.clone();
                let mut changed = false;

                // Update edition declaration
                if let Some(pos) = new_content.find("edition ") {
                    let line_end = new_content[pos..].find('\n').unwrap_or(new_content.len() - pos);
                    let line = &new_content[pos..pos + line_end];
                    if line.contains(&from.year.to_string()) {
                        new_content = new_content.replacen(&from.year.to_string(), &to.year.to_string(), 1);
                        changed = true;
                        report.changes.push(MigrationChange {
                            file: entry.path().strip_prefix(crate_path).unwrap().to_string_lossy().to_string(),
                            change_type: ChangeType::EditionUpdate,
                            description: format!("Updated edition declaration from {} to {}", from.year, to.year),
                            old_value: Some(from.year.to_string()),
                            new_value: Some(to.year.to_string()),
                        });
                    }
                }

                // Handle deprecated syntax
                for dep in &from.deprecated {
                    if new_content.contains(dep) {
                        report.warnings.push(format!(
                            "File {} uses deprecated feature '{}'",
                            entry.path().strip_prefix(crate_path).unwrap().display(),
                            dep
                        ));
                    }
                }

                // Handle new feature gates
                for feature in &to.features {
                    if feature.unstable && new_content.contains(&feature.gate) {
                        report.warnings.push(format!(
                            "File {} uses unstable feature '{}' (requires explicit opt-in)",
                            entry.path().strip_prefix(crate_path).unwrap().display(),
                            feature.name
                        ));
                    }
                }

                if changed {
                    std::fs::write(entry.path(), new_content)?;
                }
            }
        }

        Ok(())
    }
}

/// Migration report
#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub crate_path: PathBuf,
    pub from_edition: u32,
    pub to_edition: u32,
    pub changes: Vec<MigrationChange>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MigrationChange {
    pub file: String,
    pub change_type: ChangeType,
    pub description: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    EditionUpdate,
    FeatureRemoved,
    FeatureAdded,
    SyntaxChange,
    DeprecationWarning,
}

/// Edition-aware compilation configuration
#[derive(Debug, Clone)]
pub struct EditionConfig {
    pub edition: u32,
    pub features: Vec<String>,
    pub allow_unstable: bool,
    pub warn_deprecated: bool,
}

impl EditionConfig {
    pub fn new(edition: u32) -> Self {
        Self {
            edition,
            features: Vec::new(),
            allow_unstable: false,
            warn_deprecated: true,
        }
    }

    pub fn with_feature(mut self, feature: String) -> Self {
        self.features.push(feature);
        self
    }

    pub fn with_unstable(mut self, allow: bool) -> Self {
        self.allow_unstable = allow;
        self
    }

    pub fn check_feature(&self, manager: &EditionManager, feature: &str) -> Result<(), EditionError> {
        let edition = manager.get_edition(self.edition)
            .ok_or_else(|| EditionError::UnknownEdition(self.edition.to_string()))?;

        let feat = edition.features.iter().find(|f| f.name == feature || f.gate == feature)
            .ok_or_else(|| EditionError::NotSupported(format!("Feature '{}' not in edition {}", feature, self.edition)))?;

        if feat.unstable && !self.allow_unstable {
            return Err(EditionError::NotSupported(format!(
                "Feature '{}' is unstable in edition {}. Use --unstable-features to enable.",
                feature, self.edition
            )));
        }

        Ok(())
    }

    pub fn get_compiler_flags(&self, manager: &EditionManager) -> Vec<String> {
        let mut flags = vec![format!("-Zedition={}", self.edition)];

        for feature in &self.features {
            if let Some(gate) = manager.feature_gate(self.edition, feature) {
                flags.push(format!("--cfg={}", gate));
            }
        }

        if self.allow_unstable {
            flags.push("--cfg=naso_unstable".to_string());
        }

        flags
    }
}

/// CLI for edition management
#[derive(clap::Parser)]
#[command(name = "naso-edition", about = "Naso edition management")]
pub struct EditionCli {
    #[command(subcommand)]
    command: EditionCommand,
}

#[derive(clap::Subcommand)]
enum EditionCommand {
    /// List available editions
    List,
    /// Show edition details
    Show { year: u32 },
    /// Migrate crate to new edition
    Migrate {
        path: PathBuf,
        from: u32,
        to: u32,
    },
    /// Check edition compatibility
    Check { path: PathBuf },
    /// Generate Cargo.toml config for edition
    Config { year: u32 },
}

impl EditionCli {
    pub fn run() -> Result<(), EditionError> {
        let cli = Self::parse();
        let manager = EditionManager::new();

        match cli.command {
            EditionCommand::List => {
                println!("{}", "Available Naso Editions:".bold().cyan());
                for edition in manager.list_editions() {
                    let status = match edition.status {
                        EditionStatus::Stable => "Stable".green(),
                        EditionStatus::Preview => "Preview".yellow(),
                        EditionStatus::Planned => "Planned".blue(),
                    };
                    println!("  {} ({}) - {}", edition.year.to_string().bold(), status, edition.name);
                    for feat in &edition.features {
                        let unstable = if feat.unstable { " [unstable]" } else { "" };
                        println!("    • {}: {}{}", feat.name, feat.description, unstable);
                    }
                }
            }
            EditionCommand::Show { year } => {
                if let Some(edition) = manager.get_edition(year) {
                    println!("{}", format!("Edition {} ({})", year, edition.name).bold().cyan());
                    println!("Status: {:?}", edition.status);
                    println!("\nFeatures:");
                    for feat in &edition.features {
                        let unstable = if feat.unstable { " [unstable]" } else { "" };
                        println!("  • {} ({}): {}{}", feat.name, feat.gate, feat.description, unstable);
                    }
                    if !edition.deprecated.is_empty() {
                        println!("\nDeprecated:");
                        for dep in &edition.deprecated {
                            println!("  • {}", dep);
                        }
                    }
                    if !edition.removed.is_empty() {
                        println!("\nRemoved:");
                        for rem in &edition.removed {
                            println!("  • {}", rem);
                        }
                    }
                } else {
                    return Err(EditionError::UnknownEdition(year.to_string()));
                }
            }
            EditionCommand::Migrate { path, from, to } => {
                let tool = MigrationTool::new();
                let report = tool.migrate_crate(&path, from, to)?;
                println!("{}", format!("Migration from {} to {}", from, to).bold().cyan());
                println!("Changes: {}", report.changes.len());
                for change in &report.changes {
                    println!("  • {}: {}", change.file, change.description);
                }
                if !report.warnings.is_empty() {
                    println!("\nWarnings:");
                    for w in &report.warnings {
                        println!("  ⚠ {}", w.yellow());
                    }
                }
                if !report.errors.is_empty() {
                    println!("\nErrors:");
                    for e in &report.errors {
                        println!("  ✗ {}", e.red());
                    }
                }
            }
            EditionCommand::Check { path } => {
                let cargo_toml = path.join("Cargo.toml");
                if cargo_toml.exists() {
                    let content = std::fs::read_to_string(cargo_toml)?;
                    let cargo: toml::Value = content.parse()?;
                    if let Some(package) = cargo.get("package") {
                        if let Some(edition) = package.get("edition") {
                            println!("Crate edition: {}", edition);
                        }
                    }
                }
            }
            EditionCommand::Config { year } => {
                let config = manager.generate_cargo_config(year)?;
                println!("{}", config);
            }
        }

        Ok(())
    }
}

impl Default for EditionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for MigrationTool {
    fn default() -> Self {
        Self::new()
    }
}