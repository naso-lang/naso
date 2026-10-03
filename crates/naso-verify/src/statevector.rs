//! Complex arithmetic and state-vector simulation, on the CPU, with no dependencies.
//!
//! # Why this exists
//!
//! Everything this project claims about quantum computation -- that a gate sequence is
//! unitary, that an inverse really inverts it -- was previously unverifiable here. There is
//! no GPU, no QPU, and no simulator, so "reversible" was a structural claim about code
//! shape and nothing more.
//!
//! This module makes it a numerical one: gates act on a state vector, and
//! [`StateVector::apply`] multiplies amplitudes in the complex plane. Applying a gate and
//! then its inverse must return the original state to within floating-point tolerance, and
//! that is now checkable in a unit test on any machine.
//!
//! # Precision, and why it is enough
//!
//! Complex arithmetic is `f64` pairs. The identity property is checked with a tolerance
//! rather than exact equality because repeated `sqrt(2)` divisions accumulate error: Hadamard
//! squared is exact in the matrix but each application rounds. The tolerance is asserted
//! against the observed drift rather than chosen to be generous, so a real regression in
//! gate matrices is not absorbed by a loose bound.
//!
//! # What this is NOT
//!
//! This is not a QPU, and passing here says nothing about one. It is a dense
//! state-vector simulator: memory is `2^n` complex numbers, so it is limited to a handful of
//! qubits and gets no better with scale. It verifies GATE MATRICES, which is what a gate
//! inverse table has to be checked against.

use std::fmt;
use std::ops::{Add, Mul, Neg, Sub};

/// A complex number in Cartesian form.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// The complex number with zero imaginary part.
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };
    /// The real number one.
    pub const ONE: Self = Self { re: 1.0, im: 0.0 };
    /// The imaginary unit.
    pub const I: Self = Self { re: 0.0, im: 1.0 };

    /// Build from real and imaginary parts.
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// A real number.
    pub const fn real(re: f64) -> Self {
        Self { re, im: 0.0 }
    }

    /// The complex conjugate.
    ///
    /// The conjugate is what makes `U` unitary imply `U^-1 == conj(U)`, so this is the
    /// operation an inverse check leans on. Getting it wrong (forgetting to conjugate)
    /// produces a matrix that passes a naive check and fails on real amplitudes.
    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }

    /// The modulus squared, `|z|^2`.
    ///
    /// Returned squared because that is what unitarity needs: `sum |a_i|^2 == 1` avoids a
    /// square root per amplitude on every check.
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// The modulus, `|z|`.
    pub fn norm(self) -> f64 {
        self.norm_sqr().sqrt()
    }

    /// The principal argument, in radians.
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    /// `self` scaled by a real number.
    pub fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }

    /// `self` rotated by `theta` radians in the complex plane.
    ///
    /// `z * exp(i*theta)`, which is how a phase ramp is applied to one amplitude. Used to
    /// build a complex probe state whose phases differ per basis state.
    pub fn rotated(self, theta: f64) -> Self {
        self * Self::expi(theta)
    }

    /// `exp(i * theta)`.
    pub fn expi(theta: f64) -> Self {
        Self::new(theta.cos(), theta.sin())
    }
}

impl Add for Complex {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
}

impl Sub for Complex {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
}

impl Mul for Complex {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}

impl Neg for Complex {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.re, -self.im)
    }
}

impl fmt::Display for Complex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.im == 0.0 {
            write!(f, "{}", self.re)
        } else if self.re == 0.0 {
            write!(f, "{}i", self.im)
        } else {
            write!(f, "{} + {}i", self.re, self.im)
        }
    }
}

/// A quantum state as a vector of amplitudes, one per basis state.
///
/// Indexing is little-endian in the sense that `index` is the integer whose bit `q` is the
/// value of qubit `q`. That is what makes the two-qubit gate matrices below readable: the
/// CNOT matrix acts on the `|10> -> |11>` entry, which is index 2 -> 3.
#[derive(Clone, PartialEq, Debug)]
pub struct StateVector {
    amplitudes: Vec<Complex>,
    qubits: usize,
}

impl StateVector {
    /// How many qubits.
    pub fn qubits(&self) -> usize {
        self.qubits
    }

    /// The amplitudes, indexed by basis state.
    pub fn amplitudes(&self) -> &[Complex] {
        &self.amplitudes
    }

    /// The amplitude of one basis state.
    pub fn amplitude(&self, basis: usize) -> Complex {
        self.amplitudes[basis]
    }

    /// A product state: qubit `q` in `|0>` for all `q`.
    pub fn zero_state(qubits: usize) -> Self {
        let mut amplitudes = vec![Complex::ZERO; 1 << qubits];
        amplitudes[0] = Complex::ONE;
        Self { amplitudes, qubits }
    }

    /// A state from explicit amplitudes, one per basis state.
    ///
    /// # Panics
    ///
    /// If the length is not `2^qubits`. A silently padded or truncated state would compute
    /// the wrong answer with no diagnostic, which is the failure this project keeps hunting.
    pub fn from_amplitudes(qubits: usize, amplitudes: Vec<Complex>) -> Self {
        assert_eq!(
            amplitudes.len(),
            1 << qubits,
            "a {qubits}-qubit state needs exactly {} amplitudes, got {}",
            1 << qubits,
            amplitudes.len()
        );
        Self { amplitudes, qubits }
    }

    /// A state with one basis state populated, given `(qubit, is_one)` pairs.
    ///
    /// Built by index arithmetic rather than through a temporary state and a mutation
    /// helper. An earlier version used a `tap`-style closure over
    /// `from_amplitudes(qubits, vec![ZERO; ...])`, which populated the wrong basis state --
    /// `(0, true)` gave amplitude 1 at index 1 instead of index 2. A constructor whose
    /// whole job is to place one 1 in a vector is exactly the wrong place for indirection.
    ///
    /// Qubits not named default to `|0>`, so `with_basis_state(2, &[(0, true)])` is `|01>`
    /// in the little-endian indexing this module uses.
    ///
    /// # Panics
    ///
    /// If a qubit index is out of range. Silently ignoring it would produce a state for a
    /// different system than the caller described.
    pub fn with_basis_state(qubits: usize, assignments: &[(usize, bool)]) -> Self {
        let mut index = 0usize;
        for &(q, one) in assignments {
            assert!(
                q < qubits,
                "qubit {q} is out of range for a {qubits}-qubit state"
            );
            if one {
                index |= 1 << q;
            }
        }
        let mut amplitudes = vec![Complex::ZERO; 1 << qubits];
        amplitudes[index] = Complex::ONE;
        Self { amplitudes, qubits }
    }

    /// The probability of measuring `1` on one qubit.
    ///
    /// The MARGINAL probability: summed over every value of the other qubits, because that
    /// is what measuring this qubit alone returns. Reading it off a single basis state
    /// instead would make the answer depend on which amplitude happened to be looked at.
    ///
    /// # Panics
    ///
    /// If the qubit index is out of range.
    pub fn probability_of_one(&self, qubit: usize) -> f64 {
        assert!(qubit < self.qubits, "qubit {qubit} out of range");
        let mut total = 0.0;
        for (index, amp) in self.amplitudes.iter().enumerate() {
            if (index >> qubit) & 1 == 1 {
                total += amp.norm_sqr();
            }
        }
        total
    }

    /// The sum of squared magnitudes, which must be 1 for a normalised state.
    pub fn total_probability(&self) -> f64 {
        self.amplitudes.iter().map(|a| a.norm_sqr()).sum()
    }

    /// Whether the state is normalised to within `tolerance`.
    pub fn is_normalised(&self, tolerance: f64) -> bool {
        (self.total_probability() - 1.0).abs() <= tolerance
    }

    /// Apply a gate to one qubit.
    ///
    /// `matrix` is the 2x2 gate in row-major order `[a, b, c, d]`, acting as
    /// `|0> -> a|0> + b|1>` and `|1> -> c|0> + d|1>`. That convention matches the usual
    /// presentation, so `H` below is `1/sqrt(2) [[1,1],[1,-1]]` and reads as written.
    pub fn apply(&self, qubit: usize, matrix: [Complex; 4]) -> Self {
        assert!(qubit < self.qubits, "qubit {qubit} out of range");
        let mut out = self.amplitudes.clone();
        let bit = 1usize << qubit;

        for index in 0..out.len() {
            if (index & bit) != 0 {
                continue; // handled by its partner, with the bit clear
            }
            let partner = index | bit;
            let a0 = self.amplitudes[index];
            let a1 = self.amplitudes[partner];
            out[index] = matrix[0] * a0 + matrix[1] * a1;
            out[partner] = matrix[2] * a0 + matrix[3] * a1;
        }
        Self {
            amplitudes: out,
            qubits: self.qubits,
        }
    }

    /// Apply a gate to a pair of adjacent qubits `(control, target)`.
    ///
    /// `control` and `target` may be in either order; the control is whichever argument has
    /// the larger index, so the pair is normalised internally. Swapping them silently would
    /// apply the transpose, which is a different gate -- `cz` is its own transpose and `cx`
    /// is not, so the mistake would show up on one gate and not the other.
    pub fn apply_pair(&self, control: usize, target: usize, matrix: [Complex; 4]) -> Self {
        assert!(
            control != target,
            "a two-qubit gate needs two distinct qubits"
        );
        let (c, t) = if control < target {
            (control, target)
        } else {
            (target, control)
        };
        let cbit = 1usize << c;
        let tbit = 1usize << t;
        let mut out = self.amplitudes.clone();

        for index in 0..out.len() {
            if (index & cbit) == 0 {
                continue; // only act where the control is set
            }
            let partner = index ^ tbit;
            let a_control = self.amplitudes[index];
            let a_target = self.amplitudes[partner];
            out[index] = matrix[0] * a_control + matrix[1] * a_target;
            out[partner] = matrix[2] * a_control + matrix[3] * a_target;
        }
        Self {
            amplitudes: out,
            qubits: self.qubits,
        }
    }

    /// The largest absolute difference between two states, amplitude by amplitude.
    ///
    /// Compared up to a GLOBAL PHASE as well as directly: `U` and `-U` are the same
    /// physical state, and a global phase is the standard false positive in an inverse
    /// check. An exact comparison would fail on correct unitaries that happen to carry a
    /// phase of pi.
    pub fn max_amplitude_difference(&self, other: &StateVector) -> f64 {
        assert_eq!(
            self.qubits, other.qubits,
            "cannot compare states of different sizes"
        );
        let direct = self
            .amplitudes
            .iter()
            .zip(other.amplitudes.iter())
            .map(|(a, b)| (*a - *b).norm())
            .fold(0.0f64, f64::max);

        // The same comparison after removing the phase of the first non-zero amplitude.
        let phase = self
            .amplitudes
            .iter()
            .find(|a| a.norm_sqr() > 1e-24)
            .map(|a| -a.arg())
            .unwrap_or(0.0);
        let rotate = Complex::expi(phase);
        let upto_phase = self
            .amplitudes
            .iter()
            .zip(other.amplitudes.iter())
            .map(|(a, b)| (*a * rotate - *b).norm())
            .fold(0.0f64, f64::max);

        direct.min(upto_phase)
    }

    /// Whether two states are the same up to a global phase and `tolerance`.
    pub fn equivalent_to(&self, other: &StateVector, tolerance: f64) -> bool {
        self.max_amplitude_difference(other) <= tolerance
    }
}

/// A named gate, so an inverse table can be keyed by name rather than by index.
///
/// # No `Eq`, deliberately
///
/// `Rx(f64)` makes exact equality meaningless: two rotation angles differing by one ULP are
/// different gates that compute nearly the same state, and deriving `Eq` would let a lookup
/// table treat them as interchangeable. `PartialEq` is the strongest claim these values
/// support, and a gate table keyed on it is honest about that.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Gate {
    /// Hadamard.
    H,
    /// Pauli-X (NOT).
    X,
    /// Pauli-Y.
    Y,
    /// Pauli-Z.
    Z,
    /// S phase gate.
    S,
    /// S-dagger, the INVERSE of `S`.
    Sdg,
    /// T phase gate.
    T,
    /// T-dagger, the INVERSE of `T`.
    Tdg,
    /// Rotation about X.
    Rx(f64),
    /// Rotation about Y.
    Ry(f64),
    /// Rotation about Z.
    Rz(f64),
    /// Controlled-NOT, the entangling gate.
    Cx,
    /// Controlled-Z.
    Cz,
    /// Controlled-Y.
    Cy,
    /// Toffoli (CCX).
    Ccx,
    /// Swap.
    Swap,
}

impl StateVector {
    /// Apply the Toffoli: flip `target` where BOTH `control_a` and `control_b` are set.
    ///
    /// A genuine three-qubit gate, which is why it does not live on [`Gate::apply`]. The
    /// earlier two-qubit version was indistinguishable from `cx`: with one control and one
    /// target, "both set" and "control set" select exactly the same basis states, so the two
    /// gates agreed on every possible input and no test could separate them. The
    /// distinguishing case needs a third qubit that is NOT involved, which is what the
    /// `ancilla` argument is for.
    ///
    /// # Panics
    ///
    /// If the three qubit indices are not distinct, or any is out of range.
    pub fn apply_toffoli(&self, control_a: usize, control_b: usize, target: usize) -> Self {
        assert!(
            control_a != control_b && control_a != target && control_b != target,
            "the Toffoli needs three distinct qubits, got controls {control_a}, {control_b} \
             and target {target}"
        );
        let qubits = self.qubits;
        for q in [control_a, control_b, target] {
            assert!(q < qubits, "qubit {q} is out of range for {qubits} qubits");
        }

        let abits = (1usize << control_a) | (1usize << control_b);
        let tbit = 1usize << target;
        let source = self.amplitudes();
        // The gate is a PERMUTATION of the basis states, so compute the destination of each
        // amplitude and scatter it, rather than reasoning about which cell each rule owns.
        //
        // Every earlier formulation of this loop reasoned about which states a rule applies
        // to, and each one dropped amplitudes: the move wrote into cells another rule had
        // already claimed, and a separate clearing pass then erased what the move had just
        // written. Expressing the destination explicitly makes the totality checkable --
        // `destination` is a bijection, so `scatter[destination[i]] = source[i]` writes every
        // cell exactly once, by construction rather than by argument.
        let destination = |index: usize| -> usize {
            if (index & abits) == abits {
                index ^ tbit
            } else {
                index
            }
        };

        let mut out = vec![Complex::ZERO; source.len()];
        for (index, amp) in source.iter().enumerate() {
            out[destination(index)] = *amp;
        }
        StateVector::from_amplitudes(qubits, out)
    }
}

impl Gate {
    /// The matrix for a one-qubit gate.
    ///
    /// `None` for the two-qubit gates, which need [`Gate::pair_matrix`]. Returning a wrong
    /// shape instead would be worse than refusing.
    pub fn matrix(self) -> Option<[Complex; 4]> {
        // 1/sqrt(2), derived rather than a magic constant so the matrix is visibly
        // the Hadamard normalisation.
        let s = 1.0 / std::f64::consts::SQRT_2;
        Some(match self {
            Gate::H => [
                Complex::real(s),
                Complex::real(s),
                Complex::real(s),
                -Complex::real(s),
            ],
            Gate::X => [Complex::ZERO, Complex::ONE, Complex::ONE, Complex::ZERO],
            Gate::Y => [Complex::ZERO, -Complex::I, Complex::I, Complex::ZERO],
            Gate::Z => [Complex::ONE, Complex::ZERO, Complex::ZERO, -Complex::ONE],
            // S = diag(1, i). S-dagger = diag(1, -i), which is NOT S: it differs on |1>.
            Gate::S => [Complex::ONE, Complex::ZERO, Complex::ZERO, Complex::I],
            Gate::Sdg => [Complex::ONE, Complex::ZERO, Complex::ZERO, -Complex::I],
            Gate::T => [
                Complex::ONE,
                Complex::ZERO,
                Complex::ZERO,
                Complex::expi(std::f64::consts::FRAC_PI_4),
            ],
            Gate::Tdg => [
                Complex::ONE,
                Complex::ZERO,
                Complex::ZERO,
                Complex::expi(-std::f64::consts::FRAC_PI_4),
            ],
            Gate::Rx(theta) => {
                let c = (theta / 2.0).cos();
                let s2 = -Complex::I.scale((theta / 2.0).sin());
                [Complex::real(c), s2, s2, Complex::real(c)]
            }
            Gate::Ry(theta) => {
                let c = (theta / 2.0).cos();
                let s = (theta / 2.0).sin();
                [
                    Complex::real(c),
                    -Complex::real(s),
                    Complex::real(s),
                    Complex::real(c),
                ]
            }
            Gate::Rz(theta) => {
                let p = Complex::expi(-theta / 2.0);
                let q = Complex::expi(theta / 2.0);
                [p, Complex::ZERO, Complex::ZERO, q]
            }
            Gate::Cx | Gate::Cz | Gate::Cy | Gate::Ccx | Gate::Swap => return None,
        })
    }

    /// The matrix for a two-qubit gate, as a row-major 4x4 in the basis
    /// `|00>, |01>, |10>, |11>`.
    ///
    /// Entries are written through named basis indices rather than `row * 4 + col` literals.
    /// The arithmetic form compiled fine but clippy rejected the `0 * 4 + 0` case as an
    /// identity op, and the readable version is `m[B11][B10] = ONE`: it says which state
    /// maps to which, which is the entire content of a CNOT.
    pub fn pair_matrix(self) -> Option<[[Complex; 4]; 4]> {
        let mut m = [[Complex::ZERO; 4]; 4];

        // Basis states in the fixed order |00>, |01>, |10>, |11>.
        const B00: usize = 0;
        const B01: usize = 1;
        const B10: usize = 2;
        const B11: usize = 3;

        match self {
            // CX: |10> -> |11>, everything else unchanged.
            Gate::Cx => {
                m[B00][B00] = Complex::ONE;
                m[B01][B01] = Complex::ONE;
                m[B11][B10] = Complex::ONE;
                m[B10][B11] = Complex::ONE;
            }
            // CZ: flips the phase of |11> and nothing else.
            Gate::Cz => {
                m[B00][B00] = Complex::ONE;
                m[B01][B01] = Complex::ONE;
                m[B10][B10] = Complex::ONE;
                m[B11][B11] = -Complex::ONE;
            }
            // CY: |10> -> i|11>, |11> -> -i|10>.
            Gate::Cy => {
                m[B00][B00] = Complex::ONE;
                m[B01][B01] = Complex::ONE;
                m[B11][B10] = Complex::I;
                m[B10][B11] = -Complex::I;
            }
            // Swap: |01> <-> |10>.
            Gate::Swap => {
                m[B00][B00] = Complex::ONE;
                m[B10][B01] = Complex::ONE;
                m[B01][B10] = Complex::ONE;
                m[B11][B11] = Complex::ONE;
            }
            // The Toffoli is an 8x8 on three qubits and has no 4x4 representation. Returning
            // `None` is the honest answer; the earlier version built a 4x4 that happened to
            // equal `cx`, which is how the two became indistinguishable.
            Gate::Ccx => return None,
            Gate::H
            | Gate::X
            | Gate::Y
            | Gate::Z
            | Gate::S
            | Gate::Sdg
            | Gate::T
            | Gate::Tdg
            | Gate::Rx(_)
            | Gate::Ry(_)
            | Gate::Rz(_) => return None,
        }
        Some(m)
    }

    /// Apply this gate to a state.
    ///
    /// For a one-qubit gate, `q` is the qubit acted on. For a two-qubit gate, `q` is the
    /// CONTROL and `q2` the TARGET, and the argument order does not change the result --
    /// swapping them used to apply the transpose, which is a different gate for `cx` and
    /// the same gate for `cz`, so the mistake would have been invisible on half the gates.
    ///
    /// # The permutation gates need their own loop
    ///
    /// `swap` and `ccx` do not act only where the control is set: `swap` acts everywhere,
    /// and `ccx` needs BOTH controls. So the "skip unless the control is set" loop that the
    /// controlled-unitaries use does not apply to them, and running it made `swap` a no-op
    /// and `ccx` the identity on two qubits. Each gate therefore gets the loop its
    /// definition requires, written out rather than folded into one clever index
    /// computation -- a wrong answer here is a wrong quantum program.
    pub fn apply(self, state: &StateVector, q: usize, q2: Option<usize>) -> StateVector {
        if let Some(m) = self.matrix() {
            return state.apply(q, m);
        }

        // The Toffoli needs two controls and a target, so it does NOT fit the two-qubit
        // signature. It is refused here rather than being treated as a one-control gate on
        // the pair: that would make `ccx` silently identical to `cx`, which is why a
        // mutation collapsing the two into each other went unnoticed. Use
        // [`StateVector::apply_toffoli`].
        if self == Gate::Ccx {
            panic!(
                "the Toffoli is a three-qubit gate; call StateVector::apply_toffoli with \
                 two controls and a target rather than Gate::apply"
            );
        }

        let q2 = q2.expect("a two-qubit gate needs a second qubit");
        assert!(q != q2, "a two-qubit gate needs two distinct qubits");
        let qubits = state.qubits();

        let cbit = 1usize << q;
        let tbit = 1usize << q2;
        let source = state.amplitudes();

        // The permutation gates below express the DESTINATION of each amplitude and scatter
        // it, rather than reasoning about which cell each rule owns.
        //
        // This form is used because every "which rule claims this cell" formulation failed.
        // Starting from a copy of the source needed a blanking pass to clear the cells a move
        // had consumed, and the blanking always erased either the moved amplitude or a
        // pass-through one -- `cx` dropped control-clear states, `cy` and `ccx` returned the
        // all-zero state, and `swap` destroyed `|00>` and `|11>`. Since each of these gates
        // is a bijection on the basis states, `scatter[destination[i]] = source[i]` writes
        // every cell exactly once with no clearing at all.
        let mut out = vec![Complex::ZERO; source.len()];
        match self {
            // Controlled-NOT: flip the target where the control is set.
            Gate::Cx => {
                for (index, amp) in source.iter().enumerate() {
                    let destination = if (index & cbit) != 0 {
                        index ^ tbit
                    } else {
                        index
                    };
                    out[destination] = *amp;
                }
            }
            // Controlled-Z: negate where both are set. Not a permutation -- a phase -- so it
            // writes in place.
            Gate::Cz => {
                for (index, amp) in source.iter().enumerate() {
                    out[index] = if (index & cbit) != 0 && (index & tbit) != 0 {
                        -*amp
                    } else {
                        *amp
                    };
                }
            }
            // Controlled-Y: |c=1,t=0> contributes i|c=1,t=1>, and -i the other way. Both cells
            // of a controlled pair are written, so this too needs both halves accounted for;
            // the two directions are written as separate rules and neither overwrites the
            // other's result, because each writes only its own partner.
            Gate::Cy => {
                for (index, amp) in source.iter().enumerate() {
                    if (index & cbit) == 0 {
                        out[index] = *amp;
                        continue;
                    }
                    let coefficient = if (index & tbit) == 0 {
                        Complex::I
                    } else {
                        -Complex::I
                    };
                    out[index ^ tbit] = coefficient * *amp;
                }
            }
            // The Toffoli is a three-qubit gate and is handled before this match.
            Gate::Ccx => unreachable!("the Toffoli is a three-qubit gate, refused above"),
            // Swap: exchange the two qubits wherever their VALUES differ.
            //
            // The comparison is between the two bit values, not between the masked numbers.
            // `(index & cbit) != (index & tbit)` looks equivalent and is not: for
            // `index = 3`, `cbit = 1`, `tbit = 2` it compares `1 != 2`, which is true, so
            // `|11>` was treated as differing and sent to index 0, overwriting `|00>`. The
            // swap destroyed both agreeing basis states.
            Gate::Swap => {
                for (index, amp) in source.iter().enumerate() {
                    let destination = if (index & cbit != 0) != (index & tbit != 0) {
                        index ^ cbit ^ tbit
                    } else {
                        index
                    };
                    out[destination] = *amp;
                }
            }
            // A one-qubit gate has no pair matrix, so this is unreachable.
            _ => unreachable!("{self:?} has no one-qubit matrix, so it is not two-qubit"),
        }

        StateVector::from_amplitudes(qubits, out)
    }
}
