// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! An AC power flow, solved by Newton-Raphson in polar coordinates.
//!
//! This module knows electricity and nothing about pandapower, containers or solvers: it is given
//! buses carrying injections and branches carrying admittances, all in per-unit, and returns the
//! voltage at every bus. What a pandapower column means is [`crate::network`]'s problem; what a
//! state delta is belongs to [`crate::solver`].
//!
//! Scope, chosen deliberately: slack buses and PQ buses. A distribution network is loads, embedded
//! generation that behaves as negative load, and an infeed at the head — voltage-controlling
//! generators live upstream of it, in transmission. PV buses are the natural next addition and the
//! Jacobian is already blocked for them, so adding them changes the assembly, not the structure.

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
}

pub struct Bus {
    pub kind: BusKind,
    /// Net injection in per-unit on the system base, generation positive.
    pub p_pu: f64,
    pub q_pu: f64,
    /// The imposed voltage of a slack, and the flat start of everything else.
    pub vm_pu: f64,
    pub va_rad: f64,
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
pub fn admittances(buses: usize, branches: &[Branch]) -> Vec<Vec<C>> {
    let mut y = vec![vec![C::ZERO; buses]; buses];
    for branch in branches {
        let (f, t) = (branch.from, branch.to);
        let c = branch.coefficients();
        y[f][f] = y[f][f] + c.ff;
        y[f][t] = y[f][t] + c.ft;
        y[t][f] = y[t][f] + c.tf;
        y[t][t] = y[t][t] + c.tt;
    }
    y
}

/// Newton-Raphson in polar coordinates, from the flat start each bus carries.
pub fn solve(buses: &[Bus], branches: &[Branch]) -> Result<Solution, Failure> {
    energised(buses, branches)?;

    let n = buses.len();
    let y = admittances(n, branches);
    let mut vm: Vec<f64> = buses.iter().map(|bus| bus.vm_pu).collect();
    let mut va = start_angles(buses, branches);
    // Slack buses are known; every other bus contributes an angle and a magnitude unknown.
    let free: Vec<usize> = (0..n).filter(|&i| buses[i].kind != BusKind::Slack).collect();
    let m = free.len();
    if m == 0 {
        return Ok(Solution { vm_pu: vm, va_rad: va, iterations: 0 });
    }

    for iteration in 1..=MAX_ITERATIONS {
        let (p, q) = injections(&y, &vm, &va);
        // Mismatch: what the bus is asked to inject, minus what the voltages say it does.
        let mut residual = vec![0.0; 2 * m];
        for (slot, &bus) in free.iter().enumerate() {
            residual[slot] = buses[bus].p_pu - p[bus];
            residual[m + slot] = buses[bus].q_pu - q[bus];
        }
        let (worst_bus, worst) = free
            .iter()
            .enumerate()
            .map(|(slot, &bus)| (bus, residual[slot].abs().max(residual[m + slot].abs())))
            .fold((0, 0.0f64), |best, current| if current.1 > best.1 { current } else { best });
        if worst < TOLERANCE_PU {
            return Ok(Solution { vm_pu: vm, va_rad: va, iterations: iteration - 1 });
        }

        let jacobian = jacobian(&y, &vm, &va, &p, &q, &free);
        let step = gaussian(jacobian, residual)
            .map_err(|row| Failure::Singular { bus: free[if row < m { row } else { row - m }] })?;
        for (slot, &bus) in free.iter().enumerate() {
            va[bus] += step[slot];
            vm[bus] += step[m + slot];
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
pub fn thevenin(buses: usize, branches: &[Branch], shunts: &[(usize, C)]) -> Result<Vec<C>, Failure> {
    let mut y = admittances(buses, branches);
    for (bus, admittance) in shunts {
        y[*bus][*bus] = y[*bus][*bus] + *admittance;
    }
    // Solve Y·Z = I one column at a time, sharing the elimination: the matrix is factored once
    // and every column of the identity is carried through it.
    let identity: Vec<Vec<C>> = (0..buses)
        .map(|column| (0..buses).map(|row| if row == column { C::new(1.0, 0.0) } else { C::ZERO }).collect())
        .collect();
    let inverse = gaussian_complex(y, identity).map_err(|bus| Failure::Singular { bus })?;
    Ok((0..buses).map(|bus| inverse[bus][bus]).collect())
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
fn jacobian(y: &[Vec<C>], vm: &[f64], va: &[f64], p: &[f64], q: &[f64], free: &[usize]) -> Vec<Vec<f64>> {
    let m = free.len();
    let mut j = vec![vec![0.0; 2 * m]; 2 * m];
    for (row, &i) in free.iter().enumerate() {
        for (column, &k) in free.iter().enumerate() {
            let (g, b) = (y[i][k].re, y[i][k].im);
            if i == k {
                let vsq = vm[i] * vm[i];
                j[row][column] = -q[i] - b * vsq; // ∂P/∂θ
                j[row][m + column] = p[i] / vm[i] + g * vm[i]; // ∂P/∂|V|
                j[m + row][column] = p[i] - g * vsq; // ∂Q/∂θ
                j[m + row][m + column] = q[i] / vm[i] - b * vm[i]; // ∂Q/∂|V|
            } else {
                let delta = va[i] - va[k];
                let (sin, cos) = delta.sin_cos();
                let product = vm[i] * vm[k];
                j[row][column] = product * (g * sin - b * cos);
                j[row][m + column] = vm[i] * (g * cos + b * sin);
                j[m + row][column] = -product * (g * cos + b * sin);
                j[m + row][m + column] = vm[i] * (g * sin - b * cos);
            }
        }
    }
    j
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
        let buses = vec![
            Bus { kind: BusKind::Slack, p_pu: 0.0, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0 },
            Bus { kind: BusKind::Load, p_pu: -0.5, q_pu: -0.2, vm_pu: 1.0, va_rad: 0.0 },
        ];
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
            2,
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

    #[test]
    fn an_island_with_no_slack_is_named_not_solved() {
        let buses = vec![
            Bus { kind: BusKind::Slack, p_pu: 0.0, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0 },
            Bus { kind: BusKind::Load, p_pu: -0.1, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0 },
            Bus { kind: BusKind::Load, p_pu: -0.1, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0 },
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
