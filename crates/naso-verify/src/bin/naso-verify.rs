//! `naso-verify` -- the real verification entry point.
//!
//! ## Why this is not `naso verify`
//!
//! `naso-verify` depends on `naso-compiler` (it parses and lowers the AST), so the compiler
//! crate cannot depend on `naso-verify` to implement a subcommand: that is a dependency
//! cycle, which Cargo rejects. The verifier therefore ships as its own binary rather than
//! pretending to be a subcommand of `naso`. This is documented in `STATE.md`; it is a real
//! constraint, not a stylistic preference.
//!
//! ## Exit codes
//!
//! These are the contract, and they exist so a CI job can distinguish four states that all
//! look like "no output" if they collapse into one:
//!
//! | code | meaning |
//! |------|---------|
//! | 0 | every obligation was DISCHARGED |
//! | 1 | at least one obligation was REFUTED -- a claim proven false |
//! | 2 | at least one obligation was LEFT UNDECIDED -- neither confirmed nor refuted |
//! | 3 | usage error (bad arguments) |
//! | 4 | input error (file missing, or the program did not parse) |
//! | 5 | internal verifier failure (solver error, malformed SMT) |
//! | 6 | no obligations found, and `--require-obligations` was given |
//!
//! Code 2 is the load-bearing one. The prover cannot decide everything it is asked: some
//! constructs have no encoding yet and are reported as unsupported. A tool that returns 0
//! for "I proved it" *and* for "I could not look at it" is a tool whose green build means
//! nothing, and that is precisely the failure this project exists to prevent. Undecidable is
//! not passing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use naso_compiler::parser::parse_program;
use naso_verify::OutputFormat;
use naso_verify::cli::{VerifyMode, parse_verify_args, print_verify_usage};
use naso_verify::error::VerifyError;
use naso_verify::model::{DiagnosticSeverity, VerifyDiagnostic};
use naso_verify::prover::{run_linearity_prover, run_obligation_prover, run_uncomputation_prover};

const EXIT_DISCHARGED: u8 = 0;
const EXIT_REFUTED: u8 = 1;
const EXIT_UNDECIDED: u8 = 2;
const EXIT_USAGE: u8 = 3;
const EXIT_INPUT: u8 = 4;
const EXIT_INTERNAL: u8 = 5;
const EXIT_NO_OBLIGATIONS: u8 = 6;

/// How the CLI must treat one diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// The prover proved the goal. Informational: not a finding.
    Discharged,
    /// The prover disproved the claim.
    Refuted,
    /// The prover could not decide -- neither confirmed nor refuted.
    Undecided,
}

/// Classify a diagnostic by its CODE first, falling back to severity.
///
/// The code is authoritative because severity alone is not sufficient: the first version of
/// this classifier asked only "is the severity Warning?", which meant every DISCHARGED
/// obligation -- severity `Info` -- fell into the "not undecided" bucket and was then counted
/// as a refutation. The CLI cheerfully reported two obligations as `error` and printed
/// "2 refuted" on a kernel where both were proved, i.e. it reported a proof as a failure.
///
/// That is the exact failure this whole tool exists to prevent, so classification is explicit
/// and total: every code maps to exactly one class, and severity is the fallback for codes
/// this binary has never seen.
fn classify(d: &VerifyDiagnostic) -> Class {
    match d.code.as_str() {
        "NASO-OBL-000" => Class::Discharged,
        "NASO-OBL-001" => Class::Refuted,
        "NASO-OBL-002" => Class::Undecided,
        _ => match d.severity {
            DiagnosticSeverity::Error => Class::Refuted,
            DiagnosticSeverity::Warning => Class::Undecided,
            DiagnosticSeverity::Info | DiagnosticSeverity::Hint => Class::Discharged,
        },
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_verify_usage();
        return ExitCode::from(EXIT_DISCHARGED);
    }
    if args.is_empty() {
        eprintln!("error: no input files given");
        print_verify_usage();
        return ExitCode::from(EXIT_USAGE);
    }

    let config = match parse_verify_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    if config.list_codes {
        print_diagnostic_codes();
        return ExitCode::from(EXIT_DISCHARGED);
    }

    if config.inputs.is_empty() {
        eprintln!("error: no input files given");
        return ExitCode::from(EXIT_USAGE);
    }

    // Collect inputs. A missing path is an INPUT error, distinct from a failed proof: the
    // verifier was never given anything to check.
    let mut programs = Vec::new();
    for input in &config.inputs {
        let path = PathBuf::from(input);
        match collect_naso_files(&path) {
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                return ExitCode::from(EXIT_INPUT);
            }
            Ok(files) => {
                if files.is_empty() && path.is_file() {
                    eprintln!("error: {}: no .naso files found", path.display());
                    return ExitCode::from(EXIT_INPUT);
                }
                for file in files {
                    match std::fs::read_to_string(&file) {
                        Err(e) => {
                            eprintln!("error: {}: {e}", file.display());
                            return ExitCode::from(EXIT_INPUT);
                        }
                        Ok(source) => match parse_program(&source) {
                            Err(e) => {
                                eprintln!("error: {}: parse error: {e}", file.display());
                                return ExitCode::from(EXIT_INPUT);
                            }
                            Ok(program) => programs.push((file, program)),
                        },
                    }
                }
            }
        }
    }

    if programs.is_empty() {
        eprintln!("error: no .naso files found in the given inputs");
        return ExitCode::from(EXIT_INPUT);
    }

    // Run the selected prover(s).
    // (file, diagnostic) pairs. The diagnostic itself carries only a byte-offset span.
    let mut diagnostics: Vec<(String, VerifyDiagnostic)> = Vec::new();
    let mut internal_error: Option<String> = None;

    for (path, program) in &programs {
        let result = run_mode(&config.mode, program);
        match result {
            Ok(diags) => {
                // Record which file each diagnostic came from, so a multi-file run does not
                // produce unattributable errors.
                for d in diags {
                    diagnostics.push((path.display().to_string(), d));
                }
            }
            Err(e) => {
                // A solver or lowering failure is INTERNAL, and it is not swallowed: it is
                // promoted to the exit code. Letting it degrade into "no diagnostics" would
                // turn a broken verifier into a passing build.
                internal_error = Some(match internal_error {
                    Some(prev) => format!("{prev}\n  also: {}: {e}", path.display()),
                    None => format!("{}: {e}", path.display()),
                });
            }
        }
    }

    if let Some(err) = internal_error {
        eprintln!("error: internal verification failure: {err}");
        return ExitCode::from(EXIT_INTERNAL);
    }

    report(&diagnostics, config.quiet, &config.format);

    let refuted = diagnostics
        .iter()
        .any(|(_, d)| classify(d) == Class::Refuted);
    let undecided = diagnostics
        .iter()
        .any(|(_, d)| classify(d) == Class::Undecided);

    if refuted {
        ExitCode::from(EXIT_REFUTED)
    } else if undecided {
        // Undecidable is NOT success. See the module docs.
        ExitCode::from(EXIT_UNDECIDED)
    } else if config.require_obligations && obligation_count(&diagnostics) == 0 {
        ExitCode::from(EXIT_NO_OBLIGATIONS)
    } else {
        ExitCode::from(EXIT_DISCHARGED)
    }
}

fn run_mode(
    mode: &VerifyMode,
    program: &naso_compiler::ast::Program,
) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    match mode {
        VerifyMode::All => run_all(program),
        VerifyMode::Uncomputation => run_uncomputation_prover(program),
        VerifyMode::Linearity => run_linearity_prover(program),
        VerifyMode::Obligations => run_obligation_prover(program),
    }
}

/// Run every prover and CONCATENATE their diagnostics.
///
/// `run_all_provers` in the library short-circuits: it returns as soon as one prover reports
/// a problem. That is fine for a library caller that only cares whether something failed,
/// but it means one failing prover hides the rest of the output. Concatenating keeps the
/// reporting honest -- if linearity and obligations both fail, both are reported.
fn run_all(program: &naso_compiler::ast::Program) -> Result<Vec<VerifyDiagnostic>, VerifyError> {
    let mut all = Vec::new();
    for result in [
        run_uncomputation_prover(program),
        run_linearity_prover(program),
        run_obligation_prover(program),
    ] {
        all.extend(result?);
    }
    Ok(all)
}

/// How many obligations the prover actually discharged.
///
/// Derived from the informational diagnostics the obligation prover emits for each
/// discharged goal, so that `--require-obligations` fails a file containing no proofs at
/// all rather than passing vacuously.
fn obligation_count(diagnostics: &[(String, VerifyDiagnostic)]) -> usize {
    diagnostics
        .iter()
        .filter(|(_, d)| d.code == "NASO-OBL-000")
        .count()
}

fn report(diagnostics: &[(String, VerifyDiagnostic)], quiet: bool, format: &OutputFormat) {
    let mut out = std::io::stdout().lock();

    match format {
        OutputFormat::Json => {
            // Hand-rolled so the JSON shape is asserted by a test rather than trusted.
            let items: Vec<String> = diagnostics
                .iter()
                .map(|(file, d)| {
                    format!(
                        "{{\"code\":{},\"severity\":{},\"message\":{},\"file\":{},\"line\":{},\"column\":{}}}",
                        json_str(&d.code),
                        json_str(severity_name(d.severity)),
                        json_str(&d.message),
                        json_str(file),
                        d.span.line,
                        d.span.column,
                    )
                })
                .collect();
            let _ = writeln!(
                out,
                "{{\"diagnostics\":[{}],\"refuted\":{},\"undecided\":{}}}",
                items.join(","),
                diagnostics
                    .iter()
                    .any(|(_, d)| classify(d) == Class::Refuted),
                diagnostics
                    .iter()
                    .any(|(_, d)| classify(d) == Class::Undecided),
            );
        }
        OutputFormat::Sarif => {
            let results: Vec<String> = diagnostics
                .iter()
                .map(|(_file, d)| {
                    format!(
                        "{{\"ruleId\":{},\"level\":{},\"message\":{{\"text\":{}}},\"locations\":[]}}",
                        json_str(&d.code),
                        json_str(match classify(d) {
                            Class::Refuted => "error",
                            Class::Undecided => "warning",
                            Class::Discharged => "note",
                        }),
                        json_str(&d.message),
                    )
                })
                .collect();
            let _ = writeln!(
                out,
                "{{\"$schema\":\"https://json.schemastore.org/sarif-2.1.0.json\",\"version\":\"2.1.0\",\
                 \"runs\":[{{\"tool\":{{\"driver\":{{\"name\":\"naso-verify\"}}}},\
                 \"results\":[{}]}}]}}",
                results.join(",")
            );
        }
        OutputFormat::Human => {
            if !quiet {
                for (file, d) in diagnostics {
                    // A discharged obligation is printed as `note`, not `error`. Printing a
                    // proof as an error is worse than printing nothing.
                    let kind = match classify(d) {
                        Class::Refuted => "error",
                        Class::Undecided => "warning",
                        Class::Discharged => "note",
                    };
                    let _ = writeln!(
                        out,
                        "{file}:{}:{}: {kind}[{}]: {}",
                        d.span.line, d.span.column, d.code, d.message
                    );
                }
            }
            let refuted = diagnostics
                .iter()
                .filter(|(_, d)| classify(d) == Class::Refuted)
                .count();
            let undecided = diagnostics
                .iter()
                .filter(|(_, d)| classify(d) == Class::Undecided)
                .count();
            let discharged = obligation_count(diagnostics);
            let _ = writeln!(
                out,
                "{discharged} obligation(s) discharged, {refuted} refuted, {undecided} undecided"
            );
            if undecided > 0 {
                // Say which exit status this run actually produces, rather than asserting 2:
                // a refutation outranks an undecided obligation, so a run with both exits 1.
                // Telling the reader "exit 2" when the process exits 1 is the kind of small
                // lie that makes a log untrustworthy.
                let _ = writeln!(
                    out,
                    "note: {undecided} undecided obligation(s) were NOT proved. \
                     This run exits non-zero regardless."
                );
            }
        }
    }
}

fn severity_name(s: DiagnosticSeverity) -> &'static str {
    match s {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Hint => "hint",
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn print_diagnostic_codes() {
    println!("NASO-OBL-000  obligation discharged (informational)");
    println!("NASO-OBL-001  obligation refuted: the claim is false");
    println!("NASO-OBL-002  obligation left undecided: unsupported construct");
    println!("NASO-LIN-001  linear quantity leaked (unused [1] resource)");
    println!("NASO-LIN-002  linear quantity used more than once");
    println!("NASO-LIN-003  linear quantity used after consumption");
    println!("NASO-LIN-004  linear quantity budget exceeded");
    println!("NASO-UNC-001  qubit not uncomputed before scope exit");
    println!("NASO-UNC-002  uncomputation is not a no-op on this state");
}

fn collect_naso_files(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file or directory",
        ));
    }
    let mut files = Vec::new();
    collect_recursive(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            collect_recursive(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "naso") {
            out.push(p);
        }
    }
    Ok(())
}
