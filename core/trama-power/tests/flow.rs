// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The power flow against pandapower, which is the only test that means anything here.
//!
//! A load flow that converges is not a load flow that is right: `docs/DECISIONS.md` records an
//! engine that converged on `mv_oberrhein` and reported voltages between 0.00002 and 1.3 p.u.
//! Every number below is compared against `oberrhein.solved.json`, which is what
//! `pandapower.runpp` returns for the very network in `oberrhein.json`. Regenerate it with:
//!
//! ```text
//! python -c "import json, pandapower as pp; net = pp.from_json('oberrhein.json'); pp.runpp(net); \
//!   json.dump({'bus': {str(i): [float(net.res_bus.vm_pu[i]), float(net.res_bus.va_degree[i])] \
//!   for i in net.bus.index}, 'line': {str(i): float(net.res_line.loading_percent[i]) \
//!   for i in net.line.index}, 'trafo': {str(i): float(net.res_trafo.loading_percent[i]) \
//!   for i in net.trafo.index}}, open('oberrhein.solved.json','w'))"
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::{Importer, edge_properties, node_properties, parse_graph, read_sections};
use trama_power::{PowerImporter, flow, network};

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn compiled() -> Vec<u8> {
    let import = PowerImporter.load(&networks().join("oberrhein.json"), &BTreeMap::new()).unwrap();
    trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
}

fn expected() -> Value {
    serde_json::from_str(&std::fs::read_to_string(networks().join("oberrhein.solved.json")).unwrap()).unwrap()
}

/// pandapower's index for each node and edge, so a result can be compared row by row.
fn indices(container: &[u8]) -> (BTreeMap<usize, i64>, BTreeMap<usize, (String, i64)>) {
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
            (position, (row["power:kind"].as_str().unwrap().to_string(), row["power:index"].as_i64().unwrap()))
        })
        .collect();
    (by_node, by_edge)
}

#[test]
fn every_bus_voltage_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, 1.0).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap_or_else(|error| panic!("{error}"));
    let golden = expected();
    let (by_node, _) = indices(&container);

    let mut worst_vm: f64 = 0.0;
    let mut worst_va: f64 = 0.0;
    let mut compared = 0;
    for (position, index) in &by_node {
        let reference = &golden["bus"][index.to_string()];
        let (vm, va) = (reference[0].as_f64().unwrap(), reference[1].as_f64().unwrap());
        worst_vm = worst_vm.max((solution.vm_pu[*position] - vm).abs());
        // Angles are compared modulo a full turn: a 150° shift accumulated round a ring can leave
        // the same physical angle a revolution away from the one the reference printed.
        let difference = (solution.va_rad[*position].to_degrees() - va).rem_euclid(360.0);
        worst_va = worst_va.max(difference.min(360.0 - difference));
        compared += 1;
    }

    assert_eq!(compared, 179, "every bus in the reference has a counterpart");
    // A per-unit voltage is reported to five decimals by anyone who reports it at all; agreeing to
    // eight means the two are solving the same network, not two networks that look alike.
    assert!(worst_vm < 1e-8, "worst voltage difference {worst_vm:e} p.u.");
    assert!(worst_va < 1e-6, "worst angle difference {worst_va:e} degrees");
}

#[test]
fn every_branch_loading_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, 1.0).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap();
    let loadings = network::loadings(&model, &solution);
    let golden = expected();
    let (_, by_edge) = indices(&container);

    let mut worst = 0.0f64;
    let mut compared = 0;
    for (position, loading) in loadings.iter().enumerate() {
        let (kind, index) = &by_edge[&position];
        let reference = golden[kind][index.to_string()].as_f64().unwrap();
        let difference = (loading.expect("every branch in this network is rated") - reference).abs();
        worst = worst.max(difference);
        compared += 1;
    }

    assert_eq!(compared, 183, "181 lines and 2 transformers");
    assert!(worst < 1e-6, "worst loading difference {worst} percentage points");
}

/// The six lines whose switch is open still carry their own charging current, and pandapower says
/// how much. Getting this wrong is invisible — they are under half a percent loaded — which is
/// exactly why it is worth a test of its own rather than trusting the aggregate above.
#[test]
fn a_line_behind_an_open_switch_charges_but_does_not_carry() {
    let container = compiled();
    let model = network::model(&container, 1.0).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap();
    let flows = flow::branch_flows(&model.branches, &solution);
    let (_, by_edge) = indices(&container);

    let open: Vec<usize> =
        (0..model.branches.len()).filter(|position| model.bus_entity[model.branches[*position].to].is_none()).collect();
    assert_eq!(open.len(), 6, "mv_oberrhein opens six ring switches");

    let golden = expected();
    for position in open {
        let (kind, index) = &by_edge[&position];
        let reference = golden[kind][index.to_string()].as_f64().unwrap();
        assert!(reference > 0.0, "pandapower reports charging current on line {index}");
        // Not exactly zero: the detached bus is solved like any other, so what leaves it is the
        // convergence tolerance rather than a current. Four orders of magnitude below that
        // tolerance is the honest bar — asking for an exact zero would be testing the arithmetic
        // of the last iteration, not the model.
        assert!(
            flows[position].current_to.abs() < 1e-9,
            "nothing flows out of the detached end of line {index}: {}",
            flows[position].current_to.abs()
        );
    }
}

#[test]
fn scaling_the_load_moves_the_voltage_the_way_a_network_does() {
    let container = compiled();
    let light = flow::solve(
        &network::model(&container, 0.5).unwrap().buses,
        &network::model(&container, 0.5).unwrap().branches,
    )
    .unwrap();
    let heavy = flow::solve(
        &network::model(&container, 2.0).unwrap().buses,
        &network::model(&container, 2.0).unwrap().branches,
    )
    .unwrap();

    // More load, lower voltage everywhere that is not held up by the external grid.
    let sagged = light.vm_pu.iter().zip(&heavy.vm_pu).filter(|(a, b)| b < a).count();
    assert!(sagged > light.vm_pu.len() / 2, "only {sagged} buses of {} sagged under load", light.vm_pu.len());
}
