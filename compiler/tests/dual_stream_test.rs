//! Tests for the forward/inverse stream representation.
//!
//! # What is and is not established here
//!
//! Established: the two directions are distinct types, an inverse stream cannot be inverted,
//! `try_invert` refuses a stream containing an operation with no inverse, and the undo of a
//! forward stream runs its operations in REVERSE order.
//!
//! NOT established: that any backend emits these streams, or that uncomputation works. No
//! backend consumes them. `reversible` remains refused, and that refusal is correct -- see
//! `ir::dual_stream`.
//!
//! The compile-fail guard is tested by actually compiling a snippet that must NOT compile,
//! rather than by asserting a comment is still present.

use naso_compiler::ir::dual_stream::{
    DualStream, ForwardStream, InverseStream, InversionError, Op, OpClass, ProgramStreams,
};

/// A forward stream of three gates, all invertible.
fn gates() -> ForwardStream {
    ForwardStream::new(vec![
        Op::Gate {
            name: "h".into(),
            qubits: vec!["q0".into()],
        },
        Op::Gate {
            name: "cnot".into(),
            qubits: vec!["q0".into(), "q1".into()],
        },
        Op::Gate {
            name: "x".into(),
            qubits: vec!["q1".into()],
        },
    ])
}

// ---------------------------------------------------------------------------
// The type-level guard
// ---------------------------------------------------------------------------

/// An inverse stream is the forward stream's operations in REVERSE order.
///
/// This is the property that makes an ordered inverse a separate list rather than a
/// recomputed view: if `b` depends on what `a` produced, `undo(b)` must run first.
#[test]
fn inverting_reverses_the_order() {
    let inverse = DualStream::forward_only(gates())
        .try_invert()
        .expect("all gates invert");

    let names: Vec<&str> = inverse.ops().iter().map(Op::name).collect();
    assert_eq!(
        names,
        vec!["x", "cnot", "h"],
        "the undo must run the last forward operation first, not the first one. Getting \\
         this backwards is a program that does not compute its inverse."
    );
}

#[test]
fn the_two_directions_report_different_classes() {
    let forward = gates();
    let inverse = forward_dual(&forward);

    assert_eq!(forward.class(), OpClass::Forward);
    assert_eq!(inverse.class(), OpClass::Inverse);
    // Formatting must not blur them either: a log line saying "forward[3]" for an inverse
    // stream is how a bug gets misread as correct.
    assert_eq!(forward.to_string(), "forward[3]");
    assert_eq!(inverse.to_string(), "inverse[3]");
}

/// Build a dual stream with an established inverse.
fn forward_dual(forward: &ForwardStream) -> InverseStream {
    DualStream::forward_only(forward.clone())
        .try_invert()
        .expect("invertible")
}

/// The inverse stream must have NO `invert` method.
///
/// This is the whole justification for the module existing, so it is checked mechanically
/// rather than asserted in a comment. The real check is the ```compile_fail doctest on
/// `InverseStream`; this test asserts that doctest is wired up and that both directions
/// remain distinct types.
///
/// `compile_fail` was chosen over spawning cargo from inside a test: a spawned compile is
/// slow, brittle about target directories, and reports failure as an assertion message
/// rather than as the build error where it belongs. Its weakness is that it passes for ANY
/// compile error, so a typo in the probe would pass silently -- which is why the doctest
/// names the missing method, making a typo visible in the failure output.
///
/// If `impl InverseStream { fn invert }` is ever added, or a `Deref` to something with one,
/// the doctest starts compiling, cargo reports it, and the guard is gone.
#[test]
fn the_no_double_uncompute_guard_is_wired_up() {
    let source = include_str!("../src/ir/dual_stream.rs");

    assert!(
        source.contains("```compile_fail"),
        "InverseStream must carry a ```compile_fail doctest proving `invert` does not exist \
         on it. Without it, double uncomputation becomes a silent compile-time regression."
    );
    assert!(
        source.contains("inv.invert()"),
        "the compile_fail doctest must actually CALL invert(), or it proves nothing"
    );

    // Both directions must remain separate structs. Merging them behind a class flag is the
    // exact regression guarded against: it would give the inverse an `invert` too.
    for ty in ["ForwardStream", "InverseStream"] {
        assert!(
            source.contains(&format!("pub struct {ty}(")),
            "{ty} must remain its own tuple struct. One type plus an OpClass flag is what \
             this module replaces."
        );
    }

    // The inverse must not be reachable by conversion from a forward stream.
    assert!(
        !source.contains("impl From<ForwardStream> for InverseStream"),
        "a From impl would hand a caller an InverseStream from a ForwardStream, which is a \
         route to building an inverse that was never established."
    );
    assert!(
        !source.contains("impl From<InverseStream> for ForwardStream"),
        "a From impl in this direction would let an inverse stream be read as a forward one, \
         which is exactly the confusion the two types exist to prevent."
    );
}

// ---------------------------------------------------------------------------
// Refusing what cannot be uncomputed
// ---------------------------------------------------------------------------

/// A measurement has no inverse, and the diagnostic must say which one and where.
///
/// Not an error to fix silently: measuring genuinely destroys amplitude, so a program that
/// measures and then uncomputes is wrong in the physics.
#[test]
fn a_measurement_blocks_uncomputation_and_is_located() {
    let forward = ForwardStream::new(vec![
        Op::Gate {
            name: "h".into(),
            qubits: vec!["q0".into()],
        },
        Op::Measure {
            qubits: vec!["q0".into()],
        },
        Op::Gate {
            name: "x".into(),
            qubits: vec!["q1".into()],
        },
    ]);

    let err = DualStream::forward_only(forward.clone())
        .try_invert()
        .expect_err("a measurement cannot be uncomputed");

    match &err {
        InversionError::NotInvertible { index, operation } => {
            assert_eq!(*index, 1, "the diagnostic must name the offending position");
            assert_eq!(operation, "measure");
        }
        other => panic!("expected NotInvertible, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("measure") && msg.contains("position 1"),
        "the message must name the operation and where it is, or the user cannot act on \
         it: {msg}"
    );
    assert!(
        msg.contains("destroys amplitude"),
        "the message must explain WHY, so a user does not read this as a compiler bug: {msg}"
    );

    assert!(
        !DualStream::forward_only(forward).could_invert(),
        "a program with a measurement is not uncomputable"
    );
}

#[test]
fn an_allocation_blocks_uncomputation() {
    let forward = ForwardStream::new(vec![Op::Alloc {
        qubits: vec!["q0".into()],
    }]);
    let err = DualStream::forward_only(forward)
        .try_invert()
        .expect_err("alloc is not undoable");
    assert!(matches!(err, InversionError::NotInvertible { .. }));
    assert!(err.to_string().contains("alloc"));
}

/// An unmodelled operation must block uncomputation rather than be skipped.
///
/// This is the important one. If `Unknown` were treated as invertible, an operation the
/// compiler does not model would get an inverse stream that silently omits it -- which is
/// the same failure as the original `PirExpr::Reversible` bug, one level further out.
#[test]
fn an_unknown_operation_is_not_silently_invertible() {
    let forward = ForwardStream::new(vec![Op::Unknown {
        name: "mystery_gate".into(),
    }]);
    assert!(
        !forward.ops()[0].is_invertible(),
        "an unmodelled operation's inverse is unmodelled, so it must not claim to be \
         invertible"
    );
    let err = DualStream::forward_only(forward)
        .try_invert()
        .expect_err("unknown blocks");
    assert!(err.to_string().contains("mystery_gate"));
}

/// Classical operations and barriers are invertible; gates are.
#[test]
fn classical_ops_gates_and_barriers_are_invertible() {
    for op in [
        Op::Classical("add".into()),
        Op::Barrier,
        Op::Gate {
            name: "h".into(),
            qubits: vec!["q0".into()],
        },
    ] {
        assert!(op.is_invertible(), "{} should be invertible", op.name());
    }
}

/// An empty forward stream inverts to an empty inverse stream.
///
/// Not a special case worth much on its own -- included so the reversal of a
/// zero-length list is pinned, since `rev()` on empty is easy to get wrong under an
/// off-by-one edit.
#[test]
fn an_empty_stream_inverts_to_an_empty_stream() {
    let inverse = DualStream::forward_only(ForwardStream::new(vec![]))
        .try_invert()
        .expect("empty inverts");
    assert!(inverse.is_empty());
}

// ---------------------------------------------------------------------------
// Absence of an inverse must be distinguishable from an empty one
// ---------------------------------------------------------------------------

/// `inverse: None` must not read as "nothing to uncompute".
///
/// This is the distinction the `Option` exists for. A backend that treated `None` as an
/// empty stream would emit a forward-only program while the type said the inverse was
/// merely absent -- the original silent-drop bug in a new shape.
#[test]
fn an_absent_inverse_is_not_an_empty_one() {
    let forward_only = DualStream::forward_only(gates());
    assert!(!forward_only.has_inverse(), "no inverse was established");
    assert!(
        forward_only.inverse.is_none(),
        "the inverse must be absent, not empty"
    );

    let with_empty = DualStream::new(gates(), InverseStream::uncompute(vec![]));
    assert!(
        with_empty.has_inverse(),
        "an established but empty inverse is a different state from no inverse"
    );
    assert!(
        with_empty.inverse.as_ref().expect("present").is_empty(),
        "this inverse is established and empty, which is not the same as absent"
    );
}

// ---------------------------------------------------------------------------
// Program-level bookkeeping
// ---------------------------------------------------------------------------

#[test]
fn a_program_reports_which_functions_are_forward_only() {
    let mut program = ProgramStreams::new();
    program.insert(
        "established",
        DualStream::forward_only(gates())
            .try_invert()
            .map(|i| DualStream::new(gates(), i))
            .expect("invertible"),
    );
    program.insert("missing", DualStream::forward_only(gates()));

    let forward_only = program.forward_only_functions();
    assert_eq!(
        forward_only,
        vec!["missing"],
        "only the function without an established inverse may be listed"
    );
    assert!(
        !program.is_fully_reversible(),
        "one function has no inverse"
    );

    assert_eq!(program.function_names(), vec!["established", "missing"]);
}

#[test]
fn a_program_reports_functions_with_uninvertible_operations() {
    let mut program = ProgramStreams::new();
    program.insert(
        "measures",
        DualStream::forward_only(ForwardStream::new(vec![Op::Measure {
            qubits: vec!["q0".into()],
        }])),
    );
    program.insert("gates", DualStream::forward_only(gates()));

    let bad = program.functions_with_uninvertible_ops();
    assert_eq!(bad.len(), 1, "only the measuring function is uncomputable");
    assert_eq!(bad[0].0, "measures");
}

#[test]
fn an_empty_program_is_fully_reversible_vacuously() {
    // Honest about the vacuous case rather than erroring: there is nothing to refuse.
    let program = ProgramStreams::new();
    assert!(program.is_empty());
    assert!(program.is_fully_reversible());
    assert!(program.forward_only_functions().is_empty());
}

#[test]
fn a_program_of_only_gates_is_fully_reversible() {
    let mut program = ProgramStreams::new();
    let inverse = DualStream::forward_only(gates())
        .try_invert()
        .expect("invertible");
    program.insert("main", DualStream::new(gates(), inverse));
    assert!(program.is_fully_reversible());
    assert!(program.forward_only_functions().is_empty());
}

// ---------------------------------------------------------------------------
// The backend entry point
// ---------------------------------------------------------------------------

/// `established_inverse` must refuse when no inverse exists, even if one COULD be built.
///
/// This is the conflation that made `can_uncompute` dangerous, and it is the method a
/// backend would actually call. A `forward_only` stream of invertible gates must not hand
/// back an inverse here, because nothing established one -- the caller has not proved that
/// this computation is the thing being undone.
#[test]
fn the_backend_entry_point_refuses_an_unestablished_inverse() {
    let forward = gates();
    let stream = DualStream::forward_only(forward.clone());

    // It COULD be inverted -- that is not the question.
    assert!(
        stream.could_invert(),
        "these gates are all invertible, so an inverse could be built"
    );

    // But none has been established, so the backend entry point refuses.
    let err = stream
        .established_inverse()
        .expect_err("no inverse has been established");
    assert_eq!(
        err,
        InversionError::NotEstablished,
        "the refusal must be NotEstablished, not NotInvertible. The latter would tell the \\
         user their program is physically uncomputable, which is false here."
    );
    assert!(
        err.to_string()
            .contains("no inverse stream has been established"),
        "the message must distinguish 'not established' from 'cannot be inverted': {err}"
    );

    // Establishing it makes the same stream succeed.
    let established = DualStream::new(forward, stream.try_invert().expect("invertible"));
    assert!(
        established.established_inverse().is_ok(),
        "once an inverse exists the same forward stream must be emittable"
    );
}

/// A stream storing an inverse that could not have been built must be caught, not emitted.
///
/// This is an internal inconsistency -- somebody constructed an `InverseStream` containing a
/// measurement. `InverseStream::uncompute` is public so lowering can build one, which means
/// this state is reachable, and a backend reading it would emit an "uncomputation" that
/// destroys amplitude.
#[test]
fn a_stored_inverse_that_cannot_be_built_is_rejected() {
    let inconsistent = DualStream::new(
        gates(),
        InverseStream::uncompute(vec![Op::Measure {
            qubits: vec!["q0".into()],
        }]),
    );

    let err = inconsistent
        .established_inverse()
        .expect_err("an inverse containing a measurement must not be handed to a backend");
    match &err {
        InversionError::NotInvertible { index, operation } => {
            assert_eq!(*index, 0);
            assert_eq!(operation, "measure");
        }
        other => panic!("expected NotInvertible, got {other:?}"),
    }
}

/// A measurement in the forward stream must block the backend entry point too.
#[test]
fn a_measurement_blocks_the_backend_entry_point() {
    let stream = DualStream::new(
        ForwardStream::new(vec![Op::Measure {
            qubits: vec!["q0".into()],
        }]),
        // An empty but PRESENT inverse, so only the forward content can be the objection.
        InverseStream::uncompute(vec![]),
    );
    assert!(stream.has_inverse(), "an inverse is present, if empty");
    let err = stream.established_inverse().expect_err("must refuse");
    assert!(
        matches!(err, InversionError::NotInvertible { .. }),
        "expected the measurement to be the objection, got {err:?}"
    );
}
