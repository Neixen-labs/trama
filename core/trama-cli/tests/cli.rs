// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The command line, exercised as a process rather than as a function call.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("trama-cli-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn trama(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trama")).args(arguments).output().unwrap()
}

#[test]
fn compiles_geojson_into_the_container_the_fixture_holds() {
    let out = workspace("geojson").join("network.trama");

    let result =
        trama(&["compile", repository().join("fixtures/network.geojson").to_str().unwrap(), out.to_str().unwrap()]);

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(repository().join("fixtures/network.trama")).unwrap());
}

#[test]
fn compiles_an_inp_through_the_importer_that_claims_it() {
    let out = workspace("inp").join("net3.trama");

    let result = trama(&[
        "compile",
        repository().join("core/trama-epanet/tests/networks/Net3.inp").to_str().unwrap(),
        out.to_str().unwrap(),
        "-o",
        "source-crs=EPSG:3857",
    ]);

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(repository().join("fixtures/net3.trama")).unwrap());
}

#[test]
fn an_inp_without_its_crs_fails_with_a_message_naming_what_is_missing() {
    let result = trama(&[
        "compile",
        repository().join("core/trama-epanet/tests/networks/Net1.inp").to_str().unwrap(),
        workspace("nocrs").join("out.trama").to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("coordinate reference system"));
}

#[test]
fn an_unclaimed_suffix_says_what_is_missing() {
    let source = workspace("unclaimed").join("network.dwg");
    std::fs::write(&source, "not a network").unwrap();

    let result = trama(&["compile", source.to_str().unwrap(), "/dev/null"]);

    let message = String::from_utf8_lossy(&result.stderr).to_string();
    assert!(message.contains(".dwg") && message.contains("importer"), "{message}");
}

#[test]
fn validate_accepts_a_container_and_rejects_a_corrupted_one() {
    let directory = workspace("validate");
    let good = repository().join("fixtures/network.trama");
    assert!(trama(&["validate", good.to_str().unwrap()]).status.success());

    let mut bytes = std::fs::read(&good).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    let bad = directory.join("bad.trama");
    std::fs::write(&bad, bytes).unwrap();

    assert!(!trama(&["validate", bad.to_str().unwrap()]).status.success());
}

#[test]
fn exports_geojson_and_an_inp_the_way_it_was_imported() {
    let directory = workspace("export");
    let container = repository().join("fixtures/net3.trama");

    assert!(
        trama(&["export", container.to_str().unwrap(), directory.to_str().unwrap(), "--to", "geojson"])
            .status
            .success()
    );
    assert!(directory.join("nodes.geojson").exists() && directory.join("edges.geojson").exists());

    let rebuilt = directory.join("net3.inp");
    let result = trama(&[
        "export",
        container.to_str().unwrap(),
        rebuilt.to_str().unwrap(),
        "--to",
        "inp",
        "-o",
        "crs=EPSG:3857",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(std::fs::read_to_string(&rebuilt).unwrap().contains("[JUNCTIONS]"));
}

#[test]
fn an_inp_export_without_a_crs_says_so_rather_than_guessing() {
    let result = trama(&[
        "export",
        repository().join("fixtures/net3.trama").to_str().unwrap(),
        workspace("nocrs-export").join("out.inp").to_str().unwrap(),
        "--to",
        "inp",
    ]);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("coordinate reference system"));
}

#[test]
fn an_importer_declares_the_channels_its_format_implies() {
    let out = workspace("channels").join("net3.trama");
    trama(&[
        "compile",
        repository().join("core/trama-epanet/tests/networks/Net3.inp").to_str().unwrap(),
        out.to_str().unwrap(),
        "-o",
        "source-crs=EPSG:3857",
    ]);

    let container = std::fs::read(&out).unwrap();
    let names: Vec<String> = trama_format_channels(&container);
    assert_eq!(names, ["pressure", "flow"]);
}

fn trama_format_channels(container: &[u8]) -> Vec<String> {
    let sections = trama_format::read_sections(container).unwrap();
    let payload = &sections.iter().find(|s| &s.kind == b"STCH").unwrap().payload;
    let at = |offset: usize| u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
    let count = at(0);
    let strings_offset = at(4);
    let mut strings = Vec::new();
    let mut cursor = strings_offset + 4;
    for _ in 0..at(strings_offset) {
        let length = at(cursor);
        strings.push(String::from_utf8_lossy(&payload[cursor + 4..cursor + 4 + length]).into_owned());
        cursor += 4 + length;
    }
    (0..count).map(|index| strings[index * 2].clone()).collect()
}

#[test]
fn an_importer_asked_for_by_name_wins_over_the_suffix() {
    // An OpenStreetMap extract is `.json`, which the compiler already claims. Without the flag
    // it compiles as plain GeoJSON and every one-way street quietly becomes two-way.
    let directory = workspace("roads");
    let source = directory.join("extract.json");
    std::fs::write(
        &source,
        r#"{"elements":[{"type":"way","id":1,"nodes":[10,11],
           "geometry":[{"lat":40.416,"lon":-3.704},{"lat":40.417,"lon":-3.703}],
           "tags":{"highway":"residential","oneway":"yes"}}]}"#,
    )
    .unwrap();
    let out = directory.join("roads.trama");

    let result = trama(&["compile", source.to_str().unwrap(), out.to_str().unwrap(), "--importer", "roads"]);

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let graph = trama_format::parse_graph(
        &trama_format::read_sections(&std::fs::read(&out).unwrap())
            .unwrap()
            .iter()
            .find(|section| &section.kind == b"GRPH")
            .unwrap()
            .payload,
    )
    .unwrap();
    assert!(graph.edges.iter().all(|edge| edge.directed), "the road importer read the one-way tag");
}

#[test]
fn the_same_file_without_the_flag_takes_the_plain_geojson_path() {
    let directory = workspace("roads-unflagged");
    let source = directory.join("extract.json");
    std::fs::write(&source, r#"{"type":"FeatureCollection","features":[]}"#).unwrap();

    let result = trama(&["compile", source.to_str().unwrap(), directory.join("o.trama").to_str().unwrap()]);

    // It fails as GeoJSON rather than being handed to a road importer, which is the point:
    // nothing changed for a file that was compiling before.
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("out geom"));
}

#[test]
fn an_unknown_importer_name_lists_the_installed_ones() {
    let directory = workspace("roads-unknown");
    let source = directory.join("extract.json");
    std::fs::write(&source, "{}").unwrap();

    let result =
        trama(&["compile", source.to_str().unwrap(), directory.join("o.trama").to_str().unwrap(), "--importer", "osm"]);

    let message = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(message.contains("no installed importer is named 'osm'"), "{message}");
    assert!(message.contains("roads"), "the message names what is available: {message}");
}
