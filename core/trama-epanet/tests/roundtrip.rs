// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! `.inp` -> `.trama` -> `.inp`, verified the way SPEC 9 defines it: by simulation.
//!
//! Byte equality is not the criterion and could not be met — comments and field spacing are
//! not information about the network. What must survive is every node pressure and link flow
//! at every reported timestep.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trama_epanet::{exporter::export_inp, importer::EpanetImporter, solver};
use trama_format::Importer;

/// Net1 and Net3 place their nodes on a small unnamed grid. Read as metres they make a network
/// about 80 m across; read as degrees the same numbers would stretch each pipe over hundreds
/// of kilometres and across hundreds of tiles, which is a fine thing to test but a poor default.
const CRS: &str = "EPSG:3857";

fn networks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("networks")
}

fn compile(name: &str) -> Vec<u8> {
    let options: BTreeMap<String, String> = [("source-crs".to_string(), CRS.to_string())].into_iter().collect();
    let imported = EpanetImporter.load(&networks().join(name), &options).unwrap();
    trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap()
}

/// Every reported value, keyed by entity name, channel name and simulation time. Names on both
/// axes, because recompiling the rebuilt `.inp` may hand the same channel a different id.
fn results(source: &Path) -> BTreeMap<(String, String, u32), f32> {
    let container = compile_from(source);
    let (nodes, links) = solver::entity_ids(&container).unwrap();
    let mut names: BTreeMap<u64, String> = nodes.iter().map(|(name, id)| (*id, name.clone())).collect();
    names.extend(links.iter().map(|(name, id)| (*id, name.clone())));
    let channel_names: BTreeMap<u16, String> =
        trama_solver::channels(&container).unwrap().into_iter().map(|channel| (channel.id, channel.name)).collect();
    let deltas = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 86400.0).unwrap();
    deltas
        .chunks(18)
        .map(|record| {
            let id = u64::from_le_bytes(record[0..8].try_into().unwrap());
            let channel = u16::from_le_bytes(record[8..10].try_into().unwrap());
            let t = f32::from_le_bytes(record[10..14].try_into().unwrap());
            let value = f32::from_le_bytes(record[14..18].try_into().unwrap());
            ((names[&id].clone(), channel_names[&channel].clone(), t as u32), value)
        })
        .collect()
}

fn compile_from(source: &Path) -> Vec<u8> {
    let options: BTreeMap<String, String> = [("source-crs".to_string(), CRS.to_string())].into_iter().collect();
    let imported = EpanetImporter.load(source, &options).unwrap();
    trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap()
}

#[test]
fn the_rebuilt_network_simulates_identically() {
    for name in ["Net1.inp", "Net3.inp"] {
        let container = compile(name);
        let rebuilt = std::env::temp_dir().join(format!("trama-rebuilt-{name}"));
        std::fs::write(&rebuilt, export_inp(&container, CRS).unwrap()).unwrap();

        let expected = results(&networks().join(name));
        let actual = results(&rebuilt);

        assert_eq!(actual.len(), expected.len(), "{name} produced a different number of samples");
        let worst = expected.iter().map(|(key, value)| (actual[key] - value).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "{name} drifted by {worst}");
    }
}

#[test]
fn the_container_carries_one_opaque_record_and_the_core_still_validates() {
    let container = compile("Net3.inp");

    let sections = trama_format::read_sections(&container).unwrap();
    let extras: Vec<&trama_format::Section> = sections.iter().filter(|s| &s.kind == b"XTRA").collect();
    assert_eq!(extras.len(), 1);
    let text = String::from_utf8_lossy(&extras[0].payload);
    // What the core cannot type went in; what it can type stayed out.
    assert!(text.contains("[PATTERNS]"));
    assert!(!text.contains("[JUNCTIONS]") && !text.contains("[COORDINATES]"));
}

#[test]
fn an_inp_without_a_declared_crs_is_refused() {
    let outcome = EpanetImporter.load(&networks().join("Net1.inp"), &BTreeMap::new());

    let Err(error) = outcome else { panic!("an .inp with no CRS was accepted") };
    assert!(error.contains("coordinate reference system"), "{error}");
}

#[test]
fn a_channel_the_container_never_declared_is_refused() {
    let container = compile("Net1.inp");

    let error = solver::solve(&container, "head", "flow", "age", &[], 0.0, 3600.0).unwrap_err();

    assert!(error.contains("no node channel named 'head'"), "{error}");
}

#[test]
fn a_container_from_another_format_is_refused() {
    let features: Vec<serde_json::Value> = vec![serde_json::json!({
        "type": "Feature",
        "id": "a",
        "properties": {},
        "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.703, 40.417]]},
    })];
    let channels = vec![
        serde_json::json!({"name": "pressure", "entity_kind": "node", "unit": "m"}),
        serde_json::json!({"name": "flow", "entity_kind": "edge", "unit": "l/s"}),
    ];
    let container = trama_format::compile(&features, &channels, &[]).unwrap();

    let error = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 3600.0).unwrap_err();

    assert!(error.contains("not compiled from an EPANET network"), "{error}");
}

#[test]
fn water_age_grows_from_zero_toward_the_travel_time() {
    let container = compile("Net3.inp");
    let channels = trama_solver::channels(&container).unwrap();
    let age = channels.iter().find(|channel| channel.name == "age").expect("the importer declares age");
    assert_eq!(age.entity_kind, 1, "age is a node channel");

    let deltas = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 86400.0).unwrap();
    let ages = samples(&deltas, age.id);
    assert!(!ages.is_empty(), "an age channel was declared and nothing was written into it");

    // The physics this exists to show: water starts fresh and gets older as the day runs.
    let at_start: f32 = ages.iter().filter(|(t, _)| *t == 0.0).map(|(_, v)| v).sum();
    let early: f32 = ages.iter().filter(|(t, _)| *t <= 7200.0).map(|(_, v)| v).sum::<f32>()
        / ages.iter().filter(|(t, _)| *t <= 7200.0).count() as f32;
    let late: f32 = ages.iter().filter(|(t, _)| *t >= 79200.0).map(|(_, v)| v).sum::<f32>()
        / ages.iter().filter(|(t, _)| *t >= 79200.0).count() as f32;
    assert_eq!(at_start, 0.0, "at t=0 no water has aged yet");
    assert!(late > early + 1.0, "mean age must grow over the day: {early:.2}h early vs {late:.2}h late");
    // And it is hours, not seconds: a day of simulation cannot age water more than a day.
    let worst = ages.iter().map(|(_, v)| *v).fold(0.0f32, f32::max);
    assert!(worst <= 24.0 + 1.0, "age is declared in hours; {worst} looks like another unit");
}

#[test]
fn a_container_without_the_age_channel_still_solves() {
    // Containers compiled before the quality channels existed declare pressure and flow only.
    // Each is an offer, not a demand: the solver writes where declared, stays quiet elsewhere.
    let options: BTreeMap<String, String> = [("source-crs".to_string(), CRS.to_string())].into_iter().collect();
    let imported = EpanetImporter.load(&networks().join("Net1.inp"), &options).unwrap();
    let old_channels: Vec<serde_json::Value> = imported
        .channels
        .iter()
        .filter(|channel| {
            let name = channel["name"].as_str().unwrap_or_default();
            name != "age" && !name.starts_with("chem:") && !name.starts_with("trace:")
        })
        .cloned()
        .collect();
    let container = trama_format::compile(&imported.features, &old_channels, &imported.extras).unwrap();

    let deltas = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 86400.0).unwrap();

    assert!(!deltas.is_empty());
    let channels = trama_solver::channels(&container).unwrap();
    assert!(channels.iter().all(|channel| channel.name != "age"));
}

#[test]
fn closing_a_pipe_changes_the_physics_and_an_unknown_id_is_refused() {
    let container = compile("Net1.inp");
    let (_nodes, links) = solver::entity_ids(&container).unwrap();

    let open_run = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 3600.0).unwrap();
    // Net1's pipe 10 is the single link between the pump and the rest of the network.
    let closed_id = links["10"];
    let closed_run = solver::solve(&container, "pressure", "flow", "age", &[closed_id], 0.0, 3600.0).unwrap();

    assert_eq!(open_run.len(), closed_run.len(), "same entities report on the same schedule");
    let values = |deltas: &[u8]| -> Vec<f32> {
        deltas.chunks(18).map(|r| f32::from_le_bytes(r[14..18].try_into().unwrap())).collect()
    };
    let moved = values(&open_run)
        .iter()
        .zip(values(&closed_run))
        .filter(|(open, closed)| (**open - closed).abs() > 0.01)
        .count();
    assert!(moved > 10, "closing the network's main feed changed {moved} values; the closure did nothing");

    let error = solver::solve(&container, "pressure", "flow", "age", &[42], 0.0, 3600.0).err().unwrap();
    assert!(error.contains("42"), "{error}");
}

/// Every `(t, value)` written into one channel.
fn samples(deltas: &[u8], channel: u16) -> Vec<(f32, f32)> {
    deltas
        .chunks(18)
        .filter(|record| u16::from_le_bytes(record[8..10].try_into().unwrap()) == channel)
        .map(|record| {
            (
                f32::from_le_bytes(record[10..14].try_into().unwrap()),
                f32::from_le_bytes(record[14..18].try_into().unwrap()),
            )
        })
        .collect()
}

#[test]
fn the_files_own_chemical_is_simulated_under_its_own_name() {
    let container = compile("Net1.inp");
    let channels = trama_solver::channels(&container).unwrap();
    // Net1's [OPTIONS] says `Quality Chlorine mg/L`: the channel is named by the file, and the
    // unit is the file's own rather than anything this crate chose.
    let chlorine = channels.iter().find(|channel| channel.name == "chem:chlorine").expect("Net1 declares its chemical");
    assert_eq!((chlorine.entity_kind, chlorine.unit.as_str()), (1, "mg/L"));

    let deltas = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 86400.0).unwrap();
    let concentrations = samples(&deltas, chlorine.id);
    assert!(!concentrations.is_empty(), "a declared chemical channel was never written");

    // The physics this exists to show: chlorine enters at 1 mg/L from the source and decays as
    // it travels, under the [REACTIONS] coefficients the file carried through XTRA.
    let peak = concentrations.iter().map(|(_, value)| *value).fold(f32::MIN, f32::max);
    let floor = concentrations.iter().map(|(_, value)| *value).fold(f32::MAX, f32::min);
    assert!(floor >= 0.0, "a concentration went negative: {floor}");
    assert!(peak > 0.8, "the source injects 1 mg/L and no node came near it: peak {peak}");
    assert!(floor < 0.4, "decay should leave the far end well below the source: floor {floor}");
}

#[test]
fn every_reservoir_gets_a_tracing_channel_and_the_shares_stay_shares() {
    let container = compile("Net3.inp");
    let channels = trama_solver::channels(&container).unwrap();
    let lake = channels.iter().find(|channel| channel.name == "trace:Lake").expect("Net3's Lake is offered");
    let river = channels.iter().find(|channel| channel.name == "trace:River").expect("Net3's River is offered");
    assert_eq!((lake.unit.as_str(), river.unit.as_str()), ("%", "%"));

    let deltas = solver::solve(&container, "pressure", "flow", "age", &[], 0.0, 86400.0).unwrap();
    // Shares are per (node, time): each within 0..100, and the two sources together can never
    // account for more than all of a node's water.
    let mut combined: BTreeMap<(u64, u32), f32> = BTreeMap::new();
    for record in deltas.chunks(18) {
        let channel = u16::from_le_bytes(record[8..10].try_into().unwrap());
        if channel != lake.id && channel != river.id {
            continue;
        }
        let entity = u64::from_le_bytes(record[0..8].try_into().unwrap());
        let t = f32::from_le_bytes(record[10..14].try_into().unwrap()) as u32;
        let value = f32::from_le_bytes(record[14..18].try_into().unwrap());
        assert!((0.0..=100.0).contains(&value), "a share of {value}% is not a share");
        *combined.entry((entity, t)).or_default() += value;
    }
    assert!(!combined.is_empty(), "declared tracing channels were never written");
    let fullest = combined.values().fold(0.0f32, |worst, total| worst.max(*total));
    assert!(fullest <= 100.5, "Lake and River together supplied {fullest}% of some node's water");
    assert!(fullest > 90.0, "some node should drink almost entirely from the two sources; the best was {fullest}%");
}
