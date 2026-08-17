// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! An AC power flow, solved by Newton-Raphson in polar coordinates.
//!
//! This module knows electricity and nothing about pandapower, containers or solvers: it is given
//! buses carrying injections and branches carrying admittances, all in per-unit, and returns the
//! voltage at every bus. What a pandapower column means is [`crate::network`]'s problem; what a
//! state delta is belongs to [`crate::solver`].
//!
//! Three kinds of bus: slack, PQ, and PV — a machine holding a voltage by injecting whatever
//! reactive power that takes, until it reaches a limit it declares and stops holding it. The last
//! is what makes this a transmission solver as well as a distribution one, and the limit is what
//! makes it an honest one: a generator asked for more reactive power than it has stops controlling
//! voltage, and a study that ignores that reports a network held up by machines that cannot do it.

use std::fmt;

/// A complex number.
///
/// `std` has none and `num-complex` would be a dependency for six operators, against a repository
/// rule that keeps them countable. Nothing here is clever enough to be worth importing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C {
    pub re: f64,
    pub im: f64,
}

impl C {
    pub const ZERO: C = C { re: 0.0, im: 0.0 };

    pub fn new(re: f64, im: f64) -> C {
        C { re, im }
    }

    /// The complex number of unit magnitude at `radians`.
    pub fn polar(magnitude: f64, radians: f64) -> C {
        C::new(magnitude * radians.cos(), magnitude * radians.sin())
    }

    pub fn conj(self) -> C {
        C::new(self.re, -self.im)
    }

    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    pub fn inv(self) -> C {
        let denominator = self.norm_sqr();
        C::new(self.re / denominator, -self.im / denominator)
    }
}

impl std::ops::Add for C {
    type Output = C;
    fn add(self, other: C) -> C {
        C::new(self.re + other.re, self.im + other.im)
    }
}

impl std::ops::Sub for C {
    type Output = C;
    fn sub(self, other: C) -> C {
        C::new(self.re - other.re, self.im - other.im)
    }
}

impl std::ops::Mul for C {
    type Output = C;
    fn mul(self, other: C) -> C {
        C::new(self.re * other.re - self.im * other.im, self.re * other.im + self.im * other.re)
    }
}

impl std::ops::Div for C {
    type Output = C;
    // Dividing by multiplying by the reciprocal is what complex division is; clippy reads the `*`
    // in a `Div` as a transposed operator, which is a good rule and a false positive here.
    #[allow(clippy::suspicious_arithmetic_impl)]
    fn div(self, other: C) -> C {
        self * other.inv()
    }
}

impl std::ops::Neg for C {
    type Output = C;
    fn neg(self) -> C {
        C::new(-self.re, -self.im)
    }
}

/// What holds a bus's voltage, or fails to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BusKind {
    /// Voltage magnitude and angle both imposed. Every energised island needs exactly one.
    Slack,
    /// Power injected, voltage free: a load, embedded generation, or a junction with neither.
    Load,
    /// Voltage magnitude imposed, reactive power free between the limits a machine declares:
    /// a synchronous generator on automatic voltage regulation. Its real power is the bus's
    /// `p_pu` like any other injection; how much reactive power holding the voltage costs is
    /// what the study answers. A machine that declares no limits carries ±∞ and always holds.
    Voltage { q_min_pu: f64, q_max_pu: f64 },
}

pub struct Bus {
    pub kind: BusKind,
    /// Net injection in per-unit on the system base, generation positive.
    pub p_pu: f64,
    /// The reactive injection that is *not* the voltage machine's: load, static generation, a
    /// synchronous machine's own consumption. What a [`BusKind::Voltage`] machine adds on top of
    /// this is an answer rather than an input, which is why its limits live on the kind.
    pub q_pu: f64,
    /// The imposed voltage of a slack or a voltage machine, and the flat start of everything else.
    pub vm_pu: f64,
    pub va_rad: f64,
    /// Admittance to earth at this bus: a shunt capacitor or reactor under a load flow, and the
    /// source impedance of an external grid under a fault. Both are the same thing to the matrix.
    pub y_shunt: C,
}

impl Bus {
    /// A bus injecting nothing, holding nothing, and earthed by nothing: the starting point every
    /// importer fills in from there.
    pub fn floating() -> Bus {
        Bus { kind: BusKind::Load, p_pu: 0.0, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0, y_shunt: C::ZERO }
    }
}

/// One two-port in π, with an optional complex turns ratio: a line, or a transformer.
///
/// `ratio` is applied at the `from` end, which is where a tap changer sits on the high-voltage
/// side of a distribution transformer. A line is `ratio = 1 + 0j` and the arithmetic collapses to
/// the plain π model, so lines and transformers need no separate path through the solver.
pub struct Branch {
    pub from: usize,
    pub to: usize,
    /// Series admittance, per-unit.
    pub y_series: C,
    /// Half the total shunt at each end. They differ when a transformer's T model is converted to
    /// a π with unequal leakage split; for a line, and for the usual half-and-half transformer,
    /// they are equal.
    pub y_shunt_from: C,
    pub y_shunt_to: C,
    /// Complex turns ratio at the `from` end: magnitude for the tap, argument for the phase shift.
    /// A line is `1 + 0j`.
    pub ratio: C,
}

/// The four admittances of the tapped-π, which are what both the bus matrix and the branch
/// currents are built from. Derived once here so the two cannot disagree.
pub struct Coefficients {
    pub ff: C,
    pub ft: C,
    pub tf: C,
    pub tt: C,
}

impl Branch {
    pub fn coefficients(&self) -> Coefficients {
        let ratio_sqr = C::new(self.ratio.norm_sqr(), 0.0);
        Coefficients {
            // The ratio divides the from-side self term by |a|² and the two transfer terms by a
            // and its conjugate, which is what makes a phase shift asymmetric — and a phase shift
            // is exactly what a Dyn transformer imposes.
            ff: (self.y_series + self.y_shunt_from) / ratio_sqr,
            ft: -(self.y_series / self.ratio.conj()),
            tf: -(self.y_series / self.ratio),
            tt: self.y_series + self.y_shunt_to,
        }
    }
}

pub struct Solution {
    /// Voltage magnitude per bus, per-unit.
    pub vm_pu: Vec<f64>,
    /// Voltage angle per bus, radians.
    pub va_rad: Vec<f64>,
    /// Reactive power each bus injects into the network once solved, per-unit. For a bus holding
    /// a voltage this is the machine's output plus whatever else sits on the bus, and is the
    /// quantity its limits are checked against.
    pub q_pu: Vec<f64>,
    /// Buses whose machine ran out of reactive power and stopped holding its voltage, in the order
    /// they gave up. An operator reading a low voltage wants this list before anything else on the
    /// map: it names the machines that are no longer regulating, which is why the voltage moved.
    pub limited: Vec<usize>,
    pub iterations: usize,
}

/// Why a network has no answer, in terms an operator can act on.
///
/// "It did not converge" tells nobody anything. Every variant here names the bus to look at,
/// because a network that will not solve is nearly always one branch or one figure that is wrong,
/// and the residual points at it.
#[derive(Debug)]
pub enum Failure {
    /// Ran out of iterations. Carries where the mismatch was worst when it gave up.
    NoConvergence { iterations: usize, worst_bus: usize, worst_mismatch_pu: f64 },
    /// A group of buses with no slack in it: nothing sets their voltage, so nothing determines it.
    Unenergised { bus: usize, buses: usize },
    /// The Jacobian could not be factorised. A bus connected by nothing carrying admittance, most
    /// often a branch whose impedance was entered as zero.
    Singular { bus: usize },
}

impl fmt::Display for Failure {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::NoConvergence { iterations, worst_bus, worst_mismatch_pu } => write!(
                out,
                "the power flow did not converge in {iterations} iterations; the largest remaining \
                 mismatch is {worst_mismatch_pu:.4} p.u. at bus {worst_bus}, which is where to look"
            ),
            Failure::Unenergised { bus, buses } => write!(
                out,
                "{buses} buses including bus {bus} are connected to no external grid, so nothing \
                 sets their voltage"
            ),
            Failure::Singular { bus } => write!(
                out,
                "the network is singular at bus {bus}: it is most often a branch with zero impedance \
                 or a bus joined to the rest by nothing that carries current"
            ),
        }
    }
}

/// Convergence in per-unit power mismatch. 1e-8 p.u. on a 1 MVA base is a tenth of a watt, which is
/// far below anything a network model knows about itself, so this is precision to spare rather than
/// a tuning knob.
const TOLERANCE_PU: f64 = 1e-8;
const MAX_ITERATIONS: usize = 30;

/// The bus admittance matrix, dense.
///
/// ponytail: dense, O(n²) memory and O(n³) per solve. A distribution feeder is hundreds of buses,
/// where this is milliseconds. Past a few thousand it is the wrong structure and the answer is a
/// sparse matrix with an LU that reuses its ordering across iterations — the Jacobian's sparsity
/// pattern never changes between them.
pub fn admittances(buses: &[Bus], branches: &[Branch]) -> Vec<Vec<C>> {
    let mut y = vec![vec![C::ZERO; buses.len()]; buses.len()];
    for branch in branches {
        let (f, t) = (branch.from, branch.to);
        let c = branch.coefficients();
        y[f][f] = y[f][f] + c.ff;
        y[f][t] = y[f][t] + c.ft;
        y[t][f] = y[t][f] + c.tf;
        y[t][t] = y[t][t] + c.tt;
    }
    // A shunt is a branch with one end at earth, so it lands on the diagonal and nowhere else.
    for (bus, earthed) in buses.iter().enumerate() {
        y[bus][bus] = y[bus][bus] + earthed.y_shunt;
    }
    y
}

/// Newton-Raphson in polar coordinates, with reactive limits enforced around it.
///
/// Two loops. The inner one is Newton on a fixed set of bus kinds; the outer one asks what each
/// voltage machine had to inject to hold its bus, and takes the voltage control away from any that
/// exceeded what it declared it could deliver — the machine is pinned at its limit and its bus
/// becomes a PQ bus. Once taken away, control is never given back: a machine released to hold its
/// voltage again would be released into the very conditions that saturated it, and the two states
/// would alternate for as long as anyone let them.
///
/// The outer loop terminates because each pass converts at least one voltage bus and never the
/// reverse, so it runs at most once per machine. It is also why saturation cascades correctly:
/// the reactive power a pinned machine stops supplying is picked up by the ones still regulating,
/// which is how a network actually collapses, and a single pass would miss every machine but the
/// first to give up.
pub fn solve(buses: &[Bus], branches: &[Branch]) -> Result<Solution, Failure> {
    energised(buses, branches)?;

    let n = buses.len();
    let y = admittances(buses, branches);
    let mut vm: Vec<f64> = buses.iter().map(|bus| bus.vm_pu).collect();
    let mut va = start_angles(buses, branches);
    let p: Vec<f64> = buses.iter().map(|bus| bus.p_pu).collect();
    // What each bus injects reactively before its machine is asked for anything. A machine that
    // saturates has its output folded in here and stops being free.
    let mut q: Vec<f64> = buses.iter().map(|bus| bus.q_pu).collect();
    let mut kind: Vec<BusKind> = buses.iter().map(|bus| bus.kind).collect();
    let mut limited = Vec::new();
    let mut iterations = 0;

    loop {
        let solution = newton(&y, &p, &q, &kind, &mut vm, &mut va)?;
        iterations += solution.iterations;

        // What the machine itself had to produce: everything the bus injects, less what was on the
        // bus regardless. Checked after convergence rather than during it, because a mismatch
        // mid-iteration is an artefact of the step, not a statement about the machine.
        let saturated: Vec<(usize, f64)> = (0..n)
            .filter_map(|bus| match kind[bus] {
                BusKind::Voltage { q_min_pu, q_max_pu } => {
                    let machine = solution.q_pu[bus] - q[bus];
                    if machine > q_max_pu {
                        Some((bus, q_max_pu))
                    } else if machine < q_min_pu {
                        Some((bus, q_min_pu))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();
        if saturated.is_empty() {
            return Ok(Solution { limited, iterations, ..solution });
        }
        for (bus, limit) in saturated {
            kind[bus] = BusKind::Load;
            q[bus] += limit;
            limited.push(bus);
        }
    }
}

/// One Newton-Raphson solve with the bus kinds held fixed.
///
/// A slack contributes no unknown. A PQ bus contributes both an angle and a magnitude, and both
/// mismatches. A PV bus contributes only an angle and only the real mismatch: its magnitude is
/// imposed, and the reactive power that imposition costs is read off the answer rather than solved
/// for. So the system is not square in the two halves the way a PQ-only network is, and the
/// Jacobian is assembled from two index lists instead of one.
fn newton(
    y: &[Vec<C>],
    p: &[f64],
    q: &[f64],
    kind: &[BusKind],
    vm: &mut [f64],
    va: &mut [f64],
) -> Result<Solution, Failure> {
    let n = kind.len();
    let angles: Vec<usize> = (0..n).filter(|&i| kind[i] != BusKind::Slack).collect();
    let magnitudes: Vec<usize> = (0..n).filter(|&i| kind[i] == BusKind::Load).collect();
    let (a, m) = (angles.len(), magnitudes.len());
    if a == 0 {
        let (_, injected) = injections(y, vm, va);
        return Ok(Solution {
            vm_pu: vm.to_vec(),
            va_rad: va.to_vec(),
            q_pu: injected,
            limited: Vec::new(),
            iterations: 0,
        });
    }

    for iteration in 1..=MAX_ITERATIONS {
        let (injected_p, injected_q) = injections(y, vm, va);
        // Mismatch: what the bus is asked to inject, minus what the voltages say it does.
        let mut residual = vec![0.0; a + m];
        for (slot, &bus) in angles.iter().enumerate() {
            residual[slot] = p[bus] - injected_p[bus];
        }
        for (slot, &bus) in magnitudes.iter().enumerate() {
            residual[a + slot] = q[bus] - injected_q[bus];
        }
        let (worst_bus, worst) = residual
            .iter()
            .enumerate()
            .map(|(slot, value)| (if slot < a { angles[slot] } else { magnitudes[slot - a] }, value.abs()))
            .fold((0, 0.0f64), |best, current| if current.1 > best.1 { current } else { best });
        if worst < TOLERANCE_PU {
            return Ok(Solution {
                vm_pu: vm.to_vec(),
                va_rad: va.to_vec(),
                q_pu: injected_q,
                limited: Vec::new(),
                iterations: iteration - 1,
            });
        }

        let jacobian = jacobian(y, vm, va, &injected_p, &injected_q, &angles, &magnitudes);
        let step = gaussian(jacobian, residual)
            .map_err(|row| Failure::Singular { bus: if row < a { angles[row] } else { magnitudes[row - a] } })?;
        for (slot, &bus) in angles.iter().enumerate() {
            va[bus] += step[slot];
        }
        for (slot, &bus) in magnitudes.iter().enumerate() {
            vm[bus] += step[a + slot];
        }
        if iteration == MAX_ITERATIONS {
            return Err(Failure::NoConvergence { iterations: MAX_ITERATIONS, worst_bus, worst_mismatch_pu: worst });
        }
    }
    unreachable!("the loop returns on its last iteration")
}

/// What flows in at each end of a branch once the voltages are known, per-unit.
///
/// Both ends, not one: a line with charging current draws more at one end than it delivers at the
/// other, and a rating is against the larger. Reporting the average would understate every long
/// cable, which is the one place a distribution network is most likely to be near its limit.
pub struct BranchFlow {
    pub current_from: C,
    pub current_to: C,
    pub power_from: C,
    pub power_to: C,
}

pub fn branch_flows(branches: &[Branch], solution: &Solution) -> Vec<BranchFlow> {
    let v: Vec<C> =
        solution.vm_pu.iter().zip(&solution.va_rad).map(|(&magnitude, &angle)| C::polar(magnitude, angle)).collect();
    branches
        .iter()
        .map(|branch| {
            let c = branch.coefficients();
            let (vf, vt) = (v[branch.from], v[branch.to]);
            let current_from = c.ff * vf + c.ft * vt;
            let current_to = c.tf * vf + c.tt * vt;
            BranchFlow {
                current_from,
                current_to,
                power_from: vf * current_from.conj(),
                power_to: vt * current_to.conj(),
            }
        })
        .collect()
}

/// Where each bus's angle starts, with every transformer's phase shift already carried across it.
///
/// A flat start puts every angle at zero, which is fine until a network contains a Dyn transformer.
/// A distribution network is fed through one: its 150° shift means the true answer sits near −150°
/// on everything downstream, and Newton-Raphson started at zero is on the wrong side of a basin it
/// cannot cross — the residual grows instead of shrinking, and thirty iterations later the network
/// looks unsolvable when it is merely badly begun.
///
/// Walking out from each slack and subtracting the shift of every branch crossed puts each bus
/// within a few degrees of its answer before the first iteration. Buses reached by more than one
/// path keep the first angle found; the difference between paths is the load angle, which is small,
/// and it is what the iteration is for.
fn start_angles(buses: &[Bus], branches: &[Branch]) -> Vec<f64> {
    let mut angles: Vec<f64> = buses.iter().map(|bus| bus.va_rad).collect();
    let mut settled: Vec<bool> = buses.iter().map(|bus| bus.kind == BusKind::Slack).collect();
    let mut adjacency: Vec<Vec<(usize, f64)>> = vec![Vec::new(); buses.len()];
    for branch in branches {
        // Crossing from the ratio's side to the other subtracts the shift; the way back adds it.
        let shift = branch.ratio.im.atan2(branch.ratio.re);
        adjacency[branch.from].push((branch.to, -shift));
        adjacency[branch.to].push((branch.from, shift));
    }
    let mut frontier: Vec<usize> = (0..buses.len()).filter(|&bus| settled[bus]).collect();
    while let Some(bus) = frontier.pop() {
        for &(next, shift) in &adjacency[bus] {
            if !settled[next] {
                settled[next] = true;
                angles[next] = angles[bus] + shift;
                frontier.push(next);
            }
        }
    }
    angles
}

/// The diagonal of the bus impedance matrix: the Thévenin impedance seen looking into each bus.
///
/// This is what a short circuit is made of. `Z = Y⁻¹`, and `Z_ii` is the impedance a fault at bus
/// `i` sees — every path back to every source, in parallel, which is why it cannot be read off the
/// network by inspection and needs the matrix inverted.
///
/// Only the diagonal is returned because only the diagonal is asked for. The full inverse is
/// computed on the way there; a network large enough for that to matter wants the sparse
/// factorisation the `admittances` note describes, and would then want a different entry point
/// too, since the sparse answer for one bus is cheaper than for all of them.
pub fn thevenin(buses: &[Bus], branches: &[Branch]) -> Result<Vec<C>, Failure> {
    let y = admittances(buses, branches);
    let n = buses.len();
    // Solve Y·Z = I one column at a time, sharing the elimination: the matrix is factored once
    // and every column of the identity is carried through it.
    let identity: Vec<Vec<C>> = (0..n)
        .map(|column| (0..n).map(|row| if row == column { C::new(1.0, 0.0) } else { C::ZERO }).collect())
        .collect();
    let inverse = gaussian_complex(y, identity).map_err(|bus| Failure::Singular { bus })?;
    Ok((0..n).map(|bus| inverse[bus][bus]).collect())
}

/// Gaussian elimination with partial pivoting over complex numbers, with many right-hand sides.
///
/// `columns[k]` is the k-th right-hand side; the result is indexed the same way, so `out[k][i]` is
/// row `i` of the solution for column `k`. Real elimination would not do here: a fault current is
/// set by the ratio of resistance to reactance as much as by their sum.
fn gaussian_complex(mut a: Vec<Vec<C>>, mut columns: Vec<Vec<C>>) -> Result<Vec<Vec<C>>, usize> {
    let n = a.len();
    for column in 0..n {
        let pivot = (column..n).max_by(|&x, &y| a[x][column].abs().total_cmp(&a[y][column].abs())).unwrap();
        if a[pivot][column].abs() < 1e-14 {
            return Err(column);
        }
        a.swap(column, pivot);
        for rhs in columns.iter_mut() {
            rhs.swap(column, pivot);
        }
        for row in column + 1..n {
            let factor = a[row][column] / a[column][column];
            if factor == C::ZERO {
                continue;
            }
            let (upper, lower) = a.split_at_mut(row);
            for (target, source) in lower[0][column..n].iter_mut().zip(&upper[column][column..n]) {
                *target = *target - factor * *source;
            }
            for rhs in columns.iter_mut() {
                rhs[row] = rhs[row] - factor * rhs[column];
            }
        }
    }
    Ok(columns
        .into_iter()
        .map(|mut rhs| {
            for row in (0..n).rev() {
                let sum = (row + 1..n).fold(C::ZERO, |total, k| total + a[row][k] * rhs[k]);
                rhs[row] = (rhs[row] - sum) / a[row][row];
            }
            rhs
        })
        .collect())
}

/// Power leaving each bus into the network, given the voltages.
fn injections(y: &[Vec<C>], vm: &[f64], va: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let v: Vec<C> = vm.iter().zip(va).map(|(&m, &a)| C::polar(m, a)).collect();
    let mut p = vec![0.0; v.len()];
    let mut q = vec![0.0; v.len()];
    for i in 0..v.len() {
        // S = V · conj(Σ Y·V)
        let current = (0..v.len()).fold(C::ZERO, |sum, k| sum + y[i][k] * v[k]);
        let s = v[i] * current.conj();
        p[i] = s.re;
        q[i] = s.im;
    }
    (p, q)
}

/// The four blocks of ∂(P,Q)/∂(θ,|V|), laid out as one dense matrix.
///
/// Written from the standard derivation rather than by difference quotients: a numerical Jacobian
/// converges too, and then quadratically only until the step size reaches the differencing error,
/// which is precisely where a hard network needs it most.
///
/// `angles` are the buses whose angle is unknown, `magnitudes` those whose magnitude is too. They
/// are the same list only when no bus holds a voltage; a PV bus appears in the first and not the
/// second, which is what makes the matrix rectangular in its blocks and square overall.
fn jacobian(
    y: &[Vec<C>],
    vm: &[f64],
    va: &[f64],
    p: &[f64],
    q: &[f64],
    angles: &[usize],
    magnitudes: &[usize],
) -> Vec<Vec<f64>> {
    let (a, m) = (angles.len(), magnitudes.len());
    let mut j = vec![vec![0.0; a + m]; a + m];
    for (row, &i) in angles.iter().enumerate() {
        for (column, &k) in angles.iter().enumerate() {
            j[row][column] = derivatives(y, vm, va, p, q, i, k)[0];
        }
        for (column, &k) in magnitudes.iter().enumerate() {
            j[row][a + column] = derivatives(y, vm, va, p, q, i, k)[1];
        }
    }
    for (row, &i) in magnitudes.iter().enumerate() {
        for (column, &k) in angles.iter().enumerate() {
            j[a + row][column] = derivatives(y, vm, va, p, q, i, k)[2];
        }
        for (column, &k) in magnitudes.iter().enumerate() {
            j[a + row][a + column] = derivatives(y, vm, va, p, q, i, k)[3];
        }
    }
    j
}

/// `[∂P/∂θ, ∂P/∂|V|, ∂Q/∂θ, ∂Q/∂|V|]` of bus `i` with respect to bus `k`.
///
/// All four at once because they share the same trigonometry, and because the diagonal and the
/// off-diagonal are different formulas rather than one formula with a special case — keeping them
/// in one place is what stops the blocks from drifting apart.
fn derivatives(y: &[Vec<C>], vm: &[f64], va: &[f64], p: &[f64], q: &[f64], i: usize, k: usize) -> [f64; 4] {
    let (g, b) = (y[i][k].re, y[i][k].im);
    if i == k {
        let vsq = vm[i] * vm[i];
        [-q[i] - b * vsq, p[i] / vm[i] + g * vm[i], p[i] - g * vsq, q[i] / vm[i] - b * vm[i]]
    } else {
        let (sin, cos) = (va[i] - va[k]).sin_cos();
        let product = vm[i] * vm[k];
        [
            product * (g * sin - b * cos),
            vm[i] * (g * cos + b * sin),
            -product * (g * cos + b * sin),
            vm[i] * (g * sin - b * cos),
        ]
    }
}

/// Solve `a·x = b` by Gaussian elimination with partial pivoting.
///
/// ponytail: dense elimination, no factorisation reuse. The Jacobian changes every iteration so an
/// LU would be recomputed anyway; what a larger network wants is sparsity, not a saved factor.
/// Returns the pivot row it could not find a pivot for, so the caller can name the bus.
fn gaussian(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Result<Vec<f64>, usize> {
    let n = b.len();
    for column in 0..n {
        let pivot = (column..n).max_by(|&x, &y| a[x][column].abs().total_cmp(&a[y][column].abs())).unwrap();
        if a[pivot][column].abs() < 1e-12 {
            return Err(column);
        }
        a.swap(column, pivot);
        b.swap(column, pivot);
        for row in column + 1..n {
            let factor = a[row][column] / a[column][column];
            if factor == 0.0 {
                continue;
            }
            let (upper, lower) = a.split_at_mut(row);
            for (target, source) in lower[0][column..n].iter_mut().zip(&upper[column][column..n]) {
                *target -= factor * source;
            }
            b[row] -= factor * b[column];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let sum: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    Ok(x)
}

/// Every bus must reach a slack through something that carries current.
///
/// Checked before the Jacobian rather than after it fails, because an unenergised island produces
/// a singular matrix whose pivot names an arbitrary bus in it — true, and useless. This says how
/// many buses are adrift and names one, which is what someone tracing a disconnected feeder needs.
fn energised(buses: &[Bus], branches: &[Branch]) -> Result<(), Failure> {
    let mut reached = vec![false; buses.len()];
    let mut frontier: Vec<usize> =
        (0..buses.len()).filter(|&i| buses[i].kind == BusKind::Slack).inspect(|&i| reached[i] = true).collect();
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); buses.len()];
    for branch in branches {
        adjacency[branch.from].push(branch.to);
        adjacency[branch.to].push(branch.from);
    }
    while let Some(bus) = frontier.pop() {
        for &next in &adjacency[bus] {
            if !reached[next] {
                reached[next] = true;
                frontier.push(next);
            }
        }
    }
    let adrift: Vec<usize> = (0..buses.len()).filter(|&i| !reached[i]).collect();
    match adrift.first() {
        Some(&bus) => Err(Failure::Unenergised { bus, buses: adrift.len() }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two buses, one line, one load: small enough that the answer is arithmetic rather than
    /// another program's output.
    ///
    /// A slack at 1.0 p.u. feeds 0.5 + 0.2j p.u. through a pure reactance of 0.1 p.u. Solving
    /// V₂ analytically from V₂·conj((V₁−V₂)/jx) = −S gives the magnitude and angle below; the test
    /// is that Newton-Raphson finds the same numbers the algebra does.
    #[test]
    fn two_bus_feeder_matches_the_algebra() {
        let buses =
            vec![Bus { kind: BusKind::Slack, ..Bus::floating() }, Bus { p_pu: -0.5, q_pu: -0.2, ..Bus::floating() }];
        let branches = vec![Branch {
            from: 0,
            to: 1,
            y_series: C::new(0.0, -10.0), // 1/(j0.1)
            y_shunt_from: C::ZERO,
            y_shunt_to: C::ZERO,
            ratio: C::new(1.0, 0.0),
        }];
        let solution = solve(&buses, &branches).unwrap_or_else(|error| panic!("{error}"));

        // Check the solution satisfies the network equation rather than a number copied from a run.
        let v1 = C::polar(solution.vm_pu[1], solution.va_rad[1]);
        let v0 = C::polar(solution.vm_pu[0], solution.va_rad[0]);
        let current = (v0 - v1) * C::new(0.0, -10.0);
        let delivered = v1 * current.conj();
        assert!((delivered.re - 0.5).abs() < 1e-9, "P delivered {}", delivered.re);
        assert!((delivered.im - 0.2).abs() < 1e-9, "Q delivered {}", delivered.im);
        assert!(solution.vm_pu[1] < 1.0, "a load at the end of a reactance sags: {}", solution.vm_pu[1]);
    }

    #[test]
    fn a_phase_shift_is_not_symmetric() {
        // The whole point of a complex ratio: swapping the ends of a shifting branch is not the
        // same branch. If this passes with a real ratio, the shift is being dropped somewhere.
        let shift = C::polar(1.0, 30f64.to_radians());
        let y = admittances(
            &[Bus::floating(), Bus::floating()],
            &[Branch {
                from: 0,
                to: 1,
                y_series: C::new(0.0, -10.0),
                y_shunt_from: C::ZERO,
                y_shunt_to: C::ZERO,
                ratio: shift,
            }],
        );
        assert!((y[0][1] - y[1][0]).abs() > 1e-6, "a phase shift makes Y asymmetric: {:?} {:?}", y[0][1], y[1][0]);
        // Asymmetric in argument, equal in magnitude: the shift rotates power one way through the
        // branch without changing what the branch can carry. Dropping the shift would satisfy the
        // magnitude check too, which is why the inequality above has to come first.
        assert!((y[0][1].abs() - y[1][0].abs()).abs() < 1e-12, "and equal in magnitude");
        let rotation = (y[0][1] / y[1][0]).abs();
        assert!((rotation - 1.0).abs() < 1e-12, "the two differ by a pure rotation");
    }

    /// A line between two buses, one holding 1.05 p.u. and free to do so. The test is that it
    /// holds it exactly: a PV bus whose magnitude moved at all would mean the unknown was solved
    /// for rather than imposed, which is the whole difference between PV and PQ.
    #[test]
    fn a_voltage_bus_holds_its_magnitude_and_pays_for_it_in_reactive_power() {
        let buses = vec![
            Bus { kind: BusKind::Slack, ..Bus::floating() },
            Bus {
                kind: BusKind::Voltage { q_min_pu: f64::NEG_INFINITY, q_max_pu: f64::INFINITY },
                p_pu: -0.5,
                q_pu: -0.2,
                vm_pu: 1.05,
                ..Bus::floating()
            },
        ];
        let branches = vec![Branch {
            from: 0,
            to: 1,
            y_series: C::new(0.0, -10.0),
            y_shunt_from: C::ZERO,
            y_shunt_to: C::ZERO,
            ratio: C::new(1.0, 0.0),
        }];
        let solution = solve(&buses, &branches).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(solution.vm_pu[1], 1.05, "the machine holds its bus exactly");
        assert!(solution.limited.is_empty(), "a machine with no limits never gives up");
        // Holding a bus above the slack while drawing real power through a reactance takes
        // reactive power the load does not supply, so the machine is producing it.
        let machine = solution.q_pu[1] - buses[1].q_pu;
        assert!(machine > 0.0, "the machine produces reactive power to hold 1.05: {machine}");
    }

    /// The same network with the machine's reactive power capped below what holding 1.05 costs.
    /// It should stop holding, land on the cap exactly, and say so.
    #[test]
    fn a_machine_out_of_reactive_power_stops_holding_its_voltage() {
        let unlimited = vec![
            Bus { kind: BusKind::Slack, ..Bus::floating() },
            Bus {
                kind: BusKind::Voltage { q_min_pu: f64::NEG_INFINITY, q_max_pu: f64::INFINITY },
                p_pu: -0.5,
                q_pu: -0.2,
                vm_pu: 1.05,
                ..Bus::floating()
            },
        ];
        let branches = vec![Branch {
            from: 0,
            to: 1,
            y_series: C::new(0.0, -10.0),
            y_shunt_from: C::ZERO,
            y_shunt_to: C::ZERO,
            ratio: C::new(1.0, 0.0),
        }];
        let needed = {
            let free = solve(&unlimited, &branches).unwrap();
            free.q_pu[1] - unlimited[1].q_pu
        };

        let cap = needed / 2.0;
        let mut buses = unlimited;
        buses[1].kind = BusKind::Voltage { q_min_pu: f64::NEG_INFINITY, q_max_pu: cap };
        let solution = solve(&buses, &branches).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(solution.limited, vec![1], "the machine that gave up is named");
        assert!((solution.q_pu[1] - buses[1].q_pu - cap).abs() < 1e-9, "it sits on its limit exactly");
        assert!(solution.vm_pu[1] < 1.05, "and its bus falls away: {}", solution.vm_pu[1]);
    }

    #[test]
    fn an_island_with_no_slack_is_named_not_solved() {
        let buses = vec![
            Bus { kind: BusKind::Slack, ..Bus::floating() },
            Bus { p_pu: -0.1, ..Bus::floating() },
            Bus { p_pu: -0.1, ..Bus::floating() },
        ];
        let branches = vec![Branch {
            from: 0,
            to: 1,
            y_series: C::new(0.0, -10.0),
            y_shunt_from: C::ZERO,
            y_shunt_to: C::ZERO,
            ratio: C::new(1.0, 0.0),
        }];
        match solve(&buses, &branches) {
            Err(Failure::Unenergised { bus, buses }) => {
                assert_eq!((bus, buses), (2, 1));
            }
            Err(other) => panic!("wrong diagnosis: {other}"),
            Ok(_) => panic!("a bus connected to nothing cannot have a voltage"),
        }
    }
}
