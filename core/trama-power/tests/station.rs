// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A power station unit under IEC 60909 §3.7, against pandapower's own answer.
//!
//! A machine bolted to its step-up transformer is not a machine on a bus. Nothing is connected
//! between the two, so the standard corrects the pair with a single factor instead of giving the
//! machine `K_G` and the transformer `K_T`. Which factor turns on one column: whether the unit
//! transformer has an on-load tap changer.
//!
//! `station.json` is written rather than downloaded — no published pandapower network declares
//! `power_station_trafo` at all. What it exercises:
//!
//! ```text
//! ext_grid 110 --line-- b1 =+= transformer A (no OLTC, pt_percent) == machine A, 10 kV bus
//!                           |= transformer B (OLTC)                == machine B, 10 kV bus
//!                           |= transformer C (ordinary)            == machine C, 20 kV bus
//! ```
//!
//! - **A is `K_SO` and B is `K_S`**, which are different formulas rather than one with a flag:
//!   with an on-load changer the voltage ratios are squared and the two reactances *subtract*.
//! - **C is the control.** An ordinary machine behind an ordinary transformer, whose fault current
//!   must not move because the other two became units.
//! - **Every ratio is away from one.** The transformers are 115/10.5 on 110/10 buses and the
//!   machines are rated at 10.5 kV, so a term dropped from either correction shows up instead of
//!   cancelling against its own numerator.
//! - **A declares `pt_percent` and B does not**, so both the stated off-load tap range and the one
//!   pandapower derives from `tap_step_percent` are read.
//!
//! Regenerate the golden file with:
//!
//! ```text
//! python -c "import json, pandapower as pp, pandapower.shortcircuit as sc; \
//!   net = pp.from_json('station.json'); sc.calc_sc(net, case='max', ip=False, ith=False); \
//!   json.dump({'ikss_ka': {str(i): float(net.res_bus_sc.ikss_ka[i]) for i in net.bus.index}}, \
//!   open('station.sc.json','w'))"
//! ```
//!
//! pandapower warns that it cannot compute branch powers on this network, because the
//! transformers' rated voltages are not their buses' nominal — which is the whole point of the
//! fixture. The bus fault currents it is asked for are unaffected.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use trama_format::{Importer, node_properties, parse_graph, read_sections};
use trama_power::PowerImporter;
use trama_power::network::{self, Study};

/// IEC 60909's voltage factor above 1 kV, which is every bus in this network.
const C_MAX: f64 = 1.1;

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn compiled() -> Vec<u8> {
    let import = PowerImporter.load(&networks().join("station.json"), &BTreeMap::new()).unwrap();
    trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
}

fn golden() -> Value {
    serde_json::from_str(&std::fs::read_to_string(networks().join("station.sc.json")).unwrap()).unwrap()
}

/// pandapower's index for each bus, so a result can be compared row by row.
fn positions(container: &[u8]) -> Vec<i64> {
    let graph = parse_graph(
        &read_sections(container).unwrap().into_iter().find(|section| &section.kind == b"GRPH").unwrap().payload,
    )
    .unwrap();
    let nodes = node_properties(container).unwrap();
    graph.nodes.iter().map(|node| nodes[node.property_row as usize]["power:index"].as_i64().unwrap()).collect()
}

fn currents(container: &[u8]) -> BTreeMap<i64, f64> {
    let model = network::model(container, Study::Fault { c_max: C_MAX }).unwrap_or_else(|error| panic!("{error}"));
    let computed = network::fault_currents(&model, C_MAX).unwrap();
    positions(container).into_iter().zip(computed).collect()
}

/// Every bus, including the two machine terminals where the unit correction has to be undone.
#[test]
fn every_fault_current_matches_pandapower() {
    let container = compiled();
    let reference = golden();
    let computed = currents(&container);

    let mut worst = 0.0f64;
    for (index, current) in &computed {
        let expected = reference["ikss_ka"][index.to_string()].as_f64().unwrap();
        worst = worst.max((current - expected).abs() / expected);
    }

    assert_eq!(computed.len(), 5, "two units, one ordinary machine, and the two buses above them");
    assert!(worst < 1e-9, "worst fault current difference {worst:e} relative");
}

/// A unit is two things corrected as one, and both halves have to carry it. The model says so
/// directly: two units, each naming the machine's bus and its transformer's branch.
#[test]
fn a_unit_is_the_machine_and_its_transformer() {
    let container = compiled();
    let model = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap();

    assert_eq!(model.units.len(), 2, "machine C is not a unit");
    for unit in &model.units {
        // The pair factor and the machine's own differ, or there would be nothing to undo.
        assert_ne!(unit.whole, unit.inside);
        // A machine rated above its bus, which both of these are: its terminals sit at 10.5 kV on
        // a 10 kV bus, and the fault current there is referred to the former.
        assert_eq!(unit.terminal_kv, 10.5);
    }

    // And a load flow has no units at all: §3.7 has nothing to say about a network under load.
    let flow = network::model(&container, Study::Flow { scaling: 1.0 }).unwrap();
    assert!(flow.units.is_empty());
}

/// The correction that is undone inside the unit is worth having: a fault on the machine's own
/// terminals has the transformer behind it, so applying the pair factor there would be correcting
/// for an impedance that is not in the path. Measured rather than asserted about, because "it
/// changes" is the claim and its size is what says the extra Thévenin sweep earns its place.
#[test]
fn the_machine_terminals_are_not_the_network_outside() {
    let container = compiled();
    let model = network::model(&container, Study::Fault { c_max: C_MAX }).unwrap();
    let proper = network::fault_currents(&model, C_MAX).unwrap();

    // The same network with the units left alone, which is one sweep and the wrong answer at two
    // of its five buses.
    let flattened = network::Model { units: Vec::new(), ..model };
    let uniform = network::fault_currents(&flattened, C_MAX).unwrap();

    let moved: Vec<f64> =
        proper.iter().zip(&uniform).map(|(right, wrong)| (right - wrong).abs() / right * 100.0).collect();
    let touched = moved.iter().filter(|percent| **percent > 1e-9).count();

    assert_eq!(touched, 2, "one machine terminal per unit, and nowhere else: {moved:?}");
    assert!(moved.iter().cloned().fold(0.0, f64::max) > 1.0, "worth more than a rounding: {moved:?}");
}
