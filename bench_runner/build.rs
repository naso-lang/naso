use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Tell cargo to re-run if the naso source changes
    println!("cargo:rerun-if-changed=../examples/bench_phase13b.naso");
    
    eprintln!("BUILD_SCRIPT: Starting bench_phase13b compilation");
    
    // Get the target directory
    let out_dir = env::var("OUT_DIR").unwrap();
    let target_dir = PathBuf::from(&out_dir).ancestors().nth(3).unwrap().to_path_buf();
    
    // Path to the naso compiler
    let naso_compiler = PathBuf::from("/tmp/cargo-target/release/naso");
    
    // Path to the naso-ffi static library
    let _naso_ffi_lib = PathBuf::from("/tmp/cargo-target/release/libnaso_ffi.a");
    
    // Compile bench_phase13b.naso to LLVM IR
    let ll_path = PathBuf::from(&out_dir).join("bench_phase13b.ll");
    let obj_path = PathBuf::from(&out_dir).join("bench_phase13b.o");
    
    eprintln!("BUILD_SCRIPT: Compiling with naso compiler to {}", ll_path.display());
    
    // Run naso compiler to generate LLVM IR
    let mut cmd = Command::new(&naso_compiler);
    cmd.args(["build", "--target", "llvm", "../examples/bench_phase13b.naso", "-o"])
       .arg(&ll_path);
    let status = cmd.status().expect("Failed to run naso compiler");
    assert!(status.success(), "Naso compilation failed");
    
    eprintln!("BUILD_SCRIPT: Compiling LLVM IR to object file");
    
    // Compile LLVM IR to object file using llc
    let llc_path = "/tmp/clang+llvm-14.0.6-x86_64-linux-gnu-rhel-8.4/bin/llc";
    let mut cmd = Command::new(llc_path);
    cmd.args(["-filetype=obj", ll_path.to_str().unwrap(), "-o", obj_path.to_str().unwrap()]);
    let status = cmd.status().expect("Failed to run llc");
    assert!(status.success(), "llc compilation failed");
    
    eprintln!("BUILD_SCRIPT: Creating static library");
    
    // Link the object file into a static library
    let static_lib = PathBuf::from(&out_dir).join("libbench_phase13b.a");
    let mut cmd = Command::new("ar");
    cmd.args(["rcs", static_lib.to_str().unwrap(), obj_path.to_str().unwrap()]);
    let status = cmd.status().expect("Failed to run ar");
    assert!(status.success(), "ar failed");
    
    eprintln!("BUILD_SCRIPT: Build complete");
    
    // Tell cargo to link the static library
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-search=native=/tmp/cargo-target/release");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-lib=static=bench_phase13b");
    println!("cargo:rustc-link-lib=static=naso_ffi");
    // unwind is available as shared library only
    println!("cargo:rustc-link-lib=unwind");
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=rt");
    println!("cargo:rustc-link-lib=gcc_s");
    println!("cargo:rustc-link-lib=c");
    println!("cargo:rustc-link-lib=util");
    // Allow duplicate symbols from naso_ffi's std
    println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
}