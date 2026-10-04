//! QIR Primitive Operations
//!
//! Defines QIR primitive types and intrinsic functions per Microsoft QIR spec.

use crate::codegen::qir::module_builder::QIRModuleBuilder;
use inkwell::AddressSpace;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, IntType};

/// QIR Primitive Types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QirPrimitive {
    /// Opaque qubit type
    Qubit,
    /// Measurement result (i1)
    Result,
    /// Pauli operators
    PauliI,
    PauliX,
    PauliY,
    PauliZ,
    /// Rotation angles
    Double,
}

impl QirPrimitive {
    /// Get the LLVM type for this primitive
    pub fn llvm_type<'ctx>(&self, builder: &QIRModuleBuilder<'ctx>) -> BasicTypeEnum<'ctx> {
        match self {
            QirPrimitive::Qubit => builder.qubit_type().into(),
            QirPrimitive::Result => builder.result_type().into(),
            QirPrimitive::PauliI
            | QirPrimitive::PauliX
            | QirPrimitive::PauliY
            | QirPrimitive::PauliZ => builder.llvm_context().i8_type().into(),
            QirPrimitive::Double => builder.llvm_context().f64_type().into(),
        }
    }
}

/// QIR Intrinsic Function Definition
#[derive(Debug, Clone)]
pub struct QirIntrinsic {
    pub name: &'static str,
    pub ret_type: QirIntrinsicRetType,
    pub param_types: &'static [QirIntrinsicParamType],
    pub is_var_args: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum QirIntrinsicRetType {
    Void,
    Qubit,
    Result,
    Double,
    Int(i32),
    Ptr,
    QubitArray,
}

#[derive(Debug, Clone, Copy)]
pub enum QirIntrinsicParamType {
    Qubit,
    Result,
    Double,
    Int(i32),
    Ptr,
    QubitArray,
    ResultArray,
}

impl QirIntrinsic {
    /// Get the LLVM function type for this intrinsic.
    ///
    /// `custom_width_int_type` returns a `Result` in inkwell 0.10 (LLVM rejects
    /// widths outside 1..=8388608). The QIR intrinsic table is static data, so
    /// an out-of-range width is a hard programming error and is reported as
    /// such rather than being papered over with a default width.
    fn int_type_of_width<'ctx>(builder: &QIRModuleBuilder<'ctx>, width: i32) -> IntType<'ctx> {
        assert!(
            width > 0,
            "QIR intrinsic integer width must be positive, got {width}"
        );
        let bits = u32::try_from(width).expect("QIR intrinsic integer width must fit in u32");
        let nz = std::num::NonZeroU32::new(bits).expect("width must be non-zero");
        builder
            .llvm_context()
            .custom_width_int_type(nz)
            .unwrap_or_else(|err| panic!("invalid QIR intrinsic integer width {width}: {err}"))
    }

    /// Get the LLVM function type for this intrinsic
    pub fn function_type<'ctx>(&self, builder: &QIRModuleBuilder<'ctx>) -> FunctionType<'ctx> {
        // A void return is `None` in inkwell 0.10: `BasicTypeEnum` has no Void
        // variant and there is no `From<VoidType> for BasicTypeEnum`.
        let ret: Option<BasicTypeEnum<'ctx>> = match self.ret_type {
            QirIntrinsicRetType::Void => None,
            QirIntrinsicRetType::Qubit => Some(builder.qubit_type().into()),
            QirIntrinsicRetType::Result => Some(builder.result_type().into()),
            QirIntrinsicRetType::Double => Some(builder.llvm_context().f64_type().into()),
            QirIntrinsicRetType::Int(w) => Some(Self::int_type_of_width(builder, w).into()),
            QirIntrinsicRetType::Ptr => Some(
                builder
                    .llvm_context()
                    .ptr_type(AddressSpace::from(0))
                    .into(),
            ),
            QirIntrinsicRetType::QubitArray => Some(
                builder
                    .llvm_context()
                    .ptr_type(AddressSpace::from(0))
                    .into(),
            ),
        };

        let params: Vec<BasicTypeEnum<'ctx>> = self
            .param_types
            .iter()
            .map(|p| match p {
                QirIntrinsicParamType::Qubit => builder.qubit_type().into(),
                QirIntrinsicParamType::Result => builder.result_type().into(),
                QirIntrinsicParamType::Double => builder.llvm_context().f64_type().into(),
                QirIntrinsicParamType::Int(w) => Self::int_type_of_width(builder, *w).into(),
                QirIntrinsicParamType::Ptr => builder
                    .llvm_context()
                    .ptr_type(AddressSpace::from(0))
                    .into(),
                QirIntrinsicParamType::QubitArray => builder
                    .llvm_context()
                    .ptr_type(AddressSpace::from(0))
                    .into(),
                QirIntrinsicParamType::ResultArray => builder
                    .llvm_context()
                    .ptr_type(AddressSpace::from(0))
                    .into(),
            })
            .collect();

        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|p| (*p).into()).collect();
        match ret {
            Some(r) => r.fn_type(&param_types, self.is_var_args),
            None => builder
                .llvm_context()
                .void_type()
                .fn_type(&param_types, self.is_var_args),
        }
    }
}

/// All QIR Intrinsic Definitions (Base Profile)
pub const QIR_INTRINSICS: &[QirIntrinsic] = &[
    // Qubit allocation/release
    QirIntrinsic {
        name: "qir.qubit_alloc",
        ret_type: QirIntrinsicRetType::Qubit,
        param_types: &[],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.qubit_release",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.qubit_alloc_array",
        ret_type: QirIntrinsicRetType::QubitArray,
        param_types: &[QirIntrinsicParamType::Int(64)],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.qubit_release_array",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::QubitArray],
        is_var_args: false,
    },
    // Single-qubit gates
    QirIntrinsic {
        name: "qir.h",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.x",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    // The one PARAMETERIZED single-qubit gate in the QIR base profile: a rotation about Z by
    // a supplied angle.
    //
    // The `Double` parameter is the whole reason rotations were refused before. Every other
    // entry point here takes a qubit and nothing else, so a rotation had nowhere to put its
    // angle -- and emitting `qir.rz` with no argument would have declared a zero-argument
    // function, computing a rotation by zero, which is the identity and silently not a
    // rotation.
    QirIntrinsic {
        name: "qir.r1",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Double, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.y",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.z",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.s",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.t",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.rx",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Double, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.ry",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Double, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.rz",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Double, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.r1",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Double, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.rt1",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Double, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    // Two-qubit gates
    QirIntrinsic {
        name: "qir.cx",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.cy",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.cz",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.ccx",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[
            QirIntrinsicParamType::Qubit,
            QirIntrinsicParamType::Qubit,
            QirIntrinsicParamType::Qubit,
        ],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.swap",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.iswap",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Qubit, QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    // Measurement
    QirIntrinsic {
        name: "qir.mz",
        ret_type: QirIntrinsicRetType::Result,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.mx",
        ret_type: QirIntrinsicRetType::Result,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.my",
        ret_type: QirIntrinsicRetType::Result,
        param_types: &[QirIntrinsicParamType::Qubit],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.measure",
        ret_type: QirIntrinsicRetType::Result,
        param_types: &[QirIntrinsicParamType::Qubit, QirIntrinsicParamType::Int(8)],
        is_var_args: false,
    },
    // Result handling
    QirIntrinsic {
        name: "qir.result_record",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Result, QirIntrinsicParamType::Result],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.result_update",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Result, QirIntrinsicParamType::Result],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.result_get",
        ret_type: QirIntrinsicRetType::Result,
        param_types: &[QirIntrinsicParamType::Result],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.result_equal",
        ret_type: QirIntrinsicRetType::Result,
        param_types: &[QirIntrinsicParamType::Result, QirIntrinsicParamType::Result],
        is_var_args: false,
    },
    // Array operations
    QirIntrinsic {
        name: "qir.array_record",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[
            QirIntrinsicParamType::ResultArray,
            QirIntrinsicParamType::Result,
        ],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.array_update",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[
            QirIntrinsicParamType::ResultArray,
            QirIntrinsicParamType::Result,
            QirIntrinsicParamType::Int(64),
        ],
        is_var_args: false,
    },
    // Control flow (for adaptive profile)
    QirIntrinsic {
        name: "qir.if",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[
            QirIntrinsicParamType::Result,
            QirIntrinsicParamType::Ptr,
            QirIntrinsicParamType::Ptr,
        ],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.while",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Ptr, QirIntrinsicParamType::Ptr],
        is_var_args: false,
    },
    // Adjoint/Controlled (for reusable operations)
    QirIntrinsic {
        name: "qir.adjoint",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Ptr],
        is_var_args: false,
    },
    QirIntrinsic {
        name: "qir.controlled",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[
            QirIntrinsicParamType::QubitArray,
            QirIntrinsicParamType::Ptr,
        ],
        is_var_args: false,
    },
    // Profiling/debugging
    QirIntrinsic {
        name: "qir.profiler_record",
        ret_type: QirIntrinsicRetType::Void,
        param_types: &[QirIntrinsicParamType::Ptr, QirIntrinsicParamType::Int(64)],
        is_var_args: false,
    },
];

/// Get intrinsic by name
pub fn get_intrinsic(name: &str) -> Option<&'static QirIntrinsic> {
    QIR_INTRINSICS.iter().find(|i| i.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intrinsic_lookup() {
        assert!(get_intrinsic("qir.h").is_some());
        assert!(get_intrinsic("qir.cx").is_some());
        assert!(get_intrinsic("qir.mz").is_some());
        assert!(get_intrinsic("qir.qubit_alloc").is_some());
        assert!(get_intrinsic("nonexistent").is_none());
    }

    #[test]
    fn test_intrinsic_signatures() {
        let h = get_intrinsic("qir.h").unwrap();
        assert_eq!(h.param_types.len(), 1);
        assert!(matches!(h.param_types[0], QirIntrinsicParamType::Qubit));
        assert!(matches!(h.ret_type, QirIntrinsicRetType::Void));

        let cx = get_intrinsic("qir.cx").unwrap();
        assert_eq!(cx.param_types.len(), 2);
        assert!(matches!(cx.param_types[0], QirIntrinsicParamType::Qubit));
        assert!(matches!(cx.param_types[1], QirIntrinsicParamType::Qubit));

        let mz = get_intrinsic("qir.mz").unwrap();
        assert_eq!(mz.param_types.len(), 1);
        assert!(matches!(mz.ret_type, QirIntrinsicRetType::Result));
    }

    #[test]
    fn test_all_required_intrinsics() {
        let required = [
            "qir.mz",
            "qir.mx",
            "qir.my",
            "qir.h",
            "qir.x",
            "qir.cx",
            "qir.ccx",
            "qir.t",
            "qir.s",
            "qir.rz",
            "qir.r1",
            "qir.rt1",
            "qir.qubit_alloc",
            "qir.qubit_release",
            "qir.result_record",
            "qir.result_update",
        ];

        for name in required {
            assert!(get_intrinsic(name).is_some(), "Missing intrinsic: {}", name);
        }
    }
}
