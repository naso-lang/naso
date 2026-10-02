// Per-function LLVM symbol emission: one `define` per Naso function, and the
// call graph that decides which order they are emitted in.
//
// WHY THIS EXISTS
// ===============
//
// A `PirModule` used to be one flat statement list, and `LLVMModuleBuilder::build_module`
// emitted it as a single `naso_entry`. Every Naso function's parameters and locals
// therefore shared one namespace in one LLVM body. `kernels/quant_int8.naso` could not
// compile at all: its three functions each declare `input`/`output`/`scale`, at
// `Tensor[f32, 1024]` in one and `Tensor[i8, 1024]` in another, and one LLVM function
// has ONE slot per name.
//
// `PirModule::functions` now carries one `PirFunction` per Naso function, each with its
// own parameters, statements, schedule and quantity map. This module turns that list
// into one LLVM symbol per function, callee-before-caller.
//
// THE NAMING CONVENTION
// =====================
//
// A Naso function `f` becomes the LLVM symbol `naso_f`, produced by ONE function,
// [`symbol_for`], which is also what a `PirExpr::Call` resolves through. There is no
// second place that spells the name: if emission and lookup disagreed, a caller would
// look up a symbol that does not exist, and LLVM would emit a `call` to an undeclared
// function and fail verification with a message that names neither the Naso function nor
// the convention.
//
// The entry function is emitted TWICE over: as `naso_<name>` like every other function,
// and, when it is the selected entry, ALSO under [`ENTRY_SYMBOL`]. See
// [`entry_symbol`] for why it is not simply called `naso_entry`.
//
// WHY A SEPARATE FILE
// ====================
//
// `module_builder.rs` was already 600 lines and is about the LLVM `Module` itself. This
// is a self-contained question -- "in what order, under what names, with what cycle
// check" -- that can be answered and tested without an inkwell builder at all, and the
// call-graph half of it does not touch LLVM. Putting it here means the recursion refusal
// and the naming convention are unit-testable as pure data.

use crate::codegen::error::{CodegenError, CodegenResult};
use crate::ir::pir_types::{PirExpr, PirFunction};
use std::collections::{HashMap, HashSet};

/// The symbol a Naso function `name` is emitted under, and that a call site resolves.
///
/// `naso_` prefixed so a generated symbol cannot collide with a C library symbol the
/// module links against, and so an unprefixed name in the IR is visibly not one of ours.
/// The prefix is not configurable: a configurable prefix would mean a caller had to know
/// which prefix the module was built with, and a mismatch is a link error at the far end
/// rather than a diagnostic here.
pub fn symbol_for(name: &str) -> String {
    format!("naso_{name}")
}

/// The symbol that is ALSO exported under the module's conventional entry name.
///
/// The entry is emitted under `naso_<name>` like every other function, so a Naso-to-Naso
/// call to it resolves by the same rule as any other call. It is additionally emitted
/// under this name so the existing C drivers, the shipped-kernel execution tests and any
/// external caller keep working against an unchanged symbol.
pub const ENTRY_SYMBOL: &str = "naso_entry";

/// Which function the module exports as its entry point.
///
/// # The rule
///
/// A function named `main` if there is one, otherwise the FIRST function in source
/// order. Source order, not "the only one", because a one-function program and a
/// three-function program then agree: the entry is the function the author wrote first,
/// which for a single-function module is the same function the old flat backend emitted.
///
/// # Why `main` wins
///
/// `main` is the one name a Naso program already uses to mean "run me" -- `lower_program`'s
/// parameter extraction, and every shipped kernel's driver, treat it specially. A module
/// whose author named a function `main` has said which one they meant, and overriding
/// that with "first in source order" would silently run a different function than the one
/// they marked as the entry.
///
/// # Why this is decided rather than left to the driver
///
/// A module with three functions has THREE plausible entry points. Emitting all three,
/// or none, and hoping the linker picks, is how a program ends up running the wrong
/// kernel with no diagnostic. One function is designated, every other function is
/// callable only from Naso or from an external caller naming its own symbol, and the
/// choice is a documented rule rather than a side effect of emission order.
pub fn entry_function_index(functions: &[PirFunction]) -> Option<usize> {
    if functions.is_empty() {
        return None;
    }
    functions.iter().position(|f| f.name == "main").or(Some(0))
}

/// A function's primary LLVM symbol: `naso_<name>`.
///
/// ONE name for EVERY function, the entry included. A call site must not have to know
/// whether its callee happens to be the entry: if the entry were resolved as
/// `naso_entry` and everything else as `naso_<name>`, then whether `PirExpr::Call` found
/// the right symbol would depend on which function the author happened to mark as the
/// entry. The entry's second symbol is an external convenience, never a call target.
pub fn primary_symbol(index: usize, functions: &[PirFunction]) -> String {
    symbol_for(&functions[index].name)
}

/// Every symbol a function is emitted under.
///
/// The entry gets its primary symbol plus [`ENTRY_SYMBOL`]; every other function gets
/// its primary symbol alone. See [`entry_function_index`] for how the entry is chosen.
pub fn symbols_for(index: usize, functions: &[PirFunction], entry: Option<usize>) -> Vec<String> {
    let mut out = vec![primary_symbol(index, functions)];
    if entry == Some(index) {
        // The entry is emitted ONCE, under BOTH names. Two `define`s would emit the
        // body twice, which doubles the code for no benefit; instead the extra symbol is
        // a thin `tail call` shim into the primary, which `module_builder` emits. A
        // shim rather than an LLVM `alias` because the shim can carry the entry ABI
        // verbatim -- `alias` would need the two symbols to have identical types, and
        // the entry alias is the one place a signature is spelled twice.
        out.push(ENTRY_SYMBOL.to_string());
    }
    out
}

/// Functions in an order where every callee precedes its callers, with recursion refused.
///
/// DFS post-order over the call graph, the same algorithm and the same diagnostic style
/// as `codegen::wgsl_straight::emission_order`. LLVM is not WGSL: a callee may be
/// *called* before it is *defined* if it is declared first, so LLVM does not strictly
/// need this order. It is used anyway because a forward reference would have to be
/// declared with a signature computed a second time from a second place, and two
/// computations of one signature is exactly how a caller ends up disagreeing with the
/// callee about an argument's type.
///
/// # Why recursion is refused rather than emitted
///
/// LLVM will happily compile `f` calling `f` into an unbounded call chain: no stack
/// check, no diagnostic, an infinite loop or a stack overflow at run time. Naso's
/// quantitative typing makes that worse than merely slow -- a `[1]` linear resource
/// would be consumed along a path the typechecker never checked, because the typechecker
/// reasons about one finite body. LLVM accepting it says nothing about whether the
/// LANGUAGE accepts it, and a silent wrong answer here is the failure this compiler is
/// built to avoid. So it is an error naming the function, at the same place WGSL refuses
/// it.
///
/// Mutual recursion is refused by the same check and named as such: there is no ordering
/// of the emission that satisfies it.
pub fn emission_order(functions: &[PirFunction]) -> CodegenResult<Vec<usize>> {
    let by_name: HashMap<&str, usize> = functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();

    let mut order: Vec<usize> = Vec::with_capacity(functions.len());
    // `done` guards against re-emitting; `on_stack` detects cycles.
    let mut done: HashSet<usize> = HashSet::new();
    let mut on_stack: Vec<usize> = Vec::new();

    // An explicit stack rather than recursion: a call chain as deep as the program is
    // long would overflow the compiler's own stack, and a compiler that crashes on legal
    // input is its own silent wrong answer.
    for root in 0..functions.len() {
        if done.contains(&root) {
            continue;
        }
        let mut work: Vec<(usize, bool)> = vec![(root, false)];
        while let Some((index, expanded)) = work.pop() {
            if expanded {
                on_stack.retain(|i| *i != index);
                if done.insert(index) {
                    order.push(index);
                }
                continue;
            }
            if done.contains(&index) {
                continue;
            }
            if let Some(pos) = on_stack.iter().position(|i| *i == index) {
                // Name the whole cycle, not just the edge that closed it: "recursive
                // call to `f`" is unhelpful when the cycle is `f -> g -> f`, because it
                // does not say `g` is involved.
                let mut cycle: Vec<&str> = on_stack[pos..]
                    .iter()
                    .map(|i| functions[*i].name.as_str())
                    .collect();
                cycle.push(functions[index].name.as_str());
                return Err(CodegenError::UnsupportedFeature(format!(
                    "recursive call is not supported: {}. Naso functions do not \
                     recurse, and LLVM would compile this into an unbounded call chain \
                     with no diagnostic: a `[1]` linear resource would then be consumed \
                     along a path the typechecker never checked, because it reasons \
                     about one finite body.",
                    cycle.join(" -> ")
                )));
            }
            on_stack.push(index);
            work.push((index, true));
            // Callees pushed first, so they are visited -- and emitted -- first.
            let mut callees = called_indices(&functions[index], &by_name);
            callees.reverse();
            for c in callees {
                if !done.contains(&c) {
                    work.push((c, false));
                }
            }
        }
    }
    Ok(order)
}

/// Indices of the functions `func` calls, in first-appearance order, deduplicated.
///
/// A call to a name that is NOT one of this module's functions is not an edge here: it
/// is a prelude intrinsic (`clamp`, `round`) or an `extern` declaration, and the
/// expression lowering resolves those separately. Treating an unknown name as a cycle
/// would refuse `forall i in 0..n { .. clamp(..) .. }`, which is not recursive.
fn called_indices(func: &PirFunction, by_name: &HashMap<&str, usize>) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for stmt in &func.statements {
        collect_called_in_expr(&stmt.body, by_name, &mut out);
    }
    out
}

/// Every child expression, so the call graph walk cannot miss one.
///
/// A function is a separate ARM per `PirExpr` variant that has children. Adding a
/// variant with children and forgetting to add it here would make a call inside that
/// construct invisible to the cycle check -- so a recursive function could be emitted
/// as an infinite chain with no diagnostic, which is the exact failure this walk
/// exists to prevent.
///
/// The walk is deliberately OVER-APPROXIMATE for erasure: a call inside a `[0]`
/// expression is followed even though the expression is erased. Over-approximating can
/// refuse a program that would have been fine; under-approximating would accept one
/// that is wrong at run time. The trade is deliberate.
fn collect_called_in_expr(expr: &PirExpr, by_name: &HashMap<&str, usize>, out: &mut Vec<usize>) {
    match expr {
        PirExpr::Call { name, args } => {
            if let Some(index) = by_name.get(name.as_str())
                && !out.contains(index)
            {
                out.push(*index);
            }
            for a in args {
                collect_called_in_expr(a, by_name, out);
            }
        }
        PirExpr::Binary { left, right, .. } => {
            collect_called_in_expr(left, by_name, out);
            collect_called_in_expr(right, by_name, out);
        }
        PirExpr::Unary { expr, .. } | PirExpr::Cast { expr, .. } => {
            collect_called_in_expr(expr, by_name, out);
        }
        PirExpr::Index { base, indices } => {
            collect_called_in_expr(base, by_name, out);
            for i in indices {
                collect_called_in_expr(i, by_name, out);
            }
        }
        PirExpr::Field { base, .. } => collect_called_in_expr(base, by_name, out),
        PirExpr::Assign { target, value } => {
            collect_called_in_expr(target, by_name, out);
            collect_called_in_expr(value, by_name, out);
        }
        PirExpr::Let { value, body, .. } => {
            collect_called_in_expr(value, by_name, out);
            collect_called_in_expr(body, by_name, out);
        }
        PirExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_called_in_expr(cond, by_name, out);
            collect_called_in_expr(then_branch, by_name, out);
            collect_called_in_expr(else_branch, by_name, out);
        }
        PirExpr::Reversible { body, inverse } => {
            collect_called_in_expr(body, by_name, out);
            collect_called_in_expr(inverse, by_name, out);
        }
        PirExpr::QuantumOp { args, qubits, .. } => {
            for a in args.iter().chain(qubits.iter()) {
                collect_called_in_expr(a, by_name, out);
            }
        }
        PirExpr::Stmts(stmts) => {
            for s in stmts {
                collect_called_in_expr(s, by_name, out);
            }
        }
        // Leaves: `IntLit`, `FloatLit`, `BoolLit`, `Var`. There is nothing to walk, and
        // naming them here would be dead code the day a leaf grows children.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Mutability, Quantity, Span};
    use crate::ir::access_relation::AccessRelations;
    use crate::ir::pir_types::{ElemType, FnReturn, ParamKind, PirStatement};
    use crate::ir::schedule_tree::{ScheduleNode, ScheduleTree, StmtId};

    pub(crate) fn mk(name: &str, body: Vec<PirExpr>) -> PirFunction {
        let statements: Vec<PirStatement> = body
            .into_iter()
            .enumerate()
            .map(|(i, b)| PirStatement {
                id: StmtId(i),
                domain: crate::ir::affine_domain::AffineDomain::universe(0, 0),
                body: b,
                quantity: Quantity::Many,
                mutability: Mutability::Immutable,
                span: None,
            })
            .collect();
        PirFunction {
            name: name.to_string(),
            params: vec![FunctionParamHelper::scalar_param("x")],
            schedule: ScheduleTree::new(ScheduleNode::Empty, vec![]),
            statements,
            accesses: AccessRelations::new(),
            quantities: HashMap::new(),
            return_type: FnReturn::Void,
            return_stmt: None,
            span: Some(Span::default()),
        }
    }

    /// A tiny helper so the tests above read as one line per parameter.
    struct FunctionParamHelper;
    impl FunctionParamHelper {
        fn scalar_param(name: &str) -> crate::ir::pir_types::FunctionParam {
            crate::ir::pir_types::FunctionParam {
                name: name.to_string(),
                kind: ParamKind::Scalar(ElemType::F64),
                quantity: Quantity::Many,
                mutability: Mutability::Immutable,
            }
        }
    }

    fn call(name: &str) -> PirExpr {
        PirExpr::Call {
            name: name.to_string(),
            args: vec![],
        }
    }

    #[test]
    fn symbol_convention_is_naso_prefix() {
        // PINNED. The call site in `expr_lowering` resolves through this same
        // function, so the convention is asserted in one place and used in two.
        assert_eq!(
            symbol_for("quantize_int8_symmetric"),
            "naso_quantize_int8_symmetric"
        );
        assert_eq!(symbol_for("f"), "naso_f");
    }

    #[test]
    fn entry_is_main_when_present() {
        let fs = vec![mk("helper", vec![]), mk("main", vec![])];
        assert_eq!(entry_function_index(&fs), Some(1));
    }

    #[test]
    fn entry_is_first_when_no_main() {
        let fs = vec![mk("a", vec![]), mk("b", vec![])];
        assert_eq!(entry_function_index(&fs), Some(0));
    }

    #[test]
    fn entry_of_no_functions_is_none() {
        assert_eq!(entry_function_index(&[]), None);
    }

    #[test]
    fn callee_is_emitted_before_caller() {
        // `caller` is declared FIRST in source order but calls `callee`, so the order
        // must be callee-first. This is the property that makes a `call` reference a
        // real `define`.
        let fs = vec![mk("caller", vec![call("callee")]), mk("callee", vec![])];
        let order = emission_order(&fs).unwrap();
        let names: Vec<&str> = order.iter().map(|i| fs[*i].name.as_str()).collect();
        assert_eq!(names, vec!["callee", "caller"]);
    }

    #[test]
    fn self_recursion_is_refused() {
        let fs = vec![mk("f", vec![call("f")])];
        let err = emission_order(&fs).unwrap_err().to_string();
        assert!(err.contains("recursive call"), "{err}");
        // The cycle is named, so a reader is not left guessing which function.
        assert!(err.contains("f -> f"), "{err}");
    }

    #[test]
    fn mutual_recursion_is_refused_and_names_the_cycle() {
        let fs = vec![mk("f", vec![call("g")]), mk("g", vec![call("f")])];
        let err = emission_order(&fs).unwrap_err().to_string();
        assert!(err.contains("recursive call"), "{err}");
        assert!(err.contains("f -> g -> f"), "{err}");
    }

    #[test]
    fn an_unknown_callee_is_not_an_edge() {
        // `clamp` is a prelude intrinsic, not a function in this module. Treating it as
        // an edge would either refuse the module or invent an order for a callee that
        // does not exist.
        let fs = vec![mk("f", vec![call("clamp")])];
        let order = emission_order(&fs).unwrap();
        assert_eq!(order, vec![0]);
    }

    #[test]
    fn a_call_nested_in_an_argument_is_still_an_edge() {
        // The mutation this catches: a walker that only looks at the TOP level of a
        // call's argument list, or that forgets to recurse into `args` at all, would
        // emit the caller first and produce a `call` to an undeclared symbol.
        let fs = vec![
            mk(
                "caller",
                vec![PirExpr::Call {
                    name: "outer".to_string(),
                    args: vec![call("inner")],
                }],
            ),
            mk("inner", vec![]),
            mk("outer", vec![]),
        ];
        let order = emission_order(&fs).unwrap();
        let names: Vec<&str> = order.iter().map(|i| fs[*i].name.as_str()).collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("inner") < pos("caller"), "{names:?}");
    }
}
#[cfg(test)]
mod entry_symbol_tests {
    use super::tests::mk;
    use super::*;

    #[test]
    fn a_non_entry_function_has_exactly_one_symbol() {
        let fs = vec![mk("main", vec![]), mk("helper", vec![])];
        assert_eq!(
            symbols_for(1, &fs, Some(0)),
            vec!["naso_helper".to_string()]
        );
    }

    #[test]
    fn the_entry_has_its_own_name_and_the_conventional_one() {
        // Both, and the primary is FIRST, because `symbols_for` is used to emit the
        // real body under the first name and the shim under the second. Reversing them
        // would emit the shim as the body, so the module's actual entry would be a
        // function that only tail-calls the real one -- which works, but makes every
        // backtrace point at the shim.
        let fs = vec![mk("main", vec![])];
        assert_eq!(
            symbols_for(0, &fs, Some(0)),
            vec!["naso_main".to_string(), "naso_entry".to_string()]
        );
    }

    #[test]
    fn primary_symbol_is_the_same_for_the_entry_as_for_any_other() {
        // The property that stops a call site from having to special-case the entry.
        let fs = vec![mk("main", vec![]), mk("helper", vec![])];
        assert_eq!(primary_symbol(0, &fs), "naso_main");
        assert_eq!(primary_symbol(1, &fs), "naso_helper");
        assert_ne!(primary_symbol(0, &fs), ENTRY_SYMBOL);
    }
}
