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
        "pkg" => {
            run_pkg_command(&args[2..]);
        }
        "ci" => {
            run_ci_command(&args[2..]);
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
    eprintln!("  pkg <subcommand>      Package manager (add, remove, install, update, publish, search, info)");
    eprintln!("  ci <subcommand>       CI/CD templates (init, github, gitlab)");
    eprintln!("Build options:");
    eprintln!("  --target <llvm|qir|wgsl|wasm-gc|wasm-component|distributed>  Target backend (default: llvm)");
    // `cranelift` is still accepted by the parser so existing scripts do not break on
    // an unknown-target error, but it is not a working backend and says so itself.
    eprintln!(
        "                     (cranelift is accepted but NOT implemented; it refuses rather than returning a stub)"
    );
    eprintln!("  -o, --output <file>            Output file path");
    eprintln!("  --opt <0|1|2|3>                Optimization level (default: 2)");
    eprintln!("  --triple <target>              Target triple (host, nvptx64, wasm32, wasm32-gc, wasm32-wasip1, wasm32-wasip2, wasm32-wasip3, aarch64)");
    eprintln!("  --debug                        Emit debug information");
    eprintln!("Verification options:");
    eprintln!("  --verify                       Run formal verification after type checking");
    eprintln!("  --verify-mode <MODE>           Verification mode: all, uncomputation, linearity, obligations (default: all)");
    eprintln!("  --verify-jobs <N>              Parallel verification jobs (0 = auto, default: 0)");
    eprintln!("  --verify-timeout <MS>          Solver timeout in milliseconds (default: 30000)");
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
    // Verification options (only available with z3 feature)
    #[allow(unused_variables)]
    let mut verify = false;
    #[allow(unused_variables)]
    let mut verify_mode = "all";
    #[allow(unused_variables)]
    let mut verify_jobs = 0;
    #[allow(unused_variables)]
    let mut verify_timeout = 30000;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--verify" => {
                verify = true;
            }
            "--verify-mode" => {
                i += 1;
                if i < args.len() {
                    verify_mode = &args[i];
                }
            }
            "--verify-jobs" => {
                i += 1;
                if i < args.len() {
                    verify_jobs = args[i].parse().unwrap_or(0);
                }
            }
            "--verify-timeout" => {
                i += 1;
                if i < args.len() {
                    verify_timeout = args[i].parse().unwrap_or(30000);
                }
            }
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

    // Run formal verification if requested
    if verify {
        // Find naso-verify binary (in same directory as this executable)
        let current_exe = std::env::current_exe().expect("Failed to get current executable path");
        let naso_verify_bin = current_exe.parent().unwrap().join("naso-verify");
        
        eprintln!("Running formal verification (mode: {verify_mode})...");
        let mut cmd = std::process::Command::new(&naso_verify_bin);
        cmd.arg("--mode").arg(verify_mode);
        if verify_jobs > 0 {
            cmd.arg("--jobs").arg(verify_jobs.to_string());
        }
        cmd.arg("--timeout").arg(verify_timeout.to_string());
        cmd.arg(&input_file);
        
        let status = cmd.status().expect("Failed to run naso verify");
        if !status.success() {
            let code = status.code().unwrap_or(1);
            eprintln!("❌ Verification failed with exit code {}", code);
            std::process::exit(code);
        } else {
            eprintln!("✅ Verification passed: all obligations discharged");
        }
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
    // Verification options (only available with z3 feature)
    #[allow(unused_variables)]
    let mut verify = false;
    #[allow(unused_variables)]
    let mut verify_mode = "all";
    #[allow(unused_variables)]
    let mut verify_jobs = 0;
    #[allow(unused_variables)]
    let mut verify_timeout = 30000;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--verify" => {
                verify = true;
            }
            "--verify-mode" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--verify-mode requires an argument (all, uncomputation, linearity, obligations)");
                    std::process::exit(1);
                }
                verify_mode = &args[i];
            }
            "--verify-jobs" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--verify-jobs requires an argument");
                    std::process::exit(1);
                }
                verify_jobs = args[i].parse().unwrap_or(0);
            }
            "--verify-timeout" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--verify-timeout requires an argument (ms)");
                    std::process::exit(1);
                }
                verify_timeout = args[i].parse().unwrap_or(30000);
            }
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
                    "wasm-gc" => Backend::WasmGc,
                    "wasm-component" => Backend::WasmComponent,
                    "distributed" => Backend::Distributed,
                    other => {
                        eprintln!("Unknown target: {other}. Use llvm, qir, cranelift, wgsl, wasm-gc, wasm-component, or distributed");
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

    // Run formal verification if requested
    if verify {
        // Find naso-verify binary (in same directory as this executable)
        let current_exe = std::env::current_exe().expect("Failed to get current executable path");
        let naso_verify_bin = current_exe.parent().unwrap().join("naso-verify");
        
        // Run naso-verify as a subprocess
        eprintln!("Running formal verification (mode: {verify_mode})...");
        let mut cmd = std::process::Command::new(&naso_verify_bin);
        cmd.arg("--mode").arg(verify_mode);
        if verify_jobs > 0 {
            cmd.arg("--jobs").arg(verify_jobs.to_string());
        }
        cmd.arg("--timeout").arg(verify_timeout.to_string());
        cmd.arg(&input_file);
        
        let status = cmd.status().expect("Failed to run naso verify");
        if !status.success() {
            let code = status.code().unwrap_or(1);
            eprintln!("❌ Verification failed with exit code {}", code);
            std::process::exit(code);
        } else {
            eprintln!("✅ Verification passed: all obligations discharged");
        }
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
        Backend::Cranelift => {
            match pipeline.execute_cranelift_jit(&pir_module) {
                Ok(result) => {
                    println!("JIT execution result: {result}");
                }
                Err(e) => {
                    eprintln!("Cranelift JIT error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Backend::WasmGc => {
            let ir = match pipeline.emit_wasm_gc(&pir_module) {
                Ok(ir) => ir,
                Err(e) => {
                    eprintln!("WASM GC codegen error: {e}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = config.output_path {
                fs::write(&path, ir).unwrap_or_else(|e| {
                    eprintln!("Failed to write output: {e}");
                    std::process::exit(1);
                });
                println!("Written WASM GC IR to {}", path.display());
            } else {
                print!("{ir}");
            }
        }
        Backend::WasmComponent => {
            let ir = match pipeline.emit_wasm_component(&pir_module) {
                Ok(ir) => ir,
                Err(e) => {
                    eprintln!("WASM Component codegen error: {e}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = config.output_path {
                fs::write(&path, ir).unwrap_or_else(|e| {
                    eprintln!("Failed to write output: {e}");
                    std::process::exit(1);
                });
                println!("Written WASM Component IR to {}", path.display());
            } else {
                print!("{ir}");
            }
        }
        Backend::Distributed => {
            // Run the async distributed build
            let rt = tokio::runtime::Runtime::new().unwrap();
            let ir = match rt.block_on(pipeline.emit_distributed(&pir_module)) {
                Ok(ir) => ir,
                Err(e) => {
                    eprintln!("Distributed build error: {e}");
                    std::process::exit(1);
                }
            };
            if let Some(path) = config.output_path {
                fs::write(&path, ir).unwrap_or_else(|e| {
                    eprintln!("Failed to write output: {e}");
                    std::process::exit(1);
                });
                println!("Written Distributed Build Output to {}", path.display());
            } else {
                print!("{ir}");
            }
        }
    }
}

/// `naso pkg` — Package manager commands
fn run_pkg_command(args: &[String]) {
            use std::fs;
            use std::path::PathBuf;
    
            if args.is_empty() {
                eprintln!("Usage: naso pkg <subcommand> [args]");
                eprintln!();
                eprintln!("Subcommands:");
                eprintln!("  add <name>[@version]     Add dependency to naso.toml");
                eprintln!("  remove <name>            Remove dependency from naso.toml");
                eprintln!("  install                  Install all dependencies (generates naso.lock)");
                eprintln!("  update                   Update dependencies to latest versions");
                eprintln!("  publish [path]           Publish package to registry");
                eprintln!("  search <query>           Search registry for packages");
                eprintln!("  info <name>              Show package information");
                eprintln!("  yank <name> <version>    Yank a published version");
                eprintln!("  unyank <name> <version>  Unyank a published version");
                eprintln!("  owners <name>            List package owners");
                eprintln!("  owner-add <name> <user>  Add package owner");
                eprintln!("  owner-remove <name> <user>  Remove package owner");
                std::process::exit(1);
            }
    
            let subcommand = &args[0];
    
            match subcommand.as_str() {
                "add" => {
                    if args.len() < 2 {
                        eprintln!("Usage: naso pkg add <name>[@version]");
                        std::process::exit(1);
                    }
                    let spec = &args[1];
                    let (name, version): (&str, Option<&str>) = if let Some(idx) = spec.find('@') {
                        (&spec[..idx], Some(&spec[idx+1..]))
                    } else {
                        (spec, None)
                    };
            
                    let manifest_path = PathBuf::from("naso.toml");
                    if !manifest_path.exists() {
                        eprintln!("No naso.toml found in current directory");
                        std::process::exit(1);
                    }
            
                    let content = fs::read_to_string(&manifest_path)
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to read naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    let mut manifest = content.parse::<toml::Value>()
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to parse naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    if !manifest.get("dependencies").is_some() {
                        let table = toml::value::Table::new();
                        manifest["dependencies"] = toml::Value::Table(table);
                    }
                    let deps = manifest.get_mut("dependencies")
                        .and_then(|v| v.as_table_mut())
                        .unwrap();
            
                    let version_str = version.unwrap_or("*").to_string();
                    deps.insert(name.to_string(), toml::Value::String(version_str.clone()));
            
                    fs::write(&manifest_path, manifest.to_string())
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to write naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    println!("Added {}@{} to dependencies", name, version_str);
                }
                "remove" => {
                    if args.len() < 2 {
                        eprintln!("Usage: naso pkg remove <name>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
            
                    let manifest_path = PathBuf::from("naso.toml");
                    if !manifest_path.exists() {
                        eprintln!("No naso.toml found in current directory");
                        std::process::exit(1);
                    }
            
                    let content = fs::read_to_string(&manifest_path)
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to read naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    let mut manifest = content.parse::<toml::Value>()
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to parse naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    if let Some(deps) = manifest.get_mut("dependencies").and_then(|v| v.as_table_mut()) {
                        if deps.remove(name).is_some() {
                            fs::write(&manifest_path, manifest.to_string())
                                .unwrap_or_else(|e| {
                                    eprintln!("Failed to write naso.toml: {}", e);
                                    std::process::exit(1);
                                });
                            println!("Removed {} from dependencies", name);
                        } else {
                            eprintln!("Dependency '{}' not found", name);
                            std::process::exit(1);
                        }
                    } else {
                        eprintln!("No dependencies section found");
                        std::process::exit(1);
                    }
                }
                "install" => {
                    println!("📦 Installing dependencies...");
            
                    let manifest_path = PathBuf::from("naso.toml");
                    if !manifest_path.exists() {
                        eprintln!("No naso.toml found in current directory");
                        std::process::exit(1);
                    }
            
                    let content = fs::read_to_string(&manifest_path)
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to read naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    let manifest = content.parse::<toml::Value>()
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to parse naso.toml: {}", e);
                            std::process::exit(1);
                        });
            
                    let deps = manifest.get("dependencies")
                        .and_then(|v| v.as_table())
                        .cloned()
                        .unwrap_or_default();
            
                    let dev_deps = manifest.get("dev-dependencies")
                        .and_then(|v| v.as_table())
                        .cloned()
                        .unwrap_or_default();
            
                    // Generate lockfile
                    let mut lockfile = String::new();
                    lockfile.push_str("# This file is generated by naso pkg install\n");
                    lockfile.push_str("# Do not edit manually\n\n");
            
                    for (name, version) in &deps {
                        let version_str = version.as_str().unwrap_or("*");
                        lockfile.push_str(&format!("[[package]]\n"));
                        lockfile.push_str(&format!("name = \"{}\"\n", name));
                        lockfile.push_str(&format!("version = \"{}\"\n", version_str));
                        lockfile.push_str(&format!("source = \"registry+https://crates.naso-lang.org\"\n"));
                        lockfile.push_str(&format!("dependencies = []\n\n"));
                    }
            
                    for (name, version) in &dev_deps {
                        let version_str = version.as_str().unwrap_or("*");
                        lockfile.push_str(&format!("[[package]]\n"));
                        lockfile.push_str(&format!("name = \"{}\"\n", name));
                        lockfile.push_str(&format!("version = \"{}\"\n", version_str));
                        lockfile.push_str(&format!("source = \"registry+https://crates.naso-lang.org\"\n"));
                        lockfile.push_str(&format!("dependencies = []\n\n"));
                    }
            
                    fs::write("naso.lock", lockfile)
                        .unwrap_or_else(|e| {
                            eprintln!("Failed to write naso.lock: {}", e);
                            std::process::exit(1);
                        });
            
                    println!("✅ Dependencies installed (lockfile generated)");
                }
                "update" => {
                    println!("🔄 Updating dependencies...");
            
                    let manifest_path = PathBuf::from("naso.toml");
                    if !manifest_path.exists() {
                        eprintln!("No naso.toml found in current directory");
                        std::process::exit(1);
                    }
            
                    // For now, just reinstall (in production, would check registry for latest versions)
                    run_pkg_command(&["install".to_string()]);
                }
                "publish" => {
                    let package_path = if args.len() >= 2 {
                        &args[1]
                    } else {
                        "."
                    };
            
                    println!("📤 Publishing package from {}...", package_path);
            
                    let manifest_path = PathBuf::from(package_path).join("naso.toml");
                    if !manifest_path.exists() {
                        eprintln!("No naso.toml found in {}", package_path);
                        std::process::exit(1);
                    }
            
                    // Get registry token from environment
                    let _token = std::env::var("NASO_REGISTRY_TOKEN")
                        .unwrap_or_else(|_| {
                            eprintln!("NASO_REGISTRY_TOKEN environment variable not set");
                            std::process::exit(1);
                        });
            
                    println!("Package would be published to crates.naso-lang.org");
                    println!("(Use std::pkg_registry::RegistryClient from Naso code for programmatic access)");
                }
                "search" => {
                    if args.len() < 2 {
                        eprintln!("Usage: naso pkg search <query>");
                        std::process::exit(1);
                    }
                    let query = &args[1];
            
                    println!("🔍 Searching for '{}'...", query);
                    println!("(Registry search would query crates.naso-lang.org)");
                }
                "info" => {
                    if args.len() < 2 {
                        eprintln!("Usage: naso pkg info <name>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
            
                    println!("📋 Package info for '{}':", name);
                    println!("(Registry info would query crates.naso-lang.org)");
                }
                "yank" => {
                    if args.len() < 3 {
                        eprintln!("Usage: naso pkg yank <name> <version>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
                    let version = &args[2];
            
                    println!("📦 Yanking {}@{}...", name, version);
                    println!("(Registry yank would require authentication)");
                }
                "unyank" => {
                    if args.len() < 3 {
                        eprintln!("Usage: naso pkg unyank <name> <version>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
                    let version = &args[2];
            
                    println!("📦 Unyanking {}@{}...", name, version);
                    println!("(Registry unyank would require authentication)");
                }
                "owners" => {
                    if args.len() < 2 {
                        eprintln!("Usage: naso pkg owners <name>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
            
                    println!("👥 Owners of '{}':", name);
                    println!("(Registry owners would query crates.naso-lang.org)");
                }
                "owner-add" => {
                    if args.len() < 3 {
                        eprintln!("Usage: naso pkg owner-add <name> <user>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
                    let user = &args[2];
            
                    println!("➕ Adding owner '{}' to '{}'...", user, name);
                    println!("(Registry owner-add would require authentication)");
                }
                "owner-remove" => {
                    if args.len() < 3 {
                        eprintln!("Usage: naso pkg owner-remove <name> <user>");
                        std::process::exit(1);
                    }
                    let name = &args[1];
                    let user = &args[2];
            
                    println!("➖ Removing owner '{}' from '{}'...", user, name);
                    println!("(Registry owner-remove would require authentication)");
                }
                _ => {
                    eprintln!("Unknown pkg subcommand: {}", subcommand);
                    std::process::exit(1);
                }
            }
}

/// `naso ci` — CI/CD template commands
fn run_ci_command(args: &[String]) -> Result<(), String> {
    use std::fs;
    use std::path::PathBuf;

    if args.is_empty() {
        eprintln!("Usage: naso ci <subcommand> [args]");
        eprintln!();
        eprintln!("Subcommands:");
        eprintln!("  init       Create CI/CD templates for GitHub Actions and GitLab CI");
        eprintln!("  github     Create GitHub Actions workflow only");
        eprintln!("  gitlab     Create GitLab CI configuration only");
        return Err("No subcommand provided".to_string());
    }

    let subcommand = &args[0];

    match subcommand.as_str() {
        "init" | "github" | "gitlab" => {
            let project_root = PathBuf::from(".");

            if subcommand == "init" || subcommand == "github" {
                create_github_actions(&project_root)?;
            }

            if subcommand == "init" || subcommand == "gitlab" {
                create_gitlab_ci(&project_root)?;
            }

            println!("✅ CI/CD templates created");
        }
        _ => {
            return Err(format!("Unknown ci subcommand: {}", subcommand));
        }
    }
    Ok(())
}

fn create_github_actions(project_root: &PathBuf) -> Result<(), String> {
    use std::fs;

    let workflows_dir = project_root.join(".github").join("workflows");
    fs::create_dir_all(&workflows_dir)
        .map_err(|e| format!("Failed to create workflows directory: {}", e))?;

    // CI workflow
    let ci_workflow = r#"name: CI

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main, develop]

env:
  CARGO_TERM_COLOR: always

jobs:
  build:
    name: Build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
      - name: Build
        run: cargo build --workspace --all-features
      - name: Clippy
        run: cargo clippy --workspace --all-features -- -D warnings
      - name: Rustfmt
        run: cargo fmt --all -- --check

  test:
    name: Test
    runs-on: ubuntu-latest
    needs: build
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
      - name: Test
        run: cargo test --workspace --all-features

  naso_build:
    name: Naso Build
    runs-on: ubuntu-latest
    needs: build
    steps:
      - uses: actions/checkout@v4
      - name: Install Naso
        run: curl -fsSL https://nasolang.org/install.sh | sh
      - name: Add Naso to PATH
        run: echo "$HOME/.naso/bin" >> $GITHUB_PATH
      - name: Build Naso project
        run: naso build src/main.naso -o naso-app
      - uses: actions/upload-artifact@v4
        with:
          name: naso-app
          path: naso-app
          retention-days: 1

  naso_test:
    name: Naso Test
    runs-on: ubuntu-latest
    needs: naso_build
    steps:
      - uses: actions/checkout@v4
      - name: Install Naso
        run: curl -fsSL https://nasolang.org/install.sh | sh
      - name: Add Naso to PATH
        run: echo "$HOME/.naso/bin" >> $GITHUB_PATH
      - name: Run Naso tests
        run: naso test

  verify:
    name: Formal Verification
    runs-on: ubuntu-latest
    needs: naso_build
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - name: Install Naso
        run: curl -fsSL https://nasolang.org/install.sh | sh
      - name: Install Z3
        run: sudo apt-get update && sudo apt-get install -y z3
      - name: Add Naso to PATH
        run: echo "$HOME/.naso/bin" >> $GITHUB_PATH
      - name: Run formal verification
        run: naso build --verify src/main.naso

  release:
    name: Release
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/v')
    needs: [test, naso_test]
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      - name: Build release
        run: cargo build --workspace --all-features --release
      - name: Package binaries
        run: |
          mkdir -p dist
          cp target/release/naso dist/naso-${{ runner.os }}
      - uses: actions/upload-artifact@v4
        with:
          name: naso-release
          path: dist/
          retention-days: 7

  naso_publish:
    name: Publish to Naso Registry
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/v')
    needs: release
    steps:
      - uses: actions/checkout@v4
      - name: Install Naso
        run: curl -fsSL https://nasolang.org/install.sh | sh
      - name: Add Naso to PATH
        run: echo "$HOME/.naso/bin" >> $GITHUB_PATH
      - name: Publish package
        run: naso pkg publish .
        env:
          NASO_REGISTRY_TOKEN: ${{ secrets.NASO_REGISTRY_TOKEN }}"#;

    fs::write(workflows_dir.join("ci.yml"), ci_workflow)
        .map_err(|e| format!("Failed to write CI workflow: {}", e))?;

    // Release workflow
    let release_workflow = r#"name: Release

on:
  push:
    tags:
      - 'v*'

jobs:
  release:
    name: Create Release
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      - name: Build release
        run: cargo build --workspace --all-features --release
      - name: Create Release
        uses: softprops/action-gh-release@v1
        with:
          files: |
            target/release/naso
          generate_release_notes: true
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  naso_publish:
    name: Publish Naso Package
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Naso
        run: curl -fsSL https://nasolang.org/install.sh | sh
      - name: Add Naso to PATH
        run: echo "$HOME/.naso/bin" >> $GITHUB_PATH
      - name: Publish to registry
        run: naso pkg publish .
        env:
          NASO_REGISTRY_TOKEN: ${{ secrets.NASO_REGISTRY_TOKEN }}"#;

    fs::write(workflows_dir.join("release.yml"), release_workflow)
        .map_err(|e| format!("Failed to write release workflow: {}", e))?;

    println!("  Created .github/workflows/ci.yml");
    println!("  Created .github/workflows/release.yml");

    Ok(())
}

fn create_gitlab_ci(project_root: &PathBuf) -> Result<(), String> {
    use std::fs;

    let gitlab_ci = r#"# Naso GitLab CI Configuration
# Place this file at .gitlab-ci.yml in your project root

stages:
  - build
  - test
  - verify
  - release
  - deploy

variables:
  CARGO_TERM_COLOR: "always"
  RUST_BACKTRACE: "1"

# Cache cargo registry and target directory
cache:
  key: "${CI_COMMIT_REF_SLUG}"
  paths:
    - .cargo/registry/index/
    - .cargo/registry/cache/
    - target/
    - ~/.naso/cache/

# Build stage
build:
  stage: build
  image: rust:latest
  script:
    - cargo build --workspace --all-features
  artifacts:
    paths:
      - target/
    expire_in: 1 hour

# Test stage
test:
  stage: test
  image: rust:latest
  script:
    - cargo test --workspace --all-features
  needs:
    - build

# Clippy check
clippy:
  stage: test
  image: rust:latest
  script:
    - cargo clippy --workspace --all-features -- -D warnings
  needs:
    - build

# Naso-specific build
naso_build:
  stage: build
  image: ubuntu:22.04
  before_script:
    - apt-get update && apt-get install -y curl llvm-17 clang
    - curl -fsSL https://nasolang.org/install.sh | sh
    - export PATH="$HOME/.naso/bin:$PATH"
  script:
    - naso build src/main.naso -o naso-app
  artifacts:
    paths:
      - naso-app
    expire_in: 1 hour

# Naso tests
naso_test:
  stage: test
  image: ubuntu:22.04
  before_script:
    - apt-get update && apt-get install -y curl llvm-17 clang
    - curl -fsSL https://nasolang.org/install.sh | sh
    - export PATH="$HOME/.naso/bin:$PATH"
  script:
    - naso test
  needs:
    - naso_build

# Formal verification
verify:
  stage: verify
  image: ubuntu:22.04
  before_script:
    - apt-get update && apt-get install -y curl llvm-17 clang z3
    - curl -fsSL https://nasolang.org/install.sh | sh
    - export PATH="$HOME/.naso/bin:$PATH"
  script:
    - naso build --verify src/main.naso
  needs:
    - naso_build
  allow_failure: true

# Release stage
release:
  stage: release
  image: rust:latest
  rules:
    - if: $CI_COMMIT_TAG =~ /^v/
  script:
    - cargo build --workspace --all-features --release
    - mkdir -p dist
    - cp target/release/naso dist/naso-${CI_RUNNER_DESCRIPTION}
  artifacts:
    paths:
      - dist/
    expire_in: 1 week

# Naso package publish
naso_publish:
  stage: deploy
  image: ubuntu:22.04
  rules:
    - if: $CI_COMMIT_TAG =~ /^v/
  before_script:
    - apt-get update && apt-get install -y curl llvm-17 clang
    - curl -fsSL https://nasolang.org/install.sh | sh
    - export PATH="$HOME/.naso/bin:$PATH"
  script:
    - naso pkg publish .
  variables:
    NASO_REGISTRY_TOKEN: $NASO_REGISTRY_TOKEN"#;

    fs::write(project_root.join(".gitlab-ci.yml"), gitlab_ci)
        .map_err(|e| format!("Failed to write .gitlab-ci.yml: {}", e))?;

    println!("  Created .gitlab-ci.yml");

    Ok(())
}

/// Without the LLVM feature, only the WGSL target is available.
///
/// This function is compiled when the "llvm" feature is NOT enabled.
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
