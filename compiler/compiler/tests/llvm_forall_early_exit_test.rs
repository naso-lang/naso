//! `break` and `continue` in the LLVM backend, proven by EXECUTION.
//!
//! # What made these hard
//!
//! Neither keyword was a keyword. `break` lexed as the identifier `Var("break")`,
//! so it parsed into a *variable reference*, and a program using it compiled to a
//! read of an unbound variable. Two commits back they parsed into real AST nodes
//! and were then REFUSED by every backend. This file is the point where they
//! actually leave a loop.
//!
//! # The three bugs this unlocks
//!
//! 1. **A missing `position_at_end(body)`.** The `while` emitter built the loop
//!    body wherever the builder happened to be -- the header, which already had
//!    its conditional branch. Every loop then produced an EMPTY body plus a
//!    predecessorless step block, and LLVM rejected the module as malformed.
//! 2. **Double terminators.** An `if` arm ending in `break` has already closed its
//!    block. Branching it to the merge anyway gives the block two terminators.
//!    The merge PHI had to drop the arm from its incoming list for the same reason.
//! 3. **`continue` skipping a counted loop's increment.** This is the subtle one.
//!    `continue` jumping to the loop header would run the guard again without
//!    advancing the counter, so the loop would never terminate -- an infinite loop
//!    that is indistinguishable from correct code by reading it. The fix is a
//!    dedicated step block that `continue` targets, carrying `for`'s increment.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use naso_compiler::codegen::context::{CodegenContext, CodegenTarget, OptLevel};
use naso_compiler::codegen::llvm::LLVMModuleBuilder;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique directory per call, so parallel tests never collide on a temp path.
struct CaseDir(PathBuf);

impl CaseDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p =
            std::env::temp_dir().join(format!("naso-break-{}-{}-{}", tag, std::process::id(), n));
        std::fs::create_dir_all(&p).expect("case dir");
        Self(p)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Locate `llc`/`clang`.
///
/// NOT a hardcoded path. An earlier test file used
/// `/home/linuxbrew/.linuxbrew/opt/llvm@17/bin/{name}`, which passed on the dev
/// container and failed on every CI run, because CI installs LLVM system-wide and
/// puts it on `PATH`. Preferring `PATH` and falling back to the Homebrew prefix
/// works in both places, so the suite tests the COMPILER rather than the layout of
/// one machine.
fn tool(name: &str) -> String {
    if let Ok(found) = Command::new(name).arg("--version").output()
        && found.status.success()
    {
        return name.to_string();
    }
    let brew = "/home/linuxbrew/.linuxbrew/opt/llvm@17/bin";
    if std::path::Path::new(brew).join(name).exists() {
        return format!("{brew}/{name}");
    }
    panic!(
        "`{name}` not found on PATH and not in {brew}; these tests execute generated \
             code, so they cannot be skipped."
    );
}

fn build_ir(src: &str) -> String {
    let program = naso_compiler::parser::parse_program(src)
        .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}"));
    let pir = naso_compiler::lowering::lower_program(&program)
        .unwrap_or_else(|e| panic!("source must lower:\n{src}\nerror: {e}"));
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    builder
        .build_module(&pir)
        .unwrap_or_else(|e| panic!("build_module failed:\n{src}\nerror: {e}"));
    builder.module().to_string()
}

/// Compile the IR with `llc`, link a C driver, run it, return what it printed.
fn run(ir: &str, driver: &str) -> String {
    let dir = CaseDir::new("run");
    std::fs::write(dir.join("m.ll"), ir).expect("write ll");
    std::fs::write(dir.join("d.c"), driver).expect("write driver");

    let llc = Command::new(tool("llc"))
        .args([
            "-filetype=obj",
            "-relocation-model=pic",
            "m.ll",
            "-o",
            "m.o",
        ])
        .current_dir(dir.join(""))
        .output()
        .expect("run llc");
    assert!(
        llc.status.success(),
        "llc failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&llc.stderr)
    );

    // `-lm` because `round`/`floor`/`ceil` lower to libm.
    let clang = Command::new(tool("clang"))
        .args(["-O0", "m.o", "d.c", "-lm", "-o", "prog"])
        .current_dir(dir.join(""))
        .output()
        .expect("run clang");
    assert!(
        clang.status.success(),
        "clang link failed: {}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&clang.stderr)
    );

    // The program is run UNDER A TIMEOUT, and a hang is reported as a hang.
    //
    // `Command::output()` blocks forever, which turns a non-terminating loop into a
    // stuck test suite that reports nothing: no test name, no assertion, and a CI job
    // that eventually dies on its own job timeout. The first version of this helper
    // had no timeout, so a mutation that sent `continue` to the loop header instead of
    // the step block -- skipping a counted loop's increment -- wedged the whole binary
    // with zero diagnostic output.
    //
    // The timeout is generous enough that a slow machine still passes, and short
    // enough that a real infinite loop is reported within it. These programs are
    // straight-line integer arithmetic over a handful of iterations; 20s is many
    // orders of magnitude more than they need.
    let mut child = Command::new(dir.join("prog"))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn exe");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match child.try_wait().expect("poll exe") {
            Some(_) => break,
            None => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "the compiled program did not terminate within 20s.\n\
                         A loop whose counter does not advance never ends, so this is \
                         almost always an infinite loop rather than slowness: check that \
                         `continue` targets the loop's step block (which carries the \
                         increment) and not the header.\n--- IR ---\n{ir}"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    let out = child.wait_with_output().expect("read exe output");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The error text a program is refused with, or a panic.
fn compile_error(src: &str) -> String {
    let pir = match naso_compiler::lowering::lower_program(
        &naso_compiler::parser::parse_program(src)
            .unwrap_or_else(|e| panic!("source must parse:\n{src}\nerror: {e:?}")),
    ) {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    let cc = CodegenContext::new(CodegenTarget::Host, OptLevel::None).expect("codegen context");
    let mut builder = LLVMModuleBuilder::new(&cc).expect("module builder");
    match builder.build_module(&pir) {
        Ok(()) => panic!("expected a refusal, but the program compiled:\n{src}"),
        Err(e) => e.to_string(),
    }
}

/// `break` inside an affine `forall` is refused, and the refusal is ACCURATE.
///
/// This is a diagnostics test, not an execution one, because the honest outcome is
/// that the program does not compile. What matters is the text of the refusal.
///
/// It used to say: "`break` outside a loop: no enclosing loop is being built". That
/// is false. The program has a loop -- `forall i in 0..4` -- and telling an author
/// their program has no loop sends them looking in the wrong place entirely. The
/// refusal must name the real obstacle: the band has a statically known trip count,
/// so its body is straight-line code with no runtime exit.
#[test]
fn break_inside_an_affine_forall_is_refused_with_an_accurate_reason() {
    let msg = compile_error(
        "fn f() -> i64 { let mut t = 0; forall i in 0..4 { if i == 2 { break; } t = t + i; } t }\n",
    );
    assert!(
        msg.contains("affine `forall`"),
        "the refusal must name the affine band, not claim there is no loop: {msg}"
    );
    //
    // The precise false claim is "no enclosing loop is being built" -- that is the
    // sentence the old diagnostic used, and it sent the reader looking for a missing
    // loop instead of at the real obstacle.
    //
    // This deliberately does NOT test for the bare phrase "outside a loop". The
    // message contains that phrase inside "This is NOT the same as being outside a
    // loop", which is the opposite claim. My first version of this assertion matched
    // the substring and failed against a message that was in fact correct; a test
    // crude enough to reject the right answer is worse than no test.
    assert!(
        !msg.contains("no enclosing loop is being built"),
        "the refusal must NOT claim there is no enclosing loop -- there is one: {msg}"
    );
    assert!(
        msg.contains("statically known"),
        "the refusal must say the trip count is statically known, which is why there \
         is no runtime exit to branch to: {msg}"
    );
}

/// `continue` inside an affine `forall` is refused for the same reason.
///
/// A skipped iteration is not just missing a branch: the dependence analysis that
/// scheduled the band assumed the body ran on every point of the iteration space,
/// so skipping one invalidates the schedule the pass just computed.
#[test]
fn continue_inside_an_affine_forall_is_refused_with_an_accurate_reason() {
    let msg = compile_error(
        "fn f() -> i64 { let mut t = 0; forall i in 0..4 { if i == 2 { continue; } t = t + i; } t }\n",
    );
    assert!(
        msg.contains("affine `forall`"),
        "the refusal must name the affine band: {msg}"
    );
    assert!(
        !msg.contains("no enclosing loop is being built"),
        "the refusal must NOT claim there is no enclosing loop -- there is one: {msg}"
    );
    assert!(
        msg.contains("dependence analysis"),
        "the refusal must say the schedule depends on every iteration running, which \
         is the reason skipping one is not merely unimplemented: {msg}"
    );
}

/// A `forall` with no early exit still compiles and still runs correctly.
///
/// This is the guard that stops the affine flag from being over-broad. If the frame
/// were pushed around every body -- including ones with no `break` in them -- this
/// would still pass, but a mis-scoped frame elsewhere would not. The real risk the
/// flag guards against is a refusal that fires when nothing is wrong, so the positive
/// case has to be pinned too.
#[test]
fn an_affine_forall_without_early_exit_still_compiles() {
    let ir = build_ir("fn total() -> i64 { let mut t = 0; forall i in 0..4 { t = t + i; } t }\n");
    assert!(
        ir.contains("define i64 @naso_total()"),
        "a `forall` with no `break` must compile normally:\n{ir}"
    );
}

/// The two refusals must be distinguishable from each other.
///
/// `break` and `continue` fail for related but different reasons -- `break` has no
/// exit edge to branch to, `continue` would invalidate a schedule that assumed every
/// iteration runs. A single shared message would lose that distinction, and the
/// distinction is what tells a reader which construct to reach for instead.
#[test]
fn the_two_affine_refusals_say_different_things() {
    let brk = compile_error(
        "fn f() -> i64 { let mut t = 0; forall i in 0..4 { if i == 2 { break; } t = t + i; } t }\n",
    );
    let cont = compile_error(
        "fn f() -> i64 { let mut t = 0; forall i in 0..4 { if i == 2 { continue; } t = t + i; } t }\n",
    );
    assert_ne!(
        brk, cont,
        "`break` and `continue` fail for different reasons and must not share a \
         message"
    );
}

/// A `forall` still executes correctly now that its body carries an affine frame.
///
/// The frame is pushed around EVERY band body, not only bodies containing a
/// `break`. That is deliberate -- the band emitter does not know in advance whether a
/// body will contain an early exit, and a refusal that depends on scanning the body
/// first would make validity depend on a pass nobody runs. But an over-broad frame
/// that somehow broke normal loop emission would pass every refusal test above and
/// fail the shipped kernels, so this pins the positive case by execution.
///
/// `forall i in 0..4 { t = t + i; }` sums 0+1+2+3 = 6 on the CPU. A body emitted
/// once gives 0, and off-by-one bounds give 3 or 10.
#[test]
fn an_affine_forall_still_executes_with_the_frame_in_place() {
    let ir = build_ir("fn total() -> i64 { let mut t = 0; forall i in 0..4 { t = t + i; } t }\n");
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_total(void);
int main(void){ printf("%ld\n", naso_total()); return 0; }
"#
        ),
        "6",
        "0+1+2+3 = 6 requires four real iterations of the band body.\n--- IR ---\n{ir}"
    );
}

/// A nested `while` inside a `forall` body must still work.
///
/// This is the case that would expose an over-broad affine frame. The inner `while`
/// pushes a RUNTIME frame, so `break` inside it must still be allowed -- it resolves
/// to the innermost frame, which is the runtime loop, not the band. If the band frame
/// were consulted first, or if the inner push did not shadow it, this would be
/// refused and a legitimate program would stop compiling.
#[test]
fn a_break_in_a_while_inside_a_forall_resolves_to_the_runtime_loop() {
    let ir = build_ir(
        "\
fn mixed() -> i64 {
    let mut t = 0;
    forall i in 0..2 {
        let mut j = 0;
        while j < 10 { j = j + 1; if j > 2 { break; } }
        t = t + j;
    }
    t
}
",
    );
    assert_eq!(
        run(
            &ir,
            r#"
#include <stdio.h>
long naso_mixed(void);
int main(void){ printf("%ld\n", naso_mixed()); return 0; }
"#
        ),
        "6",
        "each of the 2 band iterations runs the inner `while` to j = 3, giving 3 + 3 \
         = 6. A refusal here would mean the affine frame shadowed the runtime loop \
         frame that `break` should target.\n--- IR ---\n{ir}"
    );
}
