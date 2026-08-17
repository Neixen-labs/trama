// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The short-circuit calculation against pandapower's own, on a network that carries the data.
//!
//! `oberrhein.json` cannot be the fixture here: its external grids declare no `s_sc_max_mva`, so
//! neither pandapower nor this crate can say anything about a fault on it — pandapower fails with
//! a NaN in its own admittance matrix. `cigre-mv.json` is CIGRE's medium-voltage benchmark as
//! pandapower ships it, and it does carry the infeed's fault level. Regenerate the golden file
//! with:
//!
//! ```text
//! python -c "import json, pandapower as pp, pandapower.shortcircuit as sc; \
//!   net = pp.from_json('cigre-mv.json'); sc.calc_sc(net, case='max', ip=False, ith=False); \
//!   json.dump({'ikss_ka': {str(i): float(net.res_bus_sc.ikss_ka[i]) for i in net.bus.index}}, \
//!   open('cigre-mv.solved.json','w'))"
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::{Importer, node_properties, parse_graph, read_sections};
use trama_power::network::{self, Study};
use trama_power::{PowerImporter, flow};

/// IEC 60909's voltage factor above 1 kV, which is every network this reads.
const C_MAX: f64 = 1.1;

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn compiled(name: &str) -> Vec<u8> {
    let import = PowerImporter.load(&networks().join(name), &BTreeMap::new()).unwrap();
    trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
}

fn expected(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(networks().join(name)).unwrap()).unwrap()
}

/// Each bus's position in the graph, against pandapower's own index for it.
fn positions(container: &[u8]) -> Vec<i64> {
    let graph = parse_graph(
        &read_sections(container).unwrap().into_iter().find(|section| &section.kind == b"GRPH").unwrap().payload,
    )
    .unwrap();
    let nodes = node_properties(container).unwrap();
    graph.nodes.iter().map(|node| nodes[node.property_row as usize]["power:index"].as_i64().unwrap()).collect()
}

#[test]
fn every_fault_current_matches_pandapower() {
    let container = compiled("cigre-mv.json");
    let model = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap();
    let currents = network::fault_currents(&model, C_MAX).unwrap_or_else(|error| panic!("{error}"));
    let golden = expected("cigre-mv.solved.json");

    let mut worst = 0.0f64;
    let mut compared = 0;
    for (position, index) in positions(&container).iter().enumerate() {
        let reference = golden["ikss_ka"][index.to_string()].as_f64().unwrap();
        // Relative, not absolute: these currents span 1.2 kA to 26 kA, and a tolerance that suits
        // the small ones would be meaningless against the large.
        worst = worst.max(((currents[position] - reference) / reference).abs());
        compared += 1;
    }

    assert_eq!(compared, 15, "every bus of the CIGRE benchmark");
    assert!(worst < 1e-9, "worst relative difference {worst:e}");
}

/// The load flow still agrees on this second network, which is the point of having it: the two
/// studies are built from one file by one importer, and a change that fixes one must not quietly
/// break the other.
#[test]
fn the_same_file_still_solves_its_load_flow() {
    let container = compiled("cigre-mv.json");
    let model = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap();
    let solution = flow::solve(&model.buses, &model.branches).unwrap_or_else(|error| panic!("{error}"));
    let golden = expected("cigre-mv.solved.json");

    let mut worst = 0.0f64;
    for (position, index) in positions(&container).iter().enumerate() {
        let reference = golden["bus"][index.to_string()][0].as_f64().unwrap();
        worst = worst.max((solution.vm_pu[position] - reference).abs());
    }
    assert!(worst < 1e-8, "worst voltage difference {worst:e} p.u.");
}

/// A network with no fault level declared cannot be studied, and says so rather than inventing
/// an infinite bus — which would report a fault current limited only by the lines, and read as a
/// plausible number to anyone sizing a breaker from it.
#[test]
fn a_network_without_a_declared_fault_level_is_refused() {
    let container = compiled("oberrhein.json");
    match network::model(&container, Study::Fault { c_max: C_MAX }) {
        Err(message) => assert!(message.contains("s_sc_max_mva"), "{message}"),
        Ok(_) => panic!("mv_oberrhein declares no short-circuit power; it cannot be faulted"),
    }
}

/// The same refusal for the other half of a fault this crate does not model. A synchronous machine
/// feeds a short circuit; leaving it out understates the current, and understating it is how a
/// breaker gets chosen that cannot clear the fault it was bought for. `case14` carries four.
#[test]
fn a_network_with_synchronous_machines_is_refused_a_fault_study() {
    let container = compiled("case14.json");
    match network::model(&container, Study::Fault { c_max: C_MAX }) {
        Err(message) => assert!(message.contains("subtransient"), "{message}"),
        Ok(_) => panic!("a generator feeds a fault, and this solver does not model that"),
    }
    // The same file still answers the study this crate does model, so the refusal is about the
    // fault and not about the network.
    assert!(network::model(&container, Study::Flow { scaling: 1.0 }).is_ok());
}
