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
        "repl" => {
            run_repl();
        }
        "init" => {
            run_init(&args[2..]);
        }
        "watch" => {
            if args.len() < 3 {
                eprintln!("Usage: naso watch <file>");
                std::process::exit(1);
            }
            run_watch(&args[2]);
        }
        "test" => {
            run_test(&args[2..]);
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
    eprintln!("  repl                  Start interactive REPL");
    eprintln!("  init                  Create new Naso project (zero-config)");
    eprintln!("  watch <file>          Hot reload — recompile on file change");
    eprintln!("  test                  Run tests in current directory");
    eprintln!("  build [options] <file>  Build program to target");
    eprintln!("Build options:");
    eprintln!("  --target <llvm|qir|wgsl>  Target backend (default: llvm)");
    // `cranelift` is still accepted by the parser so existing scripts do not break on
    // an unknown-target error, but it is not a working backend and says so itself.
    eprintln!(
        "                     (cranelift is accepted but NOT implemented; it refuses rather than returning a stub)"
    );
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

fn run_repl() {
    use std::io::{self, Write};
    
    println!("Naso REPL — type :quit to exit, :help for commands");
    println!("Commands: :quit, :help, :parse, :tokens, :check, :type");
    
    let mut input = String::new();
    let mut multiline = String::new();
    
    loop {
        if multiline.is_empty() {
            print!("naso> ");
        } else {
            print!("...> ");
        }
        io::stdout().flush().unwrap();
        
        input.clear();
        if io::stdin().read_line(&mut input).is_err() {
            break;
        }
        
        let line = input.trim();
        
        // Handle REPL commands
        if line.starts_with(':') {
            match line {
                ":quit" | ":exit" => {
                    println!("Goodbye!");
                    break;
                }
                ":help" => {
                    println!("Commands:");
                    println!("  :quit, :exit  — Exit REPL");
                    println!("  :help         — Show this help");
                    println!("  :parse        — Parse last input as expression");
                    println!("  :tokens       — Show tokens for last input");
                    println!("  :check        — Type check last input");
                    println!("  :type         — Show type of last expression");
                    println!();
                    println!("Multi-line: type incomplete expression, continue on next line");
                    println!("  Empty line submits the accumulated input");
                }
                ":parse" => {
                    if multiline.is_empty() && !input.trim().is_empty() {
                        match parse_program(&input) {
                            Ok(program) => {
                                let json = serde_json::to_string_pretty(&program).expect("serialize");
                                println!("{json}");
                            }
                            Err(e) => eprintln!("Parse error: {e}"),
                        }
                    } else if !multiline.is_empty() {
                        match parse_program(&multiline) {
                            Ok(program) => {
                                let json = serde_json::to_string_pretty(&program).expect("serialize");
                                println!("{json}");
                            }
                            Err(e) => eprintln!("Parse error: {e}"),
                        }
                    } else {
                        eprintln!("No input to parse");
                    }
                }
                ":tokens" => {
                    let source = if multiline.is_empty() { &input } else { &multiline };
                    let tokens = Lexer::lex(source);
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
                ":check" => {
                    let source = if multiline.is_empty() { &input } else { &multiline };
                    let mut program = match parse_program(source) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Parse error: {e}");
                            continue;
                        }
                    };
                    let result = check_program(&mut program);
                    if result.errors.is_empty() {
                        println!("OK");
                    } else {
                        for e in &result.errors {
                            eprintln!("{e}");
                        }
                    }
                }
                ":type" => {
                    let source = if multiline.is_empty() { &input } else { &multiline };
                    let mut program = match parse_program(source) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Parse error: {e}");
                            continue;
                        }
                    };
                    let result = check_program(&mut program);
                    if result.errors.is_empty() {
                        // Print types from type environment
                        println!("Type checking passed (type display not yet implemented)");
                    } else {
                        for e in &result.errors {
                            eprintln!("{e}");
                        }
                    }
                }
                _ => {
                    eprintln!("Unknown command: {line}. Type :help for commands.");
                }
            }
            multiline.clear();
            continue;
        }
        
        // Accumulate multi-line input
        if multiline.is_empty() {
            multiline.push_str(line);
        } else {
            multiline.push('\n');
            multiline.push_str(line);
        }
        
        // Check if input is complete (simple heuristic: balanced braces/parens)
        // For now, empty line submits
        if line.is_empty() && !multiline.is_empty() {
            // Try to parse and execute
            let mut program = match parse_program(&multiline) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Parse error: {e}");
                    multiline.clear();
                    continue;
                }
            };
            
            let result = check_program(&mut program);
            if !result.errors.is_empty() {
                for e in &result.errors {
                    eprintln!("{e}");
                }
            } else {
                println!("OK (execution not yet implemented)");
            }
            
            multiline.clear();
        }
    }
}

/// `naso init` — Create a new zero-config Naso project
fn run_init(args: &[String]) {
    use std::fs;
    use std::path::PathBuf;
    
    let mut project_name = None;
    let mut force = false;
    
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-f" | "--force" => force = true,
            arg if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                std::process::exit(1);
            }
            name => {
                if project_name.is_none() {
                    project_name = Some(name.to_string());
                } else {
                    eprintln!("Multiple project names not supported");
                    std::process::exit(1);
                }
            }
        }
        i += 1;
    }
    
    let project_name = project_name.unwrap_or_else(|| {
        eprintln!("Usage: naso init <project-name> [-f|--force]");
        std::process::exit(1);
    });
    
    let project_dir = PathBuf::from(&project_name);
    
    if project_dir.exists() && !force {
        eprintln!("Directory '{}' already exists. Use -f to overwrite.", project_name);
        std::process::exit(1);
    }
    
    // Create project structure
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap_or_else(|e| {
        eprintln!("Failed to create project directory: {e}");
        std::process::exit(1);
    });
    
    // Create main.naso
    let main_naso = r#"// Naso web server example
// Run: naso build src/main.naso -o main
//      ./main

extern "C" {
    fn naso_write(fd: u64, buf: *u8, len: usize) -> usize;
}

struct Stream {
    fd: Int,
}

fn main() -> () {
    let body: [u8] = [104, 101, 108, 108, 111];
    let stream = Stream { fd: 1 };
    let _ = write(stream, body);
    ()
}
"#;
    
    fs::write(src_dir.join("main.naso"), main_naso).unwrap_or_else(|e| {
        eprintln!("Failed to write main.naso: {e}");
        std::process::exit(1);
    });
    
    // Create naso.toml (optional config, mostly for metadata)
    let naso_toml = format!(r#"[project]
name = "{}"
version = "0.1.0"
description = "Naso web service"

[build]
target = "llvm"
opt = 2

[dependencies]
# Add dependencies here
# naso-ffi = {{ path = "../crates/naso-ffi" }}
"#, project_name);
    
    fs::write(project_dir.join("naso.toml"), naso_toml).unwrap_or_else(|e| {
        eprintln!("Failed to write naso.toml: {e}");
        std::process::exit(1);
    });
    
    // Create .gitignore
    let gitignore = r#"# Build artifacts
target/
*.o
*.ll
*.bc

# Executables
main
*.exe

# IDE
.vscode/
.idea/
*.swp
*~

# OS
.DS_Store
"#;
    
    fs::write(project_dir.join(".gitignore"), gitignore).unwrap_or_else(|e| {
        eprintln!("Failed to write .gitignore: {e}");
        std::process::exit(1);
    });
    
    // Create README.md
    let readme = format!(r#"# {}

Naso web service — zero-config, single binary.

## Quick Start

```bash
# Build
naso build src/main.naso -o {}

# Run
./{}
```

## Project Structure

```
{}/
├── naso.toml       # Project metadata (optional)
├── src/
│   └── main.naso   # Entry point
└── .gitignore
```

## Commands

- `naso check src/main.naso` — Type check only
- `naso repl` — Interactive REPL
- `naso build src/main.naso -o {}` — Build to native binary
- `naso build --target wgsl src/main.naso -o main.wgsl` — Build to WGSL

## Requirements

- Naso compiler (`naso`)
- LLVM 17+ (for `--target llvm`)
- Rust toolchain not required — Naso compiles directly to native code

## Learn More

- Naso language: https://nasolang.org
- QTT (Quantity-Typed Types): eliminates borrow checker
- Linear async: no Pin/Send/Sync
"#, project_name, project_name, project_name, project_name, project_name);
    
    fs::write(project_dir.join("README.md"), readme).unwrap_or_else(|e| {
        eprintln!("Failed to write README.md: {e}");
        std::process::exit(1);
    });
    
    println!("Created Naso project '{}'", project_name);
    println!("  {}/", project_name);
    println!("  ├── naso.toml");
    println!("  ├── src/");
    println!("  │   └── main.naso");
    println!("  ├── .gitignore");
    println!("  └── README.md");
    println!();
    println!("Next steps:");
    println!("  cd {}", project_name);
    println!("  naso build src/main.naso -o {}", project_name);
    println!("  ./{}", project_name);
}

/// `naso watch` — Hot reload: recompile on file change
fn run_watch(file: &str) {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;
    
    let file_path = PathBuf::from(file);
    if !file_path.exists() {
        eprintln!("File '{}' does not exist", file);
        std::process::exit(1);
    }
    
    let output_name = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out");
    
    println!("Watching {} for changes... (Ctrl+C to stop)", file);
    println!("Output: {}", output_name);
    
    // Initial build
    build_and_run(&file_path, output_name);
    
    // Watch for changes
    let mut last_modified = fs::metadata(&file_path)
        .and_then(|m| m.modified())
        .ok();
    
    loop {
        std::thread::sleep(Duration::from_millis(500));
        
        if let Ok(metadata) = fs::metadata(&file_path) {
            if let Ok(modified) = metadata.modified() {
                if last_modified.map_or(true, |last| modified > last) {
                    last_modified = Some(modified);
                    println!("\n🔄 File changed, rebuilding...");
                    build_and_run(&file_path, output_name);
                }
            }
        }
    }
}

fn build_and_run(file_path: &PathBuf, output_name: &str) {
    use std::process::Command;
    
    // Build to LLVM IR
    let build_status = Command::new("/tmp/cargo-target/debug/naso")
        .args(["build", file_path.to_str().unwrap(), "-o", output_name])
        .status();
    
    match build_status {
        Ok(status) if status.success() => {
            println!("✅ Build successful (LLVM IR)");
            
            // Compile IR to object file
            let llc_status = Command::new("/tmp/llvm17/bin/llc")
                .args([
                    "-filetype=obj",
                    output_name,
                    "-o",
                    &format!("{}.o", output_name),
                ])
                .status();
            
            match llc_status {
                Ok(status) if status.success() => {
                    // Compile entry point wrapper
                    let entry_c = "/tmp/entry.c";
                    let entry_o = "/tmp/entry.o";
                    if std::path::Path::new(entry_c).exists() {
                        let _ = Command::new("/tmp/llvm17/bin/clang")
                            .args(["-c", entry_c, "-o", entry_o])
                            .status();
                    }
                    
                    // Link with naso_ffi + entry point
                    let link_status = Command::new("/tmp/llvm17/bin/clang")
                        .args([
                            &format!("{}.o", output_name),
                            entry_o,
                            "/tmp/cargo-target/release/libnaso_ffi.a",
                            "-o", output_name,
                            "-lpthread", "-ldl"
                        ])
                        .status();
                    
                    match link_status {
                        Ok(status) if status.success() => {
                            println!("🔗 Linked successfully");
                            
                            // Run the binary
                            let run_status = Command::new(format!("./{}", output_name))
                                .status();
                            
                            match run_status {
                                Ok(status) => {
                                    println!("🏁 Process exited with code: {}", status.code().unwrap_or(-1));
                                }
                                Err(e) => {
                                    eprintln!("Failed to run binary: {}", e);
                                }
                            }
                        }
                        Ok(status) => {
                            eprintln!("❌ Link failed with exit code: {}", status.code().unwrap_or(-1));
                        }
                        Err(e) => {
                            eprintln!("Failed to run linker: {}", e);
                        }
                    }
                }
                Ok(status) => {
                    eprintln!("❌ llc failed with exit code: {}", status.code().unwrap_or(-1));
                }
                Err(e) => {
                    eprintln!("Failed to run llc: {}", e);
                }
            }
        }
        Ok(status) => {
            eprintln!("❌ Build failed with exit code: {}", status.code().unwrap_or(-1));
        }
        Err(e) => {
            eprintln!("Failed to run naso build: {}", e);
        }
    }
}

/// `naso test` — Built-in test runner: discovers and runs test functions
fn run_test(args: &[String]) {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    
    let mut test_file = None;
    let mut verbose = false;
    
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" | "--verbose" => verbose = true,
            arg if arg.starts_with('-') => {
                eprintln!("Unknown option: {arg}");
                std::process::exit(1);
            }
            path => {
                if test_file.is_none() {
                    test_file = Some(PathBuf::from(path));
                } else {
                    eprintln!("Multiple test files not supported");
                    std::process::exit(1);
                }
            }
        }
        i += 1;
    }
    
    // Default to current directory if no file specified
    let test_file = test_file.unwrap_or_else(|| PathBuf::from("."));
    
    println!("🧪 Naso Test Runner");
    println!("===================");
    
    if test_file.is_dir() {
        // Find all .naso files in tests/ directory or *.test.naso files
        let test_files = find_test_files(&test_file);
        if test_files.is_empty() {
            println!("No test files found in {}", test_file.display());
            return;
        }
        
        let mut passed = 0;
        let mut failed = 0;
        
        for file in &test_files {
            println!("\n📝 Testing {}...", file.display());
            
            // Build and run test
            let output_name = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("test_out");
            
            let build_status = Command::new("/tmp/cargo-target/debug/naso")
                .args(["build", file.to_str().unwrap(), "-o", output_name])
                .status();
            
            match build_status {
                Ok(status) if status.success() => {
                    // Compile and link
                    let _ = Command::new("/tmp/llvm17/bin/llc")
                        .args(["-filetype=obj", output_name, "-o", &format!("{}.o", output_name)])
                        .status();
                    
                    let _ = Command::new("/tmp/llvm17/bin/clang")
                        .args(["-c", "/tmp/entry.c", "-o", "/tmp/entry.o"])
                        .status();
                    
                    let link_status = Command::new("/tmp/llvm17/bin/clang")
                        .args([
                            &format!("{}.o", output_name),
                            "/tmp/entry.o",
                            "/tmp/cargo-target/release/libnaso_ffi.a",
                            "-o", output_name,
                            "-lpthread", "-ldl"
                        ])
                        .status();
                    
                    match link_status {
                        Ok(status) if status.success() => {
                            let run_status = Command::new(format!("./{}", output_name))
                                .status();
                            
                            match run_status {
                                Ok(status) if status.success() => {
                                    println!("  ✅ PASSED");
                                    passed += 1;
                                }
                                Ok(status) => {
                                    println!("  ❌ FAILED (exit code: {})", status.code().unwrap_or(-1));
                                    failed += 1;
                                }
                                Err(e) => {
                                    println!("  ❌ FAILED (run error: {})", e);
                                    failed += 1;
                                }
                            }
                        }
                        _ => {
                            println!("  ❌ FAILED (link failed)");
                            failed += 1;
                        }
                    }
                }
                _ => {
                    println!("  ❌ FAILED (build failed)");
                    failed += 1;
                }
            }
        }
        
        println!("\n===================");
        println!("Results: {} passed, {} failed", passed, failed);
        
        if failed > 0 {
            std::process::exit(1);
        }
    } else {
        // Single test file
        let output_name = test_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("test_out");
        
        let build_status = Command::new("/tmp/cargo-target/debug/naso")
            .args(["build", test_file.to_str().unwrap(), "-o", output_name])
            .status();
        
        match build_status {
            Ok(status) if status.success() => {
                let _ = Command::new("/tmp/llvm17/bin/llc")
                    .args(["-filetype=obj", output_name, "-o", &format!("{}.o", output_name)])
                    .status();
                
                let _ = Command::new("/tmp/llvm17/bin/clang")
                    .args(["-c", "/tmp/entry.c", "-o", "/tmp/entry.o"])
                    .status();
                
                let link_status = Command::new("/tmp/llvm17/bin/clang")
                    .args([
                        &format!("{}.o", output_name),
                        "/tmp/entry.o",
                        "/tmp/cargo-target/release/libnaso_ffi.a",
                        "-o", output_name,
                        "-lpthread", "-ldl"
                    ])
                    .status();
                
                match link_status {
                    Ok(status) if status.success() => {
                        let run_status = Command::new(format!("./{}", output_name))
                            .status();
                        
                        match run_status {
                            Ok(status) if status.success() => {
                                println!("✅ TEST PASSED");
                            }
                            Ok(status) => {
                                println!("❌ TEST FAILED (exit code: {})", status.code().unwrap_or(-1));
                                std::process::exit(1);
                            }
                            Err(e) => {
                                println!("❌ TEST FAILED (run error: {})", e);
                                std::process::exit(1);
                            }
                        }
                    }
                    _ => {
                        println!("❌ TEST FAILED (link failed)");
                        std::process::exit(1);
                    }
                }
            }
            _ => {
                println!("❌ TEST FAILED (build failed)");
                std::process::exit(1);
            }
        }
    }
}

fn find_test_files(dir: &PathBuf) -> Vec<PathBuf> {
    use std::fs;
    let mut files = Vec::new();
    
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "naso" {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if name.contains("test") || name.ends_with("_test") {
                            files.push(path);
                        }
                    }
                }
            }
        }
    }
    
    // Also check tests/ subdirectory
    let tests_dir = dir.join("tests");
    if tests_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&tests_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "naso" {
                            files.push(path);
                        }
                    }
                }
            }
        }
    }
    
    files
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
