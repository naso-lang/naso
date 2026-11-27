//! TCK Test Results and Reporting

use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    #[default]
    Passed,
    Failed,
    Skipped,
}

impl std::fmt::Display for TestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestStatus::Passed => write!(f, "{}", "PASS".green().bold()),
            TestStatus::Failed => write!(f, "{}", "FAIL".red().bold()),
            TestStatus::Skipped => write!(f, "{}", "SKIP".yellow().bold()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestResult {
    pub name: String,
    pub category: String,
    pub status: TestStatus,
    pub duration: Duration,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestResults {
    pub implementation: crate::harness::Implementation,
    pub results: Vec<TestResult>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total: usize,
}

impl TestResults {
    pub fn empty() -> Self {
        Self {
            implementation: crate::harness::Implementation {
                path: std::path::PathBuf::new(),
                version: String::new(),
                edition_support: Vec::new(),
            },
            results: Vec::new(),
            passed: 0,
            failed: 0,
            skipped: 0,
            total: 0,
        }
    }

    pub fn print_summary(&self, config: &crate::harness::TestConfig) -> Result<()> {
        match config.output_format {
            OutputFormat::Text => self.print_text_summary(config),
            OutputFormat::Json => self.print_json_summary()?,
            OutputFormat::Junit => self.print_junit_summary()?,
            OutputFormat::Tap => self.print_tap_summary()?,
        }
        Ok(())
    }

    fn print_text_summary(&self, config: &crate::harness::TestConfig) {
        println!("\n{}", "═══ TCK Results Summary ═══".bold().cyan());
        println!("Implementation: {} ({})", self.implementation.path.display(), self.implementation.version);
        println!("Total: {} | Passed: {} | Failed: {} | Skipped: {}",
            self.total,
            self.passed.to_string().green(),
            self.failed.to_string().red(),
            self.skipped.to_string().yellow()
        );

        if self.failed > 0 {
            println!("\n{}", "Failed Tests:".red().bold());
            for result in &self.results {
                if result.status == TestStatus::Failed {
                    println!("  {} [{}]", result.name.red(), result.category);
                    println!("    {}", result.message);
                    if config.verbose && !result.stdout.is_empty() {
                        println!("    stdout: {}", result.stdout.lines().next().unwrap_or(""));
                    }
                    if config.verbose && !result.stderr.is_empty() {
                        println!("    stderr: {}", result.stderr.lines().next().unwrap_or(""));
                    }
                }
            }
        }

        // Per-category breakdown
        let mut by_category: HashMap<String, (usize, usize, usize)> = HashMap::new();
        for result in &self.results {
            let (p, f, s) = by_category.entry(result.category.clone()).or_default();
            match result.status {
                TestStatus::Passed => *p += 1,
                TestStatus::Failed => *f += 1,
                TestStatus::Skipped => *s += 1,
            }
        }

        println!("\n{}", "Per-Category Breakdown:".bold());
        for (cat, (p, f, s)) in by_category {
            let total = p + f + s;
            let pass_rate = if total > 0 { (p as f64 / total as f64) * 100.0 } else { 0.0 };
            println!("  {}: {}/{} passed ({:.1}%) | {} failed | {} skipped",
                cat, p, total, pass_rate, f, s);
        }

        // Conformance level
        println!("\n{}", "Conformance Assessment:".bold());
        let level = self.assess_conformance();
        println!("  Level: {}", level);
    }

    fn print_json_summary(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        println!("{}", json);
        Ok(())
    }

    fn print_junit_summary(&self) -> Result<()> {
        // Simplified JUnit XML output
        println!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
        println!("<testsuites>");
        
        let mut by_category: HashMap<String, Vec<&TestResult>> = HashMap::new();
        for result in &self.results {
            by_category.entry(result.category.clone()).or_default().push(result);
        }

        for (category, results) in by_category {
            println!("  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" skipped=\"{}\">", 
                category, 
                results.len(),
                results.iter().filter(|r| r.status == TestStatus::Failed).count(),
                results.iter().filter(|r| r.status == TestStatus::Skipped).count()
            );
            
            for result in results {
                println!("    <testcase name=\"{}\" time=\"{:.3}\">", result.name, result.duration.as_secs_f64());
                match result.status {
                    TestStatus::Failed => {
                        println!("      <failure message=\"{}\"><![CDATA[stdout:\n{}\nstderr:\n{}]]></failure>", 
                            result.message.replace('&', "&").replace('<', "<").replace('>', ">"),
                            result.stdout.replace('&', "&").replace('<', "<").replace('>', ">"),
                            result.stderr.replace('&', "&").replace('<', "<").replace('>', ">")
                        );
                    }
                    TestStatus::Skipped => {
                        println!("      <skipped message=\"{}\"/>", result.message.replace('&', "&").replace('<', "<").replace('>', ">"));
                    }
                    _ => {}
                }
                println!("    </testcase>");
            }
            println!("  </testsuite>");
        }
        println!("</testsuites>");
        Ok(())
    }

    fn print_tap_summary(&self) -> Result<()> {
        println!("TAP version 13");
        println!("1..{}", self.total);

        for (i, result) in self.results.iter().enumerate() {
            let status = match result.status {
                TestStatus::Passed => "ok",
                TestStatus::Failed => "not ok",
                TestStatus::Skipped => "ok",
            };
            let skip = if result.status == TestStatus::Skipped { " # SKIP" } else { "" };
            println!("{} {} - {}{}", status, i + 1, result.name, skip);
            if result.status == TestStatus::Failed {
                println!("  ---");
                println!("  message: {}", result.message.replace('\n', "\n  "));
                println!("  severity: fail");
                println!("  ...");
            }
        }
        Ok(())
    }

    fn assess_conformance(&self) -> String {
        if self.failed > 0 {
            return "Non-Conforming (Failures)".red().to_string();
        }

        // Check which categories have tests
        let mut categories_covered = std::collections::HashSet::new();
        for result in &self.results {
            categories_covered.insert(result.category.clone());
        }

        let required_categories = [
            "syntax", "types", "semantics", "stdlib", "quantum", "effects", "verification", "editions"
        ];

        let all_required = required_categories.iter().all(|c| categories_covered.contains(*c));

        if !all_required {
            return "Partial (Missing Categories)".yellow().to_string();
        }

        // Check if we have tests in all subcategories
        let type_subcategories = ["linear", "affine", "unrestricted", "quantities", "effects", "dependent", "erasure"];
        let types_covered: std::collections::HashSet<_> = categories_covered.iter()
            .filter(|c| c.starts_with("types/"))
            .map(|c| c.strip_prefix("types/").unwrap_or(c))
            .collect();

        let all_types = type_subcategories.iter().all(|s| types_covered.contains(*s));

        if all_types && self.passed > 500 {
            "Full Conformance (Level 4)".green().bold().to_string()
        } else if all_types && self.passed > 200 {
            "Semantic Conformance (Level 2)".green().to_string()
        } else if all_types {
            "Type System Conformance (Level 1)".yellow().to_string()
        } else {
            "Syntax Conformance (Level 0)".yellow().to_string()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Junit,
    Tap,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Text => write!(f, "text"),
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Junit => write!(f, "junit"),
            OutputFormat::Tap => write!(f, "tap"),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            "junit" => Ok(OutputFormat::Junit),
            "tap" => Ok(OutputFormat::Tap),
            _ => Err(format!("Unknown output format: {}", s)),
        }
    }
}