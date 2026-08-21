// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! How far the fleet planner lands from the best feasible plan, and where that distance comes from.
//!
//! `tests/fleet.rs` asserts a bound — within a tenth of the optimum on two fixed instances. This
//! measures instead, over many random ones, and splits the gap into the two things it is made of:
//!
//! - **ordering**: the stops assigned to a round, driven in a worse sequence than they could be.
//!   Reordering each planned round optimally gives the exact ceiling on what any intra-route move
//!   — Or-opt, 3-opt, anything — could win, without writing one.
//! - **assignment**: which stops share a van. No intra-route move touches this.
//!
//! It is `#[ignore]`d because it is a measurement rather than a check: it enumerates the true
//! optimum, which is exponential, and it prints numbers rather than asserting on them. Run it with
//!
//! ```text
//! cargo test --release -p trama-routing --test gap -- --ignored --nocapture
//! ```
//!
//! **What it said on 2026-08-21**, on a 5×5 grid, 8 stops, 2 vans of capacity 8, 60 instances —
//! before the consolidation pass in `fleet.rs` existed, and after:
//!
//! ```text
//!                     missed          gap vs optimum         of which ordering
//! windows            before  after    (after)                (after)
//! shared slots         1/60   0/60    mean 7.15% max 37.66%  mean 1.67% max 22.41%
//! wrapped, slack 200  46/60   0/60    mean 7.48% max 41.05%  mean 0.00% max  0.00%
//! wrapped, slack 800  31/60   0/60    mean 6.70% max 44.81%  mean 0.31% max 11.04%
//! none                 0/60   0/60    mean 2.05% max 16.50%  mean 1.63% max 16.50%
//! ```
//!
//! The gap under wrapped windows *rises* after the fix, which is not a regression: the instances
//! that were being refused were the hard ones, and they now count towards the mean instead of
//! being skipped. The no-windows row is unchanged to the digit, which is the point — the pass
//! only runs when there are more rounds than vans.
//!
//! Two things came out of the measurement. The ordering ceiling *falls* as windows tighten, which
//! is the opposite of what `docs/DECISIONS.md` predicted when it deferred Or-opt: a tight window
//! leaves so few feasible orders that whichever one the planner finds is already the best. And
//! "missed" — instances with a feasible plan the planner refused to find — was 46 of 60. Keep
//! both window generators: the wrapped one is adversarial and overstates the failure by a long
//! way, the shared slots are what a depot actually sells.

use serde_json::{Value, json};
use trama_format::{compile, parse_graph, read_sections};
use trama_routing::Turns;
use trama_routing::fleet::{self, Fleet};

/// Rounds have to be long for an intra-route move to have anything to do: at three stops each,
/// 2-opt is already all but exhaustive and the ordering gap is zero by construction rather than
/// by merit. Hence a high capacity and few vans.
const STOPS: usize = 8;
const CAPACITY: f64 = 8.0;
const VEHICLES: usize = 2;
const INSTANCES: usize = 60;

fn line(id: &str, coordinates: Value) -> Value {
    json!({"type": "Feature", "id": id, "properties": {}, "geometry": {"type": "LineString", "coordinates": coordinates}})
}

/// A street grid. Unlike the ladder the other tests use, this has crossings — the only geometry
/// where reordering a round can win anything at all.
fn grid(n: usize) -> Vec<u8> {
    let mut features = Vec::new();
    let at = |i: usize, j: usize| [-3.70 + 0.004 * i as f64, 40.41 + 0.003 * j as f64];
    for i in 0..n {
        for j in 0..n {
            if i + 1 < n {
                features.push(line(&format!("h{i}_{j}"), json!([at(i, j), at(i + 1, j)])));
            }
            if j + 1 < n {
                features.push(line(&format!("v{i}_{j}"), json!([at(i, j), at(i, j + 1)])));
            }
        }
    }
    compile(&features, &[json!({"name": "vehicle", "entity_kind": "edge", "unit": "1"})], &[]).unwrap()
}

/// A deterministic stream, so a measurement can be repeated exactly.
struct Rng(u64);

impl Rng {
    fn below(&mut self, limit: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % limit as u64) as usize
    }
}

fn cost_matrix(graph: &trama_format::Graph, costs: &[f64], points: &[usize]) -> Vec<Vec<f64>> {
    let mut matrix = vec![vec![0.0; points.len()]; points.len()];
    for (from, origin) in points.iter().enumerate() {
        for (to, destination) in points.iter().enumerate() {
            if from == to {
                continue;
            }
            let leg = Fleet {
                depot: *origin,
                stops: vec![*destination],
                demands: vec![1.0],
                capacity: 10.0,
                vehicles: 1,
                windows: Vec::new(),
                service: Vec::new(),
            };
            // A one-stop round is out and back, and this grid is undirected, so half is the leg.
            matrix[from][to] = fleet::total_cost(&fleet::plan(graph, costs, &Turns::new(), &leg).unwrap()) / 2.0;
        }
    }
    matrix
}

fn feasible(matrix: &[Vec<f64>], windows: &[(f64, f64)], order: &[usize]) -> bool {
    let mut clock = 0.0;
    let mut previous = 0;
    for stop in order {
        clock += matrix[previous][stop + 1];
        let (opens, closes) = windows[*stop];
        clock = clock.max(opens);
        if clock > closes {
            return false;
        }
        previous = stop + 1;
    }
    true
}

fn round_trip(matrix: &[Vec<f64>], order: &[usize]) -> f64 {
    let mut total = 0.0;
    let mut previous = 0;
    for stop in order {
        total += matrix[previous][stop + 1];
        previous = stop + 1;
    }
    total + matrix[previous][0]
}

fn permute(order: &mut Vec<usize>, at: usize, visit: &mut impl FnMut(&[usize])) {
    if at == order.len() {
        visit(order);
        return;
    }
    for i in at..order.len() {
        order.swap(at, i);
        permute(order, at + 1, visit);
        order.swap(at, i);
    }
}

/// The cheapest feasible order of one group of stops, by exhaustion.
fn best_order(matrix: &[Vec<f64>], windows: &[(f64, f64)], group: &[usize]) -> f64 {
    if group.is_empty() {
        return 0.0;
    }
    let mut order = group.to_vec();
    let mut best = f64::INFINITY;
    permute(&mut order, 0, &mut |candidate| {
        if feasible(matrix, windows, candidate) {
            best = best.min(round_trip(matrix, candidate));
        }
    });
    best
}

/// Every split of the stops between the vans, with every order inside each: the true optimum.
fn optimum(matrix: &[Vec<f64>], demands: &[f64], windows: &[(f64, f64)], capacity: f64, vehicles: usize) -> f64 {
    let mut best = f64::INFINITY;
    for labelling in 0..vehicles.pow(demands.len() as u32) {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); vehicles];
        let mut code = labelling;
        for stop in 0..demands.len() {
            groups[code % vehicles].push(stop);
            code /= vehicles;
        }
        if groups.iter().any(|group| group.iter().map(|stop| demands[*stop]).sum::<f64>() > capacity) {
            continue;
        }
        best = best.min(groups.iter().map(|group| best_order(matrix, windows, group)).sum());
    }
    best
}

/// How the windows for one instance are made.
#[derive(Clone, Copy, PartialEq)]
enum Windows {
    /// None at all: the plain capacitated problem.
    None,
    /// A morning slot and an afternoon slot, which is what a depot actually sells and what many
    /// customers share.
    Slots,
    /// Wrapped tightly around one randomly chosen order, so often only that order works. Far more
    /// adversarial than real windows, and kept because it is where the planner's refusals live.
    Wrapped(f64),
}

#[test]
#[ignore = "a measurement, not a check: enumerates the optimum and prints"]
fn the_gap_and_what_it_is_made_of() {
    let container = grid(5);
    let graph =
        parse_graph(&read_sections(&container).unwrap().into_iter().find(|s| &s.kind == b"GRPH").unwrap().payload)
            .unwrap();
    let costs = trama_format::edge_lengths(&container).unwrap();
    let junctions = graph.nodes.len();
    let demands = vec![1.0; STOPS];

    println!("\nwindows            missed   gap vs optimum          of which ordering");
    for kind in [Windows::Slots, Windows::Wrapped(200.0), Windows::Wrapped(800.0), Windows::None] {
        let mut rng = Rng(0x5EED);
        let (mut gaps, mut orderings) = (Vec::new(), Vec::new());
        let (mut missed, mut impossible) = (0usize, 0usize);

        for _ in 0..INSTANCES {
            let mut chosen: Vec<usize> = Vec::new();
            while chosen.len() < STOPS + 1 {
                let node = rng.below(junctions);
                if !chosen.contains(&node) {
                    chosen.push(node);
                }
            }
            let (depot, stops) = (chosen[0], chosen[1..].to_vec());
            let mut points = vec![depot];
            points.extend(&stops);
            let matrix = cost_matrix(&graph, &costs, &points);

            // A random order, and the clock along it. Both window kinds are built from this, so
            // every instance has at least one feasible plan by construction: the single round
            // that drives that order, which fits because the demand equals one van's capacity.
            let mut order: Vec<usize> = (0..STOPS).collect();
            for i in (1..order.len()).rev() {
                order.swap(i, rng.below(i + 1));
            }
            let (mut clock, mut previous) = (0.0, 0);
            let mut arrival = [0.0; STOPS];
            for stop in &order {
                clock += matrix[previous][stop + 1];
                arrival[*stop] = clock;
                previous = stop + 1;
            }
            let span = clock + matrix[previous][0];
            let windows: Vec<(f64, f64)> = match kind {
                Windows::None => Vec::new(),
                Windows::Wrapped(slack) => {
                    (0..STOPS).map(|stop| ((arrival[stop] - slack).max(0.0), arrival[stop] + slack)).collect()
                }
                Windows::Slots => (0..STOPS)
                    .map(|_| match rng.below(2) {
                        0 => (0.0, span * 0.6),
                        _ => (span * 0.4, span * 2.0),
                    })
                    .collect(),
            };
            // The enumeration always wants windows; an instance without them has every order open.
            let unbounded = vec![(0.0, f64::INFINITY); STOPS];
            let against: &[(f64, f64)] = if windows.is_empty() { &unbounded } else { &windows };

            let fleet = Fleet {
                depot,
                stops: stops.clone(),
                demands: demands.clone(),
                capacity: CAPACITY,
                vehicles: VEHICLES,
                windows: windows.clone(),
                service: Vec::new(),
            };
            let best = optimum(&matrix, &demands, against, CAPACITY, VEHICLES);
            let Ok(plan) = fleet::plan(&graph, &costs, &Turns::new(), &fleet) else {
                // An instance with no feasible plan is nothing to answer for. One that has a plan
                // and was refused anyway is a wrong answer, and the interesting number here.
                if best.is_finite() {
                    missed += 1;
                } else {
                    impossible += 1;
                }
                continue;
            };
            if !best.is_finite() || best <= 0.0 {
                impossible += 1;
                continue;
            }

            // Distance, not `total_cost`. The clock at the end of a round includes the waiting a
            // window imposes, which no reordering can avoid and the enumeration below does not
            // count — comparing the two charges the planner for standing still.
            let planned: f64 = plan.iter().map(|a| round_trip(&matrix, &a.stops)).sum();
            let reordered: f64 = plan.iter().map(|a| best_order(&matrix, against, &a.stops)).sum();
            gaps.push(planned / best - 1.0);
            orderings.push((planned - reordered) / best);
        }

        let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64 * 100.0;
        let max = |values: &[f64]| values.iter().cloned().fold(0.0, f64::max) * 100.0;
        let name = match kind {
            Windows::None => "none".to_string(),
            Windows::Slots => "shared slots".to_string(),
            Windows::Wrapped(slack) => format!("wrapped, slack {slack:.0}"),
        };
        println!(
            "{name:<18} {missed:>2}/{INSTANCES}   mean {:>5.2}% max {:>6.2}%   mean {:>5.2}% max {:>6.2}%{}",
            mean(&gaps),
            max(&gaps),
            mean(&orderings),
            max(&orderings),
            if impossible > 0 { format!("   ({impossible} with no feasible plan)") } else { String::new() },
        );
    }
}
