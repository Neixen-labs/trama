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
//!
//! `generators.json` is written rather than downloaded, because no published benchmark carries a
//! generator's `xdss_pu`: a subtransient reactance is data a load flow never reads, so a file only
//! ever used for load flow does not have one. It is four buses —
//!
//! ```text
//! ext_grid(110 kV) --line-- b1 --trafo 110/20-- b2 --line-- b3 --load
//!                            |                   |
//!                         machine A           machine B
//! ```
//!
//! — with machine A rated at its own bus's 110 kV and running at rating, and machine B rated at
//! 21 kV on a 20 kV bus and running 5% above it, so both ratios in `K_G` are exercised rather than
//! left at one. Its golden file is the same `calc_sc` call as above, over `generators.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::{Importer, node_properties, parse_graph, read_sections};
use trama_power::PowerImporter;
use trama_power::flow::{self, C};
use trama_power::network::{self, Study};

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

/// A machine that carries no `xdss_pu` cannot be a source, and guessing one would put a source of
/// unknown strength on the bus. `case14` has four generators and none of them declares it, which
/// is what a load flow benchmark looks like: the reactance is data a load flow never reads.
#[test]
fn a_generator_that_declares_no_reactance_is_refused_rather_than_guessed() {
    let container = compiled("case14.json");
    match network::model(&container, Study::Fault { c_max: C_MAX }) {
        Err(message) => assert!(message.contains("xdss_pu"), "{message}"),
        Ok(_) => panic!("a generator with no subtransient reactance has no fault contribution to compute"),
    }
    // The same file still answers the study it does carry the data for, so the refusal is about
    // the missing column and not about the network.
    assert!(network::model(&container, Study::Flow { scaling: 1.0 }).is_ok());
}

/// Two synchronous machines feeding a fault, against `pandapower.shortcircuit.calc_sc`.
///
/// A generator is the second largest source on any network that has one, and it arrives with two
/// ratios that a simpler network would hide: machine B sits on a 20 kV bus while rated at 21 kV
/// and runs 5% above that rating, so both terms of `K_G` are exercised. Machine A is rated at its
/// bus's own voltage and runs at rating, which is the case where both terms are 1 — a solver that
/// dropped them would match on A and miss on B.
#[test]
fn every_fault_current_with_generators_matches_pandapower() {
    let container = compiled("generators.json");
    let model = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap_or_else(|error| panic!("{error}"));
    let currents = network::fault_currents(&model, C_MAX).unwrap();
    let golden = expected("generators.solved.json");

    let mut worst = 0.0f64;
    let mut compared = 0;
    for (position, index) in positions(&container).iter().enumerate() {
        let reference = golden["ikss_ka"][index.to_string()].as_f64().unwrap();
        worst = worst.max((currents[position] - reference).abs() / reference);
        assert!(reference > 1.0, "every bus here carries kiloamps: bus {index} has {reference}");
        compared += 1;
    }
    assert_eq!(compared, 4, "four buses");
    assert!(worst < 1e-9, "worst relative difference {worst:e}");
}

/// The generator contributes, and the size of that contribution is the point: removing the
/// machines drops the current at their own buses by more than a third. A solver that read the
/// table and did nothing with it would pass the comparison above only if the reference were
/// generated the same broken way, so this asserts the difference is there at all.
#[test]
fn a_generator_raises_the_fault_current_it_feeds() {
    let container = compiled("generators.json");
    let model = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap();
    let currents = network::fault_currents(&model, C_MAX).unwrap();
    let by_node = positions(&container);

    // The same network with the machines taken out. Their whole contribution is the admittance
    // they put on their bus — this file has no other shunt — so clearing it is exactly the
    // network the previous version of this solver would have computed.
    let mut bare = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap();
    for (position, index) in by_node.iter().enumerate() {
        if *index == 1 || *index == 2 {
            bare.buses[position].y_shunt = C::ZERO;
        }
    }
    let bare_currents = network::fault_currents(&bare, C_MAX).unwrap();

    for (position, index) in by_node.iter().enumerate() {
        let (fed, alone) = (currents[position], bare_currents[position]);
        assert!(fed > alone, "bus {index}: {fed} kA with the machines, {alone} kA without");
        if *index == 1 || *index == 2 {
            assert!(fed / alone > 1.33, "a machine on the faulted bus dominates it: bus {index} gains {}", fed / alone);
        }
    }
}
