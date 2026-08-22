// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A three-winding transformer whose tap changer sits at the star point, against pandapower.
//!
//! `trafo3w.rs` covers a changer at a terminal, which moves one winding's nominal voltage. This is
//! the other arrangement — regulation at the point all three windings meet — and it is different
//! arithmetic rather than the same with a flag. Moving the meeting point changes what every branch
//! sees, so from any one of them the step has to be re-referred through the position the changer
//! already stands at, which makes it a ratio of complex numbers:
//!
//! ```text
//! t' = 100·t / (100 + t·(pos − neutral)),  where t = step% · e^{i·step°}
//! ```
//!
//! and the regulated end of the equivalent branch is the *mirror* of the terminal case.
//!
//! `star-tap.json` is written rather than downloaded, like every other `trafo3w` fixture: no
//! published pandapower network carries one. Four units, each taping a different winding —
//!
//! - **A on `hv`, B on `mv`, C on `lv`**, at positions 4, −3 and 6 and angles 20°, −15° and 35°.
//!   Different windings because which end comes out regulated is the mirror of the terminal case,
//!   and a file that only ever taps on `hv` passes under either mapping. Different positions and
//!   angles because two units sharing an answer by coincidence prove half as much.
//! - **D declares a position with no `tap_step_degree`**, which taps nothing at all: every term of
//!   the correction is complex, so an undeclared angle leaves it undefined and pandapower's NaN
//!   propagates until the ratio falls back to one. That is a real file shape — `create_..._3w`
//!   leaves the column empty unless asked — and reproducing it is deliberate.
//!
//! Regenerate the golden file with the same call `trafo3w.rs` documents, over `star-tap.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::{Importer, edge_properties, node_properties, parse_graph, read_sections};
use trama_power::network::{self, Study};
use trama_power::{PowerImporter, flow};

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn compiled() -> Vec<u8> {
    let import = PowerImporter.load(&networks().join("star-tap.json"), &BTreeMap::new()).unwrap();
    trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
}

fn golden() -> Value {
    serde_json::from_str(&std::fs::read_to_string(networks().join("star-tap.solved.json")).unwrap()).unwrap()
}

/// What an edge is, which one, and which winding of it.
type Edges = BTreeMap<usize, (String, i64, String)>;

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

/// Every voltage, including the four star points — which is where a changer that regulates the
/// meeting point shows up first.
#[test]
fn every_bus_voltage_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap_or_else(|error| panic!("{error}"));
    let solution = flow::solve(&model.buses, &model.branches).unwrap_or_else(|error| panic!("{error}"));
    let reference = golden();
    let (by_node, _) = indices(&container);

    let (mut worst_vm, mut worst_va) = (0.0f64, 0.0f64);
    let mut compared = 0;
    for (position, index) in &by_node {
        let expected = match *index < 0 {
            true => &reference["star"][(-index - 1).to_string()],
            false => &reference["bus"][index.to_string()],
        };
        let (vm, va) = (expected[0].as_f64().unwrap(), expected[1].as_f64().unwrap());
        worst_vm = worst_vm.max((solution.vm_pu[*position] - vm).abs());
        let difference = (solution.va_rad[*position].to_degrees() - va).rem_euclid(360.0);
        worst_va = worst_va.max(difference.min(360.0 - difference));
        compared += 1;
    }

    assert_eq!(compared, 14, "ten buses and four star points");
    assert!(worst_vm < 1e-8, "worst voltage difference {worst_vm:e} p.u.");
    assert!(worst_va < 1e-6, "worst angle difference {worst_va:e} degrees");
}

/// And every winding's loading, which is what says the impedances came through the re-referral
/// unchanged rather than being quietly moved by it.
#[test]
fn every_winding_loading_matches_pandapower() {
    let container = compiled();
    let model = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap();
    let loadings = network::loadings(&model, &solution);
    let reference = golden();
    let (_, by_edge) = indices(&container);

    let mut worst = 0.0f64;
    let mut windings = 0;
    for (position, loading) in loadings.iter().enumerate() {
        let (kind, index, side) = &by_edge[&position];
        let loading = loading.expect("every branch here is rated");
        let expected = match kind.as_str() {
            "trafo3w" => {
                windings += 1;
                reference["trafo3w"][index.to_string()][side].as_f64().unwrap()
            }
            _ => reference[kind][index.to_string()].as_f64().unwrap(),
        };
        worst = worst.max((loading - expected).abs());
    }

    assert_eq!(windings, 12, "three windings on each of four transformers");
    assert!(worst < 1e-6, "worst loading difference {worst} percentage points");
}

/// The regulated end is the mirror of the terminal case, and unit D taps nothing.
///
/// This asserts on the model rather than on the answer, because both facts are invisible in a bus
/// voltage that already agrees: a ratio put on the wrong end of a branch is still *a* ratio, and
/// it moves the network in a direction some other error could cancel.
#[test]
fn the_regulated_end_is_the_far_one_and_a_changer_without_an_angle_is_none() {
    let container = compiled();
    let model = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap();
    let (_, by_edge) = indices(&container);

    // pandapower's own mapping: a star-point changer on `hv` regulates the equivalent branch's low
    // side, and one on `mv` or `lv` regulates its high side — the opposite of a terminal changer.
    // What that shows up as here is which branch carries a ratio away from one at all.
    let mut tapped: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (position, (kind, index, side)) in &by_edge {
        if kind != "trafo3w" {
            continue;
        }
        if (model.branches[*position].ratio.abs() - 1.0).abs() > 1e-12 {
            tapped.entry(*index).or_default().push(side.clone());
        }
    }

    // One tapped branch per unit, and it is the branch whose own winding the changer sits on —
    // the re-referral moves which *end* of that branch is regulated, never which branch it is.
    assert_eq!(tapped.get(&0).map(Vec::as_slice), Some(["hv".to_string()].as_slice()), "unit A taps on hv");
    assert_eq!(tapped.get(&1).map(Vec::as_slice), Some(["mv".to_string()].as_slice()), "unit B taps on mv");
    assert_eq!(tapped.get(&2).map(Vec::as_slice), Some(["lv".to_string()].as_slice()), "unit C taps on lv");
    // Unit D declares tap position 5 and no angle, so no branch of it is tapped at all.
    assert_eq!(tapped.get(&3), None, "a star-point changer with no step angle taps nothing");
}
