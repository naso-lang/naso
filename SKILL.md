---
name: naso-mutation-harness
description: 'Use when mutation-testing naso. Anti-false-SURVIVOR guards.'
---

# Mutation-testing the naso repo

A harness that reports SURVIVED when nothing ran is worse than no harness: it
manufactures false alarms about the tests. Four guards, all required.

## 1. The mutation must be PROVEN APPLIED

`mutate` asserts the anchor was found AND that `git diff --quiet -- <file>` is
false afterwards. A missing anchor is `MUTATION_NOT_APPLIED`, never SURVIVED.

**This bit twice here.** A shell `git checkout <file>` in the restore step
silently reverted the uncommitted work the whole change lived in, so every
mutation became a no-op. A second time, an anchor embedded `\n` inside a Rust
string literal and was mangled by a second layer of shell quoting — route any
mutation touching string literals through a Python script instead.

Restore from a **file backup**, never `git checkout`: the work under test is
uncommitted, so `git checkout` destroys it.

## 2. Never `cargo clean`; prune harder than the recipe

`ls -t *.rlib | tail -n +30` leaves 29 rlibs, which is NOT enough for this
workspace. A run then dies with `failed to build archive: No space left on
device`, and a disk-full linker death looks exactly like a mutation failure.
Use: delete the 60 largest `*.rlib`/`*.rmeta`, then `tail -n +8` on each, delete
stale test binaries, and `rm -rf debug/incremental`.

## 3. Distinguish COMPILE errors from TEST failures

`error: test failed` is a KILLED mutation. `error: could not compile` /
`error[E....]` is INCONCLUSIVE. Matching on a bare `^error` files every killed
mutation as a build error.

## 4. Require the binary to have RUN

Count `^test result:` lines. Zero means DID_NOT_RUN (disk full, link died), which
is inconclusive — never "no test failed".

## 4b. Sum failures over EVERY test binary, never the first match

`re.search(r"(\d+) passed; (\d+) failed", out)` returns the FIRST test binary's
line. `cargo test -p naso-compiler` runs dozens of binaries; a real failure in a
later one is invisible, and the mutant is reported SURVIVED. This produced a
false survivor on the core of an erasure-semantics fix — the mutant was in fact
killed by two tests.

Use the counting form, which does not care about order:

```python
passed = sum(int(m) for m in re.findall(r"(\d+) passed", out))
failed = sum(int(m) for m in re.findall(r"(\d+) failed", out))
```

Anchoring the regex to `test result: ok\. (\d+) passed` and reading one
alternation group has the same defect in a subtler form: a `FAILED` line leaves
the group empty and the total silently reads zero.

## Equivalent mutants are not evidence

A mutation guarded by `if std::env::var("SOME_UNSET_VAR").is_ok()` cannot change
behaviour and will always survive. It proves nothing about the tests. Make the
mutation unconditional, and if a survivor is suspicious, check whether the
mutant is actually reachable before believing it.

Before calling a survivor equivalent, PROVE unreachability from the frontend, not
from the arm's own code. A `naso-verify` call-site walker arm for
`ExprKind::Let`/`LetInOut`/`LetConsume` survived because the parser only ever
builds `StmtKind::Let` — grepping the *parser* for construction settled it, where
reading the arm would not.

## Prefer the compile error over the mutation

A `match` with a silent `_ => {}` arm hides every construct it does not
recognise, and the tests cannot see that. Rewrite it with NO catch-all so a new
variant is a compile error. In `naso-verify`'s call-site walker this immediately
found two unhandled variants and exposed the real bug: `return f(x)` is
`ExprKind::Return`, not `StmtKind::Return`, so the pass had been skipping the
most common call spelling in the language while reporting success.

Corollary for any new test: a checker whose VIOLATING case does not fail is not
a checker. Pair every satisfied case with a violated one and a no-call negative
control, and prefer a table of positions over a single spelling — the tail
expression of a function body is not one of its statements, which is a real skip
that only a position table exposed.

## Two filters without `--` is an INVALID setup, not a kill

`cargo test --lib F1 F2` passes both filters as libtest positionals, which
libtest REJECTS (`unexpected argument`), so rc != 0. A naive harness then labels
the mutant KILLED even though NO test ran. This fabricated a false 5/5 once the
`round` axiom was introduced: the filters `round_error_bounds_discharge_from_the_axiom`
and `the_shipped_int8_quantiser_kernel_discharges_completely` were passed without ` -- `,
cargo errored out, and the mutant was reported killed on zero evidence. Route
multi-filter runs through ` -- ` (or a single filter) so libtest OR-matches inside
ONE test binary.

## Native-library builds must be clean before trusting a verdict

z3-sys (and any native dep) accumulates divergent build artifacts under
`/var/tmp/cargo-target` during disk-pressure retries; a corrupt `libz3` makes Z3
reject a well-formed SMT script (`holds 0 assertion(s) but the script declares 1`),
which surfaces as a flaky `ParseError` in the full `cargo test` suite but NOT in
isolation. Before mutation: `cargo clean -p z3-sys && cargo build` to get ONE clean
z3, then run baseline+post-restore on that build. The malformed-script guard in
`solver.rs::verify` is the correct behavior (report ParseError, never false-prove),
but it only yields trustworthy verdicts on a single clean native build.

## MUTATIONS THAT GENUINELY SURVIVED, and the tests that killed them

- Allowing `ExprKind::Assign` as a tail value: a semicolon-less assignment is a
  tail, so `fn f(x: f32) -> f32 { x = 1.0 }` compiled to `ret double 1.0` — a
  value the source never returned. This is why the value check is an allow-list.
  Needed a dedicated test.
- Bypassing the `pending_return_stmt.is_none()` guard: produced byte-identical
  IR, because the backend's match already resolves `Some(return_stmt)` first.
  The guard is defence in depth, not load-bearing. Needed a test pinning the
  observable value so the redundancy cannot be read as permission to drop it.

## Verified per-construct mutation result — the `round` bounding axiom

`encode_round_axiom` in `crates/naso-verify/src/prover/obligations.rs` emits
`(declare-fun round (Real) Real)` + `(assert (forall ((x Real)) (<= (- x 0.5) (round x)) (<= (round x) (+ x 0.5))))`
when `encoded_uses_round_is_scoped_to_round` detects a `round` use; the inclusive
`<=` is sound because real rounding attains the boundary (`round(0.5)=1.0=0.5+0.5`).

Six mutants, run on a CLEAN z3 build (`cargo clean -p z3-sys` first), each restored
to exact original (empty `git diff`) before the next. The `round` axiom is now emitted two
ways: a GROUND instance `(t-0.5) <= round(t) <= (t+0.5)` for every free argument `t` (built
from `round_bound_for`, the SAME term the universal `forall` is), plus the universal
`(assert (forall ((x Real)) ...))` for quantified arguments. Grounding makes the scalar
obligations discharge by ground UNSAT (no Z3 e-matching) and means a mutation to
`round_bound_for` propagates to the ground path too -- a blanked edge kills the scalar
obligation it bounds, not just the tensor one.

| Mutant | What changed | Verdict | Evidence |
|---|---|---|---|
| A | lower edge blanked (`(- t 0.5)` -> fresh var) | **KILLED** | ground instance propagates break; `round_error_bounds` + count pin fail |
| B | upper edge blanked (`(+ t 0.5)` -> fresh var) | **KILLED** | same (upper edge) |
| C | walker miss (`"round"` -> `"roundx"`) | **KILLED** | `encoded_uses_round_is_scoped_to_round` fails; `needs_round` false -> no round -> obligations fail |
| D | lower bound strict (`<=` -> `<`) | **KILLED** | `round_axiom_is_well_formed` asserts the axiom renders `<= ` exactly twice; under `<` lower edge -> `(< `, count=1 -> panic |
| D2 | upper bound strict (`<=` -> `<`) | **KILLED** | same structural check, upper edge |
| E | drop round handling in `prove_obligation` | **KILLED** | round undeclared -> obligations fail; count pin + discharge tests fail |
| G1 | drop the universal gate (always assert `forall`) | **KILLED** | scalar `round(4.5) == 4.5` / `round(0.5) == 1.0` hit the real quantifier -> 30s timeout -> Undecided -> `round_equality_refutations_are_decided_not_undecided` fails |
| G2 | invert the universal gate (`!has_bound_var_round`) | **KILLED** | tensor kernel `forall i. round(input[i]/scale)` loses the universal -> refutes -> `the_shipped_int8_quantiser_kernel_discharges_completely` fails |

| G3 | drop the ground round instance (`round_bound_for`) | **KILLED** | scalar bound `round(v) <= v + 0.5` loses its ground proof -> refutes -> `the_shipped_int8_quantiser_kernel_discharges_completely` fails |
| G4 | drop the integer-value axiom (`round_integer_axiom`) | **KILLED** | Real-typed `forall` round bounds (`forall t in 0.0..1.0 { round(t) <= t + 0.5 }`) and the int8 kernel lose round's Integer pinning -> fail |
| G5 | drop `type_of_range_bound` inference in `infer_forall`/`infer_quantified` (inference.rs), force `TypeKind::Int` | **KILLED** | `round(t)` becomes a type error (round: Float->Float, t: Int) -> `typecheck_forall_float_range_binds_float_var` fails |

**11/11 killed, 0 survivors, 0 invalid.** (G1-G5 — the integer-value axiom is load-bearing for Real-typed `forall` round *bounds* (G4) and the typecheck-side Real inference is load-bearing for `round(t)` to typecheck (G5).)

Determinism: `solver::verify` holds a process-global `Mutex<()>` around every solve (z3
0.19 / z3-sys 0.10 is NOT built `Z3_THREAD_SAFE`; `Context::thread_local()` is reused across
solves, so concurrent solves otherwise risk silent parser corruption). Combined with a clean
z3-sys build and the fixed `random_seed`, the round + kernel suite is verified 20/20 at
default, 16, 8 and 1 test threads. A corrupt `z3-sys` artifact (divergent build dirs left
by disk-pressure `signal 7`/`signal 9`) makes z3 reject well-formed scripts as a ParseError,
which the guard in `solver.rs::verify` reports honestly -- never false-proved -- so
`cargo clean -p z3-sys` after any such event restores green.

D/D2 is the case that almost fabricated a false-green: semantic discharge tests CANNOT
distinguish `<=` from `<` (strict `<` *implies* the inclusive `<=` claim, so a `<=`
obligation discharges under both), and a tie value (`round(0.5)=1.0`) cannot be asserted
as a theorem without choosing a tie convention the prover deliberately refuses to make.
The ONLY soundness pin available is the structural renderer check in `round_axiom_is_well_formed`
(assert the axiom string uses inclusive `(<= `). That guard is loaded by a `round` use,
so it also kills C. This is legitimate, not slop: the invariant (`<=` must be inclusive)
is a real soundness property, and the structural check is the honest witness for it.

Accepted limitation (NOT a survivor gap): the quantified round-equality frontier
(`forall t. round(t) = t`) is now REACHABLE via a float range (`forall t in 0.0..1.0`)
-- as of this tranche the typechecker infers `t: Float` from float-literal bounds
(check.rs `type_of_range_bound`) and the encoder infers `Sort::Real` (obligations.rs
`sort_of_range_bound`). BUT it STILL times out (Undecided): the integer-value axiom
`round(x) = to_int(round(x))` (retained, load-bearing as G4 for bound discharge) does
not let Z3 ground a Real-interval witness. Refuting the equality needs a dedicated
equality-axiom encoder (a `round` definition as piecewise floor/ceil + SMT triggers),
the genuine tranche-9 equality-axiom encoder frontier. Bounds (`round(v) <= v + 0.5`) and free-argument
equality (`round(v) == 4.5`, `round(v) == v`) are decidable; `as f32` casts + Real-typed
`forall` round bounds DISCHARGE.
