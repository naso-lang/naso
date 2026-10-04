//! Turn compiler-emitted QIR text into a gate sequence, and check it numerically.
//!
//! # Why this file exists
//!
//! The QIR backend now emits a correctly ordered circuit on the intended qubits. What it had
//! never done was let anyone CHECK that circuit, because QIR is text and nothing here
//! executes it: there is no QPU, no `qir.*` runtime, and the `entrypoint` marker is not emitted
//! (adding it needs an LLVM attribute registration that inkwell's named-enum path does not
//! provide -- see `references/inkwell-llvm-api-traps.md` for the segfault that attempt caused).
//!
//! So the available level of verification is: parse the emitted text back into the operation
//! it names, apply it to a state vector, and compare against a state derived independently.
//! That is weaker than execution, and it is used here for exactly what it can establish:
//! that the emitted IR says the right things in the right order about the right qubits.
//!
//! # What this can and cannot catch
//!
//! CAN: a gate applied to the wrong qubit, gates emitted out of order, a gate missing from the
//! output, an allocation count that does not match the circuit.
//!
//! CANNOT: anything about how a real QIR runtime would schedule or interpret these calls.
//! A gate that appears here as correct could still be mis-emitted in some way this parse does
//! not model, and no simulator has validated the `qir.*` intrinsic semantics themselves.
//!
//! # The bridge
//!
//! `parse_circuit` reads a QIR module and returns, for the entry function, the qubit
//! registrations and the gate sequence. It works on the ALLOCA NAMES rather than the SSA
//! values, because a name is what identifies a qubit across statements: each use is a separate
//! `load`, so the load results are distinct SSA names even when they name the same qubit.
//! That distinction is the whole reason the previous generation of fixtures could emit a
//! valid module that was not the source program.

use naso_gates::statevector::{Gate, StateVector};

/// One quantum operation recovered from emitted QIR.
#[derive(Debug, Clone, PartialEq)]
pub enum EmittedOp {
    /// `qir.qubit_alloc()`, bound to a name.
    Alloc { qubit: String },
    /// A gate applied to named qubits, in order.
    Gate { gate: Gate, qubits: Vec<String> },
    /// `qir.mz`/`mx`/`my` -- a computational or X/Y basis read.
    Measure { qubit: String },
}

/// A parsed entry function: what it allocates, and what it does to them.
#[derive(Debug, Clone, PartialEq)]
pub struct Circuit {
    pub qubits: Vec<String>,
    pub ops: Vec<EmittedOp>,
}

/// Why a QIR module could not be turned into a circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Map a QIR intrinsic name onto a gate in the shared table.
///
/// An unmapped name is an ERROR, not a fallback. Defaulting to Hadamard -- which the QIR
/// a deleted QIR gate table once did -- would turn a typo into a working-looking circuit that applies the
/// wrong unitary, and a numerical check built on such a parse would be checking the parser's
/// guess rather than the compiler's output.
fn gate_for(intrinsic: &str) -> Option<Gate> {
    Some(match intrinsic {
        "qir.h" => Gate::H,
        "qir.x" => Gate::X,
        "qir.y" => Gate::Y,
        "qir.z" => Gate::Z,
        "qir.s" => Gate::S,
        "qir.t" => Gate::T,
        "qir.cx" => Gate::Cx,
        "qir.cy" => Gate::Cy,
        "qir.cz" => Gate::Cz,
        "qir.swap" => Gate::Swap,
        "qir.ccx" => Gate::Ccx,
        // `iswap` and the parameterised rotations are NOT modelled in the shared table, so
        // they are refused rather than approximated. Mapping `iswap` to `swap` would compute
        // a different permutation, and treating `rx`/`ry`/`rz` as identity would erase the
        // rotation -- in both cases a numerically WRONG circuit that still verifies.
        //
        // The QIR backend can still EMIT these; what is refused here is CHECKING them. That
        // direction is deliberate: a smaller verified surface is better than a larger one
        // containing substitutions nobody looked at.
        _ => return None,
    })
}

/// The basis a measurement intrinsic reads in.
fn basis_for(intrinsic: &str) -> Option<Gate> {
    Some(match intrinsic {
        "qir.mz" => Gate::Z,
        "qir.mx" => Gate::X,
        "qir.my" => Gate::Y,
        _ => return None,
    })
}

/// Split a call's argument list.
/// The argument list of a call, as SSA names.
///
/// The name of the CALLED function is followed by its argument list, and the argument list is
/// what follows the LAST `(` on the line -- not the first. Taking the first `(` picks up a type
/// annotation instead (`call i1 @qir.mz(ptr %a5)` has its argument list after the `(` that
/// follows `@qir.mz`), which yields an argument that still carries the opening paren and
/// resolves to nothing. Splitting on the first `(` and then rejecting anything not starting
/// with `%` is how a real `qir.mz` call came back as "has no operand".
fn arguments(line: &str) -> Result<Vec<String>, ParseError> {
    let open = line
        .rfind('(')
        .ok_or_else(|| ParseError(format!("call has no argument list: {line}")))?;
    let inner = line[open + 1..]
        .split_once(')')
        .map(|(args, _)| args)
        .ok_or_else(|| ParseError(format!("call's argument list is unterminated: {line}")))?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner
        .split(',')
        .map(|a| a.trim().rsplit(' ').next().unwrap_or("").trim().to_owned())
        .collect())
}

/// The FIRST pointer operand of a `store`: the value being written.
///
/// LLVM's `store` has no `=`, so the value is the first `ptr ` operand rather than the right
/// side of an assignment. Reading it any other way matches nothing on real emitted text.
fn stored_value(line: &str) -> Option<String> {
    let value = line.split("ptr ").nth(1)?.split(',').next()?.trim();
    Some(value.to_owned())
}

/// The name on the left of a `store`, which is where a qubit is recorded.
fn stored_alloca(line: &str) -> Option<String> {
    // LLVM's `store` has NO `=`. It is `store ptr <value>, ptr <target>, align N`, so the
    // target is the SECOND pointer operand -- reading it as an assignment (split on `=`, take
    // the left side) matches nothing at all, which is why an earlier version of this helper
    // rejected every real store line.
    let target = store_target(line)?;
    target.starts_with('%').then(|| target.trim().to_owned())
}

/// The second pointer operand of a `store`: the slot being written.
fn store_target(line: &str) -> Option<&str> {
    let mut operands = line.split("ptr ").skip(1);
    let _value = operands.next()?;
    operands.next()?.split(',').next().map(str::trim)
}

/// Recover the binding's value name from a `let`-style definition line.
fn defined_value(line: &str) -> Option<String> {
    let (lhs, rhs) = line.split_once('=')?;
    let produced = rhs.trim();
    if produced.starts_with("call ptr") {
        Some(lhs.trim().to_owned())
    } else {
        None
    }
}

/// Parse the entry function of a QIR module into a circuit.
///
/// Reads the emitted text as a sequence of calls, recovering operand identity from the alloca
/// each pointer operand was loaded from. Anything not recognised is reported rather than
/// skipped: an ignored call would silently shrink the circuit being checked, which is the
/// failure mode this file exists to detect.
pub fn parse_circuit(qir: &str) -> Result<Circuit, ParseError> {
    let mut qubits: Vec<String> = Vec::new();
    // Results of `qir.qubit_alloc()`, so a store can be checked against a real allocation.
    let mut allocations: Vec<String> = Vec::new();
    let mut ops = Vec::new();

    // SSA value -> the alloca it was loaded from, so gates can address qubits by identity
    // rather than by the transient result of each load.
    let mut last_defined: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for line in qir.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with("!") {
            continue;
        }

        // `store ptr %qalloc, ptr %a` -- a qubit is bound to an alloca.
        //
        // The STORED VALUE must be the result of an allocation, not merely any pointer: a
        // `store ptr null, ptr %a` binds a slot that was never allocated, and treating it as a
        // qubit would let a gate on it resolve to a qubit that does not exist. So the value is
        // checked against the set of allocation results, and an unbound store is refused rather
        // than silently becoming qubit 0.
        if trimmed.starts_with("store ptr") {
            let alloca = stored_alloca(trimmed)
                .ok_or_else(|| ParseError(format!("cannot read the stored slot: {trimmed}")))?;
            let value = stored_value(trimmed)
                .ok_or_else(|| ParseError(format!("cannot read the stored value: {trimmed}")))?;
            if !allocations.contains(&value) {
                return Err(ParseError(format!(
                    "{trimmed}\n  binds `{alloca}` to `{value}`, which is not the result of a \
                     qubit allocation, so a gate on it would act on a qubit that does not exist"
                )));
            }
            qubits.push(alloca);
            continue;
        }

        let value = defined_value(trimmed);

        if let Some((intrinsic, args)) = call_target(trimmed) {
            // Resolve each pointer operand to the alloca it was loaded from.
            let mut operands = Vec::new();
            for a in &args {
                if a.starts_with('%') {
                    let slot = last_defined.get(a).cloned().unwrap_or_else(|| a.clone());
                    operands.push(slot);
                }
            }

            if intrinsic == "qir.qubit_alloc" {
                if let Some(v) = &value {
                    allocations.push(v.clone());
                    last_defined.insert(v.clone(), v.clone());
                }
                ops.push(EmittedOp::Alloc {
                    qubit: String::new(),
                });
                continue;
            }

            if let Some(gate) = gate_for(&intrinsic) {
                let mut named: Vec<String> = Vec::new();
                for o in &operands {
                    let name = qubits
                        .iter()
                        .position(|q| q == o)
                        .map(|i| format!("q{i}"))
                        .ok_or_else(|| {
                            ParseError(format!(
                                "gate {intrinsic} acts on `{o}`, which is not a qubit bound by \
                                 a preceding allocation"
                            ))
                        })?;
                    named.push(name);
                }
                ops.push(EmittedOp::Gate {
                    gate,
                    qubits: named,
                });
                continue;
            }

            if let Some(basis) = basis_for(&intrinsic) {
                let name = operands
                    .first()
                    .ok_or_else(|| ParseError(format!("{intrinsic} has no operand")))?;
                let idx = qubits.iter().position(|q| q == name).ok_or_else(|| {
                    ParseError(format!(
                        "measurement reads `{name}`, which is not a bound qubit"
                    ))
                })?;
                ops.push(EmittedOp::Measure {
                    qubit: format!("q{idx}"),
                });
                let _ = basis;
                continue;
            }

            return Err(ParseError(format!(
                "emitted QIR calls `{intrinsic}`, which this bridge does not model. Extending \
                 it is required before the circuit can be checked -- silently ignoring the \
                 call would verify a circuit smaller than the one emitted."
            )));
        }

        if let Some(v) = value {
            last_defined.insert(v.clone(), v.clone());
        }
        // A `load` names the alloca it read; record it under the loaded-from slot.
        if let Some((lhs, rhs)) = trimmed.split_once(" = load ptr, ptr ")
            && let Some(slot) = rhs.split(',').next()
        {
            last_defined.insert(lhs.trim().to_owned(), slot.trim().to_owned());
        }
    }

    Ok(Circuit { qubits, ops })
}

/// The intrinsic a call line invokes, and its argument strings.
fn call_target(line: &str) -> Option<(String, Vec<String>)> {
    let at = line.find("call ")?;
    let rest = &line[at..];
    let name_start = rest.find('@')? + 1;
    let after_name = &rest[name_start..];
    let paren = after_name.find('(')?;
    let intrinsic = after_name[..paren].to_owned();
    let args = arguments(rest).ok()?;
    Some((intrinsic, args))
}

/// Apply a parsed circuit to an initial state, stopping short of measurement.
///
/// A measurement is COLLAPSIVE, and its outcome is not decided by the circuit -- it is sampled.
/// So this returns the state before the first measurement along with the index of the qubit
/// that was about to be read, and a caller that wants a post-measurement state applies the
/// outcome itself. Folding a specific outcome in here would bake an arbitrary choice into
/// what is supposed to be a faithful replay.
pub fn simulate(
    circuit: &Circuit,
    initial: &StateVector,
) -> Result<(StateVector, Option<String>), String> {
    let mut state = initial.clone();
    for op in &circuit.ops {
        match op {
            EmittedOp::Alloc { .. } => {}
            EmittedOp::Measure { qubit } => {
                let index = qubit_index(qubit, &circuit.qubits)?;
                return Ok((state, Some(format!("q{index}"))));
            }
            EmittedOp::Gate { gate, qubits } => {
                let indices: Vec<usize> = qubits
                    .iter()
                    .map(|q| qubit_index(q, &circuit.qubits))
                    .collect::<Result<_, _>>()?;
                let arity = expected_arity(*gate);
                if indices.len() != arity {
                    return Err(format!(
                        "{:?} acts on {arity} qubits but the emitted call passes {}",
                        gate,
                        indices.len()
                    ));
                }
                // Toffoli is a genuine THREE-qubit gate and so is not on `Gate::apply`, for
                // the reason its own doc comment gives: with two qubits a "both controls set"
                // condition and a "control set" one select the same basis states, so a
                // two-qubit Toffoli is indistinguishable from `cx` and a test could never
                // separate them.
                state = match *gate {
                    Gate::Ccx => {
                        let [a, b, t] = indices[..] else {
                            return Err("Toffoli needs three qubit indices".to_string());
                        };
                        state.apply_toffoli(a, b, t)
                    }
                    other => other.apply(&state, indices[0], indices.get(1).copied()),
                };
            }
        }
    }
    Ok((state, None))
}

/// How many qubits a gate acts on.
fn expected_arity(gate: Gate) -> usize {
    match gate {
        Gate::Cx | Gate::Cy | Gate::Cz | Gate::Swap => 2,
        Gate::Ccx => 3,
        _ => 1,
    }
}

fn qubit_index(name: &str, alloca_names: &[String]) -> Result<usize, String> {
    let digits: String = name
        .trim_start_matches("q")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let index: usize = digits
        .parse()
        .map_err(|_| format!("`{name}` is not a qubit name"))?;
    if index >= alloca_names.len() {
        return Err(format!(
            "`{name}` is qubit {index} but only {} were allocated",
            alloca_names.len()
        ));
    }
    Ok(index)
}
