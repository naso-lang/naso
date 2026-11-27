//! TCK Test Harness Infrastructure

use anyhow::{Context, Result};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use walkdir::WalkDir;

use crate::results::{TestResult, TestResults, TestStatus, OutputFormat};
use crate::categories::TestCategory;

/// Test configuration
#[derive(Debug, Clone)]
pub struct TestConfig {
    pub impl_path: PathBuf,
    pub category: Option<TestCategory>,
    pub jobs: usize,
    pub verbose: bool,
    pub output_format: OutputFormat,
    pub filter: Option<String>,
    pub list_only: bool,
}

/// Naso implementation under test
#[derive(Debug, Clone, Serialize)]
pub struct Implementation {
    pub path: PathBuf,
    pub version: String,
    pub edition_support: Vec<String>,
}

/// Parsed test metadata from .naso file header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestMetadata {
    pub name: String,
    pub category: String,
    pub expect: TestExpectation,
    pub edition: String,
    pub description: String,
    pub ignore: bool,
    pub ignore_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestExpectation {
    CompileSuccess,
    CompileError,
    RuntimeSuccess,
    RuntimeError,
    VerificationPass,
    VerificationFail,
}

/// A single test case
#[derive(Debug, Clone)]
pub struct TestCase {
    pub path: PathBuf,
    pub metadata: TestMetadata,
    pub content: String,
}

/// Test harness for running TCK tests
pub struct TestHarness {
    config: TestConfig,
    implementation: Implementation,
    test_cases: Vec<TestCase>,
    filter_regex: Option<Regex>,
}

impl TestHarness {
    pub fn new(config: TestConfig) -> Result<Self> {
        // Verify implementation exists
        let impl_path = config.impl_path.clone();
        if !impl_path.exists() {
            // Try to find in PATH
            let output = Command::new("which")
                .arg(&config.impl_path)
                .output()
                .context("Failed to check implementation path")?;
            if !output.status.success() {
                anyhow::bail!("Implementation not found: {}", config.impl_path.display());
            }
        }

        // Get implementation version
        let version = Self::get_version(&impl_path)?;
        let edition_support = Self::get_edition_support(&impl_path)?;

        let implementation = Implementation {
            path: impl_path,
            version,
            edition_support,
        };

        // Discover test cases
        let test_cases = Self::discover_tests(&config)?;

        // Build filter regex
        let filter_regex: Option<Regex> = None; // Created in discover_tests

        Ok(Self {
            config,
            implementation,
            test_cases,
            filter_regex,
        })
    }

    fn get_version(impl_path: &Path) -> Result<String> {
        let output = Command::new(impl_path)
            .arg("--version")
            .output()
            .context("Failed to get version")?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn get_edition_support(impl_path: &Path) -> Result<Vec<String>> {
        let output = Command::new(impl_path)
            .arg("--editions")
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect());
            }
        }
        Ok(vec!["2024".to_string()]) // Default
    }

    fn discover_tests(config: &TestConfig) -> Result<Vec<TestCase>> {
        let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut cases = Vec::new();

        let categories = match config.category {
            Some(cat) => vec![cat],
            None => TestCategory::all(),
        };

        // Build filter regex
        let filter_regex: Option<Regex> = None; // Created in discover_tests

        for category in categories {
            let cat_path = test_root.join(category.path());
            if !cat_path.exists() {
                continue;
            }

            for entry in WalkDir::new(&cat_path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "naso"))
            {
                let content = std::fs::read_to_string(entry.path())
                    .context(format!("Failed to read test: {}", entry.path().display()))?;
                let metadata = Self::parse_metadata(&content, entry.path(), &category)?;
                cases.push(TestCase {
                    path: entry.path().to_path_buf(),
                    metadata,
                    content,
                });
            }
        }

        // Apply filter
        if let Some(regex) = &filter_regex {
            cases.retain(|c| regex.is_match(&c.metadata.name));
        }

        Ok(cases)
    }

    fn parse_metadata(content: &str, path: &Path, category: &TestCategory) -> Result<TestMetadata> {
        let lines: Vec<&str> = content.lines().take(20).collect();
        let mut metadata = TestMetadata {
            name: path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string(),
            category: category.path().to_string(),
            expect: TestExpectation::CompileSuccess,
            edition: "2024".to_string(),
            description: String::new(),
            ignore: false,
            ignore_reason: None,
        };

        for line in lines {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("// TEST:") {
                metadata.name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("// CATEGORY:") {
                metadata.category = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("// EXPECT:") {
                metadata.expect = match rest.trim() {
                    "compile_success" => TestExpectation::CompileSuccess,
                    "compile_error" => TestExpectation::CompileError,
                    "runtime_success" => TestExpectation::RuntimeSuccess,
                    "runtime_error" => TestExpectation::RuntimeError,
                    "verification_pass" => TestExpectation::VerificationPass,
                    "verification_fail" => TestExpectation::VerificationFail,
                    other => anyhow::bail!("Unknown expectation: {}", other),
                };
            } else if let Some(rest) = line.strip_prefix("// EDITION:") {
                metadata.edition = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("// DESCRIPTION:") {
                metadata.description = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("// IGNORE:") {
                metadata.ignore = true;
                metadata.ignore_reason = Some(rest.trim().to_string());
            }
        }

        Ok(metadata)
    }

    pub fn run_all(&mut self) -> Result<TestResults> {
        if self.config.list_only {
            self.list_tests()?;
            return Ok(TestResults::empty());
        }

        println!("{}", "Naso TCK Conformance Test Suite".bold().cyan());
        println!("Implementation: {} ({})", self.implementation.path.display(), self.implementation.version);
        println!("Editions supported: {}", self.implementation.edition_support.join(", "));
        println!("Tests discovered: {}", self.test_cases.len());
        println!("Jobs: {}", self.config.jobs);
        println!();

        let pb = ProgressBar::new(self.test_cases.len() as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})").unwrap()
            .progress_chars("##-"));

        let passed = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));
        let skipped = Arc::new(AtomicUsize::new(0));

        let results: Vec<TestResult> = self.test_cases.par_iter()
            .map(|test| {
                let result = self.run_test(test);
                match &result.status {
                    TestStatus::Passed => passed.fetch_add(1, Ordering::Relaxed),
                    TestStatus::Failed => failed.fetch_add(1, Ordering::Relaxed),
                    TestStatus::Skipped => skipped.fetch_add(1, Ordering::Relaxed),
                };
                pb.inc(1);
                result
            })
            .collect();

        pb.finish_with_message("Done");

        let results = TestResults {
            implementation: self.implementation.clone(),
            results,
            passed: passed.load(Ordering::Relaxed),
            failed: failed.load(Ordering::Relaxed),
            skipped: skipped.load(Ordering::Relaxed),
            total: self.test_cases.len(),
        };

        Ok(results)
    }

    fn run_test(&self, test: &TestCase) -> TestResult {
        let start = std::time::Instant::now();

        // Check if test should be skipped
        if test.metadata.ignore {
            return TestResult {
                name: test.metadata.name.clone(),
                category: test.metadata.category.clone(),
                status: TestStatus::Skipped,
                duration: start.elapsed(),
                message: test.metadata.ignore_reason.clone().unwrap_or_default(),
                ..Default::default()
            };
        }

        // Check edition support
        if !self.implementation.edition_support.contains(&test.metadata.edition) {
            return TestResult {
                name: test.metadata.name.clone(),
                category: test.metadata.category.clone(),
                status: TestStatus::Skipped,
                duration: start.elapsed(),
                message: format!("Edition {} not supported by implementation", test.metadata.edition),
                ..Default::default()
            };
        }

        // Run the appropriate test based on expectation
        let result = match test.metadata.expect {
            TestExpectation::CompileSuccess | TestExpectation::CompileError => {
                self.test_compile(test)
            }
            TestExpectation::RuntimeSuccess | TestExpectation::RuntimeError => {
                self.test_runtime(test)
            }
            TestExpectation::VerificationPass | TestExpectation::VerificationFail => {
                self.test_verification(test)
            }
        };

        TestResult {
            name: test.metadata.name.clone(),
            category: test.metadata.category.clone(),
            duration: start.elapsed(),
            ..result
        }
    }

    fn test_compile(&self, test: &TestCase) -> TestResult {
        // Write test to temp file
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test.naso");
        std::fs::write(&test_file, &test.content).unwrap();

        // Run naso check
        let output = Command::new(&self.implementation.path)
            .args(["check", test_file.to_str().unwrap()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(out) => {
                let success = out.status.success();
                let expected_success = matches!(test.metadata.expect, TestExpectation::CompileSuccess);

                if success == expected_success {
                    TestResult {
                        status: TestStatus::Passed,
                        message: "Compile result matches expectation".to_string(),
                        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        exit_code: out.status.code(),
                        ..Default::default()
                    }
                } else {
                    TestResult {
                        status: TestStatus::Failed,
                        message: format!(
                            "Compile mismatch: expected {}, got {}",
                            if expected_success { "success" } else { "failure" },
                            if success { "success" } else { "failure" }
                        ),
                        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        exit_code: out.status.code(),
                        ..Default::default()
                    }
                }
            }
            Err(e) => TestResult {
                status: TestStatus::Failed,
                message: format!("Failed to run compiler: {}", e),
                ..Default::default()
            }
        }
    }

    fn test_runtime(&self, test: &TestCase) -> TestResult {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test.naso");
        std::fs::write(&test_file, &test.content).unwrap();

        // Build first
        let build_out = Command::new(&self.implementation.path)
            .args(["build", test_file.to_str().unwrap(), "-o", "test_bin"])
            .current_dir(temp_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        let build_out = match build_out {
            Ok(out) => out,
            Err(e) => {
                return TestResult {
                    status: TestStatus::Failed,
                    message: format!("Build failed: {}", e),
                    ..Default::default()
                };
            }
        };

        if !build_out.status.success() {
            return TestResult {
                status: TestStatus::Failed,
                message: "Build failed unexpectedly".to_string(),
                stdout: String::from_utf8_lossy(&build_out.stdout).to_string(),
                stderr: String::from_utf8_lossy(&build_out.stderr).to_string(),
                exit_code: build_out.status.code(),
                ..Default::default()
            };
        }

        // Run the binary
        let binary_path = temp_dir.path().join("test_bin");
        let run_out = Command::new(&binary_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match run_out {
            Ok(out) => {
                let success = out.status.success();
                let expected_success = matches!(test.metadata.expect, TestExpectation::RuntimeSuccess);

                // Check expected output if .expected file exists
                let expected_file = test.path.with_extension("expected");
                let output_match = if expected_file.exists() {
                    let expected = std::fs::read_to_string(&expected_file).unwrap_or_default();
                    let actual_stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    Self::compare_output(&expected, &actual_stdout)
                } else {
                    true // No expected file, just check exit code
                };

                if success == expected_success && output_match {
                    TestResult {
                        status: TestStatus::Passed,
                        message: "Runtime result matches expectation".to_string(),
                        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        exit_code: out.status.code(),
                        ..Default::default()
                    }
                } else {
                    TestResult {
                        status: TestStatus::Failed,
                        message: format!(
                            "Runtime mismatch: expected {}, got {} (output_match: {})",
                            if expected_success { "success" } else { "failure" },
                            if success { "success" } else { "failure" },
                            output_match
                        ),
                        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        exit_code: out.status.code(),
                        ..Default::default()
                    }
                }
            }
            Err(e) => TestResult {
                status: TestStatus::Failed,
                message: format!("Failed to run binary: {}", e),
                ..Default::default()
            }
        }
    }

    fn test_verification(&self, test: &TestCase) -> TestResult {
        let temp_dir = tempfile::tempdir().unwrap();
        let test_file = temp_dir.path().join("test.naso");
        std::fs::write(&test_file, &test.content).unwrap();

        // Run naso verify
        let output = Command::new(&self.implementation.path)
            .args(["verify", test_file.to_str().unwrap()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(out) => {
                let success = out.status.success();
                let expected_pass = matches!(test.metadata.expect, TestExpectation::VerificationPass);

                if success == expected_pass {
                    TestResult {
                        status: TestStatus::Passed,
                        message: "Verification result matches expectation".to_string(),
                        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        exit_code: out.status.code(),
                        ..Default::default()
                    }
                } else {
                    TestResult {
                        status: TestStatus::Failed,
                        message: format!(
                            "Verification mismatch: expected {}, got {}",
                            if expected_pass { "pass" } else { "fail" },
                            if success { "pass" } else { "fail" }
                        ),
                        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        exit_code: out.status.code(),
                        ..Default::default()
                    }
                }
            }
            Err(e) => TestResult {
                status: TestStatus::Failed,
                message: format!("Failed to run verifier: {}", e),
                ..Default::default()
            }
        }
    }

    fn compare_output(expected: &str, actual: &str) -> bool {
        // Parse expected format
        for line in expected.lines() {
            if let Some((key, value)) = line.split_once(':') {
                match key.trim() {
                    "stdout" => {
                        let expected_stdout = value.trim().trim_matches('"');
                        if !actual.contains(expected_stdout) {
                            return false;
                        }
                    }
                    "exit_code" => {
                        // Would need actual exit code
                    }
                    _ => {}
                }
            }
        }
        true
    }

    pub fn list_tests(&self) -> Result<()> {
        println!("{}", "Available TCK Tests".bold().cyan());
        println!("Implementation: {} ({})", self.implementation.path.display(), self.implementation.version);
        println!();

        let mut by_category: HashMap<String, Vec<&TestCase>> = HashMap::new();
        for test in &self.test_cases {
            by_category.entry(test.metadata.category.clone()).or_default().push(test);
        }

        for (category, tests) in by_category {
            println!("{} ({})", category.bold(), tests.len());
            for test in tests {
                let expect_str = format!("{:?}", test.metadata.expect).to_lowercase();
                let edition = &test.metadata.edition;
                let status = if test.metadata.ignore { " [IGNORED]" } else { "" };
                println!("  • {} [{}] (edition {}){}", test.metadata.name, expect_str, edition, status);
            }
            println!();
        }

        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        println!("{}", "Validating TCK Test Suite".bold().cyan());
        println!();

        let mut errors = 0;

        for test in &self.test_cases {
            // Validate metadata
            if test.metadata.name.is_empty() {
                eprintln!("{}: Missing TEST name", test.path.display());
                errors += 1;
            }

            if test.metadata.description.is_empty() {
                eprintln!("{}: Missing DESCRIPTION", test.path.display());
                errors += 1;
            }

            // Validate expected file for runtime tests
            if matches!(test.metadata.expect, TestExpectation::RuntimeSuccess | TestExpectation::RuntimeError) {
                let expected_file = test.path.with_extension("expected");
                if !expected_file.exists() {
                    eprintln!("{}: Missing .expected file for runtime test", test.path.display());
                    errors += 1;
                }
            }

            // Validate edition
            let valid_editions = ["2024", "2027", "2030"];
            if !valid_editions.contains(&test.metadata.edition.as_str()) {
                eprintln!("{}: Invalid edition '{}'", test.path.display(), test.metadata.edition);
                errors += 1;
            }
        }

        if errors > 0 {
            anyhow::bail!("Validation failed with {} errors", errors);
        }

        Ok(())
    }
}