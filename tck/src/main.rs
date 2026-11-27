//! Naso TCK - Main Entry Point
//!
//! Runs the Naso Technology Compatibility Kit conformance tests.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

mod harness;
mod results;
mod categories;

use harness::{TestHarness, TestConfig, Implementation};
use results::{TestResults, OutputFormat};
use categories::TestCategory;

#[derive(Parser)]
#[command(name = "naso-tck", version, about = "Naso Technology Compatibility Kit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to naso implementation binary
    #[arg(short, long, global = true)]
    impl_path: Option<PathBuf>,

    /// Test category to run
    #[arg(short, long, global = true)]
    category: Option<TestCategory>,

    /// Number of parallel jobs
    #[arg(short, long, global = true, default_value_t = num_cpus::get())]
    jobs: usize,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Output format
    #[arg(short, long, global = true, default_value = "text")]
    output: OutputFormat,

    /// Filter tests by pattern
    #[arg(long, global = true)]
    filter: Option<String>,

    /// List tests without running
    #[arg(long, global = true)]
    list: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run conformance tests
    Run,
    /// List all available tests
    List,
    /// Show test categories
    Categories,
    /// Validate test suite integrity
    Validate,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config = TestConfig {
        impl_path: cli.impl_path.clone().unwrap_or_else(|| PathBuf::from("naso")),
        category: cli.category,
        jobs: cli.jobs,
        verbose: cli.verbose,
        output_format: cli.output,
        filter: cli.filter,
        list_only: cli.list,
    };

    let mut harness = TestHarness::new(config.clone())?;

    match cli.command {
        Commands::Run => {
            let start = Instant::now();
            let results = harness.run_all()?;
            let duration = start.elapsed();

            results.print_summary(&config)?;
            println!("\nTotal time: {:.2}s", duration.as_secs_f64());

            if results.failed > 0 {
                std::process::exit(1);
            }
        }
        Commands::List => {
            harness.list_tests()?;
        }
        Commands::Categories => {
            TestCategory::print_all();
        }
        Commands::Validate => {
            harness.validate()?;
            println!("Test suite validation passed!");
        }
    }

    Ok(())
}