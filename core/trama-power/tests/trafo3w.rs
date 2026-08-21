// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A three-winding transformer against pandapower's own answer, in both studies.
//!
//! `trafo3w.json` is written rather than downloaded, for the same reason `generators.json` is.
//! `example_multivoltage` is the only network in `pandapower.networks` carrying a `trafo3w`, and
//! it also carries an `impedance`, two `xward`s and four open bus-bus switches — none of which
//! this solver models. Every number would differ, for reasons that have nothing to do with the
//! transformer under test.
//!
//! What the fixture is built to exercise, since a hand-written network only tests what it was
//! written to test:
//!
//! - **Two units, and their windings rated differently.** 40/15/25 MVA and 63/50/20. The pair
//!   impedances are referred onto the smaller rating of each pair, so equal ratings would let a
//!   wrong `min` pass.
//! - **A tap on the high side of one and the medium side of the other.** pandapower maps a medium
//!   or low tap onto the *low* side of the equivalent transformer between the star point and that
//!   winding, and a file that only ever taps on `hv` passes under either mapping.
//! - **Different phase shifts** — 30° and 150° on the first unit's two lower windings.
//! - **Iron losses and no-load current on one unit and not the other**, because they belong to a
//!   single branch of the star and magnetising three times over is the easy mistake.
//! - **Generation on one tertiary**, so a branch carries power up through the transformer rather
//!   than down. A sign error survives a network where everything flows one way.
//! - **All three buses of the first unit drawn at one point**, which is what a substation looks
//!   like on a map and what the importer's ten-metre separation and the star point have to share.
//! - **An ordinary two-winding transformer whose tap position has no tap changer behind it.** Six
//!   steps of 1.5% and an empty `tap_changer_type`, which pandapower reads as nominal. This is not
//!   about three windings at all: it is here because the column is easy to leave unset — this
//!   fixture did, by accident, on its first draft — and reading the position alone puts 9% of
//!   voltage on a bus that does not have it.
//!
//! The static generator declares `current_source=False`: pandapower would otherwise model it as a
//! fault contribution, which this solver does not model at all, and the fixture would be comparing
//! two different networks and calling the difference a transformer bug.
//!
//! Regenerate the golden files with:
//!
//! ```text
//! python -c "import json, numpy as np, pandapower as pp; net = pp.from_json('trafo3w.json'); \
//!   pp.runpp(net); t3, res = net.trafo3w, net.res_trafo3w; \
//!   json.dump({'bus': {str(i): [float(net.res_bus.vm_pu[i]), float(net.res_bus.va_degree[i])] \
//!   for i in net.bus.index}, 'line': {str(i): float(net.res_line.loading_percent[i]) for i in \
//!   net.line.index}, 'trafo3w': {str(i): {s: float(res['i_%s_ka' % s][i] * t3['vn_%s_kv' % s][i] \
//!   * np.sqrt(3) / t3['sn_%s_mva' % s][i] * 100) for s in ('hv','mv','lv')} for i in t3.index}, \
//!   'star': {str(i): [float(res.vm_internal_pu[i]), float(res.va_internal_degree[i])] for i in \
//!   t3.index}, 'loading_percent': {str(i): float(res.loading_percent[i]) for i in t3.index}}, \
//!   open('trafo3w.solved.json','w'))"
//! ```
//!
//! and `trafo3w.sc.json` with the same `calc_sc(net, case='max', ip=False, ith=False)` call the
//! other fault fixtures use.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::{Importer, edge_properties, node_properties, parse_graph, read_sections};
use trama_power::network::{self, Study};
use trama_power::{PowerImporter, flow};

/// IEC 60909's voltage factor above 1 kV, which is every bus in this network.
const C_MAX: f64 = 1.1;

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn imported() -> trama_format::Import {
    PowerImporter.load(&networks().join("trafo3w.json"), &BTreeMap::new()).unwrap()
}

fn compiled() -> Vec<u8> {
    let import = imported();
    trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
}

fn golden(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(networks().join(name)).unwrap()).unwrap()
}

/// What an edge is, which one, and — for a three-winding transformer — which winding.
type Edges = BTreeMap<usize, (String, i64, String)>;

/// pandapower's index for each node, and each edge's kind, index and side.
fn indices(container: &[u8]) -> (BTreeMap<usize, i64>, Edges) {
    let graph = parse_graph(
        &read_sections(container).unwrap().into_iter().find(|section| &section.kind == b"GRPH").unwrap().payload,
    )
    .unwrap();
    let nodes = node_properties(container).unwrap();
    let edges = edge_properties(container).unwrap();
    let by_node = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (position, nodes[node.property_row as usize]["power:index"].as_i64().unwrap()))
        .collect();
    let by_edge = graph
        .edges
        .iter()
        .enumerate()
        .map(|(position, edge)| {
            let row = &edges[edge.property_row as usize];
            (
                position,
                (
                    row["power:kind"].as_str().unwrap().to_string(),
                    row["power:index"].as_i64().unwrap(),
                    row.get("power:side").and_then(Value::as_str).unwrap_or_default().to_string(),
                ),
            )
        })
        .collect();
    (by_node, by_edge)
}

/// One entity per winding, and a node for each star point that was not in `net.bus`.
#[test]
fn a_three_winding_transformer_becomes_three_edges_and_a_node() {
    let import = imported();
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for feature in &import.features {
        *kinds.entry(feature["properties"]["power:kind"].as_str().unwrap()).or_default() += 1;
    }

    assert_eq!(kinds["bus"], 8, "the eight buses the file declares");
    assert_eq!(kinds["trafo3w_star"], 2, "one star point per three-winding transformer");
    assert_eq!(kinds["trafo3w"], 6, "three branches each");
    assert_eq!(kinds["trafo"], 1, "and the ordinary transformer beside them");

    // Each star point carries the high side's nominal voltage, which is what pandapower gives its
    // own auxiliary bus, and a negative index no pandapower bus can collide with.
    for feature in &import.features {
        if feature["properties"]["power:kind"] != "trafo3w_star" {
            continue;
        }
        assert!(feature["properties"]["power:index"].as_i64().unwrap() < 0);
        assert_eq!(feature["properties"]["power:vn_kv"].as_f64().unwrap(), 110.0);
    }
}

/// The first unit's three buses are drawn at one point, and so is its star. All four have to end
/// up as four nodes: collapsing any two shorts a winding out and the network solves at one voltage
/// level, which converges and is wrong.
#[test]
fn a_star_point_never_lands_on_a_bus() {
    let container = compiled();
    let (by_node, _) = indices(&container);
    let mut seen: BTreeMap<i64, usize> = BTreeMap::new();
    for index in by_node.values() {
        *seen.entry(*index).or_default() += 1;
    }
    assert_eq!(by_node.len(), 10, "eight buses and two star points");
    assert!(seen.values().all(|count| *count == 1), "no two entities share a node");
}

/// Every voltage, including the star points — which pandapower reports as `vm_internal_pu` and is
/// the one number that says the impedance split landed where it should.
#[test]
fn every_bus_voltage_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap_or_else(|error| panic!("{error}"));
    let reference = golden("trafo3w.solved.json");
    let (by_node, _) = indices(&container);

    let (mut worst_vm, mut worst_va) = (0.0f64, 0.0f64);
    let mut compared = 0;
    for (position, index) in &by_node {
        let expected = match *index < 0 {
            // The star point of transformer `n` carries index `-(n + 1)`.
            true => &reference["star"][(-index - 1).to_string()],
            false => &reference["bus"][index.to_string()],
        };
        let (vm, va) = (expected[0].as_f64().unwrap(), expected[1].as_f64().unwrap());
        worst_vm = worst_vm.max((solution.vm_pu[*position] - vm).abs());
        let difference = (solution.va_rad[*position].to_degrees() - va).rem_euclid(360.0);
        worst_va = worst_va.max(difference.min(360.0 - difference));
        compared += 1;
    }

    assert_eq!(compared, 10);
    assert!(worst_vm < 1e-8, "worst voltage difference {worst_vm:e} p.u.");
    assert!(worst_va < 1e-6, "worst angle difference {worst_va:e} degrees");
}

/// Loading per winding, not per transformer. pandapower reports one `loading_percent` for a
/// three-winding transformer — the worst of its three — and the three are compared here because
/// the largest one would hide two wrong ones underneath it.
#[test]
fn every_winding_loading_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap();
    let loadings = network::loadings(&model, &solution);
    let reference = golden("trafo3w.solved.json");
    let (_, by_edge) = indices(&container);

    let mut worst = 0.0f64;
    let mut windings = 0;
    let mut highest: BTreeMap<i64, f64> = BTreeMap::new();
    for (position, loading) in loadings.iter().enumerate() {
        let (kind, index, side) = &by_edge[&position];
        let loading = loading.expect("every branch in this network is rated");
        let expected = match kind.as_str() {
            "trafo3w" => {
                windings += 1;
                let worst_so_far = highest.entry(*index).or_insert(0.0);
                *worst_so_far = worst_so_far.max(loading);
                reference["trafo3w"][index.to_string()][side].as_f64().unwrap()
            }
            _ => reference[kind][index.to_string()].as_f64().unwrap(),
        };
        worst = worst.max((loading - expected).abs());
    }

    assert_eq!(windings, 6, "three windings on each of two transformers");
    assert!(worst < 1e-6, "worst loading difference {worst} percentage points");

    // And the worst of the three is what pandapower calls the transformer's loading.
    for (index, loading) in &highest {
        let expected = reference["loading_percent"][index.to_string()].as_f64().unwrap();
        assert!((loading - expected).abs() < 1e-6, "trafo3w {index}: {loading} against {expected}");
    }
}

/// The fault, where the three-winding transformer carries IEC 60909 §3.7's correction on each pair
/// of windings before the star split rather than on the branches after it.
///
/// The three ways this can be got wrong were each measured by breaking the code and running this
/// test: dropping the correction moves every fault current by 1.5%, and putting the tap back into
/// a fault — which IEC 60909 does not — moves it by 13%. Both are the size that reads as a
/// modelling choice rather than as a bug, and both size a breaker on the comfortable side.
#[test]
fn every_fault_current_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap();
    let currents = network::fault_currents(&model, C_MAX).unwrap();
    let reference = golden("trafo3w.sc.json");
    let (by_node, _) = indices(&container);

    let mut worst = 0.0f64;
    let mut compared = 0;
    for (position, index) in &by_node {
        // pandapower reports no fault current at the star point, which is not a bus of the network.
        if *index < 0 {
            continue;
        }
        let expected = reference["ikss_ka"][index.to_string()].as_f64().unwrap();
        worst = worst.max((currents[*position] - expected).abs() / expected);
        compared += 1;
    }

    assert_eq!(compared, 8);
    assert!(worst < 1e-9, "worst fault current difference {worst:e} relative");
}
