use std::env;
use std::fs;
use std::path::PathBuf;

use naso_compiler::lexer::Lexer;
use naso_compiler::parser::parse_program;
use naso_compiler::typecheck::check_program;

use naso_compiler::codegen::generate_wgsl_straight_line;
#[cfg(feature = "llvm")]
use naso_compiler::codegen::{
    Backend, CodegenConfig, CodegenContext, CodegenPipeline, CodegenTarget, OptLevel,
};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    let command = &args[1];

    match command.as_str() {
        "parse" | "tokens" | "check" => {
            if args.len() < 3 {
                eprintln!("Usage: naso {command} <file>");
                std::process::exit(1);
            }
            run_frontend_command(command, &args[2]);
        }
        "build" => {
            run_build_command(&args[2..]);
        }
        _ => {
            eprintln!("Unknown command: {command}");
            print_usage();
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("Usage: naso <command> [args]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  parse <file>          Parse and print AST as JSON");
    eprintln!("  tokens <file>         Print token stream");
    eprintln!("  check <file>          Type check program");
    eprintln!("  build [options] <file>  Build program to target");
    eprintln!("Build options:");
    eprintln!("  --target <llvm|qir|cranelift|wgsl>  Target backend (default: llvm)");
    eprintln!("  -o, --output <file>            Output file path");
    eprintln!("  --opt <0|1|2|3>                Optimization level (default: 2)");
    eprintln!("  --triple <target>              Target triple (host, nvptx64, wasm32, aarch64)");
    eprintln!("  --debug                        Emit debug information");
}

fn run_frontend_command(command: &str, file: &str) {
    let file_path = PathBuf::from(file);
    let source = fs::read_to_string(&file_path).unwrap_or_else(|e| {
        eprintln!("error: cannot read `{}`: {e}", file_path.display());
        std::process::exit(1);
    });

    match command {
        "parse" => match parse_program(&source) {
            Ok(program) => {
                let json = serde_json::to_string_pretty(&program).expect("failed to serialize AST");
                println!("{json}");
            }
            Err(e) => {
                eprintln!("parse error: {e}");
                std::process::exit(1);
            }
        },
        "tokens" => {
            let tokens = Lexer::lex(&source);
            for tok in &tokens {
                let kind_name = format!("{:?}", tok.kind);
                let short = match &tok.kind {
                    naso_compiler::lexer::TokenKind::Comment => "comment".into(),
                    naso_compiler::lexer::TokenKind::Newline => "newline".into(),
                    other => format!("{other}"),
                };
                println!(
                    "{:4}-{:4}  {:<14}  {}",
                    tok.span.start, tok.span.end, kind_name, short
                );
            }
        }
        "check" => {
            let mut program = match parse_program(&source) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("parse error: {e}");
                    std::process::exit(1);
                }
            };
            let result = check_program(&mut program);
            if result.errors.is_empty() {
                println!("OK");
                std::process::exit(0);
            } else {
                for e in &result.errors {
                    eprintln!("{e}");
                }
                std::process::exit(1);
            }
        }
        _ => unreachable!(),
    }
}

/// Emit WGSL for `--target wgsl`.
///
/// Deliberately independent of the LLVM feature: WGSL is a text generator, so
/// this is the one backend that works in a default build. It does its own
/// argument parsing for the two options that matter here and ignores the rest,
/// so `--opt` / `--triple` / `--debug` do not change its behaviour.
fn run_wgsl_build_command(args: &[String]) {
    let mut input_file: Option<PathBuf> = None;
    let mut output_path: Option<PathBuf> = None;
    // --kernel <name> selects the function to emit as a compute entry point.
    let mut kernel: Option<String> = None;
    // --emit-abi prints the host layout of the generated shader to stderr.
    let mut emit_abi = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" | "--opt" => i += 1, // value consumed; opt is irrelevant here
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(PathBuf::from(&args[i]));
                }
            }
            "--kernel" => {
                i += 1;
                if i < args.len() {
                    kernel = Some(args[i].clone());
                }
            }
            "--emit-abi" => emit_abi = true,
            arg if arg.starts_with('-') => {}
            path => {
                if input_file.is_none() {
                    input_file = Some(PathBuf::from(path));
                }
            }
        }
        i += 1;
    }

    let Some(input_file) = input_file else {
        eprintln!("Missing input file");
        std::process::exit(1);
    };

    let source = fs::read_to_string(&input_file).unwrap_or_else(|e| {
        eprintln!("error: cannot read `{}`: {e}", input_file.display());
        std::process::exit(1);
    });

    let mut program = match parse_program(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse error: {e}");
            std::process::exit(1);
        }
    };

    // WGSL is emitted from a typechecked program: an untypechecked function
    // could emit a shader that does not match the source's meaning.
    let result = check_program(&mut program);
    if !result.errors.is_empty() {
        for e in &result.errors {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }

    // Lower the program and consume the schedule tree.
    //
    // This backend emits no loops, so the bands are used for the DIAGNOSTIC:
    // a rejected `forall` now reports the iteration domain the lowering
    // actually computed, rather than just naming the construct.
    //
    // The side effect that matters is reachability. `lower_program` previously
    // ran only inside the `llvm`-gated build path, so in a default build the
    // lowering pass -- including its non-trivial iteration domains -- was
    // unreachable. Calling it here puts it on the `--target wgsl` path, which
    // needs no LLVM feature. A build that reaches this line and reports a
    // band with depth 1 and a real domain is proof the lowering works.
    let pir = match naso_compiler::lowering::lower_program(&program) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Lowering error: {e}");
            std::process::exit(1);
        }
    };
    let bands = naso_compiler::codegen::schedule_consumer::module_bands(&pir);
    if !bands.is_empty() {
        eprintln!(
            "note: schedule tree has {} band(s), emitted as WGSL `for` loops in \
             source order:",
            bands.len()
        );
        for b in &bands {
            eprintln!("  stmt {}: {}", b.stmt_id, b.describe());
        }
    }

    // A kernel is selected with --kernel <name>; it is emitted as a compute
    // entry point, because a tensor parameter has no WGSL function spelling.

    let wgsl = match &kernel {
        Some(name) => {
            match naso_compiler::codegen::wgsl_compute::generate_wgsl_compute(&program, name) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("WGSL compute codegen error: {e}");
                    std::process::exit(1);
                }
            }
        }
        None => match generate_wgsl_straight_line(&program) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("WGSL codegen error: {e}");
                std::process::exit(1);
            }
        },
    };

    // The host ABI is described by the SAME analysis that emitted the shader,
    // so the two cannot disagree. A WebGPU client needs it to allocate buffers:
    // it must not have to recover the layout by parsing shader text.
    if emit_abi {
        let Some(name) = kernel.as_deref() else {
            eprintln!("--emit-abi needs --kernel <name>: a scalar function has no bindings");
            std::process::exit(1);
        };
        match naso_compiler::codegen::wgsl_compute::describe_compute_abi(&program, name) {
            Ok(abi) => {
                eprintln!("@group(0) entry point `{}`", abi.entry_point);
                for b in &abi.bindings {
                    eprintln!(
                        "  binding({}) {} : array<{}> {} ({} bytes)",
                        b.index,
                        b.name,
                        b.elem,
                        b.access,
                        abi.buffer_bytes(b.index).unwrap_or(0)
                    );
                }
                for (i, (n, t)) in abi.scalars.iter().enumerate() {
                    eprintln!(
                        "  binding({}) {} : {} uniform ({} bytes, WGSL name {}_u)",
                        abi.scalar_binding(i).unwrap_or(0),
                        n,
                        t,
                        abi.scalar_bytes(i).unwrap_or(0),
                        n,
                    );
                }
                eprintln!(
                    "  workgroup_size {} x, dispatch {} workgroups for {} elements",
                    abi.workgroup_size,
                    abi.elements().div_ceil(abi.workgroup_size as usize),
                    abi.elements()
                );
            }
            Err(e) => {
                eprintln!("WGSL ABI error: {e}");
                std::process::exit(1);
            }
        }
    }

    match output_path {
        Some(path) => {
            fs::write(&path, &wgsl).unwrap_or_else(|e| {
                eprintln!("Failed to write output: {e}");
                std::process::exit(1);
            });
            println!("Written WGSL to {}", path.display());
        }
        None => print!("{wgsl}"),
    }
}

#[cfg(feature = "llvm")]
fn run_build_command(args: &[String]) {
    // Keep `--target wgsl` identical in both builds: it does not need LLVM, so
    // it must not depend on this function's feature gate.
    if args
        .windows(2)
        .any(|w| w[0] == "--target" && w[1] == "wgsl")
    {
        run_wgsl_build_command(args);
        return;
    }

    let mut config = CodegenConfig::default();
    let mut input_file = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--target requires an argument");
                    std::process::exit(1);
                }
                config.backend = match args[i].as_str() {
                    "llvm" => Backend::Llvm,
                    "qir" => Backend::Qir,
                    "cranelift" => Backend::Cranelift,
                    "wgsl" => Backend::Wgsl,
                    other => {
                        eprintln!("Unknown target: {other}. Use llvm, qir, or cranelift");
                        std::process::exit(1);
                    }
                };
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--output requires an argument");
                    std::process::exit(1);
                }
                config.output_path = Some(PathBuf::from(&args[i]));
            }
            "--opt" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--opt requires an argument");
                    std::process::exit(1);
                }
                config.opt_level = match args[i].as_str() {
                    "0" => OptLevel::None,
                    "1" => OptLevel::Less,
                    "2" => OptLevel::Default,
                    "3" => OptLevel::Aggressive,
                    other => {
                        eprintln!("Invalid optimization level: {other}. Use 0, 1, 2, or 3");
                        std::process::exit(1);
                    }
                };
            }
            "--triple" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--triple requires an argument");
                    std::process::exit(1);
                }
                config.target = CodegenTarget::from_str(&args[i]).unwrap_or_else(|e| {
                    eprintln!("Invalid target triple: {e}");
                    std::process::exit(1);
                });
            }
            "--debug" => {
                config.emit_debug = true;
            }
            arg if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                std::process::exit(1);
            }
            current_arg => {
                if input_file.is_none() {
                    input_file = Some(PathBuf::from(current_arg));
                } else {
                    eprintln!("Multiple input files not supported");
                    std::process::exit(1);
                }
            }
        }
        i += 1;
    }

    let input_file = input_file.unwrap_or_else(|| {
        eprintln!("Missing input file");
        print_usage();
        std::process::exit(1);
    });

    // Read and parse source
    let source = fs::read_to_string(&input_file).unwrap_or_else(|e| {
        eprintln!("error: cannot read `{}`: {e}", input_file.display());
        std::process::exit(1);
    });

    let mut program = match parse_program(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse error: {e}");
            std::process::exit(1);
        }
    };

    // Type check
    let result = check_program(&mut program);
    if !result.errors.is_empty() {
        for e in &result.errors {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }

    // Lower to PIR
    #[cfg(feature = "llvm")]
    let pir_module = match naso_compiler::lowering::lower_program(&program) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("lowering error: {e}");
            std::process::exit(1);
        }
    };

    // Create codegen context
    let codegen_context =
        match CodegenContext::with_debug(config.target, config.opt_level, config.emit_debug) {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("codegen context error: {e}");
                std::process::exit(1);
            }
        };

    // Create pipeline and generate output
    let pipeline = CodegenPipeline::new(codegen_context);

    match config.backend {
        Backend::Llvm => {
            let ir = match pipeline.emit_llvm(&pir_module) {
                Ok(ir) => ir,
                Err(e) => {
                    eprintln!("LLVM codegen error: {e}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = config.output_path {
                fs::write(&path, ir).unwrap_or_else(|e| {
                    eprintln!("Failed to write output: {e}");
                    std::process::exit(1);
                });
                println!("Written LLVM IR to {}", path.display());
            } else {
                print!("{ir}");
            }
        }
        Backend::Wgsl => {
            let wgsl = match generate_wgsl_straight_line(&program) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("WGSL codegen error: {e}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = config.output_path {
                fs::write(&path, &wgsl).unwrap_or_else(|e| {
                    eprintln!("Failed to write output: {e}");
                    std::process::exit(1);
                });
                println!("Written WGSL to {}", path.display());
            } else {
                print!("{wgsl}");
            }
        }
        Backend::Qir => {
            let ir = match pipeline.emit_qir(&pir_module) {
                Ok(ir) => ir,
                Err(e) => {
                    eprintln!("QIR codegen error: {e}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = config.output_path {
                fs::write(&path, ir).unwrap_or_else(|e| {
                    eprintln!("Failed to write output: {e}");
                    std::process::exit(1);
                });
                println!("Written QIR to {}", path.display());
            } else {
                print!("{ir}");
            }
        }
        Backend::Cranelift => match pipeline.execute_cranelift_jit(&pir_module) {
            Ok(result) => {
                println!("JIT execution result: {result}");
            }
            Err(e) => {
                eprintln!("Cranelift JIT error: {e}");
                std::process::exit(1);
            }
        },
    }
}

/// Without the LLVM feature, only the WGSL target is available.
///
/// WGSL is emitted as text and needs no LLVM, so refusing it here would mean
/// the one backend that works in a default build was unreachable. Every other
/// target still reports the real reason.
#[cfg(not(feature = "llvm"))]
fn run_build_command(args: &[String]) {
    let wants_wgsl = args
        .windows(2)
        .any(|w| w[0] == "--target" && w[1] == "wgsl");
    if !wants_wgsl {
        eprintln!(
            "Build command requires the LLVM backend for llvm/qir/cranelift. \
             Compile with the 'llvm' feature, or use `--target wgsl`, which \
             needs no LLVM."
        );
        std::process::exit(1);
    }
    run_wgsl_build_command(args);
}
