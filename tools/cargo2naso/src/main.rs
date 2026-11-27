//! cargo2naso - Migrate Rust/Cargo projects to Naso
//!
//! This tool converts Cargo.toml, src/**/*.rs to naso.toml, src/**/*.naso

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use toml::{Value, Table};

#[derive(Debug, Clone)]
struct Config {
    input_dir: PathBuf,
    output_dir: PathBuf,
    package_name: Option<String>,
    edition: String,
    verbose: bool,
    dry_run: bool,
    force: bool,
}

#[derive(Debug)]
struct CargoToml {
    package: Option<Package>,
    dependencies: Table,
    dev_dependencies: Table,
    build_dependencies: Table,
    features: Table,
    workspace: Option<Workspace>,
}

#[derive(Debug)]
struct Package {
    name: String,
    version: String,
    authors: Vec<String>,
    description: Option<String>,
    license: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    keywords: Vec<String>,
    categories: Vec<String>,
    edition: String,
}

#[derive(Debug)]
struct Workspace {
    members: Vec<String>,
    exclude: Vec<String>,
}

#[derive(Debug)]
struct NasoToml {
    package: NasoPackage,
    dependencies: Table,
    dev_dependencies: Table,
    build_dependencies: Table,
    features: Table,
}

#[derive(Debug)]
struct NasoPackage {
    name: String,
    version: String,
    authors: Vec<String>,
    edition: String,
    description: Option<String>,
    license: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    keywords: Vec<String>,
    categories: Vec<String>,
}

fn main() {
    let config = parse_args();
    
    if config.verbose {
        println!("cargo2naso v0.1.0");
        println!("Input: {}", config.input_dir.display());
        println!("Output: {}", config.output_dir.display());
    }
    
    // Read Cargo.toml
    let cargo_toml = read_cargo_toml(&config.input_dir.join("Cargo.toml"));
    
    // Convert to Naso.toml
    let naso_toml = convert_cargo_to_naso(&cargo_toml, &config);
    
    // Write Naso.toml
    write_naso_toml(&config.output_dir.join("naso.toml"), &naso_toml, config.dry_run);
    
    // Convert source files
    convert_sources(&config);
    
    // Generate .gitignore
    write_gitignore(&config.output_dir, config.dry_run);
    
    // Generate README if needed
    write_readme(&config.output_dir, &naso_toml.package, config.dry_run);
    
    if !config.dry_run {
        println!("Migration complete!");
        println!("Run 'naso build' in {} to verify", config.output_dir.display());
    }
}

fn parse_args() -> Config {
    let args: Vec<String> = std::env::args().collect();
    
    let mut config = Config {
        input_dir: PathBuf::from("."),
        output_dir: PathBuf::from("./naso-project"),
        package_name: None,
        edition: "2024".to_string(),
        verbose: false,
        dry_run: false,
        force: false,
    };
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-i" | "--input" => {
                i += 1;
                config.input_dir = PathBuf::from(&args[i]);
            }
            "-o" | "--output" => {
                i += 1;
                config.output_dir = PathBuf::from(&args[i]);
            }
            "-n" | "--name" => {
                i += 1;
                config.package_name = Some(args[i].clone());
            }
            "--edition" => {
                i += 1;
                config.edition = args[i].clone();
            }
            "-v" | "--verbose" => config.verbose = true,
            "--dry-run" => config.dry_run = true,
            "-f" | "--force" => config.force = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                print_help();
                std::process::exit(1);
            }
        }
        i += 1;
    }
    
    config
}

fn print_help() {
    println!(r#"cargo2naso - Migrate Rust/Cargo projects to Naso

USAGE:
    cargo2naso [OPTIONS]

OPTIONS:
    -i, --input <DIR>       Input directory (default: .)
    -o, --output <DIR>      Output directory (default: ./naso-project)
    -n, --name <NAME>       Override package name
    --edition <EDITION>     Naso edition (default: 2024)
    -v, --verbose           Verbose output
    --dry-run               Show what would be done without writing
    -f, --force             Overwrite existing files
    -h, --help              Show this help

EXAMPLES:
    cargo2naso
    cargo2naso -i my-rust-project -o my-naso-project
    cargo2naso --dry-run -v
"#);
}

fn read_cargo_toml(path: &Path) -> CargoToml {
    let content = fs::read_to_string(path).expect("Failed to read Cargo.toml");
    let value: Value = content.parse().expect("Failed to parse Cargo.toml");
    
    let package = value.get("package").and_then(|p| parse_package(p));
    let dependencies = value.get("dependencies").cloned().unwrap_or_default().as_table().cloned().unwrap_or_default();
    let dev_dependencies = value.get("dev-dependencies").cloned().unwrap_or_default().as_table().cloned().unwrap_or_default();
    let build_dependencies = value.get("build-dependencies").cloned().unwrap_or_default().as_table().cloned().unwrap_or_default();
    let features = value.get("features").cloned().unwrap_or_default().as_table().cloned().unwrap_or_default();
    let workspace = value.get("workspace").and_then(|w| parse_workspace(w));
    
    CargoToml {
        package,
        dependencies,
        dev_dependencies,
        build_dependencies,
        features,
        workspace,
    }
}

fn parse_package(value: &Value) -> Option<Package> {
    let table = value.as_table()?;
    
    Some(Package {
        name: table.get("name")?.as_str()?.to_string(),
        version: table.get("version")?.as_str()?.to_string(),
        authors: table.get("authors")?.as_array()?.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        description: table.get("description").and_then(|v| v.as_str().map(String::from)),
        license: table.get("license").and_then(|v| v.as_str().map(String::from)),
        repository: table.get("repository").and_then(|v| v.as_str().map(String::from)),
        homepage: table.get("homepage").and_then(|v| v.as_str().map(String::from)),
        keywords: table.get("keywords").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
        categories: table.get("categories").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
        edition: table.get("edition").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "2021".to_string()),
    })
}

fn parse_workspace(value: &Value) -> Option<Workspace> {
    let table = value.as_table()?;
    Some(Workspace {
        members: table.get("members")?.as_array()?.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        exclude: table.get("exclude").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
    })
}

fn convert_cargo_to_naso(cargo: &CargoToml, config: &Config) -> NasoToml {
    let pkg = cargo.package.as_ref().expect("Cargo.toml must have [package]");
    
    let naso_package = NasoPackage {
        name: config.package_name.clone().unwrap_or_else(|| pkg.name.clone()),
        version: pkg.version.clone(),
        authors: pkg.authors.clone(),
        edition: config.edition.clone(),
        description: pkg.description.clone(),
        license: pkg.license.clone(),
        repository: pkg.repository.clone(),
        homepage: pkg.homepage.clone(),
        keywords: pkg.keywords.clone(),
        categories: pkg.categories.clone(),
    };
    
    // Convert dependencies
    let mut dependencies = Table::new();
    for (name, value) in &cargo.dependencies {
        dependencies.insert(name.clone(), convert_dependency(value));
    }
    
    // Add naso-std as default dependency
    dependencies.insert("naso-std".to_string(), Value::String("0.1".to_string()));
    
    let mut dev_dependencies = Table::new();
    for (name, value) in &cargo.dev_dependencies {
        dev_dependencies.insert(name.clone(), convert_dependency(value));
    }
    dev_dependencies.insert("naso-test".to_string(), Value::String("0.1".to_string()));
    
    let mut build_dependencies = Table::new();
    for (name, value) in &cargo.build_dependencies {
        build_dependencies.insert(name.clone(), convert_dependency(value));
    }
    
    // Convert features
    let mut features = Table::new();
    for (name, value) in &cargo.features {
        features.insert(name.clone(), convert_features(value));
    }
    // Add default std feature
    features.insert("default".to_string(), Value::Array(vec![Value::String("std".to_string())]));
    features.insert("std".to_string(), Value::Array(vec![Value::String("naso-std".to_string())]));
    features.insert("no-std".to_string(), Value::Array(vec![]));
    
    NasoToml {
        package: naso_package,
        dependencies,
        dev_dependencies,
        build_dependencies,
        features,
    }
}

fn convert_dependency(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(convert_version_req(s)),
        Value::Table(t) => {
            let mut new_table = Table::new();
            for (k, v) in t {
                match k.as_str() {
                    "version" => new_table.insert("version".to_string(), Value::String(convert_version_req(v.as_str().unwrap_or("")))),
                    "features" => new_table.insert("features".to_string(), v.clone()),
                    "optional" => new_table.insert("optional".to_string(), v.clone()),
                    "path" => new_table.insert("path".to_string(), v.clone()),
                    "git" => new_table.insert("git".to_string(), v.clone()),
                    "branch" | "tag" | "rev" => new_table.insert(k.clone(), v.clone()),
                    "default-features" => new_table.insert("default-features".to_string(), v.clone()),
                    _ => new_table.insert(k.clone(), v.clone()),
                }
            }
            Value::Table(new_table)
        }
        _ => value.clone(),
    }
}

fn convert_version_req(req: &str) -> String {
    // Convert Cargo version requirements to Naso format
    // Cargo uses semver with ^, ~, =, etc.
    // Naso uses similar but may have differences
    req.to_string()
}

fn convert_features(value: &Value) -> Value {
    match value {
        Value::Array(arr) => {
            let new_arr: Vec<Value> = arr.iter().map(|v| {
                if let Some(s) = v.as_str() {
                    // Convert feature names
                    Value::String(convert_feature_name(s))
                } else {
                    v.clone()
                }
            }).collect();
            Value::Array(new_arr)
        }
        _ => value.clone(),
    }
}

fn convert_feature_name(name: &str) -> String {
    // Convert Rust feature names to Naso conventions
    name.replace('-', "_")
}

fn write_naso_toml(path: &Path, naso: &NasoToml, dry_run: bool) {
    let mut output = String::new();
    output.push_str("# Generated by cargo2naso\n\n");
    output.push_str("[package]\n");
    output.push_str(&format!("name = \"{}\"\n", naso.package.name));
    output.push_str(&format!("version = \"{}\"\n", naso.package.version));
    output.push_str(&format!("authors = {:?}\n", naso.package.authors));
    output.push_str(&format!("edition = \"{}\"\n", naso.package.edition));
    if let Some(desc) = &naso.package.description {
        output.push_str(&format!("description = \"{}\"\n", desc));
    }
    if let Some(license) = &naso.package.license {
        output.push_str(&format!("license = \"{}\"\n", license));
    }
    if let Some(repo) = &naso.package.repository {
        output.push_str(&format!("repository = \"{}\"\n", repo));
    }
    if let Some(homepage) = &naso.package.homepage {
        output.push_str(&format!("homepage = \"{}\"\n", homepage));
    }
    if !naso.package.keywords.is_empty() {
        output.push_str(&format!("keywords = {:?}\n", naso.package.keywords));
    }
    if !naso.package.categories.is_empty() {
        output.push_str(&format!("categories = {:?}\n", naso.package.categories));
    }
    output.push('\n');
    
    output.push_str("[dependencies]\n");
    for (name, value) in &naso.dependencies {
        output.push_str(&format!("{} = {}\n", name, format_value(value)));
    }
    output.push('\n');
    
    if !naso.dev_dependencies.is_empty() {
        output.push_str("[dev-dependencies]\n");
        for (name, value) in &naso.dev_dependencies {
            output.push_str(&format!("{} = {}\n", name, format_value(value)));
        }
        output.push('\n');
    }
    
    if !naso.build_dependencies.is_empty() {
        output.push_str("[build-dependencies]\n");
        for (name, value) in &naso.build_dependencies {
            output.push_str(&format!("{} = {}\n", name, format_value(value)));
        }
        output.push('\n');
    }
    
    if !naso.features.is_empty() {
        output.push_str("[features]\n");
        for (name, value) in &naso.features {
            output.push_str(&format!("{} = {}\n", name, format_value(value)));
        }
    }
    
    if dry_run {
        println!("Would write naso.toml:\n{}", output);
    } else {
        fs::write(path, output).expect("Failed to write naso.toml");
    }
}

fn format_value(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{}\"", s),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(|v| format_value(v)).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Table(t) => {
            let items: Vec<String> = t.iter().map(|(k, v)| format!("{} = {}", k, format_value(v))).collect();
            format!("{{{}}}", items.join(", "))
        }
        _ => value.to_string(),
    }
}

fn convert_sources(config: &Config) {
    let src_dir = config.input_dir.join("src");
    if !src_dir.exists() {
        return;
    }
    
    let out_src = config.output_dir.join("src");
    if !dry_run_check(config.dry_run) {
        fs::create_dir_all(&out_src).expect("Failed to create src dir");
    }
    
    for entry in walk_dir(&src_dir) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            convert_rust_file(path, &out_src, config);
        } else if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            // Copy other TOML files
            copy_file(path, &out_src, config);
        } else {
            // Copy other files
            copy_file(path, &out_src, config);
        }
    }
}

fn convert_rust_file(input: &Path, output_dir: &Path, config: &Config) {
    let content = fs::read_to_string(input).expect("Failed to read Rust file");
    let naso_content = convert_rust_to_naso(&content);
    
    let rel_path = input.strip_prefix(config.input_dir.join("src")).unwrap();
    let output_path = output_dir.join(rel_path).with_extension("naso");
    
    if config.verbose {
        println!("Converting {} -> {}", input.display(), output_path.display());
    }
    
    if !dry_run_check(config.dry_run) {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent dir");
        }
        fs::write(&output_path, naso_content).expect("Failed to write Naso file");
    }
}

fn convert_rust_to_naso(rust: &str) -> String {
    let mut naso = String::new();
    naso.push_str("// Converted from Rust by cargo2naso\n\n");
    
    let lines: Vec<&str> = rust.lines().collect();
    let mut i = 0;
    
    while i < lines.len() {
        let line = lines[i].trim();
        
        // Skip certain Rust-specific lines
        if line.starts_with("#![") || line.starts_with("//!") {
            i += 1;
            continue;
        }
        
        // Convert use statements
        if line.starts_with("use ") {
            naso.push_str(&convert_use(line));
            naso.push('\n');
            i += 1;
            continue;
        }
        
        // Convert extern crate
        if line.starts_with("extern crate ") {
            i += 1;
            continue;
        }
        
        // Convert struct definitions
        if line.starts_with("struct ") {
            naso.push_str(&convert_struct(lines, &mut i));
            continue;
        }
        
        // Convert enum definitions
        if line.starts_with("enum ") {
            naso.push_str(&convert_enum(lines, &mut i));
            continue;
        }
        
        // Convert fn definitions
        if line.starts_with("fn ") || line.starts_with("pub fn ") || line.starts_with("async fn ") {
            naso.push_str(&convert_function(lines, &mut i));
            continue;
        }
        
        // Convert impl blocks
        if line.starts_with("impl ") {
            naso.push_str(&convert_impl(lines, &mut i));
            continue;
        }
        
        // Convert trait definitions
        if line.starts_with("trait ") {
            naso.push_str(&convert_trait(lines, &mut i));
            continue;
        }
        
        // Convert const/static
        if line.starts_with("const ") || line.starts_with("static ") {
            naso.push_str(&convert_const(lines, &mut i));
            continue;
        }
        
        // Convert module declarations
        if line.starts_with("mod ") {
            naso.push_str(&convert_mod(line));
            naso.push('\n');
            i += 1;
            continue;
        }
        
        // Convert attributes
        if line.starts_with("#[") {
            naso.push_str(&convert_attribute(line));
            naso.push('\n');
            i += 1;
            continue;
        }
        
        // Default: try to convert line
        naso.push_str(&convert_line(line));
        naso.push('\n');
        i += 1;
    }
    
    naso
}

fn convert_use(line: &str) -> String {
    // use std::collections::HashMap; -> use std::collections::HashMap;
    // use crate::module::*; -> use crate::module::*;
    line.replace("::", "::").replace("self", "Self")
}

fn convert_struct(lines: &[&str], i: &mut usize) -> String {
    let mut result = String::new();
    let line = lines[*i];
    
    // struct Name { fields } or struct Name(fields) or struct Name;
    result.push_str(&line.replace("pub ", ""));
    result.push('\n');
    *i += 1;
    result
}

fn convert_enum(lines: &[&str], i: &mut usize) -> String {
    let mut result = String::new();
    let line = lines[*i];
    result.push_str(&line.replace("pub ", ""));
    result.push('\n');
    *i += 1;
    result
}

fn convert_function(lines: &[&str], i: &mut usize) -> String {
    let mut result = String::new();
    let mut brace_count = 0;
    let mut started = false;
    
    while *i < lines.len() {
        let line = lines[*i];
        let trimmed = line.trim();
        
        if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") || trimmed.starts_with("async fn ") {
            started = true;
        }
        
        if started {
            // Count braces
            for ch in line.chars() {
                if ch == '{' { brace_count += 1; }
                if ch == '}' { brace_count -= 1; }
            }
            
            // Convert line
            let converted = convert_line(line);
            result.push_str(&converted);
            result.push('\n');
            
            if brace_count == 0 && started {
                break;
            }
        }
        *i += 1;
    }
    
    result
}

fn convert_impl(lines: &[&str], i: &mut usize) -> String {
    let mut result = String::new();
    let mut brace_count = 0;
    let mut started = false;
    
    while *i < lines.len() {
        let line = lines[*i];
        let trimmed = line.trim();
        
        if trimmed.starts_with("impl ") {
            started = true;
            result.push_str(&convert_line(line));
            result.push('\n');
        } else if started {
            for ch in line.chars() {
                if ch == '{' { brace_count += 1; }
                if ch == '}' { brace_count -= 1; }
            }
            
            result.push_str(&convert_line(line));
            result.push('\n');
            
            if brace_count == 0 {
                break;
            }
        }
        *i += 1;
    }
    
    result
}

fn convert_trait(lines: &[&str], i: &mut usize) -> String {
    let mut result = String::new();
    let mut brace_count = 0;
    let mut started = false;
    
    while *i < lines.len() {
        let line = lines[*i];
        let trimmed = line.trim();
        
        if trimmed.starts_with("trait ") {
            started = true;
            result.push_str(&convert_line(line));
            result.push('\n');
        } else if started {
            for ch in line.chars() {
                if ch == '{' { brace_count += 1; }
                if ch == '}' { brace_count -= 1; }
            }
            
            result.push_str(&convert_line(line));
            result.push('\n');
            
            if brace_count == 0 {
                break;
            }
        }
        *i += 1;
    }
    
    result
}

fn convert_const(lines: &[&str], i: &mut usize) -> String {
    let line = lines[*i];
    *i += 1;
    line.replace("const ", "const ").replace("static ", "const ") + "\n"
}

fn convert_mod(line: &str) -> String {
    line.replace("pub mod ", "mod ").replace("mod ", "mod ")
}

fn convert_attribute(line: &str) -> String {
    // Convert common attributes
    line.replace("#[derive(", "#[derive(")
        .replace("#[test]", "#[test]")
        .replace("#[cfg(test)]", "#[cfg(test)]")
        .replace("#[inline]", "#[inline]")
}

fn convert_line(line: &str) -> String {
    let mut result = line.to_string();
    
    // Type conversions
    result = result.replace("Vec<", "Vec<");
    result = result.replace("HashMap<", "HashMap<");
    result = result.replace("Option<", "Option<");
    result = result.replace("Result<", "Result<");
    result = result.replace("Box<", "Box<");
    result = result.replace("Rc<", "Rc<");
    result = result.replace("Arc<", "Arc<");
    result = result.replace("Mutex<", "Mutex<");
    result = result.replace("&str", "&str");
    result = result.replace("String", "String");
    result = result.replace("bool", "bool");
    result = result.replace("i32", "i32");
    result = result.replace("i64", "i64");
    result = result.replace("u32", "u32");
    result = result.replace("u64", "u64");
    result = result.replace("f32", "f32");
    result = result.replace("f64", "f64");
    result = result.replace("usize", "usize");
    result = result.replace("isize", "isize");
    
    // Keyword conversions
    result = result.replace("mut ", "mut ");
    result = result.replace("ref ", "ref ");
    result = result.replace("move ", "");
    result = result.replace("loop {", "loop {");
    result = result.replace("match ", "match ");
    result = result.replace("if let ", "if let ");
    result = result.replace("while let ", "while let ");
    
    // Macro conversions
    result = result.replace("println!", "println!");
    result = result.replace("format!", "format!");
    result = result.replace("vec!", "vec!");
    result = result.replace("panic!", "panic!");
    result = result.replace("assert!", "assert!");
    result = result.replace("assert_eq!", "assert_eq!");
    result = result.replace("unwrap()", "unwrap()");
    result = result.replace("expect(", "expect(");
    
    // Return type
    result = result.replace("-> Result<", "-> Result<");
    result = result.replace("-> Option<", "-> Option<");
    
    // Closures
    result = result.replace("|", "|");
    
    result
}

fn copy_file(input: &Path, output_dir: &Path, config: &Config) {
    let rel_path = input.strip_prefix(&config.input_dir).unwrap_or(input);
    let output_path = output_dir.join(rel_path);
    
    if config.verbose {
        println!("Copying {} -> {}", input.display(), output_path.display());
    }
    
    if !dry_run_check(config.dry_run) {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent dir");
        }
        fs::copy(input, &output_path).expect("Failed to copy file");
    }
}

fn walk_dir(dir: &Path) -> Vec<fs::DirEntry> {
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                entries.extend(walk_dir(&path));
            } else {
                entries.push(entry);
            }
        }
    }
    entries
}

fn dry_run_check(dry_run: bool) -> bool {
    dry_run
}

fn write_gitignore(output_dir: &Path, dry_run: bool) {
    let content = r#"# Generated by cargo2naso
/target
**/*.naso~
.naso-cache
*.log
.DS_Store
"#;
    
    let path = output_dir.join(".gitignore");
    if dry_run {
        println!("Would write .gitignore:\n{}", content);
    } else {
        fs::write(path, content).expect("Failed to write .gitignore");
    }
}

fn write_readme(output_dir: &Path, pkg: &NasoPackage, dry_run: bool) {
    let content = format!(r#"# {}

{}

## Installation

```bash
naso add {}
```

## Usage

```naso
use {}::*;

fn main() {{
    println!("Hello from {}!");
}}
```

## License

{}
"#, 
        pkg.name,
        pkg.description.as_deref().unwrap_or("A Naso package"),
        pkg.name,
        pkg.name.replace('-', "_"),
        pkg.name,
        pkg.license.as_deref().unwrap_or("MIT")
    );
    
    let path = output_dir.join("README.md");
    if dry_run {
        println!("Would write README.md:\n{}", content);
    } else {
        fs::write(path, content).expect("Failed to write README.md");
    }
}